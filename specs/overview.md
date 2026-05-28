# CAPP Overview

CAPP is a Rust workspace for building durable async task processors and
web crawlers. It pairs a tower-native execution runtime with pluggable
queue backends and a small set of crawler-shaped helpers (HTTP client
config, URL routing, caching).

## Architecture (0.7)

The runtime is **one dispatcher task driving one tower service**:

```
external producers ─► producer_rx (mpsc) ─┐
                                          ▼
                                    ┌───────────┐
                                    │dispatcher │ ── service.ready()
                                    │ (1 task)  │ ── queue.pop()
                                    │           │ ── tokio::spawn(call)
                                    └───────────┘
                                          │
                          spawned futures │ result_rx (mpsc)
                                          ▼
                                    Ack / Nack → queue ack / retry / DLQ
```

- **Concurrency** lives in the service stack
  (`ServiceBuilder::concurrency_limit(N)`). The dispatcher only pops
  when `service.ready()` resolves, so all stock tower layers
  (`concurrency_limit`, `rate_limit`, `retry`, `timeout`, `buffer`,
  `load_shed`) compose without `Clone` shims.
- **Durability** is the queue's responsibility. `FjallTaskQueue` keeps
  `tasks` / `queue` / `inflight` / `dlq` partitions; the dispatcher
  calls `recover_inflight()` on startup so crashed-mid-flight tasks
  return to the queue.
- **Stats** come out of a `watch::Receiver<StatsSnapshot>` with flat
  counters (`processed`, `succeeded`, `failed`, `in_flight`,
  `terminal_failures`, `last_latency`). Optional `stats-http` and
  `observability` features expose them as JSON and OTLP respectively.

## Workspace layout

| crate | purpose |
|---|---|
| `capp` | public facade — manager, prelude, stats HTTP, observability; re-exports the workspace crates |
| `capp-queue` | `TaskQueue` trait, `Task` type, `InMemoryTaskQueue`, `FjallTaskQueue`, `ProducerHandle` |
| `capp-config` | TOML config loader, HTTP client builder, healthcheck, backoff helpers |
| `capp-router` | URL classification (opt-in via `router` feature) |
| `capp-cache` | HTTP response cache (opt-in via `cache` feature) |
| `capp-urls` | URL helpers (opt-in via `urls` feature) |
| `capp-testkit` | in-process HTTP fixtures used by tests and examples |
| `examples/` | runnable demos — see `examples/hackernews/main.rs` for the canonical full crawler shape |

## Feature flags (the `capp` crate)

- `http` — pulls in `reqwest`.
- `router`, `cache`, `urls` — opt-in re-exports of the workspace crates.
- `healthcheck` — internet-reachability probe.
- `stats-http` — hyper-backed `/stats` JSON endpoint.
- `observability` — OpenTelemetry metrics (counter + histogram per call).

## Quick start

```toml
[dependencies]
capp = { version = "0.7", features = ["http"] }
```

A complete crawler skeleton lives in `skills/build_crawler.md` §7. To
see one running:

```bash
cargo run -p capp --example hackernews       --features http
cargo run -p capp --example local_blog_crawl --features http
```

## Development commands

- `cargo fmt --all`
- `cargo clippy --workspace --all-targets --all-features -D warnings`
- `cargo test --workspace --all-features`
- `cargo doc --workspace --no-deps --open`

## Conventions

- Typed errors (`thiserror`) over panics in library code; reserve
  panics for config-load-time deploy bugs.
- Tests don't hit the network — use `capp-testkit` fixtures.
- Feature-gate every optional backend / dep so binaries stay lean.
