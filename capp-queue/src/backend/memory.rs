//! In-memory implementation of TaskStorage trait. The storage allows tasks to be
//! pushed to and popped from a queue, and also allows tasks to be set and
//! retrieved by their UUID.
use crate::queue::{TaskQueue, TaskQueueError};
use crate::task::{Task, TaskId};
use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::marker::PhantomData;
use std::sync::Mutex;

pub struct InMemoryTaskQueue<D> {
    pub hashmap: Mutex<HashMap<TaskId, String>>,
    pub list: Mutex<VecDeque<TaskId>>,
    pub dlq: Mutex<HashMap<TaskId, String>>,
    _marker: PhantomData<D>,
}

impl<D> InMemoryTaskQueue<D> {
    pub fn new() -> Self {
        Self {
            hashmap: Mutex::new(HashMap::new()),
            list: Mutex::new(VecDeque::new()),
            dlq: Mutex::new(HashMap::new()),
            _marker: PhantomData,
        }
    }
}

impl<D> Default for InMemoryTaskQueue<D> {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl<D> TaskQueue<D> for InMemoryTaskQueue<D>
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
        let mut list = self
            .list
            .lock()
            .map_err(|e| TaskQueueError::QueueError(e.to_string()))?;
        let mut hashmap = self
            .hashmap
            .lock()
            .map_err(|e| TaskQueueError::QueueError(e.to_string()))?;

        let task_value = serde_json::to_string(task)
            .map_err(|e| TaskQueueError::SerdeError(e.to_string()))?;
        hashmap.insert(task.task_id, task_value);
        list.push_back(task.task_id);
        Ok(())
    }

    async fn pop(&self) -> Result<Task<D>, TaskQueueError> {
        let mut list = self
            .list
            .lock()
            .map_err(|e| TaskQueueError::QueueError(e.to_string()))?;
        let hashmap = self
            .hashmap
            .lock()
            .map_err(|e| TaskQueueError::QueueError(e.to_string()))?;

        if let Some(task_id) = list.pop_front() {
            let task_value = hashmap
                .get(&task_id)
                .ok_or(TaskQueueError::TaskNotFound(task_id))?;
            let task: Task<D> = serde_json::from_str(task_value)
                .map_err(|e| TaskQueueError::SerdeError(e.to_string()))?;
            Ok(task)
        } else {
            Err(TaskQueueError::QueueEmpty)
        }
    }

    async fn ack(&self, task_id: &TaskId) -> Result<(), TaskQueueError> {
        let mut hashmap = self
            .hashmap
            .lock()
            .map_err(|e| TaskQueueError::QueueError(e.to_string()))?;
        hashmap
            .remove(task_id)
            .ok_or(TaskQueueError::TaskNotFound(*task_id))?;
        Ok(())
    }

    async fn nack(&self, task: &Task<D>) -> Result<(), TaskQueueError> {
        let mut dlq = self
            .dlq
            .lock()
            .map_err(|e| TaskQueueError::QueueError(e.to_string()))?;
        let task_value = serde_json::to_string(task)
            .map_err(|e| TaskQueueError::SerdeError(e.to_string()))?;
        dlq.insert(task.task_id, task_value);

        let mut hashmap = self
            .hashmap
            .lock()
            .map_err(|e| TaskQueueError::QueueError(e.to_string()))?;
        hashmap
            .remove(&task.task_id)
            .ok_or(TaskQueueError::TaskNotFound(task.task_id))?;

        Ok(())
    }

    async fn set(&self, task: &Task<D>) -> Result<(), TaskQueueError> {
        let mut hashmap = self
            .hashmap
            .lock()
            .map_err(|e| TaskQueueError::QueueError(e.to_string()))?;
        let task_value = serde_json::to_string(task)
            .map_err(|e| TaskQueueError::SerdeError(e.to_string()))?;
        hashmap.insert(task.task_id, task_value);
        Ok(())
    }
}

impl<D> std::fmt::Debug for InMemoryTaskQueue<D> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let hashmap = self.hashmap.lock().unwrap();
        let list = self.list.lock().unwrap();
        let dlq = self.dlq.lock().unwrap();

        f.debug_struct("InMemoryTaskQueue")
            .field("hashmap", &*hashmap)
            .field("list", &*list)
            .field("dlq", &*dlq)
            .finish()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    struct TestData {
        value: u32,
    }

    #[tokio::test]
    async fn test_push_and_pop() {
        let queue = InMemoryTaskQueue::<TestData>::new();
        let task = Task::new(TestData { value: 42 });

        queue.push(&task).await.unwrap();
        let popped_task = queue.pop().await.unwrap();

        assert_eq!(popped_task.payload, TestData { value: 42 });
    }

    #[tokio::test]
    async fn test_queue_empty() {
        let queue = InMemoryTaskQueue::<TestData>::new();

        match queue.pop().await {
            Err(TaskQueueError::QueueEmpty) => (),
            _ => panic!("Expected QueueEmpty error"),
        }
    }

    #[tokio::test]
    async fn test_ack() {
        let queue = InMemoryTaskQueue::<TestData>::new();
        let task = Task::new(TestData { value: 42 });

        queue.push(&task).await.unwrap();
        let popped_task = queue.pop().await.unwrap();
        queue.ack(&popped_task.task_id).await.unwrap();

        // The queue should be empty after ack
        assert!(matches!(queue.pop().await, Err(TaskQueueError::QueueEmpty)));
    }

    #[tokio::test]
    async fn test_nack() {
        let queue = InMemoryTaskQueue::<TestData>::new();
        let task = Task::new(TestData { value: 42 });

        queue.push(&task).await.unwrap();
        let popped_task = queue.pop().await.unwrap();
        queue.nack(&popped_task).await.unwrap();

        // The queue should be empty after nack
        assert!(matches!(queue.pop().await, Err(TaskQueueError::QueueEmpty)));

        // The task should be in the DLQ
        let dlq = queue.dlq.lock().unwrap();
        assert_eq!(dlq.len(), 1);
    }

    #[tokio::test]
    async fn test_set() {
        let queue = InMemoryTaskQueue::<TestData>::new();
        let mut task = Task::new(TestData { value: 42 });

        queue.push(&task).await.unwrap();

        // Modify the task
        task.payload.value = 43;
        queue.set(&task).await.unwrap();

        let updated_task = queue.pop().await.unwrap();
        assert_eq!(updated_task.payload, TestData { value: 43 });
    }

    #[tokio::test]
    async fn test_multiple_tasks() {
        let queue = InMemoryTaskQueue::<TestData>::new();
        let tasks = vec![
            Task::new(TestData { value: 1 }),
            Task::new(TestData { value: 2 }),
            Task::new(TestData { value: 3 }),
        ];

        for task in &tasks {
            queue.push(task).await.unwrap();
        }

        for expected_task in tasks {
            let popped_task = queue.pop().await.unwrap();
            assert_eq!(popped_task.payload, expected_task.payload);
        }

        // Queue should be empty now
        assert!(matches!(queue.pop().await, Err(TaskQueueError::QueueEmpty)));
    }

    #[tokio::test]
    async fn test_task_not_found() {
        let queue = InMemoryTaskQueue::<TestData>::new();
        let non_existent_task_id = TaskId::new();

        match queue.ack(&non_existent_task_id).await {
            Err(TaskQueueError::TaskNotFound(_)) => (),
            _ => panic!("Expected TaskNotFound error"),
        }
    }
}
