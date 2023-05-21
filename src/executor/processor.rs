use crate::task_deport::Task;
use async_trait::async_trait;
use std::sync::Arc;

/// A trait defining the interface for processing a task. This trait is
/// intended to be implemented by a worker that will process tasks
/// of a specific type.
#[async_trait]
pub trait TaskProcessor<D, E, S, C> {
    /// Processes the task. The worker_id is passed for logging or
    /// debugging purposes. The task is a mutable reference,
    /// allowing the processor to modify the task data as part of the processing.
    async fn process(
        &self,
        worker_id: usize,
        ctx: Arc<C>,
        storage: Arc<S>,
        task: &mut Task<D>,
    ) -> Result<(), E>;
}
