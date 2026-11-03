//! Rasterisation of a parsed SVG into a premultiplied `f32` image (anti-aliased coverage from
//! `effectcraft-path`; group opacity composites the group as a whole, as SVG specifies).

use effectcraft_geom::Mat3;
use effectcraft_raster::{Image, Mask};
use kurbo::{Affine, Point, Shape as _};
use rayon::prelude::*;

use crate::{BlendMode, Doc, FillRule, Gradient, GradientKind, Group, Node, Paint, Pattern, PatternUnits, Shape, SoftMask, Spread};

fn mat3(a: Affine) -> Mat3 {
    let c = a.as_coeffs();
    Mat3([[c[0], c[2], c[4]], [c[1], c[3], c[5]], [0.0, 0.0, 1.0]])
}

/// Rasterise `doc` into a `w`×`h` image, scaled by `scale` (1 = the document's pixel size).
pub fn rasterize(doc: &Doc, w: u32, h: u32, scale: f64) -> Image {
    rasterize_with(doc, w, h, Affine::scale(scale))
}

/// Rasterise `doc` with `m` mapping document pixels to image pixels.
pub fn rasterize_with(doc: &Doc, w: u32, h: u32, m: Affine) -> Image {
    let mut budget = SurfaceBudget {
        live_bytes: 0,
        coverage_bytes: 0,
        pattern_tile_bytes: 0,
        pattern_render_work: 0,
        pattern_render_work_limit: MAX_PATTERN_RENDER_WORK,
        clip_render_work: 0,
        clip_render_work_limit: MAX_CLIP_RENDER_WORK,
        image_render_work: 0,
        image_render_work_limit: MAX_IMAGE_RENDER_WORK,
        limit_bytes: MAX_LIVE_SURFACE_BYTES,
    };
    let Some(mut img) = budget.image(w, h, None) else { return Image::default() };
    draw_group(&doc.root, m, 1.0, &mut img, &mut budget);
    img
}

/// Bound the simultaneously live offscreen image surfaces created by nested groups.
const MAX_LIVE_SURFACE_BYTES: usize = 512 * 1024 * 1024;
/// Bound tile allocation and rasterisation work across one SVG render, even though each tile is
/// released before the next pattern paint.
const MAX_PATTERN_TILE_BYTES: usize = 512 * 1024 * 1024;
/// Bound the sum of tile-pixel × content-traversal work and destination sampling work per render.
const MAX_PATTERN_RENDER_WORK: usize = 256 * 1024 * 1024;
/// Bound repeated clip-path rasterisation across one SVG render.
const MAX_CLIP_RENDER_WORK: usize = 256 * 1024 * 1024;
/// Bound source-image sampling across one SVG render (up to 8×8 taps per destination pixel).
const MAX_IMAGE_RENDER_WORK: usize = 256 * 1024 * 1024;

struct SurfaceBudget {
    live_bytes: usize,
    coverage_bytes: usize,
    pattern_tile_bytes: usize,
    pattern_render_work: usize,
    pattern_render_work_limit: usize,
    clip_render_work: usize,
    clip_render_work_limit: usize,
    image_render_work: usize,
    image_render_work_limit: usize,
    limit_bytes: usize,
}

impl SurfaceBudget {
    fn image(&mut self, width: u32, height: u32, copy: Option<&Image>) -> Option<Image> {
        let pixels = (width as usize).checked_mul(height as usize)?;
        let bytes = pixels.checked_mul(std::mem::size_of::<[f32; 4]>())?;
        let new_live = self.live_bytes.checked_add(bytes)?;
        if new_live.checked_add(self.coverage_bytes)? > self.limit_bytes {
            return None;
        }
        let mut data = Vec::new();
        data.try_reserve_exact(pixels).ok()?;
        data.resize(pixels, [0.0; 4]);
        if let Some(source) = copy {
            if source.width != width || source.height != height || source.data.len() != pixels {
                return None;
            }
            data.copy_from_slice(&source.data);
        }
        self.live_bytes = new_live;
        Some(Image { width, height, data })
    }

    /// Reserve both the returned f32 coverage mask and tiny-skia's temporary u8 mask.
    fn coverage(&mut self, width: u32, height: u32) -> Option<usize> {
        let pixels = (width as usize).checked_mul(height as usize)?;
        let mask_pixels = (width.max(1) as usize).checked_mul(height.max(1) as usize)?;
        let output_mask = pixels.checked_mul(std::mem::size_of::<f32>())?;
        let scratch_mask = mask_pixels.checked_mul(std::mem::size_of::<u8>())?;
        let bytes = output_mask.checked_add(scratch_mask)?;
        self.reserve_coverage(bytes)?;
        Some(bytes)
    }

    fn reserve_coverage(&mut self, bytes: usize) -> Option<()> {
        let total = self.coverage_bytes.checked_add(bytes)?;
        if self.live_bytes.checked_add(total)? > self.limit_bytes {
            return None;
        }
        self.coverage_bytes = total;
        Some(())
    }

    fn release_coverage(&mut self, bytes: usize) {
        self.coverage_bytes = self.coverage_bytes.saturating_sub(bytes);
    }

    fn release(&mut self, image: &Image) {
        self.live_bytes = self.live_bytes.saturating_sub(image.data.len().saturating_mul(std::mem::size_of::<[f32; 4]>()));
    }

    /// Allocate one pattern tile while charging its full storage cost against the render-wide
    /// pattern budget. This prevents many shapes from repeatedly rasterising a near-limit tile.
    fn pattern_tile(&mut self, width: u32, height: u32) -> Option<Image> {
        let pixels = (width as usize).checked_mul(height as usize)?;
        let bytes = pixels.checked_mul(std::mem::size_of::<[f32; 4]>())?;
        let total = self.pattern_tile_bytes.checked_add(bytes)?;
        if total > MAX_PATTERN_TILE_BYTES {
            return None;
        }
        let image = self.image(width, height, None)?;
        self.pattern_tile_bytes = total;
        Some(image)
    }

    fn charge_pattern_render_work(&mut self, work: usize) -> bool {
        let Some(total) = self.pattern_render_work.checked_add(work) else { return false };
        if total > self.pattern_render_work_limit {
            return false;
        }
        self.pattern_render_work = total;
        true
    }

    fn charge_clip_render_work(&mut self, work: usize) -> bool {
        let Some(total) = self.clip_render_work.checked_add(work) else { return false };
        if total > self.clip_render_work_limit {
            return false;
        }
        self.clip_render_work = total;
        true
    }

    fn charge_image_render_work(&mut self, work: usize) -> bool {
        let Some(total) = self.image_render_work.checked_add(work) else { return false };
        if total > self.image_render_work_limit {
            return false;
        }
        self.image_render_work = total;
        true
    }
}

fn draw_group(g: &Group, m: Affine, opacity: f64, dst: &mut Image, budget: &mut SurfaceBudget) {
    let m = m * g.transform;
    let o = opacity * g.opacity;
    if o <= 0.0 {
        return;
    }
    let offscreen = !g.clip.is_empty()
        || g.mask.is_some()
        || g.blend != BlendMode::Normal
        || (g.opacity < 1.0 && !matches!(g.children.as_slice(), [Node::Shape(_) | Node::Image(_)]))
        || g.knockout;
    if !offscreen {
        for c in &g.children {
            draw_node(c, m, o, dst, budget);
        }
        return;
    }
    draw_offscreen_group(g, m, o, dst, budget);
}

/// Draws a group through an offscreen buffer, then composites it onto `dst`.
fn draw_offscreen_group(g: &Group, m: Affine, o: f64, dst: &mut Image, budget: &mut SurfaceBudget) {
    let only_clip = g.mask.is_none() && g.blend == BlendMode::Normal && o >= 1.0 && !g.knockout;
    // Draw the children on their own (isolated) or over a copy of the backdrop, keep what
    // every clip path covers and the soft mask lets through, then composite with the group's
    // opacity and blend mode.
    let backdrop = if g.isolated { None } else { budget.image(dst.width, dst.height, Some(dst)) };
    if !g.isolated && backdrop.is_none() {
        return;
    }
    let Some(mut tmp) = budget.image(dst.width, dst.height, backdrop.as_ref()) else {
        release_backdrop(budget, backdrop.as_ref());
        return;
    };
    draw_children(g, m, &mut tmp, backdrop.as_ref(), budget);
    if let Some(b) = &backdrop {
        if only_clip {
            // A non-isolated clipping group: the backdrop where the clip excludes the children.
            if !clip_onto_backdrop(g, m, dst, &tmp, budget) {
                budget.release(&tmp);
                release_backdrop(budget, backdrop.as_ref());
                return;
            }
            budget.release(&tmp);
            release_backdrop(budget, backdrop.as_ref());
            return;
        }
        if !strip_backdrop(g, m, b, &mut tmp, budget) {
            budget.release(&tmp);
            release_backdrop(budget, backdrop.as_ref());
            return;
        }
    }
    if !g.clip.is_empty() {
        let Some((cov, reserved)) = clip_coverage(g, m, dst.width, dst.height, budget) else {
            budget.release(&tmp);
            release_backdrop(budget, backdrop.as_ref());
            return;
        };
        tmp.data.par_iter_mut().zip(cov.par_iter()).for_each(|(p, c)| {
            for v in p.iter_mut() {
                *v *= c;
            }
        });
        drop(cov);
        budget.release_coverage(reserved);
    }
    if let Some(sm) = &g.mask
        && !apply_soft_mask(sm, m, &mut tmp, budget)
    {
        budget.release(&tmp);
        release_backdrop(budget, backdrop.as_ref());
        return;
    }
    if g.blend == BlendMode::Normal {
        over(dst, &tmp, o as f32);
    } else {
        let mode = g.blend;
        let k = o as f32;
        dst.data.par_iter_mut().zip(tmp.data.par_iter()).for_each(|(d, s)| {
            let s = [s[0] * k, s[1] * k, s[2] * k, s[3] * k];
            if s[3] > 0.0 {
                *d = blend_pixel(mode, *d, s);
            }
        });
    }
    budget.release(&tmp);
    release_backdrop(budget, backdrop.as_ref());
}

fn release_backdrop(budget: &mut SurfaceBudget, backdrop: Option<&Image>) {
    if let Some(b) = backdrop {
        budget.release(b);
    }
}

/// A clipping group over a backdrop: the children where the clip covers, the backdrop elsewhere.
fn clip_onto_backdrop(g: &Group, m: Affine, dst: &mut Image, tmp: &Image, budget: &mut SurfaceBudget) -> bool {
    let Some((cov, reserved)) = clip_coverage(g, m, dst.width, dst.height, budget) else { return false };
    dst.data.par_iter_mut().zip(tmp.data.par_iter()).zip(cov.par_iter()).for_each(|((d, t), c)| {
        for i in 0..4 {
            d[i] = d[i] * (1.0 - c) + t[i] * c;
        }
    });
    drop(cov);
    budget.release_coverage(reserved);
    true
}

/// Remove the backdrop's contribution (ISO 32000-1 §11.4.8): with the group's own
/// alpha αg (its children alone), C = Cn + (Cn − C0)·(α0/αg − α0). Returns false when no
/// scratch buffer is available.
fn strip_backdrop(g: &Group, m: Affine, backdrop: &Image, tmp: &mut Image, budget: &mut SurfaceBudget) -> bool {
    let Some(mut solo) = budget.image(tmp.width, tmp.height, None) else {
        return false;
    };
    draw_children(g, m, &mut solo, None, budget);
    tmp.data.par_iter_mut().zip(backdrop.data.par_iter()).zip(solo.data.par_iter()).for_each(|((t, b0), s)| {
        let ag = s[3];
        if ag <= 0.0 {
            *t = [0.0; 4];
            return;
        }
        let un = |c: &[f32; 4]| if c[3] > 0.0 { [c[0] / c[3], c[1] / c[3], c[2] / c[3]] } else { [0.0; 3] };
        let (cn, c0, a0) = (un(t), un(b0), b0[3]);
        let k = a0 / ag - a0;
        let mut out = [0.0; 4];
        for i in 0..3 {
            out[i] = (cn[i] + (cn[i] - c0[i]) * k).clamp(0.0, 1.0) * ag;
        }
        out[3] = ag;
        *t = out;
    });
    budget.release(&solo);
    drop(solo);
    true
}

/// Multiplies the group's pixels by its soft mask. Returns false when no scratch buffer is
/// available.
fn apply_soft_mask(sm: &SoftMask, m: Affine, tmp: &mut Image, budget: &mut SurfaceBudget) -> bool {
    let Some(mut mk) = budget.image(tmp.width, tmp.height, None) else {
        return false;
    };
    draw_group(&sm.content, m, 1.0, &mut mk, budget);
    let bc = sm.backdrop.map(|v| v as f32);
    let lum = sm.luminosity;
    tmp.data.par_iter_mut().zip(mk.data.par_iter()).for_each(|(p, q)| {
        let k = if lum {
            // The mask group composited over its backdrop colour, then its luminosity.
            let c = [0, 1, 2].map(|i| q[i] + bc[i] * (1.0 - q[3]));
            0.3 * c[0] + 0.59 * c[1] + 0.11 * c[2]
        } else {
            q[3]
        };
        let k = k.clamp(0.0, 1.0);
        for v in p.iter_mut() {
            *v *= k;
        }
    });
    budget.release(&mk);
    drop(mk);
    true
}

/// The product of a group's clip-path coverages, or `None` when the work or surface budget is
/// exhausted. The reservation covers the output and the temporary masks used to rasterise clips.
fn clip_coverage(g: &Group, m: Affine, w: u32, h: u32, budget: &mut SurfaceBudget) -> Option<(Vec<f32>, usize)> {
    let pixels = (w as usize).checked_mul(h as usize)?;
    let mask_pixels = (w.max(1) as usize).checked_mul(h.max(1) as usize)?;
    let paths_work = g.clip.iter().try_fold(0usize, |work, (path, _)| work.checked_add(path.elements().len().max(1)))?;
    let work = pixels.checked_mul(paths_work)?;
    if !budget.charge_clip_render_work(work) {
        return None;
    }
    let bytes = pixels.checked_mul(std::mem::size_of::<f32>())?.checked_mul(2)?.checked_add(mask_pixels.checked_mul(std::mem::size_of::<u8>())?)?;
    budget.reserve_coverage(bytes)?;
    let mut out = Vec::new();
    if out.try_reserve_exact(pixels).is_err() {
        budget.release_coverage(bytes);
        return None;
    }
    out.resize(pixels, 1.0f32);
    for (path, rule) in &g.clip {
        let rule = match rule {
            FillRule::NonZero => effectcraft_path::FillRule::NonZero,
            FillRule::EvenOdd => effectcraft_path::FillRule::EvenOdd,
        };
        let cov = effectcraft_path::fill_coverage(std::slice::from_ref(path), &mat3(m), w, h, rule);
        out.par_iter_mut().zip(cov.data.par_iter()).for_each(|(o, c)| *o *= c);
    }
    Some((out, bytes))
}

/// Draw a group's children into `dst`. Knockout groups composite each child with the
/// group's initial backdrop (`backdrop`, or transparent) instead of with the children below:
/// where the child has shape `f`, `dst = dst·(1 − f) + child + backdrop·(f − αchild)`.
fn draw_children(g: &Group, m: Affine, dst: &mut Image, backdrop: Option<&Image>, budget: &mut SurfaceBudget) {
    if !g.knockout {
        for c in &g.children {
            draw_node(c, m, 1.0, dst, budget);
        }
        return;
    }
    for c in &g.children {
        let Some(mut ci) = budget.image(dst.width, dst.height, None) else { return };
        draw_node(c, m, 1.0, &mut ci, budget);
        // Shape: the child drawn fully opaque (constant opacities to 1).
        let Some(mut si) = budget.image(dst.width, dst.height, None) else {
            budget.release(&ci);
            return;
        };
        draw_node(&opaque(c), m, 1.0, &mut si, budget);
        let zero = [0.0f32; 4];
        dst.data.par_iter_mut().enumerate().for_each(|(i, d)| {
            let (cp, f) = (ci.data[i], si.data[i][3].max(ci.data[i][3]));
            if f <= 0.0 {
                return;
            }
            let b = backdrop.map(|b| b.data[i]).unwrap_or(zero);
            for k in 0..4 {
                d[k] = d[k] * (1.0 - f) + cp[k] + b[k] * (f - cp[3]);
            }
        });
        budget.release(&ci);
        budget.release(&si);
        drop(ci);
        drop(si);
    }
}

/// A node with its constant opacities set to 1 (its shape, for knockout groups).
fn opaque(n: &Node) -> Node {
    match n {
        Node::Group(g) => {
            let mut g = g.clone();
            g.opacity = 1.0;
            g.children = g.children.iter().map(opaque).collect();
            Node::Group(g)
        }
        Node::Shape(s) => {
            let mut s = s.clone();
            s.opacity = 1.0;
            s.fill_opacity = 1.0;
            if let Some(st) = &mut s.stroke {
                st.opacity = 1.0;
            }
            Node::Shape(s)
        }
        Node::Image(i) => {
            let mut i = i.clone();
            i.opacity = 1.0;
            Node::Image(i)
        }
    }
}

/// `src` composited onto `dst` with a blend mode (both premultiplied RGBA):
/// `co = cs·(1 − αb) + cb·(1 − αs) + αs·αb·B(cb, cs)` (ISO 32000-1 §11.3.6).
pub fn blend_pixel(mode: BlendMode, dst: [f32; 4], src: [f32; 4]) -> [f32; 4] {
    let (ab, as_) = (dst[3], src[3]);
    let un = |c: [f32; 4]| if c[3] > 0.0 { [c[0] / c[3], c[1] / c[3], c[2] / c[3]] } else { [0.0; 3] };
    let (cb, cs) = (un(dst), un(src));
    let b = blend_color(mode, cb, cs);
    let mut out = [0.0; 4];
    for i in 0..3 {
        out[i] = src[i] * (1.0 - ab) + dst[i] * (1.0 - as_) + as_ * ab * b[i];
    }
    out[3] = as_ + ab * (1.0 - as_);
    out
}

fn blend_color(mode: BlendMode, cb: [f32; 3], cs: [f32; 3]) -> [f32; 3] {
    use BlendMode::*;
    let sep = |f: fn(f32, f32) -> f32| [f(cb[0], cs[0]), f(cb[1], cs[1]), f(cb[2], cs[2])];
    fn hard(b: f32, s: f32) -> f32 {
        if s <= 0.5 {
            b * 2.0 * s
        } else {
            let t = 2.0 * s - 1.0;
            b + t - b * t
        }
    }
    match mode {
        Normal => cs,
        Multiply => sep(|b, s| b * s),
        Screen => sep(|b, s| b + s - b * s),
        Overlay => sep(|b, s| hard(s, b)),
        Darken => sep(f32::min),
        Lighten => sep(f32::max),
        ColorDodge => sep(|b, s| {
            if b <= 0.0 {
                0.0
            } else if s >= 1.0 {
                1.0
            } else {
                (b / (1.0 - s)).min(1.0)
            }
        }),
        ColorBurn => sep(|b, s| {
            if b >= 1.0 {
                1.0
            } else if s <= 0.0 {
                0.0
            } else {
                1.0 - ((1.0 - b) / s).min(1.0)
            }
        }),
        HardLight => sep(hard),
        SoftLight => sep(|b, s| {
            if s <= 0.5 {
                b - (1.0 - 2.0 * s) * b * (1.0 - b)
            } else {
                let d = if b <= 0.25 { ((16.0 * b - 12.0) * b + 4.0) * b } else { b.sqrt() };
                b + (2.0 * s - 1.0) * (d - b)
            }
        }),
        Difference => sep(|b, s| (b - s).abs()),
        Exclusion => sep(|b, s| b + s - 2.0 * b * s),
        Hue => set_lum(set_sat(cs, sat(cb)), lum(cb)),
        Saturation => set_lum(set_sat(cb, sat(cs)), lum(cb)),
        Color => set_lum(cs, lum(cb)),
        Luminosity => set_lum(cb, lum(cs)),
    }
}

fn lum(c: [f32; 3]) -> f32 {
    0.3 * c[0] + 0.59 * c[1] + 0.11 * c[2]
}

fn clip_color(c: [f32; 3]) -> [f32; 3] {
    let l = lum(c);
    let n = c[0].min(c[1]).min(c[2]);
    let x = c[0].max(c[1]).max(c[2]);
    let mut c = c;
    if n < 0.0 && l - n > 0.0 {
        c = c.map(|v| l + (v - l) * l / (l - n));
    }
    if x > 1.0 && x - l > 0.0 {
        c = c.map(|v| l + (v - l) * (1.0 - l) / (x - l));
    }
    c
}

fn set_lum(c: [f32; 3], l: f32) -> [f32; 3] {
    let d = l - lum(c);
    clip_color(c.map(|v| v + d))
}

fn sat(c: [f32; 3]) -> f32 {
    c[0].max(c[1]).max(c[2]) - c[0].min(c[1]).min(c[2])
}

fn set_sat(c: [f32; 3], s: f32) -> [f32; 3] {
    let (mx, mn) = (c[0].max(c[1]).max(c[2]), c[0].min(c[1]).min(c[2]));
    if mx - mn <= 0.0 {
        return [0.0; 3];
    }
    c.map(|v| (v - mn) * s / (mx - mn))
}

fn draw_node(n: &Node, m: Affine, opacity: f64, dst: &mut Image, budget: &mut SurfaceBudget) {
    match n {
        Node::Group(g) => draw_group(g, m, opacity, dst, budget),
        Node::Shape(s) => {
            if s.opacity < 1.0 && s.fill.is_some() && s.stroke.is_some() {
                if s.fill.as_ref().is_some_and(|paint| matches!(paint, Paint::Pattern(_)))
                    || s.stroke.as_ref().is_some_and(|stroke| matches!(&stroke.paint, Paint::Pattern(_)))
                {
                    let Some(pixels) = (dst.width as usize).checked_mul(dst.height as usize) else { return };
                    if !budget.charge_pattern_render_work(pixels) {
                        return;
                    }
                }
                let Some(mut tmp) = budget.image(dst.width, dst.height, None) else { return };
                draw_shape(s, m, 1.0, &mut tmp, budget);
                over(dst, &tmp, (opacity * s.opacity) as f32);
                budget.release(&tmp);
            } else {
                draw_shape(s, m, opacity * s.opacity, dst, budget);
            }
        }
        Node::Image(im) => draw_image(im, m, opacity * im.opacity, dst, budget),
    }
}

/// Draw a raster image: bilinear samples inside the image's (anti-aliased) quadrilateral.
fn draw_image(im: &crate::Image, m: Affine, opacity: f64, dst: &mut Image, budget: &mut SurfaceBudget) {
    let (iw, ih) = (im.width as usize, im.height as usize);
    let Some(image_bytes) = iw.checked_mul(ih).and_then(|pixels| pixels.checked_mul(4)) else { return };
    if iw == 0 || ih == 0 || im.rgba.len() < image_bytes || opacity <= 0.0 || dst.width == 0 {
        return;
    }
    let full = m * im.transform;
    let inv = full.inverse();
    if !inv.as_coeffs().iter().all(|v| v.is_finite()) {
        return;
    }
    let quad = kurbo::Rect::new(0.0, 0.0, iw as f64, ih as f64).to_path(0.01);
    let w = dst.width as usize;
    // Pixel footprint in image pixels: box-filter heavy minification instead of aliasing.
    let c = inv.as_coeffs();
    let fx = (c[0] * c[0] + c[1] * c[1]).sqrt();
    let fy = (c[2] * c[2] + c[3] * c[3]).sqrt();
    let taps = (fx.max(fy).ceil() as usize).clamp(1, 8);
    let Some(destination_pixels) = (dst.width as usize).checked_mul(dst.height as usize) else { return };
    let Some(render_work) = destination_pixels.checked_mul(taps).and_then(|work| work.checked_mul(taps)) else { return };
    if !budget.charge_image_render_work(render_work) {
        return;
    }
    let Some(coverage_bytes) = budget.coverage(dst.width, dst.height) else { return };
    let cov = effectcraft_path::fill_coverage(std::slice::from_ref(&quad), &mat3(full), dst.width, dst.height, effectcraft_path::FillRule::NonZero);
    let px = |x: usize, y: usize| -> Option<[f32; 4]> {
        let i = y.checked_mul(iw)?.checked_add(x)?.checked_mul(4)?;
        let end = i.checked_add(4)?;
        let [r, g, b, a] = im.rgba.get(i..end)? else { return None };
        let a = *a as f32 / 255.0;
        Some([*r as f32 / 255.0 * a, *g as f32 / 255.0 * a, *b as f32 / 255.0 * a, a])
    };
    let k = opacity as f32;
    dst.data.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, d) in row.iter_mut().enumerate() {
            let Some(mask_index) = y.checked_mul(w).and_then(|index| index.checked_add(x)) else { continue };
            let Some(&cv) = cov.data.get(mask_index) else { continue };
            if cv <= 0.0 {
                continue;
            }
            let mut acc = [0.0f32; 4];
            for ty in 0..taps {
                for tx in 0..taps {
                    let sx = x as f64 + (tx as f64 + 0.5) / taps as f64;
                    let sy = y as f64 + (ty as f64 + 0.5) / taps as f64;
                    let p = inv * Point::new(sx, sy);
                    let (u, v) = ((p.x - 0.5).clamp(0.0, iw as f64 - 1.0), (p.y - 0.5).clamp(0.0, ih as f64 - 1.0));
                    let (x0, y0) = (u.floor() as usize, v.floor() as usize);
                    let (x1, y1) = ((x0 + 1).min(iw - 1), (y0 + 1).min(ih - 1));
                    let (a, b) = ((u - x0 as f64) as f32, (v - y0 as f64) as f32);
                    let (Some(p00), Some(p10), Some(p01), Some(p11)) = (px(x0, y0), px(x1, y0), px(x0, y1), px(x1, y1)) else { continue };
                    for i in 0..4 {
                        acc[i] += (p00[i] * (1.0 - a) + p10[i] * a) * (1.0 - b) + (p01[i] * (1.0 - a) + p11[i] * a) * b;
                    }
                }
            }
            let n = (taps * taps) as f32;
            let s = acc.map(|v| v / n * cv * k);
            if s[3] <= 0.0 {
                continue;
            }
            for i in 0..3 {
                d[i] = s[i] + d[i] * (1.0 - s[3]);
            }
            d[3] = s[3] + d[3] * (1.0 - s[3]);
        }
    });
    drop(cov);
    budget.release_coverage(coverage_bytes);
}

/// `dst = src·k over dst` (premultiplied).
fn over(dst: &mut Image, src: &Image, k: f32) {
    dst.data.par_iter_mut().zip(src.data.par_iter()).for_each(|(d, s)| {
        let a = s[3] * k;
        if a > 0.0 {
            for c in 0..3 {
                d[c] = s[c] * k + d[c] * (1.0 - a);
            }
            d[3] = a + d[3] * (1.0 - a);
        }
    });
}

fn draw_shape(s: &Shape, m: Affine, opacity: f64, dst: &mut Image, budget: &mut SurfaceBudget) {
    let m = m * s.transform;
    let path = s.geom.to_path();
    let bbox = path.bounding_box();
    let (w, h) = (dst.width, dst.height);
    let pattern_coverage_work = (w as usize).checked_mul(h as usize).and_then(|pixels| pixels.checked_mul(path.elements().len().max(1)));
    if let Some(fill) = &s.fill
        && let Some(coverage_bytes) = budget.coverage(w, h)
    {
        let coverage_allowed = !matches!(fill, Paint::Pattern(_)) || pattern_coverage_work.is_some_and(|work| budget.charge_pattern_render_work(work));
        if coverage_allowed {
            let rule = match s.fill_rule {
                FillRule::NonZero => effectcraft_path::FillRule::NonZero,
                FillRule::EvenOdd => effectcraft_path::FillRule::EvenOdd,
            };
            let cov = effectcraft_path::fill_coverage(std::slice::from_ref(&path), &mat3(m), w, h, rule);
            paint(dst, &cov, fill, (opacity * s.fill_opacity) as f32, m, bbox, budget);
        }
        budget.release_coverage(coverage_bytes);
    }
    if let Some(st) = &s.stroke
        && let Some(coverage_bytes) = budget.coverage(w, h)
    {
        let coverage_allowed = !matches!(&st.paint, Paint::Pattern(_)) || pattern_coverage_work.is_some_and(|work| budget.charge_pattern_render_work(work));
        if coverage_allowed {
            let style = st.style();
            let cov = effectcraft_path::stroke_coverage(std::slice::from_ref(&path), &style, &mat3(m), w, h);
            paint(dst, &cov, &st.paint, (opacity * st.opacity) as f32, m, bbox, budget);
        }
        budget.release_coverage(coverage_bytes);
    }
}

/// Composite paint through coverage.
fn paint(dst: &mut Image, cov: &Mask, p: &Paint, opacity: f32, m: Affine, bbox: kurbo::Rect, budget: &mut SurfaceBudget) {
    if let Paint::Pattern(pattern) = p {
        paint_pattern(dst, cov, pattern, opacity, m, bbox, budget);
        return;
    }
    let shader = match p {
        Paint::Color(c) => Shader::Solid([c[0] as f32, c[1] as f32, c[2] as f32, 1.0]),
        Paint::Gradient(g) => {
            let unit = if g.bbox_units { Affine::new([bbox.width(), 0.0, 0.0, bbox.height(), bbox.x0, bbox.y0]) } else { Affine::IDENTITY };
            let to_grad = (m * unit * g.transform).inverse();
            if !to_grad.as_coeffs().iter().all(|v| v.is_finite()) {
                return;
            }
            Shader::Gradient(g, to_grad)
        }
        Paint::Pattern(_) => return,
    };
    paint_shader(dst, cov, &shader, opacity);
}

/// Render a pattern tile once for this paint operation, then sample it repeatedly over the mask.
fn paint_pattern(dst: &mut Image, cov: &Mask, pattern: &Pattern, opacity: f32, m: Affine, bbox: kurbo::Rect, budget: &mut SurfaceBudget) {
    let fallback = || pattern.fallback.map(|c| Shader::Solid(c.map(|v| v as f32)));
    let fallback_paint = |dst: &mut Image, budget: &mut SurfaceBudget| {
        if let Some(shader) = fallback() {
            let Some(pixels) = (dst.width as usize).checked_mul(dst.height as usize) else { return };
            if budget.charge_pattern_render_work(pixels) {
                paint_shader(dst, cov, &shader, opacity);
            }
        }
    };
    if opacity <= 0.0 || pattern.render_complexity == 0 {
        return;
    }

    let bbox_transform = || {
        let (x, y, width, height) = (bbox.x0, bbox.y0, bbox.width(), bbox.height());
        (x.is_finite() && y.is_finite() && width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0)
            .then(|| Affine::new([width, 0.0, 0.0, height, x, y]))
    };
    let units = match pattern.pattern_units {
        PatternUnits::ObjectBoundingBox => match bbox_transform() {
            Some(transform) => transform,
            None => {
                fallback_paint(dst, budget);
                return;
            }
        },
        PatternUnits::UserSpaceOnUse => Affine::IDENTITY,
    };
    let content_units = match pattern.content_units {
        PatternUnits::ObjectBoundingBox => match bbox_transform() {
            Some(transform) => transform,
            None => {
                fallback_paint(dst, budget);
                return;
            }
        },
        PatternUnits::UserSpaceOnUse => Affine::IDENTITY,
    };
    let tile_to_device = m * units * pattern.transform;
    let device_to_pattern = tile_to_device.inverse();
    let pattern_to_content = units.inverse() * content_units;
    if !tile_to_device.as_coeffs().iter().all(|v| v.is_finite())
        || !device_to_pattern.as_coeffs().iter().all(|v| v.is_finite())
        || !pattern_to_content.as_coeffs().iter().all(|v| v.is_finite())
    {
        fallback_paint(dst, budget);
        return;
    }

    let c = tile_to_device.as_coeffs();
    let pixel_extent = |tile: f64, scale: f64| -> Option<u32> {
        let pixels = tile * scale;
        if !pixels.is_finite() || pixels <= 0.0 || pixels.ceil() > u32::MAX as f64 {
            return None;
        }
        Some(pixels.ceil().max(1.0) as u32)
    };
    let Some(tile_width) = pixel_extent(pattern.width, c[0].hypot(c[1])) else {
        fallback_paint(dst, budget);
        return;
    };
    let Some(tile_height) = pixel_extent(pattern.height, c[2].hypot(c[3])) else {
        fallback_paint(dst, budget);
        return;
    };
    let Some(tile_pixels) = (tile_width as usize).checked_mul(tile_height as usize) else {
        fallback_paint(dst, budget);
        return;
    };
    let Some(destination_pixels) = (dst.width as usize).checked_mul(dst.height as usize) else {
        fallback_paint(dst, budget);
        return;
    };
    let Some(content_work) = tile_pixels.checked_mul(pattern.render_complexity) else {
        fallback_paint(dst, budget);
        return;
    };
    let Some(render_work) = content_work.checked_add(destination_pixels) else {
        fallback_paint(dst, budget);
        return;
    };
    if !budget.charge_pattern_render_work(render_work) {
        fallback_paint(dst, budget);
        return;
    };
    let Some(mut tile) = budget.pattern_tile(tile_width, tile_height) else {
        fallback_paint(dst, budget);
        return;
    };
    let tile_scale = Affine::scale_non_uniform(tile_width as f64 / pattern.width, tile_height as f64 / pattern.height);
    let content_to_tile = tile_scale * Affine::translate((-pattern.x, -pattern.y)) * pattern_to_content;
    if !tile_scale.as_coeffs().iter().all(|v| v.is_finite()) || !content_to_tile.as_coeffs().iter().all(|v| v.is_finite()) {
        budget.release(&tile);
        fallback_paint(dst, budget);
        return;
    }
    draw_group(&pattern.content, content_to_tile, 1.0, &mut tile, budget);
    let shader = Shader::Pattern(pattern, &tile, device_to_pattern);
    paint_shader(dst, cov, &shader, opacity);
    budget.release(&tile);
}

/// Composite a shader through a coverage mask using premultiplied destination pixels.
fn paint_shader(dst: &mut Image, cov: &Mask, shader: &Shader<'_>, opacity: f32) {
    let w = dst.width as usize;
    if w == 0 {
        return;
    }
    dst.data.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, d) in row.iter_mut().enumerate() {
            let Some(c) = cov.data.get(y.saturating_mul(w).saturating_add(x)).copied() else { continue };
            if c <= 0.0 {
                continue;
            }
            let col = shader.at(x as f64 + 0.5, y as f64 + 0.5);
            let a = col[3] * c * opacity;
            if a <= 0.0 {
                continue;
            }
            for k in 0..3 {
                d[k] = col[k] * a + d[k] * (1.0 - a);
            }
            d[3] = a + d[3] * (1.0 - a);
        }
    });
}

enum Shader<'a> {
    Solid([f32; 4]),
    Gradient(&'a Gradient, Affine),
    Pattern(&'a Pattern, &'a Image, Affine),
}

impl Shader<'_> {
    /// Straight RGBA at a pixel centre.
    fn at(&self, x: f64, y: f64) -> [f32; 4] {
        match self {
            Shader::Solid(c) => *c,
            Shader::Gradient(g, inv) => {
                let p = *inv * Point::new(x, y);
                let t = match g.kind {
                    GradientKind::Linear { x1, y1, x2, y2 } => {
                        let (dx, dy) = (x2 - x1, y2 - y1);
                        let l2 = dx * dx + dy * dy;
                        if l2 <= 0.0 { 1.0 } else { ((p.x - x1) * dx + (p.y - y1) * dy) / l2 }
                    }
                    GradientKind::Radial { cx, cy, r, fx, fy } => radial_t(p, cx, cy, r, fx, fy),
                };
                eval_stops(&g.stops, spread(t, g.spread))
            }
            Shader::Pattern(pattern, tile, inv) => sample_pattern(pattern, tile, *inv, x, y),
        }
    }
}

/// Bilinearly sample a repeating pattern tile. Coordinates are transformed into pattern space,
/// shifted by the tile origin, then wrapped in both dimensions before filtering across seams.
fn sample_pattern(pattern: &Pattern, tile: &Image, inv: Affine, x: f64, y: f64) -> [f32; 4] {
    if tile.width == 0 || tile.height == 0 || pattern.width <= 0.0 || pattern.height <= 0.0 {
        return [0.0; 4];
    }
    let p = inv * Point::new(x, y);
    let (width, height) = (pattern.width, pattern.height);
    if !p.x.is_finite() || !p.y.is_finite() {
        return [0.0; 4];
    }
    let u = (p.x - pattern.x).rem_euclid(width) / width * tile.width as f64 - 0.5;
    let v = (p.y - pattern.y).rem_euclid(height) / height * tile.height as f64 - 0.5;
    if !u.is_finite() || !v.is_finite() {
        return [0.0; 4];
    }

    let x0 = u.floor() as i64;
    let y0 = v.floor() as i64;
    let (fx, fy) = ((u - x0 as f64) as f32, (v - y0 as f64) as f32);
    let wrap = |i: i64, size: u32| -> usize { i.rem_euclid(i64::from(size)) as usize };
    let (x0, x1) = (wrap(x0, tile.width), wrap(x0.saturating_add(1), tile.width));
    let (y0, y1) = (wrap(y0, tile.height), wrap(y0.saturating_add(1), tile.height));
    let pixel = |px: usize, py: usize| -> [f32; 4] {
        let index = py.saturating_mul(tile.width as usize).saturating_add(px);
        tile.data.get(index).copied().unwrap_or([0.0; 4])
    };
    let (p00, p10, p01, p11) = (pixel(x0, y0), pixel(x1, y0), pixel(x0, y1), pixel(x1, y1));
    let mut premultiplied = [0.0; 4];
    for i in 0..4 {
        premultiplied[i] = (p00[i] * (1.0 - fx) + p10[i] * fx) * (1.0 - fy) + (p01[i] * (1.0 - fx) + p11[i] * fx) * fy;
    }
    let alpha = premultiplied[3];
    if alpha <= 0.0 || !alpha.is_finite() {
        return [0.0; 4];
    }
    [(premultiplied[0] / alpha).clamp(0.0, 1.0), (premultiplied[1] / alpha).clamp(0.0, 1.0), (premultiplied[2] / alpha).clamp(0.0, 1.0), alpha.clamp(0.0, 1.0)]
}

/// Radial gradient parameter with a focal point: the circle through `p` from the focus.
pub(crate) fn radial_t(p: Point, cx: f64, cy: f64, r: f64, fx: f64, fy: f64) -> f64 {
    if r <= 0.0 {
        return 1.0;
    }
    let (dx, dy) = (p.x - fx, p.y - fy);
    let (ex, ey) = (fx - cx, fy - cy);
    if ex * ex + ey * ey < 1e-12 {
        return (dx * dx + dy * dy).sqrt() / r;
    }
    // Solve |e + u·d|² = r² for u > 0; t = 1/u.
    let a = dx * dx + dy * dy;
    if a < 1e-18 {
        return 0.0;
    }
    let b = ex * dx + ey * dy;
    let c = ex * ex + ey * ey - r * r;
    let disc = b * b - a * c;
    if disc < 0.0 {
        return 1.0;
    }
    let u = (-b + disc.sqrt()) / a;
    if u <= 0.0 { 1.0 } else { 1.0 / u }
}

fn spread(t: f64, s: Spread) -> f64 {
    match s {
        Spread::Pad => t.clamp(0.0, 1.0),
        Spread::Repeat => t.rem_euclid(1.0),
        Spread::Reflect => {
            let r = t.rem_euclid(2.0);
            if r > 1.0 { 2.0 - r } else { r }
        }
    }
}

pub(crate) fn eval_stops(stops: &[(f64, [f64; 4])], t: f64) -> [f32; 4] {
    let Some(first) = stops.first() else { return [0.0; 4] };
    if t <= first.0 {
        return first.1.map(|v| v as f32);
    }
    // Find the first upper stop (not the last duplicate): equal-offset segments retain the
    // reference algorithm's exact-boundary behavior, while nearby values are interpolated from
    // the preceding stop as before.
    let upper = stops.partition_point(|stop| stop.0 < t);
    let Some(b) = stops.get(upper) else { return stops.last().map_or([0.0; 4], |s| s.1.map(|v| v as f32)) };
    let Some(a) = stops.get(upper.saturating_sub(1)) else { return first.1.map(|v| v as f32) };
    let k = if b.0 > a.0 { (t - a.0) / (b.0 - a.0) } else { 1.0 };
    [0, 1, 2, 3].map(|i| (a.1[i] + (b.1[i] - a.1[i]) * k) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pattern_tile_budget_is_cumulative_after_tiles_are_released() {
        let mut budget = SurfaceBudget {
            live_bytes: 0,
            coverage_bytes: 0,
            pattern_tile_bytes: MAX_PATTERN_TILE_BYTES - std::mem::size_of::<[f32; 4]>(),
            pattern_render_work: 0,
            pattern_render_work_limit: MAX_PATTERN_RENDER_WORK,
            clip_render_work: 0,
            clip_render_work_limit: MAX_CLIP_RENDER_WORK,
            image_render_work: 0,
            image_render_work_limit: MAX_IMAGE_RENDER_WORK,
            limit_bytes: MAX_LIVE_SURFACE_BYTES,
        };
        let tile = budget.pattern_tile(1, 1).expect("last tile within the budget");
        budget.release(&tile);
        assert!(budget.pattern_tile(1, 1).is_none(), "a released tile still counts toward cumulative pattern work");
    }

    #[test]
    fn coverage_masks_share_the_live_surface_budget() {
        let mut budget = SurfaceBudget {
            live_bytes: 0,
            coverage_bytes: 0,
            pattern_tile_bytes: 0,
            pattern_render_work: 0,
            pattern_render_work_limit: MAX_PATTERN_RENDER_WORK,
            clip_render_work: 0,
            clip_render_work_limit: MAX_CLIP_RENDER_WORK,
            image_render_work: 0,
            image_render_work_limit: MAX_IMAGE_RENDER_WORK,
            limit_bytes: 4,
        };
        assert!(budget.coverage(1, 1).is_none(), "the returned f32 mask and temporary u8 mask need five bytes");
        budget.limit_bytes = 20;
        let reserved = budget.coverage(1, 1).expect("mask fits at the limit");
        assert_eq!(reserved, 5);
        assert!(budget.image(1, 1, None).is_none(), "live images and coverage masks share one cap");
        budget.release_coverage(reserved);
        assert!(budget.image(1, 1, None).is_some(), "the released coverage budget is available to images");
    }

    #[test]
    fn clip_coverage_reserves_all_masks_and_refuses_excess_work() {
        let mut group = Group::new("clip");
        group.clip.push((kurbo::Rect::new(0.0, 0.0, 1.0, 1.0).to_path(0.01), FillRule::NonZero));
        let mut budget = SurfaceBudget {
            live_bytes: 0,
            coverage_bytes: 0,
            pattern_tile_bytes: 0,
            pattern_render_work: 0,
            pattern_render_work_limit: MAX_PATTERN_RENDER_WORK,
            clip_render_work: 0,
            clip_render_work_limit: MAX_CLIP_RENDER_WORK,
            image_render_work: 0,
            image_render_work_limit: MAX_IMAGE_RENDER_WORK,
            limit_bytes: 9,
        };

        let (coverage, reserved) = clip_coverage(&group, Affine::IDENTITY, 1, 1, &mut budget).expect("clip fits exactly");
        assert_eq!(reserved, 9, "clip output, fill mask, and tiny-skia scratch mask are reserved together");
        assert_eq!(coverage.len(), 1);
        assert_eq!(budget.coverage_bytes, reserved);
        drop(coverage);
        budget.release_coverage(reserved);
        assert_eq!(budget.coverage_bytes, 0);

        budget.limit_bytes = 8;
        assert!(clip_coverage(&group, Affine::IDENTITY, 1, 1, &mut budget).is_none(), "refuse before allocating when clip masks do not fit");
        assert_eq!(budget.coverage_bytes, 0);

        budget.limit_bytes = 9;
        budget.clip_render_work_limit = 0;
        let work_before = budget.clip_render_work;
        assert!(clip_coverage(&group, Affine::IDENTITY, 1, 1, &mut budget).is_none(), "refuse clip raster work before allocating masks");
        assert_eq!(budget.coverage_bytes, 0);
        assert_eq!(budget.clip_render_work, work_before, "refused work is not charged");
    }

    #[test]
    fn image_render_checks_input_size_and_charges_sampling_work() {
        let mut budget = SurfaceBudget {
            live_bytes: 0,
            coverage_bytes: 0,
            pattern_tile_bytes: 0,
            pattern_render_work: 0,
            pattern_render_work_limit: MAX_PATTERN_RENDER_WORK,
            clip_render_work: 0,
            clip_render_work_limit: MAX_CLIP_RENDER_WORK,
            image_render_work: 0,
            image_render_work_limit: 0,
            limit_bytes: MAX_LIVE_SURFACE_BYTES,
        };
        let mut dst = Image::new(1, 1);
        let source = crate::Image {
            name: "pixel".into(),
            transform: Affine::IDENTITY,
            width: 1,
            height: 1,
            rgba: std::sync::Arc::new(vec![255, 0, 0, 255]),
            opacity: 1.0,
        };

        draw_image(&source, Affine::IDENTITY, 1.0, &mut dst, &mut budget);
        assert_eq!(dst.data.first().copied(), Some([0.0; 4]), "excess sampling work is refused before coverage allocation");
        assert_eq!(budget.coverage_bytes, 0);

        let oversized = crate::Image { width: u32::MAX, height: u32::MAX, rgba: std::sync::Arc::new(Vec::new()), ..source };
        draw_image(&oversized, Affine::IDENTITY, 1.0, &mut dst, &mut budget);
        assert_eq!(dst.data.first().copied(), Some([0.0; 4]), "overflowing source dimensions are rejected");
    }

    #[test]
    fn pattern_content_work_exhaustion_uses_a_budgeted_fallback() {
        let doc = crate::parse(
            br##"<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1">
                <defs><pattern id="p" patternUnits="userSpaceOnUse" width="1" height="1">
                    <rect width="1" height="1" fill="red"/>
                </pattern></defs>
                <rect width="1" height="1" fill="url(#p) blue"/>
            </svg>"##,
        )
        .expect("test SVG parses");
        let shapes = doc.flatten();
        let Some(Paint::Pattern(pattern)) = shapes[0].2.fill.as_ref() else { panic!("pattern paint expected") };
        let (mut dst, cov) = (Image::new(1, 1), Mask::new(1, 1, 1.0));
        let mut budget = SurfaceBudget {
            live_bytes: 0,
            coverage_bytes: 0,
            pattern_tile_bytes: 0,
            pattern_render_work: 0,
            pattern_render_work_limit: 1,
            clip_render_work: 0,
            clip_render_work_limit: MAX_CLIP_RENDER_WORK,
            image_render_work: 0,
            image_render_work_limit: MAX_IMAGE_RENDER_WORK,
            limit_bytes: MAX_LIVE_SURFACE_BYTES,
        };

        paint_pattern(&mut dst, &cov, pattern, 1.0, Affine::IDENTITY, kurbo::Rect::new(0.0, 0.0, 1.0, 1.0), &mut budget);

        assert_eq!(dst.data.first().copied(), Some([0.0, 0.0, 1.0, 1.0]));
        assert_eq!(budget.pattern_render_work, 1, "the pattern is refused and fallback sampling is charged");
    }

    #[test]
    fn pattern_coverage_work_is_charged_before_allocating_its_mask() {
        let doc = crate::parse(
            br#"<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1">
                <defs><pattern id="p" patternUnits="userSpaceOnUse" width="1" height="1">
                    <rect width="1" height="1" fill="red"/>
                </pattern></defs>
                <rect width="1" height="1" fill="url(#p)"/>
            </svg>"#,
        )
        .expect("test SVG parses");
        let Some(Node::Shape(shape)) = doc.root.children.first() else { panic!("shape expected") };
        let (mut dst, mut budget) = (
            Image::new(1, 1),
            SurfaceBudget {
                live_bytes: 0,
                coverage_bytes: 0,
                pattern_tile_bytes: 0,
                pattern_render_work: 0,
                pattern_render_work_limit: 1,
                clip_render_work: 0,
                clip_render_work_limit: MAX_CLIP_RENDER_WORK,
                image_render_work: 0,
                image_render_work_limit: MAX_IMAGE_RENDER_WORK,
                limit_bytes: MAX_LIVE_SURFACE_BYTES,
            },
        );

        draw_shape(shape, Affine::IDENTITY, 1.0, &mut dst, &mut budget);

        assert_eq!(dst.data.first().copied(), Some([0.0; 4]));
        assert_eq!(budget.pattern_tile_bytes, 0, "work is refused before a tile or coverage mask is produced");
        assert_eq!(budget.pattern_render_work, 0);
    }
}
