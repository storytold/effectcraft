use super::footage::NoFootage;
use super::opts::RenderOpts;
use super::{Arc, EvalCtx, Footage, FrameBlend, FrameRate, Image, ItemId, Layer, LayerSource, Project, Renderer, TICKS_PER_SECOND, Tick, shapes, text};
use rayon::prelude::*;

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

/// Interpret Footage ▸ Cineon Settings ▸ Logarithmic Conversion converts the log code values of
/// a decoded (premultiplied) frame of `f` to the Current Gamma. When the conversion does not
/// apply, the frame is returned unchanged.
pub fn cineon_frame(img: Arc<Image>, f: &Footage) -> Arc<Image> {
    if !f.cineon_applies() {
        return img;
    }
    let cs = f.cineon;
    let mut out = (*img).clone();
    out.data.par_iter_mut().for_each(|p| {
        let a = p[3];
        let c = if a > 1e-6 { [p[0] / a, p[1] / a, p[2] / a] } else { [0.0; 3] }.map(|v| cs.convert(v));
        *p = [c[0] * a, c[1] * a, c[2] * a, a];
    });
    Arc::new(out)
}

/// Interpret Footage ▸ Invert Alpha and Interpret As Linear Light on decoded (premultiplied)
/// pixels; linear light is encoded in `space` ([`color::Pipe::linear_footage_space`]).
pub fn interpret_pixels(img: &Image, f: &Footage, space: effectcraft_color::ColorSpace) -> Image {
    let linear = f.linear_light_applies();
    if !f.invert_alpha && !linear {
        return img.clone();
    }
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

pub(crate) fn frame_blend_mode(ctx: &EvalCtx, layer: &Layer) -> FrameBlend {
    if ctx.comp.enable_frame_blending { layer.switches.frame_blend } else { FrameBlend::Off }
}

/// Position of source time `t` among frames at `rate`: the frame at or before `t` and the
/// fraction (0..1) of the way to the next frame. Exact (integer tick arithmetic), so frames that
/// line up exactly report a fraction of 0.
pub fn frame_position(t: Tick, rate: FrameRate) -> (i64, f64) {
    // A project file can carry a zero or negative rate; there are no frames to land between.
    if rate.num <= 0 || rate.den <= 0 {
        return (0, 0.0);
    }
    let num = t.0 as i128 * rate.num as i128;
    let den = TICKS_PER_SECOND as i128 * rate.den as i128;
    (num.div_euclid(den) as i64, num.rem_euclid(den) as f64 / den as f64)
}

/// The fewest source frames one comp frame must run through before frame blending averages all
/// of them ([`mix_window`]); below this the two neighbouring frames blend as they always have.
/// Conservative on purpose: common footage (60p in a 30, 25 or 24 fps comp, 50p in 25) keeps
/// its familiar look, while fast-conformed footage (600 fps in a 25 fps comp, #3180) gets the
/// mix. Pending a comparison with After Effects, whose exact threshold is not documented.
pub const MIN_MIX_FRAMES: f64 = 4.0;

/// The most source frames one comp frame mixes; a wider span is sampled at this many evenly
/// spaced frames (After Effects is reported to blend about a dozen; this also bounds the
/// decoding per comp frame). Pending a comparison with After Effects.
pub const MAX_MIX_FRAMES: usize = 16;

/// A span of more source frames than this in one comp frame is a jump (a time remap cut), not
/// motion to blur.
pub const MAX_MIX_SPAN: f64 = 1000.0;

/// How finely [`blend_spans`] probes the source's speed: at 1/16 of a comp frame.
const SPEED_PROBE: i64 = 16;

/// How far a frame-blended layer's source runs during one comp frame from the context time
/// (Interpret Footage's conformed rate, time stretch and time remap all show up in it), as
/// `(local, whole)`: the local speed just after the comp frame's time scaled to a whole frame,
/// and the actual difference to the next comp frame. They disagree across a time remap jump
/// (a hold keyframe followed by a cut, a `loopOut` wrapping back), which then does not mix.
/// Measured on the shown source time ([`EvalCtx::nested_time`]: a precomp proxy that preserves
/// its frame rate is floored to its frames).
pub(crate) fn blend_spans(ctx: &EvalCtx, layer: &Layer) -> (Tick, Tick) {
    let frame = ctx.comp.frame_duration();
    let t0 = ctx.nested_time(layer);
    let probe = Tick(frame.0 / SPEED_PROBE);
    let local = ctx.at(ctx.time + probe).nested_time(layer) - t0;
    let whole = ctx.at(ctx.time + frame).nested_time(layer) - t0;
    (Tick(local.0.saturating_mul(SPEED_PROBE)), whole)
}

/// The source frames a frame-blended comp frame averages when the layer runs through many
/// source frames per comp frame (footage interpreted at a much higher frame rate than the comp,
/// a sped-up time stretch or time remap), so a comp frame skips whole source frames: the frames
/// at `rate` that one comp frame's worth of source from `t` overlaps, each weighted by the share
/// of a frame it covers. `spans` is [`blend_spans`]' `(local, whole)`; playing forwards the span
/// is `[t, t + span)`, backwards it ends with the frame at `t` (the unblended frame). `None` —
/// the two neighbouring frames then blend ([`frame_position`], [`blend_frames`]) — when the span
/// is shorter than [`MIN_MIX_FRAMES`], longer than [`MAX_MIX_SPAN`], or the local speed and the
/// whole frame's difference disagree by more than a source frame (a time remap jump).
pub fn mix_window(t: Tick, spans: (Tick, Tick), rate: FrameRate) -> Option<Vec<(i64, f64)>> {
    if rate.num <= 0 || rate.den <= 0 {
        return None;
    }
    let frames_of = |s: Tick| s.0 as f64 * rate.num as f64 / (TICKS_PER_SECOND as f64 * rate.den as f64);
    let width = frames_of(spans.0);
    let whole = frames_of(spans.1);
    if !width.is_finite() || !whole.is_finite() || (width - whole).abs() > 1.0 + 1e-9 {
        return None;
    }
    if width.abs() < MIN_MIX_FRAMES - 1e-9 || width.abs() > MAX_MIX_SPAN {
        return None;
    }
    let (i, w) = frame_position(t, rate);
    let p = i as f64 + w;
    let lo = if width < 0.0 { p + 1.0 + width } else { p };
    let width = width.abs();
    let hi = lo + width;
    let first = lo.floor();
    let n = hi.ceil() - first;
    if n <= MAX_MIX_FRAMES as f64 {
        let frames = (0..n as usize)
            .filter_map(|k| {
                let j = first + k as f64;
                let cover = (j + 1.0).min(hi) - j.max(lo);
                (cover > 1e-9).then_some((j as i64, cover))
            })
            .collect();
        return Some(frames);
    }
    let m = MAX_MIX_FRAMES as f64;
    Some((0..MAX_MIX_FRAMES).map(|k| ((lo + width * (k as f64 + 0.5) / m).floor() as i64, 1.0)).collect())
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

/// The size of a `width` x `height` comp frame rendered at `scale` (both compositors, and the
/// frame size Composition Settings shows for a Resolution).
pub fn frame_size(width: u32, height: u32, scale: f64) -> (u32, u32) {
    (((width as f64 * scale).round() as u32).max(1), ((height as f64 * scale).round() as u32).max(1))
}

pub fn render_frame(project: &Project, comp: ItemId, t: Tick, scale: f64) -> Image {
    Renderer::new(project, &NoFootage, RenderOpts { scale, ..Default::default() }).comp_frame(comp, t)
}

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
            let (w, h) = ctx.source_size(layer);
            (w > 0 && h > 0).then_some([0.0, 0.0, w as f64, h as f64])
        }
    }
}

/// A mask/shape path as a kurbo path (for drawing overlays).
pub fn kurbo_path(sp: &effectcraft_keyframe::ShapePath) -> kurbo::BezPath {
    effectcraft_path::to_kurbo(sp)
}
