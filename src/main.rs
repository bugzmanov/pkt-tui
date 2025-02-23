#![allow(clippy::enum_glob_use, clippy::wildcard_imports)]

mod auth;
mod errors;
mod logo;
mod markdown;
mod pocket;
mod prss;
mod readingstats;
pub mod storage;
mod tokenstorage;
mod utils;

use anyhow::Context;
use chrono::{DateTime, Local, Utc};
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
        KeyboardEnhancementFlags, MouseEvent, MouseEventKind, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use dom_smoothie::{Article, Config, Readability};
use itertools::Itertools;
use log::{error, LevelFilter};
use pocket::{GetPocketSync, SendResponse};
use prss::{RssFeedItem, RssManager};
use ratatui::{prelude::*, widgets::*};
use rayon::prelude::*;
use readingstats::{render_stats, TotalStats};
use reqwest::blocking::Client;
use serde_json::json;
use std::{
    error::Error,
    fs::{self, File},
    io::{self, Write},
    ops::Range,
    path::Path,
    sync::{Arc, Mutex},
    thread::{self},
    time::{Duration, Instant},
};
use storage::{PocketItem, PocketItemUpdate};
use style::palette::tailwind;
use tui_textarea::{CursorMove, TextArea};
use unicode_width::UnicodeWidthStr;

const PALETTES: [tailwind::Palette; 4] = [
    tailwind::BLUE,
    tailwind::EMERALD,
    tailwind::INDIGO,
    tailwind::RED,
];
const INFO_TEXT: &str = "(ZZ) quit | gg/G/j/k  - start,end,↓,↑ | ? - Help";
const ITEM_HEIGHT: usize = 4;
const DELTA_FILE: &str = "snapshot_updates.db";

pub struct Base16Palette {
    pub base_00: Color,
    pub base_01: Color,
    pub base_02: Color,
    pub base_03: Color,
    pub base_04: Color,
    pub base_05: Color,
    pub base_06: Color,
    pub base_07: Color,
    pub base_08: Color,
    pub base_09: Color,
    pub base_0a: Color,
    pub base_0b: Color,
    pub base_0c: Color,
    pub base_0d: Color,
    pub base_0e: Color,
    pub base_0f: Color,
}

pub const OCEANIC_NEXT: Base16Palette = Base16Palette {
    base_00: Color::from_u32(0x1B2B34),
    base_01: Color::from_u32(0x343D46),
    base_02: Color::from_u32(0x4F5B66),
    base_03: Color::from_u32(0x65737E),
    base_04: Color::from_u32(0xA7ADBA),
    base_05: Color::from_u32(0xC0C5CE),
    base_06: Color::from_u32(0xCDD3DE),
    base_07: Color::from_u32(0xD8DEE9),
    base_08: Color::from_u32(0xEC5f67),
    base_09: Color::from_u32(0xF99157),
    base_0a: Color::from_u32(0xFAC863),
    base_0b: Color::from_u32(0x99C794),
    base_0c: Color::from_u32(0x5FB3B3),
    base_0d: Color::from_u32(0x6699CC),
    base_0e: Color::from_u32(0xC594C5),
    base_0f: Color::from_u32(0xAB7967),
};

struct TableColors {
    buffer_bg: Color,
    header_fg: Color,
    row_fg: Color,
    selected_style_fg: Color,
    _alt_row_color: Color,
    footer_border_color: Color,
}

impl TableColors {
    const fn new(color: &tailwind::Palette) -> Self {
        Self {
            buffer_bg: OCEANIC_NEXT.base_00,
            header_fg: tailwind::SLATE.c200,
            row_fg: tailwind::SLATE.c200,
            selected_style_fg: OCEANIC_NEXT.base_0a,
            _alt_row_color: tailwind::SLATE.c900,
            footer_border_color: color.c400,
        }
    }
}

impl TableRow for PocketItem {
    fn id(&self) -> String {
        self.item_id.to_string()
    }

    fn date(&self) -> String {
        let timestamp = self.time_added.parse::<i64>().unwrap();
        let naive = DateTime::from_timestamp(timestamp, 0).unwrap();
        let datetime: DateTime<Utc> = naive.to_utc();
        let newdate = datetime.format("%Y-%m-%d");
        format!("{}", newdate)
    }

    fn title(&self) -> &str {
        &self
            .given_title
            .as_deref()
            .unwrap_or(&self.resolved_title.as_deref().unwrap_or("[empty]"))
    }

    fn item_type(&self) -> &str {
        if self.url().contains("youtube.com") {
            "video"
        } else if self.url().contains("pdf") {
            "pdf"
        } else {
            "article"
        }
    }

    fn tags(&self) -> impl Iterator<Item = &String> {
        self.tags.keys()
    }

    fn url(&self) -> &str {
        (&self.resolved_url).as_deref().unwrap_or("[empty]")
    }

    fn add_tag(&mut self, tag: &str) {
        self.tags.insert(tag.to_string(), json!({}));
    }

    fn remove_tag(&mut self, tag: &str) {
        self.tags.remove(tag);
    }

    fn rename_title_to(&mut self, new_title: String) {
        self.given_title = Some(new_title);
    }

    fn time_added(&self) -> u64 {
        self.time_added.parse::<u64>().unwrap()
    }
}

//todo: remove
trait TableRow {
    fn id(&self) -> String;
    fn date(&self) -> String;
    fn time_added(&self) -> u64;
    fn title(&self) -> &str;
    fn item_type(&self) -> &str;
    fn tags(&self) -> impl Iterator<Item = &String>;
    fn url(&self) -> &str;
    fn add_tag(&mut self, tag: &str);
    fn remove_tag(&mut self, tag: &str);
    fn rename_title_to(&mut self, new_title: String);
}

pub struct RssFeedState {
    pub items: Arc<Mutex<Vec<RssFeedItem>>>,
    pub is_loading: Arc<Mutex<bool>>,
    pub has_updates: bool,
    pub error: Option<String>,
    pub items_processed: bool,
}

impl RssFeedState {
    pub fn new() -> Self {
        Self {
            items: Arc::new(Mutex::new(Vec::new())),
            is_loading: Arc::new(Mutex::new(false)),
            has_updates: false,
            error: None,
            items_processed: false,
        }
    }
    pub fn mark_items_processed(&mut self) {
        self.items_processed = true;
        self.has_updates = false; // Clear the updates flag
    }
}

pub struct RssFeedPopupState {
    pub items: Vec<RssFeedItem>,
    pub selected_index: usize,
    pub scroll_offset: usize,
    pub visible_items: usize,
    hidden_items: prss::hidden_items::HiddenItems,
    status_message: Option<(String, Instant)>, // Message and timestamp
    pending_pocket_item: Option<RssFeedItem>,  // Store item waiting for tags
    show_description: bool,
    pub changes_made: bool,
}

impl RssFeedPopupState {
    pub fn new(mut items: Vec<RssFeedItem>, visible_items: usize) -> anyhow::Result<Self> {
        let hidden_items = prss::hidden_items::HiddenItems::load()?;
        items.retain(|item| !hidden_items.is_hidden(&item.item_id));

        Ok(Self {
            items,
            selected_index: 0,
            scroll_offset: 0,
            visible_items,
            hidden_items,
            status_message: None,
            pending_pocket_item: None,
            show_description: false,
            changes_made: false,
        })
    }

    pub fn prepare_add_to_pocket(&mut self) -> Option<RssFeedItem> {
        if let Some(selected_item) = self.items.get(self.selected_index).cloned() {
            self.pending_pocket_item = Some(selected_item.clone());
            Some(selected_item)
        } else {
            None
        }
    }
    pub fn move_selection(&mut self, delta: isize) {
        let new_index = self.selected_index as isize + delta;
        self.selected_index = new_index.clamp(0, self.items.len() as isize - 1) as usize;
        if self.selected_index < self.scroll_offset {
            self.scroll_offset = self.selected_index;
        } else if self.selected_index >= self.scroll_offset + self.visible_items {
            self.scroll_offset = self.selected_index - self.visible_items + 1;
        }
    }
    pub fn hide_current_item(&mut self) -> anyhow::Result<()> {
        if let Some(item) = self.items.get(self.selected_index) {
            self.hidden_items.hide_item(item.item_id.clone())?;
            self.items.remove(self.selected_index);
            if self.selected_index >= self.items.len() && self.items.len() > 0 {
                self.selected_index = self.items.len() - 1;
            }
        }
        Ok(())
    }
    pub fn set_status(&mut self, message: String) {
        self.status_message = Some((message, Instant::now()));
    }

    pub fn add_current_to_pocket(
        &mut self,
        pocket_client: &GetPocketSync,
        tags_input: &str,
    ) -> anyhow::Result<()> {
        if let Some(item) = self.pending_pocket_item.take() {
            // Parse tags in the application code
            let tags: Vec<String> = tags_input
                .split(',')
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .collect();

            // Add to Pocket with parsed tags
            pocket_client.add(&item.link, &tags)?;

            // Hide the item
            self.hidden_items.hide_item(item.item_id.clone())?;

            // Remove from current list
            self.items.remove(self.selected_index);
            if self.selected_index >= self.items.len() && self.items.len() > 0 {
                self.selected_index = self.items.len() - 1;
            }

            // Set success message
            self.set_status(format!("✓ Added to Pocket with {} tags", tags.len()));
            self.changes_made = true;
            Ok(())
        } else {
            Err(anyhow::anyhow!("No item selected"))
        }
    }
}

struct ReadingStats {
    articles_total: usize,
    _articles_read: usize,
    videos_total: usize,
    _videos_read: usize,
    pdfs_total: usize,
    _pdfs_read: usize,
}

impl ReadingStats {
    fn new() -> Self {
        Self {
            articles_total: 0,
            _articles_read: 0,
            videos_total: 0,
            _videos_read: 0,
            pdfs_total: 0,
            _pdfs_read: 0,
        }
    }
}

fn collect_stats(items: &Vec<impl TableRow>, start_idx: usize) -> ReadingStats {
    let mut stats = ReadingStats::new();
    let mut idx = start_idx;
    let current_date = items.get(start_idx).unwrap().date();
    while idx < items.len() && items.get(idx).unwrap().date() == current_date {
        let item = items.get(idx).unwrap();
        match item.item_type() {
            "article" => stats.articles_total += 1,
            "video" => stats.videos_total += 1,
            "pdf" => stats.pdfs_total += 1,
            _ => {} // do nothing
        }
        idx += 1;
    }
    stats
}

struct TagPopupState {
    tags: Vec<(String, usize)>,
    filtered_tags: Vec<(String, usize)>,
    selected_index: usize,
    scroll_offset: usize,
    visible_items: usize,
    filter: String,
}

impl TagPopupState {
    fn new(tags: Vec<(String, usize)>, visible_items: usize) -> Self {
        Self {
            filtered_tags: tags.clone(),
            tags,
            selected_index: 0,
            scroll_offset: 0,
            visible_items,
            filter: String::new(),
        }
    }

    fn move_selection(&mut self, delta: isize) {
        let new_index = self.selected_index as isize + delta;
        self.selected_index = new_index.clamp(0, self.tags.len() as isize - 1) as usize;

        if self.selected_index < self.scroll_offset {
            self.scroll_offset = self.selected_index;
        } else if self.selected_index >= self.scroll_offset + self.visible_items {
            self.scroll_offset = self.selected_index - self.visible_items + 1;
        }
    }

    fn _selected_tag(&self) -> Option<String> {
        self.tags
            .get(self.selected_index)
            .map(|(tag, _)| tag.clone())
    }

    fn apply_filter(&mut self) {
        self.filtered_tags = self
            .tags
            .iter()
            .filter(|(tag, _)| tag.to_lowercase().contains(&self.filter.to_lowercase()))
            .cloned()
            .collect();
        self.selected_index = 0;
        self.scroll_offset = 0;
    }

    fn add_to_filter(&mut self, ch: char) {
        self.filter.push(ch);
        self.apply_filter();
    }

    fn remove_from_filter(&mut self) {
        self.filter.pop();
        self.apply_filter();
    }

    fn clear_filter(&mut self) {
        self.filter.clear();
        self.apply_filter();
    }
}

struct DocTypePopupState {
    items: Vec<(ItemTypeFilter, &'static str, &'static str)>,
}

impl DocTypePopupState {
    fn new() -> Self {
        Self {
            items: vec![
                (ItemTypeFilter::All, "1", "All Items"),
                (ItemTypeFilter::Article, "2", "Articles"),
                (ItemTypeFilter::Video, "3", "Videos"),
                (ItemTypeFilter::PDF, "4", "PDFs"),
            ],
        }
    }

    fn select_by_number(&mut self, num: char) -> Option<ItemTypeFilter> {
        self.items
            .iter()
            .find(|(_, key, _)| key == &num.to_string())
            .map(|(filter, _, _)| filter.clone())
    }
}

enum LoadingType {
    Refresh,
    Download,
}

struct RefreshingPopup {
    text: String,
    was_redered: bool,
    refresh_type: LoadingType,
    _last_update: Instant, //todo
}

impl RefreshingPopup {
    fn new(text: String, refresh_type: LoadingType) -> Self {
        Self {
            text,
            was_redered: false,
            _last_update: Instant::now(),
            refresh_type,
        }
    }
}

struct DomainStatsPopupState {
    stats: Vec<(String, usize)>,
    selected_index: usize,
    scroll_offset: usize,
    visible_items: usize,
}

impl DomainStatsPopupState {
    fn new(stats: Vec<(String, usize)>, visible_items: usize) -> Self {
        Self {
            stats,
            selected_index: 0,
            scroll_offset: 0,
            visible_items,
        }
    }

    fn move_selection(&mut self, delta: isize) {
        let new_index = self.selected_index as isize + delta;
        self.selected_index = new_index.clamp(0, self.stats.len() as isize - 1) as usize;

        // Adjust scroll if selection is out of view
        if self.selected_index < self.scroll_offset {
            self.scroll_offset = self.selected_index;
        } else if self.selected_index >= self.scroll_offset + self.visible_items {
            self.scroll_offset = self.selected_index - self.visible_items + 1;
        }
    }
}

struct HelpPopupState {
    content: String,
}

#[derive(Clone)]
enum Confirmation {
    DeletePocketItem,
}

#[derive(Clone)]
struct SearchMode {
    search: String,
    normal_mode_positions: (usize, usize),
}

impl SearchMode {
    pub fn new(normal_mode_positions: (usize, usize)) -> Self {
        SearchMode {
            search: String::new(),
            normal_mode_positions,
        }
    }
}

#[derive(Clone)]
enum CommandType {
    RenameItem,
    JumpToDate,
    Tags,
}

#[derive(Clone)]
struct TextSuggestion {
    full_text: String,
    completion: String,
}

#[derive(Clone)]
pub struct CommandEnterMode {
    prompt: String,
    current_enter: String,
    cursor_pos: usize,
    command_type: CommandType,
    current_suggestion: Option<TextSuggestion>,
}

impl CommandEnterMode {
    fn new_empty(prompt: String, command_type: CommandType) -> Self {
        Self {
            prompt,
            current_enter: String::new(),
            cursor_pos: 0,
            command_type,
            current_suggestion: None,
        }
    }
    fn new(prompt: String, current_enter: String, command_type: CommandType) -> Self {
        let cursor_pos = current_enter.len();
        Self {
            prompt,
            current_enter,
            cursor_pos,
            command_type,
            current_suggestion: None,
        }
    }
    fn update_suggestion(&mut self, suggestions: &[String]) {
        // Get the current text being typed
        let current_text = match self.command_type {
            CommandType::Tags => {
                // For tags, look at text after the last comma
                self.current_enter
                    .split(',')
                    .last()
                    .map(|s| s.trim())
                    .unwrap_or("")
            }
            _ => &self.current_enter,
        };

        error!("Tag: {}, suggestions: {:?}", current_text, suggestions);
        if current_text.len() >= 2 {
            // Find matching suggestions
            let matching_texts: Vec<&String> = suggestions
                .iter()
                .filter(|text| {
                    text.to_lowercase()
                        .starts_with(&current_text.to_lowercase())
                        && text.len() > current_text.len()
                })
                .collect();

            // Take the first matching tag as suggestion
            if let Some(suggestion) = matching_texts.first() {
                let completion = suggestion[current_text.len()..].to_string();
                self.current_suggestion = Some(TextSuggestion {
                    full_text: suggestion.to_string(),
                    completion,
                });
            } else {
                self.current_suggestion = None;
            }
        } else {
            self.current_suggestion = None;
        }
    }

    fn complete_suggestion(&mut self) -> bool {
        if let Some(suggestion) = &self.current_suggestion {
            // Get everything before the current tag
            let prefix = self
                .current_enter
                .rsplit_once(',')
                .map(|(before, _)| format!("{},", before))
                .unwrap_or_default();

            // Get the current incomplete tag
            let current_tag = self
                .current_enter
                .split(',')
                .last()
                .map(|s| s.trim())
                .unwrap_or("");

            // Complete the tag
            self.current_enter = if prefix.is_empty() {
                format!("{}, ", suggestion.full_text)
            } else {
                format!("{} {}, ", prefix, suggestion.full_text)
            };
            self.cursor_pos = self.current_enter.len();
            self.current_suggestion = None;
            true
        } else {
            false
        }
    }
}

enum AppMode {
    Initialize,
    Normal,
    Search(SearchMode),
    Confirmation(Confirmation),
    MulticharNormalModeEnter(String),
    CommandEnter(CommandEnterMode),
    Refreshing(RefreshingPopup),
    Error(String),
}

struct FilteredItems<T> {
    pub items: Vec<T>,
    is_filter_on: bool,
    filtered: Vec<usize>,
}

impl<T> FilteredItems<T> {
    pub fn non_archived(data: Vec<PocketItem>) -> FilteredItems<PocketItem> {
        let filtered = data
            .into_iter()
            .filter(|x| x.status != "1")
            .collect::<Vec<PocketItem>>();
        let data_vec_size = filtered.len();
        FilteredItems {
            items: filtered,
            is_filter_on: false,
            filtered: Vec::with_capacity(data_vec_size),
        }
    }

    pub fn new(data: Vec<T>) -> Self {
        let data_vec_size = data.len();
        FilteredItems {
            items: data,
            is_filter_on: false,
            filtered: Vec::with_capacity(data_vec_size),
        }
    }

    pub fn len(&self) -> usize {
        if !self.is_filter_on {
            self.items.len()
        } else {
            self.filtered.len()
        }
    }

    pub fn iter(&self) -> Box<dyn Iterator<Item = &T> + '_> {
        if !self.is_filter_on {
            Box::new(self.items.iter())
        } else {
            Box::new(self.filtered.iter().map(|i| &self.items[*i]))
        }
    }

    pub fn clear_filter(&mut self) {
        self.is_filter_on = false;
        self.filtered.clear();
    }

    pub fn apply_filter<P>(&mut self, mut predicate: P)
    where
        P: FnMut(&T) -> bool,
    {
        self.is_filter_on = true;
        self.filtered.clear();
        self.items
            .iter()
            .enumerate()
            .filter(|(_, x)| predicate(x))
            .for_each(|(i, _)| self.filtered.push(i));
    }

    fn get_mut(&mut self, idx: usize) -> Option<&mut T> {
        if !self.is_filter_on {
            self.items.get_mut(idx)
        } else {
            self.filtered
                .get(idx)
                .map(|index| self.items.get_mut(*index))
                .flatten()
        }
    }

    fn get(&self, idx: usize) -> Option<&T> {
        if !self.is_filter_on {
            self.items.get(idx)
        } else {
            self.filtered
                .get(idx)
                .map(|index| self.items.get(*index))
                .flatten()
        }
    }

    fn remove(&mut self, idx: usize) {
        if !self.is_filter_on {
            self.items.remove(idx);
        } else {
            self.filtered
                .get(idx)
                .map(|index| self.items.remove(*index));
        }
    }

    fn index(&self, range: Range<usize>) -> Vec<&T> {
        if !self.is_filter_on {
            self.items[range].iter().collect()
        } else {
            if self.filtered.is_empty() {
                Vec::new()
            } else {
                let start = range.start;
                let end = std::cmp::min(range.end, self.filtered.len());
                self.filtered[start..end]
                    .iter()
                    .map(|i| &self.items[*i])
                    .collect()
            }
        }
    }
}

#[derive(Clone, PartialEq)]
enum ItemTypeFilter {
    All,
    Article,
    Video,
    PDF,
}

#[derive(PartialEq)]
enum TagSelectionMode {
    Normal,
    Filtering,
}
const SCROLL_STEP: usize = 1; // Number of items to scroll at once

struct App {
    virtual_state: TableState,
    state: TableState,
    items: FilteredItems<PocketItem>,
    longest_item_lens: (u16, u16, u16), // order is (name, address, email)
    scroll_state: ScrollbarState,
    colors: TableColors,
    color_index: usize,
    app_mode: AppMode,
    stats: TotalStats,
    pocket_client: GetPocketSync,
    tag_popup_state: Option<TagPopupState>,
    doc_type_popup_state: Option<DocTypePopupState>,
    selected_tag_filter: Option<String>,
    active_search_filter: Option<String>,
    item_type_filter: ItemTypeFilter,
    domain_filter: Option<String>,
    tag_selection_mode: TagSelectionMode,
    scroll_accumulator: f32,
    last_click_time: Option<std::time::Instant>,
    last_click_position: Option<(u16, u16)>,
    domain_stats_popup_state: Option<DomainStatsPopupState>,
    help_popup_state: Option<HelpPopupState>,
    rss_feed_popup_state: Option<RssFeedPopupState>,
    download_client: Client,
    cached_tags: Vec<String>,
    rss_feed_state: RssFeedState,
}

impl App {
    fn new(data_vec: Vec<PocketItem>, pocket_client: GetPocketSync, stats: TotalStats) -> App {
        let cached_tags = data_vec
            .iter()
            .flat_map(|item| item.tags().map(|tag| tag.to_string()))
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        App {
            virtual_state: TableState::default().with_selected(0),
            state: TableState::default().with_selected(0),
            longest_item_lens: constraint_len_calculator(&data_vec),
            // scroll_state: ScrollbarState::new((data_vec.len() - 1) * ITEM_HEIGHT),
            scroll_state: ScrollbarState::new(1), //todo: fix this
            colors: TableColors::new(&PALETTES[0]),
            color_index: 0,
            items: FilteredItems::<PocketItem>::non_archived(data_vec),
            app_mode: AppMode::Initialize,
            pocket_client,
            stats,
            tag_popup_state: None,
            doc_type_popup_state: None,
            selected_tag_filter: None,
            active_search_filter: None,
            item_type_filter: ItemTypeFilter::All,
            domain_filter: None,
            tag_selection_mode: TagSelectionMode::Normal,
            scroll_accumulator: 0.0,
            last_click_time: None,
            last_click_position: None,
            domain_stats_popup_state: None,
            help_popup_state: None,
            download_client: Client::new(),
            rss_feed_popup_state: None,
            cached_tags,
            rss_feed_state: RssFeedState::new(),
        }
    }

    fn handle_neovim_edit(&mut self) -> anyhow::Result<Option<String>> {
        // Create a temporary file
        let temp_path = format!("/tmp/pocket_tui_{}.txt", std::process::id());
        File::create(&temp_path)?;

        // Save terminal state and switch to normal mode for neovim
        disable_raw_mode()?;
        execute!(io::stdout(), LeaveAlternateScreen)?;

        // Launch neovim
        let status = std::process::Command::new("nvim")
            .arg(&temp_path)
            .status()
            .context("Failed to start neovim")?;

        // Restore terminal state for Ratatui
        enable_raw_mode()?;
        execute!(
            io::stdout(),
            EnterAlternateScreen,
            EnableMouseCapture,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )?;

        let result = if status.success() {
            let content = fs::read_to_string(&temp_path)?;
            fs::remove_file(&temp_path)?;
            Ok(Some(content))
        } else {
            Ok(None)
        };

        // Clean up temp file if it still exists
        if Path::new(&temp_path).exists() {
            fs::remove_file(&temp_path)?;
        }

        // Queue a redraw of the UI
        crossterm::queue!(
            io::stdout(),
            crossterm::terminal::Clear(crossterm::terminal::ClearType::All)
        )?;
        io::stdout().flush()?;

        result
    }

    //// ------- tmux based popup. working but requires tmux
    // fn handle_neovim_edit(&mut self) -> anyhow::Result<Option<String>> {
    //     if !self.is_inside_tmux() {
    //         return Err(anyhow::anyhow!("Must be running inside tmux session"));
    //     }

    //     // Create a temporary file
    //     let temp_path = format!("/tmp/pocket_tui_{}.txt", std::process::id());
    //     File::create(&temp_path)?;

    //     // Calculate dimensions for the popup (80% of terminal size)
    //     let terminal_size = crossterm::terminal::size()?;
    //     let width = (terminal_size.0 as f32 * 0.8) as u16;
    //     let height = (terminal_size.1 as f32 * 0.8) as u16;
    //     let x = (terminal_size.0 - width) / 2;
    //     let y = (terminal_size.1 - height) / 2;

    //     // Launch tmux popup with neovim without disturbing current terminal
    //     let tmux_cmd = format!(
    //         "tmux popup -E -d '{}' -w {} -h {} -x {} -y {} 'nvim {}'",
    //         std::env::current_dir()?.display(),
    //         width,
    //         height,
    //         x,
    //         y,
    //         temp_path
    //     );

    //     let output = std::process::Command::new("sh")
    //         .arg("-c")
    //         .arg(&tmux_cmd)
    //         .output()
    //         .context("Failed to start tmux popup with neovim")?;

    //     let result = if output.status.success() {
    //         // Read the content after editing
    //         let content = fs::read_to_string(&temp_path)?;
    //         fs::remove_file(&temp_path)?;
    //         Ok(Some(content))
    //     } else {
    //         Ok(None)
    //     };

    //     // Clean up temp file if it still exists
    //     if Path::new(&temp_path).exists() {
    //         fs::remove_file(&temp_path)?;
    //     }

    //     result
    // }

    fn is_tmux_available() -> bool {
        std::process::Command::new("tmux")
            .arg("-V")
            .output()
            .is_ok()
    }

    fn is_inside_tmux(&self) -> bool {
        std::env::var("TMUX").is_ok()
    }

    pub fn start_rss_feed_loading(&mut self) -> anyhow::Result<()> {
        let subscription_manager = RssManager::new();
        let feeds = subscription_manager.load_subscriptions()?;
        if feeds.is_empty() {
            return Ok(());
        }

        if let Ok(mut is_loading) = self.rss_feed_state.is_loading.lock() {
            if *is_loading {
                return Ok(());
            } else {
                *is_loading = true;
            }
        }

        let client = reqwest::blocking::ClientBuilder::new()
            .timeout(Duration::from_secs(10))
            .build()?;

        let items_arc = self.rss_feed_state.items.clone();
        let hidden_items = prss::hidden_items::HiddenItems::load()?;
        let is_loading_arc = self.rss_feed_state.is_loading.clone();
        thread::spawn(move || {
            let results = Arc::new(Mutex::new(Vec::new()));

            feeds.par_iter().for_each(|url| {
                match RssManager::fetch_and_parse_feed(&client, url) {
                    Ok(items) => {
                        if let Ok(mut results_guard) = results.lock() {
                            results_guard.extend(items);
                        }
                    }
                    Err(e) => error!("Error fetching {}: {}", url, e),
                }
                thread::sleep(Duration::from_millis(100));
            });

            if let Ok(mut items_guard) = items_arc.lock() {
                if let Ok(results_guard) = results.lock() {
                    // Filter out hidden items
                    let new_items: Vec<RssFeedItem> = results_guard
                        .iter()
                        .filter(|item| !hidden_items.is_hidden(&item.item_id))
                        .cloned()
                        .collect();
                    *items_guard = new_items;

                    if let Ok(mut is_loading) = is_loading_arc.lock() {
                        *is_loading = false;
                    } else {
                        panic!("is_loading lock error"); //todo
                    }
                }
            }
        });

        Ok(())
    }
    pub fn close_rss_feed_popup(&mut self) -> anyhow::Result<()> {
        if let Some(popup_state) = &self.rss_feed_popup_state {
            // Check if any changes were made
            if popup_state.changes_made {
                // Switch to refreshing mode with proper loading message
                self.app_mode = AppMode::Refreshing(RefreshingPopup::new(
                    "Refreshing Pocket data ⏳".to_string(),
                    LoadingType::Refresh,
                ));

                // Mark RSS items as processed
                self.rss_feed_state.mark_items_processed();
            }

            // Start a new RSS feed check in the background
            self.start_rss_feed_loading()?;
        }

        // Clear the popup state
        self.rss_feed_popup_state = None;
        Ok(())
    }
    fn switch_to_tags_mode(&mut self, initial_tags: Option<String>) {
        self.app_mode = AppMode::CommandEnter(CommandEnterMode::new(
            "Enter tags (comma separated): ".to_string(),
            initial_tags.unwrap_or_default(),
            CommandType::Tags,
        ));
    }
    fn process_add_to_pocket_with_tags(&mut self) -> anyhow::Result<()> {
        if let Some(popup_state) = &mut self.rss_feed_popup_state {
            if let Some(_item) = popup_state.prepare_add_to_pocket() {
                self.switch_to_tags_mode(None);
            }
        }
        Ok(())
    }
    fn switch_to_edit_tags_mode(&mut self) {
        if let Some(idx) = self.virtual_state.selected() {
            if let Some(item) = self.items.get(idx) {
                // Get current tags and join them with commas
                let current_tags = item.tags().join(", ");
                self.switch_to_tags_mode(Some(current_tags));
            }
        }
    }

    fn complete_add_to_pocket(&mut self, tags: String) -> anyhow::Result<()> {
        if let Some(popup_state) = &mut self.rss_feed_popup_state {
            if let Err(e) = popup_state.add_current_to_pocket(&self.pocket_client, &tags) {
                popup_state.set_status(format!("Error: {}", e));
            }
        }
        Ok(())
    }

    fn update_tags(&mut self, tags: String) -> anyhow::Result<()> {
        // Handle RSS item tags
        if let Some(popup_state) = &mut self.rss_feed_popup_state {
            popup_state.add_current_to_pocket(&self.pocket_client, &tags)?;
            return Ok(());
        }

        // Handle pocket item tags
        if let Some(idx) = self.virtual_state.selected() {
            if let Some(item) = self.items.get_mut(idx) {
                let item_id = item.id().parse::<usize>()?;

                // Parse the new tags
                let new_tag_set: Vec<String> = tags
                    .split(',')
                    .map(|t| t.trim().to_string())
                    .filter(|t| !t.is_empty())
                    .collect();

                // Update tags in Pocket
                self.pocket_client.update_tags(item_id, &new_tag_set)?;

                // Update local item
                // First, remove all existing tags
                let existing_tags: Vec<String> = item.tags().map(|t| t.to_string()).collect();
                for tag in existing_tags {
                    item.remove_tag(&tag);
                }

                // Then add the new tags
                for tag in new_tag_set {
                    item.add_tag(&tag);
                }
            }
        }
        Ok(())
    }

    fn download_current_pdf(&mut self) -> anyhow::Result<()> {
        if let Some(idx) = self.virtual_state.selected() {
            if let Some(item) = self.items.get(idx) {
                if item.item_type() == "pdf" {
                    // Create pdfs directory if it doesn't exist
                    fs::create_dir_all("pdfs")?;

                    // Extract filename from URL
                    let url = item.url();
                    let filename = url
                        .split('/')
                        .last()
                        .unwrap_or("download.pdf")
                        .replace("%20", "_");

                    // Construct full path
                    let mut path = std::path::PathBuf::from("pdfs");
                    path.push(&filename);

                    // Download the file in a separate thread
                    let download_url = url.to_string();
                    let path_clone = path.clone();
                    let client = self.download_client.clone();

                    // thread::spawn(move || -> anyhow::Result<()> {
                    let response = client.get(&download_url).send()?;
                    let content = response.bytes()?;
                    std::fs::write(path_clone, content)?;
                    //
                    self.pocket_client
                        .mark_as_downloaded(item.id().parse::<usize>()?)?;

                    let pdf_info = utils::extract_pdf_title(path.as_path())?;
                    if let Some(title) = pdf_info.and_then(|info| info.title) {
                        self.rename_current_item(title)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn download_and_convert_article(&mut self) -> anyhow::Result<()> {
        if let Some(idx) = self.virtual_state.selected() {
            if let Some(item) = self.items.get(idx) {
                if item.item_type() == "article" {
                    // Create articles directory if it doesn't exist
                    fs::create_dir_all("articles")?;

                    // Create sanitized filename from title
                    // let title = item.title();
                    // let filename = sanitize_filename::sanitize(title); //sanitazie_filename might be redundant dependency
                    let filename = item.item_id.clone();
                    let filename = if filename.is_empty() {
                        "untitled".to_string()
                    } else {
                        filename
                    };
                    let path = Path::new("articles").join(format!("{}.md", filename));

                    // Download the article content
                    let response = self.download_client
                                        .get(item.url())
                                        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/121.0.0.0 Safari/537.36")
                                        .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8")
                                        .header("Accept-Language", "en-US,en;q=0.5")
                                        .header("Connection", "keep-alive")
                                        .header("Upgrade-Insecure-Requests", "1")
                                        .header("Sec-Fetch-Dest", "document")
                                        .header("Sec-Fetch-Mode", "navigate")
                                        .header("Sec-Fetch-Site", "none")
                                        .header("Sec-Fetch-User", "?1")
                                        .send()?;
                    let status = response.status();
                    let html_content = response
                        .text()
                        .unwrap_or_else(|_| "No response body".to_string());
                    if !status.is_success() {
                        return Err(anyhow::anyhow!(
                            "Failed to download article: HTTP {} - {}",
                            status,
                            html_content
                        ));
                    }
                    let md = html2md::rewrite_html(&html_content, true);

                    // Configure and parse with dom_smoothie
                    let cfg = Config {
                        max_elements_to_parse: 9000,
                        text_mode: dom_smoothie::TextMode::Formatted,
                        ..Default::default()
                    };

                    let mut readability =
                        Readability::new(html_content.as_str(), Some(item.url()), Some(cfg))?;
                    // Readability::new(md.as_str(), Some(item.url()), Some(cfg))?;
                    let article: Article = readability.parse()?;

                    // Create markdown content with metadata and article details
                    let mut content = String::new();

                    // Add YAML frontmatter
                    // content.push_str("---\n");
                    // content.push_str(&format!("title: {}\n", article.title));
                    // content.push_str(&format!("url: {}\n", item.url()));
                    // content.push_str(&format!("date_added: {}\n", item.date()));

                    // // Add optional metadata if available
                    // if let Some(byline) = article.byline {
                    //     content.push_str(&format!("author: {}\n", byline));
                    // }
                    // if let Some(site_name) = article.site_name {
                    //     content.push_str(&format!("site_name: {}\n", site_name));
                    // }
                    // if let Some(published_time) = article.published_time {
                    //     content.push_str(&format!("published_time: {}\n", published_time));
                    // }
                    // if let Some(modified_time) = article.modified_time {
                    //     content.push_str(&format!("modified_time: {}\n", modified_time));
                    // }
                    // if let Some(excerpt) = article.excerpt {
                    //     content.push_str(&format!("excerpt: {}\n", excerpt));
                    // }
                    // content.push_str("---\n\n");

                    // Add article content
                    let result = markdown::normalize_markdown(&md, &article.text_content);
                    content.push_str(&article.text_content);
                    content.push_str("--------\n\n");
                    content.push_str(&md);
                    content.push_str("--------\n\n");
                    content.push_str(&result);

                    // Save to file
                    fs::write(&path, content)?;

                    // Mark as downloaded in Pocket
                    self.pocket_client
                        .mark_as_downloaded(item.id().parse::<usize>()?)?;
                }
            }
        }
        Ok(())
    }

    // /// Checks if a line is a markdown header
    // fn is_header(line: &str) -> bool {
    //     line.trim_start().starts_with('#')
    // }

    // /// Checks if a line should stay attached to the previous line
    // fn should_stay_attached(line: &str) -> bool {
    //     // Headers should be followed by their content
    //     Self::is_header(line) ||
    //     // List items should stay together
    //     line.trim_start().starts_with('*') ||
    //     line.trim_start().starts_with('-') ||
    //     line.trim_start().starts_with(|c: char| c.is_ascii_digit() && line.contains(". ")) ||
    //     // Code blocks should stay together
    //     line.trim_start().starts_with('`') ||
    //     // Continuation of a sentence (no capital letter at start)
    //     (!line.trim_start().is_empty() &&
    //      !Self::is_header(line) &&
    //      line.trim_start().chars().next()
    //          .map(|c| !c.is_uppercase())
    //          .unwrap_or(false))
    // }

    // /// Normalizes markdown content by:
    // /// 1. Removing preamble/postamble content not present in plain text
    // /// 2. Restoring proper paragraph separation while preserving markdown formatting
    // pub fn normalize_markdown(markdown: &str, plain: &str) -> String {
    //     // First, find the start of actual content
    //     let first_plain_para = plain.split("\n\n").next().unwrap_or("").trim();

    //     let markdown_lines: Vec<&str> = markdown.lines().collect();
    //     let mut start_idx = 0;

    //     // Find content start
    //     for (i, window) in markdown_lines.windows(3).enumerate() {
    //         let combined = window.join(" ");
    //         if combined.contains(first_plain_para) {
    //             start_idx = i;
    //             break;
    //         }
    //     }

    //     // Find content end
    //     let mut end_idx = markdown_lines.len();
    //     for (i, line) in markdown_lines.iter().enumerate().rev() {
    //         if line.contains("## Related posts")
    //             || line.contains("Blog Comments")
    //             || line.contains("Contents")
    //         {
    //             end_idx = i;
    //             break;
    //         }
    //     }

    //     // Process content while preserving markdown formatting
    //     let mut result = Vec::new();
    //     let mut current_group = Vec::new();

    //     for (i, line) in markdown_lines[start_idx..end_idx].iter().enumerate() {
    //         let trimmed = line.trim();
    //         if trimmed.is_empty() {
    //             if !current_group.is_empty() {
    //                 result.push(current_group.join("\n"));
    //                 current_group.clear();
    //             }
    //             continue;
    //         }

    //         // Check if this line should be kept with the previous content
    //         if i > 0 && Self::should_stay_attached(trimmed) {
    //             current_group.push(trimmed);
    //         } else {
    //             if !current_group.is_empty() {
    //                 result.push(current_group.join("\n"));
    //                 current_group.clear();
    //             }
    //             current_group.push(trimmed);
    //         }
    //     }

    //     // Add final group if any
    //     if !current_group.is_empty() {
    //         result.push(current_group.join("\n"));
    //     }

    //     // Join paragraphs with double newlines
    //     let content = result
    //         .into_iter()
    //         .filter(|p| !p.is_empty())
    //         .collect::<Vec<_>>()
    //         .join("\n\n");

    //     // Clean up the final string while preserving markdown structure
    //     content
    //         .split("\n\n")
    //         .map(|para| para.trim())
    //         .filter(|para| !para.is_empty())
    //         .collect::<Vec<_>>()
    //         .join("\n\n")
    // }

    pub fn show_rss_feed_popup(&mut self) -> anyhow::Result<()> {
        if let Ok(is_loading) = self.rss_feed_state.is_loading.lock() {
            if (*is_loading) {
                self.app_mode = AppMode::Error("RSS feed is being updated.".to_string());
                return Ok(());
            }
        }
        if let Ok(items_guard) = self.rss_feed_state.items.lock() {
            if items_guard.is_empty() {
                self.app_mode = AppMode::Error("No RSS updates available (yet)".to_string());
                return Ok(());
            }
        }
        let visible_items = 33;
        let items = if let Ok(items_guard) = self.rss_feed_state.items.lock() {
            items_guard.to_vec()
        } else {
            Vec::new()
        };

        // Create popup state with current items
        self.rss_feed_popup_state = Some(RssFeedPopupState::new(items, visible_items)?);

        // If we need to refresh the items, do it in the background
        if !self.rss_feed_state.items_processed {
            self.start_rss_feed_loading()?;
        }

        Ok(())
    }

    pub fn handle_rss_feed_selection(&mut self) -> anyhow::Result<()> {
        if let Some(popup_state) = &self.rss_feed_popup_state {
            if let Some(selected_item) = popup_state.items.get(popup_state.selected_index) {
                if !selected_item.link.is_empty() {
                    webbrowser::open(&selected_item.link)
                        .context("Failed to open link in browser")?;
                }
            }
        }
        // self.rss_feed_popup_state = None;
        Ok(())
    }
    fn show_help_popup(&mut self) -> anyhow::Result<()> {
        let content = fs::read_to_string("help.txt")?;
        self.help_popup_state = Some(HelpPopupState { content });
        Ok(())
    }

    fn refresh_data(&mut self) -> anyhow::Result<()> {
        let delta_file = Path::new("snapshot_updates.db");
        let mut stats = TotalStats::new();
        let items = reload_data(delta_file, &self.pocket_client, &mut stats)?;
        self.cached_tags = items
            .iter()
            .flat_map(|item| item.tags().map(|tag| tag.to_string()))
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        self.stats = stats;
        self.items = FilteredItems::<PocketItem>::non_archived(items);
        self.apply_filter();
        Ok(())
    }

    fn show_tag_popup(&mut self) {
        let tag_counts: Vec<(String, usize)> = self
            .items
            .iter()
            .filter(|item| {
                !item.tags().any(|tag| tag == "read") // Exclude read items
                                                      // item.favorite != "1" // Exclude favorited items
            })
            .flat_map(|item| item.tags().map(|tag| tag.to_string()))
            .fold(std::collections::HashMap::new(), |mut acc, tag| {
                *acc.entry(tag).or_insert(0) += 1;
                acc
            })
            .into_iter()
            .collect();

        let mut sorted_tag_counts = tag_counts;
        sorted_tag_counts.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1))); // sort by alfabet then by counts

        let visible_items = 26; // Adjust this value based on your UI
        self.tag_popup_state = Some(TagPopupState::new(sorted_tag_counts, visible_items));
        self.tag_selection_mode = TagSelectionMode::Normal;
    }

    fn show_domain_stats(&mut self) {
        // Create a hashmap to store domain/author counts
        let mut counts = std::collections::HashMap::new();

        // Count domains/authors for each item
        for item in self.items.iter() {
            let key = if item.item_type() == "video" || item.url().contains("medium") {
                // For videos, use author IDs if available
                match &item.authors {
                    Some(authors) if !authors.is_empty() => authors.join(", "),
                    _ => "IGNORE".to_string(),
                }
            } else {
                // For non-videos, use domain
                Self::extract_domain(item.url()).unwrap_or_else(|| "IGNORE".to_string())
            };
            if key != "IGNORE" {
                *counts.entry(key).or_insert(0) += 1;
            }
        }

        // Convert to vector and sort by count (descending)
        let mut stats: Vec<(String, usize)> = counts.into_iter().collect();
        stats.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));

        // Take top 20
        stats.truncate(40);

        let visible_items = 23; //todo: this needs to be figoured out based on popup size.
        self.domain_stats_popup_state = Some(DomainStatsPopupState::new(stats, visible_items));
    }

    pub fn apply_filter(&mut self) {
        self.items.apply_filter(|item| {
            let title_matches = match &self.active_search_filter {
                Some(filter) => {
                    let filter_lower = filter.to_lowercase();
                    item.title().to_lowercase().contains(&filter_lower)
                        || item.url().contains(&filter_lower)
                }
                None => true,
            };

            let tag_matches = match &self.selected_tag_filter {
                Some(tag) => item.tags().any(|t| t == tag),
                None => true,
            };

            let type_matches = match self.item_type_filter {
                ItemTypeFilter::All => true,
                ItemTypeFilter::Article => item.item_type() == "article",
                ItemTypeFilter::Video => item.item_type() == "video",
                ItemTypeFilter::PDF => item.item_type() == "pdf",
            };

            let domain_matches = match &self.domain_filter {
                Some(domain) => Self::extract_domain(item.url())
                    .map(|item_domain| item_domain == *domain)
                    .unwrap_or(false),
                None => true,
            };

            title_matches && tag_matches && type_matches && domain_matches
        });
        self.virtual_state.select(Some(0));
        *self.virtual_state.offset_mut() = 0;
    }

    fn show_doc_type_popup(&mut self) {
        self.doc_type_popup_state = Some(DocTypePopupState::new());
    }

    fn select_doc_type(&mut self, filter: ItemTypeFilter) {
        self.doc_type_popup_state = None;
        if self.item_type_filter != filter {
            self.item_type_filter = filter;
            self.apply_filter();
        }
    }

    fn set_item_type_filter(&mut self, filter: ItemTypeFilter) {
        self.item_type_filter = filter;
        self.apply_filter();
    }

    fn select_tag(&mut self) {
        if let Some(tag_popup_state) = &self.tag_popup_state {
            if let Some((selected_tag, _)) = tag_popup_state
                .filtered_tags
                .get(tag_popup_state.selected_index)
            {
                self.selected_tag_filter = Some(selected_tag.clone());
                self.tag_popup_state = None;
                self.apply_filter();
            }
        }
    }

    fn clear_tag_filter(&mut self) {
        self.selected_tag_filter = None;
        self.apply_filter();
    }

    fn set_search_filter(&mut self, filter: String) {
        self.active_search_filter = Some(filter);
        self.apply_filter();
    }

    fn clear_search_filter(&mut self) {
        self.active_search_filter = None;
        self.apply_filter();
    }

    fn clear_all_filters(&mut self) {
        self.active_search_filter = None;
        self.selected_tag_filter = None;
        self.domain_filter = None;
        self.items.clear_filter();
    }

    fn extract_domain(url: &str) -> Option<String> {
        let url = url
            .trim_start_matches("http://")
            .trim_start_matches("https://")
            .trim_start_matches("www.");

        url.split('/').next().map(|s| s.to_string())
    }

    fn filter_by_video_authors(&mut self, target_authors: &[String]) {
        self.items.apply_filter(|item| {
            if item.item_type() == "video" {
                // For videos, check if any authors match
                if let Some(item_authors) = &item.authors {
                    item_authors
                        .iter()
                        .any(|author| target_authors.iter().any(|target| author.contains(target)))
                } else {
                    false
                }
            } else {
                false
            }
        });
        self.virtual_state.select(Some(0));
        *self.virtual_state.offset_mut() = 0;
    }
    fn filter_by_current_domain(&mut self) -> anyhow::Result<()> {
        if let Some(idx) = self.virtual_state.selected() {
            if let Some(item) = self.items.get(idx).cloned() {
                if item.item_type() == "video" {
                    // For videos, use authors as the filter criteria
                    match &item.authors {
                        Some(authors) if !authors.is_empty() => {
                            // Use authors as filter
                            self.domain_filter = Some(authors.join(", "));
                            self.filter_by_video_authors(authors);
                        }
                        _ => {
                            // No authors available
                            self.domain_filter = Some("N/A".to_string());
                            self.apply_filter();
                        }
                    }
                } else {
                    // Regular domain filtering for non-video content
                    if let Some(domain) = Self::extract_domain(item.url()) {
                        self.domain_filter = Some(domain);
                        self.apply_filter();
                    }
                }
            }
        }
        Ok(())
    }

    fn _apply_video_author_filter(&mut self, target_authors: &[String]) {
        self.items.apply_filter(|item| {
            if item.item_type() == "video" {
                // For videos, check if any authors match
                if let Some(item_authors) = &item.authors {
                    item_authors
                        .iter()
                        .any(|author| target_authors.contains(author))
                } else {
                    false
                }
            } else {
                // Non-video items don't match when filtering by video author
                false
            }
        });
    }

    fn clear_domain_filter(&mut self) {
        self.domain_filter = None;
        self.apply_filter();
    }
    pub fn next(&mut self) {
        let i = match self.virtual_state.selected() {
            Some(i) => {
                if i < self.items.len() - 1 {
                    i + 1
                } else {
                    self.items.len() - 1
                }
            }
            None => 0,
        };
        self.virtual_state.select(Some(i));
        self.scroll_state = self.scroll_state.position(i * ITEM_HEIGHT);
    }

    pub fn previous(&mut self) {
        let i = match self.virtual_state.selected() {
            Some(i) => {
                if i > 0 {
                    i - 1
                } else {
                    0
                }
            }
            None => 0,
        };
        self.virtual_state.select(Some(i));
        if i < self.virtual_state.offset() {
            *self.virtual_state.offset_mut() = i
        }
        self.scroll_state = self.scroll_state.position(i * ITEM_HEIGHT);
    }

    pub fn set_colors(&mut self) {
        self.colors = TableColors::new(&PALETTES[self.color_index]);
    }

    fn open_current_url(&mut self) -> anyhow::Result<()> {
        if let Some(idx) = self.virtual_state.selected() {
            if let Some(item) = self.items.get_mut(idx) {
                self.pocket_client
                    .mark_as_read(item.id().parse::<usize>()?)?;
                item.add_tag("read");
                webbrowser::open(&item.url()).context("Failed to open link in a browser")?;
            }
        }
        Ok(())
    }

    //todo: usize conversion is dumb
    fn delete_article(&mut self) -> anyhow::Result<()> {
        if let Some(idx) = self.virtual_state.selected() {
            if let Some(item) = self.items.get(idx) {
                self.pocket_client.delete(item.id().parse::<usize>()?)?;

                // Log the deletion in the storage.delta
                let delta_record = storage::PocketItemUpdate::Delete {
                    item_id: item.id(),
                    timestamp: Some(Utc::now().timestamp().try_into().unwrap()),
                };
                let delta_file = Path::new("snapshot_updates.db");
                // this is needed to enrich delete event with timestamp. looks like pocket api erases this info
                storage::append_delete_to_delta(delta_file, &delta_record)?;
            }
            self.items.remove(idx);
        }
        Ok(())
    }

    fn toggle_top_tag(&mut self) -> anyhow::Result<()> {
        if let Some(idx) = self.virtual_state.selected() {
            if let Some(item) = self.items.get_mut(idx) {
                if !item.tags().any(|x| x == "top") {
                    self.pocket_client
                        .mark_as_top(item.id().parse::<usize>()?)?;
                    item.add_tag("top");
                } else {
                    self.pocket_client
                        .unmark_as_top(item.id().parse::<usize>()?)?;
                    item.remove_tag("top");
                }
            }
        }
        Ok(())
    }

    fn fav_and_archive_article(&mut self) -> anyhow::Result<()> {
        if let Some(idx) = self.virtual_state.selected() {
            if let Some(item) = self.items.get(idx) {
                self.pocket_client
                    .fav_and_archive(item.id().parse::<usize>()?)?;
            }
            self.items.remove(idx);
        }
        Ok(())
    }

    fn switch_to_search_mode(&mut self) {
        self.app_mode = AppMode::Search(SearchMode::new((
            self.virtual_state.offset(),
            self.virtual_state.selected().unwrap(),
        )));
    }

    fn switch_to_confirmation(&mut self, confirm_type: Confirmation) {
        self.app_mode = AppMode::Confirmation(confirm_type)
    }

    fn switch_to_normal_mode(&mut self) {
        self.app_mode = AppMode::Normal;
    }

    fn switch_to_normal_mode_from(&mut self, from: AppMode) {
        self.app_mode = AppMode::Normal;
        match from {
            AppMode::Search(x) => {
                self.apply_filter();
                *self.virtual_state.offset_mut() = x.normal_mode_positions.0;
                self.virtual_state.select(Some(x.normal_mode_positions.1));
            }
            _ => {} // do nothing
        }
    }

    fn scroll_down(&mut self) {
        let page_size = 13;
        let i = match self.virtual_state.selected() {
            Some(i) => {
                if (i + page_size) > self.items.len() - 1 {
                    (i + page_size) % self.items.len()
                } else {
                    i + page_size
                }
            }
            None => 0,
        };
        if self.virtual_state.offset() < self.virtual_state.selected().unwrap_or(0) {
            *self.virtual_state.offset_mut() = self.virtual_state.selected().unwrap_or(0);
        } else {
            self.virtual_state.select(Some(i));
            *self.virtual_state.offset_mut() = i;
        }
    }

    fn scroll_up(&mut self) {
        let page_size = 13;
        let i = match self.virtual_state.selected() {
            Some(i) => {
                if i > page_size {
                    i - page_size
                } else {
                    0
                }
            }
            None => 0,
        };
        if self.virtual_state.offset() < self.virtual_state.selected().unwrap_or(0) {
            self.virtual_state.select(Some(self.virtual_state.offset()));
        } else {
            self.virtual_state.select(Some(i));
            *self.virtual_state.offset_mut() = i;
        }
    }

    fn scroll_to_end(&mut self) {
        self.virtual_state.select(Some(self.items.len() - 1));
    }

    fn scroll_to_begining(&mut self) {
        self.virtual_state.select(Some(0));
        *self.virtual_state.offset_mut() = 0;
    }

    fn switch_to_rename_mode(&mut self, with_current_title: bool) {
        if let Some(idx) = self.virtual_state.selected() {
            let initial_text = if with_current_title {
                self.items.get(idx).map_or("".to_string(), |item| {
                    if item.title().is_empty() {
                        item.url().to_string()
                    } else {
                        item.title().to_string()
                    }
                })
            } else {
                String::new()
            };

            self.app_mode = AppMode::CommandEnter(CommandEnterMode::new(
                "Rename to (control+v to paste): ".to_string(),
                initial_text.clone(),
                CommandType::RenameItem,
            ));
        }
    }

    fn rename_current_item(&mut self, current_enter: String) -> anyhow::Result<()> {
        if let Some(idx) = self.virtual_state.selected() {
            if let Some(item) = self.items.get_mut(idx) {
                let normalized_title = current_enter.replace('\n', " ").trim().to_string();
                self.pocket_client.rename(
                    item.id().parse::<usize>()?,
                    item.url(),
                    &normalized_title,
                    item.time_added(),
                )?;
                item.rename_title_to(current_enter);
            }
        }
        Ok(())
    }

    fn jump_to_date(&mut self, current_enter: String) -> anyhow::Result<()> {
        match self
            .items
            .iter()
            .enumerate()
            .find(|(_, data)| &data.date() <= &current_enter)
        {
            Some((idx, _)) => {
                self.virtual_state.select(Some(idx));
                *self.virtual_state.offset_mut() = idx;
                self.scroll_state = self.scroll_state.position(idx * ITEM_HEIGHT);
            }
            None => {} /*do nothing*/
        }
        Ok(())
    }

    fn handle_mouse_event(&mut self, mouse_event: MouseEvent) -> anyhow::Result<()> {
        match mouse_event.kind {
            MouseEventKind::Down(event::MouseButton::Left) => {
                let current_time = std::time::Instant::now();
                let current_position = (mouse_event.column, mouse_event.row);

                if let (Some(last_time), Some(last_position)) =
                    (self.last_click_time, self.last_click_position)
                {
                    if current_time.duration_since(last_time) < Duration::from_millis(500)
                        && current_position == last_position
                    {
                        // Double click detected
                        self.open_current_url()?;
                    }
                }

                self.last_click_time = Some(current_time);
                self.last_click_position = Some(current_position);

                // Calculate the clicked row index
                let clicked_row = (mouse_event.row as usize).saturating_sub(1) / ITEM_HEIGHT
                    + self.virtual_state.offset();
                if clicked_row < self.items.len() {
                    self.virtual_state.select(Some(clicked_row));
                    self.scroll_state = self.scroll_state.position(clicked_row * ITEM_HEIGHT);
                }
            }
            MouseEventKind::ScrollDown => self.scroll(0.2),
            MouseEventKind::ScrollUp => self.scroll(-0.2),
            _ => {}
        }
        Ok(())
    }
    fn scroll(&mut self, delta: f32) {
        self.scroll_accumulator += delta;

        while self.scroll_accumulator >= 1.0 {
            // self.next();
            self.mousescroll_down();
            self.scroll_accumulator -= 1.0;
        }

        while self.scroll_accumulator <= -1.0 {
            // self.previous();
            self.mousescroll_up();
            self.scroll_accumulator += 1.0;
        }
    }

    fn mousescroll_down(&mut self) {
        let new_index = self
            .virtual_state
            .selected()
            .map(|i| (i + SCROLL_STEP).min(self.items.len() - 1))
            .unwrap_or(0);
        self.virtual_state.select(Some(new_index));
        self.scroll_state = self.scroll_state.position(new_index * ITEM_HEIGHT);
    }

    fn mousescroll_up(&mut self) {
        let new_index = self
            .virtual_state
            .selected()
            .map(|i| i.saturating_sub(SCROLL_STEP))
            .unwrap_or(0);
        self.virtual_state.select(Some(new_index));
        self.scroll_state = self.scroll_state.position(new_index * ITEM_HEIGHT);
    }
}

fn reload_data(
    delta_file: &Path,
    pocket_client: &GetPocketSync,
    stats: &mut TotalStats,
) -> anyhow::Result<Vec<PocketItem>> {
    pocket_client
        .refresh_delta_block(&delta_file)
        .context("failed to refresh delta during refresh")?;

    // Load and process delta updates
    let delta_items = storage::load_delta_pocket_items(&delta_file);
    let mut seen_item_ids = std::collections::HashSet::new();
    let today = Utc::now();

    let pocket_snapshot = storage::load_snapshot_file();
    let mut current_items = pocket_snapshot.pocket_items();

    // Process each delta update
    for update in delta_items {
        match update {
            PocketItemUpdate::Delete {
                item_id,
                timestamp: ts_opt,
            } => {
                if let Some(ts) = ts_opt {
                    if let Some(item) = current_items.get(&item_id) {
                        if !seen_item_ids.contains(&item_id) {
                            stats.track_as(item, &today, true, ts as i64);
                            seen_item_ids.insert(item_id.clone());
                        }
                    }
                }
                current_items.remove(&item_id);
            }
            PocketItemUpdate::Add {
                item_id: id,
                data: mut new_item,
            } => {
                if let Some(existing) = current_items.get(&id) {
                    // Update existing item
                    new_item.time_added = existing.time_added().to_string();
                    let ts: i64 = new_item.time_updated.parse::<i64>().unwrap_or(0);
                    if new_item.favorite == "1" && !seen_item_ids.contains(&id) {
                        stats.track_as(existing, &today, true, ts);
                        seen_item_ids.insert(id.clone());
                    }
                    current_items.insert(id, new_item.into()); // Assuming T can be created from PocketItem
                } else {
                    // Add new item
                    stats.track_item(&new_item, &today);
                    current_items.insert(id, new_item.into());
                }
            }
        }
    }

    // Convert back to a sorted vector
    let items: Vec<PocketItem> = current_items
        .into_values()
        .filter(|a| a.tags().all(|tag| tag != "favorite")) // Skip favorited items
        .sorted_by(|a, b| b.time_added.partial_cmp(&a.time_added).unwrap())
        .collect();

    return Ok(items);
}

fn main() -> Result<(), Box<dyn Error>> {
    let target = Box::new(File::create("log.txt").expect("Can't create file"));

    let token_opt = tokenstorage::UserTokenStorage::get_token()?;
    let token = if let Some(t) = token_opt {
        t
    } else {
        println!("Auth information is not found. Starting authentication procedure...");
        thread::sleep(Duration::from_secs(4));
        let pocket_auth = auth::PocketAuth::new()?;
        let auth_token = pocket_auth.authenticate()?;
        tokenstorage::UserTokenStorage::store_token(&auth_token)?;
        auth_token
    };

    let pocket_client = GetPocketSync::new(&token)?;

    if !storage::snapshot_exists() {
        // let animation = vec!["|", "/", "-", "\\"];
        // let mut animation_index = 0;
        // let running = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        // let running_clone = running.clone();
        // let animation_handle = thread::spawn(move || {
        //     while running_clone.load(std::sync::atomic::Ordering::SeqCst) {
        //         print!(
        //             "\rRetrieving snapshot data from pocket. This might take time... {}",
        //             animation[animation_index]
        //         );
        //         io::stdout().flush().unwrap();
        //         thread::sleep(Duration::from_millis(100));
        //         animation_index = (animation_index + 1) % animation.len();
        //     }
        // });

        println!("\rRetrieving snapshot data from pocket. This might take time... ");
        let snapshot: storage::Pocket = pocket_client.retrieve_all()?;
        storage::save_to_snapshot(&snapshot)?;
        if let Some((item_id, value)) = snapshot.list.iter().max_by_key(|(_id, item)| {
            item.get("time_added")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<i64>().ok())
                .unwrap_or(0)
        }) {
            let delta_file = Path::new(DELTA_FILE);
            let mut map: serde_json::Map<String, serde_json::Value> =
                serde_json::Map::with_capacity(1);
            map.insert(item_id.clone(), value.clone());
            storage::append_to_delta(
                delta_file,
                &storage::Pocket {
                    status: 1,
                    complete: 1,
                    list: map,
                },
            )?;
        } else {
            todo!("Oh no1");
        }
        // running.store(false, std::sync::atomic::Ordering::SeqCst);
        // let _ = animation_handle.join();
    }

    env_logger::Builder::new()
        .target(env_logger::Target::Pipe(target))
        .filter(None, LevelFilter::Trace)
        .format(|buf, record| {
            writeln!(
                buf,
                "({} {} {}:{}) {}",
                Local::now().format("%Y-%m-%d %H:%M:%S%.3f"),
                record.level(),
                record.file().unwrap_or("unknown"),
                record.line().unwrap_or(0),
                record.args()
            )
        })
        .init();

    // setup terminal
    errors::install_hooks()?;
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableMouseCapture,
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    )?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let stats = TotalStats::new();
    let list = Vec::new(); //reload_data(&delta_file, &pocket_client, &mut stats)?;

    let mut app: App = App::new(list, pocket_client, stats);
    app.start_rss_feed_loading()?;
    let res = run_app(&mut terminal, app);

    // restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    if let Err(err) = res {
        println!("{err:?}");
    }

    Ok(())
}

fn run_app<B: Backend>(terminal: &mut Terminal<B>, mut app: App) -> anyhow::Result<()> {
    loop {
        terminal
            .draw(|f| ui(f, &mut app))
            .context("Failed to draw UI")?;
        match &mut app.app_mode {
            AppMode::Initialize => {
                app.refresh_data()?;
                app.app_mode = AppMode::Normal;
            }
            AppMode::Normal => process_input_normal_mode(&mut app)?,
            AppMode::Confirmation(ref confirmation_type) => {
                let ctype = confirmation_type.clone();
                process_confirmation(&mut app, ctype)?
            }

            AppMode::Search(current) => {
                let sstr = current.clone();
                process_search_mode(&mut app, sstr)?
            }
            AppMode::MulticharNormalModeEnter(x) => {
                let cur_state = x.clone();
                process_multichar_enter_mode(&mut app, cur_state)?
            }
            AppMode::CommandEnter(enter) => {
                let cur_state = enter.clone();
                process_command_mode(&mut app, cur_state)?
            }
            AppMode::Refreshing(ref mut pop) => {
                if pop.was_redered {
                    let refresh_result = match pop.refresh_type {
                        LoadingType::Refresh => app.refresh_data(),
                        LoadingType::Download => {
                            if let Some(idx) = app.virtual_state.selected() {
                                if let Some(item) = app.items.get(idx) {
                                    match item.item_type() {
                                        "pdf" => app.download_current_pdf(),
                                        "article" => app.download_and_convert_article(),
                                        _ => Ok(()),
                                    }
                                } else {
                                    Ok(())
                                }
                            } else {
                                Ok(())
                            }
                        }
                    };

                    match refresh_result {
                        Ok(_) => {
                            app.switch_to_normal_mode();
                        }
                        Err(err) => {
                            app.app_mode = AppMode::Error(err.to_string());
                        }
                    }
                } else {
                    pop.was_redered = true;
                }

                // if pop.was_redered {
                //     let refresh_result = match pop.refresh_type {
                //         LoadingType::Refresh => app.refresh_data(),
                //         LoadingType::Download => app.download_current_pdf(),
                //     };

                //     match refresh_result {
                //         Ok(_) => {
                //             app.switch_to_normal_mode();
                //         }
                //         Err(err) => {
                //             app.app_mode = AppMode::Error(err.to_string());
                //         }
                //     }
                // } else {
                //     pop.was_redered = true;
                // }
            }
            AppMode::Error(err) => {
                if let Event::Key(key) = event::read()? {
                    if key.kind == KeyEventKind::Press {
                        if KeyCode::Esc == key.code {
                            app.switch_to_normal_mode();
                        }
                    }
                }
            }
        }
    }
}

fn process_command_mode(app: &mut App, mut cur_state: CommandEnterMode) -> anyhow::Result<()> {
    Ok(if let Event::Key(key) = event::read()? {
        if key.kind == KeyEventKind::Press {
            use KeyCode::*;
            match key.code {
                Esc => app.switch_to_normal_mode(),
                Tab => {
                    if cur_state.complete_suggestion() {
                        app.app_mode = AppMode::CommandEnter(cur_state);
                    }
                }
                Char(ch) => {
                    if (key.modifiers.contains(KeyModifiers::CONTROL)
                        || key.modifiers.contains(KeyModifiers::SUPER))
                        && (ch == 'v' || ch == 'V')
                    {
                        if let Ok(clipboard_content) = cli_clipboard::get_contents() {
                            cur_state.current_enter =
                                clipboard_content.replace('\n', " ").trim().to_string();
                        }
                    } else {
                        // For regular typing, add the character as-is
                        cur_state.current_enter.insert(cur_state.cursor_pos, ch);
                        cur_state.cursor_pos += 1;
                    }
                    cur_state.update_suggestion(&app.cached_tags);

                    app.app_mode = AppMode::CommandEnter(cur_state);

                    // cur_state.current_enter.push(ch);
                    // app.app_mode = AppMode::CommandEnter(cur_state);
                }
                Backspace => {
                    if cur_state.cursor_pos > 0 {
                        cur_state.current_enter.remove(cur_state.cursor_pos - 1);
                        cur_state.cursor_pos -= 1;

                        if let Some(tag_popup_state) = &app.tag_popup_state {
                            cur_state.update_suggestion(
                                &tag_popup_state
                                    .tags
                                    .iter()
                                    .map(|x| x.0.clone())
                                    .collect::<Vec<String>>(),
                            );
                        }
                    }
                    app.app_mode = AppMode::CommandEnter(cur_state);
                }
                Left => {
                    if cur_state.cursor_pos > 0 {
                        cur_state.cursor_pos -= 1;
                        app.app_mode = AppMode::CommandEnter(cur_state);
                    }
                }
                Right => {
                    if cur_state.cursor_pos < cur_state.current_enter.len() {
                        cur_state.cursor_pos += 1;
                        app.app_mode = AppMode::CommandEnter(cur_state);
                    }
                }
                Enter => {
                    match cur_state.command_type {
                        CommandType::RenameItem => {
                            app.rename_current_item(cur_state.current_enter)?
                        }
                        CommandType::JumpToDate => app.jump_to_date(cur_state.current_enter)?,
                        CommandType::Tags => app.update_tags(cur_state.current_enter)?,
                    }
                    app.switch_to_normal_mode();
                }
                _ => {} //do nothing
            }
        }
    })
}

fn process_multichar_enter_mode(app: &mut App, cur_state: String) -> anyhow::Result<()> {
    Ok(
        if let Event::Key(key) = event::read().context("Couldn't read user input")? {
            if key.kind == KeyEventKind::Press {
                use KeyCode::*;
                match (cur_state.as_str(), key.code) {
                    ("g", Char('g')) => {
                        app.switch_to_normal_mode();
                        app.scroll_to_begining();
                    }
                    ("g", Char('d')) => {
                        app.app_mode = AppMode::CommandEnter(CommandEnterMode::new_empty(
                            "Jump to [yyyy-mm-dd]:".to_string(),
                            CommandType::JumpToDate,
                        ));
                    }
                    ("Z", Char('Z')) => {
                        panic!("Exit");
                    }
                    _ => {
                        app.switch_to_normal_mode();
                    }
                }
            }
        },
    )
}

fn process_confirmation(app: &mut App, confirmation_type: Confirmation) -> anyhow::Result<()> {
    Ok(
        if let Event::Key(key) = event::read().context("Couldn't read user input")? {
            if key.kind == KeyEventKind::Press {
                use KeyCode::*;
                match key.code {
                    Char('y') | Char('Y') | Char('d') | Char('D') => {
                        match confirmation_type {
                            Confirmation::DeletePocketItem => app.delete_article()?,
                        };
                    }
                    _ => {} // do nothing
                }
            }
            app.switch_to_normal_mode()
        },
    )
}

fn process_search_mode(app: &mut App, mut sstr: SearchMode) -> anyhow::Result<()> {
    if event::poll(Duration::from_millis(100))? {
        match event::read()? {
            Event::Key(key) => {
                if key.kind == KeyEventKind::Press {
                    use KeyCode::*;
                    match key.code {
                        Esc => {
                            app.clear_all_filters();
                            app.switch_to_normal_mode_from(AppMode::Search(sstr))
                        }
                        Char(ch) => {
                            sstr.search.push(ch);
                            app.active_search_filter = Some(sstr.search.clone());
                            app.app_mode = AppMode::Search(sstr);
                            app.apply_filter();
                        }
                        Backspace => {
                            sstr.search.pop();
                            app.active_search_filter = Some(sstr.search.clone());
                            app.app_mode = AppMode::Search(sstr);
                            app.apply_filter();
                        }
                        Enter => {
                            app.set_search_filter(sstr.search.clone());
                            app.switch_to_normal_mode_from(AppMode::Search(sstr));
                        }
                        Down => app.next(),
                        Up => app.previous(),
                        _ => {} //do nothing
                    }
                }
            }
            Event::Mouse(mouse_event) => {
                app.handle_mouse_event(mouse_event)?;
            }
            _ => {
                // todo: proper logging
                ()
            }
        }
    }
    Ok(())
}

fn process_input_normal_mode(app: &mut App) -> anyhow::Result<()> {
    Ok(if let Event::Key(key) = event::read()? {
        if key.kind == KeyEventKind::Press {
            use KeyCode::*;
            if let Some(doc_popup_state) = &mut app.doc_type_popup_state {
                match key.code {
                    Char(ch) if ch.is_digit(10) => {
                        if let Some(filter) = doc_popup_state.select_by_number(ch) {
                            app.select_doc_type(filter);
                        }
                    }
                    Esc => app.doc_type_popup_state = None,
                    _ => {}
                }
            } else if let Some(tag_popup_state) = &mut app.tag_popup_state {
                match app.tag_selection_mode {
                    TagSelectionMode::Normal => match key.code {
                        Down => tag_popup_state.move_selection(1),
                        Up => tag_popup_state.move_selection(-1),
                        Enter => app.select_tag(),
                        Esc => app.tag_popup_state = None,
                        Char(ch) => {
                            app.tag_selection_mode = TagSelectionMode::Filtering;
                            tag_popup_state.add_to_filter(ch)
                        }
                        _ => {}
                    },
                    TagSelectionMode::Filtering => match key.code {
                        Char(ch) => tag_popup_state.add_to_filter(ch),
                        Backspace => tag_popup_state.remove_from_filter(),
                        Esc => {
                            tag_popup_state.clear_filter();
                            app.tag_selection_mode = TagSelectionMode::Normal;
                        }
                        Enter => {
                            app.tag_selection_mode = TagSelectionMode::Normal;
                            app.select_tag();
                        }
                        _ => {}
                    },
                }
            } else if let Some(ref mut domain_state) = &mut app.domain_stats_popup_state {
                match key.code {
                    Enter => {
                        if let Some((domain, _)) =
                            domain_state.stats.get(domain_state.selected_index)
                        {
                            let authors: Vec<String> =
                                domain.split(", ").map(String::from).collect();
                            if domain.contains("YT:") {
                                // This is a video author
                                app.domain_filter = Some(domain.clone());
                                app.filter_by_video_authors(&authors);
                            } else {
                                // Regular domain
                                app.domain_filter = Some(domain.clone());
                                app.apply_filter();
                            }
                            app.domain_stats_popup_state = None;
                        }
                    }
                    Esc => {
                        app.domain_stats_popup_state = None;
                    }
                    Char('j') | Down => {
                        domain_state.move_selection(1);
                    }
                    Char('k') | Up => {
                        domain_state.move_selection(-1);
                    }
                    _ => { /*do nothing */ }
                }
            } else if let Some(ref mut popup_state) = app.rss_feed_popup_state {
                match key.code {
                    Char('j') | Down => popup_state.move_selection(1),
                    Char('k') | Up => popup_state.move_selection(-1),
                    Char('p') => popup_state.show_description = !popup_state.show_description,
                    KeyCode::Char('d') => {
                        popup_state.hide_current_item()?;
                        return Ok(());
                    }
                    Char('a') => {
                        app.process_add_to_pocket_with_tags()?;
                        return Ok(());
                    }
                    Enter => app.handle_rss_feed_selection()?,
                    Esc => {
                        if (popup_state.show_description) {
                            popup_state.show_description = false;
                        } else {
                            app.close_rss_feed_popup()?;
                        }
                        // app.rss_feed_popup_state = None;
                    }
                    _ => {}
                }
            } else {
                //normal mode
                match key.code {
                    Enter => {
                        if app.tag_popup_state.is_some() {
                            app.select_tag();
                        } else {
                            app.open_current_url()?;
                        }
                    }
                    Char('Z') => {
                        app.app_mode = AppMode::MulticharNormalModeEnter("Z".to_string());
                    }
                    Esc => {
                        if app.active_search_filter.is_some() {
                            app.clear_search_filter();
                        } else if app.selected_tag_filter.is_some() {
                            app.clear_tag_filter();
                        } else if app.domain_filter.is_some() {
                            app.clear_domain_filter();
                        } else if app.item_type_filter != ItemTypeFilter::All {
                            app.set_item_type_filter(ItemTypeFilter::All);
                        }
                        if app.help_popup_state.is_some() {
                            app.help_popup_state = None;
                        }
                    }
                    Char('j') | Down => {
                        if let Some(tag_popup_state) = &mut app.tag_popup_state {
                            tag_popup_state.move_selection(1);
                        } else {
                            app.next();
                        }
                    }
                    Char('k') | Up => {
                        if let Some(tag_popup_state) = &mut app.tag_popup_state {
                            tag_popup_state.move_selection(-1);
                        } else {
                            app.previous();
                        }
                    }
                    Char('/') => app.switch_to_search_mode(),
                    Char('t') => app.toggle_top_tag()?,
                    Char('T') => app.switch_to_edit_tags_mode(),
                    Char('f') | Char('F') => app.fav_and_archive_article()?,
                    Char('d') => {
                        if key.modifiers.contains(KeyModifiers::CONTROL) {
                            app.scroll_down();
                        } else {
                            app.switch_to_confirmation(Confirmation::DeletePocketItem);
                        }
                    }
                    Char('u') => {
                        if key.modifiers.contains(KeyModifiers::CONTROL) {
                            app.scroll_up();
                        }
                    }
                    Char('g') => app.app_mode = AppMode::MulticharNormalModeEnter("g".to_string()),
                    Char('G') => {
                        app.scroll_to_end();
                    }
                    Char('r') => app.switch_to_rename_mode(true),
                    Char('R') => app.switch_to_rename_mode(false),
                    Char('z') => {
                        if app.tag_popup_state.is_none() {
                            app.show_tag_popup();
                        } else {
                            app.tag_popup_state = None;
                        }
                    }
                    Char('w') => {
                        if let Some(idx) = app.virtual_state.selected() {
                            if let Some(item) = app.items.get(idx) {
                                match item.item_type() {
                                    "pdf" | "article" => {
                                        let message = match item.item_type() {
                                            "pdf" => "Downloading pdf ⏳",
                                            "article" => "Downloading article ⏳",
                                            _ => unreachable!(),
                                        };
                                        app.app_mode = AppMode::Refreshing(RefreshingPopup::new(
                                            message.to_string(),
                                            LoadingType::Download,
                                        ));
                                    }
                                    _ => {} // Do nothing for other types
                                }
                            }
                        }
                    }
                    Char('Q') => {
                        app.app_mode = AppMode::Refreshing(RefreshingPopup::new(
                            "Refreshing ⏳".to_string(),
                            LoadingType::Refresh,
                        ));
                    }
                    Char('s') => {
                        app.filter_by_current_domain()?;
                    }
                    Char('S') => {
                        app.show_domain_stats();
                    }
                    Char('i') => app.show_doc_type_popup(),
                    Char('n') => {
                        if app.rss_feed_popup_state.is_none() {
                            app.show_rss_feed_popup()?;
                        }
                    }
                    Char('b') => {
                        match app.handle_neovim_edit() {
                            Ok(Some(content)) => {
                                // Use the edited content here
                                // For example, you could store it in the currently selected item
                                if let Some(idx) = app.virtual_state.selected() {
                                    if let Some(item) = app.items.get_mut(idx) {
                                        // Do something with the content
                                        // For example:
                                        // item.notes = content;
                                    }
                                }
                            }
                            Ok(None) => {
                                // User cancelled or no changes
                            }
                            Err(e) => {
                                // Show error in the footer or status area
                                error!("Neovim edit failed: {}", e);
                            }
                        }
                    }
                    Char('?') => app.show_help_popup()?,
                    _ => {}
                }
            }
        }
    })
}

fn ui(f: &mut Frame, app: &mut App) {
    let rects = Layout::vertical([Constraint::Min(5), Constraint::Length(3)]).split(f.area());
    app.set_colors();

    if let AppMode::Initialize = app.app_mode {
        f.render_widget(Clear, f.area());
        f.render_widget(
            Block::default().style(Style::default().bg(OCEANIC_NEXT.base_00)), //app.colors.buffer_bg)),
            f.area(),
        );
        logo::render(f, rects[0]);
        return;
    }

    render_table(f, app, rects[0]);

    render_scrollbar(f, app, rects[0]);

    render_footer(f, app, rects[1]);

    render_domain_stats_popup(f, app, rects[0]);

    render_help_popup(f, app, rects[0]);

    render_rss_feed_popup(f, app, rects[0]); //todo: move if out of render

    if let AppMode::Error(message) = &app.app_mode {
        render_error_popup(f, message, f.size(), &app.colors);
    }

    // After tag popup rendering, add:
    if let Some(doc_popup_state) = &app.doc_type_popup_state {
        let popup_area = centered_rect(40, 40, f.area());
        f.render_widget(Clear, popup_area);

        let items: Vec<ListItem> = doc_popup_state
            .items
            .iter()
            .enumerate()
            .map(|(_i, (item_type, key, label))| {
                let content = format!("{} - {}", key, label);

                let style = if &app.item_type_filter == item_type {
                    Style::default().fg(Color::Black).bg(Color::White)
                } else {
                    Style::default().fg(app.colors.row_fg)
                };
                ListItem::new(content).style(style)
            })
            .collect();

        let doc_type_list = List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Filter by Document Type: ")
                    .border_style(Style::new().fg(app.colors.footer_border_color))
                    .border_type(BorderType::Rounded),
            )
            .style(Style::new().bg(Color::Black));

        f.render_widget(doc_type_list, popup_area);
    }

    if let Some(tag_popup_state) = &app.tag_popup_state {
        let popup_area = centered_rect(60, 60, f.area());
        f.render_widget(Clear, popup_area);

        let tags_text: Vec<ListItem> = tag_popup_state
            .filtered_tags
            .iter()
            .skip(tag_popup_state.scroll_offset)
            .take(tag_popup_state.visible_items)
            .enumerate()
            .map(|(i, (tag, count))| {
                let content = format!("{:<30} {}", tag, count);
                let style = if i + tag_popup_state.scroll_offset == tag_popup_state.selected_index {
                    Style::default().fg(Color::Black).bg(Color::White)
                } else {
                    Style::default().fg(app.colors.row_fg)
                };
                ListItem::new(content).style(style)
            })
            .collect();

        let mut block = Block::default()
            .borders(Borders::ALL)
            .title("All Tags")
            .border_style(Style::new().fg(app.colors.footer_border_color))
            .border_type(BorderType::Rounded);

        if app.tag_selection_mode == TagSelectionMode::Filtering {
            block = block.title(format!("Filter: {}", tag_popup_state.filter));
        }

        let tags_list = List::new(tags_text)
            .block(block)
            .style(Style::new().bg(Color::Black));

        f.render_widget(tags_list, popup_area);

        let scrollbar = Scrollbar::default()
            .orientation(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("↑".into()))
            .end_symbol(Some("↓".into()));
        let mut scroll_state = ScrollbarState::new(tag_popup_state.filtered_tags.len())
            .position(tag_popup_state.scroll_offset);
        f.render_stateful_widget(scrollbar, popup_area, &mut scroll_state);
    }

    if let AppMode::Refreshing(pop) = &app.app_mode {
        let popup_area = centered_rect(20, 10, f.area());
        f.render_widget(Clear, popup_area);

        // Create text spans with different styles to create animation effect
        let text = Text::from(vec![Line::from(vec![Span::styled(
            &pop.text,
            Style::new().fg(app.colors.row_fg),
        )])]);

        let block = Paragraph::new(text)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::new().fg(app.colors.footer_border_color))
                    .border_type(BorderType::Rounded),
            )
            .style(Style::new().bg(Color::Black))
            .alignment(Alignment::Center);

        f.render_widget(block, popup_area);
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints(
            [
                Constraint::Percentage((100 - percent_y) / 2),
                Constraint::Percentage(percent_y),
                Constraint::Percentage((100 - percent_y) / 2),
            ]
            .as_ref(),
        )
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints(
            [
                Constraint::Percentage((100 - percent_x) / 2),
                Constraint::Percentage(percent_x),
                Constraint::Percentage((100 - percent_x) / 2),
            ]
            .as_ref(),
        )
        .split(popup_layout[1])[1]
}

fn render_table(f: &mut Frame, app: &mut App, area: Rect) {
    let length = 14; //todo calc the value

    if app.virtual_state.selected().unwrap() >= app.virtual_state.offset() + length {
        *app.virtual_state.offset_mut() = app.virtual_state.selected().unwrap() + 1 - length;
    }

    let offset = app.virtual_state.offset();
    *app.state.offset_mut() = 0;
    app.state.select(Some(
        app.virtual_state.selected().unwrap() - app.virtual_state.offset(),
    ));

    let selected_style = Style::default().fg(app.colors.selected_style_fg);

    let rows = app
        .items
        .index(offset..(offset + length))
        .into_iter()
        .enumerate()
        .map(|(x, data)| {
            let actual_index = x + offset;
            let is_same_date =
                actual_index > 0 && data.date() == app.items.get(actual_index - 1).unwrap().date();
            let multiple_entries_for_date = !is_same_date
                && actual_index < app.items.len() - 1
                && data.date() == app.items.get(actual_index + 1).unwrap().date();
            let is_read = data.tags().any(|x| x == "read");
            let is_top = data.tags().any(|x| x == "top");
            let mut base_style = Style::new();
            if is_read {
                base_style = base_style.add_modifier(Modifier::DIM);
            } else {
                if is_top {
                    base_style = base_style.add_modifier(Modifier::BOLD);
                }
            }
            Row::new(vec![
                Cell::from(Text::from(if !is_same_date {
                    format!("{}", data.date())
                } else {
                    "".to_string()
                })),
                Cell::from(Text::from(vec![
                    Line::from(Span::styled(
                        format!(
                            "{}{}",
                            if is_top { "⭐ " } else { "" },
                            if !data.title().is_empty() {
                                data.title()
                            } else {
                                data.url()
                            }
                        ),
                        base_style.fg(OCEANIC_NEXT.base_07),
                    )),
                    Line::from(vec![
                        Span::styled(
                            format!("[{}]: ", data.item_type()),
                            base_style.fg(Color::Green).add_modifier(Modifier::ITALIC),
                        ),
                        Span::styled(
                            format!("{}", data.tags().join(", ")),
                            base_style.fg(OCEANIC_NEXT.base_0e),
                        ),
                    ]),
                ])),
                if actual_index == 0 || actual_index == 1 {
                    //todo: this creates garbage
                    let tmp = render_stats(
                        &app.stats.today_stats,
                        &app.stats.week_stats,
                        &app.stats.month_stats,
                    );
                    let stats_table: Vec<&str> =
                        tmp.split("\n").skip(actual_index * 3).take(3).collect();
                    Cell::from(Text::from(stats_table.join("\n").to_string())).style(selected_style)
                } else {
                    if multiple_entries_for_date {
                        let stats = collect_stats(&app.items.items, actual_index); //todo! accessing items of items
                        let stats_str = format!(
                            "░▒▓ Text: {} | PDFs: {} | Vids: {} ▓▒░",
                            // "Day [  Text: {} | PDFs: {} |  Vids: {}  ]",
                            stats.articles_total,
                            stats.pdfs_total,
                            stats.videos_total
                        );
                        Cell::from(Text::from(format!("{}", stats_str)))
                    } else {
                        Cell::from(Text::from("".to_string()))
                    }
                },
            ])
            .height(3)
        });
    let t = Table::new(
        rows,
        [
            // + 1 is for padding.
            Constraint::Length(app.longest_item_lens.0 + 1),
            Constraint::Min(app.longest_item_lens.1 + 1),
            Constraint::Min(app.longest_item_lens.2),
        ],
    )
    .row_highlight_style(selected_style)
    .highlight_symbol(Text::from(vec![" > ".into(), "".into(), "".into()]))
    .bg(app.colors.buffer_bg)
    .highlight_spacing(HighlightSpacing::Always);
    f.render_stateful_widget(t, area, &mut app.state);
}

//todo: the thrird column is not needed
fn constraint_len_calculator<T: TableRow>(items: &[T]) -> (u16, u16, u16) {
    let name_len = 10;
    let mut title_len = items
        .iter()
        .map(TableRow::title)
        .flat_map(str::lines)
        .map(UnicodeWidthStr::width)
        .max()
        .unwrap_or(0);
    let email_len = 40;

    //todo: dynamic size detection
    if title_len > 115 {
        title_len = 115;
    }

    #[allow(clippy::cast_possible_truncation)]
    (name_len as u16, title_len as u16, email_len as u16)
}

fn render_scrollbar(f: &mut Frame, app: &mut App, area: Rect) {
    f.render_stateful_widget(
        Scrollbar::default()
            .orientation(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None),
        area.inner(Margin {
            vertical: 1,
            horizontal: 1,
        }),
        &mut app.scroll_state,
    );
}

fn render_help_popup(f: &mut Frame, app: &mut App, area: Rect) {
    if let Some(help_state) = &app.help_popup_state {
        let popup_area = centered_rect(45, 80, area);
        f.render_widget(Clear, popup_area);

        let text = Text::from(
            help_state
                .content
                .lines()
                .map(|line| Line::from(Span::styled(line, Style::default().fg(app.colors.row_fg))))
                .collect::<Vec<_>>(),
        );

        let help_widget = Paragraph::new(text)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" GetPocket TUI Help ")
                    .border_style(Style::new().fg(app.colors.header_fg))
                    .border_type(BorderType::Rounded),
            )
            .style(Style::new().bg(Color::Black))
            .alignment(Alignment::Left);

        f.render_widget(help_widget, popup_area);
    }
}

fn render_error_popup(f: &mut Frame, message: &str, area: Rect, colors: &TableColors) {
    let popup_area = centered_rect(60, 20, area);
    f.render_widget(Clear, popup_area);

    let text = Text::from(vec![
        Line::from(vec![Span::styled(
            "Error",
            Style::default()
                .fg(OCEANIC_NEXT.base_08)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from(""),
        Line::from(vec![Span::styled(
            message,
            Style::default().fg(colors.row_fg),
        )]),
        Line::from(""),
        Line::from(vec![Span::styled(
            "Press ESC to dismiss",
            Style::default().fg(OCEANIC_NEXT.base_03),
        )]),
    ]);

    let error_widget = Paragraph::new(text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::new().fg(OCEANIC_NEXT.base_08))
                .border_type(BorderType::Rounded),
        )
        .style(Style::new().bg(Color::Black))
        .alignment(Alignment::Center);

    f.render_widget(error_widget, popup_area);
}

fn render_rss_feed_popup(f: &mut Frame, app: &mut App, area: Rect) {
    if let Some(popup_state) = &app.rss_feed_popup_state {
        let popup_area = centered_rect(80, 80, area);
        f.render_widget(Clear, popup_area);
        // Calculate areas for main content and status bar
        let chunks = Layout::vertical([
            Constraint::Min(3),    // Main content
            Constraint::Length(1), // Status bar
        ])
        .split(popup_area);
        // Group items by source and count them
        let mut source_counts = std::collections::HashMap::new();
        for item in &popup_state.items {
            *source_counts.entry(&item.source).or_insert(0) += 1;
        }

        // Keep track of which sources we've seen while rendering
        let mut seen_sources = std::collections::HashSet::new();

        let items: Vec<ListItem> = popup_state
            .items
            .iter()
            .skip(popup_state.scroll_offset)
            .take(popup_state.visible_items)
            .enumerate()
            .map(|(i, item)| {
                // Show source info only if we haven't seen this source yet
                let source_column = if !seen_sources.contains(&item.source) {
                    seen_sources.insert(&item.source);
                    let count = source_counts.get(&item.source).unwrap_or(&0);
                    format!(" {} ({})", item.source, count)
                } else {
                    String::new()
                };

                let date_and_title = if let Some(pub_date) = &item.pub_date {
                    vec![
                        Span::styled(
                            format!("{:<10}", &pub_date[0..10]),
                            Style::default().fg(OCEANIC_NEXT.base_03), // Gray for date
                        ),
                        Span::raw(": "),
                        Span::styled(
                            &item.title,
                            Style::default().fg(OCEANIC_NEXT.base_05), // Default text color
                        ),
                    ]
                } else {
                    vec![
                        Span::styled(
                            format!("{:<10}", "unknown"),
                            Style::default().fg(OCEANIC_NEXT.base_03),
                        ),
                        Span::raw(": "),
                        Span::styled(&item.title, Style::default().fg(OCEANIC_NEXT.base_05)),
                    ]
                };

                let source_span = Span::styled(
                    format!("{:<25}", source_column),
                    Style::default().fg(OCEANIC_NEXT.base_0d), // Distinct color for source
                );

                let content = Line::from(
                    [
                        vec![
                            source_span,
                            Span::raw("│ "), // Table separator
                        ],
                        date_and_title,
                    ]
                    .concat(),
                );

                let style = if i + popup_state.scroll_offset == popup_state.selected_index {
                    Style::default().fg(Color::Black).bg(Color::White)
                } else {
                    Style::default()
                };

                ListItem::new(vec![content]).style(style)
            })
            .collect();

        let feed_list = List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" RSS Feeds ")
                    .border_style(Style::new().fg(app.colors.footer_border_color))
                    .border_type(BorderType::Rounded),
            )
            .style(Style::new().bg(Color::Black));

        f.render_widget(feed_list, popup_area);

        let scrollbar = Scrollbar::default()
            .orientation(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("↑".into()))
            .end_symbol(Some("↓".into()));

        let mut scroll_state =
            ScrollbarState::new(popup_state.items.len()).position(popup_state.scroll_offset);

        f.render_stateful_widget(scrollbar, popup_area, &mut scroll_state);
        if popup_state.show_description {
            if let Some(selected_item) = popup_state.items.get(popup_state.selected_index) {
                let desc_popup_area = centered_rect(70, 40, f.size());
                f.render_widget(Clear, desc_popup_area);

                let description = selected_item
                    .description
                    .as_deref()
                    .unwrap_or("No description available");

                // Wrap text to fit popup width
                let max_width = (desc_popup_area.width as usize).saturating_sub(4);
                // let wrapped_text = textwrap::fill(description, max_width);

                let wrapped_text = description
                    .split_whitespace()
                    .fold((String::new(), 0), |(mut text, len), word| {
                        if len + word.len() + 1 > max_width {
                            text.push('\n');
                            (text + word, word.len())
                        } else if text.is_empty() {
                            (word.to_string(), word.len())
                        } else {
                            (text + " " + word, len + word.len() + 1)
                        }
                    })
                    .0;

                let text = Text::from(vec![
                    Line::from(vec![
                        Span::styled("Title: ", Style::default().fg(OCEANIC_NEXT.base_0d)),
                        Span::styled(
                            &selected_item.title,
                            Style::default().fg(OCEANIC_NEXT.base_05),
                        ),
                    ]),
                    Line::from(""),
                    Line::from(vec![
                        Span::styled("Source: ", Style::default().fg(OCEANIC_NEXT.base_0d)),
                        Span::styled(
                            &selected_item.source,
                            Style::default().fg(OCEANIC_NEXT.base_05),
                        ),
                    ]),
                    Line::from(""),
                    Line::from(vec![Span::styled(
                        "Description:",
                        Style::default().fg(OCEANIC_NEXT.base_0d),
                    )]),
                    Line::from(""),
                    Line::from(vec![Span::styled(
                        wrapped_text,
                        Style::default().fg(OCEANIC_NEXT.base_05),
                    )]),
                ]);

                let description_widget = Paragraph::new(text)
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .title(" Article Preview ")
                            .border_style(Style::new().fg(app.colors.footer_border_color))
                            .border_type(BorderType::Rounded),
                    )
                    .style(Style::new().bg(Color::Black))
                    .wrap(Wrap { trim: true })
                    .scroll((0, 0));

                f.render_widget(description_widget, desc_popup_area);
            }
        }
        if let Some((message, timestamp)) = &popup_state.status_message {
            if timestamp.elapsed() < Duration::from_secs(5) {
                // Show message for 5 seconds
                let status_text = Text::from(Line::from(vec![Span::styled(
                    message,
                    Style::default().fg(OCEANIC_NEXT.base_0b), // Green for success
                )]));

                let status_widget = Paragraph::new(status_text)
                    .style(Style::default().bg(Color::Black))
                    .alignment(Alignment::Center);

                f.render_widget(status_widget, chunks[1]);
            }
        }
    }
}

fn render_footer(f: &mut Frame, app: &App, area: Rect) {
    match &app.app_mode {
        AppMode::Initialize => panic!("Should not get here!"),
        AppMode::Normal
        | AppMode::MulticharNormalModeEnter(_)
        | AppMode::Refreshing(_)
        | AppMode::Error(_) => {
            let is_filtered = app.selected_tag_filter.is_some()
                || app.item_type_filter != ItemTypeFilter::All
                || app.domain_filter.is_some()
                || app.active_search_filter.is_some();

            let mut spans = if is_filtered {
                vec![Span::raw("[Filter]")]
            } else {
                vec![Span::raw(INFO_TEXT)]
            };

            if let Some(search) = &app.active_search_filter {
                spans.extend_from_slice(&[Span::raw(" | /"), Span::raw(search)]);
            }
            if let Some(tag) = &app.selected_tag_filter {
                spans.extend_from_slice(&[Span::raw(" | Tag: "), Span::raw(tag)]);
            }
            if let Some(domain) = &app.domain_filter {
                spans.extend_from_slice(&[Span::raw(" | Site : "), Span::raw(domain)]);
            }
            if app.item_type_filter != ItemTypeFilter::All {
                let filter_text = match app.item_type_filter {
                    ItemTypeFilter::All => unreachable!(),
                    ItemTypeFilter::Article => "Articles",
                    ItemTypeFilter::Video => "Videos",
                    ItemTypeFilter::PDF => "PDFs",
                };
                spans.extend_from_slice(&[Span::raw(" | Doc type : "), Span::raw(filter_text)]);
            }

            if app.item_type_filter != ItemTypeFilter::All
                || app.selected_tag_filter.is_some()
                || app.active_search_filter.is_some()
            {
                let text = format!("[Showing {} items]", app.items.len());
                spans.extend_from_slice(&[Span::raw(" ('ESC` to clear) | "), Span::raw(text)]);
            }
            if let Ok(items) = app.rss_feed_state.items.lock() {
                if !items.is_empty() {
                    spans.extend_from_slice(&[
                        Span::raw(" | "),
                        Span::styled(
                            " RSS updates ",
                            Style::default()
                                .bg(OCEANIC_NEXT.base_0e) // Pink background
                                .fg(OCEANIC_NEXT.base_00) // Dark text for contrast
                                .add_modifier(Modifier::BOLD),
                        ),
                    ]);
                }
            }
            let info_footer = Paragraph::new(Line::from(spans))
                .style(Style::new().fg(app.colors.row_fg).bg(app.colors.buffer_bg))
                .alignment(if is_filtered {
                    Alignment::Left
                } else {
                    Alignment::Center
                })
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(Style::new().fg(app.colors.footer_border_color))
                        .border_type(BorderType::Double),
                );
            f.render_widget(info_footer, area);
        }
        AppMode::Search(search) => {
            let mut final_string = "/".to_string();
            final_string.push_str(&search.search);

            let mut textarea = TextArea::new(vec![final_string]);
            textarea.set_style(Style::new().fg(app.colors.row_fg).bg(app.colors.buffer_bg));
            textarea.set_block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::new().fg(app.colors.footer_border_color))
                    .border_type(BorderType::Rounded),
            );
            textarea.move_cursor(tui_textarea::CursorMove::End);
            f.render_widget(&textarea, area);
        }
        AppMode::Confirmation(_) => {
            let mut textarea = TextArea::default();
            textarea.set_style(Style::new().fg(app.colors.row_fg).bg(app.colors.buffer_bg));
            textarea.set_block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Delete ? ['y' or 'd' - to confirm] ")
                    .border_style(Style::new().fg(app.colors.footer_border_color))
                    .border_type(BorderType::Rounded),
            );
            textarea.move_cursor(tui_textarea::CursorMove::End);
            f.render_widget(&textarea, area);
        }
        AppMode::CommandEnter(x) => {
            let area_with_margin = area.inner(Margin::new(1, 1));

            // Create the base TextArea for input
            let input_text = format!("{}{}", x.prompt, x.current_enter);
            let mut textarea = TextArea::new(vec![input_text]);
            textarea.set_style(Style::new().fg(app.colors.row_fg).bg(app.colors.buffer_bg));
            textarea.set_block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::new().fg(app.colors.footer_border_color))
                    .border_type(BorderType::Rounded),
            );

            let prompt_len = x.prompt.len();
            let cursor_pos = (x.cursor_pos + prompt_len).try_into().unwrap();
            textarea.move_cursor(CursorMove::Jump(0, cursor_pos));

            // Render the base TextArea
            f.render_widget(&textarea, area);

            // If there's a suggestion, render it as a separate dimmed text
            if let Some(suggestion) = &x.current_suggestion {
                // let suggestion = TextSuggestion {
                //     completion: "Popa".to_string(),
                //     full_text: "Popa!".to_string(),
                // };
                let suggestion_x = (prompt_len + x.current_enter.len() + 1) as u16;
                if suggestion_x < area_with_margin.width {
                    let suggestion_area = Rect::new(
                        area_with_margin.x + suggestion_x,
                        area_with_margin.y,
                        area_with_margin.width - suggestion_x,
                        1,
                    );

                    let suggestion_text = Paragraph::new(suggestion.completion.as_str()).style(
                        Style::new()
                            .fg(OCEANIC_NEXT.base_03)
                            .add_modifier(Modifier::DIM),
                    );

                    f.render_widget(suggestion_text, suggestion_area);
                }
            }
        }
    }
}

fn render_domain_stats_popup(f: &mut Frame, app: &mut App, area: Rect) {
    if let Some(popup_state) = &app.domain_stats_popup_state {
        let popup_area = centered_rect(60, 60, area);
        f.render_widget(Clear, popup_area);

        let items: Vec<ListItem> = popup_state
            .stats
            .iter()
            .skip(popup_state.scroll_offset)
            .take(popup_state.visible_items)
            .enumerate()
            .map(|(i, (domain, count))| {
                let content = format!("{:<40} {}", domain, count);
                let style = if i + popup_state.scroll_offset == popup_state.selected_index {
                    Style::default().fg(Color::Black).bg(Color::White)
                } else {
                    Style::default().fg(app.colors.row_fg)
                };
                ListItem::new(content).style(style)
            })
            .collect();

        let title = " Domain/Author Statistics ";
        let stats_list = List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(title)
                    .border_style(Style::new().fg(app.colors.footer_border_color))
                    .border_type(BorderType::Rounded),
            )
            .style(Style::new().bg(Color::Black));

        f.render_widget(stats_list, popup_area);

        let scrollbar = Scrollbar::default()
            .orientation(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("↑".into()))
            .end_symbol(Some("↓".into()));
        let mut scroll_state =
            ScrollbarState::new(popup_state.stats.len()).position(popup_state.scroll_offset);
        f.render_stateful_widget(scrollbar, popup_area, &mut scroll_state);
    }
}
#[cfg(test)]
mod tests {}
