use herdr_marketplace::adapters::env;
use herdr_marketplace::adapters::herdr_cli::HerdrCommand;
use herdr_marketplace::adapters::herdr_socket::HerdrSocket;
use herdr_marketplace::adapters::launcher_lock;
use herdr_marketplace::adapters::operations::FsOperations;
use herdr_marketplace::adapters::tui;
use herdr_marketplace::application::run_operation::run_operation;
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
        Some("--fiche") => tui::run_fiche(env::load()?, env::fiche_target()?),
        Some("--run-operation") => operation(args.get(1)),
        Some("--help" | "-h") => {
            println!("herdr-marketplace [--toggle | --fiche | --run-operation <request>]");
            Ok(())
        }
        Some(other) => Err(AppError::Io {
            message: format!("argument inconnu : {other}"),
        }),
        None => tui::run_sidebar(env::load()?),
    }
}

fn toggle() -> Result<(), AppError> {
    let process = env::load()?;
    let origin = env::origin_from_env()?;
    let _lock = launcher_lock::acquire(&process.state_dir)?;
    toggle_sidebar(&HerdrSocket::new(process.socket_path), &origin)?;
    Ok(())
}

/// Detached run of a confirmed request, started by a fiche.
fn operation(request: Option<&String>) -> Result<(), AppError> {
    let request: OperationRequest = request
        .and_then(|json| serde_json::from_str(json).ok())
        .ok_or_else(|| AppError::Io {
            message: "requête d'opération illisible".into(),
        })?;
    let process = env::load()?;
    run_operation(
        &HerdrCommand::new(env::herdr_bin()),
        &FsOperations::new(process.state_dir),
        &request,
    );
    Ok(())
}
