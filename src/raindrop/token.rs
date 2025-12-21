use anyhow::{Context, Result};
use std::fs;
use std::io::{self, Write};
use std::path::Path;

const TOKEN_FILE: &str = "raindrop.token";

/// Get the stored Raindrop API token
pub fn get_token() -> Result<Option<String>> {
    // First check environment variable
    if let Ok(token) = std::env::var("RAINDROP_API_TOKEN") {
        if !token.is_empty() {
            return Ok(Some(token));
        }
    }

    // Then check file
    let path = Path::new(TOKEN_FILE);
    if path.exists() {
        let token = fs::read_to_string(path)
            .context("Failed to read token file")?
            .trim()
            .to_string();
        if !token.is_empty() {
            return Ok(Some(token));
        }
    }

    Ok(None)
}

/// Store the Raindrop API token to file
pub fn store_token(token: &str) -> Result<()> {
    fs::write(TOKEN_FILE, token.trim()).context("Failed to write token file")?;
    Ok(())
}

/// Check if a token exists
pub fn token_exists() -> bool {
    if std::env::var("RAINDROP_API_TOKEN")
        .map(|t| !t.is_empty())
        .unwrap_or(false)
    {
        return true;
    }
    Path::new(TOKEN_FILE).exists()
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

    // Store the token
    store_token(&token)?;
    println!("Token saved successfully!");

    Ok(token)
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
