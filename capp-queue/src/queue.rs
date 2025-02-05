//! This module provides a trait for interacting with task storage.
//! The storage allows tasks to be pushed to and popped from a queue,
//! and also allows tasks to be set and retrieved by their UUID.

use async_trait::async_trait;
use serde::{de::DeserializeOwned, Serialize};
use std::{fmt::Debug, sync::Arc};

use super::TaskQueueError;
use crate::task::{Task, TaskId};

#[async_trait]
pub trait TaskQueue<Data>
where
    Data: Debug + Clone + Serialize + DeserializeOwned + Send + Sync + 'static,
{
    async fn push(&self, task: &Task<Data>) -> Result<(), TaskQueueError>;
    async fn pop(&self) -> Result<Task<Data>, TaskQueueError>;
    async fn ack(&self, task_id: &TaskId) -> Result<(), TaskQueueError>;
    async fn nack(&self, task: &Task<Data>) -> Result<(), TaskQueueError>;
    async fn set(&self, task: &Task<Data>) -> Result<(), TaskQueueError>;
}

pub type AbstractTaskQueue<D> = Arc<dyn TaskQueue<D> + Send + Sync>;

// Trait used for round-robin queues
pub trait HasTagKey {
    type TagValue: ToString + PartialEq;
    fn get_tag_value(&self) -> Self::TagValue;
}
