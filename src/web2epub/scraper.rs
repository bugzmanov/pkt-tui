//! HTML fetching and article extraction using dom_smoothie.

use headless_chrome::{Browser, LaunchOptions};
use regex::Regex;
use reqwest::blocking::Client;
use scraper::{Html, Selector};
use serde_json::Value as JsonValue;
use std::time::Duration;
use thiserror::Error;
use url::Url;

#[derive(Debug, Error)]
pub enum ScraperError {
    #[error("HTTP request failed: {0}")]
    HttpError(#[from] reqwest::Error),

    #[error("Failed to extract readable content from the page")]
    ExtractionFailed,

    #[error("Invalid URL: {0}")]
    InvalidUrl(String),

    #[error("DOM parsing error: {0}")]
    DomError(String),

    #[error("Browser error: {0}")]
    BrowserError(String),
}

/// Extracted article content.
#[derive(Debug, Clone)]
pub struct Article {
    pub title: String,
    pub author: String,
    pub content_html: String,
}

/// Fetches HTML content from a URL.
pub fn fetch_html(client: &Client, url: &str) -> Result<String, ScraperError> {
    let response = client
        .get(url)
        .header("User-Agent", "pkt-tui-web2epub/0.1")
        .header("Accept", "text/html,application/xhtml+xml")
        .send()?;

    if !response.status().is_success() {
        return Err(ScraperError::HttpError(
            response.error_for_status().unwrap_err(),
        ));
    }

    Ok(response.text()?)
}

/// Detects if HTML is likely a JavaScript SPA that requires browser rendering.
fn is_spa_html(html: &str) -> bool {
    let lower = html.to_lowercase();

    // Check for common SPA indicators
    let has_noscript_js_required = lower.contains("you need to enable javascript")
        || lower.contains("please enable javascript")
        || lower.contains("javascript is required");

    // Check for empty body with just a root div (React/Vue/Angular pattern)
    let has_app_root_only = (lower.contains("<div id=\"root\">")
        || lower.contains("<div id=\"app\">")
        || lower.contains("<div id=\"__next\">")
        || lower.contains("id=\"ramp-builders\""))
        && !lower.contains("<article")
        && !lower.contains("<main");

    // Check body content - SPAs typically have very little content in body
    if let Some(body_start) = lower.find("<body") {
        if let Some(body_end) = lower.find("</body>") {
            let body_content = &lower[body_start..body_end];
            // If body has less than 500 chars of actual content (excluding scripts), likely SPA
            let content_without_scripts: String = body_content
                .split("<script")
                .next()
                .unwrap_or("")
                .chars()
                .filter(|c| c.is_alphanumeric())
                .collect();
            if content_without_scripts.len() < 200 && has_app_root_only {
                return true;
            }
        }
    }

    has_noscript_js_required || has_app_root_only
}

/// Fetches HTML content from a URL using a headless browser.
/// This is used as a fallback for JavaScript SPAs that require rendering.
pub fn fetch_html_via_browser(url: &str) -> Result<String, ScraperError> {
    log::info!("Attempting to fetch page via browser (SPA fallback): {}", url);

    let launch_options = LaunchOptions::default_builder()
        .headless(true)
        .sandbox(true)
        .build()
        .map_err(|e| ScraperError::BrowserError(format!("Failed to build launch options: {}", e)))?;

    let browser = Browser::new(launch_options)
        .map_err(|e| ScraperError::BrowserError(format!("Failed to launch browser: {}", e)))?;

    let tab = browser
        .new_tab()
        .map_err(|e| ScraperError::BrowserError(format!("Failed to create tab: {}", e)))?;

    // Navigate to the URL
    tab.navigate_to(url)
        .map_err(|e| ScraperError::BrowserError(format!("Failed to navigate: {}", e)))?;

    // Wait for page to load
    tab.wait_until_navigated()
        .map_err(|e| ScraperError::BrowserError(format!("Navigation timeout: {}", e)))?;

    // Wait a bit for JavaScript to render content
    std::thread::sleep(Duration::from_secs(2));

    // Check for Cloudflare challenge and wait if needed
    for _ in 0..10 {
        if let Ok(content) = tab.get_content() {
            let lower = content.to_lowercase();
            if lower.contains("just a moment") || lower.contains("checking your browser") {
                log::debug!("Cloudflare challenge detected, waiting...");
                std::thread::sleep(Duration::from_secs(2));
            } else {
                break;
            }
        }
    }

    // Additional wait for SPA content to render
    std::thread::sleep(Duration::from_secs(1));

    // Get the rendered HTML
    let html = tab
        .get_content()
        .map_err(|e| ScraperError::BrowserError(format!("Failed to get page content: {}", e)))?;

    log::info!("Successfully fetched page via browser ({} bytes)", html.len());
    Ok(html)
}

/// Attempts to fetch raw content from a URL that points to a hosted file (e.g. Markdown on GitHub).
/// Returns `Some(Article)` if the URL is recognized and raw content can be fetched and rendered,
/// or `None` if this URL doesn't have a known raw content source.
fn try_fetch_raw_content(client: &Client, url: &str) -> Option<Article> {
    let (raw_url, file_ext) = get_raw_content_url(url)?;

    log::info!("Fetching raw content from: {}", raw_url);
    let response = client
        .get(&raw_url)
        .header("User-Agent", "pkt-tui-web2epub/0.1")
        .header("Accept", "text/plain, text/markdown, */*")
        .send()
        .ok()?;

    if !response.status().is_success() {
        log::warn!("Raw content fetch failed with status: {}", response.status());
        return None;
    }

    let raw_text = response.text().ok()?;
    if raw_text.trim().is_empty() {
        return None;
    }

    // Extract a title from the file path
    let title = extract_title_from_url(url).unwrap_or_else(|| "Untitled".to_string());
    let author = extract_repo_owner(url).unwrap_or_else(|| extract_hostname(url));

    let content_html = match file_ext.as_str() {
        "md" | "markdown" | "mdown" | "mkd" => render_markdown_to_html(&raw_text),
        "rst" | "txt" => {
            // Plain text / reStructuredText: wrap in <pre> for now
            format!("<pre>{}</pre>", html_escape_basic(&raw_text))
        }
        "org" => {
            format!("<pre>{}</pre>", html_escape_basic(&raw_text))
        }
        _ => return None,
    };

    // Try to extract a better title from the first heading in the rendered content
    let title = extract_first_heading(&content_html).unwrap_or(title);

    log::info!("Successfully rendered raw {} content: '{}'", file_ext, title);
    Some(Article {
        title,
        author,
        content_html,
    })
}

/// Maps a URL to its raw content equivalent and file extension.
/// Returns `(raw_url, file_extension)` if recognized.
fn get_raw_content_url(url: &str) -> Option<(String, String)> {
    let parsed = Url::parse(url).ok()?;
    let host = parsed.host_str()?;
    let path = parsed.path();

    // GitHub: github.com/{owner}/{repo}/blob/{ref}/{path} → raw.githubusercontent.com/{owner}/{repo}/{ref}/{path}
    if host == "github.com" || host == "www.github.com" {
        if let Some(raw_path) = path.strip_prefix('/') {
            let parts: Vec<&str> = raw_path.splitn(5, '/').collect();
            // parts: [owner, repo, "blob", ref, file_path]
            if parts.len() == 5 && parts[2] == "blob" {
                let ext = get_file_extension(parts[4]);
                if is_renderable_extension(&ext) {
                    let raw_url = format!(
                        "https://raw.githubusercontent.com/{}/{}/{}/{}",
                        parts[0], parts[1], parts[3], parts[4]
                    );
                    return Some((raw_url, ext));
                }
            }
        }
    }

    // GitLab: gitlab.com/{owner}/{repo}/-/blob/{ref}/{path} → gitlab.com/{owner}/{repo}/-/raw/{ref}/{path}
    if host.contains("gitlab") {
        if let Some(blob_pos) = path.find("/-/blob/") {
            let prefix = &path[..blob_pos];
            let rest = &path[blob_pos + "/-/blob/".len()..];
            let ext = get_file_extension(rest);
            if is_renderable_extension(&ext) {
                let raw_url = format!("https://{}{}/-/raw/{}", host, prefix, rest);
                return Some((raw_url, ext));
            }
        }
    }

    // Bitbucket: bitbucket.org/{owner}/{repo}/src/{ref}/{path} → bitbucket.org/{owner}/{repo}/raw/{ref}/{path}
    if host == "bitbucket.org" || host == "www.bitbucket.org" {
        if let Some(src_pos) = path.find("/src/") {
            let prefix = &path[..src_pos];
            let rest = &path[src_pos + "/src/".len()..];
            let ext = get_file_extension(rest);
            if is_renderable_extension(&ext) {
                let raw_url = format!("https://{}{}//raw/{}", host, prefix, rest);
                return Some((raw_url, ext));
            }
        }
    }

    None
}

/// Checks if a file extension is one we can render to HTML.
fn is_renderable_extension(ext: &str) -> bool {
    matches!(ext, "md" | "markdown" | "mdown" | "mkd" | "rst" | "txt" | "org")
}

/// Extracts file extension from a path.
fn get_file_extension(path: &str) -> String {
    // Strip query/fragment
    let clean = path.split('?').next().unwrap_or(path);
    let clean = clean.split('#').next().unwrap_or(clean);
    clean
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_lowercase()
}

/// Renders Markdown text to HTML using pulldown-cmark.
fn render_markdown_to_html(markdown: &str) -> String {
    use pulldown_cmark::{html, Options, Parser};

    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_HEADING_ATTRIBUTES);

    let parser = Parser::new_ext(markdown, options);
    let mut html_output = String::new();
    html::push_html(&mut html_output, parser);
    html_output
}

/// Extracts a human-readable title from a URL path (last path component without extension).
fn extract_title_from_url(url: &str) -> Option<String> {
    let parsed = Url::parse(url).ok()?;
    let path = parsed.path();
    let filename = path.rsplit('/').next()?;
    let name = filename.split('.').next()?;
    if name.is_empty() {
        return None;
    }
    // Convert kebab-case/snake_case to title case
    Some(
        name.replace('-', " ")
            .replace('_', " ")
            .split_whitespace()
            .map(|w| {
                let mut c = w.chars();
                match c.next() {
                    None => String::new(),
                    Some(f) => f.to_uppercase().to_string() + c.as_str(),
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
    )
}

/// Extracts the repository owner/org from a GitHub/GitLab/Bitbucket URL.
fn extract_repo_owner(url: &str) -> Option<String> {
    let parsed = Url::parse(url).ok()?;
    let path = parsed.path();
    let parts: Vec<&str> = path.trim_start_matches('/').splitn(3, '/').collect();
    if parts.len() >= 2 {
        Some(format!("{}/{}", parts[0], parts[1]))
    } else {
        None
    }
}

/// Extracts the text of the first <h1> or <h2> from HTML content.
fn extract_first_heading(html: &str) -> Option<String> {
    let re = Regex::new(r"(?s)<h[12][^>]*>(.*?)</h[12]>").ok()?;
    let caps = re.captures(html)?;
    let inner = &caps[1];
    let text = strip_tags(inner).trim().to_string();
    if text.is_empty() { None } else { Some(text) }
}

/// Extracts the hostname from a URL for use as a fallback author.
fn extract_hostname(url: &str) -> String {
    Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(String::from))
        .unwrap_or_else(|| "Unknown".to_string())
}

/// Check if URL is from LessWrong or related sites that use Apollo SSR.
fn is_lesswrong_url(url: &str) -> bool {
    url.contains("lesswrong.com") || url.contains("alignmentforum.org")
}

/// Check if URL requires browser rendering (heavy JS sites that can't be scraped statically).
fn requires_browser_rendering(url: &str) -> bool {
    url.contains("x.com") || url.contains("twitter.com")
}

/// Check if URL is from X/Twitter.
fn is_twitter_url(url: &str) -> bool {
    url.contains("x.com") || url.contains("twitter.com")
}

/// Extract article content from X/Twitter's rendered DOM.
/// Handles both Twitter Articles (longform) and regular tweets.
fn extract_twitter_article(html: &str) -> Option<Article> {
    let document = Html::parse_document(html);

    // Extract author from User-Name
    let username_sel = Selector::parse("[data-testid=\"User-Name\"]").ok()?;
    let username_el = document.select(&username_sel).next()?;
    let author = username_el
        .text()
        .next()
        .unwrap_or("Unknown")
        .trim()
        .to_string();

    // Try Twitter Article (longform) first
    let article_title_sel = Selector::parse("[data-testid=\"twitter-article-title\"]").ok()?;
    let article_content_sel = Selector::parse("[data-testid=\"twitterArticleRichTextView\"]").ok()?;

    if let Some(content_el) = document.select(&article_content_sel).next() {
        // This is a Twitter Article
        let title = document
            .select(&article_title_sel)
            .next()
            .map(|el| el.text().collect::<String>().trim().to_string())
            .unwrap_or_else(|| "Untitled".to_string());

        let content_html = convert_twitter_article_html(content_el);

        if !content_html.is_empty() {
            log::info!("Extracted Twitter Article: {}", title);
            return Some(Article {
                title,
                author,
                content_html,
            });
        }
    }

    // Fall back to regular tweet extraction
    let tweet_text_sel = Selector::parse("[data-testid=\"tweetText\"]").ok()?;
    if let Some(tweet_el) = document.select(&tweet_text_sel).next() {
        let text_content = collect_text_with_newlines(&tweet_el);
        let content_html = if is_twitter_code_fragment(&text_content) {
            format!("<pre><code>{}</code></pre>", html_escape_basic(text_content.trim()))
        } else if let Some(list_html) = try_format_numbered_twitter_list(&text_content) {
            list_html
        } else {
            format!("<p>{}</p>", tweet_el.inner_html())
        };

        // Also try to get card links
        let card_sel = Selector::parse("[data-testid=\"card.wrapper\"] a[href]").ok();
        let mut links_html = String::new();
        if let Some(sel) = &card_sel {
            for card_link in document.select(sel) {
                if let Some(href) = card_link.value().attr("href") {
                    let link_text: String = card_link.text().collect();
                    let display = if link_text.trim().is_empty() {
                        href.to_string()
                    } else {
                        link_text.trim().to_string()
                    };
                    links_html.push_str(&format!("<p><a href=\"{}\">{}</a></p>\n", href, display));
                }
            }
        }

        let full_content = if links_html.is_empty() {
            content_html
        } else {
            format!("{}\n{}", content_html, links_html)
        };

        let title_source = text_content.split_whitespace().collect::<Vec<_>>().join(" ");
        let title = title_source.chars().take(80).collect::<String>();
        let title = if title.len() < title_source.len() {
            format!("{}...", title.trim())
        } else {
            title.trim().to_string()
        };

        log::info!("Extracted tweet from {}", author);
        return Some(Article {
            title,
            author,
            content_html: full_content,
        });
    }

    None
}

/// Collect text from an element while preserving visual line breaks represented by `<br>`.
fn collect_text_with_newlines(node: &scraper::ElementRef<'_>) -> String {
    fn walk(node: &scraper::ElementRef<'_>, out: &mut String) {
        for child in node.children() {
            match child.value() {
                scraper::node::Node::Text(text) => out.push_str(text.text.as_ref()),
                scraper::node::Node::Element(el) => {
                    if el.name() == "br" {
                        out.push('\n');
                        continue;
                    }
                    if let Some(child_ref) = scraper::ElementRef::wrap(child) {
                        walk(&child_ref, out);
                    }
                }
                _ => {}
            }
        }
    }

    let mut out = String::new();
    walk(node, &mut out);
    out
}

/// Heuristic for monospaced "code fragment" tweets (ASCII/box-drawing diagrams, snippets).
fn is_twitter_code_fragment(text: &str) -> bool {
    let lines: Vec<&str> = text
        .lines()
        .map(|l| l.trim_end())
        .filter(|l| !l.trim().is_empty())
        .collect();

    if lines.len() < 3 {
        return false;
    }

    let has_box_drawing = text
        .chars()
        .any(|c| matches!(c as u32, 0x2500..=0x257F | 0x2580..=0x259F));

    let pipe_lines = lines.iter().filter(|l| l.contains('|')).count();
    let dash_lines = lines
        .iter()
        .filter(|l| l.contains("---") || l.contains("___") || l.contains("==="))
        .count();

    has_box_drawing || (pipe_lines >= 2 && dash_lines >= 1)
}

/// Heuristic for a single Twitter longform block that is likely part of an
/// ASCII/box-drawing code fragment.
fn looks_like_twitter_code_line(text: &str) -> bool {
    let line = text.trim();
    if line.is_empty() {
        return false;
    }

    let has_box_drawing = line
        .chars()
        .any(|c| matches!(c as u32, 0x2500..=0x257F | 0x2580..=0x259F));
    if has_box_drawing {
        return true;
    }

    let pipe_count = line.matches('|').count();
    let has_rule = line.contains("---") || line.contains("___") || line.contains("===");
    pipe_count >= 2 || has_rule
}

/// Convert long numbered tweet text into readable HTML list/paragraphs.
/// This handles X posts that flatten "1. ... 2. ... 3. ..." into one dense block.
fn try_format_numbered_twitter_list(text: &str) -> Option<String> {
    let re = Regex::new(r"\b(\d{1,3})\.\s").ok()?;
    let matches: Vec<_> = re.captures_iter(text).collect();
    if matches.len() < 4 {
        return None;
    }

    // Require list to start at 1 to reduce false positives from versions/dates.
    let first_num = matches
        .first()
        .and_then(|m| m.get(1))
        .and_then(|m| m.as_str().parse::<u32>().ok())
        .unwrap_or(0);
    if first_num != 1 {
        return None;
    }

    let first_start = matches.first()?.get(0)?.start();
    let intro = text[..first_start].trim();

    let mut items = Vec::new();
    for (idx, m) in matches.iter().enumerate() {
        let start = m.get(0)?.start();
        let end = matches
            .get(idx + 1)
            .and_then(|next| next.get(0))
            .map(|next| next.start())
            .unwrap_or(text.len());
        let segment = text[start..end].trim();
        let item = re.replace(segment, "").trim().to_string();
        if !item.is_empty() {
            items.push(item);
        }
    }

    if items.len() < 4 {
        return None;
    }

    let mut html = String::new();
    if !intro.is_empty() {
        html.push_str(&format!("<p>{}</p>\n", render_twitter_plain_text(intro)));
    }
    html.push_str("<ol>");
    for item in items {
        html.push_str(&format!("<li>{}</li>", render_twitter_plain_text(&item)));
    }
    html.push_str("</ol>");
    Some(html)
}

/// Render plain text to safe HTML while keeping lightweight visual separators.
fn render_twitter_plain_text(text: &str) -> String {
    let normalized = text.replace(" > ", "\n");
    html_escape_basic(&normalized).replace('\n', "<br/>")
}

/// Converts a Twitter Article's DraftJS DOM content into clean HTML.
fn convert_twitter_article_html(content_el: scraper::ElementRef) -> String {
    let mut html_parts: Vec<String> = Vec::new();
    let mut pending_code_lines: Vec<String> = Vec::new();

    // Process each block in data-contents
    let block_sel = Selector::parse("[data-block=\"true\"]").unwrap();
    let img_sel = Selector::parse("img[src]").unwrap();
    let link_sel = Selector::parse("a[href]").unwrap();

    for block in content_el.select(&block_sel) {
        let classes: Vec<&str> = block.value().classes().collect();
        let class_str = classes.join(" ");
        let plain_block_text = collect_text_with_newlines(&block);

        if looks_like_twitter_code_line(&plain_block_text) {
            pending_code_lines.push(plain_block_text);
            continue;
        }

        if !pending_code_lines.is_empty() {
            let code_text = pending_code_lines
                .iter()
                .map(|l| l.trim_end())
                .collect::<Vec<_>>()
                .join("\n");
            if !code_text.trim().is_empty() {
                html_parts.push(format!(
                    "<pre><code>{}</code></pre>",
                    html_escape_basic(code_text.trim())
                ));
            }
            pending_code_lines.clear();
        }

        if class_str.contains("longform-header-two") {
            let text = collect_block_html(&block, &link_sel);
            html_parts.push(format!("<h2>{}</h2>", text));
        } else if class_str.contains("longform-header-three") {
            let text = collect_block_html(&block, &link_sel);
            html_parts.push(format!("<h3>{}</h3>", text));
        } else if class_str.contains("longform-blockquote") {
            let text = collect_block_html(&block, &link_sel);
            html_parts.push(format!("<blockquote>{}</blockquote>", text));
        } else if class_str.contains("longform-unordered-list-item") {
            // Collect consecutive list items
            let text = collect_block_html(&block, &link_sel);
            // Check if previous part is already a <ul>
            if let Some(last) = html_parts.last_mut() {
                if last.ends_with("</ul>") {
                    // Append to existing list
                    let insert_pos = last.len() - 5; // before </ul>
                    last.insert_str(insert_pos, &format!("<li>{}</li>", text));
                    continue;
                }
            }
            html_parts.push(format!("<ul><li>{}</li></ul>", text));
        } else if class_str.contains("longform-ordered-list-item") {
            let text = collect_block_html(&block, &link_sel);
            if let Some(last) = html_parts.last_mut() {
                if last.ends_with("</ol>") {
                    let insert_pos = last.len() - 5;
                    last.insert_str(insert_pos, &format!("<li>{}</li>", text));
                    continue;
                }
            }
            html_parts.push(format!("<ol><li>{}</li></ol>", text));
        } else {
            // Check if this is an image section
            let images: Vec<_> = block.select(&img_sel).collect();
            let content_images: Vec<_> = images
                .iter()
                .filter(|img| {
                    img.value()
                        .attr("src")
                        .map(|s| s.contains("twimg.com/media"))
                        .unwrap_or(false)
                })
                .collect();

            if !content_images.is_empty() {
                for img in content_images {
                    if let Some(src) = img.value().attr("src") {
                        // Upgrade to large format
                        let large_src = upgrade_twimg_url(src);
                        let alt = img.value().attr("alt").unwrap_or("Image");
                        html_parts.push(format!("<figure><img src=\"{}\" alt=\"{}\"/></figure>", large_src, alt));
                    }
                }
            } else {
                if let Some(list_html) = try_format_numbered_twitter_list(&plain_block_text) {
                    html_parts.push(list_html);
                    continue;
                }
                // Regular paragraph
                let text = collect_block_html(&block, &link_sel);
                if !text.trim().is_empty() {
                    html_parts.push(format!("<p>{}</p>", text));
                }
            }
        }
    }

    if !pending_code_lines.is_empty() {
        let code_text = pending_code_lines
            .iter()
            .map(|l| l.trim_end())
            .collect::<Vec<_>>()
            .join("\n");
        if !code_text.trim().is_empty() {
            html_parts.push(format!(
                "<pre><code>{}</code></pre>",
                html_escape_basic(code_text.trim())
            ));
        }
    }

    html_parts.join("\n")
}

/// Collects the HTML content of a DraftJS block, preserving links and inline formatting.
fn collect_block_html(block: &scraper::ElementRef, _link_sel: &Selector) -> String {
    // Strategy: walk through child nodes, extracting text and links
    // We need to handle: <span data-text="true">text</span> and <a href="...">text</a>
    // Also bold (<span style="font-weight: bold">) and italic (<span style="font-style: italic">)
    let inner = block.inner_html();

    // Extract text and links in order using the raw inner HTML
    // First, try to build content from data-offset-key spans
    let mut result = String::new();
    let fragment = Html::parse_fragment(&inner);
    let root = fragment.root_element();

    collect_node_html(&root, &mut result);

    result.trim().to_string()
}

/// Recursively collects HTML from a node tree, preserving links and basic formatting.
fn collect_node_html(node: &scraper::ElementRef, result: &mut String) {
    for child in node.children() {
        match child.value() {
            scraper::node::Node::Text(text) => {
                let t = text.text.as_ref();
                if !t.is_empty() {
                    result.push_str(t);
                }
            }
            scraper::node::Node::Element(elem) => {
                if let Some(child_ref) = scraper::ElementRef::wrap(child) {
                    let tag = elem.name();

                    if tag == "a" {
                        if let Some(href) = elem.attr("href") {
                            result.push_str(&format!("<a href=\"{}\">", href));
                            collect_node_html(&child_ref, result);
                            result.push_str("</a>");
                            continue;
                        }
                    }

                    // Check for bold/italic via style attribute
                    let style = elem.attr("style").unwrap_or("");
                    let is_bold = style.contains("font-weight: bold");
                    let is_italic = style.contains("font-style: italic");

                    if is_bold {
                        result.push_str("<strong>");
                    }
                    if is_italic {
                        result.push_str("<em>");
                    }

                    collect_node_html(&child_ref, result);

                    if is_italic {
                        result.push_str("</em>");
                    }
                    if is_bold {
                        result.push_str("</strong>");
                    }
                }
            }
            _ => {}
        }
    }
}

/// Upgrades a twimg.com media URL to use large format instead of small.
fn upgrade_twimg_url(url: &str) -> String {
    if url.contains("name=small") {
        url.replace("name=small", "name=large")
    } else if url.contains("name=medium") {
        url.replace("name=medium", "name=large")
    } else {
        url.to_string()
    }
}

/// Unescape Unicode sequences like \u003c to actual characters.
fn unescape_unicode(s: &str) -> String {
    let re = Regex::new(r"\\u([0-9a-fA-F]{4})").unwrap();
    re.replace_all(s, |caps: &regex::Captures| {
        let hex = &caps[1];
        if let Ok(code) = u32::from_str_radix(hex, 16) {
            if let Some(ch) = char::from_u32(code) {
                return ch.to_string();
            }
        }
        caps[0].to_string()
    })
    .to_string()
}

/// Extract article content from LessWrong's Apollo SSR data.
fn extract_lesswrong_article(html: &str) -> Option<Article> {
    // Extract title from: "post":{"result":{"_id":"...","slug":"...","title":"The Title"
    let title_re = Regex::new(r#""post":\s*\{\s*"result":\s*\{[^}]*"title"\s*:\s*"([^"]+)""#).ok()?;
    let title = title_re
        .captures(html)
        .map(|c| c.get(1).unwrap().as_str().to_string())
        .unwrap_or_else(|| "Untitled".to_string());

    // Extract author from: "displayName":"Author Name"
    let author_re = Regex::new(r#""user":\s*\{[^}]*"displayName"\s*:\s*"([^"]+)""#).ok()?;
    let author = author_re
        .captures(html)
        .map(|c| c.get(1).unwrap().as_str().to_string())
        .unwrap_or_else(|| "LessWrong".to_string());

    // Extract content from: "tableOfContents":{"html":"<content>"
    // The content ends at ","sections": or similar
    let content_re =
        Regex::new(r#""tableOfContents"\s*:\s*\{\s*"html"\s*:\s*"((?:[^"\\]|\\.)*)""#).ok()?;
    let content_match = content_re.captures(html)?;
    let raw_content = content_match.get(1)?.as_str();

    // Unescape Unicode and other escape sequences
    let content_html = unescape_unicode(raw_content)
        .replace("\\\"", "\"")
        .replace("\\/", "/")
        .replace("\\n", "\n")
        .replace("\\t", "\t");

    if content_html.is_empty() {
        return None;
    }

    Some(Article {
        title,
        author,
        content_html,
    })
}

/// Checks if an image element looks like content (not a small icon/button image).
fn is_content_image(img: &scraper::ElementRef) -> bool {
    // Check width attribute
    if let Some(width) = img.value().attr("width") {
        // "100%" or large percentage = content
        if width.ends_with('%') {
            if let Ok(pct) = width.trim_end_matches('%').parse::<u32>() {
                if pct >= 50 {
                    return true;
                }
            }
        }
        // Explicit pixel width - icons are typically <= 48px
        if let Ok(px) = width.parse::<u32>() {
            return px > 48;
        }
    }

    // Check height attribute for small icons
    if let Some(height) = img.value().attr("height") {
        if let Ok(px) = height.parse::<u32>() {
            if px <= 48 {
                return false;
            }
        }
    }

    // Check src - data URIs for tiny images, or icon-like paths
    if let Some(src) = img.value().attr("src") {
        let src_lower = src.to_lowercase();
        // Skip obvious icons
        if src_lower.contains("icon") || src_lower.contains("logo") || src_lower.contains("favicon") {
            return false;
        }
        // Small inline SVGs are usually icons
        if src.starts_with("data:image/svg") && src.len() < 500 {
            return false;
        }
    }

    // No explicit small size = assume content
    true
}

/// Pre-processes HTML to convert custom "steps" components to ordered lists.
/// Sites like code.claude.com use `<div role="list" class="steps">` with custom
/// data-component-part attributes that Readability doesn't understand.
fn convert_steps_to_list(html: &str) -> String {
    let document = Html::parse_document(html);
    // Match div with role="list" that has "steps" in its class attribute
    let steps_selector = Selector::parse(r#"div[role="list"][class*="steps"]"#).unwrap();
    // Match div with role="listitem" that has "step" in its class attribute
    let step_selector = Selector::parse(r#"div[role="listitem"][class*="step"]"#).unwrap();
    let title_selector = Selector::parse(r#"[data-component-part="step-title"]"#).unwrap();
    let content_selector = Selector::parse(r#"[data-component-part="step-content"]"#).unwrap();

    let mut result = html.to_string();
    let mut replacements = Vec::new();

    for steps_container in document.select(&steps_selector) {
        let mut list_items = Vec::new();

        for step in steps_container.select(&step_selector) {
            let title = step
                .select(&title_selector)
                .next()
                .map(|el| el.text().collect::<String>())
                .unwrap_or_default();

            let content = step
                .select(&content_selector)
                .next()
                .map(|el| el.inner_html())
                .unwrap_or_default();

            if !title.is_empty() || !content.is_empty() {
                let item = if !title.is_empty() && !content.is_empty() {
                    format!("<li><strong>{}</strong>: {}</li>", title.trim(), content.trim())
                } else if !title.is_empty() {
                    format!("<li><strong>{}</strong></li>", title.trim())
                } else {
                    format!("<li>{}</li>", content.trim())
                };
                list_items.push(item);
            }
        }

        if !list_items.is_empty() {
            let ordered_list = format!("<ol>{}</ol>", list_items.join("\n"));
            replacements.push((list_items.len(), ordered_list));
        }
    }

    // Use regex to find and replace the steps containers
    // Match: <div role="list" class="steps...">...</div> (the entire container)
    if !replacements.is_empty() {
        // Find opening tags of steps containers
        let steps_pattern =
            Regex::new(r#"<div[^>]*role="list"[^>]*class="[^"]*steps[^"]*"[^>]*>"#).unwrap();

        for (item_count, replacement) in replacements {
            if let Some(mat) = steps_pattern.find(&result) {
                let start = mat.start();
                // Find matching closing </div> - need to count nested divs
                if let Some(end) = find_matching_close_tag(&result[start..], "div") {
                    let full_end = start + end;
                    log::debug!(
                        "Converted steps component to ordered list with {} items",
                        item_count
                    );
                    result = format!("{}{}{}", &result[..start], replacement, &result[full_end..]);
                }
            }
        }
    }

    result
}

/// Find the position after the matching closing tag, handling nested tags.
fn find_matching_close_tag(html: &str, tag: &str) -> Option<usize> {
    let open_tag = format!("<{}", tag);
    let close_tag = format!("</{}>", tag);
    let mut depth = 0;
    let char_indices: Vec<(usize, char)> = html.char_indices().collect();
    let mut i = 0;

    while i < char_indices.len() {
        let (byte_pos, _) = char_indices[i];
        let remaining = &html[byte_pos..];

        if remaining.starts_with(&open_tag) {
            depth += 1;
            // Skip past the open tag
            let skip_bytes = open_tag.len();
            while i < char_indices.len() && char_indices[i].0 < byte_pos + skip_bytes {
                i += 1;
            }
        } else if remaining.starts_with(&close_tag) {
            depth -= 1;
            if depth == 0 {
                return Some(byte_pos + close_tag.len());
            }
            // Skip past the close tag
            let skip_bytes = close_tag.len();
            while i < char_indices.len() && char_indices[i].0 < byte_pos + skip_bytes {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    None
}

/// Pre-processes HTML to unwrap content images from button/overlay containers.
/// Uses regex to find button tags containing img tags and extract just the img.
fn unwrap_images_from_buttons(html: &str) -> String {
    let document = Html::parse_document(html);
    let button_selector = Selector::parse("button").unwrap();
    let img_selector = Selector::parse("img").unwrap();

    let mut result = html.to_string();
    let mut replacements = Vec::new();

    // Collect image srcs that need unwrapping
    for button in document.select(&button_selector) {
        if let Some(img) = button.select(&img_selector).next() {
            if is_content_image(&img) {
                if let Some(src) = img.value().attr("src") {
                    replacements.push(src.to_string());
                }
            }
        }
    }

    // For each src, find the button containing it and replace with just the img
    for src in replacements {
        // Escape special regex characters in src
        let escaped_src = regex::escape(&src);

        // Pattern: <button ...> ... <img ... src="..."> ... </button>
        // Using [\s\S]*? to match any character including newlines (non-greedy)
        let pattern = format!(
            r#"<button[^>]*>[\s\S]*?<img([^>]*src=["']{}["'][^>]*)>[\s\S]*?</button>"#,
            escaped_src
        );

        if let Ok(re) = Regex::new(&pattern) {
            if let Some(caps) = re.captures(&result) {
                // Reconstruct the img tag from captured attributes
                let img_attrs = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                let img_tag = format!("<img{}>", img_attrs);
                let full_match = caps.get(0).unwrap().as_str();
                result = result.replace(full_match, &img_tag);
                log::debug!("Unwrapped content image from button: {}", src);
            }
        }
    }

    result
}

/// Extracts the largest/best URL from a srcSet attribute.
/// srcSet format: "url1 descriptor1, url2 descriptor2, ..."
fn best_url_from_srcset(srcset: &str) -> Option<String> {
    let srcset = srcset.trim();
    if srcset.is_empty() {
        return None;
    }

    // Parse srcset entries to find the largest width
    let mut best_url = None;
    let mut best_width = 0u32;
    let chars: Vec<char> = srcset.chars().collect();
    let mut i = 0;
    let mut entry_start = 0;

    while i < chars.len() {
        if chars[i].is_whitespace() {
            let ws_start = i;
            while i < chars.len() && chars[i].is_whitespace() {
                i += 1;
            }
            if i >= chars.len() {
                break;
            }

            let digit_start = i;
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }

            if i > digit_start && i < chars.len() {
                let descriptor_char = chars[i].to_ascii_lowercase();
                if descriptor_char == 'w' || descriptor_char == 'x' {
                    let url: String = chars[entry_start..ws_start].iter().collect();
                    let url = url.trim().to_string();
                    let value_str: String = chars[digit_start..i].iter().collect();
                    let value: u32 = value_str.parse().unwrap_or(0);
                    let width = if descriptor_char == 'x' { value * 1000 } else { value };

                    if !url.is_empty() && width > best_width {
                        best_width = width;
                        best_url = Some(url);
                    }

                    i += 1;
                    while i < chars.len() && (chars[i].is_whitespace() || chars[i] == ',') {
                        i += 1;
                    }
                    entry_start = i;
                    continue;
                }
            }
        }
        i += 1;
    }

    // Handle remaining content or URL without descriptor
    if best_url.is_none() && entry_start < chars.len() {
        let remaining: String = chars[entry_start..].iter().collect();
        let url = remaining.split_whitespace().next().unwrap_or("").trim();
        if !url.is_empty() {
            return Some(url.to_string());
        }
    }

    best_url
}

/// Pre-processes HTML to convert <picture> elements to <img> elements.
/// The <picture> element uses <source srcSet=""> which Readability doesn't understand.
fn convert_picture_to_img(html: &str) -> String {
    // Use regex to find <picture>...</picture> elements
    // This works better than DOM-based replacement since HTML serialization can differ
    let picture_re = Regex::new(r"(?s)<picture[^>]*>(.*?)</picture>").unwrap();

    let result = picture_re.replace_all(html, |caps: &regex::Captures| {
        let inner = &caps[1];

        // Extract the best URL from source srcSet attributes
        // Prefer light theme (no -dark in URL), look for largest size
        let srcset_re = Regex::new(r#"srcSet="([^"]+)""#).unwrap();
        let mut best_src = None;
        let mut best_width = 0u32;
        let mut is_dark = true;

        for srcset_match in srcset_re.captures_iter(inner) {
            let srcset = &srcset_match[1];
            let this_is_dark = srcset.contains("-dark.");

            // Skip dark variants if we already have a light one
            if !is_dark && this_is_dark {
                continue;
            }

            if let Some(url) = best_url_from_srcset(srcset) {
                // If this is light theme or we don't have anything yet, consider it
                if !this_is_dark || best_src.is_none() {
                    // Get width for this URL (approximate from srcset)
                    let width = extract_width_from_srcset(srcset).unwrap_or(0);
                    if !this_is_dark || (best_src.is_none() && width >= best_width) {
                        best_src = Some(url);
                        best_width = width;
                        is_dark = this_is_dark;
                        if !this_is_dark {
                            // Found light theme, keep looking for better one
                        }
                    }
                }
            }
        }

        // Extract alt text from nested img
        let alt_re = Regex::new(r#"alt="([^"]*)""#).unwrap();
        let alt = alt_re
            .captures(inner)
            .map(|c| c[1].to_string())
            .unwrap_or_default();

        if let Some(src) = best_src {
            log::debug!("Converting <picture> to <img>: src={} alt={}", &src[..80.min(src.len())], &alt[..50.min(alt.len())]);
            format!("<img src=\"{}\" alt=\"{}\">", src, alt)
        } else {
            // Keep original if we couldn't extract a URL
            caps[0].to_string()
        }
    });

    result.to_string()
}

/// Extract the largest width from a srcset string.
fn extract_width_from_srcset(srcset: &str) -> Option<u32> {
    let width_re = Regex::new(r"(\d+)w").unwrap();
    width_re
        .captures_iter(srcset)
        .filter_map(|c| c[1].parse::<u32>().ok())
        .max()
}

/// Pre-processes HTML to simplify figure elements for Readability.
/// Readability often strips figures with complex nested structures or non-standard classes.
/// This extracts images and wraps them in simple figure elements.
fn simplify_figures(html: &str) -> String {
    // Match any figure element - we'll check if it needs simplification
    let figure_re = Regex::new(r#"(?s)<figure[^>]*>(.*?)</figure>"#).unwrap();

    let result = figure_re.replace_all(html, |caps: &regex::Captures| {
        let inner = &caps[1];
        let full_match = &caps[0];

        // Extract any img tags from inside
        let img_re = Regex::new(r#"<img[^>]+>"#).unwrap();
        let imgs: Vec<&str> = img_re.find_iter(inner).map(|m| m.as_str()).collect();

        if imgs.is_empty() {
            // No images found, keep original
            full_match.to_string()
        } else {
            // Check if the figure is "complex" (has many nested divs, unusual classes)
            let div_count = inner.matches("<div").count();
            let has_layout_classes = full_match.contains("grid")
                || full_match.contains("flex")
                || full_match.contains("@container")
                || full_match.contains("col-span");

            if div_count > 2 || has_layout_classes {
                // Complex figure - simplify to just the images
                // Also try to extract figcaption if present
                let caption_re = Regex::new(r#"(?s)<figcaption[^>]*>(.*?)</figcaption>"#).unwrap();
                let caption = caption_re
                    .captures(inner)
                    .map(|c| format!("<figcaption>{}</figcaption>", strip_tags(&c[1])));

                let img_html = imgs.join("\n");
                log::debug!(
                    "Simplifying complex figure ({} divs, {} images)",
                    div_count,
                    imgs.len()
                );

                match caption {
                    Some(cap) => format!("<figure>{}\n{}</figure>", img_html, cap),
                    None => format!("<figure>{}</figure>", img_html),
                }
            } else {
                // Simple figure, keep as-is
                full_match.to_string()
            }
        }
    });

    result.to_string()
}

/// Strip HTML tags from text, keeping only text content.
fn strip_tags(html: &str) -> String {
    let tag_re = Regex::new(r"<[^>]+>").unwrap();
    tag_re.replace_all(html, "").trim().to_string()
}

fn count_img_tags(html: &str) -> usize {
    let img_re = Regex::new(r"(?i)<img\b").unwrap();
    img_re.find_iter(html).count()
}

/// Fallback extractor for Squarespace image blocks that Readability may drop.
/// Returns normalized `<figure><img/></figure>` HTML snippets.
fn extract_squarespace_image_blocks(html: &str) -> Vec<String> {
    let document = Html::parse_document(html);
    let block_sel = Selector::parse("div.sqs-block-image, div.image-block").unwrap();
    let img_sel = Selector::parse("img").unwrap();
    let caption_sel = Selector::parse(".image-caption, figcaption").unwrap();

    let mut seen = std::collections::HashSet::new();
    let mut figures = Vec::new();

    for block in document.select(&block_sel) {
        let src = block
            .select(&img_sel)
            .find_map(|img| {
                img.value()
                    .attr("data-src")
                    .or_else(|| img.value().attr("src"))
                    .map(|s| s.to_string())
            });

        let Some(src) = src else { continue };
        if src.is_empty() || !seen.insert(src.clone()) {
            continue;
        }

        let caption = block
            .select(&caption_sel)
            .next()
            .map(|c| c.text().collect::<String>().trim().to_string())
            .filter(|s| !s.is_empty());

        let fig = match caption {
            Some(c) => format!(
                "<figure><img src=\"{}\" alt=\"\"/><figcaption>{}</figcaption></figure>",
                src,
                html_escape_basic(&c)
            ),
            None => format!("<figure><img src=\"{}\" alt=\"\"/></figure>", src),
        };
        figures.push(fig);
    }

    figures
}

/// Pre-processes HTML to normalize code blocks for Readability.
/// Many sites use custom code block structures with line numbers, syntax highlighting,
/// and non-standard nesting. This converts them to simple <pre><code> format.
fn normalize_code_blocks(html: &str) -> String {
    // Match any <pre> element
    let pre_re = Regex::new(r#"(?s)<pre([^>]*)>(.*?)</pre>"#).unwrap();

    let result = pre_re.replace_all(html, |caps: &regex::Captures| {
        let attrs = &caps[1];
        let inner = &caps[2];

        // Check if this is already a standard <pre><code> structure
        let has_direct_code = inner.trim().starts_with("<code") && inner.trim().ends_with("</code>");
        let div_count = inner.matches("<div").count();
        let has_layout_classes = attrs.contains("flex") || attrs.contains("grid");

        // If it's a simple <pre><code>text</code></pre>, leave it alone
        if has_direct_code && div_count == 0 && !has_layout_classes {
            return caps[0].to_string();
        }

        // Check if this needs normalization
        let needs_normalization = div_count > 0 || has_layout_classes;

        if !needs_normalization {
            // Simple pre without code tag - wrap content in code
            if !inner.contains("<code") {
                let text = strip_tags(inner);
                if !text.is_empty() {
                    return format!("<pre><code>{}</code></pre>", html_escape_basic(&text));
                }
            }
            return caps[0].to_string();
        }

        // Complex code block - extract text content
        // Try to find code lines in divs (common pattern for line-numbered code)
        let line_div_re = Regex::new(r#"(?s)<div[^>]*>(.*?)</div>"#).unwrap();
        let code_tag_re = Regex::new(r#"(?s)<code[^>]*>(.*?)</code>"#).unwrap();

        // First try to get content from nested <code> elements
        let code_content: String = if inner.contains("<code") {
            code_tag_re
                .captures_iter(inner)
                .map(|c| strip_tags(&c[1]))
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            // Fall back to extracting from divs (line-by-line code blocks)
            let lines: Vec<String> = line_div_re
                .captures_iter(inner)
                .map(|c| strip_tags(&c[1]))
                .filter(|s| !s.is_empty())
                .collect();

            if lines.is_empty() {
                // Last resort: just strip all tags
                strip_tags(inner)
            } else {
                lines.join("\n")
            }
        };

        if code_content.trim().is_empty() {
            // No meaningful content extracted, keep original
            caps[0].to_string()
        } else {
            log::debug!(
                "Normalizing code block ({} divs, {} chars)",
                div_count,
                code_content.len()
            );
            format!("<pre><code>{}</code></pre>", html_escape_basic(&code_content))
        }
    });

    result.to_string()
}

/// Decodes common HTML entities to their character equivalents.
fn decode_html_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&#x2F;", "/")
        .replace("&nbsp;", "\u{00A0}")
        .replace("&mdash;", "\u{2014}")
        .replace("&ndash;", "\u{2013}")
        .replace("&rsquo;", "\u{2019}")
        .replace("&lsquo;", "\u{2018}")
        .replace("&rdquo;", "\u{201D}")
        .replace("&ldquo;", "\u{201C}")
        .replace("&hellip;", "\u{2026}")
}

/// HTML escaping for code content - decodes first to avoid double-encoding.
fn html_escape_basic(s: &str) -> String {
    // First decode any existing entities to avoid double-encoding
    let decoded = decode_html_entities(s);
    // Then encode for safe HTML display
    decoded
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Preserves aside/note elements that Readability would otherwise filter out.
/// Converts <aside> tags and <div class="...aside..."> elements to <blockquote>
/// with a special class, which Readability preserves and can be styled in EPUB.
///
/// Uses class="pullquote" instead of "aside" because dom_smoothie penalizes
/// elements with "aside" in their class names.
fn preserve_aside_elements(html: &str) -> String {
    let mut result = html.to_string();

    // Convert <aside ...>...</aside> to <blockquote class="pullquote">...</blockquote>
    // Match opening aside tag with any attributes
    let aside_open_re = Regex::new(r#"<aside([^>]*)>"#).unwrap();
    result = aside_open_re
        .replace_all(&result, |caps: &regex::Captures| {
            let attrs = &caps[1];
            // Remove any existing class attribute and add our own
            let class_re = Regex::new(r#"\s*class="[^"]*""#).unwrap();
            let cleaned_attrs = class_re.replace(attrs, "");
            format!("<blockquote class=\"pullquote\"{}>", cleaned_attrs)
        })
        .to_string();

    // Replace closing </aside> tags
    result = result.replace("</aside>", "</blockquote>");

    // Convert <div class="...aside..."> or <div class="...note..."> patterns
    // These are commonly used for callouts, tips, warnings, etc.
    // Match the full div element and replace it entirely
    let div_full_re = Regex::new(
        r#"(?s)<div[^>]*class="[^"]*(?:post-aside|aside-note|callout|admonition)[^"]*"[^>]*>(.*?)</div>"#
    ).unwrap();

    result = div_full_re
        .replace_all(&result, |caps: &regex::Captures| {
            let content = &caps[1];
            log::debug!("Preserving aside/note element as pullquote");
            format!("<blockquote class=\"pullquote\">{}</blockquote>", content)
        })
        .to_string();

    result
}

/// Unwraps custom HTML elements (e.g. GitHub's `<markdown-accessiblity-table>`)
/// that Readability doesn't understand and would strip.
/// Replaces the custom element wrapper with a plain `<div>`, preserving inner content.
fn unwrap_custom_elements(html: &str) -> String {
    // Custom elements have a hyphen in their tag name (web components spec).
    // GitHub uses: markdown-accessiblity-table, task-lists, turbo-frame, etc.
    let custom_el_re =
        Regex::new(r"(?s)<((?:markdown|task|turbo|tool|relative|auto|time|clipboard|include|qbsearch|modal|react|dialog|anchored|action|file|deferred|remote|details|summary|tab|scroll|sticky|theme|color|animated|batch|filter|image|text|virtual|github|gh|readme|blob|diff|code|commit|tree|repo|nav|header|footer|sidebar|slot|template)[a-z-]*)([^>]*)>").unwrap();

    let mut result = html.to_string();

    // Find all custom element tag names used in the document
    let mut custom_tags: Vec<String> = Vec::new();
    for caps in custom_el_re.captures_iter(html) {
        let tag = caps.get(1).unwrap().as_str().to_string();
        if !custom_tags.contains(&tag) {
            custom_tags.push(tag);
        }
    }

    for tag in &custom_tags {
        // Replace opening tags with <div>
        let open_re = Regex::new(&format!(r"<{}([^>]*)>", regex::escape(tag))).unwrap();
        result = open_re.replace_all(&result, "<div>").to_string();

        // Replace closing tags with </div>
        result = result.replace(&format!("</{}>", tag), "</div>");
    }

    if !custom_tags.is_empty() {
        log::debug!(
            "Unwrapped {} custom element types: {:?}",
            custom_tags.len(),
            custom_tags
        );
    }

    result
}

/// Removes CSS layout classes that confuse Readability's content detection.
/// Classes like 'grid', 'flex', 'col-span' are for styling only and don't
/// indicate whether content is article-relevant or not.
fn strip_layout_classes(html: &str) -> String {
    // Pattern to match class attributes
    let class_re = Regex::new(r#"class="([^"]*)""#).unwrap();

    // Layout-only class patterns to remove (these are for CSS, not content semantics)
    let layout_patterns = [
        // Tailwind/CSS grid and flex
        r"@container",
        r"@md:\S+",         // responsive prefixes like @md:col-start-3
        r"@sm:\S+",
        r"@lg:\S+",
        r"@xl:\S+",
        r"col-span-\d+",
        r"col-start-\d+",
        r"row-span-\d+",
        r"grid-cols-\d+",
        r"gap-\S+",
        r"flex-\w+",        // flex-1, flex-col, etc.
        r"flex\b",          // standalone flex
        r"grid\b",          // standalone grid (not grid-area)
        r"justify-\w+",
        r"items-\w+",
        r"self-\w+",
        r"order-\d+",
        // Spacing/sizing (purely visual)
        r"p[xytblr]?-\S+",  // padding
        r"m[xytblr]?-\S+",  // margin (but not 'main' or 'menu')
        r"w-\S+",           // width
        r"h-\S+",           // height
        r"min-w-\S+",
        r"max-w-\S+",
        r"min-h-\S+",
        r"max-h-\S+",
        // Positioning (purely visual)
        r"absolute",
        r"relative",
        r"fixed",
        r"sticky",
        r"inset-\S+",
        r"top-\S+",
        r"left-\S+",
        r"right-\S+",
        r"bottom-\S+",
        r"z-\S+",
        // Transitions/animations (purely visual)
        r"transition\S*",
        r"duration-\S+",
        r"ease-\S+",
        r"animate-\S+",
        // Overflow (usually visual)
        r"overflow-\S+",
    ];

    // Build a combined regex for all patterns
    let combined_pattern = layout_patterns.join("|");
    let layout_class_re = Regex::new(&format!(r"\b({})\b", combined_pattern)).unwrap();

    let result = class_re.replace_all(html, |caps: &regex::Captures| {
        let classes = &caps[1];

        // Remove layout classes
        let cleaned: Vec<&str> = classes
            .split_whitespace()
            .filter(|c| !layout_class_re.is_match(c))
            .collect();

        if cleaned.is_empty() {
            // If all classes were layout, keep a neutral class
            String::from("class=\"\"")
        } else {
            format!("class=\"{}\"", cleaned.join(" "))
        }
    });

    log::debug!("Stripped layout classes from HTML");
    result.to_string()
}

/// Extracts article content from JSON-LD structured data (schema.org).
/// Many sites—especially paywalled ones—include the full article text in their
/// structured data for SEO/Google indexing, even when the rendered HTML is truncated.
fn extract_jsonld_article(html: &str) -> Option<Article> {
    let re =
        Regex::new(r#"(?s)<script[^>]*type="application/ld\+json"[^>]*>(.*?)</script>"#).ok()?;

    for caps in re.captures_iter(html) {
        let json_str = &caps[1];
        let value: JsonValue = serde_json::from_str(json_str).ok()?;

        // JSON-LD can be a single object or an array of objects
        let items: Vec<&JsonValue> = match &value {
            JsonValue::Array(arr) => arr.iter().collect(),
            obj @ JsonValue::Object(_) => vec![obj],
            _ => continue,
        };

        for item in items {
            let article_type = item.get("@type").and_then(|t| t.as_str()).unwrap_or("");
            if !matches!(
                article_type,
                "Article"
                    | "NewsArticle"
                    | "BlogPosting"
                    | "TechArticle"
                    | "ScholarlyArticle"
                    | "Report"
                    | "AnalysisNewsArticle"
                    | "OpinionNewsArticle"
                    | "ReviewNewsArticle"
            ) {
                continue;
            }

            let body = item
                .get("articleBody")
                .and_then(|b| b.as_str())
                .unwrap_or("");
            if body.len() < 500 {
                continue;
            }

            let title = decode_html_entities(
                item.get("headline")
                    .and_then(|h| h.as_str())
                    .unwrap_or("Untitled"),
            );

            let author = jsonld_author(item);
            let content_html = plain_text_to_html(body);

            log::info!(
                "Extracted article from JSON-LD: {} ({} chars)",
                title,
                body.len()
            );
            return Some(Article {
                title,
                author,
                content_html,
            });
        }
    }

    None
}

/// Extracts author name(s) from a JSON-LD item.
fn jsonld_author(item: &JsonValue) -> String {
    match item.get("author") {
        Some(JsonValue::String(s)) => s.clone(),
        Some(JsonValue::Object(obj)) => obj
            .get("name")
            .and_then(|n| n.as_str())
            .unwrap_or("Unknown")
            .to_string(),
        Some(JsonValue::Array(arr)) => {
            let names: Vec<&str> = arr
                .iter()
                .filter_map(|a| a.get("name").and_then(|n| n.as_str()))
                .collect();
            if names.is_empty() {
                "Unknown".to_string()
            } else {
                names.join(", ")
            }
        }
        _ => "Unknown".to_string(),
    }
}

/// Converts plain text to HTML paragraphs.
/// Splits on double newlines if present, otherwise on single newlines,
/// otherwise wraps the entire text in a single `<p>` tag.
fn plain_text_to_html(text: &str) -> String {
    let paragraphs: Vec<&str> = if text.contains("\n\n") {
        text.split("\n\n").collect()
    } else if text.contains('\n') {
        text.split('\n').collect()
    } else {
        vec![text]
    };

    paragraphs
        .iter()
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .map(|p| format!("<p>{}</p>", p))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Extracts article content from HTML using dom_smoothie (Mozilla Readability port).
pub fn extract_article(html: &str, url: &str) -> Result<Article, ScraperError> {
    // Try Twitter/X-specific extraction for rendered tweet pages
    if is_twitter_url(url) {
        if let Some(article) = extract_twitter_article(html) {
            return Ok(article);
        }
        // Fall through to standard extraction if Twitter-specific fails
    }

    // Try LessWrong-specific extraction first for Apollo SSR sites
    if is_lesswrong_url(url) {
        if let Some(article) = extract_lesswrong_article(html) {
            return Ok(article);
        }
        // Fall through to standard extraction if LessWrong-specific fails
    }

    // Pre-process HTML to convert custom components to standard HTML
    log::debug!("Starting HTML preprocessing...");
    let html_unwrapped = unwrap_custom_elements(html);
    let html_with_asides = preserve_aside_elements(&html_unwrapped);
    let html_with_pictures = convert_picture_to_img(&html_with_asides);
    let html_with_figures = simplify_figures(&html_with_pictures);
    let html_with_code = normalize_code_blocks(&html_with_figures);
    let html_stripped = strip_layout_classes(&html_with_code);
    let html_with_lists = convert_steps_to_list(&html_stripped);
    let preprocessed_html = unwrap_images_from_buttons(&html_with_lists);
    log::debug!("Preprocessing complete, running Readability...");

    use dom_smoothie::{Config, Readability};

    let config = Config {
        max_elements_to_parse: 0, // 0 = no limit
        ..Default::default()
    };

    let mut readability = Readability::new(preprocessed_html, Some(url), Some(config))
        .map_err(|e| ScraperError::DomError(e.to_string()))?;

    let article = readability
        .parse()
        .map_err(|e| ScraperError::DomError(e.to_string()))?;

    let mut content = article.content.to_string();
    let readability_text_len = strip_tags(&content).len();

    // If Readability extracted very little, check JSON-LD for fuller content.
    // Many paywalled sites include the full article in schema.org structured data.
    if let Some(jsonld) = extract_jsonld_article(html) {
        let jsonld_text_len = strip_tags(&jsonld.content_html).len();
        if jsonld_text_len > readability_text_len * 2 && jsonld_text_len > 500 {
            log::info!(
                "JSON-LD has {}x more content than Readability ({} vs {} chars), using JSON-LD",
                jsonld_text_len / readability_text_len.max(1),
                jsonld_text_len,
                readability_text_len
            );
            return Ok(jsonld);
        }
    }

    if content.is_empty() {
        // Last resort: try JSON-LD even without the ratio check
        if let Some(jsonld) = extract_jsonld_article(html) {
            return Ok(jsonld);
        }
        return Err(ScraperError::ExtractionFailed);
    }

    // Readability may drop Squarespace image blocks entirely.
    // If no images survived but source contains recognizable Squarespace image blocks,
    // append recovered figures so downstream image normalization can capture them.
    if count_img_tags(&content) == 0 {
        let recovered_figures = extract_squarespace_image_blocks(html);
        if !recovered_figures.is_empty() {
            log::info!(
                "Recovered {} Squarespace image block(s) after Readability",
                recovered_figures.len()
            );
            content.push('\n');
            content.push_str(&recovered_figures.join("\n"));
        }
    }

    // title is a String, check if empty
    let title = if article.title.is_empty() {
        "Untitled".to_string()
    } else {
        article.title
    };

    // byline is Option<String>
    let author = article
        .byline
        .filter(|b| !b.is_empty())
        .unwrap_or_else(|| extract_hostname(url));

    Ok(Article {
        title,
        author,
        content_html: content,
    })
}

/// Fetches a URL and extracts article content in one step.
/// Falls back to browser rendering for JavaScript SPAs.
pub fn fetch_and_extract(client: &Client, url: &str) -> Result<Article, ScraperError> {
    // Try raw content fetch first (e.g. Markdown files on GitHub/GitLab/Bitbucket)
    if let Some(article) = try_fetch_raw_content(client, url) {
        return Ok(article);
    }

    // Some sites always require browser rendering (heavy JS SPAs)
    if requires_browser_rendering(url) {
        log::info!("URL requires browser rendering: {}", url);
        let rendered_html = fetch_html_via_browser(url)?;
        return extract_article(&rendered_html, url);
    }

    let html = fetch_html(client, url)?;

    // Try standard extraction first
    match extract_article(&html, url) {
        Ok(article) => Ok(article),
        Err(e) => {
            // Check if this looks like a SPA that needs browser rendering
            if is_spa_html(&html) {
                log::info!("Page appears to be a JavaScript SPA, trying browser fallback");
                let rendered_html = fetch_html_via_browser(url)?;
                extract_article(&rendered_html, url)
            } else {
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_box_drawing_tweet_as_code_fragment() {
        let sample = "The stack\n\n┌─────────────┐\n│ Your Agent. │\n└─────────────┘\n";
        assert!(is_twitter_code_fragment(sample));
    }

    #[test]
    fn detects_ascii_diagram_tweet_as_code_fragment() {
        let sample = "The stack\n\n| layer | info |\n| ----- | ---- |\n| core  | loop |\n";
        assert!(is_twitter_code_fragment(sample));
    }

    #[test]
    fn does_not_mark_regular_tweet_as_code_fragment() {
        let sample = "I shipped a new release today.\nIt has better startup time.\nThanks for testing!";
        assert!(!is_twitter_code_fragment(sample));
    }

    #[test]
    fn preserves_br_as_newlines_when_collecting_tweet_text() {
        let fragment = Html::parse_fragment(
            r#"<div data-testid="tweetText"><span>Hello</span><br/><span>world</span></div>"#,
        );
        let sel = Selector::parse(r#"[data-testid="tweetText"]"#).unwrap();
        let el = fragment.select(&sel).next().unwrap();
        let text = collect_text_with_newlines(&el);
        assert_eq!(text, "Hello\nworld");
    }

    #[test]
    fn detects_code_like_single_line_blocks() {
        assert!(looks_like_twitter_code_line("│ box │ content │"));
        assert!(looks_like_twitter_code_line("| col | value |"));
        assert!(!looks_like_twitter_code_line("This is a normal prose sentence."));
    }

    #[test]
    fn converts_longform_code_fragment_blocks_to_pre_code() {
        let fragment = Html::parse_fragment(
            r#"
            <div data-testid="twitterArticleRichTextView">
              <div data-block="true"><span>The stack</span></div>
              <div data-block="true"><span>┌─────────────┐</span></div>
              <div data-block="true"><span>│ Your Agent. │</span></div>
              <div data-block="true"><span>└─────────────┘</span></div>
              <div data-block="true"><span>Each layer adds capability.</span></div>
            </div>
            "#,
        );
        let sel = Selector::parse(r#"[data-testid="twitterArticleRichTextView"]"#).unwrap();
        let content_el = fragment.select(&sel).next().unwrap();
        let html = convert_twitter_article_html(content_el);

        assert!(html.contains("<pre><code>"));
        assert!(html.contains("Your Agent."));
        assert!(html.contains("Each layer adds capability."));
    }

    #[test]
    fn formats_flattened_numbered_tweet_as_ordered_list() {
        let text = "Intro text. 1. First item > detail. 2. Second item > detail. 3. Third item > detail. 4. Fourth item.";
        let html = try_format_numbered_twitter_list(text).expect("expected formatted list");
        assert!(html.contains("<ol>"));
        assert!(html.contains("<li>First item"));
        assert!(html.contains("<br/>detail."));
        assert!(html.contains("<p>Intro text.</p>"));
    }

    #[test]
    fn does_not_format_non_list_text_as_numbered_list() {
        let text = "Version 1.2.3 was released on 2026-02-20 with a few fixes.";
        assert!(try_format_numbered_twitter_list(text).is_none());
    }
}
