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
