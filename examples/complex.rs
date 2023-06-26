//! With actual database
use async_trait::async_trait;
use capp::config::Configurable;
use capp::executor::processor::TaskProcessor;
use capp::executor::{self, ExecutorOptionsBuilder};
use capp::task_deport::{RedisTaskStorage, Task, TaskStorage};
use rustis::commands::HashCommands;
use serde::{Deserialize, Serialize};
use std::path;
use std::sync::Arc;
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
    pub flag: bool,
}

#[derive(Debug)]
pub struct TestTaskProcessor {}

pub struct Context {
    name: String,
    pub redis: rustis::client::Client,
    config: serde_yaml::Value,
    pub user_agents: Vec<String>,
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
    async fn from_config(config_file_path: impl AsRef<path::Path>) -> Self {
        let config = Self::load_config(config_file_path)
            .expect("Unable to read config file");
        let name = config["app"]["name"].as_str().unwrap().to_string();
        let uas_file_path = config["app"]["user_agents_file"].as_str().unwrap();
        let user_agents = Self::load_text_file_lines(uas_file_path)
            .expect("Unable to read user agents file");
        // connect to redis
        let uri = std::env::var("REDIS_URI").expect("Set REDIS_URI env variable");
        let redis = rustis::client::Client::connect(uri)
            .await
            .expect("Unable to make redis connection");

        Self {
            name,
            redis,
            config,
            user_agents,
        }
    }
}

#[async_trait]
impl
    TaskProcessor<TaskData, TaskProcessorError, RedisTaskStorage<TaskData>, Context>
    for TestTaskProcessor
{
    /// Processor will fail tasks which value can be divided to 3
    /// NOTE: Here i realized i need storage passed to process function as well
    ///     to be able to push more tasks into queue or modify existing
    async fn process(
        &self,
        worker_id: usize,
        ctx: Arc<Context>,
        _storage: Arc<RedisTaskStorage<TaskData>>,
        task: &mut Task<TaskData>,
    ) -> Result<(), TaskProcessorError> {
        log::info!("[worker-{}] Processing task: {:?}", worker_id, task.data);
        let rem = task.data.value % 3;
        if rem == 0 {
            return Err(TaskProcessorError::Unknown);
        };

        let _ = ctx
            .redis
            .hincrby("capp-complex", "sum", task.data.value as i64)
            .await;

        task.data.flag = true;

        // let _ ctx.redis.ta
        tokio::time::sleep(tokio::time::Duration::from_secs(rem as u64)).await;
        Ok(())
    }
}

/// Make storage filled with test data.
/// For current set following conditions should be true:
/// total tasks = 9
/// number of failed tasks = 4
async fn make_storage(
    client: rustis::client::Client,
) -> RedisTaskStorage<TaskData> {
    let storage = RedisTaskStorage::new("capp-complex", client);

    for i in 1..=3 {
        let task: Task<TaskData> = Task::new(TaskData {
            domain: "one".to_string(),
            value: i,
            flag: false,
        });
        let _ = storage.task_push(&task).await;
    }

    for i in 1..=3 {
        let task: Task<TaskData> = Task::new(TaskData {
            domain: "two".to_string(),
            value: i * 3,
            flag: false,
        });
        let _ = storage.task_push(&task).await;
    }

    for _ in 1..=3 {
        let task: Task<TaskData> = Task::new(TaskData {
            domain: "three".to_string(),
            value: 2,
            flag: false,
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
    std::env::set_var("REDIS_URI", "redis://localhost:6379/15");
    let ctx = Arc::new(Context::from_config(config_path).await);
    let storage = Arc::new(make_storage(ctx.redis.clone()).await);

    let processor = Arc::new(TestTaskProcessor {});
    let executor_options = ExecutorOptionsBuilder::default()
        .concurrency_limit(2 as usize)
        .build()
        .unwrap();
    executor::run_workers(ctx.clone(), processor, storage, executor_options).await;

    let sum_of_value: i64 =
        ctx.clone().redis.hget("capp-complex", "sum").await.unwrap();
    log::info!("Sum of values: {}", sum_of_value);
}
