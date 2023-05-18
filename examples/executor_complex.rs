//! With actual database
use async_trait::async_trait;
use capp::config::Configurable;
use capp::executor::storage::InMemoryTaskStorage;
use capp::executor::storage::TaskStorage;
use capp::executor::task::{Task, TaskProcessor};
use capp::executor::{self, ExecutorOptionsBuilder};
use rustis::commands::HashCommands;
use serde::{Deserialize, Serialize};
use std::path;
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

pub struct Context {
    name: String,
    pub redis_client: Option<rustis::client::Client>,
    config: serde_yaml::Value,
    pub user_agents: Option<Vec<String>>,
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
            redis_client: None,
            config: config.unwrap(),
            user_agents: None,
        }
    }

    fn load_uas(&mut self, uas_file_path: impl AsRef<path::Path>) {
        self.user_agents = Self::load_text_file_lines(uas_file_path).ok();
    }

    pub async fn init_redis_client(&mut self) {
        let uri = std::env::var("REDIS_URI").expect("Set REDIS_URI env variable");
        self.redis_client =
            Some(rustis::client::Client::connect(uri).await.unwrap());
    }
}

#[async_trait]
impl TaskProcessor<TaskData, TaskError, Context> for TestTaskProcessor {
    /// Processor will fail tasks which value can be divided to 3
    async fn process(
        &self,
        worker_id: usize,
        ctx: Arc<Context>,
        data: &mut TaskData,
    ) -> Result<(), TaskError> {
        log::info!("[worker-{}] Processing task: {:?}", worker_id, data);
        let rem = data.value % 3;
        if rem == 0 {
            return Err(TaskError::Unknown);
        };

        let _ = ctx
            .redis_client
            .as_ref()
            .unwrap()
            .hincrby("capp-complex", "sum", data.value as i64)
            .await;

        data.finished = true;
        tokio::time::sleep(tokio::time::Duration::from_secs(rem as u64)).await;
        Ok(())
    }
}

/// Make storage filled with test data.
/// For current set following conditions should be true:
/// total tasks = 9
/// number of failed tasks = 4
async fn make_storage() -> InMemoryTaskStorage<TaskData, TaskError> {
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
    simple_logger::SimpleLogger::new().env().init().unwrap();
    // Load app
    let config_path = "tests/simple_config.yml";
    std::env::set_var("REDIS_URI", "redis://localhost/2");
    let mut ctx = Context::from_config(config_path);
    let uas_file_path = {
        ctx.get_config_value("app.user_agents_file")
            .unwrap()
            .as_str()
            .unwrap()
            .to_owned()
    };
    ctx.load_uas(&uas_file_path);
    ctx.init_redis_client().await;

    let ctx = Arc::new(ctx);
    let storage = Arc::new(make_storage().await);
    let processor = Arc::new(TestTaskProcessor {});
    let executor_options = ExecutorOptionsBuilder::default()
        .concurrency_limit(2 as usize)
        .build()
        .unwrap();
    executor::run(ctx.clone(), processor, storage, executor_options).await;

    let sum_of_value: i64 = ctx
        .redis_client
        .as_ref()
        .unwrap()
        .hget("capp-complex", "sum")
        .await
        .unwrap();
    log::info!("Sum of values: {}", sum_of_value);
}