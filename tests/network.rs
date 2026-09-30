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

#[test]
#[ignore = "downloads an image uploaded to a GitHub README"]
fn an_image_uploaded_to_a_readme_loads_through_its_github_redirect() {
    use herdr_marketplace::adapters::image_fetch::ImageFetcher;
    use herdr_marketplace::adapters::images::{IMAGE_LIMIT, decode};
    // The first image of ChmaraX/herdr-gitview's README.
    let url = "https://github.com/user-attachments/assets/23ee0639-4a6e-42c5-a003-6e71ab619c43";
    let bytes = ImageFetcher::default()
        .fetch(url, IMAGE_LIMIT)
        .expect("follow GitHub's redirect to its storage");
    let picture = decode(&bytes).expect("decode the uploaded image");
    assert!(picture.width > 0 && picture.height > 0);
}

#[test]
#[ignore = "asks GitHub about a video uploaded to a README"]
fn a_video_uploaded_to_a_readme_is_probed_without_downloading_it() {
    use herdr_marketplace::adapters::image_fetch::{ImageFetcher, Probe};
    // The video of zenbu-labs/terminal-browser's README, 9.8 MB.
    let url = "https://github.com/user-attachments/assets/abe2f43e-fc50-4866-b753-33388967945d";
    let probe = ImageFetcher::default()
        .probe(url)
        .expect("follow GitHub's redirect to its storage");
    assert_eq!(
        probe,
        Probe {
            content_type: "video/mp4".into(),
            size: Some(9_841_526),
        }
    );
}
