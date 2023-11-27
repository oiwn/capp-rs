use async_trait::async_trait;
use capp::prelude::{
    Computation, ComputationError, WorkerId, WorkerOptionsBuilder,
};
use capp::{
    config::Configurable,
    manager::{WorkersManager, WorkersManagerOptionsBuilder},
    storage::{InMemoryTaskStorage, Task, TaskStorage},
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

#[derive(Debug)]
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
    /// TaskRunner will fail tasks which value can't be divided by 3
    async fn call(
        &self,
        _worker_id: WorkerId,
        ctx: Arc<Context>,
        _storage: Arc<dyn TaskStorage<TaskData> + Send + Sync>,
        task: &mut Task<TaskData>,
    ) -> Result<(), ComputationError> {
        // setup spans
        // let span = tracing::info_span!("computation", worker_id = %worker_id);
        // let _enter = span.enter();

        tracing::info!("Task received to process: {:?}", task.get_payload());

        let rem = task.payload.value % 3;
        if rem != 0 {
            let err_msg = format!("Can't divide {} by 3", task.payload.value);
            tokio::time::sleep(tokio::time::Duration::from_secs(rem as u64)).await;
            return Err(ComputationError::Function(err_msg));
        };

        task.payload.finished = true;
        if ctx.name == "test-app".to_string() {
            tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
        }
        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
        Ok(())
    }
}

/// Make storage filled with test data.
/// For current set following conditions should be true:
/// total tasks = 9
/// number of failed tasks = 4
async fn make_storage() -> impl TaskStorage<TaskData> + Send + Sync {
    let storage = InMemoryTaskStorage::new();

    for i in 1..=5 {
        let task: Task<TaskData> = Task::new(TaskData {
            domain: "one".to_string(),
            value: i,
            finished: false,
        });
        let _ = storage.task_push(&task).await;
    }

    for i in 1..=5 {
        let task: Task<TaskData> = Task::new(TaskData {
            domain: "two".to_string(),
            value: i * 3,
            finished: false,
        });
        let _ = storage.task_push(&task).await;
    }

    for _ in 1..=10 {
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
    let ctx = Context::from_config(config_path);
    let storage = make_storage().await;

    let computation = DivisionComputation {};
    let manager_options = WorkersManagerOptionsBuilder::default()
        .worker_options(
            WorkerOptionsBuilder::default()
                .task_limit(10)
                .build()
                .unwrap(),
        )
        .task_limit(30)
        .concurrency_limit(4_usize)
        .build()
        .unwrap();

    let mut manager =
        WorkersManager::new(ctx, computation, storage, manager_options);
    manager.run_workers().await;
}
