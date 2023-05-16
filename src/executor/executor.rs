use crate::executor::task::{Task, TaskProcessor, TaskStorage};
use chrono::Utc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

pub struct ExecutorOptions {
    // pub max_task_retries: usize,
    pub task_limit: Option<u32>,
    pub concurrency_limit: usize,
}

async fn worker<D, E, P, S>(worker_id: usize, storage: Arc<S>, processor: Arc<P>)
where
    D: Send + Sync + 'static + std::fmt::Debug,
    E: Send + Sync + 'static + std::error::Error,
    P: TaskProcessor<D, E> + Send + Sync + 'static,
    S: TaskStorage<D, E> + Send + Sync + 'static,
{
    let task: Option<Task<D, E>> = storage.list_pop().await.unwrap();
    if let Some(mut t) = task {
        match processor.process(worker_id, &mut t.data).await {
            Ok(_) => {
                t.finished = Some(Utc::now());
                storage.hashmap_set(&t).await.unwrap();
                let successful_task = storage.ack(&t).await.unwrap();
                log::info!(
                    "[worker-{}] Task {} succeed: {:?}",
                    worker_id,
                    &successful_task.task_id,
                    &successful_task.data
                );
            }
            Err(err) => {
                log::error!(
                    "[worker-{}] Task {} failed: {:?}",
                    worker_id,
                    &t.task_id,
                    &err
                );
                t.retries += 1;
                t.failed_with = Some(err);
                if t.retries < 3 {
                    storage.list_push(&t).await.unwrap();
                }
            }
        }
    } else {
        log::warn!("[worker-{}] No tasks found, waiting...", worker_id);
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    }
}

async fn worker_wrapper<D, E, P, S>(
    worker_id: usize,
    storage: Arc<S>,
    processor: Arc<P>,
    task_counter: Arc<AtomicU32>,
    task_limit: Option<u32>,
    limit_notify: Arc<tokio::sync::Notify>,
    mut shutdown: tokio::sync::watch::Receiver<()>,
) where
    D: Send + Sync + 'static + std::fmt::Debug,
    E: Send + Sync + 'static + std::error::Error,
    P: TaskProcessor<D, E> + Send + Sync + 'static,
    S: TaskStorage<D, E> + Send + Sync + 'static,
{
    'worker: loop {
        if let Some(limit) = task_limit {
            if task_counter.fetch_add(1, Ordering::SeqCst) >= limit {
                log::info!("Max tasks reached: {}", limit);
                limit_notify.notify_one();
                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                break 'worker;
            }
        };

        tokio::select! {
            _ = shutdown.changed() => {
                log::info!("[worker-{}] Worker shutting down...", worker_id);
                break 'worker;
            }
            _ = worker(worker_id, storage.clone(), processor.clone()) => {}

        };
    }
    log::info!("[worker-{}] finished", worker_id);
}

pub async fn run<D, E, P, S>(
    processor: Arc<P>,
    storage: Arc<S>,
    options: ExecutorOptions,
) where
    D: Send + Sync + 'static + std::fmt::Debug,
    E: Send + Sync + 'static + std::error::Error,
    P: TaskProcessor<D, E> + Send + Sync + 'static,
    S: TaskStorage<D, E> + Send + Sync + 'static,
{
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(());
    let limit_notify = Arc::new(tokio::sync::Notify::new());
    let task_counter = Arc::new(AtomicU32::new(0));

    let mut workers = Vec::new();

    for i in 1..=options.concurrency_limit {
        workers.push(tokio::spawn(worker_wrapper::<D, E, P, S>(
            i,
            Arc::clone(&storage),
            Arc::clone(&processor),
            Arc::clone(&task_counter),
            options.task_limit,
            Arc::clone(&limit_notify),
            shutdown_rx.clone(),
        )));
    }

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            log::warn!("Ctrl+C received, shutting down...");
            shutdown_tx.send(()).unwrap();
        }
        _ = limit_notify.notified() => {
            log::warn!("Task limit reached, shutting down...");
            shutdown_tx.send(()).unwrap();
        }
    }

    let results = futures::future::join_all(workers).await;
    for result in results {
        if let Err(e) = result {
            eprintln!("Error in one of the worker: {:?}", e);
        }
    }
}

/*
async fn worker<D, E, P, S>(
    storage: Arc<S>,
    processor: Arc<P>,
    task_counter: Arc<AtomicU32>,
    task_limit: Option<u32>,
    shutdown: tokio::sync::watch::Receiver<()>,
) where
    D: Send + Sync + 'static + std::fmt::Debug,
    E: Send + Sync + 'static + std::error::Error,
    P: TaskProcessor<D, E> + Send + Sync + 'static,
    S: TaskStorage<D, E> + Send + Sync + 'static,
{
    loop {
        // if `task_limit` tasks already executed, quit from worker
        if let Some(limit) = task_limit {
            if task_counter.load(Ordering::SeqCst) >= limit {
                log::info!("Max tasks reached: {}", limit);
                break;
            }
        };
        task_counter.fetch_add(1, Ordering::SeqCst);

        // Extracting and executring tasks
        let task: Option<Task<D, E>> = storage.list_pop().await.unwrap();
        if let Some(mut t) = task {
            match processor.process(&mut t.data).await {
                Ok(_) => {
                    t.finished = Some(Utc::now());
                    storage.hashmap_set(&t).await.unwrap();
                    log::info!("Task {} succeed: {:?}", &t.task_id, &t.data);
                }
                Err(err) => {
                    log::error!("Task {} failed: {:?}", &t.task_id, &err);
                    t.retries += 1;
                    t.failed_with = Some(err);
                    storage.list_push(&t).await.unwrap();
                }
            }
        } else {
            log::warn!("No tasks found, waiting...");
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        }

        // Check if shutdown event received
        if shutdown.borrow().has_changed() {
            log::info!("Worker shutting down...");
            break;
        }
    }
}

pub async fn run<D, E, P, S>(
    processor: Arc<P>,
    storage: Arc<S>,
    options: ExecutorOptions,
) where
    D: Send + Sync + 'static + std::fmt::Debug,
    E: Send + Sync + 'static + std::error::Error,
    P: TaskProcessor<D, E> + Send + Sync + 'static,
    S: TaskStorage<D, E> + Send + Sync + 'static,
{
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(());
    let task_counter = Arc::new(AtomicU32::new(0));

    let mut tasks = Vec::new();

    for _ in 0..options.concurrency_limit {
        tasks.push(tokio::spawn(worker::<D, E, P, S>(
            Arc::clone(&storage),
            Arc::clone(&processor),
            Arc::clone(&task_counter),
            options.task_limit,
            shutdown_rx.clone(),
        )));
    }

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            log::warn!("Ctrl+C received, shutting down...");
            shutdown_tx.send(()).unwrap();
        }
    }

    let results = futures::future::join_all(tasks).await;
    for result in results {
        if let Err(e) = result {
            eprintln!("Error in one of the tasks: {:?}", e);
        }
    }
}
*/

#[cfg(test)]
mod tests {}
