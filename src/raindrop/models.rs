use chrono::DateTime;
use serde::{Deserialize, Serialize};

// Special collection IDs
pub const COLLECTION_ALL: i64 = 0;
pub const COLLECTION_UNSORTED: i64 = -1;
pub const COLLECTION_TRASH: i64 = -99;

/// Reference to a collection in API responses ({"$id": 123})
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CollectionRef {
    #[serde(rename = "$id")]
    pub id: i64,
}

impl CollectionRef {
    pub fn new(id: i64) -> Self {
        Self { id }
    }
}

/// Raindrop item type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum RaindropType {
    #[default]
    Link,
    Article,
    Image,
    Video,
    Document,
    Audio,
    Book,
    #[serde(other)]
    Unknown,
}

impl RaindropType {
    pub fn as_str(&self) -> &'static str {
        match self {
            RaindropType::Link => "article", // treat links as articles for stats
            RaindropType::Article => "article",
            RaindropType::Image => "article",
            RaindropType::Video => "video",
            RaindropType::Document => "pdf", // treat documents as PDFs for stats
            RaindropType::Audio => "video",  // treat audio as video for stats
            RaindropType::Book => "pdf",
            RaindropType::Unknown => "article",
        }
    }
}

impl std::fmt::Display for RaindropType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RaindropType::Link => write!(f, "link"),
            RaindropType::Article => write!(f, "article"),
            RaindropType::Image => write!(f, "image"),
            RaindropType::Video => write!(f, "video"),
            RaindropType::Document => write!(f, "document"),
            RaindropType::Audio => write!(f, "audio"),
            RaindropType::Book => write!(f, "book"),
            RaindropType::Unknown => write!(f, "unknown"),
        }
    }
}

/// Media item attached to a raindrop
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Media {
    pub link: String,
    #[serde(rename = "type")]
    pub media_type: RaindropType,
}

/// A raindrop (bookmark) from the API
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Raindrop {
    #[serde(rename = "_id")]
    pub id: i64,

    pub title: String,

    pub link: String,

    #[serde(default)]
    pub excerpt: String,

    #[serde(default)]
    pub note: String,

    #[serde(default)]
    pub tags: Vec<String>,

    pub collection: CollectionRef,

    #[serde(rename = "type", default)]
    pub item_type: RaindropType,

    #[serde(default)]
    pub domain: String,

    #[serde(default)]
    pub created: String, // ISO 8601

    #[serde(rename = "lastUpdate", default)]
    pub last_update: String, // ISO 8601

    #[serde(default)]
    pub broken: bool,

    #[serde(default)]
    pub cover: String,

    #[serde(default)]
    pub media: Vec<Media>,
}

impl Raindrop {
    /// Check if this raindrop is in the trash
    pub fn is_trashed(&self) -> bool {
        self.collection.id == COLLECTION_TRASH
    }

    /// Check if this raindrop is unsorted
    pub fn is_unsorted(&self) -> bool {
        self.collection.id == COLLECTION_UNSORTED
    }

    /// Check if this raindrop has a specific tag (case-insensitive)
    pub fn has_tag(&self, tag: &str) -> bool {
        let tag_lower = tag.to_lowercase();
        self.tags.iter().any(|t| t.to_lowercase() == tag_lower)
    }

    /// Get display title (fallback to link if empty)
    pub fn display_title(&self) -> &str {
        if self.title.is_empty() {
            &self.link
        } else {
            &self.title
        }
    }

    // TableRow-compatible methods for UI compatibility

    /// Get formatted date string
    pub fn date(&self) -> String {
        if self.created.is_empty() {
            return "unknown".to_string();
        }
        // Parse ISO 8601 date
        let parsed = DateTime::parse_from_rfc3339(&self.created)
            .or_else(|_| DateTime::parse_from_rfc3339(&self.created.replace("Z", "+00:00")));

        match parsed {
            Ok(dt) => dt.format("%Y-%m-%d").to_string(),
            Err(_) => self.created.chars().take(10).collect(),
        }
    }

    /// Get time added as Unix timestamp
    pub fn time_added(&self) -> u64 {
        if self.created.is_empty() {
            return 0;
        }
        DateTime::parse_from_rfc3339(&self.created)
            .or_else(|_| DateTime::parse_from_rfc3339(&self.created.replace("Z", "+00:00")))
            .map(|dt| dt.timestamp() as u64)
            .unwrap_or(0)
    }

    /// Get item type as string
    pub fn type_str(&self) -> &str {
        match self.item_type {
            RaindropType::Video => "video",
            RaindropType::Article => {
                if self.link.contains(".pdf")
                    || crate::utils::is_acm_pdf_url(&self.link)
                    || crate::utils::is_alphaxiv_url(&self.link)
                    || crate::utils::is_arxiv_url(&self.link)
                {
                    "pdf"
                } else {
                    "article"
                }
            }
            RaindropType::Document => "pdf",
            RaindropType::Image => "image",
            RaindropType::Audio => "audio",
            RaindropType::Book => "book",
            RaindropType::Unknown => "unknown",
            RaindropType::Link => {
                // Check URL for hints
                if self.link.contains("youtube.com") || self.link.contains("youtu.be") {
                    "video"
                } else if self.link.contains(".pdf")
                    || crate::utils::is_acm_pdf_url(&self.link)
                    || crate::utils::is_alphaxiv_url(&self.link)
                    || crate::utils::is_arxiv_url(&self.link)
                {
                    "pdf"
                } else {
                    "article"
                }
            }
        }
    }

    /// Add a tag (mutably)
    pub fn add_tag(&mut self, tag: &str) {
        if !self.has_tag(tag) {
            self.tags.push(tag.to_string());
        }
    }

    /// Remove a tag (mutably)
    pub fn remove_tag(&mut self, tag: &str) {
        let tag_lower = tag.to_lowercase();
        self.tags.retain(|t| t.to_lowercase() != tag_lower);
    }

    /// Rename title
    pub fn rename_title(&mut self, new_title: String) {
        self.title = new_title;
    }
}

/// Payload for creating a new raindrop
#[derive(Debug, Clone, Serialize)]
pub struct NewRaindrop {
    pub link: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub excerpt: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,

    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub collection: Option<CollectionRef>,

    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub item_type: Option<RaindropType>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
}

impl NewRaindrop {
    pub fn new(link: impl Into<String>) -> Self {
        Self {
            link: link.into(),
            title: None,
            excerpt: None,
            note: None,
            tags: Vec::new(),
            collection: None,
            item_type: None,
            created: None,
        }
    }

    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn with_tags(mut self, tags: Vec<String>) -> Self {
        self.tags = tags;
        self
    }

    pub fn with_collection(mut self, collection_id: i64) -> Self {
        self.collection = Some(CollectionRef::new(collection_id));
        self
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }
}

/// Payload for updating an existing raindrop
#[derive(Debug, Clone, Serialize, Default)]
pub struct UpdateRaindrop {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub excerpt: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub collection: Option<CollectionRef>,

    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub item_type: Option<RaindropType>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
}

impl UpdateRaindrop {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn tags(mut self, tags: Vec<String>) -> Self {
        self.tags = Some(tags);
        self
    }

    pub fn collection(mut self, collection_id: i64) -> Self {
        self.collection = Some(CollectionRef::new(collection_id));
        self
    }

    pub fn note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }

    pub fn link(mut self, link: impl Into<String>) -> Self {
        self.link = Some(link.into());
        self
    }
}

/// A collection from the API
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Collection {
    #[serde(rename = "_id")]
    pub id: i64,

    pub title: String,

    #[serde(default)]
    pub count: i64,

    #[serde(default)]
    pub color: Option<String>,

    #[serde(default)]
    pub cover: Vec<String>,

    #[serde(default)]
    pub created: String,

    #[serde(rename = "lastUpdate", default)]
    pub last_update: String,

    #[serde(default)]
    pub public: bool,

    #[serde(default)]
    pub sort: i64,

    #[serde(default)]
    pub view: String,

    #[serde(default)]
    pub expanded: bool,

    #[serde(default)]
    pub parent: Option<CollectionRef>,
}

impl Collection {
    /// Check if this is a root collection (no parent)
    pub fn is_root(&self) -> bool {
        self.parent.is_none()
    }
}

/// User information from the API
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    #[serde(rename = "_id")]
    pub id: i64,

    #[serde(default)]
    pub email: String,

    #[serde(rename = "fullName", default)]
    pub full_name: String,

    #[serde(default)]
    pub pro: bool,

    #[serde(rename = "proExpire", default)]
    pub pro_expire: Option<String>,
}

// API Response wrappers

#[derive(Debug, Clone, Deserialize)]
pub struct RaindropsResponse {
    pub result: bool,
    #[serde(default)]
    pub items: Vec<Raindrop>,
    #[serde(default)]
    pub count: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SingleRaindropResponse {
    pub result: bool,
    pub item: Raindrop,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CollectionsResponse {
    pub result: bool,
    #[serde(default)]
    pub items: Vec<Collection>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UserResponse {
    pub result: bool,
    pub user: User,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DeleteResponse {
    pub result: bool,
}

/// Local cache for offline support
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RaindropCache {
    pub raindrops: Vec<Raindrop>,
    pub collections: Vec<Collection>,
    pub last_sync: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_raindrop_deserialize() {
        let json = r#"{
            "_id": 123456,
            "title": "Test Article",
            "link": "https://example.com",
            "excerpt": "A test excerpt",
            "note": "",
            "tags": ["rust", "programming"],
            "collection": {"$id": 789},
            "type": "article",
            "domain": "example.com",
            "created": "2024-01-15T10:30:00.000Z",
            "lastUpdate": "2024-01-20T15:45:00.000Z",
            "broken": false,
            "cover": "https://example.com/cover.jpg",
            "media": []
        }"#;

        let raindrop: Raindrop = serde_json::from_str(json).unwrap();
        assert_eq!(raindrop.id, 123456);
        assert_eq!(raindrop.title, "Test Article");
        assert_eq!(raindrop.collection.id, 789);
        assert_eq!(raindrop.item_type, RaindropType::Article);
        assert!(raindrop.has_tag("rust"));
        assert!(raindrop.has_tag("RUST")); // case insensitive
        assert!(!raindrop.has_tag("python"));
    }

    #[test]
    fn test_collection_deserialize() {
        let json = r##"{
            "_id": 789,
            "title": "Archive",
            "count": 42,
            "color": "#FF0000",
            "cover": [],
            "created": "2024-01-01T00:00:00.000Z",
            "lastUpdate": "2024-01-20T00:00:00.000Z",
            "public": false,
            "sort": 0,
            "view": "list",
            "expanded": true,
            "parent": null
        }"##;

        let collection: Collection = serde_json::from_str(json).unwrap();
        assert_eq!(collection.id, 789);
        assert_eq!(collection.title, "Archive");
        assert_eq!(collection.count, 42);
        assert!(collection.is_root());
    }

    #[test]
    fn test_new_raindrop_serialize() {
        let new_raindrop = NewRaindrop::new("https://example.com")
            .with_title("Test")
            .with_tags(vec!["tag1".into(), "tag2".into()])
            .with_collection(123);

        let json = serde_json::to_string(&new_raindrop).unwrap();
        assert!(json.contains("\"link\":\"https://example.com\""));
        assert!(json.contains("\"title\":\"Test\""));
        assert!(json.contains("\"$id\":123"));
    }
}
