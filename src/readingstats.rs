use chrono::{DateTime, Utc};

use crate::{storage::PocketItem, TableRow};
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

    pub fn track_as(
        &mut self,
        item: &PocketItem,
        today: &chrono::DateTime<Utc>,
        is_read: bool,
        read_ts: i64,
    ) {
        let datetime_ts = DateTime::from_timestamp(read_ts, 0).expect("invalid timestamp");
        let datetime: DateTime<Utc> = datetime_ts.to_utc();
        let duration = *today - datetime;

        if today.date_naive() == datetime.date_naive() {
            self.today_stats.increment(item.item_type(), is_read);
            self.week_stats.increment(item.item_type(), is_read);
            self.month_stats.increment(item.item_type(), is_read);
        } else if duration.num_days() <= 7 {
            self.week_stats.increment(item.item_type(), is_read);
            self.month_stats.increment(item.item_type(), is_read);
        } else if duration.num_days() <= 30 {
            self.month_stats.increment(item.item_type(), is_read);
        }
    }

    pub fn track_item(&mut self, item: &PocketItem, today: &chrono::DateTime<Utc>) {
        let is_read = item.tags().any(|x| x == "read"); // todo: encapsulate
        let timestamp = item.time_added.parse::<i64>().unwrap();
        self.track_as(item, today, is_read, timestamp);
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

    fn increment(&mut self, item_type: &str, is_read: bool) {
        match item_type {
            "pdf" => {
                if is_read {
                    self.pdfs_read += 1;
                } else {
                    self.pdfs_added += 1;
                }
            }
            "video" => {
                if is_read {
                    self.videos_read += 1;
                } else {
                    self.videos_added += 1;
                }
            }
            "article" => {
                if is_read {
                    self.articles_read += 1;
                } else {
                    self.articles_added += 1;
                }
            }
            _ => {
                todo!("impossible")
            }
        };
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

    let max_read = std::cmp::max(
        week_stats.articles_read,
        std::cmp::max(week_stats.videos_read, week_stats.pdfs_read),
    );
    let max_added = std::cmp::max(
        week_stats.articles_added,
        std::cmp::max(week_stats.videos_added, week_stats.pdfs_added),
    );

    let progress_bar =
        |label: &str, read: usize, added: usize, output: &mut String, draw_notch: bool| {
            let progress_added =
                "■".repeat(std::cmp::min(added, 45)) + &" ".repeat(0.max(30 - added)); // todo empty space should depend on screen size
            let progress_read = "■".repeat(std::cmp::min(read, 45)) + &" ".repeat(0.max(30 - read)); //todo empty space should depend on screen size
            let notch = if draw_notch { "_" } else { " " };
            write!(
                output,
                "{}: {:width$} │ {:3} added\n      {:width$}{notch}│ {:3}  read\n",
                label,
                progress_added,
                added,
                progress_read,
                read,
                notch = notch,
                width = max_added.max(max_read)
            )
            .unwrap();
        };

    progress_bar(
        "Text",
        week_stats.articles_read,
        week_stats.articles_added,
        &mut output,
        true,
    );
    progress_bar(
        "Vids",
        week_stats.videos_read,
        week_stats.videos_added,
        &mut output,
        true,
    );
    progress_bar(
        "PDFs",
        week_stats.pdfs_read,
        week_stats.pdfs_added,
        &mut output,
        false,
    );

    output.push_str("\n");

    output
}

//----
