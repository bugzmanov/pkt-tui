//! Image downloading and normalization for EPUB generation.

use base64::Engine;
use regex::Regex;
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
    referer: Option<&str>,
    temp_dir: &Path,
    file_index: usize,
) -> Option<String> {
    log::debug!("Fetching image: {}", image_url);

    let mut request = client
        .get(image_url)
        .header("User-Agent", "pkt-tui-web2epub/0.1")
        .header("Accept", "image/*,*/*;q=0.8");
    if let Some(referer_value) = referer {
        request = request.header("Referer", referer_value);
    }

    let response = match request.send() {
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

    let extension = match infer_image_extension(content_type, image_url) {
        Some(ext) => ext,
        None => {
            let mime_type = content_type.split(';').next().unwrap_or("").trim();
            log::warn!(
                "Could not infer image extension (mime='{}') for {}",
                mime_type,
                image_url
            );
            return None;
        }
    };

    let bytes = response.bytes().ok()?;

    // Convert EPUB-incompatible formats (webp, avif) to PNG
    let (final_bytes, final_extension) = if extension == "webp" || extension == "avif" {
        match convert_to_png(&bytes) {
            Some(png_bytes) => {
                log::debug!("Converted {} to PNG ({} -> {} bytes)", extension, bytes.len(), png_bytes.len());
                (png_bytes, "png".to_string())
            }
            None => {
                log::warn!("Failed to convert {} to PNG for {}", extension, image_url);
                (bytes.to_vec(), extension)
            }
        }
    } else {
        (bytes.to_vec(), extension)
    };

    let filename = format!("remote-image-{}.{}", file_index, final_extension);
    let file_path = temp_dir.join(&filename);

    fs::write(&file_path, &final_bytes).ok()?;

    Some(filename)
}

/// Converts image bytes (webp, avif) to PNG format for EPUB compatibility.
fn convert_to_png(image_bytes: &[u8]) -> Option<Vec<u8>> {
    let img = image::load_from_memory(image_bytes).ok()?;
    let mut png_buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut png_buf, image::ImageFormat::Png).ok()?;
    Some(png_buf.into_inner())
}

/// Infer image extension from response Content-Type, with URL-extension fallback.
fn infer_image_extension(content_type: &str, image_url: &str) -> Option<String> {
    let mime_type = content_type.split(';').next().unwrap_or("").trim().to_lowercase();
    if let Some(ext) = extension_from_mime(&mime_type) {
        return Some(ext.to_string());
    }
    get_url_image_extension(image_url)
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

/// Extracts the largest URL from a srcset attribute.
///
/// srcset format: "url1 descriptor1, url2 descriptor2, ..."
/// where descriptors are like "424w" (width) or "2x" (pixel density).
///
/// The tricky part is that URLs can contain commas (e.g., Substack CDN URLs),
/// so we can't just split on commas. Instead, we look for the pattern
/// `\s+\d+[wx]\s*,` which marks the end of a srcset entry.
///
/// We parse all entries and return the one with the largest width descriptor,
/// or the last one if using pixel density descriptors.
fn first_src_from_srcset(srcset: &str) -> Option<String> {
    let srcset = srcset.trim();
    if srcset.is_empty() {
        return None;
    }

    // Parse all srcset entries and find the largest one
    let entries = parse_srcset_entries(srcset);

    if entries.is_empty() {
        // Fallback: if no entries parsed, try simple whitespace split
        let url = srcset.split_whitespace().next()?.trim();
        if url.is_empty() {
            return None;
        }
        log::debug!("srcset fallback (no entries parsed): {}", url);
        return Some(url.to_string());
    }

    // Find the entry with the largest width (or highest pixel density)
    let best = entries
        .iter()
        .max_by_key(|(_, width)| *width)
        .map(|(url, width)| {
            log::debug!(
                "srcset: picked {}w from {} entries (range: {}w-{}w)",
                width,
                entries.len(),
                entries.iter().map(|(_, w)| w).min().unwrap_or(&0),
                entries.iter().map(|(_, w)| w).max().unwrap_or(&0)
            );
            url.clone()
        });

    best
}

/// Parses srcset into (url, width) pairs.
/// For pixel density descriptors (e.g., "2x"), we convert to approximate pixel widths.
fn parse_srcset_entries(srcset: &str) -> Vec<(String, u32)> {
    let mut entries = Vec::new();
    let chars: Vec<char> = srcset.chars().collect();
    let mut i = 0;
    let mut entry_start = 0;

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
                    // Found a descriptor! The URL is from entry_start to ws_start
                    let url: String = chars[entry_start..ws_start].iter().collect();
                    let url = url.trim().to_string();

                    // Parse the width/density value
                    let value_str: String = chars[digit_start..i].iter().collect();
                    let value: u32 = value_str.parse().unwrap_or(0);

                    // Convert density to approximate width (assume 1000px base)
                    let width = if descriptor_char == 'x' {
                        value * 1000
                    } else {
                        value
                    };

                    if !url.is_empty() {
                        entries.push((url, width));
                    }

                    // Skip past the descriptor and any following comma/whitespace
                    i += 1; // skip 'w' or 'x'
                    while i < chars.len() && (chars[i].is_whitespace() || chars[i] == ',') {
                        i += 1;
                    }
                    entry_start = i;
                    continue;
                }
            }
        }
        i += 1;
    }

    // Handle any remaining content (URL without descriptor)
    if entry_start < chars.len() {
        let remaining: String = chars[entry_start..].iter().collect();
        let url = remaining.split_whitespace().next().unwrap_or("").trim();
        if !url.is_empty() {
            entries.push((url.to_string(), 0));
        }
    }

    entries
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
                fetch_image_to_file(client, &absolute_url, Some(base_url), temp_dir, data_index)
                    .map(|f| {
                        data_index += 1;
                        f
                    })
            } else {
                // Unknown extension - try to fetch and determine type from response
                fetch_image_to_file(client, &absolute_url, Some(base_url), temp_dir, data_index)
                    .map(|f| {
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

    // Remove attributes from img tags that are not EPUB-compatible
    // srcset, sizes: responsive image attributes (already resolved)
    // loading, decoding: browser-specific lazy loading hints
    // data-*: framework-specific attributes (e.g., data-nimg from Next.js)
    // style: inline styles that may hide images (e.g., color:transparent)
    let attr_patterns = [
        r#"(<img[^>]*)\s+srcset="[^"]*""#,
        r#"(<img[^>]*)\s+srcset='[^']*'"#,
        r#"(<img[^>]*)\s+sizes="[^"]*""#,
        r#"(<img[^>]*)\s+sizes='[^']*'"#,
        r#"(<img[^>]*)\s+loading="[^"]*""#,
        r#"(<img[^>]*)\s+decoding="[^"]*""#,
        r#"(<img[^>]*)\s+data-[\w-]+="[^"]*""#,
        r#"(<img[^>]*)\s+style="[^"]*""#,
    ];

    for pattern in attr_patterns {
        if let Ok(re) = Regex::new(pattern) {
            // Some attributes may appear multiple times (e.g., multiple data-* attrs)
            loop {
                let new = re.replace_all(&result, "$1").to_string();
                if new == result {
                    break;
                }
                result = new;
            }
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

        // Multiple entries - should pick the LARGEST (848w)
        assert_eq!(
            first_src_from_srcset("https://example.com/small.jpg 424w, https://example.com/large.jpg 848w"),
            Some("https://example.com/large.jpg".to_string())
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
        // Should pick the LARGER one (848w)
        let substack_srcset = "https://substackcdn.com/image/fetch/$s_!ksVf!,w_424,c_limit,f_auto,q_auto:good,fl_progressive:steep/https%3A%2F%2Fsubstack-post-media.s3.amazonaws.com%2Fimage.png 424w, https://substackcdn.com/image/fetch/$s_!ksVf!,w_848,c_limit,f_auto/https%3A%2F%2Fsubstack-post-media.s3.amazonaws.com%2Fimage.png 848w";

        let result = first_src_from_srcset(substack_srcset);
        assert!(result.is_some());
        let url = result.unwrap();

        // The URL should contain the full Substack CDN path including commas
        // and should be the 848w version (larger)
        assert!(url.contains("substackcdn.com"));
        assert!(url.contains(",w_848,c_limit"));
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

    #[test]
    fn test_first_src_from_srcset_picks_largest() {
        // LessWrong-style srcset with many sizes
        let lesswrong_srcset = "https://cdn.example.com/w_120 120w, https://cdn.example.com/w_240 240w, https://cdn.example.com/w_480 480w, https://cdn.example.com/w_960 960w, https://cdn.example.com/w_1132 1132w";

        let result = first_src_from_srcset(lesswrong_srcset);
        assert!(result.is_some());
        let url = result.unwrap();

        // Should pick the largest (1132w)
        assert!(url.contains("w_1132"));
    }

    #[test]
    fn test_infer_image_extension_prefers_mime_type() {
        let ext = infer_image_extension("image/webp; charset=binary", "https://e.com/x.jpg");
        assert_eq!(ext, Some("webp".to_string()));
    }

    #[test]
    fn test_infer_image_extension_falls_back_to_url() {
        let ext = infer_image_extension(
            "application/octet-stream",
            "https://cdn.example.com/path/image.avif?token=abc",
        );
        assert_eq!(ext, Some("avif".to_string()));
    }

    #[test]
    fn test_infer_image_extension_returns_none_when_unknown() {
        let ext = infer_image_extension("application/octet-stream", "https://e.com/noext");
        assert_eq!(ext, None);
    }
}
