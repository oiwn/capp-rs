#[cfg(test)]
mod tests {
    mod mongodb_tests {
        use bson::Bson;
        use capp_cache::{
            CacheEntry, CacheEntryState, CacheError, HttpCache, HttpCachePayload,
            MongoHttpCache,
        };
        use dotenvy::dotenv;
        use mongodb::{Client, Database, options::ClientOptions};
        use std::collections::HashMap;

        async fn get_mongodb() -> Database {
            dotenv().ok();
            let uri =
                std::env::var("MONGODB_URI").expect("Set MONGODB_URI env variable");
            let client_options = ClientOptions::parse(&uri)
                .await
                .expect("Failed to parse options");
            let client = Client::with_options(client_options.clone())
                .expect("Failed to create client");
            let db_name = client_options
                .default_database
                .as_ref()
                .expect("No database specified");
            client.database(db_name)
        }

        async fn verify_collection_exists(
            db: &Database,
            collection_name: &str,
        ) -> bool {
            let collections = db.list_collection_names().await.unwrap();
            collections.contains(&collection_name.to_string())
        }

        async fn cleanup_collections(
            name: &str,
        ) -> Result<(), mongodb::error::Error> {
            let db = get_mongodb().await;

            // Check if collections exist before dropping
            if verify_collection_exists(&db, name).await {
                tracing::info!("Dropping collection: {}", name);
                db.collection::<MongoHttpCache<HttpCachePayload>>(name)
                    .drop()
                    .await?;
            }

            Ok(())
        }

        #[tokio::test]
        async fn test_http_cache_basic_operations() {
            let cache_name = "test_cache_basic_ops";
            cleanup_collections(cache_name).await.unwrap();

            let db = get_mongodb().await;
            let cache =
                MongoHttpCache::<HttpCachePayload>::new(db.clone(), cache_name)
                    .await
                    .unwrap();

            // Test data
            let key = "https://example.com";
            let mut headers = HashMap::new();
            headers.insert("Content-Type".to_string(), "text/html".to_string());

            let data = HttpCachePayload {
                body: "<html><body>Test page</body></html>".to_string(),
                headers,
                metadata: None,
            };

            // Test set
            cache
                .set(key, data.clone(), 200, CacheEntryState::Fresh)
                .await
                .unwrap();

            // Test get
            let entry = cache.get(key).await.unwrap().unwrap();
            assert_eq!(entry.value.body, data.body);
            assert!(matches!(entry.state, CacheEntryState::Fresh));
            assert_eq!(entry.status_code, 200);

            // Test contains
            assert!(cache.contains(key).await.unwrap());

            // Clean up
            cache.remove(key).await.unwrap();
        }

        #[tokio::test]
        async fn test_update_state() {
            let cache_name = "test_cache_update_state";
            cleanup_collections(cache_name).await.unwrap();

            let db = get_mongodb().await;
            let cache =
                MongoHttpCache::<HttpCachePayload>::new(db.clone(), cache_name)
                    .await
                    .unwrap();

            // Test data
            let key = "https://example.com/update_state";
            let mut headers = HashMap::new();
            headers.insert("Content-Type".to_string(), "text/html".to_string());

            let data = HttpCachePayload {
                body: "<html><body>Test page</body></html>".to_string(),
                headers,
                metadata: None,
            };

            // Set initial entry with Fresh state
            cache
                .set(key, data.clone(), 200, CacheEntryState::Fresh)
                .await
                .unwrap();

            // Update state to Stale
            cache
                .update_state(key, CacheEntryState::Stale)
                .await
                .unwrap();

            // Verify state was updated
            let entry = cache.get(key).await.unwrap().unwrap();
            assert!(matches!(entry.state, CacheEntryState::Stale));

            // Update to Invalid state
            cache
                .update_state(key, CacheEntryState::Invalid)
                .await
                .unwrap();

            // Verify updated to Invalid
            let entry = cache.get(key).await.unwrap().unwrap();
            assert!(matches!(entry.state, CacheEntryState::Invalid));

            // Test updating non-existent key
            let result = cache
                .update_state("nonexistent-key", CacheEntryState::Stale)
                .await;
            assert!(matches!(result, Err(CacheError::NotFound(_))));

            // Clean up
            cache.remove(key).await.unwrap();
        }

        #[tokio::test]
        async fn test_get_nonexistent() {
            let cache_name = "test_cache_get_nonexistent";
            cleanup_collections(cache_name).await.unwrap();

            let db = get_mongodb().await;
            let cache =
                MongoHttpCache::<HttpCachePayload>::new(db.clone(), cache_name)
                    .await
                    .unwrap();

            // Test retrieving non-existent key
            let result = cache.get("nonexistent-key").await.unwrap();
            assert!(result.is_none());
        }

        #[tokio::test]
        async fn test_cleanup() {
            let cache_name = "test_cache_cleanup";
            cleanup_collections(cache_name).await.unwrap();

            let db = get_mongodb().await;
            let cache =
                MongoHttpCache::<HttpCachePayload>::new(db.clone(), cache_name)
                    .await
                    .unwrap();

            // Prepare test data
            let mut headers = HashMap::new();
            headers.insert("Content-Type".to_string(), "text/html".to_string());
            let data = HttpCachePayload {
                body: "<html><body>Test page</body></html>".to_string(),
                headers,
                metadata: None,
            };

            // Create entries with different states

            // Fresh entry - should not be cleaned up
            cache
                .set("fresh-entry", data.clone(), 200, CacheEntryState::Fresh)
                .await
                .unwrap();

            // Stale entry - should not be cleaned up
            cache
                .set("stale-entry", data.clone(), 200, CacheEntryState::Stale)
                .await
                .unwrap();

            // Invalid entry - should be cleaned up
            cache
                .set("invalid-entry", data.clone(), 404, CacheEntryState::Invalid)
                .await
                .unwrap();

            // For error entries with error_count > 3, we need to manually insert using MongoDB's API
            // Insert directly with MongoDB client to set error_count > 3
            let error_entry = CacheEntry {
                key: "error-entry".to_string(),
                value: data.clone(),
                status_code: 500,
                state: CacheEntryState::Error,
                created_at: chrono::Utc::now(),
                last_accessed: chrono::Utc::now(),
                error_count: 4, // > 3, should be cleaned up
                last_error: Some("Test error".to_string()),
            };

            cache.collection.insert_one(error_entry).await.unwrap();

            // Error entry with error_count <= 3 - should not be cleaned up
            let error_entry_keep = CacheEntry {
                key: "error-entry-keep".to_string(),
                value: data.clone(),
                status_code: 500,
                state: CacheEntryState::Error,
                created_at: chrono::Utc::now(),
                last_accessed: chrono::Utc::now(),
                error_count: 2, // <= 3, should not be cleaned up
                last_error: Some("Test error".to_string()),
            };

            // let doc = bson::to_document(&error_entry_keep).unwrap();
            cache.collection.insert_one(error_entry_keep).await.unwrap();

            // Run cleanup
            let deleted_count = cache.cleanup().await.unwrap();

            // Should have deleted 2 entries (1 invalid, 1 error with count > 3)
            assert_eq!(deleted_count, 2);

            // Verify Fresh and Stale entries still exist
            assert!(cache.contains("fresh-entry").await.unwrap());
            assert!(cache.contains("stale-entry").await.unwrap());
            assert!(cache.contains("error-entry-keep").await.unwrap());

            // Verify Invalid and Error entry with count > 3 were removed
            assert!(!cache.contains("invalid-entry").await.unwrap());
            assert!(!cache.contains("error-entry").await.unwrap());
        }

        #[test]
        fn test_cache_entry_state_to_bson_conversion() {
            // Test conversion for all states
            assert_eq!(
                Bson::from(CacheEntryState::Fresh),
                Bson::String("fresh".to_string())
            );

            assert_eq!(
                Bson::from(CacheEntryState::Stale),
                Bson::String("stale".to_string())
            );

            assert_eq!(
                Bson::from(CacheEntryState::Invalid),
                Bson::String("invalid".to_string())
            );

            assert_eq!(
                Bson::from(CacheEntryState::Error),
                Bson::String("error".to_string())
            );

            // Test through a match expression to ensure we're handling all variants
            fn test_all_variants(state: CacheEntryState) -> String {
                let bson: Bson = state.into();
                match bson {
                    Bson::String(s) => s,
                    _ => panic!("Expected String BSON type"),
                }
            }

            assert_eq!(test_all_variants(CacheEntryState::Fresh), "fresh");
            assert_eq!(test_all_variants(CacheEntryState::Stale), "stale");
            assert_eq!(test_all_variants(CacheEntryState::Invalid), "invalid");
            assert_eq!(test_all_variants(CacheEntryState::Error), "error");
        }
    }
}
