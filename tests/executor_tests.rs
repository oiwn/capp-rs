#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use capp::executor::storage::{InMemoryTaskStorage, TaskStorage};
    use capp::executor::task::{Task, TaskProcessor};
    use capp::executor::{self, ExecutorOptions};
    use serde::{Deserialize, Serialize};
    use std::sync::Arc;
    use thiserror::Error;
    use tokio::runtime::Runtime;

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

    /// Make storage filled with test data.
    /// For current set following conditions should be true:
    /// total tasks = 9
    /// number of failed tasks = 4
    fn make_storage() -> Arc<InMemoryTaskStorage<TaskData, TaskError>> {
        let storage = Arc::new(InMemoryTaskStorage::new());

        let rt = Runtime::new().unwrap();

        for i in 1..=3 {
            let task: Task<TaskData> = Task::new(TaskData {
                domain: "one".to_string(),
                value: i,
                finished: false,
            });
            let _ = rt.block_on(storage.task_push(&task));
        }

        for i in 1..=3 {
            let task: Task<TaskData> = Task::new(TaskData {
                domain: "two".to_string(),
                value: i * 3,
                finished: false,
            });
            let _ = rt.block_on(storage.task_push(&task));
        }

        for _ in 1..=3 {
            let task: Task<TaskData> = Task::new(TaskData {
                domain: "three".to_string(),
                value: 2,
                finished: false,
            });
            let _ = rt.block_on(storage.task_push(&task));
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

        // all successful tasks should be removed from storage
        let keys_len = storage.hashmap.lock().unwrap().len();
        assert_eq!(keys_len, 4);
        let list_len = storage.list.lock().unwrap().len();
        assert_eq!(list_len, 4);

        // dbg!(&storage);
    }
}
