//! Newline-delimited JSON client for the Herdr socket, adapted from herdr-npm
//! v0.1.0, itself adapted from herdr-sidebar 0.13.0 `ipc.rs` (MIT, see NOTICE).

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::application::ports::{HerdrPort, OpenPluginPane};
use crate::domain::error::AppError;
use crate::domain::ids::PaneId;
use crate::domain::pane::{LayoutSnapshot, OpenedPane, PaneInfo};
use crate::domain::{PLUGIN_ID, TOKEN_VALUE};

const IPC_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_RESPONSE_BYTES: u64 = 4 * 1024 * 1024;

pub struct HerdrSocket {
    path: PathBuf,
}

impl HerdrSocket {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn call(&self, method: &str, params: Value) -> Result<Value, AppError> {
        let id = format!("{PLUGIN_ID}:{method}");
        let request = json!({
            "id": id,
            "method": method,
            "params": params,
        });
        let line = roundtrip(&self.path, &request.to_string(), method)?;
        let value: Value = serde_json::from_str(line.trim())
            .map_err(|error| AppError::uncertain(method, format!("réponse non JSON : {error}")))?;
        let got_id = value.get("id").and_then(Value::as_str);
        if got_id != Some(id.as_str()) {
            return Err(AppError::uncertain(
                method,
                format!("l'id de réponse {got_id:?} ne correspond pas à la requête"),
            ));
        }
        if let Some(error) = value.get("error") {
            let code = error
                .get("code")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string();
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("erreur Herdr")
                .to_string();
            return Err(AppError::herdr(method, code, message));
        }
        value
            .get("result")
            .cloned()
            .ok_or_else(|| AppError::uncertain(method, "réponse sans result ni error"))
    }
}

impl HerdrPort for HerdrSocket {
    fn list_panes(&self, workspace_id: Option<&str>) -> Result<Vec<PaneInfo>, AppError> {
        let params = match workspace_id {
            Some(id) => json!({ "workspace_id": id }),
            None => json!({}),
        };
        let result = self.call("pane.list", params)?;
        parse_pane_list(result)
    }

    fn pane_layout(&self, pane_id: &PaneId) -> Result<LayoutSnapshot, AppError> {
        let result = self.call("pane.layout", json!({ "pane_id": pane_id.as_str() }))?;
        let layout = result
            .get("layout")
            .cloned()
            .ok_or_else(|| AppError::SnapshotUnreadable {
                detail: "pane.layout sans layout".into(),
            })?;
        serde_json::from_value(layout).map_err(|error| AppError::SnapshotUnreadable {
            detail: error.to_string(),
        })
    }

    fn open_plugin_pane(&self, request: OpenPluginPane) -> Result<OpenedPane, AppError> {
        // workspace_id next to a split target makes Herdr 0.9.1 reject the
        // request ("use target_pane_id"). The target pane implies the tab.
        let result = self.call(
            "plugin.pane.open",
            json!({
                "plugin_id": request.plugin_id,
                "entrypoint": request.entrypoint,
                "placement": "split",
                "direction": "right",
                "target_pane_id": request.target_pane_id.as_str(),
                "focus": request.focus,
                "env": request.env,
            }),
        )?;
        let pane_id = result
            .get("plugin_pane")
            .and_then(|pane| pane.get("pane"))
            .and_then(|pane| pane.get("pane_id"))
            .and_then(Value::as_str)
            .filter(|id| !id.trim().is_empty())
            .ok_or_else(|| {
                AppError::uncertain("plugin.pane.open", "réponse sans plugin_pane.pane.pane_id")
            })?;
        Ok(OpenedPane {
            pane_id: PaneId(pane_id.to_string()),
        })
    }

    fn swap_panes(&self, source: &PaneId, target: &PaneId) -> Result<(), AppError> {
        self.call(
            "pane.swap",
            json!({
                "source_pane_id": source.as_str(),
                "target_pane_id": target.as_str(),
            }),
        )?;
        Ok(())
    }

    fn focus_pane(&self, pane_id: &PaneId) -> Result<(), AppError> {
        self.call("pane.focus", json!({ "pane_id": pane_id.as_str() }))?;
        Ok(())
    }

    fn resize_pane(&self, pane_id: &PaneId, direction: &str, amount: f64) -> Result<(), AppError> {
        self.call(
            "pane.resize",
            json!({
                "pane_id": pane_id.as_str(),
                "direction": direction,
                "amount": amount,
            }),
        )?;
        Ok(())
    }

    fn report_identity(&self, pane_id: &PaneId, token_key: &str) -> Result<(), AppError> {
        let mut tokens = BTreeMap::new();
        tokens.insert(token_key.to_string(), TOKEN_VALUE.to_string());
        self.call(
            "pane.report_metadata",
            json!({
                "pane_id": pane_id.as_str(),
                "source": PLUGIN_ID,
                "tokens": tokens,
            }),
        )?;
        Ok(())
    }

    fn close_plugin_pane(&self, pane_id: &PaneId) -> Result<(), AppError> {
        self.call("plugin.pane.close", json!({ "pane_id": pane_id.as_str() }))?;
        Ok(())
    }
}

#[derive(Deserialize)]
struct PaneListResult {
    panes: Vec<PaneInfo>,
}

fn parse_pane_list(result: Value) -> Result<Vec<PaneInfo>, AppError> {
    serde_json::from_value::<PaneListResult>(result)
        .map(|body| body.panes)
        .map_err(|error| AppError::SnapshotUnreadable {
            detail: error.to_string(),
        })
}

fn roundtrip(path: &Path, request: &str, method: &str) -> Result<String, AppError> {
    let before_send =
        |error: std::io::Error| AppError::herdr(method, "transport", error.to_string());
    let after_send = |error: std::io::Error| AppError::uncertain(method, error.to_string());
    let stream = UnixStream::connect(path).map_err(before_send)?;
    stream
        .set_read_timeout(Some(IPC_TIMEOUT))
        .map_err(before_send)?;
    stream
        .set_write_timeout(Some(IPC_TIMEOUT))
        .map_err(before_send)?;
    let mut stream = stream;
    // Once writing starts, an error cannot prove that Herdr did not apply the request.
    stream.write_all(request.as_bytes()).map_err(after_send)?;
    stream.write_all(b"\n").map_err(after_send)?;
    stream.flush().map_err(after_send)?;
    let mut line = String::new();
    BufReader::new(stream.take(MAX_RESPONSE_BYTES))
        .read_line(&mut line)
        .map_err(after_send)?;
    if !line.ends_with('\n') {
        return Err(AppError::uncertain(
            method,
            "réponse du socket vide, incomplète ou trop longue",
        ));
    }
    Ok(line)
}
