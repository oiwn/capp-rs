#[cfg(test)]
mod tests {
    use capp_queue::queue::{MongoTaskQueue, TaskQueue};
    use capp_queue::task::Task;
    use dotenvy::dotenv;
    use futures_util::StreamExt;
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
        // ensure cleanup is complete
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
    async fn test_push() {
        tracing_subscriber::fmt::init();

        // Setup
        let queue_name = "test-push";
        let queue = setup_queue(queue_name).await;
        let test_value = 42;
        let task = Task::new(TestData { value: test_value });
        let task_id = task.task_id;

        // Push task
        queue.push(&task).await.expect("Failed to push task");

        // Debug: Print collection contents
        let all_docs = queue
            .tasks_collection
            .find(doc! {})
            .await
            .expect("Failed to query collection")
            .collect::<Vec<_>>()
            .await;

        tracing::info!("Collection contents: {:?}", all_docs);

        // Verify task exists using find with no filter first
        let result = queue
            .tasks_collection
            .find_one(doc! {})
            .await
            .expect("Failed to query task");

        assert!(result.is_some(), "Collection should not be empty");

        // Now try to find specific task using proper BSON UUID
        let result = queue
            .tasks_collection
            .find_one(doc! {
                "task_id": mongodb::bson::Binary {
                    subtype: mongodb::bson::spec::BinarySubtype::Uuid,
                    bytes: task_id.get().as_bytes().to_vec(),
                }
            })
            .await
            .expect("Failed to query task");

        assert!(result.is_some(), "Task should exist in collection");

        // Verify task data
        if let Some(stored_task) = result {
            assert_eq!(
                stored_task.payload.value, test_value,
                "Task payload should match"
            );
            assert_eq!(stored_task.task_id, task_id, "Task ID should match");
        }

        // Cleanup
        cleanup_collections(queue_name)
            .await
            .expect("Cleanup failed");
    }

    #[tokio::test]
    async fn test_collection_setup() {
        let queue_name = "test-setup";
        let uri = get_mongo_connection().await;
        let client_options = ClientOptions::parse(&uri)
            .await
            .expect("Failed to parse MongoDB options");

        let db_name = client_options
            .default_database
            .as_ref()
            .expect("No database specified in MongoDB URI")
            .clone();

        // Create queue
        let _queue = setup_queue(queue_name).await;

        // Verify collections exist
        let client = Client::with_options(client_options)
            .expect("Failed to create MongoDB client");

        assert!(
            verify_collection_exists(
                &client,
                &db_name,
                &format!("{}_tasks", queue_name)
            )
            .await,
            "Tasks collection should exist"
        );
        assert!(
            verify_collection_exists(
                &client,
                &db_name,
                &format!("{}_dlq", queue_name)
            )
            .await,
            "DLQ collection should exist"
        );

        // Cleanup
        cleanup_collections(queue_name)
            .await
            .expect("Cleanup failed");
    }
}
