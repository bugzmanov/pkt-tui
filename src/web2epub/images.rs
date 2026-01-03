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
    /// Local filename after processing (None if removed)
    local_filename: Option<String>,
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
        let src = element
            .value()
            .attr("src")
            .or_else(|| element.value().attr("data-src"));

        let src = match src {
            Some(s) if !s.is_empty() => s,
            _ => {
                images.push(ImageInfo {
                    original_src: String::new(),
                    local_filename: None,
                });
                continue;
            }
        };

        log::debug!("Processing image src: {}", src);

        let local_filename = if src.starts_with("data:") {
            // Handle data URL
            if let Some((data, ext)) = parse_data_url(src) {
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
        } else if src.starts_with("blob:") {
            // Remove blob URLs - cannot be processed
            None
        } else {
            // Handle remote URL
            let absolute_url = if src.starts_with("http://") || src.starts_with("https://") {
                src.to_string()
            } else {
                // Resolve relative URL
                Url::parse(base_url)
                    .ok()
                    .and_then(|base| base.join(src).ok())
                    .map(|u| u.to_string())
                    .unwrap_or_else(|| src.to_string())
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
            original_src: src.to_string(),
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
        let encoded_src = original_src.replace('&', "&amp;");

        match &img_info.local_filename {
            Some(filename) => {
                // Replace the original src with the local filename
                // For EPUB, we'll reference images by their filename

                // Try decoded version
                result = result.replace(
                    &format!("src=\"{}\"", original_src),
                    &format!("src=\"{}\"", filename),
                );
                result = result.replace(
                    &format!("src='{}'", original_src),
                    &format!("src='{}'", filename),
                );

                // Try HTML-encoded version
                result = result.replace(
                    &format!("src=\"{}\"", encoded_src),
                    &format!("src=\"{}\"", filename),
                );
                result = result.replace(
                    &format!("src='{}'", encoded_src),
                    &format!("src='{}'", filename),
                );

                // Also handle data-src (decoded)
                result = result.replace(
                    &format!("data-src=\"{}\"", original_src),
                    &format!("src=\"{}\"", filename),
                );
                result = result.replace(
                    &format!("data-src='{}'", original_src),
                    &format!("src='{}'", filename),
                );

                // data-src (encoded)
                result = result.replace(
                    &format!("data-src=\"{}\"", encoded_src),
                    &format!("src=\"{}\"", filename),
                );
                result = result.replace(
                    &format!("data-src='{}'", encoded_src),
                    &format!("src='{}'", filename),
                );
            }
            None => {
                // Remove the image - we'll do this by replacing the img tag
                // For now, set src to empty (image won't display)
                result = result.replace(
                    &format!("src=\"{}\"", original_src),
                    "src=\"\"",
                );
                result = result.replace(
                    &format!("src=\"{}\"", encoded_src),
                    "src=\"\"",
                );
                result = result.replace(
                    &format!("src='{}'", original_src),
                    "src=''",
                );
                result = result.replace(
                    &format!("src='{}'", encoded_src),
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
}
