//! HTTP cache implementations for capp-rs.
//!
//! This crate provides HTTP caching functionality for web crawlers and other
//! HTTP-heavy applications. It offers a trait-based API with pluggable backends.
//!
//! Currently supported backends:
//! - MongoDB (with the "mongodb" feature)

mod cache;
mod error;
mod http_cache;
mod mongodb;

pub use cache::{CacheEntry, CacheEntryState, HttpCachePayload};
pub use error::CacheError;
pub use http_cache::HttpCache;

pub use mongodb::MongoHttpCache;
