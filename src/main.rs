#![allow(clippy::enum_glob_use, clippy::wildcard_imports)]

mod auth;
mod errors;
mod pocket;
mod readingstats;
pub mod storage;
mod tokenstorage;

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
use itertools::Itertools;
use log::{error, LevelFilter};
use pocket::GetPocketSync;
use ratatui::{prelude::*, widgets::*};
use readingstats::{render_stats, TotalStats};
use serde_json::json;
use std::{
    error::Error,
    fs::{self, File},
    io::{self, Write},
    ops::Range,
    path::Path,
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
            buffer_bg: OCEANIC_NEXT.base_00, //tailwind::SLATE.c950,
            header_fg: tailwind::SLATE.c200,
            row_fg: tailwind::SLATE.c200,
            selected_style_fg: OCEANIC_NEXT.base_0a, //color.c300,
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

struct RefreshingPopup {
    text: String,
    was_redered: bool,
    _last_update: Instant, //todo
}

impl RefreshingPopup {
    fn new(text: String) -> Self {
        Self {
            text,
            was_redered: false,
            _last_update: Instant::now(),
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
}

#[derive(Clone)]
pub struct CommandEnterMode {
    prompt: String,
    current_enter: String,
    cursor_pos: usize,
    command_type: CommandType,
}

impl CommandEnterMode {
    fn new_empty(prompt: String, command_type: CommandType) -> Self {
        Self {
            prompt,
            current_enter: String::new(),
            cursor_pos: 0,
            command_type,
        }
    }
    fn new(prompt: String, current_enter: String, command_type: CommandType) -> Self {
        let cursor_pos = current_enter.len();
        Self {
            prompt,
            current_enter,
            cursor_pos,
            command_type,
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
}

struct FilteredItems<T> {
    pub items: Vec<T>,
    is_filter_on: bool,
    filtered: Vec<usize>,
}

impl<T> FilteredItems<T> {
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
}

impl App {
    fn new(data_vec: Vec<PocketItem>, pocket_client: GetPocketSync, stats: TotalStats) -> App {
        App {
            virtual_state: TableState::default().with_selected(0),
            state: TableState::default().with_selected(0),
            longest_item_lens: constraint_len_calculator(&data_vec),
            // scroll_state: ScrollbarState::new((data_vec.len() - 1) * ITEM_HEIGHT),
            scroll_state: ScrollbarState::new(1), //todo: fix this
            colors: TableColors::new(&PALETTES[0]),
            color_index: 0,
            items: FilteredItems::new(data_vec),
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
        }
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
        self.stats = stats;
        self.items = FilteredItems::new(items);
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
        error!("ololo {:?}", mouse_event);
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

    let app: App = App::new(list, pocket_client, stats);
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
                    app.refresh_data()?;
                    app.switch_to_normal_mode();
                } else {
                    pop.was_redered = true;
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
                    app.app_mode = AppMode::CommandEnter(cur_state);

                    // cur_state.current_enter.push(ch);
                    // app.app_mode = AppMode::CommandEnter(cur_state);
                }
                Backspace => {
                    if cur_state.cursor_pos > 0 {
                        cur_state.current_enter.remove(cur_state.cursor_pos - 1);
                        cur_state.cursor_pos -= 1;
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
                error!("asdasdas");
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
                    Char('t') | Char('T') => app.toggle_top_tag()?,
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
                    Char('Q') => {
                        app.app_mode =
                            AppMode::Refreshing(RefreshingPopup::new("Refreshing ⏳".to_string()));
                    }
                    Char('s') => {
                        app.filter_by_current_domain()?;
                    }
                    Char('S') => {
                        app.show_domain_stats();
                    }
                    Char('i') => app.show_doc_type_popup(),
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
        render_logo(f, rects[0]);
        return;
    }

    render_table(f, app, rects[0]);

    render_scrollbar(f, app, rects[0]);

    render_footer(f, app, rects[1]);

    render_domain_stats_popup(f, app, rects[0]);

    render_help_popup(f, app, rects[0]);

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

fn render_logo(f: &mut Frame, area: Rect) {
    let mut lines = Vec::new();

    // Title section with red blocks
    let title_lines = vec![
        "    _______   __    __  ________           __     ________  __    __  ______    ",
        "   |       \\ |  \\  /  \\|        \\         /  \\   |        \\|  \\  |  \\|      \\   ",
        "   | ░▒▒▒▓▓░\\| ░░ /  ░▒ \\░▒▒▒▒▒░░        /  ░░    \\░░▓▓▓▓▒░| ▒░  | ░▒ \\░▒▓▓▓▒  ",
        "   | ▒▒__/ ▒░| ▓▓/  ░▓    | ▒▓ ______   /  ░░______ | ▒▒   | ▒▒  | ▒▓  | ▓▒    ",
        "   | ▓▓    ░▒| ██  ░▓     | ▒▓|      \\ /  ░░|      \\| ░▒   | ▓▓  | ▒█  | ▓▓    ",
        "   | ██▒▒▓▒▒ | ▓█▓▓█\\     | ▒▓ \\░░░░░░/  ░░  \\░░░░░░| ░▓   | ▒▓  | ▓█  | ▓█    ",
        "   | ▒▒      | ▒▓ \\▒▒\\    | ░▓       /  ░░          | ▒▓   | ▒▒__/ ▒▒ _| ▒█_   ",
        "   | ▓▒      | ▒▓  \\░░\\   | ▒░      |  ░░           | ▓░    \\░▒    ░▒|   ░▒ \\  ",
        "    \\░░       \\░░   \\░░    \\▒░       \\░░             \\▒░     \\░▒▓▓▒░  \\░░▒▒░░  ",
        "",
    ];

    // Process title lines
    for line in title_lines {
        let mut styled_spans = Vec::new();
        let mut current_text = String::new();

        for c in line.chars() {
            if "░▒▓█".contains(c) {
                if !current_text.is_empty() {
                    styled_spans.push(Span::raw(current_text.clone()));
                    current_text.clear();
                }
                styled_spans.push(Span::styled(
                    c.to_string(),
                    Style::default().fg(OCEANIC_NEXT.base_08),
                ));
            } else {
                current_text.push(c);
            }
        }
        if !current_text.is_empty() {
            styled_spans.push(Span::raw(current_text));
        }
        lines.push(Line::from(styled_spans));
    }

    // Dino section with light green blocks
    let dino_lines = vec![
        "                         ░▒▒░                    _______                        ",
        "                         ░▒▓▓▒▒▒▒▒░     ░▒▒▒░   /      /,░▒▒▒▒▒▒▒▒▒░          ",
        "                           ░▒▒▒▒▒▒░░▒▒▒▒▒▒▒▒░  / wait // ░▒▒▒▒▒▒▒▒▒▒▒░        ",
        "                             ░▒▒▒▒░░▒▒▒▒░     /______//           ░▓█▓▒▒▒░     ",
        "                            ░▓▓▒▒▒░░▒▒▒▒░    (______(/          ░▒▒▒▒▒▒▒▓▓▒░   ",
        "                            ░▒░    ░▒▒▒▒▒▒▒░                   ░▒▒░     ░▒▒░   ",
        "                                        ░▒▒▒░                 ░▓▒              ",
        "                                           ░▒░               ░██░              ",
        "                                            ▒▒  ░▒▒▒▒▒▒▒▒▒▒▒▒▓█▓              ",
        "                                            ▒▓▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒░              ",
        "                                            ░▓▓▒░                              ",
        "                                             ▒▒                                ",
        "                                             ░░                                ",
    ];

    // Process dino lines
    for line in dino_lines {
        let mut styled_spans = Vec::new();
        let mut current_text = String::new();

        for c in line.chars() {
            if "░▒▓█".contains(c) {
                if !current_text.is_empty() {
                    styled_spans.push(Span::raw(current_text.clone()));
                    current_text.clear();
                }
                styled_spans.push(Span::styled(
                    c.to_string(),
                    Style::default().fg(OCEANIC_NEXT.base_0b),
                ));
            } else {
                current_text.push(c);
            }
        }
        if !current_text.is_empty() {
            styled_spans.push(Span::raw(current_text));
        }
        lines.push(Line::from(styled_spans));
    }

    let popup_area = centered_rect(50, 65, area);
    f.render_widget(Clear, popup_area);

    let help_widget = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::NONE)
                .border_type(BorderType::Rounded),
        )
        .style(Style::new().bg(OCEANIC_NEXT.base_00))
        .alignment(Alignment::Left);

    f.render_widget(help_widget, popup_area);
}

fn render_footer(f: &mut Frame, app: &App, area: Rect) {
    match &app.app_mode {
        AppMode::Initialize => panic!("Should not get here!"),
        AppMode::Normal | AppMode::MulticharNormalModeEnter(_) | AppMode::Refreshing(_) => {
            let is_filtered = app.selected_tag_filter.is_some()
                || app.item_type_filter != ItemTypeFilter::All
                || app.domain_filter.is_some()
                || app.active_search_filter.is_some();

            let mut info_text = if is_filtered {
                "[Filter]".to_string()
            } else {
                INFO_TEXT.to_string()
            };

            if let Some(search) = &app.active_search_filter {
                info_text = format!("{} | /'{}'", info_text, search);
            }
            if let Some(tag) = &app.selected_tag_filter {
                info_text = format!("{} | Tag: '{}' ", info_text, tag);
            }
            if let Some(domain) = &app.domain_filter {
                info_text = format!("{} | Site: '{}' ", info_text, domain);
            }
            if app.item_type_filter != ItemTypeFilter::All {
                let filter_text = match app.item_type_filter {
                    ItemTypeFilter::All => unreachable!(),
                    ItemTypeFilter::Article => "Articles",
                    ItemTypeFilter::Video => "Videos",
                    ItemTypeFilter::PDF => "PDFs",
                };
                info_text = format!("{} | Doc type: {}", info_text, filter_text);
            }

            if app.item_type_filter != ItemTypeFilter::All
                || app.selected_tag_filter.is_some()
                || app.active_search_filter.is_some()
            {
                // format!("[Showing {} items]", app.items.len())
                info_text = format!(
                    "{} ('ESC' to clear) | [Showing {} items]",
                    info_text,
                    app.items.len()
                );
            }
            let info_footer = Paragraph::new(Line::from(info_text))
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
            let mut final_string = x.prompt.clone();
            final_string.push_str(&x.current_enter);

            let mut textarea = TextArea::new(vec![final_string]);
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

            f.render_widget(&textarea, area);
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
