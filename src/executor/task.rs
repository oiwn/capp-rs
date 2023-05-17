use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

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

/// A trait defining the interface for processing a task. This trait is
/// intended to be implemented by a worker that will process tasks
/// of a specific type.
#[async_trait]
pub trait TaskProcessor<D, E> {
    /// Processes the task. The worker_id is passed for logging or
    /// debugging purposes. The task_data is a mutable reference,
    /// allowing the processor to modify the task data as part of the processing.
    async fn process(&self, worker_id: usize, task_data: &mut D) -> Result<(), E>;
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
