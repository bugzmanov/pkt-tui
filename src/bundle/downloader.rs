//! Content download logic for offline bundles.

use super::types::{BundleItem, BundleItemType};
use anyhow::{Context, Result};
use reqwest::blocking::Client;
use std::path::{Path, PathBuf};
use std::process::Command;
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

/// Download PDF directly
pub fn download_pdf(client: &Client, item: &BundleItem, bundle_path: &Path) -> Result<PathBuf> {
    let filename = sanitize_filename(&item.title);
    let pdf_path = bundle_path
        .join("pdfs")
        .join(format!("{}.pdf", filename));
    let url = utils::normalize_acm_pdf_url(&item.link);

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
        anyhow::bail!("Failed to download PDF: HTTP {}", response.status());
    }

    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let content = response
        .bytes()
        .with_context(|| "Failed to read PDF content")?;
    if !content_type.contains("application/pdf") && crate::utils::looks_like_html(&content) {
        anyhow::bail!("PDF download blocked or returned HTML");
    }

    std::fs::write(&pdf_path, content).with_context(|| "Failed to write PDF file")?;

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
