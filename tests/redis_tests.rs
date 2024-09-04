#[cfg(test)]
mod tests {
    use capp::queue::{RedisTaskQueue, TaskQueue, TaskQueueError};
    use capp::task::Task;
    use dotenvy::dotenv;
    use rustis::client::Client;
    use rustis::commands::{GenericCommands, HashCommands, ListCommands};
    use serde::{Deserialize, Serialize};
    use tokio;

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    struct TestData {
        value: u32,
    }

    async fn get_redis_connection() -> Client {
        dotenv().ok();
        let uri = std::env::var("REDIS_URI").expect("Set REDIS_URI env variable");
        Client::connect(uri)
            .await
            .expect("Error while establishing redis connection")
    }

    async fn setup_queue() -> RedisTaskQueue<TestData> {
        let redis = get_redis_connection().await;
        RedisTaskQueue::new(redis, "capp-test-queue")
            .await
            .expect("Failed to create RedisTaskQueue")
    }

    async fn cleanup_queue(queue: &RedisTaskQueue<TestData>) {
        queue
            .client
            .del([&queue.list_key, &queue.hashmap_key, &queue.dlq_key])
            .await
            .expect("Failed to clean up Redis keys");
    }

    #[tokio::test]
    async fn test_push_and_pop() {
        let queue = setup_queue().await;
        let task = Task::new(TestData { value: 42 });

        queue.push(&task).await.expect("Failed to push task");

        let popped_task = queue.pop().await.expect("Failed to pop task");
        assert_eq!(popped_task.payload, task.payload);

        cleanup_queue(&queue).await;
    }

    #[tokio::test]
    async fn test_push_multiple_and_pop_order() {
        let queue = setup_queue().await;
        let tasks = vec![
            Task::new(TestData { value: 1 }),
            Task::new(TestData { value: 2 }),
            Task::new(TestData { value: 3 }),
        ];

        for task in &tasks {
            queue.push(task).await.expect("Failed to push task");
        }

        for expected_value in 1..=3 {
            let popped_task = queue.pop().await.expect("Failed to pop task");
            assert_eq!(popped_task.payload.value, expected_value);
        }

        assert!(matches!(queue.pop().await, Err(TaskQueueError::QueueEmpty)));

        cleanup_queue(&queue).await;
    }

    #[tokio::test]
    async fn test_ack() {
        let queue = setup_queue().await;
        let task = Task::new(TestData { value: 42 });

        queue.push(&task).await.expect("Failed to push task");
        let popped_task = queue.pop().await.expect("Failed to pop task");
        queue
            .ack(&popped_task.task_id)
            .await
            .expect("Failed to ack task");

        let task_exists: bool = queue
            .client
            .hexists(&queue.hashmap_key, popped_task.task_id.to_string())
            .await
            .expect("Failed to check task existence");
        assert!(!task_exists, "Task should have been removed after ack");

        cleanup_queue(&queue).await;
    }

    #[tokio::test]
    async fn test_nack() {
        let queue = setup_queue().await;
        let task = Task::new(TestData { value: 42 });

        queue.push(&task).await.expect("Failed to push task");
        let popped_task = queue.pop().await.expect("Failed to pop task");
        queue.nack(&popped_task).await.expect("Failed to nack task");

        let task_exists: bool = queue
            .client
            .hexists(&queue.hashmap_key, popped_task.task_id.to_string())
            .await
            .expect("Failed to check task existence");
        assert!(
            !task_exists,
            "Task should have been removed from hashmap after nack"
        );

        let dlq_tasks: Vec<String> = queue
            .client
            .lrange(&queue.dlq_key, 0, -1)
            .await
            .expect("Failed to read DLQ");
        assert_eq!(dlq_tasks.len(), 1, "Task should have been added to DLQ");

        let dlq_task: Task<TestData> = serde_json::from_str(&dlq_tasks[0])
            .expect("Failed to deserialize task from DLQ");
        assert_eq!(dlq_task.payload, popped_task.payload);

        cleanup_queue(&queue).await;
    }

    #[tokio::test]
    async fn test_set() {
        let queue = setup_queue().await;
        let mut task = Task::new(TestData { value: 42 });

        queue.push(&task).await.expect("Failed to push task");
        task.payload.value = 43;
        queue.set(&task).await.expect("Failed to set task");

        let updated_task_json: String = queue
            .client
            .hget(&queue.hashmap_key, task.task_id.to_string())
            .await
            .expect("Failed to get updated task");
        let updated_task: Task<TestData> = serde_json::from_str(&updated_task_json)
            .expect("Failed to deserialize updated task");

        assert_eq!(updated_task.payload.value, 43);

        cleanup_queue(&queue).await;
    }

    #[tokio::test]
    async fn test_queue_empty() {
        let queue = setup_queue().await;

        match queue.pop().await {
            Err(TaskQueueError::QueueEmpty) => (),
            _ => panic!("Expected QueueEmpty error"),
        }

        cleanup_queue(&queue).await;
    }
}
