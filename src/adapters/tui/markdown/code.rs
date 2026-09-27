//! Code blocks colored with syntect, the highlighter of bat and delta, with
//! Sublime Text grammars and a dark theme whose colors carry their own
//! background, readable on any terminal theme.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

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
type Highlighted = Vec<Vec<(String, Color)>>;

#[derive(Default)]
struct Cache {
    blocks: HashMap<(String, String), Arc<Highlighted>>,
    bytes: usize,
}

thread_local! {
    // Per rendering thread, bounded across README reloads and pane lifetime.
    static CACHE: RefCell<Cache> = RefCell::new(Cache::default());
}

pub fn highlight(code: &str, language: &str) -> Arc<Highlighted> {
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        let key = (code.to_string(), language.to_string());
        if let Some(block) = cache.blocks.get(&key) {
            return block.clone();
        }
        let block = Arc::new(highlight_uncached(code, language));
        let size = key.0.len()
            + key.1.len()
            + block
                .iter()
                .map(|line| {
                    line.capacity() * std::mem::size_of::<(String, Color)>()
                        + line.iter().map(|(text, _)| text.capacity()).sum::<usize>()
                })
                .sum::<usize>();
        const BUDGET: usize = 8 * 1024 * 1024;
        if cache.bytes + size > BUDGET {
            cache.blocks.clear();
            cache.bytes = 0;
        }
        if size <= BUDGET {
            cache.bytes += size;
            cache.blocks.insert(key, block.clone());
        }
        block
    })
}

fn highlight_uncached(code: &str, language: &str) -> Highlighted {
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_relayout_reuses_highlighted_code_and_changes_invalidate_it() {
        let first = highlight("let n = 1;", "rust");
        assert!(Arc::ptr_eq(&first, &highlight("let n = 1;", "rust")));
        assert!(!Arc::ptr_eq(&first, &highlight("let n = 2;", "rust")));
        assert!(!Arc::ptr_eq(&first, &highlight("let n = 1;", "text")));
    }
}
