pub mod memory;
#[cfg(feature = "redis")]
pub mod redis;
#[cfg(feature = "redis")]
pub mod redis_rr;

pub use memory::InMemoryTaskStorage;
#[cfg(feature = "redis")]
pub use redis::RedisTaskStorage;
#[cfg(feature = "redis")]
pub use redis_rr::RedisRoundRobinTaskStorage;
