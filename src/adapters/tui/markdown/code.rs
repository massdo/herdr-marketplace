//! Code blocks colored with syntect, the highlighter of bat and delta, with
//! Sublime Text grammars and a dark theme whose colors carry their own
//! background, readable on any terminal theme.

use std::sync::OnceLock;

use ratatui::style::Color;
use syntect::easy::HighlightLines;
use syntect::highlighting::{Theme, ThemeSet};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

const THEME: &str = "base16-ocean.dark";

/// Background of code blocks and inline code.
pub fn background() -> Color {
    theme()
        .settings
        .background
        .map_or(Color::Rgb(0x2b, 0x30, 0x3b), |color| {
            Color::Rgb(color.r, color.g, color.b)
        })
}

/// Default text color of code.
pub fn foreground() -> Color {
    theme()
        .settings
        .foreground
        .map_or(Color::Rgb(0xc0, 0xc5, 0xce), |color| {
            Color::Rgb(color.r, color.g, color.b)
        })
}

/// Colored pieces of each line of `code`, highlighted as `language` (the
/// word after the opening fence); a language syntect does not know stays
/// plain.
pub fn highlight(code: &str, language: &str) -> Vec<Vec<(String, Color)>> {
    let syntaxes = syntaxes();
    let syntax = Some(language.trim())
        .filter(|language| !language.is_empty())
        .and_then(|language| syntaxes.find_syntax_by_token(language))
        .unwrap_or_else(|| syntaxes.find_syntax_plain_text());
    let mut highlighter = HighlightLines::new(syntax, theme());
    LinesWithEndings::from(code)
        .map(|line| match highlighter.highlight_line(line, syntaxes) {
            Ok(pieces) => pieces
                .into_iter()
                .map(|(style, text)| {
                    let color = style.foreground;
                    (
                        text.trim_end_matches(['\n', '\r']).to_string(),
                        Color::Rgb(color.r, color.g, color.b),
                    )
                })
                .filter(|(text, _)| !text.is_empty())
                .collect(),
            Err(_) => vec![(
                line.trim_end_matches(['\n', '\r']).to_string(),
                foreground(),
            )],
        })
        .collect()
}

fn syntaxes() -> &'static SyntaxSet {
    static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();
    SYNTAXES.get_or_init(SyntaxSet::load_defaults_newlines)
}

fn theme() -> &'static Theme {
    static THEME_SET: OnceLock<Theme> = OnceLock::new();
    THEME_SET.get_or_init(|| {
        let mut themes = ThemeSet::load_defaults().themes;
        themes.remove(THEME).unwrap_or_default()
    })
}
