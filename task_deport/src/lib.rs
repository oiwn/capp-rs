//! This module provides a trait for interacting with task storage.
//! The storage allows tasks to be pushed to and popped from a queue,
//! and also allows tasks to be set and retrieved by their UUID.
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
pub use uuid::Uuid;

pub mod memory;
#[cfg(feature = "redis")]
pub mod redis;
#[cfg(feature = "redis")]
pub mod redis_rr;

pub use memory::{InMemoryTaskStorage, InMemoryTaskStorageError};
#[cfg(feature = "redis")]
pub use redis::{RedisTaskStorage, RedisTaskStorageError};
#[cfg(feature = "redis")]
pub use redis_rr::RedisRoundRobinTaskStorage;

/// A `Task` struct represents a single unit of work that will be processed
/// by a worker. It contains data of type `D`, which is used by the worker
/// during processing. The `Task` struct also includes fields for managing
/// the task's lifecycle, including the task's UUID, the start and
/// finish times, the number of retries, and any error messages.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Task<D> {
    pub task_id: Uuid,
    pub data: D,
    pub started: DateTime<Utc>,
    pub finished: Option<DateTime<Utc>>,
    pub retries: u32,
    pub error_msg: Option<String>,
}

/// A trait that describes the necessary methods for task storage. This includes
/// methods for acknowledging a task, getting a task by its UUID, setting a task,
/// popping a task from the queue, and pushing a task into the queue.
/// Whole functions should be non blocking. I.e. task_push should return None
/// to be able to process situation when there is no tasks in queue on worker side.
#[async_trait]
pub trait TaskStorage<D, E>
where
    D: Serialize + DeserializeOwned + Send + Sync + 'static,
    E: std::error::Error + Send + Sync + 'static,
{
    async fn task_ack(&self, task_id: &Uuid) -> Result<Task<D>, E>;
    async fn task_get(&self, task_id: &Uuid) -> Result<Task<D>, E>;
    async fn task_set(&self, task: &Task<D>) -> Result<(), E>;
    async fn task_pop(&self) -> Result<Option<Task<D>>, E>;
    async fn task_push(&self, task: &Task<D>) -> Result<(), E>;
}

pub trait HasTagKey {
    type TagValue: ToString + PartialEq;
    fn get_tag_value(&self) -> Self::TagValue;
}

impl<D> Task<D> {
    pub fn new(task_data: D) -> Self {
        Task {
            task_id: Uuid::new_v4(),
            data: task_data,
            started: Utc::now(),
            finished: None,
            retries: 0,
            error_msg: None,
        }
    }
}
