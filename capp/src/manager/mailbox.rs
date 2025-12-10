use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};

use capp_queue::{
    AbstractTaskQueue, ProducerHandle, ProducerMsg, Task, TaskQueueError,
    TaskStatus, WorkerResult,
};
#[cfg(feature = "observability")]
use opentelemetry::{
    KeyValue,
    metrics::{Counter, Histogram, ObservableGauge},
};
use serde::{Serialize, de::DeserializeOwned};
use tokio::sync::{broadcast, mpsc, watch};
use tower::{
    BoxError, Service, ServiceBuilder, ServiceExt, limit::ConcurrencyLimitLayer,
    util::BoxCloneService,
};
use tracing::{debug, error, info, instrument, trace, warn};

pub type MailboxService<D, Ctx> =
    BoxCloneService<ServiceRequest<D, Ctx>, (), BoxError>;

/// Request passed into the tower service stack per task.
#[derive(Clone, Debug)]
pub struct ServiceRequest<D: Clone, Ctx> {
    pub task: Task<D>,
    pub ctx: Arc<Ctx>,
    pub producer: ProducerHandle<D>,
    pub attempt: u32,
    pub worker_id: usize,
}

/// Control messages broadcast to dispatcher/workers.
#[derive(Clone, Debug)]
pub enum ControlCommand {
    Pause,
    Resume,
    Stop,
}

/// Envelope sent from dispatcher into worker inboxes.
#[derive(Clone, Debug)]
pub struct Envelope<D: Clone> {
    pub task: Task<D>,
    pub enqueued_at: SystemTime,
    pub attempt: u32,
}

/// Basic stats snapshot for mailbox runtime.
#[derive(Clone, Debug, Default, Serialize)]
pub struct StatsSnapshot {
    pub workers: HashMap<usize, WorkerMetrics>,
    pub queue_depth: Option<usize>,
    pub terminal_failures: u64,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct WorkerMetrics {
    pub processed: u64,
    pub succeeded: u64,
    pub failed: u64,
    pub last_latency: Option<Duration>,
}

#[derive(Debug)]
enum StatsEvent {
    Worker {
        worker_id: usize,
        success: bool,
        latency: Duration,
    },
    TerminalFailure,
    QueueDepth(usize),
}

/// Handle returned by [`spawn_mailbox_runtime`] to let callers enqueue tasks,
/// send control commands, and observe stats.
pub struct MailboxRuntime<D: Clone> {
    pub producer: ProducerHandle<D>,
    pub control: broadcast::Sender<ControlCommand>,
    pub stats: watch::Receiver<StatsSnapshot>,
    handles: Vec<tokio::task::JoinHandle<()>>,
}

impl<D: Clone> MailboxRuntime<D> {
    /// Stop the dispatcher/workers via broadcast and wait for the tasks to exit.
    pub async fn shutdown(self) {
        let _ = self.control.send(ControlCommand::Stop);
        for handle in self.handles {
            let _ = handle.await;
        }
    }
}

/// Defaults favor tiny bounded inboxes and short backoff when the queue is
/// empty. This keeps backpressure predictable for long-running I/O tasks.
#[derive(Debug, Clone)]
pub struct MailboxConfig {
    pub worker_count: usize,
    pub inbox_capacity: usize,
    pub producer_buffer: usize,
    pub result_buffer: usize,
    pub max_retries: u32,
    pub dequeue_backoff: Duration,
}

impl Default for MailboxConfig {
    fn default() -> Self {
        Self {
            worker_count: 4,
            inbox_capacity: 1,
            producer_buffer: 128,
            result_buffer: 128,
            max_retries: 3,
            dequeue_backoff: Duration::from_millis(50),
        }
    }
}

/// Build a boxed, cloneable tower service with common layers for the worker
/// pipeline (load-shed, buffer, optional timeout, and concurrency limit).
pub fn build_service_stack<D, Ctx, S, E>(
    service: S,
    options: ServiceStackOptions,
) -> MailboxService<D, Ctx>
where
    S: Service<ServiceRequest<D, Ctx>, Response = (), Error = E>
        + Clone
        + Send
        + 'static,
    S::Future: Send + 'static,
    E: Into<BoxError> + Send + Sync + 'static,
    D: Clone + Send + 'static,
    Ctx: Send + Sync + 'static,
{
    let concurrency_limit = options.concurrency_limit.unwrap_or(usize::MAX);
    let timeout = options.timeout.unwrap_or_else(|| Duration::from_secs(30));

    let service = service.map_err(|err| err.into());

    let builder = ServiceBuilder::new()
        .layer(ConcurrencyLimitLayer::new(concurrency_limit))
        .load_shed()
        .buffer(options.buffer)
        .timeout(timeout);

    BoxCloneService::new(builder.service(service))
}

#[derive(Debug, Clone)]
pub struct ServiceStackOptions {
    pub timeout: Option<Duration>,
    pub buffer: usize,
    pub concurrency_limit: Option<usize>,
}

impl Default for ServiceStackOptions {
    fn default() -> Self {
        Self {
            timeout: Some(Duration::from_secs(30)),
            buffer: 1,
            concurrency_limit: Some(1),
        }
    }
}

/// Spawn a dispatcher + mailbox workers running on a tower service stack.
/// This leaves the legacy polling workers untouched while enabling the new
/// v0.6 flow in parallel.
#[allow(clippy::too_many_arguments)]
pub fn spawn_mailbox_runtime<Data, Ctx>(
    queue: AbstractTaskQueue<Data>,
    ctx: Arc<Ctx>,
    service: MailboxService<Data, Ctx>,
    config: MailboxConfig,
) -> MailboxRuntime<Data>
where
    Data: std::fmt::Debug
        + Clone
        + Serialize
        + DeserializeOwned
        + Send
        + Sync
        + 'static,
    Ctx: Send + Sync + 'static,
{
    let worker_count = config.worker_count.max(1);

    let (producer, producer_rx) = ProducerHandle::channel(config.producer_buffer);
    let (result_tx, result_rx) =
        mpsc::channel::<WorkerResult<Data>>(config.result_buffer);
    let (stats_tx, stats_rx) = mpsc::channel::<StatsEvent>(worker_count * 4);
    let (stats_watch, stats_handle) = spawn_stats_collector(stats_rx);

    let (control_tx, control_rx_for_dispatcher) =
        broadcast::channel::<ControlCommand>(16);

    let mut inbox_senders = Vec::with_capacity(worker_count);
    let mut inbox_receivers = Vec::with_capacity(worker_count);
    for _ in 0..worker_count {
        let (tx, rx) = mpsc::channel::<Envelope<Data>>(config.inbox_capacity);
        inbox_senders.push(tx);
        inbox_receivers.push(rx);
    }

    let mut handles = Vec::with_capacity(worker_count + 1);
    handles.push(tokio::spawn(dispatcher_loop(
        queue.clone(),
        control_rx_for_dispatcher,
        producer_rx,
        result_rx,
        inbox_senders,
        config.max_retries,
        config.dequeue_backoff,
        stats_tx.clone(),
    )));

    for (worker_idx, inbox) in inbox_receivers.into_iter().enumerate() {
        handles.push(tokio::spawn(worker_loop(
            worker_idx,
            ctx.clone(),
            inbox,
            control_tx.subscribe(),
            service.clone(),
            result_tx.clone(),
            stats_tx.clone(),
            producer.clone(),
        )));
    }
    handles.push(stats_handle);

    MailboxRuntime {
        producer,
        control: control_tx,
        stats: stats_watch,
        handles,
    }
}

#[instrument(skip_all)]
async fn dispatcher_loop<D>(
    queue: AbstractTaskQueue<D>,
    mut control_rx: broadcast::Receiver<ControlCommand>,
    mut producer_rx: mpsc::Receiver<ProducerMsg<D>>,
    mut result_rx: mpsc::Receiver<WorkerResult<D>>,
    mut inboxes: Vec<mpsc::Sender<Envelope<D>>>,
    max_retries: u32,
    dequeue_backoff: Duration,
    stats_tx: mpsc::Sender<StatsEvent>,
) where
    D: std::fmt::Debug
        + Clone
        + Serialize
        + DeserializeOwned
        + Send
        + Sync
        + 'static,
{
    let mut paused = false;
    let mut stopping = false;
    let mut next_worker = 0usize;

    loop {
        tokio::select! {
            control = control_rx.recv() => {
                match control {
                    Ok(ControlCommand::Pause) => {
                        paused = true;
                        trace!("dispatcher paused");
                    }
                    Ok(ControlCommand::Resume) => {
                        paused = false;
                        trace!("dispatcher resumed");
                    }
                    Ok(ControlCommand::Stop) | Err(_) => {
                        info!("dispatcher stopping (graceful)");
                        stopping = true;
                        paused = true;
                        producer_rx.close();
                        inboxes.clear();
                    }
                }
            }
            msg = producer_rx.recv() => {
                if stopping {
                    continue;
                }
                if let Some(msg) = msg {
                    if let Err(err) = handle_producer_msg(&queue, msg).await {
                        warn!("failed to enqueue task from producer: {err:?}");
                    }
                } else {
                    trace!("producer channel closed");
                }
            }
            result = result_rx.recv() => {
                match result {
                    Some(result) => {
                        if let Err(err) = handle_worker_result(
                            &queue,
                            result,
                            max_retries,
                            &stats_tx,
                        ).await {
                            error!("failed to handle worker result: {err:?}");
                        }
                    }
                    None => {
                        if stopping {
                            trace!("dispatcher result channel closed; exiting");
                            break;
                        }
                    }
                }
            }
            pop_result = queue.pop(), if !paused && !stopping => {
                match pop_result {
                    Ok(mut task) => {
                        task.set_in_progress();
                        if let Err(err) = queue.set(&task).await {
                            warn!("unable to mark task in-progress: {err:?}");
                            continue;
                        }

                        let worker_idx = next_worker % inboxes.len();
                        next_worker = (next_worker + 1) % inboxes.len();
                        let envelope = Envelope {
                            attempt: task.retries.saturating_add(1),
                            enqueued_at: task.queued_at,
                            task,
                        };

                        if let Err(err) = inboxes[worker_idx].send(envelope).await {
                            warn!("failed to send task to inbox {worker_idx}: {err}");
                        }
                    }
                    Err(TaskQueueError::QueueEmpty) => {
                        tokio::time::sleep(dequeue_backoff).await;
                    }
                    Err(err) => {
                        warn!("pop error: {err:?}");
                        tokio::time::sleep(dequeue_backoff).await;
                    }
                }
            }
        }

        if stopping && result_rx.is_closed() {
            trace!("dispatcher drained inflight tasks; exiting");
            break;
        }
    }

    let _ = stats_tx.send(StatsEvent::QueueDepth(0)).await;
}

async fn handle_producer_msg<D>(
    queue: &AbstractTaskQueue<D>,
    msg: ProducerMsg<D>,
) -> Result<(), TaskQueueError>
where
    D: std::fmt::Debug
        + Clone
        + Serialize
        + DeserializeOwned
        + Send
        + Sync
        + 'static,
{
    match msg {
        ProducerMsg::Enqueue(task) => queue.push(&task).await?,
    }
    Ok(())
}

async fn handle_worker_result<D>(
    queue: &AbstractTaskQueue<D>,
    result: WorkerResult<D>,
    max_retries: u32,
    stats_tx: &mpsc::Sender<StatsEvent>,
) -> Result<(), TaskQueueError>
where
    D: std::fmt::Debug
        + Clone
        + Serialize
        + DeserializeOwned
        + Send
        + Sync
        + 'static,
{
    match result {
        WorkerResult::Ack { mut task } => {
            task.set_succeed();
            queue.set(&task).await?;
            queue.ack(&task.task_id).await?;
        }
        WorkerResult::Nack { mut task, error } => {
            task.set_retry(&error);
            if task.retries <= max_retries {
                queue.push(&task).await?;
            } else {
                task.set_dlq("max retries reached");
                queue.nack(&task).await?;
                let _ = stats_tx.send(StatsEvent::TerminalFailure).await;
            }
        }
        WorkerResult::Return { mut task } => {
            task.set_status(TaskStatus::Queued);
            task.started_at = None;
            task.finished_at = None;
            task.error_msg = None;
            queue.push(&task).await?;
        }
    }

    Ok(())
}

async fn worker_loop<D, Ctx>(
    worker_id: usize,
    ctx: Arc<Ctx>,
    mut inbox: mpsc::Receiver<Envelope<D>>,
    mut control_rx: broadcast::Receiver<ControlCommand>,
    service: MailboxService<D, Ctx>,
    result_tx: mpsc::Sender<WorkerResult<D>>,
    stats_tx: mpsc::Sender<StatsEvent>,
    producer: ProducerHandle<D>,
) where
    D: std::fmt::Debug
        + Clone
        + Serialize
        + DeserializeOwned
        + Send
        + Sync
        + 'static,
    Ctx: Send + Sync + 'static,
{
    let mut paused = false;
    let mut stop_requested = false;

    loop {
        tokio::select! {
            control = control_rx.recv() => {
                match control {
                    Ok(ControlCommand::Pause) => {
                        paused = true;
                        trace!(worker_id, "worker paused");
                    }
                    Ok(ControlCommand::Resume) => {
                        paused = false;
                        trace!(worker_id, "worker resumed");
                    }
                    Ok(ControlCommand::Stop) | Err(_) => {
                        info!(worker_id, "worker stopping after current task");
                        stop_requested = true;
                        paused = true;
                    }
                }
            }
            envelope = inbox.recv(), if !paused => {
                let Some(envelope) = envelope else {
                    debug!(worker_id, "worker inbox closed");
                    break;
                };

                let task = envelope.task;
                let attempt = envelope.attempt;
                let request_task = task.clone();
                let request = ServiceRequest {
                    task: request_task,
                    ctx: ctx.clone(),
                    producer: producer.clone(),
                    attempt,
                    worker_id,
                };

                let started = Instant::now();
                info!(
                    worker_id,
                    task_id = %task.task_id,
                    attempt,
                    "worker executing task"
                );
                match service.clone().oneshot(request).await {
                    Ok(()) => {
                        let _ = result_tx.send(WorkerResult::Ack { task }).await;
                        let _ = stats_tx
                            .send(StatsEvent::Worker {
                                worker_id,
                                success: true,
                                latency: started.elapsed(),
                            })
                            .await;
                    }
                    Err(err) => {
                        let _ = result_tx
                            .send(WorkerResult::Nack {
                                task,
                                error: err.to_string(),
                            })
                            .await;
                        let _ = stats_tx
                            .send(StatsEvent::Worker {
                                worker_id,
                                success: false,
                                latency: started.elapsed(),
                            })
                            .await;
                    }
                }
            }
        }

        if stop_requested {
            while let Ok(env) = inbox.try_recv() {
                let _ = result_tx
                    .send(WorkerResult::Return { task: env.task })
                    .await;
            }
            debug!(worker_id, "stop requested; exiting loop");
            break;
        }
    }
}

fn spawn_stats_collector(
    mut rx: mpsc::Receiver<StatsEvent>,
) -> (watch::Receiver<StatsSnapshot>, tokio::task::JoinHandle<()>) {
    let (tx, rx_watch) = watch::channel(StatsSnapshot::default());

    #[cfg(feature = "observability")]
    let otel = ObservabilityMetrics::new();

    let handle = tokio::spawn(async move {
        let mut snapshot = StatsSnapshot::default();
        while let Some(event) = rx.recv().await {
            match event {
                StatsEvent::Worker {
                    worker_id,
                    success,
                    latency,
                } => {
                    let entry = snapshot.workers.entry(worker_id).or_default();
                    entry.processed += 1;
                    if success {
                        entry.succeeded += 1;
                    } else {
                        entry.failed += 1;
                    }
                    entry.last_latency = Some(latency);
                    #[cfg(feature = "observability")]
                    otel.record_worker_event(worker_id, success, latency);
                }
                StatsEvent::TerminalFailure => {
                    snapshot.terminal_failures += 1;
                    #[cfg(feature = "observability")]
                    otel.terminal_failure.add(1, &[]);
                }
                StatsEvent::QueueDepth(depth) => {
                    snapshot.queue_depth = Some(depth);
                    #[cfg(feature = "observability")]
                    otel.update_queue_depth(depth as u64);
                }
            }

            let _ = tx.send(snapshot.clone());
        }
    });

    (rx_watch, handle)
}

#[cfg(feature = "observability")]
struct ObservabilityMetrics {
    processed: Counter<u64>,
    succeeded: Counter<u64>,
    failed: Counter<u64>,
    terminal_failure: Counter<u64>,
    latency_ms: Histogram<f64>,
    queue_depth: std::sync::Arc<std::sync::atomic::AtomicU64>,
    _queue_depth_inst: ObservableGauge<u64>,
}

#[cfg(feature = "observability")]
impl ObservabilityMetrics {
    fn new() -> Self {
        let meter = opentelemetry::global::meter("capp.mailbox");
        let processed = meter
            .u64_counter("capp_tasks_processed_total")
            .with_description("Total tasks processed by mailbox workers")
            .build();
        let succeeded = meter
            .u64_counter("capp_tasks_succeeded_total")
            .with_description("Tasks processed successfully")
            .build();
        let failed = meter
            .u64_counter("capp_tasks_failed_total")
            .with_description("Tasks processed with error (will retry or DLQ)")
            .build();
        let terminal_failure = meter
            .u64_counter("capp_tasks_terminal_failures_total")
            .with_description("Tasks moved to dead-letter after retries exhausted")
            .build();
        let latency_ms = meter
            .f64_histogram("capp_task_latency_ms")
            .with_unit("ms")
            .with_description("Task execution latency as seen by workers")
            .build();
        let queue_depth = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let gauge_state = queue_depth.clone();
        let queue_depth = meter
            .u64_observable_gauge("capp_queue_depth")
            .with_description("Last observed queue depth (when backend reports)")
            .with_callback(|obs| {
                obs.observe(
                    gauge_state.load(std::sync::atomic::Ordering::Relaxed),
                    &[],
                );
            })
            .build();

        Self {
            processed,
            succeeded,
            failed,
            terminal_failure,
            latency_ms,
            queue_depth: gauge_state,
            _queue_depth_inst: queue_depth,
        }
    }

    fn record_worker_event(
        &self,
        worker_id: usize,
        success: bool,
        latency: Duration,
    ) {
        let worker_attr = &[KeyValue::new("worker_id", worker_id.to_string())];
        self.processed.add(1, worker_attr);
        if success {
            self.succeeded.add(1, worker_attr);
        } else {
            self.failed.add(1, worker_attr);
        }
        self.latency_ms
            .record(latency.as_secs_f64() * 1_000.0, worker_attr);
    }

    fn update_queue_depth(&self, depth: u64) {
        self.queue_depth
            .store(depth, std::sync::atomic::Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use capp_queue::{InMemoryTaskQueue, JsonSerializer};
    use serde::{Deserialize, Serialize};
    use tokio::time::timeout;
    use tower::{BoxError, service_fn};

    #[derive(Clone, Debug, Serialize, Deserialize)]
    struct TestPayload {
        value: u32,
    }

    #[tokio::test]
    async fn dispatcher_and_workers_process_tasks() {
        let queue =
            Arc::new(InMemoryTaskQueue::<TestPayload, JsonSerializer>::new());
        let ctx = Arc::new(());

        let (observed_tx, mut observed_rx) = mpsc::channel::<TestPayload>(8);

        let base_service =
            service_fn(move |req: ServiceRequest<TestPayload, ()>| {
                let tx = observed_tx.clone();
                async move {
                    tx.send(req.task.payload).await.unwrap();
                    tracing::info!(
                        worker_id = req.worker_id,
                        "test worker ran task"
                    );
                    Ok::<(), BoxError>(())
                }
            });

        let service =
            build_service_stack(base_service, ServiceStackOptions::default());
        let runtime = spawn_mailbox_runtime(
            queue.clone(),
            ctx,
            service,
            MailboxConfig {
                worker_count: 2,
                ..Default::default()
            },
        );

        for value in 0..4u32 {
            runtime
                .producer
                .enqueue(Task::new(TestPayload { value }))
                .await
                .unwrap();
        }

        // Wait until all tasks observed or timeout.
        timeout(Duration::from_secs(2), async move {
            let mut seen = Vec::new();
            while let Some(val) = observed_rx.recv().await {
                seen.push(val.value);
                if seen.len() == 4 {
                    break;
                }
            }
        })
        .await
        .expect("tasks not processed in time");

        runtime.shutdown().await;
    }

    #[cfg(feature = "observability")]
    #[tokio::test]
    async fn observability_metrics_smoke() {
        let metrics = ObservabilityMetrics::new();
        metrics.record_worker_event(1, true, Duration::from_millis(3));
        metrics.update_queue_depth(5);
    }
}
