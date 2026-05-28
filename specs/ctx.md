# Current Context

# Refactor: worker pool → single-service runtime

## Decision

Commit to the single-service execution model. The worker pool goes away.
Concurrency is provided by tower's `ConcurrencyLimitLayer` (or whatever
the user wires in). Backpressure goes through `Service::poll_ready` as
tower intended. All stock tower middleware (`timeout`, `concurrency_limit`,
`rate_limit`, `retry`, `buffer`, `load_shed`) composes without
modification.

This is a breaking change to `capp-rs`. Pre-1.0; the surface change is
acceptable.

## What's removed

- The worker pool inside `spawn_mailbox_runtime` (N tokio tasks loop
  on `envelope_rx`).
- `MailboxConfig::worker_count`.
- `MailboxConfig::prefetch_per_worker`.
- The dispatcher→workers shared `async-channel`.
- `BoxCloneService` requirement — service no longer needs to be `Clone`.
- Per-worker stats (`WorkerMetrics.worker_id`, the per-worker hashmap in
  `StatsSnapshot`).
- `ServiceRequest::worker_id` field.

## What stays

- Durable queue + `push/pop/ack/nack/set` semantics.
- Dispatcher's queue-level retry (`MailboxConfig::max_retries` re-queues
  on Nack — orthogonal to in-call retry, which is now a tower layer).
- `stop_when_idle` (semantics unchanged: queue empty AND in-flight zero
  AND has-ever-seen-work).
- `MailboxRuntime` public API: `producer`, `control`, `stats`,
  `shutdown`, `join`.
- `ProducerHandle` so external code can still enqueue.

## New `MailboxConfig`

```rust
#[derive(Debug, Clone)]
pub struct MailboxConfig {
    pub producer_buffer: usize,
    pub result_buffer: usize,
    pub max_retries: u32,
    pub dequeue_backoff: Duration,
    pub stop_when_idle: bool,
}
```

Concurrency lives in the service stack, not here. Users wanting N
concurrent in-flight tasks add `.concurrency_limit(N)` to their
`ServiceBuilder`.

## New `build_service_stack`

Returns `BoxService` (not `BoxCloneService`). No `Clone` needed.

```rust
pub type MailboxService<D, Ctx> = BoxService<
    ServiceRequest<D, Ctx>,
    (),
    BoxError,
>;

pub fn build_service_stack<D, Ctx, S, E>(
    service: S,
    options: ServiceStackOptions,
) -> MailboxService<D, Ctx>
where
    S: Service<ServiceRequest<D, Ctx>, Response = (), Error = E> + Send + 'static,
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
```

## New dispatcher loop

One task. Owns the service. Pops, spawns, repeats. Tracks in-flight via
an `AtomicUsize` shared with the spawned futures.

```rust
async fn dispatcher_loop<D, Ctx>(params: DispatcherParams<D, Ctx>)
where
    D: Debug + Clone + Serialize + DeserializeOwned + Send + Sync + 'static,
    Ctx: Send + Sync + 'static,
{
    let DispatcherParams {
        queue, ctx, mut service, producer, // for ServiceRequest
        mut control_rx, mut producer_rx, mut result_rx,
        result_tx,
        max_retries, dequeue_backoff, stop_when_idle, stats_tx,
    } = params;

    let _ = queue.recover_inflight().await;

    let in_flight = Arc::new(AtomicUsize::new(0));
    let mut paused = false;
    let mut stopping = false;
    let mut saw_work = false;

    loop {
        tokio::select! {
            biased;
            control = control_rx.recv() => match control {
                Ok(ControlCommand::Pause)  => paused = true,
                Ok(ControlCommand::Resume) => paused = false,
                Ok(ControlCommand::Stop) | Err(_) => {
                    stopping = true; paused = true; producer_rx.close();
                }
            },
            msg = producer_rx.recv(), if !stopping => {
                if let Some(msg) = msg {
                    if handle_producer_msg(&queue, msg).await.is_ok() {
                        saw_work = true;
                    }
                }
            }
            result = result_rx.recv() => {
                if let Some(r) = result {
                    in_flight.fetch_sub(1, SeqCst);
                    let _ = handle_worker_result(&queue, r, max_retries, &stats_tx).await;
                }
            }
            // Tower-idiomatic: wait for capacity before popping.
            ready = service.ready(), if !paused && !stopping => {
                let ready_svc = match ready {
                    Ok(s) => s,
                    Err(_) => { stopping = true; continue; }
                };
                match queue.pop().await {
                    Ok(mut task) => {
                        saw_work = true;
                        task.set_in_progress();
                        let _ = queue.set(&task).await;

                        let attempt = task.retries.saturating_add(1);
                        let request = ServiceRequest {
                            task: task.clone(),
                            ctx: ctx.clone(),
                            producer: producer.clone(),
                            attempt,
                        };
                        let fut = ready_svc.call(request);

                        in_flight.fetch_add(1, SeqCst);
                        let result_tx = result_tx.clone();
                        let stats_tx = stats_tx.clone();
                        let started = Instant::now();
                        tokio::spawn(async move {
                            let outcome = fut.await;
                            let success = outcome.is_ok();
                            let _ = stats_tx.send(StatsEvent::Call {
                                success, latency: started.elapsed(),
                            }).await;
                            let result = match outcome {
                                Ok(()) => WorkerResult::Ack { task },
                                Err(e) => WorkerResult::Nack { task, error: e.to_string() },
                            };
                            let _ = result_tx.send(result).await;
                        });
                    }
                    Err(TaskQueueError::QueueEmpty) => {
                        if stop_when_idle
                            && saw_work
                            && in_flight.load(SeqCst) == 0
                        {
                            stopping = true; paused = true; producer_rx.close();
                        } else {
                            tokio::time::sleep(dequeue_backoff).await;
                        }
                    }
                    Err(_) => tokio::time::sleep(dequeue_backoff).await,
                }
            }
        }

        if stopping
            && in_flight.load(SeqCst) == 0
            && result_rx.is_empty()
        {
            break;
        }
    }
}
```

Key points:
- `service.ready()` from `tower::ServiceExt` is the backpressure gate.
  If the inner stack has `ConcurrencyLimit`, `RateLimit`, etc., `ready`
  blocks until they all have capacity. No bounded prefetch channel needed.
- The future returned by `call(req)` is owned and `'static`. Spawn it,
  move on.
- `in_flight` accounting + result channel preserves `stop_when_idle` and
  the queue-level retry path.

## `ServiceRequest` change

```rust
pub struct ServiceRequest<D: Clone, Ctx> {
    pub task: Task<D>,
    pub ctx: Arc<Ctx>,
    pub producer: ProducerHandle<D>,
    pub attempt: u32,
    // worker_id removed
}
```

## `StatsSnapshot` change

```rust
pub struct StatsSnapshot {
    pub processed: u64,
    pub succeeded: u64,
    pub failed: u64,
    pub last_latency: Option<Duration>,
    pub in_flight: u64,        // new
    pub terminal_failures: u64,
}

enum StatsEvent {
    Call { success: bool, latency: Duration },
    TerminalFailure,
    InFlight(usize),  // periodic snapshot from dispatcher
}
```

(Per-worker breakdown gone; replaced by aggregates.)

## Wiring example (post-refactor)

```rust
let service = ServiceBuilder::new()
    .concurrency_limit(8)                  // up to 8 in-flight tasks
    .rate_limit(8, Duration::from_secs(1)) // ≤ 8 calls/sec across all
    .timeout(Duration::from_secs(30))
    .service(service_fn(move |req: ServiceRequest<HnTask, Ctx>| async move {
        match req.task.payload.clone() {
            HnTask::Listing { page } => handle_listing(&req, page).await,
            HnTask::Item    { id }   => handle_item(&req, id).await,
        }
        .map_err(|e: anyhow::Error| -> BoxError { e.into() })
    }));

let runtime = spawn_mailbox_runtime(queue, ctx, service, MailboxConfig {
    producer_buffer: 256,
    result_buffer: 256,
    max_retries: 1,
    dequeue_backoff: Duration::from_millis(100),
    stop_when_idle: true,
});
```

No `worker_count`. No `prefetch_per_worker`. No Clone-correct shims.

## Migration of existing examples

All struct-literal `MailboxConfig { ... }` sites updated:

- `examples/basic.rs`
- `examples/mailbox.rs`
- `examples/mailbox_metrics.rs`
- `examples/mailbox_stats_http.rs`
- `examples/httpbin_tower.rs`
- `examples/local_blog_crawl.rs`
- `examples/hackernews/main.rs`
- `capp/src/manager/mailbox.rs` (3 in-tree test sites)

Pattern: drop the two removed fields, add `.concurrency_limit(N)` to the
`ServiceBuilder` chain where the old `worker_count` lived.

## Test plan

Inside `capp/src/manager/mailbox.rs`:
- Existing tests pass after migration to the new config.
- New: `concurrency_limit_caps_in_flight` — set `concurrency_limit(2)`,
  enqueue 10 slow tasks, assert `in_flight ≤ 2` throughout.
- New: `stop_when_idle_with_spawned_futures` — covers the refactored
  in-flight accounting.
- New: `service_without_clone_compiles` — type-only assertion that
  `BoxService` is the runtime requirement.

Workspace-wide: `cargo build --workspace --all-features`,
`cargo test --workspace --all-features`,
`cargo clippy --all --workspace --all-features`.

## Tower middlewares (post-refactor)

**Stock tower covers most cases now.** Compose freely:
- `timeout`, `concurrency_limit`, `rate_limit`, `retry`, `buffer`,
  `load_shed` — all work because there's exactly one service instance.

**Custom layers still warranted** because they need data structures
stock tower lacks, not because of Clone gymnastics:

### `per_host_rate_limit` (HTTP transport level)
- Wraps `Service<reqwest::Request>`. Limits concurrent in-flight (and
  optional rate) per host.
- State: `Arc<DashMap<HostKey, Arc<Semaphore>>>`. Naturally shared.
- Config: `PerHostConfig { concurrency: usize, rate: Option<(u32, Duration)> }`.
- Default key: `url::Url::host_str`. Pluggable extractor for eTLD+1.

### `response_cache` (HTTP transport level)
- Wraps `Service<reqwest::Request>`. On `GET`, lookup → return on hit,
  fetch+insert on miss.
- Storage trait: `ResponseCache`. Built-in impls: `MemoryCache`
  (LRU), `FjallCache` (persistent).
- Config: `CacheConfig { ttl, max_bytes, only_methods }`. Honors
  `Cache-Control: no-store` / `private` by default; respects `max-age`.
- Non-goal: full RFC 7234. Point users at `http-cache-reqwest` for strict
  semantics.

### Module shape

```rust
// capp/src/middleware/mod.rs
pub mod per_host_rate_limit;
pub mod response_cache;

pub use per_host_rate_limit::{PerHostRateLimitLayer, PerHostConfig};
pub use response_cache::{ResponseCacheLayer, CacheConfig, MemoryCache, FjallCache, ResponseCache};
```

## Open questions (resolve in next session)

- **`reqwest`↔`tower` bridge.** `reqwest::Client` isn't a `tower::Service`.
  Options: (a) take a hard dep on `reqwest-middleware`; (b) ship a thin
  adapter in `capp::http` (~30 lines); (c) doc-only, leave to users.
  Leaning (b).
- **`FjallCache` value encoding.** JSON for debuggability, or
  `bincode`/`postcard` for speed/size. Leaning JSON.
- **`ServiceExt::ready` reborrow.** The `ready_svc = service.ready().await?`
  pattern needs `service: &mut Service`. Verify it composes cleanly inside
  `tokio::select!` arms (the future from `ready()` borrows `service` for
  its lifetime; double-borrows under select! need care).

## Out of scope

- Multi-process / distributed rate limiting (needs external coordination).
- Per-host rate limit at the capp Service level (HTTP-level only).
- robots.txt-aware fetcher.
- Conditional-GET (ETag / If-Modified-Since) middleware.
- Response decompression (reqwest handles gzip/brotli).

## Order of work (next session)

1. Refactor `mailbox.rs`: drop workers, switch to single-service dispatcher
   with `tokio::spawn` per task. Get `cargo build --workspace` green.
2. Migrate the 8 example sites + 3 in-tree tests. `cargo test` green.
3. Update HN crawler: replace its inline `tokio::sleep` politeness with
   `ServiceBuilder::new().concurrency_limit(2).rate_limit(2, 1s)`.
4. Add the new tests listed under "Test plan."
5. (Stretch) Wire one of the custom layers (`per_host_rate_limit` is
   the higher-value one). Defer `response_cache` to a follow-up.
6. CHANGELOG entries for the breaking changes.
