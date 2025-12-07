mod common;

use capp_queue::{FjallTaskQueue, JsonSerializer, Task, TaskQueue};
use common::data::{BenchTaskData, generate_test_data};
use criterion::{Criterion, criterion_group, criterion_main};
use tempfile::tempdir;
use tokio::runtime::Runtime;

const TASK_COUNT: usize = 1_000;

fn bench_push(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let data = generate_test_data(TASK_COUNT);

    let mut group = c.benchmark_group("fjall_push");
    group.sample_size(10);

    group.bench_function("bulk_push_1000", |b| {
        b.iter(|| {
            let dir = tempdir().unwrap();
            let queue =
                FjallTaskQueue::<BenchTaskData, JsonSerializer>::open(dir.path())
                    .unwrap();
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

    let mut group = c.benchmark_group("fjall_pop");
    group.sample_size(10);

    group.bench_function("bulk_pop_1000", |b| {
        b.iter(|| {
            let dir = tempdir().unwrap();
            let queue =
                FjallTaskQueue::<BenchTaskData, JsonSerializer>::open(dir.path())
                    .unwrap();

            rt.block_on(async {
                for item in &data {
                    queue.push(&Task::new(item.clone())).await.unwrap();
                }

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

    let mut group = c.benchmark_group("fjall_mixed");
    group.sample_size(10);

    group.bench_function("push_pop_ack_cycle", |b| {
        b.iter(|| {
            let dir = tempdir().unwrap();
            let queue =
                FjallTaskQueue::<BenchTaskData, JsonSerializer>::open(dir.path())
                    .unwrap();

            rt.block_on(async {
                for chunk in data.chunks(100) {
                    for item in chunk {
                        queue.push(&Task::new(item.clone())).await.unwrap();
                    }

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
