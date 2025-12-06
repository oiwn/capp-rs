% CAPP Overview

## Purpose
CAPP (Comprehensive Asynchronous Parallel Processing) is a Rust workspace for building web crawlers and async task processors. It supplies worker orchestration, configurable queues, routing utilities, and health checks so crawlers can scale across domains and backends.

## Core Capabilities
- Multi-backend queues: in-memory, Redis, MongoDB, PostgreSQL (schema in `migrations/`).
- Workers manager: configurable concurrency, retries, round-robin distribution, dead-letter queue.
- Config + HTTP helpers: YAML-driven settings, proxy/backoff utilities; routing for URL classification.
- Health monitoring: built-in internet/endpoint checks to keep runtimes healthy.

## Workspace Layout
- `capp/`: public facade (manager, prelude) re-exporting feature-gated crates.
- `capp-queue/`: queue traits/backends, serializers, task types.
- `capp-config/`: config loader, HTTP/proxy/backoff helpers, router glue.
- `capp-router/`, `capp-cache/`, `capp-urls/`: opt-in routing, caching, and URL helpers.
- `examples/`: runnable demos (`basic`, `urls`, `hackernews`); `tests/`: integration flows with shared harness in `tests/common/`.

## Quick Start
Add the crate with optional backends:
```toml
[dependencies]
capp = { version = "0.5", features = ["redis", "mongodb", "postgres", "router"] }
```
See `examples/basic.rs` for minimal worker setup; run with `cargo run --example basic`.

## Development Commands
- `cargo fmt --all` — rustfmt (edition 2024, width 84).
- `cargo clippy --workspace --all-targets --all-features -D warnings` — lint strictly.
- `cargo test --workspace --all-features` — run unit + integration tests (tokio-based).
- `cargo doc --workspace --no-deps --open` — browse API docs.

## Notes & Expectations
- Favor typed errors (`thiserror`) over panics; avoid `unwrap` in lib paths.
- Gate backend/http/router code with features to keep binaries lean.
- Tests avoid real network calls; use provided fixtures/mocks. If enabling DB-backed queues, document required services.
