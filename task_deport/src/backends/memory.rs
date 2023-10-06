//! In-memory implementation of TaskStorage trait. The storage allows tasks to be
//! pushed to and popped from a queue, and also allows tasks to be set and
//! retrieved by their UUID.
use crate::{Task, TaskStorage};
use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::marker::PhantomData;
use std::sync::Mutex;
use thiserror::Error;
use uuid::Uuid;

#[derive(Error, Debug)]
pub enum InMemoryTaskStorageError {
    #[error("lock error")]
    LockError,

    #[error("key {0} error")]
    KeyError(Uuid),

    #[error(transparent)]
    SerializationError(#[from] serde_json::Error),
}

/// A simple in-memory implementation of the `TaskStorage` trait.
/// The `InMemoryTaskStorage` struct includes a hashmap for storing tasks by
/// their UUIDs, and a list for maintaining the order of the tasks.
pub struct InMemoryTaskStorage<D> {
    pub hashmap: Mutex<HashMap<Uuid, String>>,
    pub list: Mutex<VecDeque<Uuid>>,
    pub dlq: Mutex<HashMap<Uuid, String>>,
    _marker1: PhantomData<D>,
}

impl<D> InMemoryTaskStorage<D> {
    /// Construct a new empty in-memory task storage
    pub fn new() -> Self {
        Self {
            hashmap: Mutex::new(HashMap::new()),
            list: Mutex::new(VecDeque::new()),
            dlq: Mutex::new(HashMap::new()),
            _marker1: PhantomData,
        }
    }
}

impl<D> std::fmt::Debug for InMemoryTaskStorage<D> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Lock the mutexes to access the data.
        let hashmap = self.hashmap.lock().unwrap();
        let list = self.list.lock().unwrap();

        // Use the debug builders to format the output.
        f.debug_struct("InMemoryTaskStorage")
            .field("hashmap", &*hashmap)
            .field("list", &*list)
            .finish()
    }
}

#[async_trait]
impl<D> TaskStorage<D, InMemoryTaskStorageError> for InMemoryTaskStorage<D>
where
    D: Clone + Serialize + DeserializeOwned + Send + Sync + 'static,
{
    async fn task_ack(
        &self,
        task_id: &Uuid,
    ) -> Result<Task<D>, InMemoryTaskStorageError> {
        let mut hashmap = self
            .hashmap
            .lock()
            .map_err(|_| InMemoryTaskStorageError::LockError)?;
        let task_value = hashmap
            .remove(task_id)
            .ok_or(InMemoryTaskStorageError::KeyError(*task_id))?;
        let task = serde_json::from_str(&task_value)?;
        Ok(task)
    }

    async fn task_get(
        &self,
        task_id: &Uuid,
    ) -> Result<Task<D>, InMemoryTaskStorageError> {
        let hashmap = self
            .hashmap
            .lock()
            .map_err(|_| InMemoryTaskStorageError::LockError)?;
        let task_value = hashmap
            .get(&task_id)
            .ok_or(InMemoryTaskStorageError::KeyError(*task_id))?;
        let task: Task<D> = serde_json::from_str(task_value)?;
        Ok(task)
    }

    async fn task_set(
        &self,
        task: &Task<D>,
    ) -> Result<(), InMemoryTaskStorageError> {
        let mut hashmap = self
            .hashmap
            .lock()
            .map_err(|_| InMemoryTaskStorageError::LockError)?;
        let task_value = serde_json::to_string(task)?;
        hashmap.insert(task.task_id, task_value);
        Ok(())
    }

    async fn task_pop(&self) -> Result<Option<Task<D>>, InMemoryTaskStorageError> {
        let mut list = self
            .list
            .lock()
            .map_err(|_| InMemoryTaskStorageError::LockError)?;
        let hashmap = self
            .hashmap
            .lock()
            .map_err(|_| InMemoryTaskStorageError::LockError)?;

        if let Some(task_id) = list.pop_front() {
            let task_value = hashmap.get(&task_id).unwrap();
            let task: Task<D> = serde_json::from_str(task_value)?;
            return Ok(Some(task));
        }
        Ok(None)
    }

    async fn task_push(
        &self,
        task: &Task<D>,
    ) -> Result<(), InMemoryTaskStorageError> {
        let mut list = self
            .list
            .lock()
            .map_err(|_| InMemoryTaskStorageError::LockError)?;
        let mut hashmap = self
            .hashmap
            .lock()
            .map_err(|_| InMemoryTaskStorageError::LockError)?;

        let task_value = serde_json::to_string(task)?;
        hashmap.insert(task.task_id, task_value);
        list.push_back(task.task_id);
        Ok(())
    }

    async fn task_to_dlq(
        &self,
        task: &Task<D>,
    ) -> Result<(), InMemoryTaskStorageError> {
        let mut dlq = self
            .hashmap
            .lock()
            .map_err(|_| InMemoryTaskStorageError::LockError)?;
        let task_value = serde_json::to_string(task)?;
        dlq.insert(task.task_id, task_value);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::assert_eq;

    use super::*;
    use crate::Task;
    use serde::{Deserialize, Serialize};
    use tokio::runtime::Runtime;

    #[derive(Clone, Serialize, Deserialize)]
    struct TaskData {
        value: u32,
    }

    #[test]
    fn storage_init() {
        let storage: InMemoryTaskStorage<TaskData> = InMemoryTaskStorage::new();
        assert_eq!(storage.list.lock().unwrap().len(), 0);
    }

    #[test]
    fn storage_push_pop_ops() {
        let rt = Runtime::new().unwrap();
        let storage: InMemoryTaskStorage<TaskData> = InMemoryTaskStorage::new();

        let task = Task::new(TaskData { value: 42 });
        let push_result = rt.block_on(storage.task_push(&task));
        assert!(push_result.is_ok());
        assert_eq!(storage.list.lock().unwrap().len(), 1);
        assert_eq!(storage.hashmap.lock().unwrap().keys().len(), 1);

        let task = Task::new(TaskData { value: 33 });
        let _ = rt.block_on(storage.task_push(&task));

        let task = rt.block_on(storage.task_pop()).unwrap().unwrap();
        assert_eq!(task.payload.value, 42);
        assert_eq!(storage.list.lock().unwrap().len(), 1);
        assert_eq!(storage.hashmap.lock().unwrap().keys().len(), 2);
    }

    #[test]
    fn storage_get_set_ops() {
        let rt = Runtime::new().unwrap();
        let storage: InMemoryTaskStorage<TaskData> = InMemoryTaskStorage::new();

        let mut task = Task::new(TaskData { value: 42 });
        let _ = rt.block_on(storage.task_push(&task));
        let result = rt.block_on(storage.task_get(&task.task_id)).unwrap();
        assert_eq!(result.payload.value, 42);

        task.payload.value = 11;
        let _ = rt.block_on(storage.task_set(&task));

        let task = rt.block_on(storage.task_pop()).unwrap().unwrap();
        let task_acked = rt.block_on(storage.task_ack(&task.task_id)).unwrap();
        assert_eq!(task_acked.payload.value, 11);
        assert_eq!(task_acked.task_id, task.task_id);

        assert_eq!(storage.hashmap.lock().unwrap().keys().len(), 0);
        assert_eq!(storage.list.lock().unwrap().len(), 0);
    }

    #[test]
    fn storage_ack() {
        let rt = Runtime::new().unwrap();
        let storage: InMemoryTaskStorage<TaskData> = InMemoryTaskStorage::new();

        let task = Task::new(TaskData { value: 42 });
        let _ = rt.block_on(storage.task_push(&task));
        let task = Task::new(TaskData { value: 33 });
        let _ = rt.block_on(storage.task_push(&task));

        let task = rt.block_on(storage.task_pop()).unwrap().unwrap();
        let task = rt.block_on(storage.task_ack(&task.task_id)).unwrap();
        assert_eq!(task.payload.value, 42);

        assert_eq!(storage.hashmap.lock().unwrap().keys().len(), 1);
        assert_eq!(storage.list.lock().unwrap().len(), 1);
    }
}
