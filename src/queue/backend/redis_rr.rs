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
use rustis::client::{BatchPreparedCommand, Client, Pipeline};
use rustis::commands::{HashCommands, ListCommands, StringCommands};
use serde::{de::DeserializeOwned, Serialize};
use std::collections::HashSet;
use std::marker::PhantomData;
use std::sync::Arc;

pub struct RedisRoundRobinTaskQueue<D> {
    pub client: Client,
    pub key: String,
    pub tags: Arc<HashSet<String>>,
    _marker: PhantomData<D>,
}

impl<D> RedisRoundRobinTaskQueue<D> {
    pub async fn new(
        client: Client,
        key: &str,
        tags: HashSet<String>,
    ) -> Result<Self, TaskQueueError> {
        let queue = Self {
            client,
            key: key.to_string(),
            tags: Arc::new(tags),
            _marker: PhantomData,
        };

        // Initialize counters for each tag
        for tag in queue.tags.iter() {
            let _ = queue.client.set(queue.get_counter_key(tag), 0).await?;
        }

        Ok(queue)
    }

    pub fn get_hashmap_key(&self) -> String {
        format!("{}:hm", self.key)
    }

    pub fn get_list_key(&self, tag: &str) -> String {
        format!("{}:{}:ls", self.key, tag)
    }

    pub fn get_counter_key(&self, tag: &str) -> String {
        format!("{}:{}:counter", self.key, tag)
    }

    pub fn get_dlq_key(&self) -> String {
        format!("{}:dlq", self.key)
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

    async fn get_next_non_empty_tag(
        &self,
    ) -> Result<Option<String>, TaskQueueError> {
        for tag in self.tags.iter() {
            let count: i64 = self.client.get(self.get_counter_key(tag)).await?;
            if count > 0 {
                return Ok(Some(tag.clone()));
            }
        }
        Ok(None)
    }
}

#[async_trait]
impl<D> TaskQueue<D> for RedisRoundRobinTaskQueue<D>
where
    D: std::fmt::Debug
        + Clone
        + Serialize
        + DeserializeOwned
        + Send
        + Sync
        + 'static
        + HasTagKey,
{
    async fn push(&self, task: &Task<D>) -> Result<(), TaskQueueError> {
        let task_json = serde_json::to_string(task)
            .map_err(|e| TaskQueueError::SerdeError(e.to_string()))?;
        let tag = task.payload.get_tag_value().to_string();
        let list_key = self.get_list_key(&tag);
        let hashmap_key = self.get_hashmap_key();
        let counter_key = self.get_counter_key(&tag);

        let mut pipeline = self.client.create_pipeline();
        pipeline
            .lpush(&list_key, &task.task_id.to_string())
            .forget();
        pipeline
            .hset(&hashmap_key, [(&task.task_id.to_string(), &task_json)])
            .forget();
        pipeline.incr(counter_key).forget();
        self.execute_pipeline(pipeline).await
    }

    async fn pop(&self) -> Result<Task<D>, TaskQueueError> {
        let tag = self
            .get_next_non_empty_tag()
            .await?
            .ok_or(TaskQueueError::QueueEmpty)?;

        let list_key = self.get_list_key(&tag);
        let hashmap_key = self.get_hashmap_key();
        let counter_key = self.get_counter_key(&tag);

        let task_ids: Vec<String> = self
            .client
            .rpop(&list_key, 1)
            .await
            .map_err(|e| TaskQueueError::QueueError(e.to_string()))?;

        if let Some(task_id) = task_ids.first() {
            let task_value: String =
                self.client.hget(&hashmap_key, task_id).await?;
            let task: Task<D> = serde_json::from_str(&task_value)
                .map_err(|err| TaskQueueError::SerdeError(err.to_string()))?;

            // Decrement the counter
            self.client.decr(counter_key).await?;

            Ok(task)
        } else {
            Err(TaskQueueError::QueueEmpty)
        }
    }

    async fn ack(&self, task_id: &TaskId) -> Result<(), TaskQueueError> {
        let uuid_as_str = task_id.to_string();
        let _ = self
            .client
            .hdel(&self.get_hashmap_key(), &uuid_as_str)
            .await?;
        Ok(())
    }

    async fn nack(&self, task: &Task<D>) -> Result<(), TaskQueueError> {
        let uuid_as_str = task.task_id.to_string();
        let task_json = serde_json::to_string(task)
            .map_err(|e| TaskQueueError::SerdeError(e.to_string()))?;

        let mut pipeline = self.client.create_pipeline();
        pipeline.rpush(&self.get_dlq_key(), &task_json).forget();
        pipeline
            .hdel(&self.get_hashmap_key(), &uuid_as_str)
            .forget();
        self.execute_pipeline(pipeline).await
    }

    async fn set(&self, task: &Task<D>) -> Result<(), TaskQueueError> {
        let task_json = serde_json::to_string(task)
            .map_err(|e| TaskQueueError::SerdeError(e.to_string()))?;

        self.client
            .hset(
                &self.get_hashmap_key(),
                [(&task.task_id.to_string(), &task_json)],
            )
            .await
            .map_err(|e| TaskQueueError::QueueError(e.to_string()))?;
        Ok(())
    }
}

/* use crate::queue::{HashTagKey, TaskQueue, TaskQueueError};
use crate::task::{Task, TaskId};
use async_trait::async_trait;
use rustis::client::{BatchPreparedCommand, Client, Pipeline};
use rustis::commands::{HashCommands, ListCommands};
use serde::{de::DeserializeOwned, Serialize};
use std::collections::HashSet;
use std::marker::PhantomData;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};

pub struct RedisRoundRobinTaskQueue<D> {
    pub client: Client,
    pub key: String,
    pub tags: Arc<Mutex<HashSet<String>>>,
    pub current_tag_index: Arc<AtomicUsize>,
    _marker: PhantomData<D>,
}

impl<D> RedisRoundRobinTaskQueue<D> {
    pub async fn new(
        client: Client,
        key: &str,
        tags: HashSet<String>,
    ) -> Result<Self, TaskQueueError> {
        Ok(Self {
            client,
            key: key.to_string(),
            tags: Arc::new(Mutex::new(tags)),
            current_tag_index: Arc::new(AtomicUsize::new(0)),
            _marker: PhantomData,
        })
    }

    pub fn get_hashmap_key(&self) -> String {
        format!("{}:{}", self.key, "hm")
    }

    pub fn get_list_key(&self, queue_key: &str) -> String {
        format!("{}:{}:{}", self.key, queue_key, "ls")
    }

    fn get_counter_key(&self, tag: &str) -> String {
        format!("{}:{}:counter", self.key, tag)
    }

    pub fn get_dlq_key(&self) -> String {
        format!("{}:{}", self.key, "dlq")
    }

    fn get_next_queue(&self) -> Result<String, TaskQueueError> {
        let queues = self
            .tags
            .lock()
            .map_err(|e| TaskQueueError::QueueError(e.to_string()))?;
        let len = queues.len();
        if queues.is_empty() {
            return Err(TaskQueueError::QueueError(
                "Tags set is empty".to_string(),
            ));
        }

        let index = self.current_tag_index.fetch_add(1, Ordering::Relaxed) % len;
        Ok(queues.iter().nth(index).unwrap().clone())
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
impl<D> TaskQueue<D> for RedisRoundRobinTaskQueue<D>
where
    D: std::fmt::Debug
        + Clone
        + Serialize
        + DeserializeOwned
        + Send
        + Sync
        + 'static
        + HashTagKey,
{
    async fn push(&self, task: &Task<D>) -> Result<(), TaskQueueError> {
        let task_json = serde_json::to_string(task)
            .map_err(|e| TaskQueueError::SerdeError(e.to_string()))?;
        let queue_name = task.payload.get_tag_value().to_string();
        let list_key = self.get_list_key(&queue_name);
        let hashmap_key = self.get_hashmap_key();

        let mut pipeline = self.client.create_pipeline();
        pipeline
            .lpush(&list_key, &task.task_id.to_string())
            .forget();
        pipeline
            .hset(&hashmap_key, [(&task.task_id.to_string(), &task_json)])
            .forget();
        self.execute_pipeline(pipeline).await
    }

    async fn pop(&self) -> Result<Task<D>, TaskQueueError> {
        let queue_name = self.get_next_queue()?;
        let list_key = self.get_list_key(&queue_name);
        let hashmap_key = self.get_hashmap_key();

        let task_ids: Vec<String> = self
            .client
            .rpop(&list_key, 1)
            .await
            .map_err(|e| TaskQueueError::QueueError(e.to_string()))?;

        if !task_ids.is_empty() {
            let task_id = task_ids.first().unwrap();
            let task_value: String =
                self.client.hget(&hashmap_key, task_id).await?;
            let task: Task<D> = serde_json::from_str(&task_value)
                .map_err(|err| TaskQueueError::SerdeError(err.to_string()))?;
            return Ok(task);
        }

        Err(TaskQueueError::QueueEmpty)
    }

    async fn ack(&self, task_id: &TaskId) -> Result<(), TaskQueueError> {
        let uuid_as_str = task_id.to_string();
        let _ = self
            .client
            .hdel(&self.get_hashmap_key(), &uuid_as_str)
            .await?;
        Ok(())
    }

    async fn nack(&self, task: &Task<D>) -> Result<(), TaskQueueError> {
        let uuid_as_str = task.task_id.to_string();
        let task_json = serde_json::to_string(task)
            .map_err(|e| TaskQueueError::SerdeError(e.to_string()))?;

        let mut pipeline = self.client.create_pipeline();
        pipeline.rpush(&self.get_dlq_key(), &task_json).forget();
        pipeline
            .hdel(&self.get_hashmap_key(), &uuid_as_str)
            .forget();
        self.execute_pipeline(pipeline).await
    }

    async fn set(&self, task: &Task<D>) -> Result<(), TaskQueueError> {
        let task_json = serde_json::to_string(task)
            .map_err(|e| TaskQueueError::SerdeError(e.to_string()))?;

        self.client
            .hset(
                &self.get_hashmap_key(),
                [(&task.task_id.to_string(), &task_json)],
            )
            .await
            .map_err(|e| TaskQueueError::QueueError(e.to_string()))?;
        Ok(())
    }
}

impl<D> std::fmt::Debug for RedisRoundRobinTaskQueue<D> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RedisRoundRobinTaskQueue")
            .field("key", &self.key)
            .field("tags", &self.tags)
            .finish()
    }
} */
