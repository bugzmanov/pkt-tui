//! Background download worker thread.

use super::config::BackgroundDownloadConfig;
use super::state::{BackgroundDownloadState, DownloadFailure};
use crate::raindrop::{Raindrop, RaindropAPI};
use crate::utils;
use anyhow::Result;
use chrono::{DateTime, TimeDelta, Utc};
use log::{error, info, warn};
use pkt_tui::bundle;
use pkt_tui::web2epub;
use reqwest::blocking::Client;
use std::fs;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread;

const EPUB_DIR: &str = "epubs";
const PDF_DIR: &str = "pdfs";
const RATE_LIMIT_MS: u64 = 500;

/// Start background download in a new thread.
///
/// Returns immediately after spawning the thread.
pub fn start_background_download(
    api: Arc<RaindropAPI>,
    config: BackgroundDownloadConfig,
    state: BackgroundDownloadState,
    items: Vec<Raindrop>,
) {
    // Check if already running
    if state.is_running.load(Ordering::SeqCst) {
        info!("Background download already running, skipping");
        return;
    }

    // Filter items: created within lookback_days, not already downloaded, and is PDF or article
    let lookback_delta = TimeDelta::try_days(config.lookback_days as i64).unwrap_or_default();
    let cutoff = Utc::now() - lookback_delta;
    info!(
        "Background download filter: lookback_days={}, cutoff={}",
        config.lookback_days, cutoff
    );

    let mut recent_count = 0;
    let mut downloadable_count = 0;
    let mut not_downloaded_count = 0;

    let items_to_download: Vec<Raindrop> = items
        .into_iter()
        .filter(|item| {
            // Check if item is recent enough
            let created_dt = DateTime::parse_from_rfc3339(&item.created)
                .or_else(|_| DateTime::parse_from_rfc3339(&item.created.replace("Z", "+00:00")))
                .ok();
            let is_recent = created_dt
                .map(|dt| dt.with_timezone(&Utc) >= cutoff)
                .unwrap_or(false);

            // Check if downloadable type
            let item_type = item.type_str();
            let is_downloadable = item_type == "pdf" || item_type == "article";

            // Check if file actually exists (not just the tag)
            let file_exists = match item_type {
                "pdf" => {
                    let filename = if !item.title.trim().is_empty() {
                        format!("{}.pdf", utils::sanitize_filename(&item.title))
                    } else {
                        "".to_string()
                    };
                    !filename.is_empty() && Path::new(PDF_DIR).join(&filename).exists()
                }
                "article" => {
                    let cache_id = item.id.to_string();
                    Path::new(EPUB_DIR).join(format!("{}.epub", cache_id)).exists()
                }
                _ => false,
            };

            if is_recent {
                recent_count += 1;
            }
            if is_downloadable {
                downloadable_count += 1;
            }
            if !file_exists {
                not_downloaded_count += 1;
            }

            is_recent && !file_exists && is_downloadable
        })
        .collect();

    info!(
        "Background download stats: recent={}, downloadable={}, not_downloaded={}, final={}",
        recent_count, downloadable_count, not_downloaded_count, items_to_download.len()
    );

    if items_to_download.is_empty() {
        info!("No items to download in background");
        return;
    }

    info!(
        "Starting background download of {} items",
        items_to_download.len()
    );

    // Update state
    state.reset();
    state
        .total
        .store(items_to_download.len(), Ordering::SeqCst);
    state.is_running.store(true, Ordering::SeqCst);

    // Spawn worker thread
    thread::spawn(move || {
        // Create a new tokio runtime for async operations in this thread
        let runtime = match tokio::runtime::Runtime::new() {
            Ok(rt) => rt,
            Err(e) => {
                error!("Failed to create tokio runtime: {}", e);
                state.is_running.store(false, Ordering::SeqCst);
                return;
            }
        };

        // Create HTTP client for downloads
        let download_client = Client::new();

        for (index, item) in items_to_download.iter().enumerate() {
            // Update progress
            state.current.store(index + 1, Ordering::SeqCst);
            if let Ok(mut title) = state.current_title.lock() {
                *title = item.display_title().to_string();
            }

            info!(
                "Downloading {}/{}: {} ({})",
                index + 1,
                items_to_download.len(),
                item.display_title(),
                item.type_str()
            );

            // Attempt download with retry
            let result = download_item(&download_client, &item);
            let result = match result {
                Ok(_) => Ok(()),
                Err(e) => {
                    warn!("First attempt failed for {}: {}, retrying...", item.title, e);
                    thread::sleep(std::time::Duration::from_millis(RATE_LIMIT_MS));
                    download_item(&download_client, &item)
                }
            };

            match result {
                Ok(_) => {
                    // Add "downloaded" tag via API
                    let tag_result = runtime.block_on(api.add_tag(&item, "downloaded"));
                    if let Err(e) = tag_result {
                        error!("Failed to add 'downloaded' tag to {}: {}", item.title, e);
                    } else {
                        info!("Successfully downloaded and tagged: {}", item.title);
                    }
                }
                Err(e) => {
                    error!("Failed to download {}: {}", item.title, e);
                    if let Ok(mut failures) = state.failures.lock() {
                        failures.push(DownloadFailure {
                            title: item.title.clone(),
                            url: item.link.clone(),
                            error: e.to_string(),
                        });
                    }
                }
            }

            // Rate limit
            thread::sleep(std::time::Duration::from_millis(RATE_LIMIT_MS));
        }

        // Mark as complete
        state.is_running.store(false, Ordering::SeqCst);
        state.is_complete.store(true, Ordering::SeqCst);
        state.needs_refresh_prompt.store(true, Ordering::SeqCst);

        let failure_count = state.failure_count();
        if failure_count > 0 {
            info!(
                "Background download complete with {} failures",
                failure_count
            );
        } else {
            info!("Background download complete");
        }
    });
}

/// Download a single item (PDF or article).
fn download_item(client: &Client, item: &Raindrop) -> Result<()> {
    match item.type_str() {
        "pdf" => download_pdf(client, item),
        "article" => download_article(client, item),
        _ => Ok(()), // Skip unsupported types
    }
}

/// Download a PDF file.
fn download_pdf(client: &Client, item: &Raindrop) -> Result<()> {
    fs::create_dir_all(PDF_DIR)?;

    // Normalize URL for different PDF sources
    let url = if let Some(alphaxiv_url) = utils::normalize_alphaxiv_pdf_url(&item.link) {
        alphaxiv_url
    } else if let Some(arxiv_url) = utils::normalize_arxiv_pdf_url(&item.link) {
        arxiv_url
    } else {
        utils::normalize_acm_pdf_url(&item.link)
    };

    // Generate filename
    let mut filename = if !item.title.trim().is_empty() {
        utils::sanitize_filename(&item.title)
    } else {
        url.split('?')
            .next()
            .unwrap_or(&url)
            .split('/')
            .last()
            .unwrap_or("download.pdf")
            .replace("%20", "_")
    };
    if !filename.ends_with(".pdf") {
        filename.push_str(".pdf");
    }

    let path = std::path::PathBuf::from(PDF_DIR).join(&filename);

    // Skip if already exists
    if path.exists() {
        info!("PDF already exists: {}", path.display());
        return Ok(());
    }

    // Download
    let response = client
        .get(&url)
        .header(reqwest::header::ACCEPT, "application/pdf")
        .header(
            reqwest::header::USER_AGENT,
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36",
        )
        .send()?;

    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let content = response.bytes()?;

    // Check if we got HTML instead of PDF (Cloudflare protection)
    let final_content = if !content_type.contains("application/pdf") && utils::looks_like_html(&content) {
        bundle::downloader::download_pdf_via_browser(&url)?
    } else {
        content.to_vec()
    };

    fs::write(&path, final_content)?;
    info!("Downloaded PDF: {}", path.display());

    Ok(())
}

/// Download an article as EPUB.
fn download_article(client: &Client, item: &Raindrop) -> Result<()> {
    fs::create_dir_all(EPUB_DIR)?;

    // Use item ID for cache_id (matches manual download behavior in main.rs)
    // main.rs uses make_epub_cache_id(&item.id.to_string()) which for numeric IDs just returns the ID
    let cache_id = item.id.to_string();
    let epub_path = Path::new(EPUB_DIR).join(format!("{}.epub", cache_id));

    // Skip if already exists
    if epub_path.exists() {
        info!("EPUB already exists: {}", epub_path.display());
        return Ok(());
    }

    // Normalize URL
    let normalized_url = normalize_url_for_epub(&item.link);
    let temp_images_dir = Path::new("temp_images").join(&cache_id);

    // Convert to EPUB
    web2epub::convert_url_to_epub(client, &normalized_url, &epub_path, &temp_images_dir)?;

    // Clean up temp images
    if temp_images_dir.exists() {
        let _ = fs::remove_dir_all(&temp_images_dir);
    }

    info!("Downloaded EPUB: {}", epub_path.display());

    Ok(())
}

/// Normalize URL for EPUB conversion (matches main.rs behavior).
fn normalize_url_for_epub(url: &str) -> String {
    let trimmed = url.trim();
    if !(trimmed.starts_with("http://") || trimmed.starts_with("https://")) {
        return trimmed.to_string();
    }

    let without_fragment = trimmed.split('#').next().unwrap_or(trimmed);
    let (base, query) = match without_fragment.split_once('?') {
        Some((base, query)) => (base, Some(query)),
        None => (without_fragment, None),
    };

    if base.ends_with('/') {
        match query {
            Some(q) => format!("{}?{}", base, q),
            None => base.to_string(),
        }
    } else {
        without_fragment.to_string()
    }
}
