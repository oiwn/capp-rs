use crate::{Task, TaskId, TaskQueue, TaskQueueError, TaskSerializer};
use async_trait::async_trait;
use mongodb::{
    bson::{self, doc},
    // options::ClientOptions,
    Client,
    Collection,
};
use serde::{de::DeserializeOwned, Serialize};
use std::marker::PhantomData;

pub struct MongoTaskQueue<D, S>
where
    S: TaskSerializer,
{
    pub client: Client,
    pub tasks_collection: Collection<bson::Document>,
    pub dlq_collection: Collection<bson::Document>,
    _marker: PhantomData<(D, S)>,
}

impl<D, S> MongoTaskQueue<D, S>
where
    D: Send + Sync + 'static,
    S: TaskSerializer + Send + Sync,
{
    /* pub async fn new(
        connection_string: &str,
        queue_name: &str,
    ) -> Result<Self, TaskQueueError> {
        let client_options = ClientOptions::parse(connection_string).await?;
        let client = Client::with_options(client_options.clone())?;

        let db_name = client_options
            .default_database
            .as_ref()
            .expect("No database specified in MongoDB URI");

        let db = client.database(db_name);

        // Collections store raw BSON documents now
        let tasks_collection = db.collection(&format!("{}_tasks", queue_name));
        let dlq_collection = db.collection(&format!("{}_dlq", queue_name));

        // No need for task_id index since we use MongoDB's _id

        Ok(Self {
            client,
            tasks_collection,
            dlq_collection,
            _marker: PhantomData,
        })
    } */

    pub async fn new(
        database: mongodb::Database,
        queue_name: &str,
    ) -> Result<Self, TaskQueueError> {
        // Collections store raw BSON documents now
        let tasks_collection =
            database.collection(&format!("{}_tasks", queue_name));
        let dlq_collection = database.collection(&format!("{}_dlq", queue_name));

        Ok(Self {
            client: database.client().clone(),
            tasks_collection,
            dlq_collection,
            _marker: PhantomData,
        })
    }
}

#[async_trait]
impl<D, S> TaskQueue<D> for MongoTaskQueue<D, S>
where
    D: std::fmt::Debug
        + Clone
        + Serialize
        + DeserializeOwned
        + Send
        + Sync
        + 'static,
    S: TaskSerializer + Send + Sync,
{
    async fn push(&self, task: &Task<D>) -> Result<(), TaskQueueError> {
        // Serialize task to BSON bytes
        let bytes = S::serialize_task(task)?;

        // Convert bytes to BSON Document
        let doc = bson::from_slice::<bson::Document>(&bytes)
            .map_err(|e| TaskQueueError::Serialization(e.to_string()))?;

        // Insert document into collection
        self.tasks_collection.insert_one(doc).await?;
        Ok(())
    }

    async fn pop(&self) -> Result<Task<D>, TaskQueueError> {
        // Find task with Queued status
        match self
            .tasks_collection
            .find_one(doc! { "status": "Queued" })
            .await?
        {
            Some(doc) => {
                // Convert to Task
                let bytes = bson::to_vec(&doc)
                    .map_err(|e| TaskQueueError::Serialization(e.to_string()))?;

                let mut task: Task<D> = S::deserialize_task(&bytes)?;

                // Update status
                task.set_in_progress();

                // Save updated task
                self.set(&task).await?;

                Ok(task)
            }
            None => Err(TaskQueueError::QueueEmpty),
        }
    }

    async fn ack(&self, task_id: &TaskId) -> Result<(), TaskQueueError> {
        let result = self
            .tasks_collection
            .delete_one(doc! { "_id": task_id.get().to_string() })
            .await?;

        if result.deleted_count == 0 {
            return Err(TaskQueueError::TaskNotFound(*task_id));
        }
        Ok(())
    }

    async fn nack(&self, task: &Task<D>) -> Result<(), TaskQueueError> {
        let mut task_clone = task.clone();
        task_clone.set_dlq("Task moved to DLQ");

        // First insert to DLQ
        let bytes = S::serialize_task(&task_clone)?;
        let doc = bson::from_slice::<bson::Document>(&bytes)
            .map_err(|e| TaskQueueError::Serialization(e.to_string()))?;

        self.dlq_collection.insert_one(doc).await?;

        // Then remove from main queue
        self.tasks_collection
            .delete_one(doc! { "_id": task.task_id.get().to_string() })
            .await?;

        Ok(())
    }

    async fn set(&self, task: &Task<D>) -> Result<(), TaskQueueError> {
        // Serialize task to BSON document
        let bytes = S::serialize_task(task)?;
        let doc = bson::from_slice::<bson::Document>(&bytes)
            .map_err(|e| TaskQueueError::Serialization(e.to_string()))?;

        // Update using _id
        let result = self
            .tasks_collection
            .replace_one(doc! { "_id": task.task_id.get().to_string() }, doc)
            .await?;

        if result.matched_count == 0 {
            return Err(TaskQueueError::TaskNotFound(task.task_id));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
pub struct BsonSerializer;

impl TaskSerializer for BsonSerializer {
    fn serialize_task<T>(task: &Task<T>) -> Result<Vec<u8>, TaskQueueError>
    where
        T: Serialize + DeserializeOwned + Clone,
    {
        // Convert to BSON document first
        let mut doc = bson::to_document(task)
            .map_err(|e| TaskQueueError::Serialization(e.to_string()))?;

        // Move task_id to _id
        if let Some(task_id) = doc.remove("task_id") {
            doc.insert("_id", task_id);
        }

        // Serialize complete BSON document to bytes
        bson::to_vec(&doc).map_err(|e| TaskQueueError::Serialization(e.to_string()))
    }

    fn deserialize_task<T>(data: &[u8]) -> Result<Task<T>, TaskQueueError>
    where
        T: Serialize + DeserializeOwned + Clone,
    {
        // First get BSON document from bytes
        let mut doc: bson::Document = bson::from_slice(data)
            .map_err(|e| TaskQueueError::Deserialization(e.to_string()))?;

        if let Some(id) = doc.remove("_id") {
            doc.insert("task_id", id);
        }

        // Convert complete document back to Task
        bson::from_document(doc)
            .map_err(|e| TaskQueueError::Deserialization(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::Task;
    use mongodb::bson;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    struct TestData {
        value: u32,
    }

    #[test]
    fn test_bson_serializer_roundtrip() {
        let task = Task::new(TestData { value: 42 });
        let original_id = task.task_id;

        // Serialize
        let bytes =
            BsonSerializer::serialize_task(&task).expect("Failed to serialize");

        // Deserialize
        let recovered_task: Task<TestData> =
            BsonSerializer::deserialize_task(&bytes)
                .expect("Failed to deserialize");

        // Verify data
        assert_eq!(recovered_task.task_id, original_id);
        assert_eq!(recovered_task.payload, task.payload);
        assert_eq!(recovered_task.status, task.status);
    }

    #[test]
    fn test_bson_document_structure() {
        let task = Task::new(TestData { value: 42 });

        // Serialize to bytes
        let bytes =
            BsonSerializer::serialize_task(&task).expect("Failed to serialize");

        // Convert bytes back to BSON document to inspect structure
        let doc = bson::from_slice::<bson::Document>(&bytes)
            .expect("Failed to convert to BSON");

        // Verify _id exists and task_id doesn't
        assert!(doc.contains_key("_id"), "Document should contain _id");
        assert!(
            !doc.contains_key("task_id"),
            "Document should not contain task_id"
        );
    }

    #[test]
    fn test_invalid_bson() {
        // Try to deserialize invalid BSON
        let invalid_bytes = vec![1, 2, 3, 4];
        let result: Result<Task<TestData>, _> =
            BsonSerializer::deserialize_task(&invalid_bytes);

        assert!(result.is_err());
    }
}
