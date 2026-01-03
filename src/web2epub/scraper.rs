//! HTML fetching and article extraction using dom_smoothie.

use reqwest::blocking::Client;
use thiserror::Error;
use url::Url;

#[derive(Debug, Error)]
pub enum ScraperError {
    #[error("HTTP request failed: {0}")]
    HttpError(#[from] reqwest::Error),

    #[error("Failed to extract readable content from the page")]
    ExtractionFailed,

    #[error("Invalid URL: {0}")]
    InvalidUrl(String),

    #[error("DOM parsing error: {0}")]
    DomError(String),
}

/// Extracted article content.
#[derive(Debug, Clone)]
pub struct Article {
    pub title: String,
    pub author: String,
    pub content_html: String,
}

/// Fetches HTML content from a URL.
pub fn fetch_html(client: &Client, url: &str) -> Result<String, ScraperError> {
    let response = client
        .get(url)
        .header("User-Agent", "pkt-tui-web2epub/0.1")
        .header("Accept", "text/html,application/xhtml+xml")
        .send()?;

    if !response.status().is_success() {
        return Err(ScraperError::HttpError(
            response.error_for_status().unwrap_err(),
        ));
    }

    Ok(response.text()?)
}

/// Extracts the hostname from a URL for use as a fallback author.
fn extract_hostname(url: &str) -> String {
    Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(String::from))
        .unwrap_or_else(|| "Unknown".to_string())
}

/// Extracts article content from HTML using dom_smoothie (Mozilla Readability port).
pub fn extract_article(html: &str, url: &str) -> Result<Article, ScraperError> {
    use dom_smoothie::{Config, Readability};

    let config = Config {
        max_elements_to_parse: 0, // 0 = no limit
        ..Default::default()
    };

    let mut readability = Readability::new(html, Some(url), Some(config))
        .map_err(|e| ScraperError::DomError(e.to_string()))?;

    let article = readability
        .parse()
        .map_err(|e| ScraperError::DomError(e.to_string()))?;

    let content = article.content.to_string();
    if content.is_empty() {
        return Err(ScraperError::ExtractionFailed);
    }

    // title is a String, check if empty
    let title = if article.title.is_empty() {
        "Untitled".to_string()
    } else {
        article.title
    };

    // byline is Option<String>
    let author = article
        .byline
        .filter(|b| !b.is_empty())
        .unwrap_or_else(|| extract_hostname(url));

    Ok(Article {
        title,
        author,
        content_html: content,
    })
}

/// Fetches a URL and extracts article content in one step.
pub fn fetch_and_extract(client: &Client, url: &str) -> Result<Article, ScraperError> {
    let html = fetch_html(client, url)?;
    extract_article(&html, url)
}
