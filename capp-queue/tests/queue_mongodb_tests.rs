#[cfg(feature = "mongodb")]
#[cfg(test)]
mod tests {
    use capp_queue::{
        BsonSerializer, MongoTaskQueue, Task, TaskId, TaskQueue, TaskQueueError,
        TaskSerializer, TaskStatus,
    };
    use dotenvy::dotenv;
    use mongodb::bson::{self, doc};
    use mongodb::{Client, Database, options::ClientOptions};
    use serde::{Deserialize, Serialize};
    use std::time::Duration;

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    struct TestData {
        value: u32,
    }

    async fn get_mongodb() -> Database {
        dotenv().ok();
        let uri =
            std::env::var("MONGODB_URI").expect("Set MONGODB_URI env variable");
        let client_options = ClientOptions::parse(&uri)
            .await
            .expect("Failed to parse options");
        let client = Client::with_options(client_options.clone())
            .expect("Failed to create client");
        let db_name = client_options
            .default_database
            .as_ref()
            .expect("No database specified");
        client.database(db_name)
    }

    async fn verify_collection_exists(
        db: &Database,
        collection_name: &str,
    ) -> bool {
        let collections = db.list_collection_names().await.unwrap();
        collections.contains(&collection_name.to_string())
    }

    async fn cleanup_collections(name: &str) -> Result<(), mongodb::error::Error> {
        let db = get_mongodb().await;
        let tasks_collection_name = format!("{}_tasks", name);
        let dlq_collection_name = format!("{}_dlq", name);

        // Check if collections exist before dropping
        if verify_collection_exists(&db, &tasks_collection_name).await {
            tracing::info!("Dropping collection: {}", tasks_collection_name);
            db.collection::<Task<TestData>>(&tasks_collection_name)
                .drop()
                .await?;
        }

        if verify_collection_exists(&db, &dlq_collection_name).await {
            tracing::info!("Dropping collection: {}", dlq_collection_name);
            db.collection::<Task<TestData>>(&dlq_collection_name)
                .drop()
                .await?;
        }

        Ok(())
    }

    async fn setup_queue(name: &str) -> MongoTaskQueue<TestData, BsonSerializer> {
        if let Err(e) = cleanup_collections(name).await {
            tracing::error!("Cleanup failed: {:?}", e);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;

        let db = get_mongodb().await;

        MongoTaskQueue::new(db, name)
            .await
            .expect("Failed to create MongoTaskQueue")
    }

    #[tokio::test]
    async fn test_push() {
        // Setup test queue
        let queue_name = "test_push";
        let task = Task::new(TestData { value: 1 });

        // Get connection string and create database instance
        let db = get_mongodb().await;
        let queue = MongoTaskQueue::<TestData, BsonSerializer>::new(db, queue_name)
            .await
            .expect("Failed to create MongoTaskQueue");

        // Push task
        queue.push(&task).await.expect("Failed to push task");

        // Verify task was stored correctly
        let doc = queue
            .tasks_collection
            .find_one(doc! { "_id": task.task_id.get().to_string() })
            .await
            .expect("Failed to query")
            .expect("Document not found");

        // Verify document structure
        assert!(doc.contains_key("_id"), "Document should have _id field");
        assert!(
            !doc.contains_key("task_id"),
            "Document should not have task_id field"
        );
    }

    #[tokio::test]
    async fn test_push_and_pop() {
        let queue_name = "test_push_pop";
        let db = get_mongodb().await;

        let queue = MongoTaskQueue::<TestData, BsonSerializer>::new(db, queue_name)
            .await
            .expect("Failed to create MongoTaskQueue");

        let task = Task::new(TestData { value: 42 });
        let original_id = task.task_id;

        // Push task
        queue.push(&task).await.expect("Failed to push task");

        // Pop and verify
        let popped_task = queue.pop().await.expect("Failed to pop task");
        assert_eq!(popped_task.task_id, original_id);
        assert_eq!(popped_task.payload.value, 42);

        // Verify queue is empty
        match queue.pop().await {
            Err(TaskQueueError::QueueEmpty) => {}
            _ => panic!("Queue should be empty"),
        }
    }

    #[tokio::test]
    async fn test_pop_status_handling() {
        let queue_name = "test_pop_status";
        let db = get_mongodb().await;
        let queue = MongoTaskQueue::<TestData, BsonSerializer>::new(db, queue_name)
            .await
            .expect("Failed to create MongoTaskQueue");

        // Push a task
        let task = Task::new(TestData { value: 42 });
        let task_id = task.task_id;
        queue.push(&task).await.expect("Failed to push task");

        // Pop the task
        let popped_task = queue.pop().await.expect("Failed to pop task");
        assert_eq!(popped_task.task_id, task_id);
        assert_eq!(popped_task.status, TaskStatus::InProgress);

        // Verify task still exists in DB with updated status
        let doc = queue
            .tasks_collection
            .find_one(doc! { "_id": task_id.get().to_string() })
            .await
            .expect("Failed to query")
            .expect("Document should still exist");

        let bytes = bson::to_vec(&doc).expect("Failed to serialize BSON");
        let stored_task: Task<TestData> = BsonSerializer::deserialize_task(&bytes)
            .expect("Failed to deserialize task");

        assert_eq!(stored_task.status, TaskStatus::InProgress);

        // Try to pop again - should be empty since task is InProgress
        match queue.pop().await {
            Err(TaskQueueError::QueueEmpty) => {}
            other => panic!("Expected QueueEmpty error, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_push_pop_order() {
        let queue_name = "test_push_pop_order";
        let db = get_mongodb().await;
        let queue = MongoTaskQueue::<TestData, BsonSerializer>::new(db, queue_name)
            .await
            .expect("Failed to create MongoTaskQueue");

        // Push multiple tasks in sequence
        let tasks = vec![
            Task::new(TestData { value: 1 }),
            Task::new(TestData { value: 2 }),
            Task::new(TestData { value: 3 }),
        ];

        for task in &tasks {
            queue.push(task).await.expect("Failed to push task");
        }

        // Pop tasks and verify order
        for expected_value in 1..=3 {
            let popped_task = queue.pop().await.expect("Failed to pop task");
            assert_eq!(
                popped_task.payload.value, expected_value,
                "Tasks should be popped in FIFO order. Expected value {}, got {}",
                expected_value, popped_task.payload.value
            );
        }

        // Verify queue is empty
        match queue.pop().await {
            Err(TaskQueueError::QueueEmpty) => {}
            _ => panic!("Queue should be empty"),
        }
    }

    #[tokio::test]
    async fn test_ack() {
        let queue_name = "test_ack";
        let db = get_mongodb().await;
        let queue = MongoTaskQueue::<TestData, BsonSerializer>::new(db, queue_name)
            .await
            .expect("Failed to create MongoTaskQueue");

        // Push and pop a task
        let task = Task::new(TestData { value: 42 });
        let task_id = task.task_id;

        queue.push(&task).await.expect("Failed to push task");
        let popped_task = queue.pop().await.expect("Failed to pop task");

        // Ack the task
        queue
            .ack(&popped_task.task_id)
            .await
            .expect("Failed to ack task");

        // Verify task is gone by trying to find it
        let doc = queue
            .tasks_collection
            .find_one(doc! { "_id": task_id.get().to_string() })
            .await
            .expect("Failed to query");

        assert!(doc.is_none(), "Task should be removed after ack");

        // Try to ack non-existent task
        let non_existent_id = TaskId::new();
        match queue.ack(&non_existent_id).await {
            Err(TaskQueueError::TaskNotFound(_)) => {}
            other => panic!("Expected TaskNotFound error, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_set() {
        let queue_name = "test_set";
        let db = get_mongodb().await;
        let queue = MongoTaskQueue::<TestData, BsonSerializer>::new(db, queue_name)
            .await
            .expect("Failed to create MongoTaskQueue");

        // Create and push initial task
        let mut task = Task::new(TestData { value: 42 });
        queue.push(&task).await.expect("Failed to push task");

        // Modify task state
        task.set_in_progress();
        task.payload.value = 43;

        // Update task in database
        queue.set(&task).await.expect("Failed to set task");

        // Verify changes were saved
        let doc = queue
            .tasks_collection
            .find_one(doc! { "_id": task.task_id.get().to_string() })
            .await
            .expect("Failed to query")
            .expect("Document not found");

        let bytes = bson::to_vec(&doc).expect("Failed to serialize BSON");
        let updated_task: Task<TestData> = BsonSerializer::deserialize_task(&bytes)
            .expect("Failed to deserialize task");

        assert_eq!(updated_task.payload.value, 43);
        assert_eq!(updated_task.status, TaskStatus::InProgress);

        // Test setting non-existent task
        let non_existent_task = Task::new(TestData { value: 99 });
        match queue.set(&non_existent_task).await {
            Err(TaskQueueError::TaskNotFound(_)) => {}
            other => panic!("Expected TaskNotFound error, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_inspect_task_storage() {
        println!("Test inspect task storage");
        let queue_name = "test_inspect";
        let queue = setup_queue(queue_name).await;

        // Create and push a single task
        let task = Task::new(TestData { value: 42 });
        let task_id = task.task_id;
        queue.push(&task).await.expect("Failed to push task");

        // Get raw document and print its BSON format for inspection
        let raw_doc = queue
            .tasks_collection
            .find_one(doc! {})
            .await
            .expect("Failed to query")
            .expect("Document should exist");

        // Convert to BSON document to inspect actual format
        let bson_doc = mongodb::bson::to_document(&raw_doc)
            .expect("Failed to convert to BSON");
        println!("Raw BSON document in MongoDB: {:#?}", bson_doc);

        // Get the task_id field and print its specific type and value
        if let Some(task_id_bson) = bson_doc.get("task_id") {
            println!("task_id type: {:?}", task_id_bson.element_type());
            println!("task_id value: {:?}", task_id_bson);
        }

        // Print the UUID we're searching with
        println!("Original UUID bytes: {:?}", task_id.get().as_bytes());

        // Try finding with direct BSON query
        let found = queue
            .tasks_collection
            .find_one(bson_doc)
            .await
            .expect("Query failed");
        println!("Found with exact BSON document: {:?}", found.is_some());
    }

    #[tokio::test]
    async fn test_nack() {
        let queue_name = "test_nack";
        let db = get_mongodb().await;
        let queue = MongoTaskQueue::<TestData, BsonSerializer>::new(db, queue_name)
            .await
            .expect("Failed to create MongoTaskQueue");

        // Push and pop a task
        let task = Task::new(TestData { value: 42 });
        let task_id = task.task_id;

        queue.push(&task).await.expect("Failed to push task");
        let popped_task = queue.pop().await.expect("Failed to pop task");

        // Nack the task
        queue.nack(&popped_task).await.expect("Failed to nack task");

        // Verify task is gone from main queue
        let main_doc = queue
            .tasks_collection
            .find_one(doc! { "_id": task_id.get().to_string() })
            .await
            .expect("Failed to query main queue");
        assert!(main_doc.is_none(), "Task should be removed from main queue");

        // Verify task is in DLQ with correct status
        let dlq_doc = queue
            .dlq_collection
            .find_one(doc! { "_id": task_id.get().to_string() })
            .await
            .expect("Failed to query DLQ")
            .expect("Task should be in DLQ");

        let bytes = bson::to_vec(&dlq_doc).expect("Failed to serialize BSON");
        let dlq_task: Task<TestData> = BsonSerializer::deserialize_task(&bytes)
            .expect("Failed to deserialize task");

        assert_eq!(dlq_task.task_id, task_id);
        assert_eq!(dlq_task.status, TaskStatus::DeadLetter);
        assert!(dlq_task.error_msg.is_some());
    }

    #[tokio::test]
    async fn test_set_with_uuid_string() {
        let queue_name = "test_set_uuid_string";
        let db = get_mongodb().await;
        let queue = MongoTaskQueue::<TestData, BsonSerializer>::new(db, queue_name)
            .await
            .expect("Failed to create MongoTaskQueue");

        // Create and push initial task
        let mut task = Task::new(TestData { value: 100 });
        let task_id = task.task_id;
        queue.push(&task).await.expect("Failed to push task");

        // Verify task was stored with _id as string
        let _doc = queue
            .tasks_collection
            .find_one(doc! { "_id": task.task_id.as_string() })
            .await
            .expect("Failed to query")
            .expect("Document not found");

        // Modify task state and value
        task.payload.value = 200;
        task.set_in_progress();

        // Update task in database using set method
        queue.set(&task).await.expect("Failed to set task");

        // Verify changes were saved by querying directly with the task_id as string
        let updated_doc = queue
            .tasks_collection
            .find_one(doc! { "_id": task_id.as_string() })
            .await
            .expect("Failed to query")
            .expect("Document not found after update");

        // Convert to Task object to check values
        let bytes = bson::to_vec(&updated_doc).expect("Failed to serialize BSON");
        let updated_task: Task<TestData> = BsonSerializer::deserialize_task(&bytes)
            .expect("Failed to deserialize task");

        // Verify updates were saved correctly
        assert_eq!(updated_task.payload.value, 200);
        assert_eq!(updated_task.status, TaskStatus::InProgress);
        assert_eq!(updated_task.task_id, task_id);
    }
}
