use std::os::unix::fs::FileExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::application::ports::{CommandOutput, HerdrCli};

/// Runs the `herdr` binary: `HERDR_BIN_PATH` inside Herdr, else `herdr`.
pub struct HerdrCommand {
    bin: PathBuf,
}

impl HerdrCommand {
    pub fn new(bin: PathBuf) -> Self {
        Self { bin }
    }

    fn stdout(&self, args: &[&str]) -> Result<String, String> {
        let output = Command::new(&self.bin)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .map_err(|error| format!("{}: {error}", self.bin.display()))?;
        if !output.status.success() {
            return Err(format!(
                "herdr {} failed ({}): {}",
                args.join(" "),
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

impl HerdrCli for HerdrCommand {
    fn version(&self) -> Result<String, String> {
        self.stdout(&["--version"])
    }

    fn plugin_list(&self) -> Result<String, String> {
        self.stdout(&["plugin", "list", "--json"])
    }

    fn run(&self, args: &[String]) -> Result<CommandOutput, String> {
        // A build daemon may keep stdout/stderr open after Herdr exits. A
        // shared anonymous file has no pipe EOF to wait for, and no lock is
        // passed beyond the worker. Read only the bounded tail at exit.
        let log = tempfile::tempfile().map_err(|error| error.to_string())?;
        let status = Command::new(&self.bin)
            .args(args)
            .stdin(Stdio::null())
            .stdout(log.try_clone().map_err(|error| error.to_string())?)
            .stderr(log.try_clone().map_err(|error| error.to_string())?)
            .status()
            .map_err(|error| format!("{}: {error}", self.bin.display()))?;
        let end = log.metadata().map_err(|error| error.to_string())?.len();
        let limit = crate::application::run_operation::OUTPUT_LIMIT as u64;
        let mut bytes = vec![0; end.min(limit) as usize];
        // Do not move the shared write offset of a surviving build daemon.
        log.read_exact_at(&mut bytes, end.saturating_sub(limit))
            .map_err(|error| error.to_string())?;
        Ok(CommandOutput {
            code: status.code(),
            output: String::from_utf8_lossy(&bytes).into_owned(),
        })
    }
}
