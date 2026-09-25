use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::application::ports::HerdrCli;

/// Runs the `herdr` binary: `HERDR_BIN_PATH` inside Herdr, else `herdr`.
pub struct HerdrCommand {
    bin: PathBuf,
}

impl HerdrCommand {
    pub fn new(bin: PathBuf) -> Self {
        Self { bin }
    }
}

impl HerdrCli for HerdrCommand {
    fn version(&self) -> Result<String, String> {
        let output = Command::new(&self.bin)
            .arg("--version")
            .stdin(Stdio::null())
            .output()
            .map_err(|error| format!("{} : {error}", self.bin.display()))?;
        if !output.status.success() {
            return Err(format!(
                "{} --version a échoué ({})",
                self.bin.display(),
                output.status
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}
