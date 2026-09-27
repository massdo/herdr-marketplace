use crate::domain::error::AppError;
use crate::domain::ids::PaneId;
use crate::domain::pane::{OriginContext, PaneInfo};

/// Two-state sidebar decision for the captured origin tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SidebarDecision {
    Open,
    Close { pane_id: PaneId },
}

/// Inspect `pane.list` for the origin tab. Recognition is the session token
/// only; the visible label is ignored.
pub fn decide_sidebar(
    panes: &[PaneInfo],
    origin: &OriginContext,
) -> Result<SidebarDecision, AppError> {
    let origin_pane = panes
        .iter()
        .find(|pane| pane.pane_id == origin.pane_id.0)
        .ok_or(AppError::OriginChanged)?;
    if origin_pane.tab_id != origin.tab_id.0 || origin_pane.workspace_id != origin.workspace_id.0 {
        return Err(AppError::OriginChanged);
    }

    let recognised: Vec<&PaneInfo> = panes
        .iter()
        .filter(|pane| {
            pane.workspace_id == origin.workspace_id.0
                && pane.tab_id == origin.tab_id.0
                && pane.is_marketplace_sidebar()
        })
        .collect();

    match recognised.as_slice() {
        [] => Ok(SidebarDecision::Open),
        [one] => Ok(SidebarDecision::Close { pane_id: one.id() }),
        _ => Err(AppError::SeveralSidebars),
    }
}
