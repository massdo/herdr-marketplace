//! README pages: links, anchors and images resolved as GitHub does, text
//! drawn like GitHub, and images drawn with the kitty protocol or half
//! blocks. Offline.

mod support;

use std::io::Cursor;
use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use herdr_marketplace::adapters::images::{Picture, decode};
use herdr_marketplace::adapters::tui::details::{DetailsApp, DetailsIntent};
use herdr_marketplace::adapters::tui::details_view;
use herdr_marketplace::adapters::tui::graphics::{
    Graphics, PictureState, Pictures, block_lines, fit, from_replies, image_id, kitty_row,
    kitty_transmit,
};
use herdr_marketplace::adapters::tui::markdown::{Rendered, render_readme};
use herdr_marketplace::application::load_readme::Readme;
use herdr_marketplace::domain::details::DetailsTarget;
use herdr_marketplace::domain::readme::{
    LinkTarget, ReadmePlace, absolute_link, anchor, github_page,
};
use herdr_marketplace::domain::source::PluginSource;
use image::{ImageFormat, Rgba, RgbaImage};
use ratatui::style::Color;
use ratatui::text::Line;
use support::*;

const RAW: &str = "https://raw.githubusercontent.com";

fn source(subdir: &str) -> PluginSource {
    PluginSource {
        owner: "alexarthurs".into(),
        repo: "herdr-sidebar".into(),
        subdir: subdir.into(),
    }
}

fn place(folder: &str) -> ReadmePlace {
    ReadmePlace {
        source: source("plugins/herdr-sidebar"),
        commit: SHA_A.into(),
        folder: folder.into(),
    }
}

fn web(url: &str) -> Option<LinkTarget> {
    Some(LinkTarget::Web(url.into()))
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

fn render(markdown: &str, width: usize, pictures: &mut Pictures) -> Rendered {
    render_readme(
        markdown,
        width,
        Some(&place("plugins/herdr-sidebar")),
        pictures,
    )
}

fn png(width: u32, height: u32, color: [u8; 4]) -> Vec<u8> {
    let image = RgbaImage::from_pixel(width, height, Rgba(color));
    let mut bytes = Vec::new();
    image
        .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
        .unwrap();
    bytes
}

fn picture(width: u32, height: u32) -> Arc<Picture> {
    Arc::new(decode(&png(width, height, [200, 30, 30, 255])).unwrap())
}

#[test]
fn relative_links_start_from_the_readme_folder_at_the_commit() {
    let place = place("plugins/herdr-sidebar");
    let blob = format!("https://github.com/alexarthurs/herdr-sidebar/blob/{SHA_A}");
    assert_eq!(
        place.link("../../README.md"),
        web(&format!("{blob}/README.md"))
    );
    assert_eq!(
        place.link("docs/keys.md#mouse"),
        web(&format!("{blob}/plugins/herdr-sidebar/docs/keys.md#mouse"))
    );
    assert_eq!(place.link("/LICENSE"), web(&format!("{blob}/LICENSE")));
    assert_eq!(
        place.link("../../../../x.md"),
        web(&format!("{blob}/x.md")),
        "never above the root"
    );
    assert_eq!(
        place.link("../.."),
        web(&format!(
            "https://github.com/alexarthurs/herdr-sidebar/tree/{SHA_A}"
        ))
    );
    assert_eq!(place.link("https://herdr.dev"), web("https://herdr.dev"));
    assert_eq!(place.link("//example.com/a"), web("https://example.com/a"));
    assert_eq!(
        place.link("mailto:me@example.com"),
        web("mailto:me@example.com")
    );
    assert_eq!(
        place.link("#Install--Develop"),
        Some(LinkTarget::Anchor("install--develop".into()))
    );
    assert_eq!(
        place.link("#caf%C3%A9"),
        Some(LinkTarget::Anchor("café".into()))
    );
    for refused in [
        "javascript:alert(1)",
        "data:text/html,x",
        "file:///etc/passwd",
        "",
    ] {
        assert_eq!(place.link(refused), None, "{refused}");
    }
    assert_eq!(
        absolute_link("docs/a.md"),
        None,
        "a relative link needs its place"
    );
}

#[test]
fn images_come_from_the_raw_file_at_the_commit() {
    let place = place("plugins/herdr-sidebar");
    let raw = format!("{RAW}/alexarthurs/herdr-sidebar/{SHA_A}");
    assert_eq!(
        place.image("docs/media/hero.png"),
        Some(format!("{raw}/plugins/herdr-sidebar/docs/media/hero.png"))
    );
    assert_eq!(
        place.image("./shots/my shot.png?raw=true"),
        Some(format!("{raw}/plugins/herdr-sidebar/shots/my%20shot.png"))
    );
    assert_eq!(
        place.image("https://github.com/owner/repo/blob/main/docs/a.png?raw=true"),
        Some(format!("{RAW}/owner/repo/main/docs/a.png"))
    );
    assert_eq!(
        place.image("https://img.shields.io/badge/a-b-blue"),
        Some("https://img.shields.io/badge/a-b-blue".into())
    );
    for refused in ["data:image/png;base64,AAAA", "#top", "javascript:x", ""] {
        assert_eq!(place.image(refused), None, "{refused}");
    }
}

#[test]
fn github_pages_and_anchors_are_github_ones() {
    assert_eq!(
        github_page(&source(""), SHA_A),
        format!("https://github.com/alexarthurs/herdr-sidebar/tree/{SHA_A}")
    );
    assert_eq!(
        github_page(&source("plugins/herdr-sidebar"), SHA_A),
        format!("https://github.com/alexarthurs/herdr-sidebar/tree/{SHA_A}/plugins/herdr-sidebar")
    );
    assert_eq!(anchor("Install & Develop"), "install--develop");
    assert_eq!(anchor("Hello, World!"), "hello-world");
    assert_eq!(anchor("  `cargo` build_steps "), "cargo-build_steps");
    assert_eq!(anchor("Café 🎉"), "café-");
}

#[test]
fn the_terminal_answers_decide_how_images_are_drawn() {
    let kitty = b"\x1b_Gi=31;OK\x1b\\\x1b[6;17;8t\x1b[?62;22c";
    assert_eq!(
        from_replies(kitty, None),
        Graphics::Kitty {
            cell_width: 8,
            cell_height: 17
        }
    );
    // Herdr with its kitty graphics off still answers the query, but no
    // cell size: its client would not draw the images.
    assert_eq!(
        from_replies(b"\x1b_Gi=31;OK\x1b\\\x1b[?62;22c", None),
        Graphics::Blocks
    );
    assert_eq!(from_replies(b"\x1b[?62;22c", None), Graphics::Blocks);
    assert_eq!(from_replies(b"", None), Graphics::Blocks);
    assert_eq!(from_replies(kitty, Some("blocks")), Graphics::Blocks);
    assert_eq!(from_replies(kitty, Some("off")), Graphics::Off);
    assert_eq!(
        from_replies(b"", Some("kitty")),
        Graphics::Kitty {
            cell_width: 8,
            cell_height: 16
        }
    );
}

#[test]
fn images_keep_their_proportions_within_the_pane() {
    let kitty = Graphics::Kitty {
        cell_width: 8,
        cell_height: 17,
    };
    assert_eq!(
        fit(1600, 921, None, 70, kitty),
        (70, 19),
        "wide: the pane's width"
    );
    assert_eq!(fit(1600, 921, None, 70, Graphics::Blocks), (70, 20));
    assert_eq!(
        fit(64, 64, None, 70, Graphics::Blocks),
        (8, 4),
        "small: its own size"
    );
    assert_eq!(
        fit(1600, 900, Some(200), 70, Graphics::Blocks),
        (25, 7),
        "the HTML width"
    );
    assert_eq!(
        fit(400, 2000, None, 70, Graphics::Blocks),
        (11, 28),
        "tall: capped rows"
    );
    assert_eq!(fit(16, 16, None, 70, Graphics::Blocks), (2, 1));
}

#[test]
fn kitty_images_are_sent_in_chunks_and_drawn_with_placeholders() {
    let short = kitty_transmit(7, b"tiny", 3, 2);
    assert_eq!(
        short,
        "\x1b_Ga=T,U=1,f=100,t=d,q=2,i=7,c=3,r=2,m=0;dGlueQ==\x1b\\"
    );
    let long = kitty_transmit(7, &[1u8; 5000], 3, 2);
    let chunks: Vec<&str> = long
        .split("\x1b\\")
        .filter(|chunk| !chunk.is_empty())
        .collect();
    assert_eq!(chunks.len(), 2);
    assert!(chunks[0].starts_with("\x1b_Ga=T,U=1,f=100,t=d,q=2,i=7,c=3,r=2,m=1;"));
    assert!(chunks[1].starts_with("\x1b_Gq=2,m=0;"));

    let id = image_id("https://example.com/a.png", 5, 3);
    assert!(id > 0 && id < 1 << 24);
    assert_ne!(
        id,
        image_id("https://example.com/a.png", 6, 3),
        "one id per size"
    );
    assert_eq!(id, image_id("https://example.com/a.png", 5, 3));

    let row = kitty_row(0x0a0b0c, 1, 4);
    assert_eq!(row.width(), 4);
    assert_eq!(row.spans.len(), 1);
    assert_eq!(
        row.spans[0].content, "\u{10EEEE}\u{30D}\u{305}\u{10EEEE}\u{10EEEE}\u{10EEEE}",
        "row 1, column 0, then the next columns"
    );
    assert_eq!(row.spans[0].style.fg, Some(Color::Rgb(0x0a, 0x0b, 0x0c)));
}

#[test]
fn half_blocks_draw_two_pixels_per_cell_and_keep_transparency() {
    let mut image = RgbaImage::from_pixel(2, 2, Rgba([0, 0, 0, 0]));
    image.put_pixel(0, 0, Rgba([255, 0, 0, 255]));
    image.put_pixel(0, 1, Rgba([0, 0, 255, 255]));
    image.put_pixel(1, 1, Rgba([0, 255, 0, 255]));
    let lines = block_lines(&image, 2, 1);
    assert_eq!(lines.len(), 1);
    let cells = &lines[0].spans;
    assert_eq!(cells[0].content, "▀");
    assert_eq!(cells[0].style.fg, Some(Color::Rgb(255, 0, 0)));
    assert_eq!(cells[0].style.bg, Some(Color::Rgb(0, 0, 255)));
    assert_eq!(cells[1].content, "▄", "only the lower pixel shows");
    assert_eq!(cells[1].style.fg, Some(Color::Rgb(0, 255, 0)));
    assert_eq!(cells[1].style.bg, None);
}

#[test]
fn pictures_load_once_and_kitty_images_follow_the_layout() {
    let kitty = Graphics::Kitty {
        cell_width: 8,
        cell_height: 16,
    };
    let url = "https://example.com/logo.png";
    let mut pictures = Pictures::new(kitty);
    assert!(pictures.request(url));
    assert!(!pictures.request(url), "a single download");
    assert_eq!(pictures.state(url), Some(&PictureState::Loading));
    assert!(
        pictures.lines(url, None, 40).is_none(),
        "nothing while it loads"
    );

    pictures.loaded(url, Ok(picture(160, 80)));
    pictures.begin_layout();
    let lines = pictures.lines(url, None, 40).unwrap();
    assert_eq!(lines.len(), 5, "20 columns, half as many rows at 1:2 cells");
    let first = pictures.take_commands();
    let id = image_id(url, 20, 5);
    assert!(first.starts_with(&format!("\x1b_Ga=T,U=1,f=100,t=d,q=2,i={id},c=20,r=5,")));
    assert_eq!(pictures.take_commands(), "", "sent once");

    // A narrower pane: the image is sent at its new size, the old one freed.
    pictures.begin_layout();
    pictures.lines(url, None, 10).unwrap();
    let resized = pictures.take_commands();
    assert!(resized.starts_with(&format!("\x1b_Ga=d,d=I,q=2,i={id}\x1b\\")));
    assert!(resized.contains(&format!("i={},c=10,r=3,", image_id(url, 10, 3))));

    let mut off = Pictures::new(Graphics::Off);
    assert!(!off.request(url), "no download when images are off");
    assert!(!Pictures::default().request(url));
    pictures.loaded("https://example.com/broken.png", Err("404".into()));
    assert!(
        pictures
            .lines("https://example.com/broken.png", None, 40)
            .is_none()
    );
}

#[test]
fn common_formats_decode_and_large_images_shrink() {
    let small = png(20, 10, [1, 2, 3, 255]);
    let decoded = decode(&small).unwrap();
    assert_eq!(decoded.rgba.dimensions(), (20, 10));
    assert_eq!(decoded.png, small, "a small PNG is sent as it came");

    let mut jpeg = Vec::new();
    image::DynamicImage::ImageRgba8(RgbaImage::from_pixel(30, 12, Rgba([9, 9, 9, 255])))
        .to_rgb8()
        .write_to(&mut Cursor::new(&mut jpeg), ImageFormat::Jpeg)
        .unwrap();
    let decoded = decode(&jpeg).unwrap();
    assert_eq!(decoded.rgba.dimensions(), (30, 12));
    assert!(decoded.png.starts_with(b"\x89PNG"), "re-encoded as PNG");

    let svg = br##"<?xml version="1.0"?><svg xmlns="http://www.w3.org/2000/svg" width="40" height="20"><rect width="40" height="20" fill="#0078d4"/></svg>"##;
    let decoded = decode(svg).unwrap();
    assert_eq!(
        decoded.rgba.dimensions(),
        (80, 40),
        "drawn at twice its size"
    );
    assert_eq!(
        decoded.rgba.get_pixel(10, 10),
        &Rgba([0x00, 0x78, 0xd4, 0xff])
    );
    assert_eq!(
        (decoded.width, decoded.height),
        (40, 20),
        "shown at its own size"
    );

    let wide = decode(&png(2560, 100, [5, 5, 5, 255])).unwrap();
    assert_eq!(wide.rgba.dimensions(), (1280, 50));
    assert_eq!((wide.width, wide.height), (2560, 100));

    assert!(decode(b"not an image").is_err());
}

#[test]
fn links_are_clickable_areas_that_follow_wrapping() {
    let mut pictures = Pictures::default();
    let rendered = render(
        "See [the docs](https://example.com/docs) now.\n\nA [long link text that wraps](../../README.md) here.",
        20,
        &mut pictures,
    );
    let text = texts(&rendered.lines);
    assert_eq!(text[0], "See the docs now.");
    assert_eq!(rendered.links[0].line, 0);
    assert_eq!((rendered.links[0].start, rendered.links[0].end), (4, 12));
    assert_eq!(
        rendered.links[0].target,
        LinkTarget::Web("https://example.com/docs".into())
    );
    let wrapped: Vec<_> = rendered.links[1..].iter().collect();
    assert!(
        wrapped.len() >= 2,
        "one area per line: {:?}",
        rendered.links
    );
    let readme = format!("https://github.com/alexarthurs/herdr-sidebar/blob/{SHA_A}/README.md");
    assert!(
        wrapped
            .iter()
            .all(|area| area.target == LinkTarget::Web(readme.clone()))
    );
}

#[test]
fn headings_have_github_anchors_and_rules() {
    let mut pictures = Pictures::default();
    let rendered = render(
        "# Intro\n\ntext\n\n## Intro\n\n### Deep dive",
        30,
        &mut pictures,
    );
    let text = texts(&rendered.lines);
    assert_eq!(text[0], "Intro");
    assert_eq!(text[1], "━".repeat(30));
    let anchors: Vec<(&str, &str)> = rendered
        .anchors
        .iter()
        .map(|(name, line)| (name.as_str(), text[*line].as_str()))
        .collect();
    assert_eq!(
        anchors,
        [
            ("intro", "Intro"),
            ("intro-1", "Intro"),
            ("deep-dive", "Deep dive")
        ]
    );
    let second = rendered.anchors[1].1;
    assert_eq!(
        text[second + 1],
        "─".repeat(30),
        "a thin rule under level 2"
    );
}

#[test]
fn badges_become_labels_and_alerts_keep_their_title() {
    let mut pictures = Pictures::new(Graphics::Blocks);
    let rendered = render(
        "[![CI](https://github.com/o/r/actions/workflows/ci.yml/badge.svg)](https://github.com/o/r/actions) ![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg) ![](https://img.shields.io/badge/x-y-z)\n\n> [!WARNING]\n> Careful here.",
        60,
        &mut pictures,
    );
    let text = texts(&rendered.lines);
    assert_eq!(text[0], "\u{a0}CI\u{a0} \u{a0}License:\u{a0}MIT\u{a0}");
    assert_eq!(
        rendered.links[0].target,
        LinkTarget::Web("https://github.com/o/r/actions".into())
    );
    assert!(rendered.images.is_empty(), "badges are never downloaded");
    let warning = text.iter().position(|line| line == "▎ Warning").unwrap();
    assert_eq!(text[warning + 1], "▎ Careful here.");
}

#[test]
fn html_images_links_and_details_are_read() {
    let mut pictures = Pictures::new(Graphics::Blocks);
    let rendered = render(
        concat!(
            "<picture><source media=\"(prefers-color-scheme: dark)\" srcset=\"dark.png\">",
            "<img src=\"light.png\" alt=\"Logo\" width=\"200\"></picture>\n\n",
            "<video src=\"https://github.com/user-attachments/assets/demo\" controls></video>\n\n",
            "<details><summary>More</summary>\n\nHidden text\n\n</details>\n",
        ),
        40,
        &mut pictures,
    );
    let text = texts(&rendered.lines);
    let raw = format!("{RAW}/alexarthurs/herdr-sidebar/{SHA_A}/plugins/herdr-sidebar");
    assert_eq!(
        rendered.images,
        [format!("{raw}/dark.png")],
        "the dark variant"
    );
    assert!(text.contains(&"[image: Logo]".to_string()), "{text:#?}");
    assert!(
        text.contains(&"\u{a0}▶\u{a0}video\u{a0}".to_string()),
        "{text:#?}"
    );
    assert!(rendered.links.iter().any(|area| area.target
        == LinkTarget::Web("https://github.com/user-attachments/assets/demo".into())));
    assert!(text.contains(&"▸ More".to_string()), "{text:#?}");
    assert!(text.contains(&"Hidden text".to_string()), "{text:#?}");
}

#[test]
fn a_centered_block_centers_the_markdown_inside_it() {
    let mut pictures = Pictures::default();
    let rendered = render(
        "<div align=\"center\">\n\n# Title\n\n</div>\n\nAfter",
        20,
        &mut pictures,
    );
    let text = texts(&rendered.lines);
    assert_eq!(text[0], "       Title");
    assert_eq!(text.last().unwrap(), "After");
}

#[test]
fn a_loaded_image_replaces_its_alternative_text() {
    let url =
        format!("{RAW}/alexarthurs/herdr-sidebar/{SHA_A}/plugins/herdr-sidebar/docs/hero.png");
    let markdown = "Intro\n\n![hero](docs/hero.png)\n\nOutro";
    let mut pictures = Pictures::new(Graphics::Blocks);
    let before = render(markdown, 30, &mut pictures);
    assert_eq!(before.images, std::slice::from_ref(&url));
    assert!(texts(&before.lines).contains(&"[image: hero]".to_string()));
    assert!(pictures.request(&url));
    pictures.loaded(&url, Ok(picture(240, 60)));
    let after = render(markdown, 30, &mut pictures);
    let text = texts(&after.lines);
    assert!(!text.contains(&"[image: hero]".to_string()));
    let image: Vec<&String> = text.iter().filter(|line| line.contains('▀')).collect();
    assert_eq!(
        image.len(),
        4,
        "30 columns of a 4:1 image, at 1:2 cells: {text:#?}"
    );
    assert!(image.iter().all(|line| line.chars().count() == 30));
    assert_eq!(text.last().unwrap(), "Outro");
}

fn target() -> DetailsTarget {
    DetailsTarget {
        source: source("plugins/herdr-sidebar"),
        commit: SHA_A.into(),
        id: "herdr-sidebar".into(),
        name: "herdr-sidebar".into(),
        version: Some("1.0.0".into()),
        in_catalog: true,
        compatible: true,
    }
}

fn click(column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

fn screen(app: &DetailsApp, width: u16, height: u16) -> Vec<String> {
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| details_view::render(frame, app))
        .unwrap();
    terminal
        .backend()
        .buffer()
        .content()
        .chunks(width as usize)
        .map(|line| line.iter().map(|cell| cell.symbol()).collect())
        .collect()
}

fn loaded(markdown: &str) -> DetailsApp {
    let mut app = DetailsApp::new(target());
    app.set_viewport(60, details_view::page_rows(&app, 60, 20));
    app.readme_loaded(
        1,
        Ok(Readme::Found {
            text: markdown.into(),
            fallback: false,
        }),
    );
    app.set_viewport(60, details_view::page_rows(&app, 60, 20));
    app.intents.clear();
    app
}

#[test]
fn a_click_on_a_readme_link_opens_it_and_an_anchor_scrolls() {
    let filler = "line\n\n".repeat(30);
    let mut app = loaded(&format!(
        "See [the repo README](../../README.md) or [the end](#the-end).\n\n{filler}## The end\n\nDone."
    ));
    let lines = screen(&app, 60, 20);
    let row = lines
        .iter()
        .position(|line| line.starts_with("See the repo README"))
        .unwrap();
    app.handle_mouse(click(6, row as u16), 60, 20);
    assert_eq!(
        app.intents,
        [DetailsIntent::OpenUrl(format!(
            "https://github.com/alexarthurs/herdr-sidebar/blob/{SHA_A}/README.md"
        ))]
    );
    app.intents.clear();
    app.handle_mouse(click(1, row as u16), 60, 20);
    assert!(
        app.intents.is_empty(),
        "the text around a link opens nothing"
    );

    let column = lines[row].find("the end").unwrap() as u16 + 2;
    app.handle_mouse(click(column, row as u16), 60, 20);
    assert!(app.intents.is_empty());
    let after = screen(&app, 60, 20);
    let top = details_view::page_rows(&app, 60, 20);
    assert!(
        after
            .iter()
            .take(20 - 1)
            .any(|line| line.starts_with("The end")),
        "the heading is on screen ({top} rows): {after:#?}"
    );
}

#[test]
fn open_on_github_opens_the_plugin_folder_at_the_commit() {
    let mut app = loaded("Hello");
    let page =
        format!("https://github.com/alexarthurs/herdr-sidebar/tree/{SHA_A}/plugins/herdr-sidebar");
    app.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
    assert_eq!(app.intents, [DetailsIntent::OpenUrl(page.clone())]);
    app.intents.clear();
    let bar = &screen(&app, 60, 20)[3];
    let column = bar.find("Open on GitHub (o)").unwrap() as u16 + 3;
    app.handle_mouse(click(column, 3), 60, 20);
    assert_eq!(app.intents, [DetailsIntent::OpenUrl(page)]);
}

#[test]
fn the_details_pane_asks_for_the_images_once_and_draws_them_when_they_arrive() {
    let mut app = DetailsApp::new(target());
    app.pictures.graphics = Some(Graphics::Blocks);
    app.set_viewport(60, details_view::page_rows(&app, 60, 20));
    app.intents.clear();
    app.readme_loaded(
        1,
        Ok(Readme::Found {
            text: "![hero](docs/hero.png)\n\n![hero again](docs/hero.png)".into(),
            fallback: false,
        }),
    );
    let url =
        format!("{RAW}/alexarthurs/herdr-sidebar/{SHA_A}/plugins/herdr-sidebar/docs/hero.png");
    assert_eq!(app.intents, [DetailsIntent::LoadImages(vec![url.clone()])]);
    app.intents.clear();
    app.picture_loaded(&url, Ok(picture(120, 60)));
    assert!(app.intents.is_empty(), "no second download");
    let blocks = app
        .lines
        .iter()
        .filter(|line| line.spans.iter().any(|span| span.content == "▀"))
        .count();
    assert_eq!(blocks, 2 * 4, "twice the same image of 15 × 4 cells");
}
