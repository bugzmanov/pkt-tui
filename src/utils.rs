use anyhow::Context;
use extractous::Extractor;
use log::{debug, error};
use std::path::Path;

pub struct PDFData {
    pub title: Option<String>,
    pub text: String,
}

pub fn extract_pdf_title(path: &Path) -> anyhow::Result<Option<PDFData>> {
    // Read the file content
    let data =
        std::fs::read(path).with_context(|| format!("Failed to read file: {}", path.display()))?;
    let mut extractor = Extractor::new().set_extract_string_max_length(10000);
    let (text, metadata) = extractor
        .extract_file_to_string(path.to_str().unwrap())
        .unwrap();

    let mut title_opt: Option<String> = None;
    if None == metadata.get("pdf:PDFVersion") {
        error!("PDF Metadate that doesn't have PDFVersion: {:?}", metadata);
        return anyhow::Result::Err(anyhow::anyhow!(
            "No pdf metadata found. The file is not a pdf file."
        ));
    }

    //todo: sometimes title metadata contains garbage
    if let Some(title) = metadata.get("dc:title") {
        title_opt = title
            .first()
            .and_then(|x| (!x.is_empty()).then(|| x.clone()));
    }
    if title_opt.is_none() {
        if let Some(title) = metadata.get("pdf:docinfo:title") {
            title_opt = title
                .first()
                .and_then(|x| (!x.is_empty()).then(|| x.clone()));
        }
    }
    if title_opt.is_none() {
        if let Some(extracted_title) = extract_title(&text) {
            if !extracted_title.is_empty() {
                title_opt = Some(extracted_title);
            }
        }
    }

    debug!(
        "PDF Meta: {:?},\nTitle: {:?},\nText: {:?}",
        metadata,
        title_opt,
        &text[0..500]
    );
    // Ok(None)

    Ok(Some(PDFData {
        title: title_opt,
        text,
    }))
}

fn extract_title(text: &str) -> Option<String> {
    let min_words = 3;
    let max_words = 50;

    let by_paragraphs = text
        .trim_start()
        .split("\n\n")
        .find(|s| {
            let words = s.split_whitespace().count();
            words >= min_words && words <= max_words
        })
        .map(|s| s.replace('\n', " ").trim().to_string());

    match by_paragraphs {
        Some(title) => Some(title),
        None => text
            .split_whitespace()
            .take(10)
            .collect::<Vec<_>>()
            .join(" ")
            .into(),
    }
}
