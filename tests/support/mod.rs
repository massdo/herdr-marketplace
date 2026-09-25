//! Offline doubles shared by the integration tests.
#![allow(dead_code)]

use herdr_marketplace::application::ports::HerdrCli;
use herdr_marketplace::domain::index::{Catalog, parse_index};
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
}
