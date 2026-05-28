use std::path::Path;

use anyhow::{Context as _, Result};
use fjall::{Database, Keyspace, KeyspaceCreateOptions};

use crate::parse::{Comment, StoryMeta};

pub struct DataStore {
    _db: Database,
    pub stories: Keyspace,
    pub comments: Keyspace,
}

impl DataStore {
    pub fn open(path: &Path) -> Result<Self> {
        let db = Database::builder(path).open().context("open fjall db")?;
        let stories = db
            .keyspace("stories", KeyspaceCreateOptions::default)
            .context("open stories keyspace")?;
        let comments = db
            .keyspace("comments", KeyspaceCreateOptions::default)
            .context("open comments keyspace")?;
        Ok(Self {
            _db: db,
            stories,
            comments,
        })
    }

    pub fn has_story(&self, id: u64) -> Result<bool> {
        Ok(self.stories.contains_key(id.to_be_bytes())?)
    }

    pub fn put_story(&self, meta: &StoryMeta) -> Result<()> {
        let bytes = serde_json::to_vec(meta)?;
        self.stories.insert(meta.id.to_be_bytes(), bytes)?;
        Ok(())
    }

    // Key = story_id || comment_id (both big-endian u64). Composite is on
    // purpose: comments stay grouped by story in lexicographic order, so a
    // single `prefix(story_id_be)` scan returns the whole thread.
    pub fn put_comment(&self, c: &Comment) -> Result<()> {
        let mut key = [0u8; 16];
        key[..8].copy_from_slice(&c.story_id.to_be_bytes());
        key[8..].copy_from_slice(&c.id.to_be_bytes());
        let bytes = serde_json::to_vec(c)?;
        self.comments.insert(key, bytes)?;
        Ok(())
    }

    pub fn story_count(&self) -> usize {
        self.stories.approximate_len()
    }

    pub fn comment_count(&self) -> usize {
        self.comments.approximate_len()
    }
}
