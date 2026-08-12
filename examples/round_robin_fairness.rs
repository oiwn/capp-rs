//! Demonstrates `FjallRoundRobinTaskQueue` fairness across several tagged
//! sources sharing one concurrency-limited worker pool -- the exact shape
//! of the crawler setup that surfaced the starvation bug fixed in
//! `capp-queue::backend::fjall_round_robin`: one queue, `N` sources, and
//! `concurrency_limit == N` (see `specs/ctx.md` for the full writeup).
//!
//! Each "source" simulates a page fetch (random delay drawn from the
//! source's own range) that always produces exactly one follow-up page
//! (same source), tracked as its own tag in the round-robin queue. Fanout
//! is deliberately fixed at 1, not random: a random fanout turns this into
//! a branching (Galton-Watson) process, whose population size has huge
//! variance by pure chance, completely swamping any signal about queue
//! fairness. With fanout fixed, every source stays in a steady state
//! forever and the *only* thing that differs between sources is fetch
//! latency -- so any imbalance in the periodic fairness snapshot reflects
//! scheduling behavior, not population-size luck.
//!
//! Run with `cargo run --example round_robin_fairness` (add
//! `RUST_LOG=debug` to see every individual page processed, not just the
//! periodic fairness snapshots).

use std::{
    collections::HashMap,
    ops::RangeInclusive,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use capp::{
    manager::{
        MailboxConfig, ServiceRequest, ServiceStackOptions, build_service_stack,
        spawn_mailbox_runtime,
    },
    queue::{
        AbstractTaskQueue, FjallRoundRobinTaskQueue, HasTagKey, JsonSerializer,
        Task,
    },
    tracing, tracing_subscriber,
};
use rand::{RngExt, rng};
use tower::{BoxError, ServiceBuilder, service_fn};

const SEEDS_PER_SOURCE: u32 = 3;
const TARGET_TOTAL: u64 = 600;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct CrawlPage {
    source: String,
    page: u64,
}

impl HasTagKey for CrawlPage {
    type TagValue = String;
    fn get_tag_value(&self) -> Self::TagValue {
        self.source.clone()
    }
}

#[derive(Clone)]
struct SourceProfile {
    name: &'static str,
    delay_ms: RangeInclusive<u64>,
}

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    tracing_subscriber::fmt::init();

    // Same fanout (1) for every source -- steady state, no population
    // growth or extinction. The only thing that differs is fetch latency:
    // some sources are fast (like the report's `cryptoslate`), some slow
    // (like `coindesk`) -- the condition under which the old evict +
    // tail-requeue design let refill order silently become serve order.
    let profiles = vec![
        SourceProfile {
            name: "apple",
            delay_ms: 60..=110,
        },
        SourceProfile {
            name: "banana",
            delay_ms: 50..=100,
        },
        SourceProfile {
            name: "cherry",
            delay_ms: 40..=90,
        },
        SourceProfile {
            name: "mango",
            delay_ms: 90..=150,
        },
        SourceProfile {
            name: "kiwi",
            delay_ms: 180..=320,
        },
        SourceProfile {
            name: "papaya",
            delay_ms: 20..=60,
        },
    ];
    let worker_count = profiles.len();
    println!(
        "round-robin fairness demo: {} sources, concurrency_limit = {worker_count} \
         (== source count, matching the reported bug's setup)",
        profiles.len()
    );

    let dir = std::env::temp_dir()
        .join(format!("capp-round-robin-fairness-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let fjall_queue =
        FjallRoundRobinTaskQueue::<CrawlPage, JsonSerializer>::open(&dir)?;
    let queue: AbstractTaskQueue<CrawlPage> = Arc::new(fjall_queue);

    for profile in &profiles {
        for seed in 0..SEEDS_PER_SOURCE {
            queue
                .push(&Task::new(CrawlPage {
                    source: profile.name.to_string(),
                    page: seed as u64,
                }))
                .await?;
        }
    }

    let profiles_by_name: Arc<HashMap<String, SourceProfile>> = Arc::new(
        profiles
            .iter()
            .map(|p| (p.name.to_string(), p.clone()))
            .collect(),
    );
    let counts: Arc<Mutex<HashMap<String, u64>>> = Arc::new(Mutex::new(
        profiles.iter().map(|p| (p.name.to_string(), 0)).collect(),
    ));

    let ctx = Arc::new(());
    let base_service = {
        let profiles_by_name = profiles_by_name.clone();
        let counts = counts.clone();
        service_fn(move |req: ServiceRequest<CrawlPage, ()>| {
            let profiles_by_name = profiles_by_name.clone();
            let counts = counts.clone();
            async move {
                let page = req.task.payload.clone();
                let profile = profiles_by_name
                    .get(&page.source)
                    .expect("every task's source has a known profile");
                // `ThreadRng` is `!Send`, so it must not be held across an
                // `.await` -- scope it tightly instead of keeping one
                // instance alive across the sleep below.
                let delay_ms = rng().random_range(profile.delay_ms.clone());
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;

                {
                    let mut c = counts.lock().unwrap();
                    *c.entry(page.source.clone()).or_insert(0) += 1;
                }
                tracing::debug!(
                    source = %page.source,
                    page = page.page,
                    delay_ms,
                    "processed page"
                );

                // Fixed fanout of 1: this source's backlog neither grows
                // nor shrinks over time, keeping population size out of
                // the picture entirely.
                let child = Task::new(CrawlPage {
                    source: page.source.clone(),
                    page: page.page + 1,
                });
                let _ = req.producer.enqueue(child).await;
                Ok::<(), BoxError>(())
            }
        })
    };

    let inner = ServiceBuilder::new()
        .concurrency_limit(worker_count)
        .service(base_service);
    let service = build_service_stack(
        inner,
        ServiceStackOptions {
            timeout: Some(Duration::from_secs(10)),
        },
    );

    let runtime = spawn_mailbox_runtime(
        queue,
        ctx,
        service,
        MailboxConfig {
            producer_buffer: 1024,
            result_buffer: 1024,
            max_retries: 1,
            dequeue_backoff: Duration::from_millis(10),
            // Sources never run dry on their own (fanout is always 1), so
            // there's no natural idle point -- the logger below stops the
            // run once TARGET_TOTAL pops have been observed.
            stop_when_idle: false,
        },
    );

    // Periodic fairness snapshot: each source's share of pops so far.
    // Also owns the run's termination condition.
    let logger_running = Arc::new(AtomicBool::new(true));
    let logger = {
        let counts = counts.clone();
        let logger_running = logger_running.clone();
        let control = runtime.control.clone();
        let mut names: Vec<String> =
            profiles.iter().map(|p| p.name.to_string()).collect();
        names.sort();
        tokio::spawn(async move {
            while logger_running.load(Ordering::Relaxed) {
                tokio::time::sleep(Duration::from_millis(800)).await;
                let snapshot = counts.lock().unwrap().clone();
                let total: u64 = snapshot.values().sum();
                if total == 0 {
                    continue;
                }
                let breakdown: String = names
                    .iter()
                    .map(|name| {
                        let n = snapshot.get(name).copied().unwrap_or(0);
                        let pct = 100.0 * n as f64 / total as f64;
                        format!("{name}={n}({pct:.1}%)")
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                tracing::info!(total, "snapshot: {breakdown}");
                if total >= TARGET_TOTAL {
                    let _ = control.send(capp::manager::ControlCommand::Stop);
                    break;
                }
            }
        })
    };

    let _ = logger.await;
    logger_running.store(false, Ordering::Relaxed);
    runtime.shutdown().await;

    let final_counts = counts.lock().unwrap().clone();
    let total: u64 = final_counts.values().sum();
    println!("\nfinal per-source share (total pops = {total}):");
    let mut names: Vec<String> =
        profiles.iter().map(|p| p.name.to_string()).collect();
    names.sort();
    let mut min_share = f64::MAX;
    let mut max_share = f64::MIN;
    for name in &names {
        let n = final_counts.get(name).copied().unwrap_or(0);
        let pct = 100.0 * n as f64 / total as f64;
        min_share = min_share.min(pct);
        max_share = max_share.max(pct);
        println!("  {name:<15} {n:>5} pops ({pct:5.1}%)");
    }
    println!(
        "min/max share ratio: {:.2} (1.0 = perfectly even across sources)",
        min_share / max_share
    );

    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}
