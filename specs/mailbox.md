% Mailbox Runtime (Tower + Dispatcher)

## Overview
- New worker pipeline for v0.6 runs tasks through a central dispatcher and per-worker inboxes, powered by a Tower service stack.
- Queue I/O is centralized: dispatcher owns `pop/ack/nack/push` and feeds bounded inboxes; workers no longer poll queues directly.
- Control and stats planes are explicit: broadcast control (Pause/Resume/Stop), mpsc stats stream (processed/succeeded/failed/terminal_failures).
- Producer path goes through the dispatcher: user-facing code receives a `ProducerHandle` and enqueues via `ProducerMsg::Enqueue`.

## Components
- **ProducerHandle/ProducerMsg** (`capp-queue/src/dispatch.rs`):
  - `ProducerHandle` wraps a bounded `tokio::mpsc` sender.
  - Messages: `Enqueue(Task)` for user enqueues; worker results use `WorkerResult::{Ack,Nack,Return}`.
- **Dispatcher** (`capp/src/manager/mailbox.rs`):
  - Single task; listens to control, producer channel, worker result channel, and queue `pop`.
  - On `pop`: marks task in-progress (`set`), wraps in `Envelope { task, attempt, enqueued_at }`, distributes round-robin to worker inboxes.
  - On worker results:
    - `Ack`: `set` + `ack`.
    - `Nack`: `retry` up to `max_retries`; else `dlq` + emit `TerminalFailure`.
    - `Return`: re-queue task (used when workers stop while inbox still has items).
  - Control: `Pause` stops dequeue; `Resume` restarts; `Stop` pauses dequeue, drops inbox senders, and drains outstanding results.
  - Backoff on `QueueEmpty` with configurable delay.
- **Workers**:
  - Own inbox receiver, control receiver, stats sender, result sender, and a cloneable Tower service (`MailboxService`).
  - Loop: react to control; on `Envelope`, build `ServiceRequest { task, ctx, producer, attempt, worker_id }`, run service via `oneshot`.
  - On success -> `WorkerResult::Ack`; on error -> `WorkerResult::Nack`.
  - On Stop: finish current task, drain remaining inbox items with `WorkerResult::Return` so dispatcher re-queues.
- **Service stack**:
  - Built with `build_service_stack` (load-shed + buffer + timeout + concurrency limit); returns `BoxCloneService<ServiceRequest, (), BoxError>`.
  - Users can layer additional middleware before boxing if needed.

## Control & Observability
- Control channel: `broadcast::Sender<ControlCommand>` with `Pause`, `Resume`, `Stop`.
- Stats: `watch::Receiver<StatsSnapshot>` updated from an internal mpsc collector.
  - Snapshot fields: per-worker processed/succeeded/failed/last_latency, optional queue depth, `terminal_failures` count.
- Tracing: dispatcher and worker loops emit span/logs for dequeue, dispatch, execution, and control transitions; worker logs include `worker_id` and `task_id`.

## Graceful Shutdown
- First Stop/Pause command: dispatcher halts dequeue and drops inbox senders; workers finish current task, then return any queued envelopes (`Return`) for requeue.
- Dispatcher drains worker results and exits; outstanding tasks either succeed, retry, or move to DLQ based on `max_retries`.
- Example mirrors legacy ctrl+c behavior: first Ctrl+C triggers graceful Stop; second forces process exit.

## Example (`examples/mailbox.rs`)
- Runs 100 tasks on 4 workers with 3–6s simulated work.
- Tasks randomly fail to demonstrate retries and DLQ; `max_retries = 2`.
- Logs worker IDs, attempts, and stats (processed, succeeded, dlq).
- Graceful ctrl+c supported; run with `cargo run -p capp --example mailbox --quiet`.

## Default Tunables
- `MailboxConfig`: `worker_count=4`, `inbox_capacity=1`, `producer_buffer=128`, `result_buffer=128`, `max_retries=3`, `dequeue_backoff=50ms`.
- Service stack defaults: timeout 30s, buffer 1, concurrency limit 1 (overridable via `ServiceStackOptions`).

## Paths
- Dispatcher/protocol: `capp-queue/src/dispatch.rs`
- Mailbox runtime: `capp/src/manager/mailbox.rs`
- Demo: `examples/mailbox.rs`
