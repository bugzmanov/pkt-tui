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
        article.content_html
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
