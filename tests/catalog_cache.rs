//! The saved catalog and its ETag check, offline.

mod support;

use std::cell::RefCell;

use herdr_marketplace::adapters::catalog_cache::FileCatalogCache;
use herdr_marketplace::application::load_catalog::{LoadError, LoadedCatalog, load_catalog};
use herdr_marketplace::application::ports::{
    CachedIndex, CatalogCache, CatalogFetcher, FetchError, Fetched,
};
use support::*;

const URL: &str = "https://example.invalid/index.json";

/// Gives the same answer to every request and records the ETag sent.
struct FakeIndex {
    answer: Result<Fetched, FetchError>,
    sent: RefCell<Vec<Option<String>>>,
}

impl FakeIndex {
    fn new(answer: Result<Fetched, FetchError>) -> Self {
        Self {
            answer,
            sent: RefCell::new(Vec::new()),
        }
    }
}

impl CatalogFetcher for FakeIndex {
    fn fetch_index(
        &self,
        _url: &str,
        etag: Option<&str>,
        _limit: u64,
    ) -> Result<Fetched, FetchError> {
        self.sent.borrow_mut().push(etag.map(str::to_string));
        self.answer.clone()
    }
}

struct FakeCache(RefCell<Option<CachedIndex>>);

impl CatalogCache for FakeCache {
    fn read(&self) -> Option<CachedIndex> {
        self.0.borrow().clone()
    }

    fn replace(&self, entry: &CachedIndex) {
        *self.0.borrow_mut() = Some(entry.clone());
    }

    fn remove(&self) {
        *self.0.borrow_mut() = None;
    }
}

fn one_plugin(id: &str) -> Vec<u8> {
    index(vec![repo(
        "acme",
        id,
        1,
        vec![manifest("herdr-plugin.toml", id)],
    )])
}

fn saved(url: &str, body: Vec<u8>) -> FakeCache {
    FakeCache(RefCell::new(Some(CachedIndex {
        url: url.into(),
        etag: "\"old\"".into(),
        body,
    })))
}

fn load(fetcher: &FakeIndex, cache: &FakeCache) -> Result<LoadedCatalog, LoadError> {
    load_catalog(fetcher, cache, &FakeHerdr::with_registry(vec![]), URL)
}

fn ids(loaded: &LoadedCatalog) -> Vec<&str> {
    loaded
        .catalog
        .entries
        .iter()
        .map(|entry| entry.id.as_str())
        .collect()
}

#[test]
fn an_unchanged_index_shows_the_saved_copy() {
    let fetcher = FakeIndex::new(Ok(Fetched::Unchanged));
    let cache = saved(URL, one_plugin("old"));

    let loaded = load(&fetcher, &cache).unwrap();

    assert_eq!(*fetcher.sent.borrow(), [Some("\"old\"".to_string())]);
    assert_eq!(ids(&loaded), ["old"]);
    assert!(!loaded.not_refreshed);
}

#[test]
fn a_new_index_is_shown_and_replaces_the_saved_copy() {
    let fetcher = FakeIndex::new(Ok(Fetched::Body {
        body: one_plugin("new"),
        etag: Some("\"new\"".into()),
    }));
    let cache = saved(URL, one_plugin("old"));

    let loaded = load(&fetcher, &cache).unwrap();

    assert_eq!(ids(&loaded), ["new"]);
    assert!(!loaded.not_refreshed);
    assert_eq!(
        cache.read(),
        Some(CachedIndex {
            url: URL.into(),
            etag: "\"new\"".into(),
            body: one_plugin("new"),
        })
    );
}

#[test]
fn an_invalid_copy_or_one_of_another_url_sends_no_etag() {
    for cache in [
        saved(URL, b"not json".to_vec()),
        saved("https://other.invalid/index.json", one_plugin("old")),
    ] {
        let fetcher = FakeIndex::new(Ok(Fetched::Body {
            body: one_plugin("new"),
            etag: None,
        }));

        load(&fetcher, &cache).unwrap();

        assert_eq!(*fetcher.sent.borrow(), [None]);
    }
}

#[test]
fn an_invalid_new_index_keeps_and_shows_the_saved_copy() {
    let fetcher = FakeIndex::new(Ok(Fetched::Body {
        body: b"not json".to_vec(),
        etag: Some("\"bad\"".into()),
    }));
    let cache = saved(URL, one_plugin("old"));
    let before = cache.read();

    let loaded = load(&fetcher, &cache).unwrap();

    assert_eq!(ids(&loaded), ["old"]);
    assert!(loaded.not_refreshed);
    assert_eq!(cache.read(), before);
}

#[test]
fn without_network_the_saved_copy_is_shown() {
    let fetcher = FakeIndex::new(Err(FetchError::Failed("offline".into())));
    let cache = saved(URL, one_plugin("old"));

    let loaded = load(&fetcher, &cache).unwrap();

    assert_eq!(ids(&loaded), ["old"]);
    assert!(loaded.not_refreshed);
}

#[test]
fn the_file_cache_reads_back_what_it_wrote() {
    let dir = tempfile::tempdir().unwrap();
    let cache = FileCatalogCache::new(Some(dir.path().join("catalog")));
    let entry = CachedIndex {
        url: URL.into(),
        etag: "W/\"abc\"".into(),
        body: one_plugin("old"),
    };

    cache.replace(&entry);
    assert_eq!(cache.read(), Some(entry));

    std::fs::write(dir.path().join("catalog/index"), "unreadable").unwrap();
    assert_eq!(cache.read(), None);
}
