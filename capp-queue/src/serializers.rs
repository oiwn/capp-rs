use super::{Task, TaskQueueError};
use serde::{Serialize, de::DeserializeOwned};

// Marker traits for backend compatibility
pub trait InMemoryCompatible {}

// Mark which backends JsonSerializer is compatible with
impl InMemoryCompatible for JsonSerializer {}

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

#[cfg(test)]
mod tests {
    use super::{JsonSerializer, TaskSerializer};
    use crate::{Task, TaskQueueError};
    use serde::{Deserialize, Serialize, Serializer};

    #[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
    struct Payload {
        value: u32,
    }

    #[derive(Clone, Debug, Deserialize)]
    struct FailingPayload;

    impl Serialize for FailingPayload {
        fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            Err(serde::ser::Error::custom("boom"))
        }
    }

    #[test]
    fn json_serializer_round_trips_task() {
        let task = Task::new(Payload { value: 42 });
        let encoded = JsonSerializer::serialize_task(&task).unwrap();
        let decoded =
            JsonSerializer::deserialize_task::<Payload>(&encoded).unwrap();

        assert_eq!(decoded.payload, task.payload);
        assert_eq!(decoded.status, task.status);
    }

    #[test]
    fn json_serializer_reports_serialization_errors() {
        let task = Task::new(FailingPayload);
        let error = JsonSerializer::serialize_task(&task).unwrap_err();

        match error {
            TaskQueueError::Serialization(msg) => assert!(msg.contains("boom")),
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn json_serializer_reports_deserialization_errors() {
        let error = JsonSerializer::deserialize_task::<Payload>(b"not valid json")
            .unwrap_err();

        match error {
            TaskQueueError::Deserialization(_) => {}
            other => panic!("unexpected error: {other:?}"),
        }
    }
}
