//! Index loading, search, order and compatibility, offline.

use std::cell::Cell;
use std::path::PathBuf;

use herdr_marketplace::adapters::fetch::HttpFetcher;
use herdr_marketplace::application::load_catalog::{LoadError, load_catalog};
use herdr_marketplace::application::ports::{CommandOutput, FetchError, Fetcher, HerdrCli};
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

    fn run(&self, args: &[String]) -> Result<CommandOutput, String> {
        Err(format!("unexpected herdr {args:?}"))
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
        message.contains("cannot determine the Herdr version"),
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
    let dot_owner = repo("..", "plugin", 1, vec![manifest("herdr-plugin.toml", "x")]);
    let spaced_repo = repo(
        "bad",
        "my plugin",
        1,
        vec![manifest("herdr-plugin.toml", "x")],
    );
    let catalog = parse(vec![
        short_sha,
        upper_sha,
        no_sha,
        typed_owner,
        bad_paths,
        dot_owner,
        spaced_repo,
        good,
    ]);
    assert_eq!(only(&catalog).id, "ok");
    assert_eq!(catalog.read, 16);
    assert_eq!(catalog.rejected, 15);
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
    assert_eq!(error, IndexError::UnknownSchema("missing".into()));
    assert!(
        error.to_string().contains("unknown schemaVersion"),
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
fn search_ignores_case_on_the_listed_fields() {
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

/// One plugin per repository: name, description and topics given.
fn plugins(specs: &[(&str, &str, &str, &[&str])]) -> Vec<Entry> {
    let repos = specs
        .iter()
        .enumerate()
        .map(|(index, (repo_name, name, description, topics))| {
            let mut repo = repo(
                "acme",
                repo_name,
                100 - index as u64,
                vec![manifest("herdr-plugin.toml", &format!("acme.{repo_name}"))],
            );
            repo["topics"] = json!(topics);
            repo["manifests"][0]["name"] = json!(name);
            repo["manifests"][0]["description"] = json!(description);
            repo
        })
        .collect();
    parse(repos).entries
}

fn found_repos(entries: &[Entry], query: &str) -> Vec<String> {
    search(entries, query)
        .into_iter()
        .map(|index| entries[index].source.repo.clone())
        .collect()
}

#[test]
fn search_is_fuzzy_letters_in_order_and_close_together() {
    let entries = plugins(&[
        (
            "reviewer",
            "Code Review",
            "Review diffs beside the agent",
            &[],
        ),
        ("sidebar", "herdr-sidebar", "A file explorer", &[]),
        (
            "scattered",
            "Rust Event Viewer",
            "Real-time visual indicators in a window",
            &[],
        ),
    ]);
    assert_eq!(
        found_repos(&entries, "reviw"),
        ["reviewer"],
        "a missing letter"
    );
    assert_eq!(found_repos(&entries, "sidbar"), ["sidebar"]);
    assert_eq!(
        found_repos(&entries, "cod rev"),
        ["reviewer"],
        "every word must match"
    );
    assert!(
        found_repos(&entries, "rvw").is_empty(),
        "letters spread over a name or a sentence do not match"
    );
}

#[test]
fn a_word_that_matches_nothing_is_tried_again_with_typos() {
    let entries = plugins(&[
        (
            "reviewer",
            "Code Review",
            "Review diffs beside the agent",
            &[],
        ),
        ("tools", "Tool Box", "Handy tools", &[]),
    ]);
    assert_eq!(
        found_repos(&entries, "reveiw"),
        ["reviewer"],
        "swapped letters"
    );
    assert_eq!(
        found_repos(&entries, "revuew"),
        ["reviewer"],
        "a wrong letter"
    );
    assert_eq!(
        found_repos(&entries, "tool"),
        ["tools"],
        "an exact match leaves out the typo matches"
    );
    assert!(
        found_repos(&entries, "rev")
            .iter()
            .all(|repo| repo == "reviewer"),
        "no typo allowed under 4 letters"
    );
}

#[test]
fn results_rank_the_name_first_then_closeness_then_stars() {
    let entries = plugins(&[
        ("described", "Helper", "Browser helper for herdr", &[]),
        ("tagged", "Helper Two", "Nothing here", &["browser"]),
        ("inside", "Webbrowser", "Nothing here", &[]),
        ("named", "Terminal Browser", "Nothing here", &[]),
        ("gaps", "Browsxer", "Nothing here", &[]),
    ]);
    assert_eq!(
        found_repos(&entries, "browser"),
        ["named", "inside", "gaps", "tagged", "described"],
        "name at a word start, inside a word, with a gap; then a topic; then the description"
    );
    let starred = plugins(&[
        ("popular", "Browser One", "", &[]),
        ("quiet", "Browser Two", "", &[]),
    ]);
    assert_eq!(found_repos(&starred, "browser"), ["popular", "quiet"]);
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

#[test]
fn file_urls_decode_spaces_unicode_and_reserved_filename_characters() {
    use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
    let path = temp_file("my café #100%.json", b"catalog");
    let encoded = path
        .to_str()
        .unwrap()
        .split('/')
        .map(|segment| utf8_percent_encode(segment, NON_ALPHANUMERIC).to_string())
        .collect::<Vec<_>>()
        .join("/");
    for authority in ["", "localhost"] {
        let url = format!("file://{authority}{encoded}");
        assert_eq!(HttpFetcher::new().fetch(&url, 1024).unwrap(), b"catalog");
        assert!(matches!(
            HttpFetcher::new().fetch(&url, 2),
            Err(FetchError::Failed(_))
        ));
    }
    assert!(matches!(
        HttpFetcher::new().fetch("file://remote/tmp/index.json", 1024),
        Err(FetchError::Failed(_))
    ));
    std::fs::remove_file(path).unwrap();
}

#[test]
fn unicode_search_counts_characters_and_ranks_contiguous_matches_first() {
    let entries = plugins(&[
        ("gaps", "c a f é", "", &[]),
        ("accent", "café", "", &[]),
        ("japanese", "日本語ツール", "", &[]),
        ("rocket", "🚀 Launcher", "", &[]),
    ]);
    assert_eq!(found_repos(&entries, "日本"), ["japanese"]);
    assert_eq!(found_repos(&entries, "🚀"), ["rocket"]);
    assert_eq!(
        found_repos(&entries, "café").first().map(String::as_str),
        Some("accent")
    );
    assert_eq!(found_repos(&entries, "é").len(), 2);
}
