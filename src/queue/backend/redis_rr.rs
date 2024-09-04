//! `RedisRoundRobinTaskStorage` provides an asynchronous task storage mechanism
//! built on top of Redis, with a round-robin approach to accessing tasks across
//! different queues.
//!
//! This storage structure maintains domain-specific queues, allowing for tasks
//! to be categorized and processed based on their associated key. The round-robin
//! mechanism ensures that tasks from one domain do not dominate the queue, allowing
//! for balanced task processing across all domains.
//!
//! Features:
//! - **Tag-based Queues**: Tasks are enqueued and dequeued based on their
//!   tag, preventing any single tag from monopolizing worker resources.
//! - **Round Robin Access**: The storage fetches tasks in a round-robin manner
//!   across the tags, ensuring fair access and processing for all tags.
//! - **Asynchronous Operations**: All task operations, including enqueueing,
//!   dequeueing, and acknowledging, are performed asynchronously for optimal
//!   performance.
//!
//! # Examples
//!
//! ```rust
//! // TODO: Insert basic usage example here.
//! ```
//!
//! Note: The exact tag key for each task is determined from the `TaskData`
//! field, and can be configured during the storage initialization.

use crate::queue::{HasTagKey, TaskQueue, TaskQueueError};
use crate::task::{Task, TaskId};
use async_trait::async_trait;
use rustis::{
    client::{Client, Pipeline},
    commands::{ListCommands, StringCommands},
    Result as RedisResult,
};
use serde::{de::DeserializeOwned, Serialize};
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex};

pub struct RedisRoundRobinTaskQueue<D, T>
where
    T: ToString + Eq + std::hash::Hash + Clone,
{
    client: Client,
    queue_prefix: String,
    dlq_key: String,
    round_robin: Arc<Mutex<HashMap<T, usize>>>,
    _marker: PhantomData<D>,
}

impl<D, T> RedisRoundRobinTaskQueue<D, T>
where
    T: ToString + Eq + std::hash::Hash + Clone,
{
    pub async fn new(
        redis_url: &str,
        queue_name: &str,
    ) -> Result<Self, TaskQueueError> {
        let client = Client::connect(redis_url)
            .await
            .map_err(|e| TaskQueueError::QueueError(e.to_string()))?;

        Ok(Self {
            client,
            queue_prefix: format!("queue:{}:", queue_name),
            dlq_key: format!("dlq:{}", queue_name),
            round_robin: Arc::new(Mutex::new(HashMap::new())),
            _marker: PhantomData,
        })
    }

    fn get_queue_key(&self, tag: &T) -> String {
        format!("{}{}", self.queue_prefix, tag.to_string())
    }

    async fn execute_pipeline(
        &self,
        pipeline: Pipeline<'_>,
    ) -> Result<(), TaskQueueError> {
        pipeline
            .execute()
            .await
            .map_err(|e| TaskQueueError::QueueError(e.to_string()))?;
        Ok(())
    }
}

#[async_trait]
impl<D, T> TaskQueue<D> for RedisRoundRobinTaskQueue<D, T>
where
    D: std::fmt::Debug
        + Clone
        + Serialize
        + DeserializeOwned
        + Send
        + Sync
        + HasTagKey<TagValue = T>
        + 'static,
    T: ToString + Eq + std::hash::Hash + Clone + Send + Sync + 'static,
{
    async fn push(&self, task: &Task<D>) -> Result<(), TaskQueueError> {
        let task_json = serde_json::to_string(task)
            .map_err(|e| TaskQueueError::SerdeError(e.to_string()))?;
        let tag = task.payload.get_tag_value();
        let queue_key = self.get_queue_key(&tag);

        let mut pipeline = self.client.create_pipeline();
        pipeline.rpush(&queue_key, &task_json).forget();
        pipeline.set(task.task_id.to_string(), &task_json).forget();

        self.execute_pipeline(pipeline).await?;

        // Update round-robin state
        let mut rr = self.round_robin.lock().unwrap();
        rr.entry(tag).or_insert(0);

        Ok(())
    }

    async fn pop(&self) -> Result<Task<D>, TaskQueueError> {
        let mut rr = self.round_robin.lock().unwrap();

        if rr.is_empty() {
            return Err(TaskQueueError::QueueEmpty);
        }

        for _ in 0..rr.len() {
            let (tag, index) = rr.iter_mut().next().unwrap();
            let queue_key = self.get_queue_key(tag);

            let result: Option<String> = self
                .client
                .lpop(&queue_key, 1)
                .await
                .map_err(|e| TaskQueueError::QueueError(e.to_string()))?;

            if let Some(task_json) = result {
                *index = (*index + 1) % rr.len();
                let task: Task<D> = serde_json::from_str(&task_json)
                    .map_err(|e| TaskQueueError::SerdeError(e.to_string()))?;
                return Ok(task);
            }

            *index = (*index + 1) % rr.len();
        }

        Err(TaskQueueError::QueueEmpty)
    }

    async fn ack(&self, task_id: &TaskId) -> Result<(), TaskQueueError> {
        self.client
            .del(task_id.to_string())
            .await
            .map_err(|e| TaskQueueError::QueueError(e.to_string()))?;
        Ok(())
    }

    async fn nack(&self, task: &Task<D>) -> Result<(), TaskQueueError> {
        let task_json = serde_json::to_string(task)
            .map_err(|e| TaskQueueError::SerdeError(e.to_string()))?;

        let mut pipeline = self.client.create_pipeline();
        pipeline.rpush(&self.dlq_key, &task_json).forget();
        pipeline.del(task.task_id.to_string()).forget();

        self.execute_pipeline(pipeline).await
    }

    async fn set(&self, task: &Task<D>) -> Result<(), TaskQueueError> {
        let task_json = serde_json::to_string(task)
            .map_err(|e| TaskQueueError::SerdeError(e.to_string()))?;

        self.client
            .set(task.task_id.to_string(), &task_json)
            .await
            .map_err(|e| TaskQueueError::QueueError(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    struct TestData {
        value: u32,
        tag: String,
    }

    impl HasTagKey for TestData {
        type TagValue = String;

        fn get_tag_value(&self) -> Self::TagValue {
            self.tag.clone()
        }
    }

    #[tokio::test]
    async fn test_redis_rr_queue_operations() {
        let redis_url = "redis://127.0.0.1:6379";
        let queue = RedisRoundRobinTaskQueue::<TestData, String>::new(
            redis_url,
            "test_rr_queue",
        )
        .await
        .expect("Failed to create Redis RR queue");

        // Test push and pop with round-robin behavior
        let tasks = vec![
            Task::new(TestData {
                value: 1,
                tag: "A".to_string(),
            }),
            Task::new(TestData {
                value: 2,
                tag: "B".to_string(),
            }),
            Task::new(TestData {
                value: 3,
                tag: "A".to_string(),
            }),
            Task::new(TestData {
                value: 4,
                tag: "B".to_string(),
            }),
        ];

        for task in &tasks {
            queue.push(task).await.expect("Failed to push task");
        }

        // Pop tasks and verify round-robin order
        let popped1 = queue.pop().await.expect("Failed to pop task");
        let popped2 = queue.pop().await.expect("Failed to pop task");
        let popped3 = queue.pop().await.expect("Failed to pop task");
        let popped4 = queue.pop().await.expect("Failed to pop task");

        assert_eq!(popped1.payload.tag, "A");
        assert_eq!(popped2.payload.tag, "B");
        assert_eq!(popped3.payload.tag, "A");
        assert_eq!(popped4.payload.tag, "B");

        // Test ack
        queue
            .ack(&popped1.task_id)
            .await
            .expect("Failed to ack task");

        // Test nack
        queue.nack(&popped2).await.expect("Failed to nack task");

        // Test set
        let mut task = Task::new(TestData {
            value: 5,
            tag: "C".to_string(),
        });
        queue.push(&task).await.expect("Failed to push task");
        task.payload.value = 6;
        queue.set(&task).await.expect("Failed to set task");
        let updated_task = queue.pop().await.expect("Failed to pop updated task");
        assert_eq!(updated_task.payload.value, 6);

        // Clean up
        let client = Client::connect(redis_url)
            .await
            .expect("Failed to connect to Redis");
        client
            .del::<_, ()>("queue:test_rr_queue:A")
            .await
            .expect("Failed to delete test queue A");
        client
            .del::<_, ()>("queue:test_rr_queue:B")
            .await
            .expect("Failed to delete test queue B");
        client
            .del::<_, ()>("queue:test_rr_queue:C")
            .await
            .expect("Failed to delete test queue C");
        client
            .del::<_, ()>("dlq:test_rr_queue")
            .await
            .expect("Failed to delete test DLQ");
    }
}

/*
use crate::prelude::{HasTagKey, Task, TaskId, TaskStorage, TaskStorageError};
use async_trait::async_trait;
use rustis::commands::{GenericCommands, HashCommands, ListCommands};
use serde::{de::DeserializeOwned, Serialize};
use std::{
    marker::PhantomData,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

pub struct RedisRoundRobinTaskStorage<D> {
    pub key: String,
    pub redis: rustis::client::Client,
    pub tags: Arc<Mutex<std::collections::HashSet<String>>>,
    pub current_tag_index: Arc<AtomicUsize>,
    pub tag_field_name: String,
    _marker1: PhantomData<D>,
}

#[async_trait]
impl<D> TaskStorage<D> for RedisRoundRobinTaskStorage<D>
where
    D: std::fmt::Debug
        + Clone
        + HasTagKey
        + Serialize
        + DeserializeOwned
        + Send
        + Sync
        + 'static,
{
    async fn task_ack(
        &self,
        task_id: &TaskId,
    ) -> Result<Task<D>, TaskStorageError> {
        let hashmap_key = self.get_hashmap_key();
        let uuid_as_str = task_id.to_string();
        let task_value: String =
            self.redis.hget(&hashmap_key, &uuid_as_str).await?;
        let _ = self.redis.hdel(hashmap_key, &uuid_as_str).await;
        let task = serde_json::from_str(&task_value).map_err(|err| {
            TaskStorageError::DeserializationError(err.to_string())
        })?;
        Ok(task)
    }

    async fn task_get(
        &self,
        task_id: &TaskId,
    ) -> Result<Task<D>, TaskStorageError> {
        let hashmap_key = self.get_hashmap_key();
        let uuid_as_str = task_id.to_string();
        let task_value: String =
            self.redis.hget(&hashmap_key, &uuid_as_str).await?;
        let task_data: Task<D> =
            serde_json::from_str(&task_value).map_err(|err| {
                TaskStorageError::DeserializationError(err.to_string())
            })?;
        Ok(task_data)
    }

    async fn task_set(&self, task: &Task<D>) -> Result<(), TaskStorageError> {
        let hashmap_key = self.get_hashmap_key();
        let task_value: String = serde_json::to_string(task)
            .map_err(|err| TaskStorageError::SerializationError(err.to_string()))?;
        let uuid_as_str = task.task_id.to_string();
        let _ = self
            .redis
            .hset(&hashmap_key, [(&uuid_as_str, &task_value)])
            .await;
        Ok(())
    }

    async fn task_pop(&self) -> Result<Task<D>, TaskStorageError> {
        let queue_name = self.get_next_queue()?;
        let list_key = self.get_list_key(&queue_name);
        let hashmap_key = self.get_hashmap_key();
        let task_ids: Vec<String> = self.redis.rpop(&list_key, 1).await?;
        if task_ids.len() > 0 {
            let task_id = task_ids.first().unwrap();
            let task_value: String = self.redis.hget(&hashmap_key, task_id).await?;
            let task: Task<D> =
                serde_json::from_str(&task_value).map_err(|err| {
                    TaskStorageError::DeserializationError(err.to_string())
                })?;
            return Ok(task);
        }

        Err(TaskStorageError::StorageIsEmptyError)
    }

    async fn task_push(&self, task: &Task<D>) -> Result<(), TaskStorageError> {
        let queue_name = task.payload.get_tag_value().to_string();
        let task_value = serde_json::to_string(task)
            .map_err(|err| TaskStorageError::SerializationError(err.to_string()))?;
        let list_key = self.get_list_key(&queue_name);
        let hashmap_key = self.get_hashmap_key();
        let uuid_as_str = task.task_id.to_string();

        let _ = self.redis.lpush(&list_key, &uuid_as_str).await?;
        let _ = self
            .redis
            .hset(&hashmap_key, [(&uuid_as_str, &task_value)])
            .await?;
        Ok(())
    }

    async fn task_to_dlq(&self, task: &Task<D>) -> Result<(), TaskStorageError> {
        let task_value = serde_json::to_string(task)
            .map_err(|err| TaskStorageError::SerializationError(err.to_string()))?;
        let dlq_key = self.get_dlq_key();
        let uuid_as_str = task.task_id.to_string();

        let _ = self
            .redis
            .hset(&dlq_key, [(&uuid_as_str, &task_value)])
            .await?;
        Ok(())
    }

    async fn purge(&self) -> Result<(), TaskStorageError> {
        let _ = self.redis.del(self.get_hashmap_key()).await?;
        // TODO: can do better
        let tags: Vec<String> =
            self.tags.lock().unwrap().iter().map(|t| t.into()).collect();
        for tag in tags {
            let _ = self.redis.del(self.get_list_key(&tag)).await?;
        }
        let _ = self.redis.del(self.get_dlq_key()).await?;
        Ok(())
    }
}

impl<D> RedisRoundRobinTaskStorage<D> {
    /// Construct a new empty redis task storage
    pub fn new(
        key: &str,
        tags: std::collections::HashSet<String>,
        tag_field_name: &str,
        redis: rustis::client::Client,
    ) -> Self {
        Self {
            key: key.to_string(),
            redis,
            tags: Arc::new(Mutex::new(tags)),
            current_tag_index: Arc::new(AtomicUsize::new(0)),
            tag_field_name: tag_field_name.to_string(),
            _marker1: PhantomData,
        }
    }

    pub fn get_hashmap_key(&self) -> String {
        format!("{}:{}", self.key, "hm")
    }

    pub fn get_list_key(&self, queue_key: &str) -> String {
        format!("{}:{}:{}", self.key, queue_key, "ls")
    }

    pub fn get_dlq_key(&self) -> String {
        format!("{}:{}", self.key, "dlq")
    }

    fn get_next_queue(&self) -> Result<String, TaskStorageError> {
        let queues = self.tags.lock().unwrap();
        let len = queues.len();
        if queues.is_empty() {
            return Err(TaskStorageError::EmptyValueError(
                "self.tags is empty".to_string(),
            ));
        }

        let index = self.current_tag_index.fetch_add(1, Ordering::Relaxed) % len;
        Ok(queues.iter().nth(index).unwrap().clone())
    }
}

impl<D> std::fmt::Debug for RedisRoundRobinTaskStorage<D> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // TODO: implement debug output for data in redis
        // Use the debug builders to format the output.
        f.debug_struct("RedisRoundRobinTaskStorage")
            // .field("hashmap", &*hashmap)
            // .field("list", &*list)
            .finish()
    }
}
*/
