//! Fast Previews ▸ Adaptive Resolution for drags outside the viewer: as in After Effects, a drag
//! that changes the frame on screen (scrubbing the time, moving or trimming layers, slipping,
//! dragging keys, scrubbing values) draws a draft until the button is released.

use std::sync::Arc;

use effectcraft_engine::project::{ItemId, Project};

use crate::EffectcraftApp;

/// The project, the active comp and its current frame index when a panel starts its pass.
/// [`DragWatch::end`] compares them with the comp and frame after the panel ran its gestures, so
/// every drag the panel handles is covered without a flag per gesture.
pub(crate) struct DragWatch {
    before: Option<(ItemId, Arc<Project>, i64)>,
}

impl DragWatch {
    pub(crate) fn begin(app: &EffectcraftApp) -> Self {
        let project = app.session.project.clone();
        let before = app.session.active_comp_id().and_then(|id| {
            let frame = project.comp_arc(id)?.frame_rate.frame_at(app.session.time());
            Some((id, project, frame))
        });
        Self { before }
    }

    /// While the primary button drags (moved past a click, or held past one), a change of the
    /// current frame or of the active comp's pixels since [`Self::begin`] sets
    /// `property_interacting`, which stays set until the button is up (`App::frame`). Pixels are
    /// compared as the frame cache does: markers count when an expression reads them, a layer's
    /// audio when something draws it, before or after the edit. A click, or a drag that draws
    /// the same pixels (scrolling, zooming, the work area, selecting) keeps Full.
    pub(crate) fn end(self, app: &mut EffectcraftApp, ctx: &egui::Context) {
        let Some((id, project, frame)) = self.before else { return };
        if app.ui.viewer.property_interacting || !ctx.input(|i| i.pointer.primary_down() && i.pointer.is_decidedly_dragging()) {
            return;
        }
        if app.session.active_comp_id() != Some(id) {
            return;
        }
        let (Some(was), Some(now)) = (project.comp_arc(id), app.session.project.comp_arc(id)) else { return };
        let moved = was.frame_rate.frame_at(app.session.time()) != frame;
        let edited = || {
            let current = &app.session.project;
            let heard = current.heard_layers().union(&project.heard_layers());
            !now.same_pixels_with(&was, current.reads_markers(), &|l| heard.contains(id, l))
        };
        if moved || (!Arc::ptr_eq(&was, &now) && edited()) {
            app.ui.viewer.property_interacting = true;
            ctx.request_repaint();
        }
    }
}
