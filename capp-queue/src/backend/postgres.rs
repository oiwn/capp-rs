use crate::{Task, TaskId, TaskQueue, TaskQueueError, TaskSerializer, TaskStatus};
use async_trait::async_trait;
use serde::{de::DeserializeOwned, Serialize};
use sqlx::types::chrono::{DateTime, Utc};
use sqlx::PgPool;
use std::marker::PhantomData;

#[derive(sqlx::Type)]
#[sqlx(type_name = "task_status", rename_all = "PascalCase")]
pub enum PostgresTaskStatus {
    Queued,
    InProgress,
    Completed,
    Failed,
    DeadLetter,
}

// Add conversion between our TaskStatus and PostgresTaskStatus
impl From<TaskStatus> for PostgresTaskStatus {
    fn from(status: TaskStatus) -> Self {
        match status {
            TaskStatus::Queued => PostgresTaskStatus::Queued,
            TaskStatus::InProgress => PostgresTaskStatus::InProgress,
            TaskStatus::Completed => PostgresTaskStatus::Completed,
            TaskStatus::Failed => PostgresTaskStatus::Failed,
            TaskStatus::DeadLetter => PostgresTaskStatus::DeadLetter,
        }
    }
}

pub struct PostgresTaskQueue<D, S>
where
    S: TaskSerializer,
{
    pub pool: PgPool,
    _marker: PhantomData<(D, S)>,
}

impl<D, S> PostgresTaskQueue<D, S>
where
    D: Send + Sync + 'static,
    S: TaskSerializer + Send + Sync,
{
    pub async fn new(connection_string: &str) -> Result<Self, TaskQueueError> {
        let pool = PgPool::connect(connection_string)
            .await
            .map_err(TaskQueueError::PostgresError)?;

        Ok(Self {
            pool,
            _marker: PhantomData,
        })
    }
}

#[async_trait]
impl<D, S> TaskQueue<D> for PostgresTaskQueue<D, S>
where
    D: std::fmt::Debug
        + Clone
        + Serialize
        + DeserializeOwned
        + Send
        + Sync
        + 'static,
    S: TaskSerializer + Send + Sync,
{
    async fn push(&self, task: &Task<D>) -> Result<(), TaskQueueError> {
        // Serialize task data to JSON
        let task_bytes = S::serialize_task(task)?;
        let payload: serde_json::Value = serde_json::from_slice(&task_bytes)
            .map_err(|e| TaskQueueError::Serialization(e.to_string()))?;
        let queued_at = DateTime::<Utc>::from(task.queued_at);

        sqlx::query!(
            r#"
        INSERT INTO tasks (id, payload, status, queued_at)
        VALUES ($1, $2, 'Queued', $3)
        "#,
            task.task_id.get(),
            payload,
            queued_at,
        )
        .execute(&self.pool)
        .await
        .map_err(TaskQueueError::PostgresError)?;

        Ok(())
    }

    async fn pop(&self) -> Result<Task<D>, TaskQueueError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(TaskQueueError::PostgresError)?;

        let result = async {
        // Get oldest queued task and lock it
        let row = sqlx::query!(
            r#"
            SELECT id, payload, queued_at, started_at, finished_at, retries, error_msg
            FROM tasks 
            WHERE status = 'Queued'
            ORDER BY queued_at ASC
            FOR UPDATE SKIP LOCKED
            LIMIT 1
            "#
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(TaskQueueError::PostgresError)?;

        match row {
            Some(row) => {
                // Update status to InProgress
                let started_at = Utc::now();
                sqlx::query!(
                    r#"
                    UPDATE tasks 
                    SET status = 'InProgress', started_at = $1
                    WHERE id = $2
                    "#,
                    started_at,
                    row.id,
                )
                .execute(&mut *tx)
                .await
                .map_err(TaskQueueError::PostgresError)?;

                // Deserialize task
                let task_bytes = serde_json::to_vec(&row.payload)
                    .map_err(|e| TaskQueueError::Serialization(e.to_string()))?;

                let mut task = S::deserialize_task(&task_bytes)?;
                task.set_in_progress();
                task.started_at = Some(started_at.into());

                Ok(task)
            }
            None => Err(TaskQueueError::QueueEmpty),
        }
    }.await;

        // Either commit on success or rollback on error
        match result {
            Ok(task) => {
                tx.commit().await.map_err(TaskQueueError::PostgresError)?;
                Ok(task)
            }
            Err(e) => {
                tx.rollback().await.map_err(TaskQueueError::PostgresError)?;
                Err(e)
            }
        }
    }

    async fn ack(&self, task_id: &TaskId) -> Result<(), TaskQueueError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(TaskQueueError::PostgresError)?;

        let result =
            sqlx::query!(r#"DELETE FROM tasks WHERE id = $1"#, task_id.get(),)
                .execute(&mut *tx)
                .await
                .map_err(TaskQueueError::PostgresError)?;

        if result.rows_affected() == 0 {
            tx.rollback().await.map_err(TaskQueueError::PostgresError)?;
            return Err(TaskQueueError::TaskNotFound(*task_id));
        }

        tx.commit().await.map_err(TaskQueueError::PostgresError)?;
        Ok(())
    }

    async fn nack(&self, task: &Task<D>) -> Result<(), TaskQueueError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(TaskQueueError::PostgresError)?;

        // First move to DLQ
        let task_bytes = S::serialize_task(task)?;
        let payload: serde_json::Value = serde_json::from_slice(&task_bytes)
            .map_err(|e| TaskQueueError::Serialization(e.to_string()))?;

        sqlx::query!(
            r#"INSERT INTO dlq (id, payload, error_msg) VALUES ($1, $2, $3)"#,
            task.task_id.get(),
            payload,
            task.error_msg.as_deref(),
        )
        .execute(&mut *tx)
        .await
        .map_err(TaskQueueError::PostgresError)?;

        // Then remove from tasks
        sqlx::query!(r#"DELETE FROM tasks WHERE id = $1"#, task.task_id.get(),)
            .execute(&mut *tx)
            .await
            .map_err(TaskQueueError::PostgresError)?;

        tx.commit().await.map_err(TaskQueueError::PostgresError)?;
        Ok(())
    }

    async fn set(&self, task: &Task<D>) -> Result<(), TaskQueueError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(TaskQueueError::PostgresError)?;

        let task_bytes = S::serialize_task(task)?;
        let payload: serde_json::Value = serde_json::from_slice(&task_bytes)
            .map_err(|e| TaskQueueError::Serialization(e.to_string()))?;

        let status: PostgresTaskStatus = task.status.clone().into();

        let result = sqlx::query(
            r#"
        UPDATE tasks 
        SET payload = $1,
            status = $2,
            started_at = $3,
            finished_at = $4,
            retries = $5,
            error_msg = $6
        WHERE id = $7
        "#,
        )
        .bind(payload)
        .bind(status) // Now we're binding a proper Postgres enum type
        .bind(task.started_at.map(DateTime::<Utc>::from))
        .bind(task.finished_at.map(DateTime::<Utc>::from))
        .bind(task.retries as i32)
        .bind(&task.error_msg)
        .bind(task.task_id.get())
        .execute(&mut *tx)
        .await
        .map_err(TaskQueueError::PostgresError)?;

        if result.rows_affected() == 0 {
            tx.rollback().await.map_err(TaskQueueError::PostgresError)?;
            return Err(TaskQueueError::TaskNotFound(task.task_id));
        }

        tx.commit().await.map_err(TaskQueueError::PostgresError)?;
        Ok(())
    }
}
