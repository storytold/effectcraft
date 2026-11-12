//! Classic 3D depth of field: bokeh-shaped gather blur.
//!
//! The camera's Iris Shape (Fast Rectangle, or a regular polygon from Triangle to Decagon),
//! Iris Rotation, Roundness and Aspect Ratio define the blur kernel; Iris Diffraction Fringe
//! brightens its rim. Highlight Gain / Threshold / Saturation boost the bright parts of the layer
//! before blurring so they bloom into visible bokeh shapes. A layer whose depth varies across it
//! (tilted towards or away from the camera) is blurred progressively: the buffer is blurred at a
//! few kernel sizes and every pixel blends the two that bracket its own circle of confusion.

use effectcraft_raster::Image;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

/// Camera iris (Camera Options ▸ Iris …).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Iris {
    /// 0 = Fast Rectangle, else the number of sides (3–10).
    pub sides: u32,
    /// Degrees.
    pub rotation: f64,
    /// −1…1: 1 = circular, negative pinches the sides inwards.
    pub roundness: f64,
    /// Width / height of the aperture.
    pub aspect: f64,
    /// Rim brightness: 0 = none, 1 = the rim is twice as bright (Iris Diffraction Fringe / 100).
    pub fringe: f64,
}

impl Default for Iris {
    fn default() -> Self {
        Iris { sides: 0, rotation: 0.0, roundness: 0.0, aspect: 1.0, fringe: 0.0 }
    }
}

/// Highlight boost before the blur (Camera Options ▸ Highlight …), all 0–1.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Highlight {
    pub gain: f64,
    pub threshold: f64,
    pub saturation: f64,
}

impl Iris {
    /// Iris Shape popup index → sides (0 Fast Rectangle, 1 Triangle … 8 Decagon).
    pub fn sides_of_shape(shape: u32) -> u32 {
        if shape == 0 { 0 } else { (shape + 2).min(10) }
    }

    /// Normalised boundary distance (1 = circumscribed circle) of the iris at angle `a`.
    fn boundary(&self, a: f64) -> f64 {
        let n = self.sides.max(3) as f64;
        let seg = std::f64::consts::TAU / n;
        let rel = (a - self.rotation.to_radians()).rem_euclid(seg) - seg * 0.5;
        let poly = (std::f64::consts::PI / n).cos() / rel.cos();
        let r = self.roundness.clamp(-1.0, 1.0);
        // Positive: towards the circumscribed circle; negative: the sides bow inwards (the
        // vertices stay put, the edge midpoints move in).
        if r >= 0.0 { poly + (1.0 - poly) * r } else { (poly + (1.0 - poly) * r * 2.0).max(0.05) }
    }

    /// Whether the kernel offset (dx, dy) lies inside an iris of radius `r`.
    fn inside(&self, r: f64, dx: f64, dy: f64) -> bool {
        let a = self.aspect.clamp(0.01, 100.0).sqrt();
        let (u, v) = (dx / a, dy * a);
        let d = u.hypot(v);
        d < 1e-9 || d <= self.boundary(v.atan2(u)) * r
    }

    /// The kernel as horizontal spans: for every row offset `dy`, the (fractional) x extent
    /// `[x0, x1]` of the iris of radius `r` at that height. The iris is convex for roundness ≥ 0;
    /// a pinched iris keeps its outermost extent per row.
    pub fn spans(&self, r: f64) -> Vec<(i32, f64, f64)> {
        let a = self.aspect.clamp(0.01, 100.0).sqrt();
        let (ext_x, ext_y) = (r * a + 1.0, (r / a).ceil() as i32 + 1);
        let mut out = Vec::new();
        for dy in -ext_y..=ext_y {
            let y = dy as f64;
            // Coarse scan, then bisection of both ends.
            let step = 0.5;
            let mut first = None;
            let mut last = None;
            let mut x = -ext_x;
            while x <= ext_x {
                if self.inside(r, x, y) {
                    first.get_or_insert(x);
                    last = Some(x);
                }
                x += step;
            }
            let (Some(f), Some(l)) = (first, last) else { continue };
            let refine = |inside_x: f64, outside_x: f64| {
                let (mut i, mut o) = (inside_x, outside_x);
                for _ in 0..12 {
                    let m = (i + o) * 0.5;
                    if self.inside(r, m, y) { i = m } else { o = m }
                }
                (i + o) * 0.5
            };
            out.push((dy, refine(f, f - step), refine(l, l + step)));
        }
        out
    }
}

/// Prefix sums of each row (per channel): `p[x]` = sum of pixels `0..x`.
fn row_prefix(img: &Image) -> Vec<Vec<[f32; 4]>> {
    let w = img.width as usize;
    img.data
        .par_chunks(w.max(1))
        .map(|row| {
            let mut acc = [0.0f32; 4];
            let mut v = Vec::with_capacity(w + 1);
            v.push(acc);
            for p in row {
                for c in 0..4 {
                    acc[c] += p[c];
                }
                v.push(acc);
            }
            v
        })
        .collect()
}

/// Continuous prefix: sum over [0, t) with linear interpolation inside a pixel.
#[inline]
fn prefix_at(p: &[[f32; 4]], t: f64) -> [f32; 4] {
    let n = p.len() - 1;
    if t <= 0.0 {
        return [0.0; 4];
    }
    if t >= n as f64 {
        return p[n];
    }
    let i = t.floor() as usize;
    let f = (t - i as f64) as f32;
    let (a, b) = (p[i], p[i + 1]);
    [a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f, a[2] + (b[2] - a[2]) * f, a[3] + (b[3] - a[3]) * f]
}

/// Gather with span kernels: `(weight, spans)` pairs summed, then normalised by `norm`.
fn span_gather(img: &Image, kernels: &[(f32, Vec<(i32, f64, f64)>)], norm: f32) -> Image {
    let pre = row_prefix(img);
    let (w, h) = (img.width as usize, img.height as i64);
    let mut out = Image::new(img.width, img.height);
    let inv = 1.0 / norm;
    out.data.par_chunks_mut(w.max(1)).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let mut acc = [0.0f32; 4];
            for (wt, spans) in kernels {
                for &(dy, x0, x1) in spans {
                    let sy = y as i64 + dy as i64;
                    if sy < 0 || sy >= h {
                        continue;
                    }
                    let pr = &pre[sy as usize];
                    let (hi, lo) = (prefix_at(pr, x as f64 + x1 + 0.5), prefix_at(pr, x as f64 + x0 + 0.5));
                    for c in 0..4 {
                        acc[c] += (hi[c] - lo[c]) * wt;
                    }
                }
            }
            *o = acc.map(|v| v * inv);
        }
    });
    out
}

/// Boost the highlights of a premultiplied image in place.
pub fn boost_highlights(img: &mut Image, h: &Highlight) {
    if h.gain <= 0.0 {
        return;
    }
    let thr = h.threshold.clamp(0.0, 1.0) as f32;
    img.data.par_iter_mut().for_each(|p| {
        if p[3] <= 0.0 {
            return;
        }
        let a = p[3];
        let (r, g, b) = (p[0] / a, p[1] / a, p[2] / a);
        let l = effectcraft_color::luminance(r, g, b);
        if l < thr || (thr >= 1.0 && l < 1.0) {
            return;
        }
        let over = if thr < 1.0 { ((l - thr) / (1.0 - thr)).clamp(0.0, 1.0) } else { 1.0 };
        let k = 1.0 + h.gain as f32 * 4.0 * over.max(0.25);
        let sat = 1.0 + h.saturation as f32;
        let c = [r, g, b].map(|c| (l + (c - l) * sat).max(0.0) * k);
        *p = [c[0] * a, c[1] * a, c[2] * a, a];
    });
}

/// Weighted row-span kernels and their normalisation.
pub type Kernels = (Vec<(f32, Vec<(i32, f64, f64)>)>, f32);

/// The bokeh kernel of radius `r` as weighted row spans (see [`Iris::spans`]) and the sum of
/// weights; `None` when the blur does nothing (radius below 0.3 px). Fast Rectangle is the box
/// of the iris' width and height (one span per row covering whole pixels); the diffraction
/// fringe adds the rim as the full iris minus a shrunken one. The GPU gathers with exactly these
/// spans.
pub fn kernel_spans(iris: &Iris, r: f64) -> Option<Kernels> {
    if r < 0.3 {
        return None;
    }
    if iris.sides == 0 {
        let a = iris.aspect.clamp(0.01, 100.0).sqrt();
        let rx = (r * a).round().max(0.0) as i32;
        let ry = (r / a).round().max(0.0) as i32;
        if rx == 0 && ry == 0 {
            return None;
        }
        let spans = (-ry..=ry).map(|dy| (dy, -rx as f64 - 0.5, rx as f64 + 0.5)).collect();
        return Some((vec![(1.0, spans)], ((2 * rx + 1) * (2 * ry + 1)) as f32));
    }
    let outer = iris.spans(r);
    let area = |sp: &[(i32, f64, f64)]| sp.iter().map(|s| (s.2 - s.1) as f32).sum::<f32>();
    let mut kernels = vec![(1.0f32, outer)];
    if iris.fringe > 0.0 {
        let rim = (r * 0.15).max(1.0);
        let inner = if r - rim > 0.5 { iris.spans(r - rim) } else { Vec::new() };
        let f = iris.fringe as f32;
        kernels[0].0 = 1.0 + f;
        kernels.push((-f, inner));
    }
    let norm: f32 = kernels.iter().map(|(wt, sp)| wt * area(sp)).sum();
    (norm > 0.0).then_some((kernels, norm))
}

/// Gather blur of a premultiplied image with a bokeh kernel of radius `r` (pixels). Pixels
/// beyond the image are transparent.
pub fn bokeh_blur(img: &Image, iris: &Iris, r: f64) -> Image {
    if r < 0.3 {
        return img.clone();
    }
    if iris.sides == 0 {
        // Fast Rectangle: a separable box of the iris' width and height (the same as its spans).
        let a = iris.aspect.clamp(0.01, 100.0).sqrt();
        let rx = (r * a).round().max(0.0) as usize;
        let ry = (r / a).round().max(0.0) as usize;
        return effectcraft_raster::box_blur(img, rx, ry, 1, false);
    }
    // The iris as row spans (prefix-sum gather: cost ∝ kernel height, not area).
    match kernel_spans(iris, r) {
        Some((kernels, norm)) => span_gather(img, &kernels, norm),
        None => img.clone(),
    }
}

/// The blur levels of a depth-varying blur ([`progressive_blur`]): `None` when nothing blurs,
/// one level for a near-constant radius, else up to eight radii spanning the range.
pub fn blur_levels(radius: &[f32]) -> Option<Vec<f32>> {
    let (lo, hi) = radius.iter().fold((f32::INFINITY, 0.0f32), |(a, b), &r| (a.min(r), b.max(r)));
    if !lo.is_finite() || hi < 0.3 {
        return None;
    }
    if hi - lo < 0.75 {
        return Some(vec![(lo + hi) * 0.5]);
    }
    let n = (((hi - lo) / 2.0).ceil() as usize + 1).clamp(2, 8);
    Some((0..n).map(|i| lo + (hi - lo) * i as f32 / (n - 1) as f32).collect())
}

/// How much level `k` of `levels` contributes at radius `r` (the two levels bracketing `r`
/// are blended linearly).
pub fn level_weight(levels: &[f32], k: usize, r: f32) -> f32 {
    let n = levels.len();
    if n <= 1 {
        return 1.0;
    }
    let (lo, hi) = (levels[0], levels[n - 1]);
    let f = ((r - lo) / (hi - lo) * (n - 1) as f32).clamp(0.0, (n - 1) as f32);
    let kk = (f.floor() as usize).min(n - 2);
    let t = f - kk as f32;
    if k == kk {
        1.0 - t
    } else if k == kk + 1 {
        t
    } else {
        0.0
    }
}

/// Depth-varying blur: `radius` gives each pixel's blur radius. The image is blurred at up to
/// eight radii spanning the range and each pixel interpolates between the two levels around
/// its own radius.
pub fn progressive_blur(img: &Image, iris: &Iris, radius: &[f32]) -> Image {
    let Some(levels) = blur_levels(radius) else { return img.clone() };
    if levels.len() == 1 {
        return bokeh_blur(img, iris, levels[0] as f64);
    }
    let (lo, hi, n) = (levels[0], levels[levels.len() - 1], levels.len());
    // A pixel blends only the two levels around its radius, so the levels are made in order and
    // only that pair is held: peak memory stays a few images whatever the level count, rather
    // than one image per level. Each level's blur is parallel over its rows.
    let mut out = Image::new(img.width, img.height);
    let mut below = bokeh_blur(img, iris, lo as f64);
    for (k, &r) in levels.iter().enumerate().skip(1) {
        let above = bokeh_blur(img, iris, r as f64);
        out.data.par_iter_mut().enumerate().for_each(|(i, o)| {
            let f = ((radius[i] - lo) / (hi - lo) * (n - 1) as f32).clamp(0.0, (n - 1) as f32);
            let kk = (f.floor() as usize).min(n - 2);
            if kk + 1 == k {
                let t = f - kk as f32;
                let (a, b) = (below.data[i], above.data[i]);
                *o = [0, 1, 2, 3].map(|c| a[c] + (b[c] - a[c]) * t);
            }
        });
        below = above;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A single bright point in the middle of a transparent image.
    fn dot(size: u32) -> Image {
        let mut img = Image::new(size, size);
        let c = (size / 2) as usize;
        img.data[c * size as usize + c] = [1.0, 1.0, 1.0, 1.0];
        img
    }

    fn lit(img: &Image, x: i32, y: i32) -> bool {
        let c = img.width as i32 / 2;
        img.data[((c + y) * img.width as i32 + c + x) as usize][3] > 1e-4
    }

    #[test]
    fn kernel_shapes() {
        // A hexagon with a vertex on +x (rotation 0): the point at 0.97 r on +x is inside, the
        // point at 0.97 r at 30° (an edge midpoint at cos 30° = 0.866 r) is outside.
        let hex = Iris { sides: 6, ..Default::default() };
        let b = bokeh_blur(&dot(81), &hex, 20.0);
        assert!(lit(&b, 19, 0));
        let (x, y) = ((19.4f64 * 30f64.to_radians().cos()).round() as i32, (19.4f64 * 30f64.to_radians().sin()).round() as i32);
        assert!(!lit(&b, x, y), "edge midpoint beyond the apothem");
        // Rotating by 30° swaps them.
        let rot = Iris { rotation: 30.0, ..hex };
        let b = bokeh_blur(&dot(81), &rot, 20.0);
        assert!(!lit(&b, 19, 0) && lit(&b, x, y));
        // Full roundness: a circle.
        let round = Iris { roundness: 1.0, ..hex };
        let b = bokeh_blur(&dot(81), &round, 20.0);
        assert!(lit(&b, 19, 0) && lit(&b, x, y));
        // Triangle: far less area than a decagon of the same radius.
        let area = |sides| {
            let b = bokeh_blur(&dot(81), &Iris { sides, ..Default::default() }, 20.0);
            b.data.iter().filter(|p| p[3] > 1e-4).count()
        };
        assert!(area(3) * 2 < area(10), "{} vs {}", area(3), area(10));
        // Aspect ratio 4 makes it twice as wide and half as tall.
        let wide = Iris { sides: 8, aspect: 4.0, ..Default::default() };
        let b = bokeh_blur(&dot(121), &wide, 15.0);
        assert!(lit(&b, 28, 0) && !lit(&b, 0, 10));
        // Energy is preserved.
        let sum: f32 = b.data.iter().map(|p| p[3]).sum();
        assert!((sum - 1.0).abs() < 1e-3, "{sum}");
    }

    #[test]
    fn diffraction_fringe_brightens_the_rim() {
        let plain = bokeh_blur(&dot(81), &Iris { sides: 8, ..Default::default() }, 20.0);
        let fringe = bokeh_blur(&dot(81), &Iris { sides: 8, fringe: 2.0, ..Default::default() }, 20.0);
        let at = |img: &Image, x: i32| img.data[(40 * 81 + 40 + x) as usize][3];
        assert!((at(&plain, 0) - at(&plain, 17)).abs() < 1e-6, "flat disc");
        assert!(at(&fringe, 18) > at(&fringe, 0) * 1.5, "{} vs {}", at(&fringe, 18), at(&fringe, 0));
    }

    #[test]
    fn highlights_boost_only_above_threshold() {
        let mut img = Image::new(2, 1);
        img.data = vec![[0.9, 0.9, 0.9, 1.0], [0.3, 0.3, 0.3, 1.0]];
        boost_highlights(&mut img, &Highlight { gain: 0.5, threshold: 0.8, saturation: 0.0 });
        assert!(img.data[0][0] > 1.2 && img.data[1][0] == 0.3);
        let mut c = Image::new(1, 1);
        c.data = vec![[1.0, 0.8, 0.8, 1.0]];
        boost_highlights(&mut c, &Highlight { gain: 0.1, threshold: 0.5, saturation: 1.0 });
        assert!(c.data[0][0] - c.data[0][1] > 0.3, "more saturated: {:?}", c.data[0]);
    }

    #[test]
    fn progressive_blur_follows_the_radius_map() {
        // A vertical line, blurred 0 px at the top rising to 8 px at the bottom.
        let (w, h) = (41u32, 41u32);
        let mut img = Image::new(w, h);
        for y in 0..h as usize {
            img.data[y * w as usize + 20] = [1.0; 4];
        }
        let radius: Vec<f32> = (0..(w * h) as usize).map(|i| (i / w as usize) as f32 / 40.0 * 8.0).collect();
        let out = progressive_blur(&img, &Iris { sides: 6, roundness: 1.0, ..Default::default() }, &radius);
        let spread = |y: usize| (0..w as usize).filter(|&x| out.data[y * w as usize + x][3] > 0.03).count();
        assert!(spread(1) <= 3, "top stays sharp: {}", spread(1));
        assert!(spread(39) >= 13, "bottom is wide: {}", spread(39));
        assert!(spread(20) > spread(1) && spread(20) < spread(39));
    }
}
