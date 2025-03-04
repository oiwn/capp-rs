use crate::serializers::MongoCompatible;
use crate::{Task, TaskId, TaskQueue, TaskQueueError, TaskSerializer, TaskStatus};
use async_trait::async_trait;
use bson::doc;
use chrono::{DateTime, Utc};
use mongodb::{
    Client, Collection, IndexModel,
    options::{FindOneAndUpdateOptions, IndexOptions, ReturnDocument},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::marker::PhantomData;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MongoTask<D: Clone> {
    #[serde(rename = "_id")]
    pub task_id: String,
    pub payload: D,
    pub status: TaskStatus,
    #[serde(with = "bson::serde_helpers::chrono_datetime_as_bson_datetime")]
    pub queued_at: DateTime<Utc>,
    #[serde(
        serialize_with = "mongo_datetime::serialize",
        deserialize_with = "mongo_datetime::deserialize"
    )]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(
        serialize_with = "mongo_datetime::serialize",
        deserialize_with = "mongo_datetime::deserialize"
    )]
    pub finished_at: Option<DateTime<Utc>>,
    pub retries: u32,
    pub error_msg: Option<String>,
}

impl<D: Clone> From<Task<D>> for MongoTask<D> {
    fn from(task: Task<D>) -> Self {
        Self {
            task_id: task.task_id.get().to_string(),
            payload: task.payload,
            status: task.status,
            queued_at: task.queued_at.into(),
            started_at: task.started_at.map(|t| t.into()),
            finished_at: task.finished_at.map(|t| t.into()),
            retries: task.retries,
            error_msg: task.error_msg,
        }
    }
}

impl<D: Clone> From<MongoTask<D>> for Task<D> {
    fn from(mongo_task: MongoTask<D>) -> Self {
        Self {
            task_id: TaskId::from_string(&mongo_task.task_id),
            payload: mongo_task.payload,
            status: mongo_task.status,
            queued_at: mongo_task.queued_at.into(),
            started_at: mongo_task.started_at.map(|t| t.into()),
            finished_at: mongo_task.finished_at.map(|t| t.into()),
            retries: mongo_task.retries,
            error_msg: mongo_task.error_msg,
        }
    }
}

#[derive(Clone)]
pub struct MongoTaskQueue<D, S>
where
    D: Clone + Serialize + DeserializeOwned + Send + Sync + 'static,
    S: TaskSerializer + MongoCompatible,
{
    pub client: Client,
    pub tasks_collection: Collection<MongoTask<D>>,
    pub dlq_collection: Collection<MongoTask<D>>,
    _marker: PhantomData<(D, S)>,
}

// In serializers.rs or mongodb.rs, add a conversion trait
impl From<TaskStatus> for String {
    fn from(status: TaskStatus) -> Self {
        match status {
            TaskStatus::Queued => "Queued".to_string(),
            TaskStatus::InProgress => "InProgress".to_string(),
            TaskStatus::Completed => "Completed".to_string(),
            TaskStatus::Failed => "Failed".to_string(),
            TaskStatus::DeadLetter => "DeadLetter".to_string(),
        }
    }
}

impl<D, S> MongoTaskQueue<D, S>
where
    D: Clone + Serialize + DeserializeOwned + Send + Sync + 'static,
    S: TaskSerializer + MongoCompatible + Send + Sync,
{
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

    pub async fn setup_indexes(&self) -> Result<(), TaskQueueError> {
        let mut indexes = vec![];

        // 1. Primary key index on _id (already exists by default, but we'll ensure it's unique)
        let id_index = IndexModel::builder()
            .keys(doc! { "_id": 1 })
            .options(IndexOptions::builder().unique(true).build())
            .build();
        indexes.push(id_index);

        // 2. Compound index for pop operation (status + queued_at for FIFO ordering)
        let status_queued_index = IndexModel::builder()
            .keys(doc! { "status": 1, "queued_at": 1 })
            .build();
        indexes.push(status_queued_index);

        // 3. Index for finding tasks by status only
        let status_index = IndexModel::builder().keys(doc! { "status": 1 }).build();
        indexes.push(status_index);

        // Create indexes for tasks collection
        self.tasks_collection
            .create_indexes(indexes.clone())
            .await?;

        // Create same indexes for DLQ collection
        self.dlq_collection.create_indexes(indexes).await?;

        tracing::info!("MongoDB indexes created successfully for task queue");
        Ok(())
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
    S: TaskSerializer + MongoCompatible + Send + Sync,
{
    async fn push(&self, task: &Task<D>) -> Result<(), TaskQueueError> {
        let mongo_task = MongoTask::from(task.clone());
        let doc = bson::to_document(&mongo_task).unwrap();

        self.tasks_collection
            .update_one(
                doc! { "_id": task.task_id.get().to_string() },
                doc! { "$set": doc },
            )
            .upsert(true)
            .await
            .map_err(TaskQueueError::from)?;

        Ok(())
    }

    async fn pop(&self) -> Result<Task<D>, TaskQueueError> {
        let filter = doc! { "status": String::from(TaskStatus::Queued) };
        let update = doc! {
            "$set": {
                "status": String::from(TaskStatus::InProgress),
                "started_at": bson::DateTime::now(),
            }
        };

        let options = FindOneAndUpdateOptions::builder()
            .return_document(Some(ReturnDocument::After))
            .build();

        // Use find_one_and_update to atomically find and update a task
        let result = self
            .tasks_collection
            .find_one_and_update(filter, update)
            .with_options(options)
            .sort(doc! { "queued_at": 1 }) // Ensure FIFO
            .await?;

        match result {
            Some(doc) => {
                let task: Task<D> = doc.into();
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

        // Convert to MongoTask explicitly
        let mongo_task = MongoTask::from(task_clone);

        // Insert into DLQ
        self.dlq_collection
            .insert_one(mongo_task)
            .await
            .map_err(TaskQueueError::from)?;

        // Then remove from main queue
        self.tasks_collection
            .delete_one(doc! { "_id": task.task_id.get().to_string() })
            .await?;

        Ok(())
    }

    async fn set(&self, task: &Task<D>) -> Result<(), TaskQueueError> {
        let mongo_task = MongoTask::from(task.clone());
        let task_id = mongo_task.task_id.to_string();

        // Update using _id
        let result = self
            .tasks_collection
            .replace_one(doc! { "_id": task_id }, mongo_task)
            .await?;

        if result.matched_count == 0 {
            return Err(TaskQueueError::TaskNotFound(task.task_id));
        }
        Ok(())
    }
}

mod mongo_datetime {
    use bson;
    use chrono::{DateTime, Utc};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(
        datetime: &Option<DateTime<Utc>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match datetime {
            Some(dt) => {
                bson::serde_helpers::chrono_datetime_as_bson_datetime::serialize(
                    dt, serializer,
                )
            }
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(
        deserializer: D,
    ) -> Result<Option<DateTime<Utc>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        // Use the existing BSON datetime deserializer, but wrap in Option
        Option::deserialize(deserializer).and_then(
            |opt_dt: Option<bson::DateTime>| Ok(opt_dt.map(|dt| dt.to_chrono())),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BsonSerializer;
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
