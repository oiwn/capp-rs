use crate::{HasTagKey, Task, TaskId, TaskQueue, TaskQueueError, TaskSerializer};
use async_trait::async_trait;
use rustis::client::{BatchPreparedCommand, Client, Pipeline};
use rustis::commands::{
    GenericCommands, HashCommands, ListCommands, SortedSetCommands, ZAddOptions,
    ZRangeOptions,
};
use serde::{Serialize, de::DeserializeOwned};
use std::collections::HashSet;
use std::marker::PhantomData;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct RedisRoundRobinTaskQueue<D, S>
where
    S: TaskSerializer,
{
    pub client: Client,
    pub key_prefix: String,
    pub tags: Arc<HashSet<String>>,
    _marker: PhantomData<(D, S)>,
}

impl<D, S> RedisRoundRobinTaskQueue<D, S>
where
    D: Send + Sync + 'static + HasTagKey,
    S: TaskSerializer + Send + Sync,
{
    pub async fn new(
        client: Client,
        key_prefix: &str,
        tags: HashSet<String>,
    ) -> Result<Self, TaskQueueError> {
        let queue = Self {
            client,
            key_prefix: key_prefix.to_string(),
            tags: Arc::new(tags),
            _marker: PhantomData,
        };

        // Initialize schedule sorted set with current timestamp for all tags
        let timestamp = queue.current_timestamp()?;
        let mut pipeline = queue.client.create_pipeline();

        for tag in queue.tags.iter() {
            pipeline
                .zadd(
                    queue.get_schedule_key(),
                    vec![(timestamp as f64, tag.clone())],
                    ZAddOptions::default(),
                )
                .forget();
        }

        queue.execute_pipeline(pipeline).await?;
        Ok(queue)
    }

    // Key generation methods
    pub fn get_schedule_key(&self) -> String {
        format!("{}:schedule", self.key_prefix)
    }

    pub fn get_hashmap_key(&self) -> String {
        format!("{}:tasks:hm", self.key_prefix)
    }

    pub fn get_list_key(&self, tag: &str) -> String {
        format!("{}:{}:ls", self.key_prefix, tag)
    }

    pub fn get_dlq_key(&self) -> String {
        format!("{}:dlq", self.key_prefix)
    }

    // Helper methods
    fn current_timestamp(&self) -> Result<u64, TaskQueueError> {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .map_err(|e| TaskQueueError::QueueError(e.to_string()))
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

    // Get the tag with oldest timestamp from schedule
    async fn get_next_tag(&self) -> Result<Option<String>, TaskQueueError> {
        let results: Vec<(String, f64)> = self
            .client
            .zrange_with_scores(
                self.get_schedule_key(),
                0,
                0,
                ZRangeOptions::default(),
            )
            .await?;

        Ok(results.first().map(|(tag, _score)| tag.clone()))
    }

    async fn update_tag_timestamp(&self, tag: &str) -> Result<(), TaskQueueError> {
        let timestamp = self.current_timestamp()?;
        // Add a small increment to ensure proper ordering
        let score = timestamp as f64 + 0.001;
        self.client
            .zadd(
                self.get_schedule_key(),
                vec![(score, tag)],
                ZAddOptions::default(),
            )
            .await?;
        Ok(())
    }

    pub async fn purge(&self) -> Result<(), TaskQueueError> {
        let mut keys = vec![
            self.get_schedule_key(),
            self.get_hashmap_key(),
            self.get_dlq_key(),
        ];

        // Add list keys for all tags
        for tag in self.tags.iter() {
            keys.push(self.get_list_key(tag));
        }

        self.client
            .del(keys)
            .await
            .map_err(|e| TaskQueueError::QueueError(e.to_string()))?;
        Ok(())
    }
}

#[async_trait]
impl<D, S> TaskQueue<D> for RedisRoundRobinTaskQueue<D, S>
where
    D: std::fmt::Debug
        + Clone
        + Serialize
        + DeserializeOwned
        + Send
        + Sync
        + 'static
        + HasTagKey,
    S: TaskSerializer + Send + Sync,
{
    async fn push(&self, task: &Task<D>) -> Result<(), TaskQueueError> {
        let task_bytes = S::serialize_task(task)?;
        let task_str = String::from_utf8(task_bytes)
            .map_err(|e| TaskQueueError::Serialization(e.to_string()))?;

        let tag = task.payload.get_tag_value().to_string();
        let list_key = self.get_list_key(&tag);
        let hashmap_key = self.get_hashmap_key();
        let schedule_key = self.get_schedule_key();

        let mut pipeline = self.client.create_pipeline();

        // Add task to list and hashmap
        pipeline
            .lpush(&list_key, &task.task_id.to_string())
            .forget();
        pipeline
            .hset(&hashmap_key, [(&task.task_id.to_string(), &task_str)])
            .forget();

        // Ensure tag exists in schedule with current timestamp if it doesn't exist
        let timestamp = self.current_timestamp()?;
        pipeline
            .zadd(
                schedule_key,
                vec![(timestamp as f64, tag)],
                ZAddOptions::default()
                    .condition(rustis::commands::ZAddCondition::NX),
            )
            .forget();

        self.execute_pipeline(pipeline).await
    }

    async fn pop(&self) -> Result<Task<D>, TaskQueueError> {
        // Keep trying until we find a task or exhaust all tags
        loop {
            let tag = self
                .get_next_tag()
                .await?
                .ok_or(TaskQueueError::QueueEmpty)?;

            let list_key = self.get_list_key(&tag);
            let hashmap_key = self.get_hashmap_key();

            // Try to get task from the selected tag's list
            let task_ids: Vec<String> = self
                .client
                .rpop(&list_key, 1)
                .await
                .map_err(|e| TaskQueueError::QueueError(e.to_string()))?;

            if let Some(task_id) = task_ids.first() {
                // Get task data from hash
                let task_str: String = self
                    .client
                    .hget(&hashmap_key, task_id)
                    .await
                    .map_err(|e| TaskQueueError::QueueError(e.to_string()))?;

                let task = S::deserialize_task(task_str.as_bytes())?;

                // Update tag's timestamp in schedule
                self.update_tag_timestamp(&tag).await?;

                return Ok(task);
            }

            // No tasks in this tag's list, remove it from schedule and continue
            self.client.zrem(self.get_schedule_key(), &tag).await?;

            // Check if we still have any tags in schedule
            let count = self.client.zcard(self.get_schedule_key()).await?;
            if count == 0 {
                return Err(TaskQueueError::QueueEmpty);
            }
        }
    }

    async fn ack(&self, task_id: &TaskId) -> Result<(), TaskQueueError> {
        self.client
            .hdel(self.get_hashmap_key(), &task_id.to_string())
            .await
            .map_err(|e| TaskQueueError::QueueError(e.to_string()))?;
        Ok(())
    }

    async fn nack(&self, task: &Task<D>) -> Result<(), TaskQueueError> {
        let task_bytes = S::serialize_task(task)?;
        let task_str = String::from_utf8(task_bytes)
            .map_err(|e| TaskQueueError::Serialization(e.to_string()))?;

        let mut pipeline = self.client.create_pipeline();
        pipeline.rpush(self.get_dlq_key(), &task_str).forget();
        pipeline
            .hdel(self.get_hashmap_key(), &task.task_id.to_string())
            .forget();

        self.execute_pipeline(pipeline).await
    }

    async fn set(&self, task: &Task<D>) -> Result<(), TaskQueueError> {
        let task_bytes = S::serialize_task(task)?;
        let task_str = String::from_utf8(task_bytes)
            .map_err(|e| TaskQueueError::Serialization(e.to_string()))?;

        self.client
            .hset(
                self.get_hashmap_key(),
                [(&task.task_id.to_string(), &task_str)],
            )
            .await
            .map_err(|e| TaskQueueError::QueueError(e.to_string()))?;
        Ok(())
    }
}

impl<D, S> std::fmt::Debug for RedisRoundRobinTaskQueue<D, S>
where
    S: TaskSerializer,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RedisRoundRobinTaskQueue")
            .field("key_prefix", &self.key_prefix)
            .field("tags", &self.tags)
            .finish()
    }
}
