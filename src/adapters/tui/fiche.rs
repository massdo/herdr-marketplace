use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::text::Line;

use super::markdown;
use super::preview::preview_lines;
use crate::application::load_readme::Readme;
use crate::application::prepare_install::{InstallPreview, Prepared};
use crate::domain::compat::Platform;
use crate::domain::fiche::FicheTarget;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadmeState {
    Loading,
    Found { text: String, fallback: bool },
    NotFound,
    NetworkError(String),
}

/// Install request, from the key that opens the preview to the confirmation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallState {
    Idle,
    Preparing,
    /// « Installé »: this source is already installed at this commit.
    UpToDate,
    Refused(String),
    Preview(Box<InstallPreview>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FicheIntent {
    /// Load the README; the answer carries this request number.
    LoadReadme(u64),
    /// Build the install preview; the answer carries this request number.
    PrepareInstall(u64),
    /// Confirmed request: the arguments of `herdr`.
    Install(Vec<String>),
}

/// Fiche state: the plugin and commit received at opening, its README and
/// the install request.
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
    /// Body lines the pane shows.
    pub page: usize,
    pub full_sha: bool,
    pub install: InstallState,
    pub install_request: u64,
    /// Preview rendered for `width`, shown instead of the README.
    pub preview: Vec<Line<'static>>,
    pub preview_scroll: usize,
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
            install: InstallState::Idle,
            install_request: 0,
            preview: Vec::new(),
            preview_scroll: 0,
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

    pub fn install_prepared(&mut self, request: u64, prepared: Prepared) {
        if request != self.install_request || self.install != InstallState::Preparing {
            return;
        }
        self.install = match prepared {
            Prepared::UpToDate => InstallState::UpToDate,
            Prepared::Refused(reason) => InstallState::Refused(reason),
            Prepared::Preview(preview) => InstallState::Preview(preview),
        };
        self.preview_scroll = 0;
        self.render();
    }

    pub fn set_viewport(&mut self, width: usize, page: usize) {
        if width != self.width {
            self.width = width;
            self.render();
        }
        self.page = page.max(1);
        self.scroll_by(0);
    }

    /// Returns true when the fiche should close.
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            return key.code == KeyCode::Char('c');
        }
        match key.code {
            KeyCode::Esc if self.install == InstallState::Idle => return true,
            KeyCode::Esc => self.leave_install(),
            KeyCode::Enter => self.enter(),
            KeyCode::Char('i') => self.prepare_install(),
            KeyCode::Char('s') => self.full_sha = !self.full_sha,
            KeyCode::Up => self.scroll_by(-1),
            KeyCode::Down => self.scroll_by(1),
            KeyCode::PageUp => self.scroll_by(-(self.page as isize)),
            KeyCode::PageDown => self.scroll_by(self.page as isize),
            KeyCode::Home => self.scroll_by(isize::MIN / 2),
            KeyCode::End => self.scroll_by(isize::MAX / 2),
            _ => {}
        }
        false
    }

    pub fn showing_preview(&self) -> bool {
        matches!(self.install, InstallState::Preview(_))
    }

    fn enter(&mut self) {
        if let InstallState::Preview(preview) = &self.install {
            self.intents
                .push(FicheIntent::Install(preview.args.clone()));
            self.leave_install();
        } else if matches!(self.readme, ReadmeState::NetworkError(_)) {
            self.request += 1;
            self.readme = ReadmeState::Loading;
            self.lines.clear();
            self.intents.push(FicheIntent::LoadReadme(self.request));
        }
    }

    fn prepare_install(&mut self) {
        if matches!(
            self.install,
            InstallState::Preparing | InstallState::Preview(_)
        ) {
            return;
        }
        self.install_request += 1;
        self.install = InstallState::Preparing;
        self.intents
            .push(FicheIntent::PrepareInstall(self.install_request));
    }

    /// Escape out of the preview launches nothing.
    fn leave_install(&mut self) {
        self.install = InstallState::Idle;
        self.preview.clear();
    }

    fn render(&mut self) {
        self.lines = match &self.readme {
            ReadmeState::Found { text, .. } => markdown::render(text, self.width),
            _ => Vec::new(),
        };
        self.preview = match &self.install {
            InstallState::Preview(preview) => {
                preview_lines(preview, self.width, Platform::current())
            }
            _ => Vec::new(),
        };
    }

    fn scroll_by(&mut self, delta: isize) {
        let (scroll, len) = if self.showing_preview() {
            (&mut self.preview_scroll, self.preview.len())
        } else {
            (&mut self.scroll, self.lines.len())
        };
        let max = len.saturating_sub(self.page);
        *scroll = (*scroll as isize)
            .saturating_add(delta)
            .clamp(0, max as isize) as usize;
    }
}
