use anyhow::Context;
use chrono::{DateTime, Local, Utc};
use log::{error, LevelFilter};
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct RssFeedItem {
    pub title: String,
    pub link: String,
    pub source: String,
    pub description: Option<String>,
    pub pub_date: Option<String>,
    pub item_id: String,
}

pub struct RssManager {
    subscriptions_path: PathBuf,
}

impl RssManager {
    pub fn new() -> Self {
        Self {
            subscriptions_path: PathBuf::from("rss/subscriptions"),
        }
    }

    fn ensure_subscriptions_file(&self) -> anyhow::Result<()> {
        // Create rss directory if it doesn't exist
        if let Some(parent) = self.subscriptions_path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Create subscriptions file if it doesn't exist
        if !self.subscriptions_path.exists() {
            File::create(&self.subscriptions_path)?;
        }

        Ok(())
    }

    pub fn load_subscriptions(&self) -> anyhow::Result<Vec<String>> {
        self.ensure_subscriptions_file()?;

        let file = File::open(&self.subscriptions_path)
            .context("Failed to open RSS subscriptions file")?;
        let reader = BufReader::new(file);

        let mut subscriptions = Vec::new();
        for line in reader.lines() {
            let line = line?;
            let trimmed = line.trim();
            if !trimmed.is_empty() && !trimmed.starts_with('#') {
                subscriptions.push(trimmed.to_string());
            }
        }

        Ok(subscriptions)
    }

    pub fn add_subscription(&self, url: &str) -> anyhow::Result<()> {
        self.ensure_subscriptions_file()?;

        let mut subscriptions = self.load_subscriptions()?;
        if !subscriptions.contains(&url.to_string()) {
            subscriptions.push(url.to_string());
            let content = subscriptions.join("\n");
            fs::write(&self.subscriptions_path, content)?;
        }

        Ok(())
    }

    pub fn remove_subscription(&self, url: &str) -> anyhow::Result<()> {
        self.ensure_subscriptions_file()?;

        let mut subscriptions = self.load_subscriptions()?;
        if let Some(pos) = subscriptions.iter().position(|x| x == url) {
            subscriptions.remove(pos);
            let content = subscriptions.join("\n");
            fs::write(&self.subscriptions_path, content)?;
        }

        Ok(())
    }

    pub fn fetch_and_parse_feed(
        client: &reqwest::blocking::Client,
        url: &str,
    ) -> anyhow::Result<Vec<RssFeedItem>> {
        let response = client
                    .get(url)
                    .header(
                        "User-Agent",
                        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/91.0.4472.124 Safari/537.36"
                    )
                    .send()?;

        if !response.status().is_success() {
            error!("Failed to fetch {}: Status {}", url, response.status());
            return Err(anyhow::anyhow!("HTTP error: {}", response.status()));
        }

        let content = response.text()?;

        // Try parsing as Atom first
        if let Ok(atom_feed) = atom_syndication::Feed::read_from(content.as_bytes()) {
            let source_name = atom_feed.title().to_string();
            return Ok(atom_feed
                .entries()
                .iter()
                .map(|entry| {
                    let item_id = format!("{}:{}", source_name, entry.id());
                    RssFeedItem {
                        title: entry.title().to_string(),
                        link: entry
                            .links()
                            .first()
                            .map(|l| l.href().to_string())
                            .unwrap_or_default(),
                        description: entry.content().and_then(|c| c.value()).map(String::from),
                        pub_date: Some(
                            entry
                                .published()
                                .unwrap_or_else(|| entry.updated())
                                .to_string(),
                        ),
                        source: source_name.clone(),
                        item_id,
                    }
                })
                .collect());
        }

        // Try parsing as RSS
        match rss::Channel::read_from(content.as_bytes()) {
            Ok(rss_feed) => {
                let source_name = rss_feed.title().to_string();
                Ok(rss_feed
                    .items()
                    .iter()
                    .map(|item| {
                        let item_id = format!(
                            "{}:{}",
                            source_name,
                            item.guid()
                                .map(|g| g.value().to_string())
                                .or_else(|| item.link().map(String::from))
                                .unwrap_or_else(|| item.title().unwrap_or("unknown").to_string())
                        );
                        RssFeedItem {
                            title: item.title().unwrap_or("Untitled").to_string(),
                            link: item.link().unwrap_or_default().to_string(),
                            description: item.description().map(String::from),
                            pub_date: item
                                .pub_date()
                                .and_then(|date| Self::format_pub_date(&date))
                                .or(item.pub_date().map(String::from)),
                            source: source_name.clone(),
                            item_id,
                        }
                    })
                    .collect())
            }
            Err(e) => {
                error!("Failed to parse feed from {}: {}", url, e);
                Err(anyhow::anyhow!("Invalid feed format: {}", e))
            }
        }
    }
    fn format_pub_date(date_str: &str) -> Option<String> {
        // Try to parse the RFC 2822 date format used by RSS feeds
        if let Ok(datetime) = DateTime::parse_from_rfc2822(date_str) {
            let utc_dt: DateTime<Utc> = datetime.to_utc();
            Some(format!("{:?}", utc_dt)) // This will output in RFC 3339 format
        } else {
            error!("Failed to parse date: {}", date_str);
            None
        }
    }
}
