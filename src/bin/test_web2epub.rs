//! Test web2epub conversion end-to-end.
//!
//! Usage: cargo run --bin test_web2epub <url> <output.epub>

use reqwest::blocking::Client;
use std::env;
use std::path::Path;
use std::time::Duration;

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        eprintln!("Usage: {} <url> <output.epub>", args[0]);
        std::process::exit(1);
    }

    let url = &args[1];
    let output_path = Path::new(&args[2]);
    let temp_dir = Path::new("test_epub_temp_images");

    println!("Converting URL: {}", url);
    println!("Output: {:?}", output_path);
    println!("Temp dir: {:?}", temp_dir);

    let client = Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("Failed to build HTTP client");

    match pkt_tui::web2epub::convert_url_to_epub(&client, url, output_path, temp_dir) {
        Ok(()) => {
            println!("\nSuccess! EPUB created at {:?}", output_path);

            // List downloaded images
            if let Ok(entries) = std::fs::read_dir(temp_dir) {
                println!("\nDownloaded images:");
                for entry in entries.flatten() {
                    if let Some(name) = entry.file_name().to_str() {
                        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                        println!("  {} ({} bytes)", name, size);
                    }
                }
            }
        }
        Err(e) => {
            eprintln!("\nError: {}", e);
            std::process::exit(1);
        }
    }
}
