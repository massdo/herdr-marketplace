//! Network tests, run on demand: `cargo test -- --ignored`.

use herdr_marketplace::adapters::fetch::HttpFetcher;
use herdr_marketplace::application::load_catalog::INDEX_LIMIT;
use herdr_marketplace::application::ports::Fetcher;
use herdr_marketplace::domain::DEFAULT_INDEX_URL;
use herdr_marketplace::domain::index::parse_index;

#[test]
#[ignore = "downloads the public index"]
fn the_real_index_loads() {
    let body = HttpFetcher::new()
        .fetch(DEFAULT_INDEX_URL, INDEX_LIMIT)
        .expect("download the public index");
    let catalog = parse_index(&body).expect("parse the public index");
    println!(
        "manifests read {}, kept {}, rejected {}, pluginCount {:?}",
        catalog.read,
        catalog.entries.len(),
        catalog.rejected,
        catalog.plugin_count
    );
    assert_eq!(Some(catalog.read as u64), catalog.plugin_count);
    assert_eq!(catalog.entries.len() + catalog.rejected, catalog.read);
    assert!(!catalog.entries.is_empty());
}
