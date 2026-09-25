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
use std::time::Duration;

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
use crate::application::load_listing::{LoadedListing, load_listing};
use crate::application::load_readme::{Readme, load_readme};
use crate::application::open_fiche::{close_fiche, open_fiche};
use crate::application::ports::HerdrPort;
use crate::application::prepare_install::{Prepared, prepare_install};
use crate::domain::compat::Platform;
use crate::domain::error::AppError;
use crate::domain::fiche::FicheTarget;

use self::fiche::{FicheApp, FicheIntent};
use self::sidebar::{Intent, SidebarApp};

type Screen = Terminal<CrosstermBackend<io::Stdout>>;
type Loaded = Result<LoadedListing, String>;

/// Background answers to a fiche, tagged with their request number.
enum FicheAnswer {
    Readme(u64, Result<Readme, String>),
    Install(u64, Prepared),
}

const POLL: Duration = Duration::from_millis(100);

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
    sender: &Sender<Loaded>,
    receiver: &Receiver<Loaded>,
) -> Result<(), AppError> {
    loop {
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
        while let Ok(loaded) = receiver.try_recv() {
            app.loaded(loaded);
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

fn spawn_load(sender: Sender<Loaded>) {
    thread::spawn(move || {
        let loaded = load_listing(
            &HttpFetcher::new(),
            &HerdrCommand::new(env::herdr_bin()),
            &env::index_url(),
            Platform::current(),
        )
        .map_err(|error| error.to_string());
        let _ = sender.send(loaded);
    });
}

/// Fiche pane: README of the plugin and commit received at opening.
pub fn run_fiche(process: ProcessEnv, target: FicheTarget) -> Result<(), AppError> {
    let herdr = HerdrSocket::new(process.socket_path.clone());
    let mut app = FicheApp::new(target);
    let (sender, receiver) = mpsc::channel();
    let mut terminal = setup()?;
    let result = fiche_loop(&mut terminal, &mut app, &sender, &receiver);
    let _ = teardown(&mut terminal);
    if let Some(pane_id) = process.own_pane_id {
        let _ = close_fiche(&herdr, &pane_id);
    }
    result
}

fn fiche_loop(
    terminal: &mut Screen,
    app: &mut FicheApp,
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
                FicheIntent::Install(_) => {}
            }
        }
        while let Ok(answer) = receiver.try_recv() {
            match answer {
                FicheAnswer::Readme(request, readme) => app.readme_loaded(request, readme),
                FicheAnswer::Install(request, prepared) => app.install_prepared(request, prepared),
            }
        }
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
