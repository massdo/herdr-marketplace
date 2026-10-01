//! Selection updates and graceful closure use a private local mailbox.
use std::path::Path;

use herdr_marketplace::adapters::details_control::{DetailsControl, DetailsUpdate};
use herdr_marketplace::adapters::herdr_socket::HerdrSocket;
use herdr_marketplace::application::ports::HerdrPort;
use herdr_marketplace::domain::details::DetailsTarget;
use herdr_marketplace::domain::ids::PaneId;
use herdr_marketplace::domain::source::PluginSource;

fn update(repo: &str) -> DetailsUpdate {
    DetailsUpdate {
        target: DetailsTarget {
            source: PluginSource {
                owner: "test".into(),
                repo: repo.into(),
                subdir: String::new(),
            },
            commit: "a".repeat(40),
            id: repo.into(),
            name: repo.into(),
            version: None,
            in_catalog: true,
            compatible: true,
        },
        video_cache: Some(Path::new("/tmp/session/videos").into()),
    }
}

#[test]
fn rapid_selections_keep_the_latest_complete_record_and_release_it_once_read() {
    let state = tempfile::tempdir().unwrap();
    let socket = Path::new("/tmp/session.sock");
    let pane = PaneId("w1:p2".into());
    let control = DetailsControl::open(state.path(), socket, &pane).unwrap();
    DetailsControl::send(state.path(), socket, &pane, update("A")).unwrap();
    DetailsControl::send(state.path(), socket, &pane, update("B")).unwrap();
    assert_eq!(
        DetailsControl::take(state.path(), socket, &pane).unwrap(),
        Some(update("B"))
    );
    assert!(
        DetailsControl::take(state.path(), socket, &pane)
            .unwrap()
            .is_none()
    );
    drop(control);
    assert!(DetailsControl::send(state.path(), socket, &pane, update("A")).is_err());
    assert_eq!(
        std::fs::read_dir(state.path().join("details"))
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn pane_ids_from_other_herdr_sessions_do_not_share_updates() {
    let state = tempfile::tempdir().unwrap();
    let first = Path::new("/tmp/first.sock");
    let second = Path::new("/tmp/second.sock");
    let pane = PaneId("w1:p2".into());
    let _a = DetailsControl::open(state.path(), first, &pane).unwrap();
    let _b = DetailsControl::open(state.path(), second, &pane).unwrap();
    DetailsControl::send(state.path(), first, &pane, update("A")).unwrap();
    assert!(
        DetailsControl::take(state.path(), second, &pane)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        DetailsControl::take(state.path(), first, &pane).unwrap(),
        Some(update("A"))
    );
}

#[test]
fn graceful_close_prevents_new_selections_and_does_not_call_herdr() {
    let state = tempfile::tempdir().unwrap();
    let socket = state.path().join("absent-herdr.sock");
    let pane = PaneId("w1:p2".into());
    let _control = DetailsControl::open(state.path(), &socket, &pane).unwrap();
    let herdr = HerdrSocket::new(socket.clone()).with_details_state(state.path().into());
    herdr.close_plugin_pane(&pane).unwrap();
    assert!(DetailsControl::closing(state.path(), &socket, &pane));
    assert!(DetailsControl::send(state.path(), &socket, &pane, update("B")).is_err());
}
