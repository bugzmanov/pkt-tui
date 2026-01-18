//! HTML fetching and article extraction using dom_smoothie.

use regex::Regex;
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

/// Check if URL is from LessWrong or related sites that use Apollo SSR.
fn is_lesswrong_url(url: &str) -> bool {
    url.contains("lesswrong.com") || url.contains("alignmentforum.org")
}

/// Unescape Unicode sequences like \u003c to actual characters.
fn unescape_unicode(s: &str) -> String {
    let re = Regex::new(r"\\u([0-9a-fA-F]{4})").unwrap();
    re.replace_all(s, |caps: &regex::Captures| {
        let hex = &caps[1];
        if let Ok(code) = u32::from_str_radix(hex, 16) {
            if let Some(ch) = char::from_u32(code) {
                return ch.to_string();
            }
        }
        caps[0].to_string()
    })
    .to_string()
}

/// Extract article content from LessWrong's Apollo SSR data.
fn extract_lesswrong_article(html: &str) -> Option<Article> {
    // Extract title from: "post":{"result":{"_id":"...","slug":"...","title":"The Title"
    let title_re = Regex::new(r#""post":\s*\{\s*"result":\s*\{[^}]*"title"\s*:\s*"([^"]+)""#).ok()?;
    let title = title_re
        .captures(html)
        .map(|c| c.get(1).unwrap().as_str().to_string())
        .unwrap_or_else(|| "Untitled".to_string());

    // Extract author from: "displayName":"Author Name"
    let author_re = Regex::new(r#""user":\s*\{[^}]*"displayName"\s*:\s*"([^"]+)""#).ok()?;
    let author = author_re
        .captures(html)
        .map(|c| c.get(1).unwrap().as_str().to_string())
        .unwrap_or_else(|| "LessWrong".to_string());

    // Extract content from: "tableOfContents":{"html":"<content>"
    // The content ends at ","sections": or similar
    let content_re =
        Regex::new(r#""tableOfContents"\s*:\s*\{\s*"html"\s*:\s*"((?:[^"\\]|\\.)*)""#).ok()?;
    let content_match = content_re.captures(html)?;
    let raw_content = content_match.get(1)?.as_str();

    // Unescape Unicode and other escape sequences
    let content_html = unescape_unicode(raw_content)
        .replace("\\\"", "\"")
        .replace("\\/", "/")
        .replace("\\n", "\n")
        .replace("\\t", "\t");

    if content_html.is_empty() {
        return None;
    }

    Some(Article {
        title,
        author,
        content_html,
    })
}

/// Extracts article content from HTML using dom_smoothie (Mozilla Readability port).
pub fn extract_article(html: &str, url: &str) -> Result<Article, ScraperError> {
    // Try LessWrong-specific extraction first for Apollo SSR sites
    if is_lesswrong_url(url) {
        if let Some(article) = extract_lesswrong_article(html) {
            return Ok(article);
        }
        // Fall through to standard extraction if LessWrong-specific fails
    }

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
