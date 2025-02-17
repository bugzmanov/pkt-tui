use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph},
    Frame,
};

//todo move palette stuff to theme.rs
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

pub fn render(f: &mut Frame, area: Rect) {
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
