use herdr_marketplace::adapters::env;
use herdr_marketplace::adapters::herdr_socket::HerdrSocket;
use herdr_marketplace::adapters::launcher_lock;
use herdr_marketplace::adapters::tui;
use herdr_marketplace::application::toggle_sidebar::toggle_sidebar;
use herdr_marketplace::domain::error::AppError;

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
        Some("--help" | "-h") => {
            println!("herdr-marketplace [--toggle]");
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
