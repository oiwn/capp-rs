use std::{marker::PhantomData, path::Path, sync::Mutex};

use async_trait::async_trait;
use fjall::{
    Config, Keyspace, PartitionCreateOptions, PartitionHandle, PersistMode,
};
use serde::{Serialize, de::DeserializeOwned};
use uuid::Uuid;

use crate::{Task, TaskId, TaskQueue, TaskQueueError, TaskSerializer};

/// Fjall-backed task queue (default backend).
///
/// Layout:
/// - `tasks`: task_id -> serialized task bytes
/// - `queue`: task_id (uuid v7 bytes) -> empty value
/// - `dlq`: task_id -> serialized task bytes
pub struct FjallTaskQueue<D, S>
where
    S: TaskSerializer,
{
    db: Keyspace,
    tasks: PartitionHandle,
    queue: PartitionHandle,
    dlq: PartitionHandle,
    // Serialize push/pop/ack/nack/set to keep ordering simple.
    lock: Mutex<()>,
    _marker: PhantomData<(D, S)>,
}

impl<D, S> FjallTaskQueue<D, S>
where
    S: TaskSerializer,
{
    pub fn open(path: impl AsRef<Path>) -> Result<Self, TaskQueueError> {
        let keyspace = Config::new(path).open()?;
        let tasks =
            keyspace.open_partition("tasks", PartitionCreateOptions::default())?;
        let queue =
            keyspace.open_partition("queue", PartitionCreateOptions::default())?;
        let dlq =
            keyspace.open_partition("dlq", PartitionCreateOptions::default())?;

        Ok(Self {
            db: keyspace,
            tasks,
            queue,
            dlq,
            lock: Mutex::new(()),
            _marker: PhantomData,
        })
    }

    fn task_id_to_bytes(task_id: &TaskId) -> [u8; 16] {
        *task_id.get().as_bytes()
    }

    fn task_id_from_bytes(bytes: &[u8]) -> Result<TaskId, TaskQueueError> {
        let uuid = Uuid::from_slice(bytes).map_err(|e| {
            TaskQueueError::QueueError(format!("Invalid task id bytes: {e}"))
        })?;
        Ok(TaskId::from_uuid(uuid))
    }
}

#[async_trait]
impl<D, S> TaskQueue<D> for FjallTaskQueue<D, S>
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
        let _guard = self.lock.lock().unwrap();
        let task_bytes = S::serialize_task(task)?;

        let task_id_bytes = Self::task_id_to_bytes(&task.task_id);

        self.tasks.insert(task_id_bytes, &task_bytes)?;
        self.queue.insert(task_id_bytes, &[] as &[u8])?;

        // Best-effort sync to disk for durability.
        self.db.persist(PersistMode::SyncAll)?;
        Ok(())
    }

    async fn pop(&self) -> Result<Task<D>, TaskQueueError> {
        let _guard = self.lock.lock().unwrap();

        let Some(entry) = self.queue.first_key_value()? else {
            return Err(TaskQueueError::QueueEmpty);
        };

        let (task_id_bytes, _) = entry;
        let task_id_bytes = task_id_bytes.as_ref().to_vec();
        self.queue.remove(task_id_bytes.clone())?;

        let task_id = Self::task_id_from_bytes(&task_id_bytes)?;
        let task_bytes = self
            .tasks
            .get(task_id_bytes)?
            .ok_or(TaskQueueError::TaskNotFound(task_id))?;

        S::deserialize_task(&task_bytes)
    }

    async fn ack(&self, task_id: &TaskId) -> Result<(), TaskQueueError> {
        let _guard = self.lock.lock().unwrap();
        let task_id_bytes = Self::task_id_to_bytes(task_id);
        self.tasks.remove(task_id_bytes)?;
        Ok(())
    }

    async fn nack(&self, task: &Task<D>) -> Result<(), TaskQueueError> {
        let _guard = self.lock.lock().unwrap();
        let task_id_bytes = Self::task_id_to_bytes(&task.task_id);
        let task_bytes = S::serialize_task(task)?;

        self.dlq.insert(task_id_bytes, &task_bytes)?;
        self.tasks.remove(task_id_bytes)?;
        Ok(())
    }

    async fn set(&self, task: &Task<D>) -> Result<(), TaskQueueError> {
        let _guard = self.lock.lock().unwrap();
        let task_id_bytes = Self::task_id_to_bytes(&task.task_id);
        let task_bytes = S::serialize_task(task)?;
        self.tasks.insert(task_id_bytes, &task_bytes)?;
        Ok(())
    }
}

impl<D, S> std::fmt::Debug for FjallTaskQueue<D, S>
where
    S: TaskSerializer,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FjallTaskQueue").finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::JsonSerializer;
    use serde::{Deserialize, Serialize};
    use tempfile::tempdir;

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    struct TestData {
        value: u32,
    }

    fn make_queue() -> (tempfile::TempDir, FjallTaskQueue<TestData, JsonSerializer>)
    {
        let dir = tempdir().unwrap();
        let queue = FjallTaskQueue::open(dir.path()).unwrap();
        (dir, queue)
    }

    #[tokio::test]
    async fn push_and_pop() -> Result<(), TaskQueueError> {
        let (_dir, queue) = make_queue();
        let task = Task::new(TestData { value: 7 });

        queue.push(&task).await?;
        let popped = queue.pop().await?;
        assert_eq!(popped.payload, task.payload);
        Ok(())
    }

    #[tokio::test]
    async fn queue_empty() -> Result<(), TaskQueueError> {
        let (_dir, queue) = make_queue();
        let res = queue.pop().await;
        assert!(matches!(res, Err(TaskQueueError::QueueEmpty)));
        Ok(())
    }

    #[tokio::test]
    async fn ack_removes_task() -> Result<(), TaskQueueError> {
        let (_dir, queue) = make_queue();
        let task = Task::new(TestData { value: 1 });

        queue.push(&task).await?;
        let popped = queue.pop().await?;
        queue.ack(&popped.task_id).await?;

        let res = queue.pop().await;
        assert!(matches!(res, Err(TaskQueueError::QueueEmpty)));
        Ok(())
    }

    #[tokio::test]
    async fn nack_moves_to_dlq() -> Result<(), TaskQueueError> {
        let (_dir, queue) = make_queue();
        let task = Task::new(TestData { value: 99 });

        queue.push(&task).await?;
        let mut popped = queue.pop().await?;
        popped.set_retry("fail");
        queue.nack(&popped).await?;

        let stored = queue
            .dlq
            .get(
                FjallTaskQueue::<TestData, JsonSerializer>::task_id_to_bytes(
                    &popped.task_id,
                ),
            )?
            .map(|bytes| JsonSerializer::deserialize_task::<TestData>(&bytes));
        assert!(matches!(stored, Some(Ok(_))));
        Ok(())
    }

    #[tokio::test]
    async fn set_updates_task() -> Result<(), TaskQueueError> {
        let (_dir, queue) = make_queue();
        let mut task = Task::new(TestData { value: 3 });
        queue.push(&task).await?;

        task.payload.value = 5;
        queue.set(&task).await?;

        let popped = queue.pop().await?;
        assert_eq!(popped.payload.value, 5);
        Ok(())
    }
}
