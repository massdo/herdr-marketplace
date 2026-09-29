use std::collections::HashSet;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::style::Color;

use super::sidebar_view::{self, Hit};
use super::style::{ERROR, OK, WARN};
use crate::application::load_listing::LoadedListing;
use crate::application::open_details::Reveal;
use crate::domain::PLUGIN_ID;
use crate::domain::compat::Platform;
use crate::domain::listing::Row;
use crate::domain::operation::{Confirmation, OperationRecord, Status};
use crate::domain::registry::InstalledPlugin;
use crate::domain::search::search;
use crate::domain::source::PluginSource;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Intent {
    /// Load the index and the registry in the background.
    Load,
    /// Show the details of this row now: Enter gives them the focus, a
    /// click leaves it in the sidebar.
    Open(Box<Row>, Reveal),
    /// Show the details of this row and leave the focus in the sidebar.
    Preview(Box<Row>),
    /// A search selected this row: details already open show it.
    Follow(Box<Row>),
    /// Check the update of this row, then run it without a preview.
    Update(Box<Row>),
}

/// A message above the list, in its color.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub text: String,
    pub color: Color,
}

/// Which plugins the list shows, as the filters of VS Code's extensions view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Filter {
    #[default]
    All,
    Installed,
}

impl Filter {
    fn other(self) -> Self {
        match self {
            Self::All => Self::Installed,
            Self::Installed => Self::All,
        }
    }
}

/// Typed in the search, as in VS Code, it shows the installed plugins.
pub const INSTALLED_TOKEN: &str = "@installed";

/// Plugins matching the query, under each filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Counts {
    pub all: usize,
    pub installed: usize,
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
    pub filter: Filter,
    pub counts: Counts,
    /// The pane has the focus: the search box is outlined in color.
    pub focused: bool,
    /// Positions in `rows()` that match the query and the filter, most
    /// relevant first.
    pub visible: Vec<usize>,
    /// Incompatible plugins left out of the list that match the query.
    pub hidden: usize,
    /// The selection follows an identity, not a position.
    pub selected: Option<PluginSource>,
    /// First visible position shown in the list.
    pub offset: usize,
    /// Rows the list area can show.
    pub page: usize,
    pub intents: Vec<Intent>,
    /// Why the last details pane did not open, or how an update went.
    pub notice: Option<Notice>,
    /// The update launched from this sidebar, from the click until the
    /// registry is read again after it ends.
    pub updating: Option<PluginSource>,
    /// Updates running according to their kept results, including those
    /// launched from a details pane.
    pub running_updates: HashSet<PluginSource>,
    /// The update launched from here ended: the next registry read frees it.
    update_ended: bool,
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
            filter: Filter::All,
            counts: Counts::default(),
            focused: true,
            visible: Vec::new(),
            hidden: 0,
            selected: None,
            offset: 0,
            page: 1,
            intents: vec![Intent::Load],
            notice: None,
            updating: None,
            running_updates: HashSet::new(),
            update_ended: false,
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

    /// The registry was read again after an operation: rows are rebuilt from
    /// the same catalogue and the selection keeps its identity. An update
    /// launched from here that ended no longer runs.
    pub fn registry_refreshed(
        &mut self,
        registry: Result<Vec<InstalledPlugin>, String>,
        host: Platform,
    ) {
        if self.update_ended {
            self.update_ended = false;
            self.updating = None;
        }
        if let LoadState::Ready(loaded) = &mut self.state {
            loaded.use_registry(registry, host);
            self.refilter();
        }
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
            // Esc backs out one step: the search, the filter, the sidebar.
            KeyCode::Esc if !self.query.is_empty() => {
                self.query.clear();
                self.search_changed();
            }
            KeyCode::Esc if self.filter != Filter::All => self.set_filter(Filter::All),
            KeyCode::Esc => return true,
            KeyCode::Tab | KeyCode::BackTab => self.set_filter(self.filter.other()),
            KeyCode::Char(ch) => {
                self.query.push(ch);
                self.take_filter_token();
                self.search_changed();
            }
            KeyCode::Backspace => {
                if self.query.pop().is_some() {
                    self.search_changed();
                }
            }
            KeyCode::Up => self.browse_by(-1),
            KeyCode::Down => self.browse_by(1),
            KeyCode::PageUp => self.browse_by(-(self.page as isize)),
            KeyCode::PageDown => self.browse_by(self.page as isize),
            KeyCode::Home => self.browse_to(0),
            KeyCode::End => self.browse_to(self.visible.len().saturating_sub(1)),
            KeyCode::Enter => self.enter(),
            _ => {}
        }
        false
    }

    /// One click on a plugin selects it and shows its details, as in VS
    /// Code, and the sidebar keeps the focus; the wheel scrolls the list
    /// without moving the selection.
    pub fn handle_mouse(&mut self, mouse: MouseEvent, width: u16, height: u16) {
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                match sidebar_view::hit(self, width, height, mouse.column, mouse.row) {
                    Some(Hit::Row(position)) => {
                        self.move_to(position);
                        self.open(Reveal::Preview);
                    }
                    Some(Hit::Update(position)) => self.update_at(position),
                    Some(Hit::Retry) => self.enter(),
                    Some(Hit::Filter(filter)) => self.set_filter(filter),
                    Some(Hit::Clear) => {
                        self.query.clear();
                        self.search_changed();
                    }
                    None => {}
                }
            }
            MouseEventKind::ScrollDown => self.scroll_by(1),
            MouseEventKind::ScrollUp => self.scroll_by(-1),
            _ => {}
        }
    }

    pub fn focus(&mut self, focused: bool) {
        self.focused = focused;
    }

    /// An update of this source runs, launched from here or from a details
    /// pane: its card shows it instead of the button.
    pub fn is_updating(&self, source: &PluginSource) -> bool {
        self.updating.as_ref() == Some(source) || self.running_updates.contains(source)
    }

    pub fn set_running_updates(&mut self, sources: HashSet<PluginSource>) {
        self.running_updates = sources;
    }

    /// The checks refused the update: nothing ran.
    pub fn update_refused(&mut self, reason: &str) {
        if let Some(source) = self.updating.take() {
            let name = self.name_of(&source);
            self.notice = Some(Notice {
                text: format!("Update of {name} refused: {reason}"),
                color: ERROR,
            });
        }
    }

    /// The worker could not start: nothing ran.
    pub fn update_not_started(&mut self, text: String) {
        self.updating = None;
        self.notice = Some(Notice { text, color: ERROR });
    }

    /// The result of the update launched from here. Its card waits for the
    /// registry read that follows to show the new state.
    pub fn update_finished(&mut self, record: &OperationRecord) {
        self.update_ended = true;
        let request = &record.request;
        let name = self.name_of(&request.source);
        let manifest = match &request.confirmation {
            Some(Confirmation::Install { manifest, .. }) => Some(manifest),
            _ => None,
        };
        let version = manifest.map_or_else(
            || request.commit.chars().take(7).collect(),
            |manifest| manifest.version.clone(),
        );
        let code = record
            .exit_code
            .map_or("no exit code".to_string(), |code| format!("code {code}"));
        let (text, color) = match record.status {
            Status::Succeeded if manifest.is_some_and(|manifest| manifest.id == PLUGIN_ID) => (
                format!("Marketplace updated to {version}. Close and reopen it to use it."),
                OK,
            ),
            Status::Succeeded => (
                format!("{name} updated to {version}. Reopen its panes to use it."),
                OK,
            ),
            Status::Failed => (
                format!("Update of {name} failed ({code}). Open it to see Herdr's output."),
                ERROR,
            ),
            Status::Unconfirmed => (
                format!("Update of {name}: result not confirmed. Open it to check."),
                WARN,
            ),
            Status::Refused => {
                let reason = record
                    .output
                    .lines()
                    .find(|line| !line.trim().is_empty())
                    .unwrap_or_default();
                (format!("Update of {name} refused: {reason}"), ERROR)
            }
            Status::Running => return,
        };
        self.notice = Some(Notice { text, color });
    }

    /// The catalogue name of the row of `source`, else the source itself.
    fn name_of(&self, source: &PluginSource) -> String {
        self.rows()
            .iter()
            .find(|row| row.entry.source.same_source(source))
            .map_or_else(|| source.to_string(), |row| row.entry.name.clone())
    }

    /// The Update button of a card: no preview, no details, the selection
    /// stays. One update at a time.
    fn update_at(&mut self, position: usize) {
        if self.updating.is_some() {
            self.notice = Some(Notice {
                text: "Another marketplace operation is running: update refused".into(),
                color: ERROR,
            });
            return;
        }
        let row = self.rows()[self.visible[position]].clone();
        self.updating = Some(row.entry.source.clone());
        self.update_ended = false;
        self.notice = None;
        self.intents.push(Intent::Update(Box::new(row)));
    }

    /// Another filter starts from its first plugin, as a new search does.
    pub fn set_filter(&mut self, filter: Filter) {
        self.filter = filter;
        self.search_changed();
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

    /// A new height brings the selection back into view; otherwise a list
    /// scrolled with the wheel stays where it is.
    pub fn set_page(&mut self, page: usize) {
        let page = page.max(1);
        if page != self.page {
            self.page = page;
            self.ensure_visible();
        }
        self.offset = self.offset.min(self.last_offset());
    }

    fn enter(&mut self) {
        match &self.state {
            LoadState::Failed(_) => {
                self.state = LoadState::Loading;
                self.intents.push(Intent::Load);
            }
            LoadState::Ready(_) => self.open(Reveal::Focus),
            LoadState::Loading => {}
        }
    }

    fn open(&mut self, reveal: Reveal) {
        if let Some(row) = self.selected_row() {
            self.intents
                .push(Intent::Open(Box::new(row.clone()), reveal));
        }
    }

    /// `@installed` typed as a word switches the filter and leaves the
    /// search.
    fn take_filter_token(&mut self) {
        let words: Vec<&str> = self.query.split(' ').collect();
        if !words
            .iter()
            .any(|word| word.eq_ignore_ascii_case(INSTALLED_TOKEN))
        {
            return;
        }
        self.query = words
            .into_iter()
            .filter(|word| !word.eq_ignore_ascii_case(INSTALLED_TOKEN))
            .collect::<Vec<_>>()
            .join(" ")
            .trim_start()
            .to_string();
        self.filter = Filter::Installed;
    }

    /// A new search starts from its most relevant result, and open details
    /// follow it.
    fn search_changed(&mut self) {
        self.selected = None;
        self.refilter();
        if let Some(row) = self.selected_row() {
            self.intents.push(Intent::Follow(Box::new(row.clone())));
        }
    }

    fn refilter(&mut self) {
        let (rows, hidden): (&[Row], &[_]) = match &self.state {
            LoadState::Ready(loaded) => (&loaded.listing.rows[..], &loaded.listing.hidden[..]),
            _ => (&[], &[]),
        };
        let (all, hidden): (Vec<usize>, Vec<usize>) =
            search(rows.iter().map(|row| &row.entry).chain(hidden), &self.query)
                .into_iter()
                .partition(|&position| position < rows.len());
        let installed: Vec<usize> = all
            .iter()
            .copied()
            .filter(|&index| rows[index].installed.is_some())
            .collect();
        self.counts = Counts {
            all: all.len(),
            installed: installed.len(),
        };
        let visible = match self.filter {
            Filter::All => all,
            Filter::Installed => installed,
        };
        let first = visible
            .first()
            .map(|&index| rows[index].entry.source.clone());
        self.visible = visible;
        self.hidden = hidden.len();
        if self.selected_position().is_none() {
            self.selected = first;
            self.offset = 0;
        }
        self.ensure_visible();
    }

    fn browse_by(&mut self, delta: isize) {
        let position = self.selected_position().unwrap_or(0) as isize;
        self.browse_to((position + delta).max(0) as usize);
    }

    /// The keyboard moves the selection and the details follow it without
    /// the focus; a click selects and opens instead.
    fn browse_to(&mut self, position: usize) {
        self.move_to(position);
        if let Some(row) = self.selected_row() {
            self.intents.push(Intent::Preview(Box::new(row.clone())));
        }
    }

    fn move_to(&mut self, position: usize) {
        let Some(last) = self.visible.len().checked_sub(1) else {
            return;
        };
        let index = self.visible[position.min(last)];
        self.selected = Some(self.rows()[index].entry.source.clone());
        self.ensure_visible();
    }

    fn scroll_by(&mut self, delta: isize) {
        self.offset = (self.offset as isize + delta).clamp(0, self.last_offset() as isize) as usize;
    }

    fn last_offset(&self) -> usize {
        self.visible.len().saturating_sub(self.page)
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
