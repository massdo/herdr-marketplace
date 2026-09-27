//! Text selected with the mouse in the details pane: a drag selects, the
//! release copies through the terminal, and a copy reads as the README is
//! written. Offline.

mod support;

use std::io::Cursor;
use std::sync::Arc;

use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use herdr_marketplace::adapters::images::{Picture, decode};
use herdr_marketplace::adapters::tui::details::{DetailsApp, DetailsIntent};
use herdr_marketplace::adapters::tui::details_view;
use herdr_marketplace::adapters::tui::graphics::Graphics;
use herdr_marketplace::adapters::tui::selection::clipboard;
use herdr_marketplace::adapters::tui::style::SELECTION_BG;
use herdr_marketplace::application::load_readme::Readme;
use herdr_marketplace::domain::details::DetailsTarget;
use herdr_marketplace::domain::source::PluginSource;
use image::{ImageFormat, Rgba, RgbaImage};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use support::*;

const DOWN: MouseEventKind = MouseEventKind::Down(MouseButton::Left);
const DRAG: MouseEventKind = MouseEventKind::Drag(MouseButton::Left);
const UP: MouseEventKind = MouseEventKind::Up(MouseButton::Left);

fn target() -> DetailsTarget {
    DetailsTarget {
        source: PluginSource {
            owner: "massdo".into(),
            repo: "herdr-marketplace-fixture".into(),
            subdir: String::new(),
        },
        commit: SHA_A.into(),
        id: "herdr-marketplace-fixture".into(),
        name: "herdr-marketplace fixture".into(),
        version: Some("1.0.0".into()),
        in_catalog: true,
        compatible: true,
    }
}

/// A `width` × `height` details pane showing `markdown`.
fn loaded(markdown: &str, width: u16, height: u16) -> DetailsApp {
    let mut app = DetailsApp::new(target());
    app.readme_loaded(
        1,
        Ok(Readme::Found {
            text: markdown.into(),
            fallback: false,
        }),
    );
    app.set_viewport(width as usize, details_view::page_rows(&app, width, height));
    app.intents.clear();
    app
}

fn draw(app: &DetailsApp, width: u16, height: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| details_view::render(frame, app))
        .unwrap();
    terminal.backend().buffer().clone()
}

fn screen(app: &DetailsApp, width: u16, height: u16) -> Vec<String> {
    draw(app, width, height)
        .content()
        .chunks(width as usize)
        .map(|row| row.iter().map(|cell| cell.symbol()).collect())
        .collect()
}

/// Column and row of the first cell where the pane shows `text`.
fn find(screen: &[String], text: &str) -> (u16, u16) {
    screen
        .iter()
        .enumerate()
        .find_map(|(row, line)| {
            let byte = line.find(text)?;
            Some((line[..byte].chars().count() as u16, row as u16))
        })
        .unwrap_or_else(|| panic!("{text:?} is not in {screen:#?}"))
}

fn mouse(kind: MouseEventKind, (column, row): (u16, u16)) -> MouseEvent {
    MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

/// Presses on `from`, moves to `to` and releases there.
fn drag(app: &mut DetailsApp, from: (u16, u16), to: (u16, u16), width: u16, height: u16) {
    app.handle_mouse(mouse(DOWN, from), width, height);
    app.handle_mouse(mouse(DRAG, to), width, height);
    app.handle_mouse(mouse(UP, to), width, height);
}

fn copied(app: &DetailsApp) -> Vec<&str> {
    app.intents
        .iter()
        .filter_map(|intent| match intent {
            DetailsIntent::Copy(text) => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_drag_over_a_command_copies_it_when_the_button_is_released() {
    let command = "herdr plugin install owner/repo";
    let mut app = loaded(&format!("Install it:\n\n```sh\n{command}\n```\n"), 60, 20);
    let (column, row) = find(&screen(&app, 60, 20), command);
    let last = (column + command.len() as u16 - 1, row);
    app.handle_mouse(mouse(DOWN, (column, row)), 60, 20);
    app.handle_mouse(mouse(DRAG, last), 60, 20);
    assert!(
        app.intents.is_empty(),
        "nothing is copied before the release"
    );
    app.handle_mouse(mouse(UP, last), 60, 20);
    assert_eq!(copied(&app), [command]);
    assert!(app.selection.is_none(), "the release ends it, as in Herdr");
}

#[test]
fn a_click_selects_nothing_and_a_press_on_a_link_still_opens_it() {
    let mut app = loaded("Some text and [a link](https://example.com/doc).", 60, 20);
    let lines = screen(&app, 60, 20);
    let text = find(&lines, "Some text");
    drag(&mut app, text, text, 60, 20);
    assert!(app.intents.is_empty(), "{:?}", app.intents);

    let (column, row) = find(&lines, "a link");
    drag(&mut app, (column + 1, row), (column + 20, row), 60, 20);
    assert_eq!(
        app.intents,
        [DetailsIntent::OpenUrl("https://example.com/doc".into())]
    );
}

#[test]
fn code_is_copied_without_its_frame_and_keeps_its_indentation() {
    let code = "fn main() {\n    let x = 1;\n}";
    let mut app = loaded(&format!("```rust\n{code}\n```\n\nAfter."), 40, 20);
    let lines = screen(&app, 40, 20);
    let (_, first) = find(&lines, "fn main()");
    let (_, last) = find(&lines, " }");
    // Edge to edge, from the frame above the code to the frame below it.
    drag(&mut app, (0, first - 1), (39, last + 1), 40, 20);
    assert_eq!(copied(&app), [code]);
}

#[test]
fn a_command_cut_by_the_pane_width_is_copied_on_one_line() {
    // 23 columns leave 21 for code: the first cut falls right after a space.
    let command = "herdr plugin install owner/repository-with-a-long-name";
    let mut app = loaded(&format!("```sh\n{command}\n```\n"), 23, 20);
    let lines = screen(&app, 23, 20);
    let (column, first) = find(&lines, "herdr plugin install");
    assert!(
        lines[first as usize + 1].starts_with(" owner/repository-with "),
        "{lines:#?}"
    );
    let (_, last) = find(&lines, "-a-long-name");
    drag(&mut app, (column, first), (22, last), 23, 20);
    assert_eq!(copied(&app), [command]);
}

#[test]
fn wrapped_text_is_copied_as_written() {
    let paragraph = "The quick brown fox jumps over the lazy dog, then runs far away.";
    let item = "a list item long enough to wrap onto the next line";
    let url = "https://example.com/a/very/long/path/that/cannot/fit/on/one/line";
    let mut app = loaded(&format!("{paragraph}\n\n- {item}\n\nSee {url}\n"), 30, 30);
    let lines = screen(&app, 30, 30);
    let (_, first) = find(&lines, "The quick");
    assert!(
        lines.iter().any(|line| line.starts_with("  wrap")),
        "{lines:#?}"
    );
    // To the last row of the README, over the blank rows under it.
    drag(&mut app, (0, first), (29, 28), 30, 30);
    assert_eq!(
        copied(&app),
        [format!("{paragraph}\n\n• {item}\n\nSee {url}")]
    );
}

#[test]
fn wide_characters_are_copied_once() {
    let text = "Hello 世界, again";
    let mut app = loaded(text, 40, 12);
    let (column, row) = find(&screen(&app, 40, 12), "Hello");
    drag(&mut app, (column, row), (39, row), 40, 12);
    assert_eq!(copied(&app), [text]);
}

fn picture(width: u32, height: u32) -> Arc<Picture> {
    let image = RgbaImage::from_pixel(width, height, Rgba([200, 30, 30, 255]));
    let mut png = Vec::new();
    image
        .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
        .unwrap();
    Arc::new(decode(&png).unwrap())
}

#[test]
fn images_rules_and_frames_are_left_out_of_a_copy() {
    let mut app = DetailsApp::new(target());
    app.set_graphics(Graphics::Blocks);
    app.readme_loaded(
        1,
        Ok(Readme::Found {
            text: "Intro\n\n---\n\n![hero](hero.png)\n\n| Key | Action |\n|-----|--------|\n| i | install |\n\nOutro".into(),
            fallback: false,
        }),
    );
    app.set_viewport(60, details_view::page_rows(&app, 60, 40));
    let url = app
        .intents
        .iter()
        .find_map(|intent| match intent {
            DetailsIntent::LoadImages(urls) => urls.first().cloned(),
            _ => None,
        })
        .expect("the image is asked for");
    app.picture_loaded(&url, Ok(picture(120, 30)));
    app.intents.clear();
    let lines = screen(&app, 60, 40);
    assert!(lines.iter().any(|line| line.contains('▀')), "{lines:#?}");

    // From the title of the pane to the last row of the README.
    drag(&mut app, (0, 0), (59, 38), 60, 40);
    let copy = copied(&app)[0];
    for drawn in ['▀', '─', '━', '┌', '├', '└'] {
        assert!(!copy.contains(drawn), "{drawn:?} in {copy:?}");
    }
    for text in [
        "herdr-marketplace fixture",
        "Intro",
        "│ i   │ install │",
        "Outro",
    ] {
        assert!(copy.contains(text), "{text:?} not in {copy:?}");
    }
}

#[test]
fn the_selection_shows_what_a_copy_takes() {
    let mut app = loaded("```sh\nmake install\n```\n", 40, 20);
    let (column, row) = find(&screen(&app, 40, 20), "make install");
    app.handle_mouse(mouse(DOWN, (0, row - 1)), 40, 20);
    app.handle_mouse(mouse(DRAG, (39, row)), 40, 20);
    let buffer = draw(&app, 40, 20);
    let selected = |column: u16, row: u16| buffer[(column, row)].bg == SELECTION_BG;
    assert!((column..column + 12).all(|column| selected(column, row)));
    assert!(
        !selected(column - 1, row),
        "the padding of code is not text"
    );
    assert!(!selected(column + 12, row), "nor the room after it");
    assert!(
        !(0..40).any(|column| selected(column, row - 1)),
        "nor its frame"
    );
}

#[test]
fn the_click_that_focuses_the_pane_opens_nothing_but_can_select() {
    let mut app = loaded("Read [the docs](https://example.com/doc) first.", 60, 20);
    let (column, row) = find(&screen(&app, 60, 20), "the docs");
    app.focus_click(mouse(DOWN, (column, row)), 60, 20);
    assert!(app.intents.is_empty());
    app.handle_mouse(mouse(DRAG, (column + 7, row)), 60, 20);
    app.handle_mouse(mouse(UP, (column + 7, row)), 60, 20);
    assert_eq!(app.intents, [DetailsIntent::Copy("the docs".into())]);
}

#[test]
fn a_drag_past_the_edges_of_the_pane_stops_at_them() {
    let mut app = loaded("First line.\n\nLast line.", 40, 12);
    let lines = screen(&app, 40, 12);
    let start = find(&lines, "First");
    drag(&mut app, start, (400, 400), 40, 12);
    let copy = copied(&app)[0];
    assert!(copy.starts_with("First line.\n\nLast line.\n"), "{copy:?}");
    assert!(
        copy.ends_with(lines[11].trim_end()),
        "to the footer: {copy:?}"
    );
}

#[test]
fn a_copy_reaches_the_clipboard_through_the_terminal() {
    assert_eq!(clipboard("héllo"), "\x1b]52;c;aMOpbGxv\x07");
}
