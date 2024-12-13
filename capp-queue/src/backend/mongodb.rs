use async_trait::async_trait;
use mongodb::{
    bson::{doc, Document},
    options::{ClientOptions, FindOneAndDeleteOptions},
    Client, Collection,
};
use serde::{de::DeserializeOwned, Serialize};
use std::marker::PhantomData;

use crate::queue::{TaskQueue, TaskQueueError};
use crate::task::{Task, TaskId};

pub struct MongoTaskQueue<D> {
    pub client: Client,
    pub tasks_collection: Collection<Document>,
    pub dlq_collection: Collection<Document>,
    _marker: PhantomData<D>,
}

impl<D> MongoTaskQueue<D> {
    pub async fn new(
        connection_string: &str,
        queue_name: &str,
    ) -> Result<Self, TaskQueueError> {
        let client_options = ClientOptions::parse(connection_string)
            .await
            .map_err(|e| TaskQueueError::QueueError(e.to_string()))?;

        let client = Client::with_options(client_options)
            .map_err(|e| TaskQueueError::QueueError(e.to_string()))?;

        let db = client.database("task_queue");
        let tasks_collection = db.collection(&format!("{}_tasks", queue_name));
        let dlq_collection = db.collection(&format!("{}_dlq", queue_name));

        // Create indexes
        tasks_collection
            .create_index(doc! { "task_id": 1 }, None)
            .await
            .map_err(|e| TaskQueueError::QueueError(e.to_string()))?;

        Ok(Self {
            client,
            tasks_collection,
            dlq_collection,
            _marker: PhantomData,
        })
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
        let task_doc = mongodb::bson::to_document(&task)
            .map_err(|e| TaskQueueError::SerdeError(e.to_string()))?;

        self.tasks_collection
            .insert_one(task_doc, None)
            .await
            .map_err(|e| TaskQueueError::QueueError(e.to_string()))?;

        Ok(())
    }

    async fn pop(&self) -> Result<Task<D>, TaskQueueError> {
        let options = FindOneAndDeleteOptions::default();

        let result = self
            .tasks_collection
            .find_one_and_delete(doc! {}, options)
            .await
            .map_err(|e| TaskQueueError::QueueError(e.to_string()))?;

        match result {
            Some(doc) => {
                let task: Task<D> = mongodb::bson::from_document(doc)
                    .map_err(|e| TaskQueueError::SerdeError(e.to_string()))?;
                Ok(task)
            }
            None => Err(TaskQueueError::QueueEmpty),
        }
    }

    async fn ack(&self, task_id: &TaskId) -> Result<(), TaskQueueError> {
        let result = self
            .tasks_collection
            .delete_one(doc! { "task_id": task_id.to_string() }, None)
            .await
            .map_err(|e| TaskQueueError::QueueError(e.to_string()))?;

        if result.deleted_count == 0 {
            return Err(TaskQueueError::TaskNotFound(*task_id));
        }

        Ok(())
    }

    async fn nack(&self, task: &Task<D>) -> Result<(), TaskQueueError> {
        let task_doc = mongodb::bson::to_document(&task)
            .map_err(|e| TaskQueueError::SerdeError(e.to_string()))?;

        // Start session for transaction
        let mut session = self
            .client
            .start_session(None)
            .await
            .map_err(|e| TaskQueueError::QueueError(e.to_string()))?;

        session
            .start_transaction(None)
            .await
            .map_err(|e| TaskQueueError::QueueError(e.to_string()))?;

        // Move to DLQ and remove from main queue
        self.dlq_collection
            .insert_one_with_session(task_doc, None, &mut session)
            .await
            .map_err(|e| TaskQueueError::QueueError(e.to_string()))?;

        self.tasks_collection
            .delete_one_with_session(
                doc! { "task_id": task.task_id.to_string() },
                None,
                &mut session,
            )
            .await
            .map_err(|e| TaskQueueError::QueueError(e.to_string()))?;

        session
            .commit_transaction()
            .await
            .map_err(|e| TaskQueueError::QueueError(e.to_string()))?;

        Ok(())
    }

    async fn set(&self, task: &Task<D>) -> Result<(), TaskQueueError> {
        let task_doc = mongodb::bson::to_document(&task)
            .map_err(|e| TaskQueueError::SerdeError(e.to_string()))?;

        self.tasks_collection
            .replace_one(
                doc! { "task_id": task.task_id.to_string() },
                task_doc,
                None,
            )
            .await
            .map_err(|e| TaskQueueError::QueueError(e.to_string()))?;

        Ok(())
    }
}
