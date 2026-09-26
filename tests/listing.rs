//! Sidebar rows from a simulated Herdr registry, offline.

mod support;

use herdr_marketplace::application::load_listing::load_listing;
use herdr_marketplace::application::ports::{FetchError, Fetcher};
use herdr_marketplace::domain::compat::Platform;
use herdr_marketplace::domain::listing::build_listing;
use herdr_marketplace::domain::registry::{InstalledSource, parse_registry};
use serde_json::json;
use support::*;

struct StaticFetcher(Vec<u8>);

impl Fetcher for StaticFetcher {
    fn fetch(&self, _url: &str, _limit: u64) -> Result<Vec<u8>, FetchError> {
        Ok(self.0.clone())
    }
}

fn linux_only(owner: &str, name: &str, id: &str) -> serde_json::Value {
    let mut plugin = manifest("herdr-plugin.toml", id);
    plugin["platforms"] = json!(["linux"]);
    repo(owner, name, 7, vec![plugin])
}

#[test]
fn an_installed_linux_only_plugin_stays_listed_on_macos_marked_incompatible() {
    let catalog = catalog(vec![
        linux_only("martin-ro", "herdr-next-agent", "martinro.next-agent"),
        linux_only("someone", "linux-tool", "someone.linux-tool"),
        repo(
            "acme",
            "plugin",
            3,
            vec![manifest("herdr-plugin.toml", "acme.plugin")],
        ),
    ]);
    let installed = parse_registry(&registry(vec![github_plugin(
        "martinro.next-agent",
        "martin-ro",
        "herdr-next-agent",
        None,
        SHA_A,
    )]))
    .unwrap();

    let listing = build_listing(&catalog, &installed, Platform::Macos, HERDR);

    let sources: Vec<String> = listing
        .rows
        .iter()
        .map(|row| row.entry.source.to_string())
        .collect();
    assert_eq!(sources, ["martin-ro/herdr-next-agent", "acme/plugin"]);
    let next_agent = &listing.rows[0];
    assert!(next_agent.installed.is_some());
    assert!(!next_agent.compatible);
    assert!(next_agent.in_catalog);
    assert_eq!(
        listing.hidden_incompatible, 1,
        "someone/linux-tool is hidden"
    );
}

#[test]
fn a_plugin_installed_from_github_but_absent_from_the_catalogue_is_listed_off_catalogue() {
    let catalog = catalog(vec![repo(
        "acme",
        "plugin",
        3,
        vec![manifest("herdr-plugin.toml", "acme.plugin")],
    )]);
    let installed = parse_registry(&registry(vec![github_plugin(
        "herdr-marketplace-fixture",
        "massdo",
        "herdr-marketplace-fixture",
        Some("alt"),
        SHA_B,
    )]))
    .unwrap();

    let listing = build_listing(&catalog, &installed, Platform::Macos, HERDR);

    assert_eq!(listing.rows.len(), 2);
    let off = &listing.rows[1];
    assert!(!off.in_catalog);
    assert!(off.installed.is_some());
    assert!(off.compatible);
    assert_eq!(
        off.entry.source.to_string(),
        "massdo/herdr-marketplace-fixture/alt"
    );
    assert_eq!(off.entry.id, "herdr-marketplace-fixture");
    assert_eq!(off.entry.commit, SHA_B);
    assert_eq!(off.entry.stars, 0);
}

#[test]
fn an_off_catalogue_plugin_declared_for_linux_only_is_marked_incompatible_on_macos() {
    let mut plugin = github_plugin("gone", "acme", "gone", None, SHA_A);
    plugin["platforms"] = json!(["linux"]);
    let installed = parse_registry(&registry(vec![plugin])).unwrap();

    let listing = build_listing(&catalog(vec![]), &installed, Platform::Macos, HERDR);

    assert_eq!(listing.rows.len(), 1);
    assert!(!listing.rows[0].compatible);
    assert!(!listing.rows[0].in_catalog);
}

#[test]
fn a_locally_linked_plugin_is_not_listed() {
    let catalog = catalog(vec![repo(
        "massdo",
        "herdr-marketplace",
        1,
        vec![manifest("herdr-plugin.toml", "herdr-marketplace")],
    )]);
    let installed = parse_registry(&registry(vec![local_plugin("herdr-marketplace")])).unwrap();
    assert_eq!(installed[0].source, InstalledSource::Local);

    let listing = build_listing(&catalog, &installed, Platform::Macos, HERDR);

    assert_eq!(listing.rows.len(), 1);
    assert!(listing.rows[0].installed.is_none());
    assert!(listing.rows[0].in_catalog);
}

#[test]
fn owner_and_repo_ignore_case_but_the_subdir_does_not() {
    let catalog = catalog(vec![repo(
        "massdo",
        "herdr-marketplace-fixture",
        1,
        vec![
            manifest("herdr-plugin.toml", "herdr-marketplace-fixture"),
            manifest("alt/herdr-plugin.toml", "herdr-marketplace-fixture"),
        ],
    )]);
    let installed = parse_registry(&registry(vec![
        github_plugin(
            "herdr-marketplace-fixture",
            "MASSDO",
            "Herdr-Marketplace-Fixture",
            None,
            SHA_A,
        ),
        github_plugin(
            "other",
            "massdo",
            "herdr-marketplace-fixture",
            Some("ALT"),
            SHA_A,
        ),
    ]))
    .unwrap();

    let listing = build_listing(&catalog, &installed, Platform::Macos, HERDR);

    let summary: Vec<(String, bool, bool)> = listing
        .rows
        .iter()
        .map(|row| {
            (
                row.entry.source.to_string(),
                row.installed.is_some(),
                row.in_catalog,
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            ("massdo/herdr-marketplace-fixture".to_string(), true, true),
            (
                "massdo/herdr-marketplace-fixture/alt".to_string(),
                false,
                true
            ),
            (
                "massdo/herdr-marketplace-fixture/ALT".to_string(),
                true,
                false
            ),
        ]
    );
}

#[test]
fn an_unreadable_registry_marks_nothing_installed() {
    let herdr = FakeHerdr {
        version: "herdr 0.9.1".into(),
        registry: Err("herdr plugin list failed".into()),
    };
    let fetcher = StaticFetcher(index(vec![repo(
        "acme",
        "plugin",
        1,
        vec![manifest("herdr-plugin.toml", "acme.plugin")],
    )]));

    let loaded = load_listing(&fetcher, &herdr, "file:///index.json", Platform::Macos).unwrap();

    assert!(loaded.registry_error.is_some());
    assert_eq!(loaded.listing.rows.len(), 1);
    assert!(loaded.listing.rows[0].installed.is_none());
}

#[test]
fn the_registry_is_read_from_herdr_plugin_list_json() {
    let json = registry(vec![
        github_plugin(
            "herdr-marketplace-fixture",
            "massdo",
            "herdr-marketplace-fixture",
            Some("alt"),
            SHA_A,
        ),
        local_plugin("herdr-marketplace"),
    ]);
    let installed = parse_registry(&json).unwrap();
    assert_eq!(installed.len(), 2);
    let fixture = &installed[0];
    assert_eq!(fixture.plugin_id, "herdr-marketplace-fixture");
    assert_eq!(
        fixture.github_source().unwrap().to_string(),
        "massdo/herdr-marketplace-fixture/alt"
    );
    assert_eq!(fixture.resolved_commit(), Some(SHA_A));
    assert!(parse_registry("not json").is_err());
    assert!(parse_registry(r#"{"id":"cli:plugin","error":{"code":"x","message":"y"}}"#).is_err());
}
