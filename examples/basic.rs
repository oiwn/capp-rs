use async_trait::async_trait;
use capp::{
    computation::{Computation, ComputationError},
    config::Configurable,
    task_deport::{InMemoryTaskStorage, Task, TaskStorage},
    ExecutorOptionsBuilder, WorkerId,
};
use serde::{Deserialize, Serialize};
use std::{path, sync::Arc};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskData {
    pub domain: String,
    pub value: u32,
    pub finished: bool,
}

#[derive(Debug)]
pub struct DivisionComputation;

#[derive(Debug, Serialize, Deserialize)]
pub struct Context {
    name: String,
    config: serde_yaml::Value,
}

impl Configurable for Context {
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
impl Computation<TaskData, Context> for DivisionComputation {
    /// TaskRunner will fail tasks which value can be divided by 3
    async fn run(
        &self,
        worker_id: WorkerId,
        _ctx: Arc<Context>,
        _storage: Arc<dyn TaskStorage<TaskData> + Send + Sync>,
        task: &mut Task<TaskData>,
    ) -> Result<(), ComputationError> {
        tracing::info!(
            "[worker-{}] Task received to process: {:?}",
            worker_id,
            task.get_payload()
        );
        let rem = task.payload.value % 3;
        if rem == 0 {
            return Err(ComputationError::Function("Can't divide by 3".to_owned()));
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
async fn make_storage() -> Arc<dyn TaskStorage<TaskData> + Send + Sync> {
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
    tracing_subscriber::fmt::init();
    let config_path = "tests/simple_config.yml";
    let ctx = Arc::new(Context::from_config(config_path));
    let storage = make_storage().await;

    let computation = Arc::new(DivisionComputation {});
    let executor_options = ExecutorOptionsBuilder::default()
        .task_limit(30)
        .concurrency_limit(2_usize)
        .build()
        .unwrap();
    capp::run_workers(ctx, computation, storage, executor_options).await;
}
