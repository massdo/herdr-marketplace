use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::application::load_listing::LoadedListing;
use crate::domain::listing::Row;
use crate::domain::search::matches;
use crate::domain::source::PluginSource;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Intent {
    /// Load the index and the registry in the background.
    Load,
    /// Open the fiche of this row.
    Open(Box<Row>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadState {
    Loading,
    Failed(String),
    Ready(LoadedListing),
}

/// Sidebar state. Keys only touch memory: the index is loaded when the
/// sidebar opens and on an explicit retry, never while typing.
#[derive(Debug, Clone)]
pub struct SidebarApp {
    pub state: LoadState,
    pub query: String,
    /// Positions in `rows()` that match the query.
    pub visible: Vec<usize>,
    /// The selection follows an identity, not a position.
    pub selected: Option<PluginSource>,
    /// First visible position shown in the list.
    pub offset: usize,
    /// Rows the list area can show.
    pub page: usize,
    pub intents: Vec<Intent>,
    /// Why the last fiche did not open.
    pub notice: Option<String>,
}

impl Default for SidebarApp {
    fn default() -> Self {
        Self::new()
    }
}

impl SidebarApp {
    pub fn new() -> Self {
        Self {
            state: LoadState::Loading,
            query: String::new(),
            visible: Vec::new(),
            selected: None,
            offset: 0,
            page: 1,
            intents: vec![Intent::Load],
            notice: None,
        }
    }

    pub fn rows(&self) -> &[Row] {
        match &self.state {
            LoadState::Ready(loaded) => &loaded.listing.rows,
            _ => &[],
        }
    }

    pub fn loaded(&mut self, result: Result<LoadedListing, String>) {
        self.state = match result {
            Ok(loaded) => LoadState::Ready(loaded),
            Err(error) => LoadState::Failed(error),
        };
        self.refilter();
    }

    /// Returns true when the sidebar should close.
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c');
        }
        match key.code {
            KeyCode::Esc if self.query.is_empty() => return true,
            KeyCode::Esc => {
                self.query.clear();
                self.refilter();
            }
            KeyCode::Char(ch) => {
                self.query.push(ch);
                self.refilter();
            }
            KeyCode::Backspace => {
                if self.query.pop().is_some() {
                    self.refilter();
                }
            }
            KeyCode::Up => self.move_by(-1),
            KeyCode::Down => self.move_by(1),
            KeyCode::PageUp => self.move_by(-(self.page as isize)),
            KeyCode::PageDown => self.move_by(self.page as isize),
            KeyCode::Home => self.move_to(0),
            KeyCode::End => self.move_to(self.visible.len().saturating_sub(1)),
            KeyCode::Enter => self.enter(),
            _ => {}
        }
        false
    }

    pub fn selected_row(&self) -> Option<&Row> {
        let position = self.selected_position()?;
        self.rows().get(self.visible[position])
    }

    pub fn selected_position(&self) -> Option<usize> {
        let selected = self.selected.as_ref()?;
        let rows = self.rows();
        self.visible
            .iter()
            .position(|&index| rows[index].entry.source == *selected)
    }

    pub fn set_page(&mut self, page: usize) {
        self.page = page.max(1);
        self.ensure_visible();
    }

    fn enter(&mut self) {
        match &self.state {
            LoadState::Failed(_) => {
                self.state = LoadState::Loading;
                self.intents.push(Intent::Load);
            }
            LoadState::Ready(_) => {
                if let Some(row) = self.selected_row() {
                    self.intents.push(Intent::Open(Box::new(row.clone())));
                }
            }
            LoadState::Loading => {}
        }
    }

    fn refilter(&mut self) {
        let rows = self.rows();
        let visible: Vec<usize> = rows
            .iter()
            .enumerate()
            .filter(|(_, row)| matches(&row.entry, &self.query))
            .map(|(index, _)| index)
            .collect();
        let first = visible
            .first()
            .map(|&index| rows[index].entry.source.clone());
        self.visible = visible;
        if self.selected_position().is_none() {
            self.selected = first;
            self.offset = 0;
        }
        self.ensure_visible();
    }

    fn move_by(&mut self, delta: isize) {
        let position = self.selected_position().unwrap_or(0) as isize;
        self.move_to((position + delta).max(0) as usize);
    }

    fn move_to(&mut self, position: usize) {
        let Some(last) = self.visible.len().checked_sub(1) else {
            return;
        };
        let index = self.visible[position.min(last)];
        self.selected = Some(self.rows()[index].entry.source.clone());
        self.ensure_visible();
    }

    fn ensure_visible(&mut self) {
        let Some(position) = self.selected_position() else {
            self.offset = 0;
            return;
        };
        if position < self.offset {
            self.offset = position;
        } else if position >= self.offset + self.page {
            self.offset = position + 1 - self.page;
        }
    }
}
