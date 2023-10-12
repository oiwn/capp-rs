//! Provides implementation of trait to store task into redis
//! TODO: make sequental ops into atomic transaction
use crate::{Task, TaskId, TaskStorage, TaskStorageError};
use async_trait::async_trait;
use rustis::commands::{HashCommands, ListCommands};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::marker::PhantomData;

/// A simple implementation of the `TaskStorage` trait on top of redis
pub struct RedisTaskStorage<D> {
    pub key: String,
    pub redis: rustis::client::Client,
    _marker1: PhantomData<D>,
}

impl<D> RedisTaskStorage<D> {
    /// Construct a new empty redis task storage
    pub fn new(key: &str, redis: rustis::client::Client) -> Self {
        Self {
            key: key.to_string(),
            redis,
            _marker1: PhantomData,
        }
    }

    pub fn get_hashmap_key(&self) -> String {
        format!("{}:{}", self.key, "hm")
    }

    pub fn get_list_key(&self) -> String {
        format!("{}:{}", self.key, "ls")
    }

    pub fn get_dlq_key(&self) -> String {
        format!("{}:{}", self.key, "dlq")
    }
}

impl<D> std::fmt::Debug for RedisTaskStorage<D> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // TODO: implement debug output for data in redis
        // Use the debug builders to format the output.
        // NOTE: here i will need to do actual queries to redis
        f.debug_struct("RedisTaskStorage")
            // .field("hashmap", &*hashmap)
            // .field("list", &*list)
            .finish()
    }
}

#[async_trait]
impl<D> TaskStorage<D> for RedisTaskStorage<D>
where
    D: Clone + Serialize + DeserializeOwned + Send + Sync + 'static,
{
    async fn task_ack(
        &self,
        task_id: &TaskId,
    ) -> Result<Task<D>, TaskStorageError> {
        let hashmap_key = self.get_hashmap_key();
        let uuid_as_str = task_id.to_string();
        let task_value: String =
            self.redis.hget(&hashmap_key, &uuid_as_str).await?;
        let _ = self.redis.hdel(hashmap_key, &uuid_as_str).await;
        let task = serde_json::from_str(&task_value).map_err(|err| {
            TaskStorageError::DeserializationError(err.to_string())
        })?;
        Ok(task)
    }

    async fn task_get(
        &self,
        task_id: &TaskId,
    ) -> Result<Task<D>, TaskStorageError> {
        let hashmap_key = self.get_hashmap_key();
        let uuid_as_str = task_id.to_string();
        let task_value: String =
            self.redis.hget(&hashmap_key, &uuid_as_str).await?;
        let task_data: Task<D> =
            serde_json::from_str(&task_value).map_err(|err| {
                TaskStorageError::DeserializationError(err.to_string())
            })?;
        Ok(task_data)
    }

    async fn task_set(&self, task: &Task<D>) -> Result<(), TaskStorageError> {
        let hashmap_key = self.get_hashmap_key();
        let task_value: String = serde_json::to_string(task)
            .map_err(|err| TaskStorageError::SerializationError(err.to_string()))?;
        let uuid_as_str = task.task_id.to_string();
        let _ = self
            .redis
            .hset(&hashmap_key, [(&uuid_as_str, &task_value)])
            .await;
        Ok(())
    }

    async fn task_pop(&self) -> Result<Task<D>, TaskStorageError> {
        let list_key = self.get_list_key();
        let hashmap_key = self.get_hashmap_key();
        let task_ids: Vec<String> = self.redis.rpop(&list_key, 1).await?;
        if task_ids.len() > 0 {
            let task_id = task_ids.first().unwrap();
            let task_value: String = self.redis.hget(&hashmap_key, task_id).await?;
            let task: Task<D> =
                serde_json::from_str(&task_value).map_err(|err| {
                    TaskStorageError::DeserializationError(err.to_string())
                })?;
            return Ok(task);
        }

        Err(TaskStorageError::StorageIsEmptyError)
    }

    async fn task_push(&self, task: &Task<D>) -> Result<(), TaskStorageError> {
        let task_value = serde_json::to_string(task)
            .map_err(|err| TaskStorageError::SerializationError(err.to_string()))?;
        let list_key = self.get_list_key();
        let hashmap_key = self.get_hashmap_key();
        let uuid_as_str = task.task_id.to_string();

        let _ = self.redis.lpush(&list_key, &uuid_as_str).await?;
        let _ = self
            .redis
            .hset(&hashmap_key, [(&uuid_as_str, &task_value)])
            .await?;
        Ok(())
    }

    async fn task_to_dlq(&self, task: &Task<D>) -> Result<(), TaskStorageError> {
        let task_value = serde_json::to_string(task)
            .map_err(|err| TaskStorageError::SerializationError(err.to_string()))?;
        let dlq_key = self.get_dlq_key();
        let uuid_as_str = task.task_id.to_string();

        let _ = self
            .redis
            .hset(&dlq_key, [(&uuid_as_str, &task_value)])
            .await?;
        Ok(())
    }
}
