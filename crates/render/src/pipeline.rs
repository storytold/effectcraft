use super::accel::{Accelerator, Backend, CpuFx, FxStep, FxTarget};
use super::footage::{FootageSource, is_vector_footage};
use super::frames::{blend_frames, cineon_frame, content_bounds, field_frame, frame_blend_mode, frame_position, frame_size, interpret_pixels, roi_inside};
use super::fx_host::{FxHost, essential_overrides};
use super::opts::{Collapse3d, LayerTiming, MAX_THROUGH, RenderOpts, Through};
use super::{
    Arc, AutoKey, AutoPick, BlendMode, Buf, Comp, EffectCtx, EffectEnv, EvalCtx, Footage, FootageKind, FrameBlend, FrameRate, GroupKind, Image, ItemId,
    ItemKind, Layer, LayerCache, LayerSource, Params, Project, Quality, Region, Renderer, Tick, cache, color, masks, shapes, styles, text, three_d, vec2,
};
use effectcraft_keyframe::Value;
use effectcraft_project::LayerId;

/// Coordinate space and stack range shared by CPU and accelerated effect evaluation.
#[derive(Clone, Copy)]
pub(crate) struct EffectStack {
    pub adjustment: bool,
    pub limit: usize,
    pub origin: [f64; 2],
}

/// A property value an Essential Properties override gives a layer of a collapsed precomp.
#[derive(Clone, Debug)]
pub struct PropertyOverride {
    pub(crate) layer: LayerId,
    pub(crate) prop: u64,
    pub(crate) value: Value,
    /// A Font control changes only the font of the target text, not its text.
    pub(crate) font: bool,
}

/// A Media Replacement an Essential Properties override gives a footage layer of a collapsed
/// precomp.
#[derive(Clone, Copy, Debug)]
pub struct MediaOverride {
    pub(crate) layer: LayerId,
    pub(crate) item: ItemId,
}

impl MediaOverride {
    /// A Media Replacement of `layer`'s footage by `item`.
    pub fn new(layer: LayerId, item: ItemId) -> Self {
        MediaOverride { layer, item }
    }
}

/// A collapsed precomp layer: the nested comp and the instance overrides that apply to it.
pub struct Collapsed {
    pub item: ItemId,
    pub(crate) property_overrides: Vec<PropertyOverride>,
    pub(crate) media_overrides: Vec<MediaOverride>,
}

/// The property and media overrides of a collapsed precomp layer. `None` when its Essential
/// Properties group or the source comp's definitions are missing (the flattened path then
/// preserves the old behaviour).
fn collapsed_instance(ctx: &EvalCtx, layer: &Layer, item: ItemId) -> Option<(Vec<PropertyOverride>, Vec<MediaOverride>)> {
    use effectcraft_project::essential::{self, ControlType, EgKind};
    let over = essential::overridden(layer);
    if over.is_empty() {
        return Some((vec![], vec![]));
    }
    let g = essential::group(layer)?;
    let eg = ctx.project.comp(item)?.essential.as_ref()?;
    let mut property = vec![];
    let mut fonts: Vec<PropertyOverride> = vec![];
    let mut media = vec![];
    g.walk("", &mut |_, p| {
        if !over.contains(&p.uid) {
            return;
        }
        let Some(control) = essential::control_of(&p.match_id) else { return };
        let Some(ctl) = eg.resolve(control) else { return };
        let value = ctx.value(layer, p);
        match &ctl.kind {
            EgKind::Property { .. } => {
                let font = ctl.as_type == Some(ControlType::Font);
                for (layer, prop) in ctl.kind.targets() {
                    let o = PropertyOverride { layer, prop, value: value.clone(), font };
                    if font { fonts.push(o) } else { property.push(o) }
                }
            }
            EgKind::Media { layer: target } => {
                let Value::Scalar(id) = value else { return };
                let item = ItemId(id.max(0.0) as u64);
                if matches!(ctx.project.item(item).map(|i| &i.kind), Some(ItemKind::Footage(_))) {
                    media.push(MediaOverride { layer: *target, item });
                }
            }
            _ => {}
        }
    });
    // A Font control merges into the text its instance shows, after a whole-value (Text) override
    // of the same property, as `essential::with_overrides` does: keep one entry per property.
    for f in fonts {
        match property.iter_mut().rev().find(|o| o.layer == f.layer && o.prop == f.prop) {
            Some(o) => effectcraft_project::essential::merge_font(&mut o.value, &f.value),
            None => property.push(f),
        }
    }
    Some((property, media))
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

    /// The layer cache to keep this render's buffers in: none once its footage reads were
    /// called off, as they may have come back empty ([`FootageSource::called_off`], #329).
    pub(crate) fn cache_to_keep(&self) -> Option<&'a LayerCache> {
        self.cache.filter(|_| !self.footage.called_off())
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

    /// The render options one level down (precomps, collapsed precomps, comps read by effects):
    /// guide layers render only in their own comp, never when it is nested.
    fn nested_opts(&self) -> RenderOpts {
        RenderOpts { guides: false, ..self.opts }
    }

    /// A renderer for nested work (precomps, layers read by effects): one level deeper.
    pub(crate) fn nested(&self) -> Renderer<'a> {
        Renderer {
            opts: self.nested_opts(),
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

    /// A precomp layer's nested comp for an accelerator to render resident (no CPU render and
    /// upload of the nested frame): the renderer one level down, the comp, its time and the
    /// buffer scale, exactly as [`Self::layer_source`]'s comp branch would render it. `None`
    /// when that branch does more (composition proxy, Essential Properties overrides, frame
    /// blending, recursion) or draws nothing (outside the nested comp's span): use the CPU path.
    pub fn precomp_source(&self, ctx: &EvalCtx, layer: &Layer) -> Option<(Renderer<'a>, ItemId, Tick, f64)> {
        let LayerSource::Comp { item } = layer.source else { return None };
        if layer.switches.adjustment || self.project.comp_contains(item, ctx.comp_id) {
            return None;
        }
        let nc = self.project.comp(item)?;
        let lt = ctx.nested_time(layer);
        if self.content_times(ctx, layer).is_some() {
            return None;
        }
        if !nc.covers(lt) || self.proxy_for(item).is_some() || !essential_overrides(ctx, layer).is_empty() || frame_blend_mode(ctx, layer) != FrameBlend::Off {
            return None;
        }
        let s = self.opts.scale;
        let bs = if s < 1.0 && nc.preserve_resolution { 1.0 } else { s };
        let base = self.nested_through(layer);
        Some((Renderer { opts: RenderOpts { scale: bs, ..base.opts }, ..base }, item, lt, bs))
    }

    /// The nested comp of a precomp layer whose transformations collapse into this comp: the
    /// Collapse Transformations switch is on and the layer has no masks, effects or layer
    /// styles (those force a flattened render of the precomp, as in After Effects). Essential
    /// Properties overrides travel with it and are applied as the nested layers are evaluated,
    /// so the nested comp is not rasterised at its frame.
    pub fn collapsed(&self, ctx: &EvalCtx, layer: &Layer) -> Option<Collapsed> {
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
        // A composition proxy or frame blending is drawn by the flattened path (`precomp_source`);
        // collapsing renders the nested comp directly and would ignore both.
        if self.proxy_for(*item).is_some() || frame_blend_mode(ctx, layer) != FrameBlend::Off {
            return None;
        }
        let (property_overrides, media_overrides) = collapsed_instance(ctx, layer, *item)?;
        Some(Collapsed { item: *item, property_overrides, media_overrides })
    }

    /// A renderer and context that draw a collapsed precomp's layers straight into this comp:
    /// their transforms are concatenated with the precomp layer's (one resample, no
    /// rasterisation at the precomp's bounds), their opacity multiplied by `opacity`, and their
    /// 3D layers use this comp's camera and lights.
    pub fn collapse_into<'p>(&self, ctx: &EvalCtx<'a>, layer: &'a Layer, collapsed: &'p Collapsed, opacity: f32) -> Option<(Renderer<'p>, EvalCtx<'p>)>
    where
        'a: 'p,
    {
        let item = collapsed.item;
        let nc = self.project.comp(item)?;
        let t = ctx.nested_time(layer);
        if !nc.covers(t) {
            return None;
        }
        let nctx = EvalCtx {
            comp_id: item,
            comp: nc,
            time: t,
            property_overrides: &collapsed.property_overrides,
            media_overrides: &collapsed.media_overrides,
            ..*ctx
        };
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
        Some((
            Renderer { opts: self.nested_opts(), depth: self.depth + 1, outer, opacity_mul: opacity, collapse3d: Some(c3), through, through_lost, ..*self },
            nctx,
        ))
    }

    pub(crate) fn opacity_mul(&self) -> f32 {
        self.opacity_mul
    }

    /// Draw a collapsed 2D precomp layer: nested layers blend straight into `canvas` with their
    /// own modes (Normal precomp, no matte); otherwise they are drawn into an isolated buffer
    /// that then takes the precomp layer's track matte, Preserve Transparency and blend mode.
    /// Returns the region left to re-quantise (nested layers drawn straight into `canvas`
    /// quantise their own footprints).
    pub(crate) fn draw_collapsed(&self, ctx: &EvalCtx<'a>, layer: &'a Layer, collapsed: &Collapsed, canvas: &mut Image, blank: bool) -> Region {
        let opacity = ctx.opacity(layer) as f32 * self.opacity_mul;
        if opacity <= 0.0 {
            return Region::Empty;
        }
        let matte = layer.track_matte.is_some_and(|tm| tm.layer != layer.id && ctx.comp.layer(tm.layer).is_some());
        if layer.blend_mode == BlendMode::Normal && !matte && !layer.preserve_transparency {
            if let Some((sub, nctx)) = self.collapse_into(ctx, layer, collapsed, opacity) {
                sub.draw_comp(&nctx, canvas, blank);
            }
            return Region::Empty;
        }
        let Some((sub, nctx)) = self.collapse_into(ctx, layer, collapsed, 1.0) else { return Region::Empty };
        let mut iso = Image::new(canvas.width, canvas.height);
        sub.draw_comp(&nctx, &mut iso, true);
        self.composite_iso(ctx, layer, iso, canvas, opacity);
        Region::Full
    }

    /// Whether `layer`'s processed buffer (source → masks → effects) is cached and the same as
    /// one frame earlier: drawing it from the layer cache (an accelerator uploads it once) beats
    /// re-running its effects every frame.
    pub fn layer_is_static(&self, ctx: &EvalCtx, layer: &Layer) -> bool {
        if self.cache.is_none() {
            return false;
        }
        let (scale, draft, blur) = (self.raster_scale(ctx, layer), self.opts.draft, self.mb_on(ctx, layer));
        let prev = ctx.time - ctx.comp.frame_rate.frame_duration();
        let now = cache::layer_key(ctx, layer, scale, draft, blur);
        now.is_some() && now == cache::layer_key(&ctx.at(prev), layer, scale, draft, blur)
    }

    /// Rasterisation scale of a layer's source: the output scale, or — for text and shape layers,
    /// which are always continuously rasterised (#390, #751), and Continuously Rasterize vector
    /// footage — the output scale times the layer's on-screen scale (quarter-octave steps, rounded
    /// up, so vector content is drawn at its final size and stays sharp when scaled up).
    pub fn raster_scale(&self, ctx: &EvalCtx, layer: &Layer) -> f64 {
        let s = self.opts.scale;
        let continuous = match layer.source {
            LayerSource::Text | LayerSource::Shape => true,
            LayerSource::Footage { item } => {
                let item = ctx.media_item(layer, item);
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
    pub(crate) fn draw_wireframe(&self, ctx: &EvalCtx, layer: &Layer, canvas: &mut Image) {
        for (x, y) in self.wireframe_pixels(ctx, layer, (canvas.width, canvas.height)) {
            canvas.set(x, y, [1.0, 1.0, 1.0, 1.0]);
        }
    }

    pub(crate) fn ctx(&self, comp_id: ItemId, comp: &'a Comp, t: Tick) -> EvalCtx<'a> {
        EvalCtx { project: self.project, comp_id, comp, time: t, expr: self.expr, footage: Some(self.footage), property_overrides: &[], media_overrides: &[] }
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
        let (w, h) = frame_size(comp.width, comp.height, s);
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
                    Some(collapsed) => self.draw_collapsed(ctx, visible[i], &collapsed, canvas, blank),
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

    pub(crate) fn apply_effects(&self, ctx: &EvalCtx, layer: &Layer, buf: Buf, adjustment: bool) -> Buf {
        self.apply_effects_timed(ctx, layer, buf, adjustment, usize::MAX, None)
    }

    /// Run the first `limit` effects of the layer's stack (by position; disabled ones count) on
    /// `buf` (layer space, or the comp below for an adjustment layer).
    fn apply_effects_timed(&self, ctx: &EvalCtx, layer: &Layer, mut buf: Buf, adjustment: bool, limit: usize, timing: Option<&mut Vec<(String, f64)>>) -> Buf {
        // The comp below an adjustment layer is not in its layer space: it stays as it is.
        let origin = if adjustment { [0.0; 2] } else { ctx.effect_bounds(layer).1 };
        buf.rebase(origin);
        let mut out = self.effect_space_stack(ctx, layer, buf, EffectStack { adjustment, limit, origin }, timing);
        out.rebase([-origin[0], -origin[1]]);
        out
    }

    /// [`Renderer::apply_effects_timed`] on a buffer already in effect space (measured from
    /// `origin`, in layer coordinates); the result stays in effect space.
    fn effect_space_stack(&self, ctx: &EvalCtx, layer: &Layer, buf: Buf, stack: EffectStack, timing: Option<&mut Vec<(String, f64)>>) -> Buf {
        let mut t = CpuFx { accel: self.active_accel(), levels: self.pipe.levels, buf: Some(buf) };
        self.run_effect_stack(ctx, layer, stack, timing, &mut t);
        t.buf.unwrap_or_else(|| Buf { img: Image::new(1, 1), offset: [0.0; 2], scale: self.opts.scale })
    }

    /// Run the layer's video effect stack on `target` (in effect space, measured from `origin`
    /// in layer coordinates): runs of GPU-capable effects go to [`FxTarget::gpu`] (when an
    /// accelerator is active and supports them), the rest to [`FxTarget::cpu`] one by one
    /// (quantised to the bit depth after each).
    pub(crate) fn run_effect_stack(
        &self,
        ctx: &EvalCtx,
        layer: &Layer,
        stack: EffectStack,
        mut timing: Option<&mut Vec<(String, f64)>>,
        target: &mut dyn FxTarget,
    ) {
        let EffectStack { adjustment, limit, origin } = stack;
        if !layer.switches.effects {
            return;
        }
        let Some(fx) = layer.effects() else { return };
        let lt = layer.layer_time(ctx.time);
        let layer_size = ctx.effect_bounds(layer).0;
        // #659 / #1205: an authored layer's buffer spans only its painted content. Effects that
        // place pixels outside it grow the buffer themselves, by exactly what they need
        // (`layer_cover_pad`: CC Tiler, CC Smear, CC Mosaic sharp) instead of a blanket cover of
        // the whole layer rectangle on every stack — which paid a comp-sized texture per layer,
        // per frame in flight, for glow and blur that never use the extra room. The buffer still
        // takes the cover's pixel-grid phase: a downscaled layer resamples along that grid.
        // Effects not known to stay inside their own margins (Twirl, Bulge, Transform, ...) still
        // get the cover: they move content past the painted bounds without growing the buffer.
        let video = || {
            fx.groups().filter(|g| g.enabled).filter_map(|g| match &g.kind {
                GroupKind::Effect { effect } => effectcraft_effects::find(effect).filter(|s| !effectcraft_effects::audio_fx::is_audio_effect(s.id)),
                _ => None,
            })
        };
        if !adjustment && video().next().is_some() {
            if video().all(|s| s.skips_cover()) {
                target.align(layer_size);
            } else {
                target.cover(layer_size);
            }
        }
        let mut mask_shapes = masks::shapes(ctx, layer);
        if origin != [0.0; 2] {
            for q in mask_shapes.iter_mut().flat_map(|m| m.points.iter_mut()) {
                *q = [q[0] - origin[0], q[1] - origin[1]];
            }
        }
        // Capture once, before preceding effects can expand or replace the input. An
        // adjustment stack's original is the comp below, not its solid source.
        let needs_original =
            fx.groups().take(limit).any(|g| g.enabled && matches!(&g.kind, GroupKind::Effect { effect } if effect == "ec.channel.cccomposite"));
        let mut original = None;
        if needs_original {
            target.cpu(&mut |buf| {
                original = Some(buf.clone());
                buf
            });
        }
        let host = FxHost { r: self, ctx, layer, origin, index: 0, original: original.as_ref() };
        let env = EffectEnv {
            masks: &mask_shapes,
            host: Some(&host),
            comp_time: ctx.time.seconds(),
            frame_rate: ctx.comp.frame_rate.as_f64(),
            effect_index: 0,
            working_space: self.pipe.space,
            working_linear: self.pipe.linear,
            float_depth: self.pipe.levels.is_none(),
            shutter: self.mb_on(ctx, layer).then_some((ctx.comp.shutter_angle, ctx.comp.shutter_phase, ctx.comp.motion_blur_samples)),
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
                let hosts: Vec<_> = run.iter().map(|(i, _, _)| FxHost { index: *i, ..host }).collect();
                let steps: Vec<FxStep> = run
                    .iter()
                    .zip(&params)
                    .zip(&hosts)
                    .map(|(((i, g, s), params), host)| FxStep {
                        spec: s,
                        ctx: EffectCtx {
                            params,
                            time: lt.seconds(),
                            layer_size,
                            seed: g.uid as u32,
                            adjustment,
                            env: EffectEnv { host: Some(host), effect_index: *i, ..env },
                        },
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
            let host = FxHost { index: i, ..host };
            let env = EffectEnv { host: Some(&host), effect_index: i, ..env };
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
    pub(crate) fn source(&self, ctx: &EvalCtx, layer: &Layer) -> Option<Buf> {
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
                // A composition proxy (a rendered still or movie) stands in for the comp. It is
                // decoded like footage: its Separate Fields, Invert Alpha and Interpret As Linear
                // Light settings and the layer's Frame Blending apply. Nothing before the nested
                // comp starts or after it ends.
                let lt = ctx.nested_time(layer);
                if !self.project.comp(*item).is_some_and(|nc| nc.covers(lt)) {
                    return None;
                }
                if let Some(pf) = self.proxy_for(*item)
                    && let Some(nc) = self.project.comp(*item)
                {
                    let img = self.footage_frame(ctx, layer, *item, pf, lt, None)?;
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
                let item = ctx.media_item(layer, *item);
                let it = self.project.item(item)?;
                let ItemKind::Footage(f) = &it.kind else { return None };
                if !f.has_video {
                    return None;
                }
                // Continuously rasterised vector footage: drawn at the on-screen scale.
                if layer.switches.collapse && is_vector_footage(f) {
                    let k = self.raster_scale(ctx, layer);
                    if let Some(img) = self.footage.vector_frame(item, f, k) {
                        let mut buf = Buf { img: (*img).clone(), offset: [0.0; 2], scale: k };
                        if let Some(c) = self.pipe.media_in(f) {
                            color::convert(&mut buf.img, &c);
                        }
                        return Some(buf);
                    }
                }
                let lt = ctx.source_time(layer);
                // The proxy is decoded instead, at its own size, and fills the footage's frame.
                let proxy = self.proxy_for(item);
                let pf = proxy.unwrap_or(f);
                let size = self.reduced_size(f.width, f.height).filter(|_| proxy.is_none());
                let img = self.footage_frame(ctx, layer, item, pf, lt, size)?;
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
        let mut buf = match self.reduced_size(w, h) {
            // The source resampled it already (`FootageSource::frame_at_size`).
            Some(size) if (img.width, img.height) == size => Buf { img: img.clone(), offset: [0.0; 2], scale: s },
            Some((rw, rh)) if k <= 1.0 + 1e-9 => Buf { img: effectcraft_raster::resample(img, rw, rh), offset: [0.0; 2], scale: s },
            _ => Buf { img: img.clone(), offset: [0.0; 2], scale: 1.0 / k },
        };
        if let Some(c) = self.pipe.media_in(f) {
            color::convert(&mut buf.img, &c);
        }
        buf
    }

    /// The size of `w` × `h` layer pixels in this render when it is reduced enough that footage
    /// is resampled to it (below 3/4 scale).
    fn reduced_size(&self, w: u32, h: u32) -> Option<(u32, u32)> {
        let s = self.opts.scale;
        (s < 0.75).then(|| (((w as f64 * s).round() as u32).max(1), ((h as f64 * s).round() as u32).max(1)))
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
    /// `size`: a plain frame may come resampled to it ([`FootageSource::frame_at_size`]).
    fn footage_frame(&self, ctx: &EvalCtx, layer: &Layer, item: ItemId, f: &Footage, t: Tick, size: Option<(u32, u32)>) -> Option<Arc<Image>> {
        // Interpret Footage ▸ Separate Fields: each field is a frame at twice the rate.
        if f.fields != effectcraft_project::FieldOrder::Off && matches!(f.kind, FootageKind::Video | FootageKind::Sequence) {
            let field_rate = FrameRate::new(f.frame_rate.num * 2, f.frame_rate.den);
            let i = field_rate.frame_at(t).max(0);
            let img = cineon_frame(self.footage.frame_with(item, f, f.frame_rate.tick_of(i / 2), None, self.pipe.float_curve(f))?, f);
            let dominant_upper = f.fields == effectcraft_project::FieldOrder::UpperFirst;
            // Upper field = even lines (0, 2, …).
            let parity = if (i % 2 == 0) == dominant_upper { 0 } else { 1 };
            return Some(Arc::new(interpret_pixels(&field_frame(&img, parity), f, self.pipe.linear_footage_space(f))));
        }
        // Interpretation is per pixel but not linear: it runs on the frame before resampling.
        let per_pixel = f.invert_alpha || f.linear_light_applies();
        let size = size.filter(|_| !(per_pixel || f.cineon_applies()));
        let img = self.footage_frame_raw(ctx, layer, item, f, t, size)?;
        if per_pixel { Some(Arc::new(interpret_pixels(&img, f, self.pipe.linear_footage_space(f)))) } else { Some(img) }
    }

    fn footage_frame_raw(&self, ctx: &EvalCtx, layer: &Layer, item: ItemId, f: &Footage, t: Tick, size: Option<(u32, u32)>) -> Option<Arc<Image>> {
        let mode = frame_blend_mode(ctx, layer);
        let float = self.pipe.float_curve(f);
        if mode != FrameBlend::Off && matches!(f.kind, FootageKind::Video | FootageKind::Sequence) {
            let (i, w) = frame_position(t, f.frame_rate);
            if w > 1e-6 {
                // Blended frames are cached (Pixel Motion is expensive).
                let key = self.cache.map(|_| {
                    use std::hash::{Hash, Hasher};
                    let mut h = std::collections::hash_map::DefaultHasher::new();
                    (0x00f8_a3e5_u64, item, i, (w as f32).to_bits(), mode as u8, f.frame_rate.num, f.frame_rate.den, &f.path, float).hash(&mut h);
                    (f.alpha as u8).hash(&mut h);
                    if f.alpha == effectcraft_project::AlphaMode::Premultiplied {
                        f.premul_color.map(f32::to_bits).hash(&mut h);
                    }
                    if f.cineon_applies() {
                        let c = &f.cineon;
                        [c.converted_black, c.converted_white, c.black, c.white, c.gamma, c.rolloff].map(f32::to_bits).hash(&mut h);
                    }
                    h.finish()
                });
                if let (Some(c), Some(k)) = (self.cache, key)
                    && let Some(b) = c.get(k)
                {
                    return Some(Arc::new(b.img.clone()));
                }
                // Log scans are converted before they blend: the conversion is not linear.
                let a = self.footage.frame_with(item, f, f.frame_rate.tick_of(i), None, float).map(|a| cineon_frame(a, f));
                let b = self.footage.frame_with(item, f, f.frame_rate.tick_of(i + 1), None, float).map(|b| cineon_frame(b, f));
                if let (Some(a), Some(b)) = (a, b) {
                    let img = blend_frames(mode, &a, &b, w as f32);
                    if let (Some(c), Some(k)) = (self.cache_to_keep(), key) {
                        c.insert(k, Arc::new(Buf { img: img.clone(), offset: [0.0; 2], scale: 1.0 }));
                    }
                    return Some(Arc::new(img));
                }
            }
        }
        self.footage.frame_with(item, f, t, size, float).map(|img| cineon_frame(img, f))
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
        // The buffer the cache keeps (another worker's, if it kept one first).
        match (self.cache_to_keep(), key) {
            (Some(cache), Some(k)) => cache.insert(k, nb),
            _ => nb,
        }
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
        // The buffer the cache keeps (another worker's, if it kept one first).
        let flat = match (self.cache_to_keep(), key) {
            (Some(c), Some(k)) => c.insert(k, flat),
            _ => flat,
        };
        Some((flat, key))
    }

    /// A layer's source pixels at the context time, before masks, effects and the transform
    /// (what the motion tracker analyses, like After Effects' Layer panel). `None` for layers
    /// without pixels.
    pub fn layer_source(&self, ctx: &EvalCtx, layer: &Layer) -> Option<Buf> {
        self.source(ctx, layer)
    }

    /// A 3D model or primitive layer rendered on its own, through the comp's camera: what an
    /// effect layer parameter reads when it references one, as After Effects, which renders 3D
    /// layers through the active camera. The comp's pixels at this render's scale; the layer's
    /// blend mode, track matte and video switch don't apply (a source read, not a composite —
    /// the solo clone of a 3D track matte, see `three_d::adv`).
    pub(crate) fn solo_3d(&self, ctx: &EvalCtx, layer: &Layer) -> Buf {
        let s = self.opts.scale;
        let w = ((ctx.comp.width as f64 * s).ceil() as u32).max(1);
        let h = ((ctx.comp.height as f64 * s).ceil() as u32).max(1);
        let mut canvas = Image::new(w, h);
        // Outside the layer's in/out points there is nothing to read: the copy is transparent
        // there, like any other layer reference (the 3D pipelines don't gate on the range).
        if !layer.is_active_at(ctx.time) {
            return Buf { img: canvas, offset: [0.0; 2], scale: s };
        }
        let mut solo = layer.clone();
        solo.track_matte = None;
        solo.blend_mode = effectcraft_color::BlendMode::Normal;
        solo.preserve_transparency = false;
        solo.switches.video = true;
        three_d::compose::draw_run(self, ctx, &[&solo], &mut canvas);
        Buf { img: canvas, offset: [0.0; 2], scale: s }
    }

    /// Layer pixels after source → masks → effects (no layer styles).
    pub fn content_buf(&self, ctx: &EvalCtx, layer: &Layer) -> Option<Arc<Buf>> {
        self.content_buf_timed(ctx, layer, None).map(|(b, _)| b)
    }

    pub(crate) fn content_buf_timed(&self, ctx: &EvalCtx, layer: &Layer, mut timing: Option<&mut LayerTiming>) -> Option<(Arc<Buf>, Option<u64>)> {
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
        // The buffer the cache keeps (another worker's, if it kept one first).
        let buf = match (self.cache_to_keep(), key) {
            (Some(c), Some(k)) => c.insert(k, buf),
            _ => buf,
        };
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
            buf = self.effect_space_stack(ctx, layer, buf, EffectStack { adjustment: false, limit: effects, origin }, None);
        }
        let buf = Arc::new(buf);
        // The buffer the cache keeps (another worker's, if it kept one first).
        let buf = match (self.cache_to_keep(), key) {
            (Some(c), Some(k)) => c.insert(k, buf),
            _ => buf,
        };
        Some(buf)
    }
}
