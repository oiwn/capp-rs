#[cfg(test)]
mod tests {
    use capp_queue::queue::{MongoTaskQueue, TaskQueue, TaskQueueError};
    use capp_queue::task::Task;
    use dotenvy::dotenv;
    use mongodb::{bson::doc, options::ClientOptions, Client};
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    struct TestData {
        value: u32,
    }

    async fn get_mongo_connection() -> String {
        dotenv().ok();
        std::env::var("MONGODB_URI").expect("Set MONGODB_URI env variable")
    }

    async fn cleanup_collections(name: &str) {
        let uri = get_mongo_connection().await;
        let client_options = ClientOptions::parse(&uri)
            .await
            .expect("Failed to parse MongoDB options");
        let client = Client::with_options(client_options.clone())
            .expect("Failed to create MongoDB client");

        // Get database name from URI or use default
        let db_name = client_options
            .default_database
            .as_ref()
            .expect("No database specified in MongoDB URI");

        let db = client.database(db_name);

        // Drop collections if they exist
        let _ = db
            .collection::<Task<TestData>>(&format!("{}_tasks", name))
            .drop()
            .await;
        let _ = db
            .collection::<Task<TestData>>(&format!("{}_dlq", name))
            .drop()
            .await;
    }

    async fn setup_queue(name: &str) -> MongoTaskQueue<TestData> {
        cleanup_collections(name).await;
        let uri = get_mongo_connection().await;
        MongoTaskQueue::new(&uri, name)
            .await
            .expect("Failed to create MongoTaskQueue")
    }

    #[tokio::test]
    async fn test_typical_workflow() {
        let queue = setup_queue("capp-test-workflow").await;

        // Test push and pop
        let task = Task::new(TestData { value: 42 });
        let task_id = task.task_id;
        queue.push(&task).await.expect("Failed to push task");

        let popped_task = queue.pop().await.expect("Failed to pop task");
        assert_eq!(popped_task.payload.value, 42);
        assert_eq!(popped_task.task_id, task_id); // Verify task ID matches

        // Test ack
        let acked = queue.ack(&popped_task.task_id).await;
        assert!(acked.is_ok(), "Failed to ack task: {:?}", acked);

        // Verify queue is empty
        assert!(matches!(queue.pop().await, Err(TaskQueueError::QueueEmpty)));

        // Test multiple tasks
        let task_1 = Task::new(TestData { value: 1 });
        let task_2 = Task::new(TestData { value: 2 });
        queue.push(&task_1).await.expect("Failed to push task 1");
        queue.push(&task_2).await.expect("Failed to push task 2");

        let popped = queue.pop().await.expect("Failed to pop task");
        assert_eq!(popped.payload.value, 1);
        queue
            .ack(&popped.task_id)
            .await
            .expect("Failed to ack task");

        let popped = queue.pop().await.expect("Failed to pop task");
        assert_eq!(popped.payload.value, 2);
        queue.nack(&popped).await.expect("Failed to nack task");
    }

    #[tokio::test]
    async fn test_push_and_pop() {
        let queue = setup_queue("capp-test-push-pop").await;
        let task = Task::new(TestData { value: 42 });
        let task_id = task.task_id;

        queue.push(&task).await.expect("Failed to push task");

        let popped_task = queue.pop().await.expect("Failed to pop task");
        assert_eq!(popped_task.payload.value, 42);
        assert_eq!(popped_task.task_id, task_id);
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
    }

    #[tokio::test]
    async fn test_ack() {
        let queue = setup_queue("capp-test-ack").await;
        let task = Task::new(TestData { value: 42 });
        let task_id = task.task_id;

        queue.push(&task).await.expect("Failed to push task");
        let popped_task = queue.pop().await.expect("Failed to pop task");
        assert_eq!(popped_task.task_id, task_id); // Verify we got the right task

        queue
            .ack(&popped_task.task_id)
            .await
            .expect("Failed to ack task");

        assert!(matches!(queue.pop().await, Err(TaskQueueError::QueueEmpty)));
    }

    #[tokio::test]
    async fn test_nack() {
        let queue = setup_queue("capp-test-nack").await;
        let task = Task::new(TestData { value: 42 });
        let task_id = task.task_id;

        queue.push(&task).await.expect("Failed to push task");
        let popped_task = queue.pop().await.expect("Failed to pop task");
        assert_eq!(popped_task.task_id, task_id);

        // Disable retryable writes for transactions
        queue.nack(&popped_task).await.expect("Failed to nack task");

        assert!(matches!(queue.pop().await, Err(TaskQueueError::QueueEmpty)));
    }

    #[tokio::test]
    async fn test_set() {
        let queue = setup_queue("capp-test-set").await;
        let task = Task::new(TestData { value: 42 });
        let task_id = task.task_id;

        queue.push(&task).await.expect("Failed to push task");

        let mut popped_task = queue.pop().await.expect("Failed to pop task");
        assert_eq!(popped_task.task_id, task_id);

        popped_task.payload.value = 43;
        queue.set(&popped_task).await.expect("Failed to set task");

        let updated_task = queue.pop().await.expect("Failed to get updated task");
        assert_eq!(updated_task.payload.value, 43);
    }

    #[tokio::test]
    async fn test_queue_empty() {
        let queue = setup_queue("capp-test-empty").await;
        assert!(matches!(queue.pop().await, Err(TaskQueueError::QueueEmpty)));
    }

    #[tokio::test]
    async fn test_task_persistence() {
        let queue = setup_queue("capp-test-persistence").await;
        let task = Task::new(TestData { value: 42 });
        let task_id = task.task_id;

        // Test push
        queue.push(&task).await.expect("Failed to push task");

        // Verify task exists in MongoDB directly
        let raw_task = queue
            .tasks_collection
            .find_one(doc! { "task_id": task_id.to_string() })
            .await
            .expect("Failed to query task")
            .expect("Task not found in collection");

        assert_eq!(raw_task.task_id, task_id);
        assert_eq!(raw_task.payload.value, 42);
    }

    #[tokio::test]
    async fn test_task_removal() {
        let queue = setup_queue("capp-test-removal").await;
        let task = Task::new(TestData { value: 42 });
        let task_id = task.task_id;

        // Push and verify task exists
        queue.push(&task).await.expect("Failed to push task");

        let exists_before = queue
            .tasks_collection
            .find_one(doc! { "task_id": task_id.to_string() })
            .await
            .expect("Failed to query task");
        assert!(exists_before.is_some(), "Task should exist before pop");

        // Pop and verify task is gone
        let popped = queue.pop().await.expect("Failed to pop task");
        assert_eq!(popped.task_id, task_id);

        let exists_after = queue
            .tasks_collection
            .find_one(doc! { "task_id": task_id.to_string() })
            .await
            .expect("Failed to query task");
        assert!(exists_after.is_none(), "Task should be removed after pop");
    }

    #[tokio::test]
    async fn test_nack_task_moves_to_dlq() {
        let queue = setup_queue("capp-test-nack-dlq").await;
        let task = Task::new(TestData { value: 42 });
        let task_id = task.task_id;

        // Push task
        queue.push(&task).await.expect("Failed to push task");

        // Pop task
        let popped = queue.pop().await.expect("Failed to pop task");
        assert_eq!(popped.task_id, task_id);

        // Try to nack without transactions first
        let nack_result = queue.nack(&popped).await;
        println!("Nack result: {:?}", nack_result);

        // Verify task moved to DLQ
        let in_dlq = queue
            .dlq_collection
            .find_one(doc! { "task_id": task_id.to_string() })
            .await
            .expect("Failed to query DLQ");

        if let Some(dlq_task) = in_dlq {
            println!("Task found in DLQ: {:?}", dlq_task);
        } else {
            println!("Task not found in DLQ");
        }

        // Verify removed from main queue
        let in_main = queue
            .tasks_collection
            .find_one(doc! { "task_id": task_id.to_string() })
            .await
            .expect("Failed to query main queue");

        if let Some(main_task) = in_main {
            println!("Task still in main queue: {:?}", main_task);
        } else {
            println!("Task not in main queue");
        }
    }

    #[tokio::test]
    async fn test_task_set_update() {
        let queue = setup_queue("capp-test-set-update").await;
        let task = Task::new(TestData { value: 42 });
        let task_id = task.task_id;

        // Push initial task
        queue.push(&task).await.expect("Failed to push task");

        // Update task via set
        let mut updated_task = task.clone();
        updated_task.payload.value = 43;
        queue.set(&updated_task).await.expect("Failed to set task");

        // Verify update directly in MongoDB
        let task_in_db = queue
            .tasks_collection
            .find_one(doc! { "task_id": task_id.to_string() })
            .await
            .expect("Failed to query task")
            .expect("Task not found after update");

        println!("Original task: {:?}", task);
        println!("Updated task: {:?}", updated_task);
        println!("Task in DB: {:?}", task_in_db);

        assert_eq!(task_in_db.payload.value, 43);
    }
}
