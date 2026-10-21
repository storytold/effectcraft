//! Placed-layer (smart object) geometry: the non-affine corner quad (a projective map of the
//! content rectangle) and the placed-layer warp (`warp` in `SoLd`: the named warp styles with
//! bend and horizontal / vertical distortion, and custom envelope warps whose quilt mesh is a
//! grid of bicubic Bézier patches), and a renderer that bakes both into document-space pixels.
//!
//! The custom mesh is evaluated exactly (bicubic Bézier patches over `meshPoints`, split by
//! `quiltSliceX` / `quiltSliceY`). Photoshop does not publish the curves of its named styles;
//! the styles here are clean-room geometric definitions of what each name describes (Arc,
//! Arch, Bulge, Shell, Flag, Wave, Fish, Rise, Fisheye, Inflate, Squeeze, Twist), matched by
//! behaviour, not numerically.

use crate::descriptor::{DValue, Descriptor};
use crate::{Pixels, SmartObject};

/// The most pixels a baked smart object may have: 2^24 (256 MiB as float RGBA). The size comes
/// from the file's transform, which is untrusted.
pub const MAX_BAKE_PIXELS: u64 = 1 << 24;

/// A placed layer's warp.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Warp {
    /// `warpNone`, `warpArc`, `warpArcLower`, `warpArcUpper`, `warpArch`, `warpBulge`,
    /// `warpShellLower`, `warpShellUpper`, `warpFlag`, `warpWave`, `warpFish`, `warpRise`,
    /// `warpFisheye`, `warpInflate`, `warpSqueeze`, `warpTwist`, `warpCustom`.
    pub style: String,
    /// Bend, −100..100 (%).
    pub bend: f64,
    /// Horizontal and vertical distortion, −100..100 (%).
    pub horizontal: f64,
    pub vertical: f64,
    /// The style is applied rotated a quarter turn (`warpRotate` = vertical).
    pub rotate_vertical: bool,
    /// The warped area in content pixels: left, top, right, bottom.
    pub bounds: [f64; 4],
    /// Custom envelope: patch columns / rows and the (3·cols+1)×(3·rows+1) control points
    /// (content pixels, row-major).
    pub cols: usize,
    pub rows: usize,
    pub mesh: Vec<[f64; 2]>,
}

pub(crate) fn floats(v: Option<&DValue>) -> Vec<f64> {
    match v {
        Some(DValue::UnitFloats(_, v)) => v.clone(),
        Some(DValue::List(l)) => l
            .iter()
            .filter_map(|x| match x {
                DValue::Double(v) | DValue::UnitFloat(_, v) => Some(*v),
                DValue::Integer(i) => Some(*i as f64),
                _ => None,
            })
            .collect(),
        _ => vec![],
    }
}

/// The eight numbers of a transform entry (`Trnf`, `nonAffineTransform`), when it has exactly eight.
pub(crate) fn eight(d: &Descriptor, key: &str) -> Option<[f64; 8]> {
    floats(d.get(key)).try_into().ok()
}

/// The corner quad (top-left, top-right, bottom-right, bottom-left) from x0 y0 … x3 y3.
pub(crate) fn quad_from(t: [f64; 8]) -> [[f64; 2]; 4] {
    [[t[0], t[1]], [t[2], t[3]], [t[4], t[5]], [t[6], t[7]]]
}

impl Warp {
    /// Read a `warp` descriptor; `None` for no warp.
    pub fn read(d: &Descriptor) -> Option<Warp> {
        let style = d.enum_value("warpStyle").unwrap_or("warpNone").to_string();
        let b = d.obj("bounds");
        let bounds = b
            .map(|b| [b.num("Left").unwrap_or(0.0), b.num("Top ").unwrap_or(0.0), b.num("Rght").unwrap_or(0.0), b.num("Btom").unwrap_or(0.0)])
            .unwrap_or([0.0; 4]);
        let mut w = Warp {
            bend: d.num("warpValue").unwrap_or(0.0),
            horizontal: d.num("warpPerspective").unwrap_or(0.0),
            vertical: d.num("warpPerspectiveOther").unwrap_or(0.0),
            rotate_vertical: d.enum_value("warpRotate") == Some("Vrtc"),
            bounds,
            cols: 1,
            rows: 1,
            mesh: vec![],
            style,
        };
        if let Some(env) = d.obj("customEnvelopeWarp") {
            let cols = floats(env.get("quiltSliceX")).len().saturating_sub(1).max(1);
            let rows = floats(env.get("quiltSliceY")).len().saturating_sub(1).max(1);
            if let Some(DValue::ObjectArray(_, pts)) = env.get("meshPoints") {
                let xs = floats(pts.get("Hrzn"));
                let ys = floats(pts.get("Vrtc"));
                let n = cols.checked_mul(3).zip(rows.checked_mul(3)).and_then(|(c, r)| (c + 1).checked_mul(r + 1));
                if n.is_some_and(|n| xs.len() == n && ys.len() == n) {
                    w.cols = cols;
                    w.rows = rows;
                    w.mesh = xs.into_iter().zip(ys).map(|(x, y)| [x, y]).collect();
                }
            }
        }
        let identity = w.style == "warpNone" || (w.style != "warpCustom" && w.bend == 0.0 && w.horizontal == 0.0 && w.vertical == 0.0);
        let custom_ok = w.style != "warpCustom" || !w.mesh.is_empty();
        (!identity && custom_ok).then_some(w)
    }

    /// The warp of a content point (content pixels → content pixels).
    pub fn apply(&self, p: [f64; 2], w: f64, h: f64) -> [f64; 2] {
        let [l, t, r, b] = if self.bounds[2] > self.bounds[0] && self.bounds[3] > self.bounds[1] { self.bounds } else { [0.0, 0.0, w, h] };
        let (bw, bh) = (r - l, b - t);
        let (u, v) = ((p[0] - l) / bw, (p[1] - t) / bh);
        if self.style == "warpCustom" {
            // A mesh that does not match cols × rows (public fields) leaves the point where it is.
            return self.mesh_point(u.clamp(0.0, 1.0), v.clamp(0.0, 1.0)).unwrap_or(p);
        }
        // Styles work in a centred frame (pixels), optionally rotated a quarter turn.
        let (x, y, fw, fh) = if self.rotate_vertical { (bh * (v - 0.5), -bw * (u - 0.5), bh, bw) } else { (bw * (u - 0.5), bh * (v - 0.5), bw, bh) };
        let [mut x2, mut y2] = style_map(&self.style, self.bend / 100.0, x, y, fw, fh);
        // Distortions: a perspective-like taper across the width (horizontal) or height.
        let (hd, vd) = if self.rotate_vertical { (self.vertical / 100.0, self.horizontal / 100.0) } else { (self.horizontal / 100.0, self.vertical / 100.0) };
        let s = (x2 / (fw / 2.0)).clamp(-1.5, 1.5);
        let tt = (y2 / (fh / 2.0)).clamp(-1.5, 1.5);
        y2 *= 1.0 + hd * s;
        x2 *= 1.0 + vd * tt;
        let (ox, oy) = if self.rotate_vertical { (-y2, x2) } else { (x2, y2) };
        [l + bw / 2.0 + ox, t + bh / 2.0 + oy]
    }

    /// A point of the custom quilt mesh at (u, v) ∈ [0, 1]².
    fn mesh_point(&self, u: f64, v: f64) -> Option<[f64; 2]> {
        let (cols, rows) = (self.cols, self.rows);
        if cols == 0 || rows == 0 {
            return None;
        }
        let pu = (u * cols as f64).min(cols as f64 - 1e-9);
        let pv = (v * rows as f64).min(rows as f64 - 1e-9);
        let (ci, cj) = (pu.floor() as usize, pv.floor() as usize);
        let (fu, fv) = (pu - ci as f64, pv - cj as f64);
        let stride = cols.checked_mul(3)?.checked_add(1)?;
        let bern = |t: f64| {
            let s = 1.0 - t;
            [s * s * s, 3.0 * s * s * t, 3.0 * s * t * t, t * t * t]
        };
        let (bu, bv) = (bern(fu), bern(fv));
        let mut out = [0.0; 2];
        for j in 0..4 {
            for i in 0..4 {
                let idx = cj.saturating_mul(3).saturating_add(j).saturating_mul(stride).saturating_add(ci.saturating_mul(3)).saturating_add(i);
                let q = self.mesh.get(idx)?;
                let k = bu[i] * bv[j];
                out[0] += q[0] * k;
                out[1] += q[1] * k;
            }
        }
        Some(out)
    }
}

/// A named style in a centred frame `fw`×`fh` (y down), bend `b` ∈ [−1, 1].
fn style_map(style: &str, b: f64, x: f64, y: f64, fw: f64, fh: f64) -> [f64; 2] {
    use std::f64::consts::PI;
    let (hw, hh) = (fw / 2.0, fh / 2.0);
    let s = x / hw; // −1..1 across
    let t = y / hh; // −1..1 down
    // Concentric arcs: the midline bends through an angle of b·π, rows keep their distance.
    let arc = |x: f64, y: f64| -> [f64; 2] {
        let theta = b * PI;
        if theta.abs() < 1e-9 {
            return [x, y];
        }
        let r = fw / theta;
        let phi = x / hw * theta / 2.0;
        let rr = r - y;
        // Arc centre below the shape for upward bends; keep the midline's centre fixed.
        [rr * phi.sin(), r - rr * phi.cos()]
    };
    let bump = 1.0 - s * s;
    match style {
        "warpArc" => arc(x, y),
        "warpArcLower" => {
            let a = arc(x, y);
            let k = (t + 1.0) / 2.0;
            [x + (a[0] - x) * k, y + (a[1] - y) * k]
        }
        "warpArcUpper" => {
            let a = arc(x, y);
            let k = (1.0 - t) / 2.0;
            [x + (a[0] - x) * k, y + (a[1] - y) * k]
        }
        "warpArch" => [x, y - b * hh * bump],
        "warpBulge" => [x, y + t * b * hh * bump],
        "warpShellLower" => [x * (1.0 - b * 0.25 * (1.0 - t) / 2.0), y + (t + 1.0) / 2.0 * b * hh * bump],
        "warpShellUpper" => [x * (1.0 - b * 0.25 * (t + 1.0) / 2.0), y - (1.0 - t) / 2.0 * b * hh * bump],
        "warpFlag" => [x, y - b * hh * 0.5 * (PI * s).sin()],
        "warpWave" => [x + b * hw * 0.1 * (PI * t).sin(), y - b * hh * 0.5 * (PI * s).sin() * (1.0 - 0.5 * t)],
        "warpFish" => [x, y + t * b * hh * (PI * (s + 1.0) / 2.0).sin() * (1.0 - 0.5 * (s + 1.0) / 2.0)],
        "warpRise" => [x, y - b * hh * (PI * s / 2.0).sin()],
        "warpFisheye" | "warpInflate" => {
            let k = if style == "warpFisheye" { (1.0 - (s * s + t * t) / 2.0).max(0.0) } else { 1.0 };
            [x * (1.0 + b * 0.5 * k * (1.0 - t * t)), y * (1.0 + b * 0.5 * k * bump)]
        }
        "warpSqueeze" => [x * (1.0 - b * 0.5 * (1.0 - t * t)), y * (1.0 + b * 0.5 * bump)],
        "warpTwist" => {
            let r = ((s * s + t * t) / 2.0).sqrt().min(1.0);
            let a = b * PI / 2.0 * (1.0 - r);
            let (sn, cs) = a.sin_cos();
            [x * cs - y * sn, x * sn + y * cs]
        }
        _ => [x, y],
    }
}

/// The projective map taking the unit square's corners (0,0), (1,0), (1,1), (0,1) to `q`
/// (Heckbert's square-to-quad), as a 3×3 matrix applied to (u, v, 1).
pub(crate) fn square_to_quad(q: [[f64; 2]; 4]) -> [[f64; 3]; 3] {
    #[cfg(test)]
    crate::counters::bump(&crate::counters::QUAD);
    let [p0, p1, p2, p3] = q;
    let sx = p0[0] - p1[0] + p2[0] - p3[0];
    let sy = p0[1] - p1[1] + p2[1] - p3[1];
    if sx.abs() < 1e-12 && sy.abs() < 1e-12 {
        return [[p1[0] - p0[0], p2[0] - p1[0], p0[0]], [p1[1] - p0[1], p2[1] - p1[1], p0[1]], [0.0, 0.0, 1.0]];
    }
    let (dx1, dx2) = (p1[0] - p2[0], p3[0] - p2[0]);
    let (dy1, dy2) = (p1[1] - p2[1], p3[1] - p2[1]);
    let den = dx1 * dy2 - dx2 * dy1;
    let g = (sx * dy2 - dx2 * sy) / den;
    let h = (dx1 * sy - sx * dy1) / den;
    [[p1[0] - p0[0] + g * p1[0], p3[0] - p0[0] + h * p3[0], p0[0]], [p1[1] - p0[1] + g * p1[1], p3[1] - p0[1] + h * p3[1], p0[1]], [g, h, 1.0]]
}

/// The baked size of placed content with document-space bounds `bb` at `k` pixels per document pixel.
fn size_for(bb: [f64; 4], k: f64) -> (u32, u32) {
    (((bb[2] - bb[0]) * k).ceil().clamp(1.0, 16384.0) as u32, ((bb[3] - bb[1]) * k).ceil().clamp(1.0, 16384.0) as u32)
}

impl SmartObject {
    /// Whether the corner quad is a parallelogram (an affine placement).
    pub fn is_affine(&self) -> bool {
        let q = self.quad;
        let ex = q[0][0] + q[2][0] - q[1][0] - q[3][0];
        let ey = q[0][1] + q[2][1] - q[1][1] - q[3][1];
        ex.abs() < 1e-3 && ey.abs() < 1e-3
    }

    /// Whether placing needs baking (a perspective quad or a warp).
    pub fn needs_bake(&self) -> bool {
        !self.is_affine() || self.warp.is_some()
    }

    /// A content point (pixels of a `w`×`h` content) in document pixels: the warp, then the quad.
    pub fn map(&self, p: [f64; 2], w: f64, h: f64) -> [f64; 2] {
        self.map_with(&square_to_quad(self.quad), p, w, h)
    }

    /// [`Self::map`] with the quad's projective matrix already solved (`square_to_quad(self.quad)`).
    fn map_with(&self, m: &[[f64; 3]; 3], p: [f64; 2], w: f64, h: f64) -> [f64; 2] {
        let p = match &self.warp {
            Some(wp) => wp.apply(p, w, h),
            None => p,
        };
        let (u, v) = (p[0] / w.max(1e-9), p[1] / h.max(1e-9));
        let z = m[2][0] * u + m[2][1] * v + m[2][2];
        let z = if z.abs() < 1e-12 { 1e-12 } else { z };
        [(m[0][0] * u + m[0][1] * v + m[0][2]) / z, (m[1][0] * u + m[1][1] * v + m[1][2]) / z]
    }

    /// The document-space bounds (x0, y0, x1, y1) of the placed (warped) content.
    pub fn placed_bounds(&self, w: f64, h: f64) -> [f64; 4] {
        self.bounds_with(&square_to_quad(self.quad), w, h)
    }

    fn bounds_with(&self, m: &[[f64; 3]; 3], w: f64, h: f64) -> [f64; 4] {
        let n = 32;
        let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
        for j in 0..=n {
            for i in 0..=n {
                let p = self.map_with(m, [i as f64 / n as f64 * w, j as f64 / n as f64 * h], w, h);
                b = [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])];
            }
        }
        [b[0].floor(), b[1].floor(), b[2].ceil(), b[3].ceil()]
    }

    /// The baked size for a `w`×`h` content at `k` pixels per document pixel.
    pub fn placed_size(&self, w: f64, h: f64, k: f64) -> (u32, u32) {
        size_for(self.placed_bounds(w, h), k)
    }

    /// The scale and size to bake a `w`×`h` content at, wanting `k` pixels per document pixel but
    /// never more than [`MAX_BAKE_PIXELS`] pixels, nor more than 4× the `doc` size across either
    /// way: a quad corner far outside the document lowers `k` (possibly below 1) instead of asking
    /// for a gigantic image. Returns the scale actually used and the size in pixels.
    pub fn bake_scale(&self, w: f64, h: f64, k: f64, doc: (u32, u32)) -> (f64, (u32, u32)) {
        let bb = self.placed_bounds(w, h);
        let (bw, bh) = ((bb[2] - bb[0]).max(1.0), (bb[3] - bb[1]).max(1.0));
        if !(bw.is_finite() && bh.is_finite() && k.is_finite() && k > 0.0) {
            return (1.0, (1, 1));
        }
        let (max_w, max_h) = ((f64::from(doc.0) * 4.0).max(1.0), (f64::from(doc.1) * 4.0).max(1.0));
        let capped = k.min(max_w / bw).min(max_h / bh).min((MAX_BAKE_PIXELS as f64 / (bw * bh)).sqrt());
        // Rounding up could step over a cap, so a lowered scale rounds down.
        let side = |len: f64| if capped < k { (len * capped).floor() } else { (len * capped).ceil() }.clamp(1.0, 16384.0) as u32;
        (capped, (side(bw), side(bh)))
    }

    /// Bake the content (straight RGBA) as placed: warped and mapped by the quad, rendered at
    /// `k` pixels per document pixel (into `size` pixels when given; see [`Self::placed_size`]).
    /// Returns the pixels and their document-space origin. `None` when the content is empty or
    /// the baked image would exceed [`MAX_BAKE_PIXELS`].
    pub fn render_placed(&self, content: &Pixels, k: f64, size: Option<(u32, u32)>) -> Option<(Pixels, [f64; 2])> {
        let (cw, ch) = (content.width as f64, content.height as f64);
        if content.width == 0 || content.height == 0 {
            return None;
        }
        // The quad is solved once for the bounds, the size and the whole grid.
        let m = square_to_quad(self.quad);
        let bb = self.bounds_with(&m, cw, ch);
        let (ow, oh) = match size {
            Some((w, h)) => (w.max(1), h.max(1)),
            None => size_for(bb, k),
        };
        let (ow, oh) = (ow as usize, oh as usize);
        let count = ow.checked_mul(oh).filter(|&c| c as u64 <= MAX_BAKE_PIXELS)?;
        // A forward grid, filled triangle by triangle with inverse barycentric sampling.
        let n = 48usize;
        let mut dst = Vec::with_capacity((n + 1) * (n + 1));
        let mut src = Vec::with_capacity((n + 1) * (n + 1));
        for j in 0..=n {
            for i in 0..=n {
                let s = [i as f64 / n as f64 * cw, j as f64 / n as f64 * ch];
                let d = self.map_with(&m, s, cw, ch);
                dst.push([(d[0] - bb[0]) * k, (d[1] - bb[1]) * k]);
                src.push(s);
            }
        }
        let mut out: Vec<[f32; 4]> = Vec::new();
        out.try_reserve_exact(count).ok()?;
        out.resize(count, [0.0; 4]);
        let sample = |x: f64, y: f64| -> [f32; 4] {
            let (fx, fy) = ((x - 0.5).clamp(0.0, cw - 1.0), (y - 0.5).clamp(0.0, ch - 1.0));
            let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
            let (x1, y1) = ((x0 + 1).min(content.width as usize - 1), (y0 + 1).min(content.height as usize - 1));
            let (a, b) = ((fx - x0 as f64) as f32, (fy - y0 as f64) as f32);
            let at = |x: usize, y: usize| {
                let p = content.data[y * content.width as usize + x];
                [p[0] * p[3], p[1] * p[3], p[2] * p[3], p[3]]
            };
            let (p00, p10, p01, p11) = (at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1));
            let mut o = [0.0; 4];
            for c in 0..4 {
                o[c] = (p00[c] * (1.0 - a) + p10[c] * a) * (1.0 - b) + (p01[c] * (1.0 - a) + p11[c] * a) * b;
            }
            o
        };
        let idx = |i: usize, j: usize| j * (n + 1) + i;
        for j in 0..n {
            for i in 0..n {
                for tri in [[idx(i, j), idx(i + 1, j), idx(i + 1, j + 1)], [idx(i, j), idx(i + 1, j + 1), idx(i, j + 1)]] {
                    let [a, b, c] = tri.map(|t| dst[t]);
                    let den = (b[1] - c[1]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[1] - c[1]);
                    if den.abs() < 1e-12 {
                        continue;
                    }
                    // Loop-invariant per triangle: the source corners and the barycentric coefficients.
                    let [sa, sb, sc] = tri.map(|t| src[t]);
                    let (l1y, l1x) = (b[1] - c[1], c[0] - b[0]);
                    let (l2y, l2x) = (c[1] - a[1], a[0] - c[0]);
                    let x0 = a[0].min(b[0]).min(c[0]).floor().max(0.0) as usize;
                    let x1 = (a[0].max(b[0]).max(c[0]).ceil() as usize).min(ow);
                    let y0 = a[1].min(b[1]).min(c[1]).floor().max(0.0) as usize;
                    let y1 = (a[1].max(b[1]).max(c[1]).ceil() as usize).min(oh);
                    for y in y0..y1 {
                        for x in x0..x1 {
                            let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
                            let l1 = (l1y * (px - c[0]) + l1x * (py - c[1])) / den;
                            let l2 = (l2y * (px - c[0]) + l2x * (py - c[1])) / den;
                            let l3 = 1.0 - l1 - l2;
                            if l1 < -1e-9 || l2 < -1e-9 || l3 < -1e-9 {
                                continue;
                            }
                            let sx = sa[0] * l1 + sb[0] * l2 + sc[0] * l3;
                            let sy = sa[1] * l1 + sb[1] * l2 + sc[1] * l3;
                            out[y * ow + x] = sample(sx, sy);
                        }
                    }
                }
            }
        }
        // Back to straight alpha.
        for p in &mut out {
            if p[3] > 0.0 {
                for c in 0..3 {
                    p[c] /= p[3];
                }
            }
        }
        Some((Pixels { width: ow as u32, height: oh as u32, data: out }, [bb[0], bb[1]]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn so(quad: [[f64; 2]; 4], warp: Option<Warp>) -> SmartObject {
        SmartObject { uuid: "x".into(), quad, size: Some([20.0, 10.0]), warp }
    }

    #[test]
    fn perspective_quad_maps_corners_and_is_projective() {
        let q = [[10.0, 10.0], [50.0, 0.0], [50.0, 40.0], [10.0, 30.0]];
        let s = so(q, None);
        assert!(!s.is_affine() && s.needs_bake());
        for (c, want) in [([0.0, 0.0], q[0]), ([20.0, 0.0], q[1]), ([20.0, 10.0], q[2]), ([0.0, 10.0], q[3])] {
            let p = s.map(c, 20.0, 10.0);
            assert!((p[0] - want[0]).abs() < 1e-9 && (p[1] - want[1]).abs() < 1e-9, "{c:?} → {p:?}");
        }
        // Projective, not bilinear: the content centre is where the diagonals cross.
        let c = s.map([10.0, 5.0], 20.0, 10.0);
        assert!((c[0] - 30.0).abs() > 0.5, "{c:?}");
        let affine = so([[0.0, 0.0], [20.0, 0.0], [20.0, 10.0], [0.0, 10.0]], None);
        assert!(affine.is_affine() && !affine.needs_bake());
    }

    #[test]
    fn custom_mesh_and_styles_warp() {
        // An identity 4×4 mesh shifted 3 px right in its interior row: interior points move.
        let mut mesh = vec![];
        for j in 0..4 {
            for i in 0..4 {
                mesh.push([i as f64 * 20.0 / 3.0 + if j == 1 || j == 2 { 3.0 } else { 0.0 }, j as f64 * 10.0 / 3.0]);
            }
        }
        let w = Warp { style: "warpCustom".into(), bounds: [0.0, 0.0, 20.0, 10.0], cols: 1, rows: 1, mesh, ..Default::default() };
        let p = w.apply([10.0, 5.0], 20.0, 10.0);
        assert!((p[0] - 12.25).abs() < 1e-9 && (p[1] - 5.0).abs() < 1e-9, "{p:?}");
        assert_eq!(w.apply([0.0, 0.0], 20.0, 10.0), [0.0, 0.0]);
        // Arc: the top centre rises, the bottom centre too (concentric arcs); Bulge: top up,
        // bottom down; Flag keeps the centre.
        let style = |s: &str| Warp { style: s.into(), bend: 50.0, bounds: [0.0, 0.0, 200.0, 100.0], ..Default::default() };
        let top = style("warpArc").apply([100.0, 0.0], 200.0, 100.0);
        let corner = style("warpArc").apply([0.0, 0.0], 200.0, 100.0);
        assert!(top[1] < corner[1], "arc {top:?} {corner:?}");
        let b = style("warpBulge");
        assert!(b.apply([100.0, 0.0], 200.0, 100.0)[1] < 0.0 && b.apply([100.0, 100.0], 200.0, 100.0)[1] > 100.0);
        assert!((style("warpFlag").apply([100.0, 50.0], 200.0, 100.0)[1] - 50.0).abs() < 1e-9);
        // Vertical rotation swaps the roles of the axes.
        let mut v = style("warpBulge");
        v.rotate_vertical = true;
        let l = v.apply([0.0, 50.0], 200.0, 100.0);
        assert!(l[0] < 0.0 && (l[1] - 50.0).abs() < 1e-9, "{l:?}");
        // Horizontal distortion: the right side grows taller.
        let d = Warp { style: "warpArc".into(), horizontal: 50.0, bounds: [0.0, 0.0, 200.0, 100.0], ..Default::default() };
        assert!(d.apply([200.0, 0.0], 200.0, 100.0)[1] < d.apply([0.0, 0.0], 200.0, 100.0)[1]);
    }

    #[test]
    fn render_placed_bakes_a_perspective_quad() {
        // 2×1 content: red | green, pinned to a trapezoid.
        let content = Pixels { width: 2, height: 1, data: vec![[1.0, 0.0, 0.0, 1.0], [0.0, 1.0, 0.0, 1.0]] };
        let s = SmartObject { uuid: "x".into(), quad: [[0.0, 0.0], [40.0, 5.0], [40.0, 15.0], [0.0, 20.0]], size: None, warp: None };
        let (px, origin) = s.render_placed(&content, 1.0, None).unwrap();
        assert_eq!(origin, [0.0, 0.0]);
        assert_eq!((px.width, px.height), (40, 20));
        let at = |x: usize, y: usize| px.data[y * 40 + x];
        assert!(at(5, 10)[0] > 0.9 && at(5, 10)[3] > 0.99, "{:?}", at(5, 10));
        assert!(at(35, 10)[1] > 0.9, "{:?}", at(35, 10));
        assert_eq!(at(38, 1)[3], 0.0, "outside the trapezoid");
    }

    #[test]
    fn custom_warp_with_a_missing_or_short_mesh_does_not_panic() {
        let base = Warp { style: "warpCustom".into(), bounds: [0.0, 0.0, 20.0, 10.0], cols: 1, rows: 1, ..Default::default() };
        // Empty and too-short meshes leave the point where it is.
        assert_eq!(base.apply([5.0, 5.0], 20.0, 10.0), [5.0, 5.0]);
        let short = Warp { mesh: vec![[0.0, 0.0]; 5], ..base.clone() };
        assert_eq!(short.apply([5.0, 5.0], 20.0, 10.0), [5.0, 5.0]);
        for (cols, rows) in [(0, 0), (0, 3), (usize::MAX, 1), (1, usize::MAX), (usize::MAX, usize::MAX)] {
            let w = Warp { cols, rows, ..base.clone() };
            assert_eq!(w.apply([5.0, 5.0], 20.0, 10.0), [5.0, 5.0]);
        }
    }

    #[test]
    fn an_absurd_quad_corner_bakes_within_four_times_the_document_and_2_pow_24_pixels() {
        let doc = (1920u32, 1080u32);
        for far in [1e4, 1e6, 1e12] {
            let s = SmartObject { uuid: "x".into(), quad: [[0.0, 0.0], [far, 5.0], [far, far], [0.0, 20.0]], size: None, warp: None };
            assert!(s.needs_bake());
            // What the importer asks for: the unbounded size is the 16384² clamp, 4 GiB as float RGBA.
            assert_eq!(s.placed_size(100.0, 100.0, 4.0), (16384, 16384));
            let (k, (w, h)) = s.bake_scale(100.0, 100.0, 4.0, doc);
            assert!(k > 0.0 && k <= 4.0, "{k}");
            assert!(w <= doc.0 * 4 && h <= doc.1 * 4, "{w}x{h}");
            assert!(u64::from(w) * u64::from(h) <= MAX_BAKE_PIXELS, "{w}x{h}");
        }
        // A quad inside the document keeps the scale and the size it always had.
        let s = SmartObject { uuid: "x".into(), quad: [[0.0, 0.0], [400.0, 50.0], [400.0, 150.0], [0.0, 200.0]], size: None, warp: None };
        let (k, size) = s.bake_scale(100.0, 100.0, 2.0, doc);
        assert_eq!((k, size), (2.0, s.placed_size(100.0, 100.0, 2.0)));
        // Not-a-number corners stay small too.
        let s = SmartObject { uuid: "x".into(), quad: [[f64::NAN; 2]; 4], size: None, warp: None };
        let (_, (w, h)) = s.bake_scale(100.0, 100.0, 4.0, doc);
        assert!(u64::from(w) * u64::from(h) <= MAX_BAKE_PIXELS);
    }

    #[test]
    fn render_placed_refuses_an_output_over_the_pixel_cap() {
        let content = Pixels { width: 2, height: 1, data: vec![[1.0, 0.0, 0.0, 1.0], [0.0, 1.0, 0.0, 1.0]] };
        let s = SmartObject { uuid: "x".into(), quad: [[0.0, 0.0], [40.0, 5.0], [40.0, 15.0], [0.0, 20.0]], size: None, warp: None };
        assert!(s.render_placed(&content, 1.0, Some((16384, 16384))).is_none());
        assert!(s.render_placed(&content, 1.0, Some((u32::MAX, u32::MAX))).is_none());
    }
}
