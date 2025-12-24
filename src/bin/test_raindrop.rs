//! Test binary for Raindrop.io API integration
//!
//! Run with: cargo run --bin test_raindrop
//!
//! Set RAINDROP_API_TOKEN env var or it will prompt for token.

use anyhow::Result;
use pkt_tui::raindrop::{token, NewRaindrop, RaindropAPISync, UpdateRaindrop, COLLECTION_ALL};

fn main() -> Result<()> {
    println!("=== Raindrop.io API Integration Test ===\n");

    // Step 1: Get token
    println!("1. Getting API token...");
    let token = token::get_or_prompt_token()?;
    println!(
        "   ✓ Token obtained ({}...)\n",
        &token[..8.min(token.len())]
    );

    // Step 2: Create API client
    println!("2. Creating API client...");
    let client = RaindropAPISync::new(&token)?;
    println!("   ✓ Client created\n");

    // Step 3: Get user info
    println!("3. Fetching user info...");
    let user = client.get_user()?;
    println!("   ✓ User: {} ({})", user.full_name, user.email);
    println!("   ✓ Pro account: {}\n", user.pro);

    // Step 4: Get collections
    println!("4. Fetching collections...");
    let collections = client.get_all_collections()?;
    println!("   ✓ Found {} collections:", collections.len());
    for col in &collections {
        let parent_info = if col.is_root() {
            String::new()
        } else {
            format!(
                " (child of {})",
                col.parent.as_ref().map(|p| p.id).unwrap_or(0)
            )
        };
        println!(
            "     - [{}] {} ({} items){}",
            col.id, col.title, col.count, parent_info
        );
    }
    println!();

    // Step 5: Check for Archive collection
    println!("5. Looking for 'Archive' collection...");
    let archive = collections
        .iter()
        .find(|c| c.title.to_lowercase() == "archive");
    match archive {
        Some(col) => println!(
            "   ✓ Found Archive collection: id={}, {} items\n",
            col.id, col.count
        ),
        None => println!("   ⚠ No 'Archive' collection found (will need to create one)\n"),
    }

    // Step 6: Fetch raindrops (first page only to be quick)
    println!("6. Fetching raindrops (first page)...");
    let raindrops = client.get_raindrops(COLLECTION_ALL, 0)?;
    println!("   ✓ Got {} raindrops on first page", raindrops.len());
    if !raindrops.is_empty() {
        println!("   Sample items:");
        for rd in raindrops.iter().take(3) {
            let tags_str = if rd.tags.is_empty() {
                String::from("no tags")
            } else {
                rd.tags.join(", ")
            };
            println!(
                "     - [{}] {} ({})",
                rd.id,
                truncate(&rd.title, 40),
                tags_str
            );
        }
    }
    println!();

    // Step 7: Test add/update/delete cycle (optional - requires confirmation)
    println!("7. Test write operations?");
    println!("   This will create a test raindrop, update it, then delete it.");
    print!("   Proceed? [y/N]: ");
    std::io::Write::flush(&mut std::io::stdout())?;

    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;

    if input.trim().to_lowercase() == "y" {
        test_write_operations(&client)?;
    } else {
        println!("   Skipped write tests.\n");
    }

    println!("=== All tests completed successfully! ===");
    Ok(())
}

fn test_write_operations(client: &RaindropAPISync) -> Result<()> {
    // Add a test raindrop
    println!("\n   7a. Creating test raindrop...");
    let new_raindrop = NewRaindrop::new("https://example.com/pkt-tui-test")
        .with_title("PKT-TUI Integration Test")
        .with_tags(vec!["test".into(), "pkt-tui".into()]);

    let created = client.add_raindrop(&new_raindrop)?;
    println!("       ✓ Created raindrop id={}", created.id);

    // Update it
    println!("   7b. Updating raindrop...");
    let update = UpdateRaindrop::new()
        .title("PKT-TUI Integration Test (Updated)")
        .tags(vec!["test".into(), "pkt-tui".into(), "updated".into()]);

    let updated = client.update_raindrop(created.id, &update)?;
    println!(
        "       ✓ Updated: title='{}', tags={:?}",
        updated.title, updated.tags
    );

    // Delete it
    println!("   7c. Deleting test raindrop...");
    client.delete_raindrop(created.id)?;
    println!("       ✓ Deleted (moved to trash)");

    // Delete again to permanently remove
    println!("   7d. Permanently deleting from trash...");
    client.delete_raindrop(created.id)?;
    println!("       ✓ Permanently deleted\n");

    Ok(())
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len - 3])
    }
}
