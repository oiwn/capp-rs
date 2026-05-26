use std::{marker::PhantomData, path::Path, sync::Mutex};

use async_trait::async_trait;
use fjall::{Database, Keyspace, KeyspaceCreateOptions, PersistMode};
use serde::{Serialize, de::DeserializeOwned};
use uuid::Uuid;

use crate::{Task, TaskId, TaskQueue, TaskQueueError, TaskSerializer};

/// Fjall-backed task queue (default backend).
///
/// Layout:
/// - `tasks`: task_id -> serialized task bytes
/// - `queue`: task_id (uuid v7 bytes) -> empty value (work to do)
/// - `inflight`: task_id -> empty value (leased to a worker, not yet acked)
/// - `dlq`: task_id -> serialized task bytes
pub struct FjallTaskQueue<D, S>
where
    S: TaskSerializer,
{
    db: Database,
    tasks: Keyspace,
    queue: Keyspace,
    inflight: Keyspace,
    dlq: Keyspace,
    // Serialize push/pop/ack/nack/set to keep ordering simple.
    lock: Mutex<()>,
    _marker: PhantomData<(D, S)>,
}

impl<D, S> FjallTaskQueue<D, S>
where
    S: TaskSerializer,
{
    pub fn open(path: impl AsRef<Path>) -> Result<Self, TaskQueueError> {
        let db = Database::builder(path).open()?;
        let tasks = db.keyspace("tasks", KeyspaceCreateOptions::default)?;
        let queue = db.keyspace("queue", KeyspaceCreateOptions::default)?;
        let inflight = db.keyspace("inflight", KeyspaceCreateOptions::default)?;
        let dlq = db.keyspace("dlq", KeyspaceCreateOptions::default)?;

        Ok(Self {
            db,
            tasks,
            queue,
            inflight,
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
        // Covers the retry path: a task popped earlier is being requeued.
        self.inflight.remove(task_id_bytes)?;

        // Best-effort sync to disk for durability.
        self.db.persist(PersistMode::SyncAll)?;
        Ok(())
    }

    async fn pop(&self) -> Result<Task<D>, TaskQueueError> {
        let _guard = self.lock.lock().unwrap();

        let Some(entry) = self.queue.first_key_value() else {
            return Err(TaskQueueError::QueueEmpty);
        };

        let (task_id_bytes, _) = entry.into_inner()?;
        let task_id_bytes = task_id_bytes.to_vec();
        self.queue.remove(task_id_bytes.clone())?;
        self.inflight.insert(task_id_bytes.clone(), &[] as &[u8])?;
        self.db.persist(PersistMode::SyncAll)?;

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
        self.inflight.remove(task_id_bytes)?;
        self.db.persist(PersistMode::SyncAll)?;
        Ok(())
    }

    async fn nack(&self, task: &Task<D>) -> Result<(), TaskQueueError> {
        let _guard = self.lock.lock().unwrap();
        let task_id_bytes = Self::task_id_to_bytes(&task.task_id);
        let task_bytes = S::serialize_task(task)?;

        self.dlq.insert(task_id_bytes, &task_bytes)?;
        self.tasks.remove(task_id_bytes)?;
        self.inflight.remove(task_id_bytes)?;
        self.db.persist(PersistMode::SyncAll)?;
        Ok(())
    }

    async fn set(&self, task: &Task<D>) -> Result<(), TaskQueueError> {
        let _guard = self.lock.lock().unwrap();
        let task_id_bytes = Self::task_id_to_bytes(&task.task_id);
        let task_bytes = S::serialize_task(task)?;
        self.tasks.insert(task_id_bytes, &task_bytes)?;
        Ok(())
    }

    async fn recover_inflight(&self) -> Result<u64, TaskQueueError> {
        let _guard = self.lock.lock().unwrap();

        let mut recovered = 0u64;
        let mut keys = Vec::new();
        for item in self.inflight.iter() {
            let (key, _) = item.into_inner()?;
            keys.push(key.to_vec());
        }

        for key in keys {
            self.queue.insert(key.clone(), &[] as &[u8])?;
            self.inflight.remove(key)?;
            recovered += 1;
        }

        if recovered > 0 {
            self.db.persist(PersistMode::SyncAll)?;
        }
        Ok(recovered)
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

    fn inflight_contains<S: TaskSerializer>(
        queue: &FjallTaskQueue<TestData, S>,
        task_id: &TaskId,
    ) -> bool {
        let key = FjallTaskQueue::<TestData, S>::task_id_to_bytes(task_id);
        queue
            .inflight
            .get(key)
            .map(|v| v.is_some())
            .unwrap_or(false)
    }

    #[tokio::test]
    async fn pop_moves_to_inflight() -> Result<(), TaskQueueError> {
        let (_dir, queue) = make_queue();
        let task = Task::new(TestData { value: 11 });

        queue.push(&task).await?;
        let popped = queue.pop().await?;

        assert!(inflight_contains(&queue, &popped.task_id));
        Ok(())
    }

    #[tokio::test]
    async fn ack_clears_inflight() -> Result<(), TaskQueueError> {
        let (_dir, queue) = make_queue();
        let task = Task::new(TestData { value: 12 });

        queue.push(&task).await?;
        let popped = queue.pop().await?;
        queue.ack(&popped.task_id).await?;

        assert!(!inflight_contains(&queue, &popped.task_id));
        Ok(())
    }

    #[tokio::test]
    async fn nack_clears_inflight() -> Result<(), TaskQueueError> {
        let (_dir, queue) = make_queue();
        let task = Task::new(TestData { value: 13 });

        queue.push(&task).await?;
        let mut popped = queue.pop().await?;
        popped.set_retry("fail");
        queue.nack(&popped).await?;

        assert!(!inflight_contains(&queue, &popped.task_id));
        Ok(())
    }

    #[tokio::test]
    async fn push_retry_clears_inflight() -> Result<(), TaskQueueError> {
        let (_dir, queue) = make_queue();
        let mut task = Task::new(TestData { value: 14 });

        queue.push(&task).await?;
        let popped = queue.pop().await?;
        assert!(inflight_contains(&queue, &popped.task_id));

        task.set_retry("transient");
        queue.push(&task).await?;

        assert!(!inflight_contains(&queue, &task.task_id));
        Ok(())
    }

    #[tokio::test]
    async fn recover_inflight_moves_back_to_queue() -> Result<(), TaskQueueError> {
        let dir = tempdir().unwrap();
        let task_id = {
            let queue: FjallTaskQueue<TestData, JsonSerializer> =
                FjallTaskQueue::open(dir.path())?;
            let task = Task::new(TestData { value: 99 });
            queue.push(&task).await?;
            let popped = queue.pop().await?;
            assert!(inflight_contains(&queue, &popped.task_id));
            popped.task_id
        };

        // Reopen — simulates fresh process startup after crash.
        let queue: FjallTaskQueue<TestData, JsonSerializer> =
            FjallTaskQueue::open(dir.path())?;
        let recovered = queue.recover_inflight().await?;
        assert_eq!(recovered, 1);

        let re_popped = queue.pop().await?;
        assert_eq!(re_popped.task_id, task_id);
        Ok(())
    }
}
