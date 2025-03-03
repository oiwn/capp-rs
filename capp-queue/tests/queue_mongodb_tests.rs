#[cfg(feature = "mongodb")]
#[cfg(test)]
mod tests {
    use capp_queue::{
        BsonSerializer, MongoTaskQueue, Task, TaskId, TaskQueue, TaskQueueError,
        TaskStatus,
    };
    use chrono::Utc;
    use dotenvy::dotenv;
    use fake::{Dummy, Faker};
    use mongodb::bson::doc;
    use mongodb::{Client, Database, options::ClientOptions};
    use serde::{Deserialize, Serialize};
    use std::{collections::HashSet, time::Duration};

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Dummy)]
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

    #[allow(dead_code)]
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
        let queue_name = "queue_push";
        let task = Task::new(TestData { value: 42 });

        cleanup_collections(queue_name).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;

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
        assert!(
            doc.task_id.contains('-'),
            "Document should have task_id which store something like uuid"
        );

        // Verify task details
        assert_eq!(doc.payload.value, 42);
        assert_eq!(doc.status, TaskStatus::Queued);
        assert!(doc.started_at.is_none());
        assert!(doc.finished_at.is_none());
        assert_eq!(doc.retries, 0);
        assert!(doc.error_msg.is_none());

        // Verify timestamp fields
        assert!(
            doc.queued_at.timestamp() > 0,
            "Queued timestamp should be set"
        );

        // Optional: More detailed timestamp checks
        let now = Utc::now();
        assert!(
            doc.queued_at <= now
                && doc.queued_at > now - chrono::Duration::minutes(1),
            "Queued timestamp should be recent"
        );
    }

    #[tokio::test]
    async fn test_push_and_pop() {
        let queue_name = "queue_push_pop";

        cleanup_collections(queue_name).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;

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
        let queue_name = "pop_status";

        cleanup_collections(queue_name).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;

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

        let stored_task: Task<TestData> = doc.into();
        assert_eq!(stored_task.status, TaskStatus::InProgress);

        // Try to pop again - should be empty since task is InProgress
        match queue.pop().await {
            Err(TaskQueueError::QueueEmpty) => {}
            other => panic!("Expected QueueEmpty error, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_push_pop_order() {
        let queue_name = "push_pop_order";

        cleanup_collections(queue_name).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;

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
        let queue_name = "ack";

        cleanup_collections(queue_name).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;

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
        let queue_name = "set";

        cleanup_collections(queue_name).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;

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

        let updated_task: Task<TestData> = doc.into();

        assert_eq!(updated_task.payload.value, 43);
        assert_eq!(updated_task.status, TaskStatus::InProgress);

        // Test setting non-existent task
        let non_existent_task = Task::new(TestData { value: 99 });
        match queue.set(&non_existent_task).await {
            Err(TaskQueueError::TaskNotFound(_)) => {}
            other => panic!("Expected TaskNotFound error, got {:?}", other),
        }
    }

    /*
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
    */

    #[tokio::test]
    async fn test_nack() {
        let queue_name = "nack";

        cleanup_collections(queue_name).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;

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

        let dlq_task: Task<TestData> = dlq_doc.into();

        assert_eq!(dlq_task.task_id, task_id);
        assert_eq!(dlq_task.status, TaskStatus::DeadLetter);
        assert!(dlq_task.error_msg.is_some());
    }

    /*
    #[tokio::test]
    async fn test_concurrent_task_access_race_condition() {
        let db = get_mongodb().await;
        let queue_name = "test_concurrent_race";

        // Clean up any existing data
        cleanup_collections(queue_name).await.unwrap();

        // Create a queue
        let queue =
            MongoTaskQueue::<TestData, BsonSerializer>::new(db.clone(), queue_name)
                .await
                .unwrap();

        // Create a single task
        let task = Task::new(TestData { value: 100 });
        let task_id = task.task_id;
        queue.push(&task).await.unwrap();

        // Verify task was added by trying to fetch it
        let task_exists = queue
            .tasks_collection
            .count_documents(doc! { "_id": task_id.as_string() })
            .await
            .unwrap();
        assert_eq!(task_exists, 1, "Task should exist in the collection");

        // Simulate multiple workers trying to process the same task concurrently
        let mut handles = vec![];
        let worker_count = 3;

        for worker_id in 0..worker_count {
            let queue_clone = queue.clone();
            let handle = tokio::spawn(async move {
                // Worker process:
                // 1. Pop a task
                if let Ok(popped_task) = queue_clone.pop().await {
                    println!(
                        "Worker {} popped task {}",
                        worker_id, popped_task.task_id
                    );

                    // 2. Process the task (simulate work)
                    tokio::time::sleep(Duration::from_millis(50)).await;

                    // 3. Try to ack the task
                    let ack_result = queue_clone.ack(&popped_task.task_id).await;
                    if let Err(e) = &ack_result {
                        println!("Worker {} ack error: {:?}", worker_id, e);
                    }

                    // Return task ID and ack result
                    (worker_id, popped_task.task_id, ack_result)
                } else {
                    (worker_id, TaskId::new(), Err(TaskQueueError::QueueEmpty))
                }
            });
            handles.push(handle);
        }

        // Wait for all workers to complete
        let mut results = vec![];
        for handle in handles {
            let result = handle.await.unwrap();
            results.push(result);
        }

        // Analyze results
        let successful_acks = results
            .iter()
            .filter(|(_, _, result)| result.is_ok())
            .count();

        let task_not_found_errors = results
            .iter()
            .filter(|(_, _, result)| {
                matches!(result, Err(TaskQueueError::TaskNotFound(_)))
            })
            .count();

        // Only one worker should succeed, others should get TaskNotFound
        assert_eq!(
            successful_acks, 1,
            "Only one worker should successfully ack the task"
        );
        assert_eq!(
            task_not_found_errors,
            worker_count - 1,
            "Other workers should get TaskNotFound errors"
        );

        // The task should no longer be in the queue
        let task_exists = queue
            .tasks_collection
            .count_documents(doc! { "_id": task_id.as_string() })
            .await
            .unwrap();
        assert_eq!(
            task_exists, 0,
            "Task should no longer exist in the collection"
        );

        // Clean up
        cleanup_collections(queue_name).await.unwrap();
    }

    #[tokio::test]
    async fn test_duplicate_key_race_condition() {
        let db = get_mongodb().await;
        let queue_name = "test_duplicate_key_race";

        // Clean up any existing data
        cleanup_collections(queue_name).await.unwrap();

        // Create a queue
        let queue =
            MongoTaskQueue::<TestData, BsonSerializer>::new(db.clone(), queue_name)
                .await
                .unwrap();

        // Create a single task
        let task = Task::new(TestData { value: 100 });
        let task_id = task.task_id;
        queue.push(&task).await.unwrap();

        // Verify task was added
        let task_exists = queue
            .tasks_collection
            .count_documents(doc! { "_id": task_id.as_string() })
            .await
            .unwrap();
        assert_eq!(task_exists, 1, "Task should exist in the collection");

        // Simulate multiple workers popping the same task and trying to modify it concurrently
        let queue1 = queue.clone();
        let queue2 = queue.clone();
        let queue3 = queue.clone();

        // Worker 1: Pop the task
        let task1 = queue1.pop().await.unwrap();

        // Worker 2: Also try to pop the same task
        let task2_future = async {
            // Use raw MongoDB operations to find and modify the task directly
            // This simulates another worker getting the same task
            let doc = queue2
                .tasks_collection
                .find_one_and_update(
                    doc! { "_id": task_id.as_string() },
                    doc! { "$set": { "status": "InProgress" } },
                )
                .await
                .unwrap();

            // Convert to Task
            if let Some(doc) = doc {
                let bytes = bson::to_vec(&doc).unwrap();
                BsonSerializer::deserialize_task(&bytes)
            } else {
                Err(TaskQueueError::TaskNotFound(task_id))
            }
        };

        let task2 = task2_future.await.unwrap();

        // Now both workers have the same task and think they own it
        assert_eq!(task1.task_id, task2.task_id);

        // Worker 1: Try to ack the task
        let ack_result = queue1.ack(&task1.task_id).await;
        println!("Worker 1 ack result: {:?}", ack_result);
        assert!(ack_result.is_ok());

        // Worker 2: Try to nack the task (should fail with TaskNotFound)
        let nack_result = queue2.nack(&task2).await;
        println!("Worker 2 nack result: {:?}", nack_result);
        assert!(matches!(nack_result, Err(TaskQueueError::TaskNotFound(_))));

        // Worker 3: Try to push the task again (should fail with duplicate key)
        let push_result = queue3.push(&task).await;
        println!("Worker 3 push result: {:?}", push_result);

        // Check if the error is a MongoDB duplicate key error
        if let Err(TaskQueueError::MongodbError(err)) = &push_result {
            assert!(err.to_string().contains("duplicate key error"));
        } else {
            panic!(
                "Expected MongoDB duplicate key error, got {:?}",
                push_result
            );
        }

        // Clean up
        cleanup_collections(queue_name).await.unwrap();
    }
    */

    #[tokio::test]
    async fn test_concurrent_task_pop() {
        let queue_name = "concurrent_pop";

        cleanup_collections(queue_name).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;

        let db = get_mongodb().await;
        let queue =
            MongoTaskQueue::<TestData, BsonSerializer>::new(db.clone(), queue_name)
                .await
                .unwrap();

        // Create multiple identical tasks
        let tasks: Vec<Task<TestData>> = (0..10)
            .map(|_| Task::new(TestData::dummy(&Faker)))
            .collect();

        // Push all tasks
        for task in &tasks {
            queue.push(task).await.unwrap();
        }

        // Simulate concurrent workers
        let handles: Vec<_> = (0..5)
            .map(|_| {
                let queue_clone = queue.clone();
                tokio::spawn(async move { queue_clone.pop().await })
            })
            .collect();

        // Collect results
        let results: Vec<_> = futures::future::join_all(handles)
            .await
            .into_iter()
            .filter_map(Result::ok)
            .collect();

        // Verify unique tasks were popped
        let unique_tasks: HashSet<_> = results
            .iter()
            .map(|task| task.as_ref().unwrap().task_id)
            .collect();

        assert_eq!(results.len(), 5, "Should pop exactly 5 unique tasks");
        assert_eq!(unique_tasks.len(), 5, "Should have 5 unique task IDs");

        // Verify remaining tasks
        let remaining_count = queue
            .tasks_collection
            .count_documents(doc! { "status": String::from(TaskStatus::Queued) })
            .await
            .unwrap();

        assert_eq!(remaining_count, 5, "Should have 5 remaining queued tasks");
    }
}
