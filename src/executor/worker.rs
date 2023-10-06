use super::processor::TaskProcessor;
use crate::config::Configurable;
use serde::{de::DeserializeOwned, Serialize};
use std::sync::Arc;
use task_deport::{Task, TaskStorage};

#[derive(Clone, Default)]
pub struct WorkerOptions {
    pub max_retries: u32,
    pub no_task_found_delay_sec: u64,
}

pub struct Worker<D, PE, SE, P, S, C> {
    worker_id: usize,
    ctx: Arc<C>,
    storage: Arc<S>,
    processor: Arc<P>,
    options: WorkerOptions,
    // phantom
    _payload_type: std::marker::PhantomData<D>,
    _processor_error_type: std::marker::PhantomData<PE>,
    _storage_error_type: std::marker::PhantomData<SE>,
}

/// A worker implementation that fetches a task from the storage, processes it,
/// and then updates the task status. If the processing fails,
/// the task is retried up to N times.
impl<D, PE, SE, P, S, C> Worker<D, PE, SE, P, S, C>
where
    D: std::fmt::Debug
        + Clone
        + Serialize
        + DeserializeOwned
        + Send
        + Sync
        + 'static,
    PE: std::error::Error + Send + Sync + 'static,
    SE: std::error::Error + Send + Sync + 'static,
    P: TaskProcessor<D, PE, S, C> + Send + Sync + 'static,
    S: TaskStorage<D, SE> + Send + Sync + 'static,
    C: Configurable + Send + Sync + 'static,
{
    pub fn new(
        worker_id: usize,
        ctx: Arc<C>,
        storage: Arc<S>,
        processor: Arc<P>,
        options: WorkerOptions,
    ) -> Self {
        Self {
            worker_id,
            ctx,
            storage,
            processor,
            options,
            _payload_type: std::marker::PhantomData,
            _storage_error_type: std::marker::PhantomData,
            _processor_error_type: std::marker::PhantomData,
        }
    }

    pub async fn run(&self) {
        let task: Option<Task<D>> = self.storage.task_pop().await.unwrap();

        if let Some(mut t) = task {
            t.set_in_process();
            match self
                .processor
                .process(
                    self.worker_id,
                    self.ctx.clone(),
                    self.storage.clone(),
                    &mut t,
                )
                .await
            {
                Ok(_) => {
                    t.set_succeed();
                    self.storage.task_set(&t).await.unwrap();
                    let successful_task =
                        self.storage.task_ack(&t.task_id).await.unwrap();
                    tracing::info!(
                        "[worker-{}] Task {} succeed: {:?}",
                        self.worker_id,
                        &successful_task.task_id,
                        &successful_task.payload
                    );
                }
                Err(err) => {
                    t.set_retry(&err.to_string());
                    if t.retries < self.options.max_retries {
                        self.storage.task_push(&t).await.unwrap();

                        tracing::error!(
                            "[worker-{}] Task {} failed, retrying ({}): {:?}",
                            self.worker_id,
                            &t.task_id,
                            &t.retries,
                            &err
                        );
                    } else {
                        // TODO: send to dlq
                        t.set_dlq("Max retries");
                        self.storage.task_to_dlq(&t).await.unwrap();
                        tracing::error!(
                        "[worker-{}] Task {} failed, exceed retyring attempts ({}): {:?}",
                        self.worker_id, &t.task_id, &t.retries, &err
                    );
                    }
                }
            }
        } else {
            tracing::warn!(
                "[worker-{}] No tasks found, waiting...",
                self.worker_id
            );
            tokio::time::sleep(tokio::time::Duration::from_secs(
                self.options.no_task_found_delay_sec,
            ))
            .await;
        }
    }
}
