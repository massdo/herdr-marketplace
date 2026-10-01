//! README details pane: loading, Markdown rendering, keyboard and pane handling,
//! offline.

mod support;

use std::cell::RefCell;
use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use herdr_marketplace::adapters::image_fetch::Probe;
use herdr_marketplace::adapters::tui::details::{
    Command, DetailsApp, DetailsIntent, InstalledView, ReadmeState, VideoStatus,
};
use herdr_marketplace::adapters::tui::graphics::Graphics;
use herdr_marketplace::adapters::tui::video::VideoEvent;
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
use herdr_marketplace::domain::{DETAILS_TOKEN_KEY, SIDEBAR_TOKEN_KEY};
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
        !app.handle_key(key(KeyCode::Esc)),
        "escape leaves the details pane open"
    );
    assert!(
        app.handle_key(key(KeyCode::Char('q'))),
        "q closes the details pane"
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
fn opening_b_while_a_is_shown_updates_the_same_pane_without_opening_or_closing() {
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
        ["update w1:details-a"],
        "the working pane keeps its width"
    );
    assert!(herdr.opened.borrow().is_empty());
    assert_eq!(*herdr.updated.borrow(), [b]);
    assert_eq!(
        herdr
            .panes
            .borrow()
            .iter()
            .filter(|p| p.tab_id == "w1:t1" && p.is_marketplace_details())
            .count(),
        1
    );
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

    // Closed since with q: it opens again.
    let herdr = FakePanes::new(vec![
        pane("w1:side", "w1:t1", Some(SIDEBAR_TOKEN_KEY)),
        pane("w1:work", "w1:t1", None),
    ]);
    show_details(&herdr, &sidebar, &target(""), Reveal::Focus, Some(&a)).unwrap();
    assert_eq!(herdr.calls.borrow()[0], "open details next to w1:work");
}

#[test]
fn duplicate_cleanup_keeps_the_pane_already_shown() {
    let herdr = FakePanes::new(vec![
        pane("w1:side", "w1:t1", Some(SIDEBAR_TOKEN_KEY)),
        pane("w1:duplicate", "w1:t1", Some(DETAILS_TOKEN_KEY)),
        pane("w1:shown", "w1:t1", Some(DETAILS_TOKEN_KEY)),
    ]);
    let shown = Shown {
        pane_id: PaneId("w1:shown".into()),
        target: target(""),
    };
    assert_eq!(
        show_details(
            &herdr,
            &PaneId("w1:side".into()),
            &target(""),
            Reveal::Preview,
            Some(&shown)
        )
        .unwrap(),
        Some(shown)
    );
    assert_eq!(*herdr.calls.borrow(), ["close w1:duplicate"]);
    assert!(herdr.opened.borrow().is_empty());
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
    assert_eq!(*herdr.calls.borrow(), ["update w1:details-a"]);
    assert_eq!(followed.pane_id, a.pane_id);
    assert!(
        herdr.opened.borrow().is_empty(),
        "the sidebar keeps the focus"
    );
}

#[test]
fn closing_returns_focus_to_the_sidebar_or_else_to_a_remaining_pane() {
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

const VIDEO: &str =
    "https://github.com/user-attachments/assets/abe2f43e-fc50-4866-b753-33388967945d";

/// An 80 × 40 details pane whose README plays a video of 9.8 MB in a block
/// of 60 × 16 cells of 8 × 17 pixels, between "Intro" and "Outro", then
/// 60 lines of text.
fn with_video() -> DetailsApp {
    let mut app = DetailsApp::new(target(""));
    app.set_graphics(Graphics::Kitty {
        cell_width: 8,
        cell_height: 17,
    });
    app.video_player(true);
    app.set_viewport(80, details_view::page_rows(&app, 80, 40));
    let filler: String = (1..=30).map(|n| format!("line {n}\n\n")).collect();
    app.readme_loaded(
        1,
        Ok(Readme::Found {
            text: format!(
                "Intro\n\n<video src=\"{VIDEO}\" width=\"480\" height=\"270\"></video>\n\nOutro\n\n{filler}"
            ),
            fallback: false,
        }),
    );
    app.video_probed(
        VIDEO,
        Ok(Probe {
            content_type: "video/mp4".into(),
            size: Some(9_841_526),
        }),
    );
    app.set_viewport(80, details_view::page_rows(&app, 80, 40));
    app.intents.clear();
    let place = &app.video_places[0];
    assert_eq!((place.columns, place.rows), (60, 16));
    app
}

/// The 80 × 40 pane as drawn, row by row, with the color of each cell.
fn drawn(app: &DetailsApp) -> Vec<(String, Vec<Color>)> {
    let mut terminal = Terminal::new(TestBackend::new(80, 40)).unwrap();
    terminal
        .draw(|frame| details_view::render(frame, app))
        .unwrap();
    terminal
        .backend()
        .buffer()
        .content()
        .chunks(80)
        .map(|row| {
            (
                row.iter().map(|cell| cell.symbol()).collect(),
                row.iter().map(|cell| cell.fg).collect(),
            )
        })
        .collect()
}

/// Row and column where the pane shows `text`.
fn find(app: &DetailsApp, text: &str) -> (u16, u16) {
    let rows = drawn(app);
    let row = rows
        .iter()
        .position(|(row, _)| row.contains(text))
        .unwrap_or_else(|| panic!("no {text:?} in {rows:#?}"));
    let column = rows[row].0[..rows[row].0.find(text).unwrap()]
        .chars()
        .count();
    (row as u16, column as u16)
}

fn play_intent() -> DetailsIntent {
    DetailsIntent::PlayVideo {
        start: 0,
        url: VIDEO.into(),
        looped: false,
        fit: (60 * 8, 16 * 17),
    }
}

#[test]
fn a_click_on_the_play_button_plays_the_video_in_frames_that_fit_its_cells() {
    let mut app = with_video();
    let (row, column) = find(&app, "▶ Play video (9.8 MB)");
    app.handle_mouse(
        mouse(MouseEventKind::Down(MouseButton::Left), column + 3, row),
        80,
        40,
    );
    assert_eq!(app.intents, [play_intent()]);
    assert_eq!(
        app.video.as_ref().map(|video| &video.status),
        Some(&VideoStatus::Starting)
    );

    // A click in the block of the video that plays stops it.
    app.intents.clear();
    app.handle_mouse(
        mouse(MouseEventKind::Down(MouseButton::Left), column, row + 2),
        80,
        40,
    );
    assert_eq!(app.intents, [DetailsIntent::StopVideo]);
    assert!(app.video.is_none());
}

#[test]
fn p_plays_the_video_then_pauses_and_resumes_without_restarting() {
    let mut app = with_video();
    app.handle_key(key(KeyCode::Char('p')));
    assert_eq!(app.intents, [play_intent()]);
    app.intents.clear();
    app.video_event(VideoEvent::Playing);
    app.handle_key(key(KeyCode::Char('p')));
    assert_eq!(app.intents, [DetailsIntent::PauseVideo(true)]);
    assert_eq!(app.video.as_ref().unwrap().status, VideoStatus::Paused);
    app.intents.clear();
    app.handle_key(key(KeyCode::Char('p')));
    assert_eq!(app.intents, [DetailsIntent::PauseVideo(false)]);
    app.press(Command::StopVideo);

    // Scrolled past its block, p finds no video to play.
    app.intents.clear();
    app.handle_key(key(KeyCode::End));
    assert!(
        drawn(&app)
            .iter()
            .all(|(row, _)| !row.starts_with("Intro") && !row.contains("Play video"))
    );
    app.handle_key(key(KeyCode::Char('p')));
    assert!(app.intents.is_empty(), "{:?}", app.intents);
}

#[test]
fn the_player_controls_seek_pause_and_ignore_background_download_progress() {
    let mut app = with_video();
    app.handle_key(key(KeyCode::Char('p')));
    app.video_event(VideoEvent::Playing);
    app.video_event(VideoEvent::Position {
        milliseconds: 10_000,
        duration: Some(30_000),
    });
    app.video_event(VideoEvent::Downloading(5_000_000, Some(9_000_000)));
    assert_eq!(app.video.as_ref().unwrap().status, VideoStatus::Playing);
    app.intents.clear();
    let (row, column) = find(&app, "[Pause]");
    app.handle_mouse(
        mouse(MouseEventKind::Down(MouseButton::Left), column + 2, row),
        80,
        40,
    );
    assert_eq!(app.intents, [DetailsIntent::PauseVideo(true)]);
    assert_eq!(app.video.as_ref().unwrap().position, 10_000);
    app.intents.clear();
    app.handle_key(key(KeyCode::Right));
    assert_eq!(app.intents, [DetailsIntent::SeekVideo(15_000)]);
    assert_eq!(app.video.as_ref().unwrap().position, 15_000);
    assert_eq!(
        app.video.as_ref().unwrap().status,
        VideoStatus::Buffering { paused: true }
    );
    app.intents.clear();
    let (row, column) = find(&app, "[+5s]");
    app.handle_mouse(
        mouse(MouseEventKind::Down(MouseButton::Left), column + 2, row),
        80,
        40,
    );
    assert_eq!(app.intents, [DetailsIntent::SeekVideo(20_000)]);
    app.video_event(VideoEvent::Downloading(6_000_000, Some(9_000_000)));
    assert_eq!(app.video.as_ref().unwrap().position, 20_000);
    assert_eq!(
        app.video.as_ref().unwrap().status,
        VideoStatus::Buffering { paused: true }
    );
    app.intents.clear();
    let (pause_row, pause_column) = find(&app, "[Play]");
    app.handle_mouse(
        mouse(
            MouseEventKind::Down(MouseButton::Left),
            pause_column + 2,
            pause_row,
        ),
        80,
        40,
    );
    assert_eq!(app.intents, [DetailsIntent::PauseVideo(false)]);
    assert_eq!(
        app.video.as_ref().unwrap().status,
        VideoStatus::Buffering { paused: false }
    );
    find(&app, "Buffering…");
    let (row, column) = find(&app, "[====");
    app.intents.clear();
    app.handle_mouse(
        mouse(MouseEventKind::Down(MouseButton::Left), column + 4, row),
        80,
        40,
    );
    assert!(
        matches!(app.intents.as_slice(), [DetailsIntent::SeekVideo(to)] if *to > 0 && *to < 30_000)
    );
    app.video_event(VideoEvent::Ended);
    app.intents.clear();
    app.press(Command::SeekVideo(5000));
    assert!(matches!(
        app.intents.as_slice(),
        [DetailsIntent::PlayVideo { start: 5000, .. }]
    ));
}

#[test]
fn autoplay_waits_for_a_visible_pane_and_respects_a_manual_stop() {
    let mut app = with_video();
    app.autoplay_video(false);
    assert!(app.video.is_none());
    assert!(app.intents.is_empty());
    app.autoplay_video(true);
    assert_eq!(app.intents, [play_intent()]);
    app.intents.clear();
    app.autoplay_video(true);
    assert!(app.intents.is_empty(), "only one playback starts");
    app.handle_key(key(KeyCode::Char('p')));
    assert_eq!(app.intents, [DetailsIntent::StopVideo]);
    app.intents.clear();
    app.handle_key(key(KeyCode::End));
    app.handle_key(key(KeyCode::Home));
    app.autoplay_video(false);
    app.autoplay_video(true);
    assert!(app.intents.is_empty(), "a stopped video must stay stopped");
    app.handle_key(key(KeyCode::Char('p')));
    assert_eq!(app.intents, [play_intent()], "manual replay still works");
}

#[test]
fn autoplay_resumes_a_hidden_video_but_does_not_repeat_an_end_or_failure() {
    let mut app = with_video();
    app.autoplay_video(true);
    app.intents.clear();
    app.video_event(VideoEvent::Hidden);
    app.autoplay_video(false);
    assert!(app.intents.is_empty());
    app.autoplay_video(true);
    assert_eq!(app.intents, [play_intent()]);
    app.intents.clear();
    app.video_event(VideoEvent::Ended);
    app.autoplay_video(true);
    assert!(app.intents.is_empty(), "a non-looped video must not repeat");
    app.handle_key(key(KeyCode::Char('p')));
    app.intents.clear();
    app.video_event(VideoEvent::Failed("network down".into()));
    app.autoplay_video(true);
    assert!(
        app.intents.is_empty(),
        "a failure must not retry on every tick"
    );
}

#[test]
fn autoplay_does_not_download_a_video_outside_the_view() {
    let mut app = with_video();
    app.handle_key(key(KeyCode::End));
    app.autoplay_video(true);
    assert!(app.intents.is_empty());
    app.handle_key(key(KeyCode::Home));
    app.autoplay_video(true);
    assert_eq!(app.intents, [play_intent()]);
}

#[test]
fn the_block_tells_the_download_then_leaves_its_cells_to_the_frames() {
    let mut app = with_video();
    app.handle_key(key(KeyCode::Char('p')));
    app.video_event(VideoEvent::Downloading(4_920_763, Some(9_841_526)));
    let rows = drawn(&app);
    assert!(
        rows.iter()
            .any(|(row, _)| row.contains(" Downloading video… 50% ")),
        "{rows:#?}"
    );
    assert!(!rows.iter().any(|(row, _)| row.contains("Play video")));
    app.video_event(VideoEvent::Downloading(3_200_000, None));
    assert!(
        drawn(&app)
            .iter()
            .any(|(row, _)| row.contains(" Downloading video… 3.2 MB "))
    );

    app.video_event(VideoEvent::Playing);
    let rows = drawn(&app);
    let intro = rows
        .iter()
        .position(|(row, _)| row.starts_with("Intro"))
        .unwrap();
    let outro = rows
        .iter()
        .position(|(row, _)| row.starts_with("Outro"))
        .unwrap();
    assert!(
        rows[intro + 1..outro]
            .iter()
            .all(|(row, _)| row.trim().is_empty() || row.contains("[Pause]")),
        "{rows:#?}"
    );

    app.video_event(VideoEvent::Ended);
    assert_eq!(app.video.as_ref().unwrap().status, VideoStatus::Ended);
    assert!(
        drawn(&app)
            .iter()
            .any(|(row, _)| row.contains("▶ Play video (9.8 MB)"))
    );
    app.intents.clear();
    let (row, column) = find(&app, "▶ Play video (9.8 MB)");
    app.handle_mouse(
        mouse(MouseEventKind::Down(MouseButton::Left), column + 2, row),
        80,
        40,
    );
    assert_eq!(
        app.intents,
        [play_intent()],
        "a finished video replays with one click"
    );
}

#[test]
fn a_failed_video_keeps_its_button_and_tells_why_in_red_under_it() {
    let mut app = with_video();
    app.handle_key(key(KeyCode::Char('p')));
    app.video_event(VideoEvent::Failed("no frame\u{1b}[31m after 10 s".into()));
    let rows = drawn(&app);
    let button = rows
        .iter()
        .position(|(row, _)| row.contains("▶ Play video (9.8 MB)"))
        .expect("the button stays");
    assert!(rows[button].0.contains("Open in browser"));
    let (row, colors) = &rows[button + 1];
    let start = row
        .find("Could not play the video: no frame")
        .unwrap_or_else(|| {
            panic!("no reason under the button: {rows:#?}");
        });
    assert!(!row.contains('\u{1b}'));
    assert_eq!(colors[row[..start].chars().count()], Color::Red);

    // The button tries again.
    app.intents.clear();
    let (row, column) = find(&app, "▶ Play video (9.8 MB)");
    app.handle_mouse(
        mouse(MouseEventKind::Down(MouseButton::Left), column + 3, row),
        80,
        40,
    );
    assert_eq!(app.intents, [play_intent()]);
}
