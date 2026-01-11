use chrono::{DateTime, Utc};

use crate::raindrop::Raindrop;
//----
pub struct Stats {
    articles_added: usize,
    articles_read: usize,
    pdfs_added: usize,
    pdfs_read: usize,
    videos_added: usize,
    videos_read: usize,
}

pub struct TotalStats {
    pub today_stats: Stats,
    pub week_stats: Stats,
    pub month_stats: Stats,
}

impl TotalStats {
    pub fn new() -> Self {
        TotalStats {
            today_stats: Stats::new(),
            week_stats: Stats::new(),
            month_stats: Stats::new(),
        }
    }

    pub fn from_items(items: &[Raindrop]) -> Self {
        let mut stats = Self::new();
        let today = Utc::now();

        for item in items {
            stats.track_raindrop(item, &today);
        }

        stats
    }

    pub fn track_raindrop(&mut self, item: &Raindrop, today: &DateTime<Utc>) {
        let is_read = item.tags.iter().any(|t| t == "archived");
        let item_type = item.item_type.as_str();

        // Parse created date for "added" stats
        let created = match DateTime::parse_from_rfc3339(&item.created) {
            Ok(dt) => dt.with_timezone(&Utc),
            Err(_) => return,
        };

        // Track "added" items based on created date
        let created_duration = *today - created;
        if today.date_naive() == created.date_naive() {
            self.today_stats.increment_added(item_type);
            self.week_stats.increment_added(item_type);
            self.month_stats.increment_added(item_type);
        } else if created_duration.num_days() <= 7 {
            self.week_stats.increment_added(item_type);
            self.month_stats.increment_added(item_type);
        } else if created_duration.num_days() <= 30 {
            self.month_stats.increment_added(item_type);
        }

        // Track "read" items based on last_update date (when archived)
        if is_read {
            let last_update = match DateTime::parse_from_rfc3339(&item.last_update) {
                Ok(dt) => dt.with_timezone(&Utc),
                Err(_) => return,
            };
            let read_duration = *today - last_update;
            if today.date_naive() == last_update.date_naive() {
                self.today_stats.increment_read(item_type);
                self.week_stats.increment_read(item_type);
                self.month_stats.increment_read(item_type);
            } else if read_duration.num_days() <= 7 {
                self.week_stats.increment_read(item_type);
                self.month_stats.increment_read(item_type);
            } else if read_duration.num_days() <= 30 {
                self.month_stats.increment_read(item_type);
            }
        }
    }
}

impl Stats {
    fn new() -> Self {
        Stats {
            articles_added: 0,
            articles_read: 0,
            pdfs_added: 0,
            pdfs_read: 0,
            videos_added: 0,
            videos_read: 0,
        }
    }

    fn increment_added(&mut self, item_type: &str) {
        match item_type {
            "pdf" => self.pdfs_added += 1,
            "video" => self.videos_added += 1,
            "article" => self.articles_added += 1,
            _ => {}
        }
    }

    fn increment_read(&mut self, item_type: &str) {
        match item_type {
            "pdf" => self.pdfs_read += 1,
            "video" => self.videos_read += 1,
            "article" => self.articles_read += 1,
            _ => {}
        }
    }
}

/**
Text: │  23 added
     _│_   2 read
Vids: │  23 added
     _│_  2 read
PDFs: │   2 added
      │   0 read

      Day [░░Text: {}░|░PDFs: {}░|░Vids: {}░░]"
      */

pub fn render_stats(_today_stats: &Stats, week_stats: &Stats, _month_stats: &Stats) -> String {
    use std::fmt::Write;

    let mut output = String::new();

    // Find max value to scale bars proportionally
    let max_val = [
        week_stats.articles_read,
        week_stats.articles_added,
        week_stats.videos_read,
        week_stats.videos_added,
        week_stats.pdfs_read,
        week_stats.pdfs_added,
    ]
    .into_iter()
    .max()
    .unwrap_or(1)
    .max(1);

    const BAR_WIDTH: usize = 25;

    let make_bar = |value: usize| -> String {
        let bar_len = if max_val > 0 {
            (value * BAR_WIDTH) / max_val.max(1)
        } else {
            0
        };
        let bar_len = bar_len.min(BAR_WIDTH);
        let filled = "■".repeat(bar_len);
        let empty = "░".repeat(BAR_WIDTH - bar_len);
        format!("{}{}", filled, empty)
    };

    let progress_bar =
        |label: &str, read: usize, added: usize, output: &mut String| {
            let bar_added = make_bar(added);
            let bar_read = make_bar(read);
            write!(
                output,
                "{}: {} {:3} +\n      {} {:3} ✓\n",
                label,
                bar_added,
                added,
                bar_read,
                read,
            )
            .unwrap();
        };

    progress_bar(
        "Text",
        week_stats.articles_read,
        week_stats.articles_added,
        &mut output,
    );
    progress_bar(
        "Vids",
        week_stats.videos_read,
        week_stats.videos_added,
        &mut output,
    );
    progress_bar(
        "PDFs",
        week_stats.pdfs_read,
        week_stats.pdfs_added,
        &mut output,
    );

    output
}

//----
