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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Task;

    #[tokio::test]
    async fn channel_enqueue_delivers_task() {
        let (producer, mut rx) = ProducerHandle::channel(4);
        let task = Task::new(42u32);

        producer.enqueue(task.clone()).await.unwrap();

        match rx.recv().await {
            Some(ProducerMsg::Enqueue(received)) => {
                assert_eq!(received.payload, task.payload);
            }
            other => panic!("unexpected message: {other:?}"),
        }
    }

    #[tokio::test]
    async fn sender_clone_enqueues() {
        let (tx, mut rx) = mpsc::channel(4);
        let producer = ProducerHandle::new(tx);
        let sender = producer.sender();
        let task = Task::new("hi");

        sender.send(ProducerMsg::Enqueue(task)).await.unwrap();

        let msg = rx.recv().await;
        assert!(matches!(msg, Some(ProducerMsg::Enqueue(_))));
    }

    #[tokio::test]
    async fn enqueue_errors_when_channel_closed() {
        let (producer, rx) = ProducerHandle::channel(1);
        drop(rx);

        let result = producer.enqueue(Task::new(7u32)).await;
        assert!(matches!(result, Err(ProducerError::ChannelClosed)));
    }
}
