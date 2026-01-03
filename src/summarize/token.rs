use anyhow::{Context, Result};
use serde::Deserialize;
use std::fs;
use std::path::Path;
use std::time::Duration;

const CONFIG_FILE: &str = "config.toml";

#[derive(Deserialize, Default)]
struct Config {
    #[serde(default)]
    summarize: SummarizeConfig,
}

#[derive(Deserialize, Default)]
struct SummarizeConfig {
    token: Option<String>,
    #[serde(default = "default_daemon_url")]
    daemon_url: String,
    #[serde(default = "default_length")]
    length: String,
}

fn default_daemon_url() -> String {
    "http://127.0.0.1:8787".to_string()
}

fn default_length() -> String {
    "xl".to_string()
}

/// Get the stored Summarize daemon token
pub fn get_token() -> Result<Option<String>> {
    // Env var takes precedence
    if let Ok(token) = std::env::var("SUMMARIZE_DAEMON_TOKEN") {
        if !token.is_empty() {
            return Ok(Some(token));
        }
    }

    // Check config.toml
    let config_path = Path::new(CONFIG_FILE);
    if config_path.exists() {
        let content = fs::read_to_string(config_path).context("Failed to read config file")?;
        let config: Config = toml::from_str(&content).context("Failed to parse config file")?;
        if let Some(token) = config.summarize.token {
            if !token.is_empty() {
                return Ok(Some(token));
            }
        }
    }

    Ok(None)
}

/// Get daemon URL from config
pub fn get_daemon_url() -> String {
    // Check config.toml
    let config_path = Path::new(CONFIG_FILE);
    if config_path.exists() {
        if let Ok(content) = fs::read_to_string(config_path) {
            if let Ok(config) = toml::from_str::<Config>(&content) {
                return config.summarize.daemon_url;
            }
        }
    }

    default_daemon_url()
}

/// Get summary length from config
pub fn get_length() -> String {
    // Check config.toml
    let config_path = Path::new(CONFIG_FILE);
    if config_path.exists() {
        if let Ok(content) = fs::read_to_string(config_path) {
            if let Ok(config) = toml::from_str::<Config>(&content) {
                return config.summarize.length;
            }
        }
    }

    default_length()
}

/// Check if daemon is reachable (with short timeout)
pub fn check_daemon_health() -> bool {
    let url = get_daemon_url();
    let client = match reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
    {
        Ok(c) => c,
        Err(_) => return false,
    };

    match client.get(format!("{}/health", url)).send() {
        Ok(resp) => resp.status().is_success(),
        Err(_) => false,
    }
}
