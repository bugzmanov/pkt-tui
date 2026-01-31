//! Page analysis tool for debugging web2epub image issues.
//!
//! Usage: cargo run --bin analyze_page <url> [--generate-epub]
//!
//! This tool fetches a web page, extracts article content, and analyzes
//! all images to identify issues with image downloading and processing.
//!
//! Options:
//!   --generate-epub  Also generate an EPUB and verify images are embedded correctly

use reqwest::blocking::Client;
use scraper::{Html, Selector};
use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::Path;
use std::time::Duration;
use url::Url;

const KNOWN_IMAGE_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "webp", "svg", "avif", "bmp", "tiff",
];

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

/// Extracts the first URL from a srcset attribute.
/// srcset URLs can contain commas, so we look for width descriptors (e.g., "424w").
fn first_src_from_srcset(srcset: &str) -> Option<String> {
    let srcset = srcset.trim();
    if srcset.is_empty() {
        return None;
    }

    let chars: Vec<char> = srcset.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        if chars[i].is_whitespace() {
            let ws_start = i;
            while i < chars.len() && chars[i].is_whitespace() {
                i += 1;
            }
            if i >= chars.len() {
                break;
            }

            let digit_start = i;
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }

            if i > digit_start && i < chars.len() {
                let descriptor_char = chars[i].to_ascii_lowercase();
                if descriptor_char == 'w' || descriptor_char == 'x' {
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

    let url = srcset.split_whitespace().next()?.trim();
    if url.is_empty() {
        None
    } else {
        Some(url.to_string())
    }
}

#[derive(Debug)]
struct ImageAnalysis {
    index: usize,
    source_attr: String,
    original_url: String,
    resolved_url: String,
    url_extension: Option<String>,
    download_result: DownloadResult,
}

#[derive(Debug)]
enum DownloadResult {
    Success {
        status: u16,
        content_type: String,
        mime_extension: Option<&'static str>,
        size_bytes: usize,
    },
    HttpError {
        status: u16,
        message: String,
    },
    NetworkError(String),
    InvalidUrl(String),
    DataUrl {
        mime_type: String,
        size_bytes: usize,
    },
    BlobUrl,
}

fn analyze_image(client: &Client, base_url: &str, attr: &str, raw_value: &str, index: usize) -> ImageAnalysis {
    let resolved_url = if raw_value.starts_with("data:") {
        raw_value.to_string()
    } else if raw_value.starts_with("blob:") {
        raw_value.to_string()
    } else if raw_value.starts_with("http://") || raw_value.starts_with("https://") {
        raw_value.to_string()
    } else {
        // Resolve relative URL
        Url::parse(base_url)
            .ok()
            .and_then(|base| base.join(raw_value).ok())
            .map(|u| u.to_string())
            .unwrap_or_else(|| raw_value.to_string())
    };

    let download_result = if resolved_url.starts_with("data:") {
        // Parse data URL
        if let Some(rest) = resolved_url.strip_prefix("data:") {
            if let Some((mime_and_encoding, data)) = rest.split_once(',') {
                let mime = mime_and_encoding.replace(";base64", "");
                let size = data.len() * 3 / 4; // Approximate decoded size
                DownloadResult::DataUrl {
                    mime_type: mime,
                    size_bytes: size,
                }
            } else {
                DownloadResult::InvalidUrl("Malformed data URL".to_string())
            }
        } else {
            DownloadResult::InvalidUrl("Invalid data URL".to_string())
        }
    } else if resolved_url.starts_with("blob:") {
        DownloadResult::BlobUrl
    } else {
        // Try to fetch the image
        match client
            .get(&resolved_url)
            .header("User-Agent", "pkt-tui-web2epub/0.1")
            .header("Accept", "image/*")
            .send()
        {
            Ok(response) => {
                let status = response.status().as_u16();
                if response.status().is_success() {
                    let content_type = response
                        .headers()
                        .get("content-type")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("")
                        .to_string();
                    let mime_type = content_type.split(';').next().unwrap_or("").trim();
                    let mime_extension = extension_from_mime(mime_type);

                    match response.bytes() {
                        Ok(bytes) => DownloadResult::Success {
                            status,
                            content_type,
                            mime_extension,
                            size_bytes: bytes.len(),
                        },
                        Err(e) => DownloadResult::NetworkError(format!("Failed to read body: {}", e)),
                    }
                } else {
                    DownloadResult::HttpError {
                        status,
                        message: response.status().to_string(),
                    }
                }
            }
            Err(e) => DownloadResult::NetworkError(e.to_string()),
        }
    };

    ImageAnalysis {
        index,
        source_attr: attr.to_string(),
        original_url: raw_value.to_string(),
        resolved_url: resolved_url.clone(),
        url_extension: get_url_image_extension(&resolved_url),
        download_result,
    }
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <url> [--generate-epub]", args[0]);
        eprintln!("\nAnalyzes a web page for EPUB conversion issues.");
        eprintln!("Checks all images and reports download success/failure.");
        eprintln!("\nOptions:");
        eprintln!("  --generate-epub  Also generate an EPUB and verify images are embedded correctly");
        std::process::exit(1);
    }

    let url = &args[1];
    let generate_epub = args.iter().any(|a| a == "--generate-epub");

    println!("=== Page Analysis Tool ===\n");
    println!("URL: {}\n", url);

    // Build HTTP client
    let client = Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("Failed to build HTTP client");

    // Fetch the page
    println!("Fetching page...");
    let html = match client
        .get(url)
        .header("User-Agent", "pkt-tui-web2epub/0.1")
        .header("Accept", "text/html,application/xhtml+xml")
        .send()
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                eprintln!("Failed to fetch page: HTTP {}", resp.status());
                std::process::exit(1);
            }
            resp.text().unwrap_or_default()
        }
        Err(e) => {
            eprintln!("Failed to fetch page: {}", e);
            std::process::exit(1);
        }
    };
    println!("Page fetched: {} bytes\n", html.len());

    // Extract article using library's extract_article (which includes preprocessing)
    println!("Extracting article content...");
    let (article_title, article_html) = {
        use pkt_tui::web2epub::extract_article_from_html;
        match extract_article_from_html(&html, url) {
            Ok(article) => {
                println!("Title: {}", article.title);
                println!("Author: {}", article.author);
                (article.title.clone(), article.content_html.clone())
            }
            Err(e) => {
                eprintln!("Failed to extract article: {}", e);
                std::process::exit(1);
            }
        }
    };
    println!("Extracted content: {} bytes\n", article_html.len());

    // Parse and analyze images in extracted content
    println!("=== Image Analysis (Extracted Content) ===\n");
    let document = Html::parse_fragment(&article_html);
    let img_selector = Selector::parse("img").unwrap();

    let mut images: Vec<ImageAnalysis> = Vec::new();
    let mut index = 0;

    for element in document.select(&img_selector) {
        // Check various src attributes
        let candidates = [
            ("data-src", element.value().attr("data-src")),
            ("data-original", element.value().attr("data-original")),
            ("data-lazy-src", element.value().attr("data-lazy-src")),
            ("data-srcset", element.value().attr("data-srcset")),
            ("srcset", element.value().attr("srcset")),
            ("src", element.value().attr("src")),
        ];

        // Find the first non-empty attribute
        let mut found_attr = None;
        for (attr_name, attr_value) in candidates {
            if let Some(value) = attr_value {
                if !value.is_empty() {
                    let resolved_value = if attr_name.ends_with("srcset") {
                        first_src_from_srcset(value).unwrap_or_else(|| value.to_string())
                    } else {
                        value.to_string()
                    };
                    found_attr = Some((attr_name, resolved_value));
                    break;
                }
            }
        }

        if let Some((attr_name, value)) = found_attr {
            let analysis = analyze_image(&client, url, attr_name, &value, index);
            images.push(analysis);
            index += 1;
        } else {
            println!("Image {}: No valid src attribute found", index);
            println!("  Available attrs: {:?}\n", element.value().attrs().collect::<Vec<_>>());
            index += 1;
        }
    }

    // Print analysis results
    let mut success_count = 0;
    let mut fail_count = 0;

    for img in &images {
        println!("Image {}:", img.index);
        println!("  Source attr: {}", img.source_attr);
        println!("  Original: {}", truncate_url(&img.original_url, 80));
        if img.original_url != img.resolved_url {
            println!("  Resolved: {}", truncate_url(&img.resolved_url, 80));
        }
        println!("  URL extension: {:?}", img.url_extension);

        match &img.download_result {
            DownloadResult::Success { status, content_type, mime_extension, size_bytes } => {
                println!("  Status: OK (HTTP {})", status);
                println!("  Content-Type: {}", content_type);
                println!("  MIME extension: {:?}", mime_extension);
                println!("  Size: {} bytes", size_bytes);
                if mime_extension.is_none() {
                    println!("  WARNING: Unknown MIME type - image will be skipped!");
                    fail_count += 1;
                } else {
                    success_count += 1;
                }
            }
            DownloadResult::HttpError { status, message } => {
                println!("  Status: FAILED (HTTP {} - {})", status, message);
                fail_count += 1;
            }
            DownloadResult::NetworkError(msg) => {
                println!("  Status: NETWORK ERROR - {}", msg);
                fail_count += 1;
            }
            DownloadResult::InvalidUrl(msg) => {
                println!("  Status: INVALID URL - {}", msg);
                fail_count += 1;
            }
            DownloadResult::DataUrl { mime_type, size_bytes } => {
                println!("  Status: DATA URL");
                println!("  MIME type: {}", mime_type);
                println!("  Approx size: {} bytes", size_bytes);
                if extension_from_mime(mime_type).is_some() {
                    success_count += 1;
                } else {
                    println!("  WARNING: Unknown MIME type - image will be skipped!");
                    fail_count += 1;
                }
            }
            DownloadResult::BlobUrl => {
                println!("  Status: BLOB URL (cannot be processed)");
                fail_count += 1;
            }
        }
        println!();
    }

    // Summary
    println!("=== Summary ===\n");
    println!("Total images found: {}", images.len());
    println!("Successful: {}", success_count);
    println!("Failed/Skipped: {}", fail_count);

    if fail_count > 0 {
        println!("\n=== Failure Analysis ===\n");
        for img in &images {
            let is_failure = match &img.download_result {
                DownloadResult::Success { mime_extension, .. } => mime_extension.is_none(),
                DownloadResult::DataUrl { mime_type, .. } => extension_from_mime(mime_type).is_none(),
                _ => true,
            };

            if is_failure {
                println!("Image {} failed:", img.index);
                println!("  URL: {}", truncate_url(&img.resolved_url, 100));
                match &img.download_result {
                    DownloadResult::Success { content_type, .. } => {
                        println!("  Reason: Unknown Content-Type '{}'", content_type);
                    }
                    DownloadResult::HttpError { status, message } => {
                        println!("  Reason: HTTP error {} - {}", status, message);
                    }
                    DownloadResult::NetworkError(msg) => {
                        println!("  Reason: Network error - {}", msg);
                    }
                    DownloadResult::InvalidUrl(msg) => {
                        println!("  Reason: Invalid URL - {}", msg);
                    }
                    DownloadResult::DataUrl { mime_type, .. } => {
                        println!("  Reason: Unknown data URL MIME type '{}'", mime_type);
                    }
                    DownloadResult::BlobUrl => {
                        println!("  Reason: Blob URLs cannot be downloaded");
                    }
                }
                println!();
            }
        }
    }

    // Save extracted HTML for inspection
    let debug_dir = Path::new("analyze_page_debug");
    fs::create_dir_all(debug_dir).ok();

    // Also save raw HTML for debugging
    let raw_path = debug_dir.join("raw_page.html");
    fs::write(&raw_path, &html).ok();

    let extracted_path = debug_dir.join("extracted_content.html");
    fs::write(&extracted_path, &article_html).ok();
    println!("\nDebug files saved to {:?}", debug_dir);
    println!("  - raw_page.html: Original HTML before extraction");
    println!("  - extracted_content.html: Article content from readability");

    // Optionally generate EPUB and verify
    if generate_epub {
        println!("\n=== EPUB Generation ===\n");

        let temp_dir = debug_dir.join("epub_images");
        let epub_path = debug_dir.join("test_output.epub");

        // Clean up previous run
        let _ = fs::remove_dir_all(&temp_dir);
        let _ = fs::remove_file(&epub_path);

        println!("Generating EPUB...");
        match pkt_tui::web2epub::convert_url_to_epub(&client, url, &epub_path, &temp_dir) {
            Ok(()) => {
                println!("EPUB created: {:?}", epub_path);

                // Verify downloaded images
                let mut downloaded_images: HashSet<String> = HashSet::new();
                if let Ok(entries) = fs::read_dir(&temp_dir) {
                    for entry in entries.flatten() {
                        if let Some(name) = entry.file_name().to_str() {
                            downloaded_images.insert(name.to_string());
                        }
                    }
                }

                println!("\nDownloaded {} images:", downloaded_images.len());
                for name in &downloaded_images {
                    let path = temp_dir.join(name);
                    let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                    println!("  {} ({} bytes)", name, size);
                }

                // Check if EPUB contains all images
                println!("\n=== EPUB Verification ===\n");

                if downloaded_images.len() == success_count {
                    println!("SUCCESS: All {} downloadable images are in the EPUB", success_count);
                } else {
                    println!("WARNING: Expected {} images, got {}", success_count, downloaded_images.len());
                }

                // Verify EPUB structure
                let epub_size = fs::metadata(&epub_path).map(|m| m.len()).unwrap_or(0);
                println!("\nEPUB file size: {} bytes", epub_size);

                // Try to extract and verify EPUB content
                if let Ok(epub_file) = fs::File::open(&epub_path) {
                    if let Ok(mut archive) = zip::ZipArchive::new(epub_file) {
                        let mut epub_images = 0;
                        let mut has_chapter = false;

                        for i in 0..archive.len() {
                            if let Ok(file) = archive.by_index(i) {
                                let name = file.name();
                                if name.ends_with(".jpg") || name.ends_with(".png") || name.ends_with(".gif") || name.ends_with(".webp") || name.ends_with(".svg") {
                                    epub_images += 1;
                                }
                                if name.contains("chapter.xhtml") {
                                    has_chapter = true;
                                }
                            }
                        }

                        println!("EPUB contains {} image files", epub_images);
                        println!("EPUB contains chapter.xhtml: {}", has_chapter);

                        if epub_images == success_count {
                            println!("\nVERIFICATION PASSED: EPUB correctly contains all images");
                        } else {
                            println!("\nVERIFICATION FAILED: EPUB has {} images, expected {}", epub_images, success_count);
                        }
                    }
                }
            }
            Err(e) => {
                eprintln!("Failed to generate EPUB: {}", e);
            }
        }
    }

    // Final verdict
    println!("\n=== Final Verdict ===\n");
    if fail_count == 0 {
        println!("PASS: All images can be downloaded and processed");
    } else {
        println!("ISSUES FOUND: {} images failed to download or have unsupported formats", fail_count);
    }
}

fn truncate_url(url: &str, max_len: usize) -> String {
    if url.len() <= max_len {
        url.to_string()
    } else {
        format!("{}...", &url[..max_len - 3])
    }
}
