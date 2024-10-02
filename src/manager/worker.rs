use crate::{
    config::Configurable,
    manager::{Computation, WorkerStats},
    queue::{AbstractTaskQueue, TaskQueue, TaskQueueError},
};
use derive_builder::Builder;
use serde::{de::DeserializeOwned, Serialize};
use std::{sync::Arc, time::Duration};
use tokio::sync::{
    broadcast,
    mpsc::{self, error::TryRecvError},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WorkerId(usize);

#[derive(Builder, Default, Clone, Debug)]
#[builder(public, setter(into))]
pub struct WorkerOptions {
    #[builder(default = "3")]
    pub max_retries: u32,
    #[builder(default = "None")]
    pub task_limit: Option<usize>,
    #[builder(default = "std::time::Duration::from_secs(5)")]
    pub no_task_found_delay: Duration,
}

pub enum WorkerCommand {
    Shutdown, // Gracefully shut down worker
}

pub struct Worker<Data, Comp, Ctx> {
    worker_id: WorkerId,
    ctx: Arc<Ctx>,
    queue: AbstractTaskQueue<Data>,
    computation: Arc<Comp>,
    pub stats: WorkerStats,
    pub options: WorkerOptions,
}

/// A worker implementation that fetches a task from the storage, processes it,
/// and then updates the task status. If the processing fails,
/// the task is retried up to N times.
impl<Data, Comp, Ctx> Worker<Data, Comp, Ctx>
where
    Data: std::fmt::Debug
        + Clone
        + Serialize
        + DeserializeOwned
        + Send
        + Sync
        + 'static,
    Comp: Computation<Data, Ctx> + Send + Sync + 'static,
    Ctx: Configurable + Send + Sync + 'static,
{
    pub fn new(
        worker_id: WorkerId,
        ctx: Arc<Ctx>,
        queue: Arc<dyn TaskQueue<Data> + Send + Sync>,
        computation: Arc<Comp>,
        options: WorkerOptions,
    ) -> Self {
        Self {
            worker_id,
            ctx,
            queue,
            computation,
            options,
            stats: WorkerStats::new(),
        }
    }

    pub fn get_stats(&self) -> &WorkerStats {
        &self.stats
    }

    /// Worker run lify-cycle
    /// 1) pop task from queue (or wait a bit)
    /// 2) run computation over task
    /// 3) update task according to computation result
    ///    Return true if should continue or false otherwise
    pub async fn run(&mut self) -> anyhow::Result<bool> {
        // Implement limiting amount of tasks per worker
        if let Some(limit) = self.options.task_limit {
            if self.stats.tasks_processed >= limit {
                tracing::info!(
                    "[{}] task_limit reached: {}",
                    self.worker_id,
                    limit
                );
                return Ok(false);
            }
        };

        let start_time = std::time::Instant::now();
        match self.queue.pop().await {
            Ok(mut task) => {
                task.set_in_progress();
                let result = {
                    self.computation
                        .call(
                            self.worker_id,
                            self.ctx.clone(),
                            self.queue.clone(),
                            &mut task,
                        )
                        .await
                };
                match result {
                    Ok(_) => {
                        task.set_succeed();
                        self.queue.set(&task).await.unwrap();
                        self.queue.ack(&task.task_id).await.unwrap();
                        tracing::info!(
                            "[{}] Task {} succeed: {:?}",
                            self.worker_id,
                            &task.task_id,
                            &task.payload
                        );

                        // record stats on success
                        self.stats.record_execution_time(start_time.elapsed());
                        self.stats.record_success();
                    }
                    Err(err) => {
                        task.set_retry(&err.to_string());
                        if task.retries < self.options.max_retries {
                            self.queue.push(&task).await.unwrap();
                            tracing::error!(
                                "[{}] Task {} failed, retrying ({}): {:?}",
                                self.worker_id,
                                &task.task_id,
                                &task.retries,
                                &err
                            );
                        } else {
                            task.set_dlq("Max retries");
                            self.queue.nack(&task).await.unwrap();
                            tracing::error!(
                                "[{}] Task {} failed, max reties ({}): {:?}",
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
            Err(TaskQueueError::QueueEmpty) => {
                tracing::warn!("[{}] No tasks found, waiting...", self.worker_id);
                // wait for a while till try to fetch task
                tokio::time::sleep(self.options.no_task_found_delay).await;
            }
            Err(_err) => {}
        };
        Ok(true)
    }
}

impl<Data, Comp, Ctx> std::fmt::Debug for Worker<Data, Comp, Ctx> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Worker")
            .field("worker_id", &self.worker_id)
            .field("options", &self.options)
            .field("stats", &self.stats)
            // Optionally, you can add other fields here
            .finish()
    }
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

/// This wrapper used to create new Worker setup internal logging
/// and handle comminications with worker
pub async fn worker_wrapper<Data, Comp, Ctx>(
    worker_id: WorkerId,
    ctx: Arc<Ctx>,
    storage: Arc<dyn TaskQueue<Data> + Send + Sync>,
    computation: Arc<Comp>,
    mut commands: mpsc::Receiver<WorkerCommand>,
    mut terminate: broadcast::Receiver<()>,
    worker_options: WorkerOptions,
) where
    Data: std::fmt::Debug
        + Clone
        + Serialize
        + DeserializeOwned
        + Send
        + Sync
        + 'static,
    Comp: Computation<Data, Ctx> + Send + Sync + 'static,
    Ctx: Configurable + Send + Sync + 'static,
{
    let mut worker = Worker::new(
        worker_id,
        ctx.clone(),
        storage.clone(),
        computation.clone(),
        worker_options,
    );
    let mut should_stop = false;

    'worker: loop {
        tokio::select! {
            _ = terminate.recv() => {
                tracing::info!("Terminating immediately");
                return;
            },
            run_result = worker.run(), if !should_stop => {
                match commands.try_recv() {
                    Ok(WorkerCommand::Shutdown) => {
                        tracing::error!("[{}] Shutdown received", worker_id);
                        should_stop = true;
                    }
                    Err(TryRecvError::Disconnected) => break 'worker,
                    _ => {}

                }
                // If worker ask to shutdown for some reason
                // i.e some amount of tasks finished
                if let Ok(re) = run_result {
                    if !re {
                        return;
                    }
                }
            }
        };

        // If a stop command was received, finish any ongoing work and then exit.
        if should_stop {
            tracing::info!(
                "[{}] Completing current task before stopping.",
                worker_id
            );
            break;
        }
    }

    tracing::info!("completed");
}

/// This wrapper used to create new Worker setup internal logging
/// and handle comminications with worker
pub async fn worker_wrapper_old<Data, Comp, Ctx>(
    worker_id: WorkerId,
    ctx: Arc<Ctx>,
    storage: Arc<dyn TaskQueue<Data> + Send + Sync>,
    computation: Arc<Comp>,
    mut commands: mpsc::Receiver<WorkerCommand>,
    mut terminate: broadcast::Receiver<()>,
    worker_options: WorkerOptions,
) where
    Data: std::fmt::Debug
        + Clone
        + Serialize
        + DeserializeOwned
        + Send
        + Sync
        + 'static,
    Comp: Computation<Data, Ctx> + Send + Sync + 'static,
    Ctx: Configurable + Send + Sync + 'static,
{
    let mut worker = Worker::new(
        worker_id,
        ctx.clone(),
        storage.clone(),
        computation.clone(),
        worker_options,
    );
    let mut should_stop = false;

    // setup spans
    let span = tracing::info_span!("worker", _id = %worker_id);
    let _enter = span.enter();

    'worker: loop {
        tokio::select! {
            _ = terminate.recv() => {
                tracing::info!("Terminating immediately");
                return;
            },
            run_result = worker.run(), if !should_stop => {
                match commands.try_recv() {
                    Ok(WorkerCommand::Shutdown) => {
                        tracing::error!("Shutdown received");
                        should_stop = true;
                    }
                    Err(TryRecvError::Disconnected) => break 'worker,
                    _ => {}

                }
                // If worker ask to shutdown for some reason
                // i.e some amount of tasks finished
                if let Ok(re) = run_result {
                    if !re {
                        return;
                    }
                }
            }
        };

        // If a stop command was received, finish any ongoing work and then exit.
        if should_stop {
            tracing::info!("Completing current task before stopping.",);
            break;
        }
    }

    tracing::info!("completed");
}

#[cfg(test)]
mod tests {
    use std::assert_eq;

    use super::*;

    #[test]
    fn worker_options() {
        let options = WorkerOptionsBuilder::default().build().unwrap();
        assert_eq!(options.max_retries, 3);
        assert_eq!(options.task_limit, None);
        assert_eq!(options.no_task_found_delay, Duration::from_millis(5000));
    }
}
