use crate::application::open_sidebar::open_sidebar;
use crate::application::ports::HerdrPort;
use crate::domain::error::AppError;
use crate::domain::geometry::pick_working_target;
use crate::domain::ids::PaneId;
use crate::domain::pane::OriginContext;
use crate::domain::sidebar_decision::{SidebarDecision, decide_sidebar};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToggleOutcome {
    Opened(PaneId),
    Closed(PaneId),
}

/// Two-state toggle. The OS launcher lock is acquired by the adapter around
/// this call; this function itself starts at `pane.list`.
pub fn toggle_sidebar<H: HerdrPort>(
    herdr: &H,
    origin: &OriginContext,
) -> Result<ToggleOutcome, AppError> {
    let panes = herdr.list_panes(Some(origin.workspace_id.as_str()))?;
    match decide_sidebar(&panes, origin)? {
        SidebarDecision::Open => Ok(ToggleOutcome::Opened(open_sidebar(herdr, origin)?)),
        SidebarDecision::Close { pane_id } => {
            close_recognised_sidebar(herdr, origin, &pane_id)?;
            Ok(ToggleOutcome::Closed(pane_id))
        }
    }
}

/// Close the recognised sidebar. From the sidebar, return focus to the working
/// pane that donated space; from another pane of the tab, leave focus where it
/// is.
pub fn close_recognised_sidebar<H: HerdrPort>(
    herdr: &H,
    origin: &OriginContext,
    sidebar_id: &PaneId,
) -> Result<(), AppError> {
    let return_focus = if origin.pane_id == *sidebar_id {
        let panes = herdr.list_panes(Some(origin.workspace_id.as_str()))?;
        let layout = herdr.pane_layout(sidebar_id)?;
        Some(pick_working_target(&panes, &layout, &origin.tab_id)?.id())
    } else {
        None
    };
    herdr.close_plugin_pane(sidebar_id)?;
    if let Some(pane_id) = return_focus {
        herdr.focus_pane(&pane_id)?;
    }
    Ok(())
}
