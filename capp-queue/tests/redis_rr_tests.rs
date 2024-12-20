#[cfg(feature = "redis")]
#[cfg(test)]
mod tests {
    use capp_queue::{
        HasTagKey, RedisRoundRobinTaskQueue, Task, TaskQueue, TaskQueueError,
    };
    use dotenvy::dotenv;
    use rustis::client::Client;
    use rustis::commands::{
        GenericCommands, HashCommands, ListCommands, SortedSetCommands,
    };
    use serde::{Deserialize, Serialize};
    use std::collections::HashSet;
    use std::sync::Arc;
    use std::time::Duration;

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

    // Cleanup before each test
    async fn cleanup_before_test(test_name: &str) {
        let redis = get_redis_connection().await;
        let pattern = format!("test-rr-{}*", test_name);
        // Get all keys matching our test pattern
        let keys: Vec<String> =
            redis.keys(&pattern).await.expect("Failed to get keys");
        if !keys.is_empty() {
            redis.del(keys).await.expect("Failed to delete keys");
        }
    }

    async fn setup_queue(test_name: &str) -> RedisRoundRobinTaskQueue<TestData> {
        // cleanup tests if present
        cleanup_before_test(test_name).await;
        let redis = get_redis_connection().await;
        let tags = HashSet::from([
            "tag1".to_string(),
            "tag2".to_string(),
            "tag3".to_string(),
        ]);
        let unique_name = format!("test-rr-{}", test_name);
        RedisRoundRobinTaskQueue::new(redis, &unique_name, tags)
            .await
            .expect("Failed to create RedisRoundRobinTaskQueue")
    }

    async fn cleanup_queue(queue: &RedisRoundRobinTaskQueue<TestData>) {
        let mut keys = vec![
            queue.get_hashmap_key(),
            queue.get_schedule_key(),
            queue.get_dlq_key(),
        ];

        // Add list keys for all known tags
        for tag in ["tag1", "tag2", "tag3"].iter() {
            keys.push(queue.get_list_key(tag));
        }

        queue
            .client
            .del(keys)
            .await
            .expect("Failed to clean up Redis keys");
    }

    #[tokio::test]
    async fn test_typical_workflow() {
        let queue = setup_queue("workflow").await;

        // Push tasks with different tags
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

        // Verify tasks are stored properly
        let hashmap_len: u64 =
            queue.client.hlen(queue.get_hashmap_key()).await.unwrap() as u64;
        assert_eq!(hashmap_len, 6, "All tasks should be in hashmap");

        // Pop tasks and verify round-robin behavior
        let mut popped_values = Vec::new();
        let mut popped_tags = Vec::new();

        for _ in 0..6 {
            let task = queue.pop().await.expect("Failed to pop task");
            popped_values.push(task.payload.value);
            popped_tags.push(task.payload.tag.clone());
        }

        // Verify even distribution of tags
        let tag1_count = popped_tags.iter().filter(|&t| t == "tag1").count();
        let tag2_count = popped_tags.iter().filter(|&t| t == "tag2").count();
        let tag3_count = popped_tags.iter().filter(|&t| t == "tag3").count();

        assert_eq!(tag1_count, 2, "Should have 2 tasks from tag1");
        assert_eq!(tag2_count, 2, "Should have 2 tasks from tag2");
        assert_eq!(tag3_count, 2, "Should have 2 tasks from tag3");

        // Verify queue is empty
        assert!(matches!(queue.pop().await, Err(TaskQueueError::QueueEmpty)));

        cleanup_queue(&queue).await;
    }

    #[tokio::test]
    async fn test_time_weighted_scheduling() {
        let queue = setup_queue("time-weighted").await;

        // Push initial tasks
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
        ];

        for task in &tasks {
            queue.push(task).await.expect("Failed to push task");
        }

        // Pop first task and delay
        let first_task = queue.pop().await.expect("Failed to pop first task");
        let first_tag = first_task.payload.tag.clone();

        // Add artificial delay
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Push more tasks to the first tag
        queue
            .push(&Task::new(TestData {
                value: 4,
                tag: first_tag.clone(),
            }))
            .await
            .expect("Failed to push task");

        // Pop tasks and verify order
        let second_task = queue.pop().await.expect("Failed to pop second task");
        assert_ne!(
            second_task.payload.tag, first_tag,
            "Second task should be from different tag due to timestamp"
        );

        cleanup_queue(&queue).await;
    }

    #[tokio::test]
    async fn test_empty_tag_handling() {
        let queue = setup_queue("empty-tag").await;

        // Push tasks for only two tags
        let tasks = vec![
            Task::new(TestData {
                value: 1,
                tag: "tag1".to_string(),
            }),
            Task::new(TestData {
                value: 2,
                tag: "tag2".to_string(),
            }),
        ];

        for task in &tasks {
            queue.push(task).await.expect("Failed to push task");
        }

        // Pop all tasks
        let mut popped_tags = Vec::new();
        while let Ok(task) = queue.pop().await {
            popped_tags.push(task.payload.tag);
        }

        assert_eq!(popped_tags.len(), 2, "Should pop exactly 2 tasks");
        assert!(
            !popped_tags.contains(&"tag3".to_string()),
            "Should not have tag3"
        );

        cleanup_queue(&queue).await;
    }

    #[tokio::test]
    async fn test_ack_and_nack() {
        let queue = setup_queue("ack-nack").await;
        let task = Task::new(TestData {
            value: 42,
            tag: "tag1".to_string(),
        });

        // Push and pop a task
        queue.push(&task).await.expect("Failed to push task");
        let popped_task = queue.pop().await.expect("Failed to pop task");

        // Test ack
        queue
            .ack(&popped_task.task_id)
            .await
            .expect("Failed to ack task");
        let task_exists: bool = queue
            .client
            .hexists(&queue.get_hashmap_key(), popped_task.task_id.to_string())
            .await
            .expect("Failed to check task existence");
        assert!(!task_exists, "Task should be removed after ack");

        // Test nack
        let task = Task::new(TestData {
            value: 43,
            tag: "tag2".to_string(),
        });
        queue.push(&task).await.expect("Failed to push task");
        let popped_task = queue.pop().await.expect("Failed to pop task");
        queue.nack(&popped_task).await.expect("Failed to nack task");

        // Verify task is in DLQ
        let dlq_len: u64 = queue
            .client
            .llen(&queue.get_dlq_key())
            .await
            .expect("Failed to get DLQ length") as u64;
        assert_eq!(dlq_len, 1, "Task should be in DLQ");

        cleanup_queue(&queue).await;
    }

    #[tokio::test]
    async fn test_concurrent_access() {
        let queue = Arc::new(setup_queue("concurrent").await);
        let mut handles = vec![];

        // Spawn multiple tasks pushing to the same tag
        for i in 0..10 {
            let queue_clone = Arc::clone(&queue);
            let handle = tokio::spawn(async move {
                let task = Task::new(TestData {
                    value: i,
                    tag: "tag1".to_string(),
                });
                queue_clone.push(&task).await.expect("Failed to push task");
            });
            handles.push(handle);
        }

        // Wait for all pushes to complete
        for handle in handles {
            handle.await.expect("Task failed");
        }

        // Verify all tasks were pushed
        let list_len: u64 = queue
            .client
            .llen(&queue.get_list_key("tag1"))
            .await
            .expect("Failed to get list length") as u64;
        assert_eq!(list_len, 10, "All tasks should be pushed");

        cleanup_queue(&queue).await;
    }

    #[tokio::test]
    async fn test_purge() {
        let queue = setup_queue("purge").await;

        // Push some tasks
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
        ];

        for task in &tasks {
            queue.push(task).await.expect("Failed to push task");
        }

        // Verify data exists
        let hashmap_len: u64 = queue
            .client
            .hlen(&queue.get_hashmap_key())
            .await
            .expect("Failed to get hashmap length")
            as u64;
        assert_eq!(hashmap_len, 3, "Should have 3 tasks before purge");

        // Purge queue
        queue.purge().await.expect("Failed to purge queue");

        // Verify all data is removed
        let hashmap_len: u64 = queue
            .client
            .hlen(&queue.get_hashmap_key())
            .await
            .expect("Failed to get hashmap length")
            as u64;
        assert_eq!(hashmap_len, 0, "Should have no tasks after purge");

        let schedule_len: u64 = queue
            .client
            .zcard(&queue.get_schedule_key())
            .await
            .expect("Failed to get schedule length")
            as u64;
        assert_eq!(schedule_len, 0, "Schedule should be empty after purge");

        cleanup_queue(&queue).await;
    }
}
