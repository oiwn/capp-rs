% Updating to 0.6

## Current State
- Config loader, proxy/http helpers, examples, and tests now consume `toml::Value` and point to `.toml` fixtures (e.g., `tests/simple_config.toml`).
- YAML dependencies were removed in favor of `toml`; sample configs converted to TOML; AGENTS.md updated to reference TOML configs.
- Basic example run succeeds against the TOML fixture.
- Queue backend changes in flight for v0.6: fjall is the default backend; queue keys now use UUID v7 (roughly time-ordered) instead of a monotonic u64 counter. On-disk compatibility with prior queue layouts is intentionally broken.

## Follow-ups
- Bump workspace/crate versions to `0.6.0` and refresh README dependency snippet to match.
- Run fmt/clippy/test across the workspace after any doc/code touch-ups.
- Add a short changelog/README note calling out the breaking config format change.

## v0.6 execution plan (tower + mailbox + control/stats)
- Tower-native service stack: replace/augment `Computation::call` with a `tower::Service<Task>` stack (layers for rate-limit, timeout, retry/backoff, tracing, buffer/load-shed); provide a builder; no back-compat needed.
- Dispatcher + mailboxes: single dispatcher owns queue I/O (`pop/ack/nack`), wraps tasks in `Envelope`, and sends to per-worker bounded `tokio::mpsc` inboxes (RR or load-aware). Workers never poll queue directly in 0.6.
- Workers: each owns inbox rx, control rx, stats tx, and a tower service stack. Loop: recv envelope -> run service -> ack/nack via dispatcher helper -> emit stats (latency/status/counters).
- Control channel: broadcast commands (Pause/Resume dequeue, SetRateLimit, Scale, Drain, ReloadConfig). Dispatcher pauses pulling; workers respond to per-worker commands.
- Stats channel: workers send metrics to aggregator; dispatcher can add queue depth if backend supports. Snapshot exposed to HTTP/metrics.
- Observability: small Hyper endpoint behind `http` feature for `/metrics` and `/health`; tracing spans around dequeue/dispatch/service with task/worker IDs.
