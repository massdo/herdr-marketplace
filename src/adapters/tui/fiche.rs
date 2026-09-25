use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::text::Line;

use super::markdown;
use crate::application::load_readme::Readme;
use crate::domain::fiche::FicheTarget;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadmeState {
    Loading,
    Found { text: String, fallback: bool },
    NotFound,
    NetworkError(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FicheIntent {
    /// Load the README; the answer carries this request number.
    LoadReadme(u64),
}

/// Fiche state: the plugin and commit received at opening, and its README.
#[derive(Debug, Clone)]
pub struct FicheApp {
    pub target: FicheTarget,
    pub readme: ReadmeState,
    /// Number of the latest README request; older answers are dropped.
    pub request: u64,
    /// README rendered for `width`.
    pub lines: Vec<Line<'static>>,
    pub width: usize,
    pub scroll: usize,
    /// README lines the pane shows.
    pub page: usize,
    pub full_sha: bool,
    pub intents: Vec<FicheIntent>,
}

impl FicheApp {
    pub fn new(target: FicheTarget) -> Self {
        Self {
            target,
            readme: ReadmeState::Loading,
            request: 1,
            lines: Vec::new(),
            width: 80,
            scroll: 0,
            page: 1,
            full_sha: false,
            intents: vec![FicheIntent::LoadReadme(1)],
        }
    }

    pub fn readme_loaded(&mut self, request: u64, result: Result<Readme, String>) {
        if request != self.request {
            return;
        }
        self.readme = match result {
            Ok(Readme::Found { text, fallback }) => ReadmeState::Found { text, fallback },
            Ok(Readme::NotFound) => ReadmeState::NotFound,
            Err(error) => ReadmeState::NetworkError(error),
        };
        self.scroll = 0;
        self.render();
    }

    pub fn set_viewport(&mut self, width: usize, page: usize) {
        if width != self.width {
            self.width = width;
            self.render();
        }
        self.page = page.max(1);
        self.scroll = self.scroll.min(self.max_scroll());
    }

    /// Returns true when the fiche should close.
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            return key.code == KeyCode::Char('c');
        }
        match key.code {
            KeyCode::Esc => return true,
            KeyCode::Up => self.scroll_by(-1),
            KeyCode::Down => self.scroll_by(1),
            KeyCode::PageUp => self.scroll_by(-(self.page as isize)),
            KeyCode::PageDown => self.scroll_by(self.page as isize),
            KeyCode::Home => self.scroll = 0,
            KeyCode::End => self.scroll = self.max_scroll(),
            KeyCode::Char('s') => self.full_sha = !self.full_sha,
            KeyCode::Enter if matches!(self.readme, ReadmeState::NetworkError(_)) => {
                self.request += 1;
                self.readme = ReadmeState::Loading;
                self.lines.clear();
                self.intents.push(FicheIntent::LoadReadme(self.request));
            }
            _ => {}
        }
        false
    }

    fn render(&mut self) {
        self.lines = match &self.readme {
            ReadmeState::Found { text, .. } => markdown::render(text, self.width),
            _ => Vec::new(),
        };
    }

    fn max_scroll(&self) -> usize {
        self.lines.len().saturating_sub(self.page)
    }

    fn scroll_by(&mut self, delta: isize) {
        let next = (self.scroll as isize + delta).max(0) as usize;
        self.scroll = next.min(self.max_scroll());
    }
}
