#[cfg(test)]
mod tests {
    use capp_queue::queue::{RedisTaskQueue, TaskQueue, TaskQueueError};
    use capp_queue::task::Task;
    use dotenvy::dotenv;
    use rustis::client::Client;
    use rustis::commands::{GenericCommands, HashCommands, ListCommands};
    use serde::{Deserialize, Serialize};

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

    async fn setup_queue(name: &str) -> RedisTaskQueue<TestData> {
        let redis = get_redis_connection().await;
        RedisTaskQueue::new(redis, name)
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
    async fn test_typical_workflow() {
        let redis = get_redis_connection().await;
        let queue: RedisTaskQueue<TestData> =
            RedisTaskQueue::new(redis, "capp-test-workflow")
                .await
                .expect("Failed to create RedisTaskQueue");
        // clean queue
        queue
            .client
            .del([&queue.list_key, &queue.hashmap_key, &queue.dlq_key])
            .await
            .expect("Failed to clean up Redis keys");

        // check push
        let task = Task::new(TestData { value: 42 });
        let result = queue.push(&task).await;
        assert!(result.is_ok());

        // check pop
        let task_from_queue = queue.pop().await;
        assert!(!&task_from_queue.is_err());
        let task_from_queue = task_from_queue.unwrap();
        assert!(task_from_queue.get_payload().value == 42);

        // ack task and check queue is empty
        let acked = queue.ack(&task_from_queue.task_id).await;
        assert!(acked.is_ok());
        let queue_list_len = queue.client.llen(&queue.list_key).await;
        assert_eq!(queue_list_len.unwrap(), 0);
        let queue_hashmap_len = queue.client.hlen(&queue.hashmap_key).await;
        assert_eq!(queue_hashmap_len.unwrap(), 0);

        // check how ack/nack working and check ordering
        let task_1 = Task::new(TestData { value: 1 });
        let task_2 = Task::new(TestData { value: 2 });
        let _ = queue.push(&task_1).await;
        let _ = queue.push(&task_2).await;
        let queue_list_len = queue.client.llen(&queue.list_key).await;
        assert_eq!(queue_list_len.unwrap(), 2);
        let queue_hashmap_len = queue.client.hlen(&queue.hashmap_key).await;
        assert_eq!(queue_hashmap_len.unwrap(), 2);

        let task_1_from_queue = queue.pop().await.unwrap();
        assert_eq!(task_1_from_queue.payload.value, 1);
        let acked = queue.ack(&task_1_from_queue.task_id).await;
        assert!(acked.is_ok());

        let task_2_from_queue = queue.pop().await.unwrap();
        assert_eq!(task_2_from_queue.payload.value, 2);
        let nacked = queue.nack(&task_2_from_queue).await;
        assert!(nacked.is_ok());
        let dlq_len = queue.client.llen(&queue.dlq_key).await.unwrap();
        assert_eq!(dlq_len, 1);

        queue
            .client
            .del([&queue.list_key, &queue.hashmap_key, &queue.dlq_key])
            .await
            .expect("Failed to clean up Redis keys");
    }

    #[tokio::test]
    async fn test_push_and_pop() {
        let queue = setup_queue("capp-test-push-pop").await;
        let task = Task::new(TestData { value: 42 });

        queue.push(&task).await.expect("Failed to push task");

        let popped_task = queue.pop().await.expect("Failed to pop task");
        assert_eq!(popped_task.payload, task.payload);

        cleanup_queue(&queue).await;
    }

    #[tokio::test]
    async fn test_push_multiple_and_pop_order() {
        let queue = setup_queue("capp-test-push-pop-order").await;
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
        let queue = setup_queue("capp-test-ack").await;
        let task = Task::new(TestData { value: 42 });

        queue.push(&task).await.expect("Failed to push task");
        let popped_task = queue.pop().await.expect("Failed to pop task");
        dbg!(&popped_task);
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
        let queue = setup_queue("capp-test-nack").await;
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

        let dlq_len = queue
            .client
            .llen(&queue.dlq_key)
            .await
            .expect("Failed to read DLQ");
        assert_eq!(dlq_len, 1, "Task should have been added to DLQ");

        let dlq_task: Vec<String> =
            queue.client.rpop(&queue.dlq_key, 1).await.unwrap();

        let dlq_task: Task<TestData> = serde_json::from_str(&dlq_task[0])
            .expect("Failed to deserialize task from DLQ");
        assert_eq!(dlq_task.payload.value, popped_task.payload.value);

        cleanup_queue(&queue).await;
    }

    #[tokio::test]
    async fn test_set() {
        let queue = setup_queue("capp-test-set").await;
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
        let queue = setup_queue("capp-test-empty").await;
        cleanup_queue(&queue).await;

        match queue.pop().await {
            Err(TaskQueueError::QueueEmpty) => (),
            _ => panic!("Expected QueueEmpty error"),
        }
    }
}
