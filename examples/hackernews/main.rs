//! Hacker News crawler — smoke test for the `build_crawler` skill.
//!
//! Fetches `/news?p=N` pages, then each `/item?id=ID`, and persists
//! stories + comments into a Fjall data store side-by-side with the
//! durable `FjallTaskQueue` driving the workers.
//!
//! Run: `cargo run -p capp --example hackernews --features http`
#![cfg(feature = "http")]
mod parse;
mod store;
mod task;

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context as _, Result};
use capp::{
    manager::{
        MailboxConfig, ServiceRequest, ServiceStackOptions, build_service_stack,
        spawn_mailbox_runtime,
    },
    queue::{JsonSerializer, Task},
    tracing, tracing_subscriber,
};
use capp_queue::FjallTaskQueue;
use reqwest::Client;
use tower::{BoxError, ServiceBuilder, service_fn};

use crate::parse::{Comment, StoryMeta, parse_item, parse_listing};
use crate::store::DataStore;
use crate::task::HnTask;

const BASE: &str = "https://news.ycombinator.com";
const MAX_PAGES: u32 = 2;

struct CrawlContext {
    client: Client,
    data: DataStore,
}

const DATA_DIR: &str = ".hackernews";

#[tokio::main]
async fn main() -> Result<()> {
    std::fs::create_dir_all(DATA_DIR).context("create data dir")?;
    let queue_path: PathBuf = format!("{DATA_DIR}/queue.fjall").into();
    let data_path: PathBuf = format!("{DATA_DIR}/data.fjall").into();

    if std::env::args().any(|a| a == "--dump") {
        return dump(&data_path);
    }

    tracing_subscriber::fmt::init();

    let queue = Arc::new(
        FjallTaskQueue::<HnTask, JsonSerializer>::open(&queue_path)
            .context("open queue")?,
    );

    let ctx = Arc::new(CrawlContext {
        client: Client::builder()
            .user_agent(
                "capp-rs/hackernews-example (https://github.com/oiwn/capp-rs)",
            )
            .timeout(Duration::from_secs(15))
            .build()
            .context("build reqwest client")?,
        data: DataStore::open(data_path.as_path())?,
    });

    let inner = ServiceBuilder::new()
        .concurrency_limit(2)
        .rate_limit(2, Duration::from_secs(1))
        .service(service_fn(
            move |req: ServiceRequest<HnTask, CrawlContext>| async move {
                match req.task.payload.clone() {
                    HnTask::Listing { page } => handle_listing(&req, page).await,
                    HnTask::Item { id } => handle_item(&req, id).await,
                }
                .map_err(|e: anyhow::Error| -> BoxError { e.into() })
            },
        ));
    let service = build_service_stack(
        inner,
        ServiceStackOptions {
            timeout: Some(Duration::from_secs(30)),
        },
    );

    let runtime = spawn_mailbox_runtime(
        queue,
        ctx.clone(),
        service,
        MailboxConfig {
            producer_buffer: 256,
            result_buffer: 256,
            max_retries: 1,
            dequeue_backoff: Duration::from_millis(100),
            stop_when_idle: true,
        },
    );

    runtime
        .producer
        .enqueue(Task::new(HnTask::Listing { page: 1 }))
        .await
        .context("seed enqueue")?;

    runtime.join().await;

    let stories = ctx.data.story_count();
    let comments = ctx.data.comment_count();
    tracing::info!(stories, comments, "crawl complete");
    Ok(())
}

const DUMP_SAMPLES: usize = 5;

fn dump(data_path: &Path) -> Result<()> {
    let store = DataStore::open(data_path)?;
    println!(
        "{}: ~{} stories, ~{} comments\n",
        data_path.display(),
        store.story_count(),
        store.comment_count(),
    );

    let stories: Vec<StoryMeta> = store
        .stories
        .iter()
        .take(DUMP_SAMPLES)
        .filter_map(|g| g.into_inner().ok())
        .filter_map(|(_, v)| serde_json::from_slice(&v).ok())
        .collect();

    for s in stories {
        println!(
            "[{}] {}p · {}c · {}",
            s.id,
            s.score,
            s.comments_count,
            s.by.as_deref().unwrap_or("?"),
        );
        println!("  {}", truncate(&s.title, 70));
        let link = s.url.clone().unwrap_or_else(|| {
            format!("https://news.ycombinator.com/item?id={}", s.id)
        });
        println!("  → {link}");

        let sample: Vec<Comment> = store
            .comments
            .prefix(s.id.to_be_bytes())
            .take(2)
            .filter_map(|g| g.into_inner().ok())
            .filter_map(|(_, v)| serde_json::from_slice(&v).ok())
            .collect();
        for c in sample {
            println!(
                "  ↳ {}: {}",
                c.by.as_deref().unwrap_or("?"),
                truncate(&html_to_text(&c.text_html), 70),
            );
        }
        println!();
    }
    Ok(())
}

fn truncate(s: &str, n: usize) -> String {
    let s = s.replace('\n', " ");
    if s.chars().count() <= n {
        s
    } else {
        let head: String = s.chars().take(n.saturating_sub(3)).collect();
        format!("{head}...")
    }
}

/// Render stored comment HTML into readable plain text for the dump:
/// strips tags and decodes entities (`&gt;` → `>` etc.).
fn html_to_text(s: &str) -> String {
    scraper::Html::parse_fragment(s)
        .root_element()
        .text()
        .collect()
}

async fn handle_listing(
    req: &ServiceRequest<HnTask, CrawlContext>,
    page: u32,
) -> Result<()> {
    let url = format!("{BASE}/news?p={page}");
    let response = req.ctx.client.get(&url).send().await?;
    let status = response.status();
    let body = response.text().await?;

    if !status.is_success() {
        anyhow::bail!("listing p={page}: HTTP {status} ({} bytes)", body.len());
    }

    let listing = parse_listing(&body);
    if listing.stories.is_empty() {
        anyhow::bail!(
            "listing p={page}: 0 stories parsed from {} bytes (likely HN \
             markup change or anti-bot response)",
            body.len()
        );
    }

    let mut new_items = 0u32;
    for id in listing.stories.iter().copied() {
        if req.ctx.data.has_story(id)? {
            continue;
        }
        req.producer.enqueue(Task::new(HnTask::Item { id })).await?;
        new_items += 1;
    }
    if let Some(next) = listing.next_page
        && next <= MAX_PAGES
    {
        req.producer
            .enqueue(Task::new(HnTask::Listing { page: next }))
            .await?;
    }

    tracing::info!(
        page,
        stories = listing.stories.len(),
        new_items,
        next_page = ?listing.next_page,
        "listing done"
    );
    Ok(())
}

async fn handle_item(
    req: &ServiceRequest<HnTask, CrawlContext>,
    id: u64,
) -> Result<()> {
    let url = format!("{BASE}/item?id={id}");
    let response = req.ctx.client.get(&url).send().await?;
    let status = response.status();
    let body = response.text().await?;

    if !status.is_success() {
        anyhow::bail!("item {id}: HTTP {status} ({} bytes)", body.len());
    }

    let Some(item) = parse_item(id, &body) else {
        tracing::warn!(
            id,
            %status,
            body_len = body.len(),
            "could not parse item page (selectors missed)"
        );
        return Ok(());
    };

    req.ctx.data.put_story(&item.story)?;
    for c in &item.comments {
        req.ctx.data.put_comment(c)?;
    }
    tracing::info!(id, comments = item.comments.len(), "item stored");
    Ok(())
}
