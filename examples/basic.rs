use async_trait::async_trait;
use capp::executor::storage::InMemoryTaskStorage;
use capp::executor::storage::TaskStorage;
use capp::executor::task::{Task, TaskProcessor};
use capp::executor::{self, ExecutorOptionsBuilder};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use thiserror::Error;

#[derive(Error, Debug, Serialize, Deserialize)]
pub enum TaskError {
    #[error("unknown error")]
    Unknown,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskData {
    pub domain: String,
    pub value: u32,
    pub finished: bool,
}
#[derive(Debug)]
pub struct TestTaskProcessor {}

#[async_trait]
impl TaskProcessor<TaskData, TaskError> for TestTaskProcessor {
    /// Processor will fail tasks which value can be divided to 3
    async fn process(
        &self,
        worker_id: usize,
        data: &mut TaskData,
    ) -> Result<(), TaskError> {
        log::info!("[worker-{}] Processing task: {:?}", worker_id, data);
        let rem = data.value % 3;
        if rem == 0 {
            return Err(TaskError::Unknown);
        };

        data.finished = true;
        Ok(())
    }
}

/// Make storage filled with test data.
/// For current set following conditions should be true:
/// total tasks = 9
/// number of failed tasks = 4
async fn make_storage() -> Arc<InMemoryTaskStorage<TaskData, TaskError>> {
    let storage = Arc::new(InMemoryTaskStorage::new());

    for i in 1..=3 {
        let task: Task<TaskData> = Task::new(TaskData {
            domain: "one".to_string(),
            value: i,
            finished: false,
        });
        let _ = storage.task_push(&task).await;
    }

    for i in 1..=3 {
        let task: Task<TaskData> = Task::new(TaskData {
            domain: "two".to_string(),
            value: i * 3,
            finished: false,
        });
        let _ = storage.task_push(&task).await;
    }

    for _ in 1..=3 {
        let task: Task<TaskData> = Task::new(TaskData {
            domain: "three".to_string(),
            value: 2,
            finished: false,
        });
        let _ = storage.task_push(&task).await;
    }
    storage
}

#[tokio::main]
async fn main() {
    simple_logger::SimpleLogger::new().env().init().unwrap();
    let storage = make_storage().await;
    let processor = Arc::new(TestTaskProcessor {});
    let executor_options = ExecutorOptionsBuilder::default().build().unwrap();
    executor::run(processor, storage, executor_options).await;
}
