# capp-rs refactoring backlog

Findings from a workspace-wide code review (May 2026). Items are roughly ordered
by impact, not by where they appear in the codebase.

## Architecture / correctness

### 1. ~~Mailbox runtime head-of-line blocks the worker pool~~ — DONE
Replaced N per-worker mpsc inboxes with a single shared `async-channel` MPMC
channel consumed by all workers. `MailboxConfig.inbox_capacity` renamed to
`prefetch_per_worker`. See `slow_worker_does_not_block_others` test for the
regression guard.

### 2. ~~Task lost on worker/process crash (Fjall backend)~~ — DONE
`TaskQueue::recover_inflight` added. Fjall now keeps an `inflight` keyspace;
`pop` moves the row there, `ack`/`nack`/`push` clear it, and dispatcher calls
`recover_inflight` at startup. In-memory backend mirrors the contract for
trait parity and shared tests.

### 3. ~~`queue_depth` in `StatsSnapshot` is essentially fake~~ — DONE
Removed `StatsSnapshot.queue_depth`, the `StatsEvent::QueueDepth` variant, the
shutdown emit, and the OTel `capp_queue_depth` observable gauge. Nothing was
sampling depth, so the field, variant, and metric are gone. If real depth
observability is wanted later, it needs `TaskQueue::len()` plus periodic
sampling — track that as a separate item if/when needed.

### 4. Wasted `set` writes around lifecycle transitions
- After `pop()`, dispatcher calls `task.set_in_progress(); queue.set(&task)`.
  If `set` fails the popped task is silently dropped (`continue`). Still
  open — should either be skipped or persisted transactionally with the pop.
- ~~In `handle_worker_result::Ack`, `set_succeed(); queue.set(); queue.ack()`~~
  — DONE: the ack-time `set` was removed.

### 5. ~~`tokio::select!` priority in dispatcher~~ — DONE
Added `biased;` to dispatcher and worker `select!` so `ControlCommand` is
checked first. Producer arm also gated by `if !stopping` to avoid a hot-loop
where a closed mpsc returns `None` forever and starves the result arm.

### 6. Worker stop drain — now documented as best-effort
After moving to a shared channel, workers no longer try to drain a personal
inbox on Stop. Tasks already inside `service.oneshot(...)` run to completion;
tasks left in the shared channel get picked up by whichever worker is still
running. Anything not popped from the queue stays there for the next process.
This is documented in CHANGELOG; failures in `result_tx.send(...)` during
shutdown are still silently ignored.

### 6a. ~~Tower `ConcurrencyLimitLayer` is global, not per-worker~~ — DONE

`build_service_stack` no longer adds `ConcurrencyLimitLayer`,
`.load_shed()`, or `.buffer()` to the service stack — only `.timeout()`
remains. `ServiceStackOptions` is now `{ timeout: Option<Duration> }`.
`MailboxConfig.worker_count` is the sole concurrency knob. Callers that
need a per-resource sub-cap compose their own `ServiceBuilder` chain
before passing the service in. The `slow_worker_does_not_block_others`
test now uses `ServiceStackOptions::default()` and serves as the
regression guard.

## Public API / ergonomics

### 7. ~~Library panics on missing/invalid config~~ — PARTIALLY ADDRESSED
- `HttpClientParams::from_config`, `build_http_client` proxy, and
  `healthcheck::test_proxy` panics: intentionally left as fail-early on
  config-load-time data (deploy bug, not a library defect).
- `TaskId::from_string`: DONE — now returns `Result<Self, uuid::Error>` and
  implements `FromStr`.

### 8. ~~`healthcheck::internet` is brittle~~ — DONE
Rewritten: no `.unwrap()`, no magic 404+9 check. New contract — any HTTP
response within 1s = `true`; errors and timeouts = `false`.

### 9. `TaskId` and `Task<D>` API mixed signals
`TaskId::Display` formats as `TaskId(<uuid>)` but JSON serializes as plain
UUID — asymmetric. `Task<D>` exposes every field publicly while also providing
state-mutating setters (`set_in_progress`, `set_retry`); both tests and the
runtime then bypass them (`task.retries = 3`, `task.started_at = None`). Pick
one: plain data struct or state machine.

### 10. `Configurable` trait mixes contract with helpers
`load_config` and `load_text_file_lines` don't use `self` and have nothing to
do with the trait contract. Move them to free functions in the module; the
trait should be `config()` + `get_config_value`.

### 11. ~~`danger_accept_invalid_certs(true)` always on~~ — WONTFIX
Intentional default for crawler use cases (servers with broken/expired certs).
Revisit if/when the http module is rewritten on top of `wreq`.

### 12. README and doc drift
- README footer: "Please read non-existent guidelines and submit PRs."
- `capp/src/lib.rs` doc shows `capp = "0.5"` while real version is `0.6.2`;
  README says `0.6`.
- README still advertises "Round-Robin Processing: Fair task distribution
  across different domains" but `HasTagKey` has no consumer.
- `lib.rs` module list mentions `config`, `healthcheck`, `http`, `task` as
  modules; they're actually feature-gated re-exports.

## Dependencies

### 13. `backoff 0.4.0` is unmaintained
Last release 2021, no maintainer. Re-exported at `capp::backoff` so it leaks
into users' APIs. Migrate to `backon` (or another maintained alternative).

### 14. `tokio` `features = ["full"]` workspace-wide
Fine for binaries; bloats compile time for a library re-exported into other
apps. Pin specific features (`rt-multi-thread`, `macros`, `sync`, `time`).

### 15. Dev-dep cleanup
- `pin_project_lite` in `capp/Cargo.toml` dev-deps — no `Pin<&mut Self>`
  projections in tests that I found.
- `dotenvy` in `capp/Cargo.toml` dev-deps — no `.env` loading paths visible.

### 16. Verify `chrono` declaration in `capp-cache/Cargo.toml`
`capp-cache/src/file.rs` and `cache.rs` use `chrono`. Make sure it's declared
explicitly rather than coming in transitively.

## Code quality

### 17. Global `Mutex<()>` + `PersistMode::SyncAll` per push
`FjallTaskQueue` and `FileHttpCache` serialize all operations through one
mutex and force a sync-to-disk on every write. For a framework promising
"async parallel processing", every push/pop/ack waits on the same lock at
fsync speed. At minimum, make persistence mode configurable and document the
throughput ceiling. Consider sharding or fine-grained locks if higher
throughput is a goal.

### 18. `HasTagKey` is unfinished, not dead — wire it up
Defined in `capp-queue/src/queue.rs` but no implementation consumes it. The
intended use case is real and matches the README claim: when crawling many
sources at once, the dispatcher should pull pages from each source in
rotation (e.g. `a.com → b.com → c.com → a.com → ...`) rather than draining
all of `a.com` before touching `b.com`. Without this, one source with a
large backlog starves every other source on the queue.

Design surface (two viable layers):

- **Backend-level (preferred).** Tasks with `HasTagKey` get stored in per-tag
  sub-queues (e.g. Fjall `queue:<tag>` keyspace prefix). The backend's `pop`
  rotates the starting tag each call. Works across processes; requires
  extending the `TaskQueue` trait with something like `push_tagged` /
  `pop_round_robin`, or making round-robin the default `pop` behavior when
  the payload implements `HasTagKey`.
- **Dispatcher-level (in-process only).** Dispatcher pre-fetches into a
  per-tag staging map and emits to the shared `async-channel` in
  round-robin order. Doesn't help if `pop()` is already FIFO across tags
  and the head is dominated by one tag — needs either a peek primitive or
  large prefetch.

Either approach needs `HasTagKey::TagValue` made cheap enough to use on every
dispatch hop (currently `ToString + PartialEq` — fine for `String`, would
allocate for richer types). Probably swap to `Hash + Eq + Clone`.

Not the next-up item; flagged so it isn't deleted as "dead trait."

### 19. `capp-config/src/router.rs` is empty
Delete the file or add a real module.

### 20. `CacheEntry<T>` is HTTP-shaped despite the generic name
`status_code` and HTTP fields are baked in; `HttpCache<T>::set` requires
`status_code`. Either rename to `http_cache::HttpCacheEntry`, or make the
generic actually carry the HTTP metadata.

### 21. `.unwrap()` in `stats_http.rs` production paths
`Response::builder().body(...).unwrap()` and
`"application/json".parse().unwrap()` should use `unwrap_or_else` with a safe
fallback or be documented as unreachable.

### 22. Duplicate helpers in `capp-testkit`
`html_page` and `html_page_owned` are the same shape; consolidate behind a
generic over `AsRef<str>`.

### 23. `task_id_from_bytes` in fjall `pop()`
Constructed only to populate a `TaskNotFound` error; the deserialized task has
its own `task_id`. Minor cleanup.

### 24. ~~Defensive worker-index access~~ — OBSOLETE
The per-worker inbox array was removed in the shared-channel refactor; the
indexing concern no longer applies.

## Tests

### 25. Runtime integration coverage is thin
`mailbox.rs` unit-tests individual handlers. Missing end-to-end tests for:
pause → resume mid-flight, Stop draining with multiple inflight tasks, the
retry cap path through `spawn_mailbox_runtime`, producer-closed-but-tasks-pending.

### 26. `test_proxy` hits `httpbin.org` from a unit test
Flaky in CI without internet, depends on a third party. Mark `#[ignore]` or
point at `capp-testkit`.

### 27. Benches have no recorded baseline
`capp-queue/benches/*` exist but no published numbers to detect regressions.
Optional.

## Priority order for the next refactor pass

1. ~~Mailbox runtime: shared-channel consumption (item 1).~~ Done.
2. ~~Fjall durability: in-flight recovery / visibility timeouts (item 2).~~
   Done (recovery; visibility timeouts still open).
3. ~~Tower service stack defaults (item 6a).~~ Done.
4. ~~Drop fake `queue_depth` metric (item 3).~~ Done.
5. **Library panic surface: convert `expect`/`unwrap` on user-supplied data
   to `Result` (items 7, 8, 11).** Next up.
6. Dispatcher `set` failure silently drops popped task (item 4, remaining
   half).
7. Doc / dead-code cleanup (items 12, 19).
8. Dependency hygiene: drop `backoff`, slim `tokio` features (items 13, 14).
9. Implement `HasTagKey` round-robin (item 18).
10. Persistence/lock granularity (item 17).
