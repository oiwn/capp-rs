//! HTTPBin mailbox demo with Tower rate limiting + timeouts.
//! Requires `--features http` to enable the reqwest client.

#![cfg(feature = "http")]

use std::{sync::Arc, time::Duration};

use anyhow::anyhow;
use capp::{
    manager::{MailboxConfig, ServiceRequest, spawn_mailbox_runtime},
    queue::{InMemoryTaskQueue, JsonSerializer, Task},
    tracing, tracing_subscriber,
};
use reqwest::Client;
use tokio::sync::mpsc;
use tower::{
    BoxError, ServiceBuilder,
    buffer::Buffer,
    limit::{ConcurrencyLimitLayer, RateLimitLayer},
    service_fn,
    util::BoxCloneService,
};

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct FetchTask {
    id: u32,
    url: String,
}

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    tracing_subscriber::fmt::init();

    let queue = Arc::new(InMemoryTaskQueue::<FetchTask, JsonSerializer>::new());
    let ctx = Arc::new(());
    let (done_tx, mut done_rx) = mpsc::channel::<FetchTask>(32);

    let client = Client::builder()
        .user_agent("capp-httpbin-example/0.1")
        .build()?;

    let base_service = {
        let client = client.clone();
        let done_tx = done_tx.clone();
        service_fn(move |req: ServiceRequest<FetchTask, ()>| {
            let client = client.clone();
            let done_tx = done_tx.clone();
            async move {
                tracing::info!(
                    task_id = req.task.payload.id,
                    attempt = req.attempt,
                    worker_id = req.worker_id,
                    url = %req.task.payload.url,
                    "fetching"
                );

                let response = client.get(&req.task.payload.url).send().await?;
                let status = response.status();
                let body = response.bytes().await?;

                if !status.is_success() {
                    return Err(anyhow!(
                        "non-success response: {}",
                        status.as_u16()
                    ))
                    .map_err(Into::into);
                }

                tracing::info!(
                    task_id = req.task.payload.id,
                    status = status.as_u16(),
                    bytes = body.len(),
                    "fetched"
                );

                let _ = done_tx.send(req.task.payload).await;
                Ok::<(), BoxError>(())
            }
        })
    };

    let inner = ServiceBuilder::new()
        .layer(ConcurrencyLimitLayer::new(4))
        .timeout(Duration::from_secs(2))
        .service(base_service);

    let rate_limited = ServiceBuilder::new()
        .layer(RateLimitLayer::new(2, Duration::from_secs(1)))
        .service(inner);

    // Buffer provides a cloneable handle around the non-clone rate limiter.
    let service = Buffer::new(rate_limited, 8);

    let service = BoxCloneService::new(service);

    let tasks = vec![
        FetchTask {
            id: 1,
            url: "https://httpbin.org/get".to_string(),
        },
        FetchTask {
            id: 2,
            url: "https://httpbin.org/html".to_string(),
        },
        FetchTask {
            id: 3,
            url: "https://httpbin.org/bytes/512".to_string(),
        },
        FetchTask {
            id: 4,
            url: "https://httpbin.org/delay/1".to_string(),
        },
        FetchTask {
            id: 5,
            url: "https://httpbin.org/delay/3".to_string(),
        },
        FetchTask {
            id: 6,
            url: "https://httpbin.org/status/500".to_string(),
        },
        FetchTask {
            id: 7,
            url: "https://httpbin.org/delay/5".to_string(),
        },
    ];
    let task_count = tasks.len();

    let runtime = spawn_mailbox_runtime(
        queue.clone(),
        ctx,
        service,
        MailboxConfig {
            worker_count: 4,
            inbox_capacity: 1,
            producer_buffer: task_count,
            result_buffer: task_count,
            max_retries: 1,
            dequeue_backoff: Duration::from_millis(50),
        },
    );

    for task in tasks {
        runtime
            .producer
            .enqueue(Task::new(task))
            .await
            .expect("producer channel open");
    }

    let mut stats_rx = runtime.stats.clone();
    let mut completed = 0usize;
    let mut terminal_failures = 0u64;

    while completed + (terminal_failures as usize) < task_count {
        tokio::select! {
            maybe = done_rx.recv() => {
                if maybe.is_some() {
                    completed += 1;
                } else {
                    break;
                }
            }
            changed = stats_rx.changed() => {
                if changed.is_ok() {
                    terminal_failures = stats_rx.borrow().terminal_failures;
                }
            }
        }
    }

    tracing::info!(
        completed,
        terminal_failures,
        "run finished (timeouts expected for /delay endpoints >2s)"
    );
    runtime.shutdown().await;
    Ok(())
}
