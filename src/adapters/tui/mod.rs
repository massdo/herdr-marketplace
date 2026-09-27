pub mod details;
pub mod details_view;
pub mod focus;
pub mod graphics;
pub mod markdown;
pub mod preview;
pub mod sidebar;
pub mod sidebar_view;
pub mod style;

use std::io::{self, Write, stdout};
use std::process::{Command as Process, Stdio};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crossterm::event::{
    self, DisableFocusChange, EnableFocusChange, Event, KeyEvent, KeyEventKind, MouseEvent,
};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use crossterm::{Command, execute};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::adapters::env::{self, ProcessEnv};
use crate::adapters::fetch::HttpFetcher;
use crate::adapters::herdr_cli::HerdrCommand;
use crate::adapters::herdr_socket::HerdrSocket;
use crate::adapters::image_fetch::ImageFetcher;
use crate::adapters::images::{Picture, load_in_background};
use crate::adapters::operations::{FsOperations, spawn_operation};
use crate::application::load_listing::{LoadedListing, load_listing, read_registry};
use crate::application::load_readme::{Readme, load_readme};
use crate::application::open_details::{close_details, open_details};
use crate::application::ports::{HerdrCli, HerdrPort, Operations};
use crate::application::prepare_install::{Prepared, prepare_install};
use crate::application::prepare_removal::prepare_removal;
use crate::application::run_operation::current_operation;
use crate::domain::compat::Platform;
use crate::domain::details::DetailsTarget;
use crate::domain::error::AppError;
use crate::domain::install::installed_from;
use crate::domain::operation::{
    Confirmation, OperationKind, OperationRecord, OperationRequest, Status,
};
use crate::domain::registry::InstalledPlugin;
use crate::domain::source::PluginSource;
use crate::domain::uninstall::RemovalPlan;

use self::details::{DetailsApp, DetailsIntent, InstalledView};
use self::focus::FocusClicks;
use self::graphics::Probe;
use self::sidebar::{Intent, SidebarApp};

type Screen = Terminal<CrosstermBackend<io::Stdout>>;

/// Background answers to the sidebar.
enum SidebarAnswer {
    Loaded(Result<LoadedListing, String>),
    /// Registry read again after an operation ended.
    Registry(Result<Vec<InstalledPlugin>, String>),
}

/// Background answers to a details pane, tagged with their request number.
enum DetailsAnswer {
    Readme(u64, Result<Readme, String>),
    Install(u64, Prepared),
    Registry(u64, InstalledView),
    Removal(u64, Box<Result<RemovalPlan, String>>),
    Operation(Box<OperationRecord>),
    Picture(String, Result<Arc<Picture>, String>),
}

const POLL: Duration = Duration::from_millis(100);
/// Command that opens an address in the browser.
pub const OPEN_ENV: &str = "HERDR_MARKETPLACE_OPEN";
const OPERATION_CHECK: Duration = Duration::from_secs(1);

/// Sidebar pane. The catalogue loads in a background thread so drawing and
/// the keyboard never wait for the network.
pub fn run_sidebar(process: ProcessEnv) -> Result<(), AppError> {
    let herdr = HerdrSocket::new(process.socket_path.clone());
    let mut app = SidebarApp::new();
    let (sender, receiver) = mpsc::channel();
    let mut terminal = setup()?;
    let result = sidebar_loop(
        &mut terminal,
        &mut app,
        &herdr,
        &process,
        &sender,
        &receiver,
    );
    let _ = teardown(&mut terminal);
    if let Some(pane_id) = process.own_pane_id {
        let _ = herdr.close_plugin_pane(&pane_id);
    }
    result
}

fn sidebar_loop(
    terminal: &mut Screen,
    app: &mut SidebarApp,
    herdr: &HerdrSocket,
    process: &ProcessEnv,
    sender: &Sender<SidebarAnswer>,
    receiver: &Receiver<SidebarAnswer>,
) -> Result<(), AppError> {
    let operations = FsOperations::new(process.state_dir.clone());
    let mut seen_finish = operations.latest_finish();
    let mut last_check = Instant::now();
    let mut focus = FocusClicks::default();
    loop {
        // An operation that ended since the last look: read the registry again.
        if last_check.elapsed() >= OPERATION_CHECK {
            last_check = Instant::now();
            let latest = operations.latest_finish();
            if latest > seen_finish {
                seen_finish = latest;
                let sender = sender.clone();
                thread::spawn(move || {
                    let registry = read_registry(&HerdrCommand::new(env::herdr_bin()));
                    let _ = sender.send(SidebarAnswer::Registry(registry));
                });
            }
        }
        for intent in std::mem::take(&mut app.intents) {
            match intent {
                Intent::Load => spawn_load(sender.clone()),
                Intent::Open(row) => {
                    let opened = match &process.own_pane_id {
                        Some(sidebar) => {
                            open_details(herdr, sidebar, &DetailsTarget::from_row(&row)).map(drop)
                        }
                        None => Err(AppError::OriginMissing),
                    };
                    app.notice = opened
                        .err()
                        .map(|error| format!("Details not opened: {error}"));
                }
            }
        }
        while let Ok(answer) = receiver.try_recv() {
            match answer {
                SidebarAnswer::Loaded(loaded) => app.loaded(loaded),
                SidebarAnswer::Registry(registry) => {
                    app.registry_refreshed(registry, Platform::current())
                }
            }
        }
        let size = terminal.size()?;
        app.set_page(sidebar_view::page_rows(app, size.width, size.height));
        terminal.draw(|frame| sidebar_view::render(frame, app))?;
        match next_input()? {
            Some((Input::Key(key), _)) if app.handle_key(key) => return Ok(()),
            Some((Input::Mouse(mouse), at)) if !focus.swallows(&mouse, at) => {
                app.handle_mouse(mouse, size.width, size.height)
            }
            Some((Input::FocusGained, at)) => {
                focus.focus_gained(at);
                app.focus(true);
            }
            Some((Input::FocusLost, _)) => app.focus(false),
            _ => {}
        }
    }
}

fn spawn_load(sender: Sender<SidebarAnswer>) {
    thread::spawn(move || {
        let loaded = load_listing(
            &HttpFetcher::new(),
            &HerdrCommand::new(env::herdr_bin()),
            &env::index_url(),
            Platform::current(),
        )
        .map_err(|error| error.to_string());
        let _ = sender.send(SidebarAnswer::Loaded(loaded));
    });
}

/// Details pane: README of the plugin and commit received at opening.
pub fn run_details(process: ProcessEnv, target: DetailsTarget) -> Result<(), AppError> {
    let herdr = HerdrSocket::new(process.socket_path.clone());
    let operations = FsOperations::new(process.state_dir.clone());
    let mut app = DetailsApp::new(target);
    let (sender, receiver) = mpsc::channel();
    let mut terminal = setup()?;
    // Before the event reader starts: the answers come on the input.
    let probe = graphics::probe();
    let result = details_loop(
        &mut terminal,
        &mut app,
        probe,
        &operations,
        &sender,
        &receiver,
    );
    let _ = teardown(&mut terminal);
    if let Some(pane_id) = process.own_pane_id {
        let _ = close_details(&herdr, &pane_id);
    }
    result
}

fn details_loop(
    terminal: &mut Screen,
    app: &mut DetailsApp,
    probe: Probe,
    operations: &FsOperations,
    sender: &Sender<DetailsAnswer>,
    receiver: &Receiver<DetailsAnswer>,
) -> Result<(), AppError> {
    let mut focus = FocusClicks::default();
    let image_sender = sender.clone();
    let images = load_in_background(ImageFetcher::default(), move |url, picture| {
        let _ = image_sender.send(DetailsAnswer::Picture(url, picture));
    });
    let mut last_operation_check = None::<Instant>;
    let started = Instant::now();
    let mut waiting = match probe {
        Probe::Known(graphics) => {
            app.set_graphics(graphics);
            None
        }
        Probe::Waiting { fallback } => Some(fallback),
    };
    loop {
        // Herdr gives a new pane its size in pixels with its first layout.
        if let Some(fallback) = waiting {
            let window = crossterm::terminal::window_size()
                .ok()
                .map(|size| (size.columns, size.rows, size.width, size.height));
            if let Some(graphics) = graphics::settle(window, started.elapsed(), fallback) {
                app.set_graphics(graphics);
                waiting = None;
            }
        }
        for intent in std::mem::take(&mut app.intents) {
            let sender = sender.clone();
            let target = app.target.clone();
            match intent {
                DetailsIntent::LoadReadme(request) => {
                    thread::spawn(move || {
                        let readme =
                            load_readme(&HttpFetcher::new(), &target.source, &target.commit);
                        let _ = sender.send(DetailsAnswer::Readme(request, readme));
                    });
                }
                DetailsIntent::PrepareInstall(request) => {
                    thread::spawn(move || {
                        let prepared = prepare_install(
                            &HttpFetcher::new(),
                            &HerdrCommand::new(env::herdr_bin()),
                            &target,
                            Platform::current(),
                        );
                        let _ = sender.send(DetailsAnswer::Install(request, prepared));
                    });
                }
                DetailsIntent::Install(preview) => {
                    let confirmation = Confirmation::Install {
                        target,
                        manifest: Box::new(preview.manifest),
                        plan: preview.plan,
                    };
                    launch(
                        app,
                        operations,
                        sender,
                        OperationKind::Install,
                        preview.args,
                        confirmation,
                    )
                }
                DetailsIntent::Uninstall(plan) => {
                    let confirmation = Confirmation::Uninstall {
                        installed: plan.installed,
                    };
                    launch(
                        app,
                        operations,
                        sender,
                        OperationKind::Uninstall,
                        plan.args,
                        confirmation,
                    )
                }
                DetailsIntent::ReadRegistry(request) => {
                    thread::spawn(move || {
                        let installed =
                            installed_view(&HerdrCommand::new(env::herdr_bin()), &target.source);
                        let _ = sender.send(DetailsAnswer::Registry(request, installed));
                    });
                }
                DetailsIntent::PrepareRemoval(request) => {
                    thread::spawn(move || {
                        let plan =
                            prepare_removal(&HerdrCommand::new(env::herdr_bin()), &target.source);
                        let _ = sender.send(DetailsAnswer::Removal(request, Box::new(plan)));
                    });
                }
                DetailsIntent::LoadImages(urls) => {
                    for url in urls {
                        let _ = images.send(url);
                    }
                }
                DetailsIntent::OpenUrl(url) => open_url(&url),
            }
        }
        while let Ok(answer) = receiver.try_recv() {
            match answer {
                DetailsAnswer::Readme(request, readme) => app.readme_loaded(request, readme),
                DetailsAnswer::Install(request, prepared) => {
                    app.install_prepared(request, prepared)
                }
                DetailsAnswer::Registry(request, installed) => {
                    app.registry_read(request, installed)
                }
                DetailsAnswer::Removal(request, plan) => app.removal_prepared(request, *plan),
                DetailsAnswer::Operation(record) => app.operation_finished(*record),
                DetailsAnswer::Picture(url, picture) => app.picture_loaded(&url, picture),
            }
        }
        if last_operation_check.is_none_or(|last| last.elapsed() >= OPERATION_CHECK) {
            last_operation_check = Some(Instant::now());
            app.operation_seen(current_operation(operations, &app.target.source));
        }
        let size = terminal.size()?;
        let page = details_view::page_rows(app, size.width, size.height);
        app.set_viewport(size.width as usize, page);
        // Images the layout draws reach the terminal before their cells.
        let images = app.pictures.take_commands();
        if !images.is_empty() {
            terminal.backend_mut().write_all(images.as_bytes())?;
            terminal.backend_mut().flush()?;
        }
        terminal.draw(|frame| details_view::render(frame, app))?;
        match next_input()? {
            Some((Input::Key(key), _)) if app.handle_key(key) => return Ok(()),
            Some((Input::Mouse(mouse), at)) if !focus.swallows(&mouse, at) => {
                app.handle_mouse(mouse, size.width, size.height)
            }
            Some((Input::FocusGained, at)) => focus.focus_gained(at),
            _ => {}
        }
    }
}

/// Starts a confirmed request outside the details pane, unless an operation
/// runs.
fn launch(
    app: &mut DetailsApp,
    operations: &FsOperations,
    sender: Sender<DetailsAnswer>,
    kind: OperationKind,
    args: Vec<String>,
    confirmation: Confirmation,
) {
    let guard = match operations.try_begin() {
        Ok(Some(guard)) => guard,
        Ok(None) => {
            app.operation_refused(
                "Another marketplace operation is running: request refused".into(),
            );
            return;
        }
        Err(error) => {
            app.operation_refused(format!("Could not reserve the operation: {error}"));
            return;
        }
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or(0);
    let request = OperationRequest {
        id: format!("{now}-{}", std::process::id()),
        kind,
        source: app.target.source.clone(),
        commit: app.target.commit.clone(),
        args,
        confirmation: Some(confirmation),
    };
    match std::env::current_exe().and_then(|binary| spawn_operation(&request, guard, &binary)) {
        Ok(child) => {
            app.operation_launched(request.id.clone(), kind);
            thread::spawn(move || {
                let result = child
                    .wait_with_output()
                    .map_err(|error| error.to_string())
                    .and_then(|output| {
                        serde_json::from_slice::<OperationRecord>(&output.stdout).map_err(|error| {
                            format!("{error}: {}", String::from_utf8_lossy(&output.stderr))
                        })
                    })
                    .and_then(|record| {
                        if record.request == request {
                            Ok(record)
                        } else {
                            Err("worker returned another operation's result".into())
                        }
                    });
                let record = result.unwrap_or_else(|error| {
                    let mut record = OperationRecord::running(&request);
                    record.status = Status::Unconfirmed;
                    record.output =
                        "the operation stopped without reporting its result; check the registry"
                            .into();
                    record.persistence_error = Some(error);
                    record.finished_unix_ms = Some(
                        SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .map(|elapsed| elapsed.as_millis() as u64)
                            .unwrap_or(0),
                    );
                    record
                });
                let _ = sender.send(DetailsAnswer::Operation(Box::new(record)));
            });
        }
        Err(error) => app.operation_refused(format!("Could not start: {error}")),
    }
}

/// The browser opens `url`: `HERDR_MARKETPLACE_OPEN` names the command, else
/// `open` on macOS and `xdg-open` elsewhere.
fn open_url(url: &str) {
    let opener = std::env::var(OPEN_ENV)
        .ok()
        .filter(|opener| !opener.trim().is_empty())
        .unwrap_or_else(|| {
            if cfg!(target_os = "macos") {
                "open"
            } else {
                "xdg-open"
            }
            .into()
        });
    if let Ok(mut child) = Process::new(opener)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        thread::spawn(move || child.wait());
    }
}

fn installed_view<H: HerdrCli>(herdr: &H, source: &PluginSource) -> InstalledView {
    let registry = match read_registry(herdr) {
        Ok(registry) => registry,
        Err(error) => return InstalledView::Uncertain(error),
    };
    match installed_from(&registry, source) {
        Ok(Some(plugin)) => {
            InstalledView::At(plugin.resolved_commit().unwrap_or_default().to_string())
        }
        Ok(None) => InstalledView::NotInstalled,
        Err(error) => InstalledView::Uncertain(error),
    }
}

enum Input {
    Key(KeyEvent),
    Mouse(MouseEvent),
    FocusGained,
    FocusLost,
}

/// The next input and when it was read.
fn next_input() -> Result<Option<(Input, Instant)>, AppError> {
    if !event::poll(POLL)? {
        return Ok(None);
    }
    let input = match event::read()? {
        Event::Key(key) if key.kind == KeyEventKind::Press => Input::Key(key),
        Event::Mouse(mouse) => Input::Mouse(mouse),
        Event::FocusGained => Input::FocusGained,
        Event::FocusLost => Input::FocusLost,
        _ => return Ok(None),
    };
    Ok(Some((input, Instant::now())))
}

/// Clicks and the wheel, in SGR encoding. Unlike crossterm's
/// `EnableMouseCapture`, no event for every move of the pointer.
struct EnableMouse;

impl Command for EnableMouse {
    fn write_ansi(&self, f: &mut impl std::fmt::Write) -> std::fmt::Result {
        f.write_str("\x1b[?1000h\x1b[?1006h")
    }
}

struct DisableMouse;

impl Command for DisableMouse {
    fn write_ansi(&self, f: &mut impl std::fmt::Write) -> std::fmt::Result {
        f.write_str("\x1b[?1006l\x1b[?1000l")
    }
}

fn setup() -> Result<Screen, AppError> {
    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouse, EnableFocusChange)?;
    Terminal::new(CrosstermBackend::new(stdout)).map_err(AppError::from)
}

fn teardown(terminal: &mut Screen) -> io::Result<()> {
    execute!(
        terminal.backend_mut(),
        DisableFocusChange,
        DisableMouse,
        LeaveAlternateScreen
    )?;
    disable_raw_mode()?;
    terminal.show_cursor()?;
    Ok(())
}
