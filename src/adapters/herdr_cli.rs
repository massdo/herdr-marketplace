use std::fs::File;
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::application::ports::{CommandOutput, HerdrCli};

/// Runs the `herdr` binary: `HERDR_BIN_PATH` inside Herdr, else `herdr`.
pub struct HerdrCommand {
    bin: PathBuf,
    operation_lock: Option<File>,
}

impl HerdrCommand {
    pub fn new(bin: PathBuf) -> Self {
        Self {
            bin,
            operation_lock: None,
        }
    }

    /// Herdr keeps the reservation if the reporting worker dies mid-command.
    pub fn with_operation_lock(mut self, lock: File) -> Self {
        self.operation_lock = Some(lock);
        self
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
        let mut command = Command::new(&self.bin);
        command.args(args).stdin(Stdio::null());
        if let Some(lock) = &self.operation_lock {
            let fd = lock.as_raw_fd();
            // SAFETY: fcntl allocates nothing and only changes the child.
            // self holds the descriptor throughout command execution.
            unsafe {
                command.pre_exec(move || {
                    if libc::fcntl(fd, libc::F_SETFD, 0) == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        }
        let output = command
            .output()
            .map_err(|error| format!("{}: {error}", self.bin.display()))?;
        let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
        text.push_str(&String::from_utf8_lossy(&output.stderr));
        Ok(CommandOutput {
            code: output.status.code(),
            output: text,
        })
    }
}
