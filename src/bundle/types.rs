//! Types for the offline bundle feature.

use serde::Serialize;
use std::path::PathBuf;

/// Current state of the bundling process
#[derive(Clone)]
pub struct BundlingState {
    pub total_items: usize,
    pub current_item: usize,
    pub current_item_title: String,
    pub phase: BundlingPhase,
    pub errors: Vec<String>,
    pub bundle_path: PathBuf,
}

impl BundlingState {
    pub fn new(total_items: usize, bundle_path: PathBuf) -> Self {
        Self {
            total_items,
            current_item: 0,
            current_item_title: String::new(),
            phase: BundlingPhase::Preparing,
            errors: Vec::new(),
            bundle_path,
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub enum BundlingPhase {
    Preparing,
    DownloadingItem(String),
    GeneratingThumbnail(String),
    GeneratingIndex,
    Complete,
    Failed(String),
}

/// Represents a single item to be bundled
#[derive(Clone, Debug)]
pub struct BundleItem {
    pub id: i64,
    pub title: String,
    pub link: String,
    pub item_type: BundleItemType,
    pub domain: String,
    pub tags: Vec<String>,
}

#[derive(Clone, PartialEq, Debug)]
pub enum BundleItemType {
    Article,
    Video,
    Pdf,
}

impl BundleItemType {
    pub fn as_str(&self) -> &'static str {
        match self {
            BundleItemType::Article => "article",
            BundleItemType::Video => "video",
            BundleItemType::Pdf => "pdf",
        }
    }
}

/// Machine-readable manifest for iOS Shortcuts
#[derive(Serialize)]
pub struct BundleManifest {
    pub created: String,
    pub item_count: usize,
    pub items: Vec<ManifestItem>,
}

/// Individual item in the manifest
#[derive(Serialize, Clone)]
pub struct ManifestItem {
    pub id: i64,
    pub title: String,
    pub source_url: String,
    pub local_path: String,
    pub item_type: String,
    pub thumbnail: Option<String>,
    pub domain: String,
}
