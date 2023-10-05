use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum TaskStatus {
    Queued,
    InProgress,
    Completed,
    Failed,
    DeadLetter,
}

/// A `Task` struct represents a single unit of work that will be processed
/// by a worker. It contains payload of type `D`, which is used by the worker
/// during processing. The `Task` struct also includes fields for managing
/// the task's lifecycle, including the task's UUID, the start and
/// finish times, the number of retries, and any error messages.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Task<D: Clone> {
    pub task_id: Uuid,
    pub payload: D,
    pub status: TaskStatus,
    pub queued: DateTime<Utc>,
    pub started: Option<DateTime<Utc>>,
    pub finished: Option<DateTime<Utc>>,
    pub retries: u32,
    pub error_msg: Option<String>,
}

impl<D: Clone> Task<D> {
    pub fn new(payload: D) -> Self {
        Task {
            task_id: Uuid::new_v4(),
            payload,
            status: TaskStatus::Queued,
            queued: Utc::now(),
            started: None,
            finished: None,
            retries: 0,
            error_msg: None,
        }
    }

    pub fn set_in_process(&mut self) {
        self.status = TaskStatus::InProgress;
        self.started = Some(Utc::now());
    }

    pub fn set_succeed(&mut self) {
        self.status = TaskStatus::Completed;
        self.finished = Some(Utc::now());
    }

    pub fn set_retry(&mut self, err_msg: &str) {
        self.status = TaskStatus::Failed;
        self.finished = Some(Utc::now());
        self.retries += 1;
        self.error_msg = Some(err_msg.to_string());
    }

    pub fn set_dlq(&mut self, err_msg: &str) {
        self.status = TaskStatus::DeadLetter;
        self.finished = Some(Utc::now());
        self.retries = 0;
        self.error_msg = Some(err_msg.to_string());
    }

    pub fn set_status(&mut self, new_status: TaskStatus) {
        self.status = new_status;
    }

    pub fn get_payload(&self) -> &D {
        &self.payload
    }
}
