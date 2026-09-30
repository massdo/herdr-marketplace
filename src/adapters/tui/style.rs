use ratatui::style::{Color, Modifier, Style};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub const ACCENT: Color = Color::Rgb(0x00, 0x78, 0xd4);
pub const MUTED: Color = Color::Rgb(0x8a, 0x8a, 0x8a);
pub const SELECTION_BG: Color = Color::DarkGray;
pub const SELECTION_FG: Color = Color::White;
pub const OK: Color = Color::Green;
pub const WARN: Color = Color::Yellow;
pub const ERROR: Color = Color::Red;

/// Background of a secondary button, as in VS Code.
pub const BUTTON_BG: Color = Color::Rgb(0x3a, 0x3d, 0x41);
/// Frame of a plugin card: light, just enough to part plugins.
pub const CARD: Color = Color::Rgb(0x4a, 0x50, 0x58);
/// Color of the star next to a plugin's stars.
pub const GOLD: Color = Color::Rgb(0xf2, 0xb8, 0x2d);

/// Look of a button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// The main action: white on blue.
    Primary,
    /// An action that deletes: white on red.
    Danger,
    Plain,
}

impl Tone {
    pub fn style(self) -> Style {
        let background = match self {
            Self::Primary => ACCENT,
            Self::Danger => ERROR,
            Self::Plain => BUTTON_BG,
        };
        Style::default()
            .fg(Color::White)
            .bg(background)
            .add_modifier(Modifier::BOLD)
    }
}

/// Text of a button, with its key written on it: " Install (i) ".
pub fn button_text(label: &str, key: &str) -> String {
    format!(" {label} ({key}) ")
}

/// A size in megabytes of 10⁶ bytes, to a tenth: "9.8 MB".
pub fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1e6)
}

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

/// `text` cut in its middle, so both ends of an owner/repo/subdir stay
/// visible.
pub fn ellipsize_middle(text: &str, max_cells: usize) -> String {
    if text.width() <= max_cells || max_cells < 3 {
        return ellipsize(text, max_cells);
    }
    let tail_cells = (max_cells - 1) / 2;
    let head = ellipsize(text, max_cells - tail_cells);
    let mut tail: Vec<char> = Vec::new();
    let mut used = 0;
    for ch in text.chars().rev() {
        let width = ch.width().unwrap_or(0);
        if used + width > tail_cells {
            break;
        }
        tail.push(ch);
        used += width;
    }
    format!("{head}{}", tail.into_iter().rev().collect::<String>())
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
