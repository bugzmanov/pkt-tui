use anyhow::{Context, Result};
use serde::Deserialize;
use std::fs;
use std::io::{self, Write};
use std::path::Path;

const CONFIG_FILE: &str = "config.toml";
const LEGACY_TOKEN_FILE: &str = "raindrop.token";

#[derive(Deserialize, Default)]
struct Config {
    #[serde(default)]
    raindrop: RaindropConfig,
}

#[derive(Deserialize, Default)]
struct RaindropConfig {
    token: Option<String>,
}

/// Get the stored Raindrop API token
pub fn get_token() -> Result<Option<String>> {
    // Env var takes precedence
    if let Ok(token) = std::env::var("RAINDROP_API_TOKEN") {
        if !token.is_empty() {
            return Ok(Some(token));
        }
    }

    // Check config.toml
    let config_path = Path::new(CONFIG_FILE);
    if config_path.exists() {
        let content = fs::read_to_string(config_path).context("Failed to read config file")?;
        let config: Config = toml::from_str(&content).context("Failed to parse config file")?;
        if let Some(token) = config.raindrop.token {
            if !token.is_empty() {
                return Ok(Some(token));
            }
        }
    }

    // Fall back to legacy token file for backwards compatibility
    let legacy_path = Path::new(LEGACY_TOKEN_FILE);
    if legacy_path.exists() {
        let token = fs::read_to_string(legacy_path)
            .context("Failed to read legacy token file")?
            .trim()
            .to_string();
        if !token.is_empty() {
            return Ok(Some(token));
        }
    }

    Ok(None)
}

/// Check if a token exists
pub fn token_exists() -> bool {
    if std::env::var("RAINDROP_API_TOKEN")
        .map(|t| !t.is_empty())
        .unwrap_or(false)
    {
        return true;
    }

    // Check config.toml
    if let Ok(content) = fs::read_to_string(CONFIG_FILE) {
        if let Ok(config) = toml::from_str::<Config>(&content) {
            if config.raindrop.token.is_some() {
                return true;
            }
        }
    }

    Path::new(LEGACY_TOKEN_FILE).exists()
}

/// Prompt the user for their Raindrop API token
pub fn prompt_for_token() -> Result<String> {
    println!();
    println!("╔══════════════════════════════════════════════════════════════════╗");
    println!("║                    Raindrop.io API Token Setup                   ║");
    println!("╠══════════════════════════════════════════════════════════════════╣");
    println!("║                                                                  ║");
    println!("║  To get your API token:                                          ║");
    println!("║                                                                  ║");
    println!("║  1. Go to https://app.raindrop.io/settings/integrations          ║");
    println!("║  2. Scroll down to 'For Developers'                              ║");
    println!("║  3. Click 'Create test token'                                    ║");
    println!("║  4. Copy the token and paste it below                            ║");
    println!("║                                                                  ║");
    println!("╚══════════════════════════════════════════════════════════════════╝");
    println!();

    print!("Enter your Raindrop API token: ");
    io::stdout().flush()?;

    let mut token = String::new();
    io::stdin()
        .read_line(&mut token)
        .context("Failed to read token input")?;

    let token = token.trim().to_string();
    if token.is_empty() {
        anyhow::bail!("Token cannot be empty");
    }

    // Save to config.toml
    save_token_to_config(&token)?;
    println!("Token saved to config.toml!");

    Ok(token)
}

fn save_token_to_config(token: &str) -> Result<()> {
    // Load existing config as a table or create new
    let mut config: toml::Table = if Path::new(CONFIG_FILE).exists() {
        let content = fs::read_to_string(CONFIG_FILE)?;
        toml::from_str(&content).unwrap_or_default()
    } else {
        toml::Table::new()
    };

    // Get or create raindrop section
    let raindrop = config
        .entry("raindrop")
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));

    if let toml::Value::Table(ref mut table) = raindrop {
        table.insert("token".to_string(), toml::Value::String(token.to_string()));
    }

    let content = toml::to_string_pretty(&config)?;
    fs::write(CONFIG_FILE, content)?;

    Ok(())
}

/// Get existing token or prompt for a new one
pub fn get_or_prompt_token() -> Result<String> {
    match get_token()? {
        Some(token) => Ok(token),
        None => prompt_for_token(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn test_env_var_token() {
        // Save original value
        let original = env::var("RAINDROP_API_TOKEN").ok();

        // Test with env var
        env::set_var("RAINDROP_API_TOKEN", "test_token_123");
        let token = get_token().unwrap();
        assert_eq!(token, Some("test_token_123".to_string()));

        // Restore original
        match original {
            Some(val) => env::set_var("RAINDROP_API_TOKEN", val),
            None => env::remove_var("RAINDROP_API_TOKEN"),
        }
    }
}
