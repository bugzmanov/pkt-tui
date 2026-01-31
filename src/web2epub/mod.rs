//! Web to EPUB conversion module.
//!
//! This module provides functionality to download web articles and convert them to EPUB format.
//! It mirrors the functionality of the bugzarch TypeScript tool.
//!
//! # Example
//!
//! ```ignore
//! use pkt_tui::web2epub;
//! use reqwest::blocking::Client;
//! use std::path::Path;
//!
//! let client = Client::new();
//! web2epub::convert_url_to_epub(
//!     &client,
//!     "https://example.com/article",
//!     Path::new("output.epub"),
//!     Path::new("temp_images"),
//! )?;
//! ```

mod epub;
mod images;
mod scraper;

use reqwest::blocking::Client;
use std::path::Path;
use thiserror::Error;

// Re-export the Article struct and extract function for external use
pub use scraper::Article;
pub use scraper::extract_article as extract_article_from_html;

#[derive(Debug, Error)]
pub enum Web2EpubError {
    #[error("Scraper error: {0}")]
    Scraper(#[from] scraper::ScraperError),

    #[error("Image processing error: {0}")]
    Image(#[from] images::ImageError),

    #[error("EPUB generation error: {0}")]
    Epub(#[from] epub::EpubError),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Converts a URL to an EPUB file.
///
/// This is the main entry point for the web2epub module. It:
/// 1. Fetches the HTML content from the URL
/// 2. Extracts the article content using readability algorithms
/// 3. Downloads and normalizes images
/// 4. Generates an EPUB file
///
/// # Arguments
///
/// * `client` - HTTP client for making requests
/// * `url` - The URL of the article to convert
/// * `output_path` - Path where the EPUB file will be written
/// * `temp_dir` - Directory for temporary image storage
///
/// # Returns
///
/// Returns `Ok(())` on success, or an error if any step fails.
pub fn convert_url_to_epub(
    client: &Client,
    url: &str,
    output_path: &Path,
    temp_dir: &Path,
) -> Result<(), Web2EpubError> {
    // Step 1: Fetch and extract article
    log::info!("web2epub: Fetching article from {}", url);
    let article = scraper::fetch_and_extract(client, url)?;
    log::info!(
        "web2epub: Extracted article: '{}' by '{}'",
        article.title,
        article.author
    );

    // Step 2: Normalize images (download to temp dir)
    log::info!("web2epub: Normalizing images to {:?}", temp_dir);
    let content_with_local_images =
        images::normalize_images(&article.content_html, url, temp_dir, client)?;

    // Create article with normalized content
    let normalized_article = Article {
        title: article.title,
        author: article.author,
        content_html: content_with_local_images,
    };

    // Step 3: Generate EPUB
    log::info!("web2epub: Generating EPUB at {:?}", output_path);
    epub::create_epub(&normalized_article, temp_dir, output_path, url)?;

    log::info!("web2epub: Successfully created EPUB");
    Ok(())
}

/// Fetches and extracts article content without creating an EPUB.
///
/// Useful for previewing or processing article content separately.
pub fn extract_article(client: &Client, url: &str) -> Result<Article, Web2EpubError> {
    Ok(scraper::fetch_and_extract(client, url)?)
}
