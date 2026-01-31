//! State management for background downloads.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// Information about a failed download.
#[derive(Debug, Clone)]
pub struct DownloadFailure {
    pub title: String,
    pub url: String,
    pub error: String,
}

/// Thread-safe state for background download progress.
#[derive(Debug, Clone)]
pub struct BackgroundDownloadState {
    /// Current item index (0-based).
    pub current: Arc<AtomicUsize>,
    /// Total number of items to download.
    pub total: Arc<AtomicUsize>,
    /// Title of the item currently being downloaded.
    pub current_title: Arc<Mutex<String>>,
    /// Whether the background download is currently running.
    pub is_running: Arc<AtomicBool>,
    /// Whether the background download has completed.
    pub is_complete: Arc<AtomicBool>,
    /// List of failed downloads.
    pub failures: Arc<Mutex<Vec<DownloadFailure>>>,
    /// Whether the UI should prompt user to refresh.
    pub needs_refresh_prompt: Arc<AtomicBool>,
}

impl BackgroundDownloadState {
    /// Create a new background download state.
    pub fn new() -> Self {
        Self {
            current: Arc::new(AtomicUsize::new(0)),
            total: Arc::new(AtomicUsize::new(0)),
            current_title: Arc::new(Mutex::new(String::new())),
            is_running: Arc::new(AtomicBool::new(false)),
            is_complete: Arc::new(AtomicBool::new(false)),
            failures: Arc::new(Mutex::new(Vec::new())),
            needs_refresh_prompt: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Check if background download is currently running.
    pub fn is_running(&self) -> bool {
        self.is_running.load(Ordering::SeqCst)
    }

    /// Check if background download has completed.
    pub fn is_complete(&self) -> bool {
        self.is_complete.load(Ordering::SeqCst)
    }

    /// Check if user should be prompted to refresh.
    pub fn needs_refresh_prompt(&self) -> bool {
        self.needs_refresh_prompt.load(Ordering::SeqCst)
    }

    /// Get current progress (current, total).
    pub fn progress(&self) -> (usize, usize) {
        (
            self.current.load(Ordering::SeqCst),
            self.total.load(Ordering::SeqCst),
        )
    }

    /// Get the number of failures.
    pub fn failure_count(&self) -> usize {
        self.failures.lock().unwrap().len()
    }

    /// Reset state for a new download batch.
    pub fn reset(&self) {
        self.current.store(0, Ordering::SeqCst);
        self.total.store(0, Ordering::SeqCst);
        self.is_running.store(false, Ordering::SeqCst);
        self.is_complete.store(false, Ordering::SeqCst);
        self.needs_refresh_prompt.store(false, Ordering::SeqCst);
        if let Ok(mut title) = self.current_title.lock() {
            title.clear();
        }
        if let Ok(mut failures) = self.failures.lock() {
            failures.clear();
        }
    }

    /// Acknowledge completion and clear the prompt flag.
    pub fn acknowledge(&self) {
        self.needs_refresh_prompt.store(false, Ordering::SeqCst);
        self.is_complete.store(false, Ordering::SeqCst);
    }
}

impl Default for BackgroundDownloadState {
    fn default() -> Self {
        Self::new()
    }
}
