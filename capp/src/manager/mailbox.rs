use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use capp_queue::{
    AbstractTaskQueue, ProducerHandle, ProducerMsg, Task, TaskQueueError,
    WorkerResult,
};
#[cfg(feature = "observability")]
use opentelemetry::metrics::{Counter, Histogram};
use serde::{Serialize, de::DeserializeOwned};
use tokio::sync::{broadcast, mpsc, watch};
use tower::{
    BoxError, Service, ServiceBuilder, ServiceExt, util::BoxService,
};
use tracing::{debug, error, info, instrument, trace, warn};

pub type MailboxService<D, Ctx> =
    BoxService<ServiceRequest<D, Ctx>, (), BoxError>;

/// Request passed into the tower service stack per task.
#[derive(Clone, Debug)]
pub struct ServiceRequest<D: Clone, Ctx> {
    pub task: Task<D>,
    pub ctx: Arc<Ctx>,
    pub producer: ProducerHandle<D>,
    pub attempt: u32,
}

/// Control messages broadcast to the dispatcher.
#[derive(Clone, Debug)]
pub enum ControlCommand {
    Pause,
    Resume,
    Stop,
}

/// Flat stats snapshot. Concurrency is observed externally as `in_flight`.
#[derive(Clone, Debug, Default, Serialize)]
pub struct StatsSnapshot {
    pub processed: u64,
    pub succeeded: u64,
    pub failed: u64,
    pub last_latency: Option<Duration>,
    pub in_flight: u64,
    pub terminal_failures: u64,
}

#[derive(Debug)]
enum StatsEvent {
    Call { success: bool, latency: Duration },
    TerminalFailure,
    InFlight(u64),
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
    /// Stop the dispatcher via broadcast and wait for the tasks to exit.
    pub async fn shutdown(self) {
        let _ = self.control.send(ControlCommand::Stop);
        for handle in self.handles {
            let _ = handle.await;
        }
    }

    /// Wait for the runtime to exit on its own (e.g. with
    /// [`MailboxConfig::stop_when_idle`]). Does not send `Stop`.
    pub async fn join(self) {
        for handle in self.handles {
            let _ = handle.await;
        }
    }
}

/// Defaults favor a tiny shared channel and short backoff when the queue is
/// empty. Concurrency now lives in the tower service stack — add
/// `.concurrency_limit(N)` to your `ServiceBuilder` chain.
#[derive(Debug, Clone)]
pub struct MailboxConfig {
    pub producer_buffer: usize,
    pub result_buffer: usize,
    pub max_retries: u32,
    pub dequeue_backoff: Duration,
    /// Auto-stop the dispatcher once the queue has been observed non-empty
    /// at least once and is now empty with no tasks in flight. Suitable for
    /// finite crawls; leave `false` for long-running processors. The
    /// dispatcher assumes all new enqueues come from service handlers —
    /// external producers that publish after the first idle tick may be
    /// cut off.
    pub stop_when_idle: bool,
}

impl Default for MailboxConfig {
    fn default() -> Self {
        Self {
            producer_buffer: 128,
            result_buffer: 128,
            max_retries: 3,
            dequeue_backoff: Duration::from_millis(50),
            stop_when_idle: false,
        }
    }
}

/// Wrap the user's service in a per-call timeout and box it. Concurrency,
/// rate-limit, retry, etc. should be composed in the caller's
/// `ServiceBuilder` chain before passing the service in.
pub fn build_service_stack<D, Ctx, S, E>(
    service: S,
    options: ServiceStackOptions,
) -> MailboxService<D, Ctx>
where
    S: Service<ServiceRequest<D, Ctx>, Response = (), Error = E>
        + Send
        + 'static,
    S::Future: Send + 'static,
    E: Into<BoxError> + Send + Sync + 'static,
    D: Clone + Send + 'static,
    Ctx: Send + Sync + 'static,
{
    let timeout = options.timeout.unwrap_or_else(|| Duration::from_secs(30));
    let stack = ServiceBuilder::new()
        .timeout(timeout)
        .service(service.map_err(Into::into));
    BoxService::new(stack)
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

/// Spawn a single-service dispatcher driving the supplied tower stack.
/// Concurrency is provided by the service stack itself (typically
/// `ServiceBuilder::new().concurrency_limit(N)…`).
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
    let (producer, producer_rx) =
        ProducerHandle::channel(config.producer_buffer);
    let (result_tx, result_rx) =
        mpsc::channel::<WorkerResult<Data>>(config.result_buffer);
    let (stats_tx, stats_rx) = mpsc::channel::<StatsEvent>(64);
    let (stats_watch, stats_handle) = spawn_stats_collector(stats_rx);

    let (control_tx, control_rx) = broadcast::channel::<ControlCommand>(16);

    let handles = vec![
        tokio::spawn(dispatcher_loop(DispatcherParams {
            queue,
            ctx,
            service,
            producer: producer.clone(),
            control_rx,
            producer_rx,
            result_rx,
            result_tx,
            max_retries: config.max_retries,
            dequeue_backoff: config.dequeue_backoff,
            stop_when_idle: config.stop_when_idle,
            stats_tx,
        })),
        stats_handle,
    ];

    MailboxRuntime {
        producer,
        control: control_tx,
        stats: stats_watch,
        handles,
    }
}

struct DispatcherParams<D: Clone, Ctx> {
    queue: AbstractTaskQueue<D>,
    ctx: Arc<Ctx>,
    service: MailboxService<D, Ctx>,
    producer: ProducerHandle<D>,
    control_rx: broadcast::Receiver<ControlCommand>,
    producer_rx: mpsc::Receiver<ProducerMsg<D>>,
    result_rx: mpsc::Receiver<WorkerResult<D>>,
    result_tx: mpsc::Sender<WorkerResult<D>>,
    max_retries: u32,
    dequeue_backoff: Duration,
    stop_when_idle: bool,
    stats_tx: mpsc::Sender<StatsEvent>,
}

#[instrument(skip_all)]
async fn dispatcher_loop<D, Ctx>(params: DispatcherParams<D, Ctx>)
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
    let DispatcherParams {
        queue,
        ctx,
        mut service,
        producer,
        mut control_rx,
        mut producer_rx,
        mut result_rx,
        result_tx,
        max_retries,
        dequeue_backoff,
        stop_when_idle,
        stats_tx,
    } = params;

    match queue.recover_inflight().await {
        Ok(0) => {}
        Ok(n) => info!(recovered = n, "recovered in-flight tasks at startup"),
        Err(err) => warn!(?err, "inflight recovery failed; continuing"),
    }

    let in_flight = Arc::new(AtomicU64::new(0));
    let mut paused = false;
    let mut stopping = false;
    let mut saw_work = false;

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
                    }
                }
            }
            msg = producer_rx.recv(), if !stopping => {
                if let Some(msg) = msg {
                    if let Err(err) = handle_producer_msg(&queue, msg).await {
                        warn!("failed to enqueue task from producer: {err:?}");
                    } else {
                        saw_work = true;
                    }
                } else {
                    trace!("producer channel closed");
                }
            }
            result = result_rx.recv() => {
                if let Some(result) = result {
                    let n = in_flight.fetch_sub(1, Ordering::SeqCst) - 1;
                    let _ = stats_tx.send(StatsEvent::InFlight(n)).await;
                    if let Err(err) = handle_worker_result(
                        &queue,
                        result,
                        max_retries,
                        &stats_tx,
                    ).await {
                        error!("failed to handle worker result: {err:?}");
                    }
                }
            }
            ready = service.ready(), if !paused && !stopping => {
                let ready_svc = match ready {
                    Ok(svc) => svc,
                    Err(err) => {
                        error!("service.ready() failed: {err:?}");
                        stopping = true;
                        paused = true;
                        producer_rx.close();
                        continue;
                    }
                };
                match queue.pop().await {
                    Ok(mut task) => {
                        saw_work = true;
                        task.set_in_progress();
                        if let Err(err) = queue.set(&task).await {
                            warn!("unable to mark task in-progress: {err:?}");
                            continue;
                        }

                        let attempt = task.retries.saturating_add(1);
                        let request = ServiceRequest {
                            task: task.clone(),
                            ctx: ctx.clone(),
                            producer: producer.clone(),
                            attempt,
                        };

                        let fut = ready_svc.call(request);

                        let n = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                        let _ = stats_tx.send(StatsEvent::InFlight(n)).await;

                        let result_tx = result_tx.clone();
                        let stats_tx = stats_tx.clone();
                        let task_id = task.task_id;
                        let started = Instant::now();
                        debug!(%task_id, attempt, "dispatching task");
                        tokio::spawn(async move {
                            let outcome = fut.await;
                            let success = outcome.is_ok();
                            let _ = stats_tx
                                .send(StatsEvent::Call {
                                    success,
                                    latency: started.elapsed(),
                                })
                                .await;
                            let result = match outcome {
                                Ok(()) => WorkerResult::Ack { task },
                                Err(err) => WorkerResult::Nack {
                                    task,
                                    error: err.to_string(),
                                },
                            };
                            let _ = result_tx.send(result).await;
                        });
                    }
                    Err(TaskQueueError::QueueEmpty) => {
                        if stop_when_idle
                            && saw_work
                            && in_flight.load(Ordering::SeqCst) == 0
                        {
                            info!("dispatcher idle; auto-stopping");
                            stopping = true;
                            paused = true;
                            producer_rx.close();
                        } else {
                            tokio::time::sleep(dequeue_backoff).await;
                        }
                    }
                    Err(err) => {
                        warn!("pop error: {err:?}");
                        tokio::time::sleep(dequeue_backoff).await;
                    }
                }
            }
        }

        if stopping
            && in_flight.load(Ordering::SeqCst) == 0
            && result_rx.is_empty()
        {
            trace!("dispatcher drained; exiting");
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
                StatsEvent::Call { success, latency } => {
                    snapshot.processed += 1;
                    if success {
                        snapshot.succeeded += 1;
                    } else {
                        snapshot.failed += 1;
                    }
                    snapshot.last_latency = Some(latency);
                    #[cfg(feature = "observability")]
                    otel.record_call(success, latency);
                }
                StatsEvent::TerminalFailure => {
                    snapshot.terminal_failures += 1;
                    #[cfg(feature = "observability")]
                    otel.terminal_failure.add(1, &[]);
                }
                StatsEvent::InFlight(n) => {
                    snapshot.in_flight = n;
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
            .with_description("Total tasks processed by the mailbox runtime")
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
            .with_description("Task execution latency")
            .build();

        Self {
            processed,
            succeeded,
            failed,
            terminal_failure,
            latency_ms,
        }
    }

    fn record_call(&self, success: bool, latency: Duration) {
        self.processed.add(1, &[]);
        if success {
            self.succeeded.add(1, &[]);
        } else {
            self.failed.add(1, &[]);
        }
        self.latency_ms
            .record(latency.as_secs_f64() * 1_000.0, &[]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use capp_queue::{InMemoryTaskQueue, JsonSerializer, TaskQueue};
    use serde::{Deserialize, Serialize};
    use std::io;
    use std::sync::atomic::{AtomicU32, AtomicU64};
    use tokio::time::timeout;
    use tower::{BoxError, ServiceBuilder, service_fn};

    #[derive(Clone, Debug, Serialize, Deserialize)]
    struct TestPayload {
        value: u32,
    }

    #[tokio::test]
    async fn dispatcher_processes_tasks() {
        let queue =
            Arc::new(InMemoryTaskQueue::<TestPayload, JsonSerializer>::new());
        let ctx = Arc::new(());

        let (observed_tx, mut observed_rx) = mpsc::channel::<TestPayload>(8);

        let base_service =
            service_fn(move |req: ServiceRequest<TestPayload, ()>| {
                let tx = observed_tx.clone();
                async move {
                    tx.send(req.task.payload).await.unwrap();
                    Ok::<(), BoxError>(())
                }
            });

        let service =
            build_service_stack(base_service, ServiceStackOptions::default());
        let runtime = spawn_mailbox_runtime(
            queue.clone(),
            ctx,
            service,
            MailboxConfig::default(),
        );

        for value in 0..4u32 {
            runtime
                .producer
                .enqueue(Task::new(TestPayload { value }))
                .await
                .unwrap();
        }

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
    async fn concurrency_limit_caps_in_flight() {
        let queue =
            Arc::new(InMemoryTaskQueue::<TestPayload, JsonSerializer>::new());
        let ctx = Arc::new(());

        let in_flight = Arc::new(AtomicU64::new(0));
        let max_seen = Arc::new(AtomicU64::new(0));
        let in_flight_clone = in_flight.clone();
        let max_seen_clone = max_seen.clone();

        let base_service =
            service_fn(move |_req: ServiceRequest<TestPayload, ()>| {
                let in_flight = in_flight_clone.clone();
                let max_seen = max_seen_clone.clone();
                async move {
                    let cur = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                    max_seen.fetch_max(cur, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(40)).await;
                    in_flight.fetch_sub(1, Ordering::SeqCst);
                    Ok::<(), BoxError>(())
                }
            });

        let stack = ServiceBuilder::new()
            .concurrency_limit(2)
            .service(base_service);
        let service =
            build_service_stack(stack, ServiceStackOptions::default());

        let runtime = spawn_mailbox_runtime(
            queue.clone(),
            ctx,
            service,
            MailboxConfig {
                stop_when_idle: true,
                dequeue_backoff: Duration::from_millis(10),
                ..Default::default()
            },
        );

        for value in 0..10u32 {
            runtime
                .producer
                .enqueue(Task::new(TestPayload { value }))
                .await
                .unwrap();
        }

        timeout(Duration::from_secs(5), runtime.join())
            .await
            .expect("runtime did not finish in time");

        assert!(
            max_seen.load(Ordering::SeqCst) <= 2,
            "in-flight peaked at {} (expected ≤ 2)",
            max_seen.load(Ordering::SeqCst)
        );
        assert!(
            max_seen.load(Ordering::SeqCst) >= 1,
            "no tasks observed in-flight"
        );
    }

    #[tokio::test]
    async fn stop_when_idle_with_spawned_futures() {
        let queue =
            Arc::new(InMemoryTaskQueue::<TestPayload, JsonSerializer>::new());
        let ctx = Arc::new(());

        let processed = Arc::new(AtomicU32::new(0));
        let processed_clone = processed.clone();
        let base_service =
            service_fn(move |_req: ServiceRequest<TestPayload, ()>| {
                let counter = processed_clone.clone();
                async move {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    counter.fetch_add(1, Ordering::Relaxed);
                    Ok::<(), BoxError>(())
                }
            });

        let stack = ServiceBuilder::new()
            .concurrency_limit(4)
            .service(base_service);
        let service =
            build_service_stack(stack, ServiceStackOptions::default());

        let total = 6u32;
        for value in 0..total {
            queue
                .push(&Task::new(TestPayload { value }))
                .await
                .unwrap();
        }

        let runtime = spawn_mailbox_runtime(
            queue.clone(),
            ctx,
            service,
            MailboxConfig {
                stop_when_idle: true,
                dequeue_backoff: Duration::from_millis(10),
                ..Default::default()
            },
        );

        timeout(Duration::from_secs(5), runtime.join())
            .await
            .expect("runtime did not auto-stop in time");

        assert_eq!(
            processed.load(Ordering::Relaxed),
            total,
            "runtime auto-stopped before all spawned futures completed"
        );
    }

    #[tokio::test]
    async fn service_without_clone_compiles() {
        // Type-only assertion: a non-Clone service satisfies the bounds of
        // build_service_stack. If this stops compiling, the Clone-free
        // contract regressed.
        struct NonCloneSvc;
        impl Service<ServiceRequest<TestPayload, ()>> for NonCloneSvc {
            type Response = ();
            type Error = BoxError;
            type Future = std::pin::Pin<
                Box<
                    dyn std::future::Future<Output = Result<(), BoxError>>
                        + Send,
                >,
            >;

            fn poll_ready(
                &mut self,
                _cx: &mut std::task::Context<'_>,
            ) -> std::task::Poll<Result<(), Self::Error>> {
                std::task::Poll::Ready(Ok(()))
            }

            fn call(
                &mut self,
                _req: ServiceRequest<TestPayload, ()>,
            ) -> Self::Future {
                Box::pin(async { Ok(()) })
            }
        }

        let _svc: MailboxService<TestPayload, ()> = build_service_stack(
            NonCloneSvc,
            ServiceStackOptions { timeout: None },
        );
    }

    #[tokio::test]
    async fn shutdown_does_not_lose_pending_tasks() {
        let queue =
            Arc::new(InMemoryTaskQueue::<TestPayload, JsonSerializer>::new());
        let ctx = Arc::new(());

        let processed = Arc::new(AtomicU32::new(0));
        let processed_clone = processed.clone();
        let base_service =
            service_fn(move |_req: ServiceRequest<TestPayload, ()>| {
                let counter = processed_clone.clone();
                async move {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    counter.fetch_add(1, Ordering::Relaxed);
                    Ok::<(), BoxError>(())
                }
            });
        let stack = ServiceBuilder::new()
            .concurrency_limit(2)
            .service(base_service);
        let service =
            build_service_stack(stack, ServiceStackOptions::default());

        let runtime = spawn_mailbox_runtime(
            queue.clone(),
            ctx,
            service,
            MailboxConfig::default(),
        );

        let total = 16u32;
        for value in 0..total {
            runtime
                .producer
                .enqueue(Task::new(TestPayload { value }))
                .await
                .unwrap();
        }

        tokio::time::sleep(Duration::from_millis(60)).await;
        runtime.shutdown().await;

        let processed_count = processed.load(Ordering::Relaxed);
        let queue_len = queue.list.lock().unwrap().len() as u32;
        let inflight_len = queue.inflight.lock().unwrap().len() as u32;

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
        let mut service = build_service_stack(
            base_service,
            ServiceStackOptions { timeout: None },
        );
        let (producer, _producer_rx) = ProducerHandle::channel(1);
        let request = ServiceRequest {
            task: Task::new(TestPayload { value: 99 }),
            ctx: Arc::new(()),
            producer,
            attempt: 1,
        };

        let result = service.ready().await.unwrap().call(request).await;
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
    async fn stats_collector_tracks_calls_and_in_flight() {
        let (stats_tx, stats_rx) = mpsc::channel(8);
        let (mut stats_watch, handle) = spawn_stats_collector(stats_rx);

        stats_tx
            .send(StatsEvent::Call {
                success: false,
                latency: Duration::from_millis(5),
            })
            .await
            .unwrap();
        stats_tx.send(StatsEvent::TerminalFailure).await.unwrap();
        stats_tx.send(StatsEvent::InFlight(3)).await.unwrap();

        // Wait for at least one update; then read whatever has accumulated.
        timeout(Duration::from_secs(1), stats_watch.changed())
            .await
            .unwrap()
            .unwrap();
        drop(stats_tx);
        let _ = handle.await;

        let snapshot = stats_watch.borrow().clone();
        assert_eq!(snapshot.failed, 1);
        assert_eq!(snapshot.processed, 1);
        assert_eq!(snapshot.terminal_failures, 1);
        assert_eq!(snapshot.in_flight, 3);
    }

    #[cfg(feature = "observability")]
    #[tokio::test]
    async fn observability_metrics_smoke() {
        let metrics = ObservabilityMetrics::new();
        metrics.record_call(true, Duration::from_millis(3));
    }
}
