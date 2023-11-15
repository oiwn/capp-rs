use crate::{
    config::Configurable,
    task_deport::{TaskStorage, TaskStorageError},
    AbstractTaskStorage, Computation, WorkerStats,
};
use derive_builder::Builder;
use serde::{de::DeserializeOwned, Serialize};
use std::sync::Arc;
use tokio::sync::mpsc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WorkerId(usize);

#[derive(Builder, Default, Clone, Debug)]
#[builder(public, setter(into))]
pub struct WorkerOptions {
    #[builder(default = "3")]
    pub max_retries: u32,
    #[builder(default = "None")]
    pub task_limit: Option<usize>,
    #[builder(default = "5000")]
    pub no_task_found_delay_ms: u64,
}

pub enum WorkerCommand {
    Stop,      // stop after current task processed
    Terminate, // terminate immediately
}

pub struct Worker<Data, Comp, Ctx> {
    worker_id: WorkerId,
    ctx: Arc<Ctx>,
    storage: AbstractTaskStorage<Data>,
    computation: Arc<Comp>,
    stats: WorkerStats,
    options: WorkerOptions,
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
        storage: Arc<dyn TaskStorage<Data> + Send + Sync>,
        computation: Arc<Comp>,
        options: WorkerOptions,
    ) -> Self {
        Self {
            worker_id,
            ctx,
            storage,
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
    /// Return true if should continue or false otherwise
    pub async fn run(&mut self) -> bool {
        // Implement limiting amount of tasks per worker
        if let Some(limit) = self.options.task_limit {
            if self.stats.tasks_processed >= limit {
                tracing::info!(
                    "[{}] task_limit reached: {}",
                    self.worker_id,
                    limit
                );
                return false;
            }
        };

        let start_time = std::time::Instant::now();
        match self.storage.task_pop().await {
            Ok(mut task) => {
                task.set_in_process();
                let result = self
                    .computation
                    .run(
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
            Err(TaskStorageError::StorageIsEmptyError) => {
                tracing::warn!(
                    "[worker-{}] No tasks found, waiting...",
                    self.worker_id
                );
                // wait for a while till try to fetch task
                tokio::time::sleep(tokio::time::Duration::from_millis(
                    self.options.no_task_found_delay_ms,
                ))
                .await;
            }
            Err(_err) => {}
        };
        true
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

/// A wrapper for the worker function that also checks for task
/// limits and handles shutdown signals.
pub async fn worker_wrapper<Data, Comp, Ctx>(
    worker_id: WorkerId,
    ctx: Arc<Ctx>,
    storage: Arc<dyn TaskStorage<Data> + Send + Sync>,
    computation: Arc<Comp>,
    mut commands: mpsc::Receiver<WorkerCommand>,
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
            biased;

            command = commands.recv() => {
                match command {
                    Some(WorkerCommand::Terminate) => {
                        // Terminate immediately
                        tracing::info!("[worker-{}] terminating immediately.", worker_id);
                        return;
                    }
                    Some(WorkerCommand::Stop) => {
                        // Stop after current work is done, only if not already stopping
                        if !should_stop {
                            tracing::info!("[worker-{}] stopping after current work.", worker_id);
                            should_stop = true;
                        }
                    }
                    None => {
                        // The command channel has closed, we should stop the worker
                        break 'worker;
                    }
                }
            },
            should_continue = worker.run(), if !should_stop => {
                if !should_continue {
                    break 'worker;
                }
                // Normal work execution
            }
        };

        // If a stop command was received, finish any ongoing work and then exit.
        if should_stop {
            tracing::info!(
                "[worker-{}] completing current task before stopping.",
                worker_id
            );
            worker.run().await;
            break;
        }
    }

    tracing::info!("[worker-{}] completed", worker_id);
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
        assert_eq!(options.no_task_found_delay_ms, 5000);
    }
}
