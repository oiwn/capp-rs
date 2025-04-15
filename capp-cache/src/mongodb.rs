use crate::{CacheEntry, CacheEntryState, CacheError, HttpCache};
use async_trait::async_trait;
use chrono::Utc;
use mongodb::{
    Collection, Database, IndexModel,
    bson::{self, Bson, doc},
    options::IndexOptions,
};
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

/// Helper to convert CacheEntryState to BSON
impl From<CacheEntryState> for Bson {
    fn from(state: CacheEntryState) -> Self {
        match state {
            CacheEntryState::Fresh => Bson::String("fresh".to_string()),
            CacheEntryState::Stale => Bson::String("stale".to_string()),
            CacheEntryState::Invalid => Bson::String("invalid".to_string()),
            CacheEntryState::Error => Bson::String("error".to_string()),
        }
    }
}

/// MongoDB implementation of the HTTP cache
#[derive(Debug, Clone)]
pub struct MongoHttpCache<T>
where
    T: Send + Sync + Serialize + for<'de> Deserialize<'de>,
{
    pub collection: Collection<CacheEntry<T>>,
}

impl<T> MongoHttpCache<T>
where
    T: Send + Sync + Serialize + for<'de> Deserialize<'de>,
{
    /// Create a new MongoDB-backed HTTP cache
    pub async fn new(
        db: Database,
        collection_name: &str,
    ) -> Result<Self, CacheError> {
        let collection = db.collection(collection_name);
        let cache = Self { collection };
        cache.setup_indexes().await?;
        Ok(cache)
    }

    /// Set up necessary indexes for the cache collection
    pub async fn setup_indexes(&self) -> Result<(), CacheError> {
        let mut indexes = vec![];

        // Key index
        let key_index = IndexModel::builder()
            .keys(doc! { "key": 1 })
            .options(IndexOptions::builder().unique(true).build())
            .build();
        indexes.push(key_index);

        // State + Last Accessed index for cleanup
        let state_index = IndexModel::builder()
            .keys(doc! {
                "state": 1,
                "last_accessed": 1
            })
            .build();
        indexes.push(state_index);

        self.collection.create_indexes(indexes).await?;
        info!("HTTP cache indexes created successfully");
        Ok(())
    }
}

#[async_trait]
impl<T> HttpCache<T> for MongoHttpCache<T>
where
    T: Send + Sync + Serialize + for<'de> Deserialize<'de>,
{
    async fn get(&self, key: &str) -> Result<Option<CacheEntry<T>>, CacheError> {
        let result = self.collection.find_one(doc! { "key": key }).await?;

        if let Some(mut entry) = result {
            // Update last accessed time
            entry.last_accessed = Utc::now();
            self.collection
                .update_one(
                    doc! { "key": key },
                    doc! { "$set": { "last_accessed": entry.last_accessed }},
                )
                .await?;

            Ok(Some(entry))
        } else {
            Ok(None)
        }
    }

    async fn set(
        &self,
        key: &str,
        value: T,
        status_code: u16,
        state: CacheEntryState,
    ) -> Result<(), CacheError> {
        let entry = CacheEntry {
            key: key.to_string(),
            value,
            status_code,
            state,
            created_at: Utc::now(),
            last_accessed: Utc::now(),
            error_count: 0,
            last_error: None,
        };

        // Convert to document for update
        let doc = bson::to_document(&entry)?;

        self.collection
            .update_one(doc! { "key": key }, doc! { "$set": doc })
            .upsert(true)
            .await?;

        debug!("Cached HTTP entry for key: {}", key);
        Ok(())
    }

    async fn update_state(
        &self,
        key: &str,
        state: CacheEntryState,
    ) -> Result<(), CacheError> {
        // Explicitly convert state to BSON
        let state_bson = Bson::from(state);

        // Create a current timestamp that will be properly serialized
        let current_time = Utc::now();

        let result = self
            .collection
            .update_one(
                doc! { "key": key },
                doc! {
                    "$set": {
                        "state": state_bson,
                        "last_accessed": current_time
                    }
                },
            )
            .await?;

        if result.matched_count == 0 {
            return Err(CacheError::NotFound(key.to_string()));
        }

        Ok(())
    }

    async fn remove(&self, key: &str) -> Result<(), CacheError> {
        let result = self.collection.delete_one(doc! { "key": key }).await?;

        if result.deleted_count == 0 {
            return Err(CacheError::NotFound(key.to_string()));
        }

        Ok(())
    }

    async fn cleanup(&self) -> Result<u64, CacheError> {
        let result = self
            .collection
            .delete_many(doc! {
                "$or": [
                    { "state": "invalid" },
                    { "state": "error", "error_count": { "$gt": 3 } }
                ]
            })
            .await?;

        Ok(result.deleted_count)
    }

    async fn contains(&self, key: &str) -> Result<bool, CacheError> {
        let count = self.collection.count_documents(doc! { "key": key }).await?;

        Ok(count > 0)
    }
}
