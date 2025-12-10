pub mod backend;
pub mod dispatch;
pub mod queue;
pub mod serializers;
pub mod task;

#[cfg(feature = "fjall")]
pub use crate::backend::FjallTaskQueue;
pub use crate::backend::InMemoryTaskQueue;
#[cfg(feature = "mongodb")]
pub use crate::backend::MongoTaskQueue;
#[cfg(feature = "postgres")]
pub use crate::backend::PostgresTaskQueue;
#[cfg(feature = "redis")]
pub use crate::backend::{RedisRoundRobinTaskQueue, RedisTaskQueue};
pub use crate::dispatch::{
    ProducerError, ProducerHandle, ProducerMsg, WorkerResult,
};
pub use crate::queue::{AbstractTaskQueue, HasTagKey, TaskQueue};
#[cfg(feature = "mongodb")]
pub use crate::serializers::BsonSerializer;
pub use crate::serializers::{JsonSerializer, TaskSerializer};
pub use crate::task::{Task, TaskId, TaskStatus};

use thiserror::Error;

#[derive(Error, Debug)]
pub enum TaskQueueError {
    #[error("Queue error: {0}")]
    QueueError(String),
    #[error("Ser/De error: {0}")]
    SerdeError(String),
    #[error("Serialization error: {0}")]
    Serialization(String),
    #[error("Deserialization error: {0}")]
    Deserialization(String),
    #[error("Task not found: {0}")]
    TaskNotFound(TaskId),
    #[error("Queue is empty")]
    QueueEmpty,
    #[cfg(feature = "fjall")]
    #[error("Fjall error")]
    FjallError(#[from] fjall::Error),
    #[cfg(feature = "redis")]
    #[error("Redis error")]
    RedisError(#[from] rustis::Error),
    #[cfg(feature = "mongodb")]
    #[error("Mongodb Error")]
    MongodbError(#[from] mongodb::error::Error),
    #[cfg(feature = "postgres")]
    #[error("Postgres Error")]
    PostgresError(#[from] sqlx::Error),
}
