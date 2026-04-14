//! HTTP cache implementations for capp-rs.
//!
//! This crate provides HTTP caching functionality for web crawlers and other
//! HTTP-heavy applications. It offers a trait-based API with pluggable backends.
//!
//! Currently supported backends:
//! - File-backed cache with a Fjall metadata index

mod cache;
mod error;
mod file;
mod http_cache;

pub use cache::{CacheEntry, CacheEntryState, HttpCachePayload};
pub use error::CacheError;
pub use file::FileHttpCache;
pub use http_cache::HttpCache;
