//! A click on a pane without the focus only gives it the focus.

use std::time::{Duration, Instant};

use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use herdr_marketplace::adapters::tui::focus::FocusClicks;

fn mouse(kind: MouseEventKind) -> MouseEvent {
    MouseEvent {
        kind,
        column: 3,
        row: 4,
        modifiers: KeyModifiers::NONE,
    }
}

fn press() -> MouseEvent {
    mouse(MouseEventKind::Down(MouseButton::Left))
}

#[test]
fn the_click_that_gives_the_focus_only_focuses() {
    let start = Instant::now();
    let mut focus = FocusClicks::default();
    assert!(
        !focus.swallows(&press(), start),
        "a pane that has the focus acts"
    );

    // Herdr reports the focus, then forwards the click that gave it.
    focus.focus_gained(start);
    let release = start + Duration::from_micros(100);
    assert!(focus.swallows(&press(), release));
    assert!(
        !focus.swallows(&mouse(MouseEventKind::Up(MouseButton::Left)), release),
        "releases and the wheel are never swallowed"
    );
    assert!(
        !focus.swallows(&press(), start + Duration::from_millis(600)),
        "the next click acts"
    );
}

#[test]
fn a_click_long_after_a_focus_from_the_keyboard_acts() {
    let start = Instant::now();
    let mut focus = FocusClicks::default();
    focus.focus_gained(start);
    assert!(!focus.swallows(&mouse(MouseEventKind::ScrollDown), start));
    assert!(!focus.swallows(&press(), start + Duration::from_secs(2)));
}
