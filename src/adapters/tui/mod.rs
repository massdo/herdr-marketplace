pub mod sidebar;
pub mod sidebar_view;
pub mod style;

use std::io::{self, stdout};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use crossterm::event::{self, Event, KeyEventKind};
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
use crate::application::ports::HerdrPort;
use crate::domain::compat::Platform;
use crate::domain::error::AppError;

use self::sidebar::{Intent, SidebarApp};

type Screen = Terminal<CrosstermBackend<io::Stdout>>;
type Loaded = Result<LoadedListing, String>;

const POLL: Duration = Duration::from_millis(100);

/// Sidebar pane. The catalogue loads in a background thread so drawing and
/// the keyboard never wait for the network.
pub fn run_sidebar(process: ProcessEnv) -> Result<(), AppError> {
    let herdr = HerdrSocket::new(process.socket_path.clone());
    let mut app = SidebarApp::new();
    let (sender, receiver) = mpsc::channel();
    let mut terminal = setup()?;
    let result = sidebar_loop(&mut terminal, &mut app, &sender, &receiver);
    let _ = teardown(&mut terminal);
    if let Some(pane_id) = process.own_pane_id {
        let _ = herdr.close_plugin_pane(&pane_id);
    }
    result
}

fn sidebar_loop(
    terminal: &mut Screen,
    app: &mut SidebarApp,
    sender: &Sender<Loaded>,
    receiver: &Receiver<Loaded>,
) -> Result<(), AppError> {
    loop {
        for intent in std::mem::take(&mut app.intents) {
            match intent {
                Intent::Load => spawn_load(sender.clone()),
                Intent::Open(_) => {}
            }
        }
        while let Ok(loaded) = receiver.try_recv() {
            app.loaded(loaded);
        }
        let height = terminal.size()?.height;
        app.set_page(sidebar_view::page_rows(app, height));
        terminal.draw(|frame| sidebar_view::render(frame, app))?;
        if event::poll(POLL)?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
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
