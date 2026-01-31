//! Background download module for automatically downloading PDFs and articles.

mod config;
mod state;
mod worker;

pub use config::{load_config, BackgroundDownloadConfig};
pub use state::{BackgroundDownloadState, DownloadFailure};
pub use worker::start_background_download;
