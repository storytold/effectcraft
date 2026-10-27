use super::footage::NoFootage;
use super::opts::RenderOpts;
use super::*;

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
            let (w, h) = source_size(ctx.project, layer);
            (w > 0 && h > 0).then_some([0.0, 0.0, w as f64, h as f64])
        }
    }
}

/// A mask/shape path as a kurbo path (for drawing overlays).
pub fn kurbo_path(sp: &effectcraft_keyframe::ShapePath) -> kurbo::BezPath {
    effectcraft_path::to_kurbo(sp)
}
