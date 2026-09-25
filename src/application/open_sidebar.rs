//! Left dock, adapted from herdr-npm v0.1.0 (split, swap, resize, focus).

use std::collections::BTreeMap;

use crate::application::pane_size::settle_size;
use crate::application::ports::{HerdrPort, OpenPluginPane};
use crate::domain::error::AppError;
use crate::domain::geometry::{pick_working_target, preferred_left_resize};
use crate::domain::ids::PaneId;
use crate::domain::pane::OriginContext;
use crate::domain::{PLUGIN_ID, SIDEBAR_ENTRYPOINT, SIDEBAR_TOKEN_KEY};

/// Open the sidebar to the left of the working-pane target.
pub fn open_sidebar<H: HerdrPort>(herdr: &H, origin: &OriginContext) -> Result<PaneId, AppError> {
    let panes = herdr.list_panes(Some(origin.workspace_id.as_str()))?;
    let origin_pane = panes
        .iter()
        .find(|pane| pane.pane_id == origin.pane_id.0)
        .ok_or(AppError::OriginChanged)?;
    if origin_pane.tab_id != origin.tab_id.0 || origin_pane.workspace_id != origin.workspace_id.0 {
        return Err(AppError::OriginChanged);
    }

    let layout = herdr.pane_layout(&origin.pane_id)?;
    let target = pick_working_target(&panes, &layout, &origin.tab_id)?;
    let opened = herdr.open_plugin_pane(OpenPluginPane {
        plugin_id: PLUGIN_ID.to_string(),
        entrypoint: SIDEBAR_ENTRYPOINT.to_string(),
        target_pane_id: target.id(),
        focus: false,
        env: BTreeMap::new(),
    })?;

    let cleanup = |herdr: &H, pane_id: &PaneId, error: AppError| -> AppError {
        if matches!(error, AppError::Uncertain { .. }) {
            return error;
        }
        match herdr.close_plugin_pane(pane_id) {
            Ok(()) => error,
            Err(AppError::Uncertain { .. }) => AppError::uncertain(
                "plugin.pane.close",
                "sidebar ouverte mais une étape suivante a échoué et sa fermeture n'est pas confirmée",
            ),
            Err(_) => error,
        }
    };

    if let Err(error) = herdr.swap_panes(&opened.pane_id, &target.id()) {
        return Err(cleanup(herdr, &opened.pane_id, error));
    }
    if let Err(error) = herdr.report_identity(&opened.pane_id, SIDEBAR_TOKEN_KEY) {
        return Err(cleanup(herdr, &opened.pane_id, error));
    }

    match herdr.pane_layout(&opened.pane_id) {
        Ok(layout) => {
            let resized = match preferred_left_resize(&layout, opened.pane_id.as_str()) {
                Some(step) => {
                    // Herdr resizes the edge in the requested direction.
                    // Shrink from the working pane's left edge so an explorer
                    // to the sidebar's left cannot become the resize target.
                    let resize_target = if step.direction == "left" {
                        target.id()
                    } else {
                        opened.pane_id.clone()
                    };
                    herdr.resize_pane(&resize_target, step.direction, step.amount)
                }
                // Already about 32 columns: no layout change sizes it.
                None => settle_size(herdr, &opened.pane_id),
            };
            if let Err(error) = resized {
                return Err(cleanup(herdr, &opened.pane_id, error));
            }
        }
        Err(error) => return Err(cleanup(herdr, &opened.pane_id, error)),
    }

    if let Err(error) = herdr.focus_pane(&opened.pane_id) {
        return Err(cleanup(herdr, &opened.pane_id, error));
    }
    Ok(opened.pane_id)
}
