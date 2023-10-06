use async_trait::async_trait;
use capp::{
    config::Configurable,
    executor::{self, processor::TaskProcessor, ExecutorOptionsBuilder},
    task_deport::{InMemoryTaskStorage, Task, TaskStorage},
};
use serde::{Deserialize, Serialize};
use std::{path, sync::Arc};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum TaskProcessorError {
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

#[derive(Debug, Serialize, Deserialize)]
pub struct Context {
    name: String,
    config: serde_yaml::Value,
}

impl Configurable for Context {
    fn name(&self) -> &str {
        self.name.as_str()
    }
    fn config(&self) -> &serde_yaml::Value {
        &self.config
    }
}

impl Context {
    fn from_config(config_file_path: impl AsRef<path::Path>) -> Self {
        let config = Self::load_config(config_file_path);
        Self {
            name: "test-app".to_string(),
            config: config.unwrap(),
        }
    }
}

#[async_trait]
impl
    TaskProcessor<
        TaskData,
        TaskProcessorError,
        InMemoryTaskStorage<TaskData>,
        Context,
    > for TestTaskProcessor
{
    /// Processor will fail tasks which value can be divided to 3
    async fn process(
        &self,
        worker_id: usize,
        _ctx: Arc<Context>,
        _storage: Arc<InMemoryTaskStorage<TaskData>>,
        task: &mut Task<TaskData>,
    ) -> Result<(), TaskProcessorError> {
        tracing::info!(
            "[worker-{}] Task received to process: {:?}",
            worker_id,
            task.get_payload()
        );
        let rem = task.payload.value % 3;
        if rem == 0 {
            return Err(TaskProcessorError::Unknown);
        };

        task.payload.finished = true;
        tokio::time::sleep(tokio::time::Duration::from_secs(rem as u64)).await;
        Ok(())
    }
}

/// Make storage filled with test data.
/// For current set following conditions should be true:
/// total tasks = 9
/// number of failed tasks = 4
async fn make_storage() -> InMemoryTaskStorage<TaskData> {
    let storage = InMemoryTaskStorage::new();

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
    tracing_subscriber::fmt::init();
    // Load app
    let config_path = "tests/simple_config.yml";
    let ctx = Arc::new(Context::from_config(config_path));

    let storage = Arc::new(make_storage().await);
    let processor = Arc::new(TestTaskProcessor {});
    let executor_options = ExecutorOptionsBuilder::default()
        .task_limit(10)
        .concurrency_limit(2_usize)
        .build()
        .unwrap();
    executor::run_workers(ctx, processor, storage, executor_options).await;
}
