//! Classic 3D: cameras, 3D views, lights, materials and the depth-sorted, lit, shadowed
//! compositor for runs of 3D layers.
//!
//! The 2D compositor (`Renderer::comp_frame`) walks the layer stack bottom-to-top; each run of
//! consecutive 3D layers is handed to `compose::draw_run` as a group (2D layers break 3D
//! groups, as in After Effects).

pub mod adv;
pub mod bokeh;
pub mod camera;
pub(crate) mod compose;
pub mod light;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_orient;

pub use camera::{CameraState, Dof, Rig, View3D, ViewCam, Views3D, active_camera, default_camera, default_view_cam};
pub use compose::{Geo as PlaneGeo, Plane3d, PlaneDof, Run3d, SkyDraw};
pub use light::{LightState, Material, lights_at};

use effectcraft_geom::Mat3;
use effectcraft_project::Layer;

use crate::EvalCtx;

/// Layer space → comp pixels through `cam` (2D projective), for viewer overlays and picking in
/// any 3D view. 2D layers ignore the camera.
pub fn layer_to_view(ctx: &EvalCtx, layer: &Layer, cam: &CameraState) -> Mat3 {
    if !layer.is_3d() {
        return ctx.layer_to_comp(layer).0;
    }
    let (w, h) = (ctx.comp.width as f64, ctx.comp.height as f64);
    (cam.projection(w, h) * ctx.world_matrix(layer)).plane_to_mat3()
}

/// Project a world point into comp pixels through `cam` (None behind a perspective camera).
pub fn project_point(ctx: &EvalCtx, cam: &CameraState, p: [f64; 3]) -> Option<[f64; 2]> {
    cam.project(ctx.comp.width as f64, ctx.comp.height as f64, p.into()).map(|v| [v.x, v.y])
}
