% CAPP Overview

## Purpose
CAPP (Comprehensive Asynchronous Parallel Processing) is a Rust workspace for building web crawlers and async task processors. It supplies worker orchestration, configurable queues, routing utilities, and health checks so crawlers can scale across domains and backends.

## Core Capabilities
- Multi-backend queues: in-memory, Fjall (default), MongoDB.
- Workers manager: configurable concurrency, retries, round-robin distribution, dead-letter queue.
- Config + HTTP helpers: TOML-driven settings, proxy/backoff utilities; routing for URL classification.
- Mailbox runtime: Tower-native execution pipeline with rate limits, timeouts, and stats.
- Health monitoring: built-in internet/endpoint checks to keep runtimes healthy.

## Workspace Layout
- `capp/`: public facade (manager, prelude) re-exporting feature-gated crates.
- `capp-queue/`: queue traits/backends, serializers, task types.
- `capp-config/`: config loader, HTTP/proxy/backoff helpers, router glue.
- `capp-router/`, `capp-cache/`, `capp-urls/`: opt-in routing, caching, and URL helpers.
- `examples/`: runnable demos (`basic`, `urls`, `mailbox`, `httpbin_tower`); `tests/`: integration flows with shared harness in `tests/common/`.

## Quick Start
Add the crate with optional backends:
```toml
[dependencies]
capp = { version = "0.6", features = ["mongodb", "router"] }
```
See `examples/basic.rs` for minimal worker setup; run with `cargo run --example basic`.

## Example: Mailbox Stats HTTP
Run the mailbox demo with a live stats endpoint:
```sh
cargo run -p capp --features stats-http --example mailbox_stats_http
```
Then query the stats endpoint:
```sh
curl http://127.0.0.1:8080/stats
```
The demo runs indefinitely and enqueues a follow-up task after each success. Stop
it with Ctrl+C.

## Development Commands
- `cargo fmt --all` — rustfmt (edition 2024, width 84).
- `cargo clippy --workspace --all-targets --all-features -D warnings` — lint strictly.
- `cargo test --workspace --all-features` — run unit + integration tests (tokio-based).
- `cargo doc --workspace --no-deps --open` — browse API docs.

## Notes & Expectations
- Favor typed errors (`thiserror`) over panics; avoid `unwrap` in lib paths.
- Gate backend/http/router code with features to keep binaries lean.
- Tests avoid real network calls; use provided fixtures/mocks. If enabling DB-backed queues, document required services.
