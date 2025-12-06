#![deny(clippy::unwrap_used)]
#![allow(clippy::collapsible_if)]
use crate::manager::{Computation, WorkerStats};
use capp_config::Configurable;
use capp_queue::{AbstractTaskQueue, Task, TaskQueue, TaskQueueError};
use derive_builder::Builder;
use serde::{Serialize, de::DeserializeOwned};
use std::{sync::Arc, time::Duration};
use tokio::sync::{
    broadcast,
    mpsc::{self, error::TryRecvError},
};
use tracing::{error, info, instrument, warn};

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
    #[builder(default = "std::time::Duration::from_secs(60)")]
    pub computation_timeout: Duration,
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
    ///
    /// Return true if should continue or false otherwise
    pub async fn run(&mut self) -> anyhow::Result<bool> {
        // Implement limiting amount of tasks per worker
        if let Some(limit) = self.options.task_limit {
            if self.stats.tasks_processed >= limit {
                warn!("task_limit reached: {}", limit);
                return Ok(false);
            }
        };

        let start_time = std::time::Instant::now();

        // Try to get a task, handle QueueEmpty specially
        let pop_result = self.queue.pop().await;
        let mut task = match pop_result {
            Ok(task) => task,
            Err(TaskQueueError::QueueEmpty) => {
                warn!("No tasks found, waiting...");
                tokio::time::sleep(self.options.no_task_found_delay).await;
                return Ok(true);
            }
            Err(e) => {
                error!("Error popping task: {:?}", e);
                // For queue errors, wait a bit then try again
                tokio::time::sleep(Duration::from_secs(1)).await;
                return Ok(true);
            }
        };

        // Mark task as in progress
        task.set_in_progress();
        if let Err(e) = self.queue.set(&task).await {
            error!("Failed to update task status: {:?}", e);
            self.handle_error(&mut task, &format!("Queue error: {e}"))
                .await?;
            return Ok(true);
        }

        // Run computation with timeout
        let computation_timeout = self.options.computation_timeout;
        let computation_result = tokio::time::timeout(
            computation_timeout,
            self.computation.call(
                self.worker_id,
                self.ctx.clone(),
                self.queue.clone(),
                &mut task,
            ),
        )
        .await;

        match computation_result {
            // Timeout occurred
            Err(_elapsed) => {
                self.handle_error(&mut task, "Computation timeout").await?;
            }
            // Computation completed but may have errored
            Ok(result) => match result {
                Ok(_) => {
                    // Successful completion
                    task.set_succeed();
                    if let Err(e) = self.queue.set(&task).await {
                        error!("Failed to update successful task: {:?}", e);
                        self.handle_error(&mut task, &format!("Queue error: {e}"))
                            .await?;
                        return Ok(true);
                    }

                    if let Err(e) = self.queue.ack(&task.task_id).await {
                        error!("Failed to ack successful task: {:?}", e);
                        self.handle_error(&mut task, &format!("Queue error: {e}"))
                            .await?;
                        return Ok(true);
                    }

                    info!("Task {} succeed: {:?}", &task.task_id, &task.payload);
                    self.stats.record_execution_time(start_time.elapsed());
                    self.stats.record_success();
                }
                Err(err) => {
                    self.handle_error(&mut task, &err.to_string()).await?;
                }
            },
        }

        Ok(true)
    }

    // Helper method to handle errors uniformly
    async fn handle_error(
        &mut self,
        task: &mut Task<Data>,
        error_msg: &str,
    ) -> anyhow::Result<()> {
        task.set_retry(error_msg);
        self.stats
            .record_execution_time(std::time::Instant::now().elapsed());
        self.stats.record_failure();

        if task.retries < self.options.max_retries {
            if let Err(e) = self.queue.push(task).await {
                error!("Failed to push task for retry: {e:?}");
                // If we can't push for retry, try to send to DLQ
                task.set_dlq(&format!("Failed to retry: {e}"));
                if let Err(e) = self.queue.nack(task).await {
                    error!("Failed to nack task after retry failure: {:?}", e);
                }
            } else {
                error!(
                    "Task {} failed, retrying ({}/{}): {}",
                    &task.task_id,
                    &task.retries,
                    self.options.max_retries,
                    error_msg
                );
            }
        } else {
            task.set_dlq("Max retries exceeded");
            if let Err(e) = self.queue.nack(task).await {
                error!("Failed to nack task: {:?}", e);
            }
            error!(
                "Task {} failed, max retries ({}) exceeded: {}",
                &task.task_id, self.options.max_retries, error_msg
            );
        }

        Ok(())
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
#[instrument(fields(worker_id = %worker_id), skip(ctx, storage, computation, commands, terminate, worker_options))]
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
                info!("Terminating immediately");
                return;
            },
            run_result = worker.run(), if !should_stop => {
                match commands.try_recv() {
                    Ok(WorkerCommand::Shutdown) => {
                        error!("[{}] Shutdown received", worker_id);
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
            info!("[{}] Completing current task before stopping.", worker_id);
            break;
        }
    }

    info!("completed");
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::manager::ComputationError;
    use async_trait::async_trait;
    use capp_queue::{InMemoryTaskQueue, JsonSerializer};
    use serde::{Deserialize, Serialize};
    use std::time::Duration;
    use toml::Value;

    // Test data structures
    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct TestData {
        value: u32,
        should_timeout: bool,
        should_fail: bool,
    }

    struct TestContext {
        config: Value,
    }

    impl Configurable for TestContext {
        fn config(&self) -> &Value {
            &self.config
        }
    }

    struct TestComputation {
        execution_time: Duration,
    }

    #[async_trait]
    impl Computation<TestData, TestContext> for TestComputation {
        async fn call(
            &self,
            _worker_id: WorkerId,
            _ctx: Arc<TestContext>,
            _queue: AbstractTaskQueue<TestData>,
            task: &mut Task<TestData>,
        ) -> Result<(), ComputationError> {
            // Simulate processing time
            tokio::time::sleep(self.execution_time).await;

            if task.payload.should_timeout {
                // This will be caught by the timeout wrapper
                tokio::time::sleep(Duration::from_secs(60)).await;
            }

            if task.payload.should_fail {
                return Err(ComputationError::Function(
                    "Task failed as requested".into(),
                ));
            }

            Ok(())
        }
    }

    fn setup_test_worker() -> (
        Worker<TestData, TestComputation, TestContext>,
        Arc<InMemoryTaskQueue<TestData, JsonSerializer>>,
    ) {
        let ctx = Arc::new(TestContext {
            config: Value::Table(toml::map::Map::new()),
        });
        let queue = Arc::new(InMemoryTaskQueue::<TestData, JsonSerializer>::new());
        let computation = Arc::new(TestComputation {
            execution_time: Duration::from_millis(50),
        });
        let options = WorkerOptionsBuilder::default()
            .max_retries(3u32)
            .computation_timeout(Duration::from_millis(100))
            .build()
            .unwrap();

        let worker =
            Worker::new(WorkerId::new(1), ctx, queue.clone(), computation, options);

        (worker, queue)
    }

    #[tokio::test]
    async fn test_successful_task_processing() {
        let (mut worker, queue) = setup_test_worker();

        // Push a task that should succeed
        let task = Task::new(TestData {
            value: 42,
            should_timeout: false,
            should_fail: false,
        });
        queue.push(&task).await.unwrap();

        // Run the worker
        let result = worker.run().await;
        assert!(result.is_ok());

        // Verify stats
        assert_eq!(worker.stats.tasks_succeeded, 1);
        assert_eq!(worker.stats.tasks_failed, 0);
    }

    #[tokio::test]
    async fn test_task_retry_on_failure() {
        let (mut worker, queue) = setup_test_worker();

        // Push a task that should fail
        let task = Task::new(TestData {
            value: 42,
            should_timeout: false,
            should_fail: true,
        });
        let task_id = task.task_id;
        queue.push(&task).await.unwrap();

        // Run the worker
        let result = worker.run().await;
        assert!(result.is_ok());

        // Verify stats
        assert_eq!(worker.stats.tasks_succeeded, 0);
        assert_eq!(worker.stats.tasks_failed, 1);

        // Verify task was pushed back for retry
        let retried_task = queue.pop().await.unwrap();
        assert_eq!(retried_task.task_id, task_id);
        assert_eq!(retried_task.retries, 1);
    }

    #[tokio::test]
    async fn test_task_timeout() {
        let (mut worker, queue) = setup_test_worker();

        // Push a task that should timeout
        let task = Task::new(TestData {
            value: 42,
            should_timeout: true,
            should_fail: false,
        });
        let task_id = task.task_id;
        queue.push(&task).await.unwrap();

        // Run the worker
        let result = worker.run().await;
        assert!(result.is_ok());

        // Verify stats
        assert_eq!(worker.stats.tasks_succeeded, 0);
        assert_eq!(worker.stats.tasks_failed, 1);

        // Verify task was pushed back for retry
        let retried_task = queue.pop().await.unwrap();
        assert_eq!(retried_task.task_id, task_id);
        assert_eq!(retried_task.retries, 1);
    }

    #[tokio::test]
    async fn test_max_retries_exceeded() {
        let (mut worker, queue) = setup_test_worker();

        // Push a task that will fail
        let task = Task::new(TestData {
            value: 42,
            should_timeout: false,
            should_fail: true,
        });
        // let task_id = task.task_id;
        queue.push(&task).await.unwrap();

        // Run worker multiple times to exceed max retries
        for _ in 0..4 {
            // Original try + 3 retries
            let result = worker.run().await;
            assert!(result.is_ok());
        }

        // Verify stats
        assert_eq!(worker.stats.tasks_succeeded, 0);
        assert_eq!(worker.stats.tasks_failed, 3); // 3 retries

        // Verify task is not in main queue
        assert!(matches!(queue.pop().await, Err(TaskQueueError::QueueEmpty)));

        // Verify task was moved to DLQ (note: would need to add DLQ check functionality)
    }

    #[tokio::test]
    async fn test_task_limit() {
        let ctx = Arc::new(TestContext {
            config: Value::Table(toml::map::Map::new()),
        });
        let queue = Arc::new(InMemoryTaskQueue::<TestData, JsonSerializer>::new());
        let computation = Arc::new(TestComputation {
            execution_time: Duration::from_millis(50),
        });
        let options = WorkerOptionsBuilder::default()
            .max_retries(3u32)
            .task_limit(Some(2))
            .build()
            .unwrap();

        let mut worker =
            Worker::new(WorkerId::new(1), ctx, queue.clone(), computation, options);

        // Push 3 tasks
        for i in 0..3 {
            let task = Task::new(TestData {
                value: i,
                should_timeout: false,
                should_fail: false,
            });
            queue.push(&task).await.unwrap();
        }

        // Run worker until task limit is reached
        let mut runs = 0;
        while let Ok(true) = worker.run().await {
            runs += 1;
        }

        assert_eq!(runs, 2); // Should process exactly 2 tasks
        assert_eq!(worker.stats.tasks_processed, 2);
    }

    #[tokio::test]
    async fn test_empty_queue_handling() {
        let (mut worker, _queue) = setup_test_worker();

        // Run worker with empty queue
        let result = worker.run().await;
        assert!(result.is_ok());

        // Verify stats unchanged
        assert_eq!(worker.stats.tasks_processed, 0);
        assert_eq!(worker.stats.tasks_succeeded, 0);
        assert_eq!(worker.stats.tasks_failed, 0);
    }

    #[test]
    fn worker_options() {
        let options = WorkerOptionsBuilder::default().build().unwrap();
        assert_eq!(options.max_retries, 3);
        assert_eq!(options.task_limit, None);
        assert_eq!(options.no_task_found_delay, Duration::from_millis(5000));
    }
}
