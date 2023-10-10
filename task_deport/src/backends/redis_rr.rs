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

use async_trait::async_trait;
use rustis::commands::{HashCommands, ListCommands};
use serde::{de::DeserializeOwned, Serialize};
use std::{
    marker::PhantomData,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

use crate::{HasTagKey, Task, TaskId, TaskStorage, TaskStorageError};

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
    D: Clone + HasTagKey + Serialize + DeserializeOwned + Send + Sync + 'static,
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
