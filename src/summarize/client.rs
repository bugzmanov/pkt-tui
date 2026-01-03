use anyhow::{Context, Result};
use reqwest::blocking::Client;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use std::io::BufRead;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

use super::token::{get_daemon_url, get_length};

#[derive(Serialize)]
struct SummarizeRequest {
    url: String,
    mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    length: Option<String>,
}

#[derive(Deserialize)]
struct SummarizeResponse {
    ok: bool,
    id: Option<String>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct ChunkData {
    text: String,
}

#[derive(Deserialize)]
struct StatusData {
    text: String,
}

#[derive(Deserialize)]
struct ErrorData {
    message: String,
}

#[derive(Deserialize)]
struct MetaData {
    #[serde(rename = "modelLabel")]
    model_label: Option<String>,
    #[serde(rename = "inputSummary")]
    input_summary: Option<String>,
    #[serde(rename = "summaryFromCache")]
    summary_from_cache: Option<bool>,
}

/// Metadata from summarization
#[derive(Clone, Default)]
pub struct SummarizeMeta {
    pub model_label: Option<String>,
    pub input_summary: Option<String>,
    pub from_cache: bool,
}

/// Shared state for background thread communication
pub struct SummarizeSharedState {
    pub content: Arc<Mutex<String>>,
    pub status: Arc<Mutex<String>>,
    pub meta: Arc<Mutex<Option<SummarizeMeta>>>,
    pub error: Arc<Mutex<Option<String>>>,
    pub is_complete: Arc<AtomicBool>,
}

impl SummarizeSharedState {
    pub fn new() -> Self {
        Self {
            content: Arc::new(Mutex::new(String::new())),
            status: Arc::new(Mutex::new("Starting...".to_string())),
            meta: Arc::new(Mutex::new(None)),
            error: Arc::new(Mutex::new(None)),
            is_complete: Arc::new(AtomicBool::new(false)),
        }
    }
}

pub struct SummarizeClient {
    client: Client,
    token: String,
}

impl SummarizeClient {
    pub fn new(token: String) -> Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(120))
            .build()?;
        Ok(Self { client, token })
    }

    /// Start a summarization job and return the session ID
    pub fn start_summarize(&self, url: &str) -> Result<String> {
        let request = SummarizeRequest {
            url: url.to_string(),
            mode: "url".to_string(),
            length: Some(get_length()),
        };

        let daemon_url = get_daemon_url();
        let resp: SummarizeResponse = self
            .client
            .post(format!("{}/v1/summarize", daemon_url))
            .header(AUTHORIZATION, format!("Bearer {}", self.token))
            .header(CONTENT_TYPE, "application/json")
            .json(&request)
            .send()
            .context("Failed to connect to summarize daemon")?
            .json()
            .context("Failed to parse response")?;

        if !resp.ok {
            anyhow::bail!(resp.error.unwrap_or_else(|| "Unknown error".to_string()));
        }

        resp.id.context("No session ID returned")
    }

    /// Stream SSE events, updating shared state
    pub fn stream_events(
        &self,
        session_id: &str,
        content: Arc<Mutex<String>>,
        status: Arc<Mutex<String>>,
        meta: Arc<Mutex<Option<SummarizeMeta>>>,
        error: Arc<Mutex<Option<String>>>,
        is_complete: Arc<AtomicBool>,
    ) -> Result<()> {
        let daemon_url = get_daemon_url();
        let resp = self
            .client
            .get(format!("{}/v1/summarize/{}/events", daemon_url, session_id))
            .header(AUTHORIZATION, format!("Bearer {}", self.token))
            .send()
            .context("Failed to connect to SSE stream")?;

        let reader = std::io::BufReader::new(resp);
        let mut current_event = String::new();
        let mut current_data = String::new();

        for line_result in reader.lines() {
            let line = match line_result {
                Ok(l) => l,
                Err(e) => {
                    if let Ok(mut err) = error.lock() {
                        *err = Some(format!("Stream read error: {}", e));
                    }
                    is_complete.store(true, Ordering::SeqCst);
                    break;
                }
            };

            // Handle keepalive comments
            if line.starts_with(':') {
                continue;
            }

            // Parse event line
            if let Some(event_type) = line.strip_prefix("event: ") {
                current_event = event_type.to_string();
            } else if let Some(data) = line.strip_prefix("data: ") {
                current_data = data.to_string();
            } else if line.is_empty() && !current_event.is_empty() {
                // Process complete event
                match current_event.as_str() {
                    "meta" => {
                        if let Ok(data) = serde_json::from_str::<MetaData>(&current_data) {
                            if let Ok(mut m) = meta.lock() {
                                *m = Some(SummarizeMeta {
                                    model_label: data.model_label,
                                    input_summary: data.input_summary,
                                    from_cache: data.summary_from_cache.unwrap_or(false),
                                });
                            }
                        }
                    }
                    "status" => {
                        if let Ok(data) = serde_json::from_str::<StatusData>(&current_data) {
                            if let Ok(mut s) = status.lock() {
                                *s = data.text;
                            }
                        }
                    }
                    "chunk" => {
                        if let Ok(data) = serde_json::from_str::<ChunkData>(&current_data) {
                            if let Ok(mut c) = content.lock() {
                                c.push_str(&data.text);
                            }
                        }
                    }
                    "error" => {
                        if let Ok(data) = serde_json::from_str::<ErrorData>(&current_data) {
                            if let Ok(mut err) = error.lock() {
                                *err = Some(data.message);
                            }
                        }
                        is_complete.store(true, Ordering::SeqCst);
                        break;
                    }
                    "done" => {
                        is_complete.store(true, Ordering::SeqCst);
                        break;
                    }
                    _ => {}
                }

                current_event.clear();
                current_data.clear();
            }
        }

        Ok(())
    }
}
