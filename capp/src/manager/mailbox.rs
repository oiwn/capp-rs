use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};

use capp_queue::{
    AbstractTaskQueue, ProducerHandle, ProducerMsg, Task, TaskQueueError,
    WorkerResult,
};
#[cfg(feature = "observability")]
use opentelemetry::{
    KeyValue,
    metrics::{Counter, Histogram},
};
use serde::{Serialize, de::DeserializeOwned};
use tokio::sync::{broadcast, mpsc, watch};
use tower::{BoxError, Service, ServiceBuilder, ServiceExt, util::BoxCloneService};
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

/// Defaults favor a tiny shared channel and short backoff when the queue is
/// empty. This keeps backpressure predictable for long-running I/O tasks.
#[derive(Debug, Clone)]
pub struct MailboxConfig {
    pub worker_count: usize,
    /// Per-worker prefetch slots. The dispatcher → workers channel total
    /// capacity is `prefetch_per_worker * worker_count`. With the default
    /// of 1, the in-flight buffer equals the worker count.
    pub prefetch_per_worker: usize,
    pub producer_buffer: usize,
    pub result_buffer: usize,
    pub max_retries: u32,
    pub dequeue_backoff: Duration,
}

impl Default for MailboxConfig {
    fn default() -> Self {
        Self {
            worker_count: 4,
            prefetch_per_worker: 1,
            producer_buffer: 128,
            result_buffer: 128,
            max_retries: 3,
            dequeue_backoff: Duration::from_millis(50),
        }
    }
}

/// Wrap the user's service in a per-call timeout and box it for the worker
/// pool. Concurrency is governed by [`MailboxConfig::worker_count`]; this
/// helper does not add a second concurrency layer. Callers that need
/// per-resource rate limiting should compose their own `ServiceBuilder`
/// chain before passing the service in.
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
    let timeout = options.timeout.unwrap_or_else(|| Duration::from_secs(30));
    let service = service.map_err(|err| err.into());
    let stack = ServiceBuilder::new().timeout(timeout).service(service);
    BoxCloneService::new(stack)
}

#[derive(Debug, Clone)]
pub struct ServiceStackOptions {
    pub timeout: Option<Duration>,
}

impl Default for ServiceStackOptions {
    fn default() -> Self {
        Self {
            timeout: Some(Duration::from_secs(30)),
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

    let channel_capacity = config
        .prefetch_per_worker
        .saturating_mul(worker_count)
        .max(1);
    let (envelope_tx, envelope_rx) =
        async_channel::bounded::<Envelope<Data>>(channel_capacity);

    let mut handles = Vec::with_capacity(worker_count + 1);
    handles.push(tokio::spawn(dispatcher_loop(DispatcherParams {
        queue: queue.clone(),
        control_rx: control_rx_for_dispatcher,
        producer_rx,
        result_rx,
        envelope_tx,
        max_retries: config.max_retries,
        dequeue_backoff: config.dequeue_backoff,
        stats_tx: stats_tx.clone(),
    })));

    for worker_idx in 0..worker_count {
        handles.push(tokio::spawn(worker_loop(WorkerParams {
            worker_id: worker_idx,
            ctx: ctx.clone(),
            envelope_rx: envelope_rx.clone(),
            control_rx: control_tx.subscribe(),
            service: service.clone(),
            result_tx: result_tx.clone(),
            stats_tx: stats_tx.clone(),
            producer: producer.clone(),
        })));
    }
    // Workers each hold a clone; drop the local copy so the channel can
    // fully close once the dispatcher drops its sender on Stop.
    drop(envelope_rx);
    handles.push(stats_handle);

    MailboxRuntime {
        producer,
        control: control_tx,
        stats: stats_watch,
        handles,
    }
}

struct DispatcherParams<D: Clone> {
    queue: AbstractTaskQueue<D>,
    control_rx: broadcast::Receiver<ControlCommand>,
    producer_rx: mpsc::Receiver<ProducerMsg<D>>,
    result_rx: mpsc::Receiver<WorkerResult<D>>,
    envelope_tx: async_channel::Sender<Envelope<D>>,
    max_retries: u32,
    dequeue_backoff: Duration,
    stats_tx: mpsc::Sender<StatsEvent>,
}

#[instrument(skip_all)]
async fn dispatcher_loop<D>(params: DispatcherParams<D>)
where
    D: std::fmt::Debug
        + Clone
        + Serialize
        + DeserializeOwned
        + Send
        + Sync
        + 'static,
{
    let DispatcherParams {
        queue,
        mut control_rx,
        mut producer_rx,
        mut result_rx,
        envelope_tx,
        max_retries,
        dequeue_backoff,
        stats_tx,
    } = params;

    // Recover any tasks left in-flight by a previous process before we start
    // dispatching. Failure is logged but not fatal.
    match queue.recover_inflight().await {
        Ok(0) => {}
        Ok(n) => info!(recovered = n, "recovered in-flight tasks at startup"),
        Err(err) => warn!(?err, "inflight recovery failed; continuing"),
    }

    let mut paused = false;
    let mut stopping = false;

    loop {
        tokio::select! {
            biased;
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
                        envelope_tx.close();
                    }
                }
            }
            msg = producer_rx.recv(), if !stopping => {
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

                        let envelope = Envelope {
                            attempt: task.retries.saturating_add(1),
                            enqueued_at: task.queued_at,
                            task,
                        };

                        if let Err(err) = envelope_tx.send(envelope).await {
                            warn!("failed to dispatch envelope: {err}");
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
            // Skip the set-before-ack: ack removes the row anyway.
            task.set_succeed();
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
    }

    Ok(())
}

struct WorkerParams<D: Clone, Ctx> {
    worker_id: usize,
    ctx: Arc<Ctx>,
    envelope_rx: async_channel::Receiver<Envelope<D>>,
    control_rx: broadcast::Receiver<ControlCommand>,
    service: MailboxService<D, Ctx>,
    result_tx: mpsc::Sender<WorkerResult<D>>,
    stats_tx: mpsc::Sender<StatsEvent>,
    producer: ProducerHandle<D>,
}

async fn worker_loop<D, Ctx>(params: WorkerParams<D, Ctx>)
where
    D: std::fmt::Debug
        + Clone
        + Serialize
        + DeserializeOwned
        + Send
        + Sync
        + 'static,
    Ctx: Send + Sync + 'static,
{
    let WorkerParams {
        worker_id,
        ctx,
        envelope_rx,
        mut control_rx,
        service,
        result_tx,
        stats_tx,
        producer,
    } = params;
    let mut paused = false;

    loop {
        tokio::select! {
            biased;
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
                        info!(worker_id, "worker stopping; channel will drain");
                        // Workers exit when the shared channel reports closed
                        // (sender dropped AND empty). Don't bail mid-flight.
                        paused = false;
                    }
                }
            }
            envelope = envelope_rx.recv(), if !paused => {
                let envelope = match envelope {
                    Ok(envelope) => envelope,
                    Err(_) => {
                        debug!(worker_id, "shared channel closed; worker exiting");
                        break;
                    }
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

        Self {
            processed,
            succeeded,
            failed,
            terminal_failure,
            latency_ms,
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use capp_queue::{InMemoryTaskQueue, JsonSerializer, TaskQueue};
    use serde::{Deserialize, Serialize};
    use std::io;
    use tokio::task::yield_now;
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

    #[tokio::test]
    async fn slow_worker_does_not_block_others() {
        // With the old per-worker round-robin, one slow worker would block
        // the whole pool. With the shared MPMC channel, idle workers pick up
        // whatever the dispatcher just pushed.
        let queue =
            Arc::new(InMemoryTaskQueue::<TestPayload, JsonSerializer>::new());
        let ctx = Arc::new(());

        let (observed_tx, mut observed_rx) = mpsc::channel::<u32>(16);

        let base_service =
            service_fn(move |req: ServiceRequest<TestPayload, ()>| {
                let tx = observed_tx.clone();
                async move {
                    // One slow task; rest are fast.
                    if req.task.payload.value == 0 {
                        tokio::time::sleep(Duration::from_millis(200)).await;
                    }
                    tx.send(req.task.payload.value).await.unwrap();
                    Ok::<(), BoxError>(())
                }
            });

        // With the simplified stack, MailboxConfig.worker_count is the only
        // concurrency knob — no tower semaphore to override.
        let service = build_service_stack(
            base_service,
            ServiceStackOptions {
                timeout: Some(Duration::from_secs(5)),
            },
        );
        let runtime = spawn_mailbox_runtime(
            queue.clone(),
            ctx,
            service,
            MailboxConfig {
                worker_count: 4,
                prefetch_per_worker: 1,
                ..Default::default()
            },
        );

        let started = Instant::now();
        for value in 0..4u32 {
            runtime
                .producer
                .enqueue(Task::new(TestPayload { value }))
                .await
                .unwrap();
        }

        // Fast tasks (values 1..=3) must complete well before the slow one.
        let mut fast_seen = 0;
        while fast_seen < 3 {
            let value = timeout(Duration::from_millis(500), observed_rx.recv())
                .await
                .expect("fast tasks were blocked")
                .expect("observed channel closed");
            if value != 0 {
                fast_seen += 1;
            }
        }
        let elapsed_fast = started.elapsed();
        assert!(
            elapsed_fast < Duration::from_millis(150),
            "fast tasks took {elapsed_fast:?}, expected < 150ms"
        );

        // The slow task eventually completes.
        let slow_value = timeout(Duration::from_millis(500), observed_rx.recv())
            .await
            .expect("slow task missing")
            .expect("observed channel closed");
        assert_eq!(slow_value, 0);

        runtime.shutdown().await;
    }

    #[tokio::test]
    async fn shutdown_does_not_lose_pending_tasks() {
        // Enqueue more tasks than the workers can drain immediately, then
        // shut down. Any tasks that did not run should remain in the queue
        // (queue + inflight); nothing should be silently dropped.
        let queue =
            Arc::new(InMemoryTaskQueue::<TestPayload, JsonSerializer>::new());
        let ctx = Arc::new(());

        let processed = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let processed_clone = processed.clone();
        let base_service =
            service_fn(move |_req: ServiceRequest<TestPayload, ()>| {
                let counter = processed_clone.clone();
                async move {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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

        let total = 16u32;
        for value in 0..total {
            runtime
                .producer
                .enqueue(Task::new(TestPayload { value }))
                .await
                .unwrap();
        }

        // Let some — but not all — tasks process before shutdown.
        tokio::time::sleep(Duration::from_millis(60)).await;
        runtime.shutdown().await;

        let processed_count = processed.load(std::sync::atomic::Ordering::Relaxed);
        let queue_len = queue.list.lock().unwrap().len() as u32;
        let inflight_len = queue.inflight.lock().unwrap().len() as u32;

        // No task is lost: every task is either processed (acked, removed
        // from tasks/inflight) or still tracked in queue/inflight.
        assert_eq!(
            processed_count + queue_len + inflight_len,
            total,
            "processed={processed_count} queue={queue_len} inflight={inflight_len}"
        );
        assert!(
            processed_count < total,
            "expected partial completion at shutdown; processed all {total}"
        );
    }

    #[tokio::test]
    async fn build_service_stack_boxes_errors() {
        let base_service =
            service_fn(|_req: ServiceRequest<TestPayload, ()>| async move {
                Err::<(), io::Error>(io::Error::new(io::ErrorKind::Other, "boom"))
            });
        let service = build_service_stack(
            base_service,
            ServiceStackOptions { timeout: None },
        );
        let (producer, _producer_rx) = ProducerHandle::channel(1);
        let request = ServiceRequest {
            task: Task::new(TestPayload { value: 99 }),
            ctx: Arc::new(()),
            producer,
            attempt: 1,
            worker_id: 0,
        };

        let result = service.clone().oneshot(request).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn handle_producer_msg_enqueues_task() {
        let queue =
            Arc::new(InMemoryTaskQueue::<TestPayload, JsonSerializer>::new());
        let abstract_queue: AbstractTaskQueue<TestPayload> = queue.clone();
        let task = Task::new(TestPayload { value: 10 });

        handle_producer_msg(&abstract_queue, ProducerMsg::Enqueue(task.clone()))
            .await
            .unwrap();

        let popped = queue.pop().await.unwrap();
        assert_eq!(popped.payload.value, task.payload.value);
    }

    #[tokio::test]
    async fn handle_worker_result_ack_removes_task() {
        let queue =
            Arc::new(InMemoryTaskQueue::<TestPayload, JsonSerializer>::new());
        let abstract_queue: AbstractTaskQueue<TestPayload> = queue.clone();
        let (stats_tx, _stats_rx) = mpsc::channel(4);
        let task = Task::new(TestPayload { value: 1 });

        queue.push(&task).await.unwrap();
        handle_worker_result(
            &abstract_queue,
            WorkerResult::Ack { task: task.clone() },
            1,
            &stats_tx,
        )
        .await
        .unwrap();

        assert!(queue.hashmap.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn handle_worker_result_retry_requeues_task() {
        let queue =
            Arc::new(InMemoryTaskQueue::<TestPayload, JsonSerializer>::new());
        let abstract_queue: AbstractTaskQueue<TestPayload> = queue.clone();
        let (stats_tx, _stats_rx) = mpsc::channel(4);
        let task = Task::new(TestPayload { value: 2 });

        queue.push(&task).await.unwrap();
        handle_worker_result(
            &abstract_queue,
            WorkerResult::Nack {
                task: task.clone(),
                error: "nope".to_string(),
            },
            3,
            &stats_tx,
        )
        .await
        .unwrap();

        assert!(queue.hashmap.lock().unwrap().contains_key(&task.task_id));
    }

    #[tokio::test]
    async fn handle_worker_result_dlq_emits_terminal_failure() {
        let queue =
            Arc::new(InMemoryTaskQueue::<TestPayload, JsonSerializer>::new());
        let abstract_queue: AbstractTaskQueue<TestPayload> = queue.clone();
        let (stats_tx, mut stats_rx) = mpsc::channel(4);
        let mut task = Task::new(TestPayload { value: 3 });
        task.retries = 3;

        queue.push(&task).await.unwrap();
        handle_worker_result(
            &abstract_queue,
            WorkerResult::Nack {
                task: task.clone(),
                error: "boom".to_string(),
            },
            3,
            &stats_tx,
        )
        .await
        .unwrap();

        assert!(queue.dlq.lock().unwrap().contains_key(&task.task_id));
        assert!(matches!(
            stats_rx.recv().await,
            Some(StatsEvent::TerminalFailure)
        ));
    }

    #[tokio::test]
    async fn stats_collector_tracks_failures() {
        let (stats_tx, stats_rx) = mpsc::channel(8);
        let (mut stats_watch, handle) = spawn_stats_collector(stats_rx);

        stats_tx
            .send(StatsEvent::Worker {
                worker_id: 1,
                success: false,
                latency: Duration::from_millis(5),
            })
            .await
            .unwrap();
        stats_tx.send(StatsEvent::TerminalFailure).await.unwrap();

        timeout(Duration::from_secs(1), stats_watch.changed())
            .await
            .unwrap()
            .unwrap();
        let snapshot = stats_watch.borrow().clone();
        let worker = snapshot.workers.get(&1).unwrap();
        assert_eq!(worker.failed, 1);
        assert_eq!(snapshot.terminal_failures, 1);

        drop(stats_tx);
        let _ = handle.await;
    }

    #[tokio::test]
    async fn worker_loop_reports_failure() {
        let (envelope_tx, envelope_rx) = async_channel::bounded(2);
        let (control_tx, control_rx) = broadcast::channel(4);
        let (result_tx, mut result_rx) = mpsc::channel(4);
        let (stats_tx, mut stats_rx) = mpsc::channel(4);
        let (producer, _producer_rx) = ProducerHandle::channel(1);

        let base_service =
            service_fn(|_req: ServiceRequest<TestPayload, ()>| async move {
                Err::<(), BoxError>(
                    io::Error::new(io::ErrorKind::Other, "fail").into(),
                )
            });
        let service = build_service_stack(
            base_service,
            ServiceStackOptions { timeout: None },
        );

        let handle = tokio::spawn(worker_loop(WorkerParams {
            worker_id: 0,
            ctx: Arc::new(()),
            envelope_rx,
            control_rx,
            service,
            result_tx,
            stats_tx,
            producer,
        }));

        envelope_tx
            .send(Envelope {
                task: Task::new(TestPayload { value: 42 }),
                enqueued_at: std::time::SystemTime::now(),
                attempt: 1,
            })
            .await
            .unwrap();

        let result = timeout(Duration::from_secs(1), result_rx.recv())
            .await
            .unwrap();
        assert!(matches!(result, Some(WorkerResult::Nack { .. })));
        let stats = timeout(Duration::from_secs(1), stats_rx.recv())
            .await
            .unwrap();
        assert!(matches!(
            stats,
            Some(StatsEvent::Worker { success: false, .. })
        ));

        let _ = control_tx.send(ControlCommand::Stop);
        envelope_tx.close();
        let _ = handle.await;
    }

    #[tokio::test]
    async fn worker_loop_resumes_after_pause_and_exits_on_close() {
        // Pause halts envelope intake; Resume restores it; closing the shared
        // channel causes the worker to exit cleanly.
        let (envelope_tx, envelope_rx) = async_channel::bounded(2);
        let (control_tx, control_rx) = broadcast::channel(4);
        let (result_tx, mut result_rx) = mpsc::channel(4);
        let (stats_tx, _stats_rx) = mpsc::channel(4);
        let (producer, _producer_rx) = ProducerHandle::channel(1);

        let base_service =
            service_fn(|_req: ServiceRequest<TestPayload, ()>| async move {
                Ok::<(), BoxError>(())
            });
        let service =
            build_service_stack(base_service, ServiceStackOptions::default());

        let handle = tokio::spawn(worker_loop(WorkerParams {
            worker_id: 1,
            ctx: Arc::new(()),
            envelope_rx,
            control_rx,
            service,
            result_tx,
            stats_tx,
            producer,
        }));

        let _ = control_tx.send(ControlCommand::Pause);
        yield_now().await;

        envelope_tx
            .send(Envelope {
                task: Task::new(TestPayload { value: 7 }),
                enqueued_at: std::time::SystemTime::now(),
                attempt: 1,
            })
            .await
            .unwrap();

        // While paused, no result should arrive.
        assert!(
            timeout(Duration::from_millis(50), result_rx.recv())
                .await
                .is_err()
        );

        let _ = control_tx.send(ControlCommand::Resume);
        let result = timeout(Duration::from_secs(1), result_rx.recv())
            .await
            .unwrap();
        assert!(matches!(result, Some(WorkerResult::Ack { .. })));

        // Closing the shared channel makes the worker exit.
        envelope_tx.close();
        let _ = handle.await;
    }

    #[tokio::test]
    async fn worker_loop_exits_on_closed_channel() {
        let (envelope_tx, envelope_rx) = async_channel::bounded(1);
        let (_control_tx, control_rx) = broadcast::channel(4);
        let (result_tx, _result_rx) = mpsc::channel(1);
        let (stats_tx, _stats_rx) = mpsc::channel(1);
        let (producer, _producer_rx) = ProducerHandle::channel(1);

        envelope_tx.close();
        drop(envelope_tx);

        let base_service =
            service_fn(|_req: ServiceRequest<TestPayload, ()>| async move {
                Ok::<(), BoxError>(())
            });
        let service =
            build_service_stack(base_service, ServiceStackOptions::default());

        let handle = tokio::spawn(worker_loop(WorkerParams {
            worker_id: 2,
            ctx: Arc::new(()),
            envelope_rx,
            control_rx,
            service,
            result_tx,
            stats_tx,
            producer,
        }));

        let _ = handle.await;
    }

    #[cfg(feature = "observability")]
    #[tokio::test]
    async fn observability_metrics_smoke() {
        let metrics = ObservabilityMetrics::new();
        metrics.record_worker_event(1, true, Duration::from_millis(3));
    }
}
