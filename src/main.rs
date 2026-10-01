use herdr_marketplace::adapters::env;
use herdr_marketplace::adapters::herdr_cli::HerdrCommand;
use herdr_marketplace::adapters::herdr_socket::HerdrSocket;
use herdr_marketplace::adapters::launcher_lock;
use herdr_marketplace::adapters::operations::{FsOperations, inherited_lock};
use herdr_marketplace::adapters::tui;
use herdr_marketplace::application::run_operation::{run_locked_operation, run_operation};
use herdr_marketplace::application::toggle_sidebar::toggle_sidebar;
use herdr_marketplace::domain::error::AppError;
use herdr_marketplace::domain::operation::OperationRequest;

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(error.exit_code());
    }
}

fn run() -> Result<(), AppError> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--toggle") => toggle(),
        Some("--details") => tui::run_details(env::load()?, env::details_target()?),
        Some("--video-cache") => {
            let owner = args
                .get(1)
                .and_then(|pid| pid.parse::<libc::pid_t>().ok())
                .ok_or_else(|| AppError::Io {
                    message: "invalid video cache owner".into(),
                })?;
            tui::video_cache::run_cache_process(
                &env::load()?.state_dir,
                owner,
                std::sync::Arc::new(
                    herdr_marketplace::adapters::image_fetch::ImageFetcher::default(),
                ),
            )
            .map_err(|message| AppError::Io { message })
        }
        Some("--run-operation") => operation(args.get(1), args.get(2)),
        Some("--help" | "-h") => {
            println!("herdr-marketplace [--toggle | --details | --run-operation <request>]");
            Ok(())
        }
        Some(other) => Err(AppError::Io {
            message: format!("unknown argument: {other}"),
        }),
        None => tui::run_sidebar(env::load()?),
    }
}

fn toggle() -> Result<(), AppError> {
    let process = env::load()?;
    let origin = env::origin_from_env()?;
    let _lock = launcher_lock::acquire(&process.state_dir)?;
    toggle_sidebar(
        &HerdrSocket::new(process.socket_path).with_details_state(process.state_dir),
        &origin,
    )?;
    Ok(())
}

/// Detached run of a confirmed request, started by a details pane.
fn operation(request: Option<&String>, fd: Option<&String>) -> Result<(), AppError> {
    use std::io::Read;
    let mut stdin_request = String::new();
    let request = if request.map(String::as_str) == Some("-") {
        std::io::stdin().read_to_string(&mut stdin_request)?;
        Some(&stdin_request)
    } else {
        request
    };
    let request: OperationRequest = request
        .and_then(|json| serde_json::from_str(json).ok())
        .ok_or_else(|| AppError::Io {
            message: "unreadable operation request".into(),
        })?;
    let process = env::load()?;
    let herdr = HerdrCommand::new(env::herdr_bin());
    let operations = FsOperations::new(process.state_dir);
    let record = match fd {
        Some(fd) => {
            let fd = fd.parse().map_err(|_| AppError::Io {
                message: "invalid operation lock descriptor".into(),
            })?;
            run_locked_operation(&herdr, &operations, &request, inherited_lock(fd)?)
        }
        None => run_operation(&herdr, &operations, &request),
    };
    // The result reaches an open pane even if it could not be saved. If the
    // pane closed, a broken pipe cannot undo execution or persistence.
    use std::io::Write;
    let _ = serde_json::to_writer(std::io::stdout().lock(), &record);
    let _ = std::io::stdout().flush();
    Ok(())
}
