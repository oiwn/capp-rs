use crate::task_deport::Task;
use async_trait::async_trait;
use std::sync::Arc;
use thiserror::Error;

use super::worker::WorkerId;

#[derive(Error, Debug)]
pub enum TaskProcessorError {
    #[error("I/O error: {0}")]
    IOError(#[from] std::io::Error),
    #[error("Database error: {0}")]
    DBError(String),
    #[error("Task storage error: {0}")]
    StorageError(String),
    #[error("Task error: {0}")]
    TaskError(String),
    #[error("Processor error: {0}")]
    ProcessorError(String),
    #[error("Max retries: {0}")]
    MaxRetriesError(String),
}

/// A trait defining the interface for processing a task. This trait is
/// intended to be implemented by a worker that will process tasks
/// of a specific type.
#[async_trait]
pub trait TaskProcessor<D: Clone, S, C> {
    /// Processes the task. The worker_id is passed for logging or
    /// debugging purposes. The task is a mutable reference,
    /// allowing the processor to modify the task data as part of the processing.
    async fn process(
        &self,
        worker_id: WorkerId,
        ctx: Arc<C>,
        storage: Arc<S>,
        task: &mut Task<D>,
    ) -> Result<(), TaskProcessorError>;
}
