use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
pub use uuid::Uuid;

/// A `Task` struct represents a single unit of work that will be processed
/// by a worker. It contains payload of type `D`, which is used by the worker
/// during processing. The `Task` struct also includes fields for managing
/// the task's lifecycle, including the task's UUID, the start and
/// finish times, the number of retries, and any error messages.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Task<D: Clone> {
    pub task_id: Uuid,
    pub payload: D,
    pub started: DateTime<Utc>,
    pub finished: Option<DateTime<Utc>>,
    pub retries: u32,
    pub error_msg: Option<String>,
}

impl<D: Clone> Task<D> {
    pub fn new(payload: D) -> Self {
        Task {
            task_id: Uuid::new_v4(),
            payload,
            started: Utc::now(),
            finished: None,
            retries: 0,
            error_msg: None,
        }
    }

    pub fn get_payload(&self) -> &D {
        &self.payload
    }
}
