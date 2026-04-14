% Current Context

# Purpose

This document captures the current state of `capp-rs` and the working
direction for the next storage cleanup. It replaces older session notes that
described work which has already landed in the tree.

# Current State

## Runtime and execution model

- The mailbox runtime is the primary modern execution path.
- Queue I/O is centralized in the dispatcher; workers execute tasks through a
  Tower service stack.
- The legacy `WorkersManager` and `Computation` flow have been removed.

See also:
- `specs/mailbox.md`
- `specs/migration-0.6.md`

## Storage backends

### Queue

Current queue backends in the repository:

- `FjallTaskQueue`: the default persistent backend.
- `InMemoryTaskQueue`: useful for tests, examples, and local development.

Notes:

- Fjall is already the default feature in `capp-queue`.
- In-memory queue is not considered a persistence backend and should stay for
  tests and lightweight examples.
- There is no active Redis backend in this repository.
- There is no active PostgreSQL backend in this repository.

### Cache

- `capp-cache` now uses a file-backed cache implementation with a Fjall metadata
  index.
- Full serialized `CacheEntry<T>` records are stored on disk.
- Cache files are segmented by date so they can be pruned manually by
  year/month/day when needed.
- The cache crate no longer depends on MongoDB or BSON.

## Configuration

- Config is TOML-only.
- There is currently no dedicated cache storage configuration surface for
  selecting a cache directory or cache backend.
- Adding file-backed cache support will require a small `[cache]` section in the
  config model or a similarly explicit API.

## Observability

- OTLP metrics support is implemented behind the `observability` feature.
- Example wiring exists and is already documented in `specs/monitoring.md`.

# Decisions

## Persistent storage direction

The intended direction is:

- Fjall becomes the only supported persistent storage backend in the workspace.
- In-memory storage remains available for tests, examples, and non-durable
  local use.
- The codebase should remain structurally extensible so that additional
  backends can be added later if there is a real need.
- The legacy `WorkersManager` and `Computation` path has been removed so the
  runtime model is no longer split across two orchestration styles.

This means:

- remove MongoDB from queue and cache implementations
- remove MongoDB-related features, tests, benches, and docs
- keep the public traits clean so future backend additions remain possible

## Cache direction

The cache should move to file-backed storage with a simple default layout:

- default cache dir: `./.cache`
- configurable cache dir later through config
- file storage for cached payloads
- optional/simple Fjall index for metadata and lookup
- date-based directory segmentation so cache files can be managed manually by
  year/month/day when needed

Initial implementation should optimize for simplicity and predictable local
operation, not backend generality.

Cleanup behavior for the first version should stay narrow:

- built-in cleanup by cache key
- built-in delete-all support
- broader date-based deletion can be handled manually by the user via the cache
  directory layout
- richer cleanup/index capabilities can be added later if needed

# Near-Term Plan

## 1. Legacy worker path cleanup

- completed: `WorkersManager` and the older `Computation`-driven flow removed
- completed: examples and docs moved onto the mailbox runtime
- completed: compatibility language for the legacy worker model removed

## 2. Make `capp-cache` storage-neutral

- completed: MongoDB-specific error variants removed from core cache errors
- completed: BSON-only serde helpers removed from cache entry types
- completed: `HttpCache<T>` kept as the abstraction point

## 3. Add file-backed cache with Fjall metadata

- completed: full serialized `CacheEntry<T>` records stored under `.cache`
- completed: lookup/cleanup metadata stored in a small Fjall keyspace
- completed: cache supports:
  - `get`
  - `set`
  - `update_state`
  - `remove`
  - `cleanup`
  - `contains`
  - `clear_all` as an implementation-specific helper

## 4. Add explicit cache configuration

Proposed initial TOML shape:

```toml
[cache]
enabled = true
dir = ".cache"
```

This should live in `capp-config`, not as caller-managed ad hoc setup.

Avoid adding generic backend selection until a second real cache backend exists.

## 5. Refresh documentation

- rewrite README storage language around Fjall + in-memory
- update overview and migration notes
- keep this file focused on live architectural context rather than one-off
  status notes

# Implementation Stages

## Stage 1. Queue backend cleanup

- completed: `MongoTaskQueue` removed
- completed: MongoDB-related queue tests and benchmarks removed
- completed: `mongodb` feature flags and dependencies dropped from
  `capp-queue` and `capp`
- completed: queue exports, errors, and serializers simplified after MongoDB
  removal
- keep `FjallTaskQueue` and `InMemoryTaskQueue`

Exit criteria:

- workspace no longer contains a MongoDB queue backend
- queue crate builds and tests without MongoDB-related code paths

## Stage 2. Legacy runtime cleanup

- completed: `WorkersManager` removed
- completed: old `Computation`-based orchestration flow removed
- completed: examples and docs updated to use mailbox runtime only
- completed: compatibility wording for the legacy worker path removed

Exit criteria:

- mailbox runtime is the only documented orchestration model
- legacy manager code is deleted from the workspace

## Stage 3. Cache core redesign

- completed: `capp-cache` made storage-neutral at the type level
- completed: MongoDB-specific cache errors removed
- completed: BSON-specific serialization assumptions removed from core cache types
- completed: `HttpCache<T>` preserved as the backend boundary

Exit criteria:

- `capp-cache` core types no longer depend on MongoDB/BSON
- cache crate is ready to host a file-backed backend cleanly

## Stage 4. File-backed cache implementation

- completed: file-backed cache added under `./.cache`
- completed: full serialized `CacheEntry<T>` records stored on disk
- completed: files organized with date-based segmentation
- completed: simple Fjall metadata index added for lookup and state tracking
- completed: supports `get`, `set`, `update_state`, `remove`, `cleanup`, and `contains`
- completed: cleanup built-ins limited to delete-by-key and delete-all

Exit criteria:

- cache works locally without MongoDB
- users can inspect and manually prune cache files by directory layout

## Stage 5. Configuration integration

- add cache configuration to `capp-config`
- start with a minimal config surface such as:

```toml
[cache]
enabled = true
dir = ".cache"
```

- wire config usage into examples and docs
- avoid generic backend selection at this stage

Exit criteria:

- cache path is configured through `capp-config`
- examples and docs use the new config model

## Stage 6. Documentation and verification

- rewrite README storage sections around Fjall + in-memory + file cache
- update overview/migration docs to remove MongoDB language
- add or update tests for Fjall queue and file cache behavior
- run fmt, clippy, and relevant tests after each major cleanup stage

Exit criteria:

- docs match the codebase
- no MongoDB-specific storage language remains in primary docs
- core queue/cache paths are covered by tests

# Constraints

- Breaking changes are acceptable within the current `0.6` cleanup work.
- Avoid unnecessary backend abstraction layers beyond the existing traits.
- Prefer simple local storage semantics over distributed-database features.
- Do not remove `InMemoryTaskQueue`; it serves a different purpose than Fjall.

# Open Questions

- Whether the first Fjall cache index should only track key lookup and state, or
  also include extra metadata to support future administrative operations.
  
