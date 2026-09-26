pub mod details;
pub mod details_view;
pub mod markdown;
pub mod preview;
pub mod sidebar;
pub mod sidebar_view;
pub mod style;

use std::io::{self, stdout};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crossterm::event::{self, Event, KeyEvent, KeyEventKind, MouseEvent};
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
}

const POLL: Duration = Duration::from_millis(100);
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
            Some(Input::Key(key)) if app.handle_key(key) => return Ok(()),
            Some(Input::Mouse(mouse)) => app.handle_mouse(mouse, size.width, size.height),
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
    let result = details_loop(&mut terminal, &mut app, &operations, &sender, &receiver);
    let _ = teardown(&mut terminal);
    if let Some(pane_id) = process.own_pane_id {
        let _ = close_details(&herdr, &pane_id);
    }
    result
}

fn details_loop(
    terminal: &mut Screen,
    app: &mut DetailsApp,
    operations: &FsOperations,
    sender: &Sender<DetailsAnswer>,
    receiver: &Receiver<DetailsAnswer>,
) -> Result<(), AppError> {
    loop {
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
            }
        }
        app.operation_seen(current_operation(operations, &app.target.source));
        let size = terminal.size()?;
        let page = details_view::page_rows(app, size.width, size.height);
        app.set_viewport(size.width as usize, page);
        terminal.draw(|frame| details_view::render(frame, app))?;
        match next_input()? {
            Some(Input::Key(key)) if app.handle_key(key) => return Ok(()),
            Some(Input::Mouse(mouse)) => app.handle_mouse(mouse, size.width, size.height),
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
}

fn next_input() -> Result<Option<Input>, AppError> {
    if !event::poll(POLL)? {
        return Ok(None);
    }
    match event::read()? {
        Event::Key(key) if key.kind == KeyEventKind::Press => Ok(Some(Input::Key(key))),
        Event::Mouse(mouse) => Ok(Some(Input::Mouse(mouse))),
        _ => Ok(None),
    }
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
    execute!(stdout, EnterAlternateScreen, EnableMouse)?;
    Terminal::new(CrosstermBackend::new(stdout)).map_err(AppError::from)
}

fn teardown(terminal: &mut Screen) -> io::Result<()> {
    execute!(terminal.backend_mut(), DisableMouse, LeaveAlternateScreen)?;
    disable_raw_mode()?;
    terminal.show_cursor()?;
    Ok(())
}
