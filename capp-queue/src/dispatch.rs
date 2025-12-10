use thiserror::Error;
use tokio::sync::mpsc;

use crate::Task;

/// Message sent from user code into the dispatcher so enqueue operations are
/// centralized in a single task.
#[derive(Debug)]
pub enum ProducerMsg<D: Clone> {
    Enqueue(Task<D>),
}

/// Cloneable handle that lets user-facing services enqueue tasks without
/// touching the queue directly. Internally this just forwards to the dispatcher
/// over a bounded channel.
#[derive(Clone, Debug)]
pub struct ProducerHandle<D: Clone> {
    tx: mpsc::Sender<ProducerMsg<D>>,
}

impl<D> ProducerHandle<D>
where
    D: Clone + Send + 'static,
{
    pub fn new(tx: mpsc::Sender<ProducerMsg<D>>) -> Self {
        Self { tx }
    }

    /// Convenience to build a bounded producer channel and the receiving side.
    pub fn channel(
        buffer: usize,
    ) -> (ProducerHandle<D>, mpsc::Receiver<ProducerMsg<D>>) {
        let (tx, rx) = mpsc::channel(buffer);
        (ProducerHandle { tx }, rx)
    }

    pub async fn enqueue(&self, task: Task<D>) -> Result<(), ProducerError> {
        self.tx
            .send(ProducerMsg::Enqueue(task))
            .await
            .map_err(|_| ProducerError::ChannelClosed)
    }

    pub fn sender(&self) -> mpsc::Sender<ProducerMsg<D>> {
        self.tx.clone()
    }
}

/// Result sent from a worker back to the dispatcher after a task finishes.
#[derive(Debug)]
pub enum WorkerResult<D: Clone> {
    Ack { task: Task<D> },
    Nack { task: Task<D>, error: String },
    Return { task: Task<D> },
}

#[derive(Debug, Error)]
pub enum ProducerError {
    #[error("producer channel closed")]
    ChannelClosed,
}
