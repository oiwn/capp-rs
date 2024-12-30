mod common;

use capp_queue::{JsonSerializer, RedisTaskQueue, TaskQueue};
use criterion::{criterion_group, criterion_main, Criterion};
use dotenvy::dotenv;
use rustis::client::Client;
use rustis::commands::GenericCommands;
use tokio::runtime::Runtime;

use capp_queue::task::Task;
use common::data::{generate_test_data, BenchTaskData};

const TASK_COUNT: usize = 1000;

async fn get_redis_connection() -> Client {
    dotenv().ok();
    let uri = std::env::var("REDIS_URI").expect("Set REDIS_URI env variable");
    Client::connect(uri)
        .await
        .expect("Error establishing redis connection")
}

async fn setup_queue() -> RedisTaskQueue<BenchTaskData, JsonSerializer> {
    let redis = get_redis_connection().await;
    RedisTaskQueue::new(redis, "bench-queue")
        .await
        .expect("Failed to create RedisTaskQueue")
}

async fn cleanup_queue(queue: &RedisTaskQueue<BenchTaskData, JsonSerializer>) {
    queue
        .client
        .del([&queue.list_key, &queue.hashmap_key, &queue.dlq_key])
        .await
        .expect("Failed to clean up Redis keys");
}

fn bench_push(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let data = generate_test_data(TASK_COUNT);

    let mut group = c.benchmark_group("redis_push");
    group.sample_size(10);

    group.bench_function("bulk_push_1000", |b| {
        b.iter(|| {
            rt.block_on(async {
                let queue = setup_queue().await;

                // Push all tasks
                for item in &data {
                    queue.push(&Task::new(item.clone())).await.unwrap();
                }

                // Cleanup after benchmark
                cleanup_queue(&queue).await;
            });
        })
    });

    group.finish();
}

fn bench_pop(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let data = generate_test_data(TASK_COUNT);

    let mut group = c.benchmark_group("redis_pop");
    group.sample_size(10);

    group.bench_function("bulk_pop_1000", |b| {
        b.iter(|| {
            rt.block_on(async {
                let queue = setup_queue().await;

                // Setup - push all tasks first
                for item in &data {
                    queue.push(&Task::new(item.clone())).await.unwrap();
                }

                // Benchmark popping
                for _ in 0..TASK_COUNT {
                    queue.pop().await.unwrap();
                }

                // Cleanup
                cleanup_queue(&queue).await;
            });
        })
    });

    group.finish();
}

fn bench_mixed_ops(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let data = generate_test_data(TASK_COUNT);

    let mut group = c.benchmark_group("redis_mixed");
    group.sample_size(10);

    group.bench_function("push_pop_ack_cycle", |b| {
        b.iter(|| {
            rt.block_on(async {
                let queue = setup_queue().await;

                // Push-Pop-Ack cycles in smaller batches
                for chunk in data.chunks(100) {
                    // Push batch
                    for item in chunk {
                        queue.push(&Task::new(item.clone())).await.unwrap();
                    }

                    // Pop and ack batch
                    for _ in 0..chunk.len() {
                        let task = queue.pop().await.unwrap();
                        queue.ack(&task.task_id).await.unwrap();
                    }
                }

                // Cleanup
                cleanup_queue(&queue).await;
            });
        })
    });

    group.finish();
}

criterion_group!(benches, bench_push, bench_pop, bench_mixed_ops);
criterion_main!(benches);
