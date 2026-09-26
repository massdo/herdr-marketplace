//! A click on a pane without the focus only gives it the focus.

use std::time::{Duration, Instant};

use crossterm::event::{MouseEvent, MouseEventKind};

/// Herdr reports the focus a click gives a pane, then forwards that click a
/// fraction of a millisecond later.
const FOCUS_CLICK: Duration = Duration::from_millis(200);

/// Tells the click that gave the focus from a click on a pane that had it.
/// The first only focuses, so that clicking the sidebar to type a search
/// never opens a plugin.
#[derive(Debug, Default)]
pub struct FocusClicks {
    gained: Option<Instant>,
}

impl FocusClicks {
    pub fn focus_gained(&mut self, now: Instant) {
        self.gained = Some(now);
    }

    /// Whether `mouse` is the click that just gave the focus, to be ignored.
    pub fn swallows(&mut self, mouse: &MouseEvent, now: Instant) -> bool {
        match mouse.kind {
            MouseEventKind::Down(_) => self
                .gained
                .take()
                .is_some_and(|gained| now.saturating_duration_since(gained) < FOCUS_CLICK),
            _ => false,
        }
    }
}
