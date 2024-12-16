#[cfg(test)]
mod tests {
    use capp_queue::queue::{MongoTaskQueue, TaskQueue};
    use capp_queue::task::Task;
    use dotenvy::dotenv;
    // use futures_util::StreamExt;
    use mongodb::bson::{self, doc};
    use mongodb::{options::ClientOptions, Client};
    use serde::{Deserialize, Serialize};
    use std::time::Duration;

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    struct TestData {
        value: u32,
    }

    async fn get_mongo_connection() -> String {
        dotenv().ok();
        std::env::var("MONGODB_URI").expect("Set MONGODB_URI env variable")
    }

    async fn verify_collection_exists(
        client: &Client,
        db_name: &str,
        collection_name: &str,
    ) -> bool {
        let db = client.database(db_name);
        let collections = db.list_collection_names().await.unwrap();
        collections.contains(&collection_name.to_string())
    }

    async fn cleanup_collections(name: &str) -> Result<(), mongodb::error::Error> {
        let uri = get_mongo_connection().await;
        let client_options = ClientOptions::parse(&uri).await?;
        let client = Client::with_options(client_options.clone())?;

        let db_name = client_options
            .default_database
            .as_ref()
            .expect("No database specified in MongoDB URI");

        let db = client.database(db_name);

        let tasks_collection_name = format!("{}_tasks", name);
        let dlq_collection_name = format!("{}_dlq", name);

        // Check if collections exist before dropping
        if verify_collection_exists(&client, db_name, &tasks_collection_name).await
        {
            tracing::info!("Dropping collection: {}", tasks_collection_name);
            db.collection::<Task<TestData>>(&tasks_collection_name)
                .drop()
                .await?;
        }

        if verify_collection_exists(&client, db_name, &dlq_collection_name).await {
            tracing::info!("Dropping collection: {}", dlq_collection_name);
            db.collection::<Task<TestData>>(&dlq_collection_name)
                .drop()
                .await?;
        }

        Ok(())
    }

    async fn setup_queue(name: &str) -> MongoTaskQueue<TestData> {
        if let Err(e) = cleanup_collections(name).await {
            tracing::error!("Cleanup failed: {:?}", e);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        let uri = get_mongo_connection().await;
        MongoTaskQueue::new(&uri, name)
            .await
            .expect("Failed to create MongoTaskQueue")
    }

    #[test]
    fn test_task_id_serde() {
        let task = Task::new(TestData { value: 42 });
        let bson_doc = bson::to_document(&task).expect("Failed to convert to BSON");
        let task_from_bson: Task<TestData> =
            bson::from_document(bson_doc).expect("Failed to convert from BSON");
        // Verify task_id survived the round trip
        assert_eq!(task.task_id, task_from_bson.task_id);
    }

    #[tokio::test]
    async fn test_queue_initialization() {
        let queue_name = "test_init";
        cleanup_collections(queue_name)
            .await
            .expect("Cleanup failed");

        // Ensure cleanup is complete
        tokio::time::sleep(Duration::from_millis(100)).await;

        let uri = get_mongo_connection().await;
        let queue = MongoTaskQueue::<TestData>::new(&uri, queue_name)
            .await
            .expect("Failed to create MongoTaskQueue");

        let client_options = ClientOptions::parse(&uri)
            .await
            .expect("Failed to parse MongoDB options");

        let db_name = client_options
            .default_database
            .as_ref()
            .expect("No database specified in MongoDB URI");

        // Verify collections were created
        let client = Client::with_options(client_options.clone())
            .expect("Failed to create MongoDB client");

        assert!(
            verify_collection_exists(
                &client,
                db_name,
                &format!("{}_tasks", queue_name)
            )
            .await,
            "Tasks collection should exist"
        );

        assert!(
            verify_collection_exists(
                &client,
                db_name,
                &format!("{}_dlq", queue_name)
            )
            .await,
            "DLQ collection should exist"
        );

        // Verify indexes were created using list_index_names()
        let index_names = queue
            .tasks_collection
            .list_index_names()
            .await
            .expect("Failed to get index names");

        // MongoDB always creates _id_ index by default, plus our task_id index
        assert_eq!(
            index_names.len(),
            2,
            "Should have 2 indexes (_id and task_id)"
        );
        assert!(
            index_names.iter().any(|name| name.contains("task_id")),
            "task_id index should exist"
        );

        // Cleanup after test
        cleanup_collections(queue_name)
            .await
            .expect("Cleanup failed");
    }

    #[tokio::test]
    async fn test_push_and_verify_format() {
        let queue_name = "test_push_format";
        let queue = setup_queue(queue_name).await;

        // Create and push a task
        let task = Task::new(TestData { value: 42 });
        let task_id = task.task_id;
        queue.push(&task).await.expect("Failed to push task");

        // Get raw document and inspect the BSON format
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

        // Cleanup
        cleanup_collections(queue_name)
            .await
            .expect("Cleanup failed");
    }

    #[tokio::test]
    async fn test_task_id_mongodb_format() {
        let queue_name = "test-id-format";
        let queue = setup_queue(queue_name).await;

        // Create and push a test task
        let task = Task::new(TestData { value: 42 });
        let task_id = task.task_id;
        queue.push(&task).await.expect("Failed to push task");

        // Get the raw BSON document to examine how it's actually stored
        let raw_doc = queue
            .tasks_collection
            .find_one(doc! {})
            .await
            .expect("Failed to query")
            .expect("Document should exist");

        // Get the raw document as BSON to inspect it
        let bson_doc = mongodb::bson::to_document(&raw_doc)
            .expect("Failed to convert to BSON");
        println!("Raw BSON document in MongoDB: {:#?}", bson_doc);

        // Specifically examine the task_id field
        let task_id_field =
            bson_doc.get("task_id").expect("task_id field should exist");
        println!("task_id type: {:?}", task_id_field.element_type());
        println!("task_id value: {:#?}", task_id_field);
        println!("Original UUID bytes: {:?}", task_id.get().as_bytes());

        // Try to find document with exact BSON match
        let found = queue
            .tasks_collection
            .find_one(doc! { "task_id": task_id.to_string() })
            .await
            .expect("Failed to query")
            .is_some();
        println!("Found with exact BSON document: {}", found);

        // Clean up
        cleanup_collections(queue_name)
            .await
            .expect("Cleanup failed");
    }

    #[tokio::test]
    async fn test_inspect_task_storage() {
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

        // No cleanup - leave the data in MongoDB for manual inspection
    }
}
