use super::processor::TaskProcessor;
use crate::config::Configurable;
use crate::executor::stats::WorkerStats;
use serde::{de::DeserializeOwned, Serialize};
use std::sync::{
    atomic::{AtomicU32, Ordering},
    Arc,
};
use task_deport::{Task, TaskStorage};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WorkerId(usize);

#[derive(Clone, Default)]
pub struct WorkerOptions {
    pub max_retries: u32,
    pub no_task_found_delay_sec: u64,
}

#[derive(Clone, Default)]
pub struct WorkerContext<C, P, S> {
    pub context: C,
    pub processor: P,
    pub storage: S,
}

pub struct Worker<D, P, S, C> {
    worker_id: WorkerId,
    ctx: Arc<C>,
    storage: Arc<S>,
    processor: Arc<P>,
    stats: WorkerStats,
    options: WorkerOptions,
    // phantom
    _payload_type: std::marker::PhantomData<D>,
}

/// A worker implementation that fetches a task from the storage, processes it,
/// and then updates the task status. If the processing fails,
/// the task is retried up to N times.
impl<D, P, S, C> Worker<D, P, S, C>
where
    D: std::fmt::Debug
        + Clone
        + Serialize
        + DeserializeOwned
        + Send
        + Sync
        + 'static,
    P: TaskProcessor<D, S, C> + Send + Sync + 'static,
    S: TaskStorage<D> + Send + Sync + 'static,
    C: Configurable + Send + Sync + 'static,
{
    pub fn new(
        worker_id: WorkerId,
        ctx: Arc<C>,
        storage: Arc<S>,
        processor: Arc<P>,
        options: WorkerOptions,
    ) -> Self {
        Self {
            worker_id,
            ctx,
            storage,
            processor,
            options,
            stats: WorkerStats::new(),
            // phantom
            _payload_type: std::marker::PhantomData,
        }
    }

    pub fn get_stats(&self) -> &WorkerStats {
        &self.stats
    }

    pub async fn run(&mut self) {
        let start_time = std::time::Instant::now();
        match self.storage.task_pop().await {
            Ok(mut task) => {
                task.set_in_process();
                let result = self
                    .processor
                    .process(
                        self.worker_id,
                        self.ctx.clone(),
                        self.storage.clone(),
                        &mut task,
                    )
                    .await;
                match result {
                    Ok(_) => {
                        task.set_succeed();
                        self.storage.task_set(&task).await.unwrap();
                        let successful_task =
                            self.storage.task_ack(&task.task_id).await.unwrap();
                        tracing::info!(
                            "[worker-{}] Task {} succeed: {:?}",
                            self.worker_id,
                            &successful_task.task_id,
                            &successful_task.payload
                        );

                        // record stats on success
                        self.stats.record_execution_time(start_time.elapsed());
                        self.stats.record_success();
                    }
                    Err(err) => {
                        task.set_retry(&err.to_string());
                        if task.retries < self.options.max_retries {
                            self.storage.task_push(&task).await.unwrap();
                            tracing::error!(
                                "[worker-{}] Task {} failed, retrying ({}): {:?}",
                                self.worker_id,
                                &task.task_id,
                                &task.retries,
                                &err
                            );
                        } else {
                            task.set_dlq("Max retries");
                            self.storage.task_to_dlq(&task).await.unwrap();
                            tracing::error!(
                                "[worker-{}] Task {} failed, max reties ({}): {:?}",
                                self.worker_id,
                                &task.task_id,
                                &task.retries,
                                &err
                            );
                        }

                        self.stats.record_execution_time(start_time.elapsed());
                        self.stats.record_failure();
                    }
                }
            }
            Err(task_deport::TaskStorageError::StorageIsEmptyError) => {
                tracing::warn!(
                    "[worker-{}] No tasks found, waiting...",
                    self.worker_id
                );
                // wait for a while till try to fetch task
                tokio::time::sleep(tokio::time::Duration::from_secs(
                    self.options.no_task_found_delay_sec,
                ))
                .await;
            }
            Err(_err) => {}
        }
    }

    /*
    pub async fn run_old(&mut self) {
        let start_time = std::time::Instant::now();
        let task: Task<D> = self.storage.task_pop().await;

        t.set_in_process();
        match self
            .processor
            .process(
                self.worker_id,
                self.ctx.clone(),
                self.storage.clone(),
                &mut t,
            )
            .await
        {
            Ok(_) => {
                t.set_succeed();
                self.storage.task_set(&t).await.unwrap();
                let successful_task =
                    self.storage.task_ack(&t.task_id).await.unwrap();
                tracing::info!(
                    "[worker-{}] Task {} succeed: {:?}",
                    self.worker_id,
                    &successful_task.task_id,
                    &successful_task.payload
                );

                // record stats on success
                self.stats.record_execution_time(start_time.elapsed());
                self.stats.record_success();
            }
            Err(err) => {
                t.set_retry(&err.to_string());
                if t.retries < self.options.max_retries {
                    self.storage.task_push(&t).await.unwrap();
                    tracing::error!(
                        "[worker-{}] Task {} failed, retrying ({}): {:?}",
                        self.worker_id,
                        &t.task_id,
                        &t.retries,
                        &err
                    );
                } else {
                    t.set_dlq("Max retries");
                    self.storage.task_to_dlq(&t).await.unwrap();
                    tracing::error!(
                        "[worker-{}] Task {} failed, max retyring attempts ({}): {:?}",
                        self.worker_id, &t.task_id, &t.retries, &err);
                }

                self.stats.record_execution_time(start_time.elapsed());
                self.stats.record_failure();
            }
        }
    }
    */
}

impl WorkerId {
    pub fn new(id: usize) -> Self {
        Self(id)
    }

    pub fn get(&self) -> usize {
        self.0
    }
}

impl std::fmt::Display for WorkerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A wrapper for the worker function that also checks for task
/// limits and handles shutdown signals.
pub async fn worker_wrapper<D, P, S, C>(
    worker_id: WorkerId,
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
    P: TaskProcessor<D, S, C> + Send + Sync + 'static,
    S: TaskStorage<D> + Send + Sync + 'static,
    C: Configurable + Send + Sync + 'static,
{
    let mut worker = Worker::new(
        worker_id,
        ctx.clone(),
        storage.clone(),
        processor.clone(),
        worker_options,
    );
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
                tracing::info!("[worker-{}] shutting down...", worker_id);
                break 'worker;
            }
            _ = worker.run() => {
                tracing::info!("[worker-{}] stats: {:?}", worker_id, worker.get_stats())
            }

        };
    }
    tracing::info!("[worker-{}] completed", worker_id);
}
