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

    pub fn set_in_progress(&mut self) {
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

//*****************************************************************************
// TaskId with ser/de traits implemented (to convert underlaying Uuid)
//*****************************************************************************

impl TaskId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn get(&self) -> Uuid {
        self.0
    }
}

impl Default for TaskId {
    fn default() -> Self {
        Self::new()
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

//*****************************************************************************
// Tests
//*****************************************************************************

#[cfg(test)]
mod tests {
    use core::panic;

    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Clone, Serialize, Deserialize, Default)]
    struct TaskData {
        value: u32,
    }

    #[test]
    fn task_id_serde() {
        let task = Task::new(TaskData { value: 1 });
        let task_id = task.task_id.clone();
        let serialized_task_value = serde_json::to_value(task).unwrap();
        let serialized_task_json = serialized_task_value.to_string();
        let desrialized_task: Task<TaskData> =
            serde_json::from_str(&serialized_task_json).unwrap();
        assert_eq!(task_id, desrialized_task.task_id);
    }

    #[test]
    fn test_task_creation() {
        let task = Task::new(TaskData::default());
        assert!(task.started_at.is_none());
        assert!(task.finished_at.is_none());
        assert_eq!(task.retries, 0);
        assert_eq!(task.payload.value, 0);
        match task.status {
            TaskStatus::Queued => {}
            _ => panic!("Wrong status (task.status)"),
        };
    }

    #[test]
    fn test_in_progress() {
        let mut task = Task::new(TaskData::default());

        task.set_in_progress();
        match task.status {
            TaskStatus::InProgress => {}
            _ => panic!("Wrong status (task.status)"),
        };
        assert!(task.started_at.is_some());
        assert!(task.finished_at.is_none());
    }

    #[test]
    fn test_succeed() {
        let mut task = Task::new(TaskData::default());

        task.set_succeed();
        match task.status {
            TaskStatus::Completed => {}
            _ => panic!("Wrong status (task.status)"),
        };
        assert!(task.finished_at.is_some());
        assert!(task.started_at.is_none());
    }

    #[test]
    fn test_set_retry() {
        let mut task = Task::new(TaskData::default());

        task.set_retry("Wrong task value");
        match task.status {
            TaskStatus::Failed => {}
            _ => panic!("Wrong status (task.status)"),
        };
        assert!(task.finished_at.is_some());
        assert_eq!(task.retries, 1);
        assert!(task.error_msg.is_some());
        assert!(task.started_at.is_none());
    }

    #[test]
    fn test_set_dlq() {
        let mut task = Task::new(TaskData::default());

        task.set_dlq("Wrong task value");
        match task.status {
            TaskStatus::DeadLetter => {}
            _ => panic!("Wrong status (task.status)"),
        };
        assert!(task.finished_at.is_some());
        assert!(task.started_at.is_none());
        assert!(task.error_msg.is_some());
    }

    #[test]
    fn task_flow_succeed() {
        let mut task = Task::new(TaskData::default());

        task.set_in_progress();
        task.payload.value += 1;

        std::thread::sleep(std::time::Duration::from_millis(5));

        task.set_retry("Wrong task value");
        task.payload.value += 1;

        task.set_in_progress();
        std::thread::sleep(std::time::Duration::from_millis(5));

        task.set_succeed();

        match task.status {
            TaskStatus::Completed => {}
            _ => panic!("Wrong status (task.status)"),
        };
        assert_eq!(task.retries, 1);
        assert_eq!(task.get_payload().value, 2);
        assert!(task.started_at.is_some());
        assert!(task.finished_at.is_some());

        // finished_at - started_at
        assert!(
            task.finished_at
                .unwrap()
                .duration_since(task.started_at.unwrap())
                .unwrap()
                < std::time::Duration::from_millis(10)
        );
        // finished_at - queue_at
        assert!(
            task.finished_at
                .unwrap()
                .duration_since(task.queued_at)
                .unwrap()
                >= std::time::Duration::from_millis(10)
        );
    }

    #[test]
    fn test_flow() {
        let mut task = Task::new(TaskData::default());

        task.set_in_progress();
        task.payload.value += 1;

        std::thread::sleep(std::time::Duration::from_millis(5));

        task.set_retry("Wrong task value");
        task.payload.value += 1;

        task.set_in_progress();
        std::thread::sleep(std::time::Duration::from_millis(5));

        task.set_dlq("Failed to complete task");

        match task.status {
            TaskStatus::DeadLetter => {}
            _ => panic!("Wrong status (task.status)"),
        };
        assert_eq!(task.retries, 1);
        assert_eq!(task.get_payload().value, 2);
        assert!(task.started_at.is_some());
        assert!(task.finished_at.is_some());

        // finished_at - started_at
        assert!(
            task.finished_at
                .unwrap()
                .duration_since(task.started_at.unwrap())
                .unwrap()
                < std::time::Duration::from_millis(10)
        );
        // finished_at - queue_at
        assert!(
            task.finished_at
                .unwrap()
                .duration_since(task.queued_at)
                .unwrap()
                >= std::time::Duration::from_millis(10)
        );
    }
}
