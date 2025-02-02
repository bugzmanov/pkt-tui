use std::path::Path;
use std::{collections::HashMap, fs};

use log::error;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{json, Map, Value};
use std::fs::{File, OpenOptions};
use std::io::{prelude::*, BufReader};

#[derive(Serialize, Debug, Deserialize)]
pub struct Pocket {
    pub status: i64,
    pub complete: i64,
    pub list: Map<String, Value>,
}

impl Default for Pocket {
    fn default() -> Self {
        Pocket {
            status: 1,
            complete: 1,
            list: Map::new(),
        }
    }
}
impl Pocket {
    pub fn get_item(&self, item_id: &str) -> Option<PocketItem> {
        self.list
            .get(item_id)
            .map(|opt| serde_json::from_value(opt.clone()).unwrap())
    }

    pub fn pocket_items(self) -> HashMap<String, PocketItem> {
        let mut items: HashMap<String, PocketItem> = HashMap::new();
        for (key, value) in self.list {
            items.insert(key, serde_json::from_value(value).unwrap());
        }
        items
    }
}

fn default_favorite() -> String {
    "0".to_string()
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PocketItem {
    #[serde(rename = "item_id")]
    pub item_id: String,
    #[serde(default = "default_favorite")]
    pub favorite: String,
    pub status: String,
    #[serde(rename = "time_added")]
    pub time_added: String,
    #[serde(rename = "time_updated")]
    pub time_updated: String,
    #[serde(rename = "time_read")]
    pub time_read: String,
    #[serde(rename = "time_favorited")]
    pub time_favorited: String,
    #[serde(rename = "sort_id")]
    pub sort_id: i64,
    #[serde(rename = "resolved_title")]
    pub resolved_title: Option<String>,
    #[serde(rename = "given_title")]
    pub given_title: Option<String>,
    #[serde(rename = "resolved_url")]
    pub resolved_url: Option<String>,
    // pub excerpt: String,
    #[serde(rename = "is_article")]
    pub is_article: Option<String>,
    #[serde(default)]
    pub is_index: Option<String>,
    #[serde(rename = "has_video")]
    #[serde(default)]
    pub has_video: String,
    #[serde(rename = "has_image")]
    #[serde(default)]
    pub has_image: String,
    #[serde(rename = "word_count")]
    #[serde(default)]
    pub word_count: String,
    #[serde(default)]
    pub lang: String,
    // #[serde(rename = "top_image_url")]
    // pub top_image_url: String,
    #[serde(default)]
    pub tags: Map<String, Value>,

    #[serde(default)]
    #[serde(deserialize_with = "PocketItem::deserialize_authors")]
    pub authors: Option<Vec<String>>,
    // pub image: Image,
    // pub images: Images,
    // #[serde(rename = "domain_metadata")]
    // pub domain_metadata: DomainMetadata,
    #[serde(rename = "listen_duration_estimate")]
    pub listen_duration_estimate: i64,
}

impl PocketItem {
    /* json shape:
        "authors":{"189194339":{"author_id":"189194339","item_id":"4026299054","name":"BrnoJUG","url":"https://www.youtube.com/channel/UCTgGnw_UUCd1hvqJbiVdnvA"}}
    */
    fn deserialize_authors<'de, D>(deserializer: D) -> Result<Option<Vec<String>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let authors_map: Option<HashMap<String, serde_json::Value>> =
            Option::deserialize(deserializer)?;

        // If there are no authors, return None
        Ok(authors_map.map(|map| {
            map.values()
                .map(|v| {
                    let resource = match v.get("url") {
                        Some(x) if x.is_null() => "",
                        Some(x) if x.as_str().unwrap().contains("youtube") => "YT:",
                        Some(x) if x.as_str().unwrap().contains("medium") => "medium:",
                        Some(_) => "",
                        None => "",
                    };
                    let name = v.get("name").unwrap().as_str().unwrap();
                    format!("{}{}", resource, name)
                })
                .collect()
        }))
    }
}

pub enum PocketItemUpdate {
    Delete {
        item_id: String,
        timestamp: Option<u64>,
    },
    Add {
        item_id: String,
        data: PocketItem,
    },
}

const SNAPSHOT_FILE: &str = "snapshot.db";
static _DELTA_PREFIX: &'static str = "delta";

pub fn snapshot_exists() -> bool {
    Path::new(SNAPSHOT_FILE).exists()
}

pub fn save_to_snapshot(pocket: &Pocket) -> anyhow::Result<()> {
    let json = serde_json::to_string_pretty(&pocket)?;
    fs::write(SNAPSHOT_FILE, json)?;
    Ok(())
}

pub fn load_snapshot_file() -> Pocket {
    let data = fs::read_to_string(SNAPSHOT_FILE).expect("file should exist");
    let json: Pocket = serde_json::from_str(&data).expect("incorrect format");
    json
}

// pub fn delta_file() -> Path {
//     format!("{}/{}", DATA_DIRECTORY, DELTA_PREFIX).into()
// }
pub fn append_delete_to_delta(
    delta_file: &Path,
    pocket_update: &PocketItemUpdate,
) -> anyhow::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .append(true)
        .open(delta_file)?;

    let json = match pocket_update {
        PocketItemUpdate::Delete { item_id, timestamp } => {
            json!({
                "item_id": item_id,
                "status": "2",
                "timestamp": timestamp.unwrap_or(0),
            })
        }
        _ => return Err(anyhow::anyhow!("Only delete updates are supported")),
    };

    writeln!(&mut file, "{}", json.to_string())?;
    Ok(())
}

pub fn append_to_delta(delta_file: &Path, pocket: &Pocket) -> anyhow::Result<()> {
    let content: Vec<String> = pocket
        .list
        .values()
        .map(|v| serde_json::to_string(v).expect(&format!("can't convert to json {:?}", v)))
        .collect();

    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .append(true)
        .open(delta_file)
        .unwrap();

    for line in content {
        writeln!(&mut file, "{}", line)?;
    }
    Ok(())
}

pub fn load_delta_for_tests(delta_file: &Path) -> Map<String, Value> {
    match File::open(delta_file) {
        Ok(file) => {
            let buf = BufReader::new(file);

            buf.lines()
                .map(|l| {
                    let json_str = l.expect("couldn't parse line");
                    let value: Value = serde_json::from_str(&json_str)
                        .expect(&("couldn't parse json: ".to_owned() + &json_str));
                    let item_id = value
                        .get("item_id")
                        .map(|x| x.to_string())
                        .expect(&format!("invalid json shape: {:?}", value));
                    (item_id, value)
                })
                .collect()
        }
        Err(e) => {
            // todo: error needs to be propagated up
            error!("Error while opening file: {:?}", e);
            Map::new()
        }
    }
}

pub fn load_delta_pocket_items(delta_file: &Path) -> Vec<PocketItemUpdate> {
    match File::open(delta_file) {
        Ok(file) => {
            let buf = BufReader::new(file);

            buf.lines()
                .map(|l| {
                    let json_str = l.expect("couldn't parse line");
                    let js_value: Value = serde_json::from_str(&json_str)
                        .expect(&("couldn't parse json: ".to_owned() + &json_str));
                    if js_value["status"] != json!("2") {
                        let value: PocketItem = serde_json::from_value(js_value)
                            .expect(&("couldn't parse json: ".to_owned() + &json_str));
                        PocketItemUpdate::Add {
                            item_id: value.item_id.clone(),
                            data: value,
                        }
                    } else {
                        // deleted items
                        let item_id = js_value["item_id"].as_str().unwrap_or("-1");
                        let ts_opt = js_value["timestamp"].as_u64();
                        PocketItemUpdate::Delete {
                            item_id: item_id.to_string(),
                            timestamp: ts_opt,
                        }
                    }
                })
                .collect()
        }
        Err(e) => {
            //todo: propagte error back to the caller
            error!("Delta file wasn't found! {:?}", e);
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::*;
    use tempfile::NamedTempFile;

    #[test]
    fn test_basic_parsing() {
        let data = r#"
{
  "status": 1,
  "complete": 1,
  "list": {
    "3991838057": {
      "item_id": "3991838057",
      "resolved_id": "3991838057",
      "given_url": "https://www.phoronix.com/news/Linux-6.8-Networking",
      "given_title": "Linux 6.8 Network Optimizations Can Boost TCP Performance For Many Concurre",
      "favorite": "0",
      "status": "0",
      "time_added": "1709806547",
      "time_updated": "1709806555",
      "time_read": "0",
      "time_favorited": "0",
      "sort_id": 0,
      "resolved_title": "Linux 6.8 Network Optimizations Can Boost TCP Performance For Many Concurrent Connections By ~40%",
      "resolved_url": "https://www.phoronix.com/news/Linux-6.8-Networking",
      "excerpt": "Beyond the usual new wired/wireless network hardware support and the other routine churn in the big Linux networking subsystem, the Linux 6.",
      "is_article": "1",
      "is_index": "0",
      "has_video": "0",
      "has_image": "1",
      "word_count": "390",
      "lang": "en",
      "top_image_url": "https://www.phoronix.net/image.php?id=2024&image=epyc_network",
      "tags": {
        "lowlevel": {
          "item_id": "3991838057",
          "tag": "lowlevel"
        },
        "network": {
          "item_id": "3991838057",
          "tag": "network"
        },
        "performance": {
          "item_id": "3991838057",
          "tag": "performance"
        }
      },
      "authors": {
        "159666303": {
          "item_id": "3991838057",
          "author_id": "159666303",
          "name": "Michael Larabel",
          "url": "https://www.michaellarabel.com/"
        }
      },
      "image": {
        "item_id": "3991838057",
        "src": "https://www.phoronix.com/assets/categories/linuxnetworking.webp",
        "width": "100",
        "height": "100"
      },
      "images": {
        "1": {
          "item_id": "3991838057",
          "image_id": "1",
          "src": "https://www.phoronix.com/assets/categories/linuxnetworking.webp",
          "width": "100",
          "height": "100",
          "credit": "",
          "caption": ""
        }
      },
      "domain_metadata": {
        "name": "Phoronix",
        "logo": "https://logo.clearbit.com/phoronix.com?size=800",
        "greyscale_logo": "https://logo.clearbit.com/phoronix.com?size=800&greyscale=true"
      },
      "listen_duration_estimate": 151
    }
  }
}
    "#;

        let parsed: Pocket = serde_json::from_str(data).unwrap();
        assert_eq!(parsed.list.len(), 1);

        assert_eq!(
            parsed
                .pocket_items()
                .values()
                .into_iter()
                .next()
                .unwrap()
                .resolved_url
                .as_ref()
                .unwrap(),
            "https://www.phoronix.com/news/Linux-6.8-Networking"
        );
    }

    #[test]
    fn test2() -> Result<()> {
        let data = r#"
{
  "status": 1,
  "complete": 1,
  "list": {
    "123": {
      "item_id": "123",
      "given_url": "https://www.phoronix.com/news/Linux-6.8-Networking"
      }
    }
}
    "#;

        let mut file = NamedTempFile::new().unwrap();
        let path = file.as_ref();
        let pocket: Pocket = serde_json::from_str(data).unwrap();

        append_to_delta(path, &pocket).unwrap();

        let data2 = r#"
{
  "status": 1,
  "complete": 1,
  "list": {
    "456": {
      "item_id": "456",
      "given_url": "http://localhost"
      }
    }
}
    "#;

        append_to_delta(path, &serde_json::from_str(data2).unwrap());

        let map = load_delta_for_tests(path);
        assert_eq!(map.len(), 2);
        Ok(())
    }
}
