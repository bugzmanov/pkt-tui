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
