# Changelog

## Unreleased

### Added

- Added internal `capp-testkit` workspace crate for reusable local HTTP
  fixtures used by tests and examples.
- Added Hyper/Tokio-backed local fixture server with request logging and
  `/stats` reporting.
- Added built-in local fixture scenarios:
  - `blog`
  - `blog_with_posts(n)`
  - `catalog`
  - `infinite`
- Added `local_blog_crawl` mailbox example that crawls a generated local
  100-post blog and validates coverage against fixture stats.
- `TaskQueue::recover_inflight` trait method (default returns `Ok(0)`) for
  reclaiming tasks owned by a previous, crashed process at startup.
- `FjallTaskQueue` and `InMemoryTaskQueue` now track an `inflight` set:
  `pop` moves the task there, and `ack`/`nack`/`push` clear it.

### Changed

- Reintroduced a realistic crawler example path using the mailbox runtime and a
  local in-process site instead of only external-service demos.
- Expanded test coverage around `capp-testkit`, healthcheck timeout/failure
  behavior, and queue serializer success/error paths.
- `healthcheck::internet` no longer panics on reqwest errors and no longer
  depends on a `404 + content_length == 9` magic check. New contract: returns
  `true` iff any HTTP response is received within 1 second; any error or
  timeout yields `false`.
- **Breaking:** `TaskId::from_string` now returns `Result<Self, uuid::Error>`
  instead of panicking on malformed input. `TaskId` also implements
  `FromStr` so callers can `s.parse::<TaskId>()`.
- **Breaking:** `MailboxConfig.inbox_capacity` renamed to
  `prefetch_per_worker`. The mailbox runtime now uses a single shared
  `async-channel` MPMC channel of capacity
  `prefetch_per_worker * worker_count` instead of N per-worker `mpsc`
  inboxes. Eliminates head-of-line blocking when one worker is slow.
- **Breaking:** `WorkerResult::Return` variant removed. Workers no longer
  produce "return" results; on shutdown the dispatcher closes the shared
  channel and any tasks not picked up remain in the queue's `inflight` set
  for the next process to recover.
- **Breaking:** `ServiceStackOptions` reduced to a single field
  (`timeout: Option<Duration>`). The `buffer` and `concurrency_limit`
  fields were removed.
- **Breaking:** `build_service_stack` no longer adds
  `ConcurrencyLimitLayer`, `.load_shed()`, or `.buffer()` to the service
  stack — only `.timeout()` remains. `MailboxConfig.worker_count` is now
  the sole concurrency knob; the previous defaults
  (`concurrency_limit = Some(1)` + `buffer = 1`) silently serialized the
  worker pool through a shared semaphore and a redundant mpsc. Callers
  that need a per-resource sub-cap should compose their own
  `ServiceBuilder` chain before passing the service in.
- **Breaking:** `StatsSnapshot.queue_depth` field, `StatsEvent::QueueDepth`
  variant, and the OTel `capp_queue_depth` observable gauge were removed.
  Nothing was sampling queue depth, so the metric was always zero. Real
  depth observability needs `TaskQueue::len()` + periodic sampling and is
  not part of this release.
- Mailbox dispatcher gives `ControlCommand` priority over other events via
  `biased;` selection.
- `handle_worker_result::Ack` no longer writes the task before acking; ack
  removes the row.

### Fixed

- Crashed-mid-flight tasks on the Fjall backend are no longer lost. On
  startup, `spawn_mailbox_runtime` calls `recover_inflight` to move any
  leased-but-unacked rows back to the queue.
