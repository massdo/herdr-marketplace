//! Text selected with the mouse. Herdr selects text only in a pane that
//! leaves it the mouse; the details pane asks for clicks and the wheel, so
//! it selects on its own, the way Herdr does: from the cell pressed to the
//! cell under the pointer, in reading order, copied when the button is
//! released.

use std::ops::Range;

use base64::Engine as _;
use ratatui::buffer::Buffer;
use ratatui::style::Style;
use unicode_width::UnicodeWidthStr;

use super::style::{SELECTION_BG, SELECTION_FG};

/// How a line reads in a copy. A row is copied as drawn, without its
/// trailing spaces, except for what only the README layout knows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Flow {
    /// Column where the text starts: before it, the padding of code, or the
    /// indentation of a line that goes on from the one above.
    pub start: usize,
    /// Column after the text of code, whose trailing spaces count.
    pub end: Option<usize>,
    /// The line goes on from the one above, cut by the pane width: what the
    /// cut took out between them, a space or nothing.
    pub joins: Option<&'static str>,
    /// Drawn for looks only, an image, a rule or a frame: never copied.
    pub decor: bool,
}

impl Flow {
    pub const DECOR: Self = Self {
        start: 0,
        end: None,
        joins: None,
        decor: true,
    };
}

/// A drag over the pane, from the cell pressed to the cell under the
/// pointer, `(column, row)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    anchor: (u16, u16),
    cursor: (u16, u16),
    /// The pointer left the cell pressed: a click selects nothing.
    dragged: bool,
}

impl Selection {
    pub fn new(column: u16, row: u16) -> Self {
        Self {
            anchor: (column, row),
            cursor: (column, row),
            dragged: false,
        }
    }

    pub fn drag(&mut self, column: u16, row: u16) {
        self.cursor = (column, row);
        self.dragged |= self.cursor != self.anchor;
    }

    pub fn dragged(&self) -> bool {
        self.dragged
    }

    /// First and last cells, in reading order.
    fn ordered(&self) -> ((u16, u16), (u16, u16)) {
        let reading = |(column, row): (u16, u16)| (row, column);
        if reading(self.anchor) <= reading(self.cursor) {
            (self.anchor, self.cursor)
        } else {
            (self.cursor, self.anchor)
        }
    }

    /// Rows selected, among `height`; none until the pointer moves.
    fn rows(&self, height: u16) -> Range<u16> {
        if !self.dragged {
            return 0..0;
        }
        let ((_, first), (_, last)) = self.ordered();
        first.min(height)..last.saturating_add(1).min(height)
    }

    /// Columns `first..=last` taken on `row`, `width` columns wide: the
    /// selected cells that hold the text of `flow`.
    fn columns(&self, row: u16, width: u16, flow: Flow) -> Option<(u16, u16)> {
        let ((first_column, first_row), (last_column, last_row)) = self.ordered();
        let first = if row == first_row { first_column } else { 0 };
        let last = if row == last_row {
            last_column
        } else {
            u16::MAX
        };
        let start = u16::try_from(flow.start).unwrap_or(u16::MAX);
        let end = match flow.end {
            Some(end) => u16::try_from(end).unwrap_or(u16::MAX).checked_sub(1)?,
            None => u16::MAX,
        };
        let (first, last) = (first.max(start), last.min(end).min(width.checked_sub(1)?));
        (first <= last).then_some((first, last))
    }
}

/// Paints the cells a copy takes, so the selection shows what it copies.
pub fn highlight(buffer: &mut Buffer, flows: &[Flow], selection: &Selection) {
    let area = buffer.area;
    let style = Style::default().fg(SELECTION_FG).bg(SELECTION_BG);
    for row in selection.rows(area.height) {
        let flow = flows.get(usize::from(row)).copied().unwrap_or_default();
        if flow.decor {
            continue;
        }
        if let Some((first, last)) = selection.columns(row, area.width, flow) {
            for column in first..=last {
                buffer[(area.x + column, area.y + row)].set_style(style);
            }
        }
    }
}

/// The text a selection covers, as it reads: a line cut by the pane width
/// goes on without a line break, and what is drawn for looks is left out,
/// as are blank lines at either end.
pub fn copy(buffer: &Buffer, flows: &[Flow], selection: &Selection) -> String {
    let area = buffer.area;
    let mut text = String::new();
    let mut above = None;
    for row in selection.rows(area.height) {
        let flow = flows.get(usize::from(row)).copied().unwrap_or_default();
        if flow.decor {
            continue;
        }
        let mut line = String::new();
        if let Some((first, last)) = selection.columns(row, area.width, flow) {
            let mut column = 0;
            while column <= last {
                let symbol = buffer[(area.x + column, area.y + row)].symbol();
                if column >= first {
                    // Labels keep their words together with no-break spaces.
                    line.push_str(if symbol == "\u{a0}" { " " } else { symbol });
                }
                // The cells a wide character covers after its first are blank.
                column = column.saturating_add(symbol.width().max(1) as u16);
            }
        }
        if flow.end.is_none() {
            line.truncate(line.trim_end().len());
        }
        if !text.is_empty() {
            match flow.joins {
                Some(cut) if above.is_some_and(|above: u16| above + 1 == row) => text.push_str(cut),
                _ => text.push('\n'),
            }
        }
        text.push_str(&line);
        above = Some(row);
    }
    text.truncate(text.trim_end().len());
    text
}

/// OSC 52: `text` for the clipboard. Herdr copies it to the system
/// clipboard and says so, as for its own selections.
pub fn clipboard(text: &str) -> String {
    let encoded = base64::engine::general_purpose::STANDARD.encode(text);
    format!("\x1b]52;c;{encoded}\x07")
}
