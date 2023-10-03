#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use capp::config::Configurable;
    use capp::executor::processor::TaskProcessor;
    use capp::executor::{self, ExecutorOptionsBuilder};
    use capp::task_deport::{InMemoryTaskStorage, Task, TaskStorage};
    use serde::{Deserialize, Serialize};
    use std::sync::Arc;
    use thiserror::Error;
    use tokio::runtime::Runtime;

    #[derive(Error, Debug)]
    pub enum TaskProcessorError {
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

    #[derive(Debug, Serialize, Deserialize)]
    pub struct Context {
        name: String,
        config: serde_yaml::Value,
        is_test: bool,
    }

    impl Configurable for Context {
        fn name(&self) -> &str {
            self.name.as_str()
        }
        fn config(&self) -> &serde_yaml::Value {
            &self.config
        }
    }

    impl Context {
        fn from_config(config_file_path: impl AsRef<std::path::Path>) -> Self {
            let config = Self::load_config(config_file_path);
            Self {
                name: "test-app".to_string(),
                is_test: true,
                config: config.unwrap(),
            }
        }
    }

    #[async_trait]
    impl
        TaskProcessor<
            TaskData,
            TaskProcessorError,
            InMemoryTaskStorage<TaskData>,
            Context,
        > for TestTaskProcessor
    {
        /// Process will fail tasks which value can be divided to 3
        async fn process(
            &self,
            worker_id: usize,
            _ctx: Arc<Context>,
            _storage: Arc<InMemoryTaskStorage<TaskData>>,
            task: &mut Task<TaskData>,
        ) -> Result<(), TaskProcessorError> {
            log::info!("[worker-{}] Processing task: {:?}", worker_id, task);
            let rem = task.payload.value % 3;
            if rem == 0 {
                return Err(TaskProcessorError::Unknown);
            };

            task.payload.finished = true;
            Ok(())
        }
    }

    /// Make storage filled with test data.
    /// For current set following conditions should be true:
    /// total tasks = 9
    /// number of failed tasks = 4
    fn make_storage() -> InMemoryTaskStorage<TaskData> {
        let storage = InMemoryTaskStorage::new();

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

        let ctx = Arc::new(Context::from_config("tests/simple_config.yml"));
        let storage = Arc::new(make_storage());

        let storage_len_before = storage.list.lock().unwrap().len();
        assert_eq!(storage_len_before, 9);

        // dbg!(&storage);

        let processor = Arc::new(TestTaskProcessor {});
        let executor_options = ExecutorOptionsBuilder::default()
            .concurrency_limit(2 as usize)
            .task_limit(Some(9))
            .build()
            .unwrap();

        rt.block_on(executor::run_workers(
            ctx.clone(),
            processor,
            storage.clone(),
            executor_options,
        ));

        let storage_len_after = storage.list.lock().unwrap().len();

        // 4 tasks should fail
        assert_eq!(storage_len_after, 4);

        // all successful tasks should be removed from storage
        // 4 should left
        let keys_len = storage.hashmap.lock().unwrap().len();
        assert_eq!(keys_len, 4);
        let list_len = storage.list.lock().unwrap().len();
        assert_eq!(list_len, 4);

        // dbg!(&storage);
    }
}
