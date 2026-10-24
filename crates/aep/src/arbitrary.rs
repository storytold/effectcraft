//! Arbitrary-data effect parameters (type 11): the values of Hue/Saturation and Levels that are not
//! ordinary parameters.
//!
//! In the effect's value group (`sspc/tdgp`) the parameter's `tdbs` (`tdsb` 1, a 4-byte `cdat`) is
//! followed by a `LIST aRbs` holding one `aRbp`: the data. The `aRbp` in `parT` right after the
//! parameter's `pard` holds the default (443, 511). Setting Master Hue, Saturation or Lightness of
//! Hue/Saturation (444-446) or a Levels value (512-516) changes only bytes of this `aRbp` (and stale
//! UI bytes of `pard`); the visible parameters of the same names keep no record.

use crate::Chunk;
use crate::bytes::{be_f32, be_i32, be_u16, be_u32};
use crate::effects::Effect;

impl<'f, 'a> Effect<'f, 'a> {
    /// The arbitrary data stored for the parameter with this match name (`<effect>-NNNN`): the
    /// `aRbp` of the `LIST aRbs` that follows the parameter's value record. `None` when there is
    /// none.
    pub fn arbitrary_data(&self, match_name: &str) -> Option<&'f [u8]> {
        let values = self.chunk().child_list(b"tdgp")?;
        let kids = values.children();
        let at = kids.iter().position(|c| c.is(b"tdmn") && c.bytes().is_some_and(|b| name_is(b, match_name)))?;
        // The value record, then the data list (no match name of its own).
        let rest = kids.get(at.checked_add(1)?..)?;
        let mut it = rest.iter().take(3);
        let _record = it.next().filter(|c| c.is_list(b"tdbs"))?;
        it.find(|c| c.is_list(b"aRbs")).and_then(|l| l.child(b"aRbp")).and_then(Chunk::bytes)
    }
}

fn name_is(body: &[u8], name: &str) -> bool {
    let end = body.iter().position(|c| *c == 0).unwrap_or(body.len());
    body.get(..end) == Some(name.as_bytes())
}

/// Hue/Saturation's data (180 bytes, `ADBE HUE SATURATION-0003`): the master hue, saturation and
/// lightness as `i32` at bytes 0, 4 and 8 (444: hue 90 is `0x5a`; 445: saturation 50; 446:
/// lightness -30), then six colour ranges (Reds, Yellows, Greens, Cyans, Blues, Magentas) of 28
/// bytes each: four `i32` range points in degrees (Reds 315, 345, 15, 45 by default) and the
/// range's hue, saturation and lightness. Only the master values are sampled; the ranges' layout is
/// read from the default data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HueSaturationData {
    /// Master hue (degrees), saturation and lightness (-100..100).
    pub master: [i32; 3],
    /// Per colour range: the four range points (degrees) and its hue, saturation and lightness.
    pub ranges: [([i32; 4], [i32; 3]); 6],
}

impl HueSaturationData {
    /// The default data (443): master 0, 0, 0 and every range at its default points.
    pub const DEFAULT_RANGES: [[i32; 4]; 6] =
        [[315, 345, 15, 45], [15, 45, 75, 105], [75, 105, 135, 165], [135, 165, 195, 225], [195, 225, 255, 285], [255, 285, 315, 345]];

    /// Decodes the data; `None` when it is not 180 bytes.
    pub fn parse(data: &[u8]) -> Option<Self> {
        if data.len() != 180 {
            return None;
        }
        let i = |at: usize| be_i32(data, at);
        let master = [i(0)?, i(4)?, i(8)?];
        let mut ranges = [([0; 4], [0; 3]); 6];
        for (k, r) in ranges.iter_mut().enumerate() {
            let base = 12usize.checked_add(k.checked_mul(28)?)?;
            r.0 = [i(base)?, i(base + 4)?, i(base + 8)?, i(base + 12)?];
            r.1 = [i(base + 16)?, i(base + 20)?, i(base + 24)?];
        }
        Some(Self { master, ranges })
    }

    /// Whether every colour range is at its default (points as in 443, no adjustment).
    pub fn ranges_are_default(&self) -> bool {
        self.ranges.iter().zip(Self::DEFAULT_RANGES).all(|((points, hsl), d)| *points == d && *hsl == [0, 0, 0])
    }
}

/// Levels' data (108 bytes, `ADBE Easy Levels2-0002`): a `u32` 0x000f10a7 and a `u32` 1 (511, in
/// every sample), then five channels (RGB, Red, Green, Blue, Alpha) of five `f32`: input black,
/// input white, gamma, output black, output white. The RGB values are sampled (512 input black
/// 0.1, 513 input white 0.8, 514 gamma 2, 515 output black 0.2, 516 output white 0.9); the other
/// channels' layout is read from the default data (0, 1, 1, 0, 1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LevelsData {
    pub header: [u32; 2],
    /// RGB, Red, Green, Blue, Alpha: input black, input white, gamma, output black, output white.
    pub channels: [[f32; 5]; 5],
}

impl LevelsData {
    /// The values of a channel at its default.
    pub const DEFAULT_CHANNEL: [f32; 5] = [0.0, 1.0, 1.0, 0.0, 1.0];

    /// Decodes the data; `None` when it is not 108 bytes.
    pub fn parse(data: &[u8]) -> Option<Self> {
        if data.len() != 108 {
            return None;
        }
        let header = [be_u32(data, 0)?, be_u32(data, 4)?];
        let mut channels = [[0.0; 5]; 5];
        for (k, c) in channels.iter_mut().enumerate() {
            let base = 8usize.checked_add(k.checked_mul(20)?)?;
            for (j, v) in c.iter_mut().enumerate() {
                *v = be_f32(data, base.checked_add(j.checked_mul(4)?)?)?;
            }
        }
        Some(Self { header, channels })
    }
}

/// Curves' data (1644 bytes, `ADBE CurvesCustom-0001`; FORMAT S.1, 961-964): `u16` 1, `u16` 1,
/// five 256-byte lookup tables (RGB, Red, Green, Blue, Alpha: output 0..255 per input 0..255),
/// then five 72-byte point records: 16 slots of (`u16` input, `u16` output), both 0..255, the
/// number of points as `u32` at 64 and an `i32` at 68 (-1 at the default; the point count - 2 in
/// every edited sample, H: the active point). Every stored table is the natural cubic spline
/// through its points ([`curve_lut`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurvesData {
    /// RGB, Red, Green, Blue, Alpha: the points (input, output), as stored.
    pub curves: [Vec<(u8, u8)>; 5],
}

/// The points of an untouched curve.
pub const CURVE_IDENTITY: [(u8, u8); 2] = [(0, 0), (255, 255)];

impl CurvesData {
    pub const SIZE: usize = 1644;
    const POINTS: usize = 1284;

    /// Decodes the data; `None` when it is not 1644 bytes or a point count is beyond 16.
    pub fn parse(data: &[u8]) -> Option<Self> {
        if data.len() != Self::SIZE {
            return None;
        }
        let mut curves: [Vec<(u8, u8)>; 5] = Default::default();
        for (c, pts) in curves.iter_mut().enumerate() {
            let base = Self::POINTS.checked_add(c.checked_mul(72)?)?;
            let n = usize::try_from(be_u32(data, base.checked_add(64)?)?).ok()?;
            if n > 16 {
                return None;
            }
            for k in 0..n {
                let at = base.checked_add(k.checked_mul(4)?)?;
                let x = u8::try_from(be_u16(data, at)?).ok()?;
                let y = u8::try_from(be_u16(data, at.checked_add(2)?)?).ok()?;
                pts.push((x, y));
            }
        }
        Some(Self { curves })
    }

    /// Whether every curve is the identity.
    pub fn is_default(&self) -> bool {
        self.curves.iter().all(|c| c.as_slice() == CURVE_IDENTITY)
    }

    /// The data After Effects stores for these curves: the tables rebuilt from the points.
    /// `None` when a curve has fewer than 2 or more than 16 points or inputs that do not rise.
    pub fn to_bytes(&self) -> Option<Vec<u8>> {
        let mut out = vec![0u8; Self::SIZE];
        out.get_mut(0..4)?.copy_from_slice(&[0, 1, 0, 1]);
        for (c, pts) in self.curves.iter().enumerate() {
            if pts.len() > 16 {
                return None;
            }
            let lut = curve_lut(pts)?;
            out.get_mut(4 + 256 * c..4 + 256 * (c + 1))?.copy_from_slice(&lut);
            let base = Self::POINTS + 72 * c;
            for (k, (x, y)) in pts.iter().enumerate() {
                out.get_mut(base + 4 * k..base + 4 * k + 4)?.copy_from_slice(&[0, *x, 0, *y]);
            }
            let n = u32::try_from(pts.len()).ok()?;
            out.get_mut(base + 64..base + 68)?.copy_from_slice(&n.to_be_bytes());
            let active: i32 = if pts.as_slice() == CURVE_IDENTITY { -1 } else { i32::try_from(pts.len()).ok()? - 2 };
            out.get_mut(base + 68..base + 72)?.copy_from_slice(&active.to_be_bytes());
        }
        Some(out)
    }
}

fn rising(points: &[(u8, u8)]) -> bool {
    points.windows(2).all(|w| matches!(w, [a, b] if a.0 < b.0))
}

/// The lookup table After Effects stores for a curve: the natural cubic spline (second derivative
/// 0 at both ends) through the points, at the integers 0..255, rounded to the nearest integer and
/// clamped to 0..255; flat outside the first and last point (961-964: 0 of 1280 entries differ).
/// `None` for fewer than 2 points or inputs that do not rise.
pub fn curve_lut(points: &[(u8, u8)]) -> Option<[u8; 256]> {
    let n = points.len();
    if n < 2 || !rising(points) {
        return None;
    }
    let xs: Vec<f64> = points.iter().map(|p| f64::from(p.0)).collect();
    let ys: Vec<f64> = points.iter().map(|p| f64::from(p.1)).collect();
    let x = |i: usize| xs.get(i).copied().unwrap_or(0.0);
    let y = |i: usize| ys.get(i).copied().unwrap_or(0.0);
    let h = |i: usize| (x(i + 1) - x(i)).max(1.0);
    let slope = |i: usize| (y(i + 1) - y(i)) / h(i);
    // Second derivatives: the tridiagonal system of the natural spline (Thomas algorithm).
    let mut m = vec![0.0f64; n];
    let k = n - 2;
    if k > 0 {
        let mut diag: Vec<f64> = (0..k).map(|i| 2.0 * (h(i) + h(i + 1))).collect();
        let upper: Vec<f64> = (0..k).map(|i| h(i + 1)).collect();
        let mut rhs: Vec<f64> = (0..k).map(|i| 6.0 * (slope(i + 1) - slope(i))).collect();
        for i in 1..k {
            let (Some(&dp), Some(&up), Some(&rp)) = (diag.get(i - 1), upper.get(i - 1), rhs.get(i - 1)) else { return None };
            let w = h(i) / dp;
            if let (Some(d), Some(r)) = (diag.get_mut(i), rhs.get_mut(i)) {
                *d -= w * up;
                *r -= w * rp;
            }
        }
        for i in (0..k).rev() {
            let next = m.get(i + 2).copied().unwrap_or(0.0);
            let (Some(&r), Some(&u), Some(&d)) = (rhs.get(i), upper.get(i), diag.get(i)) else { return None };
            if let Some(slot) = m.get_mut(i + 1) {
                *slot = (r - u * next) / d;
            }
        }
    }
    let mut lut = [0u8; 256];
    for (xi, slot) in lut.iter_mut().enumerate() {
        let xf = xi as f64;
        let v = if xf <= x(0) {
            y(0)
        } else if xf >= x(n - 1) {
            y(n - 1)
        } else {
            let i = (0..n - 1).find(|&i| xf >= x(i) && xf <= x(i + 1)).unwrap_or(0);
            let hh = h(i);
            let (a, b) = ((x(i + 1) - xf) / hh, (xf - x(i)) / hh);
            let (mi, mj) = (m.get(i).copied().unwrap_or(0.0), m.get(i + 1).copied().unwrap_or(0.0));
            a * y(i) + b * y(i + 1) + ((a * a * a - a) * mi + (b * b * b - b) * mj) * hh * hh / 6.0
        };
        *slot = (v + 0.5).floor().clamp(0.0, 255.0) as u8;
    }
    Some(lut)
}

/// Mesh Warp's data (`ADBE MESH WARP-0004`; FORMAT S.2, 965, 985): `u32` rows plus one, `u32`
/// columns plus one (a 1 x 2-cell export renders as made only in this order, `export-tests\mesh`;
/// the vertices run row-major, columns plus one per row), 12 more bytes (zero, then
/// `0d 0d`),
/// then per vertex, row-major, ten `f32`: the position and the handles top, right, bottom and
/// left, all as fractions 0..1 of the layer; a handle pointing out of the mesh is stored (0, 0).
#[derive(Debug, Clone, PartialEq)]
pub struct MeshData {
    pub columns: u32,
    pub rows: u32,
    /// Bytes 8..20 as stored.
    pub tail: [u8; 12],
    /// Per vertex: position, top, right, bottom, left.
    pub vertices: Vec<[[f32; 2]; 5]>,
}

impl MeshData {
    /// Decodes the data; `None` when the size does not fit the grid or the grid is beyond 31 x 31
    /// cells.
    pub fn parse(data: &[u8]) -> Option<Self> {
        let (r1, c1) = (be_u32(data, 0)?, be_u32(data, 4)?);
        if !(2..=32).contains(&c1) || !(2..=32).contains(&r1) {
            return None;
        }
        let count = usize::try_from(c1.checked_mul(r1)?).ok()?;
        if data.len() != 20usize.checked_add(count.checked_mul(40)?)? {
            return None;
        }
        let mut tail = [0u8; 12];
        tail.copy_from_slice(data.get(8..20)?);
        let mut vertices = Vec::with_capacity(count);
        for v in 0..count {
            let base = 20 + 40 * v;
            let mut rec = [[0.0f32; 2]; 5];
            for (j, p) in rec.iter_mut().enumerate() {
                *p = [be_f32(data, base + 8 * j)?, be_f32(data, base + 8 * j + 4)?];
            }
            vertices.push(rec);
        }
        Some(Self { columns: c1 - 1, rows: r1 - 1, tail, vertices })
    }

    /// The vertex at column `i`, row `j` of an undistorted mesh with its default handles (one
    /// third of a cell along the grid lines; those pointing out of the mesh are (0, 0)).
    pub fn default_vertex(columns: u32, rows: u32, i: u32, j: u32) -> [[f32; 2]; 5] {
        let (cw, rh) = (1.0 / f64::from(columns.max(1)), 1.0 / f64::from(rows.max(1)));
        let (x, y) = (f64::from(i) * cw, f64::from(j) * rh);
        let p = |dx: f64, dy: f64, out: bool| if out { [0.0, 0.0] } else { [(x + dx) as f32, (y + dy) as f32] };
        [[x as f32, y as f32], p(0.0, -rh / 3.0, j == 0), p(cw / 3.0, 0.0, i == columns), p(0.0, rh / 3.0, j == rows), p(-cw / 3.0, 0.0, i == 0)]
    }

    /// The bytes After Effects stores for this mesh.
    pub fn to_bytes(&self) -> Option<Vec<u8>> {
        let count = usize::try_from(self.columns.checked_add(1)?.checked_mul(self.rows.checked_add(1)?)?).ok()?;
        if self.vertices.len() != count {
            return None;
        }
        let mut out = Vec::with_capacity(20 + 40 * count);
        out.extend((self.rows + 1).to_be_bytes());
        out.extend((self.columns + 1).to_be_bytes());
        out.extend(self.tail);
        for v in &self.vertices {
            for p in v {
                out.extend(p[0].to_be_bytes());
                out.extend(p[1].to_be_bytes());
            }
        }
        Some(out)
    }
}
