use std::collections::BTreeMap;
use std::path::Path;

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

/// Why the details show a plugin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reveal {
    /// Enter: the pane opens if needed and takes the focus.
    Focus,
    /// The arrows or a click: the pane opens if needed and the sidebar
    /// keeps the focus.
    Preview,
    /// A search: an open pane shows the new selection, none opens.
    Follow,
}

/// One details pane per tab, right of the sidebar. The pane that already
/// shows `target` stays. Selecting another plugin updates the same pane, so
/// its size never changes and two Details panes never overlap. Without one,
/// it takes the left half of the working pane. `None`: no pane to follow.
pub fn show_details<H: HerdrPort>(
    herdr: &H,
    sidebar: &PaneId,
    target: &DetailsTarget,
    reveal: Reveal,
    shown: Option<&Shown>,
) -> Result<Option<Shown>, AppError> {
    show_details_cached(herdr, sidebar, target, reveal, shown, None)
}

pub fn show_details_cached<H: HerdrPort>(
    herdr: &H,
    sidebar: &PaneId,
    target: &DetailsTarget,
    reveal: Reveal,
    shown: Option<&Shown>,
    video_cache: Option<&Path>,
) -> Result<Option<Shown>, AppError> {
    let panes = herdr.list_panes(None)?;
    let origin = panes
        .iter()
        .find(|pane| pane.pane_id == sidebar.0)
        .cloned()
        .ok_or(AppError::OriginChanged)?;
    let mut current: Vec<PaneId> = panes
        .iter()
        .filter(|pane| pane.tab_id == origin.tab_id && pane.is_marketplace_details())
        .map(PaneInfo::id)
        .collect();
    if reveal == Reveal::Follow && current.is_empty() {
        return Ok(None);
    }
    let focus = reveal == Reveal::Focus;
    if let Some(index) =
        shown.and_then(|shown| current.iter().position(|pane| *pane == shown.pane_id))
    {
        current.swap(0, index);
    }
    // Repair any duplicates left by a previous version before changing content.
    for extra in current.iter().skip(1) {
        herdr.close_plugin_pane(extra)?;
    }
    if let Some(shown) =
        shown.filter(|shown| shown.target == *target && current.contains(&shown.pane_id))
    {
        if focus {
            herdr.focus_pane(&shown.pane_id)?;
        }
        return Ok(Some(shown.clone()));
    }

    if let Some(pane_id) = current.first() {
        herdr.update_details(pane_id, target, video_cache)?;
        if focus {
            herdr.focus_pane(pane_id)?;
        }
        return Ok(Some(Shown {
            pane_id: pane_id.clone(),
            target: target.clone(),
        }));
    }
    let pane_id = {
        let panes = herdr.list_panes(Some(&origin.workspace_id))?;
        let layout = herdr.pane_layout(sidebar)?;
        let working = pick_working_target(&panes, &layout, &origin.tab())?.id();
        let pane_id = open_pane(herdr, &working, target, focus, video_cache)?;
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
    };
    if let Err(error) = herdr
        .report_identity(&pane_id, DETAILS_TOKEN_KEY)
        .and_then(|()| settle_size(herdr, &pane_id))
    {
        let _ = herdr.close_plugin_pane(&pane_id);
        return Err(error);
    }
    Ok(Some(Shown {
        pane_id,
        target: target.clone(),
    }))
}

/// A details pane of `target`, split right of `beside`.
fn open_pane<H: HerdrPort>(
    herdr: &H,
    beside: &PaneId,
    target: &DetailsTarget,
    focus: bool,
    video_cache: Option<&Path>,
) -> Result<PaneId, AppError> {
    let target_json = serde_json::to_string(target).map_err(|error| AppError::Io {
        message: error.to_string(),
    })?;
    let mut env = BTreeMap::from([(DETAILS_ENV.to_string(), target_json)]);
    if let Some(folder) = video_cache {
        env.insert(
            "HERDR_MARKETPLACE_VIDEO_CACHE".into(),
            folder.to_string_lossy().into_owned(),
        );
    }
    let opened = herdr.open_plugin_pane(OpenPluginPane {
        plugin_id: PLUGIN_ID.to_string(),
        entrypoint: DETAILS_ENTRYPOINT.to_string(),
        target_pane_id: beside.clone(),
        focus,
        env,
    })?;
    Ok(opened.pane_id)
}

/// q: focus returns to the sidebar if it is still open, else to another
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
