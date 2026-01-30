use log::{debug, error};
use std::path::Path;

pub struct PDFData {
    pub title: Option<String>,
    #[allow(dead_code)]
    pub text: String,
}

/// Extract PDF title from document metadata
pub fn extract_pdf_title(path: &Path) -> anyhow::Result<Option<PDFData>> {
    use lopdf::Document;

    let doc = match Document::load(path) {
        Ok(doc) => doc,
        Err(e) => {
            debug!("Failed to load PDF for title extraction: {}", e);
            return Ok(None);
        }
    };

    // Try to get title from PDF metadata (Info dictionary)
    let title = doc
        .trailer
        .get(b"Info")
        .ok()
        .and_then(|info_ref| info_ref.as_reference().ok())
        .and_then(|ref_id| doc.get_object(ref_id).ok())
        .and_then(|info_obj| info_obj.as_dict().ok())
        .and_then(|info_dict| info_dict.get(b"Title").ok())
        .and_then(|title_obj| {
            // Handle different string encodings in PDFs
            match title_obj {
                lopdf::Object::String(bytes, _) => {
                    // Try UTF-16BE first (common for non-ASCII)
                    if bytes.len() >= 2 && bytes[0] == 0xFE && bytes[1] == 0xFF {
                        // UTF-16BE with BOM
                        let utf16: Vec<u16> = bytes[2..]
                            .chunks(2)
                            .filter_map(|chunk| {
                                if chunk.len() == 2 {
                                    Some(u16::from_be_bytes([chunk[0], chunk[1]]))
                                } else {
                                    None
                                }
                            })
                            .collect();
                        String::from_utf16(&utf16).ok()
                    } else {
                        // Try as UTF-8 or Latin-1
                        String::from_utf8(bytes.clone())
                            .or_else(|_| {
                                // Fallback to Latin-1
                                Ok::<String, std::string::FromUtf8Error>(
                                    bytes.iter().map(|&b| b as char).collect(),
                                )
                            })
                            .ok()
                    }
                }
                _ => None,
            }
        })
        .filter(|t| !t.trim().is_empty());

    if let Some(ref t) = title {
        debug!("Extracted PDF title: {}", t);
    } else {
        debug!("No title found in PDF metadata");
    }

    Ok(Some(PDFData {
        title,
        text: String::new(),
    }))
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

/// Check if URL is an alphaxiv.org paper page (PDF wrapper)
pub fn is_alphaxiv_url(url: &str) -> bool {
    url.contains("alphaxiv.org/abs/")
}

/// Convert alphaxiv.org paper URL to direct PDF URL
/// e.g., https://www.alphaxiv.org/abs/2601.10825 -> https://fetcher.alphaxiv.org/v2/pdf/2601.10825v1.pdf
pub fn normalize_alphaxiv_pdf_url(url: &str) -> Option<String> {
    if !is_alphaxiv_url(url) {
        return None;
    }

    // Extract paper ID from URL like https://www.alphaxiv.org/abs/2601.10825
    let paper_id = url.split("/abs/").nth(1)?.split(&['?', '#', '/'][..]).next()?;

    // Default to v1 if no version specified
    let versioned_id = if paper_id.contains('v') {
        paper_id.to_string()
    } else {
        format!("{}v1", paper_id)
    };

    Some(format!(
        "https://fetcher.alphaxiv.org/v2/pdf/{}.pdf",
        versioned_id
    ))
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
