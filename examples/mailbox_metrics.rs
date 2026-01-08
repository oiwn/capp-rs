//! Minimal mailbox demo that emits OTLP metrics to Prometheus.
//!
//! Quickstart (Prometheus with OTLP receiver enabled):
//! ```
//! docker run --rm \
//!   -p 9090:9090 \
//!   prom/prometheus:latest \
//!   --config.file=/etc/prometheus/prometheus.yml \
//!   --enable-feature=otlp-write-receiver
//!
//! # Prometheus OTLP HTTP receiver is served on the web port; point to the base (no /v1/metrics).
//! OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:9090/api/v1/otlp \
//! cargo run -p capp --features observability --example mailbox_metrics
//!
//! Then open http://localhost:9090 and query `capp_tasks_processed_total`.
//! ```

#![cfg(feature = "observability")]

use std::{sync::Arc, time::Duration};

use anyhow::anyhow;
use capp::{
    manager::{
        MailboxConfig, ServiceRequest, ServiceStackOptions, build_service_stack,
        spawn_mailbox_runtime,
    },
    observability,
    queue::{InMemoryTaskQueue, JsonSerializer, Task},
    tracing, tracing_subscriber,
};
use rand::{Rng, rng};
use tokio::sync::mpsc;
use tower::{BoxError, service_fn};

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct DemoTask {
    id: u32,
    delay_ms: u64,
}

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    tracing_subscriber::fmt::init();

    let endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok();
    let metrics =
        observability::init_metrics("capp-mailbox-example", endpoint.as_deref())?;

    let task_count = 20u32;
    let queue = Arc::new(InMemoryTaskQueue::<DemoTask, JsonSerializer>::new());
    let ctx = Arc::new(());

    let (done_tx, mut done_rx) = mpsc::channel::<DemoTask>(task_count as usize);

    // Simple worker: sleeps, fails sometimes to exercise metrics, then reports completion.
    let base_service = {
        let done_tx = done_tx.clone();
        service_fn(move |req: ServiceRequest<DemoTask, ()>| {
            let done_tx = done_tx.clone();
            async move {
                let should_fail =
                    req.task.payload.id.is_multiple_of(5) && req.attempt <= 2;
                let delay = Duration::from_millis(req.task.payload.delay_ms);
                tracing::info!(
                    task_id = req.task.payload.id,
                    attempt = req.attempt,
                    worker_id = req.worker_id,
                    delay_ms = req.task.payload.delay_ms,
                    should_fail,
                    "processing task"
                );
                tokio::time::sleep(delay).await;
                if should_fail {
                    return Err(anyhow!("simulated failure")).map_err(Into::into);
                }
                let _ = done_tx.send(req.task.payload).await;
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

    // Enqueue tasks with small random delays.
    let mut rng = rng();
    for id in 0..task_count {
        let delay_ms = rng.random_range(100..800);
        runtime
            .producer
            .enqueue(Task::new(DemoTask { id, delay_ms }))
            .await
            .expect("producer channel open");
    }

    // Wait for completions or stop after a short timeout.
    let mut completed = 0usize;
    let mut dlq = 0u64;
    let mut stats_rx = runtime.stats.clone();
    let wait = tokio::time::timeout(Duration::from_secs(10), async {
        while completed + (dlq as usize) < task_count as usize {
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
                        dlq = stats_rx.borrow().terminal_failures;
                    }
                }
            }
        }
    })
    .await;

    tracing::info!(?wait, completed, dlq, "run finished");
    metrics.shutdown();
    runtime.shutdown().await;
    Ok(())
}
