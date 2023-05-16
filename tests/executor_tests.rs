#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use capp::executor::storage::TaskStorage;
    use capp::executor::task::{Task, TaskProcessor};
    use capp::executor::{self, ExecutorOptions};
    use serde::{Deserialize, Serialize};
    use std::collections::{HashMap, VecDeque};
    use std::sync::{Arc, Mutex};
    use thiserror::Error;
    use tokio::runtime::Runtime;
    use uuid::Uuid;

    #[derive(Error, Debug, Serialize, Deserialize)]
    pub enum TaskError {
        #[error("unknown error")]
        Unknown,
    }
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct TaskData {
        pub domain: String,
        pub value: u32,
        pub finished: bool,
    }
    #[derive(Debug)]
    pub struct TestTaskProcessor {}

    pub struct TestStorage {
        hashmap: Mutex<HashMap<Uuid, String>>,
        list: Mutex<VecDeque<Uuid>>,
    }

    impl TestStorage {
        pub fn new() -> Self {
            Self {
                hashmap: Mutex::new(HashMap::new()),
                list: Mutex::new(VecDeque::new()),
            }
        }
    }

    impl std::fmt::Debug for TestStorage {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            // Lock the mutexes to access the data.
            let hashmap = self.hashmap.lock().unwrap();
            let list = self.list.lock().unwrap();

            // Use the debug builders to format the output.
            f.debug_struct("Database")
                .field("hashmap", &*hashmap)
                .field("list", &*list)
                .finish()
        }
    }

    #[async_trait]
    impl TaskProcessor<TaskData, TaskError> for TestTaskProcessor {
        /// Process will fail tasks which value can be divided to 3
        async fn process(
            &self,
            worker_id: usize,
            data: &mut TaskData,
        ) -> Result<(), TaskError> {
            log::info!("[worker-{}] Processing task: {:?}", worker_id, data);
            let rem = data.value % 3;
            if rem == 0 {
                return Err(TaskError::Unknown);
            };

            data.finished = true;
            Ok(())
        }
    }

    #[async_trait]
    impl TaskStorage<TaskData, TaskError> for TestStorage {
        async fn hashmap_set(
            &self,
            task: &Task<TaskData>,
        ) -> Result<(), TaskError> {
            let mut hashmap = self.hashmap.lock().unwrap();
            let task_data = serde_json::to_string(task).unwrap();
            hashmap.insert(task.task_id, task_data);
            Ok(())
        }

        async fn hashmap_get(
            &self,
            task_id: Uuid,
        ) -> Result<Option<Task<TaskData>>, TaskError> {
            let hashmap = self.hashmap.lock().unwrap();
            let data = hashmap.get(&task_id).unwrap();
            let task_data: Task<TaskData> = serde_json::from_str(data).unwrap();
            Ok(Some(task_data))
        }

        async fn list_push(&self, task: &Task<TaskData>) -> Result<(), TaskError> {
            let mut list = self.list.lock().unwrap();
            let mut hashmap = self.hashmap.lock().unwrap();

            let task_str = serde_json::to_string(task).unwrap();
            hashmap.insert(task.task_id, task_str);
            list.push_back(task.task_id);
            Ok(())
        }

        async fn list_pop(&self) -> Result<Option<Task<TaskData>>, TaskError> {
            let mut list = self.list.lock().unwrap();
            let hashmap = self.hashmap.lock().unwrap();

            if let Some(task_id) = list.pop_front() {
                let task_data = hashmap.get(&task_id).unwrap();
                let task: Task<TaskData> = serde_json::from_str(task_data).unwrap();
                return Ok(Some(task));
            }
            Ok(None)
        }

        async fn ack(
            &self,
            task: &Task<TaskData>,
        ) -> Result<Task<TaskData>, TaskError> {
            let mut hashmap = self.hashmap.lock().unwrap();
            let task_data = hashmap.remove(&task.task_id).unwrap();
            let task = serde_json::from_str(&task_data).unwrap();
            Ok(task)
        }
    }

    /// Make storage filled with test data.
    /// For current set following conditions should be true:
    /// total tasks = 9
    /// number of failed tasks = 4
    fn make_storage() -> Arc<TestStorage> {
        let storage = Arc::new(TestStorage::new());

        let rt = Runtime::new().unwrap();

        for i in 1..=3 {
            let task: Task<TaskData> = Task::new(TaskData {
                domain: "one".to_string(),
                value: i,
                finished: false,
            });
            let _ = rt.block_on(storage.list_push(&task));
        }

        for i in 1..=3 {
            let task: Task<TaskData> = Task::new(TaskData {
                domain: "two".to_string(),
                value: i * 3,
                finished: false,
            });
            let _ = rt.block_on(storage.list_push(&task));
        }

        for _ in 1..=3 {
            let task: Task<TaskData> = Task::new(TaskData {
                domain: "three".to_string(),
                value: 2,
                finished: false,
            });
            let _ = rt.block_on(storage.list_push(&task));
        }
        storage
    }

    #[test]
    fn test_storage() {
        let storage = make_storage();
        assert_eq!(storage.list.lock().unwrap().len(), 9);
    }

    #[test]
    fn test_executor() {
        let rt = Runtime::new().unwrap();
        let storage = make_storage();

        let storage_len_before = storage.list.lock().unwrap().len();

        assert_eq!(storage_len_before, 9);

        // dbg!(&storage);

        let processor = Arc::new(TestTaskProcessor {});
        rt.block_on(executor::run(
            processor,
            storage.clone(),
            ExecutorOptions {
                task_limit: Some(9),
                concurrency_limit: 2,
            },
        ));

        let storage_len_after = storage.list.lock().unwrap().len();

        // 4 tasks should fail
        assert_eq!(storage_len_after, 4);

        // all successful tasks should be removed from queue
        let keys_len = storage.hashmap.lock().unwrap().len();
        assert_eq!(keys_len, 4);

        // dbg!(&storage);
    }
}
