//! Index loading, search, order and compatibility, offline.

use std::cell::Cell;
use std::path::PathBuf;

use herdr_marketplace::adapters::fetch::HttpFetcher;
use herdr_marketplace::application::load_catalog::{LoadError, load_catalog};
use herdr_marketplace::application::ports::{FetchError, Fetcher, HerdrCli};
use herdr_marketplace::domain::compat::Platform;
use herdr_marketplace::domain::index::{Catalog, Entry, IndexError, parse_index};
use herdr_marketplace::domain::search::search;
use herdr_marketplace::domain::text::clean;
use herdr_marketplace::domain::version::Version;
use serde_json::{Value, json};

const SHA: &str = "c8268d42a98d9140254f4bf4ca13c23a587faed8";
const HERDR: Version = Version {
    major: 0,
    minor: 9,
    patch: 1,
};

fn manifest(path: &str, id: &str) -> Value {
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

fn repo(owner: &str, name: &str, stars: u64, manifests: Vec<Value>) -> Value {
    json!({
        "id": 42,
        "fullName": format!("{owner}/{name}"),
        "owner": owner,
        "name": name,
        "description": "Repository description",
        "stars": stars,
        "topics": ["herdr-plugin", "terminal"],
        "headCommit": SHA,
        "manifests": manifests,
    })
}

fn index(repos: Vec<Value>) -> Vec<u8> {
    let count: usize = repos
        .iter()
        .map(|repo| repo["manifests"].as_array().map_or(0, Vec::len))
        .sum();
    serde_json::to_vec(&json!({
        "schemaVersion": 1,
        "generatedAt": "2026-09-25T14:01:00.000Z",
        "pluginCount": count,
        "repositoryCount": repos.len(),
        "source": {"provider": "github", "query": "topic:herdr-plugin is:public"},
        "plugins": repos,
    }))
    .unwrap()
}

fn parse(repos: Vec<Value>) -> Catalog {
    parse_index(&index(repos)).unwrap()
}

fn only(catalog: &Catalog) -> &Entry {
    assert_eq!(catalog.entries.len(), 1, "{catalog:?}");
    &catalog.entries[0]
}

struct FakeHerdr(&'static str);

impl HerdrCli for FakeHerdr {
    fn version(&self) -> Result<String, String> {
        Ok(self.0.to_string())
    }

    fn plugin_list(&self) -> Result<String, String> {
        Ok(r#"{"id":"cli:plugin","result":{"plugins":[],"type":"plugin_list"}}"#.into())
    }
}

struct CountingFetcher {
    body: Vec<u8>,
    calls: Cell<usize>,
}

impl Fetcher for CountingFetcher {
    fn fetch(&self, _url: &str, _limit: u64) -> Result<Vec<u8>, FetchError> {
        self.calls.set(self.calls.get() + 1);
        Ok(self.body.clone())
    }
}

#[test]
fn a_root_manifest_has_an_empty_subdir() {
    let catalog = parse(vec![repo(
        "massdo",
        "fixture",
        1,
        vec![manifest("herdr-plugin.toml", "fixture")],
    )]);
    let entry = only(&catalog);
    assert_eq!(entry.source.owner, "massdo");
    assert_eq!(entry.source.repo, "fixture");
    assert_eq!(entry.source.subdir, "");
    assert_eq!(entry.source.to_string(), "massdo/fixture");
    assert_eq!(entry.commit, SHA);
}

#[test]
fn the_subdir_is_the_folder_of_the_manifest() {
    let catalog = parse(vec![
        repo(
            "zenbu-labs",
            "terminal-browser",
            2,
            vec![manifest("herdr-plugin/herdr-plugin.toml", "browser")],
        ),
        repo(
            "acme",
            "mono",
            1,
            vec![manifest("plugins/a/herdr-plugin.toml", "a")],
        ),
    ]);
    let subdirs: Vec<&str> = catalog
        .entries
        .iter()
        .map(|entry| entry.source.subdir.as_str())
        .collect();
    assert_eq!(subdirs, ["herdr-plugin", "plugins/a"]);
    assert_eq!(
        catalog.entries[0].source.to_string(),
        "zenbu-labs/terminal-browser/herdr-plugin"
    );
}

#[test]
fn each_manifest_of_a_repository_is_an_entry() {
    let catalog = parse(vec![repo(
        "acme",
        "mono",
        1,
        vec![
            manifest("herdr-plugin.toml", "root"),
            manifest("alt/herdr-plugin.toml", "alt"),
        ],
    )]);
    assert_eq!(catalog.entries.len(), 2);
    assert_eq!(catalog.read, 2);
    assert_eq!(catalog.plugin_count, Some(2));
}

#[test]
fn entries_with_the_same_id_stay_distinct() {
    let catalog = parse(vec![
        repo(
            "one",
            "plugin",
            1,
            vec![manifest("herdr-plugin.toml", "same")],
        ),
        repo(
            "two",
            "plugin",
            1,
            vec![manifest("herdr-plugin.toml", "same")],
        ),
        repo(
            "one",
            "plugin-alt",
            1,
            vec![
                manifest("herdr-plugin.toml", "same"),
                manifest("alt/herdr-plugin.toml", "same"),
            ],
        ),
    ]);
    let sources: Vec<String> = catalog
        .entries
        .iter()
        .map(|entry| entry.source.to_string())
        .collect();
    assert_eq!(
        sources,
        [
            "one/plugin",
            "one/plugin-alt",
            "one/plugin-alt/alt",
            "two/plugin"
        ]
    );
    assert!(catalog.entries.iter().all(|entry| entry.id == "same"));
}

#[test]
fn optional_fields_may_be_absent() {
    let catalog = parse(vec![json!({
        "owner": "bare",
        "name": "plugin",
        "headCommit": SHA,
        "manifests": [{"path": "herdr-plugin.toml", "id": "bare.plugin"}],
    })]);
    let entry = only(&catalog);
    assert_eq!(entry.name, "bare.plugin");
    assert_eq!(entry.version, None);
    assert_eq!(entry.description, None);
    assert_eq!(entry.stars, 0);
    assert!(entry.topics.is_empty());
    assert_eq!(entry.platforms, None);
    assert_eq!(entry.min_herdr_version, None);
}

#[test]
fn the_repository_description_stands_in_for_a_missing_one() {
    let mut plugin = manifest("herdr-plugin.toml", "plugin");
    plugin.as_object_mut().unwrap().remove("description");
    let catalog = parse(vec![repo("acme", "plugin", 1, vec![plugin])]);
    assert_eq!(
        only(&catalog).description.as_deref(),
        Some("Repository description")
    );
}

#[test]
fn null_platforms_are_compatible_everywhere() {
    let mut plugin = manifest("herdr-plugin.toml", "plugin");
    plugin["platforms"] = Value::Null;
    let catalog = parse(vec![repo("acme", "plugin", 1, vec![plugin])]);
    let entry = only(&catalog);
    assert_eq!(entry.platforms, None);
    assert!(entry.is_compatible(Platform::Macos, HERDR));
    assert!(entry.is_compatible(Platform::Linux, HERDR));
}

#[test]
fn the_host_platform_must_be_declared() {
    let mut plugin = manifest("herdr-plugin.toml", "plugin");
    plugin["platforms"] = json!(["linux"]);
    let catalog = parse(vec![repo("acme", "plugin", 1, vec![plugin])]);
    let entry = only(&catalog);
    assert!(entry.is_compatible(Platform::Linux, HERDR));
    assert!(!entry.is_compatible(Platform::Macos, HERDR));
}

#[test]
fn the_plugin_version_does_not_matter() {
    let mut plugin = manifest("herdr-plugin.toml", "plugin");
    plugin["version"] = json!("0.7.0rc4");
    let catalog = parse(vec![repo("acme", "plugin", 1, vec![plugin])]);
    let entry = only(&catalog);
    assert_eq!(entry.version.as_deref(), Some("0.7.0rc4"));
    assert!(entry.is_compatible(Platform::Macos, HERDR));
}

#[test]
fn min_herdr_version_is_read_like_herdr() {
    let cases = [
        (json!("0.9.1"), true),
        (json!("0.8.2"), true),
        (json!("v0.9.1"), true),
        (json!(" 0.9.1 "), true),
        (json!("0.9.2"), false),
        (json!("1.0.0"), false),
        (json!("0.9"), false),
        (json!("0.9.1.0"), false),
        (json!("0.9.1-rc1"), false),
        (json!("latest"), false),
        (json!(""), false),
        (Value::Null, false),
    ];
    for (min, compatible) in cases {
        let mut plugin = manifest("herdr-plugin.toml", "plugin");
        plugin["minHerdrVersion"] = min.clone();
        let catalog = parse(vec![repo("acme", "plugin", 1, vec![plugin])]);
        assert_eq!(
            only(&catalog).is_compatible(Platform::Macos, HERDR),
            compatible,
            "minHerdrVersion {min}"
        );
    }
    let mut plugin = manifest("herdr-plugin.toml", "plugin");
    plugin.as_object_mut().unwrap().remove("minHerdrVersion");
    let catalog = parse(vec![repo("acme", "plugin", 1, vec![plugin])]);
    assert!(!only(&catalog).is_compatible(Platform::Macos, HERDR));
}

#[test]
fn herdr_version_output_is_read_like_herdr() {
    assert_eq!(Version::from_herdr_output("herdr 0.9.1\n"), Some(HERDR));
    assert_eq!(
        Version::from_herdr_output("herdr 0.9.1-preview.42"),
        Some(HERDR)
    );
    assert_eq!(Version::from_herdr_output("herdr dev"), None);
    assert_eq!(Version::from_herdr_output("0.9.1"), None);
    assert_eq!(Version::from_herdr_output(""), None);
}

#[test]
fn an_unreadable_herdr_version_is_an_error_instead_of_a_list() {
    let fetcher = CountingFetcher {
        body: index(vec![repo(
            "acme",
            "plugin",
            1,
            vec![manifest("herdr-plugin.toml", "plugin")],
        )]),
        calls: Cell::new(0),
    };
    let result = load_catalog(&fetcher, &FakeHerdr("herdr unknown\n"), "file:///unused");
    assert!(
        matches!(result, Err(LoadError::HerdrVersion(_))),
        "{result:?}"
    );
    let message = result.unwrap_err().to_string();
    assert!(
        message.contains("version de Herdr indéterminable"),
        "{message}"
    );
}

#[test]
fn invalid_entries_are_rejected_and_counted() {
    let mut short_sha = repo(
        "bad",
        "short-sha",
        1,
        vec![manifest("herdr-plugin.toml", "x")],
    );
    short_sha["headCommit"] = json!("c8268d4");
    let mut upper_sha = repo(
        "bad",
        "upper-sha",
        1,
        vec![manifest("herdr-plugin.toml", "x")],
    );
    upper_sha["headCommit"] = json!(SHA.to_uppercase());
    let mut no_sha = repo("bad", "no-sha", 1, vec![manifest("herdr-plugin.toml", "x")]);
    no_sha.as_object_mut().unwrap().remove("headCommit");
    let mut typed_owner = repo(
        "bad",
        "typed-owner",
        1,
        vec![manifest("herdr-plugin.toml", "x")],
    );
    typed_owner["owner"] = json!(7);
    let mut no_id = manifest("herdr-plugin.toml", "x");
    no_id.as_object_mut().unwrap().remove("id");
    let bad_paths = repo(
        "bad",
        "paths",
        1,
        vec![
            manifest("README.md", "x"),
            manifest("xherdr-plugin.toml", "x"),
            manifest("/herdr-plugin.toml", "x"),
            manifest("a//herdr-plugin.toml", "x"),
            manifest("../herdr-plugin.toml", "x"),
            manifest("a/./herdr-plugin.toml", "x"),
            manifest("a\\b/herdr-plugin.toml", "x"),
            no_id,
            json!("not an object"),
        ],
    );
    let good = repo(
        "good",
        "plugin",
        1,
        vec![manifest("herdr-plugin.toml", "ok")],
    );
    let catalog = parse(vec![
        short_sha,
        upper_sha,
        no_sha,
        typed_owner,
        bad_paths,
        good,
    ]);
    assert_eq!(only(&catalog).id, "ok");
    assert_eq!(catalog.read, 14);
    assert_eq!(catalog.rejected, 13);
    assert_eq!(catalog.entries.len() + catalog.rejected, catalog.read);
}

#[test]
fn invalid_json_is_an_error_never_a_partial_list() {
    let mut body = index(vec![repo(
        "acme",
        "plugin",
        1,
        vec![manifest("herdr-plugin.toml", "plugin")],
    )]);
    body.truncate(body.len() - 10);
    assert!(matches!(
        parse_index(&body),
        Err(IndexError::InvalidJson(_))
    ));
    assert!(matches!(
        parse_index(b"<html>"),
        Err(IndexError::InvalidJson(_))
    ));
}

#[test]
fn an_unknown_schema_version_is_an_error() {
    for schema in [json!(2), json!("1"), json!(0)] {
        let body = serde_json::to_vec(&json!({"schemaVersion": schema, "plugins": []})).unwrap();
        assert!(
            matches!(parse_index(&body), Err(IndexError::UnknownSchema(_))),
            "schemaVersion {schema}"
        );
    }
    let body = serde_json::to_vec(&json!({"plugins": []})).unwrap();
    let error = parse_index(&body).unwrap_err();
    assert_eq!(error, IndexError::UnknownSchema("absent".into()));
    assert!(
        error.to_string().contains("schemaVersion inconnu"),
        "{error}"
    );
}

#[test]
fn a_missing_plugin_list_is_an_error() {
    let body = serde_json::to_vec(&json!({"schemaVersion": 1})).unwrap();
    assert_eq!(parse_index(&body), Err(IndexError::MissingPlugins));
}

#[test]
fn the_index_is_read_from_a_file_url() {
    let path = temp_file(
        "index.json",
        &index(vec![repo(
            "acme",
            "plugin",
            1,
            vec![manifest("herdr-plugin.toml", "plugin")],
        )]),
    );
    let url = format!("file://{}", path.display());
    let loaded = load_catalog(&HttpFetcher::new(), &FakeHerdr("herdr 0.9.1\n"), &url).unwrap();
    assert_eq!(loaded.herdr, HERDR);
    assert_eq!(only(&loaded.catalog).id, "plugin");

    let missing = format!("file://{}.absent", path.display());
    assert_eq!(
        HttpFetcher::new().fetch(&missing, 1024),
        Err(FetchError::NotFound)
    );
    assert!(matches!(
        HttpFetcher::new().fetch(&url, 8),
        Err(FetchError::Failed(_))
    ));
    let _ = std::fs::remove_file(path);
}

#[test]
fn the_catalogue_is_sorted_by_stars_then_identity() {
    let catalog = parse(vec![
        repo(
            "zeta",
            "plugin",
            5,
            vec![manifest("herdr-plugin.toml", "z")],
        ),
        repo(
            "alpha",
            "plugin",
            5,
            vec![manifest("b/herdr-plugin.toml", "b")],
        ),
        repo(
            "alpha",
            "plugin",
            5,
            vec![manifest("a/herdr-plugin.toml", "a")],
        ),
        repo(
            "popular",
            "plugin",
            900,
            vec![manifest("herdr-plugin.toml", "p")],
        ),
        repo(
            "quiet",
            "plugin",
            0,
            vec![manifest("herdr-plugin.toml", "q")],
        ),
    ]);
    let ids: Vec<&str> = catalog
        .entries
        .iter()
        .map(|entry| entry.id.as_str())
        .collect();
    assert_eq!(ids, ["p", "a", "b", "z", "q"]);
}

#[test]
fn search_is_a_case_insensitive_substring_on_the_listed_fields() {
    let mut other = repo(
        "someone",
        "unrelated",
        1,
        vec![manifest("herdr-plugin.toml", "other")],
    );
    other["topics"] = json!(["misc"]);
    other["manifests"][0]["description"] = json!("Nothing to see");
    other["manifests"][0]["name"] = json!("Other");
    let mut browser = repo(
        "zenbu-labs",
        "terminal-browser",
        10,
        vec![manifest(
            "herdr-plugin/herdr-plugin.toml",
            "zenbu-labs.terminal-browser",
        )],
    );
    browser["topics"] = json!(["web", "Claude-Code"]);
    browser["manifests"][0]["name"] = json!("Terminal Browser");
    browser["manifests"][0]["description"] = json!("Open a browser inside herdr");
    let catalog = parse(vec![other, browser]);
    let entries = &catalog.entries;
    let found = |query: &str| -> Vec<&str> {
        search(entries, query)
            .into_iter()
            .map(|index| entries[index].id.as_str())
            .collect()
    };
    let browser_only = ["zenbu-labs.terminal-browser"];
    assert_eq!(found("terminal BROWSER"), browser_only, "name");
    assert_eq!(found("ZENBU-LABS.term"), browser_only, "id");
    assert_eq!(found("inside HERDR"), browser_only, "description");
    assert_eq!(found("labs/terminal"), browser_only, "owner/repo");
    assert_eq!(found("claude-code"), browser_only, "topics");
    assert!(found("no such plugin").is_empty());
    assert_eq!(found(""), ["zenbu-labs.terminal-browser", "other"]);
    assert_eq!(found("   "), ["zenbu-labs.terminal-browser", "other"]);
}

#[test]
fn searching_makes_no_network_call() {
    let fetcher = CountingFetcher {
        body: index(vec![repo(
            "acme",
            "plugin",
            1,
            vec![manifest("herdr-plugin.toml", "plugin")],
        )]),
        calls: Cell::new(0),
    };
    let loaded = load_catalog(
        &fetcher,
        &FakeHerdr("herdr 0.9.1"),
        "https://example.invalid",
    )
    .unwrap();
    assert_eq!(fetcher.calls.get(), 1);
    for query in ["p", "pl", "plu", "", "zzz", "acme/plugin"] {
        search(&loaded.catalog.entries, query);
    }
    assert_eq!(fetcher.calls.get(), 1);
}

#[test]
fn control_characters_are_removed_before_display() {
    assert_eq!(
        clean("a\u{1b}[31mred\u{7}\u{9b}2J\u{7f}\tb\nc"),
        "a[31mred2J b c"
    );
    assert_eq!(clean("Café ★ plugin"), "Café ★ plugin");
}

fn temp_file(name: &str, body: &[u8]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "herdr-marketplace-test-{}-{}",
        std::process::id(),
        name
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, body).unwrap();
    path
}
