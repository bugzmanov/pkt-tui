//! Offline bundle creation module.
//!
//! This module provides functionality to create offline bundles from marked items,
//! including downloading content, generating thumbnails, and creating an HTML index.

pub mod downloader;
pub mod index_html;
pub mod thumbnail;
pub mod types;

pub use types::*;

use anyhow::Result;
use chrono::Local;
use reqwest::blocking::Client;
use std::fs;
use std::path::{Path, PathBuf};

/// Create the bundle directory structure
pub fn create_bundle_directories(bundle_path: &Path) -> Result<()> {
    fs::create_dir_all(bundle_path)?;
    fs::create_dir_all(bundle_path.join("articles"))?;
    fs::create_dir_all(bundle_path.join("videos"))?;
    fs::create_dir_all(bundle_path.join("pdfs"))?;
    fs::create_dir_all(bundle_path.join("thumbs"))?;
    Ok(())
}

/// Generate the default bundle path with today's date
pub fn default_bundle_path() -> PathBuf {
    let date = Local::now().format("%Y-%m-%d").to_string();
    PathBuf::from(format!("offline-bundle-{}", date))
}

/// Create the complete bundle from items
///
/// This is the main entry point for bundle creation. It:
/// 1. Creates the directory structure
/// 2. Downloads each item based on type
/// 3. Generates thumbnails where possible
/// 4. Creates the HTML index and JSON manifest
pub fn create_bundle<F>(
    items: &[BundleItem],
    bundle_path: &Path,
    client: &Client,
    mut progress_callback: F,
) -> Result<BundleManifest>
where
    F: FnMut(usize, &str, BundlingPhase),
{
    // Create directory structure
    create_bundle_directories(bundle_path)?;

    let mut manifest_items = Vec::new();
    let thumbs_dir = bundle_path.join("thumbs");

    for (i, item) in items.iter().enumerate() {
        progress_callback(i, &item.title, BundlingPhase::DownloadingItem(item.title.clone()));

        // Download the content
        let local_path = match downloader::download_item(client, item, bundle_path) {
            Ok(path) => path,
            Err(e) => {
                log::error!("Failed to download {}: {}", item.title, e);
                // Continue with other items
                continue;
            }
        };

        // Generate thumbnail
        progress_callback(
            i,
            &item.title,
            BundlingPhase::GeneratingThumbnail(item.title.clone()),
        );

        let thumbnail = match item.item_type {
            BundleItemType::Pdf => {
                thumbnail::generate_pdf_thumbnail(&local_path, &thumbs_dir, item.id)
                    .unwrap_or(None)
            }
            BundleItemType::Video => {
                thumbnail::download_video_thumbnail(client, item, bundle_path).unwrap_or(None)
            }
            BundleItemType::Article => {
                thumbnail::generate_epub_thumbnail(&local_path, &thumbs_dir, item.id)
                    .unwrap_or(None)
            }
        };

        // Get relative path for manifest
        let relative_path = local_path
            .strip_prefix(bundle_path)
            .unwrap_or(&local_path)
            .to_string_lossy()
            .to_string();

        manifest_items.push(ManifestItem {
            id: item.id,
            title: item.title.clone(),
            source_url: item.link.clone(),
            local_path: relative_path,
            item_type: item.item_type.as_str().to_string(),
            thumbnail,
            domain: item.domain.clone(),
        });
    }

    progress_callback(items.len(), "", BundlingPhase::GeneratingIndex);

    // Generate HTML index
    let created = Local::now().format("%Y-%m-%d %H:%M").to_string();
    let html = index_html::generate_index(&manifest_items, &created);
    fs::write(bundle_path.join("index.html"), html)?;

    // Generate manifest
    let manifest = BundleManifest {
        created: Local::now().to_rfc3339(),
        item_count: manifest_items.len(),
        items: manifest_items,
    };

    let manifest_json = serde_json::to_string_pretty(&manifest)?;
    fs::write(bundle_path.join("manifest.json"), manifest_json)?;

    // Clean up temp_images directory if it exists
    let temp_images = bundle_path.join("temp_images");
    if temp_images.exists() {
        let _ = fs::remove_dir_all(temp_images);
    }

    Ok(manifest)
}
