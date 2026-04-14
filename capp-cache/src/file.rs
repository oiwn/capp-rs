use std::{
    marker::PhantomData,
    path::{Path, PathBuf},
};

use async_trait::async_trait;
use chrono::{Datelike, Utc};
use fjall::{
    Config, Keyspace, PartitionCreateOptions, PartitionHandle, PersistMode,
};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tracing::debug;

use crate::{CacheEntry, CacheEntryState, CacheError, HttpCache};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheIndexEntry {
    file_path: String,
    state: CacheEntryState,
    status_code: u16,
    created_at: chrono::DateTime<Utc>,
    last_accessed: chrono::DateTime<Utc>,
    error_count: i32,
    last_error: Option<String>,
}

/// File-backed HTTP cache with a Fjall metadata index.
///
/// Layout under the provided base directory:
/// - `index/`: Fjall keyspace storing cache metadata keyed by cache key
/// - `files/YYYY/MM/DD/*.json`: full serialized `CacheEntry<T>` payloads
pub struct FileHttpCache<T>
where
    T: Send + Sync + Serialize + for<'de> Deserialize<'de>,
{
    base_dir: PathBuf,
    files_dir: PathBuf,
    keyspace: Keyspace,
    entries: PartitionHandle,
    lock: Mutex<()>,
    _marker: PhantomData<T>,
}

impl<T> FileHttpCache<T>
where
    T: Send + Sync + Serialize + for<'de> Deserialize<'de>,
{
    pub fn open(path: impl AsRef<Path>) -> Result<Self, CacheError> {
        let base_dir = path.as_ref().to_path_buf();
        let files_dir = base_dir.join("files");
        let index_dir = base_dir.join("index");

        std::fs::create_dir_all(&files_dir)?;
        std::fs::create_dir_all(&index_dir)?;

        let keyspace = Config::new(index_dir).open()?;
        let entries = keyspace
            .open_partition("entries", PartitionCreateOptions::default())?;

        Ok(Self {
            base_dir,
            files_dir,
            keyspace,
            entries,
            lock: Mutex::new(()),
            _marker: PhantomData,
        })
    }

    pub async fn clear_all(&self) -> Result<u64, CacheError> {
        let _guard = self.lock.lock().await;

        let keys = self.collect_keys()?;
        let deleted = keys.len() as u64;

        for key in keys {
            self.remove_internal(&key).await?;
        }

        Ok(deleted)
    }

    fn collect_keys(&self) -> Result<Vec<String>, CacheError> {
        let mut keys = Vec::new();
        for item in self.entries.iter() {
            let (key, _) = item?;
            let key = String::from_utf8(key.as_ref().to_vec())
                .map_err(|err| CacheError::Deserialization(err.to_string()))?;
            keys.push(key);
        }
        Ok(keys)
    }

    fn file_path_from_metadata(
        &self,
        metadata: &CacheIndexEntry,
    ) -> Result<PathBuf, CacheError> {
        let rel = PathBuf::from(&metadata.file_path);
        if rel.is_absolute() {
            return Err(CacheError::Path(format!(
                "cache metadata contains absolute path: {}",
                metadata.file_path
            )));
        }
        Ok(self.base_dir.join(rel))
    }

    fn new_relative_file_path(&self, key: &str) -> PathBuf {
        let now = Utc::now();
        let hash = format!("{:x}", md5::compute(key.as_bytes()));
        PathBuf::from("files")
            .join(format!("{:04}", now.year()))
            .join(format!("{:02}", now.month()))
            .join(format!("{:02}", now.day()))
            .join(format!("{}-{}.json", now.timestamp_millis(), hash))
    }

    fn read_metadata(
        &self,
        key: &str,
    ) -> Result<Option<CacheIndexEntry>, CacheError> {
        let Some(bytes) = self.entries.get(key.as_bytes())? else {
            return Ok(None);
        };

        let metadata = serde_json::from_slice(&bytes)
            .map_err(|err| CacheError::Deserialization(err.to_string()))?;
        Ok(Some(metadata))
    }

    fn write_metadata(
        &self,
        key: &str,
        metadata: &CacheIndexEntry,
    ) -> Result<(), CacheError> {
        let bytes = serde_json::to_vec(metadata)
            .map_err(|err| CacheError::Serialization(err.to_string()))?;
        self.entries.insert(key.as_bytes(), bytes)?;
        self.keyspace.persist(PersistMode::SyncAll)?;
        Ok(())
    }

    async fn write_entry(
        &self,
        path: &Path,
        entry: &CacheEntry<T>,
    ) -> Result<(), CacheError> {
        let bytes = serde_json::to_vec(entry)
            .map_err(|err| CacheError::Serialization(err.to_string()))?;

        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        tokio::fs::write(path, bytes).await?;
        Ok(())
    }

    async fn read_entry(
        &self,
        path: &Path,
    ) -> Result<Option<CacheEntry<T>>, CacheError> {
        match tokio::fs::read(path).await {
            Ok(bytes) => {
                let entry = serde_json::from_slice(&bytes)
                    .map_err(|err| CacheError::Deserialization(err.to_string()))?;
                Ok(Some(entry))
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(CacheError::Io(err)),
        }
    }

    async fn remove_internal(&self, key: &str) -> Result<(), CacheError> {
        let Some(metadata) = self.read_metadata(key)? else {
            return Ok(());
        };

        let path = self.file_path_from_metadata(&metadata)?;
        match tokio::fs::remove_file(&path).await {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(CacheError::Io(err)),
        }

        self.entries.remove(key.as_bytes())?;
        self.keyspace.persist(PersistMode::SyncAll)?;
        Ok(())
    }
}

#[async_trait]
impl<T> HttpCache<T> for FileHttpCache<T>
where
    T: Send + Sync + Serialize + for<'de> Deserialize<'de>,
{
    async fn get(&self, key: &str) -> Result<Option<CacheEntry<T>>, CacheError> {
        let _guard = self.lock.lock().await;

        let Some(mut metadata) = self.read_metadata(key)? else {
            return Ok(None);
        };

        let path = self.file_path_from_metadata(&metadata)?;
        let Some(mut entry) = self.read_entry(&path).await? else {
            self.entries.remove(key.as_bytes())?;
            self.keyspace.persist(PersistMode::SyncAll)?;
            return Ok(None);
        };

        let now = Utc::now();
        entry.last_accessed = now;
        metadata.last_accessed = now;
        self.write_entry(&path, &entry).await?;
        self.write_metadata(key, &metadata)?;
        Ok(Some(entry))
    }

    async fn set(
        &self,
        key: &str,
        value: T,
        status_code: u16,
        state: CacheEntryState,
    ) -> Result<(), CacheError> {
        let _guard = self.lock.lock().await;

        let existing = self.read_metadata(key)?;
        let now = Utc::now();
        let rel_path = existing
            .as_ref()
            .map(|metadata| PathBuf::from(&metadata.file_path))
            .unwrap_or_else(|| self.new_relative_file_path(key));
        let path = self.base_dir.join(&rel_path);
        let created_at = existing
            .as_ref()
            .map(|metadata| metadata.created_at)
            .unwrap_or(now);
        let error_count = if state == CacheEntryState::Error {
            1
        } else {
            0
        };
        let last_error = if state == CacheEntryState::Error {
            Some("cache entry marked as error".to_string())
        } else {
            None
        };

        let entry = CacheEntry {
            key: key.to_string(),
            value,
            status_code,
            state: state.clone(),
            created_at,
            last_accessed: now,
            error_count,
            last_error: last_error.clone(),
        };

        self.write_entry(&path, &entry).await?;

        let metadata = CacheIndexEntry {
            file_path: rel_path.to_string_lossy().into_owned(),
            state,
            status_code,
            created_at,
            last_accessed: now,
            error_count,
            last_error,
        };
        self.write_metadata(key, &metadata)?;

        debug!("cached HTTP entry for key: {}", key);
        Ok(())
    }

    async fn update_state(
        &self,
        key: &str,
        state: CacheEntryState,
    ) -> Result<(), CacheError> {
        let _guard = self.lock.lock().await;

        let Some(mut metadata) = self.read_metadata(key)? else {
            return Err(CacheError::NotFound(key.to_string()));
        };

        let path = self.file_path_from_metadata(&metadata)?;
        let Some(mut entry) = self.read_entry(&path).await? else {
            self.entries.remove(key.as_bytes())?;
            self.keyspace.persist(PersistMode::SyncAll)?;
            return Err(CacheError::NotFound(key.to_string()));
        };

        let now = Utc::now();
        entry.state = state.clone();
        entry.last_accessed = now;
        metadata.state = state.clone();
        metadata.last_accessed = now;

        if state == CacheEntryState::Error {
            entry.error_count += 1;
            metadata.error_count += 1;
            let message = "cache entry marked as error".to_string();
            entry.last_error = Some(message.clone());
            metadata.last_error = Some(message);
        }

        self.write_entry(&path, &entry).await?;
        self.write_metadata(key, &metadata)?;
        Ok(())
    }

    async fn remove(&self, key: &str) -> Result<(), CacheError> {
        let _guard = self.lock.lock().await;

        if self.read_metadata(key)?.is_none() {
            return Err(CacheError::NotFound(key.to_string()));
        }

        self.remove_internal(key).await
    }

    async fn cleanup(&self) -> Result<u64, CacheError> {
        let _guard = self.lock.lock().await;

        let mut keys_to_delete = Vec::new();
        for item in self.entries.iter() {
            let (key, value) = item?;
            let key = String::from_utf8(key.as_ref().to_vec())
                .map_err(|err| CacheError::Deserialization(err.to_string()))?;
            let metadata: CacheIndexEntry = serde_json::from_slice(&value)
                .map_err(|err| CacheError::Deserialization(err.to_string()))?;

            if metadata.state == CacheEntryState::Invalid
                || (metadata.state == CacheEntryState::Error
                    && metadata.error_count > 3)
            {
                keys_to_delete.push(key);
            }
        }

        let deleted = keys_to_delete.len() as u64;
        for key in keys_to_delete {
            self.remove_internal(&key).await?;
        }

        Ok(deleted)
    }

    async fn contains(&self, key: &str) -> Result<bool, CacheError> {
        let _guard = self.lock.lock().await;
        Ok(self.read_metadata(key)?.is_some())
    }
}

impl<T> std::fmt::Debug for FileHttpCache<T>
where
    T: Send + Sync + Serialize + for<'de> Deserialize<'de>,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileHttpCache")
            .field("base_dir", &self.base_dir)
            .field("files_dir", &self.files_dir)
            .finish()
    }
}
