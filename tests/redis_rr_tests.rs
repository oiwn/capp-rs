#[cfg(test)]
mod tests {
    // Test redis round robin queue
    use capp::prelude::Task;
    use capp::storage::{HasTagKey, RedisRoundRobinTaskStorage, TaskStorage};
    use dotenvy::dotenv;
    use rustis::commands::GenericCommands;
    use serde::{Deserialize, Serialize};
    use std::collections::{HashMap, HashSet};
    use tokio;

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct TaskData {
        tag: String,
        value: u32,
    }

    impl HasTagKey for TaskData {
        type TagValue = String;
        fn get_tag_value(&self) -> Self::TagValue {
            self.tag.clone()
        }
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
        let tags: HashSet<String> = vec!["one", "two"]
            .into_iter()
            .map(|x| x.to_string())
            .collect();
        let _guard = RedisTestGuard {
            key: "td:rr:test",
            redis: &redis,
        };
        let tag_counts: HashMap<String, u32> = vec![
            ("one".to_string(), 5), // 5 dummy data for tag "one"
            ("two".to_string(), 3), // 3 dummy data for tag "two"
        ]
        .into_iter()
        .collect();

        // generating tasks, total 8
        let mut tasks = Vec::new();
        for (tag, count) in tag_counts.iter() {
            for i in 0..*count {
                tasks.push(Task::new(TaskData {
                    tag: tag.into(),
                    value: i,
                }));
            }
        }

        let storage: RedisRoundRobinTaskStorage<TaskData> =
            RedisRoundRobinTaskStorage::new(
                "td:rr:test",
                tags.clone(),
                "tag",
                redis.clone(),
            );

        for task in &tasks {
            let _ = storage.task_push(&task).await;
        }

        let _task = storage.task_pop().await.unwrap();
        // assert!(vec!["one", "two"].contains(task.payload.tag));
        // assert_eq!(task.payload.tag, "one");
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
