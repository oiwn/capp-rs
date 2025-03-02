use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// HTTP-specific cache payload
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HttpCachePayload {
    /// The HTTP response body
    pub body: String,
    /// HTTP response headers
    pub headers: HashMap<String, String>,
    /// Additional metadata about the request/response
    pub metadata: Option<HashMap<String, String>>,
}

/// Represents the state of a cached entry
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum CacheEntryState {
    /// Recently cached, valid
    Fresh,
    /// Needs refresh but usable
    Stale,
    /// Must be refreshed before use (invalid page, but loaded)
    Invalid,
    /// Failed to cache properly
    Error,
}

/// A generic cached entry with HTTP-specific fields
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry<T> {
    /// Unique identifier for this cache entry
    pub key: String,
    /// The cached payload
    pub value: T,
    /// HTTP response status code
    pub status_code: u16,
    /// Current state of the cache entry
    pub state: CacheEntryState,
    /// When this entry was first created
    pub created_at: DateTime<Utc>,
    /// When this entry was last accessed
    pub last_accessed: DateTime<Utc>,
    /// Number of errors encountered for this entry
    pub error_count: i32,
    /// Most recent error message
    pub last_error: Option<String>,
    /// Time-to-live in seconds
    pub ttl: Option<u64>,
}
