//! Thumbnail generation for offline bundles.

use super::types::BundleItem;
use anyhow::Result;
use reqwest::blocking::Client;
use std::path::Path;

/// Generate thumbnail for PDF (Stub - PDF support removed)
pub fn generate_pdf_thumbnail(
    _pdf_path: &Path,
    _thumbs_dir: &Path,
    _item_id: i64,
) -> Result<Option<String>> {
    Ok(None)
}

/// Download video thumbnail from YouTube
pub fn download_video_thumbnail(
    client: &Client,
    item: &BundleItem,
    bundle_path: &Path,
) -> Result<Option<String>> {
    let thumb_filename = format!("thumb_{}.jpg", item.id);
    let thumb_path = bundle_path.join("thumbs").join(&thumb_filename);

    // Extract YouTube video ID
    let video_id = extract_youtube_id(&item.link);
    if let Some(id) = video_id {
        // Try different thumbnail qualities
        for quality in ["maxresdefault", "hqdefault", "mqdefault", "default"] {
            let thumb_url = format!("https://img.youtube.com/vi/{}/{}.jpg", id, quality);
            if let Ok(response) = client.get(&thumb_url).send() {
                if response.status().is_success() {
                    if let Ok(bytes) = response.bytes() {
                        std::fs::write(&thumb_path, bytes)?;
                        return Ok(Some(thumb_filename));
                    }
                }
            }
        }
    }

    Ok(None)
}

/// Generate EPUB thumbnail (placeholder for now)
pub fn generate_epub_thumbnail(
    _epub_path: &Path,
    _thumbs_dir: &Path,
    _item_id: i64,
) -> Result<Option<String>> {
    // For now, return None and let the HTML use a placeholder
    // A more complete implementation could extract the cover image from the EPUB
    Ok(None)
}

/// Extract YouTube video ID from URL
fn extract_youtube_id(url: &str) -> Option<String> {
    // Handle youtube.com/watch?v=ID format
    if url.contains("youtube.com") {
        if let Ok(parsed) = url::Url::parse(url) {
            for (key, value) in parsed.query_pairs() {
                if key == "v" {
                    return Some(value.to_string());
                }
            }
        }
    }
    // Handle youtu.be/ID format
    else if url.contains("youtu.be") {
        if let Ok(parsed) = url::Url::parse(url) {
            if let Some(segments) = parsed.path_segments() {
                if let Some(id) = segments.into_iter().next() {
                    if !id.is_empty() {
                        return Some(id.to_string());
                    }
                }
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_youtube_id() {
        assert_eq!(
            extract_youtube_id("https://www.youtube.com/watch?v=dQw4w9WgXcQ"),
            Some("dQw4w9WgXcQ".to_string())
        );
        assert_eq!(
            extract_youtube_id("https://youtu.be/dQw4w9WgXcQ"),
            Some("dQw4w9WgXcQ".to_string())
        );
        assert_eq!(
            extract_youtube_id("https://youtube.com/watch?v=abc123&t=30"),
            Some("abc123".to_string())
        );
        assert_eq!(extract_youtube_id("https://example.com"), None);
    }
}
