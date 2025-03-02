mod common;

use capp_queue::{JsonSerializer, PostgresTaskQueue, TaskQueue};
use criterion::{Criterion, criterion_group, criterion_main};
use dotenvy::dotenv;
use sqlx::PgPool;
use tokio::runtime::Runtime;

use capp_queue::task::Task;
use common::data::{BenchTaskData, generate_test_data};

const TASK_COUNT: usize = 1000;

async fn get_db_connection() -> String {
    dotenv().ok();
    std::env::var("DATABASE_URL").expect("Set DATABASE_URL env variable")
}

async fn setup_queue() -> PostgresTaskQueue<BenchTaskData, JsonSerializer> {
    let db_url = get_db_connection().await;
    let queue = PostgresTaskQueue::new(&db_url)
        .await
        .expect("Failed to create PostgresTaskQueue");

    // Clean any existing data
    cleanup_database(&queue.pool).await;

    queue
}

async fn cleanup_database(pool: &PgPool) {
    // Clean both tables
    sqlx::query!("TRUNCATE TABLE tasks, dlq")
        .execute(pool)
        .await
        .expect("Failed to clean database");
}

fn bench_push(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let data = generate_test_data(TASK_COUNT);

    let mut group = c.benchmark_group("postgres_push");
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
                cleanup_database(&queue.pool).await;
            });
        })
    });

    group.finish();
}

fn bench_pop(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let data = generate_test_data(TASK_COUNT);

    let mut group = c.benchmark_group("postgres_pop");
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
                cleanup_database(&queue.pool).await;
            });
        })
    });

    group.finish();
}

fn bench_mixed_ops(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let data = generate_test_data(TASK_COUNT);

    let mut group = c.benchmark_group("postgres_mixed");
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
                cleanup_database(&queue.pool).await;
            });
        })
    });

    group.finish();
}

criterion_group!(benches, bench_push, bench_pop, bench_mixed_ops);
criterion_main!(benches);
