// use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::time::SystemTime;
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum TaskStatus {
    Queued,
    InProgress,
    Completed,
    Failed,
    DeadLetter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TaskId(Uuid);

/// A `Task` struct represents a single unit of work that will be processed
/// by a worker. It contains payload of type `D`, which is used by the worker
/// during processing. The `Task` struct also includes fields for managing
/// the task's lifecycle, including the task's UUID, the start and
/// finish times, the number of retries, and any error messages.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Task<D: Clone> {
    pub task_id: TaskId,
    pub payload: D,
    pub status: TaskStatus,
    pub queued_at: SystemTime,
    pub started_at: Option<SystemTime>,
    pub finished_at: Option<SystemTime>,
    pub retries: u32,
    pub error_msg: Option<String>,
}

impl<D: Clone> Task<D> {
    pub fn new(payload: D) -> Self {
        Task {
            task_id: TaskId::new(),
            payload,
            status: TaskStatus::Queued,
            queued_at: SystemTime::now(),
            started_at: None,
            finished_at: None,
            retries: 0,
            error_msg: None,
        }
    }

    pub fn set_in_process(&mut self) {
        self.status = TaskStatus::InProgress;
        self.started_at = Some(SystemTime::now());
    }

    pub fn set_succeed(&mut self) {
        self.status = TaskStatus::Completed;
        self.finished_at = Some(SystemTime::now());
    }

    pub fn set_retry(&mut self, err_msg: &str) {
        self.status = TaskStatus::Failed;
        self.finished_at = Some(SystemTime::now());
        self.retries += 1;
        self.error_msg = Some(err_msg.to_string());
    }

    pub fn set_dlq(&mut self, err_msg: &str) {
        self.status = TaskStatus::DeadLetter;
        self.finished_at = Some(SystemTime::now());
        self.error_msg = Some(err_msg.to_string());
    }

    pub fn set_status(&mut self, new_status: TaskStatus) {
        self.status = new_status;
    }

    pub fn get_payload(&self) -> &D {
        &self.payload
    }
}

impl TaskId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn get(&self) -> Uuid {
        self.0
    }
}

// Custom serialization for TaskId.
impl serde::Serialize for TaskId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        // Directly serialize the inner Uuid.
        self.0.serialize(serializer)
    }
}

// Custom deserialization for TaskId.
impl<'de> serde::Deserialize<'de> for TaskId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // Deserialize a Uuid and then wrap it in a TaskId.
        let uuid = Uuid::deserialize(deserializer)?;
        Ok(TaskId(uuid))
    }
}

impl std::fmt::Display for TaskId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "TaskId({})", self.0)
    }
}
