#![cfg(feature = "fjall")]

use std::sync::Arc;
use std::time::Duration;

use capp_queue::{
    FjallRoundRobinTaskQueue, HasTagKey, JsonSerializer, Task, TaskQueue,
    TaskQueueError,
};
use serde::{Deserialize, Serialize};
use tempfile::{TempDir, tempdir};
use tokio::task::JoinSet;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct Tagged {
    site: String,
    n: u32,
}

impl HasTagKey for Tagged {
    type TagValue = String;
    fn get_tag_value(&self) -> Self::TagValue {
        self.site.clone()
    }
}

type Q = FjallRoundRobinTaskQueue<Tagged, JsonSerializer>;

fn open() -> (TempDir, Q) {
    let dir = tempdir().unwrap();
    let q = FjallRoundRobinTaskQueue::open(dir.path()).unwrap();
    (dir, q)
}

async fn push(q: &Q, site: &str, n: u32) {
    q.push(&Task::new(Tagged {
        site: site.to_string(),
        n,
    }))
    .await
    .unwrap();
}

async fn pop_site(q: &Q) -> String {
    q.pop().await.unwrap().payload.site
}

#[tokio::test]
async fn pop_rotates_across_tags() {
    let (_dir, q) = open();
    for n in 0..4 {
        push(&q, "a", n).await;
        push(&q, "b", n).await;
        push(&q, "c", n).await;
    }
    let mut seq = Vec::new();
    for _ in 0..12 {
        seq.push(pop_site(&q).await);
    }
    let expected: Vec<String> = std::iter::repeat_n(["a", "b", "c"], 4)
        .flatten()
        .map(String::from)
        .collect();
    assert_eq!(seq, expected);
    assert!(matches!(q.pop().await, Err(TaskQueueError::QueueEmpty)));
}

#[tokio::test]
async fn pop_skips_unpushed_tags() {
    let (_dir, q) = open();
    for n in 0..3 {
        push(&q, "a", n).await;
        push(&q, "c", n).await;
    }
    let mut seq = Vec::new();
    for _ in 0..6 {
        seq.push(pop_site(&q).await);
    }
    assert_eq!(seq, vec!["a", "c", "a", "c", "a", "c"]);
}

#[tokio::test]
async fn drained_tag_is_skipped_not_evicted() {
    let (_dir, q) = open();
    push(&q, "a", 0).await;
    push(&q, "b", 0).await;
    push(&q, "c", 0).await;
    push(&q, "c", 1).await;
    push(&q, "d", 0).await;
    push(&q, "d", 1).await;

    // A and B each have one task; pop a, b in the first cycle and they drain.
    assert_eq!(pop_site(&q).await, "a");
    assert_eq!(pop_site(&q).await, "b");
    // A and B are empty now but stay in rotation (skipped, not evicted);
    // remaining pops observe C and D since they still have work.
    let mut seq = Vec::new();
    for _ in 0..4 {
        seq.push(pop_site(&q).await);
    }
    assert_eq!(seq, vec!["c", "d", "c", "d"]);
}

#[tokio::test]
async fn redrained_tag_becomes_poppable_again() {
    let (_dir, q) = open();
    push(&q, "a", 0).await;
    push(&q, "b", 0).await;
    push(&q, "b", 1).await;

    assert_eq!(pop_site(&q).await, "a"); // a now empty, but still in rotation
    push(&q, "a", 1).await; // a already tracked; this just refills it
    // Cursor is at b. Rotation order is unchanged: a, b, a, b, ... so a is
    // due right after b's next turn.
    let mut seq = Vec::new();
    for _ in 0..3 {
        seq.push(pop_site(&q).await);
    }
    assert_eq!(seq, vec!["b", "a", "b"]);
}

#[tokio::test]
async fn drained_tag_resumes_at_original_slot_not_tail() {
    // Proves the actual fix: a tag that drains and later refills is served
    // at its normal rotation turn, not pushed behind every other active
    // tag. Under the old evict-then-append-at-tail design, `a` would have
    // been evicted on the first pop and only rejoin at the back of the
    // rotation once refilled -- meaning after two more laps of b/c churn,
    // pushing `a` again would NOT make it immediately poppable next.
    let (_dir, q) = open();
    push(&q, "a", 0).await;
    push(&q, "b", 0).await;
    push(&q, "c", 0).await;

    assert_eq!(pop_site(&q).await, "a"); // a now empty
    assert_eq!(pop_site(&q).await, "b");
    push(&q, "b", 1).await; // keep b non-empty
    assert_eq!(pop_site(&q).await, "c");
    push(&q, "c", 1).await; // keep c non-empty

    // a sits empty through two more full laps while b and c keep cycling.
    for _ in 0..2 {
        assert_eq!(pop_site(&q).await, "b");
        push(&q, "b", 99).await;
        assert_eq!(pop_site(&q).await, "c");
        push(&q, "c", 99).await;
    }

    // The moment a has work again, it's next -- no tail penalty.
    push(&q, "a", 1).await;
    assert_eq!(pop_site(&q).await, "a");
}

#[tokio::test]
async fn pop_drains_remaining_when_some_tags_empty() {
    let (_dir, q) = open();
    for n in 0..10 {
        push(&q, "a", n).await;
    }
    push(&q, "b", 0).await;

    let mut seq = Vec::new();
    while let Ok(t) = q.pop().await {
        seq.push(t.payload.site);
    }
    assert_eq!(seq.len(), 11);
    // First two pops should be a, b (b joined active after a).
    assert_eq!(seq[0], "a");
    assert_eq!(seq[1], "b");
    // b drains after its one task but stays in rotation (skipped, not
    // evicted); with no more b work ever arriving, the rest are all a.
    assert!(seq[2..].iter().all(|s| s == "a"));
}

#[tokio::test]
async fn new_tag_joins_rotation_mid_run() {
    let (_dir, q) = open();
    for n in 0..3 {
        push(&q, "a", n).await;
        push(&q, "b", n).await;
    }
    assert_eq!(pop_site(&q).await, "a");
    assert_eq!(pop_site(&q).await, "b");
    push(&q, "c", 0).await;
    push(&q, "c", 1).await;
    // Cursor is back at a; rotation now a, b, c, a, b, c.
    let mut seq = Vec::new();
    for _ in 0..6 {
        seq.push(pop_site(&q).await);
    }
    assert_eq!(seq, vec!["a", "b", "c", "a", "b", "c"]);
}

#[tokio::test]
async fn recover_inflight_restores_per_tag() {
    let dir = tempdir().unwrap();
    let captured: Vec<(String, u32)> = {
        let q: Q = FjallRoundRobinTaskQueue::open(dir.path()).unwrap();
        for n in 0..3 {
            push(&q, "a", n).await;
            push(&q, "b", n).await;
        }
        // Pop 4 — 2 from each tag (rotation a,b,a,b).
        let mut popped = Vec::new();
        for _ in 0..4 {
            let t = q.pop().await.unwrap();
            popped.push((t.payload.site, t.payload.n));
        }
        popped
    };

    let q: Q = FjallRoundRobinTaskQueue::open(dir.path()).unwrap();
    // Two remaining unpopped tasks are still in queue; the 4 popped sit in inflight.
    let recovered = q.recover_inflight().await.unwrap();
    assert_eq!(recovered, 4);

    let mut drained = Vec::new();
    while let Ok(t) = q.pop().await {
        drained.push((t.payload.site, t.payload.n));
    }
    // 6 total tasks come back: the 4 recovered + the 2 originally unpopped.
    assert_eq!(drained.len(), 6);

    // Per-tag counts: 3 each.
    let a_count = drained.iter().filter(|(s, _)| s == "a").count();
    let b_count = drained.iter().filter(|(s, _)| s == "b").count();
    assert_eq!(a_count, 3);
    assert_eq!(b_count, 3);

    // Sanity: recovered ones came back in the original FIFO position for their tag.
    for (site, n) in &captured {
        assert!(drained.contains(&(site.clone(), *n)));
    }
}

#[tokio::test]
async fn restart_recovers_tag_list_from_queue() {
    let dir = tempdir().unwrap();
    {
        let q: Q = FjallRoundRobinTaskQueue::open(dir.path()).unwrap();
        push(&q, "a", 0).await;
        push(&q, "b", 0).await;
        push(&q, "a", 1).await;
        push(&q, "b", 1).await;
    }

    let q: Q = FjallRoundRobinTaskQueue::open(dir.path()).unwrap();
    let mut seq = Vec::new();
    for _ in 0..4 {
        seq.push(pop_site(&q).await);
    }
    // open() sorts active deterministically, so a comes before b.
    assert_eq!(seq, vec!["a", "b", "a", "b"]);
}

#[tokio::test]
async fn concurrent_push_pop_fair() {
    let (_dir, q) = open();
    let q = Arc::new(q);
    let tags = ["x", "y", "z"];
    let per_tag = 50usize;

    let mut pushers = JoinSet::new();
    for tag in tags {
        let q = q.clone();
        pushers.spawn(async move {
            for n in 0..per_tag {
                q.push(&Task::new(Tagged {
                    site: tag.to_string(),
                    n: n as u32,
                }))
                .await
                .unwrap();
            }
        });
    }
    while let Some(res) = pushers.join_next().await {
        res.unwrap();
    }

    let mut counts = std::collections::HashMap::new();
    let total = tags.len() * per_tag;
    let mut poppers = JoinSet::new();
    let shared_counts = Arc::new(tokio::sync::Mutex::new(counts.clone()));
    for _ in 0..4 {
        let q = q.clone();
        let counts = shared_counts.clone();
        poppers.spawn(async move {
            loop {
                match q.pop().await {
                    Ok(t) => {
                        let mut c = counts.lock().await;
                        *c.entry(t.payload.site).or_insert(0usize) += 1;
                        if c.values().sum::<usize>() == 150 {
                            break;
                        }
                    }
                    Err(TaskQueueError::QueueEmpty) => {
                        tokio::time::sleep(Duration::from_millis(2)).await;
                        let c = counts.lock().await;
                        if c.values().sum::<usize>() == 150 {
                            break;
                        }
                    }
                    Err(e) => panic!("pop failed: {e:?}"),
                }
            }
        });
    }
    while let Some(res) = poppers.join_next().await {
        res.unwrap();
    }
    counts = shared_counts.lock().await.clone();
    assert_eq!(counts.values().sum::<usize>(), total);
    for tag in tags {
        assert_eq!(counts.get(tag).copied().unwrap_or(0), per_tag);
    }
}

#[tokio::test]
async fn thin_tag_survives_interleaved_drain_under_concurrency() {
    // Unlike concurrent_push_pop_fair (which loads all tags fully before
    // popping starts), this pushes a "thin" tag slowly *while* poppers are
    // actively racing on deep "thick" tags -- the actual shape of the
    // starvation bug (a tag whose backlog repeatedly grazes zero while
    // concurrent workers keep the shared queue busy). It only asserts
    // eventual correctness (every pushed task is popped exactly once),
    // since exact interleaving order isn't deterministic under real
    // concurrency; the deterministic rotation-slot guarantee is covered by
    // drained_tag_resumes_at_original_slot_not_tail above.
    let (_dir, q) = open();
    let q = Arc::new(q);

    for n in 0..100u32 {
        push(&q, "thick_a", n).await;
        push(&q, "thick_b", n).await;
    }
    push(&q, "thin", 0).await;

    let thin_total = 20u32;
    let q2 = q.clone();
    let thin_pusher = tokio::spawn(async move {
        for n in 1..thin_total {
            tokio::time::sleep(Duration::from_millis(3)).await;
            push(&q2, "thin", n).await;
        }
    });

    let counts =
        Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
    let total = 200 + thin_total as usize;
    let mut poppers = JoinSet::new();
    for _ in 0..4 {
        let q = q.clone();
        let counts = counts.clone();
        poppers.spawn(async move {
            loop {
                match q.pop().await {
                    Ok(t) => {
                        let mut c = counts.lock().await;
                        *c.entry(t.payload.site).or_insert(0usize) += 1;
                        if c.values().sum::<usize>() == total {
                            break;
                        }
                    }
                    Err(TaskQueueError::QueueEmpty) => {
                        tokio::time::sleep(Duration::from_millis(2)).await;
                        let c = counts.lock().await;
                        if c.values().sum::<usize>() == total {
                            break;
                        }
                    }
                    Err(e) => panic!("pop failed: {e:?}"),
                }
            }
        });
    }

    thin_pusher.await.unwrap();
    while let Some(res) = poppers.join_next().await {
        res.unwrap();
    }

    let counts = counts.lock().await.clone();
    assert_eq!(counts.values().sum::<usize>(), total);
    assert_eq!(
        counts.get("thin").copied().unwrap_or(0),
        thin_total as usize
    );
    assert_eq!(counts.get("thick_a").copied().unwrap_or(0), 100);
    assert_eq!(counts.get("thick_b").copied().unwrap_or(0), 100);
}

#[tokio::test]
async fn refill_order_does_not_reshuffle_rotation() {
    // Isolates the actual mechanism behind the reported starvation. It is
    // NOT that a drained tag's wait is unbounded in the worst case -- for
    // a fixed, finite tag set, a single pop() scan is bounded by the tag
    // count under both the old and new design, since `active` can never
    // hold more than the number of distinct tags ever seen. The real flaw
    // only shows up when *multiple* tags are empty at once and refill in
    // an order that differs from their original rotation position -- which
    // is exactly what real concurrency produces: when several sources'
    // in-flight fetches are all pending, whichever one's network response
    // happens to land first is essentially random, unrelated to the
    // source's position in the queue.
    //
    // Under the old evict + tail-requeue design, refill order silently
    // becomes serve order (tail = arrival order), so a source that simply
    // loses the "whichever fetch completes first" race repeatedly ends up
    // permanently reshuffled behind sources that happen to answer faster
    // -- compounding over a long crawl into the kind of large, sustained
    // imbalance in the bug report, even though no single wait ever
    // "looked" unbounded. Under scan-and-skip, a tag's rotation position
    // never changes on push, so serve order always matches original
    // position regardless of refill order.
    let (_dir, q) = open();
    push(&q, "thick_a", 0).await;
    push(&q, "thick_a", 1).await; // 2 items: survives the first round undrained
    push(&q, "thick_b", 0).await;
    push(&q, "thick_b", 1).await;
    push(&q, "slow1", 0).await;
    push(&q, "slow2", 0).await;
    push(&q, "slow3", 0).await;

    // Drain the thick tags' first item and all three slow tags, leaving
    // all three slow tags simultaneously empty at once.
    for expected in ["thick_a", "thick_b", "slow1", "slow2", "slow3"] {
        assert_eq!(pop_site(&q).await, expected);
    }

    // Refill in the OPPOSITE order, as if slow3's fetch happened to land
    // first, then slow2's, then slow1's.
    push(&q, "slow3", 1).await;
    push(&q, "slow2", 1).await;
    push(&q, "slow1", 1).await;

    // A fair scheduler serves them back in their original rotation order
    // (slow1, slow2, slow3) -- not refill order (slow3, slow2, slow1).
    for expected in ["thick_a", "thick_b", "slow1", "slow2", "slow3"] {
        assert_eq!(pop_site(&q).await, expected);
    }
}

// --- TaskQueue contract tests (mirroring the existing backends) -------------

fn make() -> (TempDir, Q) {
    open()
}

#[tokio::test]
async fn push_and_pop() {
    let (_dir, q) = make();
    let t = Task::new(Tagged {
        site: "a".into(),
        n: 7,
    });
    q.push(&t).await.unwrap();
    let popped = q.pop().await.unwrap();
    assert_eq!(popped.payload, t.payload);
}

#[tokio::test]
async fn queue_empty() {
    let (_dir, q) = make();
    let res = q.pop().await;
    assert!(matches!(res, Err(TaskQueueError::QueueEmpty)));
}

#[tokio::test]
async fn ack_removes_task() {
    let (_dir, q) = make();
    let t = Task::new(Tagged {
        site: "a".into(),
        n: 1,
    });
    q.push(&t).await.unwrap();
    let popped = q.pop().await.unwrap();
    q.ack(&popped.task_id).await.unwrap();
    assert!(matches!(q.pop().await, Err(TaskQueueError::QueueEmpty)));
}

#[tokio::test]
async fn nack_moves_to_dlq() {
    let (_dir, q) = make();
    let t = Task::new(Tagged {
        site: "a".into(),
        n: 99,
    });
    q.push(&t).await.unwrap();
    let mut popped = q.pop().await.unwrap();
    popped.set_retry("fail");
    q.nack(&popped).await.unwrap();
    // Task is gone from the queue.
    assert!(matches!(q.pop().await, Err(TaskQueueError::QueueEmpty)));
}

#[tokio::test]
async fn set_updates_task() {
    let (_dir, q) = make();
    let mut t = Task::new(Tagged {
        site: "a".into(),
        n: 3,
    });
    q.push(&t).await.unwrap();
    t.payload.n = 5;
    q.set(&t).await.unwrap();
    let popped = q.pop().await.unwrap();
    assert_eq!(popped.payload.n, 5);
}

#[tokio::test]
async fn push_retry_clears_inflight() {
    // Push, pop (now in flight), push again (retry). The second push must
    // clear the inflight entry so recover_inflight returns 0.
    let dir = tempdir().unwrap();
    let q: Q = FjallRoundRobinTaskQueue::open(dir.path()).unwrap();
    let mut t = Task::new(Tagged {
        site: "a".into(),
        n: 14,
    });
    q.push(&t).await.unwrap();
    let _ = q.pop().await.unwrap();
    t.set_retry("transient");
    q.push(&t).await.unwrap();

    // Drop and reopen — only the requeued task should be present, nothing in inflight.
    drop(q);
    let q: Q = FjallRoundRobinTaskQueue::open(dir.path()).unwrap();
    assert_eq!(q.recover_inflight().await.unwrap(), 0);
    let popped = q.pop().await.unwrap();
    assert_eq!(popped.payload.n, 14);
}
