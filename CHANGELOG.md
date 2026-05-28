# Changelog

## 0.7.0

Major architecture change: the mailbox worker pool is gone. The runtime
is now one dispatcher task driving one tower service; concurrency,
rate-limiting, retry, etc. all live in the caller's `ServiceBuilder`
chain. Stock tower middleware composes without `Clone` shims because
there is exactly one service instance.

### Changed

- **Breaking:** `MailboxConfig` lost `worker_count` and
  `prefetch_per_worker`. Add `.concurrency_limit(N)` to your
  `ServiceBuilder` chain instead. Remaining fields: `producer_buffer`,
  `result_buffer`, `max_retries`, `dequeue_backoff`, `stop_when_idle`.
- **Breaking:** `MailboxService<D, Ctx>` is now
  `BoxService<ServiceRequest<D, Ctx>, (), BoxError>` (was
  `BoxCloneService`). Services no longer need to be `Clone`.
- **Breaking:** `build_service_stack` drops the `Clone` bound on the
  inner service; signature otherwise unchanged.
- **Breaking:** `ServiceRequest` lost the `worker_id` field.
- **Breaking:** `StatsSnapshot` is now flat — `processed`, `succeeded`,
  `failed`, `last_latency`, `in_flight` (new), `terminal_failures`.
  The per-worker `HashMap<usize, WorkerMetrics>` is gone, along with
  `WorkerMetrics`. The internal `StatsEvent::Worker` variant was
  replaced by `Call { success, latency }` + `InFlight(u64)`.
- **Breaking:** `Envelope<D>` type removed (was only used between the
  dispatcher and the removed worker pool).
- **Breaking:** OpenTelemetry metrics no longer carry a `worker_id`
  attribute — they're emitted as plain counters/histogram.
- `examples/hackernews/main.rs` replaces the inline
  `tokio::sleep(POLITE_DELAY)` with a tower
  `concurrency_limit(2).rate_limit(2, 1s)` chain.
- `examples/httpbin_tower.rs` drops the `Buffer` + `BoxCloneService`
  shim that was previously needed to make the rate-limited service
  `Clone`.

### Added

- `MailboxConfig::stop_when_idle: bool` — auto-stop the dispatcher
  once the queue is empty and all in-flight futures have completed.
  Suitable for finite crawls; existing tests cover the
  spawn-future drain semantics.
- `StatsSnapshot::in_flight: u64` — live gauge of dispatched-but-not-
  yet-completed futures.
- New in-tree tests: `concurrency_limit_caps_in_flight`,
  `stop_when_idle_with_spawned_futures`,
  `service_without_clone_compiles`.

### Removed

- `async-channel` dependency. The shared MPMC channel that fed
  envelopes to the worker pool is gone with the pool.

### Migration from 0.6.x

```rust
// before
MailboxConfig {
    worker_count: 4,
    prefetch_per_worker: 1,
    producer_buffer: 128,
    result_buffer: 128,
    max_retries: 2,
    dequeue_backoff: Duration::from_millis(50),
    stop_when_idle: false,
}

// after
let inner = ServiceBuilder::new()
    .concurrency_limit(4)              // ← where worker_count went
    .service(my_service);
let service = build_service_stack(inner, ServiceStackOptions::default());

MailboxConfig {
    producer_buffer: 128,
    result_buffer: 128,
    max_retries: 2,
    dequeue_backoff: Duration::from_millis(50),
    stop_when_idle: false,
}
```

If you logged `req.worker_id` inside service handlers, drop the field.
If you read `StatsSnapshot.workers`, read the flat counters instead.

## 0.6.2

### Added

- Internal `capp-testkit` workspace crate for reusable local HTTP
  fixtures used by tests and examples.
- Hyper/Tokio-backed local fixture server with request logging and
  `/stats` reporting.
- Built-in local fixture scenarios: `blog`, `blog_with_posts(n)`,
  `catalog`, `infinite`.
- `local_blog_crawl` mailbox example that crawls a generated local
  100-post blog and validates coverage against fixture stats.
- `TaskQueue::recover_inflight` trait method (default returns
  `Ok(0)`) for reclaiming tasks owned by a previous, crashed process
  at startup.
- `FjallTaskQueue` and `InMemoryTaskQueue` now track an `inflight`
  set: `pop` moves the task there, and `ack` / `nack` / `push` clear
  it.

### Changed

- Reintroduced a realistic crawler example path using the mailbox
  runtime and a local in-process site instead of only external-service
  demos.
- Expanded test coverage around `capp-testkit`, healthcheck
  timeout/failure behavior, and queue serializer success/error paths.
- `healthcheck::internet` no longer panics on reqwest errors and no
  longer depends on a `404 + content_length == 9` magic check. New
  contract: returns `true` iff any HTTP response is received within 1
  second; any error or timeout yields `false`.
- **Breaking:** `TaskId::from_string` now returns
  `Result<Self, uuid::Error>` instead of panicking on malformed input.
  `TaskId` also implements `FromStr` so callers can
  `s.parse::<TaskId>()`.
- **Breaking:** `MailboxConfig.inbox_capacity` renamed to
  `prefetch_per_worker`. The mailbox runtime now uses a single shared
  `async-channel` MPMC channel of capacity
  `prefetch_per_worker * worker_count` instead of N per-worker `mpsc`
  inboxes. Eliminates head-of-line blocking when one worker is slow.
  *(Note: the worker pool was removed entirely in 0.7.0.)*
- **Breaking:** `WorkerResult::Return` variant removed. Workers no
  longer produce "return" results; on shutdown the dispatcher closes
  the shared channel and any tasks not picked up remain in the queue's
  `inflight` set for the next process to recover.
- **Breaking:** `ServiceStackOptions` reduced to a single field
  (`timeout: Option<Duration>`). The `buffer` and `concurrency_limit`
  fields were removed.
- **Breaking:** `build_service_stack` no longer adds
  `ConcurrencyLimitLayer`, `.load_shed()`, or `.buffer()` to the
  service stack — only `.timeout()` remains. `MailboxConfig.worker_count`
  is now the sole concurrency knob; the previous defaults
  (`concurrency_limit = Some(1)` + `buffer = 1`) silently serialized
  the worker pool through a shared semaphore and a redundant mpsc.
  Callers that need a per-resource sub-cap should compose their own
  `ServiceBuilder` chain before passing the service in.
- **Breaking:** `StatsSnapshot.queue_depth` field,
  `StatsEvent::QueueDepth` variant, and the OTel `capp_queue_depth`
  observable gauge were removed. Nothing was sampling queue depth, so
  the metric was always zero. Real depth observability needs
  `TaskQueue::len()` + periodic sampling and is not part of this
  release.
- Mailbox dispatcher gives `ControlCommand` priority over other events
  via `biased;` selection.
- `handle_worker_result::Ack` no longer writes the task before acking;
  ack removes the row.

### Fixed

- Crashed-mid-flight tasks on the Fjall backend are no longer lost. On
  startup, `spawn_mailbox_runtime` calls `recover_inflight` to move any
  leased-but-unacked rows back to the queue.
