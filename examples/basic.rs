use std::{path, sync::Arc, time::Duration};

use capp::{
    config::Configurable,
    manager::{
        MailboxConfig, ServiceRequest, ServiceStackOptions, build_service_stack,
        spawn_mailbox_runtime,
    },
    queue::{InMemoryTaskQueue, JsonSerializer, Task},
};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tower::{BoxError, service_fn};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskData {
    pub domain: String,
    pub value: u32,
    pub finished: bool,
}

#[derive(Debug)]
pub struct Context {
    config: toml::Value,
}

impl Configurable for Context {
    fn config(&self) -> &toml::Value {
        &self.config
    }
}

impl Context {
    fn from_config(config_file_path: impl AsRef<path::Path>) -> Self {
        let config = Self::load_config(config_file_path).unwrap();
        Self { config }
    }
}

async fn enqueue_tasks(
    runtime: &capp::manager::MailboxRuntime<TaskData>,
) -> Result<(), BoxError> {
    for i in 1..=5 {
        runtime
            .producer
            .enqueue(Task::new(TaskData {
                domain: "one".to_string(),
                value: i,
                finished: false,
            }))
            .await?;
    }

    for i in 1..=5 {
        runtime
            .producer
            .enqueue(Task::new(TaskData {
                domain: "two".to_string(),
                value: i * 3,
                finished: false,
            }))
            .await?;
    }

    for _ in 1..=10 {
        runtime
            .producer
            .enqueue(Task::new(TaskData {
                domain: "three".to_string(),
                value: 2,
                finished: false,
            }))
            .await?;
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    tracing_subscriber::fmt::init();

    let config_path = "tests/simple_config.toml";
    let ctx = Arc::new(Context::from_config(config_path));
    let queue = Arc::new(InMemoryTaskQueue::<TaskData, JsonSerializer>::new());
    let (done_tx, mut done_rx) = mpsc::channel::<TaskData>(32);

    let service = build_service_stack(
        service_fn(move |req: ServiceRequest<TaskData, Context>| {
            let done_tx = done_tx.clone();
            async move {
                let mut payload = req.task.payload;
                let rem = payload.value % 3;

                tracing::info!(
                    worker_id = req.worker_id,
                    value = payload.value,
                    attempt = req.attempt,
                    "processing division task"
                );

                if rem != 0 {
                    tokio::time::sleep(Duration::from_secs(rem as u64)).await;
                    return Err(
                        format!("can't divide {} by 3", payload.value).into()
                    );
                }

                payload.finished = true;
                tokio::time::sleep(Duration::from_millis(200)).await;
                let _ = done_tx.send(payload).await;
                Ok::<(), BoxError>(())
            }
        }),
        ServiceStackOptions {
            timeout: Some(Duration::from_secs(5)),
            buffer: 8,
            concurrency_limit: Some(4),
        },
    );

    let runtime = spawn_mailbox_runtime(
        queue,
        ctx,
        service,
        MailboxConfig {
            worker_count: 4,
            inbox_capacity: 1,
            producer_buffer: 32,
            result_buffer: 32,
            max_retries: 2,
            dequeue_backoff: Duration::from_millis(25),
        },
    );

    enqueue_tasks(&runtime).await?;

    let mut completed = 0usize;
    let expected = 5usize;
    while completed < expected {
        if done_rx.recv().await.is_some() {
            completed += 1;
        } else {
            break;
        }
    }

    let stats = runtime.stats.borrow().clone();
    tracing::info!(
        completed,
        terminal_failures = stats.terminal_failures,
        "basic example finished"
    );

    runtime.shutdown().await;
    Ok(())
}
