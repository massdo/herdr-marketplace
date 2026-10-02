pub mod animation;
pub mod details;
pub mod details_view;
pub mod focus;
pub mod graphics;
pub mod markdown;
pub mod preview;
pub mod selection;
pub mod sidebar;
pub mod sidebar_view;
pub mod style;
pub mod video;
pub mod video_cache;

use std::collections::HashSet;
use std::io::{self, Write, stdout};
use std::process::{Command as Process, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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

use crate::adapters::catalog_cache::FileCatalogCache;
use crate::adapters::details_control::{DetailsControl, DetailsUpdate};
use crate::adapters::download_cancel::Cancellation;
use crate::adapters::env::{self, ProcessEnv};
use crate::adapters::fetch::HttpFetcher;
use crate::adapters::herdr_cli::HerdrCommand;
use crate::adapters::herdr_socket::HerdrSocket;
use crate::adapters::image_fetch::{self, ImageFetcher};
use crate::adapters::images::{Animation, Picture, frames_within, load_in_background};
use crate::adapters::operations::{FsOperations, spawn_operation};
use crate::adapters::pane_graphics::Layer;
use crate::application::load_catalog::CatalogLoad;
use crate::application::load_listing::{LoadedListing, read_registry};
use crate::application::load_readme::{Readme, load_readme};
use crate::application::open_details::{Reveal, Shown, close_details, show_details_cached};
use crate::application::ports::{HerdrCli, HerdrPort, Operations};
use crate::application::prepare_install::{InstallPreview, Prepared, prepare_install};
use crate::application::prepare_removal::prepare_removal;
use crate::application::run_operation::current_operation;
use crate::application::update_plugin::prepare_update;
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

use self::animation::Player;
use self::details::{DetailsApp, DetailsIntent, InstalledView};
use self::focus::FocusClicks;
use self::graphics::{Graphics, Probe};
use self::sidebar::{Intent, Notice, SidebarApp};
use self::style::ERROR;
use self::video::{Cache, FIRST_FRAME, Frames, Playback, Video, VideoEvent};
use self::video_cache::CacheProcess;

type Screen = Terminal<CrosstermBackend<io::Stdout>>;

/// Background answers to the sidebar.
enum SidebarAnswer {
    Loaded(Result<LoadedListing, String>),
    /// Registry read again after an operation ended.
    Registry(Result<Vec<InstalledPlugin>, String>),
    /// The checks of the update a card asked for.
    UpdatePrepared(Box<DetailsTarget>, Box<Result<InstallPreview, String>>),
    /// The result of the update launched from a card.
    Operation(Box<OperationRecord>),
}

/// Background answers to a details pane, tagged with their request number.
enum DetailsAnswer {
    Readme(u64, Result<Readme, String>),
    Install(u64, Prepared),
    Update(u64, Box<Result<InstallPreview, String>>),
    Registry(u64, InstalledView),
    Removal(u64, Box<Result<RemovalPlan, String>>),
    Operation(Box<OperationRecord>),
    Picture(String, Result<Arc<Picture>, String>),
    /// Frames of an animated image, made for cells of this many pixels.
    Frames(String, (u32, u32), Result<Arc<Animation>, String>),
    VideoProbed(String, Result<image_fetch::Probe, String>),
    /// News of the playback of this number.
    Video(u64, VideoEvent),
}

const POLL: Duration = Duration::from_millis(100);
/// How long the selection rests before the details follow it: holding an
/// arrow opens no pane for the plugins it passes, and typing a word shows
/// only the plugin it finds.
const PREVIEW_DELAY: Duration = Duration::from_millis(200);
const SEARCH_DELAY: Duration = Duration::from_millis(500);
/// Command that opens an address in the browser.
pub const OPEN_ENV: &str = "HERDR_MARKETPLACE_OPEN";
const OPERATION_CHECK: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, PartialEq, Eq)]
enum PaneExit {
    User,
    External,
    Changed(Box<DetailsUpdate>),
}

/// A static sidebar may never write again after Herdr closes its PTY.
/// Poll hangup explicitly without consuming keyboard input.
fn terminal_closed(fd: libc::c_int) -> bool {
    let mut poll = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: one descriptor; poll reads no bytes from it.
    unsafe {
        libc::poll(&mut poll, 1, 0) > 0
            && poll.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0
    }
}

/// Sidebar pane. The catalogue loads in a background thread so drawing and
/// the keyboard never wait for the network.
pub fn run_sidebar(mut process: ProcessEnv) -> Result<(), AppError> {
    let _control = process
        .own_pane_id
        .as_ref()
        .map(|pane| DetailsControl::open(&process.state_dir, &process.socket_path, pane))
        .transpose()?;
    let videos = process.state_dir.join("videos");
    video::clean_stale(&videos);
    let cache =
        CacheProcess::start(&process.state_dir).map_err(|message| AppError::Io { message })?;
    process.video_cache = Some(cache.folder().to_path_buf());
    let herdr =
        HerdrSocket::new(process.socket_path.clone()).with_details_state(process.state_dir.clone());
    let mut app = SidebarApp::new();
    let (sender, receiver) = mpsc::channel();
    let mut terminal = setup(Mouse::Clicks)?;
    let result = sidebar_loop(
        &mut terminal,
        &mut app,
        &herdr,
        &process,
        &sender,
        &receiver,
    );
    // Finish transfers and delete the session before Herdr closes our PTY.
    drop(cache);
    let _ = teardown(&mut terminal);
    if result != Ok(PaneExit::External)
        && let Some(pane_id) = process.own_pane_id
    {
        let _ = herdr.close_plugin_pane(&pane_id);
    }
    result.map(|_| ())
}

fn sidebar_loop(
    terminal: &mut Screen,
    app: &mut SidebarApp,
    herdr: &HerdrSocket,
    process: &ProcessEnv,
    sender: &Sender<SidebarAnswer>,
    receiver: &Receiver<SidebarAnswer>,
) -> Result<PaneExit, AppError> {
    let shutdown = PaneShutdown::new()?;
    let operations = FsOperations::new(process.state_dir.clone());
    let mut seen_finish = operations.latest_finish();
    let mut last_check = Instant::now();
    let mut focus = FocusClicks::default();
    let mut shown = None::<Shown>;
    // The plugin the details will show once the selection rests, and when.
    let mut pending = None::<(DetailsTarget, Reveal, Instant)>;
    // The last key or focus: the search caret blinks from there.
    let mut caret_lit = Instant::now();
    loop {
        if shutdown.stopped.load(Ordering::Acquire)
            || terminal_closed(libc::STDIN_FILENO)
            || process.own_pane_id.as_ref().is_some_and(|pane| {
                DetailsControl::closing(&process.state_dir, &process.socket_path, pane)
            })
        {
            return Ok(PaneExit::External);
        }
        // An operation that ended since the last look: read the registry again.
        if last_check.elapsed() >= OPERATION_CHECK {
            last_check = Instant::now();
            let latest = operations.latest_finish();
            if latest > seen_finish {
                seen_finish = latest;
                spawn_registry(sender.clone());
            }
            app.set_running_updates(running_updates(app, &operations));
        }
        for intent in std::mem::take(&mut app.intents) {
            match intent {
                Intent::Load => start_load(app, sender.clone()),
                Intent::Update(row) => {
                    let sender = sender.clone();
                    thread::spawn(move || {
                        let target = DetailsTarget::from_row(&row);
                        let result = prepare_update(
                            &HttpFetcher::new(),
                            &HerdrCommand::new(env::herdr_bin()),
                            &target,
                            Platform::current(),
                        );
                        let _ = sender.send(SidebarAnswer::UpdatePrepared(
                            Box::new(target),
                            Box::new(result),
                        ));
                    });
                }

                Intent::Open(row, reveal) => {
                    pending = None;
                    let target = DetailsTarget::from_row(&row);
                    show(herdr, process, app, &mut shown, &target, reveal);
                }
                Intent::Preview(row) => {
                    let due = Instant::now() + PREVIEW_DELAY;
                    pending = Some((DetailsTarget::from_row(&row), Reveal::Preview, due));
                }
                Intent::Follow(row) => {
                    // The arrows asked for details: the search does not cancel them.
                    let reveal = match pending {
                        Some((_, Reveal::Preview, _)) => Reveal::Preview,
                        _ => Reveal::Follow,
                    };
                    let due = Instant::now() + SEARCH_DELAY;
                    pending = Some((DetailsTarget::from_row(&row), reveal, due));
                }
            }
        }
        if let Some((target, reveal, _)) = pending.take_if(|(_, _, due)| Instant::now() >= *due) {
            show(herdr, process, app, &mut shown, &target, reveal);
        }
        while let Ok(answer) = receiver.try_recv() {
            match answer {
                SidebarAnswer::Loaded(loaded) => app.loaded(loaded),
                SidebarAnswer::Registry(registry) => {
                    app.registry_refreshed(registry, Platform::current())
                }
                SidebarAnswer::UpdatePrepared(target, result) => match *result {
                    Ok(preview) => start_update(app, &operations, sender, *target, preview),
                    Err(reason) => app.update_refused(&reason),
                },
                // Read now: the result may not have been saved for the
                // latest finish to notice.
                SidebarAnswer::Operation(record) => {
                    app.update_finished(&record);
                    spawn_registry(sender.clone());
                }
            }
        }
        let size = terminal.size()?;
        app.set_page(sidebar_view::page_rows(app, size.width, size.height));
        let turn = app.blink(caret_lit.elapsed());
        terminal.draw(|frame| sidebar_view::render(frame, app))?;
        let wait = pending.as_ref().map_or(POLL, |(_, _, due)| {
            due.saturating_duration_since(Instant::now()).min(POLL)
        });
        // A caret that shows turns on time, not at the next poll.
        let wait = if app.focused { wait.min(turn) } else { wait };
        let input = next_input(wait)?;
        if let Some((Input::Key(_) | Input::FocusGained, at)) = &input {
            caret_lit = *at;
        }
        match input {
            Some((Input::Key(key), _)) if app.handle_key(key) => return Ok(PaneExit::User),
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

/// The details of `target` beside the sidebar; the notice says why not.
fn show(
    herdr: &HerdrSocket,
    process: &ProcessEnv,
    app: &mut SidebarApp,
    shown: &mut Option<Shown>,
    target: &DetailsTarget,
    reveal: Reveal,
) {
    let result = match &process.own_pane_id {
        Some(sidebar) => show_details_cached(
            herdr,
            sidebar,
            target,
            reveal,
            shown.as_ref(),
            process.video_cache.as_deref(),
        ),
        None => Err(AppError::OriginMissing),
    };
    match result {
        Ok(Some(now)) => {
            *shown = Some(now);
            app.notice = None;
        }
        Ok(None) => {}
        Err(error) => {
            app.notice = Some(Notice {
                text: format!("Details not opened: {error}"),
                color: ERROR,
            })
        }
    }
}

/// Starts the update a card asked for, checked like a details pane's.
fn start_update(
    app: &mut SidebarApp,
    operations: &FsOperations,
    sender: &Sender<SidebarAnswer>,
    target: DetailsTarget,
    preview: InstallPreview,
) {
    let request = OperationRequest {
        id: operation_id(),
        kind: OperationKind::Update,
        source: target.source.clone(),
        commit: target.commit.clone(),
        args: preview.args,
        confirmation: Some(Confirmation::Install {
            target,
            manifest: Box::new(preview.manifest),
            plan: preview.plan,
        }),
    };
    let sender = sender.clone();
    let done = move |record| {
        let _ = sender.send(SidebarAnswer::Operation(Box::new(record)));
    };
    match start_operation(operations, request, done) {
        Ok(()) => {}
        Err(StartError::Busy) => app
            .update_not_started("Another marketplace operation is running: update refused".into()),
        Err(StartError::Lock(error) | StartError::Spawn(error)) => {
            app.update_not_started(format!("Could not start the update: {error}"))
        }
    }
}

/// The updates of listed plugins that run now, wherever they were launched.
fn running_updates(app: &SidebarApp, operations: &FsOperations) -> HashSet<PluginSource> {
    app.rows()
        .iter()
        .filter(|row| row.update().is_some())
        .map(|row| &row.entry.source)
        .filter(|source| {
            current_operation(operations, source).is_some_and(|record| {
                record.status == Status::Running && record.request.kind == OperationKind::Update
            })
        })
        .cloned()
        .collect()
}

fn spawn_registry(sender: Sender<SidebarAnswer>) {
    thread::spawn(move || {
        let registry = read_registry(&HerdrCommand::new(env::herdr_bin()));
        let _ = sender.send(SidebarAnswer::Registry(registry));
    });
}

fn start_load(app: &mut SidebarApp, sender: Sender<SidebarAnswer>) {
    let cache = FileCatalogCache::new(env::catalog_dir());
    let herdr = HerdrCommand::new(env::herdr_bin());
    let load = match CatalogLoad::new(&cache, &herdr, &env::index_url()) {
        Ok(load) => load,
        Err(error) => {
            app.loaded(Err(error.to_string()));
            return;
        }
    };
    // Local data before the first draw: no Loading flash on a warm open.
    if let Some(cached) = load.cached() {
        app.loaded(Ok(LoadedListing::new(
            cached,
            read_registry(&herdr),
            Platform::current(),
        )));
    }
    thread::spawn(move || {
        let loaded = load
            .refresh(&HttpFetcher::new(), &cache)
            .map(|loaded| LoadedListing::new(loaded, read_registry(&herdr), Platform::current()))
            .map_err(|error| error.to_string());
        let _ = sender.send(SidebarAnswer::Loaded(loaded));
    });
}

/// Details pane: README of the plugin and commit received at opening.
pub fn run_details(mut process: ProcessEnv, mut target: DetailsTarget) -> Result<(), AppError> {
    let herdr =
        HerdrSocket::new(process.socket_path.clone()).with_details_state(process.state_dir.clone());
    // The videos a pane that stopped left.
    video::clean_stale(&process.state_dir.join("videos"));
    let _control = process
        .own_pane_id
        .as_ref()
        .map(|pane| DetailsControl::open(&process.state_dir, &process.socket_path, pane))
        .transpose()?;
    // Herdr selects text only where the pane leaves it the mouse: the pane
    // gets the drags too, and selects on its own.
    let mut terminal = setup(Mouse::Drags)?;
    // Before the event reader starts: the answers come on the input.
    let mut probe = graphics::probe();
    let result = loop {
        let mut app = DetailsApp::new(target);
        // Every selection has its own answer channel. Late README, install,
        // registry, image and video answers cannot modify the next plugin.
        let (sender, receiver) = mpsc::channel();
        let mut player = process
            .own_pane_id
            .as_ref()
            .map(|pane| Player::new(process.socket_path.clone(), pane.as_str().to_string()));
        app.video_player(player.is_some() && env::ffmpeg_bin().is_some());
        let result = details_loop(
            &mut terminal,
            &mut app,
            probe,
            player.as_mut(),
            &process,
            &sender,
            &receiver,
        );
        match result {
            Ok(PaneExit::Changed(update)) => {
                probe = app.pictures.graphics().map(Probe::Known).unwrap_or(probe);
                app.pictures.begin_layout();
                terminal
                    .backend_mut()
                    .write_all(app.pictures.take_commands().as_bytes())?;
                terminal.clear()?;
                target = update.target;
                process.video_cache = update.video_cache;
            }
            other => break other,
        }
    };
    let _ = teardown(&mut terminal);
    if result != Ok(PaneExit::External)
        && let Some(pane_id) = process.own_pane_id
    {
        let _ = close_details(&herdr, &pane_id);
    }
    result.map(|_| ())
}

/// The loop of a details pane. The video that plays stops when it returns,
/// before the pane closes.
fn details_loop(
    terminal: &mut Screen,
    app: &mut DetailsApp,
    probe: Probe,
    mut player: Option<&mut Player>,
    process: &ProcessEnv,
    sender: &Sender<DetailsAnswer>,
    receiver: &Receiver<DetailsAnswer>,
) -> Result<PaneExit, AppError> {
    // Herdr may close the PTY with SIGHUP or SIGTERM instead of `q`. Let
    // the loop unwind so playback and its cache are cleaned in that case too.
    let shutdown = PaneShutdown::new()?;
    let operations = &FsOperations::new(process.state_dir.clone());
    let cache = Arc::new(match &process.video_cache {
        Some(folder) => Cache::shared(folder.clone()),
        None => Cache::new(process.state_dir.join("videos")),
    });
    // The video that plays, and the number of the last one started.
    let mut playback = None::<Playback>;
    let mut playbacks = 0;
    let mut focus = FocusClicks::default();
    // Dropping this selection also interrupts its image/probe HTTP requests.
    let image_stop = CancelImages(Cancellation::default());
    let image_fetcher = ImageFetcher::default().cancellable(&image_stop.0);
    let image_sender = sender.clone();
    let images = load_in_background(image_fetcher, move |url, picture| {
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
        if shutdown.stopped.load(Ordering::Acquire)
            || terminal_closed(libc::STDIN_FILENO)
            || process.own_pane_id.as_ref().is_some_and(|pane| {
                DetailsControl::closing(&process.state_dir, &process.socket_path, pane)
            })
        {
            return Ok(PaneExit::External);
        }
        if let Some(pane) = &process.own_pane_id
            && let Some(update) =
                DetailsControl::take(&process.state_dir, &process.socket_path, pane)?
            && (update.target != app.target || update.video_cache != process.video_cache)
        {
            return Ok(PaneExit::Changed(Box::new(update)));
        }
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
                DetailsIntent::PrepareUpdate(request) => {
                    thread::spawn(move || {
                        let result = prepare_update(
                            &HttpFetcher::new(),
                            &HerdrCommand::new(env::herdr_bin()),
                            &target,
                            Platform::current(),
                        );
                        let _ = sender.send(DetailsAnswer::Update(request, Box::new(result)));
                    });
                }
                DetailsIntent::Update(preview) => {
                    let confirmation = Confirmation::Install {
                        target,
                        manifest: Box::new(preview.manifest),
                        plan: preview.plan,
                    };
                    launch(
                        app,
                        operations,
                        sender,
                        OperationKind::Update,
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
                DetailsIntent::ProbeVideos(urls) => {
                    let cancellation = image_stop.0.clone();
                    thread::spawn(move || {
                        let fetcher = ImageFetcher::default().cancellable(&cancellation);
                        for url in urls {
                            let probe = fetcher.probe(&url).map_err(|error| error.to_string());
                            let _ = sender.send(DetailsAnswer::VideoProbed(url, probe));
                        }
                    });
                }
                DetailsIntent::PlayVideo {
                    url,
                    looped,
                    fit,
                    start,
                } => {
                    // The video that played stops first.
                    playback = None;
                    playbacks += 1;
                    let number = playbacks;
                    match (env::ffmpeg_bin(), player.as_deref_mut()) {
                        (Some(ffmpeg), Some(player)) => {
                            let socket = player.socket().to_path_buf();
                            let pane = player.pane().to_string();
                            playback = Some(Playback::start(Video {
                                url,
                                start,
                                looped,
                                fit,
                                cache: cache.clone(),
                                ffmpeg,
                                download: Arc::new(ImageFetcher::default()),
                                open: Box::new(move || {
                                    Layer::open(&socket, &pane, "video")
                                        .map(|layer| Box::new(layer) as Box<dyn Frames>)
                                }),
                                visible: player.visible(),
                                first_frame: FIRST_FRAME,
                                events: Arc::new(move |event| {
                                    let _ = sender.send(DetailsAnswer::Video(number, event));
                                }),
                            }));
                        }
                        _ => app.video_event(VideoEvent::Failed("FFmpeg is missing".into())),
                    }
                }
                DetailsIntent::StopVideo => playback = None,
                DetailsIntent::PauseVideo(paused) => {
                    if let Some(playback) = &playback {
                        playback.pause(paused);
                    }
                }
                DetailsIntent::SeekVideo(milliseconds) => {
                    if let Some(playback) = &playback {
                        playback.seek(milliseconds);
                    }
                }
                DetailsIntent::OpenUrl(url) => open_url(&url),
                DetailsIntent::Copy(text) => {
                    let backend = terminal.backend_mut();
                    backend.write_all(selection::clipboard(&text).as_bytes())?;
                    backend.flush()?;
                }
            }
        }
        while let Ok(answer) = receiver.try_recv() {
            match answer {
                DetailsAnswer::Readme(request, readme) => app.readme_loaded(request, readme),
                DetailsAnswer::Install(request, prepared) => {
                    app.install_prepared(request, prepared)
                }
                DetailsAnswer::Update(request, result) => app.update_prepared(request, *result),
                DetailsAnswer::Registry(request, installed) => {
                    app.registry_read(request, installed)
                }
                DetailsAnswer::Removal(request, plan) => app.removal_prepared(request, *plan),
                DetailsAnswer::Operation(record) => app.operation_finished(*record),
                DetailsAnswer::Picture(url, picture) => app.picture_loaded(&url, picture),
                DetailsAnswer::Frames(url, fit, frames) => {
                    app.pictures.frames_decoded(&url, fit, frames)
                }
                DetailsAnswer::VideoProbed(url, probe) => app.video_probed(&url, probe),
                // Playback stops; a complete file stays cached until closure.
                DetailsAnswer::Video(number, event) if number == playbacks => {
                    if matches!(
                        event,
                        VideoEvent::Ended | VideoEvent::Hidden | VideoEvent::Failed(_)
                    ) {
                        playback = None;
                    }
                    app.video_event(event);
                }
                // A video that was stopped for another one.
                DetailsAnswer::Video(..) => {}
            }
        }
        if last_operation_check.is_none_or(|last| last.elapsed() >= OPERATION_CHECK) {
            last_operation_check = Some(Instant::now());
            app.operation_seen(current_operation(operations, &app.target.source));
        }
        let size = terminal.size()?;
        let page = details_view::page_rows(app, size.width, size.height);
        app.set_viewport(size.width as usize, page);
        if !app.video_places.is_empty()
            && let Some(player) = player.as_deref_mut()
        {
            app.autoplay_video(player.visible().load(Ordering::Acquire));
        }
        // Images the layout draws reach the terminal before their cells.
        let images = app.pictures.take_commands();
        if !images.is_empty() {
            terminal.backend_mut().write_all(images.as_bytes())?;
            terminal.backend_mut().flush()?;
        }
        terminal.draw(|frame| details_view::render(frame, app))?;
        if let Some(player) = player.as_deref_mut() {
            animate(app, player, size.width, size.height, sender);
        }
        if let Some(playback) = &mut playback {
            playback.show(details_view::video_spot(app, size.width, size.height));
        }
        match next_input(POLL)? {
            Some((Input::Key(key), _)) if app.handle_key(key) => return Ok(PaneExit::User),
            Some((Input::Mouse(mouse), at)) if focus.swallows(&mouse, at) => {
                app.focus_click(mouse, size.width, size.height)
            }
            Some((Input::Mouse(mouse), _)) => app.handle_mouse(mouse, size.width, size.height),
            Some((Input::FocusGained, at)) => focus.focus_gained(at),
            _ => {}
        }
    }
}

struct PaneShutdown {
    stopped: Arc<AtomicBool>,
    signals: Vec<signal_hook::SigId>,
}

struct CancelImages(Cancellation);

impl Drop for CancelImages {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

impl PaneShutdown {
    fn new() -> io::Result<Self> {
        let mut shutdown = Self {
            stopped: Arc::new(AtomicBool::new(false)),
            signals: Vec::new(),
        };
        for signal in [signal_hook::consts::SIGHUP, signal_hook::consts::SIGTERM] {
            shutdown.signals.push(signal_hook::flag::register(
                signal,
                shutdown.stopped.clone(),
            )?);
        }
        Ok(shutdown)
    }
}

impl Drop for PaneShutdown {
    fn drop(&mut self) {
        for signal in self.signals.drain(..) {
            signal_hook::low_level::unregister(signal);
        }
    }
}

/// Plays the animated images the pane shows over their first frame; their
/// frames decode in the background the first time they show.
fn animate(
    app: &mut DetailsApp,
    player: &mut Player,
    width: u16,
    height: u16,
    sender: &Sender<DetailsAnswer>,
) {
    let mut shown = Vec::new();
    if let Some(Graphics::Kitty {
        cell_width,
        cell_height,
    }) = app.pictures.graphics()
    {
        for (url, spot) in details_view::spots(app, width, height) {
            // Herdr cuts a layer larger than its cells: frames fit in them.
            let fit = (
                u32::from(spot.cells.columns) * u32::from(cell_width),
                u32::from(spot.rows) * u32::from(cell_height),
            );
            if let Some((file, budget)) = app.pictures.frames_to_decode(&url, fit) {
                let sender = sender.clone();
                thread::spawn(move || {
                    let frames = frames_within(&file, fit, budget).map(Arc::new);
                    let _ = sender.send(DetailsAnswer::Frames(url, fit, frames));
                });
            } else if let Some(animation) = app.pictures.animation(&url, fit) {
                shown.push((url, animation.clone(), spot));
            }
        }
    }
    player.show(&shown);
    app.covered = shown
        .iter()
        .filter(|(url, animation, _)| !animation.opaque && player.live(url))
        .map(|(url, ..)| url.clone())
        .collect();
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
    let request = OperationRequest {
        id: operation_id(),
        kind,
        source: app.target.source.clone(),
        commit: app.target.commit.clone(),
        args,
        confirmation: Some(confirmation),
    };
    let id = request.id.clone();
    let done = move |record| {
        let _ = sender.send(DetailsAnswer::Operation(Box::new(record)));
    };
    match start_operation(operations, request, done) {
        Ok(()) => app.operation_launched(id, kind),
        Err(StartError::Busy) => app
            .operation_refused("Another marketplace operation is running: request refused".into()),
        Err(StartError::Lock(error)) => {
            app.operation_refused(format!("Could not reserve the operation: {error}"))
        }
        Err(StartError::Spawn(error)) => app.operation_refused(format!("Could not start: {error}")),
    }
}

/// Tells an operation's result from an older one of the same source.
fn operation_id() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or(0);
    format!("{now}-{}", std::process::id())
}

/// Why a confirmed request did not start.
enum StartError {
    /// Another marketplace operation holds the lock.
    Busy,
    Lock(String),
    Spawn(String),
}

/// Reserves the operation lock and starts the worker, which runs on even if
/// the pane closes. `done` receives its result, or an unconfirmed one when
/// the worker reports none.
fn start_operation(
    operations: &FsOperations,
    request: OperationRequest,
    done: impl FnOnce(OperationRecord) + Send + 'static,
) -> Result<(), StartError> {
    let guard = match operations.try_begin() {
        Ok(Some(guard)) => guard,
        Ok(None) => return Err(StartError::Busy),
        Err(error) => return Err(StartError::Lock(error)),
    };
    let child = std::env::current_exe()
        .and_then(|binary| spawn_operation(&request, guard, &binary))
        .map_err(|error| StartError::Spawn(error.to_string()))?;
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
                "the operation stopped without reporting its result; check the registry".into();
            record.persistence_error = Some(error);
            record.finished_unix_ms = Some(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|elapsed| elapsed.as_millis() as u64)
                    .unwrap_or(0),
            );
            record
        });
        done(record);
    });
    Ok(())
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
        Ok(Some(plugin)) => InstalledView::At {
            commit: plugin.resolved_commit().unwrap_or_default().to_string(),
            version: plugin.version.clone(),
        },
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

/// The next input within `wait`, and when it was read.
fn next_input(wait: Duration) -> Result<Option<(Input, Instant)>, AppError> {
    if !event::poll(wait)? {
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

/// Mouse reports a pane asks for, in SGR encoding. Unlike crossterm's
/// `EnableMouseCapture`, no event for every move of the pointer.
#[derive(Clone, Copy)]
enum Mouse {
    /// Clicks and the wheel.
    Clicks,
    /// Clicks, the wheel, and the moves of a held button.
    Drags,
}

impl Command for Mouse {
    fn write_ansi(&self, f: &mut impl std::fmt::Write) -> std::fmt::Result {
        f.write_str(match self {
            Self::Clicks => "\x1b[?1000h\x1b[?1006h",
            Self::Drags => "\x1b[?1002h\x1b[?1006h",
        })
    }
}

struct DisableMouse;

impl Command for DisableMouse {
    fn write_ansi(&self, f: &mut impl std::fmt::Write) -> std::fmt::Result {
        f.write_str("\x1b[?1006l\x1b[?1002l\x1b[?1000l")
    }
}

fn setup(mouse: Mouse) -> Result<Screen, AppError> {
    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen, mouse, EnableFocusChange)?;
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

#[cfg(test)]
mod lifecycle_tests {
    use super::terminal_closed;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

    #[test]
    fn a_closed_pty_is_detected_even_without_an_input_event_or_a_write() {
        let (mut master, mut slave) = (-1, -1);
        // SAFETY: valid pointers for the two new descriptors, default termios.
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            },
            0
        );
        // SAFETY: openpty returned two owned descriptors.
        let (master, slave) =
            unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
        assert!(!terminal_closed(slave.as_raw_fd()));
        drop(master);
        assert!(terminal_closed(slave.as_raw_fd()));
    }
}
