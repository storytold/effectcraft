//! Rasterisation of a parsed SVG into a premultiplied `f32` image (anti-aliased coverage from
//! `effectcraft-path`; group opacity composites the group as a whole, as SVG specifies).

use effectcraft_geom::Mat3;
use effectcraft_raster::{Image, Mask};
use kurbo::{Affine, Point, Shape as _};
use rayon::prelude::*;

use crate::{BlendMode, Doc, FillRule, Gradient, GradientKind, Group, Node, Paint, Shape, SoftMask, Spread};

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
    rasterize_with_budget(doc, w, h, m, MAX_LIVE_SURFACE_BYTES)
}

/// [`rasterize_with`] with at most `limit_bytes` of image surfaces (the destination included)
/// live at once.
pub(crate) fn rasterize_with_budget(doc: &Doc, w: u32, h: u32, m: Affine, limit_bytes: usize) -> Image {
    let mut budget = SurfaceBudget { live_bytes: 0, limit_bytes, depth: 0, plain: false };
    let Some(mut img) = budget.image(w, h, None) else { return Image::default() };
    draw_group(&doc.root, m, 1.0, &mut img, &mut budget);
    img
}

/// Bound the simultaneously live offscreen image surfaces created by nested groups.
const MAX_LIVE_SURFACE_BYTES: usize = 512 * 1024 * 1024;

/// The deepest group nesting drawn: drawing recurses, and a crafted document can nest groups
/// without end, so groups deeper than this are not drawn.
const MAX_GROUP_DEPTH: usize = 256;

/// The state of one rasterisation: the surface budget and where the drawing is in the tree.
struct SurfaceBudget {
    live_bytes: usize,
    limit_bytes: usize,
    /// Groups entered on the way to the one being drawn.
    depth: usize,
    /// Every group being drawn below the current one composites Normal, without knockout
    /// (known once for a whole subtree, so it isn't walked again at every level).
    plain: bool,
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
    if budget.depth >= MAX_GROUP_DEPTH {
        return;
    }
    budget.depth += 1;
    draw_group_entered(g, m, opacity, dst, budget);
    budget.depth -= 1;
}

fn draw_group_entered(g: &Group, m: Affine, opacity: f64, dst: &mut Image, budget: &mut SurfaceBudget) {
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
    let only_clip = !g.clip.is_empty() && g.mask.is_none() && g.blend == BlendMode::Normal && o >= 1.0 && !g.knockout;
    let plain = only_clip && (budget.plain || !needs_backdrop(&g.children, MAX_GROUP_DEPTH.saturating_sub(budget.depth)));
    if plain {
        let outer = std::mem::replace(&mut budget.plain, true);
        let left = MAX_GROUP_DEPTH.saturating_sub(budget.depth);
        if clips_cover_content(g, m, dst.width, dst.height, left) {
            for c in &g.children {
                draw_node(c, m, 1.0, dst, budget);
            }
        } else {
            draw_clip_local(g, m, dst, budget);
        }
        budget.plain = outer;
        return;
    }
    draw_offscreen_group(g, m, o, dst, budget);
}

/// Whether every clip of `g` is a rectangle along the pixel grid's axes that fully covers each
/// pixel of a `w`×`h` image its children can draw on: the clips change nothing there, so the
/// children are drawn straight onto the image, without a surface (a page or artboard clip
/// around the artwork inside it, the outermost groups of most PDF pages). `levels`: how many
/// levels of groups below are drawn.
fn clips_cover_content(g: &Group, m: Affine, w: u32, h: u32, levels: usize) -> bool {
    let Some(rects) = g.clip.iter().map(|(p, _)| axis_rect(p, m)).collect::<Option<Vec<_>>>() else { return false };
    // Children that draw nothing need no clip.
    let Some(b) = draw_bounds(&g.children, m, levels) else { return true };
    let (fw, fh) = (f64::from(w), f64::from(h));
    // The pixels the children can touch (a pixel of margin for anti-aliasing), on the image.
    let (bx0, by0) = ((b.x0.floor() - 1.0).max(0.0), (b.y0.floor() - 1.0).max(0.0));
    let (bx1, by1) = ((b.x1.ceil() + 1.0).min(fw), (b.y1.ceil() + 1.0).min(fh));
    if !(bx0 < bx1 && by0 < by1) {
        // Nothing reaches the image (or the bounds aren't numbers): keep the clip.
        return false;
    }
    rects.iter().all(|r| r.x0.ceil() <= bx0 && bx1 <= r.x1.floor() && r.y0.ceil() <= by0 && by1 <= r.y1.floor())
}

/// A device-space rectangle holding every pixel `nodes` drawn with `m` can cover, `None` when
/// they draw nothing. Cheap rather than tight: a stroke counts as its path's bounds grown by half
/// its width times its miter limit (at least √2, for square caps), and anything it can't
/// bound (more than `levels` levels deep, a transform that isn't finite) as the whole plane.
fn draw_bounds(nodes: &[Node], m: Affine, levels: usize) -> Option<kurbo::Rect> {
    let everything = kurbo::Rect::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::INFINITY, f64::INFINITY);
    if !m.as_coeffs().iter().all(|v| v.is_finite()) {
        return Some(everything);
    }
    let mut out: Option<kurbo::Rect> = None;
    for n in nodes {
        let r = match n {
            Node::Group(g) if levels == 0 => (!g.children.is_empty()).then_some(everything),
            Node::Group(g) => {
                let gm = m * g.transform;
                let mut r = draw_bounds(&g.children, gm, levels - 1);
                for (clip, _) in &g.clip {
                    r = r.map(|r| r.intersect(gm.transform_rect_bbox(clip.bounding_box())));
                }
                r
            }
            Node::Shape(s) => {
                let sm = m * s.transform;
                let b = s.geom.to_path().bounding_box();
                let fill = s.fill.as_ref().map(|_| sm.transform_rect_bbox(b));
                let stroke = s.stroke.as_ref().map(|st| {
                    let pad = (st.width.abs() * 0.5 * st.miter.max(std::f64::consts::SQRT_2)).max(0.0);
                    sm.transform_rect_bbox(b.inflate(pad, pad))
                });
                match (fill, stroke) {
                    (Some(a), Some(b)) => Some(a.union(b)),
                    (a, b) => a.or(b),
                }
            }
            Node::Image(im) => Some((m * im.transform).transform_rect_bbox(kurbo::Rect::new(0.0, 0.0, f64::from(im.width), f64::from(im.height)))),
        };
        if let Some(r) = r {
            let r = if [r.x0, r.y0, r.x1, r.y1].iter().any(|v| v.is_nan()) { everything } else { r };
            out = Some(out.map_or(r, |o| o.union(r)));
        }
    }
    out
}

/// The device-space rectangle a clip path is when it is one along the pixel grid's axes: a
/// move, three or four lines and an optional close, its sides alternately horizontal and
/// vertical.
pub(crate) fn axis_rect(path: &kurbo::BezPath, m: Affine) -> Option<kurbo::Rect> {
    use kurbo::PathEl;
    let els = path.elements();
    let mut pts: Vec<Point> = Vec::with_capacity(5);
    for (i, el) in els.iter().enumerate() {
        match el {
            PathEl::MoveTo(p) if i == 0 => pts.push(m * *p),
            PathEl::LineTo(p) if i > 0 && pts.len() < 5 => pts.push(m * *p),
            PathEl::ClosePath if i > 0 && i + 1 == els.len() => {}
            _ => return None,
        }
    }
    const EPS: f64 = 1e-6;
    let same = |a: Point, b: Point| (a.x - b.x).abs() <= EPS && (a.y - b.y).abs() <= EPS;
    if pts.len() == 5 && pts.first().zip(pts.last()).is_some_and(|(a, b)| same(*a, *b)) {
        pts.pop();
    }
    let p: [Point; 4] = pts.try_into().ok()?;
    let sides = [(p[0], p[1]), (p[1], p[2]), (p[2], p[3]), (p[3], p[0])];
    // Each side horizontal or vertical (and not a point), the next one the other way.
    let horizontal = |(a, b): (Point, Point)| -> Option<bool> {
        let (dx, dy) = ((b.x - a.x).abs(), (b.y - a.y).abs());
        match (dx > EPS, dy > EPS) {
            (true, false) => Some(true),
            (false, true) => Some(false),
            _ => None,
        }
    };
    let kinds = sides.map(horizontal);
    let [Some(k0), Some(k1), Some(k2), Some(k3)] = kinds else { return None };
    if k0 == k1 || k1 == k2 || k2 == k3 || k3 == k0 {
        return None;
    }
    let r = kurbo::Rect::from_points(p[0], p[2]);
    [r.x0, r.y0, r.x1, r.y1].iter().all(|v| v.is_finite()).then_some(r)
}

/// Whether drawing `nodes` reads what is below them beyond Normal compositing: a blend mode or
/// a knockout group anywhere inside. Past `levels` levels of groups the answer is yes.
fn needs_backdrop(nodes: &[Node], levels: usize) -> bool {
    nodes.iter().any(|n| match n {
        Node::Group(g) => levels == 0 || g.blend != BlendMode::Normal || g.knockout || needs_backdrop(&g.children, levels - 1),
        Node::Shape(_) | Node::Image(_) => false,
    })
}

/// A plain clipping group (no soft mask, blend mode, group opacity or knockout) whose content
/// composites Normal throughout: drawn into a surface the size of its clip's bounds and
/// composited over `dst`. With Normal content that is the same as drawing it over a copy of
/// the backdrop and keeping the clip (`(C over B)·c + B·(1 − c) = (C·c) over B`), without two
/// full-size copies per nesting level: a page continuously rasterised at 400 % lost its
/// nested clipped artwork to the surface budget (#2895).
fn draw_clip_local(g: &Group, m: Affine, dst: &mut Image, budget: &mut SurfaceBudget) {
    let mut bounds: Option<kurbo::Rect> = None;
    for (path, _) in &g.clip {
        let b = m.transform_rect_bbox(path.bounding_box());
        bounds = Some(bounds.map_or(b, |a| a.intersect(b)));
    }
    let Some((x0, y0, w, h)) = bounds.and_then(|b| pixel_rect(b, dst.width, dst.height)) else { return };
    let Some(mut tmp) = budget.image(w, h, None) else { return };
    let local = to_local((x0, y0), m);
    for c in &g.children {
        draw_node(c, local, 1.0, &mut tmp, budget);
    }
    let cov = clip_coverage(g, local, w, h);
    tmp.data.par_iter_mut().zip(cov.par_iter()).for_each(|(p, c)| {
        for v in p.iter_mut() {
            *v *= c;
        }
    });
    over_at(dst, &tmp, (x0, y0), 1.0);
    budget.release(&tmp);
}

/// The pixels of a `w`×`h` image that the device-space rectangle `r` touches, with a pixel of
/// margin for anti-aliasing, as `(x, y, width, height)`; `None` when it misses the image. A
/// rectangle that isn't finite stands for the whole image.
fn pixel_rect(r: kurbo::Rect, w: u32, h: u32) -> Option<(u32, u32, u32, u32)> {
    if w == 0 || h == 0 {
        return None;
    }
    if ![r.x0, r.y0, r.x1, r.y1].iter().all(|v| v.is_finite()) {
        return Some((0, 0, w, h));
    }
    let (fw, fh) = (f64::from(w), f64::from(h));
    let x0 = (r.x0.min(r.x1) - 1.0).floor().clamp(0.0, fw);
    let y0 = (r.y0.min(r.y1) - 1.0).floor().clamp(0.0, fh);
    let x1 = (r.x0.max(r.x1) + 1.0).ceil().clamp(0.0, fw);
    let y1 = (r.y0.max(r.y1) + 1.0).ceil().clamp(0.0, fh);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    Some((x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32))
}

/// A translation by `-origin`: maps device space onto a surface whose top-left pixel sits at
/// `origin` of the device image.
fn to_local(origin: (u32, u32), m: Affine) -> Affine {
    Affine::translate((-f64::from(origin.0), -f64::from(origin.1))) * m
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
            clip_onto_backdrop(g, m, dst, &tmp);
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
        let cov = clip_coverage(g, m, dst.width, dst.height);
        tmp.data.par_iter_mut().zip(cov.par_iter()).for_each(|(p, c)| {
            for v in p.iter_mut() {
                *v *= c;
            }
        });
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
fn clip_onto_backdrop(g: &Group, m: Affine, dst: &mut Image, tmp: &Image) {
    let cov = clip_coverage(g, m, dst.width, dst.height);
    dst.data.par_iter_mut().zip(tmp.data.par_iter()).zip(cov.par_iter()).for_each(|((d, t), c)| {
        for i in 0..4 {
            d[i] = d[i] * (1.0 - c) + t[i] * c;
        }
    });
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
    // The mask's content is a tree of its own: what is known of the masked group's isn't.
    let plain = std::mem::replace(&mut budget.plain, false);
    draw_group(&sm.content, m, 1.0, &mut mk, budget);
    budget.plain = plain;
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
    // Coverage over the pixels the image's quadrilateral reaches only (see `draw_shape`).
    let rect = kurbo::Rect::new(0.0, 0.0, iw as f64, ih as f64);
    let Some((x0, y0, rw, rh)) = pixel_rect(full.transform_rect_bbox(rect), dst.width, dst.height) else { return };
    let quad = rect.to_path(0.01);
    let cov = effectcraft_path::fill_coverage(std::slice::from_ref(&quad), &mat3(to_local((x0, y0), full)), rw, rh, effectcraft_path::FillRule::NonZero);
    let (w, cw) = (dst.width as usize, cov.width as usize);
    if w == 0 || cw == 0 {
        return;
    }
    let (ox, oy) = (x0 as usize, y0 as usize);
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
    dst.data.par_chunks_mut(w).enumerate().skip(oy).zip(cov.data.par_chunks(cw)).for_each(|((y, row), crow)| {
        let Some(seg) = row.get_mut(ox..) else { return };
        for (lx, (d, &cv)) in seg.iter_mut().zip(crow).enumerate() {
            let x = ox + lx;
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

/// `dst = src·k over dst` for `src` placed with its top-left pixel at `origin` of `dst`.
fn over_at(dst: &mut Image, src: &Image, origin: (u32, u32), k: f32) {
    let (w, sw) = (dst.width as usize, src.width as usize);
    if w == 0 || sw == 0 {
        return;
    }
    let (x0, y0) = (origin.0 as usize, origin.1 as usize);
    dst.data.par_chunks_mut(w).skip(y0).zip(src.data.par_chunks(sw)).for_each(|(row, srow)| {
        let Some(seg) = row.get_mut(x0..) else { return };
        for (d, s) in seg.iter_mut().zip(srow) {
            let a = s[3] * k;
            if a > 0.0 {
                for c in 0..3 {
                    d[c] = s[c] * k + d[c] * (1.0 - a);
                }
                d[3] = a + d[3] * (1.0 - a);
            }
        }
    });
}

/// Draw a shape. Its coverage is computed over the pixels its fill or stroke can reach only,
/// not the whole image: a page drawn at a large scale cost a full-size coverage pass per path
/// and per run of text (#2895).
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
        if let Some((x0, y0, rw, rh)) = pixel_rect(m.transform_rect_bbox(bbox), w, h) {
            let cov = effectcraft_path::fill_coverage(std::slice::from_ref(&path), &mat3(to_local((x0, y0), m)), rw, rh, rule);
            paint(dst, &cov, (x0, y0), fill, (opacity * s.fill_opacity) as f32, m, bbox);
        }
    }
    if let Some(st) = &s.stroke {
        // The outline as `effectcraft_path::stroke_coverage` makes it (the resolution scale of
        // the whole transform, which a translation doesn't change).
        let style = st.style();
        if let Some(outline) = effectcraft_path::stroke_outline(std::slice::from_ref(&path), &style, mat3(m).mean_scale())
            && let Some((x0, y0, rw, rh)) = pixel_rect(m.transform_rect_bbox(outline.bounding_box()), w, h)
        {
            let cov =
                effectcraft_path::fill_coverage(std::slice::from_ref(&outline), &mat3(to_local((x0, y0), m)), rw, rh, effectcraft_path::FillRule::NonZero);
            paint(dst, &cov, (x0, y0), &st.paint, (opacity * st.opacity) as f32, m, bbox);
        }
    }
}

/// Composite paint through coverage `cov`, whose top-left pixel sits at `origin` of `dst`.
fn paint(dst: &mut Image, cov: &Mask, origin: (u32, u32), p: &Paint, opacity: f32, m: Affine, bbox: kurbo::Rect) {
    let (w, cw) = (dst.width as usize, cov.width as usize);
    if w == 0 || cw == 0 {
        return;
    }
    let (x0, y0) = (origin.0 as usize, origin.1 as usize);
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
    dst.data.par_chunks_mut(w).enumerate().skip(y0).zip(cov.data.par_chunks(cw)).for_each(|((y, row), crow)| {
        let Some(seg) = row.get_mut(x0..) else { return };
        for (lx, (d, &c)) in seg.iter_mut().zip(crow).enumerate() {
            if c <= 0.0 {
                continue;
            }
            let col = shader.at((x0 + lx) as f64 + 0.5, y as f64 + 0.5);
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
