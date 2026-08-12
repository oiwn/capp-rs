# Current Context

## Task: fix FjallRoundRobinTaskQueue starvation under shared concurrency-limited consumption

Status: spec only, no implementation yet.

### Context

`woid-gather` crawls 8 tagged sources through one shared
`FjallRoundRobinTaskQueue` + one concurrency-limited worker pool
(`concurrency_limit == sources.len() == 8`). Some sources get starved of
worker time even though their own backlog is small and cheap to drain,
while the same source crawled alone (dedicated queue) drains in seconds.

Root cause, confirmed by reading `capp-queue/src/backend/fjall_round_robin.rs`
(this repo) and its test suite — **not a race condition, a design choice**,
committed in `7a37bc5` (the only commit touching this file) and directly
asserted by tests `drained_tag_is_evicted` / `redrained_tag_rejoins_on_push`:

- `pop()` rotates through `RotationState.active: Vec<String>` (tags) via a
  `cursor`. After popping a task, if that tag's queue prefix is now empty,
  the tag is **evicted from `active` entirely** (`evict_at`, lines
  151-158 and 174-179).
- A tag only rejoins via `push()` → `RotationState::ensure()`, which
  **appends it to the tail** of `active` (line 43-47).
- `QueueEmpty` is returned only when `active.is_empty()` (line 144-146).

At `concurrency_limit ≈ tag_count` (the crawler's `worker_count =
DEFAULT_WORKER_COUNT.max(sources.len())` heuristic gives exactly 8 for 8
sources), each tag typically has only ~1 concurrent fetch feeding it, so
its queue backlog constantly grazes 0-1 items. Every time it does, the tag
is evicted and has to wait behind every other currently-resident tag
(re-admitted at the tail) before it gets served again. Tags whose fanout
keeps ≥2 items queued at all times never get evicted and receive a clean,
fair turn every rotation — this explains the observed bias toward
high-fanout sources and the report's own repro numbers.

Sibling backends (`FjallTaskQueue`, `InMemoryTaskQueue`) return
`QueueEmpty` iff and only iff there is truly zero pending work anywhere —
plain FIFO, no rotation state. The round-robin backend's `QueueEmpty`
contract should match that exactly; today it's only equivalent by
accident of the eviction design.

**Consumer contract that must not break** (`capp/src/manager/mailbox.rs`,
`dispatcher_loop`, lines ~300-374 — the only pop-consumer loop in this
repo, backend-agnostic): `pop()` is called once per iteration, throttled
by `service.ready()`/`concurrency_limit`. `Err(QueueEmpty)` is a normal,
cheap, expected result that triggers `sleep(dequeue_backoff)` and retry —
**and**, when `stop_when_idle: true`, `QueueEmpty` combined with
`saw_work && in_flight == 0` permanently stops the dispatcher. So
`QueueEmpty` must only ever mean "no recoverable work exists anywhere,"
never "the tag whose turn it is happens to be empty right now" — a
mismatch here would make finite crawls with `stop_when_idle: true`
terminate early while other tags still have queued work.

Research (deficit round robin / weighted fair queueing literature, and
Inngest's public writeup on their multi-tenant queue,
https://www.inngest.com/blog/building-the-inngest-queue-pt-i-fairness-multi-tenancy)
confirms the standard fix for this class of bug: don't tie a tenant/tag's
membership in the rotation to whether its queue is momentarily empty.
Keep a stable index of known tags and skip (not evict) a tag with no
current work; only treat "no work anywhere" as the terminal signal. This
is also just what the module's own doc comment already promises ("Pops
rotate across tags so one busy tag can't starve the others") — the
current implementation just doesn't deliver on it under this exact
worker:tag ratio.

Decision: fix it properly in `capp-queue` (Option A — stop evicting on
transient drain), not by tuning worker counts on the caller side.

### Approach

Replace evict-on-drain with scan-and-skip. `pop()` walks forward from
`cursor` through at most `active.len()` tags, returns the first one with
a queued item, and only advances `cursor` past a tag it actually served.
No tag is ever removed from `active` inside `pop()`. `QueueEmpty` is
returned only after a full lap finds nothing — which, given `push()`
always calls `ensure()` in the same locked critical section that inserts
the queue key, remains equivalent to "no queued item exists for any known
tag," matching the sibling backends' contract exactly.

```rust
async fn pop(&self) -> Result<Task<D>, TaskQueueError> {
    let mut state = self.state.lock().unwrap();
    if state.active.is_empty() {
        return Err(TaskQueueError::QueueEmpty);
    }
    let n = state.active.len();
    for step in 0..n {
        let idx = (state.cursor + step) % n;
        let tag = state.active[idx].clone();
        let prefix = encode_tag_prefix(&tag);

        let Some(guard) = self.queue.prefix(&prefix).next() else {
            continue; // transiently empty: skip, do not evict
        };
        let (key, _) = guard.into_inner()?;
        let task_id = extract_task_id_from_key(&key)?;
        let task_id_bytes = Self::task_id_to_bytes(&task_id);

        self.queue.remove(key.as_ref())?;
        self.inflight.insert(task_id_bytes, encode_tag_blob(&tag))?;
        self.db.persist(PersistMode::SyncAll)?;

        let task_bytes = self.tasks.get(task_id_bytes)?
            .ok_or(TaskQueueError::TaskNotFound(task_id))?;
        let task = S::deserialize_task::<D>(&task_bytes)?;

        state.cursor = (idx + 1) % n;
        return Ok(task);
    }
    Err(TaskQueueError::QueueEmpty)
}
```

Why this gives real fairness, not just "less bad": `cursor` only advances
past a tag when that tag is actually served. If a tag is found empty on a
given `pop()` call, the *next* `pop()` call (by any worker) starts
scanning from the same `cursor` and hits that tag first again. So a tag
with pending work is checked on literally every subsequent `pop()` until
it's served — it can never be skipped indefinitely while other tags cycle
past it. This is strictly stronger than "one skip per lap"; it's the
ideal round-robin the doc comment already claims.

Consequences to spell out before implementing:
- `RotationState::evict_at` becomes dead code under this design (nothing
  in `pop()` calls it anymore) — remove it, or repurpose it only if we
  add an explicit "close tag" API (see Out of scope below).
- `RotationState::ensure` is unchanged — still needed by `push()` and
  `recover_inflight()` to register a tag the first time it's seen.
- The `warn!(...); evict_at(idx); continue` defensive branch (old lines
  151-158) goes away entirely — a prefix-scan miss becomes the ordinary,
  expected "skip" path, not a warning-worthy anomaly.
- `active` now only ever grows (tags are never pruned automatically).
  For the crawler's use case (≤ dozens of source tags, one queue per
  crawl run) this is a non-issue — full-lap cost is O(tag count), trivial.
  Flag as a known trade-off, not something to solve now: if this backend
  is ever used with high-cardinality/churning tags, unbounded `active`
  growth would need an explicit prune/close-tag API. **Out of scope for
  this fix.**

### Critical files (for the implementation session)

- `capp-queue/src/backend/fjall_round_robin.rs` — core change: `pop()`
  rewritten as above; `evict_at` removed; module doc comment (lines
  11-14) updated to describe scan-and-skip instead of implying eviction;
  `RotationState` struct comment updated if needed.
- `capp-queue/tests/round_robin.rs` — test fallout:
  - `drained_tag_is_evicted` (lines 82-101): rewrite. Under the new
    design the tag is never removed from `active`; rename to something
    like `drained_tag_is_skipped_not_evicted` and assert the tag stays
    present in rotation and is immediately poppable again once it has
    new work, without needing to "rejoin."
  - `redrained_tag_rejoins_on_push` (lines 103-118): rewrite for the same
    reason — there's no evict/rejoin cycle anymore, so assert a pushed
    task to an already-tracked-but-currently-empty tag becomes poppable
    in its existing rotation slot (sequence expectations will differ from
    today's asserted `b, a, b`).
  - `pop_drains_remaining_when_some_tags_empty` (lines 120-138): re-audit;
    likely still passes unchanged for this exact input shape, but the
    inline comment ("b joined active after a") needs correcting since `b`
    is no longer evicted.
  - All other existing tests (`pop_rotates_across_tags`,
    `pop_skips_unpushed_tags`, `new_tag_joins_rotation_mid_run`,
    `recover_inflight_restores_per_tag`,
    `restart_recovers_tag_list_from_queue`, `concurrent_push_pop_fair`,
    and the generic TaskQueue contract tests) are structurally
    independent of the eviction mechanism and should keep passing as-is.
  - Add a new regression test reproducing the reported bug directly: a
    tag whose backlog is kept at 0-1 items by a slow concurrent producer,
    alongside a tag with a deep/fast-refilling backlog and several
    concurrent poppers (mirroring `concurrent_push_pop_fair`'s shape but
    with an uneven producer rate) — assert the thin tag is never skipped
    for more than one full lap in a row / gets served within `n` pops of
    having work available, not "eventually, after however many
    evict/rejoin cycles."
- No other files in this repo consume `FjallRoundRobinTaskQueue` (grep
  confirmed) or depend on its `QueueEmpty` timing beyond the generic
  `dispatcher_loop` contract described above, which this change
  preserves exactly. No changes needed in `capp/src/manager/mailbox.rs`
  or any example.
- `CHANGELOG.md`: add an entry (non-breaking bug fix — `pop()`/`push()`
  signatures and `TaskQueue` trait are unchanged, only internal fairness
  behavior changes) — likely a `0.7.1` patch bump in `Cargo.toml`
  (`version.workspace = true`, root `Cargo.toml:14`), described as
  "fix: round-robin queue no longer evicts a tag on transient drain,
  eliminating tail-of-rotation starvation for low-backlog tags."

### Out of scope

- Any change to `woid-gather` itself (not checked out in this workspace).
  Real-world validation of this fix happens there, after bumping the
  `capp-rs` git dependency, by re-running the original 8-source combined
  crawl and confirming source-level `visited`/`stored` parity improves
  relative to the isolated single-source baselines.
- The separate anomaly noted in the original bug report — `cryptoslate`
  visiting 737 URLs (highest of all 8 sources) while storing 0 — does not
  fit a starvation explanation (a starved tag would show *low* visit
  counts, not the highest) and is very likely an unrelated bug in
  `woid-gather`'s crawl/classification logic, not in `capp-queue`. Not
  addressed by this plan; would need `woid-gather` source access to
  investigate.
- Explicit tag close/prune API — noted as a future extension point only,
  not needed at the crawler's tag cardinality.

### Verification (for the implementation session)

- `cargo test -p capp-queue --features fjall` (full crate test suite).
- `cargo test -p capp-queue --test round_robin --features fjall`
  (targeted: rewritten eviction tests + new starvation regression test).
- `cargo clippy -p capp-queue --features fjall` (catch the now-dead
  `evict_at` if not removed).
- Manual sanity: a quick loop pushing to 2 tags at very different rates
  (one steady producer, one bursty) and popping concurrently, confirming
  neither tag's pop gap exceeds `active.len()` pops — this is what the
  new regression test should encode formally.
