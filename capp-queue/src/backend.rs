#[cfg(feature = "fjall")]
pub mod fjall;
#[cfg(feature = "fjall")]
pub mod fjall_round_robin;
pub mod memory;

#[cfg(feature = "fjall")]
pub use fjall::FjallTaskQueue;
#[cfg(feature = "fjall")]
pub use fjall_round_robin::FjallRoundRobinTaskQueue;
pub use memory::InMemoryTaskQueue;
