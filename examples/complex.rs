use async_trait::async_trait;
use capp::prelude::{WorkerId, WorkerOptionsBuilder};
use capp::{
    config::Configurable,
    manager::{
        Computation, ComputationError, WorkersManager, WorkersManagerOptionsBuilder,
    },
    storage::{RedisTaskStorage, Task, TaskStorage},
};
use rustis::commands::HashCommands;
use serde::{Deserialize, Serialize};
use std::{path, sync::Arc};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskData {
    pub domain: String,
    pub value: u32,
    pub flag: bool,
}

pub struct DivisionComputation;

#[derive(Clone)]
pub struct Context {
    pub redis: rustis::client::Client,
    config: serde_yaml::Value,
    pub user_agents: Vec<String>,
}

impl Configurable for Context {
    fn config(&self) -> &serde_yaml::Value {
        &self.config
    }
}

impl Context {
    async fn from_config(config_file_path: impl AsRef<path::Path>) -> Self {
        let config = Self::load_config(config_file_path)
            .expect("Unable to read config file");
        let uas_file_path = config["app"]["user_agents_file"].as_str().unwrap();
        let user_agents = Self::load_text_file_lines(uas_file_path)
            .expect("Unable to read user agents file");
        // connect to redis
        let uri = std::env::var("REDIS_URI").expect("Set REDIS_URI env variable");
        let redis = rustis::client::Client::connect(uri)
            .await
            .expect("Unable to make redis connection");

        Self {
            redis,
            config,
            user_agents,
        }
    }
}

#[async_trait]
impl Computation<TaskData, Context> for DivisionComputation {
    /// Processor will fail tasks which value can be divided to 3
    async fn call(
        &self,
        worker_id: WorkerId,
        ctx: Arc<Context>,
        _storage: Arc<dyn TaskStorage<TaskData> + Send + Sync>,
        task: &mut Task<TaskData>,
    ) -> Result<(), ComputationError> {
        tracing::info!(
            "[worker-{}] Processing task: {:?}",
            worker_id,
            task.payload
        );
        let rem = task.payload.value % 3;
        if rem != 0 {
            return Err(ComputationError::Function(format!(
                "Can't divide {} by 3",
                &task.payload.value
            )));
        };

        let _ = ctx
            .redis
            .hincrby("capp-complex", "sum", task.payload.value as i64)
            .await;

        task.payload.flag = true;

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
) -> impl TaskStorage<TaskData> + Send + Sync {
    let storage = RedisTaskStorage::new("capp-complex", client);

    for i in 1..=5 {
        let task = Task::new(TaskData {
            domain: "one".to_string(),
            value: i,
            flag: false,
        });
        let _ = storage.task_push(&task).await;
    }

    for i in 1..=5 {
        let task = Task::new(TaskData {
            domain: "two".to_string(),
            value: i * 3,
            flag: false,
        });
        let _ = storage.task_push(&task).await;
    }

    for _ in 1..=5 {
        let task = Task::new(TaskData {
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
    tracing_subscriber::fmt::init();
    // Load app
    let config_path = "tests/simple_config.yml";
    std::env::set_var("REDIS_URI", "redis://localhost:6379/15");
    let ctx = Context::from_config(config_path).await;
    let storage = make_storage(ctx.redis.clone()).await;

    let computation = DivisionComputation {};
    let manager_options = WorkersManagerOptionsBuilder::default()
        .worker_options(
            WorkerOptionsBuilder::default()
                .task_limit(10)
                .build()
                .unwrap(),
        )
        .concurrency_limit(4_usize)
        .build()
        .unwrap();
    let mut manager =
        WorkersManager::new(ctx.clone(), computation, storage, manager_options);
    manager.run_workers().await;

    let sum_of_value: i64 = ctx.redis.hget("capp-complex", "sum").await.unwrap();
    tracing::info!("Sum of values: {}", sum_of_value);
}
