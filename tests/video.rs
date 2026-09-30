//! README videos play with a private FFmpeg. Offline.

use std::fs;

use herdr_marketplace::adapters::env::ffmpeg_from;

#[test]
fn ffmpeg_is_the_variable_s_file_else_the_one_next_to_the_executable() {
    let folder = tempfile::tempdir().unwrap();
    let exe = folder.path().join("herdr-marketplace");
    let chosen = folder.path().join("chosen-ffmpeg");
    let missing = folder.path().join("missing-ffmpeg");
    fs::write(&chosen, "").unwrap();
    let chosen_text = chosen.to_str().unwrap();
    let missing_text = missing.to_str().unwrap();

    assert_eq!(
        ffmpeg_from(Some(chosen_text), Some(&exe)),
        Some(chosen.clone())
    );
    assert_eq!(ffmpeg_from(Some(""), Some(&exe)), None);
    assert_eq!(ffmpeg_from(Some(missing_text), Some(&exe)), None);
    assert_eq!(ffmpeg_from(None, Some(&exe)), None);
    assert_eq!(ffmpeg_from(None, None), None);

    let next_to_exe = folder.path().join("ffmpeg");
    fs::write(&next_to_exe, "").unwrap();
    assert_eq!(ffmpeg_from(Some(chosen_text), Some(&exe)), Some(chosen));
    assert_eq!(ffmpeg_from(Some(""), Some(&exe)), Some(next_to_exe.clone()));
    assert_eq!(
        ffmpeg_from(Some(missing_text), Some(&exe)),
        Some(next_to_exe.clone())
    );
    assert_eq!(ffmpeg_from(None, Some(&exe)), Some(next_to_exe));
    assert_eq!(ffmpeg_from(None, None), None);
}
