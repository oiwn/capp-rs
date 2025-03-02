use thiserror::Error;

#[derive(Error, Debug)]
pub enum CacheError {
    #[error("Cache entry not found: {0}")]
    NotFound(String),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Deserialization error: {0}")]
    Deserialization(String),

    #[error("MongoDB error: {0}")]
    Mongo(#[from] mongodb::error::Error),

    #[error("MongoDB serialization error: {0}")]
    MongoBsonSer(#[from] mongodb::bson::ser::Error),

    #[error("MongoDB deserialization error: {0}")]
    MongoBsonDe(#[from] mongodb::bson::de::Error),
}
