# capp-rs refactoring backlog

Open items only. DONE items from earlier passes (mailbox shared-channel
consumption, in-flight recovery, queue_depth removal, tower stack
defaults, `tokio::select! biased`, worker-pool removal in 0.7) live in
git history and `CHANGELOG.md`.

## Architecture / correctness

### 1. Wasted `set` writes around lifecycle transitions
After `pop()`, dispatcher calls `task.set_in_progress(); queue.set(&task)`.
If `set` fails the popped task is silently dropped (`continue`). Should
either be skipped or persisted transactionally with the pop.

## Public API / ergonomics

### 2. Library panics on missing/invalid config
- `HttpClientParams::from_config`, `build_http_client` proxy, and
  `healthcheck::test_proxy` panics: intentionally left as fail-early on
  config-load-time data (deploy bug, not a library defect). Re-evaluate.

### 3. `TaskId` and `Task<D>` API mixed signals
`TaskId::Display` formats as `TaskId(<uuid>)` but JSON serializes as
plain UUID — asymmetric. `Task<D>` exposes every field publicly while
also providing state-mutating setters (`set_in_progress`, `set_retry`);
both tests and the runtime then bypass them (`task.retries = 3`,
`task.started_at = None`). Pick one: plain data struct or state machine.

### 4. `Configurable` trait mixes contract with helpers
`load_config` and `load_text_file_lines` don't use `self` and have
nothing to do with the trait contract. Move them to free functions in
the module; the trait should be `config()` + `get_config_value`.

### 5. README and doc drift
- README footer: "Please read non-existent guidelines and submit PRs."
- `capp/src/lib.rs` doc shows `capp = "0.5"` while real version is
  `0.7.0`.
- README still advertises "Round-Robin Processing: Fair task
  distribution across different domains" but `HasTagKey` has no
  consumer (see item 9).
- `lib.rs` module list mentions `config`, `healthcheck`, `http`, `task`
  as modules; they're actually feature-gated re-exports.

## Dependencies

### 6. `backoff 0.4.0` is unmaintained
Last release 2021, no maintainer. Re-exported at `capp::backoff` so it
leaks into users' APIs. Migrate to `backon` (or another maintained
alternative).

### 7. `tokio` `features = ["full"]` workspace-wide
Fine for binaries; bloats compile time for a library re-exported into
other apps. Pin specific features (`rt-multi-thread`, `macros`, `sync`,
`time`).

### 8. Dev-dep cleanup
- `pin_project_lite` in `capp/Cargo.toml` dev-deps — no `Pin<&mut Self>`
  projections in tests that I found.
- `dotenvy` in `capp/Cargo.toml` dev-deps — no `.env` loading paths
  visible.

## Code quality

### 9. `HasTagKey` is unfinished, not dead — wire it up
Defined in `capp-queue/src/queue.rs` but no implementation consumes it.
Intended use case: when crawling many sources at once, the dispatcher
should pull pages from each source in rotation (e.g. `a.com → b.com →
c.com → a.com → ...`) rather than draining all of `a.com` before
touching `b.com`. Without this, one source with a large backlog
starves every other source on the queue.

Design surface (two viable layers):

- **Backend-level (preferred).** Tasks with `HasTagKey` get stored in
  per-tag sub-queues (e.g. Fjall `queue:<tag>` keyspace prefix). The
  backend's `pop` rotates the starting tag each call. Works across
  processes; requires extending the `TaskQueue` trait with something
  like `push_tagged` / `pop_round_robin`, or making round-robin the
  default `pop` behavior when the payload implements `HasTagKey`.
- **Dispatcher-level (in-process only).** Dispatcher pre-fetches into
  a per-tag staging map and emits in round-robin order. Doesn't help
  if `pop()` is already FIFO across tags and the head is dominated by
  one tag — needs either a peek primitive or large prefetch.

Either approach needs `HasTagKey::TagValue` made cheap enough to use on
every dispatch hop (currently `ToString + PartialEq` — fine for
`String`, would allocate for richer types). Probably swap to
`Hash + Eq + Clone`.

### 10. Global `Mutex<()>` + `PersistMode::SyncAll` per push
`FjallTaskQueue` and `FileHttpCache` serialize all operations through
one mutex and force a sync-to-disk on every write. For a framework
promising "async parallel processing", every push/pop/ack waits on the
same lock at fsync speed. At minimum, make persistence mode
configurable and document the throughput ceiling. Consider sharding or
fine-grained locks if higher throughput is a goal.

### 11. `capp-config/src/router.rs` is empty
Delete the file or add a real module.

### 12. `CacheEntry<T>` is HTTP-shaped despite the generic name
`status_code` and HTTP fields are baked in; `HttpCache<T>::set`
requires `status_code`. Either rename to `http_cache::HttpCacheEntry`,
or make the generic actually carry the HTTP metadata.

### 13. `.unwrap()` in `stats_http.rs` production paths
`Response::builder().body(...).unwrap()` and
`"application/json".parse().unwrap()` should use `unwrap_or_else` with
a safe fallback or be documented as unreachable.

### 14. Duplicate helpers in `capp-testkit`
`html_page` and `html_page_owned` are the same shape; consolidate
behind a generic over `AsRef<str>`.

### 15. `task_id_from_bytes` in fjall `pop()`
Constructed only to populate a `TaskNotFound` error; the deserialized
task has its own `task_id`. Minor cleanup.

## Tests

### 16. Runtime integration coverage is thin
Missing end-to-end tests for: pause → resume mid-flight, Stop draining
with multiple in-flight tasks, the retry cap path through
`spawn_mailbox_runtime`, producer-closed-but-tasks-pending.

### 17. `test_proxy` hits `httpbin.org` from a unit test
Flaky in CI without internet, depends on a third party. Mark
`#[ignore]` or point at `capp-testkit`.

### 18. Benches have no recorded baseline
`capp-queue/benches/*` exist but no published numbers to detect
regressions. Optional.

## Priority for the next pass

1. Library panic surface (items 2, 5 doc parts).
2. Dispatcher `set` failure handling (item 1).
3. Doc / dead-code cleanup (items 5, 11).
4. Dependency hygiene: drop `backoff`, slim `tokio` features (items
   6, 7).
5. `HasTagKey` round-robin (item 9).
6. Persistence/lock granularity (item 10).
