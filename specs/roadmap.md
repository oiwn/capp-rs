% Roadmap for 0.6

## Focus Areas
- Config format swap to TOML (breaking change) and version bump to 0.6.
- Queue backend expansion with fjall KV store, inspired by FetchBox task 03 queue workers.
- Worker ingestion model improvements (message boxes/mailboxes) to decouple producers and executors.
- Per-worker monitoring and optional control server for observability.

## Proposed Work Items
- Fjall backend
  - Add fjall as a feature-gated queue backend; mirror in-memory/Redis trait surface.
  - Implement durable task storage, visibility timeouts, retry bookkeeping, and DLQ parity.
  - Define migration path and schemas (if any) for fjall files; document operational notes.
- Message boxes for workers
  - Explore mailbox model where workers pull from dedicated queues/partitions; compare to shared round-robin.
  - Evaluate load-balancing vs. fairness, backpressure behavior, and failure isolation.
  - Prototype API changes in `capp::manager` to allow mailbox-aware scheduling without breaking the prelude ergonomics.
- Monitoring/Server
  - Add per-worker metrics (processed, failed, retries, latency) exposed via traits and pluggable sinks.
  - Optional lightweight HTTP server to expose health/metrics endpoints; consider reuse of existing healthcheck module.
  - Provide hooks for structured logging/tracing spans per worker and queue backend.
- Config + docs
  - Move all sample configs/tests to TOML; update README/AGENTS with new format and feature flags.
  - Document new backends and monitoring endpoints; add examples for fjall + mailbox usage.
- Maybe remove queue backends? only use fjall for this.
- We also need to consider replacing "call" with tower service, with which we could have whole bunch of goodies as rate limiting, which could be controlled via channels

## Open Questions
- Mailbox model: should workers own stable partitions or can they steal tasks? How to avoid hot partition starvation?
^^^ provide scenario where worker will need to steal task somewhere?
- Fjall ops: expected compaction/settings defaults for long-running crawlers; disk layout guidance.
- Monitoring transport: expose only HTTP/metrics, or also allow channel callbacks for embedding?
^^^ i think will need to have channel to control the worker

## Risks
- Compatibility break from YAML → TOML and API shifts for mailbox support.
- Additional backend increases test matrix; need feature guards and mocked coverage to keep CI stable.

## Research & References
- FetchBox Task 03 queue workers (mailbox + fjall inspiration): https://github.com/oiwn/FetchBox/blob/main/specs/task_03_queue_workers.md
- Fjall embedded KV store crate (API, durability characteristics): https://crates.io/crates/fjall
- Tokio mailboxes/queues for worker models (bounded mpsc, broadcast) docs: https://docs.rs/tokio/latest/tokio/sync/index.html
