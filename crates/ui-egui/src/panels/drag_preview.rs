//! Fast Previews ▸ Adaptive Resolution for drags outside the viewer: as in After Effects, a drag
//! that changes the frame on screen (scrubbing the time, moving or trimming layers, slipping,
//! dragging keys, scrubbing values) draws a draft until the button is released.

use std::sync::Arc;

use effectcraft_engine::project::{Comp, ItemId};

use crate::EffectcraftApp;

/// The active comp and its current frame index when a panel starts its pass. [`DragWatch::end`]
/// compares them with the comp and frame after the panel ran its gestures, so every drag the
/// panel handles is covered without a flag per gesture.
pub(crate) struct DragWatch {
    before: Option<(ItemId, Arc<Comp>, i64)>,
}

impl DragWatch {
    pub(crate) fn begin(app: &EffectcraftApp) -> Self {
        let before = app.session.active_comp_id().and_then(|id| {
            let comp = app.session.project.comp_arc(id)?;
            let frame = comp.frame_rate.frame_at(app.session.time());
            Some((id, comp, frame))
        });
        Self { before }
    }

    /// While the primary button drags (moved past a click, or held past one), a change of the
    /// current frame or of the active comp's pixels since [`Self::begin`] sets
    /// `property_interacting`, which stays set until the button is up (`App::frame`). A click,
    /// or a drag that draws the same pixels (scrolling, zooming, the work area, selecting, markers
    /// no expression reads, as for the frame cache) keeps Full.
    pub(crate) fn end(self, app: &mut EffectcraftApp, ctx: &egui::Context) {
        let Some((id, comp, frame)) = self.before else { return };
        if app.ui.viewer.property_interacting || !ctx.input(|i| i.pointer.primary_down() && i.pointer.is_decidedly_dragging()) {
            return;
        }
        if app.session.active_comp_id() != Some(id) {
            return;
        }
        let Some(now) = app.session.project.comp_arc(id) else { return };
        let moved = comp.frame_rate.frame_at(app.session.time()) != frame;
        if moved || (!Arc::ptr_eq(&comp, &now) && !now.same_pixels_with(&comp, app.session.project.reads_markers(), &|_| false)) {
            app.ui.viewer.property_interacting = true;
            ctx.request_repaint();
        }
    }
}
