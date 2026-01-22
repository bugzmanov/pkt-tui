use log::{debug, error};
use std::path::Path;

pub struct PDFData {
    pub title: Option<String>,
    #[allow(dead_code)]
    pub text: String,
}

pub fn extract_pdf_title(_path: &Path) -> anyhow::Result<Option<PDFData>> {
    // Stub implementation as PDF support is removed
    Ok(None)
}

pub fn normalize_acm_pdf_url(url: &str) -> String {
    if !url.contains("dl.acm.org") {
        return url.to_string();
    }

    let (base, fragment) = match url.split_once('#') {
        Some((base, fragment)) => (base, Some(fragment)),
        None => (url, None),
    };

    let mut normalized = base.to_string();
    if normalized.contains("/doi/epdf/") {
        normalized = normalized.replace("/doi/epdf/", "/doi/pdf/");
    }

    if normalized.contains("/doi/pdf/") && !normalized.contains("download=true") {
        if normalized.contains('?') {
            normalized.push('&');
        } else {
            normalized.push('?');
        }
        normalized.push_str("download=true");
    }

    if let Some(fragment) = fragment {
        normalized.push('#');
        normalized.push_str(fragment);
    }

    normalized
}

pub fn is_acm_pdf_url(url: &str) -> bool {
    url.contains("dl.acm.org/doi/epdf/") || url.contains("dl.acm.org/doi/pdf/")
}

pub fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            _ => c,
        })
        .take(120)
        .collect::<String>()
        .trim()
        .to_string()
}

pub fn looks_like_html(bytes: &[u8]) -> bool {
    let prefix = bytes.iter().take(256).copied().collect::<Vec<u8>>();
    let lower = String::from_utf8_lossy(&prefix).to_lowercase();
    lower.contains("<!doctype html")
        || lower.contains("<html")
        || lower.contains("just a moment")
        || lower.contains("cf-ray")
}

// YouTube channel cache utilities

use std::collections::HashMap;
use std::fs;

const YOUTUBE_CHANNEL_CACHE_FILE: &str = "youtube_channel_cache.json";

fn get_cache_path() -> std::path::PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("pviewer")
        .join(YOUTUBE_CHANNEL_CACHE_FILE)
}

pub fn load_youtube_channel_cache() -> HashMap<String, String> {
    let cache_path = get_cache_path();
    debug!("Loading YouTube channel cache from {:?}", cache_path);

    match fs::read_to_string(&cache_path) {
        Ok(content) => {
            match serde_json::from_str(&content) {
                Ok(cache) => cache,
                Err(e) => {
                    error!("Failed to parse YouTube channel cache: {}", e);
                    HashMap::new()
                }
            }
        }
        Err(_) => {
            debug!("No YouTube channel cache file found, starting fresh");
            HashMap::new()
        }
    }
}

pub fn save_youtube_channel_cache(cache: &HashMap<String, String>) {
    let cache_path = get_cache_path();
    debug!("Saving YouTube channel cache to {:?}", cache_path);

    // Ensure parent directory exists
    if let Some(parent) = cache_path.parent() {
        if let Err(e) = fs::create_dir_all(parent) {
            error!("Failed to create cache directory: {}", e);
            return;
        }
    }

    match serde_json::to_string_pretty(cache) {
        Ok(json) => {
            if let Err(e) = fs::write(&cache_path, json) {
                error!("Failed to write YouTube channel cache: {}", e);
            }
        }
        Err(e) => {
            error!("Failed to serialize YouTube channel cache: {}", e);
        }
    }
}

/// Fetch YouTube channel name via oEmbed API
pub fn fetch_youtube_channel(client: &reqwest::blocking::Client, url: &str) -> Option<String> {
    let oembed_url = format!("https://www.youtube.com/oembed?url={}&format=json",
        urlencoding::encode(url));

    debug!("Fetching YouTube channel for: {}", url);

    match client.get(&oembed_url).send() {
        Ok(response) => {
            if !response.status().is_success() {
                debug!("oEmbed request failed with status: {}", response.status());
                return None;
            }

            match response.json::<serde_json::Value>() {
                Ok(json) => {
                    json.get("author_name")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                }
                Err(e) => {
                    debug!("Failed to parse oEmbed response: {}", e);
                    None
                }
            }
        }
        Err(e) => {
            debug!("oEmbed request failed: {}", e);
            None
        }
    }
}

/// Check if a URL is a YouTube video
pub fn is_youtube_url(url: &str) -> bool {
    url.contains("youtube.com") || url.contains("youtu.be")
}
