//! This module provides a trait for interacting with task storage.
//! The storage allows tasks to be pushed to and popped from a queue,
//! and also allows tasks to be set and retrieved by their UUID.
use async_trait::async_trait;
use serde::{de::DeserializeOwned, Serialize};
use thiserror::Error;

pub mod backends;
pub mod task;

pub use backends::InMemoryTaskStorage;
#[cfg(feature = "redis")]
pub use backends::{RedisRoundRobinTaskStorage, RedisTaskStorage};
pub use task::{Task, TaskId};

#[derive(Error, Debug)]
pub enum TaskStorageError {
    #[error("Storage error: {0}")]
    StorageError(String),
    #[error("Serialization error: {0}")]
    SerializationError(String),
    #[error("Deserialization error: {0}")]
    DeserializationError(String),
    #[error("Task not found {0}")]
    TaskNotFound(TaskId),
    #[error("Empty value ")]
    EmptyValueError(String),
    #[error("Storage is empty")]
    StorageIsEmptyError,
    #[cfg(feature = "redis")]
    #[error("Redis error")]
    RedisError(#[from] rustis::Error),
}

/// A trait that describes the necessary methods for task storage. This includes
/// methods for acknowledging a task, getting a task by its UUID, setting a task,
/// popping a task from the queue, and pushing a task into the queue.
/// Whole functions should be non blocking. I.e. task_push should return None
/// to be able to process situation when there is no tasks in queue on worker side.
#[async_trait]
pub trait TaskStorage<D>
where
    D: Clone + Serialize + DeserializeOwned + Send + Sync + 'static,
{
    async fn task_ack(&self, task_id: &TaskId)
        -> Result<Task<D>, TaskStorageError>;
    async fn task_get(&self, task_id: &TaskId)
        -> Result<Task<D>, TaskStorageError>;
    async fn task_set(&self, task: &Task<D>) -> Result<(), TaskStorageError>;
    async fn task_pop(&self) -> Result<Task<D>, TaskStorageError>;
    async fn task_push(&self, task: &Task<D>) -> Result<(), TaskStorageError>;
    async fn task_to_dlq(&self, task: &Task<D>) -> Result<(), TaskStorageError>;
}

pub trait HasTagKey {
    type TagValue: ToString + PartialEq;
    fn get_tag_value(&self) -> Self::TagValue;
}
