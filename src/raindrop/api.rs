use anyhow::{bail, Context, Result};
use reqwest::{header, Client, StatusCode};
use std::time::Duration;
use tokio::runtime::Runtime;
use tokio::time::sleep;

use super::models::*;

const BASE_URL: &str = "https://api.raindrop.io/rest/v1";
const USER_AGENT: &str = "pkt-tui/raindrop";
const PAGE_SIZE: u32 = 50;

/// Async Raindrop API client
#[derive(Debug, Clone)]
pub struct RaindropAPI {
    token: String,
    client: Client,
}

impl RaindropAPI {
    /// Create a new API client with the given token
    pub fn new(token: impl Into<String>) -> Result<Self> {
        let token = token.into();

        let mut headers = header::HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            header::HeaderValue::from_str(&format!("Bearer {}", token))
                .context("Invalid token format")?,
        );
        headers.insert(
            header::USER_AGENT,
            header::HeaderValue::from_static(USER_AGENT),
        );
        headers.insert(
            header::CONTENT_TYPE,
            header::HeaderValue::from_static("application/json"),
        );

        let client = Client::builder()
            .default_headers(headers)
            .build()
            .context("Failed to build HTTP client")?;

        Ok(Self { token, client })
    }

    /// Get the current user info
    pub async fn get_user(&self) -> Result<User> {
        let url = format!("{}/user", BASE_URL);
        let response = self.client.get(&url).send().await?;

        self.handle_response_status(&response)?;

        let data: UserResponse = response
            .json()
            .await
            .context("Failed to parse user response")?;
        if !data.result {
            bail!("API returned result=false for user request");
        }
        Ok(data.user)
    }

    /// Get all collections (root level)
    pub async fn get_collections(&self) -> Result<Vec<Collection>> {
        let url = format!("{}/collections", BASE_URL);
        let response = self.client.get(&url).send().await?;

        self.handle_response_status(&response)?;

        let data: CollectionsResponse = response
            .json()
            .await
            .context("Failed to parse collections response")?;
        if !data.result {
            bail!("API returned result=false for collections request");
        }
        Ok(data.items)
    }

    /// Get child collections
    pub async fn get_child_collections(&self) -> Result<Vec<Collection>> {
        let url = format!("{}/collections/childrens", BASE_URL);
        let response = self.client.get(&url).send().await?;

        self.handle_response_status(&response)?;

        let data: CollectionsResponse = response
            .json()
            .await
            .context("Failed to parse child collections response")?;
        if !data.result {
            bail!("API returned result=false for child collections request");
        }
        Ok(data.items)
    }

    /// Get all collections (root + children)
    pub async fn get_all_collections(&self) -> Result<Vec<Collection>> {
        let mut all = self.get_collections().await?;
        let children = self.get_child_collections().await?;
        all.extend(children);
        Ok(all)
    }

    /// Get raindrops from a collection with pagination
    ///
    /// Use COLLECTION_ALL (0) to get all raindrops
    /// Automatically waits and retries on rate limiting.
    pub async fn get_raindrops(&self, collection_id: i64, page: u32) -> Result<Vec<Raindrop>> {
        self.get_raindrops_sorted(collection_id, page, "-created")
            .await
    }

    /// Get raindrops sorted by a field
    /// Common sorts: "-created", "-lastUpdate", "title", etc.
    pub async fn get_raindrops_sorted(
        &self,
        collection_id: i64,
        page: u32,
        sort: &str,
    ) -> Result<Vec<Raindrop>> {
        let url = format!("{}/raindrops/{}", BASE_URL, collection_id);

        loop {
            let response = self
                .client
                .get(&url)
                .query(&[
                    ("page", page.to_string()),
                    ("perpage", PAGE_SIZE.to_string()),
                    ("sort", sort.to_string()),
                ])
                .send()
                .await?;

            if response.status() == StatusCode::TOO_MANY_REQUESTS {
                let retry_after = response
                    .headers()
                    .get("Retry-After")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(30);

                eprintln!("Rate limited, waiting {} seconds...", retry_after);
                sleep(Duration::from_secs(retry_after)).await;
                continue;
            }

            self.handle_response_status(&response)?;

            let data: RaindropsResponse = response
                .json()
                .await
                .context("Failed to parse raindrops response")?;
            if !data.result {
                bail!("API returned result=false for raindrops request");
            }
            return Ok(data.items);
        }
    }

    /// Get raindrops created or updated since a given timestamp (delta sync)
    /// Uses search filter with created date for efficiency
    pub async fn get_raindrops_since(
        &self,
        collection_id: i64,
        since_timestamp: &str,
    ) -> Result<Vec<Raindrop>> {
        let mut updated_items = Vec::new();
        let mut page = 0u32;

        'pages: loop {
            let items = self
                .get_raindrops_sorted(collection_id, page, "-lastUpdate")
                .await?;

            if items.is_empty() {
                break;
            }

            for item in items {
                let created_match =
                    !item.created.is_empty() && item.created.as_str() >= since_timestamp;
                let updated_match =
                    !item.last_update.is_empty() && item.last_update.as_str() >= since_timestamp;
                log::debug!(
                    "Comparing item {}: created >= {} => {}, lastUpdate >= {} => {}",
                    item.id,
                    since_timestamp,
                    created_match,
                    since_timestamp,
                    updated_match
                );

                if created_match || updated_match {
                    updated_items.push(item);
                } else if !item.last_update.is_empty()
                    && item.last_update.as_str() < since_timestamp
                {
                    // Items are sorted by descending lastUpdate, so once we hit one older
                    // than our cutoff we can stop fetching further pages.
                    break 'pages;
                }
            }

            page += 1;
        }

        Ok(updated_items)
    }

    /// Get raindrops with count (returns items and total count)
    async fn get_raindrops_with_count(
        &self,
        collection_id: i64,
        page: u32,
    ) -> Result<(Vec<Raindrop>, i64)> {
        let url = format!("{}/raindrops/{}", BASE_URL, collection_id);

        loop {
            let response = self
                .client
                .get(&url)
                .query(&[
                    ("page", page.to_string()),
                    ("perpage", PAGE_SIZE.to_string()),
                ])
                .send()
                .await?;

            if response.status() == StatusCode::TOO_MANY_REQUESTS {
                let retry_after = response
                    .headers()
                    .get("Retry-After")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(30);

                eprintln!("Rate limited, waiting {} seconds...", retry_after);
                sleep(Duration::from_secs(retry_after)).await;
                continue;
            }

            self.handle_response_status(&response)?;

            let data: RaindropsResponse = response
                .json()
                .await
                .context("Failed to parse raindrops response")?;
            if !data.result {
                bail!("API returned result=false for raindrops request");
            }
            return Ok((data.items, data.count));
        }
    }

    /// Get ALL raindrops from a collection (handles pagination)
    ///
    /// Progress callback receives (downloaded_count, total_count).
    pub async fn get_all_raindrops<F>(
        &self,
        collection_id: i64,
        mut progress: Option<F>,
    ) -> Result<Vec<Raindrop>>
    where
        F: FnMut(i32, i64),
    {
        let mut all_raindrops = Vec::new();
        let mut page = 0u32;
        let mut total_count = 0i64;

        loop {
            let (raindrops, count) = self.get_raindrops_with_count(collection_id, page).await?;

            if page == 0 {
                total_count = count;
            }

            if raindrops.is_empty() {
                break;
            }

            all_raindrops.extend(raindrops);

            if let Some(ref mut cb) = progress {
                cb(all_raindrops.len() as i32, total_count);
            }

            page += 1;
        }

        Ok(all_raindrops)
    }

    /// Add a new raindrop
    pub async fn add_raindrop(&self, raindrop: &NewRaindrop) -> Result<Raindrop> {
        let url = format!("{}/raindrop", BASE_URL);
        let response = self.client.post(&url).json(raindrop).send().await?;

        self.handle_response_status(&response)?;

        let data: SingleRaindropResponse = response
            .json()
            .await
            .context("Failed to parse add raindrop response")?;
        if !data.result {
            bail!("API returned result=false for add raindrop request");
        }
        Ok(data.item)
    }

    /// Update an existing raindrop
    pub async fn update_raindrop(&self, id: i64, update: &UpdateRaindrop) -> Result<Raindrop> {
        let url = format!("{}/raindrop/{}", BASE_URL, id);
        let response = self.client.put(&url).json(update).send().await?;

        self.handle_response_status(&response)?;

        let data: SingleRaindropResponse = response
            .json()
            .await
            .context("Failed to parse update raindrop response")?;
        if !data.result {
            bail!("API returned result=false for update raindrop request");
        }
        Ok(data.item)
    }

    /// Delete a raindrop (moves to trash, or permanently deletes if already in trash)
    /// Returns Ok(()) even if item was already deleted (404)
    pub async fn delete_raindrop(&self, id: i64) -> Result<()> {
        let url = format!("{}/raindrop/{}", BASE_URL, id);
        let response = self.client.delete(&url).send().await?;

        // Treat 404 as success - item is already gone
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(());
        }

        self.handle_response_status(&response)?;

        let data: DeleteResponse = response
            .json()
            .await
            .context("Failed to parse delete response")?;
        if !data.result {
            bail!("API returned result=false for delete request");
        }
        Ok(())
    }

    /// Move a raindrop to a different collection
    pub async fn move_raindrop(&self, id: i64, collection_id: i64) -> Result<Raindrop> {
        let update = UpdateRaindrop::new().collection(collection_id);
        self.update_raindrop(id, &update).await
    }

    /// Update tags for a raindrop
    pub async fn update_tags(&self, id: i64, tags: Vec<String>) -> Result<Raindrop> {
        let update = UpdateRaindrop::new().tags(tags);
        self.update_raindrop(id, &update).await
    }

    /// Add a tag to a raindrop
    pub async fn add_tag(&self, raindrop: &Raindrop, tag: &str) -> Result<Raindrop> {
        let mut tags = raindrop.tags.clone();
        if !raindrop.has_tag(tag) {
            tags.push(tag.to_string());
        }
        self.update_tags(raindrop.id, tags).await
    }

    /// Remove a tag from a raindrop
    pub async fn remove_tag(&self, raindrop: &Raindrop, tag: &str) -> Result<Raindrop> {
        let tag_lower = tag.to_lowercase();
        let tags: Vec<String> = raindrop
            .tags
            .iter()
            .filter(|t| t.to_lowercase() != tag_lower)
            .cloned()
            .collect();
        self.update_tags(raindrop.id, tags).await
    }

    /// Find a collection by name (case-insensitive)
    pub async fn find_collection_by_name(&self, name: &str) -> Result<Option<Collection>> {
        let collections = self.get_all_collections().await?;
        let name_lower = name.to_lowercase();
        Ok(collections
            .into_iter()
            .find(|c| c.title.to_lowercase() == name_lower))
    }

    /// Handle HTTP response status codes
    fn handle_response_status(&self, response: &reqwest::Response) -> Result<()> {
        match response.status() {
            StatusCode::OK => Ok(()),
            StatusCode::CREATED => Ok(()),
            StatusCode::BAD_REQUEST => bail!("Bad request: invalid parameters"),
            StatusCode::UNAUTHORIZED => bail!("Unauthorized: invalid or expired token"),
            StatusCode::FORBIDDEN => bail!("Forbidden: access denied"),
            StatusCode::NOT_FOUND => bail!("Not found"),
            StatusCode::TOO_MANY_REQUESTS => {
                // Rate limited - get retry-after if available
                let retry_after = response
                    .headers()
                    .get("Retry-After")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(60);
                bail!("Rate limited. Retry after {} seconds", retry_after);
            }
            StatusCode::INTERNAL_SERVER_ERROR => bail!("Internal server error"),
            StatusCode::SERVICE_UNAVAILABLE => bail!("Service unavailable"),
            status => bail!("Unexpected status code: {}", status),
        }
    }
}

/// Synchronous wrapper for RaindropAPI
pub struct RaindropAPISync {
    api: RaindropAPI,
    runtime: Runtime,
}

impl RaindropAPISync {
    /// Create a new sync API client
    pub fn new(token: impl Into<String>) -> Result<Self> {
        let api = RaindropAPI::new(token)?;
        let runtime = Runtime::new().context("Failed to create Tokio runtime")?;
        Ok(Self { api, runtime })
    }

    /// Get the current user info
    pub fn get_user(&self) -> Result<User> {
        self.runtime.block_on(self.api.get_user())
    }

    /// Get all collections
    pub fn get_collections(&self) -> Result<Vec<Collection>> {
        self.runtime.block_on(self.api.get_collections())
    }

    /// Get child collections
    pub fn get_child_collections(&self) -> Result<Vec<Collection>> {
        self.runtime.block_on(self.api.get_child_collections())
    }

    /// Get all collections (root + children)
    pub fn get_all_collections(&self) -> Result<Vec<Collection>> {
        self.runtime.block_on(self.api.get_all_collections())
    }

    /// Get raindrops from a collection (single page)
    pub fn get_raindrops(&self, collection_id: i64, page: u32) -> Result<Vec<Raindrop>> {
        self.runtime
            .block_on(self.api.get_raindrops(collection_id, page))
    }

    /// Get raindrops from a collection using a specific sort order
    pub fn get_raindrops_sorted(
        &self,
        collection_id: i64,
        page: u32,
        sort: &str,
    ) -> Result<Vec<Raindrop>> {
        self.runtime
            .block_on(self.api.get_raindrops_sorted(collection_id, page, sort))
    }

    /// Get ALL raindrops from a collection (handles pagination)
    pub fn get_all_raindrops(&self, collection_id: i64) -> Result<Vec<Raindrop>> {
        self.runtime.block_on(
            self.api
                .get_all_raindrops::<fn(i32, i64)>(collection_id, None),
        )
    }

    /// Get ALL raindrops with progress callback (downloaded, total)
    pub fn get_all_raindrops_with_progress<F>(
        &self,
        collection_id: i64,
        progress: F,
    ) -> Result<Vec<Raindrop>>
    where
        F: FnMut(i32, i64),
    {
        self.runtime
            .block_on(self.api.get_all_raindrops(collection_id, Some(progress)))
    }

    /// Get raindrops updated since a timestamp (delta sync)
    pub fn get_raindrops_since(
        &self,
        collection_id: i64,
        since_timestamp: &str,
    ) -> Result<Vec<Raindrop>> {
        self.runtime
            .block_on(self.api.get_raindrops_since(collection_id, since_timestamp))
    }

    /// Add a new raindrop
    pub fn add_raindrop(&self, raindrop: &NewRaindrop) -> Result<Raindrop> {
        self.runtime.block_on(self.api.add_raindrop(raindrop))
    }

    /// Update an existing raindrop
    pub fn update_raindrop(&self, id: i64, update: &UpdateRaindrop) -> Result<Raindrop> {
        self.runtime.block_on(self.api.update_raindrop(id, update))
    }

    /// Delete a raindrop
    pub fn delete_raindrop(&self, id: i64) -> Result<()> {
        self.runtime.block_on(self.api.delete_raindrop(id))
    }

    /// Move a raindrop to a different collection
    pub fn move_raindrop(&self, id: i64, collection_id: i64) -> Result<Raindrop> {
        self.runtime
            .block_on(self.api.move_raindrop(id, collection_id))
    }

    /// Update tags for a raindrop
    pub fn update_tags(&self, id: i64, tags: Vec<String>) -> Result<Raindrop> {
        self.runtime.block_on(self.api.update_tags(id, tags))
    }

    /// Add a tag to a raindrop
    pub fn add_tag(&self, raindrop: &Raindrop, tag: &str) -> Result<Raindrop> {
        self.runtime.block_on(self.api.add_tag(raindrop, tag))
    }

    /// Remove a tag from a raindrop
    pub fn remove_tag(&self, raindrop: &Raindrop, tag: &str) -> Result<Raindrop> {
        self.runtime.block_on(self.api.remove_tag(raindrop, tag))
    }

    /// Find a collection by name
    pub fn find_collection_by_name(&self, name: &str) -> Result<Option<Collection>> {
        self.runtime
            .block_on(self.api.find_collection_by_name(name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_api_creation() {
        // Just test that we can create the client
        let api = RaindropAPI::new("test_token");
        assert!(api.is_ok());
    }
}
