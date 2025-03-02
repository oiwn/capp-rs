use super::WorkerId;
use crate::manager::{
    Computation, WorkerCommand, WorkerOptions, WorkerOptionsBuilder, worker_wrapper,
};
use capp_config::Configurable;
use capp_queue::queue::{AbstractTaskQueue, TaskQueue};
use derive_builder::Builder;
use serde::{Serialize, de::DeserializeOwned};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio::{
    signal,
    sync::{broadcast, mpsc},
};
use tracing::Instrument;

type WorkerCommandSenders =
    Arc<Mutex<HashMap<WorkerId, mpsc::Sender<WorkerCommand>>>>;

#[derive(Builder, Default, Clone, Debug)]
#[builder(public, setter(into))]
pub struct WorkersManagerOptions {
    #[builder(default = "WorkerOptionsBuilder::default().build().unwrap()")]
    pub worker_options: WorkerOptions,
    #[builder(default = "None")]
    pub task_limit: Option<u32>,
    #[builder(default = "4")]
    pub concurrency_limit: usize,
    #[builder(default = "10")]
    pub no_task_found_delay_sec: usize,
}

// New WorkersManager struct
pub struct WorkersManager<Data, Comp, Ctx> {
    pub ctx: Arc<Ctx>,
    pub computation: Arc<Comp>,
    // pub queue: Arc<dyn TaskQueue<Data> + Send + Sync>,
    pub queue: AbstractTaskQueue<Data>,
    pub options: WorkersManagerOptions,
}

impl<Data, Comp, Ctx> WorkersManager<Data, Comp, Ctx>
where
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
    pub fn new(
        ctx: Ctx,
        computation: Comp,
        queue: impl TaskQueue<Data> + Send + Sync + 'static,
        options: WorkersManagerOptions,
    ) -> Self {
        Self {
            ctx: Arc::new(ctx),
            computation: Arc::new(computation),
            queue: Arc::new(queue),
            options,
        }
    }

    pub fn new_from_arcs(
        ctx: Arc<Ctx>,
        computation: Arc<Comp>,
        // queue: Arc<dyn TaskQueue<Data> + Send + Sync>,
        queue: AbstractTaskQueue<Data>,
        options: WorkersManagerOptions,
    ) -> Self {
        Self {
            ctx,
            computation,
            queue,
            options,
        }
    }

    pub async fn run_workers(&mut self) {
        // This will start the workers and handle the shutdown signals
        let mut worker_handlers = Vec::new();
        let command_senders: WorkerCommandSenders =
            Arc::new(Mutex::new(HashMap::new()));
        let (terminate_sender, _) = broadcast::channel::<()>(10);

        for i in 1..=self.options.concurrency_limit {
            let worker_id = WorkerId::new(i);
            let (command_sender, command_receiver) =
                mpsc::channel::<WorkerCommand>(100);

            command_senders
                .lock()
                .unwrap()
                .insert(worker_id, command_sender);
            let terminate_receiver = terminate_sender.subscribe();

            let worker_span = tracing::info_span!("worker", worker_id = %i);
            let worker = tokio::spawn(worker_wrapper::<Data, Comp, Ctx>(
                WorkerId::new(i),
                Arc::clone(&self.ctx),
                Arc::clone(&self.queue),
                Arc::clone(&self.computation),
                command_receiver,
                terminate_receiver,
                self.options.worker_options.clone(),
            ))
            .instrument(worker_span.clone());
            worker_handlers.push(worker);
        }

        // Following part setup separate thread to catch ctrl+c
        // signal. Single press will send Shutdown signal to all workers.
        // next ctrl-c will terminate workers immediately.
        self.ctrl_c_handler(command_senders, terminate_sender).await;

        for (worker_id, handler) in worker_handlers.into_iter().enumerate() {
            let worker_id = worker_id + 1;
            match handler.await {
                Ok(res) => {
                    tracing::info!("[{}] Worker stopped: {:?}", worker_id, res);
                }
                Err(err) => {
                    tracing::error!(
                        "[{}] Fatal error in one of the workers: {:?}",
                        worker_id,
                        err
                    );
                }
            }
        }

        tracing::info!("All workers stopped")
    }

    async fn ctrl_c_handler(
        &mut self,
        command_senders: WorkerCommandSenders,
        terminate_sender: tokio::sync::broadcast::Sender<()>,
    ) {
        let ctrl_c_counter = Arc::new(AtomicUsize::new(0));

        // Setup signal handling
        let signal_counter = ctrl_c_counter.clone();
        let command_senders = command_senders.clone();

        tokio::spawn(async move {
            loop {
                signal::ctrl_c()
                    .await
                    .expect("Failed to listen for ctrl+c event");
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
                            let _ = sender.send(WorkerCommand::Shutdown).await;
                        }
                    }
                    _ => {
                        // Second Ctrl+C: Force terminate all workers.
                        tracing::warn!(
                            "Ctrl+C received again, terminating all workers..."
                        );
                        terminate_sender.send(()).unwrap();
                        break;
                    }
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excutor_options_builder() {
        let options = WorkersManagerOptionsBuilder::default().build().unwrap();
        assert_eq!(options.concurrency_limit, 4);
    }
}
