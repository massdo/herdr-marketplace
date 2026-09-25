use std::collections::BTreeMap;

use crate::application::pane_size::settle_size;
use crate::application::ports::{HerdrPort, OpenPluginPane};
use crate::domain::error::AppError;
use crate::domain::fiche::FicheTarget;
use crate::domain::geometry::pick_working_target;
use crate::domain::ids::PaneId;
use crate::domain::{FICHE_ENTRYPOINT, FICHE_ENV, FICHE_TOKEN_KEY, PLUGIN_ID};

/// One fiche per tab: the current one closes, with its process and any
/// answer it was waiting for, before the fiche of `target` opens next to the
/// working pane.
pub fn open_fiche<H: HerdrPort>(
    herdr: &H,
    sidebar: &PaneId,
    target: &FicheTarget,
) -> Result<PaneId, AppError> {
    let panes = herdr.list_panes(None)?;
    let origin = panes
        .iter()
        .find(|pane| pane.pane_id == sidebar.0)
        .cloned()
        .ok_or(AppError::OriginChanged)?;
    for old in panes
        .iter()
        .filter(|pane| pane.tab_id == origin.tab_id && pane.is_marketplace_fiche())
    {
        herdr.close_plugin_pane(&old.id())?;
    }

    let panes = herdr.list_panes(Some(&origin.workspace_id))?;
    let layout = herdr.pane_layout(sidebar)?;
    let working = pick_working_target(&panes, &layout, &origin.tab())?;
    let target_json = serde_json::to_string(target).map_err(|error| AppError::Io {
        message: error.to_string(),
    })?;
    let opened = herdr.open_plugin_pane(OpenPluginPane {
        plugin_id: PLUGIN_ID.to_string(),
        entrypoint: FICHE_ENTRYPOINT.to_string(),
        target_pane_id: working.id(),
        focus: true,
        env: BTreeMap::from([(FICHE_ENV.to_string(), target_json)]),
    })?;
    if let Err(error) = herdr
        .report_identity(&opened.pane_id, FICHE_TOKEN_KEY)
        .and_then(|()| settle_size(herdr, &opened.pane_id))
    {
        let _ = herdr.close_plugin_pane(&opened.pane_id);
        return Err(error);
    }
    Ok(opened.pane_id)
}

/// Escape: focus returns to the sidebar if it is still open, else to another
/// pane of the tab, then the fiche closes.
pub fn close_fiche<H: HerdrPort>(herdr: &H, fiche: &PaneId) -> Result<(), AppError> {
    let panes = herdr.list_panes(None)?;
    if let Some(own) = panes.iter().find(|pane| pane.pane_id == fiche.0) {
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
    herdr.close_plugin_pane(fiche)
}
