//! Rasterisation of a parsed SVG into a premultiplied `f32` image (anti-aliased coverage from
//! `effectcraft-path`; group opacity composites the group as a whole, as SVG specifies).
//!
//! Every render runs under a [`SurfaceBudget`]. Masks and pixel scans are restricted to the
//! device-space [`Region`] an item can touch, so many small items on a large canvas are cheap;
//! whole-canvas passes (offscreen groups) are charged to a render-wide pixel-work budget. When a
//! budget refuses work the first refusal is recorded and surfaced by [`try_rasterize`].

use effectcraft_geom::Mat3;
use effectcraft_raster::{Image, Mask};
use kurbo::{Affine, Point, Shape as _};
use rayon::prelude::*;

use crate::{BlendMode, Doc, FillRule, Gradient, GradientKind, Group, Node, Paint, Pattern, PatternUnits, Shape, SoftMask, Spread};

fn mat3(a: Affine) -> Mat3 {
    let c = a.as_coeffs();
    Mat3([[c[0], c[2], c[4]], [c[1], c[3], c[5]], [0.0, 0.0, 1.0]])
}

/// Why a render was refused: a resource budget would have been exceeded, so some content was
/// skipped (or, for [`RasterError::SurfaceLimit`] on the canvas itself, nothing was drawn).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RasterError {
    /// A memory limit (canvas, offscreen surface, coverage mask or pattern tile) was exceeded.
    SurfaceLimit(&'static str),
    /// A render-wide work limit (pixels, clip, image sampling or pattern content) was exceeded.
    WorkLimit(&'static str),
}

impl std::fmt::Display for RasterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RasterError::SurfaceLimit(what) => write!(f, "SVG render refused: {what} exceeds the memory limit"),
            RasterError::WorkLimit(what) => write!(f, "SVG render refused: {what} exceeds the work limit"),
        }
    }
}

impl std::error::Error for RasterError {}

/// Rasterise `doc` into a `w`×`h` image, scaled by `scale` (1 = the document's pixel size).
///
/// Content refused by a resource budget is silently dropped; use [`try_rasterize`] to detect that.
pub fn rasterize(doc: &Doc, w: u32, h: u32, scale: f64) -> Image {
    rasterize_with(doc, w, h, Affine::scale(scale))
}

/// Rasterise `doc` with `m` mapping document pixels to image pixels.
///
/// Content refused by a resource budget is silently dropped (the image drawn so far is returned,
/// or an empty image when the canvas itself is refused); use [`try_rasterize_with`] to detect that.
pub fn rasterize_with(doc: &Doc, w: u32, h: u32, m: Affine) -> Image {
    render(doc, w, h, m).0
}

/// Like [`rasterize`], but reports a refusal as an error instead of returning a partial image.
pub fn try_rasterize(doc: &Doc, w: u32, h: u32, scale: f64) -> Result<Image, RasterError> {
    try_rasterize_with(doc, w, h, Affine::scale(scale))
}

/// Like [`rasterize_with`], but returns the first budget refusal as an error.
pub fn try_rasterize_with(doc: &Doc, w: u32, h: u32, m: Affine) -> Result<Image, RasterError> {
    match render(doc, w, h, m) {
        (image, None) => Ok(image),
        (_, Some(error)) => Err(error),
    }
}

/// Render `doc`, returning the image drawn so far and the first budget refusal, if any.
fn render(doc: &Doc, w: u32, h: u32, m: Affine) -> (Image, Option<RasterError>) {
    // Nested knockout and non-isolated groups draw their children more than once; the number of
    // node draws is bounded by a multiple of the tree's size, past which drawing stops.
    let mut budget = SurfaceBudget::new();
    budget.draws_left = node_count(&doc.root).saturating_mul(MAX_DRAWS_PER_NODE).max(MIN_DRAWS);
    let Some(mut img) = budget.image(w, h, None) else { return (Image::default(), budget.refusal) };
    draw_group(&doc.root, m, 1.0, &mut img, &mut budget);
    (img, budget.refusal)
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
/// Bound the pixels touched by every pass of one render: surface allocations and whole-canvas
/// composites plus the region area of each fill, stroke, image and clip. Region restriction keeps
/// small items near zero; this stops many large items from adding up to hours of work.
const MAX_PIXEL_WORK: u64 = 1 << 31;

/// How many times each node of a document may be drawn, on average, in one rasterisation.
const MAX_DRAWS_PER_NODE: usize = 64;
const MIN_DRAWS: usize = 100_000;

/// The nodes in `g`'s tree, `g` included.
fn node_count(g: &Group) -> usize {
    let mut n = 0usize;
    let mut stack = vec![g];
    while let Some(g) = stack.pop() {
        n = n.saturating_add(1);
        for c in &g.children {
            match c {
                Node::Group(cg) => stack.push(cg),
                _ => n = n.saturating_add(1),
            }
        }
    }
    n
}

/// A pixel rectangle on the canvas, half-open: `x0..x1` by `y0..y1`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Region {
    /// Left edge.
    x0: u32,
    /// Top edge.
    y0: u32,
    /// Right edge (exclusive).
    x1: u32,
    /// Bottom edge (exclusive).
    y1: u32,
}

impl Region {
    /// Width in pixels.
    fn width(self) -> u32 {
        self.x1.saturating_sub(self.x0)
    }

    /// Height in pixels.
    fn height(self) -> u32 {
        self.y1.saturating_sub(self.y0)
    }

    /// Pixel count.
    fn area(self) -> usize {
        (self.width() as usize).saturating_mul(self.height() as usize)
    }

    /// The overlap of two regions, if any.
    fn intersect(self, other: Region) -> Option<Region> {
        let r = Region { x0: self.x0.max(other.x0), y0: self.y0.max(other.y0), x1: self.x1.min(other.x1), y1: self.y1.min(other.y1) };
        (r.x1 > r.x0 && r.y1 > r.y0).then_some(r)
    }

    /// The shift that maps canvas coordinates to coordinates relative to this region.
    fn origin_shift(self) -> Affine {
        Affine::translate((-(self.x0 as f64), -(self.y0 as f64)))
    }
}

/// Device-space rectangle of `rect` under `m`, grown by `pad` device pixels, clipped to the
/// canvas, or `None` when nothing is visible. Non-finite geometry is treated conservatively as
/// covering the whole canvas.
fn device_region(rect: kurbo::Rect, m: Affine, pad: f64, w: u32, h: u32) -> Option<Region> {
    let full = || (w > 0 && h > 0).then_some(Region { x0: 0, y0: 0, x1: w, y1: h });
    let (cw, ch) = (w as f64, h as f64);
    let corners = [(rect.x0, rect.y0), (rect.x1, rect.y0), (rect.x0, rect.y1), (rect.x1, rect.y1)].map(|(x, y)| m * Point::new(x, y));
    // `f64::min`/`max` ignore NaN, so check every corner before folding.
    if corners.iter().any(|p| !(p.x.is_finite() && p.y.is_finite())) {
        return full();
    }
    let (mut x0, mut y0, mut x1, mut y1) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for p in corners {
        x0 = x0.min(p.x);
        y0 = y0.min(p.y);
        x1 = x1.max(p.x);
        y1 = y1.max(p.y);
    }
    let pad = if pad.is_finite() { pad.max(0.0) } else { f64::INFINITY };
    let (x0, y0, x1, y1) = (x0 - pad, y0 - pad, x1 + pad, y1 + pad);
    if !(x0.is_finite() && y0.is_finite() && x1.is_finite() && y1.is_finite()) {
        return full();
    }
    let (x0, y0) = (x0.floor().clamp(0.0, cw), y0.floor().clamp(0.0, ch));
    let (x1, y1) = (x1.ceil().clamp(0.0, cw), y1.ceil().clamp(0.0, ch));
    (x1 > x0 && y1 > y0).then_some(Region { x0: x0 as u32, y0: y0 as u32, x1: x1 as u32, y1: y1 as u32 })
}

/// Region a path (optionally stroked with `stroke`) can touch on the canvas.
fn path_device_region(bbox: kurbo::Rect, m: Affine, stroke: Option<&crate::Stroke>, w: u32, h: u32) -> Option<Region> {
    let c = m.as_coeffs();
    let norm = (c[0] * c[0] + c[1] * c[1] + c[2] * c[2] + c[3] * c[3]).sqrt();
    // Anti-aliasing fringe plus the stroke's reach (half width, miter join, square cap).
    let pad = 2.0 + stroke.map_or(0.0, |st| st.width.abs() * 0.5 * st.miter.max(1.5) * norm);
    device_region(bbox, m, pad, w, h)
}

/// Coverage of `path` under `m` over `region` only; the mask is `region`-sized, with its origin at
/// the region's top-left.
fn region_fill_coverage(path: &kurbo::BezPath, m: Affine, rule: effectcraft_path::FillRule, region: Region) -> Mask {
    let m = region.origin_shift() * m;
    effectcraft_path::fill_coverage(std::slice::from_ref(path), &mat3(m), region.width(), region.height(), rule)
}

/// Run `f(y, row_segment)` in parallel over the rows of `region`, where `row_segment` is the
/// region's `x0..x1` span of canvas row `y`.
fn for_region_rows<F>(dst: &mut Image, region: Region, f: F)
where
    F: Fn(usize, &mut [[f32; 4]]) + Sync + Send,
{
    let w = dst.width as usize;
    if w == 0 {
        return;
    }
    let (x0, x1, y0, y1) = (region.x0 as usize, region.x1 as usize, region.y0 as usize, region.y1 as usize);
    let Some(rows) = dst.data.get_mut(y0.saturating_mul(w)..y1.saturating_mul(w)) else { return };
    rows.par_chunks_mut(w).enumerate().for_each(|(i, row)| {
        if let Some(segment) = row.get_mut(x0..x1) {
            f(y0 + i, segment);
        }
    });
}

/// Render-wide accounting of live surfaces, coverage masks, pattern tiles and per-feature work, used to bound hostile input.
struct SurfaceBudget {
    /// Bytes of image surfaces currently alive.
    live_bytes: usize,
    /// Bytes reserved for coverage masks.
    coverage_bytes: usize,
    /// Cumulative bytes of pattern tiles allocated this render.
    pattern_tile_bytes: usize,
    /// Pattern content work charged so far.
    pattern_render_work: usize,
    /// Maximum pattern content work.
    pattern_render_work_limit: usize,
    /// Clip work charged so far.
    clip_render_work: usize,
    /// Maximum clip work.
    clip_render_work_limit: usize,
    /// Image sampling work charged so far.
    image_render_work: usize,
    /// Maximum image sampling work.
    image_render_work_limit: usize,
    /// Pixels touched by passes so far.
    pixel_work: u64,
    /// Maximum pixels touched by passes.
    pixel_work_limit: u64,
    /// Maximum combined live surface and coverage bytes.
    limit_bytes: usize,
    /// The first refusal of this render, if any.
    refusal: Option<RasterError>,
    /// Node draws left (see [`MAX_DRAWS_PER_NODE`]).
    draws_left: usize,
}

impl SurfaceBudget {
    /// A budget with the default render limits.
    fn new() -> SurfaceBudget {
        SurfaceBudget {
            live_bytes: 0,
            coverage_bytes: 0,
            pattern_tile_bytes: 0,
            pattern_render_work: 0,
            pattern_render_work_limit: MAX_PATTERN_RENDER_WORK,
            clip_render_work: 0,
            clip_render_work_limit: MAX_CLIP_RENDER_WORK,
            image_render_work: 0,
            image_render_work_limit: MAX_IMAGE_RENDER_WORK,
            pixel_work: 0,
            pixel_work_limit: MAX_PIXEL_WORK,
            limit_bytes: MAX_LIVE_SURFACE_BYTES,
            refusal: None,
            draws_left: usize::MAX,
        }
    }

    /// Record `error` as the render's refusal unless an earlier one is already recorded.
    fn refuse(&mut self, error: RasterError) {
        self.refusal.get_or_insert(error);
    }

    /// Charge `pixels` of pass work; `false` (recording a refusal, no charge) past the limit.
    fn charge_pixels(&mut self, pixels: usize) -> bool {
        let total = u64::try_from(pixels).ok().and_then(|n| self.pixel_work.checked_add(n));
        match total {
            Some(total) if total <= self.pixel_work_limit => {
                self.pixel_work = total;
                true
            }
            _ => {
                self.refuse(RasterError::WorkLimit("pixel passes"));
                false
            }
        }
    }

    /// Allocate a `width`×`height` surface (optionally a copy of `copy`), charging its pixels as
    /// pass work. `None` if the memory or pixel-work limit would be exceeded.
    fn image(&mut self, width: u32, height: u32, copy: Option<&Image>) -> Option<Image> {
        let Some((pixels, bytes)) = (width as usize).checked_mul(height as usize).and_then(|p| Some((p, p.checked_mul(std::mem::size_of::<[f32; 4]>())?)))
        else {
            self.refuse(RasterError::SurfaceLimit("surface size"));
            return None;
        };
        let new_live = self.live_bytes.checked_add(bytes).filter(|live| live.checked_add(self.coverage_bytes).is_some_and(|t| t <= self.limit_bytes));
        let Some(new_live) = new_live else {
            self.refuse(RasterError::SurfaceLimit("offscreen surface"));
            return None;
        };
        if !self.charge_pixels(pixels) {
            return None;
        }
        let mut data = Vec::new();
        if data.try_reserve_exact(pixels).is_err() {
            self.refuse(RasterError::SurfaceLimit("offscreen surface"));
            return None;
        }
        data.resize(pixels, [0.0; 4]);
        if let Some(source) = copy {
            if source.width != width || source.height != height || source.data.len() != pixels {
                self.refuse(RasterError::SurfaceLimit("surface copy"));
                return None;
            }
            data.copy_from_slice(&source.data);
        }
        self.live_bytes = new_live;
        Some(Image { width, height, data })
    }

    /// Reserve both the returned f32 coverage mask and tiny-skia's temporary u8 mask.
    fn coverage(&mut self, width: u32, height: u32) -> Option<usize> {
        let sizes = (|| {
            let pixels = (width as usize).checked_mul(height as usize)?;
            let mask_pixels = (width.max(1) as usize).checked_mul(height.max(1) as usize)?;
            pixels.checked_mul(std::mem::size_of::<f32>())?.checked_add(mask_pixels.checked_mul(std::mem::size_of::<u8>())?)
        })();
        let Some(bytes) = sizes else {
            self.refuse(RasterError::SurfaceLimit("coverage mask"));
            return None;
        };
        self.reserve_coverage(bytes)?;
        Some(bytes)
    }

    /// Reserve `bytes` of coverage-mask storage against the live-surface limit; `None` if it would overflow or exceed the limit.
    fn reserve_coverage(&mut self, bytes: usize) -> Option<()> {
        let total = self.coverage_bytes.checked_add(bytes).filter(|total| self.live_bytes.checked_add(*total).is_some_and(|t| t <= self.limit_bytes));
        let Some(total) = total else {
            self.refuse(RasterError::SurfaceLimit("coverage mask"));
            return None;
        };
        self.coverage_bytes = total;
        Some(())
    }

    /// Return `bytes` previously reserved with `reserve_coverage` to the budget.
    fn release_coverage(&mut self, bytes: usize) {
        self.coverage_bytes = self.coverage_bytes.saturating_sub(bytes);
    }

    fn release(&mut self, image: &Image) {
        self.live_bytes = self.live_bytes.saturating_sub(image.data.len().saturating_mul(std::mem::size_of::<[f32; 4]>()));
    }

    /// Allocate one pattern tile while charging its full storage cost against the render-wide
    /// pattern budget. This prevents many shapes from repeatedly rasterising a near-limit tile.
    fn pattern_tile(&mut self, width: u32, height: u32) -> Option<Image> {
        let total = (width as usize)
            .checked_mul(height as usize)
            .and_then(|pixels| pixels.checked_mul(std::mem::size_of::<[f32; 4]>()))
            .and_then(|bytes| self.pattern_tile_bytes.checked_add(bytes))
            .filter(|total| *total <= MAX_PATTERN_TILE_BYTES);
        let Some(total) = total else {
            self.refuse(RasterError::SurfaceLimit("pattern tile"));
            return None;
        };
        let image = self.image(width, height, None)?;
        self.pattern_tile_bytes = total;
        Some(image)
    }

    /// Charge `work` against the render-wide pattern content budget; `false` (and no charge) if the limit would be exceeded.
    fn charge_pattern_render_work(&mut self, work: usize) -> bool {
        match self.pattern_render_work.checked_add(work) {
            Some(total) if total <= self.pattern_render_work_limit => {
                self.pattern_render_work = total;
                true
            }
            _ => {
                self.refuse(RasterError::WorkLimit("pattern content"));
                false
            }
        }
    }

    /// Charge `work` against the render-wide clip budget; `false` (and no charge) if the limit would be exceeded.
    fn charge_clip_render_work(&mut self, work: usize) -> bool {
        match self.clip_render_work.checked_add(work) {
            Some(total) if total <= self.clip_render_work_limit => {
                self.clip_render_work = total;
                true
            }
            _ => {
                self.refuse(RasterError::WorkLimit("clip paths"));
                false
            }
        }
    }

    /// Charge `work` against the render-wide image sampling budget; `false` (and no charge) if the limit would be exceeded.
    fn charge_image_render_work(&mut self, work: usize) -> bool {
        match self.image_render_work.checked_add(work) {
            Some(total) if total <= self.image_render_work_limit => {
                self.image_render_work = total;
                true
            }
            _ => {
                self.refuse(RasterError::WorkLimit("image sampling"));
                false
            }
        }
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

/// Draws a group through an offscreen buffer, then composites it onto `dst`. A clipped group can
/// only change the region its clips touch, so it works on a region-sized crop of `dst` (with the
/// transform shifted to match) instead of a whole-canvas layer; the crop is written back after.
fn draw_offscreen_group(g: &Group, m: Affine, o: f64, dst: &mut Image, budget: &mut SurfaceBudget) {
    if !g.clip.is_empty() {
        let Some(region) = clip_region(g, m, dst.width, dst.height) else { return };
        if region.width() < dst.width || region.height() < dst.height {
            let Some(mut sub) = budget.image(region.width(), region.height(), None) else { return };
            let sw = sub.width as usize;
            let dw = dst.width as usize;
            for (y, row) in sub.data.chunks_mut(sw.max(1)).enumerate() {
                let start = (region.y0 as usize + y).saturating_mul(dw).saturating_add(region.x0 as usize);
                if let Some(src) = dst.data.get(start..start.saturating_add(sw)) {
                    row.copy_from_slice(src);
                }
            }
            offscreen_layer(g, region.origin_shift() * m, o, &mut sub, budget);
            for (y, row) in sub.data.chunks(sw.max(1)).enumerate() {
                let start = (region.y0 as usize + y).saturating_mul(dw).saturating_add(region.x0 as usize);
                if let Some(out) = dst.data.get_mut(start..start.saturating_add(sw)) {
                    out.copy_from_slice(row);
                }
            }
            budget.release(&sub);
            return;
        }
    }
    offscreen_layer(g, m, o, dst, budget);
}

/// The canvas region every clip path of `g` can touch, or `None` when the clips exclude everything.
fn clip_region(g: &Group, m: Affine, w: u32, h: u32) -> Option<Region> {
    let mut region = (w > 0 && h > 0).then_some(Region { x0: 0, y0: 0, x1: w, y1: h });
    for (path, _) in &g.clip {
        let own = device_region(kurbo::Shape::bounding_box(path), m, 2.0, w, h);
        region = region.zip(own).and_then(|(a, b)| a.intersect(b));
    }
    region
}

/// Draws a group through offscreen layers the size of `dst`, then composites it onto `dst`.
fn offscreen_layer(g: &Group, m: Affine, o: f64, dst: &mut Image, budget: &mut SurfaceBudget) {
    let only_clip = g.mask.is_none() && g.blend == BlendMode::Normal && o >= 1.0 && !g.knockout;
    // Compositing, clipping and masking are whole-canvas passes on top of the surface allocations.
    if !budget.charge_pixels(dst.data.len()) {
        return;
    }
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
        let Some(clip) = clip_coverage(g, m, dst.width, dst.height, budget) else {
            budget.release(&tmp);
            release_backdrop(budget, backdrop.as_ref());
            return;
        };
        apply_clip(&mut tmp, &clip);
        budget.release_coverage(clip.reserved);
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

/// Release the budget held by an optional backdrop copy.
fn release_backdrop(budget: &mut SurfaceBudget, backdrop: Option<&Image>) {
    if let Some(b) = backdrop {
        budget.release(b);
    }
}

/// A clipping group over a backdrop: the children where the clip covers, the backdrop elsewhere.
fn clip_onto_backdrop(g: &Group, m: Affine, dst: &mut Image, tmp: &Image, budget: &mut SurfaceBudget) -> bool {
    let Some(clip) = clip_coverage(g, m, dst.width, dst.height, budget) else { return false };
    // Outside the clip region the coverage is zero, so the backdrop in `dst` is already right.
    if let Some(region) = clip.region {
        let (w, rw) = (tmp.width as usize, region.width() as usize);
        let (tmp_data, cov) = (&tmp.data, &clip.data);
        for_region_rows(dst, region, |y, row| {
            for (j, d) in row.iter_mut().enumerate() {
                let x = region.x0 as usize + j;
                let (Some(t), Some(c)) = (tmp_data.get(y * w + x), cov.get((y - region.y0 as usize) * rw + j)) else { continue };
                for i in 0..4 {
                    d[i] = d[i] * (1.0 - c) + t[i] * c;
                }
            }
        });
    }
    budget.release_coverage(clip.reserved);
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

/// The product of a group's clip-path coverages, held only over the region every clip can touch.
struct ClipCoverage {
    /// The canvas region the clip lets anything through, or `None` when the clips exclude everything.
    region: Option<Region>,
    /// Coverage over `region`, row-major.
    data: Vec<f32>,
    /// Coverage bytes reserved in the budget, to release once the coverage is applied.
    reserved: usize,
}

/// Multiply `tmp` by the clip: zero outside the clip region, scaled by coverage inside it.
fn apply_clip(tmp: &mut Image, clip: &ClipCoverage) {
    let w = tmp.width as usize;
    let Some(region) = clip.region else {
        tmp.data.par_iter_mut().for_each(|p| *p = [0.0; 4]);
        return;
    };
    let (x0, x1, y0, y1) = (region.x0 as usize, region.x1 as usize, region.y0 as usize, region.y1 as usize);
    let rw = region.width() as usize;
    if w == 0 {
        return;
    }
    let data = &clip.data;
    tmp.data.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        if y < y0 || y >= y1 {
            row.fill([0.0; 4]);
            return;
        }
        for (x, p) in row.iter_mut().enumerate() {
            if x < x0 || x >= x1 {
                *p = [0.0; 4];
            } else {
                let c = data.get((y - y0) * rw + (x - x0)).copied().unwrap_or(0.0);
                for v in p.iter_mut() {
                    *v *= c;
                }
            }
        }
    });
}

/// The product of a group's clip-path coverages, or `None` when the work or surface budget is
/// exhausted. Only the region every clip path can touch is rasterised and reserved: the output
/// mask, a temporary per-path f32 mask and tiny-skia's u8 scratch mask, all region-sized.
fn clip_coverage(g: &Group, m: Affine, w: u32, h: u32, budget: &mut SurfaceBudget) -> Option<ClipCoverage> {
    let paths_work = g.clip.iter().try_fold(0usize, |work, (path, _)| work.checked_add(path.elements().len().max(1)));
    let Some(paths_work) = paths_work else {
        budget.refuse(RasterError::WorkLimit("clip paths"));
        return None;
    };
    let Some(region) = clip_region(g, m, w, h) else { return Some(ClipCoverage { region: None, data: Vec::new(), reserved: 0 }) };
    let pixels = region.area();
    let Some(work) = pixels.checked_mul(paths_work) else {
        budget.refuse(RasterError::WorkLimit("clip paths"));
        return None;
    };
    if !budget.charge_clip_render_work(work) || !budget.charge_pixels(pixels.saturating_mul(g.clip.len().max(1))) {
        return None;
    }
    let bytes = pixels
        .checked_mul(std::mem::size_of::<f32>())
        .and_then(|b| b.checked_mul(2))
        .and_then(|b| b.checked_add(pixels.checked_mul(std::mem::size_of::<u8>())?));
    let Some(bytes) = bytes else {
        budget.refuse(RasterError::SurfaceLimit("coverage mask"));
        return None;
    };
    budget.reserve_coverage(bytes)?;
    let mut out = Vec::new();
    if out.try_reserve_exact(pixels).is_err() {
        budget.release_coverage(bytes);
        budget.refuse(RasterError::SurfaceLimit("coverage mask"));
        return None;
    }
    out.resize(pixels, 1.0f32);
    for (path, rule) in &g.clip {
        let rule = match rule {
            FillRule::NonZero => effectcraft_path::FillRule::NonZero,
            FillRule::EvenOdd => effectcraft_path::FillRule::EvenOdd,
        };
        let cov = region_fill_coverage(path, m, rule, region);
        out.par_iter_mut().zip(cov.data.par_iter()).for_each(|(o, c)| *o *= c);
    }
    Some(ClipCoverage { region: Some(region), data: out, reserved: bytes })
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
            let (Some(&cp), Some(sp)) = (ci.data.get(i), si.data.get(i)) else { return };
            let f = sp[3].max(cp[3]);
            if f <= 0.0 {
                return;
            }
            let b = backdrop.and_then(|b| b.data.get(i).copied()).unwrap_or(zero);
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
    let Some(left) = budget.draws_left.checked_sub(1) else {
        budget.refuse(RasterError::WorkLimit("node draws"));
        return;
    };
    budget.draws_left = left;
    if left == 0 {
        log::warn!("svg: draw budget reached, the rest of the drawing is skipped");
    }
    match n {
        Node::Group(g) => draw_group(g, m, opacity, dst, budget),
        Node::Shape(s) => {
            if s.opacity < 1.0 && s.fill.is_some() && s.stroke.is_some() {
                let shape_m = m * s.transform;
                let Some(region) = path_device_region(s.geom.to_path().bounding_box(), shape_m, s.stroke.as_ref(), dst.width, dst.height) else { return };
                let has_pattern = s.fill.as_ref().is_some_and(|paint| matches!(paint, Paint::Pattern(_)))
                    || s.stroke.as_ref().is_some_and(|stroke| matches!(&stroke.paint, Paint::Pattern(_)));
                if has_pattern && !budget.charge_pattern_render_work(region.area()) {
                    return;
                }
                // Group the fill and stroke in a region-sized layer so a small shape stays cheap.
                let Some(mut tmp) = budget.image(region.width(), region.height(), None) else { return };
                draw_shape(s, region.origin_shift() * m, 1.0, &mut tmp, budget);
                over_region(dst, &tmp, region, (opacity * s.opacity) as f32);
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
    // Pixel footprint in image pixels: box-filter heavy minification instead of aliasing.
    let c = inv.as_coeffs();
    let fx = (c[0] * c[0] + c[1] * c[1]).sqrt();
    let fy = (c[2] * c[2] + c[3] * c[3]).sqrt();
    let taps = (fx.max(fy).ceil() as usize).clamp(1, 8);
    let Some(region) = device_region(kurbo::Rect::new(0.0, 0.0, iw as f64, ih as f64), full, 2.0, dst.width, dst.height) else { return };
    let Some(render_work) = region.area().checked_mul(taps).and_then(|work| work.checked_mul(taps)) else {
        budget.refuse(RasterError::WorkLimit("image sampling"));
        return;
    };
    if !budget.charge_image_render_work(render_work) || !budget.charge_pixels(region.area()) {
        return;
    }
    let Some(coverage_bytes) = budget.coverage(region.width(), region.height()) else { return };
    let cov = region_fill_coverage(&quad, full, effectcraft_path::FillRule::NonZero, region);
    let rw = region.width() as usize;
    let px = |x: usize, y: usize| -> Option<[f32; 4]> {
        let i = y.checked_mul(iw)?.checked_add(x)?.checked_mul(4)?;
        let end = i.checked_add(4)?;
        let [r, g, b, a] = im.rgba.get(i..end)? else { return None };
        let a = *a as f32 / 255.0;
        Some([*r as f32 / 255.0 * a, *g as f32 / 255.0 * a, *b as f32 / 255.0 * a, a])
    };
    let k = opacity as f32;
    for_region_rows(dst, region, |y, row| {
        for (j, d) in row.iter_mut().enumerate() {
            let x = region.x0 as usize + j;
            let Some(mask_index) = y.checked_sub(region.y0 as usize).and_then(|ry| ry.checked_mul(rw)).and_then(|index| index.checked_add(j)) else { continue };
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

/// `region` of `dst = src·k over dst` (premultiplied), where `src` is a `region`-sized layer.
fn over_region(dst: &mut Image, src: &Image, region: Region, k: f32) {
    let sw = src.width as usize;
    for_region_rows(dst, region, |y, row| {
        for (j, d) in row.iter_mut().enumerate() {
            let Some(s) = (y - region.y0 as usize).checked_mul(sw).and_then(|i| i.checked_add(j)).and_then(|i| src.data.get(i)) else { continue };
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

/// Fill and stroke one shape onto `dst`. Coverage is built only over the device region the shape
/// can touch; work or memory beyond the budget skips that fill or stroke and records a refusal.
fn draw_shape(s: &Shape, m: Affine, opacity: f64, dst: &mut Image, budget: &mut SurfaceBudget) {
    let m = m * s.transform;
    let path = s.geom.to_path();
    let bbox = path.bounding_box();
    let (w, h) = (dst.width, dst.height);
    let elements = path.elements().len().max(1);
    if let Some(fill) = &s.fill
        && let Some(region) = path_device_region(bbox, m, None, w, h)
        && budget.charge_pixels(region.area())
        && let Some(coverage_bytes) = budget.coverage(region.width(), region.height())
    {
        let coverage_allowed =
            !matches!(fill, Paint::Pattern(_)) || region.area().checked_mul(elements).is_some_and(|work| budget.charge_pattern_render_work(work));
        if coverage_allowed {
            let rule = match s.fill_rule {
                FillRule::NonZero => effectcraft_path::FillRule::NonZero,
                FillRule::EvenOdd => effectcraft_path::FillRule::EvenOdd,
            };
            let cov = region_fill_coverage(&path, m, rule, region);
            paint(dst, &cov, region, fill, (opacity * s.fill_opacity) as f32, m, bbox, budget);
        }
        budget.release_coverage(coverage_bytes);
    }
    if let Some(st) = &s.stroke
        && let Some(region) = path_device_region(bbox, m, s.stroke.as_ref(), w, h)
        && budget.charge_pixels(region.area())
        && let Some(coverage_bytes) = budget.coverage(region.width(), region.height())
    {
        let coverage_allowed =
            !matches!(&st.paint, Paint::Pattern(_)) || region.area().checked_mul(elements).is_some_and(|work| budget.charge_pattern_render_work(work));
        if coverage_allowed {
            let style = st.style();
            let cov = effectcraft_path::stroke_coverage(std::slice::from_ref(&path), &style, &mat3(region.origin_shift() * m), region.width(), region.height());
            paint(dst, &cov, region, &st.paint, (opacity * st.opacity) as f32, m, bbox, budget);
        }
        budget.release_coverage(coverage_bytes);
    }
}

/// Composite paint through coverage.
#[allow(clippy::too_many_arguments)]
fn paint(dst: &mut Image, cov: &Mask, region: Region, p: &Paint, opacity: f32, m: Affine, bbox: kurbo::Rect, budget: &mut SurfaceBudget) {
    if let Paint::Pattern(pattern) = p {
        paint_pattern(dst, cov, region, pattern, opacity, m, bbox, budget);
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
    paint_shader(dst, cov, region, &shader, opacity);
}

/// Render a pattern tile once for this paint operation, then sample it repeatedly over the mask.
/// When a budget refusal swaps the pattern for its fallback colour the pattern content is lost, so
/// the refusal is recorded; fallbacks for degenerate geometry (not a refusal) are not.
#[allow(clippy::too_many_arguments)]
fn paint_pattern(dst: &mut Image, cov: &Mask, region: Region, pattern: &Pattern, opacity: f32, m: Affine, bbox: kurbo::Rect, budget: &mut SurfaceBudget) {
    let fallback = || pattern.fallback.map(|c| Shader::Solid(c.map(|v| v as f32)));
    let fallback_paint = |dst: &mut Image, budget: &mut SurfaceBudget| {
        if let Some(shader) = fallback()
            && budget.charge_pattern_render_work(region.area())
        {
            paint_shader(dst, cov, region, &shader, opacity);
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
    let destination_pixels = region.area();
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
    paint_shader(dst, cov, region, &shader, opacity);
    budget.release(&tile);
}

/// Composite a shader through a `region`-sized coverage mask using premultiplied destination pixels.
fn paint_shader(dst: &mut Image, cov: &Mask, region: Region, shader: &Shader<'_>, opacity: f32) {
    let rw = region.width() as usize;
    for_region_rows(dst, region, |y, row| {
        for (j, d) in row.iter_mut().enumerate() {
            let Some(index) = y.checked_sub(region.y0 as usize).and_then(|ry| ry.checked_mul(rw)).and_then(|i| i.checked_add(j)) else { continue };
            let Some(c) = cov.data.get(index).copied() else { continue };
            if c <= 0.0 {
                continue;
            }
            let col = shader.at((region.x0 as usize + j) as f64 + 0.5, y as f64 + 0.5);
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

    /// A one-pixel white-source image for sampling tests.
    fn pixel_image() -> crate::Image {
        crate::Image { name: "pixel".into(), transform: Affine::IDENTITY, width: 1, height: 1, rgba: std::sync::Arc::new(vec![255, 0, 0, 255]), opacity: 1.0 }
    }

    /// Parse a one-pattern test SVG.
    fn pattern_doc(fill: &str) -> Doc {
        let svg = format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1">
                <defs><pattern id="p" patternUnits="userSpaceOnUse" width="1" height="1">
                    <rect width="1" height="1" fill="red"/>
                </pattern></defs>
                <rect width="1" height="1" fill="{fill}"/>
            </svg>"##
        );
        crate::parse(svg.as_bytes()).expect("test SVG parses")
    }

    /// Pattern tile bytes accumulate across the render even after earlier tiles are released.
    #[test]
    fn pattern_tile_budget_is_cumulative_after_tiles_are_released() {
        let mut budget = SurfaceBudget::new();
        budget.pattern_tile_bytes = MAX_PATTERN_TILE_BYTES - std::mem::size_of::<[f32; 4]>();
        let tile = budget.pattern_tile(1, 1).expect("last tile within the budget");
        budget.release(&tile);
        assert!(budget.pattern_tile(1, 1).is_none(), "a released tile still counts toward cumulative pattern work");
        assert_eq!(budget.refusal, Some(RasterError::SurfaceLimit("pattern tile")));
    }

    /// Coverage masks count against the same limit as live surfaces.
    #[test]
    fn coverage_masks_share_the_live_surface_budget() {
        let mut budget = SurfaceBudget::new();
        budget.limit_bytes = 4;
        assert!(budget.coverage(1, 1).is_none(), "the returned f32 mask and temporary u8 mask need five bytes");
        budget.limit_bytes = 20;
        let reserved = budget.coverage(1, 1).expect("mask fits at the limit");
        assert_eq!(reserved, 5);
        assert!(budget.image(1, 1, None).is_none(), "live images and coverage masks share one cap");
        budget.release_coverage(reserved);
        assert!(budget.image(1, 1, None).is_some(), "the released coverage budget is available to images");
    }

    /// Non-finite transformed corners cover the whole canvas instead of being ignored by `min`/`max`.
    #[test]
    fn device_region_with_non_finite_corner_is_the_full_canvas() {
        let full = Some(Region { x0: 0, y0: 0, x1: 10, y1: 6 });
        let rect = kurbo::Rect::new(0.0, 0.0, 1.0, 1.0);
        assert_eq!(device_region(rect, Affine::new([f64::NAN, 0.0, 0.0, 1.0, 0.0, 0.0]), 0.0, 10, 6), full);
        assert_eq!(device_region(rect, Affine::new([1.0, 0.0, 0.0, 1.0, f64::NAN, 3.0]), 0.0, 10, 6), full);
        assert_eq!(device_region(rect, Affine::new([f64::INFINITY, 0.0, 0.0, 1.0, 0.0, 0.0]), 0.0, 10, 6), full);
        // A NaN in a single corner only (rect itself has a NaN edge).
        assert_eq!(device_region(kurbo::Rect::new(0.0, 0.0, f64::NAN, 1.0), Affine::IDENTITY, 0.0, 10, 6), full);
        assert_eq!(device_region(rect, Affine::translate((3.0, 2.0)), 0.0, 10, 6), Some(Region { x0: 3, y0: 2, x1: 4, y1: 3 }));
        assert_eq!(device_region(rect, Affine::translate((30.0, 2.0)), 0.0, 10, 6), None);
    }

    /// Clip coverage reserves only region-sized masks and refuses excess work, recording why.
    #[test]
    fn clip_coverage_reserves_all_masks_and_refuses_excess_work() {
        let mut group = Group::new("clip");
        group.clip.push((kurbo::Rect::new(0.0, 0.0, 1.0, 1.0).to_path(0.01), FillRule::NonZero));
        let mut budget = SurfaceBudget::new();
        budget.limit_bytes = 9;

        let clip = clip_coverage(&group, Affine::IDENTITY, 1, 1, &mut budget).expect("clip fits exactly");
        assert_eq!(clip.reserved, 9, "clip output, per-path f32 mask, and tiny-skia scratch mask are reserved together");
        assert_eq!(clip.data.len(), 1);
        assert_eq!(budget.coverage_bytes, clip.reserved);
        budget.release_coverage(clip.reserved);
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

    /// A clip that touches a few pixels of a large canvas reserves region-sized masks only.
    #[test]
    fn clip_coverage_is_restricted_to_the_clip_region() {
        let mut group = Group::new("clip");
        group.clip.push((kurbo::Rect::new(10.0, 10.0, 12.0, 12.0).to_path(0.01), FillRule::NonZero));
        let mut budget = SurfaceBudget::new();
        let clip = clip_coverage(&group, Affine::IDENTITY, 4096, 4096, &mut budget).expect("tiny clip");
        let region = clip.region.expect("clip is visible");
        assert!(region.area() < 100, "region {region:?} is a few pixels, not the canvas");
        assert_eq!(clip.data.len(), region.area());
        assert!(budget.clip_render_work < 1000);
    }

    /// Image drawing rejects overflowing source sizes and charges sampling work to the image budget.
    #[test]
    fn image_render_checks_input_size_and_charges_sampling_work() {
        let mut budget = SurfaceBudget::new();
        budget.image_render_work_limit = 0;
        let mut dst = Image::new(1, 1);
        let source = pixel_image();

        draw_image(&source, Affine::IDENTITY, 1.0, &mut dst, &mut budget);
        assert_eq!(dst.data.first().copied(), Some([0.0; 4]), "excess sampling work is refused before coverage allocation");
        assert_eq!(budget.coverage_bytes, 0);
        assert_eq!(budget.refusal, Some(RasterError::WorkLimit("image sampling")));

        let oversized = crate::Image { width: u32::MAX, height: u32::MAX, rgba: std::sync::Arc::new(Vec::new()), ..source };
        draw_image(&oversized, Affine::IDENTITY, 1.0, &mut dst, &mut budget);
        assert_eq!(dst.data.first().copied(), Some([0.0; 4]), "overflowing source dimensions are rejected");
    }

    /// Exhausting pattern content work falls back to the paint fallback and records the refusal.
    #[test]
    fn pattern_content_work_exhaustion_uses_a_budgeted_fallback() {
        let doc = pattern_doc("url(#p) blue");
        let shapes = doc.flatten();
        let Some(Paint::Pattern(pattern)) = shapes[0].2.fill.as_ref() else { panic!("pattern paint expected") };
        let (mut dst, cov) = (Image::new(1, 1), Mask::new(1, 1, 1.0));
        let mut budget = SurfaceBudget::new();
        budget.pattern_render_work_limit = 1;
        let region = Region { x0: 0, y0: 0, x1: 1, y1: 1 };

        paint_pattern(&mut dst, &cov, region, pattern, 1.0, Affine::IDENTITY, kurbo::Rect::new(0.0, 0.0, 1.0, 1.0), &mut budget);

        assert_eq!(dst.data.first().copied(), Some([0.0, 0.0, 1.0, 1.0]));
        assert_eq!(budget.pattern_render_work, 1, "the pattern is refused and fallback sampling is charged");
        assert_eq!(budget.refusal, Some(RasterError::WorkLimit("pattern content")), "the lost pattern content is reported");
    }

    /// Pattern coverage work is charged before its mask is allocated.
    #[test]
    fn pattern_coverage_work_is_charged_before_allocating_its_mask() {
        let doc = pattern_doc("url(#p)");
        let Some(Node::Shape(shape)) = doc.root.children.first() else { panic!("shape expected") };
        let mut dst = Image::new(1, 1);
        let mut budget = SurfaceBudget::new();
        budget.pattern_render_work_limit = 1;

        draw_shape(shape, Affine::IDENTITY, 1.0, &mut dst, &mut budget);

        assert_eq!(dst.data.first().copied(), Some([0.0; 4]));
        assert_eq!(budget.pattern_tile_bytes, 0, "work is refused before a tile or coverage mask is produced");
        assert_eq!(budget.pattern_render_work, 0);
        assert!(budget.refusal.is_some(), "the skipped fill is reported");
    }

    /// Whole-canvas offscreen groups spend the pixel budget and are then refused explicitly.
    #[test]
    fn full_canvas_offscreen_groups_exhaust_the_pixel_budget_and_report_it() {
        let mut group = Group::new("g");
        group.opacity = 0.5;
        group.children = vec![Node::Group(Group::new("a")), Node::Group(Group::new("b"))];
        let mut budget = SurfaceBudget::new();
        budget.pixel_work_limit = 64 * 64 * 20;
        let mut dst = budget.image(64, 64, None).expect("canvas fits");
        for _ in 0..100 {
            draw_group(&group, Affine::IDENTITY, 1.0, &mut dst, &mut budget);
        }
        assert_eq!(budget.refusal, Some(RasterError::WorkLimit("pixel passes")));
        assert!(budget.pixel_work <= budget.pixel_work_limit, "refused work is never charged");
        assert_eq!(budget.live_bytes, dst.data.len() * 16, "refusals leave no leaked surfaces");
    }
}
