//! Provides implementation of trait to store task into redis
//! TODO: make sequental ops into atomic transaction
use async_trait::async_trait;
use rustis::commands::{HashCommands, ListCommands};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::marker::PhantomData;
use thiserror::Error;
use uuid::Uuid;

use crate::{Task, TaskStorage};

#[derive(Error, Debug)]
pub enum RedisTaskStorageError {
    #[error("lock error")]
    LockError,

    #[error("key {0} error")]
    KeyError(Uuid),

    #[error(transparent)]
    RedisError(#[from] rustis::Error),

    #[error(transparent)]
    SerializationError(#[from] serde_json::Error),
}

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

    fn get_hashmap_key(&self) -> String {
        format!("{}:{}", self.key, "hm")
    }

    fn get_list_key(&self) -> String {
        format!("{}:{}", self.key, "ls")
    }
}

impl<D> std::fmt::Debug for RedisTaskStorage<D> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // TODO: implement debug output for data in redis
        // Use the debug builders to format the output.
        f.debug_struct("RedisTaskStorage")
            // .field("hashmap", &*hashmap)
            // .field("list", &*list)
            .finish()
    }
}

#[async_trait]
impl<D> TaskStorage<D, RedisTaskStorageError> for RedisTaskStorage<D>
where
    D: Serialize + DeserializeOwned + Send + Sync + 'static,
{
    async fn task_ack(
        &self,
        task_id: &Uuid,
    ) -> Result<Task<D>, RedisTaskStorageError> {
        let hashmap_key = self.get_hashmap_key();
        let uuid_as_str = task_id.to_string();
        let task_value: String =
            self.redis.hget(&hashmap_key, &uuid_as_str).await?;
        let _ = self.redis.hdel(hashmap_key, &uuid_as_str).await;
        let task = serde_json::from_str(&task_value)?;
        Ok(task)
    }

    async fn task_get(
        &self,
        task_id: &Uuid,
    ) -> Result<Task<D>, RedisTaskStorageError> {
        let hashmap_key = self.get_hashmap_key();
        let uuid_as_str = task_id.to_string();
        let task_value: String =
            self.redis.hget(&hashmap_key, &uuid_as_str).await?;
        let task_data: Task<D> = serde_json::from_str(&task_value)?;
        Ok(task_data)
    }

    async fn task_set(&self, task: &Task<D>) -> Result<(), RedisTaskStorageError> {
        let hashmap_key = self.get_hashmap_key();
        let task_value: String = serde_json::to_string(task)?;
        let uuid_as_str = task.task_id.to_string();
        let _ = self
            .redis
            .hset(&hashmap_key, [(&uuid_as_str, &task_value)])
            .await;
        Ok(())
    }

    async fn task_pop(&self) -> Result<Option<Task<D>>, RedisTaskStorageError> {
        let list_key = self.get_list_key();
        let hashmap_key = self.get_hashmap_key();
        let task_id_result: Option<Vec<String>> =
            self.redis.rpop(&list_key, 1).await.ok();
        if let Some(task_ids_vec) = task_id_result {
            let task_id = task_ids_vec.first().unwrap();
            let task_value: String = self.redis.hget(&hashmap_key, task_id).await?;
            let task: Task<D> = serde_json::from_str(&task_value)?;
            return Ok(Some(task));
        }

        Ok(None)
    }

    async fn task_push(&self, task: &Task<D>) -> Result<(), RedisTaskStorageError> {
        let task_value = serde_json::to_string(task)?;
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
}
