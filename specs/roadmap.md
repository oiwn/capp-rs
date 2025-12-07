% Roadmap for 0.6

## Goals
- Cut 0.6 with TOML as the only config format; breaking changes are fine (single-user).
- Move the compute path onto `tower::Service` so we can stack rate limiting, retries, and timeouts declaratively.
- Add fjall-backed storage (queue first, cache optional) to reduce external deps.
- Shift workers to message-driven mailboxes instead of blind polling, with control channels.
- Expose live stats/health over an embedded HTTP endpoint for observability.

## Pillars
1) Tower-native computation pipeline
   - Wrap/replace `Computation::call` with a `Service`-based runner; allow layers for rate limiting, backoff, timeouts, and tracing.
   - Provide a compatibility shim for existing computations during migration (not required to be stable long-term).
2) Fjall-backed storage
   - Queue backend: durable task store with visibility timeouts, retry counters, DLQ parity with existing backends; feature-gated.
   - Cache backend: lightweight fjall-based cache for recent responses/metadata (optional feature).
   - Operational notes: compaction defaults, file layout, tuning guidance for long-running crawlers.
3) Message-driven workers
   - Replace queue polling with per-worker mailboxes (tokio mpsc) fed by a dispatcher.
   - Add a control channel (broadcast) to pause/resume/scale workers and tweak rate limits dynamically.
   - Task stealing scenario: if a worker’s partition goes idle while others are saturated, allow stealing from a shared overflow queue to avoid starvation.
4) Observability
   - Surface per-worker stats (processed/failed/retries/latency) and queue depth via a small Hyper server behind an `http` feature.
   - Keep tracing spans around Service layers; add a metrics sink (prometheus text or JSON).
5) Docs & migration
   - Update README/examples to highlight tower + fjall usage and the mailbox model.
   - Note the breaking API/config changes and minimal migration steps (TOML only, new worker wiring).

## Proposed Work Items
- Tower integration
  - Introduce a `Service` wrapper for computations and expose a builder for stacking layers (rate limit, retry/backoff, timeout, tracing).
  - Adjust `WorkersManager` to execute via the Service stack; provide a helper to adapt existing `Computation` impls.
- Fjall queue backend
  - Implement feature-gated fjall queue with parity to in-memory/Redis semantics (ack/nack/retry/dlq, visibility timeouts).
  - Add tests for crash/restart behavior and simple benchmarks; document disk layout and compaction settings.
- Fjall cache backend (optional)
  - Add a minimal fjall cache feature with get/set/ttl; integrate into `capp-cache` helpers if present.
- Mailbox dispatch
  - Add dispatcher that fans out tasks from the queue into per-worker mailboxes; workers consume from channels instead of direct polling.
  - Add optional task stealing from a shared overflow channel to keep workers busy when partitions drain.
  - Control channel for commands (pause/resume/scale, rate-limit tweaks) to workers.
- Observability server
  - Add a small Hyper-based HTTP endpoint to expose stats/health; gate behind `http`.
  - Emit structured metrics from the dispatcher/worker loops; wire tracing fields through Service layers.
- Docs & cleanup
  - Refresh README/examples/specs to show tower usage and fjall backends; mark previous YAML/polling APIs as replaced.
  - Add a short migration note for 0.6 (TOML configs, tower service wrapper, mailbox manager).

## Decisions
- No backward compatibility guarantee for 0.6; old polling/Computation-only paths can be removed after shims.
- Fjall added as feature-gated backends; existing backends stay feature-flagged but fjall can be the default in docs.

## Open Questions
- Should fjall become the default queue/cache backend in examples, with others as opt-in?
- Metrics format: Prometheus text vs JSON; do we need push or only pull via HTTP?
