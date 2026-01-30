//! HTML fetching and article extraction using dom_smoothie.

use headless_chrome::{Browser, LaunchOptions};
use regex::Regex;
use reqwest::blocking::Client;
use scraper::{Html, Selector};
use std::time::Duration;
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

    #[error("Browser error: {0}")]
    BrowserError(String),
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

/// Detects if HTML is likely a JavaScript SPA that requires browser rendering.
fn is_spa_html(html: &str) -> bool {
    let lower = html.to_lowercase();

    // Check for common SPA indicators
    let has_noscript_js_required = lower.contains("you need to enable javascript")
        || lower.contains("please enable javascript")
        || lower.contains("javascript is required");

    // Check for empty body with just a root div (React/Vue/Angular pattern)
    let has_app_root_only = (lower.contains("<div id=\"root\">")
        || lower.contains("<div id=\"app\">")
        || lower.contains("<div id=\"__next\">")
        || lower.contains("id=\"ramp-builders\""))
        && !lower.contains("<article")
        && !lower.contains("<main");

    // Check body content - SPAs typically have very little content in body
    if let Some(body_start) = lower.find("<body") {
        if let Some(body_end) = lower.find("</body>") {
            let body_content = &lower[body_start..body_end];
            // If body has less than 500 chars of actual content (excluding scripts), likely SPA
            let content_without_scripts: String = body_content
                .split("<script")
                .next()
                .unwrap_or("")
                .chars()
                .filter(|c| c.is_alphanumeric())
                .collect();
            if content_without_scripts.len() < 200 && has_app_root_only {
                return true;
            }
        }
    }

    has_noscript_js_required || has_app_root_only
}

/// Fetches HTML content from a URL using a headless browser.
/// This is used as a fallback for JavaScript SPAs that require rendering.
pub fn fetch_html_via_browser(url: &str) -> Result<String, ScraperError> {
    log::info!("Attempting to fetch page via browser (SPA fallback): {}", url);

    let launch_options = LaunchOptions::default_builder()
        .headless(true)
        .sandbox(true)
        .build()
        .map_err(|e| ScraperError::BrowserError(format!("Failed to build launch options: {}", e)))?;

    let browser = Browser::new(launch_options)
        .map_err(|e| ScraperError::BrowserError(format!("Failed to launch browser: {}", e)))?;

    let tab = browser
        .new_tab()
        .map_err(|e| ScraperError::BrowserError(format!("Failed to create tab: {}", e)))?;

    // Navigate to the URL
    tab.navigate_to(url)
        .map_err(|e| ScraperError::BrowserError(format!("Failed to navigate: {}", e)))?;

    // Wait for page to load
    tab.wait_until_navigated()
        .map_err(|e| ScraperError::BrowserError(format!("Navigation timeout: {}", e)))?;

    // Wait a bit for JavaScript to render content
    std::thread::sleep(Duration::from_secs(2));

    // Check for Cloudflare challenge and wait if needed
    for _ in 0..10 {
        if let Ok(content) = tab.get_content() {
            let lower = content.to_lowercase();
            if lower.contains("just a moment") || lower.contains("checking your browser") {
                log::debug!("Cloudflare challenge detected, waiting...");
                std::thread::sleep(Duration::from_secs(2));
            } else {
                break;
            }
        }
    }

    // Additional wait for SPA content to render
    std::thread::sleep(Duration::from_secs(1));

    // Get the rendered HTML
    let html = tab
        .get_content()
        .map_err(|e| ScraperError::BrowserError(format!("Failed to get page content: {}", e)))?;

    log::info!("Successfully fetched page via browser ({} bytes)", html.len());
    Ok(html)
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

/// Check if URL requires browser rendering (heavy JS sites that can't be scraped statically).
fn requires_browser_rendering(url: &str) -> bool {
    url.contains("x.com") || url.contains("twitter.com")
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

/// Checks if an image element looks like content (not a small icon/button image).
fn is_content_image(img: &scraper::ElementRef) -> bool {
    // Check width attribute
    if let Some(width) = img.value().attr("width") {
        // "100%" or large percentage = content
        if width.ends_with('%') {
            if let Ok(pct) = width.trim_end_matches('%').parse::<u32>() {
                if pct >= 50 {
                    return true;
                }
            }
        }
        // Explicit pixel width - icons are typically <= 48px
        if let Ok(px) = width.parse::<u32>() {
            return px > 48;
        }
    }

    // Check height attribute for small icons
    if let Some(height) = img.value().attr("height") {
        if let Ok(px) = height.parse::<u32>() {
            if px <= 48 {
                return false;
            }
        }
    }

    // Check src - data URIs for tiny images, or icon-like paths
    if let Some(src) = img.value().attr("src") {
        let src_lower = src.to_lowercase();
        // Skip obvious icons
        if src_lower.contains("icon") || src_lower.contains("logo") || src_lower.contains("favicon") {
            return false;
        }
        // Small inline SVGs are usually icons
        if src.starts_with("data:image/svg") && src.len() < 500 {
            return false;
        }
    }

    // No explicit small size = assume content
    true
}

/// Pre-processes HTML to convert custom "steps" components to ordered lists.
/// Sites like code.claude.com use `<div role="list" class="steps">` with custom
/// data-component-part attributes that Readability doesn't understand.
fn convert_steps_to_list(html: &str) -> String {
    let document = Html::parse_document(html);
    // Match div with role="list" that has "steps" in its class attribute
    let steps_selector = Selector::parse(r#"div[role="list"][class*="steps"]"#).unwrap();
    // Match div with role="listitem" that has "step" in its class attribute
    let step_selector = Selector::parse(r#"div[role="listitem"][class*="step"]"#).unwrap();
    let title_selector = Selector::parse(r#"[data-component-part="step-title"]"#).unwrap();
    let content_selector = Selector::parse(r#"[data-component-part="step-content"]"#).unwrap();

    let mut result = html.to_string();
    let mut replacements = Vec::new();

    for steps_container in document.select(&steps_selector) {
        let mut list_items = Vec::new();

        for step in steps_container.select(&step_selector) {
            let title = step
                .select(&title_selector)
                .next()
                .map(|el| el.text().collect::<String>())
                .unwrap_or_default();

            let content = step
                .select(&content_selector)
                .next()
                .map(|el| el.inner_html())
                .unwrap_or_default();

            if !title.is_empty() || !content.is_empty() {
                let item = if !title.is_empty() && !content.is_empty() {
                    format!("<li><strong>{}</strong>: {}</li>", title.trim(), content.trim())
                } else if !title.is_empty() {
                    format!("<li><strong>{}</strong></li>", title.trim())
                } else {
                    format!("<li>{}</li>", content.trim())
                };
                list_items.push(item);
            }
        }

        if !list_items.is_empty() {
            let ordered_list = format!("<ol>{}</ol>", list_items.join("\n"));
            replacements.push((list_items.len(), ordered_list));
        }
    }

    // Use regex to find and replace the steps containers
    // Match: <div role="list" class="steps...">...</div> (the entire container)
    if !replacements.is_empty() {
        // Find opening tags of steps containers
        let steps_pattern =
            Regex::new(r#"<div[^>]*role="list"[^>]*class="[^"]*steps[^"]*"[^>]*>"#).unwrap();

        for (item_count, replacement) in replacements {
            if let Some(mat) = steps_pattern.find(&result) {
                let start = mat.start();
                // Find matching closing </div> - need to count nested divs
                if let Some(end) = find_matching_close_tag(&result[start..], "div") {
                    let full_end = start + end;
                    log::debug!(
                        "Converted steps component to ordered list with {} items",
                        item_count
                    );
                    result = format!("{}{}{}", &result[..start], replacement, &result[full_end..]);
                }
            }
        }
    }

    result
}

/// Find the position after the matching closing tag, handling nested tags.
fn find_matching_close_tag(html: &str, tag: &str) -> Option<usize> {
    let open_tag = format!("<{}", tag);
    let close_tag = format!("</{}>", tag);
    let mut depth = 0;
    let mut char_indices: Vec<(usize, char)> = html.char_indices().collect();
    let mut i = 0;

    while i < char_indices.len() {
        let (byte_pos, _) = char_indices[i];
        let remaining = &html[byte_pos..];

        if remaining.starts_with(&open_tag) {
            depth += 1;
            // Skip past the open tag
            let skip_bytes = open_tag.len();
            while i < char_indices.len() && char_indices[i].0 < byte_pos + skip_bytes {
                i += 1;
            }
        } else if remaining.starts_with(&close_tag) {
            depth -= 1;
            if depth == 0 {
                return Some(byte_pos + close_tag.len());
            }
            // Skip past the close tag
            let skip_bytes = close_tag.len();
            while i < char_indices.len() && char_indices[i].0 < byte_pos + skip_bytes {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    None
}

/// Pre-processes HTML to unwrap content images from button/overlay containers.
/// Uses regex to find button tags containing img tags and extract just the img.
fn unwrap_images_from_buttons(html: &str) -> String {
    let document = Html::parse_document(html);
    let button_selector = Selector::parse("button").unwrap();
    let img_selector = Selector::parse("img").unwrap();

    let mut result = html.to_string();
    let mut replacements = Vec::new();

    // Collect image srcs that need unwrapping
    for button in document.select(&button_selector) {
        if let Some(img) = button.select(&img_selector).next() {
            if is_content_image(&img) {
                if let Some(src) = img.value().attr("src") {
                    replacements.push(src.to_string());
                }
            }
        }
    }

    // For each src, find the button containing it and replace with just the img
    for src in replacements {
        // Escape special regex characters in src
        let escaped_src = regex::escape(&src);

        // Pattern: <button ...> ... <img ... src="..."> ... </button>
        // Using [\s\S]*? to match any character including newlines (non-greedy)
        let pattern = format!(
            r#"<button[^>]*>[\s\S]*?<img([^>]*src=["']{}["'][^>]*)>[\s\S]*?</button>"#,
            escaped_src
        );

        if let Ok(re) = Regex::new(&pattern) {
            if let Some(caps) = re.captures(&result) {
                // Reconstruct the img tag from captured attributes
                let img_attrs = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                let img_tag = format!("<img{}>", img_attrs);
                let full_match = caps.get(0).unwrap().as_str();
                result = result.replace(full_match, &img_tag);
                log::debug!("Unwrapped content image from button: {}", src);
            }
        }
    }

    result
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

    // Pre-process HTML to convert custom components to standard HTML
    let html_with_lists = convert_steps_to_list(html);
    let preprocessed_html = unwrap_images_from_buttons(&html_with_lists);

    use dom_smoothie::{Config, Readability};

    let config = Config {
        max_elements_to_parse: 0, // 0 = no limit
        ..Default::default()
    };

    let mut readability = Readability::new(preprocessed_html, Some(url), Some(config))
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
/// Falls back to browser rendering for JavaScript SPAs.
pub fn fetch_and_extract(client: &Client, url: &str) -> Result<Article, ScraperError> {
    // Some sites always require browser rendering (heavy JS SPAs)
    if requires_browser_rendering(url) {
        log::info!("URL requires browser rendering: {}", url);
        let rendered_html = fetch_html_via_browser(url)?;
        return extract_article(&rendered_html, url);
    }

    let html = fetch_html(client, url)?;

    // Try standard extraction first
    match extract_article(&html, url) {
        Ok(article) => Ok(article),
        Err(e) => {
            // Check if this looks like a SPA that needs browser rendering
            if is_spa_html(&html) {
                log::info!("Page appears to be a JavaScript SPA, trying browser fallback");
                let rendered_html = fetch_html_via_browser(url)?;
                extract_article(&rendered_html, url)
            } else {
                Err(e)
            }
        }
    }
}
