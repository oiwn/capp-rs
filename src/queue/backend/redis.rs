//! Provides implementation of trait to store task into redis
use crate::prelude::*;
use async_trait::async_trait;
use rustis::client::{BatchPreparedCommand, Client, Pipeline};
use rustis::commands::{HashCommands, ListCommands};
use serde::{de::DeserializeOwned, Serialize};
use std::marker::PhantomData;

pub struct RedisTaskQueue<D> {
    pub client: Client,
    pub list_key: String,
    pub hashmap_key: String,
    pub dlq_key: String,
    _marker: PhantomData<D>,
}

impl<D> RedisTaskQueue<D> {
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
            .map_err(|e| TaskQueueError::QueueError(e.to_string()))?;
        Ok(())
    }
}

#[async_trait]
impl<D> TaskQueue<D> for RedisTaskQueue<D>
where
    D: std::fmt::Debug
        + Clone
        + Serialize
        + DeserializeOwned
        + Send
        + Sync
        + 'static,
{
    async fn push(&self, task: &Task<D>) -> Result<(), TaskQueueError> {
        let task_json = serde_json::to_string(task)
            .map_err(|e| TaskQueueError::SerdeError(e.to_string()))?;

        let mut pipeline = self.client.create_pipeline();
        pipeline
            .rpush(&self.list_key, &task.task_id.to_string())
            .forget();
        pipeline
            .hset(&self.hashmap_key, [(&task.task_id.to_string(), &task_json)])
            .forget();
        self.execute_pipeline(pipeline).await
    }

    async fn pop(&self) -> Result<Task<D>, TaskQueueError> {
        let task_ids: Vec<String> = self
            .client
            .lpop(&self.list_key, 1)
            .await
            .map_err(|e| TaskQueueError::QueueError(e.to_string()))?;

        dbg!(&task_ids);

        if !task_ids.is_empty() {
            let task_id = task_ids.first().unwrap();
            let task_value: String =
                self.client.hget(&self.hashmap_key, task_id).await?;

            let task: Task<D> = serde_json::from_str(&task_value)
                .map_err(|err| TaskQueueError::SerdeError(err.to_string()))?;
            return Ok(task);
        }

        Err(TaskQueueError::QueueEmpty)
    }

    async fn ack(&self, task_id: &TaskId) -> Result<(), TaskQueueError> {
        let uuid_as_str = task_id.to_string();
        let _ = self.client.hdel(&self.hashmap_key, &uuid_as_str).await?;
        Ok(())
    }

    async fn nack(&self, task: &Task<D>) -> Result<(), TaskQueueError> {
        let uuid_as_str = task.task_id.to_string();
        let task_json = serde_json::to_string(task)
            .map_err(|e| TaskQueueError::SerdeError(e.to_string()))?;

        let mut pipeline = self.client.create_pipeline();
        pipeline.rpush(&self.dlq_key, &task_json).forget();
        pipeline.hdel(&self.hashmap_key, &uuid_as_str).forget();
        self.execute_pipeline(pipeline).await
    }

    async fn set(&self, task: &Task<D>) -> Result<(), TaskQueueError> {
        let task_json = serde_json::to_string(task)
            .map_err(|e| TaskQueueError::SerdeError(e.to_string()))?;

        self.client
            .hset(&self.hashmap_key, [(&task.task_id.to_string(), &task_json)])
            .await
            .map_err(|e| TaskQueueError::QueueError(e.to_string()))?;
        Ok(())
    }
}
