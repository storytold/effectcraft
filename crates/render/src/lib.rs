//! The EffectCraft compositor (CPU reference path).
//!
//! `Renderer::comp_frame` renders a composition at a time:
//! bottom-to-top over visible layers → **source** (solid, footage, text, shapes, precomp) →
//! **masks** → **effects** → **transform** (2D affine or 3D projective through the active camera,
//! with motion blur sub-samples) → **track matte** → **blend** with the layer's mode and opacity.
//! Adjustment layers run their effects on everything below, limited by their own bounds/masks.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod audio;
pub mod auto;
pub mod cache;
pub mod color;
pub mod disk_cache;
use color::Region;
pub mod eval;
pub mod masks;
pub mod passes;
pub mod shapes;
pub mod styles;
pub mod text;
pub mod three_d;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub use auto::{AutoKey, AutoPick};
pub use cache::{CacheStats, LayerCache, LayerStore, PrefetchStore};
use effectcraft_color::BlendMode;
use effectcraft_effects::{Buf, EffectCtx, EffectEnv, EffectHost, LayerPixels, Params};
use effectcraft_geom::{Mat3, Mat4, vec2};
use effectcraft_project::{Comp, Footage, FootageKind, FrameBlend, GroupKind, ItemId, ItemKind, Layer, LayerSource, MatteKind, Project, Quality, Sampling};
pub use effectcraft_raster::Image;
use effectcraft_raster::{WarpOpts, composite_warp};
use effectcraft_time::{FrameRate, TICKS_PER_SECOND, Tick};
pub use eval::{EvalCtx, ExprHost, effect_bounds, source_size};
use rayon::prelude::*;

mod accel;
mod footage;
mod frames;
mod fx_host;
mod layers;
mod opts;
mod pipeline;

pub use accel::{Accelerator, Backend, FxStep, FxTarget};
pub use footage::{CancellableFootage, FloatCurve, FootageSource, NoFootage, is_vector_footage};
pub use frames::{
    adaptive_samples, blend_frames, content_bounds, extended_region, field_frame, frame_position, frame_size, interpret_pixels, kurbo_path, render_frame,
    roi_inside,
};
#[cfg(test)]
use fx_host::FxHost;
use fx_host::MAX_FX_DEPTH;
pub(crate) use fx_host::comp_scene;
pub use fx_host::essential_overrides;
pub use layers::{Placement, StyledLayer};
pub(crate) use opts::Collapse3d;
pub use opts::{LayerTiming, RenderOpts};
use opts::{MAX_THROUGH, Through};

pub struct Renderer<'a> {
    pub project: &'a Project,
    pub footage: &'a dyn FootageSource,
    pub expr: Option<&'a dyn ExprHost>,
    pub opts: RenderOpts,
    /// Processed-layer cache shared across frames (see [`cache`]). `None` renders everything.
    pub cache: Option<&'a LayerCache>,
    /// Optional per-layer timing sink (`frame --bench` / perf readouts).
    pub profile: Option<&'a std::sync::Mutex<Vec<LayerTiming>>>,
    /// GPU backend used when [`RenderOpts::backend`] asks for it.
    pub accel: Option<&'a dyn Accelerator>,
    /// Current nesting depth (precomp recursion guard).
    depth: usize,
    /// The project's colour pipeline (bit depth, working space, linear blending).
    pub(crate) pipe: color::Pipe,
    /// Collapsed precomp being drawn into its parent: nested comp pixels → parent comp pixels
    /// (before the output scale).
    outer: Option<Mat3>,
    /// Opacity of the collapsed precomp layers this comp is drawn through.
    opacity_mul: f32,
    /// Collapsed precomp: nested 3D layers join the parent's 3D space.
    pub(crate) collapse3d: Option<Collapse3d<'a>>,
    /// Switches inherited from the precomp layers this comp is rendered through (see
    /// [`RenderOpts::nested_switches`]): the worst quality and whether motion blur is allowed.
    pub(crate) inherited: Option<(Quality, bool)>,
    /// Classic 3D depth of field is left to an accelerator ([`three_d::Plane3d::dof`]).
    pub(crate) defer_dof: bool,
    /// This renderer draws into the top-level frame's canvas (the top comp, or a collapsed
    /// precomp drawn straight into it): the region of interest's offset applies.
    top: bool,
    /// The collapsed precomp layers this comp is drawn through, outermost first: motion blur
    /// re-evaluates their transforms at each sub-sample (see [`Renderer::buf_matrix_at`]).
    through: [Option<Through<'a>>; MAX_THROUGH],
    /// More collapsed levels than [`MAX_THROUGH`]: their motion is not blurred.
    through_lost: bool,
}

#[cfg(test)]
mod tests;
