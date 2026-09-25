/// Third-party text made safe for one terminal line: tabs and line breaks
/// become spaces, every other control character (C0, DEL, C1, so ESC and CSI
/// included) is dropped.
pub fn clean(value: &str) -> String {
    value
        .chars()
        .filter_map(|ch| match ch {
            '\t' | '\n' | '\r' => Some(' '),
            ch if ch.is_control() => None,
            ch => Some(ch),
        })
        .collect()
}
