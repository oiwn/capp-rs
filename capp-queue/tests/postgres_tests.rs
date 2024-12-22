#[cfg(test)]
mod tests {
    use capp_queue::{
        JsonSerializer, PostgresTaskQueue, Task, TaskQueue, TaskQueueError,
    };
    use serde::{Deserialize, Serialize};
    use serial_test::serial;
    use sqlx::{Executor, PgPool, Row};
    use std::sync::Arc;

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    struct TestData {
        value: u32,
    }

    async fn get_db_connection() -> String {
        dotenvy::dotenv().ok();
        std::env::var("DATABASE_URL").expect("Set DATABASE_URL env variable")
    }

    async fn cleanup_database(pool: &PgPool) -> Result<(), sqlx::Error> {
        pool.execute("TRUNCATE TABLE tasks, dlq").await?;
        Ok(())
    }

    #[tokio::test]
    #[serial]
    async fn test_database_cleanup() {
        let queue = PostgresTaskQueue::<TestData, JsonSerializer>::new(
            &get_db_connection().await,
        )
        .await
        .expect("Failed to create PostgresTaskQueue");

        cleanup_database(&queue.pool)
            .await
            .expect("Failed to clean database");

        // Verify clean state
        let count = sqlx::query!("SELECT COUNT(*) as count FROM tasks")
            .fetch_one(&queue.pool)
            .await
            .expect("Failed to count tasks");
        assert_eq!(
            count.count.unwrap(),
            0,
            "Database should be empty after cleanup"
        );
    }

    #[tokio::test]
    #[serial]
    async fn test_push() {
        let queue = PostgresTaskQueue::<TestData, JsonSerializer>::new(
            &get_db_connection().await,
        )
        .await
        .expect("Failed to create PostgresTaskQueue");

        // Clean up before test
        cleanup_database(&queue.pool)
            .await
            .expect("Failed to clean database");

        // Create and push a task
        let task = Task::new(TestData { value: 42 });
        let task_id = task.task_id;
        queue.push(&task).await.expect("Failed to push task");

        // Verify task was stored correctly
        let row =
            sqlx::query!("SELECT payload FROM tasks WHERE id = $1", task_id.get(),)
                .fetch_one(&queue.pool)
                .await
                .expect("Failed to fetch task");

        let stored_payload: serde_json::Value = row.payload;
        assert_eq!(stored_payload["payload"]["value"], 42);
    }

    #[tokio::test]
    #[serial]
    async fn test_push_and_pop_fifo() {
        let queue = PostgresTaskQueue::<TestData, JsonSerializer>::new(
            &get_db_connection().await,
        )
        .await
        .expect("Failed to create PostgresTaskQueue");

        // Clean up before test
        cleanup_database(&queue.pool)
            .await
            .expect("Failed to clean database");

        // Push multiple tasks in sequence
        let tasks = vec![
            Task::new(TestData { value: 1 }),
            Task::new(TestData { value: 2 }),
            Task::new(TestData { value: 3 }),
        ];

        for task in &tasks {
            queue.push(task).await.expect("Failed to push task");
            // Add small delay to ensure different queued_at times
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        }

        // Pop tasks and verify order
        for expected_value in 1..=3 {
            let popped_task = queue.pop().await.expect("Failed to pop task");
            assert_eq!(
                popped_task.payload.value, expected_value,
                "Tasks should be popped in FIFO order"
            );
        }

        // Verify queue is empty
        match queue.pop().await {
            Err(TaskQueueError::QueueEmpty) => {}
            _ => panic!("Queue should be empty"),
        }
    }

    #[tokio::test]
    #[serial]
    async fn test_concurrent_pop() {
        let queue = Arc::new(
            PostgresTaskQueue::<TestData, JsonSerializer>::new(
                &get_db_connection().await,
            )
            .await
            .expect("Failed to create PostgresTaskQueue"),
        );

        // Clean up before test
        cleanup_database(&queue.pool)
            .await
            .expect("Failed to clean database");

        // Push several tasks
        for i in 1..=5 {
            let task = Task::new(TestData { value: i });
            queue.push(&task).await.expect("Failed to push task");
        }

        // Try to pop tasks concurrently
        let mut handles = vec![];
        for _ in 0..5 {
            let queue_clone = queue.clone();
            let handle = tokio::spawn(async move { queue_clone.pop().await });
            handles.push(handle);
        }

        // Collect results
        let mut popped_values = vec![];
        for handle in handles {
            if let Ok(Ok(task)) = handle.await {
                popped_values.push(task.payload.value);
            }
        }

        // Sort values to check we got all unique tasks
        popped_values.sort();
        assert_eq!(popped_values, vec![1, 2, 3, 4, 5]);

        // Verify queue is empty
        match queue.pop().await {
            Err(TaskQueueError::QueueEmpty) => {}
            _ => panic!("Queue should be empty"),
        }
    }

    #[tokio::test]
    #[serial]
    async fn test_set() {
        let queue = PostgresTaskQueue::<TestData, JsonSerializer>::new(
            &get_db_connection().await,
        )
        .await
        .expect("Failed to create PostgresTaskQueue");

        // Clean up before test
        cleanup_database(&queue.pool)
            .await
            .expect("Failed to clean database");

        // Create and push initial task
        let mut task = Task::new(TestData { value: 42 });
        queue.push(&task).await.expect("Failed to push task");

        // Modify task state
        task.set_in_progress();
        task.payload.value = 43;

        // Update task in database
        queue.set(&task).await.expect("Failed to set task");

        // Verify changes were saved correctly using regular query
        let row = sqlx::query(
            r#"
            SELECT payload, status::text as status
            FROM tasks 
            WHERE id = $1
            "#,
        )
        .bind(task.task_id.get())
        .fetch_one(&queue.pool)
        .await
        .expect("Failed to fetch task");

        let payload: serde_json::Value = row.get("payload");
        let status: String = row.get("status");

        let stored_payload: TestData =
            serde_json::from_value(payload["payload"].clone())
                .expect("Failed to deserialize payload");

        assert_eq!(stored_payload.value, 43, "Payload should be updated");
        assert_eq!(status, "InProgress", "Status should be updated");

        // Test setting non-existent task
        let non_existent_task = Task::new(TestData { value: 99 });
        match queue.set(&non_existent_task).await {
            Err(TaskQueueError::TaskNotFound(_)) => {}
            other => panic!("Expected TaskNotFound error, got {:?}", other),
        }

        cleanup_database(&queue.pool)
            .await
            .expect("Failed to clean database");
    }
}
