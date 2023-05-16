use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// General definition of Task which contains data of type T required for execution
/// of task type D and could return error of type E
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Task<D, E> {
    pub task_id: Uuid,
    pub data: D,
    pub started: DateTime<Utc>,
    pub finished: Option<DateTime<Utc>>,
    pub retries: u32,
    pub failed_with: Option<E>,
}

#[async_trait]
pub trait TaskStorage<D, E> {
    async fn hashmap_set(&self, task: &Task<D, E>) -> Result<(), E>;
    async fn hashmap_get(&self, task_id: Uuid) -> Result<Option<Task<D, E>>, E>;
    async fn list_push(&self, task: &Task<D, E>) -> Result<(), E>;
    async fn list_pop(&self) -> Result<Option<Task<D, E>>, E>;
    async fn ack(&self, task: &Task<D, E>) -> Result<Task<D, E>, E>;
}

#[async_trait]
pub trait TaskProcessor<D, E> {
    async fn process(&self, worker_id: usize, task_data: &mut D) -> Result<(), E>;
}

impl<D, E> Task<D, E> {
    pub fn new(task_data: D) -> Self {
        Task {
            task_id: Uuid::new_v4(),
            data: task_data,
            started: Utc::now(),
            finished: None,
            retries: 0,
            failed_with: None,
        }
    }
}
