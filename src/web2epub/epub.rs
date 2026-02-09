//! EPUB generation using epub-builder.

use epub_builder::{EpubBuilder, EpubContent, ZipLibrary};
use std::fs::{self, File};
use std::io::Cursor;
use std::path::Path;
use thiserror::Error;

use super::scraper::Article;

#[derive(Debug, Error)]
pub enum EpubError {
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("EPUB builder error: {0}")]
    BuilderError(String),
}

/// Returns the MIME type for a given file extension.
fn mime_from_extension(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .as_deref()
    {
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("png") => "image/png",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("svg") => "image/svg+xml",
        Some("avif") => "image/avif",
        Some("bmp") => "image/bmp",
        Some("tiff") => "image/tiff",
        _ => "application/octet-stream",
    }
}

/// Creates an EPUB file from an article.
///
/// # Arguments
/// * `article` - The extracted article content
/// * `images_dir` - Directory containing downloaded images
/// * `output_path` - Path where the EPUB will be written
/// * `source_url` - Original URL of the article (included in the EPUB)
pub fn create_epub(
    article: &Article,
    images_dir: &Path,
    output_path: &Path,
    source_url: &str,
) -> Result<(), EpubError> {
    // Ensure parent directory exists
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let zip_library =
        ZipLibrary::new().map_err(|e| EpubError::BuilderError(format!("Zip error: {}", e)))?;

    let mut builder = EpubBuilder::new(zip_library)
        .map_err(|e| EpubError::BuilderError(format!("Builder init error: {}", e)))?;

    // Set metadata
    let title_with_source = format!("{} ({})", article.title, source_url);
    builder
        .metadata("title", &title_with_source)
        .map_err(|e| EpubError::BuilderError(format!("Metadata error: {}", e)))?;
    builder
        .metadata("author", &article.author)
        .map_err(|e| EpubError::BuilderError(format!("Metadata error: {}", e)))?;

    // Add a simple stylesheet
    let css = r#"
body {
    font-family: serif;
    line-height: 1.6;
    margin: 1em;
}
img {
    max-width: 100%;
    height: auto;
}
a {
    color: #0066cc;
}
pre, code {
    font-family: monospace;
    background: #f4f4f4;
    padding: 0.2em 0.4em;
}
pre {
    padding: 1em;
    overflow-x: auto;
}
blockquote {
    border-left: 3px solid #ccc;
    margin-left: 0;
    padding-left: 1em;
    color: #666;
}
blockquote.pullquote {
    background: #f8f9fa;
    border-left: 4px solid #5c7cfa;
    padding: 1em;
    margin: 1em 0;
    color: #333;
    font-size: 0.95em;
}
blockquote.pullquote a {
    color: #364fc7;
}
"#;
    builder
        .stylesheet(css.as_bytes())
        .map_err(|e| EpubError::BuilderError(format!("Stylesheet error: {}", e)))?;

    // Add images as resources
    if images_dir.exists() {
        for entry in fs::read_dir(images_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() {
                if let Some(filename) = path.file_name().and_then(|f| f.to_str()) {
                    let mime = mime_from_extension(&path);
                    let data = fs::read(&path)?;
                    builder
                        .add_resource(filename, Cursor::new(data), mime)
                        .map_err(|e| EpubError::BuilderError(format!("Resource error: {}", e)))?;
                }
            }
        }
    }

    // Create the content XHTML
    let title_html = format!(r#"<h1>{}</h1>"#, html_escape(&article.title));

    let source_html = format!(
        r#"<p><em>Source:</em> <a href="{}">{}</a></p>"#,
        html_escape(source_url),
        html_escape(source_url)
    );

    // Convert HTML content to valid XHTML
    let xhtml_content = html_to_xhtml(&article.content_html);

    let full_content = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE html>
<html xmlns="http://www.w3.org/1999/xhtml">
<head>
    <meta charset="UTF-8"/>
    <title>{}</title>
    <link rel="stylesheet" type="text/css" href="stylesheet.css"/>
</head>
<body>
{}
{}
{}
</body>
</html>"#,
        html_escape(&article.title),
        title_html,
        source_html,
        xhtml_content
    );

    // Add the main content
    builder
        .add_content(
            EpubContent::new("chapter.xhtml", full_content.as_bytes()).title(&article.title),
        )
        .map_err(|e| EpubError::BuilderError(format!("Content error: {}", e)))?;

    // Generate the EPUB
    let mut output = File::create(output_path)?;
    builder
        .generate(&mut output)
        .map_err(|e| EpubError::BuilderError(format!("Generate error: {}", e)))?;

    Ok(())
}

/// Escapes HTML special characters.
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

/// Converts HTML to valid XHTML by fixing self-closing tags.
fn html_to_xhtml(html: &str) -> String {
    let mut result = html.to_string();

    // List of void elements that need to be self-closing in XHTML
    let void_elements = ["img", "br", "hr", "meta", "link", "input", "area", "base", "col", "embed", "source", "track", "wbr"];

    for tag in void_elements {
        // Match <tag ...> (not already self-closing) and convert to <tag ... />
        // This is a simple approach - handles most common cases

        // Pattern: <tag followed by attributes, ending with > but not />
        let mut pos = 0;
        let mut new_result = String::new();

        while pos < result.len() {
            // Look for opening tag
            if let Some(tag_start) = result[pos..].find(&format!("<{}", tag)) {
                let absolute_start = pos + tag_start;

                // Check if this is actually the tag (not e.g., <imgage)
                let after_tag = absolute_start + 1 + tag.len();
                if after_tag < result.len() {
                    let next_char = result.chars().nth(after_tag).unwrap_or('x');
                    if next_char.is_alphanumeric() {
                        // Not our tag, continue
                        new_result.push_str(&result[pos..absolute_start + 1]);
                        pos = absolute_start + 1;
                        continue;
                    }
                }

                // Find the closing >
                if let Some(tag_end) = result[absolute_start..].find('>') {
                    let absolute_end = absolute_start + tag_end;

                    // Check if already self-closing
                    if absolute_end > 0 && result.chars().nth(absolute_end - 1) == Some('/') {
                        // Already self-closing, copy as-is
                        new_result.push_str(&result[pos..absolute_end + 1]);
                    } else {
                        // Need to make it self-closing
                        new_result.push_str(&result[pos..absolute_end]);
                        new_result.push_str(" />");
                    }
                    pos = absolute_end + 1;
                } else {
                    // No closing >, just copy the rest
                    new_result.push_str(&result[pos..]);
                    pos = result.len();
                }
            } else {
                // No more tags of this type, copy the rest
                new_result.push_str(&result[pos..]);
                break;
            }
        }
        result = new_result;
    }

    result
}
