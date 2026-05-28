use std::{collections::HashSet, marker::PhantomData, path::Path, sync::Mutex};

use async_trait::async_trait;
use fjall::{Database, Keyspace, KeyspaceCreateOptions, PersistMode};
use serde::{Serialize, de::DeserializeOwned};
use tracing::warn;
use uuid::Uuid;

use crate::{HasTagKey, Task, TaskId, TaskQueue, TaskQueueError, TaskSerializer};

/// Round-robin Fjall task queue.
///
/// Pops rotate across tags so one busy tag can't starve the others. FIFO
/// within a tag is preserved by sorting on the task's UUID v7 suffix.
///
/// Layout:
/// - `tasks`: task_id (uuid bytes) -> serialized `Task<D>`
/// - `queue`: `<u16 BE tag-len><tag utf8><uuid v7 bytes>` -> empty
/// - `inflight`: task_id -> `<u16 BE tag-len><tag utf8>` (so recover knows the bucket)
/// - `dlq`: task_id -> serialized `Task<D>`
pub struct FjallRoundRobinTaskQueue<D, S>
where
    D: HasTagKey,
    S: TaskSerializer,
{
    db: Database,
    tasks: Keyspace,
    queue: Keyspace,
    inflight: Keyspace,
    dlq: Keyspace,
    state: Mutex<RotationState>,
    _marker: PhantomData<(D, S)>,
}

#[derive(Debug, Default)]
struct RotationState {
    active: Vec<String>,
    cursor: usize,
    present: HashSet<String>,
}

impl RotationState {
    fn ensure(&mut self, tag: &str) {
        if self.present.insert(tag.to_string()) {
            self.active.push(tag.to_string());
        }
    }

    fn evict_at(&mut self, idx: usize) {
        let tag = self.active.remove(idx);
        self.present.remove(&tag);
        if self.active.is_empty() {
            self.cursor = 0;
        } else if idx < self.cursor {
            self.cursor -= 1;
        } else {
            self.cursor %= self.active.len();
        }
    }
}

impl<D, S> FjallRoundRobinTaskQueue<D, S>
where
    D: HasTagKey,
    S: TaskSerializer,
{
    pub fn open(path: impl AsRef<Path>) -> Result<Self, TaskQueueError> {
        let db = Database::builder(path).open()?;
        let tasks = db.keyspace("tasks", KeyspaceCreateOptions::default)?;
        let queue = db.keyspace("queue", KeyspaceCreateOptions::default)?;
        let inflight = db.keyspace("inflight", KeyspaceCreateOptions::default)?;
        let dlq = db.keyspace("dlq", KeyspaceCreateOptions::default)?;

        let state = Self::rebuild_state(&queue)?;

        Ok(Self {
            db,
            tasks,
            queue,
            inflight,
            dlq,
            state: Mutex::new(state),
            _marker: PhantomData,
        })
    }

    fn rebuild_state(queue: &Keyspace) -> Result<RotationState, TaskQueueError> {
        let mut present: HashSet<String> = HashSet::new();
        let mut active: Vec<String> = Vec::new();
        for item in queue.iter() {
            let (key, _) = item.into_inner()?;
            let tag = decode_tag_from_key(&key)?.to_string();
            if present.insert(tag.clone()) {
                active.push(tag);
            }
        }
        // Deterministic order across restarts.
        active.sort();
        Ok(RotationState {
            active,
            cursor: 0,
            present,
        })
    }

    fn task_id_to_bytes(task_id: &TaskId) -> [u8; 16] {
        *task_id.get().as_bytes()
    }
}

#[async_trait]
impl<D, S> TaskQueue<D> for FjallRoundRobinTaskQueue<D, S>
where
    D: HasTagKey
        + std::fmt::Debug
        + Clone
        + Serialize
        + DeserializeOwned
        + Send
        + Sync
        + 'static,
    S: TaskSerializer + Send + Sync,
{
    async fn push(&self, task: &Task<D>) -> Result<(), TaskQueueError> {
        let tag = task.payload.get_tag_value().to_string();
        let task_id_bytes = Self::task_id_to_bytes(&task.task_id);
        let task_bytes = S::serialize_task(task)?;
        let queue_key = encode_queue_key(&tag, &task_id_bytes);

        let mut state = self.state.lock().unwrap();
        self.tasks.insert(task_id_bytes, &task_bytes)?;
        self.queue.insert(queue_key, &[] as &[u8])?;
        self.inflight.remove(task_id_bytes)?;
        self.db.persist(PersistMode::SyncAll)?;

        state.ensure(&tag);
        Ok(())
    }

    async fn pop(&self) -> Result<Task<D>, TaskQueueError> {
        let mut state = self.state.lock().unwrap();

        loop {
            if state.active.is_empty() {
                return Err(TaskQueueError::QueueEmpty);
            }
            let idx = state.cursor;
            let tag = state.active[idx].clone();
            let prefix = encode_tag_prefix(&tag);

            let Some(guard) = self.queue.prefix(&prefix).next() else {
                warn!(
                    tag = %tag,
                    "active tag has no queued key; evicting from rotation"
                );
                state.evict_at(idx);
                continue;
            };
            let (key, _) = guard.into_inner()?;

            let task_id = extract_task_id_from_key(&key)?;
            let task_id_bytes = Self::task_id_to_bytes(&task_id);

            self.queue.remove(key.as_ref())?;
            self.inflight.insert(task_id_bytes, encode_tag_blob(&tag))?;
            self.db.persist(PersistMode::SyncAll)?;

            let task_bytes = self
                .tasks
                .get(task_id_bytes)?
                .ok_or(TaskQueueError::TaskNotFound(task_id))?;
            let task = S::deserialize_task::<D>(&task_bytes)?;

            let still_pending = self.queue.prefix(&prefix).next().is_some();
            if still_pending {
                state.cursor = (idx + 1) % state.active.len();
            } else {
                state.evict_at(idx);
            }
            return Ok(task);
        }
    }

    async fn ack(&self, task_id: &TaskId) -> Result<(), TaskQueueError> {
        let _guard = self.state.lock().unwrap();
        let task_id_bytes = Self::task_id_to_bytes(task_id);
        self.tasks.remove(task_id_bytes)?;
        self.inflight.remove(task_id_bytes)?;
        self.db.persist(PersistMode::SyncAll)?;
        Ok(())
    }

    async fn nack(&self, task: &Task<D>) -> Result<(), TaskQueueError> {
        let _guard = self.state.lock().unwrap();
        let task_id_bytes = Self::task_id_to_bytes(&task.task_id);
        let task_bytes = S::serialize_task(task)?;

        self.dlq.insert(task_id_bytes, &task_bytes)?;
        self.tasks.remove(task_id_bytes)?;
        self.inflight.remove(task_id_bytes)?;
        self.db.persist(PersistMode::SyncAll)?;
        Ok(())
    }

    async fn set(&self, task: &Task<D>) -> Result<(), TaskQueueError> {
        let _guard = self.state.lock().unwrap();
        let task_id_bytes = Self::task_id_to_bytes(&task.task_id);
        let task_bytes = S::serialize_task(task)?;
        self.tasks.insert(task_id_bytes, &task_bytes)?;
        Ok(())
    }

    async fn recover_inflight(&self) -> Result<u64, TaskQueueError> {
        let mut state = self.state.lock().unwrap();

        let mut entries: Vec<([u8; 16], String)> = Vec::new();
        for item in self.inflight.iter() {
            let (key, value) = item.into_inner()?;
            if key.len() != 16 {
                return Err(TaskQueueError::QueueError(
                    "inflight key is not a 16-byte task id".into(),
                ));
            }
            let mut id = [0u8; 16];
            id.copy_from_slice(&key);
            let tag = decode_tag_blob(&value)?.to_string();
            entries.push((id, tag));
        }

        let mut recovered = 0u64;
        for (task_id_bytes, tag) in entries {
            // Recover to the original FIFO position by reusing the task's
            // uuid v7 suffix (oldest within its tag's sub-queue).
            self.queue
                .insert(encode_queue_key(&tag, &task_id_bytes), &[] as &[u8])?;
            self.inflight.remove(&task_id_bytes[..])?;
            state.ensure(&tag);
            recovered += 1;
        }

        if recovered > 0 {
            self.db.persist(PersistMode::SyncAll)?;
        }
        Ok(recovered)
    }
}

impl<D, S> std::fmt::Debug for FjallRoundRobinTaskQueue<D, S>
where
    D: HasTagKey,
    S: TaskSerializer,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FjallRoundRobinTaskQueue").finish()
    }
}

fn encode_queue_key(tag: &str, task_id_bytes: &[u8; 16]) -> Vec<u8> {
    let tag_bytes = tag.as_bytes();
    let len =
        u16::try_from(tag_bytes.len()).expect("tag too long (max 65535 bytes)");
    let mut key = Vec::with_capacity(2 + tag_bytes.len() + 16);
    key.extend_from_slice(&len.to_be_bytes());
    key.extend_from_slice(tag_bytes);
    key.extend_from_slice(task_id_bytes);
    key
}

fn encode_tag_prefix(tag: &str) -> Vec<u8> {
    let tag_bytes = tag.as_bytes();
    let len =
        u16::try_from(tag_bytes.len()).expect("tag too long (max 65535 bytes)");
    let mut prefix = Vec::with_capacity(2 + tag_bytes.len());
    prefix.extend_from_slice(&len.to_be_bytes());
    prefix.extend_from_slice(tag_bytes);
    prefix
}

fn encode_tag_blob(tag: &str) -> Vec<u8> {
    encode_tag_prefix(tag)
}

fn decode_tag_from_key(key: &[u8]) -> Result<&str, TaskQueueError> {
    if key.len() < 2 {
        return Err(TaskQueueError::QueueError("queue key too short".into()));
    }
    let len = u16::from_be_bytes([key[0], key[1]]) as usize;
    if key.len() < 2 + len + 16 {
        return Err(TaskQueueError::QueueError(
            "queue key shorter than declared tag length".into(),
        ));
    }
    std::str::from_utf8(&key[2..2 + len])
        .map_err(|e| TaskQueueError::QueueError(format!("invalid tag utf8: {e}")))
}

fn decode_tag_blob(bytes: &[u8]) -> Result<&str, TaskQueueError> {
    if bytes.len() < 2 {
        return Err(TaskQueueError::QueueError("tag blob too short".into()));
    }
    let len = u16::from_be_bytes([bytes[0], bytes[1]]) as usize;
    if bytes.len() != 2 + len {
        return Err(TaskQueueError::QueueError(
            "tag blob length mismatch".into(),
        ));
    }
    std::str::from_utf8(&bytes[2..])
        .map_err(|e| TaskQueueError::QueueError(format!("invalid tag utf8: {e}")))
}

fn extract_task_id_from_key(key: &[u8]) -> Result<TaskId, TaskQueueError> {
    if key.len() < 18 {
        return Err(TaskQueueError::QueueError(
            "queue key missing task id suffix".into(),
        ));
    }
    let suffix = &key[key.len() - 16..];
    let uuid = Uuid::from_slice(suffix)
        .map_err(|e| TaskQueueError::QueueError(format!("invalid uuid: {e}")))?;
    Ok(TaskId::from_uuid(uuid))
}
