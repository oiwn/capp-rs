pub mod memory;
#[cfg(feature = "mongodb")]
pub mod mongodb;
#[cfg(feature = "redis")]
pub mod redis;
#[cfg(feature = "redis")]
pub mod redis_rr;

pub use memory::InMemoryTaskQueue;
#[cfg(feature = "mongodb")]
pub use mongodb::MongoTaskQueue;
#[cfg(feature = "redis")]
pub use redis::RedisTaskQueue;
#[cfg(feature = "redis")]
pub use redis_rr::RedisRoundRobinTaskQueue;
