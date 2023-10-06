use crate::config::Configurable;
use crate::executor::processor::TaskProcessor;
use crate::task_deport::{Task, TaskStorage};
use derive_builder::Builder;
use serde::{de::DeserializeOwned, Serialize};
use std::sync::{
    atomic::{AtomicU32, Ordering},
    Arc,
};
use tracing;

#[derive(Clone, Default)]
pub struct WorkerOptions {
    pub max_retries: u32,
    pub no_task_found_delay_sec: u64,
}

#[derive(Builder, Default, Clone)]
#[builder(public, setter(into))]
pub struct ExecutorOptions {
    #[builder(
        default = "WorkerOptions { max_retries: 3, no_task_found_delay_sec: 10 }"
    )]
    pub worker_options: WorkerOptions,
    #[builder(default = "None")]
    pub task_limit: Option<u32>,
    #[builder(default = "4")]
    pub concurrency_limit: usize,
    #[builder(default = "10")]
    pub no_task_found_delay_sec: usize,
}

/// A worker function that fetches a task from the storage, processes it,
/// and then updates the task status. If the processing fails,
/// the task is retried up to N times.
async fn worker<D, PE, SE, P, S, C>(
    worker_id: usize,
    ctx: Arc<C>,
    storage: Arc<S>,
    processor: Arc<P>,
    worker_options: &WorkerOptions,
) where
    D: Clone
        + Serialize
        + DeserializeOwned
        + Send
        + Sync
        + 'static
        + std::fmt::Debug,
    PE: std::error::Error + Send + Sync + 'static,
    SE: std::error::Error + Send + Sync + 'static,
    P: TaskProcessor<D, PE, S, C> + Send + Sync + 'static,
    S: TaskStorage<D, SE> + Send + Sync + 'static,
    C: Configurable + Send + Sync + 'static,
{
    let task: Option<Task<D>> = storage.task_pop().await.unwrap();
    if let Some(mut t) = task {
        t.set_in_process();
        match processor
            .process(worker_id, ctx, storage.clone(), &mut t)
            .await
        {
            Ok(_) => {
                t.set_succeed();
                storage.task_set(&t).await.unwrap();
                let successful_task = storage.task_ack(&t.task_id).await.unwrap();
                tracing::info!(
                    "[worker-{}] Task {} succeed: {:?}",
                    worker_id,
                    &successful_task.task_id,
                    &successful_task.payload
                );
            }
            Err(err) => {
                t.set_retry(&err.to_string());
                if t.retries < worker_options.max_retries {
                    storage.task_push(&t).await.unwrap();

                    tracing::error!(
                        "[worker-{}] Task {} failed, retrying ({}): {:?}",
                        worker_id,
                        &t.task_id,
                        &t.retries,
                        &err
                    );
                } else {
                    // TODO: send to dlq
                    let x: u64 = 10;
                    tracing::error!(
                        "[worker-{}] Task {} failed, exceed retyring attempts ({}): {:?}",
                        worker_id, &t.task_id, &t.retries, &err
                    );
                }
            }
        }
    } else {
        tracing::warn!("[worker-{}] No tasks found, waiting...", worker_id);
        tokio::time::sleep(tokio::time::Duration::from_secs(
            worker_options.no_task_found_delay_sec,
        ))
        .await;
    }
}

/// A wrapper function for the worker function that also checks for task
/// limits and handles shutdown signals.
async fn worker_wrapper<D, PE, SE, P, S, C>(
    worker_id: usize,
    ctx: Arc<C>,
    storage: Arc<S>,
    processor: Arc<P>,
    task_counter: Arc<AtomicU32>,
    task_limit: Option<u32>,
    limit_notify: Arc<tokio::sync::Notify>,
    mut shutdown: tokio::sync::watch::Receiver<()>,
    worker_options: WorkerOptions,
) where
    D: Clone
        + Serialize
        + DeserializeOwned
        + Send
        + Sync
        + 'static
        + std::fmt::Debug,
    PE: std::error::Error + Send + Sync + 'static,
    SE: std::error::Error + Send + Sync + 'static,
    P: TaskProcessor<D, PE, S, C> + Send + Sync + 'static,
    S: TaskStorage<D, SE> + Send + Sync + 'static,
    C: Configurable + Send + Sync + 'static,
{
    'worker: loop {
        if let Some(limit) = task_limit {
            if task_counter.fetch_add(1, Ordering::SeqCst) >= limit {
                tracing::info!("Max tasks reached: {}", limit);
                limit_notify.notify_one();
                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                break 'worker;
            }
        };

        tokio::select! {
            _ = shutdown.changed() => {
                tracing::info!("[worker-{}] Worker shutting down...", worker_id);
                break 'worker;
            }
            _ = worker(
                worker_id,
                ctx.clone(),
                storage.clone(),
                processor.clone(),
                &worker_options
            ) => {}

        };
    }
    tracing::info!("[worker-{}] finished.", worker_id);
}

/// Runs the executor with the provided task processor, storage, and options.
/// This function creates a number of workers based on the concurrency limit option.
/// It then waits for either a shutdown signal (Ctrl+C) or for the task limit
/// to be reached. In either case, it sends a shutdown signal to all workers
/// and waits for them to finish.
pub async fn run_workers<D, PE, SE, P, S, C>(
    ctx: Arc<C>,
    processor: Arc<P>,
    storage: Arc<S>,
    options: ExecutorOptions,
) where
    D: Clone
        + Serialize
        + DeserializeOwned
        + Send
        + Sync
        + 'static
        + std::fmt::Debug,
    PE: Send + Sync + 'static + std::error::Error,
    SE: Send + Sync + 'static + std::error::Error,
    P: TaskProcessor<D, PE, S, C> + Send + Sync + 'static,
    S: TaskStorage<D, SE> + Send + Sync + 'static,
    C: Configurable + Send + Sync + 'static,
{
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(());
    let limit_notify = Arc::new(tokio::sync::Notify::new());
    let task_counter = Arc::new(AtomicU32::new(0));

    let mut workers = Vec::new();

    for i in 1..=options.concurrency_limit {
        workers.push(tokio::spawn(worker_wrapper::<D, PE, SE, P, S, C>(
            i,
            Arc::clone(&ctx),
            Arc::clone(&storage),
            Arc::clone(&processor),
            Arc::clone(&task_counter),
            options.task_limit,
            Arc::clone(&limit_notify),
            shutdown_rx.clone(),
            options.worker_options.clone(),
        )));
    }

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            log::warn!("Ctrl+C received, shutting down...");
            shutdown_tx.send(()).unwrap();
        }
        _ = limit_notify.notified() => {
            log::warn!("Task limit reached, shutting down...");
            shutdown_tx.send(()).unwrap();
        }
    }

    let results = futures::future::join_all(workers).await;
    for result in results {
        if let Err(e) = result {
            eprintln!("Error in one of the worker: {:?}", e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excutor_options_builder() {
        let executor_options = ExecutorOptionsBuilder::default().build().unwrap();
        assert_eq!(executor_options.concurrency_limit, 4);
    }
}
