use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::text::Line;

use super::markdown;
use super::preview::preview_lines;
use crate::application::load_readme::Readme;
use crate::application::prepare_install::{InstallPreview, Prepared};
use crate::domain::compat::Platform;
use crate::domain::details::DetailsTarget;
use crate::domain::operation::{OperationKind, OperationRecord, Status};
use crate::domain::uninstall::RemovalPlan;

/// The source of the details pane as the registry shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstalledView {
    Unknown,
    /// Installed at this commit.
    At(String),
    NotInstalled,
    /// Registry unreadable, or several plugins installed from this source.
    Uncertain(String),
}

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
    /// This source is already installed at this commit: nothing to do.
    UpToDate,
    Refused(String),
    Preview(Box<InstallPreview>),
}

/// Removal request, from the key that asks for it to the second
/// confirmation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemovalState {
    Idle,
    Preparing,
    Refused(String),
    Confirm(Box<RemovalPlan>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetailsIntent {
    /// Load the README; the answer carries this request number.
    LoadReadme(u64),
    /// Build the install preview; the answer carries this request number.
    PrepareInstall(u64),
    /// Confirmed request: the arguments of `herdr`.
    Install(Vec<String>),
    /// Read the registry; the answer carries this request number.
    ReadRegistry(u64),
    /// Find the installed plugin to remove; the answer carries this number.
    PrepareRemoval(u64),
    /// Confirmed removal: the arguments of `herdr`.
    Uninstall(Vec<String>),
}

/// Details pane state: the plugin and commit received at opening, its README, and
/// the install or removal request.
#[derive(Debug, Clone)]
pub struct DetailsApp {
    pub target: DetailsTarget,
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
    pub removal: RemovalState,
    pub removal_request: u64,
    pub installed: InstalledView,
    pub registry_request: u64,
    /// Latest kept result of an operation on this source.
    pub operation: Option<OperationRecord>,
    /// Operation launched from this pane whose result has not appeared yet.
    pub launched: Option<(String, OperationKind)>,
    /// Why the last confirmation launched nothing.
    pub notice: Option<String>,
    pub intents: Vec<DetailsIntent>,
}

impl DetailsApp {
    pub fn new(target: DetailsTarget) -> Self {
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
            removal: RemovalState::Idle,
            removal_request: 0,
            installed: InstalledView::Unknown,
            registry_request: 1,
            operation: None,
            launched: None,
            notice: None,
            intents: vec![DetailsIntent::LoadReadme(1), DetailsIntent::ReadRegistry(1)],
        }
    }

    pub fn registry_read(&mut self, request: u64, installed: InstalledView) {
        if request == self.registry_request {
            self.installed = installed;
        }
    }

    /// Latest kept result for this source, looked at on every tick. When an
    /// operation ends, the registry is read again.
    pub fn operation_seen(&mut self, record: Option<OperationRecord>) {
        let was_running = self.operation_running();
        let launched = self.launched.as_ref().map(|(id, _)| id);
        if record.as_ref().map(|record| &record.request.id) == launched {
            self.launched = None;
        }
        self.operation = record;
        if was_running && !self.operation_running() {
            self.registry_request += 1;
            self.intents
                .push(DetailsIntent::ReadRegistry(self.registry_request));
        }
    }

    pub fn operation_launched(&mut self, id: String, kind: OperationKind) {
        self.launched = Some((id, kind));
        self.notice = None;
    }

    pub fn operation_refused(&mut self, reason: String) {
        self.notice = Some(reason);
    }

    pub fn operation_running(&self) -> bool {
        self.launched.is_some()
            || self
                .operation
                .as_ref()
                .is_some_and(|record| record.status == Status::Running)
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

    pub fn removal_prepared(&mut self, request: u64, plan: Result<RemovalPlan, String>) {
        if request != self.removal_request || self.removal != RemovalState::Preparing {
            return;
        }
        self.removal = match plan {
            Ok(plan) => RemovalState::Confirm(Box::new(plan)),
            Err(reason) => RemovalState::Refused(reason),
        };
    }

    pub fn set_viewport(&mut self, width: usize, page: usize) {
        if width != self.width {
            self.width = width;
            self.render();
        }
        self.page = page.max(1);
        self.scroll_by(0);
    }

    /// Returns true when the details pane should close.
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            return key.code == KeyCode::Char('c');
        }
        match key.code {
            KeyCode::Esc if self.install != InstallState::Idle => self.leave_install(),
            KeyCode::Esc if self.removal != RemovalState::Idle => {
                self.removal = RemovalState::Idle;
            }
            KeyCode::Esc => return true,
            KeyCode::Enter => self.enter(),
            KeyCode::Char('i') => self.prepare_install(),
            KeyCode::Char('r') => self.prepare_removal(),
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
                .push(DetailsIntent::Install(preview.args.clone()));
            self.leave_install();
        } else if let RemovalState::Confirm(plan) = &self.removal {
            self.intents
                .push(DetailsIntent::Uninstall(plan.args.clone()));
            self.removal = RemovalState::Idle;
        } else if matches!(self.readme, ReadmeState::NetworkError(_)) {
            self.request += 1;
            self.readme = ReadmeState::Loading;
            self.lines.clear();
            self.intents.push(DetailsIntent::LoadReadme(self.request));
        }
    }

    fn prepare_install(&mut self) {
        if matches!(
            self.install,
            InstallState::Preparing | InstallState::Preview(_)
        ) {
            return;
        }
        self.removal = RemovalState::Idle;
        self.install_request += 1;
        self.install = InstallState::Preparing;
        self.intents
            .push(DetailsIntent::PrepareInstall(self.install_request));
    }

    /// `r` asks for the removal; a second key, Enter, confirms it.
    fn prepare_removal(&mut self) {
        if matches!(
            self.removal,
            RemovalState::Preparing | RemovalState::Confirm(_)
        ) {
            return;
        }
        self.leave_install();
        self.removal_request += 1;
        self.removal = RemovalState::Preparing;
        self.intents
            .push(DetailsIntent::PrepareRemoval(self.removal_request));
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
