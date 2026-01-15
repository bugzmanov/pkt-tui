//! Image downloading and normalization for EPUB generation.

use base64::Engine;
use reqwest::blocking::Client;
use scraper::{Html, Selector};
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use thiserror::Error;
use url::Url;

#[derive(Debug, Error)]
pub enum ImageError {
    #[error("HTTP request failed: {0}")]
    HttpError(#[from] reqwest::Error),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Invalid URL: {0}")]
    InvalidUrl(String),
}

/// Known image extensions that are supported.
const KNOWN_IMAGE_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "webp", "svg", "avif", "bmp", "tiff",
];

/// Extracts file extension from MIME type.
fn extension_from_mime(mime_type: &str) -> Option<&'static str> {
    if !mime_type.starts_with("image/") {
        return None;
    }

    let raw = mime_type.strip_prefix("image/")?;
    match raw {
        "jpeg" => Some("jpg"),
        "png" => Some("png"),
        "gif" => Some("gif"),
        "webp" => Some("webp"),
        "svg+xml" => Some("svg"),
        "avif" => Some("avif"),
        "bmp" => Some("bmp"),
        "tiff" => Some("tiff"),
        _ => None,
    }
}

/// Gets image extension from URL path.
fn get_url_image_extension(image_url: &str) -> Option<String> {
    let url = Url::parse(image_url).ok()?;
    let path = url.path();
    let ext = path.rsplit('.').next()?.to_lowercase();
    if KNOWN_IMAGE_EXTENSIONS.contains(&ext.as_str()) {
        Some(ext)
    } else {
        None
    }
}

/// Parses a data URL and returns the decoded bytes and extension.
fn parse_data_url(data_url: &str) -> Option<(Vec<u8>, &'static str)> {
    // Format: data:image/png;base64,<data>
    let rest = data_url.strip_prefix("data:")?;
    let (mime_and_encoding, data) = rest.split_once(',')?;

    if !mime_and_encoding.ends_with(";base64") {
        return None;
    }

    let mime = mime_and_encoding.strip_suffix(";base64")?;
    let extension = extension_from_mime(mime)?;

    let decoded = base64::engine::general_purpose::STANDARD
        .decode(data)
        .ok()?;

    Some((decoded, extension))
}

/// Downloads an image and saves it to the temp directory.
fn fetch_image_to_file(
    client: &Client,
    image_url: &str,
    temp_dir: &Path,
    file_index: usize,
) -> Option<String> {
    log::debug!("Fetching image: {}", image_url);

    let response = match client
        .get(image_url)
        .header("User-Agent", "pkt-tui-web2epub/0.1")
        .header("Accept", "image/*")
        .send()
    {
        Ok(r) => r,
        Err(e) => {
            log::warn!("Failed to fetch image {}: {}", image_url, e);
            return None;
        }
    };

    if !response.status().is_success() {
        log::warn!("Image fetch failed with status {}: {}", response.status(), image_url);
        return None;
    }

    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let mime_type = content_type.split(';').next().unwrap_or("").trim();
    let extension = match extension_from_mime(mime_type) {
        Some(ext) => ext,
        None => {
            log::warn!("Unknown image MIME type '{}' for {}", mime_type, image_url);
            return None;
        }
    };

    let bytes = response.bytes().ok()?;
    let filename = format!("remote-image-{}.{}", file_index, extension);
    let file_path = temp_dir.join(&filename);

    fs::write(&file_path, &bytes).ok()?;

    Some(filename)
}

/// Represents an image found in the HTML.
#[derive(Debug)]
struct ImageInfo {
    /// Original src or data-src value
    original_src: String,
    /// Attribute name where the original src was found
    source_attr: String,
    /// Optional placeholder src attribute value
    placeholder_src: Option<String>,
    /// Local filename after processing (None if removed)
    local_filename: Option<String>,
}

#[derive(Debug)]
struct ImageSource {
    attr_name: &'static str,
    raw_value: String,
    resolved_value: String,
    placeholder_src: Option<String>,
}

/// Extracts the first URL from a srcset attribute.
///
/// srcset format: "url1 descriptor1, url2 descriptor2, ..."
/// where descriptors are like "424w" (width) or "2x" (pixel density).
///
/// The tricky part is that URLs can contain commas (e.g., Substack CDN URLs),
/// so we can't just split on commas. Instead, we look for the pattern
/// `\s+\d+[wx]\s*,` which marks the end of a srcset entry.
fn first_src_from_srcset(srcset: &str) -> Option<String> {
    let srcset = srcset.trim();
    if srcset.is_empty() {
        return None;
    }

    // Strategy: Find the first width/density descriptor (e.g., "424w" or "2x")
    // that's followed by a comma or end of string. The URL is everything before
    // that descriptor.

    // Look for patterns like " 424w," or " 424w" at end, or " 2x," etc.
    // The descriptor is always preceded by whitespace and optionally followed by comma.

    let mut chars: Vec<char> = srcset.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        // Look for whitespace followed by digits followed by 'w' or 'x'
        if chars[i].is_whitespace() {
            let ws_start = i;
            // Skip whitespace
            while i < chars.len() && chars[i].is_whitespace() {
                i += 1;
            }
            if i >= chars.len() {
                break;
            }

            // Check if we have digits followed by 'w' or 'x'
            let digit_start = i;
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }

            if i > digit_start && i < chars.len() {
                let descriptor_char = chars[i].to_ascii_lowercase();
                if descriptor_char == 'w' || descriptor_char == 'x' {
                    // Found a descriptor! The URL is from start to ws_start
                    let url: String = chars[..ws_start].iter().collect();
                    let url = url.trim();
                    if !url.is_empty() {
                        return Some(url.to_string());
                    }
                }
            }
        }
        i += 1;
    }

    // Fallback: if no descriptor found, try simple whitespace split
    let url = srcset.split_whitespace().next()?.trim();
    if url.is_empty() {
        None
    } else {
        Some(url.to_string())
    }
}

fn pick_image_source(element: &scraper::ElementRef<'_>) -> Option<ImageSource> {
    let placeholder_src = element.value().attr("src").map(|s| s.to_string());

    let candidates = [
        "data-src",
        "data-original",
        "data-lazy-src",
        "data-srcset",
        "srcset",
        "src",
    ];

    for attr_name in candidates {
        if let Some(raw_value) = element.value().attr(attr_name) {
            if raw_value.is_empty() {
                continue;
            }

            let resolved_value = if attr_name.ends_with("srcset") {
                first_src_from_srcset(raw_value)?
            } else {
                raw_value.to_string()
            };

            return Some(ImageSource {
                attr_name,
                raw_value: raw_value.to_string(),
                resolved_value,
                placeholder_src: placeholder_src.clone(),
            });
        }
    }

    None
}

/// Normalizes images in HTML content.
///
/// - Downloads remote images to temp_dir
/// - Decodes data URLs and saves to temp_dir
/// - Removes blob URLs (cannot be processed)
/// - Updates src attributes to reference local files
/// - Removes srcset and sizes attributes
pub fn normalize_images(
    html: &str,
    base_url: &str,
    temp_dir: &Path,
    client: &Client,
) -> Result<String, ImageError> {
    // Ensure temp directory exists
    fs::create_dir_all(temp_dir)?;

    let document = Html::parse_fragment(html);
    let img_selector = Selector::parse("img").unwrap();

    let mut images: Vec<ImageInfo> = Vec::new();
    let mut data_index = 0usize;

    // First pass: collect all images and process them
    for element in document.select(&img_selector) {
        let source = match pick_image_source(&element) {
            Some(source) => source,
            None => continue,
        };

        log::debug!(
            "Processing image src ({}): {}",
            source.attr_name,
            source.resolved_value
        );

        let local_filename = if source.resolved_value.starts_with("data:") {
            // Handle data URL
            if let Some((data, ext)) = parse_data_url(&source.resolved_value) {
                let filename = format!("data-image-{}.{}", data_index, ext);
                let file_path = temp_dir.join(&filename);
                data_index += 1;
                if fs::write(&file_path, &data).is_ok() {
                    Some(filename)
                } else {
                    None
                }
            } else {
                None
            }
        } else if source.resolved_value.starts_with("blob:") {
            // Remove blob URLs - cannot be processed
            None
        } else {
            // Handle remote URL
            let absolute_url = if source.resolved_value.starts_with("http://")
                || source.resolved_value.starts_with("https://")
            {
                source.resolved_value.to_string()
            } else {
                // Resolve relative URL
                Url::parse(base_url)
                    .ok()
                    .and_then(|base| base.join(&source.resolved_value).ok())
                    .map(|u| u.to_string())
                    .unwrap_or_else(|| source.resolved_value.to_string())
            };

            log::debug!("Resolved absolute URL: {}", absolute_url);

            // Check if URL has a known image extension
            let url_ext = get_url_image_extension(&absolute_url);
            if url_ext.is_some() {
                // Known extension - keep the URL as-is for epub-builder to handle
                // Actually, we need to download it for EPUB embedding
                fetch_image_to_file(client, &absolute_url, temp_dir, data_index)
                    .map(|f| {
                        data_index += 1;
                        f
                    })
            } else {
                // Unknown extension - try to fetch and determine type from response
                fetch_image_to_file(client, &absolute_url, temp_dir, data_index).map(|f| {
                    data_index += 1;
                    f
                })
            }
        };

        images.push(ImageInfo {
            original_src: source.raw_value,
            source_attr: source.attr_name.to_string(),
            placeholder_src: source.placeholder_src,
            local_filename,
        });
    }

    // Second pass: rebuild HTML with updated image sources
    let mut result = html.to_string();

    // We need to carefully replace image tags
    // This is a simplified approach - for complex cases we might need a proper DOM manipulation
    for img_info in &images {
        if img_info.original_src.is_empty() {
            continue;
        }

        // The HTML might have HTML-encoded URLs (e.g., &amp; instead of &)
        // We need to try both the decoded and encoded versions
        let original_src = &img_info.original_src;
        let source_attr = &img_info.source_attr;
        let encoded_src = original_src.replace('&', "&amp;");

        match &img_info.local_filename {
            Some(filename) => {
                // Replace the original src with the local filename
                // For EPUB, we'll reference images by their filename

                // If source was data-src (or similar), we need to:
                // 1. Remove the original src attribute (placeholder) first
                // 2. Then replace data-src with src
                if source_attr != "src" {
                    if let Some(placeholder_src) = &img_info.placeholder_src {
                        let encoded_placeholder = placeholder_src.replace('&', "&amp;");
                        // Remove the placeholder src attribute entirely
                        result = result.replace(&format!(" src=\"{}\"", placeholder_src), "");
                        result = result.replace(&format!(" src='{}'", placeholder_src), "");
                        result = result.replace(&format!(" src=\"{}\"", encoded_placeholder), "");
                        result = result.replace(&format!(" src='{}'", encoded_placeholder), "");
                    }
                }

                // Now replace the source attribute with src
                // Try decoded version
                result = result.replace(
                    &format!("{}=\"{}\"", source_attr, original_src),
                    &format!("src=\"{}\"", filename),
                );
                result = result.replace(
                    &format!("{}='{}'", source_attr, original_src),
                    &format!("src='{}'", filename),
                );

                // Try HTML-encoded version
                result = result.replace(
                    &format!("{}=\"{}\"", source_attr, encoded_src),
                    &format!("src=\"{}\"", filename),
                );
                result = result.replace(
                    &format!("{}='{}'", source_attr, encoded_src),
                    &format!("src='{}'", filename),
                );
            }
            None => {
                // Remove the image - we'll do this by replacing the img tag
                // For now, set src to empty (image won't display)
                result = result.replace(
                    &format!("{}=\"{}\"", source_attr, original_src),
                    "src=\"\"",
                );
                result = result.replace(
                    &format!("{}=\"{}\"", source_attr, encoded_src),
                    "src=\"\"",
                );
                result = result.replace(
                    &format!("{}='{}'", source_attr, original_src),
                    "src=''",
                );
                result = result.replace(
                    &format!("{}='{}'", source_attr, encoded_src),
                    "src=''",
                );
            }
        }
    }

    // Remove srcset and sizes attributes
    let srcset_patterns = [
        (r#" srcset="[^"]*""#, ""),
        (r#" srcset='[^']*'"#, ""),
        (r#" sizes="[^"]*""#, ""),
        (r#" sizes='[^']*'"#, ""),
    ];

    for (pattern, _replacement) in srcset_patterns {
        // Simple regex-like replacement
        while let Some(start) = result.find(pattern.trim_start_matches(' ').split('=').next().unwrap()) {
            if let Some(rel_start) = result[start..].find('=') {
                let quote_start = start + rel_start + 1;
                if quote_start < result.len() {
                    let quote_char = result.chars().nth(quote_start).unwrap();
                    if quote_char == '"' || quote_char == '\'' {
                        if let Some(quote_end) = result[quote_start + 1..].find(quote_char) {
                            let full_end = quote_start + 1 + quote_end + 1;
                            // Find the start of the attribute name
                            let attr_start = result[..start].rfind(' ').map(|p| p).unwrap_or(start);
                            result = format!("{}{}", &result[..attr_start], &result[full_end..]);
                            continue;
                        }
                    }
                }
            }
            break;
        }
    }

    Ok(result)
}

/// Returns a set of image filenames that were downloaded to the temp directory.
pub fn get_downloaded_images(temp_dir: &Path) -> HashSet<String> {
    let mut images = HashSet::new();

    if let Ok(entries) = fs::read_dir(temp_dir) {
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                images.insert(name.to_string());
            }
        }
    }

    images
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extension_from_mime() {
        assert_eq!(extension_from_mime("image/jpeg"), Some("jpg"));
        assert_eq!(extension_from_mime("image/png"), Some("png"));
        assert_eq!(extension_from_mime("image/svg+xml"), Some("svg"));
        assert_eq!(extension_from_mime("text/html"), None);
    }

    #[test]
    fn test_get_url_image_extension() {
        assert_eq!(
            get_url_image_extension("https://example.com/image.jpg"),
            Some("jpg".to_string())
        );
        assert_eq!(
            get_url_image_extension("https://example.com/image.PNG"),
            Some("png".to_string())
        );
        assert_eq!(get_url_image_extension("https://example.com/noext"), None);
    }

    #[test]
    fn test_first_src_from_srcset_simple() {
        // Simple srcset with width descriptor
        assert_eq!(
            first_src_from_srcset("https://example.com/img.jpg 424w"),
            Some("https://example.com/img.jpg".to_string())
        );

        // Multiple entries
        assert_eq!(
            first_src_from_srcset("https://example.com/small.jpg 424w, https://example.com/large.jpg 848w"),
            Some("https://example.com/small.jpg".to_string())
        );

        // With pixel density descriptor
        assert_eq!(
            first_src_from_srcset("https://example.com/img.jpg 2x"),
            Some("https://example.com/img.jpg".to_string())
        );
    }

    #[test]
    fn test_first_src_from_srcset_with_commas_in_url() {
        // Substack-style URLs with commas as parameters (the bug that was fixed)
        let substack_srcset = "https://substackcdn.com/image/fetch/$s_!ksVf!,w_424,c_limit,f_auto,q_auto:good,fl_progressive:steep/https%3A%2F%2Fsubstack-post-media.s3.amazonaws.com%2Fimage.png 424w, https://substackcdn.com/image/fetch/$s_!ksVf!,w_848,c_limit,f_auto/https%3A%2F%2Fsubstack-post-media.s3.amazonaws.com%2Fimage.png 848w";

        let result = first_src_from_srcset(substack_srcset);
        assert!(result.is_some());
        let url = result.unwrap();

        // The URL should contain the full Substack CDN path including commas
        assert!(url.contains("substackcdn.com"));
        assert!(url.contains(",w_424,c_limit"));
        assert!(url.ends_with("image.png"));
    }

    #[test]
    fn test_first_src_from_srcset_edge_cases() {
        // Empty string
        assert_eq!(first_src_from_srcset(""), None);

        // Whitespace only
        assert_eq!(first_src_from_srcset("   "), None);

        // URL without descriptor (fallback to whitespace split)
        assert_eq!(
            first_src_from_srcset("https://example.com/img.jpg"),
            Some("https://example.com/img.jpg".to_string())
        );
    }
}
