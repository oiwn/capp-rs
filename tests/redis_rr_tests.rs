#[cfg(test)]
mod tests {
    use capp::queue::{
        HasTagKey, RedisRoundRobinTaskQueue, TaskQueue, TaskQueueError,
    };
    use capp::task::Task;
    use dotenvy::dotenv;
    use rustis::client::Client;
    use rustis::commands::{GenericCommands, HashCommands, ListCommands};
    use serde::{Deserialize, Serialize};
    use std::collections::HashSet;
    use tokio;

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    struct TestData {
        value: u32,
        tag: String,
    }

    impl HasTagKey for TestData {
        type TagValue = String;
        fn get_tag_value(&self) -> Self::TagValue {
            self.tag.clone()
        }
    }

    async fn get_redis_connection() -> Client {
        dotenv().ok();
        let uri = std::env::var("REDIS_URI").expect("Set REDIS_URI env variable");
        Client::connect(uri)
            .await
            .expect("Error while establishing redis connection")
    }

    async fn setup_queue(test_name: &str) -> RedisRoundRobinTaskQueue<TestData> {
        let redis = get_redis_connection().await;
        let tags = HashSet::from([
            "tag1".to_string(),
            "tag2".to_string(),
            "tag3".to_string(),
        ]);
        let unique_name = format!("capp-test-redis-rr-{}", test_name);
        RedisRoundRobinTaskQueue::new(redis, &unique_name, tags)
            .await
            .expect("Failed to create RedisRoundRobinTaskQueue")
    }

    async fn cleanup_queue(queue: &RedisRoundRobinTaskQueue<TestData>) {
        let mut keys_to_delete = vec![
            queue.get_hashmap_key(),
            queue.get_list_key("tag1"),
            queue.get_list_key("tag2"),
            queue.get_list_key("tag3"),
            queue.get_dlq_key(),
        ];
        // Add counter keys to the list of keys to delete
        keys_to_delete.extend(queue.get_counter_keys());
        queue
            .client
            .del(keys_to_delete)
            .await
            .expect("Failed to clean up Redis keys");
    }

    #[tokio::test]
    async fn test_typical_workflow() {
        let queue = setup_queue("workflow").await;
        cleanup_queue(&queue).await;

        // Push tasks with different tags
        let task1 = Task::new(TestData {
            value: 1,
            tag: "tag1".to_string(),
        });
        let task2 = Task::new(TestData {
            value: 2,
            tag: "tag2".to_string(),
        });
        let task3 = Task::new(TestData {
            value: 3,
            tag: "tag3".to_string(),
        });

        queue.push(&task1).await.expect("Failed to push task1");
        queue.push(&task2).await.expect("Failed to push task2");
        queue.push(&task3).await.expect("Failed to push task3");

        // Pop tasks and check round-robin order
        let popped_task1 = queue.pop().await.expect("Failed to pop task1");
        let popped_task2 = queue.pop().await.expect("Failed to pop task2");
        let popped_task3 = queue.pop().await.expect("Failed to pop task3");

        // Check that all tags are represented
        let popped_tags: HashSet<String> = vec![
            popped_task1.payload.tag.clone(),
            popped_task2.payload.tag.clone(),
            popped_task3.payload.tag.clone(),
        ]
        .into_iter()
        .collect();

        assert_eq!(
            popped_tags,
            HashSet::from([
                "tag1".to_string(),
                "tag2".to_string(),
                "tag3".to_string()
            ]),
            "All tags should be represented in the popped tasks"
        );

        // Ack one task, nack another, and check queue state
        queue
            .ack(&popped_task1.task_id)
            .await
            .expect("Failed to ack task1");
        queue
            .nack(&popped_task2)
            .await
            .expect("Failed to nack task2");

        let hashmap_len = queue
            .client
            .hlen(&queue.get_hashmap_key())
            .await
            .expect("Failed to get hashmap length");
        assert_eq!(hashmap_len, 1, "Only one task should remain in the hashmap");

        let dlq_len = queue
            .client
            .llen(&queue.get_dlq_key())
            .await
            .expect("Failed to get DLQ length");
        assert_eq!(dlq_len, 1, "One task should be in the DLQ");

        cleanup_queue(&queue).await;
    }

    #[tokio::test]
    async fn test_push_and_pop() {
        let queue = setup_queue("push-pop").await;
        cleanup_queue(&queue).await;
        let task = Task::new(TestData {
            value: 42,
            tag: "tag1".to_string(),
        });

        queue.push(&task).await.expect("Failed to push task");

        let popped_task = queue.pop().await.expect("Failed to pop task");
        assert_eq!(popped_task.payload, task.payload);

        cleanup_queue(&queue).await;
    }

    #[tokio::test]
    async fn test_empty_queue() {
        let queue = setup_queue("empty-queue").await;
        cleanup_queue(&queue).await;

        let task_1 = Task::new(TestData {
            value: 1,
            tag: "tag1".to_string(),
        });

        let task_2 = Task::new(TestData {
            value: 2,
            tag: "tag2".to_string(),
        });

        let task_3 = Task::new(TestData {
            value: 3,
            tag: "tag2".to_string(),
        });

        queue.push(&task_1).await.expect("Failed to push task");
        queue.push(&task_2).await.expect("Failed to push task");
        queue.push(&task_3).await.expect("Failed to push task");

        queue.pop().await.expect("Failed to pop task");
        queue.pop().await.expect("Failed to pop task");
        queue.pop().await.expect("Failed to pop task");

        let should_be_error = queue.pop().await;
        assert!(should_be_error.is_err());

        cleanup_queue(&queue).await;
    }

    #[tokio::test]
    async fn test_round_robin_behavior() {
        let queue = setup_queue("round-robin").await;
        cleanup_queue(&queue).await;

        let tasks = vec![
            Task::new(TestData {
                value: 1,
                tag: "tag1".to_string(),
            }),
            Task::new(TestData {
                value: 2,
                tag: "tag2".to_string(),
            }),
            Task::new(TestData {
                value: 3,
                tag: "tag3".to_string(),
            }),
            Task::new(TestData {
                value: 4,
                tag: "tag1".to_string(),
            }),
            Task::new(TestData {
                value: 5,
                tag: "tag2".to_string(),
            }),
            Task::new(TestData {
                value: 6,
                tag: "tag3".to_string(),
            }),
        ];

        for task in &tasks {
            queue.push(task).await.expect("Failed to push task");
        }

        let mut popped_values = Vec::new();
        for _ in 0..6 {
            let popped_task = queue.pop().await.expect("Failed to pop task");
            popped_values.push(popped_task.payload.value);
        }

        assert_eq!(popped_values.len(), 6, "Should have popped 6 tasks");
        assert!(
            popped_values.contains(&1) && popped_values.contains(&4),
            "Should contain both tag1 tasks"
        );
        assert!(
            popped_values.contains(&2) && popped_values.contains(&5),
            "Should contain both tag2 tasks"
        );
        assert!(
            popped_values.contains(&3) && popped_values.contains(&6),
            "Should contain both tag3 tasks"
        );

        // Attempt to pop again, should be empty
        match queue.pop().await {
            Err(TaskQueueError::QueueEmpty) => (),
            _ => panic!("Queue not empty!"),
        }
        cleanup_queue(&queue).await;
    }

    #[tokio::test]
    async fn test_ack() {
        let queue = setup_queue("ack").await;
        cleanup_queue(&queue).await;
        let task = Task::new(TestData {
            value: 42,
            tag: "tag1".to_string(),
        });

        queue.push(&task).await.expect("Failed to push task");
        let popped_task = queue.pop().await.expect("Failed to pop task");
        queue
            .ack(&popped_task.task_id)
            .await
            .expect("Failed to ack task");

        let task_exists: bool = queue
            .client
            .hexists(&queue.get_hashmap_key(), popped_task.task_id.to_string())
            .await
            .expect("Failed to check task existence");
        assert!(!task_exists, "Task should have been removed after ack");

        cleanup_queue(&queue).await;
    }

    #[tokio::test]
    async fn test_nack() {
        let queue = setup_queue("nack").await;
        let task = Task::new(TestData {
            value: 42,
            tag: "tag1".to_string(),
        });

        queue.push(&task).await.expect("Failed to push task");
        let popped_task = queue.pop().await.expect("Failed to pop task");
        queue.nack(&popped_task).await.expect("Failed to nack task");

        let task_exists: bool = queue
            .client
            .hexists(&queue.get_hashmap_key(), popped_task.task_id.to_string())
            .await
            .expect("Failed to check task existence");
        assert!(
            !task_exists,
            "Task should have been removed from hashmap after nack"
        );

        let dlq_len = queue
            .client
            .llen(&queue.get_dlq_key())
            .await
            .expect("Failed to read DLQ");
        assert_eq!(dlq_len, 1, "Task should have been added to DLQ");

        cleanup_queue(&queue).await;
    }

    #[tokio::test]
    async fn test_set() {
        let queue = setup_queue("set").await;
        let mut task = Task::new(TestData {
            value: 42,
            tag: "tag1".to_string(),
        });

        queue.push(&task).await.expect("Failed to push task");
        task.payload.value = 43;
        queue.set(&task).await.expect("Failed to set task");

        let updated_task_json: String = queue
            .client
            .hget(&queue.get_hashmap_key(), task.task_id.to_string())
            .await
            .expect("Failed to get updated task");
        let updated_task: Task<TestData> = serde_json::from_str(&updated_task_json)
            .expect("Failed to deserialize updated task");

        assert_eq!(updated_task.payload.value, 43);

        cleanup_queue(&queue).await;
    }

    #[tokio::test]
    async fn test_queue_empty() {
        let queue = setup_queue("empty").await;
        cleanup_queue(&queue).await;

        match queue.pop().await {
            Err(TaskQueueError::QueueEmpty) => (),
            _ => panic!("Expected QueueEmpty error"),
        }
    }
}
