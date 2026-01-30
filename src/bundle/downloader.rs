//! Content download logic for offline bundles.

use super::types::{BundleItem, BundleItemType};
use anyhow::{bail, Context, Result};
use headless_chrome::{Browser, LaunchOptions};
use log::{debug, info};
use reqwest::blocking::Client;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;
use crate::utils;

/// Sanitize a filename to remove invalid characters
fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            _ => c,
        })
        .take(100) // Limit length
        .collect::<String>()
        .trim()
        .to_string()
}

/// Download PDF using Chrome browser to bypass Cloudflare protection.
/// Uses visible browser because Cloudflare heavily detects headless mode.
/// A Chrome window will briefly appear during download.
/// Returns the PDF bytes if successful.
pub fn download_pdf_via_browser(url: &str) -> Result<Vec<u8>> {
    info!("Attempting PDF download via browser: {}", url);

    // Create temp directory for downloads
    let temp_dir = std::env::temp_dir().join(format!("pdf_download_{}", std::process::id()));
    std::fs::create_dir_all(&temp_dir).context("Failed to create temp download directory")?;

    let launch_options = LaunchOptions::default_builder()
        .headless(false)  // Visible browser - Cloudflare heavily detects headless mode
        .sandbox(true)
        .build()
        .context("Failed to build browser launch options")?;

    let browser = Browser::new(launch_options).context("Failed to launch Chrome browser")?;

    let tab = browser.new_tab().context("Failed to create browser tab")?;

    // Set download behavior to download PDFs instead of displaying them
    tab.call_method(headless_chrome::protocol::cdp::Page::SetDownloadBehavior {
        behavior: headless_chrome::protocol::cdp::Page::SetDownloadBehaviorBehaviorOption::Allow,
        download_path: Some(temp_dir.to_string_lossy().to_string()),
    })
    .context("Failed to set download behavior")?;

    // Navigate to the URL - this may trigger a download (ERR_ABORTED) or show a page
    // ERR_ABORTED is expected when Chrome starts downloading instead of displaying
    let nav_result = tab.navigate_to(url);

    if let Err(ref e) = nav_result {
        let err_str = format!("{:?}", e);
        // ERR_ABORTED means download started - this is expected for PDF URLs
        if !err_str.contains("ERR_ABORTED") {
            return Err(anyhow::anyhow!("Failed to navigate: {}", e));
        }
        debug!("Navigation aborted (download likely started)");
    }

    // If navigation succeeded, wait for page and check for Cloudflare
    if nav_result.is_ok() {
        let _ = tab.wait_until_navigated();

        // Check if we're on a challenge page
        if let Ok(page_content) = tab.get_content() {
            let lower = page_content.to_lowercase();
            if lower.contains("just a moment") || lower.contains("checking your browser") {
                debug!("On Cloudflare challenge page, waiting for it to complete...");
                for _ in 0..15 {
                    std::thread::sleep(Duration::from_secs(2));
                    if let Ok(content) = tab.get_content() {
                        let l = content.to_lowercase();
                        if !l.contains("just a moment") && !l.contains("checking your browser") {
                            break;
                        }
                    }
                }
            }
        }
    }

    // Wait for download to complete (check for files in temp dir)
    debug!("Waiting for PDF download to complete...");
    let mut downloaded_file: Option<PathBuf> = None;
    for _ in 0..30 {
        std::thread::sleep(Duration::from_secs(1));

        // Look for PDF files in temp dir (ignore .crdownload partial files)
        if let Ok(entries) = std::fs::read_dir(&temp_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                if name.ends_with(".pdf") && !name.ends_with(".crdownload") {
                    // Check if file is still being written (size not changing)
                    if let Ok(meta1) = std::fs::metadata(&path) {
                        std::thread::sleep(Duration::from_millis(500));
                        if let Ok(meta2) = std::fs::metadata(&path) {
                            if meta1.len() == meta2.len() && meta1.len() > 0 {
                                downloaded_file = Some(path);
                                break;
                            }
                        }
                    }
                }
            }
        }
        if downloaded_file.is_some() {
            break;
        }
    }

    let pdf_path = downloaded_file.ok_or_else(|| {
        // Clean up temp dir
        let _ = std::fs::remove_dir_all(&temp_dir);
        anyhow::anyhow!("PDF download via browser timed out or failed")
    })?;

    // Read the downloaded PDF
    let content = std::fs::read(&pdf_path).context("Failed to read downloaded PDF")?;

    // Clean up temp dir
    let _ = std::fs::remove_dir_all(&temp_dir);

    // Verify we got a PDF
    if content.len() < 4 || &content[0..4] != b"%PDF" {
        bail!("Downloaded file is not a valid PDF");
    }

    info!(
        "Successfully downloaded PDF via browser ({} bytes)",
        content.len()
    );
    Ok(content)
}

/// Download web article and convert to EPUB using existing web2epub
pub fn download_article(
    client: &Client,
    item: &BundleItem,
    bundle_path: &Path,
) -> Result<PathBuf> {
    let filename = sanitize_filename(&item.title);
    let epub_path = bundle_path
        .join("articles")
        .join(format!("{}.epub", filename));
    let temp_dir = bundle_path.join("temp_images").join(format!("article_{}", item.id));

    // Create temp dir
    std::fs::create_dir_all(&temp_dir)?;

    // Use existing web2epub module
    crate::web2epub::convert_url_to_epub(client, &item.link, &epub_path, &temp_dir)
        .with_context(|| format!("Failed to convert article: {}", item.title))?;

    // Clean up temp dir
    if temp_dir.exists() {
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    Ok(epub_path)
}

/// Download video using yt-dlp
pub fn download_video(item: &BundleItem, bundle_path: &Path) -> Result<PathBuf> {
    let filename = sanitize_filename(&item.title);
    let output_template = bundle_path
        .join("videos")
        .join(format!("{}.%(ext)s", filename));

    let status = Command::new("yt-dlp")
        .arg("-f")
        .arg("bestvideo[ext=mp4]+bestaudio[ext=m4a]/best[ext=mp4]/best")
        .arg("-o")
        .arg(output_template.to_str().unwrap())
        .arg("--no-playlist")
        .arg(&item.link)
        .status()
        .with_context(|| "Failed to run yt-dlp. Is it installed?")?;

    if !status.success() {
        anyhow::bail!("yt-dlp failed for {}", item.link);
    }

    // Find the actual output file (yt-dlp fills in extension)
    let videos_dir = bundle_path.join("videos");
    for entry in std::fs::read_dir(&videos_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path
            .file_stem()
            .map(|s| s.to_string_lossy().starts_with(&filename))
            .unwrap_or(false)
        {
            return Ok(path);
        }
    }

    anyhow::bail!("Could not find downloaded video for {}", item.title)
}

/// Download PDF directly, with headless browser fallback for Cloudflare-protected URLs
pub fn download_pdf(client: &Client, item: &BundleItem, bundle_path: &Path) -> Result<PathBuf> {
    let filename = sanitize_filename(&item.title);
    let pdf_path = bundle_path
        .join("pdfs")
        .join(format!("{}.pdf", filename));
    // Normalize URL for different PDF sources
    let url = if let Some(alphaxiv_url) = utils::normalize_alphaxiv_pdf_url(&item.link) {
        alphaxiv_url
    } else {
        utils::normalize_acm_pdf_url(&item.link)
    };

    // First, try normal HTTP download
    let response = client
        .get(&url)
        .header(reqwest::header::ACCEPT, "application/pdf")
        .header(
            reqwest::header::USER_AGENT,
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36",
        )
        .send()
        .with_context(|| format!("Failed to download PDF: {}", url))?;

    if !response.status().is_success() {
        bail!("Failed to download PDF: HTTP {}", response.status());
    }

    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let content = response
        .bytes()
        .with_context(|| "Failed to read PDF content")?;

    // Check if we got HTML instead of PDF (Cloudflare protection)
    let is_cloudflare_blocked =
        !content_type.contains("application/pdf") && utils::looks_like_html(&content);

    let final_content = if is_cloudflare_blocked {
        info!(
            "PDF download blocked by Cloudflare, attempting headless browser fallback for: {}",
            url
        );
        download_pdf_via_browser(&url)?
    } else {
        content.to_vec()
    };

    std::fs::write(&pdf_path, final_content).with_context(|| "Failed to write PDF file")?;

    Ok(pdf_path)
}

/// Download content based on item type
pub fn download_item(
    client: &Client,
    item: &BundleItem,
    bundle_path: &Path,
) -> Result<PathBuf> {
    match item.item_type {
        BundleItemType::Article => download_article(client, item, bundle_path),
        BundleItemType::Video => download_video(item, bundle_path),
        BundleItemType::Pdf => download_pdf(client, item, bundle_path),
    }
}
