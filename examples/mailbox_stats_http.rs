//! Mailbox demo exposing stats over HTTP (hyper) on 127.0.0.1:8080.
//! Requires `--features stats-http`.

#![cfg(feature = "stats-http")]

use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
    time::Duration,
};

use capp::{
    manager::{
        MailboxConfig, ServiceRequest, ServiceStackOptions, build_service_stack,
        spawn_mailbox_runtime,
    },
    queue::{InMemoryTaskQueue, JsonSerializer, Task},
    stats_http, tracing, tracing_subscriber,
};
use rand::{Rng, rng};
use tokio::signal;
use tower::{BoxError, service_fn};

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct DemoTask {
    id: u32,
    delay_ms: u64,
}

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    tracing_subscriber::fmt::init();

    let task_count = 200u32;
    let queue = Arc::new(InMemoryTaskQueue::<DemoTask, JsonSerializer>::new());
    let ctx = Arc::new(());

    let next_id = Arc::new(AtomicU32::new(task_count));

    let base_service = {
        let next_id = next_id.clone();
        service_fn(move |req: ServiceRequest<DemoTask, ()>| {
            let next_id = next_id.clone();
            async move {
                let delay = Duration::from_millis(req.task.payload.delay_ms);
                tracing::info!(
                    task_id = req.task.payload.id,
                    attempt = req.attempt,
                    worker_id = req.worker_id,
                    delay_ms = req.task.payload.delay_ms,
                    "processing task"
                );
                tokio::time::sleep(delay).await;
                if req.task.payload.id % 7 == 0 && req.attempt <= 1 {
                    return Err(anyhow::anyhow!("simulated failure"))
                        .map_err(Into::into);
                }

                // Enqueue a follow-up task so the workload continues indefinitely.
                let new_id =
                    next_id.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
                let new_delay = rng().random_range(100..800);
                req.producer
                    .enqueue(Task::new(DemoTask {
                        id: new_id,
                        delay_ms: new_delay,
                    }))
                    .await
                    .map_err(|err| -> BoxError { err.into() })?;

                Ok::<(), BoxError>(())
            }
        })
    };

    let service = build_service_stack(
        base_service,
        ServiceStackOptions {
            timeout: Some(Duration::from_secs(5)),
            buffer: 8,
            concurrency_limit: Some(4),
        },
    );

    let runtime = spawn_mailbox_runtime(
        queue.clone(),
        ctx,
        service,
        MailboxConfig {
            worker_count: 4,
            inbox_capacity: 1,
            producer_buffer: task_count as usize,
            result_buffer: task_count as usize,
            max_retries: 2,
            dequeue_backoff: Duration::from_millis(10),
        },
    );

    // Serve stats over HTTP.
    let addr: SocketAddr = "127.0.0.1:8080".parse().unwrap();
    let stats_rx = runtime.stats.clone();
    tokio::spawn(async move {
        if let Err(err) = stats_http::serve_stats(addr, stats_rx).await {
            tracing::error!(?err, "stats server exited with error");
        }
    });

    // Enqueue tasks with small random delays.
    for id in 0..task_count {
        let delay_ms = rng().random_range(100..800);
        runtime
            .producer
            .enqueue(Task::new(DemoTask { id, delay_ms }))
            .await
            .expect("producer channel open");
    }

    tracing::info!(
        "stats live at http://127.0.0.1:8080/stats; press Ctrl+C to stop"
    );
    signal::ctrl_c().await.expect("listen for ctrl+c");
    tracing::info!("shutdown requested");
    runtime.shutdown().await;
    Ok(())
}
