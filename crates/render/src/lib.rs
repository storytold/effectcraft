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

/// Supplies decoded footage frames (implemented by the media layer).
pub trait FootageSource: Send + Sync {
    /// The frame of `item` at source time `t`, straight from the file (any size).
    fn frame(&self, item: ItemId, footage: &Footage, t: Tick) -> Option<Arc<Image>>;
    /// `frames` interleaved stereo sample frames of `item`'s audio from source time `t` at
    /// `rate` Hz (`None` when unavailable).
    fn audio(&self, _item: ItemId, _footage: &Footage, _t: Tick, _frames: usize, _rate: u32) -> Option<Vec<f32>> {
        None
    }
    /// Auxiliary 3D channels of `item` at source time `t` (multi-layer OpenEXR: depth, IDs,
    /// Cryptomatte, any named channel), with `scale` relative to the file's pixels. `None` when
    /// the footage has none.
    fn aux(&self, _item: ItemId, _footage: &Footage, _t: Tick) -> Option<Arc<effectcraft_raster::AuxChannels>> {
        None
    }
    /// The parsed 3D model of a [`effectcraft_project::FootageKind::Model`] item (Advanced 3D).
    fn model(&self, _item: ItemId, _footage: &Footage) -> Option<Arc<effectcraft_model::Model>> {
        None
    }
    /// Set the decoded-frame cache budget in bytes (Settings ▸ Memory & CPU); sources without
    /// a cache ignore it.
    fn set_cache_budget(&self, _bytes: usize) {}
    /// Drop the decoded frames it keeps (Edit ▸ Purge ▸ All Memory / Image Cache Memory): the
    /// next reads decode again. Sources without a cache ignore it.
    fn purge(&self) {}
    /// Settings ▸ Disk ▸ Conformed Audio Folder: where decoded audio is kept between reads
    /// (`None` = off); sources that don't decode audio ignore it.
    fn set_conform_folder(&self, _folder: Option<std::path::PathBuf>) {}
    /// The decoded-frame cache budget in bytes, if the source has a cache.
    fn cache_budget(&self) -> Option<usize> {
        None
    }
    /// Vector footage (SVG) rasterised at `scale` × its pixel size, for Continuously Rasterize.
    /// `None` when the footage is not vector or cannot be read.
    fn vector_frame(&self, _item: ItemId, _footage: &Footage, _scale: f64) -> Option<Arc<Image>> {
        None
    }
}

/// Footage that can be rasterised at any scale (SVG).
pub fn is_vector_footage(f: &Footage) -> bool {
    matches!(f.codec.as_str(), "SVG" | "PDF" | "AI" | "EPS")
}

/// Layer parameters and audio for effects (see [`effectcraft_effects::EffectHost`]). What it
/// hands out is in effect space (see [`effect_bounds`]): the other layer's for its pixels, the
/// running layer's for its own frames and the comp scene.
struct FxHost<'r, 'a, 'c> {
    r: &'r Renderer<'a>,
    ctx: &'c EvalCtx<'a>,
    layer: &'c Layer,
    /// Where the stack's effect space starts in layer coordinates (zero on adjustment layers,
    /// whose effects run on the comp below).
    origin: [f64; 2],
    /// Index of the effect being rendered (bounds `self_at` to effects before it).
    index: std::sync::atomic::AtomicUsize,
}

impl FxHost<'_, '_, '_> {
    /// Another layer's pixels for a layer parameter, in its effect space; `layer_space`: `buf`
    /// is still in layer space (its source or its content).
    fn pixels(&self, other: &Layer, mut buf: Buf, layer_space: bool) -> LayerPixels {
        let (size, origin) = self.ctx.effect_bounds(other);
        if layer_space {
            buf.rebase(origin);
        }
        LayerPixels { buf, size }
    }
}

/// The comp's camera and first light relative to `layer`, in its layer pixels (see
/// [`EffectHost::comp_scene`]): the comp camera's view of the layer (as if 3D at its transform),
/// brought back into the layer's own pixel grid through the inverse of how the layer itself
/// composites.
pub(crate) fn comp_scene(ctx: &EvalCtx, layer: &Layer) -> Option<effectcraft_effects::CompScene> {
    let world = ctx.world_matrix(layer);
    let (cam, _, _) = ctx.camera();
    let p = (cam * world).0;
    let back = ctx.layer_to_comp(layer).0.inverse()?.0;
    let rows = [p[0], p[1], p[3]];
    let camera: [[f64; 4]; 3] = std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| back[i][k] * rows[k][j]).sum()));
    let inv = world.inverse();
    let light = three_d::light::lights_at(ctx).first().and_then(|l| {
        let inv = inv.as_ref()?;
        let pos = inv.apply(l.pos);
        let dir = inv.apply_vec(l.dir);
        let len = (dir.x * dir.x + dir.y * dir.y + dir.z * dir.z).sqrt().max(1e-12);
        let kind = match l.kind {
            effectcraft_project::LightKind::Parallel => 0,
            effectcraft_project::LightKind::Ambient => 2,
            _ => 1,
        };
        Some(effectcraft_effects::CompLight { pos: [pos.x, pos.y, pos.z], dir: [dir.x / len, dir.y / len, dir.z / len], color: l.color, kind })
    });
    let camera_layer = ctx.comp.active_camera(ctx.time).is_some();
    Some(effectcraft_effects::CompScene { camera: Some(camera), camera_layer, light })
}

/// Nesting limit for effects that render layers (other layers, other times).
const MAX_FX_DEPTH: usize = 8;

impl EffectHost for FxHost<'_, '_, '_> {
    fn particles(&self) -> Option<&dyn effectcraft_effects::psim::ParticleSim> {
        self.r.active_accel().and_then(|a| a.particles())
    }
    fn layer(&self, id: u64, masks_and_effects: bool) -> Option<LayerPixels> {
        let other = self.ctx.layer(effectcraft_project::LayerId(id))?;
        if other.id == self.layer.id || self.r.depth > MAX_FX_DEPTH {
            return None;
        }
        let sub = self.r.nested();
        // The cached layer buffer is shared; effects get their own copy.
        let buf = if masks_and_effects { (*sub.content_buf(self.ctx, other)?).clone() } else { sub.source(self.ctx, other)? };
        Some(self.pixels(other, buf, true))
    }

    fn layer_masks(&self, id: u64) -> Option<LayerPixels> {
        let other = self.ctx.layer(effectcraft_project::LayerId(id))?;
        if other.id == self.layer.id || self.r.depth > MAX_FX_DEPTH {
            return None;
        }
        let sub = self.r.nested();
        let buf = (*sub.layer_input(self.ctx, other, 0)?).clone();
        Some(self.pixels(other, buf, false))
    }

    fn audio(&self, id: u64, start: f64, frames: usize, rate: u32) -> Option<Vec<f32>> {
        let other = self.ctx.layer(effectcraft_project::LayerId(id))?;
        let LayerSource::Footage { item } = &other.source else { return None };
        let ItemKind::Footage(f) = &self.r.project.item(*item)?.kind else { return None };
        if !f.has_audio {
            return None;
        }
        let st = other.layer_time(Tick::from_seconds_f64(start));
        self.r.footage.audio(*item, f, st, frames, rate)
    }

    fn self_at(&self, layer_time: f64, effects: usize) -> Option<Buf> {
        if self.r.depth > MAX_FX_DEPTH || self.layer.switches.adjustment {
            return None;
        }
        let n = effects.min(self.index.load(std::sync::atomic::Ordering::Relaxed));
        let t = self.layer.comp_time(Tick::from_seconds_f64(layer_time));
        let sub = self.r.nested();
        sub.layer_input(&self.ctx.at(t), self.layer, n).map(|b| (*b).clone())
    }

    fn aux(&self) -> Option<Arc<effectcraft_raster::AuxChannels>> {
        match &self.layer.source {
            LayerSource::Footage { item } => {
                let ItemKind::Footage(f) = &self.r.project.item(*item)?.kind else { return None };
                let a = self.r.footage.aux(*item, f, self.ctx.source_time(self.layer))?;
                // Aux pixels per layer pixel (the layer is the footage at its nominal size).
                if f.width > 0 && a.width > 0 {
                    let mut a = (*a).clone();
                    a.scale = a.width as f64 / f.width as f64;
                    return Some(Arc::new(a));
                }
                Some(a)
            }
            LayerSource::Comp { item } => {
                if self.r.depth > MAX_FX_DEPTH || self.r.project.comp_contains(*item, self.ctx.comp_id) {
                    return None;
                }
                self.r.nested().comp_aux(*item, self.ctx.nested_time(self.layer)).map(Arc::new)
            }
            _ => None,
        }
    }

    fn params_at(&self, layer_time: f64) -> Option<Params> {
        let i = self.index.load(std::sync::atomic::Ordering::Relaxed);
        let g = self.layer.effects()?.groups().nth(i)?;
        let ctx = self.ctx.at(self.layer.comp_time(Tick::from_seconds_f64(layer_time)));
        Some(effectcraft_effects::flatten_params(g, &mut |pr| ctx.value(self.layer, pr)))
    }

    fn comp_scene(&self) -> Option<effectcraft_effects::CompScene> {
        Some(comp_scene(self.ctx, self.layer)?.rebased(self.origin))
    }

    fn layer_at(&self, id: u64, comp_time: f64, masks_and_effects: bool) -> Option<LayerPixels> {
        let other = self.ctx.layer(effectcraft_project::LayerId(id))?;
        if other.id == self.layer.id || self.r.depth > MAX_FX_DEPTH {
            return None;
        }
        let ctx = self.ctx.at(Tick::from_seconds_f64(comp_time));
        let sub = self.r.nested();
        Some(if masks_and_effects {
            self.pixels(other, (*sub.content_buf(&ctx, other)?).clone(), true)
        } else {
            self.pixels(other, (*sub.layer_input(&ctx, other, 0)?).clone(), false)
        })
    }
}

/// The Essential Properties overrides of a precomp layer, evaluated at the context time.
pub fn essential_overrides(ctx: &EvalCtx, layer: &Layer) -> Vec<effectcraft_project::essential::Override> {
    use effectcraft_project::essential;
    let over = essential::overridden(layer);
    let Some(g) = essential::group(layer).filter(|_| !over.is_empty()) else { return vec![] };
    let mut out = vec![];
    g.walk("", &mut |_, p| {
        if over.contains(&p.uid)
            && let Some(control) = essential::control_of(&p.match_id)
        {
            out.push(essential::Override { control, value: ctx.value(layer, p) });
        }
    });
    out
}

/// No footage available (renders footage layers as transparent).
pub struct NoFootage;
impl FootageSource for NoFootage {
    fn frame(&self, _: ItemId, _: &Footage, _: Tick) -> Option<Arc<Image>> {
        None
    }
}

/// Which compositor renders a frame (Project Settings ▸ Video Rendering and Effects).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    /// The CPU reference compositor (Mercury Software Only).
    #[default]
    Cpu,
    /// The GPU compositor whenever an [`Accelerator`] is attached, whatever the project says.
    Gpu,
    /// The GPU compositor when an [`Accelerator`] is attached and the project's renderer is
    /// Mercury GPU Acceleration (the viewer's choice); the CPU otherwise.
    Auto,
}

impl Backend {
    pub fn parse(s: &str) -> Option<Backend> {
        match s.to_ascii_lowercase().as_str() {
            "cpu" | "software" | "mercury software only" => Some(Backend::Cpu),
            "gpu" | "mercury gpu acceleration" => Some(Backend::Gpu),
            "auto" => Some(Backend::Auto),
            _ => None,
        }
    }
}

/// One effect of a GPU effect chain (see [`Accelerator::effects`]).
pub struct FxStep<'x> {
    pub spec: &'static effectcraft_effects::EffectSpec,
    pub ctx: EffectCtx<'x>,
}

/// A GPU (or other hardware) backend for the compositor. The CPU [`Renderer`] stays the
/// reference: an accelerator renders what it supports and returns `None` for the rest, which
/// then runs on the CPU. Implemented by `effectcraft-gpu`.
pub trait Accelerator: Send + Sync {
    /// Adapter / backend name for status readouts ("Apple M2 (Metal)").
    fn name(&self) -> String;
    /// Render a top-level comp frame (what [`Renderer::comp_frame`] returns). `None` = not
    /// handled (the CPU renders it).
    fn comp_frame(&self, r: &Renderer, comp: ItemId, t: Tick) -> Option<Image>;
    /// The accelerator implements effect `id` with the CPU effect's semantics.
    fn supports_effect(&self, id: &str) -> bool;
    /// Run a chain of supported effects on `buf`, clamping and quantising to `levels` after
    /// every effect when set (8/16 bpc). `None` = not handled.
    fn effects(&self, chain: &[FxStep], buf: &Buf, levels: Option<f32>) -> Option<Buf>;
    /// Rasterise and shade an Advanced 3D scene at its raster size (depth buffer, PBR, image
    /// based light, shadow maps), with the CPU rasteriser's semantics
    /// ([`three_d::adv::raster::render`]). `None` = not handled (the CPU renders it).
    fn raster_3d(&self, _scene: &three_d::adv::Scene) -> Option<three_d::adv::Target> {
        None
    }
    /// Render a whole prepared Advanced 3D run (every motion-blur sub-sample rasterised and
    /// resolved, averaged, depth of field, encoding) with [`three_d::adv::render_prepared`]'s
    /// semantics. `None` = not handled (the CPU path runs, using [`Self::raster_3d`]).
    fn render_3d(&self, _run: &three_d::adv::Prepared) -> Option<three_d::adv::Rendered> {
        None
    }
    /// A particle simulation backend (GPU particles) for the stepped particle effects, with
    /// the CPU simulation's semantics. `None` = they simulate on the CPU.
    fn particles(&self) -> Option<&dyn effectcraft_effects::psim::ParticleSim> {
        None
    }
    /// Timing history [`Backend::Auto`] uses to send each comp's top-level frames to the
    /// faster compositor. `None` = Auto always tries the accelerator first.
    fn auto_pick(&self) -> Option<&AutoPick> {
        None
    }
    /// Deferred readbacks: where a GPU readback cannot be waited for (WebGPU in a browser
    /// worker), a frame renders in *passes*. A readback the accelerator does not have yet is
    /// started, the step returns a cheap placeholder and the pass is marked missed
    /// ([`Self::pass_missed`]); the caller waits for the readbacks in flight (asynchronously)
    /// and renders the frame again, and the steps whose results arrived are served from them.
    /// The frame is done after a pass with no miss. `frame_begin` forgets the previous frame's
    /// results, `pass_begin` clears the miss. Synchronous accelerators never miss.
    fn frame_begin(&self) {}
    fn pass_begin(&self) {}
    fn pass_missed(&self) -> bool {
        false
    }
    /// Raised from a pass's first miss to its end (results computed meanwhile may hold
    /// placeholders): give it to [`LayerCache::set_gate`] so they are not cached.
    fn miss_gate(&self) -> Option<Arc<std::sync::atomic::AtomicBool>> {
        None
    }
    /// With deferred readbacks: a future that resolves once the readbacks in flight have
    /// landed (the browser delivers them from its event loop; natively the device is polled
    /// before it returns, so the future is ready). `None` = nothing to wait for. See
    /// [`passes::in_passes`].
    fn settle(&self) -> Option<std::pin::Pin<Box<dyn std::future::Future<Output = ()> + '_>>> {
        None
    }
}

/// Where an effect stack runs (see [`Renderer::run_effects_on`]): a CPU buffer, or an image
/// resident on an accelerator.
pub trait FxTarget {
    /// Run a chain of accelerator-supported effects (quantising after each to the bit depth).
    /// `false` = not handled: the effects then run one by one through [`FxTarget::cpu`].
    fn gpu(&mut self, steps: &[FxStep]) -> bool;
    /// Run one CPU effect: `f` maps the buffer.
    fn cpu(&mut self, f: &mut dyn FnMut(Buf) -> Buf);
}

/// The CPU effect target (GPU chains go through [`Accelerator::effects`]: upload, readback).
struct CpuFx<'a> {
    accel: Option<&'a dyn Accelerator>,
    levels: Option<f32>,
    buf: Option<Buf>,
}

impl FxTarget for CpuFx<'_> {
    fn gpu(&mut self, steps: &[FxStep]) -> bool {
        let (Some(a), Some(b)) = (self.accel, &self.buf) else { return false };
        match a.effects(steps, b, self.levels) {
            Some(out) => {
                self.buf = Some(out);
                true
            }
            None => false,
        }
    }
    fn cpu(&mut self, f: &mut dyn FnMut(Buf) -> Buf) {
        if let Some(b) = self.buf.take() {
            self.buf = Some(f(b));
        }
    }
}

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RenderOpts {
    /// Output scale relative to comp pixels (1 = Full, 0.5 = Half…).
    pub scale: f64,
    pub motion_blur: bool,
    /// Render guide layers (viewer only).
    pub guides: bool,
    /// Draft quality: fewer motion-blur samples, bilinear only.
    pub draft: bool,
    /// 3D view camera override (viewer Front/Top/Custom views); None = the comp's active camera.
    pub view: Option<three_d::CameraState>,
    /// CPU or GPU compositor (the GPU needs [`Renderer::accel`]).
    pub backend: Backend,
    /// Region of interest `[x, y, w, h]` in comp pixels (viewer only): the top-level frame covers
    /// just this rectangle, and only its pixels are composited.
    pub roi: Option<[f64; 4]>,
    /// Which proxies stand in for footage and compositions (Render Settings ▸ Proxy Use).
    pub proxy: effectcraft_project::render_queue::ProxyUse,
    /// Switches Affect Nested Comps (Settings ▸ General): a precomp layer's Quality and Motion
    /// Blur switches also limit the layers of the nested comp (Draft/Wireframe or motion blur off
    /// propagate down; they never raise a nested layer's own setting).
    pub nested_switches: bool,
    /// Realtime Shadows in Draft (Settings ▸ 3D): draft renders (`draft`) still cast shadows.
    pub draft_shadows: bool,
}

impl RenderOpts {
    /// Whether 3D layers cast shadows in this render.
    pub fn shadows(&self) -> bool {
        !self.draft || self.draft_shadows
    }
}

impl Default for RenderOpts {
    fn default() -> Self {
        RenderOpts {
            scale: 1.0,
            motion_blur: true,
            guides: false,
            draft: false,
            view: None,
            roi: None,
            backend: Backend::Cpu,
            nested_switches: true,
            draft_shadows: true,
            proxy: effectcraft_project::render_queue::ProxyUse::CurrentSettings,
        }
    }
}

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

/// One collapsed precomp layer a nested comp is drawn through.
#[derive(Clone, Copy)]
struct Through<'a> {
    comp_id: ItemId,
    comp: &'a Comp,
    /// The containing comp's time at this frame.
    time: Tick,
    layer: &'a Layer,
    /// The precomp layer is motion blurred: its own motion blurs the nested layers.
    blur: bool,
}

/// Collapsed precomps within collapsed precomps that motion blur follows (more than
/// [`Renderer::collapsed`]'s nesting limit).
const MAX_THROUGH: usize = 16;

/// A motion-blur shutter: the comp's, or for a collapsed precomp's layers the outermost
/// containing comp's (they are drawn into its frames).
#[derive(Clone, Copy)]
struct Shutter {
    /// Shutter angle and phase as fractions of a frame.
    angle: f64,
    phase: f64,
    /// Frame duration (seconds).
    frame: f64,
    /// Samples Per Frame and Adaptive Sample Limit.
    samples: u32,
    limit: u32,
}

impl Shutter {
    /// Seconds from the frame time to sub-sample `i` of `n`, spread over the open shutter.
    fn offset(&self, i: usize, n: usize) -> f64 {
        let f = if n > 1 { i as f64 / (n - 1) as f64 } else { 0.0 };
        (self.phase + self.angle * f) * self.frame
    }
}

/// How a collapsed precomp's 3D layers are placed in the parent (see `Renderer::collapse_into`).
#[derive(Clone, Copy)]
pub(crate) struct Collapse3d<'a> {
    /// Nested comp world → parent world (the precomp layer's world transform).
    pub world: Mat4,
    /// The parent's camera.
    pub cam: three_d::CameraState,
    /// The outermost comp's context (its lights light the nested layers).
    pub parent: EvalCtx<'a>,
}

/// Time spent on one layer of one frame (milliseconds).
#[derive(Clone, Debug, Default)]
pub struct LayerTiming {
    pub depth: usize,
    pub layer: String,
    /// Source + masks + effects (cache lookup only, on a hit).
    pub process_ms: f64,
    /// Per effect: (effect id, ms). Empty on a cache hit.
    pub effects: Vec<(String, f64)>,
    /// Transform + matte + blend.
    pub composite_ms: f64,
    /// The processed buffer came from the layer cache.
    pub cached: bool,
}

impl<'a> Renderer<'a> {
    pub fn new(project: &'a Project, footage: &'a dyn FootageSource, opts: RenderOpts) -> Renderer<'a> {
        Renderer {
            project,
            footage,
            expr: None,
            opts,
            cache: None,
            profile: None,
            accel: None,
            depth: 0,
            pipe: color::Pipe::of(&project.settings),
            outer: None,
            opacity_mul: 1.0,
            collapse3d: None,
            inherited: None,
            defer_dof: false,
            top: true,
            through: [None; MAX_THROUGH],
            through_lost: false,
        }
    }

    /// A layer's Quality switch, limited by the precomp layers above it when
    /// [`RenderOpts::nested_switches`] is on.
    pub fn quality(&self, layer: &Layer) -> Quality {
        let rank = |q: Quality| match q {
            Quality::Best => 0,
            Quality::Draft => 1,
            Quality::Wireframe => 2,
        };
        match self.inherited {
            Some((q, _)) if rank(q) > rank(layer.switches.quality) => q,
            _ => layer.switches.quality,
        }
    }

    /// A layer's Motion Blur switch, limited by the precomp layers above it.
    pub(crate) fn layer_motion_blur(&self, layer: &Layer) -> bool {
        layer.switches.motion_blur && self.inherited.is_none_or(|(_, mb)| mb)
    }

    /// The renderer for the nested comp of precomp layer `layer`: its switches pass down when
    /// [`RenderOpts::nested_switches`] is on.
    fn nested_through(&self, layer: &Layer) -> Renderer<'a> {
        let mut sub = self.nested();
        if self.opts.nested_switches {
            sub.inherited = Some((self.quality(layer), self.layer_motion_blur(layer)));
        }
        sub
    }

    /// A renderer for nested work (precomps, layers read by effects): one level deeper.
    fn nested(&self) -> Renderer<'a> {
        Renderer {
            depth: self.depth + 1,
            outer: None,
            opacity_mul: 1.0,
            collapse3d: None,
            defer_dof: false,
            top: false,
            through: [None; MAX_THROUGH],
            through_lost: false,
            ..*self
        }
    }

    /// The nested comp of a precomp layer whose transformations collapse into this comp: the
    /// Collapse Transformations switch is on and the layer has no masks, effects or layer
    /// styles (those force a flattened render of the precomp, as in After Effects).
    pub fn collapsed(&self, ctx: &EvalCtx, layer: &Layer) -> Option<ItemId> {
        if !layer.switches.collapse || layer.switches.adjustment || self.depth > 12 {
            return None;
        }
        let LayerSource::Comp { item } = &layer.source else { return None };
        if self.project.comp_contains(*item, ctx.comp_id) || self.project.comp(*item).is_none() {
            return None;
        }
        if layer.masks().is_some_and(|m| !m.children.is_empty()) {
            return None;
        }
        if layer.switches.effects && layer.effects().is_some_and(|fx| fx.groups().any(|g| g.enabled)) {
            return None;
        }
        if styles::active(ctx, layer) {
            return None;
        }
        // Instance overrides (Essential Properties) need the nested comp rendered on its own.
        if !effectcraft_project::essential::overridden(layer).is_empty() {
            return None;
        }
        Some(*item)
    }

    /// A renderer and context that draw a collapsed precomp's layers straight into this comp:
    /// their transforms are concatenated with the precomp layer's (one resample, no
    /// rasterisation at the precomp's bounds), their opacity multiplied by `opacity`, and their
    /// 3D layers use this comp's camera and lights.
    pub fn collapse_into(&self, ctx: &EvalCtx<'a>, layer: &'a Layer, item: ItemId, opacity: f32) -> Option<(Renderer<'a>, EvalCtx<'a>)> {
        let nc = self.project.comp(item)?;
        let t = ctx.nested_time(layer);
        if !nc.covers(t) {
            return None;
        }
        let nctx = EvalCtx { comp_id: item, comp: nc, time: t, ..*ctx };
        let (l2c, _) = ctx.layer_to_comp(layer);
        let outer = Some(self.outer.map_or(l2c, |o| o * l2c));
        let world = ctx.world_matrix(layer);
        let c3 = Collapse3d {
            world: self.collapse3d.map_or(world, |c| c.world * world),
            cam: three_d::compose::camera_for(self, ctx),
            parent: self.collapse3d.map_or(*ctx, |c| c.parent),
        };
        // Motion blur follows the precomp layer's own motion too.
        let link = Through { comp_id: ctx.comp_id, comp: ctx.comp, time: ctx.time, layer, blur: self.mb_on(ctx, layer) };
        let mut through = self.through;
        let slot = through.iter_mut().find(|t| t.is_none());
        let through_lost = self.through_lost || slot.is_none();
        if let Some(slot) = slot {
            *slot = Some(link);
        }
        Some((Renderer { depth: self.depth + 1, outer, opacity_mul: opacity, collapse3d: Some(c3), through, through_lost, ..*self }, nctx))
    }

    pub(crate) fn opacity_mul(&self) -> f32 {
        self.opacity_mul
    }

    /// Draw a collapsed 2D precomp layer: nested layers blend straight into `canvas` with their
    /// own modes (Normal precomp, no matte); otherwise they are drawn into an isolated buffer
    /// that then takes the precomp layer's track matte, Preserve Transparency and blend mode.
    /// Returns the region left to re-quantise (nested layers drawn straight into `canvas`
    /// quantise their own footprints).
    fn draw_collapsed(&self, ctx: &EvalCtx<'a>, layer: &Layer, item: ItemId, canvas: &mut Image, blank: bool) -> Region {
        let opacity = ctx.opacity(layer) as f32 * self.opacity_mul;
        if opacity <= 0.0 {
            return Region::Empty;
        }
        let matte = layer.track_matte.is_some_and(|tm| tm.layer != layer.id && ctx.comp.layer(tm.layer).is_some());
        if layer.blend_mode == BlendMode::Normal && !matte && !layer.preserve_transparency {
            if let Some((sub, nctx)) = self.collapse_into(ctx, layer, item, opacity) {
                sub.draw_comp(&nctx, canvas, blank);
            }
            return Region::Empty;
        }
        let Some((sub, nctx)) = self.collapse_into(ctx, layer, item, 1.0) else { return Region::Empty };
        let mut iso = Image::new(canvas.width, canvas.height);
        sub.draw_comp(&nctx, &mut iso, true);
        self.composite_iso(ctx, layer, iso, canvas, opacity);
        Region::Full
    }

    /// Rasterisation scale of a layer's source: the output scale, or — for text layers, which are
    /// always continuously rasterised, and Continuously Rasterize shape layers and vector footage —
    /// the output scale times the layer's on-screen scale (quarter-octave steps, rounded up, so
    /// vector content is drawn at its final size and stays sharp when scaled up).
    pub fn raster_scale(&self, ctx: &EvalCtx, layer: &Layer) -> f64 {
        let s = self.opts.scale;
        let continuous = match layer.source {
            LayerSource::Text => true,
            LayerSource::Shape => layer.switches.collapse,
            LayerSource::Footage { item } => {
                layer.switches.collapse && matches!(self.project.item(item).map(|i| &i.kind), Some(ItemKind::Footage(f)) if is_vector_footage(f))
            }
            _ => false,
        };
        if !continuous {
            return s;
        }
        let (l2c, _) = ctx.layer_to_comp(layer);
        let m = self.outer.map_or(l2c, |o| o * l2c);
        let a = layer.transform().map(|tr| ctx.v3(layer, tr, "anchor", [0.0; 3])).unwrap_or([0.0; 3]);
        let p0 = m.apply(vec2(a[0], a[1]));
        let px = m.apply(vec2(a[0] + 1.0, a[1]));
        let py = m.apply(vec2(a[0], a[1] + 1.0));
        let k = ((px.x - p0.x).hypot(px.y - p0.y)).max((py.x - p0.x).hypot(py.y - p0.y));
        if !k.is_finite() || k <= 1e-6 {
            return s;
        }
        // The step is rounded up past rounding noise only: an unscaled layer whose position isn't
        // a whole number maps the unit vectors to lengths a hair over 1, which must stay 1 (#390:
        // rounded up to 2^¼, unscaled text was drawn larger and resampled, softer).
        let mut k = 2f64.powf((k.log2() * 4.0 - 1e-6).ceil() / 4.0).clamp(1.0 / 16.0, 64.0);
        // Keep the buffer within about 16 million pixels.
        if let Some(b) = content_bounds(ctx, layer) {
            let area = ((b[2] - b[0]) * (b[3] - b[1])).max(1.0) * s * s;
            k = k.min((16.0e6 / area).sqrt().max(1.0));
        }
        s * k
    }

    /// Draw a Wireframe-quality layer: its bounds as a one-pixel outline.
    fn draw_wireframe(&self, ctx: &EvalCtx, layer: &Layer, canvas: &mut Image) {
        for (x, y) in self.wireframe_pixels(ctx, layer, (canvas.width, canvas.height)) {
            canvas.set(x, y, [1.0, 1.0, 1.0, 1.0]);
        }
    }

    fn ctx(&self, comp_id: ItemId, comp: &'a Comp, t: Tick) -> EvalCtx<'a> {
        EvalCtx { project: self.project, comp_id, comp, time: t, expr: self.expr, footage: Some(self.footage) }
    }

    /// This renderer with another accelerator (or none): the passes of
    /// [`passes::in_passes`] render with the accelerator, its CPU fallback without.
    pub fn with_accel(&self, accel: Option<&'a dyn Accelerator>) -> Renderer<'a> {
        Renderer { accel, ..*self }
    }

    /// The accelerator to use for this render, if any (see [`Backend`]).
    pub fn active_accel(&self) -> Option<&'a dyn Accelerator> {
        let a = self.accel?;
        match self.opts.backend {
            Backend::Cpu => None,
            Backend::Gpu => Some(a),
            Backend::Auto => self.project.settings.gpu_acceleration.then_some(a),
        }
    }

    /// Render a composition at comp time `t` (transparent background, comp size × scale).
    /// Top-level frames go to the GPU when [`Self::active_accel`] is set and handles them.
    pub fn comp_frame(&self, comp_id: ItemId, t: Tick) -> Image {
        // Classic 3D runs composite on the GPU too (M12.7), so Auto sends 3D comps there as well;
        // Auto renders each comp on whichever compositor measured faster ([`AutoPick`]).
        if self.depth == 0
            && let Some(a) = self.active_accel()
        {
            let auto = self.frame_auto(a, comp_id, false);
            if auto.is_none_or(|(p, key)| p.choose(key)) {
                let t0 = web_time::Instant::now();
                if let Some(img) = a.comp_frame(self, comp_id, t) {
                    if let Some((p, key)) = auto {
                        p.record(key, true, t0.elapsed().as_secs_f64() * 1e3);
                    }
                    return img;
                }
                if let Some((p, key)) = auto {
                    p.declined(key);
                }
            }
            let t0 = web_time::Instant::now();
            let img = self.comp_frame_cpu(comp_id, t);
            if let Some((p, key)) = auto {
                p.record(key, false, t0.elapsed().as_secs_f64() * 1e3);
            }
            return img;
        }
        self.comp_frame_cpu(comp_id, t)
    }

    /// The Auto timing history and key for a top-level frame of `comp` (`None` unless the
    /// backend is [`Backend::Auto`] and the accelerator keeps history).
    pub fn frame_auto(&self, a: &'a dyn Accelerator, comp: ItemId, display: bool) -> Option<(&'a AutoPick, AutoKey)> {
        if self.opts.backend != Backend::Auto || self.opts.roi.is_some() {
            return None;
        }
        Some((a.auto_pick()?, AutoKey::new(comp, self.opts.scale, display)))
    }

    /// [`Self::comp_frame`] on the CPU compositor only.
    pub fn comp_frame_cpu(&self, comp_id: ItemId, t: Tick) -> Image {
        let Some(comp) = self.project.comp(comp_id) else { return Image::new(1, 1) };
        let s = self.opts.scale;
        let w = ((comp.width as f64 * s).round() as u32).max(1);
        let h = ((comp.height as f64 * s).round() as u32).max(1);
        // Region of interest on a comp with 3D layers: render the frame and crop it. A region
        // reaching past the comp frame (the Extended Viewer) renders Classic 3D natively — the
        // planes project through the offset camera — and Advanced 3D as the frame, padded.
        if self.depth == 0
            && let Some(r) = self.opts.roi
            && comp.has_3d()
            && (roi_inside(r, comp.width, comp.height) || comp.renderer == effectcraft_project::Renderer::Advanced3D)
        {
            let full = Renderer { opts: RenderOpts { roi: None, ..self.opts }, ..*self }.comp_frame(comp_id, t);
            let (x0, y0) = ((r[0] * s).round() as i64, (r[1] * s).round() as i64);
            let (rw, rh) = (((r[2] * s).round() as u32).max(1), ((r[3] * s).round() as u32).max(1));
            let mut out = Image::new(rw, rh);
            for y in 0..rh as i64 {
                let sy = y + y0;
                if sy < 0 || sy >= full.height as i64 {
                    continue;
                }
                for x in 0..rw as i64 {
                    let sx = x + x0;
                    if sx >= 0 && sx < full.width as i64 {
                        out.data[(y * rw as i64 + x) as usize] = full.data[(sy * full.width as i64 + sx) as usize];
                    }
                }
            }
            return out;
        }
        let (w, h) = match self.roi_offset() {
            Some((_, _, rw, rh)) => (rw, rh),
            None => (w, h),
        };
        let mut canvas = Image::new(w, h);
        if self.depth > 16 {
            return canvas;
        }
        let ctx = self.ctx(comp_id, comp, t);
        self.draw_comp(&ctx, &mut canvas, true);
        // The canvas is already quantised after every layer; only re-encoding changes it.
        if let Some(c) = self.pipe.from_blend() {
            color::convert(&mut canvas, &c);
            self.pipe.quantize(&mut canvas);
        }
        // The top-level comp leaves the working space for the (sRGB) display / output.
        if self.depth == 0
            && let Some(c) = self.pipe.output()
        {
            color::convert(&mut canvas, &c);
            self.pipe.quantize(&mut canvas);
        }
        if self.depth == 0
            && let Some((lin, mode, enc)) = self.pipe.output_hdr()
        {
            color::output_hdr(&mut canvas, lin, mode, enc);
            self.pipe.quantize(&mut canvas);
        }
        canvas
    }

    /// Draw a comp's layers (bottom to top) into `canvas` (blending space).
    /// `blank`: the canvas is fully transparent (lets the first layer skip re-quantising).
    fn draw_comp(&self, ctx: &EvalCtx<'a>, canvas: &mut Image, mut blank: bool) {
        // Bottom-to-top, with runs of consecutive 3D layers depth-sorted (farthest first).
        let visible = self.visible_layers(ctx);
        let mut i = 0;
        while i < visible.len() {
            if visible[i].is_3d() {
                let mut j = i;
                while j < visible.len() && visible[j].is_3d() {
                    j += 1;
                }
                three_d::compose::draw_run(self, ctx, &visible[i..j], canvas);
                i = j;
                self.pipe.quantize(canvas);
                blank = false;
            } else {
                let changed = match self.collapsed(ctx, visible[i]) {
                    Some(item) => self.draw_collapsed(ctx, visible[i], item, canvas, blank),
                    None => self.draw_layer(ctx, visible[i], canvas, blank),
                };
                i += 1;
                // 8/16 bpc: the comp is an integer buffer after every layer (only the pixels the
                // layer touched need re-quantising).
                self.pipe.quantize_region(canvas, changed);
                blank = false;
            }
        }
    }

    /// Evaluate an effect instance's parameters.
    fn effect_params(&self, ctx: &EvalCtx, layer: &Layer, g: &effectcraft_project::PropGroup) -> Params {
        // Nested groups (Paint strokes, Puppet meshes and pins) are flattened too.
        effectcraft_effects::flatten_params(g, &mut |pr| ctx.value(layer, pr))
    }

    fn apply_effects(&self, ctx: &EvalCtx, layer: &Layer, buf: Buf, adjustment: bool) -> Buf {
        self.apply_effects_timed(ctx, layer, buf, adjustment, usize::MAX, None)
    }

    /// Run the first `limit` effects of the layer's stack (by position; disabled ones count) on
    /// `buf` (layer space, or the comp below for an adjustment layer).
    fn apply_effects_timed(&self, ctx: &EvalCtx, layer: &Layer, mut buf: Buf, adjustment: bool, limit: usize, timing: Option<&mut Vec<(String, f64)>>) -> Buf {
        // The comp below an adjustment layer is not in its layer space: it stays as it is.
        let origin = if adjustment { [0.0; 2] } else { ctx.effect_bounds(layer).1 };
        buf.rebase(origin);
        let mut out = self.effect_space_stack(ctx, layer, buf, adjustment, limit, timing, origin);
        out.rebase([-origin[0], -origin[1]]);
        out
    }

    /// [`Renderer::apply_effects_timed`] on a buffer already in effect space (measured from
    /// `origin`, in layer coordinates); the result stays in effect space.
    #[allow(clippy::too_many_arguments)]
    fn effect_space_stack(
        &self,
        ctx: &EvalCtx,
        layer: &Layer,
        buf: Buf,
        adjustment: bool,
        limit: usize,
        timing: Option<&mut Vec<(String, f64)>>,
        origin: [f64; 2],
    ) -> Buf {
        let mut t = CpuFx { accel: self.active_accel(), levels: self.pipe.levels, buf: Some(buf) };
        self.run_effect_stack(ctx, layer, adjustment, limit, timing, origin, &mut t);
        t.buf.unwrap_or_else(|| Buf { img: Image::new(1, 1), offset: [0.0; 2], scale: self.opts.scale })
    }

    /// Run the layer's video effect stack on `target` (in effect space, measured from `origin`
    /// in layer coordinates): runs of GPU-capable effects go to [`FxTarget::gpu`] (when an
    /// accelerator is active and supports them), the rest to [`FxTarget::cpu`] one by one
    /// (quantised to the bit depth after each).
    #[allow(clippy::too_many_arguments)]
    fn run_effect_stack(
        &self,
        ctx: &EvalCtx,
        layer: &Layer,
        adjustment: bool,
        limit: usize,
        mut timing: Option<&mut Vec<(String, f64)>>,
        origin: [f64; 2],
        target: &mut dyn FxTarget,
    ) {
        if !layer.switches.effects {
            return;
        }
        let Some(fx) = layer.effects() else { return };
        let lt = layer.layer_time(ctx.time);
        let layer_size = ctx.effect_bounds(layer).0;
        let mut mask_shapes = masks::shapes(ctx, layer);
        if origin != [0.0; 2] {
            for q in mask_shapes.iter_mut().flat_map(|m| m.points.iter_mut()) {
                *q = [q[0] - origin[0], q[1] - origin[1]];
            }
        }
        let host = FxHost { r: self, ctx, layer, origin, index: Default::default() };
        let env = EffectEnv {
            masks: &mask_shapes,
            host: Some(&host),
            comp_time: ctx.time.seconds(),
            frame_rate: ctx.comp.frame_rate.as_f64(),
            effect_index: 0,
            working_space: self.pipe.space,
            working_linear: self.pipe.linear,
            shutter: self.mb_on(ctx, layer).then_some((ctx.comp.shutter_angle, ctx.comp.shutter_phase, ctx.comp.motion_blur_samples)),
            layer_span: eval::layer_span(self.project, layer),
        };
        // Video effects in stack order (index, group, spec); disabled and audio effects skipped.
        let stack: Vec<(usize, &effectcraft_project::PropGroup, &'static effectcraft_effects::EffectSpec)> = fx
            .groups()
            .enumerate()
            .take_while(|(i, _)| *i < limit)
            .filter(|(_, g)| g.enabled)
            .filter_map(|(i, g)| match &g.kind {
                GroupKind::Effect { effect } => effectcraft_effects::find(effect).map(|s| (i, g, s)),
                _ => None,
            })
            .filter(|(_, _, s)| !effectcraft_effects::audio_fx::is_audio_effect(s.id))
            .collect();
        let accel = self.active_accel();
        let mut k = 0;
        while k < stack.len() {
            let (i, g, spec) = stack[k];
            // A run of GPU effects goes to the accelerator in one go.
            if let Some(a) = accel
                && spec.gpu
                && a.supports_effect(spec.id)
            {
                let run: Vec<_> = stack[k..].iter().take_while(|(_, _, s)| s.gpu && a.supports_effect(s.id)).collect();
                let params: Vec<Params> = run.iter().map(|(_, g, _)| self.effect_params(ctx, layer, g)).collect();
                let steps: Vec<FxStep> = run
                    .iter()
                    .zip(&params)
                    .map(|((i, g, s), params)| FxStep {
                        spec: s,
                        ctx: EffectCtx { params, time: lt.seconds(), layer_size, seed: g.uid as u32, adjustment, env: EffectEnv { effect_index: *i, ..env } },
                    })
                    .collect();
                let t0 = web_time::Instant::now();
                if target.gpu(&steps) {
                    if let Some(v) = timing.as_deref_mut() {
                        let ms = t0.elapsed().as_secs_f64() * 1e3 / run.len() as f64;
                        v.extend(run.iter().map(|(_, _, s)| (format!("{} (gpu)", s.id), ms)));
                    }
                    k += run.len();
                    continue;
                }
            }
            k += 1;
            let params = self.effect_params(ctx, layer, g);
            host.index.store(i, std::sync::atomic::Ordering::Relaxed);
            let env = EffectEnv { effect_index: i, ..env };
            let ectx = EffectCtx { params: &params, time: lt.seconds(), layer_size, seed: g.uid as u32, adjustment, env };
            let t0 = web_time::Instant::now();
            target.cpu(&mut |buf| {
                let mut buf = effectcraft_effects::apply(spec, &ectx, buf);
                // 8/16 bpc effects write integer pixels.
                self.pipe.quantize(&mut buf.img);
                buf
            });
            if let Some(v) = timing.as_deref_mut() {
                v.push((spec.id.to_string(), t0.elapsed().as_secs_f64() * 1e3));
            }
        }
    }

    /// Source pixels of a layer in layer space (before masks/effects).
    fn source(&self, ctx: &EvalCtx, layer: &Layer) -> Option<Buf> {
        let s = self.opts.scale;
        match &layer.source {
            LayerSource::Solid { item } => {
                let ItemKind::Solid(sol) = &self.project.item(*item)?.kind else { return None };
                let w = ((sol.width as f64 * s).ceil() as u32).max(1);
                let h = ((sol.height as f64 * s).ceil() as u32).max(1);
                if layer.switches.adjustment {
                    return Some(Buf { img: Image::filled(w, h, [1.0; 4]), offset: [0.0; 2], scale: s });
                }
                let mut c = sol.color;
                if let Some(conv) = self.pipe.authored_in() {
                    c = conv.apply(c);
                }
                let px = self.pipe.quantize_px([c[0], c[1], c[2], 1.0]);
                Some(Buf { img: Image::filled(w, h, px), offset: [0.0; 2], scale: s })
            }
            LayerSource::Comp { item } => {
                if self.project.comp_contains(*item, ctx.comp_id) {
                    return None;
                }
                // A composition proxy (a rendered still or movie) stands in for the comp.
                // Nothing before the nested comp starts or after it ends.
                let lt = ctx.nested_time(layer);
                if !self.project.comp(*item).is_some_and(|nc| nc.covers(lt)) {
                    return None;
                }
                if let Some(pf) = self.proxy_for(*item)
                    && let Some(nc) = self.project.comp(*item)
                {
                    let img = self.footage.frame(*item, pf, lt)?;
                    return Some(self.footage_buf(&img, pf, nc.width, nc.height));
                }
                // Essential Properties overrides render the nested comp with this instance's
                // values.
                let ov = essential_overrides(ctx, layer);
                let tmp = if ov.is_empty() { None } else { effectcraft_project::essential::with_overrides(self.project, *item, &ov) };
                // Preserve resolution when nested: drawn at full size even when this comp renders
                // smaller (the buffer's scale places it).
                let full = s < 1.0 && self.project.comp(*item).is_some_and(|nc| nc.preserve_resolution);
                let bs = if full { 1.0 } else { s };
                let base = self.nested_through(layer);
                let base = Renderer { opts: RenderOpts { scale: bs, ..base.opts }, ..base };
                let sub = match &tmp {
                    Some(p) => Renderer { project: p, ..base },
                    None => base,
                };
                // Frame blending between the nested comp's frames (time-stretched/remapped).
                let mode = frame_blend_mode(ctx, layer);
                if mode != FrameBlend::Off
                    && let Some(nc) = self.project.comp(*item)
                {
                    let (i, w) = frame_position(lt, nc.frame_rate);
                    if w > 1e-6 {
                        let a = sub.comp_frame(*item, nc.frame_rate.tick_of(i));
                        let b = sub.comp_frame(*item, nc.frame_rate.tick_of(i + 1));
                        return Some(Buf { img: blend_frames(mode, &a, &b, w as f32), offset: [0.0; 2], scale: bs });
                    }
                }
                Some(Buf { img: sub.comp_frame(*item, lt), offset: [0.0; 2], scale: bs })
            }
            LayerSource::Footage { item } => {
                let it = self.project.item(*item)?;
                let ItemKind::Footage(f) = &it.kind else { return None };
                if !f.has_video {
                    return None;
                }
                // Continuously rasterised vector footage: drawn at the on-screen scale.
                if layer.switches.collapse && is_vector_footage(f) {
                    let k = self.raster_scale(ctx, layer);
                    if let Some(img) = self.footage.vector_frame(*item, f, k) {
                        let mut buf = Buf { img: (*img).clone(), offset: [0.0; 2], scale: k };
                        if let Some(c) = self.pipe.media_in(f) {
                            color::convert(&mut buf.img, &c);
                        }
                        return Some(buf);
                    }
                }
                let lt = ctx.source_time(layer);
                // The proxy is decoded instead, at its own size, and fills the footage's frame.
                let pf = self.proxy_for(*item).unwrap_or(f);
                let img = self.footage_frame(ctx, layer, *item, pf, lt)?;
                Some(self.footage_buf(&img, pf, f.width, f.height))
            }
            LayerSource::Text => Some(self.authored(text::render(ctx, layer, self.raster_scale(ctx, layer)))),
            LayerSource::Shape => layer.props.sub("contents").map(|c| self.authored(shapes::render(ctx, layer, c, self.raster_scale(ctx, layer)))),
            _ => None,
        }
    }

    /// The proxy footage used for `item` under this render's Proxy Use, if any.
    pub fn proxy_for(&self, item: ItemId) -> Option<&'a Footage> {
        let it = self.project.item(item)?;
        let px = it.proxy.as_deref()?;
        (self.opts.proxy.uses(matches!(it.kind, ItemKind::Comp(_)), px.enabled) && px.footage.has_video).then_some(&px.footage)
    }

    /// A decoded frame of `f` as a layer buffer covering `w`×`h` layer pixels (the footage's
    /// nominal size; proxies are scaled up to it), converted from its colour profile.
    fn footage_buf(&self, img: &Image, f: &Footage, w: u32, h: u32) -> Buf {
        let s = self.opts.scale;
        let k = if img.width > 0 { w.max(1) as f64 / img.width as f64 } else { 1.0 };
        // Keep native pixels when downsampling is small; resample otherwise.
        let mut buf = if s < 0.75 && k <= 1.0 + 1e-9 {
            let rw = ((w as f64 * s).round() as u32).max(1);
            let rh = ((h as f64 * s).round() as u32).max(1);
            Buf { img: effectcraft_raster::resample(img, rw, rh), offset: [0.0; 2], scale: s }
        } else {
            Buf { img: img.clone(), offset: [0.0; 2], scale: 1.0 / k }
        };
        if let Some(c) = self.pipe.media_in(f) {
            color::convert(&mut buf.img, &c);
        }
        buf
    }

    /// Authored colours into the working space (linear working spaces).
    fn authored(&self, mut buf: Buf) -> Buf {
        if let Some(c) = self.pipe.authored_in() {
            color::convert(&mut buf.img, &c);
        }
        buf
    }

    /// A footage frame at source time `t`, frame-blended between the two nearest source frames
    /// when the layer's Frame Blending switch (and the comp's Enable Frame Blending) is on and
    /// `t` falls between frames (footage rate ≠ comp rate, time stretch, time remapping).
    fn footage_frame(&self, ctx: &EvalCtx, layer: &Layer, item: ItemId, f: &Footage, t: Tick) -> Option<Arc<Image>> {
        // Interpret Footage ▸ Separate Fields: each field is a frame at twice the rate.
        if f.fields != effectcraft_project::FieldOrder::Off && matches!(f.kind, FootageKind::Video | FootageKind::Sequence) {
            let field_rate = FrameRate::new(f.frame_rate.num * 2, f.frame_rate.den);
            let i = field_rate.frame_at(t).max(0);
            let img = self.footage.frame(item, f, f.frame_rate.tick_of(i / 2))?;
            let dominant_upper = f.fields == effectcraft_project::FieldOrder::UpperFirst;
            // Upper field = even lines (0, 2, …).
            let parity = if (i % 2 == 0) == dominant_upper { 0 } else { 1 };
            return Some(Arc::new(interpret_pixels(&field_frame(&img, parity), f)));
        }
        let img = self.footage_frame_raw(ctx, layer, item, f, t)?;
        if f.invert_alpha || f.linear_light_applies() { Some(Arc::new(interpret_pixels(&img, f))) } else { Some(img) }
    }

    fn footage_frame_raw(&self, ctx: &EvalCtx, layer: &Layer, item: ItemId, f: &Footage, t: Tick) -> Option<Arc<Image>> {
        let mode = frame_blend_mode(ctx, layer);
        if mode != FrameBlend::Off && matches!(f.kind, FootageKind::Video | FootageKind::Sequence) {
            let (i, w) = frame_position(t, f.frame_rate);
            if w > 1e-6 {
                // Blended frames are cached (Pixel Motion is expensive).
                let key = self.cache.map(|_| {
                    use std::hash::{Hash, Hasher};
                    let mut h = std::collections::hash_map::DefaultHasher::new();
                    (0x00f8_a3e5_u64, item, i, (w as f32).to_bits(), mode as u8, f.frame_rate.num, f.frame_rate.den, &f.path).hash(&mut h);
                    h.finish()
                });
                if let (Some(c), Some(k)) = (self.cache, key)
                    && let Some(b) = c.get(k)
                {
                    return Some(Arc::new(b.img.clone()));
                }
                let a = self.footage.frame(item, f, f.frame_rate.tick_of(i));
                let b = self.footage.frame(item, f, f.frame_rate.tick_of(i + 1));
                if let (Some(a), Some(b)) = (a, b) {
                    let img = blend_frames(mode, &a, &b, w as f32);
                    if let (Some(c), Some(k)) = (self.cache, key) {
                        c.insert(k, Arc::new(Buf { img: img.clone(), offset: [0.0; 2], scale: 1.0 }));
                    }
                    return Some(Arc::new(img));
                }
            }
        }
        self.footage.frame(item, f, t)
    }

    /// Source → masks, clamped/quantised to the project depth.
    fn masked_source(&self, ctx: &EvalCtx, layer: &Layer) -> Option<Buf> {
        let mut buf = match self.content_times(ctx, layer) {
            Some(times) => self.blurred_source(ctx, layer, &times)?,
            None => self.source(ctx, layer)?,
        };
        // A solid is filled with a quantised colour: only masks can take it off the grid.
        let solid = matches!(layer.source, LayerSource::Solid { .. });
        if masks::apply(ctx, layer, &mut buf, self.mask_blur(ctx, layer)) || !solid {
            self.pipe.quantize(&mut buf.img);
        }
        Some(buf)
    }

    /// Fully processed layer buffer (source → masks → effects → layer styles, flattened).
    /// Served from the layer cache when the layer's content key matches.
    pub fn layer_buf(&self, ctx: &EvalCtx, layer: &Layer) -> Option<Arc<Buf>> {
        self.layer_buf_keyed(ctx, layer).map(|(b, _)| b)
    }

    /// [`Self::layer_buf`] in the blending space (linear with Blend Colors Using 1.0 Gamma).
    pub fn blend_layer_buf(&self, ctx: &EvalCtx, layer: &Layer) -> Option<Arc<Buf>> {
        let (b, k) = self.layer_buf_keyed(ctx, layer)?;
        Some(self.to_blend(b, k))
    }

    /// A buffer converted to the blending space (cached under a key derived from `key`).
    pub(crate) fn to_blend(&self, b: Arc<Buf>, key: Option<u64>) -> Arc<Buf> {
        let Some(c) = self.pipe.to_blend() else { return b };
        let key = key.map(|k| cache::derive(k, 0x1ea7));
        if let (Some(cache), Some(k)) = (self.cache, key)
            && let Some(x) = cache.get(k)
        {
            return x;
        }
        let mut nb = (*b).clone();
        color::convert(&mut nb.img, &c);
        let nb = Arc::new(nb);
        if let (Some(cache), Some(k)) = (self.cache, key) {
            cache.insert(k, nb.clone());
        }
        nb
    }

    fn layer_buf_keyed(&self, ctx: &EvalCtx, layer: &Layer) -> Option<(Arc<Buf>, Option<u64>)> {
        let st = self.styled(ctx, layer, None)?;
        if st.passes.is_empty() {
            return Some((st.body, st.key));
        }
        let key = st.key.map(|k| cache::derive(k, 0xf1a7));
        if let (Some(c), Some(k)) = (self.cache, key)
            && let Some(b) = c.get(k)
        {
            return Some((b, key));
        }
        let flat = Arc::new(styles::Styled { passes: st.passes.iter().map(|(b, m)| ((**b).clone(), *m)).collect(), body: (*st.body).clone() }.flatten());
        if let (Some(c), Some(k)) = (self.cache, key) {
            c.insert(k, flat.clone());
        }
        Some((flat, key))
    }

    /// A layer's source pixels at the context time, before masks, effects and the transform
    /// (what the motion tracker analyses, like After Effects' Layer panel). `None` for layers
    /// without pixels.
    pub fn layer_source(&self, ctx: &EvalCtx, layer: &Layer) -> Option<Buf> {
        self.source(ctx, layer)
    }

    /// Layer pixels after source → masks → effects (no layer styles).
    pub fn content_buf(&self, ctx: &EvalCtx, layer: &Layer) -> Option<Arc<Buf>> {
        self.content_buf_timed(ctx, layer, None).map(|(b, _)| b)
    }

    fn content_buf_timed(&self, ctx: &EvalCtx, layer: &Layer, mut timing: Option<&mut LayerTiming>) -> Option<(Arc<Buf>, Option<u64>)> {
        let key = self.cache.and_then(|_| cache::layer_key(ctx, layer, self.raster_scale(ctx, layer), self.opts.draft, self.mb_on(ctx, layer)));
        let key = self.content_key(ctx, layer, key);
        if let (Some(c), Some(k)) = (self.cache, key)
            && let Some(b) = c.get(k)
        {
            if let Some(t) = timing.as_deref_mut() {
                t.cached = true;
            }
            return Some((b, key));
        }
        let buf = self.masked_source(ctx, layer)?;
        let buf = Arc::new(self.apply_effects_timed(ctx, layer, buf, false, usize::MAX, timing.map(|t| &mut t.effects)));
        if let (Some(c), Some(k)) = (self.cache, key) {
            c.insert(k, buf.clone());
        }
        Some((buf, key))
    }

    /// A layer's input at the context time: source → masks → its first `effects` effects
    /// (what Time effects read at neighbouring times), in effect space (see [`effect_bounds`]):
    /// what effect `effects` sees. Served from / stored in the layer cache under its own key,
    /// so scrubbing reuses frames rendered for earlier output frames.
    pub fn layer_input(&self, ctx: &EvalCtx, layer: &Layer, effects: usize) -> Option<Arc<Buf>> {
        let key = self.cache.and_then(|_| cache::input_key(ctx, layer, self.raster_scale(ctx, layer), self.opts.draft, self.mb_on(ctx, layer), effects));
        let key = self.content_key(ctx, layer, key);
        if let (Some(c), Some(k)) = (self.cache, key)
            && let Some(b) = c.get(k)
        {
            return Some(b);
        }
        let mut buf = self.masked_source(ctx, layer)?;
        let origin = ctx.effect_bounds(layer).1;
        buf.rebase(origin);
        if effects > 0 {
            buf = self.effect_space_stack(ctx, layer, buf, false, effects, None, origin);
        }
        let buf = Arc::new(buf);
        if let (Some(c), Some(k)) = (self.cache, key) {
            c.insert(k, buf.clone());
        }
        Some(buf)
    }

    /// The layer with its styles: exterior passes and body (cached per pass).
    fn styled(&self, ctx: &EvalCtx, layer: &Layer, mut timing: Option<&mut LayerTiming>) -> Option<StyledLayer> {
        let (content, ckey) = self.content_buf_timed(ctx, layer, timing.as_deref_mut())?;
        if !styles::active(ctx, layer) {
            return Some(StyledLayer { passes: vec![], body: content.clone(), content, key: ckey, ckey, plain: true });
        }
        let modes = styles::pass_modes(ctx, layer);
        let key = ckey.map(|k| cache::styles_key(ctx, layer, k));
        if let (Some(c), Some(k)) = (self.cache, key)
            && let Some(body) = c.get(k)
        {
            let passes: Option<Vec<_>> = modes.iter().enumerate().map(|(i, m)| c.get(cache::derive(k, i as u64 + 1)).map(|b| (b, *m))).collect();
            if let Some(passes) = passes {
                return Some(StyledLayer { passes, body, content, key, ckey, plain: false });
            }
        }
        let t0 = web_time::Instant::now();
        let size = source_size(self.project, layer);
        let layer_size = if size.0 == 0 { [ctx.comp.width as f64, ctx.comp.height as f64] } else { [size.0 as f64, size.1 as f64] };
        let st = styles::render(ctx, layer, &content, layer_size);
        if let Some(t) = timing {
            t.cached = false;
            t.effects.push(("layerStyles".into(), t0.elapsed().as_secs_f64() * 1e3));
        }
        let body = Arc::new(st.body);
        let passes: Vec<_> = st.passes.into_iter().map(|(b, m)| (Arc::new(b), m)).collect();
        if let (Some(c), Some(k)) = (self.cache, key) {
            c.insert(k, body.clone());
            for (i, (b, _)) in passes.iter().enumerate() {
                c.insert(cache::derive(k, i as u64 + 1), b.clone());
            }
        }
        Some(StyledLayer { passes, body, content, key, ckey, plain: false })
    }

    /// The styled layer's buffers in the blending space.
    fn styled_to_blend(&self, st: StyledLayer) -> StyledLayer {
        if self.pipe.to_blend().is_none() {
            return st;
        }
        let body = self.to_blend(st.body, st.key);
        let content = if st.plain { body.clone() } else { self.to_blend(st.content, st.ckey) };
        let passes = st.passes.into_iter().enumerate().map(|(i, (b, m))| (self.to_blend(b, st.key.map(|k| cache::derive(k, i as u64 + 1))), m)).collect();
        StyledLayer { passes, body, content, ..st }
    }

    fn sampling(&self, layer: &Layer) -> effectcraft_raster::Sampling {
        if self.quality(layer) == Quality::Draft {
            // Draft quality: no interpolation (nearest neighbour), as After Effects' Draft.
            effectcraft_raster::Sampling::Nearest
        } else if self.opts.draft {
            effectcraft_raster::Sampling::Bilinear
        } else if layer.switches.sampling == Sampling::Bicubic {
            effectcraft_raster::Sampling::Bicubic
        } else {
            effectcraft_raster::Sampling::Bilinear
        }
    }

    /// The region of interest of a top-level 2D frame: (x, y) offset of the output in output
    /// pixels and its size. `None` renders the whole comp (no ROI, or a nested comp).
    pub(crate) fn roi_offset(&self) -> Option<(f64, f64, u32, u32)> {
        let r = self.opts.roi.filter(|_| self.top)?;
        let s = self.opts.scale;
        if r[2] <= 0.0 || r[3] <= 0.0 {
            return None;
        }
        Some(((r[0] * s).round(), (r[1] * s).round(), ((r[2] * s).round() as u32).max(1), ((r[3] * s).round() as u32).max(1)))
    }

    /// The region of interest's offset in output pixels for 3D projection (0 without one).
    pub(crate) fn out_offset(&self) -> (f64, f64) {
        self.roi_offset().map_or((0.0, 0.0), |(x, y, _, _)| (x, y))
    }

    /// Scaled comp pixel → output pixel: the region of interest's offset.
    fn out_matrix(&self) -> Mat3 {
        match self.roi_offset() {
            Some((x, y, _, _)) => Mat3::translate(vec2(-x, -y)),
            None => Mat3::IDENTITY,
        }
    }

    /// Buffer pixel → output pixel matrix for the layer at the context time.
    fn buf_matrix(&self, ctx: &EvalCtx, layer: &Layer, buf: &Buf) -> Mat3 {
        let (l2c, _) = ctx.layer_to_comp(layer);
        self.place_matrix(self.outer.map_or(l2c, |o| o * l2c), buf)
    }

    /// Buffer pixel → output pixel matrix for a layer → (top-level) comp matrix.
    fn place_matrix(&self, l2c: Mat3, buf: &Buf) -> Mat3 {
        let s = self.opts.scale;
        self.out_matrix()
            * Mat3::scale(vec2(s, s))
            * l2c
            * Mat3::scale(vec2(1.0 / buf.scale, 1.0 / buf.scale))
            * Mat3::translate(vec2(-buf.offset[0], -buf.offset[1]))
    }

    /// The collapsed precomp layers this comp is drawn through, outermost first.
    fn links(&self) -> impl Iterator<Item = &Through<'a>> {
        self.through.iter().map_while(Option::as_ref)
    }

    /// The shutter of a layer's motion blur: its comp's, or for a collapsed precomp's layers the
    /// outermost containing comp's.
    fn shutter(&self, ctx: &EvalCtx) -> Shutter {
        let c: &Comp = match self.links().next() {
            Some(l) => l.comp,
            None => ctx.comp,
        };
        Shutter {
            angle: c.shutter_angle / 360.0,
            phase: c.shutter_phase / 360.0,
            frame: c.frame_duration().seconds(),
            samples: c.motion_blur_samples,
            limit: c.motion_blur_adaptive_limit,
        }
    }

    /// Whether a layer moves within the shutter: its own motion blur, or that of a collapsed
    /// precomp layer it is drawn through.
    fn blurred(&self, ctx: &EvalCtx, layer: &Layer) -> bool {
        self.mb_on(ctx, layer) || (!self.through_lost && self.links().any(|l| l.blur))
    }

    /// The collapsed precomps' matrix (nested comp → the frame's comp) and the nested comp's time
    /// `dt` seconds into the outermost comp's shutter: each precomp layer moves if it is motion
    /// blurred, and times map down through each one (stretch, time remapping).
    fn outer_at(&self, ctx: &EvalCtx, dt: f64) -> (Option<Mat3>, Tick) {
        let shift = Tick::from_seconds_f64(dt);
        let mut outer: Option<Mat3> = None;
        let mut t: Option<Tick> = None;
        for l in self.links() {
            let at = EvalCtx { comp_id: l.comp_id, comp: l.comp, time: t.unwrap_or(l.time + shift), ..*ctx };
            let (m, _) = if l.blur { at.layer_to_comp(l.layer) } else { EvalCtx { time: l.time, ..at }.layer_to_comp(l.layer) };
            outer = Some(outer.map_or(m, |o| o * m));
            t = Some(at.nested_time(l.layer));
        }
        (outer, t.unwrap_or(ctx.time + shift))
    }

    /// [`Self::buf_matrix`] for the motion-blur sub-sample `dt` seconds into the shutter: the
    /// layer moves if it is motion blurred, the collapsed precomps above it if theirs are.
    fn buf_matrix_at(&self, ctx: &EvalCtx, layer: &Layer, buf: &Buf, dt: f64) -> Mat3 {
        if self.through_lost || self.links().next().is_none() {
            return self.buf_matrix(&ctx.at(ctx.time + Tick::from_seconds_f64(dt)), layer, buf);
        }
        let (outer, t) = self.outer_at(ctx, dt);
        let lctx = if self.mb_on(ctx, layer) { ctx.at(t) } else { *ctx };
        let (l2c, _) = lctx.layer_to_comp(layer);
        self.place_matrix(outer.map_or(l2c, |o| o * l2c), buf)
    }

    /// Shape and text content that animates within the shutter (paths, Trim Paths, text
    /// animators…) of a motion-blurred layer: the times its source is drawn at and averaged,
    /// Samples Per Frame of them, as After Effects does for shape layers. `None` when the content
    /// holds still (compared through its cache key, which leaves the transform out).
    fn content_times(&self, ctx: &EvalCtx, layer: &Layer) -> Option<Vec<Tick>> {
        if !matches!(layer.source, LayerSource::Shape | LayerSource::Text) || !self.mb_on(ctx, layer) {
            return None;
        }
        let sh = self.shutter(ctx);
        if sh.angle <= 0.0 {
            return None;
        }
        let n = if self.opts.draft { 4 } else { (sh.samples as usize).clamp(2, 64) };
        let times: Vec<Tick> = (0..n).map(|i| self.outer_at(ctx, sh.offset(i, n)).1).collect();
        let key = |t: Tick| cache::layer_key(&ctx.at(t), layer, 1.0, false, false);
        let k0 = key(*times.first()?)?;
        let moves = [times.get(n / 2), times.last()].into_iter().flatten().any(|t| key(*t) != Some(k0));
        moves.then_some(times)
    }

    /// The layer's source drawn at each of `times` and averaged (premultiplied) on the union of
    /// their bounds. Content that also changes its raster scale within the shutter, or would need
    /// an outsized buffer, is drawn once.
    fn blurred_source(&self, ctx: &EvalCtx, layer: &Layer, times: &[Tick]) -> Option<Buf> {
        let bufs: Vec<Buf> = times.iter().filter_map(|t| self.source(&ctx.at(*t), layer)).collect();
        let scale = bufs.first()?.scale;
        // Pixel = layer point × scale + offset: with the largest offset every buffer lands at
        // x + (off - its offset) ≥ 0.
        let off = [0, 1].map(|i| bufs.iter().map(|b| b.offset[i]).fold(f64::MIN, f64::max));
        let w = bufs.iter().map(|b| off[0] - b.offset[0] + b.img.width as f64).fold(0.0, f64::max).ceil();
        let h = bufs.iter().map(|b| off[1] - b.offset[1] + b.img.height as f64).fold(0.0, f64::max).ceil();
        if bufs.iter().any(|b| (b.scale - scale).abs() > 1e-9) || !(w * h).is_finite() || w * h > (1u64 << 26) as f64 {
            return self.source(ctx, layer);
        }
        let mut acc = Image::new(w as u32, h as u32);
        let k = 1.0 / bufs.len() as f32;
        for b in &bufs {
            let m = Mat3::translate(vec2(off[0] - b.offset[0], off[1] - b.offset[1]));
            effectcraft_raster::accumulate_warp(&mut acc, &b.img, &m, effectcraft_raster::Sampling::Bilinear, k);
        }
        Some(Buf { img: acc, offset: off, scale })
    }

    /// A processed-layer cache key that also covers content motion blur (the averaged source
    /// depends on the content at every sub-sample).
    fn content_key(&self, ctx: &EvalCtx, layer: &Layer, key: Option<u64>) -> Option<u64> {
        let k = key?;
        let Some(times) = self.content_times(ctx, layer) else { return Some(k) };
        times.iter().try_fold(cache::derive(k, 0x6d62_6c72), |d, t| Some(cache::derive(d, cache::layer_key(&ctx.at(*t), layer, 1.0, false, false)?)))
    }

    /// Whether a layer is motion blurred: the render options, the comp's switch and the layer's
    /// switch (limited by the precomp layers above when switches affect nested comps). Layer
    /// motion, mask motion blur, effects that read the shutter and Advanced 3D all ask this.
    pub(crate) fn mb_on(&self, ctx: &EvalCtx, layer: &Layer) -> bool {
        self.opts.motion_blur && ctx.comp.enable_motion_blur && self.layer_motion_blur(layer)
    }

    /// What a layer's mask motion blur may do in this render (see [`masks::MaskBlur`]).
    fn mask_blur(&self, ctx: &EvalCtx, layer: &Layer) -> masks::MaskBlur {
        masks::MaskBlur {
            allowed: self.opts.motion_blur && ctx.comp.enable_motion_blur && self.inherited.is_none_or(|(_, mb)| mb),
            layer: self.layer_motion_blur(layer),
            samples: if self.opts.draft { 4 } else { ctx.comp.motion_blur_samples as usize },
        }
    }

    /// Motion-blur sub-samples for a layer buffer (1 = no motion blur). As in After Effects, a 2D
    /// layer gets about one sample per output pixel its corners travel during the shutter,
    /// at least Samples Per Frame and at most the comp's Adaptive Sample Limit; a layer that
    /// does not move within the shutter is drawn once.
    fn mb_samples(&self, ctx: &EvalCtx, layer: &Layer, buf: &Buf) -> usize {
        if !self.blurred(ctx, layer) {
            return 1;
        }
        let travel = self.shutter_travel(ctx, layer, buf);
        if travel < 1e-3 {
            return 1;
        }
        if self.opts.draft {
            return 4;
        }
        let sh = self.shutter(ctx);
        adaptive_samples(travel, sh.samples, sh.limit)
    }

    /// Largest distance (output pixels) a corner of the layer buffer moves during the shutter,
    /// measured along the path at a few points.
    fn shutter_travel(&self, ctx: &EvalCtx, layer: &Layer, buf: &Buf) -> f64 {
        let sh = self.shutter(ctx);
        let (w, h) = (buf.img.width as f64, buf.img.height as f64);
        let corners = [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h), (w * 0.5, h * 0.5)];
        const PROBES: usize = 8;
        let mut prev: Option<Vec<effectcraft_geom::Vec2>> = None;
        let mut travel = vec![0.0f64; corners.len()];
        for i in 0..PROBES {
            let m = self.buf_matrix_at(ctx, layer, buf, sh.offset(i, PROBES));
            let pts: Vec<_> = corners.iter().map(|&(x, y)| m.apply(vec2(x, y))).collect();
            if let Some(p) = &prev {
                for (k, (a, b)) in p.iter().zip(&pts).enumerate() {
                    travel[k] += (b.x - a.x).hypot(b.y - a.y);
                }
            }
            prev = Some(pts);
        }
        travel.into_iter().fold(0.0, f64::max)
    }

    /// Canvas pixels that compositing `buf` for `layer` can change: the transformed buffer's
    /// bounds plus the resampling footprint. Stencil/silhouette modes, motion blur and
    /// perspective fall back to the whole canvas.
    ///
    /// A quantised buffer placed pixel-exactly (integer translation, Normal, 100%) over a blank
    /// canvas leaves exactly its own (quantised) pixels: nothing to re-quantise.
    fn footprint(&self, ctx: &EvalCtx, layer: &Layer, buf: &Buf, opacity: f32, blank: bool) -> Region {
        if layer.blend_mode.is_stencil() || self.mb_samples(ctx, layer, buf) > 1 {
            return Region::Full;
        }
        let m = self.buf_matrix(ctx, layer, buf);
        if !m.is_affine() {
            return Region::Full;
        }
        let matte = layer.track_matte.is_some_and(|tm| tm.layer != layer.id && ctx.comp.layer(tm.layer).is_some());
        let k = &m.0;
        let integral = |v: f64| v == v.round();
        if blank
            && layer.blend_mode == BlendMode::Normal
            && opacity == 1.0
            && !matte
            && !layer.preserve_transparency
            && self.pipe.to_blend().is_none()
            && k[0][0] == 1.0
            && k[1][1] == 1.0
            && k[0][1] == 0.0
            && k[1][0] == 0.0
            && integral(k[0][2])
            && integral(k[1][2])
        {
            return Region::Empty;
        }
        let (w, h) = (buf.img.width as f64, buf.img.height as f64);
        let c = [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)].map(|(x, y)| m.apply(vec2(x, y)));
        let (x0, x1) = (c.iter().map(|p| p.x).fold(f64::INFINITY, f64::min), c.iter().map(|p| p.x).fold(f64::NEG_INFINITY, f64::max));
        let (y0, y1) = (c.iter().map(|p| p.y).fold(f64::INFINITY, f64::min), c.iter().map(|p| p.y).fold(f64::NEG_INFINITY, f64::max));
        color::PxRect::covering(x0, y0, x1, y1, 3.0).map_or(Region::Full, Region::Rect)
    }

    /// How a processed layer buffer lands in the output: one buffer → output matrix, or one per
    /// motion-blur sub-sample (averaged), with the layer's sampling filter and dissolve seed.
    pub fn placement(&self, ctx: &EvalCtx, layer: &Layer, buf: &Buf) -> Placement {
        let samples = self.mb_samples(ctx, layer, buf);
        let (sampling, seed) = (self.sampling(layer), layer.id.0 as u32);
        if samples <= 1 {
            return Placement { matrices: vec![self.buf_matrix(ctx, layer, buf)], sampling, seed };
        }
        let sh = self.shutter(ctx);
        let matrices = (0..samples).map(|i| self.buf_matrix_at(ctx, layer, buf, sh.offset(i, samples))).collect();
        Placement { matrices, sampling, seed }
    }

    /// Draw a processed layer into `target` (transform, motion blur) with `mode`/`opacity`.
    fn place(&self, ctx: &EvalCtx, layer: &Layer, buf: &Buf, target: &mut Image, mode: BlendMode, opacity: f32) {
        let pl = self.placement(ctx, layer, buf);
        let opts = WarpOpts { sampling: pl.sampling, opacity, mode, seed: pl.seed, clip: None };
        if let [m] = pl.matrices.as_slice() {
            composite_warp(target, &buf.img, m, &opts);
            return;
        }
        // Sub-samples sum straight into one buffer (each warp is row-parallel).
        let mut acc = Image::new(target.width, target.height);
        let k = 1.0 / pl.matrices.len() as f32;
        for m in &pl.matrices {
            effectcraft_raster::accumulate_warp(&mut acc, &buf.img, m, opts.sampling, k);
        }
        target.blend_from(&acc, mode, opacity, pl.seed);
    }

    /// Draw a 2D layer into `canvas`; returns the region it may have changed.
    fn draw_layer(&self, ctx: &EvalCtx, layer: &Layer, canvas: &mut Image, blank: bool) -> Region {
        let opacity = ctx.opacity(layer) as f32 * self.opacity_mul;
        if self.quality(layer) == Quality::Wireframe && !layer.switches.adjustment {
            self.draw_wireframe(ctx, layer, canvas);
            return Region::Full;
        }
        if opacity <= 0.0 && !layer.switches.adjustment {
            return Region::Empty;
        }
        if layer.switches.adjustment {
            self.draw_adjustment(ctx, layer, canvas, opacity);
            return Region::Full;
        }
        let mut timing = self.profile.map(|_| LayerTiming { depth: self.depth, layer: layer.name.clone(), ..Default::default() });
        let t0 = web_time::Instant::now();
        let Some(st) = self.styled(ctx, layer, timing.as_mut()) else { return Region::Empty };
        let st = self.styled_to_blend(st);
        let t1 = web_time::Instant::now();
        let changed = if st.plain {
            self.composite_layer(ctx, layer, &st.body, canvas, opacity);
            self.footprint(ctx, layer, &st.body, opacity, blank)
        } else {
            self.composite_styled(ctx, layer, &st, canvas, opacity);
            Region::Full
        };
        if let (Some(prof), Some(mut timing)) = (self.profile, timing) {
            timing.process_ms = (t1 - t0).as_secs_f64() * 1e3;
            timing.composite_ms = t1.elapsed().as_secs_f64() * 1e3;
            if let Ok(mut v) = prof.lock() {
                v.push(timing);
            }
        }
        changed
    }

    /// Composite a styled layer: exterior passes with their own modes, then the body with the
    /// layer's mode; the layer's opacity fades the whole stack. Knockout clears what is below the
    /// layer's content; switched-off R/G/B channels keep the values below.
    fn composite_styled(&self, ctx: &EvalCtx, layer: &Layer, st: &StyledLayer, canvas: &mut Image, opacity: f32) {
        let bl = styles::blending(ctx, layer);
        let matte = layer.track_matte.is_some_and(|tm| tm.layer != layer.id && ctx.comp.layer(tm.layer).is_some());
        if matte || layer.preserve_transparency {
            let mut iso = Image::new(canvas.width, canvas.height);
            for (b, m) in &st.passes {
                self.place(ctx, layer, b, &mut iso, *m, 1.0);
            }
            self.place(ctx, layer, &st.body, &mut iso, BlendMode::Normal, 1.0);
            self.composite_iso(ctx, layer, iso, canvas, opacity);
            return;
        }
        let mut tmp = canvas.clone();
        if bl.knockout > 0 {
            let mut k = Image::new(canvas.width, canvas.height);
            self.place(ctx, layer, &st.content, &mut k, BlendMode::Normal, 1.0);
            tmp.data.par_iter_mut().zip(k.data.par_iter()).for_each(|(p, q)| {
                let keep = 1.0 - q[3].clamp(0.0, 1.0);
                for c in p.iter_mut() {
                    *c *= keep;
                }
            });
        }
        for (b, m) in &st.passes {
            self.place(ctx, layer, b, &mut tmp, *m, 1.0);
        }
        self.place(ctx, layer, &st.body, &mut tmp, layer.blend_mode, 1.0);
        let ch = bl.channels;
        canvas.data.par_iter_mut().zip(tmp.data.par_iter()).for_each(|(c, t)| {
            for i in 0..4 {
                let v = if i < 3 && !ch[i] { c[i] } else { t[i] };
                c[i] += (v - c[i]) * opacity;
            }
        });
    }

    fn composite_layer(&self, ctx: &EvalCtx, layer: &Layer, buf: &Buf, canvas: &mut Image, opacity: f32) {
        let matte = layer.track_matte.is_some_and(|tm| tm.layer != layer.id && ctx.comp.layer(tm.layer).is_some());
        if !matte && !layer.preserve_transparency {
            self.place(ctx, layer, buf, canvas, layer.blend_mode, opacity);
            return;
        }
        // Render in isolation, then matte / preserve transparency, then blend.
        let mut iso = Image::new(canvas.width, canvas.height);
        self.place(ctx, layer, buf, &mut iso, BlendMode::Normal, 1.0);
        self.composite_iso(ctx, layer, iso, canvas, opacity);
    }

    /// Apply the track matte / preserve transparency to an isolated layer render, then blend.
    fn composite_iso(&self, ctx: &EvalCtx, layer: &Layer, iso: Image, canvas: &mut Image, opacity: f32) {
        self.composite_iso_with(ctx, layer, iso, canvas, opacity, None);
    }

    /// [`Self::composite_iso`] with the track matte's pixels already drawn (`matte`, e.g. a 3D
    /// matte seen through the camera); otherwise the matte layer is placed in 2D.
    pub(crate) fn composite_iso_with(&self, ctx: &EvalCtx, layer: &Layer, mut iso: Image, canvas: &mut Image, opacity: f32, matte_img: Option<Image>) {
        let matte = layer.track_matte.and_then(|tm| ctx.comp.layer(tm.layer).filter(|m| m.id != layer.id).map(|m| (m, tm.kind)));
        let preserve = layer.preserve_transparency;
        if let Some((m, kind)) = matte {
            let mimg = match matte_img {
                Some(i) if i.width == canvas.width && i.height == canvas.height => i,
                _ => {
                    let mut mimg = Image::new(canvas.width, canvas.height);
                    if m.is_active_at(ctx.time)
                        && let Some(mb) = self.layer_buf(ctx, m)
                    {
                        let mo = ctx.opacity(m) as f32;
                        self.place(ctx, m, &mb, &mut mimg, BlendMode::Normal, mo);
                    }
                    mimg
                }
            };
            iso.data.par_iter_mut().zip(mimg.data.par_iter()).for_each(|(p, q)| {
                let k = match kind {
                    MatteKind::Alpha => q[3],
                    MatteKind::AlphaInverted => 1.0 - q[3],
                    MatteKind::Luma => effectcraft_color::luminance(q[0], q[1], q[2]),
                    MatteKind::LumaInverted => 1.0 - effectcraft_color::luminance(q[0], q[1], q[2]),
                }
                .clamp(0.0, 1.0);
                for c in p.iter_mut() {
                    *c *= k;
                }
            });
        }
        if preserve {
            iso.data.par_iter_mut().zip(canvas.data.par_iter()).for_each(|(p, q)| {
                for c in p.iter_mut() {
                    *c *= q[3];
                }
            });
        }
        canvas.blend_from(&iso, layer.blend_mode, opacity, layer.id.0 as u32);
    }

    /// Adjustment layer: effects applied to the comp below, limited to the layer's footprint.
    fn draw_adjustment(&self, ctx: &EvalCtx, layer: &Layer, canvas: &mut Image, opacity: f32) {
        let Some(mut foot) = self.source(ctx, layer) else { return };
        let _ = masks::apply(ctx, layer, &mut foot, self.mask_blur(ctx, layer));
        let mut matte = Image::new(canvas.width, canvas.height);
        self.place(ctx, layer, &foot, &mut matte, BlendMode::Normal, 1.0);
        let o = self.roi_offset().map(|(x, y, _, _)| [-x, -y]).unwrap_or([0.0; 2]);
        let mut below = Buf { img: canvas.clone(), offset: o, scale: self.opts.scale };
        // Effects run in the working space, not the linear blending space.
        if let Some(c) = self.pipe.from_blend() {
            color::convert(&mut below.img, &c);
        }
        let mut adjusted = self.apply_effects(ctx, layer, below, true);
        if let Some(c) = self.pipe.to_blend() {
            color::convert(&mut adjusted.img, &c);
        }
        if adjusted.img.width != canvas.width || adjusted.img.height != canvas.height {
            return;
        }
        canvas.data.par_iter_mut().zip(adjusted.img.data.par_iter()).zip(matte.data.par_iter()).for_each(|((c, a), m)| {
            let k = m[3] * opacity;
            for i in 0..4 {
                c[i] += (a[i] - c[i]) * k;
            }
        });
    }
}

/// A layer ready to composite (see [`Renderer::styled_layer`]).
pub struct StyledLayer {
    /// Exterior style passes (Drop Shadow, Outer Glow) with their blend modes.
    pub passes: Vec<(Arc<Buf>, BlendMode)>,
    pub body: Arc<Buf>,
    /// Pre-style pixels (knockout shape).
    pub content: Arc<Buf>,
    /// Styled cache key (the content key when plain).
    key: Option<u64>,
    /// Content cache key.
    ckey: Option<u64>,
    /// No layer styles: composite `body` the usual way.
    pub plain: bool,
}

/// Where a layer buffer lands (see [`Renderer::placement`]).
#[derive(Clone, Debug)]
pub struct Placement {
    /// Buffer pixel → output pixel; several = motion-blur sub-samples (equal weights).
    pub matrices: Vec<Mat3>,
    pub sampling: effectcraft_raster::Sampling,
    /// Dissolve noise seed.
    pub seed: u32,
}

/// Hooks for accelerated compositors (`effectcraft-gpu`): the pieces of the CPU walk they reuse
/// or fall back to. The CPU compositor itself is [`Renderer::comp_frame_cpu`].
impl<'a> Renderer<'a> {
    /// The project's colour pipeline.
    pub fn pipe(&self) -> color::Pipe {
        self.pipe
    }

    /// Nesting depth (0 = the top-level comp).
    pub fn depth(&self) -> usize {
        self.depth
    }

    /// The auxiliary 3D channels of comp `comp_id` at comp time `t` (depth, layer IDs, normals,
    /// UVs, Cryptomatte; see `three_d::compose::aux_pass`), at the renderer's output scale.
    pub fn comp_aux(&self, comp_id: ItemId, t: Tick) -> Option<effectcraft_raster::AuxChannels> {
        let ctx = self.eval_ctx(comp_id, t)?;
        Some(three_d::compose::aux_pass(self, &ctx))
    }

    /// Evaluation context of comp `comp_id` at comp time `t`.
    pub fn eval_ctx(&self, comp_id: ItemId, t: Tick) -> Option<EvalCtx<'a>> {
        Some(self.ctx(comp_id, self.project.comp(comp_id)?, t))
    }

    /// The layers drawn at the context time, bottom to top (solo, guides and visibility
    /// applied).
    pub fn visible_layers(&self, ctx: &EvalCtx<'a>) -> Vec<&'a Layer> {
        let (comp, t) = (ctx.comp, ctx.time);
        let any_solo = comp.layers.iter().any(|l| l.switches.solo && l.source.is_av() && l.is_active_at(t));
        comp.layers
            .iter()
            .rev()
            .filter(|l| l.is_active_at(t) && l.has_video() && !l.environment && (!any_solo || l.switches.solo) && (self.opts.guides || !l.switches.guide))
            .collect()
    }

    /// Draw a run of consecutive 3D layers into `canvas` (CPU).
    pub fn draw_3d_run(&self, ctx: &EvalCtx<'a>, run: &[&Layer], canvas: &mut Image) {
        three_d::compose::draw_run(self, ctx, run, canvas);
    }

    /// A run of consecutive 3D layers prepared for an accelerator (planes, lights, shadow
    /// casters; see [`three_d::Run3d`]) at output size `out`. `None` = draw it with
    /// [`Self::draw_3d_run`] (adjustment or wireframe layers in the run, Advanced 3D).
    pub fn prepare_3d_run(&self, ctx: &EvalCtx<'a>, run: &[&Layer], out: (u32, u32)) -> Option<three_d::Run3d> {
        three_d::compose::gpu_run(self, ctx, run, out)
    }

    /// A run of consecutive 3D layers of an Advanced 3D comp prepared for an accelerator's
    /// compositor ([`three_d::adv::Prepared`]): rendered as one scene and composited (Normal,
    /// premultiplied over) onto the canvas. `None` = not an Advanced 3D comp, or a layer needs the
    /// 2D compositing path (blend mode, track matte, Preserve Transparency): draw it with
    /// [`Self::draw_3d_run`].
    pub fn prepare_adv_run<'p>(&'p self, ctx: &'p EvalCtx<'a>, run: &'p [&'p Layer], out: (u32, u32)) -> Option<three_d::adv::Prepared<'p, 'a>> {
        three_d::adv::prepare_run(self, ctx, run, out)
    }

    /// A run of consecutive 3D layers of an Advanced 3D comp whose layers need the 2D
    /// compositing path (blend mode, track matte, Preserve Transparency), split for an
    /// accelerator's compositor ([`three_d::adv::SplitRun`]). `None` = [`Self::prepare_adv_run`]
    /// covers it, or it isn't an Advanced 3D run.
    pub fn split_adv_run<'p>(&'p self, ctx: &'p EvalCtx<'a>, run: &'p [&'p Layer], out: (u32, u32)) -> Option<three_d::adv::SplitRun<'p, 'a>> {
        three_d::adv::split_run(self, ctx, run, out)
    }

    /// Whether the comp's 3D layers draw with the Advanced 3D renderer.
    pub fn is_adv3d(&self, ctx: &EvalCtx) -> bool {
        three_d::adv::active(self, ctx)
    }

    /// An Environment Light Background layer ready to draw on an accelerator
    /// ([`three_d::SkyDraw`]; the CPU draws it in `draw_3d_run`). `None` = nothing to draw.
    pub fn sky_draw(&self, ctx: &EvalCtx, layer: &Layer) -> Option<three_d::SkyDraw> {
        three_d::compose::sky_draw(self, ctx, layer)
    }

    /// The pixels a Wireframe-quality layer sets (white, opaque) on a `size` canvas: its
    /// bounds' outline, one pixel wide. Empty when the layer has no bounds.
    pub fn wireframe_pixels(&self, ctx: &EvalCtx, layer: &Layer, size: (u32, u32)) -> Vec<(u32, u32)> {
        let Some(b) = content_bounds(ctx, layer) else { return vec![] };
        let s = self.opts.scale;
        let (l2c, _) = ctx.layer_to_comp(layer);
        let m = Mat3::scale(vec2(s, s)) * self.outer.map_or(l2c, |o| o * l2c);
        let c = [(b[0], b[1]), (b[2], b[1]), (b[2], b[3]), (b[0], b[3])].map(|(x, y)| m.apply(vec2(x, y)));
        let (w, h) = (size.0 as i64, size.1 as i64);
        let mut out = vec![];
        for i in 0..4 {
            let (p, q) = (c[i], c[(i + 1) % 4]);
            let n = ((q.x - p.x).abs().max((q.y - p.y).abs()) * 2.0).ceil().clamp(1.0, 1.0e5) as usize;
            for j in 0..=n {
                let t = j as f64 / n as f64;
                let (x, y) = ((p.x + (q.x - p.x) * t).floor() as i64, (p.y + (q.y - p.y) * t).floor() as i64);
                if x >= 0 && y >= 0 && x < w && y < h {
                    out.push((x as u32, y as u32));
                }
            }
        }
        out
    }

    /// Run the layer's effect stack on `target` (an accelerator's resident image): GPU-capable
    /// runs through [`FxTarget::gpu`], the others through [`FxTarget::cpu`], with the CPU's
    /// parameters and environment. `adjustment`: the stack runs on the comp below (adjustment
    /// layers).
    pub fn run_effects_on(&self, ctx: &EvalCtx, layer: &Layer, adjustment: bool, target: &mut dyn FxTarget) {
        // `target` is used as it is: the comp below for adjustment layers (the callers).
        self.run_effect_stack(ctx, layer, adjustment, usize::MAX, None, [0.0; 2], target);
    }

    /// An adjustment layer's footprint (its source with masks applied, layer space): where
    /// its effects show. `None` = the layer changes nothing.
    pub fn adjustment_footprint(&self, ctx: &EvalCtx, layer: &Layer) -> Option<Arc<Buf>> {
        // Cached, so an accelerator sees the same buffer (and uploads it once) every frame.
        let key = self.cache.and_then(|_| cache::footprint_key(ctx, layer, self.raster_scale(ctx, layer), self.opts.draft, self.mb_on(ctx, layer)));
        if let (Some(c), Some(k)) = (self.cache, key)
            && let Some(b) = c.get(k)
        {
            return Some(b);
        }
        let mut foot = self.source(ctx, layer)?;
        let _ = masks::apply(ctx, layer, &mut foot, self.mask_blur(ctx, layer));
        let foot = Arc::new(foot);
        if let (Some(c), Some(k)) = (self.cache, key) {
            c.insert(k, foot.clone());
        }
        Some(foot)
    }

    /// Draw one 2D layer into `canvas` on the CPU (any kind: adjustment, wireframe, styled…).
    pub fn draw_layer_cpu(&self, ctx: &EvalCtx, layer: &Layer, canvas: &mut Image) {
        self.draw_layer(ctx, layer, canvas, false);
    }

    /// Draw a collapsed precomp layer into `canvas` on the CPU.
    pub fn draw_collapsed_cpu(&self, ctx: &EvalCtx<'a>, layer: &Layer, item: ItemId, canvas: &mut Image) {
        self.draw_collapsed(ctx, layer, item, canvas, false);
    }

    /// The layer's finished buffers (styles included) in the blending space.
    pub fn styled_layer(&self, ctx: &EvalCtx, layer: &Layer) -> Option<StyledLayer> {
        let mut timing = self.profile.map(|_| LayerTiming { depth: self.depth, layer: layer.name.clone(), ..Default::default() });
        let t0 = web_time::Instant::now();
        let st = self.styled(ctx, layer, timing.as_mut()).map(|st| self.styled_to_blend(st));
        if let (Some(prof), Some(mut timing)) = (self.profile, timing) {
            timing.process_ms = t0.elapsed().as_secs_f64() * 1e3;
            if let Ok(mut v) = prof.lock() {
                v.push(timing);
            }
        }
        st
    }

    /// Opacity the layer composites with (its Opacity × collapsed precomps' opacity).
    pub fn layer_opacity(&self, ctx: &EvalCtx, layer: &Layer) -> f32 {
        ctx.opacity(layer) as f32 * self.opacity_mul
    }

    /// The layer's track matte layer and kind, if it has a valid one.
    pub fn track_matte<'c>(&self, ctx: &EvalCtx<'c>, layer: &Layer) -> Option<(&'c Layer, MatteKind)> {
        layer.track_matte.and_then(|tm| ctx.comp.layer(tm.layer).filter(|m| m.id != layer.id).map(|m| (m, tm.kind)))
    }
}

/// The layer's frame blending mode, if the comp's Enable Frame Blending switch is on.
/// One field of an interlaced frame as a full frame: the field's lines (`parity` 0 = even /
/// upper, 1 = odd / lower) kept, the other lines interpolated from their neighbours.
pub fn field_frame(img: &Image, parity: usize) -> Image {
    let (w, h) = (img.width as usize, img.height as usize);
    let mut out = img.clone();
    if h < 2 {
        return out;
    }
    for y in 0..h {
        if y % 2 == parity {
            continue;
        }
        let above = y.checked_sub(1);
        let below = (y + 1 < h).then_some(y + 1);
        for x in 0..w {
            let px = match (above, below) {
                (Some(a), Some(b)) => {
                    let (p, q) = (img.data[a * w + x], img.data[b * w + x]);
                    [(p[0] + q[0]) * 0.5, (p[1] + q[1]) * 0.5, (p[2] + q[2]) * 0.5, (p[3] + q[3]) * 0.5]
                }
                (Some(a), None) => img.data[a * w + x],
                (None, Some(b)) => img.data[b * w + x],
                (None, None) => img.data[y * w + x],
            };
            out.data[y * w + x] = px;
        }
    }
    out
}

/// Interpret Footage ▸ Invert Alpha and Interpret As Linear Light on decoded (premultiplied)
/// pixels.
pub fn interpret_pixels(img: &Image, f: &Footage) -> Image {
    let linear = f.linear_light_applies();
    if !f.invert_alpha && !linear {
        return img.clone();
    }
    let space = f.color_profile.unwrap_or(effectcraft_color::ColorSpace::Srgb);
    let mut out = img.clone();
    for p in out.data.iter_mut() {
        let a = p[3];
        let mut c = if a > 1e-6 { [p[0] / a, p[1] / a, p[2] / a] } else { [0.0; 3] };
        if linear {
            // The file's values are linear light: encode them like the rest of the footage.
            c = c.map(|v| space.encode(v));
        }
        let a = if f.invert_alpha { 1.0 - a } else { a };
        *p = [c[0] * a, c[1] * a, c[2] * a, a];
    }
    out
}

fn frame_blend_mode(ctx: &EvalCtx, layer: &Layer) -> FrameBlend {
    if ctx.comp.enable_frame_blending { layer.switches.frame_blend } else { FrameBlend::Off }
}

/// Position of source time `t` among frames at `rate`: the frame at or before `t` and the
/// fraction (0..1) of the way to the next frame. Exact (integer tick arithmetic), so frames that
/// line up exactly report a fraction of 0.
pub fn frame_position(t: Tick, rate: FrameRate) -> (i64, f64) {
    let num = t.0 as i128 * rate.num as i128;
    let den = TICKS_PER_SECOND as i128 * rate.den as i128;
    (num.div_euclid(den) as i64, num.rem_euclid(den) as f64 / den as f64)
}

/// Blend two neighbouring frames `w` of the way from `a` to `b`: a cross-fade (Frame Mix) or a
/// motion-compensated interpolation (Pixel Motion).
pub fn blend_frames(mode: FrameBlend, a: &Image, b: &Image, w: f32) -> Image {
    match mode {
        FrameBlend::PixelMotion => effectcraft_raster::flow::interpolate(a, b, w),
        _ => effectcraft_raster::flow::mix(a, b, w),
    }
}

/// Convenience: render one frame of a comp with no footage source.
/// Adaptive motion-blur sample count for a layer whose corners travel `travel` output pixels
/// during the shutter: one sample per pixel, clamped to Samples Per Frame … Adaptive Sample Limit.
pub fn adaptive_samples(travel: f64, per_frame: u32, limit: u32) -> usize {
    let lo = per_frame.clamp(2, 64) as f64;
    let hi = (limit.clamp(16, 256) as f64).max(lo);
    travel.ceil().clamp(lo, hi) as usize
}

/// A region of interest `[x, y, w, h]` lies within a `w`×`h` comp frame.
pub fn roi_inside(r: [f64; 4], w: u32, h: u32) -> bool {
    r[0] >= 0.0 && r[1] >= 0.0 && r[0] + r[2] <= w as f64 + 1e-6 && r[1] + r[3] <= h as f64 + 1e-6
}

/// The Extended Viewer's region of interest: the comp frame grown by `margin` × its size on every
/// side (`margin` = 1 shows a pasteboard one comp wide around the frame), intersected with the
/// `visible` comp-space rectangle `[x0, y0, x1, y1]` and snapped outward to whole 16 px steps from the frame edges
/// so panning re-renders rarely. `None` when the visible area lies inside the frame.
pub fn extended_region(w: u32, h: u32, visible: [f64; 4], margin: f64) -> Option<[f64; 4]> {
    let (w, h) = (w as f64, h as f64);
    let (mx, my) = (w * margin, h * margin);
    let snap_lo = |v: f64| (v / 16.0).floor() * 16.0;
    let snap_hi = |v: f64| (v / 16.0).ceil() * 16.0;
    let x0 = snap_lo(visible[0].max(-mx)).min(0.0);
    let y0 = snap_lo(visible[1].max(-my)).min(0.0);
    let x1 = w + snap_hi(visible[2].min(w + mx) - w).max(0.0);
    let y1 = h + snap_hi(visible[3].min(h + my) - h).max(0.0);
    if x0 >= 0.0 && y0 >= 0.0 && x1 <= w && y1 <= h {
        return None;
    }
    Some([x0, y0, x1 - x0, y1 - y0])
}

pub fn render_frame(project: &Project, comp: ItemId, t: Tick, scale: f64) -> Image {
    Renderer::new(project, &NoFootage, RenderOpts { scale, ..Default::default() }).comp_frame(comp, t)
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_auto;
#[cfg(test)]
mod tests_remap;
#[cfg(test)]
mod tests_roi;
#[cfg(test)]
mod tests_styles;

/// Layer-space bounds `[x0, y0, x1, y1]` of a layer's content at the context time (source size
/// for solids/footage/precomps, glyph bounds for text, painted bounds for shapes). Used for viewer
/// handles and hit testing.
pub fn content_bounds(ctx: &EvalCtx, layer: &effectcraft_project::Layer) -> Option<[f64; 4]> {
    match &layer.source {
        LayerSource::Text => {
            let glyphs = text::glyph_paths(ctx, layer);
            let paths: Vec<_> = glyphs.into_iter().map(|g| g.0).collect();
            effectcraft_path::bounds(&paths).map(|r| [r.x0, r.y0, r.x1, r.y1])
        }
        LayerSource::Shape => layer.props.sub("contents").and_then(|c| shapes::content_bounds(ctx, layer, c)).map(|r| [r.x0, r.y0, r.x1, r.y1]),
        LayerSource::Camera | LayerSource::Light { .. } => None,
        _ => {
            let (w, h) = source_size(ctx.project, layer);
            (w > 0 && h > 0).then_some([0.0, 0.0, w as f64, h as f64])
        }
    }
}

/// A mask/shape path as a kurbo path (for drawing overlays).
pub fn kurbo_path(sp: &effectcraft_keyframe::ShapePath) -> kurbo::BezPath {
    effectcraft_path::to_kurbo(sp)
}
#[cfg(test)]
mod tests_audio_fx;
#[cfg(test)]
mod tests_aux;
#[cfg(test)]
mod tests_collapse;
#[cfg(test)]
mod tests_color;
#[cfg(test)]
mod tests_frame_blend;
#[cfg(test)]
mod tests_mask_blur;
#[cfg(test)]
mod tests_nested;
#[cfg(test)]
mod tests_paint;
#[cfg(test)]
mod tests_time;
