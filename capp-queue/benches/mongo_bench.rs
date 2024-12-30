mod common;

use capp_queue::{backend::mongodb::BsonSerializer, MongoTaskQueue, TaskQueue};
use criterion::{criterion_group, criterion_main, Criterion};
use dotenvy::dotenv;
// use mongodb::{options::ClientOptions, Client};
use tokio::runtime::Runtime;

use capp_queue::task::Task;
use common::data::{generate_test_data, BenchTaskData};

const TASK_COUNT: usize = 1000;
const QUEUE_NAME: &str = "bench_queue";

async fn get_mongo_connection() -> String {
    dotenv().ok();
    std::env::var("MONGODB_URI").expect("Set MONGODB_URI env variable")
}

async fn setup_queue() -> MongoTaskQueue<BenchTaskData, BsonSerializer> {
    let uri = get_mongo_connection().await;
    let queue = MongoTaskQueue::new(&uri, QUEUE_NAME)
        .await
        .expect("Failed to create MongoTaskQueue");

    // Clean any existing data
    cleanup_collections(&queue).await;

    queue
}

async fn cleanup_collections(
    queue: &MongoTaskQueue<BenchTaskData, BsonSerializer>,
) {
    let tasks_coll = &queue.tasks_collection;
    let dlq_coll = &queue.dlq_collection;

    // Drop collections if they exist
    let _ = tasks_coll.drop().await;
    let _ = dlq_coll.drop().await;
}

fn bench_push(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let data = generate_test_data(TASK_COUNT);

    let mut group = c.benchmark_group("mongodb_push");
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
                cleanup_collections(&queue).await;
            });
        })
    });

    group.finish();
}

fn bench_pop(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let data = generate_test_data(TASK_COUNT);

    let mut group = c.benchmark_group("mongodb_pop");
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
                cleanup_collections(&queue).await;
            });
        })
    });

    group.finish();
}

fn bench_mixed_ops(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let data = generate_test_data(TASK_COUNT);

    let mut group = c.benchmark_group("mongodb_mixed");
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
                cleanup_collections(&queue).await;
            });
        })
    });

    group.finish();
}

criterion_group!(benches, bench_push, bench_pop, bench_mixed_ops);
criterion_main!(benches);
