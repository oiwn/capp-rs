use crate::{CacheEntry, CacheEntryState, CacheError};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Core HTTP cache trait that backends must implement
#[async_trait]
pub trait HttpCache<T>: Send + Sync
where
    T: Send + Sync + Serialize + for<'de> Deserialize<'de>,
{
    /// Get a value from cache
    async fn get(&self, key: &str) -> Result<Option<CacheEntry<T>>, CacheError>;

    /// Set or update a value in cache
    async fn set(
        &self,
        key: &str,
        value: T,
        status_code: u16,
        state: CacheEntryState,
        ttl: Option<u64>,
    ) -> Result<(), CacheError>;

    /// Update entry state
    async fn update_state(
        &self,
        key: &str,
        state: CacheEntryState,
    ) -> Result<(), CacheError>;

    /// Remove entry from cache
    async fn remove(&self, key: &str) -> Result<(), CacheError>;

    /// Remove stale/invalid/error entries
    async fn cleanup(&self) -> Result<u64, CacheError>;

    /// Check if a URL is cached
    async fn contains(&self, key: &str) -> Result<bool, CacheError>;
}
