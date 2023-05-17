//! This module provides a trait for interacting with task storage, and an
//! in-memory implementation of this trait. The storage allows tasks to be
//! pushed to and popped from a queue, and also allows tasks to be set and
//! retrieved by their UUID.

use super::task::Task;
use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::marker::PhantomData;
use std::sync::Mutex;
use uuid::Uuid;

/// A trait that describes the necessary methods for task storage. This includes
/// methods for acknowledging a task, getting a task by its UUID, setting a task,
/// popping a task from the queue, and pushing a task into the queue.
#[async_trait]
pub trait TaskStorage<D, E>
where
    D: Serialize + DeserializeOwned + Send + Sync + 'static,
    E: std::error::Error + Send + Sync + 'static,
{
    async fn task_ack(&self, task_id: &Uuid) -> Result<Task<D>, E>;
    async fn task_get(&self, task_id: &Uuid) -> Result<Option<Task<D>>, E>;
    async fn task_set(&self, task: &Task<D>) -> Result<(), E>;
    async fn task_pop(&self) -> Result<Option<Task<D>>, E>;
    async fn task_push(&self, task: &Task<D>) -> Result<(), E>;
}

/// A simple in-memory implementation of the `TaskStorage` trait.
/// The `InMemoryTaskStorage` struct includes a hashmap for storing tasks by
/// their UUIDs, and a list for maintaining the order of the tasks.
pub struct InMemoryTaskStorage<D, E> {
    pub hashmap: Mutex<HashMap<Uuid, String>>,
    pub list: Mutex<VecDeque<Uuid>>,
    _marker1: PhantomData<D>,
    _marker2: PhantomData<E>,
}

impl<D, E> InMemoryTaskStorage<D, E> {
    /// Construct a new empty in-memory task storage
    pub fn new() -> Self {
        Self {
            hashmap: Mutex::new(HashMap::new()),
            list: Mutex::new(VecDeque::new()),
            _marker1: PhantomData,
            _marker2: PhantomData,
        }
    }
}

impl<D, E> std::fmt::Debug for InMemoryTaskStorage<D, E> {
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
impl<D, E> TaskStorage<D, E> for InMemoryTaskStorage<D, E>
where
    D: Serialize + DeserializeOwned + Send + Sync + 'static,
    E: Serialize + DeserializeOwned + std::error::Error + Send + Sync + 'static,
{
    async fn task_ack(&self, task_id: &Uuid) -> Result<Task<D>, E> {
        let mut hashmap = self.hashmap.lock().unwrap();
        let task_data = hashmap.remove(task_id).unwrap();
        let task = serde_json::from_str(&task_data).unwrap();
        Ok(task)
    }

    async fn task_get(&self, task_id: &Uuid) -> Result<Option<Task<D>>, E> {
        let hashmap = self.hashmap.lock().unwrap();
        let data = hashmap.get(&task_id).unwrap();
        let task_data: Task<D> = serde_json::from_str(data).unwrap();
        Ok(Some(task_data))
    }

    async fn task_set(&self, task: &Task<D>) -> Result<(), E> {
        let mut hashmap = self.hashmap.lock().unwrap();
        let task_data = serde_json::to_string(task).unwrap();
        hashmap.insert(task.task_id, task_data);
        Ok(())
    }

    async fn task_pop(&self) -> Result<Option<Task<D>>, E> {
        let mut list = self.list.lock().unwrap();
        let hashmap = self.hashmap.lock().unwrap();

        if let Some(task_id) = list.pop_front() {
            let task_data = hashmap.get(&task_id).unwrap();
            let task: Task<D> = serde_json::from_str(task_data).unwrap();
            return Ok(Some(task));
        }
        Ok(None)
    }

    async fn task_push(&self, task: &Task<D>) -> Result<(), E> {
        let mut list = self.list.lock().unwrap();
        let mut hashmap = self.hashmap.lock().unwrap();

        let task_str = serde_json::to_string(task).unwrap();
        hashmap.insert(task.task_id, task_str);
        list.push_back(task.task_id);
        Ok(())
    }
}
