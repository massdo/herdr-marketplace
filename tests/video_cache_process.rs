//! The cache owner can be forcibly killed by Herdr; its helper must still
//! remove every visited video's file, even with no details pane left.
use std::fs;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn killing_marketplace_still_cleans_the_cache_without_any_details_client() {
    let state = tempfile::tempdir().unwrap();
    let mut owner = Process(Command::new("sleep").arg("30").spawn().unwrap());
    let folder = state.path().join("videos").join(owner.0.id().to_string());
    let mut helper = Process(
        Command::new(env!("CARGO_BIN_EXE_herdr-marketplace"))
            .arg("--video-cache")
            .arg(owner.0.id().to_string())
            .env("HERDR_PLUGIN_STATE_DIR", state.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while !folder.join("cache.sock").exists() {
        assert!(Instant::now() < deadline, "cache helper did not start");
        assert!(
            helper.0.try_wait().unwrap().is_none(),
            "cache helper exited"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    // Files of two plugins remain after their details clients have gone.
    fs::write(folder.join("A.mp4"), "video A").unwrap();
    fs::write(folder.join("B.mp4"), "video B").unwrap();
    owner.0.kill().unwrap();
    owner.0.wait().unwrap();
    while folder.exists() || helper.0.try_wait().unwrap().is_none() {
        assert!(
            Instant::now() < deadline,
            "killed Marketplace left its video cache"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
