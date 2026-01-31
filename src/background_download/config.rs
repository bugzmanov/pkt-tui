//! Configuration for background downloads.

use anyhow::Result;
use serde::Deserialize;
use std::fs;
use std::path::Path;

const CONFIG_FILE: &str = "config.toml";

/// Configuration for background download feature.
#[derive(Debug, Clone)]
pub struct BackgroundDownloadConfig {
    /// Whether background downloads are enabled.
    pub enabled: bool,
    /// Number of days to look back for items to download.
    pub lookback_days: u32,
}

impl Default for BackgroundDownloadConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            lookback_days: 14,
        }
    }
}

#[derive(Deserialize, Default)]
struct Config {
    #[serde(default)]
    background_download: BackgroundDownloadConfigToml,
}

#[derive(Deserialize, Default)]
struct BackgroundDownloadConfigToml {
    enabled: Option<bool>,
    lookback_days: Option<u32>,
}

/// Load background download configuration from config.toml.
pub fn load_config() -> Result<BackgroundDownloadConfig> {
    let config_path = Path::new(CONFIG_FILE);
    if !config_path.exists() {
        return Ok(BackgroundDownloadConfig::default());
    }

    let content = fs::read_to_string(config_path)?;
    let config: Config = toml::from_str(&content)?;

    Ok(BackgroundDownloadConfig {
        enabled: config.background_download.enabled.unwrap_or(true),
        lookback_days: config.background_download.lookback_days.unwrap_or(14),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = BackgroundDownloadConfig::default();
        assert!(config.enabled);
        assert_eq!(config.lookback_days, 14);
    }
}
