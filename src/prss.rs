use anyhow::Context;
use chrono::{DateTime, Local, Utc};
use log::{error, LevelFilter};
use reqwest::header::{ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RssFeedItem {
    pub title: String,
    pub link: String,
    pub source: String,
    pub description: Option<String>,
    pub pub_date: Option<String>,
    pub item_id: String,
    pub matched_tags: Option<Vec<String>>,
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
}

pub struct FeedFetchResult {
    pub items: Vec<RssFeedItem>,
    pub headers: metadata::FeedHeaders,
    pub not_modified: bool,
}

impl RssManager {
    pub fn fetch_and_parse_feed(
        client: &reqwest::blocking::Client,
        url: &str,
        conditional: Option<&metadata::FeedHeaders>,
    ) -> anyhow::Result<FeedFetchResult> {
        let mut request = client
            .get(url)
            .header(
                "User-Agent",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/91.0.4472.124 Safari/537.36",
            );

        if let Some(headers) = conditional {
            if let Some(etag) = &headers.etag {
                request = request.header(IF_NONE_MATCH, etag);
            }
            if let Some(last_modified) = &headers.last_modified {
                request = request.header(IF_MODIFIED_SINCE, last_modified);
            }
        }

        let response = request.send()?;

        if response.status() == StatusCode::NOT_MODIFIED {
            let headers = conditional.cloned().unwrap_or_default();
            return Ok(FeedFetchResult {
                items: Vec::new(),
                headers,
                not_modified: true,
            });
        }

        if !response.status().is_success() {
            error!("Failed to fetch {}: Status {}", url, response.status());
            return Err(anyhow::anyhow!("HTTP error: {}", response.status()));
        }

        let header_map = response.headers().clone();
        let content = response.text()?;

        let response_headers = metadata::FeedHeaders {
            etag: header_map
                .get(ETAG)
                .and_then(|value| value.to_str().ok())
                .map(|s| s.to_string()),
            last_modified: header_map
                .get(LAST_MODIFIED)
                .and_then(|value| value.to_str().ok())
                .map(|s| s.to_string()),
        };

        // Extract base URL for resolving relative links
        let base_url = Self::extract_base_url(url);

        // Try parsing as Atom first
        if let Ok(atom_feed) = atom_syndication::Feed::read_from(content.as_bytes()) {
            let source_name = atom_feed.title().to_string();
            let items = atom_feed
                .entries()
                .iter()
                .map(|entry| {
                    let item_id = format!("{}:{}", source_name, entry.id());
                    let raw_link = Self::select_atom_entry_link(entry)
                        .map(|l| l.href().to_string())
                        .unwrap_or_default();
                    RssFeedItem {
                        title: entry.title().to_string(),
                        link: Self::resolve_url(&raw_link, &base_url),
                        description: entry.content().and_then(|c| c.value()).map(String::from),
                        pub_date: Some(
                            entry
                                .published()
                                .unwrap_or_else(|| entry.updated())
                                .to_string(),
                        ),
                        source: source_name.clone(),
                        item_id,
                        matched_tags: None,
                    }
                })
                .collect();
            return Ok(FeedFetchResult {
                items,
                headers: response_headers,
                not_modified: false,
            });
        }

        // Try parsing as RSS
        match rss::Channel::read_from(content.as_bytes()) {
            Ok(rss_feed) => {
                let source_name = rss_feed.title().to_string();
                let items = rss_feed
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
                            matched_tags: None,
                        }
                    })
                    .collect();
                Ok(FeedFetchResult {
                    items,
                    headers: response_headers,
                    not_modified: false,
                })
            }
            Err(e) => {
                error!("Failed to parse feed from {}: {}", url, e);
                Err(anyhow::anyhow!("Invalid feed format: {}", e))
            }
        }
    }

    /// Extract base URL (scheme + host) from a feed URL
    fn extract_base_url(url: &str) -> String {
        // Parse URL to get scheme://host
        if let Some(scheme_end) = url.find("://") {
            let after_scheme = &url[scheme_end + 3..];
            if let Some(path_start) = after_scheme.find('/') {
                return url[..scheme_end + 3 + path_start].to_string();
            }
        }
        url.to_string()
    }

    /// Resolve a potentially relative URL against a base URL
    fn resolve_url(link: &str, base_url: &str) -> String {
        if link.starts_with("http://") || link.starts_with("https://") {
            // Already absolute
            link.to_string()
        } else if link.starts_with('/') {
            // Absolute path - prepend base URL
            format!("{}{}", base_url, link)
        } else {
            // Relative path - just return as-is (rare case)
            link.to_string()
        }
    }

    /// Blogger feeds often list comment links first; prefer article/permalink links.
    fn select_atom_entry_link(entry: &atom_syndication::Entry) -> Option<&atom_syndication::Link> {
        entry
            .links()
            .iter()
            .find(|link| link.rel().eq_ignore_ascii_case("alternate"))
            .or_else(|| {
                entry.links().iter().find(|link| {
                    link.mime_type()
                        .map(|mime| mime.contains("html"))
                        .unwrap_or(false)
                })
            })
            .or_else(|| entry.links().first())
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

pub mod metadata {
    use anyhow::Context;
    use serde::{Deserialize, Serialize};
    use std::collections::{HashMap, HashSet};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    const METADATA_FILE: &str = "rss/feed_meta.json";

    #[derive(Clone, Debug, Default, Serialize, Deserialize)]
    pub struct FeedHeaders {
        pub etag: Option<String>,
        pub last_modified: Option<String>,
    }

    #[derive(Default, Serialize, Deserialize)]
    struct MetadataData {
        feeds: HashMap<String, FeedHeaders>,
        hidden_items: HashSet<String>,
    }

    pub struct RssMetadataStore {
        path: PathBuf,
        data: MetadataData,
    }

    impl RssMetadataStore {
        pub fn load() -> anyhow::Result<Self> {
            let metadata_path = PathBuf::from(METADATA_FILE);
            if let Some(parent) = metadata_path.parent() {
                fs::create_dir_all(parent)?;
            }

            let data = if metadata_path.exists() {
                let content =
                    fs::read_to_string(&metadata_path).context("Failed to read metadata file")?;
                serde_json::from_str::<MetadataData>(&content)
                    .context("Failed to parse metadata file")?
            } else {
                MetadataData::default()
            };

            Ok(Self {
                path: metadata_path,
                data,
            })
        }

        pub fn save(&self) -> anyhow::Result<()> {
            let serialized = serde_json::to_string_pretty(&self.data)?;
            fs::write(&self.path, serialized)?;
            Ok(())
        }

        pub fn feed_headers(&self, url: &str) -> Option<FeedHeaders> {
            self.data.feeds.get(url).cloned()
        }

        pub fn update_feed_headers(&mut self, url: &str, headers: FeedHeaders) {
            self.data.feeds.insert(url.to_string(), headers);
        }

        pub fn is_hidden(&self, item_id: &str) -> bool {
            self.data.hidden_items.contains(item_id)
        }

        pub fn hide_item(&mut self, item_id: String) {
            self.data.hidden_items.insert(item_id);
        }

        pub fn hidden_items(&self) -> HashSet<String> {
            self.data.hidden_items.clone()
        }
    }

    pub type SharedMetadataStore = Arc<Mutex<RssMetadataStore>>;

    pub fn load_shared_store() -> anyhow::Result<SharedMetadataStore> {
        Ok(Arc::new(Mutex::new(RssMetadataStore::load()?)))
    }
}

pub mod hidden_items {
    use super::metadata::SharedMetadataStore;
    use anyhow::anyhow;

    #[derive(Clone)]
    pub struct HiddenItems {
        metadata: SharedMetadataStore,
    }

    impl HiddenItems {
        pub fn new(metadata: SharedMetadataStore) -> Self {
            Self { metadata }
        }

        pub fn load(metadata: SharedMetadataStore) -> anyhow::Result<Self> {
            Ok(Self { metadata })
        }

        pub fn hide_item(&self, item_id: String) -> anyhow::Result<()> {
            let mut guard = self
                .metadata
                .lock()
                .map_err(|_| anyhow!("Failed to lock RSS metadata store"))?;
            if !guard.is_hidden(&item_id) {
                guard.hide_item(item_id);
                guard.save()?;
            }
            Ok(())
        }

        pub fn is_hidden(&self, item_id: &str) -> bool {
            if let Ok(guard) = self.metadata.lock() {
                guard.is_hidden(item_id)
            } else {
                false
            }
        }
    }
}
