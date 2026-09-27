//! Left dock geometry, adapted from herdr-npm v0.1.0, itself adapted from
//! herdr-sidebar 0.13.0 (MIT, see NOTICE).

use crate::domain::error::AppError;
use crate::domain::ids::TabId;
use crate::domain::pane::{LayoutSnapshot, PaneInfo, ResizeStep};
use crate::domain::{MAX_SIDEBAR_SHARE, MIN_SIDEBAR_SHARE, PREFERRED_OUTER_COLUMNS};

/// Working-pane target: leftmost, then highest, then pane id. The marketplace
/// sidebar and the explorer are never split.
pub fn pick_working_target<'a>(
    panes: &'a [PaneInfo],
    layout: &LayoutSnapshot,
    origin_tab: &TabId,
) -> Result<&'a PaneInfo, AppError> {
    let mut candidates: Vec<(&PaneInfo, (u16, u16, &str))> = Vec::new();
    for pane in panes {
        if pane.tab_id != origin_tab.0 || pane.is_excluded_working_target() {
            continue;
        }
        let Some(layout_pane) = layout
            .panes
            .iter()
            .find(|item| item.pane_id == pane.pane_id)
        else {
            continue;
        };
        candidates.push((
            pane,
            (
                layout_pane.rect.x,
                layout_pane.rect.y,
                pane.pane_id.as_str(),
            ),
        ));
    }
    candidates.sort_by_key(|(_, key)| *key);
    candidates
        .into_iter()
        .map(|(pane, _)| pane)
        .next()
        .ok_or(AppError::NoWorkingTarget)
}

/// Ratio delta that moves the left-docked sidebar toward 32 outer columns,
/// clamped to Herdr's usable share of the local split.
pub fn preferred_left_resize(layout: &LayoutSnapshot, pane_id: &str) -> Option<ResizeStep> {
    let pane_rect = layout
        .panes
        .iter()
        .find(|pane| pane.pane_id == pane_id)?
        .rect;
    let divider_x = i64::from(pane_rect.x) + i64::from(pane_rect.width);
    let split = layout
        .splits
        .iter()
        .filter(|split| split.direction == "right")
        .filter(|split| split.rect.width > 0)
        .filter(|split| {
            let split_divider = i64::from(split.rect.x)
                + (f64::from(split.rect.width) * split.ratio).round() as i64;
            i64::from(split.rect.x) <= i64::from(pane_rect.x)
                && (split_divider - divider_x).abs() <= 2
        })
        .min_by_key(|split| split.rect.width)?;

    let requested = i64::from(PREFERRED_OUTER_COLUMNS);
    let min = (f64::from(split.rect.width) * MIN_SIDEBAR_SHARE).ceil() as i64;
    let max = (f64::from(split.rect.width) * MAX_SIDEBAR_SHARE).floor() as i64;
    let target_w = requested.clamp(min, max.max(min));
    let delta = (target_w - i64::from(pane_rect.width)) as f64 / f64::from(split.rect.width);
    if delta.abs() < 0.005 {
        return None;
    }
    Some(ResizeStep {
        direction: if delta > 0.0 { "right" } else { "left" },
        amount: delta.abs(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::pane::{LayoutPane, LayoutRect, LayoutSplit};
    use std::collections::BTreeMap;

    fn pane(id: &str) -> PaneInfo {
        PaneInfo {
            pane_id: id.into(),
            workspace_id: "w1".into(),
            tab_id: "w1:t1".into(),
            focused: false,
            label: None,
            title: None,
            tokens: BTreeMap::new(),
        }
    }

    fn rect(x: u16, width: u16) -> LayoutRect {
        LayoutRect {
            x,
            y: 0,
            width,
            height: 40,
        }
    }

    fn layout(panes: Vec<LayoutPane>, splits: Vec<LayoutSplit>) -> LayoutSnapshot {
        LayoutSnapshot {
            workspace_id: "w1".into(),
            tab_id: "w1:t1".into(),
            area: rect(0, 120),
            focused_pane_id: "w1:p-ed".into(),
            panes,
            splits,
        }
    }

    #[test]
    fn working_target_skips_explorer_and_picks_leftmost() {
        let mut explorer = pane("w1:p-exp");
        explorer.label = Some("Sidebar".into());
        let panes = vec![explorer, pane("w1:p-ed")];
        let snapshot = layout(
            vec![
                LayoutPane {
                    pane_id: "w1:p-exp".into(),
                    focused: false,
                    rect: rect(0, 32),
                },
                LayoutPane {
                    pane_id: "w1:p-ed".into(),
                    focused: true,
                    rect: rect(32, 88),
                },
            ],
            vec![],
        );
        let chosen = pick_working_target(&panes, &snapshot, &TabId("w1:t1".into())).unwrap();
        assert_eq!(chosen.pane_id, "w1:p-ed");
    }

    #[test]
    fn a_half_width_sidebar_shrinks_toward_32_columns() {
        let snapshot = layout(
            vec![
                LayoutPane {
                    pane_id: "w1:p-mkt".into(),
                    focused: true,
                    rect: rect(0, 60),
                },
                LayoutPane {
                    pane_id: "w1:p-ed".into(),
                    focused: false,
                    rect: rect(60, 60),
                },
            ],
            vec![LayoutSplit {
                id: "s".into(),
                direction: "right".into(),
                ratio: 0.5,
                rect: rect(0, 120),
            }],
        );
        let step = preferred_left_resize(&snapshot, "w1:p-mkt").unwrap();
        assert_eq!(step.direction, "left");
        assert!((step.amount - 28.0 / 120.0).abs() < 1e-9, "{step:?}");
    }
}
