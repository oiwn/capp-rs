//! This module provides a trait for interacting with task storage.
//! The storage allows tasks to be pushed to and popped from a queue,
//! and also allows tasks to be set and retrieved by their UUID.

use async_trait::async_trait;
use serde::{de::DeserializeOwned, Serialize};
use std::{fmt::Debug, sync::Arc};
use thiserror::Error;

pub use crate::backend::InMemoryTaskQueue;
#[cfg(feature = "mongodb")]
pub use crate::backend::MongoTaskQueue;
#[cfg(feature = "redis")]
pub use crate::backend::{RedisRoundRobinTaskQueue, RedisTaskQueue};
use crate::task::{Task, TaskId};

#[derive(Error, Debug)]
pub enum TaskQueueError {
    #[error("Queue error: {0}")]
    QueueError(String),
    #[error("Ser/De error: {0}")]
    SerdeError(String),
    #[error("Task not found: {0}")]
    TaskNotFound(TaskId),
    #[error("Queue is empty")]
    QueueEmpty,
    #[cfg(feature = "redis")]
    #[error("Redis error")]
    RedisError(#[from] rustis::Error),
    #[cfg(feature = "mongodb")]
    #[error("Mongodb Error")]
    MongodbError(#[from] mongodb::error::Error),
}

#[async_trait]
pub trait TaskQueue<Data>
where
    Data: std::fmt::Debug
        + Clone
        + Serialize
        + DeserializeOwned
        + Send
        + Sync
        + 'static,
{
    async fn push(&self, task: &Task<Data>) -> Result<(), TaskQueueError>;
    async fn pop(&self) -> Result<Task<Data>, TaskQueueError>;
    async fn ack(&self, task_id: &TaskId) -> Result<(), TaskQueueError>;
    async fn nack(&self, task: &Task<Data>) -> Result<(), TaskQueueError>;
    // NOTE: probably need to move into different trait
    async fn set(&self, task: &Task<Data>) -> Result<(), TaskQueueError>;
}

pub type AbstractTaskQueue<D> = Arc<dyn TaskQueue<D> + Send + Sync>;

// Trait used for round-robin queues
pub trait HasTagKey {
    type TagValue: ToString + PartialEq;
    fn get_tag_value(&self) -> Self::TagValue;
}
