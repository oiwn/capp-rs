//! Crawl a generated local blog fixture with the mailbox runtime.
//! Requires `--features http`.

#![cfg(feature = "http")]

use std::{collections::HashSet, sync::Arc, time::Duration};

use anyhow::{Context as _, Result, ensure};
use capp::{
    manager::{
        MailboxConfig, ServiceRequest, ServiceStackOptions, build_service_stack,
        spawn_mailbox_runtime,
    },
    queue::{InMemoryTaskQueue, JsonSerializer, Task},
    tracing, tracing_subscriber,
};
use capp_testkit::{Scenario, TestServerHandle};
use reqwest::Client;
use scraper::{Html, Selector};
use tokio::sync::{Mutex, mpsc};
use tower::{BoxError, ServiceBuilder, service_fn};
use url::Url;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct CrawlTask {
    url: String,
}

#[derive(Debug)]
struct CrawlContext {
    client: Client,
    base_url: Url,
    seen_paths: Mutex<HashSet<String>>,
    link_selector: Selector,
}

impl CrawlContext {
    fn new(base_url: Url) -> anyhow::Result<Self> {
        Ok(Self {
            client: Client::builder()
                .user_agent("capp-local-blog-example/0.1")
                .build()
                .context("build reqwest client")?,
            base_url,
            seen_paths: Mutex::new(HashSet::new()),
            link_selector: Selector::parse("a")
                .map_err(|_| anyhow::anyhow!("parse anchor selector"))?,
        })
    }

    async fn mark_seen(&self, path: &str) -> bool {
        let mut seen = self.seen_paths.lock().await;
        seen.insert(path.to_string())
    }

    async fn seen_count(&self) -> usize {
        self.seen_paths.lock().await.len()
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let blog_posts = 100u32;
    let expected_pages = blog_posts as usize + 2;
    let server = TestServerHandle::spawn(Scenario::blog_with_posts(blog_posts))
        .await
        .context("spawn local blog fixture")?;
    let base_url = Url::parse(&server.base_url()).context("parse base url")?;
    let ctx = Arc::new(CrawlContext::new(base_url.clone())?);
    let queue = Arc::new(InMemoryTaskQueue::<CrawlTask, JsonSerializer>::new());
    let (done_tx, mut done_rx) = mpsc::channel::<String>(expected_pages * 2);

    ensure!(ctx.mark_seen("/").await, "seed path should be new");

    let inner = ServiceBuilder::new()
        .concurrency_limit(8)
        .service(service_fn(
            move |req: ServiceRequest<CrawlTask, CrawlContext>| {
                let done_tx = done_tx.clone();
                async move {
                    let response =
                        req.ctx.client.get(&req.task.payload.url).send().await?;
                    let status = response.status();
                    let body = response.text().await?;

                    if !status.is_success() {
                        return Err(anyhow::anyhow!(
                            "non-success response for {}: {}",
                            req.task.payload.url,
                            status
                        ))
                        .map_err(Into::into);
                    }

                    let discovered = {
                        let document = Html::parse_document(&body);
                        let mut discovered = Vec::new();
                        for node in document.select(&req.ctx.link_selector) {
                            let Some(href) = node.value().attr("href") else {
                                continue;
                            };
                            let Ok(url) = req.ctx.base_url.join(href) else {
                                continue;
                            };

                            if url.domain() != req.ctx.base_url.domain() {
                                continue;
                            }

                            discovered.push(url.to_string());
                        }
                        discovered
                    };

                    for discovered_url in discovered {
                        let url = Url::parse(&discovered_url)
                            .context("parse discovered url")?;
                        let path = url.path().to_string();
                        if req.ctx.mark_seen(&path).await {
                            req.producer
                                .enqueue(Task::new(CrawlTask {
                                    url: discovered_url,
                                }))
                                .await?;
                        }
                    }

                    let _ = done_tx.send(req.task.payload.url).await;
                    Ok::<(), BoxError>(())
                }
            },
        ));
    let service = build_service_stack(
        inner,
        ServiceStackOptions {
            timeout: Some(Duration::from_secs(5)),
        },
    );

    let runtime = spawn_mailbox_runtime(
        queue,
        ctx.clone(),
        service,
        MailboxConfig {
            producer_buffer: 512,
            result_buffer: 512,
            max_retries: 0,
            dequeue_backoff: Duration::from_millis(10),
            stop_when_idle: false,
        },
    );

    runtime
        .producer
        .enqueue(Task::new(CrawlTask {
            url: base_url.to_string(),
        }))
        .await
        .expect("producer channel open");

    let mut completed = 0usize;
    while completed < expected_pages {
        let url = tokio::time::timeout(Duration::from_secs(10), done_rx.recv())
            .await
            .context("timed out waiting for crawl completion")?
            .context("done channel closed unexpectedly")?;
        completed += 1;
        tracing::info!(completed, url, "page crawled");
    }

    let server_stats = server.stats().await;
    let local_seen = ctx.seen_count().await;

    tracing::info!(
        expected_pages,
        completed,
        local_seen,
        server_requests = server_stats.total_requests,
        unique_paths = server_stats.per_path.len(),
        "crawl finished"
    );

    ensure!(
        completed == expected_pages,
        "expected {expected_pages} completed pages, got {completed}"
    );
    ensure!(
        local_seen == expected_pages,
        "expected {expected_pages} discovered pages, got {local_seen}"
    );
    ensure!(
        server_stats.total_requests as usize == expected_pages,
        "expected {expected_pages} server requests, got {}",
        server_stats.total_requests
    );
    ensure!(
        server_stats.per_path.get("/archive") == Some(&1),
        "archive should be requested exactly once"
    );
    ensure!(
        server_stats.per_path.get("/post/100") == Some(&1),
        "last blog post should be requested exactly once"
    );

    runtime.shutdown().await;
    server
        .shutdown()
        .await
        .context("shutdown local blog fixture")?;
    Ok(())
}
