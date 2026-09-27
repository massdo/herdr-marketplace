use std::fs::{File, OpenOptions};
use std::path::Path;

use crate::domain::error::AppError;

/// Exclusive OS lock on `launcher.lock`. Released when this guard is dropped,
/// including on process death.
pub struct LauncherLock {
    _file: File,
}

/// Wait for an exclusive lock under `HERDR_PLUGIN_STATE_DIR` (or the XDG
/// fallback already resolved by `env::load`).
pub fn acquire(state_dir: &Path) -> Result<LauncherLock, AppError> {
    std::fs::create_dir_all(state_dir)?;
    let path = state_dir.join("launcher.lock");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)?;
    file.lock().map_err(|error| AppError::Io {
        message: format!("cannot acquire {}: {error}", path.display()),
    })?;
    Ok(LauncherLock { _file: file })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn second_acquirer_waits_until_the_first_drops() {
        let dir = std::env::temp_dir().join(format!(
            "herdr-marketplace-lock-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let held = acquire(&dir).unwrap();
        let dir_clone = dir.clone();
        let (started_tx, started_rx) = mpsc::channel();
        let (acquired_tx, acquired_rx) = mpsc::channel();
        let handle = thread::spawn(move || {
            started_tx.send(()).unwrap();
            let _second = acquire(&dir_clone).unwrap();
            acquired_tx.send(()).unwrap();
        });
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let while_held = acquired_rx.recv_timeout(Duration::from_millis(100));
        drop(held);
        assert_eq!(while_held, Err(mpsc::RecvTimeoutError::Timeout));
        acquired_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        handle.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
