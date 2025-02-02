#![allow(dead_code)]
use std::path::Path;

use crate::storage::{self, Pocket};
use anyhow::{bail, format_err, Context, Result};
use log::error;
use reqwest::Body;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::json;
use thiserror::Error;
use tokio::runtime::Runtime;

const SEND_ENDPOINT: &str = "https://getpocket.com/v3/send";
const GET_ENDPOINT: &str = "https://getpocket.com/v3/get";

pub static CONSUMER_KEY: &'static str = "110856-cba018037b073c92d23edc4";

/* const RATE_LIMIT_HEADERS: [(&str, &str); 6] = [
    ("X-Limit-User-Limit", "Current rate limit enforced per user"),
    (
        "X-Limit-User-Remaining",
        "Number of calls remaining before hitting user's rate limit",
    ),
    (
        "X-Limit-User-Reset",
        "Seconds until user's rate limit resets",
    ),
    (
        "X-Limit-Key-Limit",
        "Current rate limit enforced per consumer key",
    ),
    (
        "X-Limit-Key-Remaining",
        "Number of calls remaining before hitting consumer key's rate limit",
    ),
    (
        "X-Limit-Key-Reset:",
        "Seconds until consumer key rate limit resets",
    ),
];*/

#[derive(Debug, Error)]
pub enum ClientError<'a> {
    #[error("{0}")]
    JsonError(serde_json::Error),
    #[error("There was an issue with the parameters. `{0}`")]
    InvalidParams(&'a str),
    #[error("Access to this resource is restricted")]
    AccessDenied,
    #[error("Token authentication failed. Please check your token and try again.")]
    TokenError,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Response {
    pub action_results: Vec<bool>,
    pub action_errors: Vec<Option<String>>,
    pub status: i32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ActionError {
    pub message: String,
    #[serde(rename = "type")]
    pub error_type: String,
    pub code: i32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ExtendedResponse {
    pub action_results: serde_json::Value,
    pub action_errors: Vec<Option<ActionError>>,
    pub status: i32,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum SendResponse {
    Standart(Response),
    Extended(ExtendedResponse),
}

#[derive(Debug, Clone)]
pub struct Reqwester {
    pub client: reqwest::Client,
}

#[derive(Debug, Clone)]
pub struct GetPocket {
    pub consumer_key: String,
    pub access_token: String,
    pub reqwester: Reqwester,
}

impl GetPocket {
    pub fn new_hardcode(acces_token: &str) -> Self {
        GetPocket::new(CONSUMER_KEY.to_string(), acces_token.to_string())
    }

    pub fn new(consumer_key: String, access_token: String) -> Self {
        let reqwester = Self::init_reqwester();

        Self {
            consumer_key,
            access_token,
            reqwester,
        }
    }

    async fn send<T>(&self, params: T) -> Result<SendResponse>
    where
        T: Serialize,
    {
        #[derive(Serialize)]
        struct RequestParams<'a, T> {
            consumer_key: &'a str,
            access_token: &'a str,
            actions: T,
        }

        impl<'a, T> RequestParams<'a, T>
        where
            T: Serialize,
        {
            fn into_body(self) -> Result<Body, serde_json::Error> {
                let json = serde_json::to_string(&self)?;
                Ok(Body::from(json))
            }
        }

        let req_param = RequestParams {
            consumer_key: &self.consumer_key,
            access_token: &self.access_token,
            actions: params,
        };

        let params = format!("{SEND_ENDPOINT}");

        let client = &self.reqwester.client;
        // let res = client.post(&params).send().await?;
        let res = client
            .post(&params)
            .body(req_param.into_body()?)
            .send()
            .await?;

        if let Err(err) = ApiRequestError::handler_status(res.status()) {
            log::error!("Http communication error: {}", res.text().await?);
            bail!(err);
        }

        let res_body = &res.text().await?;
        log::info!("GetPocket API communication response: {}", &res_body);

        let res_ser: Result<SendResponse, serde_json::Error> = serde_json::from_str(&res_body);

        match res_ser {
            Ok(SendResponse::Extended(extended_res)) => {
                if !extended_res.action_errors.iter().all(|e| e.is_none()) {
                    bail!(format_err!(
                        "Action errors: {:?}",
                        extended_res.action_errors
                    ));
                }
                Ok(SendResponse::Extended(extended_res))
            }
            Ok(other_res) => Ok(other_res),
            Err(err) => Err(ClientError::JsonError(err).into()),
        }
    }

    fn init_reqwester() -> Reqwester {
        use reqwest::header;

        let mut headers = header::HeaderMap::new();
        headers.insert(
            "Content-Type",
            header::HeaderValue::from_static("application/json; charset=UTF-8"),
        );
        headers.insert(
            "X-Accept",
            header::HeaderValue::from_static("application/json"),
        );

        let client = reqwest::Client::builder()
            .connection_verbose(true)
            .default_headers(headers)
            .build()
            .unwrap();

        Reqwester { client }
    }

    //note: "since" kinda sort works .
    // so the working scenarios so far:
    //    - sort:oldest, since: 0, offset - to paginate up to the most recetn
    //    - sort: newest, since - recent timestamps. need to figure out how to use offset in this scenario
    // for instance: offset=0, sort=oldest, since = 1738402326 will just return the oldest record :shrug:
    // using since deep in the past doesn't work.
    // it looks like they are using some serious bucketing under neath. the older you go the more crude the bucket size
    pub async fn retrieve(
        &self,
        since: Option<&str>,
        offset: Option<u32>,
        oldest_to_newest: bool,
    ) -> Result<Pocket> {
        let client = &self.reqwester.client;
        let mut params = json!({
            "consumer_key": self.consumer_key,
            "access_token": self.access_token,
            "detailType":"complete",
            "sort": (if oldest_to_newest { "oldest" } else {"newest"}),
            "state": "all",
            "count": 100, //api claims that this will be capped at 30 eventually
        });
        if let Some(timestamp) = since {
            params["since"] = json!(timestamp);
        }
        if let Some(page_offset) = offset {
            params["offset"] = json!(page_offset);
        }
        let res = client.post(GET_ENDPOINT).json(&params).send().await?;

        if let Err(err) = ApiRequestError::handler_status(res.status()) {
            bail!(err);
        }

        let res_body = &res.text().await?;

        let res_ser: Pocket = serde_json::from_str(&res_body).map_err(|e| format_err!(e))?;

        Ok(res_ser)
    }

    pub async fn delete(&self, item_id: usize) -> Result<SendResponse> {
        let now = chrono::Utc::now().timestamp();
        self.send(json!([{
            "item_id": item_id.to_string(),
            "timestamp": now.to_string(),
            "action": "delete"
        }]))
        .await
    }

    pub async fn fav_and_archive(&self, item_id: usize) -> Result<SendResponse> {
        self.send(json!([{
            "item_id": item_id.to_string(),
            "action": "favorite"
        },
        {
            "item_id": item_id.to_string(),
            "action": "archive"
        }
        ]))
        .await
    }

    pub async fn add_tag(&self, item_id: usize, tag: &str) -> Result<SendResponse> {
        self.send(json!([{
            "item_id": item_id.to_string(),
            "tags": tag,
            "action": "tags_add"
        }]))
        .await
    }

    pub async fn remove_tag(&self, item_id: usize, tag: &str) -> Result<SendResponse> {
        self.send(json!([{
            "item_id": item_id.to_string(),
            "tags": tag,
            "action": "tags_remove"
        }]))
        .await
    }

    pub async fn rename(
        &self,
        item_id: usize,
        url: &str,
        title: &str,
        timestamp: u64,
    ) -> Result<SendResponse> {
        self.send(json!([{
            "item_id": item_id.to_string(),
            "title": title,
            "url": url,
            "action": "add",
            "time": timestamp
        }]))
        .await
    }
}

pub struct GetPocketSync {
    get_pocket: GetPocket,
    runtime: Runtime,
}

impl GetPocketSync {
    pub fn new(access_token: &str) -> Result<Self> {
        let client = GetPocket::new_hardcode(access_token);
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        Ok(GetPocketSync {
            get_pocket: client,
            runtime: rt,
        })
    }

    pub fn delete(&self, item_id: usize) -> Result<SendResponse> {
        self.runtime
            .block_on(self.get_pocket.delete(item_id))
            .context(format!("Faile to delet an Item {}", item_id))
    }

    pub fn mark_as_read(&self, item_id: usize) -> Result<SendResponse> {
        self.runtime
            .block_on(self.get_pocket.add_tag(item_id, "read"))
            .context(format!("Faile to mark as read Item {}", item_id))
    }

    pub fn mark_as_top(&self, item_id: usize) -> Result<SendResponse> {
        self.runtime
            .block_on(self.get_pocket.add_tag(item_id, "top"))
            .context(format!("Faile to mark as read Item {}", item_id))
    }

    pub fn unmark_as_top(&self, item_id: usize) -> Result<SendResponse> {
        self.runtime
            .block_on(self.get_pocket.remove_tag(item_id, "top"))
            .context(format!("Faile to mark as read Item {}", item_id))
    }

    pub fn fav_and_archive(&self, item_id: usize) -> Result<SendResponse> {
        self.runtime
            .block_on(self.get_pocket.fav_and_archive(item_id))
            .context(format!("Faile to fav_and_archive an Item {}", item_id))
    }

    //todo: this might blow up if pocket list size is very long
    //todo: this does fetching & priting a the same time
    pub fn retrieve_all(&self) -> Result<Pocket> {
        self.runtime.block_on(async {
            let mut offset = 0;
            let mut all_items = Pocket::default();
            let loading_chars = ["|", "/", "-", "\\"];
            let mut loading_idx = 0;
            let dots = std::sync::Arc::new(std::sync::Mutex::new(String::new()));

            // print!("\nFetch in progress: ");

            let (tx, rx) = std::sync::mpsc::channel();
            let dots_clone = dots.clone();
            std::thread::spawn(move || {
                while rx.try_recv().is_err() {
                    let dots_str = dots_clone.lock().unwrap().clone();
                    print!(
                        "\rFetch in progress: {}{} ",
                        dots_str, loading_chars[loading_idx]
                    );
                    loading_idx = (loading_idx + 1) % loading_chars.len();
                    std::io::Write::flush(&mut std::io::stdout()).unwrap();
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            });

            loop {
                let batch = self
                    .get_pocket
                    .retrieve(Some("0"), Some(offset), true)
                    .await?; //todo: don't know how long Some(0) for offset will be working
                if batch.list.is_empty() {
                    break;
                }

                let list_size = batch.list.len() as u32;
                // Merge the items
                all_items.list.extend(batch.list);

                offset += list_size;
                dots.lock().unwrap().push('.');
            }

            let _ = tx.send(());

            all_items.list.retain(|_id, item| {
                item.get("status")
                    .map_or(true, |s| s.as_str().unwrap_or("") != "2")
            });
            Ok(all_items)
        })
    }

    pub fn refresh_delta_block(&self, delta_file: &Path) -> Result<()> {
        self.runtime
            .block_on(refresh_delta(delta_file, &self.get_pocket))
            .context("Failed to refresh pocket delta")
    }

    pub fn rename(
        &self,
        item_id: usize,
        url: &str,
        title: &str,
        timestamp: u64,
    ) -> Result<SendResponse> {
        self.runtime
            .block_on(self.get_pocket.rename(item_id, url, title, timestamp))
            .context("Failed to rename pocket item")
    }
}

#[derive(Error, Debug)]
#[error("Request has encountered an error. {0} - {1} ")]
pub struct ApiRequestError<'a>(u32, &'a str);

impl ApiRequestError<'_> {
    pub fn handler_status(status_code: StatusCode) -> Result<()> {
        match status_code {
            StatusCode::BAD_REQUEST => bail!(ApiRequestError(400, "Invalid request, please make sure you follow the documentation for proper syntax.")),
            StatusCode::UNAUTHORIZED => bail!(ApiRequestError(401, "Problem authenticating the user.")),
            StatusCode::FORBIDDEN => bail!(ApiRequestError(403, "User was authenticated, but access denied due to lack of permission or rate limiting.")),
            StatusCode::INTERNAL_SERVER_ERROR => bail!(ApiRequestError(500, "Internal Server Error")),
            StatusCode::SERVICE_UNAVAILABLE => bail!(ApiRequestError(502, "Pocket's sync server is down for scheduled maintenance.")),
            _ => Ok(()),
        }
    }
}

//todo: duplicates last record if no updates found
pub async fn refresh_delta(delta_file: &Path, pocket: &GetPocket) -> Result<()> {
    let current = storage::load_delta_pocket_items(delta_file);
    if let Some(max_ts) = current
        .iter()
        .map(|item| match item {
            storage::PocketItemUpdate::Delete {
                item_id: _,
                timestamp: _,
            } => 0,
            storage::PocketItemUpdate::Add {
                item_id: _,
                data: x,
            } => x.time_added.parse::<usize>().unwrap_or(0),
        })
        .max()
    {
        let update = pocket
            .retrieve(Some(&max_ts.to_string()), None, false)
            .await?; //todo: what if we can not fetch everything
        storage::append_to_delta(delta_file, &update)?;
        Ok(())
    } else {
        todo!("why-delta-is-unavailable???");
    }
}

pub fn refresh_delta_block(delta_file: &Path, pocket: &GetPocket) -> Result<()> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(refresh_delta(delta_file, pocket))
        .context("Failed to refresh pocket delta")
}

//todo move to integration tests
#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::{pocket::CONSUMER_KEY, *};

    static ACCESS_TOKEN: &'static str = "ololoev";

    use super::GetPocket;

    #[tokio::test]
    async fn basic_pocket_tests() -> anyhow::Result<()> {
        let get_pocket = GetPocket::new(CONSUMER_KEY.to_string(), ACCESS_TOKEN.to_string());
        let result = get_pocket.retrieve(Some("1709824779000")).await?;
        // assert_eq!(format!("{:?}", result), "sss".to_string());
        Ok(())
    }

    #[tokio::test]
    async fn pocket_delete_test() -> anyhow::Result<()> {
        env_logger::init();
        let get_pocket = GetPocket::new(CONSUMER_KEY.to_string(), ACCESS_TOKEN.to_string());
        let result = get_pocket.delete(2456660519).await?;
        assert_eq!(format!("{:?}", result), "sss".to_string());
        Ok(())
    }

    #[tokio::test]
    async fn fetch_delta() -> anyhow::Result<()> {
        let get_pocket = GetPocket::new(CONSUMER_KEY.to_string(), ACCESS_TOKEN.to_string());
        let result = get_pocket.retrieve(Some("1709824779000")).await?;
        let path = Path::new("temp"); //file.as_ref();
        storage::append_to_delta(path, &result)?;
        Ok(())
    }
}
