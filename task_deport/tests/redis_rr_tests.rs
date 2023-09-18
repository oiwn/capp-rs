#[cfg(test)]
mod tests {
    // Test redis round robin queue
    use dotenvy::dotenv;
    use rustis::commands::{GenericCommands, HashCommands, ListCommands};
    use serde::{Deserialize, Serialize};
    use std::collections::HashSet;
    use task_deport::RedisRoundRobinTaskStorage;
    use task_deport::Task;
    use task_deport::TaskStorage;
    use tokio;

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct TaskData {
        tag: String,
        value: u32,
    }

    struct RedisTestGuard<'a> {
        key: &'a str,
        redis: &'a rustis::client::Client,
    }

    impl<'a> Drop for RedisTestGuard<'a> {
        fn drop(&mut self) {
            let _ = self.redis.del(self.key);
        }
    }

    async fn get_redis_connection() -> rustis::client::Client {
        dotenv().ok();
        let uri = std::env::var("REDIS_URI").expect("Set REDIS_URI env variable");
        rustis::client::Client::connect(uri)
            .await
            .expect("Error while establishing redis connection")
    }

    #[tokio::test]
    async fn test_flow() {
        let redis = get_redis_connection().await;
        let tags: HashSet<String> = vec!["one", "two", "more"]
            .into_iter()
            .map(|x| x.to_string())
            .collect();
        let _guard = RedisTestGuard {
            key: "td:rr:test",
            redis: &redis,
        };

        let storage: RedisRoundRobinTaskStorage<TaskData> =
            RedisRoundRobinTaskStorage::new(
                "td:rr:test",
                tags.clone(),
                "tag",
                redis.clone(),
            );

        // Generating tasks
        let tasks: Vec<_> = tags
            .iter()
            .enumerate()
            .map(|(value, tag)| {
                Task::new(TaskData {
                    tag: tag.clone(),
                    value: value as u32,
                })
            })
            .collect();

        for task in &tasks {
            let _ = storage.task_push(&task).await;
        }

        let task = storage.task_pop().await.unwrap().unwrap();
        assert_eq!(task.data.tag, "one");
    }

    /*
    #[tokio::test]
    async fn test_task_get() {
        let redis = get_redis_connection().await;
        let task_storage: RedisTaskStorage<TaskData> =
            RedisTaskStorage::new("td:rr:test", redis.clone());
        let _guard = RedisTestGuard {
            key: "td:rr:test".to_string(),
            redis: &redis,
        };

        let task = Task::new(TaskData { value: 42 });
        task_storage
            .redis
            .hset(
                &task_storage.get_hashmap_key(),
                [(
                    &task.task_id.to_string(),
                    &serde_json::to_string(&task).unwrap(),
                )],
            )
            .await
            .unwrap();

        let task_data = task_storage.task_get(&task.task_id).await.unwrap();
        assert_eq!(task_data.data.value, task.data.value);
    }

    #[tokio::test]
    async fn test_task_pop() {
        let redis = get_redis_connection().await;
        let task_storage: RedisTaskStorage<TaskData> =
            RedisTaskStorage::new("td:rr:test", redis.clone());
        let _guard = RedisTestGuard {
            key: "td:rr:test".to_string(),
            redis: &redis,
        };

        let task = Task::new(TaskData { value: 42 });
        task_storage
            .redis
            .lpush(&task_storage.get_list_key(), &task.task_id.to_string())
            .await
            .unwrap();
        task_storage
            .redis
            .hset(
                &task_storage.get_hashmap_key(),
                [(
                    &task.task_id.to_string(),
                    &serde_json::to_string(&task).unwrap(),
                )],
            )
            .await
            .unwrap();

        let task_data = task_storage.task_pop().await.unwrap().unwrap();
        assert_eq!(task_data.data.value, task.data.value);
    }
    */
}
