% Roadmap for 0.6

## Goals
- Cut 0.6 with TOML-only configs; breaking changes are fine (single-user).
- Tower-native compute path (layered rate limit/timeout/retry/tracing).
- Fjall-backed storage (queue default; cache optional).
- Mailbox-based workers (dispatcher + inboxes + control/stats).
- Observability via embedded HTTP metrics/health.

## Status Checklist
- [x] Tower service stack for worker execution (mailbox runtime).
- [x] Fjall queue backend + benches (default feature).
- [x] Mailbox dispatcher + per-worker inboxes, control/stop, stats snapshot.
- [ ] Observability HTTP endpoint with Prometheus text output (decision: use Prometheus text; JSON optional).
- [x] README/docs refresh + migration note (TOML-only, tower+mailbox wiring).
- [ ] Optional fjall cache backend (get/set/ttl) if still desired.

## Pillars (Current State + Next Steps)
1) Tower-native computation pipeline  
   - Done: boxed `MailboxService` builder with load-shed/buffer/timeout/concurrency; workers execute via tower.  
   - Next: adapter/shim for legacy `Computation` if we keep it during migration.

2) Fjall-backed storage  
   - Done: fjall queue backend feature-gated; benches in `capp-queue`.  
   - Next: doc compaction/tuning, optional cache backend if we still want it.

3) Message-driven workers  
   - Done: dispatcher owns queue I/O; per-worker inboxes; control channel (Pause/Resume/Stop); returns unprocessed envelopes on stop; stats/terminal failures tracked.  
   - Next: optional task stealing/overflow queue if needed.

4) Observability  
   - Current: stats snapshot via watch channel; tracing around dequeue/dispatch/service; example logs successes/DLQ.  
   - TODO: small Hyper endpoint behind `http` exposing Prometheus text (primary) and maybe JSON; include queue depth when backend supports.

5) Docs & migration  
   - Current: specs/examples cover tower+mailbox + fjall; TOML already the only config.  
   - Done: README refresh + migration guide in `specs/migration-0.6.md`.

## Decisions
- No backward compatibility guarantee for 0.6; old polling paths can be removed after any shims.  
- Fjall stays feature-gated but is the default backend in docs/examples.  
- Metrics format: Prometheus text exposition for the HTTP endpoint; JSON optional toggle.

## Open Items
- Implement observability HTTP endpoint (Prometheus text first).  
- README/docs/migration note.  
- Decide/implement fjall cache backend or explicitly drop it from 0.6 scope.  
- Consider task stealing/overflow queue if needed for imbalance scenarios.
