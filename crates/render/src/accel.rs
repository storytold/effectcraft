use super::frames::content_bounds;
use super::fx_host::MAX_FX_DEPTH;
use super::layers::StyledLayer;
use super::opts::LayerTiming;
use super::{
    Arc, AutoPick, Buf, EffectCtx, EvalCtx, Image, ItemId, ItemKind, Layer, LayerSource, Mat3, MatteKind, Renderer, Tick, cache, color, masks, three_d, vec2,
};

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
    /// Render a top-level comp frame like [`Self::comp_frame`], composite it over `bg` and
    /// convert it to the H.264 export's 8-bit BT.709 limited-range 4:2:0 (Y plane, then U and V
    /// at half size; see `effectcraft_raster::yuv`). Every byte is within one level of the CPU
    /// conversion of the same frame (a driver's arithmetic may move a value near a rounding
    /// boundary). `None` = not handled (the caller renders and converts on the CPU).
    fn comp_frame_yuv420(&self, _r: &Renderer, _comp: ItemId, _t: Tick, _bg: [f32; 3]) -> Option<Vec<u8>> {
        None
    }
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
    /// Whether the accelerator can still render frames. `false` once it has stopped for good
    /// (its device was lost or retired): every frame it is asked for falls back to the CPU, so
    /// callers that limit frames in flight for it can stop doing so.
    fn available(&self) -> bool {
        true
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
    /// A rendered comp frame is done (the GPU pool ages sizes no frame asks for, and evicts
    /// only those once full). The multi-frame render paths call it once per comp frame:
    /// [`crate::passes::comp_frame`] (exports; a field-rendered output frame is two), the
    /// export's 4:2:0 frames ([`Accelerator::comp_frame_yuv420`]), remote frames and the
    /// preview workers. One-off renders (`Session::render`, single-frame export,
    /// thumbnails) don't, so they don't advance the pool's clock. Without it the pool never
    /// finds a stale size and keeps its first sizes, dropping newcomers.
    fn frame_end(&self) {}
    /// A render of many frames (an export) is done: drop what is kept only to speed up the
    /// next frame (pooled working textures). Exports call it, even when they fail, panic or
    /// are dropped; other multi-frame paths (preview, remote frames) don't, so there the pool
    /// keeps up to its budget until the next export ends.
    fn render_end(&self) {}
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
    /// Grow the buffer to cover its whole layer rectangle (`size` layer pixels from its own
    /// corner, the `layer_size` handed to effects), as [`effectcraft_effects::Buf::cover`].
    /// Authored buffers (text, shapes) only span their painted content; this gives the stack
    /// the room a sized source's buffer already has (#659).
    fn cover(&mut self, size: [f64; 2]) {
        self.cpu(&mut |mut buf| {
            buf.cover(size);
            buf
        });
    }
    /// Give the buffer the pixel-grid phase the layer rectangle's cover would
    /// ([`effectcraft_effects::Buf::align`]): grows left/top by less than 64 pixels.
    fn align(&mut self, size: [f64; 2]) {
        self.cpu(&mut |mut buf| {
            buf.align(size);
            buf
        });
    }
}

/// The CPU effect target (GPU chains go through [`Accelerator::effects`]: upload, readback).
pub(crate) struct CpuFx<'a> {
    pub(crate) accel: Option<&'a dyn Accelerator>,
    pub(crate) levels: Option<f32>,
    pub(crate) buf: Option<Buf>,
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

    /// The auxiliary channels effects on `layer` read ([`EffectHost::aux`]): its multi-layer
    /// OpenEXR footage's at the layer's source time (Time Remap included), or a precomp's
    /// generated 3D channels at its nested time. `None` for other layers.
    pub fn layer_aux(&self, ctx: &EvalCtx, layer: &Layer) -> Option<Arc<effectcraft_raster::AuxChannels>> {
        match &layer.source {
            LayerSource::Footage { item } => {
                let item = ctx.media_item(layer, *item);
                let ItemKind::Footage(f) = &self.project.item(item)?.kind else { return None };
                let a = self.footage.aux(item, f, ctx.source_time(layer))?;
                // Aux pixels per layer pixel (the layer is the footage at its nominal size). The
                // planes are only copied when that changes the scale.
                let scale = a.width as f64 / f.width as f64;
                if f.width > 0 && a.width > 0 && a.scale != scale {
                    let mut a = (*a).clone();
                    a.scale = scale;
                    return Some(Arc::new(a));
                }
                Some(a)
            }
            LayerSource::Comp { item } => {
                if self.depth > MAX_FX_DEPTH || self.project.comp_contains(*item, ctx.comp_id) {
                    return None;
                }
                self.nested().comp_aux(*item, ctx.nested_time(layer)).map(Arc::new)
            }
            _ => None,
        }
    }

    /// Evaluation context of comp `comp_id` at comp time `t`.
    pub fn eval_ctx(&self, comp_id: ItemId, t: Tick) -> Option<EvalCtx<'a>> {
        Some(self.ctx(comp_id, self.project.comp(comp_id)?, t))
    }

    /// The layers drawn at the context time, bottom to top (solo, guides and visibility
    /// applied).
    pub fn visible_layers(&self, ctx: &EvalCtx<'a>) -> Vec<&'a Layer> {
        let (comp, t) = (ctx.comp, ctx.time);
        let any_solo = comp.any_solo();
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
        self.run_effect_stack(ctx, layer, crate::pipeline::EffectStack { adjustment, limit: usize::MAX, origin: [0.0; 2] }, None, target);
    }

    /// [`Renderer::run_effects_on`] for a (non-adjustment) layer's own pixels: `target` holds
    /// them in effect space (as [`Renderer::layer_input`] hands them out) and the result stays
    /// there. Returns the effect-space origin in layer coordinates (rebase by its negation).
    pub fn run_layer_effects_on(&self, ctx: &EvalCtx, layer: &Layer, target: &mut dyn FxTarget) -> [f64; 2] {
        let origin = ctx.effect_bounds(layer).1;
        self.run_effect_stack(ctx, layer, crate::pipeline::EffectStack { adjustment: false, limit: usize::MAX, origin }, None, target);
        origin
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
        // The buffer the cache keeps (another worker's, if it kept one first).
        let foot = match (self.cache_to_keep(), key) {
            (Some(c), Some(k)) => c.insert(k, foot),
            _ => foot,
        };
        Some(foot)
    }

    /// Draw one 2D layer into `canvas` on the CPU (any kind: adjustment, wireframe, styled…).
    pub fn draw_layer_cpu(&self, ctx: &EvalCtx, layer: &Layer, canvas: &mut Image) {
        self.draw_layer(ctx, layer, canvas, false);
    }

    /// Draw a collapsed precomp layer into `canvas` on the CPU.
    pub fn draw_collapsed_cpu(&self, ctx: &EvalCtx<'a>, layer: &'a Layer, collapsed: &crate::Collapsed, canvas: &mut Image) {
        self.draw_collapsed(ctx, layer, collapsed, canvas, false);
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
