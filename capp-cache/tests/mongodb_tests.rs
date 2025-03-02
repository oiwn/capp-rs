#[cfg(test)]
mod tests {

    mod mongodb_tests {
        use capp_cache::{
            CacheEntryState, HttpCache, HttpCachePayload, MongoHttpCache,
        };
        use mongodb::Client;
        use std::collections::HashMap;

        async fn setup_test_db() -> mongodb::Database {
            let client = Client::with_uri_str("mongodb://localhost:27017")
                .await
                .unwrap();
            client.database("test_db")
        }

        #[tokio::test]
        async fn test_http_cache_basic_operations() {
            let db = setup_test_db().await;
            let cache = MongoHttpCache::<HttpCachePayload>::new(
                db.clone(),
                "test_http_cache",
            )
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
                .set(key, data.clone(), 200, CacheEntryState::Fresh, Some(3600))
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
            db.drop().await.unwrap();
        }
    }
}
