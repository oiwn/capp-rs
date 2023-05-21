//! Typical real world example of another one Hackernews crawler!
//! Goals:
//!     - [ ] Collect posts information
//!     - [ ] Store them into database
//!     - [ ] Use redis bloom filter to filter links
//!     - [ ] crawl comments for post
use async_trait::async_trait;
use capp::config::Configurable;
use capp::executor::storage::InMemoryTaskStorage;
use capp::executor::storage::TaskStorage;
use capp::executor::task::{Task, TaskProcessor};
use capp::executor::{self, ExecutorOptionsBuilder};
use once_cell::sync::Lazy;
use rustis::commands::HashCommands;
use scraper::Selector;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path;
use std::sync::Arc;
use thiserror::Error;

static _SELECTORS: Lazy<HashMap<&'static str, Selector>> = Lazy::new(|| {
    let mut selectors = HashMap::new();
    selectors.insert("posts", Selector::parse("table tr.athing").unwrap());
    selectors.insert(
        "post_author",
        Selector::parse("span.subline a.hnuser").unwrap(),
    );
    selectors.insert("post_title", Selector::parse("span.titleline > a").unwrap());
    selectors
});

#[derive(Debug)]
struct HNPost<'a> {
    title: &'a str,
    url: &'a str,
    author: &'a str,
}

#[derive(Debug)]
struct HNComment<'a> {
    author: &'a str,
    comment: &'a str,
}

#[derive(Error, Debug)]
pub enum TaskError {
    #[error("unknown error")]
    Unknown,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskData {
    pub url: String,
}

#[derive(Debug)]
pub struct TestTaskProcessor {}

pub struct Context {
    name: String,
    redis: rustis::client::Client,
    mongo: mongodb::Database,
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
        let name = config["name"].as_str().unwrap().to_string();
        let uas_file_path = config["app"]["user_agents_file"].as_str().unwrap();
        let user_agents = Self::load_text_file_lines(uas_file_path)
            .expect("Unable to read user agents file");
        // connect to redis
        let uri = std::env::var("REDIS_URI").expect("Set REDIS_URI env variable");
        let redis = rustis::client::Client::connect(uri)
            .await
            .expect("Unable to make redis connection");
        // connect to mongodb
        let uri = std::env::var("mongodb://localhost:27017/ig")
            .expect("Set MONGODB_URI evn variable");
        let mongo = mongodb::Client::with_uri_str(uri)
            .await
            .expect("Unable to make mongodb connection")
            .default_database()
            .expect("Unable to get default database for given uri");

        Self {
            name,
            redis,
            mongo,
            config,
            user_agents,
        }
    }
}

#[async_trait]
impl TaskProcessor<TaskData, TaskError, Context> for TestTaskProcessor {
    /// Processor will fail tasks which value can be divided to 3
    async fn process(
        &self,
        worker_id: usize,
        _ctx: Arc<Context>,
        data: &mut TaskData,
    ) -> Result<(), TaskError> {
        log::info!("[worker-{}] Processing task: {:?}", worker_id, data.url);

        // let _ = ctx
        //     .redis
        //     .as_ref()
        //     .unwrap()
        //     .hincrby("capp-complex", "sum", data.value as i64)
        //     .await;

        Ok(())
    }
}

#[tokio::main]
async fn main() {
    simple_logger::SimpleLogger::new().env().init().unwrap();
    std::env::set_var("REDIS_URI", "redis://localhost/15");
    std::env::set_var("MONGODB_URI", "mongodb://localhost:27017/hn");

    let _ctx = Arc::new(Context::from_config("examples/hn_config.yml"));

    /*
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
    */
}
