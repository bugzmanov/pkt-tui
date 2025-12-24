//! Raindrop.io API client module
//!
//! This module provides a complete Rust client for the Raindrop.io API.
//!
//! # Usage
//!
//! ```rust,no_run
//! use raindrop::{RaindropAPISync, token, models::*};
//!
//! // Get or prompt for token
//! let token = token::get_or_prompt_token().unwrap();
//!
//! // Create API client
//! let client = RaindropAPISync::new(&token).unwrap();
//!
//! // Fetch all raindrops
//! let raindrops = client.get_all_raindrops(COLLECTION_ALL).unwrap();
//!
//! // Add a new raindrop
//! let new = NewRaindrop::new("https://example.com")
//!     .with_title("Example")
//!     .with_tags(vec!["test".into()]);
//! let created = client.add_raindrop(&new).unwrap();
//! ```

pub mod api;
pub mod models;
pub mod token;

// Re-export token functions at module level for convenience
pub use token::{get_or_prompt_token, get_token};

// Re-export commonly used items
pub use api::{RaindropAPI, RaindropAPISync};
pub use models::{
    Collection, CollectionRef, NewRaindrop, Raindrop, RaindropCache, RaindropType, UpdateRaindrop,
    User, COLLECTION_ALL, COLLECTION_TRASH, COLLECTION_UNSORTED,
};

use anyhow::{Context, Result};
use log::{debug, error, info};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;

const CACHE_FILE: &str = "raindrop_data.json";
const DELTA_FILE: &str = "raindrop_delta.jsonl";
const META_FILE: &str = "raindrop_meta.json";

/// Small metadata file for sync state (cheap to update frequently)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SyncMeta {
    pub last_sync: String,
}

/// Save sync metadata
pub fn save_meta(meta: &SyncMeta) -> Result<()> {
    let json = serde_json::to_string(meta).context("Failed to serialize meta")?;
    fs::write(META_FILE, json).context("Failed to write meta file")?;
    Ok(())
}

/// Load sync metadata
pub fn load_meta() -> Result<Option<SyncMeta>> {
    let path = Path::new(META_FILE);
    if !path.exists() {
        return Ok(None);
    }
    let data = fs::read_to_string(path).context("Failed to read meta file")?;
    let meta: SyncMeta = serde_json::from_str(&data).context("Failed to parse meta file")?;
    Ok(Some(meta))
}

/// Update just the last_sync timestamp (very cheap operation)
pub fn update_last_sync(timestamp: &str) -> Result<()> {
    save_meta(&SyncMeta {
        last_sync: timestamp.to_string(),
    })
}

/// Get last sync time from meta file, falling back to cache file
pub fn get_last_sync() -> Option<String> {
    // Try meta file first
    if let Ok(Some(meta)) = load_meta() {
        if !meta.last_sync.is_empty() {
            return Some(meta.last_sync);
        }
    }
    // Fall back to cache file
    if let Ok(Some(cache)) = load_cache() {
        if !cache.last_sync.is_empty() {
            return Some(cache.last_sync);
        }
    }
    None
}

/// Represents an update operation for the delta file
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op")]
pub enum RaindropUpdate {
    /// Add or update a raindrop
    #[serde(rename = "upsert")]
    Upsert { item: Raindrop },
    /// Delete a raindrop
    #[serde(rename = "delete")]
    Delete { id: i64, timestamp: String },
}

/// Save raindrops and collections to local cache
pub fn save_cache(cache: &RaindropCache) -> Result<()> {
    let json = serde_json::to_string_pretty(cache).context("Failed to serialize cache")?;
    fs::write(CACHE_FILE, json).context("Failed to write cache file")?;
    Ok(())
}

/// Load raindrops and collections from local cache
pub fn load_cache() -> Result<Option<RaindropCache>> {
    let path = Path::new(CACHE_FILE);
    if !path.exists() {
        return Ok(None);
    }

    let data = fs::read_to_string(path).context("Failed to read cache file")?;
    let cache: RaindropCache = serde_json::from_str(&data).context("Failed to parse cache file")?;
    Ok(Some(cache))
}

/// Check if cache exists
pub fn cache_exists() -> bool {
    Path::new(CACHE_FILE).exists()
}

/// Delete the cache file
pub fn delete_cache() -> Result<()> {
    let path = Path::new(CACHE_FILE);
    if path.exists() {
        fs::remove_file(path).context("Failed to delete cache file")?;
    }
    Ok(())
}

// ---- Delta file operations ----

/// Append a single update to the delta file
pub fn append_raindrop_delta(update: &RaindropUpdate) -> Result<()> {
    let path = Path::new(DELTA_FILE);
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .append(true)
        .open(path)
        .context("Failed to open delta file")?;

    let json = serde_json::to_string(update).context("Failed to serialize update")?;
    writeln!(&mut file, "{}", json).context("Failed to write to delta file")?;
    Ok(())
}

/// Append multiple upserts to the delta file (for batch operations)
pub fn append_raindrop_upserts(items: &[Raindrop]) -> Result<()> {
    if items.is_empty() {
        return Ok(());
    }

    let path = Path::new(DELTA_FILE);
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .append(true)
        .open(path)
        .context("Failed to open delta file")?;

    for item in items {
        let update = RaindropUpdate::Upsert { item: item.clone() };
        let json = serde_json::to_string(&update).context("Failed to serialize update")?;
        writeln!(&mut file, "{}", json).context("Failed to write to delta file")?;
    }
    Ok(())
}

/// Append a delete operation to the delta file
pub fn append_raindrop_delete(id: i64) -> Result<()> {
    let update = RaindropUpdate::Delete {
        id,
        timestamp: now_iso8601(),
    };
    append_raindrop_delta(&update)
}

/// Load all updates from the delta file
pub fn load_raindrop_delta() -> Vec<RaindropUpdate> {
    let path = Path::new(DELTA_FILE);
    info!("Loading delta from: {:?}", path);
    match File::open(path) {
        Ok(file) => {
            let buf = BufReader::new(file);
            let updates: Vec<RaindropUpdate> = buf
                .lines()
                .filter_map(|line| match line {
                    Ok(json_str) => {
                        debug!(
                            "Parsing delta line: {}",
                            &json_str[..json_str.len().min(100)]
                        );
                        match serde_json::from_str::<RaindropUpdate>(&json_str) {
                            Ok(update) => {
                                info!("Successfully parsed delta update");
                                Some(update)
                            }
                            Err(e) => {
                                eprintln!("ERROR: Failed to parse delta line: {}", e);
                                error!("Failed to parse delta line: {} - {}", json_str, e);
                                None
                            }
                        }
                    }
                    Err(e) => {
                        error!("Failed to read delta line: {}", e);
                        None
                    }
                })
                .collect();
            info!("Loaded {} delta updates", updates.len());
            updates
        }
        Err(e) => {
            if e.kind() == std::io::ErrorKind::NotFound {
                info!("Delta file not found (this is normal for first run)");
            } else {
                error!("Error opening delta file: {:?}", e);
            }
            Vec::new()
        }
    }
}

/// Apply delta updates to build the current state
/// Returns a HashMap of id -> Raindrop (deleted items are removed)
pub fn apply_delta_to_state(
    base_items: Vec<Raindrop>,
    updates: Vec<RaindropUpdate>,
) -> Vec<Raindrop> {
    use std::collections::HashMap;

    let base_count = base_items.len();
    let mut items_map: HashMap<i64, Raindrop> =
        base_items.into_iter().map(|item| (item.id, item)).collect();

    info!(
        "Applying {} updates to {} base items",
        updates.len(),
        base_count
    );

    for update in &updates {
        match update {
            RaindropUpdate::Upsert { item } => {
                info!(
                    "Delta: upserting item {} '{}'",
                    item.id,
                    &item.title[..item.title.len().min(30)]
                );
                items_map.insert(item.id, item.clone());
            }
            RaindropUpdate::Delete { id, timestamp } => {
                let existed = items_map.contains_key(id);
                info!(
                    "Delta: deleting item {} (existed: {}, ts: {})",
                    id, existed, timestamp
                );
                items_map.remove(id);
            }
        }
    }

    info!("After applying delta: {} items", items_map.len());
    items_map.into_values().collect()
}

/// Check if delta file exists
pub fn delta_exists() -> bool {
    Path::new(DELTA_FILE).exists()
}

/// Delete the delta file (e.g., after compaction)
pub fn delete_delta() -> Result<()> {
    let path = Path::new(DELTA_FILE);
    if path.exists() {
        fs::remove_file(path).context("Failed to delete delta file")?;
    }
    Ok(())
}

/// Compact the delta file by rewriting it with only the final state
/// This removes redundant updates and deleted items
pub fn compact_delta() -> Result<()> {
    let updates = load_raindrop_delta();
    if updates.is_empty() {
        return Ok(());
    }

    let items = apply_delta_to_state(Vec::new(), updates);

    // Delete old delta file and write fresh one
    delete_delta()?;
    append_raindrop_upserts(&items)?;

    Ok(())
}

/// Get current timestamp in ISO 8601 format
pub fn now_iso8601() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Convert Unix timestamp (seconds) to ISO 8601 string
pub fn unix_to_iso8601(timestamp: i64) -> String {
    use chrono::{TimeZone, Utc};
    Utc.timestamp_opt(timestamp, 0)
        .single()
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_else(|| Utc::now().to_rfc3339())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unix_to_iso8601() {
        let result = unix_to_iso8601(1609459200); // 2021-01-01 00:00:00 UTC
        assert!(result.starts_with("2021-01-01"));
    }

    #[test]
    fn test_now_iso8601() {
        let now = now_iso8601();
        assert!(now.contains("T")); // ISO 8601 format
        assert!(now.len() > 20);
    }
}
