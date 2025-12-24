//! Raindrop TUI - A terminal UI for browsing Raindrop.io bookmarks
//!
//! Run with: cargo run --bin raindrop_tui
//!
//! Set RAINDROP_API_TOKEN env var or it will prompt for token.

use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use pkt_tui::raindrop::{
    load_cache, now_iso8601, save_cache, token, Raindrop, RaindropAPISync, RaindropCache,
    COLLECTION_ALL,
};
use ratatui::{prelude::*, widgets::*};
use std::io;

struct App {
    items: Vec<Raindrop>,
    state: TableState,
    client: RaindropAPISync,
    filter: Option<String>,
    filtered_indices: Vec<usize>,
    status: String,
}

impl App {
    fn new(items: Vec<Raindrop>, client: RaindropAPISync) -> Self {
        let mut state = TableState::default();
        state.select(Some(0));
        let status = format!("{} items loaded", items.len());
        Self {
            items,
            state,
            client,
            filter: None,
            filtered_indices: Vec::new(),
            status,
        }
    }

    fn visible_items(&self) -> Vec<&Raindrop> {
        if self.filter.is_some() {
            self.filtered_indices
                .iter()
                .map(|&i| &self.items[i])
                .collect()
        } else {
            self.items.iter().collect()
        }
    }

    fn visible_len(&self) -> usize {
        if self.filter.is_some() {
            self.filtered_indices.len()
        } else {
            self.items.len()
        }
    }

    fn selected_item(&self) -> Option<&Raindrop> {
        let idx = self.state.selected()?;
        if self.filter.is_some() {
            self.filtered_indices.get(idx).map(|&i| &self.items[i])
        } else {
            self.items.get(idx)
        }
    }

    fn next(&mut self) {
        if self.visible_len() == 0 {
            return;
        }
        let i = match self.state.selected() {
            Some(i) => {
                if i >= self.visible_len() - 1 {
                    i
                } else {
                    i + 1
                }
            }
            None => 0,
        };
        self.state.select(Some(i));
    }

    fn previous(&mut self) {
        if self.visible_len() == 0 {
            return;
        }
        let i = match self.state.selected() {
            Some(i) => {
                if i == 0 {
                    0
                } else {
                    i - 1
                }
            }
            None => 0,
        };
        self.state.select(Some(i));
    }

    fn page_down(&mut self) {
        if self.visible_len() == 0 {
            return;
        }
        let i = self.state.selected().unwrap_or(0);
        let new_i = (i + 10).min(self.visible_len() - 1);
        self.state.select(Some(new_i));
    }

    fn page_up(&mut self) {
        let i = self.state.selected().unwrap_or(0);
        let new_i = i.saturating_sub(10);
        self.state.select(Some(new_i));
    }

    fn go_to_top(&mut self) {
        self.state.select(Some(0));
    }

    fn go_to_bottom(&mut self) {
        if self.visible_len() > 0 {
            self.state.select(Some(self.visible_len() - 1));
        }
    }

    fn filter_by_tag(&mut self, tag: &str) {
        self.filtered_indices.clear();
        for (i, item) in self.items.iter().enumerate() {
            if item.has_tag(tag) {
                self.filtered_indices.push(i);
            }
        }
        self.filter = Some(tag.to_string());
        self.state.select(Some(0));
        self.status = format!(
            "Filtered by tag '{}': {} items",
            tag,
            self.filtered_indices.len()
        );
    }

    fn clear_filter(&mut self) {
        self.filter = None;
        self.filtered_indices.clear();
        self.state.select(Some(0));
        self.status = format!("{} items", self.items.len());
    }

    fn open_selected(&self) {
        if let Some(item) = self.selected_item() {
            let _ = webbrowser::open(&item.link);
        }
    }

    fn delete_selected(&mut self) -> Result<()> {
        if let Some(item) = self.selected_item() {
            let id = item.id;
            self.client.delete_raindrop(id)?;

            // Remove from local list
            if let Some(pos) = self.items.iter().position(|i| i.id == id) {
                self.items.remove(pos);
            }

            // Update filter indices if active
            if self.filter.is_some() {
                self.filtered_indices.retain(|&i| i < self.items.len());
            }

            self.status = format!("Deleted item {}", id);
        }
        Ok(())
    }
}

fn main() -> Result<()> {
    // Get token
    let token = token::get_or_prompt_token()?;

    // Create client
    let client = RaindropAPISync::new(&token)?;

    // Try to load from cache first
    let mut items = if let Ok(Some(cache)) = load_cache() {
        println!(
            "Loaded {} items from cache (synced: {})",
            cache.raindrops.len(),
            cache.last_sync
        );
        cache.raindrops
    } else {
        // Fetch all raindrops with progress
        print!("Fetching raindrops from Raindrop.io... ");
        let _ = io::Write::flush(&mut io::stdout());
        let fetched =
            client.get_all_raindrops_with_progress(COLLECTION_ALL, |downloaded, total| {
                print!(
                    "\rFetching raindrops from Raindrop.io... {}/{}",
                    downloaded, total
                );
                let _ = io::Write::flush(&mut io::stdout());
            })?;
        println!();

        // Save to cache
        let cache = RaindropCache {
            raindrops: fetched.clone(),
            collections: vec![],
            last_sync: now_iso8601(),
        };
        if let Err(e) = save_cache(&cache) {
            eprintln!("Warning: Failed to save cache: {}", e);
        }

        println!("Fetched {} items from Raindrop.io", fetched.len());
        fetched
    };

    // Sort by date (newest first)
    items.sort_by(|a, b| b.created.cmp(&a.created));

    println!("Starting TUI with {} items...", items.len());

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create app
    let mut app = App::new(items, client);

    // Run app
    let res = run_app(&mut terminal, &mut app);

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    if let Err(err) = res {
        println!("Error: {:?}", err);
    }

    Ok(())
}

fn run_app<B: Backend>(terminal: &mut Terminal<B>, app: &mut App) -> Result<()> {
    loop {
        terminal.draw(|f| ui(f, app))?;

        if let Event::Key(key) = event::read()? {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                KeyCode::Down | KeyCode::Char('j') => app.next(),
                KeyCode::Up | KeyCode::Char('k') => app.previous(),
                KeyCode::Char('d') => app.page_down(),
                KeyCode::Char('u') => app.page_up(),
                KeyCode::Char('g') => app.go_to_top(),
                KeyCode::Char('G') => app.go_to_bottom(),
                KeyCode::Enter => app.open_selected(),
                KeyCode::Char('c') => app.clear_filter(),
                KeyCode::Char('x') => {
                    if let Err(e) = app.delete_selected() {
                        app.status = format!("Error: {}", e);
                    }
                }
                KeyCode::Char('t') => {
                    // Filter by first tag of selected item
                    if let Some(item) = app.selected_item() {
                        if let Some(tag) = item.tags.first() {
                            let tag = tag.clone();
                            app.filter_by_tag(&tag);
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

fn ui(f: &mut Frame, app: &mut App) {
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(0),
        Constraint::Length(3),
    ])
    .split(f.area());

    // Header
    let header = Paragraph::new(format!(
        " Raindrop TUI | {} | Filter: {}",
        app.status,
        app.filter.as_deref().unwrap_or("none")
    ))
    .style(Style::default().fg(Color::Cyan))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Raindrop.io "),
    );
    f.render_widget(header, chunks[0]);

    // Table header
    let header_cells = ["Date", "Title", "Tags", "Type"]
        .iter()
        .map(|h| Cell::from(*h).style(Style::default().fg(Color::Yellow)));
    let table_header = Row::new(header_cells).height(1);

    // Collect rows data first to avoid borrow issues
    let row_data: Vec<(String, String, String, String, bool, bool)> = app
        .visible_items()
        .iter()
        .map(|item| {
            (
                item.date(),
                item.display_title().to_string(),
                item.tags.join(", "),
                item.type_str().to_string(),
                item.has_tag("read"),
                item.has_tag("top"),
            )
        })
        .collect();

    let rows: Vec<Row> = row_data
        .iter()
        .map(|(date, title, tags, item_type, is_read, is_top)| {
            let mut style = Style::default();
            if *is_read {
                style = style.add_modifier(Modifier::DIM);
            }
            if *is_top {
                style = style.fg(Color::Yellow);
            }

            Row::new(vec![
                Cell::from(date.clone()),
                Cell::from(if *is_top {
                    format!("* {}", title)
                } else {
                    title.clone()
                }),
                Cell::from(tags.clone()),
                Cell::from(item_type.clone()),
            ])
            .style(style)
            .height(1)
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(12),
            Constraint::Percentage(50),
            Constraint::Percentage(35),
            Constraint::Length(8),
        ],
    )
    .header(table_header)
    .block(Block::default().borders(Borders::ALL))
    .row_highlight_style(
        Style::default()
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    )
    .highlight_symbol("> ");

    f.render_stateful_widget(table, chunks[1], &mut app.state);

    // Footer
    let footer = Paragraph::new(" q:Quit | j/k:Navigate | Enter:Open | t:FilterByTag | c:ClearFilter | x:Delete | g/G:Top/Bottom ")
        .style(Style::default().fg(Color::Gray))
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(footer, chunks[2]);
}
