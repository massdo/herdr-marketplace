/// Third-party text made safe for one terminal line: tabs and line breaks
/// become spaces, every other control character (C0, DEL, C1, so ESC and CSI
/// included) and Unicode format characters are dropped.
pub fn clean(value: &str) -> String {
    value
        .chars()
        .filter_map(|ch| match ch {
            '\t' | '\n' | '\r' => Some(' '),
            ch if ch.is_control() || is_format(ch) => None,
            ch => Some(ch),
        })
        .collect()
}

pub fn is_format(ch: char) -> bool {
    unicode_general_category::get_general_category(ch)
        == unicode_general_category::GeneralCategory::Format
}

/// The installation preview must reveal invisible formatting in commands.
pub fn preview_text(value: &str) -> String {
    clean(
        &value
            .chars()
            .map(|ch| {
                if is_format(ch) || ch.is_control() {
                    format!("⟨U+{:04X}⟩", ch as u32)
                } else {
                    ch.to_string()
                }
            })
            .collect::<String>(),
    )
}
