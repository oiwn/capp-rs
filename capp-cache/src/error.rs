use thiserror::Error;

#[derive(Error, Debug)]
pub enum CacheError {
    #[error("Cache entry not found: {0}")]
    NotFound(String),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Deserialization error: {0}")]
    Deserialization(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Path error: {0}")]
    Path(String),

    #[error("Fjall error: {0}")]
    Fjall(#[from] fjall::Error),
}
