use super::frames::adaptive_samples;
use super::opts::{LayerTiming, Through};
use super::*;

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

impl<'a> Renderer<'a> {
    /// The layer with its styles: exterior passes and body (cached per pass).
    pub(crate) fn styled(&self, ctx: &EvalCtx, layer: &Layer, mut timing: Option<&mut LayerTiming>) -> Option<StyledLayer> {
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
        let mut body = Arc::new(st.body);
        let mut passes: Vec<_> = st.passes.into_iter().map(|(b, m)| (Arc::new(b), m)).collect();
        // The buffers the cache keeps (another worker's, if it kept them first).
        if let (Some(c), Some(k)) = (self.cache_to_keep(), key) {
            body = c.insert(k, body);
            for (i, (b, _)) in passes.iter_mut().enumerate() {
                *b = c.insert(cache::derive(k, i as u64 + 1), b.clone());
            }
        }
        Some(StyledLayer { passes, body, content, key, ckey, plain: false })
    }

    /// The styled layer's buffers in the blending space.
    pub(crate) fn styled_to_blend(&self, st: StyledLayer) -> StyledLayer {
        if self.pipe.to_blend().is_none() {
            return st;
        }
        let body = self.to_blend(st.body, st.key);
        let content = if st.plain { body.clone() } else { self.to_blend(st.content, st.ckey) };
        let passes = st.passes.into_iter().enumerate().map(|(i, (b, m))| (self.to_blend(b, st.key.map(|k| cache::derive(k, i as u64 + 1))), m)).collect();
        StyledLayer { passes, body, content, ..st }
    }

    pub(crate) fn sampling(&self, layer: &Layer) -> effectcraft_raster::Sampling {
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
    pub(crate) fn content_times(&self, ctx: &EvalCtx, layer: &Layer) -> Option<Vec<Tick>> {
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
    pub(crate) fn blurred_source(&self, ctx: &EvalCtx, layer: &Layer, times: &[Tick]) -> Option<Buf> {
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
    pub(crate) fn content_key(&self, ctx: &EvalCtx, layer: &Layer, key: Option<u64>) -> Option<u64> {
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
    pub(crate) fn mask_blur(&self, ctx: &EvalCtx, layer: &Layer) -> masks::MaskBlur {
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
    pub(crate) fn place(&self, ctx: &EvalCtx, layer: &Layer, buf: &Buf, target: &mut Image, mode: BlendMode, opacity: f32) {
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
    pub(crate) fn draw_layer(&self, ctx: &EvalCtx, layer: &Layer, canvas: &mut Image, blank: bool) -> Region {
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
    pub(crate) fn composite_iso(&self, ctx: &EvalCtx, layer: &Layer, iso: Image, canvas: &mut Image, opacity: f32) {
        self.composite_iso_with(ctx, layer, iso, canvas, opacity, None);
    }

    /// [`Self::composite_iso`] with the track matte's pixels already drawn (`matte`, e.g. a 3D
    /// matte seen through the camera); otherwise the matte layer is placed in 2D.
    pub(crate) fn composite_iso_with(&self, ctx: &EvalCtx, layer: &Layer, mut iso: Image, canvas: &mut Image, opacity: f32, matte_img: Option<Image>) {
        self.apply_track_matte(ctx, layer, &mut iso, matte_img);
        if layer.preserve_transparency {
            iso.data.par_iter_mut().zip(canvas.data.par_iter()).for_each(|(p, q)| {
                for c in p.iter_mut() {
                    *c *= q[3];
                }
            });
        }
        canvas.blend_from(&iso, layer.blend_mode, opacity, layer.id.0 as u32);
    }

    /// Multiply `img` (comp-sized) by the layer's track matte, if it has one; `matte_img` as in
    /// [`Self::composite_iso_with`].
    fn apply_track_matte(&self, ctx: &EvalCtx, layer: &Layer, img: &mut Image, matte_img: Option<Image>) {
        let matte = layer.track_matte.and_then(|tm| ctx.comp.layer(tm.layer).filter(|m| m.id != layer.id).map(|m| (m, tm.kind)));
        if let Some((m, kind)) = matte {
            let mimg = match matte_img {
                Some(i) if i.width == img.width && i.height == img.height => i,
                _ => {
                    let mut mimg = Image::new(img.width, img.height);
                    if m.is_active_at(ctx.time)
                        && let Some(mb) = self.layer_buf(ctx, m)
                    {
                        let mo = ctx.opacity(m) as f32;
                        self.place(ctx, m, &mb, &mut mimg, BlendMode::Normal, mo);
                    }
                    mimg
                }
            };
            img.data.par_iter_mut().zip(mimg.data.par_iter()).for_each(|(p, q)| {
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
    }

    /// Adjustment layer: effects applied to the comp below, limited to the layer's footprint
    /// and its track matte.
    fn draw_adjustment(&self, ctx: &EvalCtx, layer: &Layer, canvas: &mut Image, opacity: f32) {
        let Some(mut foot) = self.source(ctx, layer) else { return };
        let _ = masks::apply(ctx, layer, &mut foot, self.mask_blur(ctx, layer));
        let mut matte = Image::new(canvas.width, canvas.height);
        self.place(ctx, layer, &foot, &mut matte, BlendMode::Normal, 1.0);
        self.apply_track_matte(ctx, layer, &mut matte, None);
        let o = self.roi_offset().map(|(x, y, _, _)| [-x, -y]).unwrap_or([0.0; 2]);
        let mut below = Buf { img: canvas.clone(), offset: o, scale: self.opts.scale };
        // Effects run in the working space, not the linear blending space.
        if let Some(c) = self.pipe.from_blend() {
            color::convert(&mut below.img, &c);
        }
        let mut adjusted = self.apply_effects(ctx, layer, below, true);
        // An effect that changed the frame (Motion Tile's Output size): back onto the canvas.
        if let Some((dx, dy)) =
            effectcraft_effects::adjustment_crop(adjusted.offset, o, (adjusted.img.width, adjusted.img.height), (canvas.width, canvas.height))
        {
            adjusted.img = adjusted.img.crop(dx, dy, canvas.width, canvas.height);
        }
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
    pub(crate) key: Option<u64>,
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
