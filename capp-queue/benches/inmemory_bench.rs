mod common;

use capp_queue::{InMemoryTaskQueue, JsonSerializer, Task, TaskQueue};
use criterion::{Criterion, criterion_group, criterion_main};
use tokio::runtime::Runtime;

use common::data::{BenchTaskData, generate_test_data};

const TASK_COUNT: usize = 1000;

fn bench_push(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let data = generate_test_data(TASK_COUNT);

    let mut group = c.benchmark_group("inmemory_push");
    group.sample_size(10);

    group.bench_function("bulk_push_1000", |b| {
        b.iter(|| {
            let queue = InMemoryTaskQueue::<BenchTaskData, JsonSerializer>::new();
            rt.block_on(async {
                for item in &data {
                    queue.push(&Task::new(item.clone())).await.unwrap();
                }
            });
        })
    });

    group.finish();
}

fn bench_pop(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let data = generate_test_data(TASK_COUNT);

    let mut group = c.benchmark_group("inmemory_pop");
    group.sample_size(10);

    group.bench_function("bulk_pop_1000", |b| {
        b.iter(|| {
            let queue = InMemoryTaskQueue::<BenchTaskData, JsonSerializer>::new();

            // Setup - push all tasks first
            rt.block_on(async {
                for item in &data {
                    queue.push(&Task::new(item.clone())).await.unwrap();
                }

                // Benchmark popping
                for _ in 0..TASK_COUNT {
                    queue.pop().await.unwrap();
                }
            });
        })
    });

    group.finish();
}

fn bench_mixed_ops(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let data = generate_test_data(TASK_COUNT);

    let mut group = c.benchmark_group("inmemory_mixed");
    group.sample_size(10);

    group.bench_function("push_pop_ack_cycle", |b| {
        b.iter(|| {
            let queue = InMemoryTaskQueue::<BenchTaskData, JsonSerializer>::new();

            rt.block_on(async {
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
            });
        })
    });

    group.finish();
}

criterion_group!(benches, bench_push, bench_pop, bench_mixed_ops);
criterion_main!(benches);
