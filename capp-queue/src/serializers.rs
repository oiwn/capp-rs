use super::{Task, TaskQueueError};
use serde::{de::DeserializeOwned, Serialize};

pub trait TaskSerializer: Send + Sync {
    fn serialize_task<T>(task: &Task<T>) -> Result<Vec<u8>, TaskQueueError>
    where
        T: Clone + Serialize + DeserializeOwned;

    fn deserialize_task<T>(data: &[u8]) -> Result<Task<T>, TaskQueueError>
    where
        T: Clone + Serialize + DeserializeOwned;
}

#[derive(Debug, Clone, Copy)]
pub struct JsonSerializer;

impl TaskSerializer for JsonSerializer {
    fn serialize_task<T>(task: &Task<T>) -> Result<Vec<u8>, TaskQueueError>
    where
        T: Serialize + DeserializeOwned + Clone,
    {
        serde_json::to_vec(task)
            .map_err(|e| TaskQueueError::Serialization(e.to_string()))
    }

    fn deserialize_task<T>(data: &[u8]) -> Result<Task<T>, TaskQueueError>
    where
        T: Serialize + DeserializeOwned + Clone,
    {
        serde_json::from_slice(data)
            .map_err(|e| TaskQueueError::Deserialization(e.to_string()))
    }
}

#[cfg(feature = "mongodb")]
use mongodb::bson::{self};

#[cfg(feature = "mongodb")]
#[derive(Debug, Clone, Copy)]
pub struct BsonSerializer;

#[cfg(feature = "mongodb")]
impl TaskSerializer for BsonSerializer {
    fn serialize_task<T>(task: &Task<T>) -> Result<Vec<u8>, TaskQueueError>
    where
        T: Serialize + DeserializeOwned + Clone,
    {
        // Convert to BSON document first
        let mut doc = bson::to_document(task)
            .map_err(|e| TaskQueueError::Serialization(e.to_string()))?;

        // Move task_id to _id
        if let Some(task_id) = doc.remove("task_id") {
            doc.insert("_id", task_id);
        }

        // Serialize complete BSON document to bytes
        bson::to_vec(&doc).map_err(|e| TaskQueueError::Serialization(e.to_string()))
    }

    fn deserialize_task<T>(data: &[u8]) -> Result<Task<T>, TaskQueueError>
    where
        T: Serialize + DeserializeOwned + Clone,
    {
        // First get BSON document from bytes
        let mut doc: bson::Document = bson::from_slice(data)
            .map_err(|e| TaskQueueError::Deserialization(e.to_string()))?;

        if let Some(id) = doc.remove("_id") {
            doc.insert("task_id", id);
        }

        // Convert complete document back to Task
        bson::from_document(doc)
            .map_err(|e| TaskQueueError::Deserialization(e.to_string()))
    }
}
