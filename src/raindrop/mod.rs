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
    Collection, CollectionRef, NewRaindrop, Raindrop, RaindropCache, RaindropType,
    UpdateRaindrop, User, COLLECTION_ALL, COLLECTION_TRASH, COLLECTION_UNSORTED,
};

use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

const CACHE_FILE: &str = "raindrop_data.json";

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
    let cache: RaindropCache =
        serde_json::from_str(&data).context("Failed to parse cache file")?;
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
