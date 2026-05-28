# Current Context

Capp-rs is at **0.7.0**. The worker-pool → single-service runtime
refactor described in the previous version of this file is done; see
`CHANGELOG.md` for the user-facing summary and `capp/src/manager/mailbox.rs`
for the implementation.

## What just landed (0.7.0)

- One dispatcher task owns one tower `BoxService`. Concurrency moved
  out of `MailboxConfig` and into the caller's `ServiceBuilder`.
- `MailboxConfig`: dropped `worker_count` and `prefetch_per_worker`;
  added `stop_when_idle` (introduced earlier but now load-bearing).
- `ServiceRequest`: dropped `worker_id`.
- `StatsSnapshot`: flat counters + `in_flight` gauge; the per-worker
  HashMap is gone.
- HN crawler example replaced `tokio::sleep` politeness with stock
  `concurrency_limit(2) + rate_limit(2, 1s)`.
- Internal `async-channel` dependency removed (MPMC was a worker-pool
  artifact).

## What's next (candidates, no order locked in)

- **Custom tower layers.** Two were sketched in the refactor spec but
  deferred: `per_host_rate_limit` (per-host semaphores via a
  `DashMap<Host, Semaphore>`) and `response_cache` (LRU + Fjall
  backends). The first one is higher value for real crawlers.
- **`reqwest` ↔ `tower` adapter.** `reqwest::Client` isn't a
  `tower::Service`; a small in-tree adapter would let users compose the
  HTTP transport with tower layers cleanly. ~30 lines.
- **`HasTagKey` round-robin dispatch.** Tracked in
  `specs/refactoring.md` item 18. Designed, not implemented; addresses
  starvation when one source dominates the queue.
- **Library panic surface.** Convert remaining `expect`/`unwrap` on
  user-supplied data to `Result`. `specs/refactoring.md` items 7-11.

## Open questions

- `FjallCache` value encoding (JSON vs `bincode`/`postcard`) — relevant
  if/when `response_cache` lands.
- Whether to bundle a `reqwest` adapter or document a `tower-reqwest`-
  style integration and leave it to users.
