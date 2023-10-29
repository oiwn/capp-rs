#[cfg(test)]
mod tests {
    use dotenvy::dotenv;
    use rustis::commands::{HashCommands, ListCommands};
    use serde::{Deserialize, Serialize};
    use task_deport::RedisTaskStorage;
    use task_deport::Task;
    use task_deport::TaskStorage;
    use tokio;

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct TaskData {
        value: u32,
    }

    async fn get_redis_connection() -> rustis::client::Client {
        dotenv().ok();
        let uri = std::env::var("REDIS_URI").expect("Set REDIS_URI env variable");
        rustis::client::Client::connect(uri)
            .await
            .expect("Error while establishing redis connection")
    }

    #[tokio::test]
    async fn test_task_set() {
        let redis = get_redis_connection().await;
        let task_storage = RedisTaskStorage::new("td:test", redis);
        let task = Task::new(TaskData { value: 0 });
        task_storage.task_set(&task).await.unwrap();

        let task_value: String = task_storage
            .redis
            .hget(&task_storage.get_hashmap_key(), &task.task_id.to_string())
            .await
            .unwrap();
        let task_data: Task<TaskData> = serde_json::from_str(&task_value).unwrap();
        assert_eq!(task_data.payload.value, task.payload.value);
    }

    #[tokio::test]
    async fn test_task_get() {
        let redis = get_redis_connection().await;
        let task_storage: RedisTaskStorage<TaskData> =
            RedisTaskStorage::new("td:test", redis);
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
        assert_eq!(task_data.payload.value, task.payload.value);
    }

    #[tokio::test]
    async fn test_task_pop() {
        let redis = get_redis_connection().await;
        let task_storage: RedisTaskStorage<TaskData> =
            RedisTaskStorage::new("test", redis);
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
        assert_eq!(task_data.payload.value, task.payload.value);
    }
}
