//! Offline doubles shared by the integration tests.
#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::BTreeMap;

use herdr_marketplace::application::ports::{CommandOutput, HerdrCli, HerdrPort, OpenPluginPane};
use herdr_marketplace::domain::error::AppError;
use herdr_marketplace::domain::ids::PaneId;
use herdr_marketplace::domain::index::{Catalog, parse_index};
use herdr_marketplace::domain::pane::{
    LayoutPane, LayoutRect, LayoutSnapshot, OpenedPane, PaneInfo,
};
use herdr_marketplace::domain::version::Version;
use serde_json::{Value, json};

pub const SHA_A: &str = "c8268d42a98d9140254f4bf4ca13c23a587faed8";
pub const SHA_B: &str = "1be1b7bb9d9ad3d8733a66d132b87c908ff210c6";
pub const HERDR: Version = Version {
    major: 0,
    minor: 9,
    patch: 1,
};

pub fn manifest(path: &str, id: &str) -> Value {
    json!({
        "path": path,
        "id": id,
        "name": format!("{id} name"),
        "version": "1.0.0",
        "description": format!("{id} description"),
        "platforms": ["linux", "macos"],
        "minHerdrVersion": "0.9.1",
    })
}

pub fn repo(owner: &str, name: &str, stars: u64, manifests: Vec<Value>) -> Value {
    json!({
        "owner": owner,
        "name": name,
        "fullName": format!("{owner}/{name}"),
        "description": "Repository description",
        "stars": stars,
        "topics": ["herdr-plugin"],
        "headCommit": SHA_A,
        "manifests": manifests,
    })
}

pub fn index(repos: Vec<Value>) -> Vec<u8> {
    let count: usize = repos
        .iter()
        .map(|repo| repo["manifests"].as_array().map_or(0, Vec::len))
        .sum();
    serde_json::to_vec(&json!({
        "schemaVersion": 1,
        "pluginCount": count,
        "plugins": repos,
    }))
    .unwrap()
}

pub fn catalog(repos: Vec<Value>) -> Catalog {
    parse_index(&index(repos)).unwrap()
}

/// One entry of `herdr plugin list --json`, installed from GitHub.
pub fn github_plugin(id: &str, owner: &str, repo: &str, subdir: Option<&str>, sha: &str) -> Value {
    let mut source = json!({
        "kind": "github",
        "owner": owner,
        "repo": repo,
        "requested_ref": sha,
        "resolved_commit": sha,
        "managed_path": format!("/tmp/xdg/herdr/plugins/github/{id}"),
    });
    if let Some(subdir) = subdir {
        source["subdir"] = json!(subdir);
    }
    json!({
        "plugin_id": id,
        "name": format!("{id} installed"),
        "version": "1.0.0",
        "min_herdr_version": "0.9.1",
        "enabled": true,
        "manifest_path": "/tmp/xdg/herdr/plugins/github/herdr-plugin.toml",
        "plugin_root": "/tmp/xdg/herdr/plugins/github",
        "platforms": ["linux", "macos"],
        "source": source,
    })
}

pub fn local_plugin(id: &str) -> Value {
    json!({
        "plugin_id": id,
        "name": id,
        "version": "0.1.0",
        "min_herdr_version": "0.9.1",
        "enabled": true,
        "manifest_path": "/work/herdr-plugin.toml",
        "plugin_root": "/work",
        "source": {"kind": "local"},
    })
}

pub fn registry(plugins: Vec<Value>) -> String {
    json!({"id": "cli:plugin", "result": {"plugins": plugins, "type": "plugin_list"}}).to_string()
}

/// `herdr` whose version and registry are given.
pub struct FakeHerdr {
    pub version: String,
    pub registry: Result<String, String>,
}

impl FakeHerdr {
    pub fn with_registry(plugins: Vec<Value>) -> Self {
        Self {
            version: "herdr 0.9.1\n".into(),
            registry: Ok(registry(plugins)),
        }
    }
}

impl HerdrCli for FakeHerdr {
    fn version(&self) -> Result<String, String> {
        Ok(self.version.clone())
    }

    fn plugin_list(&self) -> Result<String, String> {
        self.registry.clone()
    }

    fn run(&self, args: &[String]) -> Result<CommandOutput, String> {
        Err(format!("unexpected herdr {args:?}"))
    }
}

pub fn pane(id: &str, tab: &str, token: Option<&str>) -> PaneInfo {
    PaneInfo {
        pane_id: id.into(),
        workspace_id: "w1".into(),
        tab_id: tab.into(),
        focused: false,
        label: None,
        title: None,
        tokens: token
            .map(|key| BTreeMap::from([(key.to_string(), "v1".to_string())]))
            .unwrap_or_default(),
    }
}

/// Herdr socket double: a fixed pane list, every call recorded in order.
pub struct FakePanes {
    pub panes: RefCell<Vec<PaneInfo>>,
    pub calls: RefCell<Vec<String>>,
    pub opened: RefCell<Vec<OpenPluginPane>>,
}

impl FakePanes {
    pub fn new(panes: Vec<PaneInfo>) -> Self {
        Self {
            panes: RefCell::new(panes),
            calls: RefCell::new(Vec::new()),
            opened: RefCell::new(Vec::new()),
        }
    }

    fn record(&self, call: String) {
        self.calls.borrow_mut().push(call);
    }
}

impl HerdrPort for FakePanes {
    fn list_panes(&self, _workspace_id: Option<&str>) -> Result<Vec<PaneInfo>, AppError> {
        Ok(self.panes.borrow().clone())
    }

    fn pane_layout(&self, pane_id: &PaneId) -> Result<LayoutSnapshot, AppError> {
        let panes = self.panes.borrow();
        let tab = panes
            .iter()
            .find(|pane| pane.pane_id == pane_id.0)
            .map(|pane| pane.tab_id.clone())
            .unwrap_or_default();
        let layout_panes = panes
            .iter()
            .filter(|pane| pane.tab_id == tab)
            .enumerate()
            .map(|(index, pane)| LayoutPane {
                pane_id: pane.pane_id.clone(),
                focused: false,
                rect: LayoutRect {
                    x: index as u16 * 40,
                    y: 0,
                    width: 40,
                    height: 40,
                },
            })
            .collect();
        Ok(LayoutSnapshot {
            workspace_id: "w1".into(),
            tab_id: tab,
            area: LayoutRect {
                x: 0,
                y: 0,
                width: 160,
                height: 40,
            },
            focused_pane_id: pane_id.0.clone(),
            panes: layout_panes,
            splits: Vec::new(),
        })
    }

    fn open_plugin_pane(&self, request: OpenPluginPane) -> Result<OpenedPane, AppError> {
        self.record(format!(
            "open {} next to {}",
            request.entrypoint, request.target_pane_id
        ));
        self.opened.borrow_mut().push(request);
        Ok(OpenedPane {
            pane_id: PaneId("w1:new".into()),
        })
    }

    fn swap_panes(&self, source: &PaneId, target: &PaneId) -> Result<(), AppError> {
        self.record(format!("swap {source} {target}"));
        Ok(())
    }

    fn focus_pane(&self, pane_id: &PaneId) -> Result<(), AppError> {
        self.record(format!("focus {pane_id}"));
        Ok(())
    }

    fn resize_pane(&self, pane_id: &PaneId, direction: &str, _amount: f64) -> Result<(), AppError> {
        self.record(format!("resize {pane_id} {direction}"));
        Ok(())
    }

    fn report_identity(&self, pane_id: &PaneId, token_key: &str) -> Result<(), AppError> {
        self.record(format!("identity {pane_id} {token_key}"));
        Ok(())
    }

    fn close_plugin_pane(&self, pane_id: &PaneId) -> Result<(), AppError> {
        self.record(format!("close {pane_id}"));
        self.panes
            .borrow_mut()
            .retain(|pane| pane.pane_id != pane_id.0);
        Ok(())
    }
}
