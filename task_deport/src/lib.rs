//! This module provides a trait for interacting with task storage.
//! The storage allows tasks to be pushed to and popped from a queue,
//! and also allows tasks to be set and retrieved by their UUID.
use async_trait::async_trait;
use serde::{de::DeserializeOwned, Serialize};
pub use uuid::Uuid;

pub mod backends;
pub mod task;

pub use backends::{InMemoryTaskStorage, InMemoryTaskStorageError};
#[cfg(feature = "redis")]
pub use backends::{
    RedisRoundRobinTaskStorage, RedisTaskStorage, RedisTaskStorageError,
};
pub use task::Task;

/// A trait that describes the necessary methods for task storage. This includes
/// methods for acknowledging a task, getting a task by its UUID, setting a task,
/// popping a task from the queue, and pushing a task into the queue.
/// Whole functions should be non blocking. I.e. task_push should return None
/// to be able to process situation when there is no tasks in queue on worker side.
#[async_trait]
pub trait TaskStorage<D, E>
where
    D: Clone + Serialize + DeserializeOwned + Send + Sync + 'static,
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
