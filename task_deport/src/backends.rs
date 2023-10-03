pub mod memory;
#[cfg(feature = "redis")]
pub mod redis;
#[cfg(feature = "redis")]
pub mod redis_rr;

pub use memory::{InMemoryTaskStorage, InMemoryTaskStorageError};
#[cfg(feature = "redis")]
pub use redis::{RedisTaskStorage, RedisTaskStorageError};
#[cfg(feature = "redis")]
pub use redis_rr::RedisRoundRobinTaskStorage;
