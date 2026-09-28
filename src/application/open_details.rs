use std::collections::BTreeMap;

use crate::application::pane_size::settle_size;
use crate::application::ports::{HerdrPort, OpenPluginPane};
use crate::domain::details::DetailsTarget;
use crate::domain::error::AppError;
use crate::domain::geometry::pick_working_target;
use crate::domain::ids::PaneId;
use crate::domain::pane::PaneInfo;
use crate::domain::{DETAILS_ENTRYPOINT, DETAILS_ENV, DETAILS_TOKEN_KEY, PLUGIN_ID};

/// The details pane a sidebar showed last, and the plugin it shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shown {
    pub pane_id: PaneId,
    pub target: DetailsTarget,
}

/// One details pane per tab, right of the sidebar. The pane that already
/// shows `target` stays. Otherwise a new pane takes the place of the current
/// one, which closes with its process and any answer it was waiting for, so
/// the working pane keeps its width; without one, it takes the left half of
/// the working pane. `focus` hands it the focus, as Enter and a click do; a
/// preview leaves the focus in the sidebar.
pub fn show_details<H: HerdrPort>(
    herdr: &H,
    sidebar: &PaneId,
    target: &DetailsTarget,
    focus: bool,
    shown: Option<&Shown>,
) -> Result<Shown, AppError> {
    let panes = herdr.list_panes(None)?;
    let origin = panes
        .iter()
        .find(|pane| pane.pane_id == sidebar.0)
        .cloned()
        .ok_or(AppError::OriginChanged)?;
    let current: Vec<PaneId> = panes
        .iter()
        .filter(|pane| pane.tab_id == origin.tab_id && pane.is_marketplace_details())
        .map(PaneInfo::id)
        .collect();
    if let Some(shown) =
        shown.filter(|shown| shown.target == *target && current.contains(&shown.pane_id))
    {
        if focus {
            herdr.focus_pane(&shown.pane_id)?;
        }
        return Ok(shown.clone());
    }

    let pane_id = match current.first() {
        Some(place) => open_pane(herdr, place, target, focus)?,
        None => {
            let panes = herdr.list_panes(Some(&origin.workspace_id))?;
            let layout = herdr.pane_layout(sidebar)?;
            let working = pick_working_target(&panes, &layout, &origin.tab())?.id();
            let pane_id = open_pane(herdr, &working, target, focus)?;
            // The swap focuses the pane it moves.
            let placed = herdr.swap_panes(&pane_id, &working).and_then(|()| {
                if focus {
                    Ok(())
                } else {
                    herdr.focus_pane(sidebar)
                }
            });
            if let Err(error) = placed {
                let _ = herdr.close_plugin_pane(&pane_id);
                return Err(error);
            }
            pane_id
        }
    };
    if let Err(error) = herdr
        .report_identity(&pane_id, DETAILS_TOKEN_KEY)
        .and_then(|()| {
            current
                .iter()
                .try_for_each(|old| herdr.close_plugin_pane(old))
        })
        .and_then(|()| settle_size(herdr, &pane_id))
    {
        let _ = herdr.close_plugin_pane(&pane_id);
        return Err(error);
    }
    Ok(Shown {
        pane_id,
        target: target.clone(),
    })
}

/// A details pane of `target`, split right of `beside`.
fn open_pane<H: HerdrPort>(
    herdr: &H,
    beside: &PaneId,
    target: &DetailsTarget,
    focus: bool,
) -> Result<PaneId, AppError> {
    let target_json = serde_json::to_string(target).map_err(|error| AppError::Io {
        message: error.to_string(),
    })?;
    let opened = herdr.open_plugin_pane(OpenPluginPane {
        plugin_id: PLUGIN_ID.to_string(),
        entrypoint: DETAILS_ENTRYPOINT.to_string(),
        target_pane_id: beside.clone(),
        focus,
        env: BTreeMap::from([(DETAILS_ENV.to_string(), target_json)]),
    })?;
    Ok(opened.pane_id)
}

/// Escape: focus returns to the sidebar if it is still open, else to another
/// pane of the tab, then the details pane closes.
pub fn close_details<H: HerdrPort>(herdr: &H, details: &PaneId) -> Result<(), AppError> {
    let panes = herdr.list_panes(None)?;
    if let Some(own) = panes.iter().find(|pane| pane.pane_id == details.0) {
        let mut others = panes
            .iter()
            .filter(|pane| pane.tab_id == own.tab_id && pane.pane_id != own.pane_id);
        let next = others
            .clone()
            .find(|pane| pane.is_marketplace_sidebar())
            .or_else(|| others.next());
        if let Some(next) = next {
            herdr.focus_pane(&next.id())?;
        }
    }
    herdr.close_plugin_pane(details)
}
