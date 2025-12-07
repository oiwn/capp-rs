% Updating to 0.6

## Quick prompt for next session
- Target: implement tower-native worker pipeline with dispatcher/mailboxes/control/stats and fjall default queue (UUID v7 keys, no back-compat), starting in new/parallel modules so the current API stays untouched until the switch.
- Read alongside `specs/overview.md` for project shape; use `capp-queue` fjall backend and `capp/src/manager` when wiring.
- Key files to touch: `capp-queue/src` (dispatcher helpers, producer handle), `capp/src/manager/worker.rs` (switch to inbox + service), `capp/src/lib.rs` (re-export tower/tokio utils if needed), bench/docs updates as needed.
- Tests/benches: `cargo test -p capp-queue --features fjall`, `cargo bench --bench fjall_bench`. fmt/clippy/test before PR.

^^^ i think would be better to start this in separate files we should have this new api in parallel.

## Current State
- Config loader, proxy/http helpers, examples, and tests now consume `toml::Value` and point to `.toml` fixtures (e.g., `tests/simple_config.toml`).
- YAML dependencies were removed in favor of `toml`; sample configs converted to TOML; AGENTS.md updated to reference TOML configs.
- Basic example run succeeds against the TOML fixture.
- Queue backend changes in flight for v0.6: fjall is the default backend; queue keys now use UUID v7 (roughly time-ordered) instead of a monotonic u64 counter. On-disk compatibility with prior queue layouts is intentionally broken.

## Follow-ups
- Run fmt/clippy/test across the workspace after any doc/code touch-ups.
- Add a short changelog/README note calling out the breaking config format change.

## v0.6 execution plan (tower + mailbox + control/stats)
- Tower-native service stack: replace/augment `Computation::call` with a `tower::Service<Task>` stack (layers for rate-limit, timeout, retry/backoff, tracing, buffer/load-shed); provide a builder; no back-compat needed.
- Dispatcher + mailboxes: single dispatcher owns queue I/O (`pop/ack/nack`), wraps tasks in `Envelope`, and sends to per-worker bounded `tokio::mpsc` inboxes (RR or load-aware). Workers never poll queue directly in 0.6.
- Workers: each owns inbox rx, control rx, stats tx, and a tower service stack. Loop: recv envelope -> run service -> ack/nack via dispatcher helper -> emit stats (latency/status/counters).
- Control channel: broadcast commands (keep minimal for now: Pause/Resume dequeue and Stop). Dispatcher pauses pulling; workers respond to stop signals.
^^^ should have really small subset of commands for now, only pause/resume and stop. 
- Stats channel: workers send metrics to aggregator; dispatcher can add queue depth if backend supports. Snapshot exposed to HTTP/metrics.
- Observability: small Hyper endpoint behind `http` feature for `/metrics` and `/health`; tracing spans around dequeue/dispatch/service with task/worker IDs.
- User function enqueue path: service receives a `ProducerHandle` that sends `ProducerMsg::Enqueue(task)` to dispatcher; dispatcher centralizes queue pushes (batchable). Workers do not touch queue directly.
- Pros vs polling: centralized queue I/O, backpressure via inboxes, tower layering, clear control/stats plane, batching potential. Cons: more components, dispatcher is a single choke point, slight latency hop, more testing surface.
- Inbox sizing: default bounded inbox per worker of 1 (tasks are I/O-heavy/long); keep configurable if small buffering (2–4) is desired later.

^^^ p

### Detailed flow (dispatcher + workers)
- Init:
  - Build tower stack (rate-limit/timeout/retry/tracing) via builder.
  - Create per-worker inbox channels (bounded) and control broadcast; create producer channel (for user enqueues) into dispatcher.
  - Spawn dispatcher with queue handle + inbox senders + producer/control receivers.
  - Spawn workers with inbox receiver + control receiver + stats sender + cloned service stack + producer handle (tx to dispatcher).
- Dispatcher loop (single task):
  - Respect control commands (Pause/Resume/Stop); when paused, skip dequeue; on Stop, drain/exit.
  - `select!` on:
    - `queue.pop()`: on Ok -> wrap `Envelope { task, enqueued_at, attempt, result_tx }` -> pick inbox (RR or load-aware) -> send; on `QueueEmpty` -> small delay/backoff.
    - `producer_rx`: handle `ProducerMsg::Enqueue(task)` by `push` to queue (batchable later).
    - `result_rx` (oneshots from workers): `Ack { task }` -> `queue.ack`; `Nack { task, reason }` -> retry bookkeeping -> `nack/DLQ`.
- Worker loop (per worker):
  - `select!` on control vs inbox:
    - On command: pause/resume/stop locally.
    - On `Envelope`: run tower service with `ServiceRequest { task, ctx, producer, attempt }`.
      - Success: mark task success, send `WorkerResult::Ack { task }` to dispatcher result channel, emit stats (latency, counters).
      - Error: set retry/DLQ info on task, send `WorkerResult::Nack { task, reason }`, emit failure stats.
      - User code can call `producer.enqueue(task)` to submit new work via dispatcher.
- Stats/observability:
  - Workers send periodic metrics to aggregator; dispatcher can report queue depth/dequeue rate.
  - HTTP `/metrics` and `/health` behind `http` feature; tracing spans around dequeue/dispatch/service calls with task/worker IDs.
