//! Helper binary to test the new "fetch recent raindrops" approach before
//! wiring it into the main TUI. It pages through Raindrop items sorted by
//! `lastUpdate`, stopping once everything returned is older than the chosen
//! timestamp. This simulates the logic we plan to adopt in the app and lets
//! us confirm it surfaces freshly created bookmarks.

use anyhow::Result;
use chrono::{Duration, Utc};
use pkt_tui::raindrop::{self, RaindropAPISync, COLLECTION_ALL};
use std::env;

fn main() -> Result<()> {
    env_logger::init();

    let since = determine_since_timestamp()?;
    println!("Using since timestamp: {}", since);

    let token = raindrop::token::get_or_prompt_token()?;
    let client = RaindropAPISync::new(&token)?;

    let items = client.get_raindrops_since(COLLECTION_ALL, &since)?;
    if items.is_empty() {
        println!("No items were created or updated since {}", since);
    } else {
        println!("Found {} candidate items:", items.len());
        for item in &items {
            println!(
                "- {} | {} | created {} | lastUpdate {}",
                item.id,
                item.display_title(),
                item.created,
                item.last_update
            );
        }
    }

    Ok(())
}

fn determine_since_timestamp() -> Result<String> {
    if let Some(ts) = env::args().nth(1) {
        return Ok(ts);
    }

    if let Some(ts) = raindrop::get_last_sync() {
        return Ok(ts);
    }

    let one_day = Duration::try_days(1).expect("1 day duration");
    let fallback = (Utc::now() - one_day).to_rfc3339();
    println!(
        "No CLI argument or stored last_sync found; defaulting to {}",
        fallback
    );
    Ok(fallback)
}
