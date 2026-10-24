//! Rasterisation of a parsed SVG into a premultiplied `f32` image (anti-aliased coverage from
//! `effectcraft-path`; group opacity composites the group as a whole, as SVG specifies).

use effectcraft_geom::Mat3;
use effectcraft_raster::{Image, Mask};
use kurbo::{Affine, Point, Shape as _};
use rayon::prelude::*;

use crate::{BlendMode, Doc, FillRule, Gradient, GradientKind, Group, Node, Paint, Shape, Spread};

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
    let mut budget = SurfaceBudget { live_bytes: 0, limit_bytes: MAX_LIVE_SURFACE_BYTES };
    let Some(mut img) = budget.image(w, h, None) else { return Image::default() };
    draw_group(&doc.root, m, 1.0, &mut img, &mut budget);
    img
}

/// Bound the simultaneously live offscreen image surfaces created by nested groups.
const MAX_LIVE_SURFACE_BYTES: usize = 512 * 1024 * 1024;

struct SurfaceBudget {
    live_bytes: usize,
    limit_bytes: usize,
}

impl SurfaceBudget {
    fn image(&mut self, width: u32, height: u32, copy: Option<&Image>) -> Option<Image> {
        let pixels = (width as usize).checked_mul(height as usize)?;
        let bytes = pixels.checked_mul(std::mem::size_of::<[f32; 4]>())?;
        let new_live = self.live_bytes.checked_add(bytes)?;
        if new_live > self.limit_bytes {
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

    fn release(&mut self, image: &Image) {
        self.live_bytes = self.live_bytes.saturating_sub(image.data.len().saturating_mul(std::mem::size_of::<[f32; 4]>()));
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
    let only_clip = g.mask.is_none() && g.blend == BlendMode::Normal && o >= 1.0 && !g.knockout;
    // Draw the children on their own (isolated) or over a copy of the backdrop, keep what
    // every clip path covers and the soft mask lets through, then composite with the group's
    // opacity and blend mode.
    let backdrop = if g.isolated { None } else { budget.image(dst.width, dst.height, Some(dst)) };
    if !g.isolated && backdrop.is_none() {
        return;
    }
    let mut tmp = match &backdrop {
        Some(b) => match budget.image(dst.width, dst.height, Some(b)) {
            Some(image) => image,
            None => {
                budget.release(b);
                return;
            }
        },
        None => match budget.image(dst.width, dst.height, None) {
            Some(image) => image,
            None => return,
        },
    };
    draw_children(g, m, &mut tmp, backdrop.as_ref(), budget);
    if let Some(b) = &backdrop {
        if only_clip {
            // A non-isolated clipping group: the backdrop where the clip excludes the children.
            let cov = clip_coverage(g, m, dst.width, dst.height);
            dst.data.par_iter_mut().zip(tmp.data.par_iter()).zip(cov.par_iter()).for_each(|((d, t), c)| {
                for i in 0..4 {
                    d[i] = d[i] * (1.0 - c) + t[i] * c;
                }
            });
            budget.release(&tmp);
            drop(tmp);
            if let Some(b) = &backdrop {
                budget.release(b);
            }
            return;
        }
        // Remove the backdrop's contribution (ISO 32000-1 §11.4.8): with the group's own
        // alpha αg (its children alone), C = Cn + (Cn − C0)·(α0/αg − α0).
        let Some(mut solo) = budget.image(dst.width, dst.height, None) else {
            budget.release(&tmp);
            drop(tmp);
            if let Some(b) = &backdrop {
                budget.release(b);
            }
            return;
        };
        draw_children(g, m, &mut solo, None, budget);
        tmp.data.par_iter_mut().zip(b.data.par_iter()).zip(solo.data.par_iter()).for_each(|((t, b0), s)| {
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
    }
    if !g.clip.is_empty() {
        let cov = clip_coverage(g, m, dst.width, dst.height);
        tmp.data.par_iter_mut().zip(cov.par_iter()).for_each(|(p, c)| {
            for v in p.iter_mut() {
                *v *= c;
            }
        });
    }
    if let Some(sm) = &g.mask {
        let Some(mut mk) = budget.image(dst.width, dst.height, None) else {
            budget.release(&tmp);
            drop(tmp);
            if let Some(b) = &backdrop {
                budget.release(b);
            }
            return;
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
    drop(tmp);
    if let Some(b) = &backdrop {
        budget.release(b);
    }
}

/// The product of a group's clip-path coverages.
fn clip_coverage(g: &Group, m: Affine, w: u32, h: u32) -> Vec<f32> {
    let mut out = vec![1.0f32; w as usize * h as usize];
    for (path, rule) in &g.clip {
        let rule = match rule {
            FillRule::NonZero => effectcraft_path::FillRule::NonZero,
            FillRule::EvenOdd => effectcraft_path::FillRule::EvenOdd,
        };
        let cov = effectcraft_path::fill_coverage(std::slice::from_ref(path), &mat3(m), w, h, rule);
        out.par_iter_mut().zip(cov.data.par_iter()).for_each(|(o, c)| *o *= c);
    }
    out
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
                let Some(mut tmp) = budget.image(dst.width, dst.height, None) else { return };
                draw_shape(s, m, 1.0, &mut tmp);
                over(dst, &tmp, (opacity * s.opacity) as f32);
                budget.release(&tmp);
            } else {
                draw_shape(s, m, opacity * s.opacity, dst);
            }
        }
        Node::Image(im) => draw_image(im, m, opacity * im.opacity, dst),
    }
}

/// Draw a raster image: bilinear samples inside the image's (anti-aliased) quadrilateral.
fn draw_image(im: &crate::Image, m: Affine, opacity: f64, dst: &mut Image) {
    let (iw, ih) = (im.width as usize, im.height as usize);
    if iw == 0 || ih == 0 || im.rgba.len() < iw * ih * 4 || opacity <= 0.0 {
        return;
    }
    let full = m * im.transform;
    let inv = full.inverse();
    if !inv.as_coeffs().iter().all(|v| v.is_finite()) {
        return;
    }
    let quad = kurbo::Rect::new(0.0, 0.0, iw as f64, ih as f64).to_path(0.01);
    let cov = effectcraft_path::fill_coverage(std::slice::from_ref(&quad), &mat3(full), dst.width, dst.height, effectcraft_path::FillRule::NonZero);
    let w = dst.width as usize;
    if w == 0 {
        return;
    }
    let px = |x: usize, y: usize| -> [f32; 4] {
        let i = (y * iw + x) * 4;
        let a = im.rgba[i + 3] as f32 / 255.0;
        [im.rgba[i] as f32 / 255.0 * a, im.rgba[i + 1] as f32 / 255.0 * a, im.rgba[i + 2] as f32 / 255.0 * a, a]
    };
    // Pixel footprint in image pixels: box-filter heavy minification instead of aliasing.
    let c = inv.as_coeffs();
    let fx = (c[0] * c[0] + c[1] * c[1]).sqrt();
    let fy = (c[2] * c[2] + c[3] * c[3]).sqrt();
    let taps = (fx.max(fy).ceil() as usize).clamp(1, 8);
    let k = opacity as f32;
    dst.data.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, d) in row.iter_mut().enumerate() {
            let cv = cov.data[y * w + x];
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
                    let (p00, p10, p01, p11) = (px(x0, y0), px(x1, y0), px(x0, y1), px(x1, y1));
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

fn draw_shape(s: &Shape, m: Affine, opacity: f64, dst: &mut Image) {
    let m = m * s.transform;
    let path = s.geom.to_path();
    let bbox = path.bounding_box();
    let (w, h) = (dst.width, dst.height);
    if let Some(fill) = &s.fill {
        let rule = match s.fill_rule {
            FillRule::NonZero => effectcraft_path::FillRule::NonZero,
            FillRule::EvenOdd => effectcraft_path::FillRule::EvenOdd,
        };
        let cov = effectcraft_path::fill_coverage(std::slice::from_ref(&path), &mat3(m), w, h, rule);
        paint(dst, &cov, fill, (opacity * s.fill_opacity) as f32, m, bbox);
    }
    if let Some(st) = &s.stroke {
        let style = st.style();
        let cov = effectcraft_path::stroke_coverage(std::slice::from_ref(&path), &style, &mat3(m), w, h);
        paint(dst, &cov, &st.paint, (opacity * st.opacity) as f32, m, bbox);
    }
}

/// Composite paint through coverage.
fn paint(dst: &mut Image, cov: &Mask, p: &Paint, opacity: f32, m: Affine, bbox: kurbo::Rect) {
    let w = dst.width as usize;
    if w == 0 {
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
    };
    dst.data.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, d) in row.iter_mut().enumerate() {
            let c = cov.data[y * w + x];
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
        }
    }
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
