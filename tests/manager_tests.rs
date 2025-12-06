#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use capp::config::Configurable;
    use capp::manager::{
        Computation, ComputationError, WorkerId, WorkerOptionsBuilder,
        WorkersManager, WorkersManagerOptionsBuilder,
    };
    use capp::queue::{AbstractTaskQueue, InMemoryTaskQueue, TaskQueue};
    use capp::task::Task;
    use serde::{Deserialize, Serialize};
    use std::sync::Arc;
    use tokio::runtime::Runtime;

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct TestData {
        pub domain: String,
        pub value: u32,
        pub finished: bool,
    }

    #[derive(Debug)]
    pub struct TestComputation {}

    #[derive(Debug, Serialize, Deserialize)]
    pub struct Context {
        name: String,
        config: toml::Value,
        is_test: bool,
    }

    impl Configurable for Context {
        fn config(&self) -> &toml::Value {
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
    impl Computation<TestData, Context> for TestComputation {
        /// Process will fail tasks which value can be divided to 3
        async fn call(
            &self,
            worker_id: WorkerId,
            _ctx: Arc<Context>,
            _storage: AbstractTaskQueue<TestData>,
            task: &mut Task<TestData>,
        ) -> Result<(), ComputationError> {
            tracing::info!("[worker-{}] Processing task: {:?}", worker_id, task);
            let rem = task.payload.value % 3;
            // fail if can be divided by 3
            if rem == 0 {
                return Err(ComputationError::Function(format!(
                    "Can be divide {} to 3",
                    &task.payload.value
                )));
            };

            task.payload.finished = true;
            Ok(())
        }
    }

    /// Make storage filled with test data.
    /// For current set following conditions should be true:
    /// total tasks = 9
    /// number of failed tasks = 4
    fn make_storage() -> InMemoryTaskQueue<TestData> {
        let storage = InMemoryTaskQueue::new();

        let rt = Runtime::new().unwrap();

        // Only 1 number can be divided by 3
        for i in 1..=3 {
            let task: Task<TestData> = Task::new(TestData {
                domain: "one".to_string(),
                value: i,
                finished: false,
            });
            let _ = rt.block_on(storage.push(&task));
        }

        // all 3 numbers can be divided by 3
        for i in 1..=3 {
            let task: Task<TestData> = Task::new(TestData {
                domain: "two".to_string(),
                value: i * 3,
                finished: false,
            });
            let _ = rt.block_on(storage.push(&task));
        }

        // No numbers can be divided by 3
        for _ in 1..=3 {
            let task: Task<TestData> = Task::new(TestData {
                domain: "three".to_string(),
                value: 2,
                finished: false,
            });
            let _ = rt.block_on(storage.push(&task));
        }
        storage
    }

    #[test]
    fn test_storage() {
        let storage = make_storage();
        assert_eq!(storage.list.lock().unwrap().len(), 9);
    }

    #[test]
    fn test_manager() {
        let rt = Runtime::new().unwrap();

        let ctx = Arc::new(Context::from_config("tests/simple_config.toml"));
        let storage = Arc::new(make_storage());

        let storage_len_before = storage.list.lock().unwrap().len();
        assert_eq!(storage_len_before, 9);

        let computation = Arc::new(TestComputation {});
        let manager_options = WorkersManagerOptionsBuilder::default()
            .worker_options(
                WorkerOptionsBuilder::default()
                    .max_retries(10_u32)
                    .task_limit(3)
                    .no_task_found_delay(std::time::Duration::from_millis(50))
                    .build()
                    .unwrap(),
            )
            .concurrency_limit(1 as usize)
            .task_limit(Some(9))
            .build()
            .unwrap();

        let mut manager = WorkersManager::new_from_arcs(
            ctx,
            computation,
            storage.clone(),
            manager_options,
        );
        rt.block_on(manager.run_workers());

        let storage_len_after = storage.list.lock().unwrap().len();

        // 4 tasks should fail
        assert_eq!(storage_len_after, 7);
        // all successful tasks should be removed from storage
        // 7 should left
        let keys_len = storage.hashmap.lock().unwrap().len();
        assert_eq!(keys_len, 7);

        // dbg!(&storage);
    }
}
