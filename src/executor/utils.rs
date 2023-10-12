use crate::config::Configurable;
use crate::executor::worker::WorkerId;
use crate::executor::{
    processor::TaskProcessor, worker::worker_wrapper, SharedStats, Worker,
    WorkerOptions, WorkerStats,
};
use crate::task_deport::TaskStorage;
use derive_builder::Builder;

use std::sync::{atomic::AtomicU32, Arc};
// use hyper::{
//     self,
//     service::{make_service_fn, service_fn},
//     Body, Request, Response, Server,
// };
use serde::{de::DeserializeOwned, Serialize};

#[derive(Builder, Default, Clone)]
#[builder(public, setter(into))]
pub struct ExecutorOptions {
    #[builder(
        default = "WorkerOptions { max_retries: 3, no_task_found_delay_sec: 10 }"
    )]
    pub worker_options: WorkerOptions,
    #[builder(default = "None")]
    pub task_limit: Option<u32>,
    #[builder(default = "4")]
    pub concurrency_limit: usize,
    #[builder(default = "10")]
    pub no_task_found_delay_sec: usize,
}

/*
async fn run_server(shared_stats: &SharedStats) {
    let make_svc = make_service_fn(|_conn| {
        // let shared_stats = Arc::clone(&shared_stats);
        async move {
            Ok::<_, hyper::Error>(service_fn(move |_req: Request<Body>| {
                handle_request(_req, shared_stats)
            }))
        }
    });

    let addr = ([127, 0, 0, 1], 3000).into();
    let server = Server::bind(&addr).serve(make_svc);

    if let Err(e) = server.await {
        eprintln!("server error: {}", e);
    }
}

async fn handle_request(
    _req: Request<Body>,
    shared_stats: &SharedStats,
) -> Result<Response<Body>, hyper::Error> {
    let stats = shared_stats.lock().unwrap(); // Handle this unwrap more gracefully in production
    let json = serde_json::json!(*stats).to_string();
    Ok(Response::new(Body::from(json)))
}
*/

/// Runs the executor with the provided task processor, storage, and options.
/// This function creates a number of workers based on the concurrency limit option.
/// It then waits for either a shutdown signal (Ctrl+C) or for the task limit
/// to be reached. In either case, it sends a shutdown signal to all workers
/// and waits for them to finish.
pub async fn run_workers<D, P, S, C>(
    ctx: Arc<C>,
    processor: Arc<P>,
    storage: Arc<S>,
    options: ExecutorOptions,
) where
    D: Clone
        + Serialize
        + DeserializeOwned
        + Send
        + Sync
        + 'static
        + std::fmt::Debug,
    P: TaskProcessor<D, S, C> + Send + Sync + 'static,
    S: TaskStorage<D> + Send + Sync + 'static,
    C: Configurable + Send + Sync + 'static,
{
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(());
    let limit_notify = Arc::new(tokio::sync::Notify::new());
    let task_counter = Arc::new(AtomicU32::new(0));

    let mut worker_handlers = Vec::new();

    for i in 1..=options.concurrency_limit {
        worker_handlers.push(tokio::spawn(worker_wrapper::<D, P, S, C>(
            WorkerId::new(i),
            Arc::clone(&ctx),
            Arc::clone(&storage),
            Arc::clone(&processor),
            Arc::clone(&task_counter),
            options.task_limit,
            Arc::clone(&limit_notify),
            shutdown_rx.clone(),
            options.worker_options.clone(),
        )));
    }

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            tracing::warn!("Ctrl+C received, shutting down...");
            shutdown_tx.send(()).unwrap();
        }
        _ = limit_notify.notified() => {
            tracing::warn!("Task limit reached, shutting down...");
            shutdown_tx.send(()).unwrap();
        }
    }

    let results = futures::future::join_all(worker_handlers).await;
    for result in results {
        if let Err(e) = result {
            tracing::error!("Fatal error in one of the workers: {:?}", e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excutor_options_builder() {
        let executor_options = ExecutorOptionsBuilder::default().build().unwrap();
        assert_eq!(executor_options.concurrency_limit, 4);
    }
}
