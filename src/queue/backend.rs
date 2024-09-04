pub mod memory;
#[cfg(feature = "redis")]
pub mod redis;
// #[cfg(feature = "redis")]
// pub mod redis_rr;

pub use memory::InMemoryTaskQueue;
#[cfg(feature = "redis")]
pub use redis::RedisTaskQueue;
// #[cfg(feature = "redis2")]
// pub use redis_rr::RedisRoundRobinTaskStorage;
