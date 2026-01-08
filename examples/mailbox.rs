use std::{sync::Arc, time::Duration};

use anyhow::anyhow;
use capp::{
    manager::{
        ControlCommand, MailboxConfig, ServiceRequest, ServiceStackOptions,
        build_service_stack, spawn_mailbox_runtime,
    },
    queue::{InMemoryTaskQueue, JsonSerializer, Task},
    tracing, tracing_subscriber,
};
use rand::{Rng, rng};
use tokio::{signal, sync::mpsc};
use tower::{BoxError, service_fn};

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct DemoTask {
    id: u32,
    sleep_ms: u64,
}

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    tracing_subscriber::fmt::init();

    #[cfg(feature = "observability")]
    let _metrics = {
        let endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok();
        capp::observability::init_metrics(
            "capp-mailbox-example",
            endpoint.as_deref(),
        )?
    };

    let task_count = 100u32;
    let queue = Arc::new(InMemoryTaskQueue::<DemoTask, JsonSerializer>::new());
    let ctx = Arc::new(());

    let (done_tx, mut done_rx) = mpsc::channel::<DemoTask>(task_count as usize);

    // User function wrapped as a tower service. It sleeps for a random delay,
    // randomly fails, and reports completion via a channel so we can await all tasks.
    // Delays are intentionally large so the run lasts a few minutes.
    let base_service = {
        let done_tx = done_tx.clone();
        service_fn(move |req: ServiceRequest<DemoTask, ()>| {
            let done_tx = done_tx.clone();
            async move {
                let delay = Duration::from_millis(req.task.payload.sleep_ms);
                let stubborn = req.task.payload.id.is_multiple_of(15);
                let random_fail = req.task.payload.id.is_multiple_of(5);
                let should_fail = (stubborn && req.attempt <= 3) || random_fail;
                tracing::info!(
                    task_id = req.task.payload.id,
                    attempt = req.attempt,
                    worker_id = req.worker_id,
                    stubborn,
                    delay_ms = req.task.payload.sleep_ms,
                    "processing task"
                );
                tokio::time::sleep(delay).await;

                if should_fail && req.attempt <= 3 {
                    tracing::warn!(
                        task_id = req.task.payload.id,
                        attempt = req.attempt,
                        worker_id = req.worker_id,
                        "simulated failure, will retry if under threshold"
                    );
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
            timeout: Some(Duration::from_secs(15)),
            buffer: 8,
            concurrency_limit: Some(4),
        },
    );

    // Spin up dispatcher + mailbox workers.
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

    // Log stats as workers progress.
    let mut stats_rx = runtime.stats.clone();
    let mut stats_for_done = runtime.stats.clone();
    let stats_handle = tokio::spawn(async move {
        while stats_rx.changed().await.is_ok() {
            let snapshot = stats_rx.borrow().clone();
            if snapshot.workers.is_empty() {
                continue;
            }
            let total: u64 = snapshot.workers.values().map(|w| w.processed).sum();
            let successes: u64 =
                snapshot.workers.values().map(|w| w.succeeded).sum();
            let dlq = snapshot.terminal_failures;
            tracing::info!(processed = total, succeeded = successes, dlq, "stats");
        }
    });

    // Handle Ctrl+C: first press triggers graceful stop (drain in-flight tasks),
    // second press forces exit.
    let control_tx = runtime.control.clone();
    let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel::<()>();
    let mut stop_tx = Some(stop_tx);
    tokio::spawn(async move {
        let mut count = 0usize;
        loop {
            if signal::ctrl_c().await.is_err() {
                break;
            }
            count += 1;
            if count == 1 {
                tracing::warn!("Ctrl+C received, requesting graceful stop");
                let _ = control_tx.send(ControlCommand::Stop);
                if let Some(tx) = stop_tx.take() {
                    let _ = tx.send(());
                }
            } else {
                tracing::error!("Second Ctrl+C received, forcing exit");
                std::process::exit(130);
            }
        }
    });

    // Enqueue tasks with random delays.
    let mut rng = rng();
    for id in 0..task_count {
        let sleep_ms = rng.random_range(200..1_000);
        runtime
            .producer
            .enqueue(Task::new(DemoTask { id, sleep_ms }))
            .await
            .expect("producer channel open");
    }

    // Wait for all tasks to report completion.
    let mut completed = 0usize;
    let mut dlq = 0u64;
    while completed + (dlq as usize) < task_count as usize {
        tokio::select! {
            _ = &mut stop_rx => {
                tracing::info!(completed, dlq, "graceful stop requested; exiting receive loop");
                break;
            }
            maybe = done_rx.recv() => {
                if let Some(done) = maybe {
                    completed += 1;
                    tracing::debug!(task_id = done.id, completed, dlq, "task complete");
                } else {
                    break;
                }
            }
            changed = stats_for_done.changed() => {
                if changed.is_ok() {
                    dlq = stats_for_done.borrow().terminal_failures;
                }
            }
        }
    }

    runtime.shutdown().await;
    let _ = stats_handle.await;
    tracing::info!("all tasks processed, shutting down");
    Ok(())
}
