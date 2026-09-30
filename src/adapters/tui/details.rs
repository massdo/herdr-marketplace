use std::collections::HashSet;
use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::text::Line;

use super::details_view;
use super::graphics::{Graphics, Pictures};
use super::markdown::{self, LinkArea, Place, VideoPlace};
use super::preview::preview_lines;
use super::selection::{Flow, Selection};
use super::style::Tone;
use super::video::VideoEvent;
use crate::adapters::image_fetch::Probe;
use crate::adapters::images::Picture;
use crate::application::load_readme::Readme;
use crate::application::prepare_install::{InstallPreview, Prepared};
use crate::domain::compat::Platform;
use crate::domain::details::DetailsTarget;
use crate::domain::install::Plan;
use crate::domain::operation::{OperationKind, OperationRecord, Status};
use crate::domain::readme::{LinkTarget, ReadmePlace, github_page};
use crate::domain::uninstall::RemovalPlan;

/// Lines the wheel scrolls per step.
const WHEEL: isize = 3;

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

/// What a button, or the key written on it, does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// Build the install preview; nothing runs yet.
    Install,
    /// Ask for the removal; nothing runs yet.
    Remove,
    /// Run the install or removal on screen.
    Confirm,
    /// Leave the preview or the removal without running anything.
    Cancel,
    RetryReadme,
    ToggleSha,
    /// The plugin's page on GitHub, in the browser.
    OpenGitHub,
    /// The link of this area of the README.
    OpenLink(usize),
    /// Play the video of this block of the README, instead of the one that
    /// plays.
    PlayVideo(usize),
    /// Stop the video that plays.
    StopVideo,
}

/// The video of the pane: one at a time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoView {
    pub url: String,
    pub status: VideoStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VideoStatus {
    /// Bytes downloaded, of the whole size when known.
    Downloading(u64, Option<u64>),
    Playing,
    /// It stopped for this reason; its button stays, to try again.
    Failed(String),
}

/// A button of the action bar, under the header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Button {
    pub label: String,
    pub key: &'static str,
    pub tone: Tone,
    pub command: Command,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetailsIntent {
    /// Load the README; the answer carries this request number.
    LoadReadme(u64),
    /// Build the install preview; the answer carries this request number.
    PrepareInstall(u64),
    /// Confirmed preview, including the state the user reviewed.
    Install(Box<InstallPreview>),
    /// Read the registry; the answer carries this request number.
    ReadRegistry(u64),
    /// Find the installed plugin to remove; the answer carries this number.
    PrepareRemoval(u64),
    /// Confirmed removal and the exact installed plugin the user reviewed.
    Uninstall(Box<RemovalPlan>),
    /// Download and decode these images of the README.
    LoadImages(Vec<String>),
    /// Ask the type and size of these videos of the README.
    ProbeVideos(Vec<String>),
    /// Download this video and play it in frames of `fit` pixels at most,
    /// instead of the one that plays.
    PlayVideo {
        url: String,
        looped: bool,
        fit: (u32, u32),
    },
    StopVideo,
    /// Open this address in the browser.
    OpenUrl(String),
    /// Put this text on the clipboard.
    Copy(String),
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
    /// How each of `lines` reads in a copy.
    pub flows: Vec<Flow>,
    /// Parts of `lines` a click opens.
    pub links: Vec<LinkArea>,
    /// First line of each README heading, by anchor.
    pub anchors: Vec<(String, usize)>,
    /// Where the README draws its images.
    pub places: Vec<Place>,
    /// Where the videos of the README play.
    pub video_places: Vec<VideoPlace>,
    pub video: Option<VideoView>,
    pub pictures: Pictures,
    /// Animated images whose frames Herdr draws over their cells, which stay
    /// empty: their first frame would show through transparent pixels.
    pub covered: HashSet<String>,
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
    /// Text the mouse is selecting, copied when the button is released.
    pub selection: Option<Selection>,
    pub intents: Vec<DetailsIntent>,
}

impl DetailsApp {
    pub fn new(target: DetailsTarget) -> Self {
        Self {
            target,
            readme: ReadmeState::Loading,
            request: 1,
            lines: Vec::new(),
            flows: Vec::new(),
            links: Vec::new(),
            anchors: Vec::new(),
            places: Vec::new(),
            video_places: Vec::new(),
            video: None,
            pictures: Pictures::default(),
            covered: HashSet::new(),
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
            selection: None,
            intents: vec![DetailsIntent::LoadReadme(1), DetailsIntent::ReadRegistry(1)],
        }
    }

    pub fn registry_read(&mut self, request: u64, installed: InstalledView) {
        if request == self.registry_request {
            self.installed = installed;
        }
    }

    /// Latest kept result for this source, looked at once per second. When an
    /// operation ends, the registry is read again.
    pub fn operation_seen(&mut self, record: Option<OperationRecord>) {
        // Keep a result received directly from the worker when persistence
        // failed. An old file must not replace it with Running again.
        if self.operation.as_ref().is_some_and(|current| {
            current.persistence_error.is_some()
                && record.as_ref().is_none_or(|saved| {
                    saved.request.id == current.request.id
                        || (saved.status != Status::Running
                            && saved.finished_unix_ms <= current.finished_unix_ms)
                })
        }) {
            return;
        }
        let was_running = self.operation_running();
        let launched = self.launched.as_ref().map(|(id, _)| id);
        if record.as_ref().map(|record| &record.request.id) == launched {
            self.launched = None;
        }
        self.operation = record;
        let orphaned = self.operation.as_ref().is_some_and(|record| {
            record.status == Status::Unconfirmed && record.finished_unix_ms.is_none()
        });
        if (was_running && !self.operation_running()) || orphaned {
            self.registry_request += 1;
            self.intents
                .push(DetailsIntent::ReadRegistry(self.registry_request));
        }
    }

    /// The worker exited, including before it could create a result file.
    pub fn operation_finished(&mut self, record: OperationRecord) {
        self.launched = None;
        self.operation = Some(record);
        self.scroll = 0;
        self.registry_request += 1;
        self.intents
            .push(DetailsIntent::ReadRegistry(self.registry_request));
    }

    pub fn operation_launched(&mut self, id: String, kind: OperationKind) {
        self.launched = Some((id, kind));
        self.notice = None;
    }

    pub fn operation_refused(&mut self, reason: String) {
        self.notice = Some(reason);
        self.scroll = 0;
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

    /// The terminal is known: images are laid out again, drawn this way.
    pub fn set_graphics(&mut self, graphics: Graphics) {
        self.pictures.decide(graphics);
        self.render();
    }

    /// An image of the README arrived: the README is laid out again.
    pub fn picture_loaded(&mut self, url: &str, picture: Result<Arc<Picture>, String>) {
        self.pictures.loaded(url, picture);
        self.render();
    }

    /// Whether the pane can play videos: FFmpeg, and a pane Herdr knows.
    pub fn video_player(&mut self, available: bool) {
        self.pictures.video_player(available);
        self.render();
    }

    /// A video of the README was probed: the README is laid out again.
    pub fn video_probed(&mut self, url: &str, probe: Result<Probe, String>) {
        self.pictures.video_probed(url, probe);
        self.render();
    }

    /// News of the video that plays; once it ends, its button comes back.
    pub fn video_event(&mut self, event: VideoEvent) {
        let Some(video) = &mut self.video else {
            return;
        };
        video.status = match event {
            VideoEvent::Downloading(received, total) => VideoStatus::Downloading(received, total),
            VideoEvent::Playing => VideoStatus::Playing,
            VideoEvent::Failed(reason) => VideoStatus::Failed(reason),
            VideoEvent::Ended => {
                self.video = None;
                return;
            }
        };
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
        self.preview_scroll = 0;
    }

    pub fn set_viewport(&mut self, width: usize, page: usize) {
        if width != self.width {
            self.width = width;
            self.render();
        }
        self.page = page;
        self.scroll_by(0);
    }

    /// Returns true when the details pane should close.
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            return key.code == KeyCode::Char('c');
        }
        match key.code {
            KeyCode::Esc
                if self.install != InstallState::Idle || self.removal != RemovalState::Idle =>
            {
                self.press(Command::Cancel)
            }
            KeyCode::Char('q') => return true,
            KeyCode::Enter if self.showing_confirmation() => self.press(Command::Confirm),
            KeyCode::Enter => self.press(Command::RetryReadme),
            KeyCode::Char('i') => self.press(Command::Install),
            KeyCode::Char('r') => self.press(Command::Remove),
            KeyCode::Char('s') => self.press(Command::ToggleSha),
            KeyCode::Char('o') => self.press(Command::OpenGitHub),
            KeyCode::Char('p') => self.toggle_video(),
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

    /// A click runs the button or the link under it; a drag from elsewhere
    /// selects text, copied when the button is released; the wheel scrolls.
    pub fn handle_mouse(&mut self, mouse: MouseEvent, width: u16, height: u16) {
        let (column, row) = cell(mouse, width, height);
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.selection = None;
                match details_view::hit(self, width, height, mouse.column, mouse.row) {
                    Some(command) => self.press(command),
                    None => self.selection = Some(Selection::new(column, row)),
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(selection) = &mut self.selection {
                    selection.drag(column, row);
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if let Some(selection) = self.selection.take()
                    && selection.dragged()
                {
                    let text = details_view::selected_text(self, &selection, width, height);
                    if !text.is_empty() {
                        self.intents.push(DetailsIntent::Copy(text));
                    }
                }
            }
            MouseEventKind::ScrollDown => self.scroll_by(WHEEL),
            MouseEventKind::ScrollUp => self.scroll_by(-WHEEL),
            _ => {}
        }
    }

    /// The click that gave the pane the focus runs nothing, but a drag from
    /// it selects text, as in any Herdr pane.
    pub fn focus_click(&mut self, mouse: MouseEvent, width: u16, height: u16) {
        if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
            let (column, row) = cell(mouse, width, height);
            self.selection = Some(Selection::new(column, row));
        }
    }

    /// Runs a button, clicked or through its key.
    pub fn press(&mut self, command: Command) {
        match command {
            Command::Install => self.prepare_install(),
            Command::Remove => self.prepare_removal(),
            Command::Confirm => self.confirm(),
            Command::Cancel => {
                self.leave_install();
                self.removal = RemovalState::Idle;
            }
            Command::RetryReadme => self.retry_readme(),
            Command::ToggleSha => self.full_sha = !self.full_sha,
            Command::OpenGitHub => self.intents.push(DetailsIntent::OpenUrl(github_page(
                &self.target.source,
                &self.target.commit,
            ))),
            Command::OpenLink(area) => match self.links.get(area).map(|area| area.target.clone()) {
                Some(LinkTarget::Web(url)) => self.intents.push(DetailsIntent::OpenUrl(url)),
                Some(LinkTarget::Anchor(anchor)) => self.go_to(&anchor),
                None => {}
            },
            Command::PlayVideo(index) => self.play_video(index),
            Command::StopVideo => {
                if self.video.take().is_some() {
                    self.intents.push(DetailsIntent::StopVideo);
                }
            }
        }
    }

    /// The video of block `index` downloads, then plays in frames that fit
    /// in the pixels of its cells.
    fn play_video(&mut self, index: usize) {
        let (
            Some(place),
            Some(Graphics::Kitty {
                cell_width,
                cell_height,
            }),
        ) = (self.video_places.get(index), self.pictures.graphics())
        else {
            return;
        };
        let fit = (
            u32::from(place.columns) * u32::from(cell_width),
            u32::from(place.rows) * u32::from(cell_height),
        );
        self.video = Some(VideoView {
            url: place.url.clone(),
            status: VideoStatus::Downloading(0, None),
        });
        self.intents.push(DetailsIntent::PlayVideo {
            url: place.url.clone(),
            looped: place.looped,
            fit,
        });
    }

    /// `p` stops the video that plays; else it plays the first video of
    /// the README with a line of its block on screen.
    fn toggle_video(&mut self) {
        if self
            .video
            .as_ref()
            .is_some_and(|video| !matches!(video.status, VideoStatus::Failed(_)))
        {
            self.press(Command::StopVideo);
            return;
        }
        if self.showing_confirmation() {
            return;
        }
        let (prefix, _, _) = details_view::body_lines(self, self.width);
        let shown = self.scroll..self.scroll + self.page;
        let visible = self.video_places.iter().position(|place| {
            let first = prefix.len() + place.line;
            first < shown.end && shown.start < first + usize::from(place.rows)
        });
        if let Some(index) = visible {
            self.press(Command::PlayVideo(index));
        }
    }

    /// Scrolls to the heading of `anchor`, as a link to `#anchor` does.
    fn go_to(&mut self, anchor: &str) {
        let Some(&(_, line)) = self.anchors.iter().find(|(name, _)| name == anchor) else {
            return;
        };
        let (prefix, _, _) = details_view::body_lines(self, self.width);
        self.scroll = prefix.len() + line;
        self.scroll_by(0);
    }

    /// What can be done now, each button with its key. A confirmation shows
    /// only Confirm and Cancel; a running operation, only the GitHub page.
    pub fn buttons(&self) -> Vec<Button> {
        let button = |label: &str, key, tone, command| Button {
            label: label.to_string(),
            key,
            tone,
            command,
        };
        let github = button("Open on GitHub", "o", Tone::Plain, Command::OpenGitHub);
        if self.operation_running() {
            return vec![github];
        }
        if let InstallState::Preview(preview) = &self.install {
            let label = match preview.plan {
                Plan::Install => "Confirm install",
                Plan::Switch { .. } => "Confirm switch",
            };
            return vec![
                button(label, "Enter", Tone::Primary, Command::Confirm),
                button("Cancel", "Esc", Tone::Plain, Command::Cancel),
            ];
        }
        if matches!(self.removal, RemovalState::Confirm(_)) {
            return vec![
                button("Confirm removal", "Enter", Tone::Danger, Command::Confirm),
                button("Cancel", "Esc", Tone::Plain, Command::Cancel),
            ];
        }
        if self.install == InstallState::Preparing || self.removal == RemovalState::Preparing {
            return vec![button("Cancel", "Esc", Tone::Plain, Command::Cancel)];
        }
        let install = if self.target.compatible {
            Tone::Primary
        } else {
            Tone::Plain
        };
        let catalog = self.target.in_catalog;
        let mut buttons = Vec::new();
        match &self.installed {
            InstalledView::Unknown => {}
            InstalledView::NotInstalled if catalog => {
                buttons.push(button("Install", "i", install, Command::Install));
            }
            InstalledView::NotInstalled => {}
            InstalledView::At(sha) if *sha == self.target.commit => {
                buttons.push(button("Remove", "r", Tone::Plain, Command::Remove));
            }
            InstalledView::At(_) => {
                if catalog {
                    let commit: String = self.target.commit.chars().take(7).collect();
                    let label = format!("Switch to {commit}");
                    buttons.push(button(&label, "i", install, Command::Install));
                }
                buttons.push(button("Remove", "r", Tone::Plain, Command::Remove));
            }
            InstalledView::Uncertain(_) => {
                if catalog {
                    buttons.push(button("Install", "i", Tone::Plain, Command::Install));
                }
                buttons.push(button("Remove", "r", Tone::Plain, Command::Remove));
            }
        }
        if matches!(self.readme, ReadmeState::NetworkError(_)) {
            buttons.push(button(
                "Retry README",
                "Enter",
                Tone::Plain,
                Command::RetryReadme,
            ));
        }
        buttons.push(github);
        buttons
    }

    pub fn showing_preview(&self) -> bool {
        matches!(self.install, InstallState::Preview(_))
    }

    pub fn showing_confirmation(&self) -> bool {
        self.showing_preview() || matches!(self.removal, RemovalState::Confirm(_))
    }

    /// Runs the install or removal on screen, never one the pane hides.
    fn confirm(&mut self) {
        if self.page == 0 || self.width == 0 {
            return;
        }
        if let InstallState::Preview(preview) = &self.install {
            self.intents.push(DetailsIntent::Install(preview.clone()));
            self.leave_install();
        } else if let RemovalState::Confirm(plan) = &self.removal {
            self.intents.push(DetailsIntent::Uninstall(plan.clone()));
            self.removal = RemovalState::Idle;
        }
    }

    fn retry_readme(&mut self) {
        if matches!(self.readme, ReadmeState::NetworkError(_)) {
            self.request += 1;
            self.readme = ReadmeState::Loading;
            self.lines.clear();
            self.flows.clear();
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
        self.scroll = 0;
        self.install_request += 1;
        self.install = InstallState::Preparing;
        self.intents
            .push(DetailsIntent::PrepareInstall(self.install_request));
    }

    /// Remove asks for the removal; Confirm runs it.
    fn prepare_removal(&mut self) {
        if matches!(
            self.removal,
            RemovalState::Preparing | RemovalState::Confirm(_)
        ) {
            return;
        }
        self.leave_install();
        self.scroll = 0;
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
        self.pictures.begin_layout();
        let rendered = match &self.readme {
            ReadmeState::Found { text, fallback } => {
                let place = ReadmePlace {
                    source: self.target.source.clone(),
                    commit: self.target.commit.clone(),
                    folder: if *fallback {
                        String::new()
                    } else {
                        self.target.source.subdir.clone()
                    },
                };
                markdown::render_readme(text, self.width, Some(&place), &mut self.pictures)
            }
            _ => markdown::Rendered::default(),
        };
        self.lines = rendered.lines;
        self.flows = rendered.flows;
        self.links = rendered.links;
        self.anchors = rendered.anchors;
        self.places = rendered.places;
        self.video_places = rendered.video_places;
        let wanted: Vec<String> = rendered
            .images
            .into_iter()
            .filter(|url| self.pictures.request(url))
            .collect();
        if !wanted.is_empty() {
            self.intents.push(DetailsIntent::LoadImages(wanted));
        }
        let probes: Vec<String> = rendered
            .videos
            .into_iter()
            .filter(|url| self.pictures.request_video(url))
            .collect();
        if !probes.is_empty() {
            self.intents.push(DetailsIntent::ProbeVideos(probes));
        }
        self.preview = match &self.install {
            InstallState::Preview(preview) => {
                preview_lines(preview, self.width, Platform::current())
            }
            _ => Vec::new(),
        };
    }

    fn scroll_by(&mut self, delta: isize) {
        let (prefix, content, _) = details_view::body_lines(self, self.width);
        let len = prefix.len() + content.len();
        let scroll = if self.showing_confirmation() {
            &mut self.preview_scroll
        } else {
            &mut self.scroll
        };
        let max = len.saturating_sub(self.page);
        *scroll = (*scroll as isize)
            .saturating_add(delta)
            .clamp(0, max as isize) as usize;
    }
}

/// The cell under the pointer, kept in the pane: a drag goes on past its
/// edges.
fn cell(mouse: MouseEvent, width: u16, height: u16) -> (u16, u16) {
    (
        mouse.column.min(width.saturating_sub(1)),
        mouse.row.min(height.saturating_sub(1)),
    )
}
