use anyhow::{bail, Context, Result};
use reqwest::{header, Client, StatusCode};
use tokio::runtime::Runtime;

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

        let data: UserResponse = response.json().await.context("Failed to parse user response")?;
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
    pub async fn get_raindrops(
        &self,
        collection_id: i64,
        page: u32,
    ) -> Result<Vec<Raindrop>> {
        let url = format!("{}/raindrops/{}", BASE_URL, collection_id);
        let response = self
            .client
            .get(&url)
            .query(&[("page", page.to_string()), ("perpage", PAGE_SIZE.to_string())])
            .send()
            .await?;

        self.handle_response_status(&response)?;

        let data: RaindropsResponse = response
            .json()
            .await
            .context("Failed to parse raindrops response")?;
        if !data.result {
            bail!("API returned result=false for raindrops request");
        }
        Ok(data.items)
    }

    /// Get ALL raindrops from a collection (handles pagination)
    ///
    /// Optionally provide a progress callback that receives the count downloaded so far.
    /// Negative count indicates rate-limited pause.
    pub async fn get_all_raindrops<F>(
        &self,
        collection_id: i64,
        mut progress: Option<F>,
    ) -> Result<Vec<Raindrop>>
    where
        F: FnMut(i32),
    {
        let mut all_raindrops = Vec::new();
        let mut page = 0u32;

        loop {
            let raindrops = self.get_raindrops(collection_id, page).await?;

            if raindrops.is_empty() {
                break;
            }

            all_raindrops.extend(raindrops);

            if let Some(ref mut cb) = progress {
                cb(all_raindrops.len() as i32);
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
    pub async fn delete_raindrop(&self, id: i64) -> Result<()> {
        let url = format!("{}/raindrop/{}", BASE_URL, id);
        let response = self.client.delete(&url).send().await?;

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

    /// Get ALL raindrops from a collection (handles pagination)
    pub fn get_all_raindrops(&self, collection_id: i64) -> Result<Vec<Raindrop>> {
        self.runtime
            .block_on(self.api.get_all_raindrops::<fn(i32)>(collection_id, None))
    }

    /// Get ALL raindrops with progress callback
    pub fn get_all_raindrops_with_progress<F>(
        &self,
        collection_id: i64,
        progress: F,
    ) -> Result<Vec<Raindrop>>
    where
        F: FnMut(i32),
    {
        self.runtime
            .block_on(self.api.get_all_raindrops(collection_id, Some(progress)))
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
