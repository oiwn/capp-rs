use super::WorkerId;
use crate::config::Configurable;
use crate::task_deport::TaskStorage;
use crate::task_executor::{
    worker_wrapper, Computation, WorkerCommand, WorkerOptions,
};
use derive_builder::Builder;
use tokio::{signal, sync::mpsc};

use serde::{de::DeserializeOwned, Serialize};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

type WorkerCommandSenders =
    Arc<Mutex<HashMap<WorkerId, mpsc::Sender<WorkerCommand>>>>;

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

/// Runs the executor with the provided task processor, storage, and options.
/// This function creates a number of workers based on the concurrency limit option.
/// It then waits for either a shutdown signal (Ctrl+C) or for the task limit
/// to be reached. In either case, it sends a shutdown signal to all workers
/// and waits for them to finish.
pub async fn run_workers<Data, Comp, Ctx>(
    ctx: Arc<Ctx>,
    computation: Arc<Comp>,
    storage: Arc<dyn TaskStorage<Data> + Send + Sync>,
    options: ExecutorOptions,
) where
    Data: Clone
        + Serialize
        + DeserializeOwned
        + Send
        + Sync
        + 'static
        + std::fmt::Debug,
    Comp: Computation<Data, Ctx> + Send + Sync + 'static,
    Ctx: Configurable + Send + Sync + 'static,
{
    let mut worker_handlers = Vec::new();
    let command_senders: WorkerCommandSenders =
        Arc::new(Mutex::new(HashMap::new()));

    for i in 1..=options.concurrency_limit {
        let worker_id = WorkerId::new(i);
        let (command_sender, command_receiver) =
            mpsc::channel::<WorkerCommand>(100);

        command_senders
            .lock()
            .unwrap()
            .insert(worker_id.clone(), command_sender);

        worker_handlers.push(tokio::spawn(worker_wrapper::<Data, Comp, Ctx>(
            WorkerId::new(i),
            Arc::clone(&ctx),
            Arc::clone(&storage),
            Arc::clone(&computation),
            command_receiver,
            options.worker_options.clone(),
        )));
    }

    let ctrl_c_counter = Arc::new(AtomicUsize::new(0));

    // Setup signal handling
    let signal_counter = ctrl_c_counter.clone();
    let command_senders = command_senders.clone();

    tokio::spawn(async move {
        loop {
            signal::ctrl_c().await.expect("Failed to listen for event");
            let count = signal_counter.fetch_add(1, Ordering::SeqCst);

            match count {
                0 => {
                    // First Ctrl+C: Attempt to gracefully stop all workers.
                    tracing::warn!(
                        "Ctrl+C received, sending stop command to all workers..."
                    );
                    let senders: Vec<_> = {
                        let lock = command_senders.lock().unwrap();
                        lock.values().cloned().collect()
                    };
                    for sender in senders {
                        let _ = sender.send(WorkerCommand::Stop).await;
                    }
                }
                _ => {
                    // Second Ctrl+C: Force terminate all workers.
                    tracing::warn!(
                        "Ctrl+C received again, terminating all workers..."
                    );
                    let senders: Vec<_> = {
                        let lock = command_senders.lock().unwrap();
                        lock.values().cloned().collect()
                    };
                    for sender in senders {
                        let _ = sender.send(WorkerCommand::Terminate).await;
                    }
                    break;
                }
            }
        }
    });

    // tokio::select! {
    //     _ = tokio::signal::ctrl_c() => {
    //         tracing::warn!("Ctrl+C received, shutting down...");
    //         let senders = command_senders.lock().unwrap();
    //         for sender in senders.values() {
    //             let _ = sender.send(crate::WorkerCommand::Stop).await;
    //         }
    //     }
    // }

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
