use crate::application::ports::HerdrPort;
use crate::domain::error::AppError;
use crate::domain::ids::PaneId;

const SETTLE_STEP: f64 = 0.0001;

/// Herdr 0.9.1 starts a pane opened in a split at the size of the first pane
/// on screen, and sizes its terminal to the layout only at the next layout
/// change. A back-and-forth resize of a tiny step is that change, and leaves
/// the split ratio as it was.
pub fn settle_size<H: HerdrPort>(herdr: &H, pane_id: &PaneId) -> Result<(), AppError> {
    herdr.resize_pane(pane_id, "left", SETTLE_STEP)?;
    herdr.resize_pane(pane_id, "right", SETTLE_STEP)
}
