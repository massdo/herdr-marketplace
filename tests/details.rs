//! README details pane: loading, Markdown rendering, keyboard and pane handling,
//! offline.

mod support;

use std::cell::RefCell;
use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use herdr_marketplace::adapters::tui::details::{
    DetailsApp, DetailsIntent, InstalledView, ReadmeState,
};
use herdr_marketplace::adapters::tui::{details_view, markdown};
use herdr_marketplace::application::load_readme::{Readme, load_readme};
use herdr_marketplace::application::open_details::{Reveal, Shown, close_details, show_details};
use herdr_marketplace::application::ports::{FetchError, Fetcher};
use herdr_marketplace::domain::compat::Platform;
use herdr_marketplace::domain::details::DetailsTarget;
use herdr_marketplace::domain::ids::PaneId;
use herdr_marketplace::domain::listing::build_listing;
use herdr_marketplace::domain::registry::parse_registry;
use herdr_marketplace::domain::source::PluginSource;
use herdr_marketplace::domain::{DETAILS_ENV, DETAILS_TOKEN_KEY, SIDEBAR_TOKEN_KEY};
use ratatui::backend::{CrosstermBackend, TestBackend};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier};
use ratatui::text::Line;
use ratatui::{Terminal, TerminalOptions, Viewport};
use serde_json::json;
use support::*;

const RAW: &str = "https://raw.githubusercontent.com";

/// Terminal output captured byte for byte.
#[derive(Clone, Default)]
struct Recorder(std::rc::Rc<RefCell<Vec<u8>>>);

impl std::io::Write for Recorder {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Answers by URL; any other URL is a 404. Every request is recorded.
struct FakeRaw {
    answers: HashMap<String, Result<Vec<u8>, FetchError>>,
    asked: RefCell<Vec<String>>,
}

impl FakeRaw {
    fn new(answers: Vec<(String, Result<&str, FetchError>)>) -> Self {
        Self {
            answers: answers
                .into_iter()
                .map(|(url, answer)| (url, answer.map(|body| body.as_bytes().to_vec())))
                .collect(),
            asked: RefCell::new(Vec::new()),
        }
    }
}

impl Fetcher for FakeRaw {
    fn fetch(&self, url: &str, _limit: u64) -> Result<Vec<u8>, FetchError> {
        self.asked.borrow_mut().push(url.to_string());
        self.answers
            .get(url)
            .cloned()
            .unwrap_or(Err(FetchError::NotFound))
    }
}

fn source(subdir: &str) -> PluginSource {
    PluginSource {
        owner: "massdo".into(),
        repo: "herdr-marketplace-fixture".into(),
        subdir: subdir.into(),
    }
}

fn target(subdir: &str) -> DetailsTarget {
    DetailsTarget {
        source: source(subdir),
        commit: SHA_A.into(),
        id: "herdr-marketplace-fixture".into(),
        name: "herdr-marketplace fixture".into(),
        version: Some("1.0.0".into()),
        in_catalog: true,
        compatible: true,
    }
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn texts(lines: &[Line]) -> Vec<String> {
    lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect()
        })
        .collect()
}

fn span_style(lines: &[Line], text: &str) -> ratatui::style::Style {
    lines
        .iter()
        .flat_map(|line| line.spans.iter())
        .find(|span| span.content == text)
        .unwrap_or_else(|| panic!("no span {text:?} in {:?}", texts(lines)))
        .style
}

#[test]
fn the_readme_of_a_subfolder_plugin_comes_from_its_folder_at_the_commit() {
    let own = format!("{RAW}/massdo/herdr-marketplace-fixture/{SHA_A}/alt/README.md");
    let raw = FakeRaw::new(vec![(own.clone(), Ok("# Alt"))]);
    let readme = load_readme(&raw, &source("alt"), SHA_A).unwrap();
    assert_eq!(
        readme,
        Readme::Found {
            text: "# Alt".into(),
            fallback: false
        }
    );
    assert_eq!(*raw.asked.borrow(), [own]);
}

#[test]
fn a_subfolder_without_readme_falls_back_to_the_root_one() {
    let root = format!("{RAW}/massdo/herdr-marketplace-fixture/{SHA_A}/README.md");
    let raw = FakeRaw::new(vec![(root.clone(), Ok("# Root"))]);
    let readme = load_readme(&raw, &source("alt"), SHA_A).unwrap();
    assert_eq!(
        readme,
        Readme::Found {
            text: "# Root".into(),
            fallback: true
        }
    );
    assert_eq!(
        *raw.asked.borrow(),
        herdr_marketplace::application::load_readme::README_NAMES
            .iter()
            .map(|file| format!("{RAW}/massdo/herdr-marketplace-fixture/{SHA_A}/alt/{file}"))
            .chain([root])
            .collect::<Vec<_>>()
    );
}

#[test]
fn a_missing_readme_is_not_found_and_differs_from_a_network_error() {
    let raw = FakeRaw::new(vec![]);
    assert_eq!(
        load_readme(&raw, &source("alt"), SHA_A),
        Ok(Readme::NotFound)
    );
    let raw = FakeRaw::new(vec![]);
    assert_eq!(load_readme(&raw, &source(""), SHA_A), Ok(Readme::NotFound));
    assert_eq!(
        *raw.asked.borrow(),
        herdr_marketplace::application::load_readme::README_NAMES
            .iter()
            .map(|file| format!("{RAW}/massdo/herdr-marketplace-fixture/{SHA_A}/{file}"))
            .collect::<Vec<_>>(),
        "a root plugin tries the usual names without fallback"
    );

    let own = format!("{RAW}/massdo/herdr-marketplace-fixture/{SHA_A}/alt/README.md");
    let raw = FakeRaw::new(vec![(own, Err(FetchError::Failed("timeout".into())))]);
    assert_eq!(
        load_readme(&raw, &source("alt"), SHA_A),
        Err("timeout".into())
    );
}

#[test]
fn a_network_error_can_be_retried_and_older_answers_are_dropped() {
    let mut app = DetailsApp::new(target(""));
    assert_eq!(
        app.intents,
        [DetailsIntent::LoadReadme(1), DetailsIntent::ReadRegistry(1)]
    );
    app.intents.clear();
    app.readme_loaded(1, Err("connection refused".into()));
    assert!(matches!(app.readme, ReadmeState::NetworkError(_)));

    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.readme, ReadmeState::Loading);
    assert_eq!(app.intents, [DetailsIntent::LoadReadme(2)]);

    app.readme_loaded(
        1,
        Ok(Readme::Found {
            text: "old answer".into(),
            fallback: false,
        }),
    );
    assert_eq!(app.readme, ReadmeState::Loading, "request 1 is stale");
    app.readme_loaded(
        2,
        Ok(Readme::Found {
            text: "fresh answer".into(),
            fallback: false,
        }),
    );
    assert_eq!(texts(&app.lines), ["fresh answer"]);
}

#[test]
fn the_whole_readme_is_reachable_with_the_keyboard() {
    let readme: String = (1..=60).map(|index| format!("Line {index}.\n\n")).collect();
    let mut app = DetailsApp::new(target(""));
    app.set_viewport(40, 10);
    app.readme_loaded(
        1,
        Ok(Readme::Found {
            text: readme,
            fallback: false,
        }),
    );
    let last = app.lines.len();
    app.handle_key(key(KeyCode::End));
    assert_eq!(app.scroll, last - 10);
    assert_eq!(texts(&app.lines[app.scroll..]).last().unwrap(), "Line 60.");
    app.handle_key(key(KeyCode::PageUp));
    assert_eq!(app.scroll, last - 20);
    app.handle_key(key(KeyCode::Up));
    assert_eq!(app.scroll, last - 21);
    app.handle_key(key(KeyCode::Home));
    assert_eq!(app.scroll, 0);
    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::PageDown));
    assert_eq!(app.scroll, 11);
    for _ in 0..200 {
        app.handle_key(key(KeyCode::Down));
    }
    assert_eq!(app.scroll, last - 10);
    assert!(
        app.handle_key(key(KeyCode::Esc)),
        "escape closes the details pane"
    );
}

#[test]
fn the_header_shows_identity_and_a_short_sha_with_the_full_one_on_demand() {
    let mut app = DetailsApp::new(target("alt"));
    app.readme_loaded(
        1,
        Ok(Readme::Found {
            text: "Root README".into(),
            fallback: true,
        }),
    );
    let screen = |app: &DetailsApp| {
        let mut terminal = Terminal::new(TestBackend::new(100, 12)).unwrap();
        terminal
            .draw(|frame| details_view::render(frame, app))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        buffer
            .content()
            .chunks(100)
            .map(|line| line.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect::<Vec<_>>()
    };
    let lines = screen(&app);
    assert!(
        lines[0].starts_with("herdr-marketplace fixture  1.0.0"),
        "{lines:#?}"
    );
    assert!(
        lines[1].starts_with("massdo/herdr-marketplace-fixture/alt"),
        "{lines:#?}"
    );
    assert!(lines[2].starts_with("commit c8268d4 "), "{lines:#?}");
    assert!(lines[3].starts_with(" Open on GitHub (o) "), "{lines:#?}");
    assert!(
        lines[5].starts_with("No README in alt/: showing the repository root README"),
        "{lines:#?}"
    );
    assert!(lines[7].starts_with("Root README"), "{lines:#?}");

    app.handle_key(key(KeyCode::Char('s')));
    assert!(
        screen(&app)[2].starts_with(&format!("commit {SHA_A}")),
        "full SHA"
    );
}

const EVERY_ELEMENT: &str = r#"# Title

Intro with **bold**, *italic*, ~~gone~~ and `code`, a [link](https://example.com/doc) and <https://example.com/auto>.

- first
- second
  - nested

1. one
2. two

- [x] done
- [ ] todo

> quoted text

---

```rust
fn main() { let x = 1; }
```

| Left | Right |
|:-----|------:|
| a | 1 |
| long cell | 22 |

![diagram](docs/diagram.png)

<p align="center"><img src="logo.png" alt="logo"></p>

<div>kept <b>text</b> &amp; more</div>

Inline <img src="x.png" alt="small icon"> and <span>span text</span>.
"#;

#[test]
fn every_listed_markdown_element_is_rendered() {
    let lines = markdown::render(EVERY_ELEMENT, 60);
    let text = texts(&lines);
    let has = |expected: &str| {
        assert!(
            text.iter().any(|line| line == expected),
            "{expected:?} not in {text:#?}"
        )
    };

    has("Title");
    has(&"━".repeat(60));
    let title = span_style(&lines, "Title");
    assert!(title.add_modifier.contains(Modifier::BOLD));
    assert_eq!(
        title.fg,
        Some(herdr_marketplace::adapters::tui::style::ACCENT)
    );
    assert!(
        span_style(&lines, "bold")
            .add_modifier
            .contains(Modifier::BOLD)
    );
    assert!(
        span_style(&lines, "italic")
            .add_modifier
            .contains(Modifier::ITALIC)
    );
    assert!(
        span_style(&lines, "gone")
            .add_modifier
            .contains(Modifier::CROSSED_OUT)
    );
    let code = span_style(&lines, "code");
    assert!(code.bg.is_some(), "inline code sits on its own background");
    let joined = text.join(" ");
    assert!(
        joined.contains("a link and https://example.com/auto."),
        "a link shows its text, not its address: {text:#?}"
    );
    assert!(
        span_style(&lines, "link")
            .add_modifier
            .contains(Modifier::UNDERLINED)
    );
    has("• first");
    has("• second");
    has("  ◦ nested");
    has("1. one");
    has("2. two");
    has("☑ done");
    has("☐ todo");
    has("▎ quoted text");
    has(&"─".repeat(60));
    let code_line = lines
        .iter()
        .find(|line| line.spans.iter().any(|span| span.content.contains("main")))
        .expect("code block");
    assert_eq!(code_line.width(), 60, "a code block fills the line");
    let colors: std::collections::HashSet<Option<Color>> = code_line
        .spans
        .iter()
        .filter(|span| !span.content.trim().is_empty())
        .map(|span| span.style.fg)
        .collect();
    assert!(colors.len() > 1, "Rust is highlighted: {code_line:?}");
    assert!(
        code_line.spans.iter().all(|span| span.style.bg.is_some()),
        "{code_line:?}"
    );
    has("┌───────────┬───────┐");
    has("│ Left      │ Right │");
    has("├───────────┼───────┤");
    has("│ a         │     1 │");
    has("│ long cell │    22 │");
    has("└───────────┴───────┘");
    has("[image: diagram]");
    assert!(
        text.iter()
            .any(|line| line.trim() == "[image: logo]" && line.starts_with("   ")),
        "an image in a centered paragraph is centered: {text:#?}"
    );
    has("kept text & more");
    has("Inline \u{a0}small\u{a0}icon\u{a0} and span text.");
}

#[test]
fn paragraphs_wrap_to_the_pane_width() {
    let paragraph = "word ".repeat(40);
    let lines = markdown::render(&paragraph, 30);
    let text = texts(&lines);
    assert!(text.len() > 1, "{text:#?}");
    assert!(
        text.iter()
            .all(|line| unicode_width::UnicodeWidthStr::width(line.as_str()) <= 30),
        "{text:#?}"
    );
    let long_url = format!("see https://example.com/{}", "x".repeat(80));
    let text = texts(&markdown::render(&long_url, 30));
    assert!(
        text.iter()
            .all(|line| unicode_width::UnicodeWidthStr::width(line.as_str()) <= 30),
        "{text:#?}"
    );
}

const TRAPPED: &str = "# Trap\u{1b}[2J\n\nred \u{1b}[31mtext\u{1b}[0m, title \u{1b}]0;PWNED\u{7}, clipboard \u{1b}]52;c;SGVsbG8=\u{7}, csi \u{9b}6n, dcs \u{1b}P+q\u{1b}\\, bell\u{7}, tab\there\r\n\n```\ncode \u{1b}[1mbold\u{1b}[0m\n```\n\n<p>\u{1b}]8;;https://evil.example\u{7}link\u{1b}]8;;\u{7}</p>\n";

#[test]
fn control_sequences_of_a_readme_never_reach_the_terminal() {
    let mut app = DetailsApp::new(target(""));
    app.readme_loaded(
        1,
        Ok(Readme::Found {
            text: TRAPPED.into(),
            fallback: false,
        }),
    );

    let mut cells = Terminal::new(TestBackend::new(80, 24)).unwrap();
    cells
        .draw(|frame| details_view::render(frame, &app))
        .unwrap();
    let buffer = cells.backend().buffer().clone();
    let screen: String = buffer.content().iter().map(|cell| cell.symbol()).collect();
    assert!(!screen.chars().any(char::is_control), "{screen:?}");
    assert!(screen.contains("red [31mtext[0m"), "{screen:?}");

    let recorder = Recorder::default();
    let mut bytes = Terminal::with_options(
        CrosstermBackend::new(recorder.clone()),
        TerminalOptions {
            viewport: Viewport::Fixed(Rect::new(0, 0, 80, 24)),
        },
    )
    .unwrap();
    bytes
        .draw(|frame| details_view::render(frame, &app))
        .unwrap();
    let written = String::from_utf8_lossy(&recorder.0.borrow()).into_owned();
    assert!(written.contains("PWNED"), "the text itself stays visible");
    for forbidden in [
        "\u{7}",
        "\u{1b}]",
        "\u{1b}P",
        "\u{1b}\\",
        "\u{9b}",
        "\u{1b}[2J",
        "\u{1b}[31m",
        "\t",
        "\r",
    ] {
        assert!(
            !written.contains(forbidden),
            "{forbidden:?} reached the terminal"
        );
    }
}

#[test]
fn a_preview_opens_right_of_the_sidebar_and_leaves_it_the_focus() {
    let herdr = FakePanes::new(vec![
        pane("w1:side", "w1:t1", Some(SIDEBAR_TOKEN_KEY)),
        pane("w1:work", "w1:t1", None),
    ]);

    let shown = show_details(
        &herdr,
        &PaneId("w1:side".into()),
        &target(""),
        Reveal::Preview,
        None,
    )
    .unwrap()
    .unwrap();

    assert_eq!(
        *herdr.calls.borrow(),
        [
            "open details next to w1:work",
            "swap w1:new w1:work",
            "focus w1:side",
            "identity w1:new herdr_marketplace_details",
            "resize w1:new left",
            "resize w1:new right",
        ],
        "split, then swapped onto the working pane's left half"
    );
    assert!(!herdr.opened.borrow()[0].focus);
    assert_eq!(shown.pane_id, PaneId("w1:new".into()));
    assert_eq!(shown.target, target(""));
}

#[test]
fn enter_opens_right_of_the_sidebar_with_the_focus() {
    let herdr = FakePanes::new(vec![
        pane("w1:side", "w1:t1", Some(SIDEBAR_TOKEN_KEY)),
        pane("w1:work", "w1:t1", None),
    ]);

    show_details(
        &herdr,
        &PaneId("w1:side".into()),
        &target(""),
        Reveal::Focus,
        None,
    )
    .unwrap();

    assert_eq!(
        herdr.calls.borrow()[..2],
        ["open details next to w1:work", "swap w1:new w1:work"],
        "the swap focuses the pane it moves"
    );
    assert!(!herdr.calls.borrow().contains(&"focus w1:side".to_string()));
    assert!(herdr.opened.borrow()[0].focus);
}

#[test]
fn opening_b_while_a_is_shown_replaces_a_in_place() {
    let herdr = FakePanes::new(vec![
        pane("w1:side", "w1:t1", Some(SIDEBAR_TOKEN_KEY)),
        pane("w1:work", "w1:t1", None),
        pane("w1:details-a", "w1:t1", Some(DETAILS_TOKEN_KEY)),
        pane("w1:details-other-tab", "w1:t2", Some(DETAILS_TOKEN_KEY)),
    ]);
    let a = Shown {
        pane_id: PaneId("w1:details-a".into()),
        target: target(""),
    };
    let b = target("alt");

    show_details(
        &herdr,
        &PaneId("w1:side".into()),
        &b,
        Reveal::Preview,
        Some(&a),
    )
    .unwrap();

    assert_eq!(
        *herdr.calls.borrow(),
        [
            "open details next to w1:details-a",
            "identity w1:new herdr_marketplace_details",
            "close w1:details-a",
            "resize w1:new left",
            "resize w1:new right",
        ],
        "the working pane keeps its width"
    );
    let opened = herdr.opened.borrow();
    assert!(!opened[0].focus);
    let handed: DetailsTarget = serde_json::from_str(&opened[0].env[DETAILS_ENV]).unwrap();
    assert_eq!(handed, b, "B's pane only ever knows B");
}

#[test]
fn the_plugin_already_shown_keeps_its_pane_and_enter_focuses_it() {
    let panes = vec![
        pane("w1:side", "w1:t1", Some(SIDEBAR_TOKEN_KEY)),
        pane("w1:work", "w1:t1", None),
        pane("w1:details-a", "w1:t1", Some(DETAILS_TOKEN_KEY)),
    ];
    let a = Shown {
        pane_id: PaneId("w1:details-a".into()),
        target: target(""),
    };
    let sidebar = PaneId("w1:side".into());

    let herdr = FakePanes::new(panes.clone());
    assert_eq!(
        show_details(&herdr, &sidebar, &target(""), Reveal::Preview, Some(&a)).unwrap(),
        Some(a.clone())
    );
    assert!(
        herdr.calls.borrow().is_empty(),
        "{:?}",
        herdr.calls.borrow()
    );

    let herdr = FakePanes::new(panes);
    show_details(&herdr, &sidebar, &target(""), Reveal::Focus, Some(&a)).unwrap();
    assert_eq!(*herdr.calls.borrow(), ["focus w1:details-a"]);

    // Closed since with Esc: it opens again.
    let herdr = FakePanes::new(vec![
        pane("w1:side", "w1:t1", Some(SIDEBAR_TOKEN_KEY)),
        pane("w1:work", "w1:t1", None),
    ]);
    show_details(&herdr, &sidebar, &target(""), Reveal::Focus, Some(&a)).unwrap();
    assert_eq!(herdr.calls.borrow()[0], "open details next to w1:work");
}

#[test]
fn a_search_moves_open_details_and_opens_none() {
    let sidebar = PaneId("w1:side".into());
    let herdr = FakePanes::new(vec![
        pane("w1:side", "w1:t1", Some(SIDEBAR_TOKEN_KEY)),
        pane("w1:work", "w1:t1", None),
        pane("w1:details-other-tab", "w1:t2", Some(DETAILS_TOKEN_KEY)),
    ]);
    assert_eq!(
        show_details(&herdr, &sidebar, &target(""), Reveal::Follow, None).unwrap(),
        None
    );
    assert!(
        herdr.calls.borrow().is_empty(),
        "{:?}",
        herdr.calls.borrow()
    );

    let herdr = FakePanes::new(vec![
        pane("w1:side", "w1:t1", Some(SIDEBAR_TOKEN_KEY)),
        pane("w1:work", "w1:t1", None),
        pane("w1:details-a", "w1:t1", Some(DETAILS_TOKEN_KEY)),
    ]);
    let a = Shown {
        pane_id: PaneId("w1:details-a".into()),
        target: target(""),
    };
    let followed = show_details(&herdr, &sidebar, &target("alt"), Reveal::Follow, Some(&a))
        .unwrap()
        .unwrap();
    assert_eq!(followed.target, target("alt"));
    assert_eq!(
        herdr.calls.borrow()[..3],
        [
            "open details next to w1:details-a",
            "identity w1:new herdr_marketplace_details",
            "close w1:details-a",
        ]
    );
    assert!(
        !herdr.opened.borrow()[0].focus,
        "the sidebar keeps the focus"
    );
}

#[test]
fn escape_returns_focus_to_the_sidebar_or_else_to_a_remaining_pane() {
    let herdr = FakePanes::new(vec![
        pane("w1:work", "w1:t1", None),
        pane("w1:side", "w1:t1", Some(SIDEBAR_TOKEN_KEY)),
        pane("w1:details", "w1:t1", Some(DETAILS_TOKEN_KEY)),
    ]);
    close_details(&herdr, &PaneId("w1:details".into())).unwrap();
    assert_eq!(*herdr.calls.borrow(), ["focus w1:side", "close w1:details"]);

    let herdr = FakePanes::new(vec![
        pane("w1:other-tab", "w1:t2", None),
        pane("w1:work", "w1:t1", None),
        pane("w1:details", "w1:t1", Some(DETAILS_TOKEN_KEY)),
    ]);
    close_details(&herdr, &PaneId("w1:details".into())).unwrap();
    assert_eq!(*herdr.calls.borrow(), ["focus w1:work", "close w1:details"]);
}

#[test]
fn an_installed_plugin_that_is_incompatible_is_read_at_its_installed_commit() {
    let mut installed = github_plugin(
        "martinro.next-agent",
        "martin-ro",
        "herdr-next-agent",
        None,
        SHA_B,
    );
    installed["platforms"] = json!(["linux"]);
    installed["version"] = json!("0.9.0");
    let mut next_agent = manifest("herdr-plugin.toml", "martinro.next-agent");
    next_agent["platforms"] = json!(["linux"]);
    let listing = build_listing(
        &catalog(vec![repo(
            "martin-ro",
            "herdr-next-agent",
            3,
            vec![next_agent],
        )]),
        &parse_registry(&registry(vec![installed])).unwrap(),
        Platform::Macos,
        HERDR,
    );
    let details = DetailsTarget::from_row(&listing.rows[0]);
    assert_eq!(
        details.commit, SHA_B,
        "installed commit, not the indexed one"
    );
    assert_eq!(details.version.as_deref(), Some("0.9.0"));
    assert!(!details.compatible);

    let listing = build_listing(
        &catalog(vec![repo(
            "acme",
            "plugin",
            3,
            vec![manifest("herdr-plugin.toml", "acme.plugin")],
        )]),
        &[],
        Platform::Macos,
        HERDR,
    );
    assert_eq!(DetailsTarget::from_row(&listing.rows[0]).commit, SHA_A);
}

fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

#[test]
fn a_click_on_the_commit_shows_the_full_sha() {
    let mut app = DetailsApp::new(target(""));
    app.handle_mouse(
        mouse(MouseEventKind::Down(MouseButton::Left), 9, 2),
        100,
        12,
    );
    assert!(app.full_sha);
    app.handle_mouse(
        mouse(MouseEventKind::Down(MouseButton::Left), 9, 2),
        100,
        12,
    );
    assert!(!app.full_sha);
}

#[test]
fn the_wheel_scrolls_the_readme() {
    let mut app = DetailsApp::new(target(""));
    let text: String = (1..=40).map(|n| format!("line {n}\n\n")).collect();
    app.readme_loaded(
        1,
        Ok(Readme::Found {
            text,
            fallback: false,
        }),
    );
    app.set_viewport(60, details_view::page_rows(&app, 60, 12));
    app.handle_mouse(mouse(MouseEventKind::ScrollDown, 5, 6), 60, 12);
    assert_eq!(app.scroll, 3);
    app.handle_mouse(mouse(MouseEventKind::ScrollUp, 5, 6), 60, 12);
    app.handle_mouse(mouse(MouseEventKind::ScrollUp, 5, 6), 60, 12);
    assert_eq!(app.scroll, 0);
}

#[test]
fn a_readme_network_error_offers_a_retry_button() {
    let mut app = DetailsApp::new(target(""));
    app.registry_read(1, InstalledView::NotInstalled);
    app.readme_loaded(1, Err("timeout".into()));
    let mut terminal = Terminal::new(TestBackend::new(100, 12)).unwrap();
    terminal
        .draw(|frame| details_view::render(frame, &app))
        .unwrap();
    let bar: String = terminal.backend().buffer().content()[300..400]
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(bar.starts_with(" Install (i) "), "{bar:?}");
    let retry = bar
        .find("Retry README (Enter)")
        .unwrap_or_else(|| panic!("{bar:?}"));
    app.intents.clear();
    app.handle_mouse(
        mouse(MouseEventKind::Down(MouseButton::Left), retry as u16 + 3, 3),
        100,
        12,
    );
    assert_eq!(app.intents, [DetailsIntent::LoadReadme(2)]);
    assert_eq!(app.readme, ReadmeState::Loading);
}

#[test]
fn usual_readme_names_are_tried_in_the_plugin_folder_before_root() {
    for name in ["readme.md", "Readme.md", "README", "README.markdown"] {
        let own = format!("{RAW}/massdo/herdr-marketplace-fixture/{SHA_A}/alt/{name}");
        let root = format!("{RAW}/massdo/herdr-marketplace-fixture/{SHA_A}/README.md");
        let raw = FakeRaw::new(vec![(own, Ok("# Own")), (root.clone(), Ok("# Root"))]);
        assert_eq!(
            load_readme(&raw, &source("alt"), SHA_A),
            Ok(Readme::Found {
                text: "# Own".into(),
                fallback: false
            })
        );
        assert!(!raw.asked.borrow().contains(&root));
    }
}
