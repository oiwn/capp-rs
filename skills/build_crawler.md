---
name: build_crawler
description: Build a capp-rs crawler for a user-named target website. Recon the site with pginf, design a task model from the URL groups it reveals, then assemble the queue + mailbox + tower pieces into a durable crawler.
argument-hint: <target-url>
allowed-tools: Bash, Read, Write, Edit
---

`build_crawler` produces a working web crawler on top of `capp-rs`. The
skill assumes the user has `pginf` installed and on PATH
(`cargo install pageinfo-rs`). It does *not* prescribe where the crawler
lives in the user's project — ask the user, or follow their conventions.

## When to use

User names a target site and wants a Rust crawler that persists structured
records. Examples: "build a crawler for example.com", "crawl this forum and
store the posts", "scrape the listings on site X".

Out of scope (ask the user, do not guess):

- JS-rendered sites (no DOM after first HTTP response)
- Login / session flows
- robots.txt enforcement
- Anything behind a WAF that 403s on `pginf fetch`

## Workflow

### 1. Recon with pginf

Probe the target before writing a line of code. The goal is to fill in
this checklist; record findings somewhere the user can review.

```
pginf fetch <url>                         # reachable? status? server? body size?
pginf links <url> --format toon           # URL groups, internal/external split, pagination
pginf meta <url> --format toon            # title, canonical, feed URLs
pginf json <url>                          # JSON-LD / Next.js data (if present, prefer it)
pginf html -u <url> -s "<selector>"       # preview a CSS selector against real HTML
```

For pagination: look at the footer of `pginf links … | tail` for
`rel="next"` or numeric `?page=`/`?p=` patterns.

For each *page kind* the site has (listing, detail, comments, category,
search, …) repeat the `pginf html -u … -s "<selector>"` probe until the
selectors you'll use in the crawler are confirmed against live HTML.

### 2. Recon checklist — write this down for the user

Before designing the task model, you must be able to answer:

- Entry URLs (seeds).
- Page kinds and their URL patterns.
- Pagination shape (link, query param, infinite scroll = ask user).
- For each page kind: the selectors that yield the records you want.
- Whether structured data (JSON-LD, Next.js `__NEXT_DATA__`) replaces
  scraping for some pages.
- Stable record id (e.g. numeric story id, slug). If there is none, you
  will need to synthesize one — flag this to the user.
- Rate-limit signals: response time, body size, `Retry-After` on `pginf
  fetch --refresh`.

### 3. Common gotchas seen in real sites

- **Implicit tree structure.** Comment / reply pages often render flat
  rows with an `indent` attribute or depth class; the tree is
  reconstructed from row *order + depth*, not from a parent-id field.
- **Paired rows.** A record's data may live in two adjacent `<tr>`s
  (e.g. one for title, one for metadata). Pair them by a shared id
  embedded in element ids (`score_<id>`, `up_<id>`), not by DOM
  adjacency alone — ads/separators break adjacency.
- **`&nbsp;` in counts.** `"37&nbsp;comments"` — parse the leading
  digits, don't split on whitespace.
- **Special rows.** Sponsored / job / dead entries often lack the
  selectors normal rows have. Detect and skip, or tag separately.
- **Big detail pages.** A listing row may be 1 KB but its detail page
  500 KB+ — concurrency limits should reflect this.

### 4. Task model

One typed enum, one variant per page kind:

```rust
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
enum CrawlTask {
    Listing { page: u32 },
    Detail  { id: String },
    // … one variant per page kind you found in recon
}
```

Workers receive a `CrawlTask`, match on the variant, fetch + parse, and
enqueue freshly discovered tasks via the producer handle. Detail tasks
write parsed records to the data store before returning.

### 5. Persistence

Two Fjall instances side by side (the queue owns its own keyspaces):

```rust
// Queue — capp-queue manages tasks/queue/inflight/dlq inside this dir.
let queue = Arc::new(
    FjallTaskQueue::<CrawlTask, JsonSerializer>::open("./<name>-queue.fjall")?
);

// Data store — your own Fjall DB at a separate path, one keyspace
// per record kind (stories, comments, …). Keys = stable record id
// as bytes; values = JSON.
let db = fjall::Config::new("./<name>-data.fjall").open()?;
let stories  = db.open_partition("stories",  Default::default())?;
let comments = db.open_partition("comments", Default::default())?;
```

Idempotency: before fetching, check the data partition for the record id.
Skip if present unless the user asked for a refresh mode.

### 6. Cargo dependencies

Minimum for a Fjall-backed HTTP crawler against capp-rs ≥ 0.7:

```toml
[dependencies]
capp        = { version = "0.7", features = ["http"] }   # reqwest re-exported
capp-queue  = { version = "0.7" }                        # FjallTaskQueue
tokio       = { version = "1.51", features = ["full"] }
tower       = { version = "0.5", features = ["util", "timeout", "limit"] }
fjall       = "3.1"
serde       = { version = "1", features = ["derive"] }
serde_json  = "1"
anyhow      = "1"
tracing             = "0.1"
tracing-subscriber  = "0.3"
scraper     = "0.26"   # if you scrape HTML
url         = "2.5"
```

Add `features = ["http", "observability"]` to `capp` if you want OTLP
metrics, and `features = ["http", "stats-http"]` for the built-in JSON
stats endpoint.

### 7. Assemble the crawler

Architecture in one line: **one dispatcher task owns one tower service;
concurrency and rate-limit are layers on that service.** No worker
pool, no `Clone` requirement, no `Buffer` shim.

The canonical reference is `examples/hackernews/main.rs` — read it
before adapting. The skeleton below is paste-ready and mirrors that
example's structure with the two handler patterns (listing-that-fans-out
and detail-that-stores).

```rust
use std::{path::PathBuf, sync::Arc, time::Duration};

use anyhow::{Context as _, Result};
use capp::{
    manager::{
        MailboxConfig, ServiceRequest, ServiceStackOptions,
        build_service_stack, spawn_mailbox_runtime,
    },
    queue::{JsonSerializer, Task},
    tracing, tracing_subscriber,
};
use capp_queue::FjallTaskQueue;
use reqwest::Client;
use tower::{BoxError, ServiceBuilder, service_fn};

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
enum CrawlTask {
    Listing { page: u32 },
    Detail  { id: u64 },
}

struct CrawlContext {
    client: Client,
    data:   DataStore,        // your own Fjall wrapper; see §5
}

const BASE: &str       = "https://example.com";
const MAX_PAGES: u32   = 5;
const DATA_DIR: &str   = ".crawler";

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    std::fs::create_dir_all(DATA_DIR).context("create data dir")?;

    let queue_path: PathBuf = format!("{DATA_DIR}/queue.fjall").into();
    let data_path:  PathBuf = format!("{DATA_DIR}/data.fjall").into();

    let queue = Arc::new(
        FjallTaskQueue::<CrawlTask, JsonSerializer>::open(&queue_path)
            .context("open queue")?,
    );

    let ctx = Arc::new(CrawlContext {
        client: Client::builder()
            .user_agent("my-crawler/0.1 (+https://github.com/me/proj)")
            .timeout(Duration::from_secs(15))
            .build()
            .context("build reqwest client")?,
        data: DataStore::open(&data_path)?,
    });

    // Tower stack: politeness lives here, not in MailboxConfig.
    let inner = ServiceBuilder::new()
        .concurrency_limit(2)                       // ≤ 2 in-flight requests
        .rate_limit(2, Duration::from_secs(1))      // ≤ 2 req/sec
        .service(service_fn(
            move |req: ServiceRequest<CrawlTask, CrawlContext>| async move {
                match req.task.payload.clone() {
                    CrawlTask::Listing { page } => handle_listing(&req, page).await,
                    CrawlTask::Detail  { id   } => handle_detail(&req, id).await,
                }
                .map_err(|e: anyhow::Error| -> BoxError { e.into() })
            },
        ));

    let service = build_service_stack(
        inner,
        ServiceStackOptions { timeout: Some(Duration::from_secs(30)) },
    );

    let runtime = spawn_mailbox_runtime(
        queue,
        ctx.clone(),
        service,
        MailboxConfig {
            producer_buffer: 256,
            result_buffer:   256,
            max_retries:     1,
            dequeue_backoff: Duration::from_millis(100),
            stop_when_idle:  true,   // finite crawl; set false for long-running
        },
    );

    // Seed.
    runtime
        .producer
        .enqueue(Task::new(CrawlTask::Listing { page: 1 }))
        .await
        .context("seed enqueue")?;

    runtime.join().await;          // returns once stop_when_idle fires

    tracing::info!(
        stories  = ctx.data.story_count(),
        details  = ctx.data.detail_count(),
        "crawl complete"
    );
    Ok(())
}

async fn handle_listing(
    req: &ServiceRequest<CrawlTask, CrawlContext>,
    page: u32,
) -> Result<()> {
    let url      = format!("{BASE}/list?p={page}");
    let response = req.ctx.client.get(&url).send().await?;
    let status   = response.status();
    let body     = response.text().await?;

    // Guard 1: HTTP status. Without this, an HTML error page parses to
    // zero records, the handler silently Ok()s, and the dispatcher
    // auto-stops with no work done.
    if !status.is_success() {
        anyhow::bail!("listing p={page}: HTTP {status} ({} bytes)", body.len());
    }

    let listing = parse_listing(&body);

    // Guard 2: empty parse on a 200 is suspicious — likely markup
    // change or anti-bot page. Bail so the queue retries.
    if listing.ids.is_empty() {
        anyhow::bail!(
            "listing p={page}: 0 records from {} bytes", body.len()
        );
    }

    let mut new_items = 0u32;
    for id in listing.ids {
        if req.ctx.data.has_detail(id)? { continue; }       // idempotent
        req.producer.enqueue(Task::new(CrawlTask::Detail { id })).await?;
        new_items += 1;
    }
    if let Some(next) = listing.next_page
        && next <= MAX_PAGES
    {
        req.producer
            .enqueue(Task::new(CrawlTask::Listing { page: next }))
            .await?;
    }

    tracing::info!(page, new_items, "listing done");
    Ok(())
}

async fn handle_detail(
    req: &ServiceRequest<CrawlTask, CrawlContext>,
    id: u64,
) -> Result<()> {
    let url      = format!("{BASE}/item/{id}");
    let response = req.ctx.client.get(&url).send().await?;
    let status   = response.status();
    let body     = response.text().await?;

    if !status.is_success() {
        anyhow::bail!("detail {id}: HTTP {status} ({} bytes)", body.len());
    }

    let Some(detail) = parse_detail(id, &body) else {
        // Selector miss — log but don't retry. (Use bail! instead if
        // empty parses should be retried.)
        tracing::warn!(id, body_len = body.len(), "parser miss");
        return Ok(());
    };

    req.ctx.data.put_detail(&detail)?;
    tracing::info!(id, "detail stored");
    Ok(())
}
```

Tunables to think about per site:

- `concurrency_limit(N)` — start at 2; raise once you see clean
  responses. Detail pages are typically much heavier than listings.
- `rate_limit(N, period)` — most polite-site target is `(2, 1s)` or
  `(1, 1s)`. Stock tower layer; no external dep.
- `max_retries` — `1` is usually enough for transient HTTP errors.
  Permanent failures land in the DLQ partition.
- `stop_when_idle: true` for one-shot crawls (the example above);
  `false` for daemons that should keep listening on `producer`.

### 7a. Handler hygiene — fail loudly, retry through the queue

The dispatcher Nacks any handler that returns `Err`, and the
queue-level retry path (`MailboxConfig::max_retries`) re-pushes the
task. Use this — *don't* silently `Ok(())` when something looks wrong.
The two guards already shown in `handle_listing` above (status check
+ empty-parse check) are the minimum bar. Without them, an
anti-bot page or a 403 parses to nothing, the handler Ok()s, the
dispatcher decrements in-flight, the queue empties, and
`stop_when_idle: true` cleanly auto-stops with zero records stored.

The `req.attempt` field tells you which retry you're on (`1` on first
try). Use it to skip side-effects on retries, or to log differently:

```rust
if req.attempt > 1 {
    tracing::warn!(id, attempt = req.attempt, "retrying");
}
```

### 7b. Observing progress

`runtime.stats` is a `watch::Receiver<StatsSnapshot>` with flat counters
(`processed`, `succeeded`, `failed`, `terminal_failures`, `in_flight`,
`last_latency`). Drive it from another tokio task:

```rust
let mut stats_rx = runtime.stats.clone();
tokio::spawn(async move {
    while stats_rx.changed().await.is_ok() {
        let s = stats_rx.borrow().clone();
        tracing::info!(
            processed   = s.processed,
            succeeded   = s.succeeded,
            failed      = s.failed,
            in_flight   = s.in_flight,
            dlq         = s.terminal_failures,
            "stats"
        );
    }
});
```

For an HTTP endpoint serving the snapshot as JSON, enable
`features = ["stats-http"]` and use `capp::stats_http::serve_stats(addr,
runtime.stats.clone())`. See `examples/mailbox_stats_http.rs`.

For OTLP metrics, enable `features = ["observability"]` and call
`capp::observability::init_metrics(...)`. See
`examples/mailbox_metrics.rs`.

### 7c. Shutdown

- **One-shot crawl**: set `stop_when_idle: true`, seed the queue, call
  `runtime.join().await`. The runtime exits when the queue is empty
  *and* no spawned futures remain. The example above uses this pattern.
- **Long-running**: set `stop_when_idle: false`. Wire Ctrl+C to
  `runtime.control.send(ControlCommand::Stop)` (see
  `examples/mailbox.rs` for a two-press graceful/forced pattern), then
  `runtime.join().await`. `Stop` closes the producer side, drains
  spawned futures, then exits.
- **Pause / resume**: `runtime.control.send(ControlCommand::Pause)` halts
  dispatch but keeps in-flight tasks running and result handling alive;
  `Resume` continues.

Recovery across process restarts is automatic: the FjallTaskQueue
persists `tasks` / `queue` / `inflight` / `dlq` partitions, and the
dispatcher calls `queue.recover_inflight()` on startup, moving any
tasks that were in-flight at the previous crash back to the queue.

### 8. HTTP client

Use `reqwest::Client` directly for most sites. If the target trips a WAF
(403/429/503 on `pginf fetch`), the user has two options — present them:

1. Switch to `pageinfo_rs::PageClient` (browser TLS fingerprinting via
   `wreq`, automatic fallback). Adds a dep but solves WAF blocks.
2. Configure a proxy in the `reqwest::Client` builder.

Set a real, identifiable `User-Agent`. Anonymous defaults get blocked
faster.

### 9. Inspection / dump mode

Building a quick `--dump` subcommand is a cheap way for the user to
verify the data Fjall after a run. Open the data partitions read-only
and iterate:

```rust
fn dump(data_path: &Path) -> Result<()> {
    let store = DataStore::open(data_path)?;
    println!("{}: ~{} records", data_path.display(), store.detail_count());
    for entry in store.details.iter().take(5).filter_map(|g| g.into_inner().ok()) {
        let (_, value) = entry;
        let record: MyDetail = serde_json::from_slice(&value)?;
        println!("- {}: {}", record.id, record.title);
    }
    Ok(())
}
```

See `examples/hackernews/main.rs` for a full implementation; users
invoke it with `cargo run --example hackernews --features http -- --dump`.

### 10. Hand-off to the user

Before declaring done, report:

1. Recon notes — entry URLs, page kinds, pagination, selectors,
   structured-data findings, rate-limit observations.
2. Task model — the `CrawlTask` enum variants and what each does.
3. Persistence layout — queue path, data path, one row per partition
   with key shape and value shape.
4. Run command — exactly how the user invokes it (`cargo run …`,
   `cargo run --example …`, etc.; follow whatever their project uses).
5. Verification — what they should see in the data Fjall after a short
   run (sample keys, sample value). If you shipped a `--dump` mode,
   tell them the exact command.

If any recon step was ambiguous or selectors looked fragile, say so
explicitly. Do not paper over uncertainty with "it should work."

## Reference (in this repo)

Pick by shape, not by name:

| example | shape | feature flags |
|---|---|---|
| `examples/hackernews/main.rs` | full crawler: Fjall queue + data store, listing→detail fan-out, `--dump`, status/parse guards | `http` |
| `examples/local_blog_crawl.rs` | in-memory queue, link-graph crawl against an in-process test server | `http` |
| `examples/httpbin_tower.rs` | minimal real-HTTP demo with rate_limit + concurrency_limit | `http` |
| `examples/mailbox.rs` | non-HTTP demo of producer/consumer pattern + Ctrl+C graceful stop | — |
| `examples/mailbox_metrics.rs` | OTLP metrics wiring | `observability` |
| `examples/mailbox_stats_http.rs` | live JSON stats endpoint on `:8080` | `stats-http` |

External:

- `pageinfo_rs` library — if direct lib usage is needed instead of the
  `pginf` CLI.
- `fjall` crate — partition API for the data store.

## Quick verification commands (for the user)

After scaffolding, the user should be able to:

```bash
cargo build                                   # compile
cargo test                                    # if tests exist
cargo run -- ...                              # the crawl
cargo run -- --dump                           # inspect the data Fjall
```

For capp-rs reference runs in this repo:

```bash
cargo run -p capp --example hackernews       --features http
cargo run -p capp --example hackernews       --features http -- --dump
cargo run -p capp --example local_blog_crawl --features http
cargo run -p capp --example httpbin_tower    --features http
```
