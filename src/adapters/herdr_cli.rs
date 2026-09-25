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
            .map_err(|error| format!("{} : {error}", self.bin.display()))?;
        if !output.status.success() {
            return Err(format!(
                "herdr {} a échoué ({}) : {}",
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
        let output = Command::new(&self.bin)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .map_err(|error| format!("{} : {error}", self.bin.display()))?;
        let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
        text.push_str(&String::from_utf8_lossy(&output.stderr));
        Ok(CommandOutput {
            code: output.status.code(),
            output: text,
        })
    }
}
