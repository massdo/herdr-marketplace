use ratatui::style::{Color, Modifier, Style};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub const ACCENT: Color = Color::Rgb(0x00, 0x78, 0xd4);
pub const MUTED: Color = Color::Rgb(0x8a, 0x8a, 0x8a);
pub const SELECTION_BG: Color = Color::DarkGray;
pub const SELECTION_FG: Color = Color::White;
pub const OK: Color = Color::Green;
pub const WARN: Color = Color::Yellow;
pub const ERROR: Color = Color::Red;

pub fn muted() -> Style {
    Style::default().fg(MUTED)
}

pub fn bold() -> Style {
    Style::default().add_modifier(Modifier::BOLD)
}

/// `text` cut to `max_cells` terminal cells, with an ellipsis when cut.
pub fn ellipsize(text: &str, max_cells: usize) -> String {
    if text.width() <= max_cells {
        return text.to_string();
    }
    if max_cells == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let width = ch.width().unwrap_or(0);
        if used + width > max_cells - 1 {
            break;
        }
        out.push(ch);
        used += width;
    }
    out.push('…');
    out
}

/// Greedy word wrap on cell width; words longer than a line are split.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut used = 0;
    for word in text.split(' ') {
        let word_width = word.width();
        if used > 0 && used + 1 + word_width > width {
            lines.push(std::mem::take(&mut line));
            used = 0;
        }
        if used > 0 {
            line.push(' ');
            used += 1;
        }
        for ch in word.chars() {
            let ch_width = ch.width().unwrap_or(0);
            if used + ch_width > width {
                lines.push(std::mem::take(&mut line));
                used = 0;
            }
            line.push(ch);
            used += ch_width;
        }
    }
    lines.push(line);
    lines
}
