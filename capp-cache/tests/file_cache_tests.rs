use std::collections::HashMap;

use capp_cache::{CacheEntryState, FileHttpCache, HttpCache, HttpCachePayload};
use tempfile::tempdir;

fn make_payload(body: &str) -> HttpCachePayload {
    let mut headers = HashMap::new();
    headers.insert("content-type".to_string(), "text/plain".to_string());

    let mut metadata = HashMap::new();
    metadata.insert("source".to_string(), "test".to_string());

    HttpCachePayload {
        body: body.to_string(),
        headers,
        metadata: Some(metadata),
    }
}

#[tokio::test]
async fn set_get_contains_and_remove_work() {
    let dir = tempdir().unwrap();
    let cache = FileHttpCache::<HttpCachePayload>::open(dir.path()).unwrap();
    let key = "https://example.com/page";

    cache
        .set(key, make_payload("hello"), 200, CacheEntryState::Fresh)
        .await
        .unwrap();

    assert!(cache.contains(key).await.unwrap());

    let entry = cache.get(key).await.unwrap().unwrap();
    assert_eq!(entry.key, key);
    assert_eq!(entry.status_code, 200);
    assert_eq!(entry.value.body, "hello");
    assert_eq!(entry.state, CacheEntryState::Fresh);
    assert!(entry.last_accessed >= entry.created_at);

    cache.remove(key).await.unwrap();
    assert!(!cache.contains(key).await.unwrap());
    assert!(cache.get(key).await.unwrap().is_none());
}

#[tokio::test]
async fn update_state_persists_to_disk() {
    let dir = tempdir().unwrap();
    let cache = FileHttpCache::<HttpCachePayload>::open(dir.path()).unwrap();
    let key = "https://example.com/state";

    cache
        .set(key, make_payload("state"), 200, CacheEntryState::Fresh)
        .await
        .unwrap();
    cache
        .update_state(key, CacheEntryState::Stale)
        .await
        .unwrap();

    let entry = cache.get(key).await.unwrap().unwrap();
    assert_eq!(entry.state, CacheEntryState::Stale);
}

#[tokio::test]
async fn cleanup_removes_invalid_and_repeated_errors() {
    let dir = tempdir().unwrap();
    let cache = FileHttpCache::<HttpCachePayload>::open(dir.path()).unwrap();

    cache
        .set("fresh", make_payload("fresh"), 200, CacheEntryState::Fresh)
        .await
        .unwrap();
    cache
        .set(
            "invalid",
            make_payload("invalid"),
            404,
            CacheEntryState::Invalid,
        )
        .await
        .unwrap();
    cache
        .set("error", make_payload("error"), 500, CacheEntryState::Fresh)
        .await
        .unwrap();

    for _ in 0..4 {
        cache
            .update_state("error", CacheEntryState::Error)
            .await
            .unwrap();
    }

    let deleted = cache.cleanup().await.unwrap();
    assert_eq!(deleted, 2);
    assert!(cache.contains("fresh").await.unwrap());
    assert!(!cache.contains("invalid").await.unwrap());
    assert!(!cache.contains("error").await.unwrap());
}

#[tokio::test]
async fn clear_all_removes_everything() {
    let dir = tempdir().unwrap();
    let cache = FileHttpCache::<HttpCachePayload>::open(dir.path()).unwrap();

    cache
        .set("one", make_payload("1"), 200, CacheEntryState::Fresh)
        .await
        .unwrap();
    cache
        .set("two", make_payload("2"), 200, CacheEntryState::Fresh)
        .await
        .unwrap();

    let deleted = cache.clear_all().await.unwrap();
    assert_eq!(deleted, 2);
    assert!(!cache.contains("one").await.unwrap());
    assert!(!cache.contains("two").await.unwrap());
}

#[tokio::test]
async fn cache_files_are_stored_in_date_segmented_directories() {
    let dir = tempdir().unwrap();
    let cache = FileHttpCache::<HttpCachePayload>::open(dir.path()).unwrap();

    cache
        .set(
            "https://example.com/segmented",
            make_payload("segmented"),
            200,
            CacheEntryState::Fresh,
        )
        .await
        .unwrap();

    let files_dir = dir.path().join("files");
    let years = std::fs::read_dir(&files_dir)
        .unwrap()
        .map(|item| item.unwrap().path())
        .collect::<Vec<_>>();
    assert_eq!(years.len(), 1);
    assert!(years[0].is_dir());

    let months = std::fs::read_dir(&years[0])
        .unwrap()
        .map(|item| item.unwrap().path())
        .collect::<Vec<_>>();
    assert_eq!(months.len(), 1);
    assert!(months[0].is_dir());

    let days = std::fs::read_dir(&months[0])
        .unwrap()
        .map(|item| item.unwrap().path())
        .collect::<Vec<_>>();
    assert_eq!(days.len(), 1);
    assert!(days[0].is_dir());

    let files = std::fs::read_dir(&days[0])
        .unwrap()
        .map(|item| item.unwrap().path())
        .collect::<Vec<_>>();
    assert_eq!(files.len(), 1);
    assert!(files[0].is_file());
}
