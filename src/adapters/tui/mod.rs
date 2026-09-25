pub mod fiche;
pub mod fiche_view;
pub mod markdown;
pub mod preview;
pub mod sidebar;
pub mod sidebar_view;
pub mod style;

use std::io::{self, stdout};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crossterm::event::{self, Event, KeyEvent, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::adapters::env::{self, ProcessEnv};
use crate::adapters::fetch::HttpFetcher;
use crate::adapters::herdr_cli::HerdrCommand;
use crate::adapters::herdr_socket::HerdrSocket;
use crate::adapters::operations::{FsOperations, spawn_operation};
use crate::application::load_listing::{LoadedListing, load_listing, read_registry};
use crate::application::load_readme::{Readme, load_readme};
use crate::application::open_fiche::{close_fiche, open_fiche};
use crate::application::ports::{HerdrCli, HerdrPort, Operations};
use crate::application::prepare_install::{Prepared, prepare_install};
use crate::application::prepare_removal::prepare_removal;
use crate::application::run_operation::{current_operation, operation_running};
use crate::domain::compat::Platform;
use crate::domain::error::AppError;
use crate::domain::fiche::FicheTarget;
use crate::domain::install::installed_from;
use crate::domain::operation::{OperationKind, OperationRequest};
use crate::domain::registry::InstalledPlugin;
use crate::domain::source::PluginSource;
use crate::domain::uninstall::RemovalPlan;

use self::fiche::{FicheApp, FicheIntent, InstalledView};
use self::sidebar::{Intent, SidebarApp};

type Screen = Terminal<CrosstermBackend<io::Stdout>>;

/// Background answers to the sidebar.
enum SidebarAnswer {
    Loaded(Result<LoadedListing, String>),
    /// Registry read again after an operation ended.
    Registry(Result<Vec<InstalledPlugin>, String>),
}

/// Background answers to a fiche, tagged with their request number.
enum FicheAnswer {
    Readme(u64, Result<Readme, String>),
    Install(u64, Prepared),
    Registry(u64, InstalledView),
    Removal(u64, Box<Result<RemovalPlan, String>>),
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
                            open_fiche(herdr, sidebar, &FicheTarget::from_row(&row)).map(drop)
                        }
                        None => Err(AppError::OriginMissing),
                    };
                    app.notice = opened
                        .err()
                        .map(|error| format!("Fiche non ouverte : {error}"));
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
        if let Some(key) = next_key()?
            && app.handle_key(key)
        {
            return Ok(());
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

/// Fiche pane: README of the plugin and commit received at opening.
pub fn run_fiche(process: ProcessEnv, target: FicheTarget) -> Result<(), AppError> {
    let herdr = HerdrSocket::new(process.socket_path.clone());
    let operations = FsOperations::new(process.state_dir.clone());
    let mut app = FicheApp::new(target);
    let (sender, receiver) = mpsc::channel();
    let mut terminal = setup()?;
    let result = fiche_loop(&mut terminal, &mut app, &operations, &sender, &receiver);
    let _ = teardown(&mut terminal);
    if let Some(pane_id) = process.own_pane_id {
        let _ = close_fiche(&herdr, &pane_id);
    }
    result
}

fn fiche_loop(
    terminal: &mut Screen,
    app: &mut FicheApp,
    operations: &FsOperations,
    sender: &Sender<FicheAnswer>,
    receiver: &Receiver<FicheAnswer>,
) -> Result<(), AppError> {
    loop {
        for intent in std::mem::take(&mut app.intents) {
            let sender = sender.clone();
            let target = app.target.clone();
            match intent {
                FicheIntent::LoadReadme(request) => {
                    thread::spawn(move || {
                        let readme =
                            load_readme(&HttpFetcher::new(), &target.source, &target.commit);
                        let _ = sender.send(FicheAnswer::Readme(request, readme));
                    });
                }
                FicheIntent::PrepareInstall(request) => {
                    thread::spawn(move || {
                        let prepared = prepare_install(
                            &HttpFetcher::new(),
                            &HerdrCommand::new(env::herdr_bin()),
                            &target,
                            Platform::current(),
                        );
                        let _ = sender.send(FicheAnswer::Install(request, prepared));
                    });
                }
                FicheIntent::Install(args) => launch(app, operations, OperationKind::Install, args),
                FicheIntent::Uninstall(args) => {
                    launch(app, operations, OperationKind::Uninstall, args)
                }
                FicheIntent::ReadRegistry(request) => {
                    thread::spawn(move || {
                        let installed =
                            installed_view(&HerdrCommand::new(env::herdr_bin()), &target.source);
                        let _ = sender.send(FicheAnswer::Registry(request, installed));
                    });
                }
                FicheIntent::PrepareRemoval(request) => {
                    thread::spawn(move || {
                        let plan =
                            prepare_removal(&HerdrCommand::new(env::herdr_bin()), &target.source);
                        let _ = sender.send(FicheAnswer::Removal(request, Box::new(plan)));
                    });
                }
            }
        }
        while let Ok(answer) = receiver.try_recv() {
            match answer {
                FicheAnswer::Readme(request, readme) => app.readme_loaded(request, readme),
                FicheAnswer::Install(request, prepared) => app.install_prepared(request, prepared),
                FicheAnswer::Registry(request, installed) => app.registry_read(request, installed),
                FicheAnswer::Removal(request, plan) => app.removal_prepared(request, *plan),
            }
        }
        app.operation_seen(current_operation(operations, &app.target.source));
        let size = terminal.size()?;
        let page = fiche_view::page_rows(app, size.width, size.height);
        app.set_viewport(size.width as usize, page);
        terminal.draw(|frame| fiche_view::render(frame, app))?;
        if let Some(key) = next_key()?
            && app.handle_key(key)
        {
            return Ok(());
        }
    }
}

/// Starts a confirmed request outside the fiche, unless an operation runs.
fn launch(app: &mut FicheApp, operations: &FsOperations, kind: OperationKind, args: Vec<String>) {
    if operation_running(operations) {
        app.operation_refused(
            "Une opération de la marketplace est déjà en cours : demande refusée".into(),
        );
        return;
    }
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
    };
    match spawn_operation(&request) {
        Ok(()) => app.operation_launched(request.id, kind),
        Err(error) => app.operation_refused(format!("Lancement impossible : {error}")),
    }
}

fn installed_view<H: HerdrCli>(herdr: &H, source: &PluginSource) -> InstalledView {
    let registry = match read_registry(herdr) {
        Ok(registry) => registry,
        Err(error) => return InstalledView::Unreadable(error),
    };
    match installed_from(&registry, source) {
        Ok(Some(plugin)) => {
            InstalledView::At(plugin.resolved_commit().unwrap_or_default().to_string())
        }
        Ok(None) => InstalledView::NotInstalled,
        Err(error) => InstalledView::Unreadable(error),
    }
}

fn next_key() -> Result<Option<KeyEvent>, AppError> {
    if !event::poll(POLL)? {
        return Ok(None);
    }
    match event::read()? {
        Event::Key(key) if key.kind == KeyEventKind::Press => Ok(Some(key)),
        _ => Ok(None),
    }
}

fn setup() -> Result<Screen, AppError> {
    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen)?;
    Terminal::new(CrosstermBackend::new(stdout)).map_err(AppError::from)
}

fn teardown(terminal: &mut Screen) -> io::Result<()> {
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    disable_raw_mode()?;
    terminal.show_cursor()?;
    Ok(())
}
