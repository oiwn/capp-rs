use async_trait::async_trait;
use capp_queue::queue::AbstractTaskQueue;
use capp_queue::task::Task;
use serde::{Serialize, de::DeserializeOwned};
use std::sync::Arc;
use thiserror::Error;

use super::worker::WorkerId;

#[derive(Error, Debug)]
pub enum ComputationError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Database error: {0}")]
    Db(String),
    #[error("Task storage error: {0}")]
    Storage(String),
    #[error("Task error: {0}")]
    Task(String),
    #[error("Computation execution error: {0}")]
    Function(String),
    #[error("Max retries: {0}")]
    MaxRetries(String),
}

/// A trait defining the interface for processing a task. This trait is
/// intended to be implemented by a worker that will process tasks
/// of a specific type.
#[async_trait]
pub trait Computation<Data, Ctx>
where
    Data: Clone + Serialize + DeserializeOwned + Send + Sync + 'static,
    Ctx: Send + Sync + 'static,
{
    /// Do computation The worker_id is passed for logging or
    /// debugging purposes. The task is a mutable reference,
    /// allowing the processor to modify the task data as part of the processing.
    async fn call(
        &self,
        worker_id: WorkerId,
        ctx: Arc<Ctx>,
        // NOTE: i used type alias instead of this
        // is something put this line back and remove next one!
        // storage: Arc<dyn TaskStorage<Data> + Send + Sync>,
        queue: AbstractTaskQueue<Data>,
        task: &mut Task<Data>,
    ) -> Result<(), ComputationError>;
}
