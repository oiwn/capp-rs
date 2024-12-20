use async_trait::async_trait;
use rustis::client::{BatchPreparedCommand, Client, Pipeline};
use rustis::commands::{HashCommands, ListCommands};
use serde::{de::DeserializeOwned, Serialize};
use std::marker::PhantomData;

use crate::{Task, TaskId, TaskQueue, TaskQueueError, TaskSerializer};

pub struct RedisTaskQueue<D, S>
where
    S: TaskSerializer,
{
    pub client: Client,
    pub list_key: String,
    pub hashmap_key: String,
    pub dlq_key: String,
    _marker: PhantomData<(D, S)>,
}

impl<D, S> RedisTaskQueue<D, S>
where
    D: Send + Sync + 'static,
    S: TaskSerializer + Send + Sync,
{
    pub async fn new(
        client: Client,
        queue_name: &str,
    ) -> Result<Self, TaskQueueError> {
        Ok(Self {
            client,
            list_key: format!("{}:{}", queue_name, "ls"),
            hashmap_key: format!("{}:{}", queue_name, "hm"),
            dlq_key: format!("{}:{}", queue_name, "dlq"),
            _marker: PhantomData,
        })
    }

    async fn execute_pipeline(
        &self,
        pipeline: Pipeline<'_>,
    ) -> Result<(), TaskQueueError> {
        pipeline
            .execute()
            .await
            .map(|_: ()| ())
            .map_err(|e| TaskQueueError::QueueError(e.to_string()))
    }
}

#[async_trait]
impl<D, S> TaskQueue<D> for RedisTaskQueue<D, S>
where
    D: std::fmt::Debug
        + Clone
        + Serialize
        + DeserializeOwned
        + Send
        + Sync
        + 'static,
    S: TaskSerializer + Send + Sync,
{
    async fn push(&self, task: &Task<D>) -> Result<(), TaskQueueError> {
        let task_bytes = S::serialize_task(task)?;
        let task_str = String::from_utf8(task_bytes)
            .map_err(|e| TaskQueueError::Serialization(e.to_string()))?;

        let mut pipeline = self.client.create_pipeline();
        pipeline
            .rpush(&self.list_key, &task.task_id.to_string())
            .forget();
        pipeline
            .hset(&self.hashmap_key, [(&task.task_id.to_string(), &task_str)])
            .forget();

        self.execute_pipeline(pipeline).await
    }

    async fn pop(&self) -> Result<Task<D>, TaskQueueError> {
        let task_ids: Vec<String> = self
            .client
            .lpop(&self.list_key, 1)
            .await
            .map_err(|e| TaskQueueError::QueueError(e.to_string()))?;

        if let Some(task_id) = task_ids.first() {
            let task_str: String = self
                .client
                .hget(&self.hashmap_key, task_id)
                .await
                .map_err(|e| TaskQueueError::QueueError(e.to_string()))?;

            let task_bytes = task_str.as_bytes();
            S::deserialize_task(task_bytes)
        } else {
            Err(TaskQueueError::QueueEmpty)
        }
    }

    async fn ack(&self, task_id: &TaskId) -> Result<(), TaskQueueError> {
        self.client
            .hdel(&self.hashmap_key, &task_id.to_string())
            .await
            .map_err(|e| TaskQueueError::QueueError(e.to_string()))?;
        Ok(())
    }

    async fn nack(&self, task: &Task<D>) -> Result<(), TaskQueueError> {
        let task_bytes = S::serialize_task(task)?;
        let task_str = String::from_utf8(task_bytes)
            .map_err(|e| TaskQueueError::Serialization(e.to_string()))?;

        let mut pipeline = self.client.create_pipeline();
        pipeline.rpush(&self.dlq_key, &task_str).forget();
        pipeline
            .hdel(&self.hashmap_key, &task.task_id.to_string())
            .forget();

        self.execute_pipeline(pipeline).await
    }

    async fn set(&self, task: &Task<D>) -> Result<(), TaskQueueError> {
        let task_bytes = S::serialize_task(task)?;
        let task_str = String::from_utf8(task_bytes)
            .map_err(|e| TaskQueueError::Serialization(e.to_string()))?;

        self.client
            .hset(&self.hashmap_key, [(&task.task_id.to_string(), &task_str)])
            .await
            .map_err(|e| TaskQueueError::QueueError(e.to_string()))?;
        Ok(())
    }
}

impl<D, S> std::fmt::Debug for RedisTaskQueue<D, S>
where
    S: TaskSerializer,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RedisTaskQueue")
            .field("list_key", &self.list_key)
            .field("hashmap_key", &self.hashmap_key)
            .field("dlq_key", &self.dlq_key)
            .finish()
    }
}
