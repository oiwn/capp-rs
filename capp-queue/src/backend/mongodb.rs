use async_trait::async_trait;
use mongodb::{
    bson::doc,
    error::TRANSIENT_TRANSACTION_ERROR,
    error::UNKNOWN_TRANSACTION_COMMIT_RESULT,
    options::{ClientOptions, IndexOptions},
    Client, ClientSession, Collection, IndexModel,
};
use serde::{de::DeserializeOwned, Serialize};

use crate::queue::{TaskQueue, TaskQueueError};
use crate::task::{Task, TaskId};

pub struct MongoTaskQueue<D: Clone>
where
    D: Send + Sync + 'static,
{
    pub client: Client,
    pub tasks_collection: Collection<Task<D>>,
    pub dlq_collection: Collection<Task<D>>,
}

impl<D> MongoTaskQueue<D>
where
    D: Clone + Serialize + DeserializeOwned + Send + Sync + 'static,
{
    pub async fn new(
        connection_string: &str,
        queue_name: &str,
    ) -> Result<Self, TaskQueueError> {
        let client_options = ClientOptions::parse(connection_string).await?;

        let client = Client::with_options(client_options.clone())?;

        // Get database name from URI or use default
        let db_name = client_options
            .default_database
            .as_ref()
            .expect("No database specified in MongoDB URI");

        let db = client.database(db_name);
        let tasks_collection =
            db.collection::<Task<D>>(&format!("{}_tasks", queue_name));
        let dlq_collection =
            db.collection::<Task<D>>(&format!("{}_dlq", queue_name));

        // Create index on task_id
        let index_model = IndexModel::builder()
            .keys(doc! { "task_id": 1 })
            .options(IndexOptions::builder().unique(true).build())
            .build();

        tasks_collection.create_index(index_model).await?;

        Ok(Self {
            client,
            tasks_collection,
            dlq_collection,
        })
    }
}

impl<D> MongoTaskQueue<D>
where
    D: Clone + Serialize + DeserializeOwned + Send + Sync + 'static,
{
    // Helper method to execute the nack transaction
    async fn execute_nack_transaction(
        &self,
        task: &Task<D>,
        session: &mut ClientSession,
    ) -> mongodb::error::Result<()> {
        // Move to DLQ
        self.dlq_collection
            .insert_one(task)
            .session(&mut *session)
            .await?;

        // Remove from main queue
        self.tasks_collection
            .delete_one(doc! { "task_id": task.task_id.to_string() })
            .session(&mut *session)
            .await?;

        // Commit with retry logic for unknown commit results
        loop {
            let result = session.commit_transaction().await;
            if let Err(ref error) = result {
                if error.contains_label(UNKNOWN_TRANSACTION_COMMIT_RESULT) {
                    continue;
                }
            }
            return result;
        }
    }
}

#[async_trait]
impl<D> TaskQueue<D> for MongoTaskQueue<D>
where
    D: std::fmt::Debug
        + Clone
        + Serialize
        + DeserializeOwned
        + Send
        + Sync
        + 'static,
{
    async fn push(&self, task: &Task<D>) -> Result<(), TaskQueueError> {
        self.tasks_collection.insert_one(task).await?;
        Ok(())
    }

    async fn pop(&self) -> Result<Task<D>, TaskQueueError> {
        match self.tasks_collection.find_one_and_delete(doc! {}).await? {
            Some(task) => Ok(task),
            None => Err(TaskQueueError::QueueEmpty),
        }
    }

    async fn ack(&self, task_id: &TaskId) -> Result<(), TaskQueueError> {
        let result = self
            .tasks_collection
            .delete_one(doc! { "task_id": task_id.to_string() })
            .await?;
        if result.deleted_count == 0 {
            return Err(TaskQueueError::TaskNotFound(*task_id));
        }
        Ok(())
    }
    async fn nack(&self, task: &Task<D>) -> Result<(), TaskQueueError> {
        let mut session = self.client.start_session().await?; // Convert to MongodbError

        session.start_transaction().await?; // Convert to MongodbError

        while let Err(error) =
            self.execute_nack_transaction(task, &mut session).await
        {
            if !error.contains_label(TRANSIENT_TRANSACTION_ERROR) {
                return Err(TaskQueueError::MongodbError(error));
            }
            session.start_transaction().await?; // Convert to MongodbError
        }

        Ok(())
    }

    async fn set(&self, task: &Task<D>) -> Result<(), TaskQueueError> {
        self.tasks_collection
            .replace_one(doc! { "task_id": task.task_id.to_string() }, task)
            .await?; // Convert to MongodbError

        Ok(())
    }
}
