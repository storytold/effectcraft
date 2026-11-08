//! Arbitrary-data effect parameters (type 11): the values of Hue/Saturation and Levels that are not
//! ordinary parameters.
//!
//! In the effect's value group (`sspc/tdgp`) the parameter's `tdbs` (`tdsb` 1, a 4-byte `cdat`) is
//! followed by a `LIST aRbs` holding one `aRbp`: the data. The `aRbp` in `parT` right after the
//! parameter's `pard` holds the default (443, 511). Setting Master Hue, Saturation or Lightness of
//! Hue/Saturation (444-446) or a Levels value (512-516) changes only bytes of this `aRbp` (and stale
//! UI bytes of `pard`); the visible parameters of the same names keep no record.

use crate::container::Chunk;
use crate::container::bytes::{be_f32, be_i32, be_u16, be_u32};
use crate::container::effects::Effect;

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
/// columns plus one (a 1 x 2-cell read-back test renders as made only in this order, `export-tests\mesh`;
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

/// EXtractoR's Channel Info data (`EXtractoR-0013`, 688 bytes, the plug-in's own little-endian
/// layout; read from the instances of the importer block of 2026-11-05: the defaults and a preset
/// of our own that reads `Z` into Red, Green and Blue): a `u32` 1 and 28 bytes, then four records
/// of 164 bytes (Red, Green, Blue, Alpha), each a kind in byte 0 (1 a channel of the footage by
/// name, 2 the input's own channel: `(copy)`; bytes 1..4 are not always zero, `0x32` at 3 in one
/// definition's default, so only byte 0 is read) and the channel name from byte 35,
/// NUL-terminated. The other bytes are not read (they differ between instances with the same
/// channels; the `u32` at 4 of a chosen channel looks like its position in the footage's sorted
/// channel list, 13 of 14 for `Z`, where the defaults hold 0). Only the name is read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractorChannels {
    /// Per output channel (red, green, blue, alpha): the kind and the name.
    pub channels: [(u32, String); 4],
}

impl ExtractorChannels {
    /// The data's length.
    pub const LEN: usize = 688;
    const RECORD: usize = 164;

    /// `None` when the data does not have this layout.
    pub fn parse(data: &[u8]) -> Option<Self> {
        if data.len() != Self::LEN || data.get(..4)? != 1u32.to_le_bytes() {
            return None;
        }
        let mut channels: [(u32, String); 4] = Default::default();
        for (k, slot) in channels.iter_mut().enumerate() {
            let at = 32usize.checked_add(k.checked_mul(Self::RECORD)?)?;
            let rec = data.get(at..at.checked_add(Self::RECORD)?)?;
            let kind = u32::from(*rec.first()?);
            let name = rec.get(35..)?;
            let end = name.iter().position(|b| *b == 0)?;
            let name = std::str::from_utf8(name.get(..end)?).ok()?;
            *slot = (kind, name.to_string());
        }
        Some(Self { channels })
    }

    /// The channels of a fresh EXtractoR: Red, Green and Blue from the channels of those names,
    /// Alpha copied from the input.
    pub fn is_default(&self) -> bool {
        let want = [(1, "R"), (1, "G"), (1, "B"), (2, "(copy)")];
        self.channels.iter().zip(want).all(|((k, n), (wk, wn))| *k == wk && n == wn)
    }

    /// Per output channel (red, green, blue, alpha) the channel it reads, `Err` with the
    /// position of the first record of another kind or without a name: a named channel by its
    /// name, `(copy)` as EffectCraft's [`EXTRACTOR_COPY`](effectcraft_effects::EXTRACTOR_COPY),
    /// the effect input's own channel of the same position. It is not `R`, `G`, `B` or `A`:
    /// EffectCraft's EXtractoR reads those from an OpenEXR footage's channels of that name, the
    /// file's own, which loses what an earlier effect did to the channel.
    pub fn sources(&self) -> Result<[String; 4], usize> {
        let mut out: [String; 4] = Default::default();
        for (k, ((kind, name), slot)) in self.channels.iter().zip(out.iter_mut()).enumerate() {
            *slot = match (kind, name.trim()) {
                (1, n) if !n.is_empty() => n.to_string(),
                (2, _) => effectcraft_effects::EXTRACTOR_COPY.to_string(),
                _ => return Err(k),
            };
        }
        Ok(out)
    }
}

/// IDentifier's Channel Info data (`IDentifier-0005`, 148 bytes): the chosen channel's name,
/// NUL-terminated from byte 19, or `Object ID` when none is chosen (importer blocks 1300 and 1310
/// of 2026-11-05: `objectId` in a preset of our own that chose that UINT channel, `Object ID` in
/// every other instance). The bytes around it differ between instances with the same settings
/// and are not read (a chosen channel has 15 at byte 4, which looks like its position in the
/// EXR's sorted channel list, as for EXtractoR).
pub fn identifier_channel_type(data: &[u8]) -> Option<&str> {
    let s = data.get(19..)?;
    let end = s.iter().position(|b| *b == 0)?;
    std::str::from_utf8(s.get(..end)?).ok()
}

/// The Cryptomatte plug-in's Settings data (`Cryptomatte-0001`, the plug-in's own layout,
/// little-endian; read from 88 instances of one project and from our own sample 1400, fresh
/// instances and two presets that pick a layer and an object, docs/aep-format.md 3.7): the magic
/// `cry1`; bytes 4..32 are not read (they differ between instances holding the same settings);
/// the Cryptomatte layer's name at 32, NUL-padded to 64 bytes (`CryptoObject`); at 96 a `u32` 1
/// and a `u32` (neither read); at 104 the `u32` length of the selection text including its NUL,
/// the `u32` MurmurHash3 (x86, 32-bit, seed 0) of that text with its NUL (not read) and a zero
/// byte; the selection text from byte 113 (`<11a97486>, <deadbeef>`, the matte list as ID hashes,
/// the hashes of the picked names). Bytes after it are left-overs of a longer earlier text. A
/// fresh effect stores no layer and an empty text (length 1, its NUL); the `u32`s at 100 and 108
/// are then both `0x514e28b7`, the hash of one NUL byte: the one at 100 looks like the hash of a
/// first text that is empty in every sample, the zero byte at 112 like that text.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CryptomatteData {
    /// The Cryptomatte layer's name (`CryptoMaterial`: the EXR layer's name, which is also its
    /// channel prefix in every sample); empty when none is chosen.
    pub layer: String,
    /// The selection (matte list) as the plug-in stores it.
    pub selection: String,
}

impl CryptomatteData {
    /// Decodes the data; `None` when it is not this layout (no `cry1`, too short, a length past
    /// the end, no zero byte at 112, a text without its NUL, text that is not UTF-8).
    pub fn parse(data: &[u8]) -> Option<Self> {
        if data.get(..4)? != b"cry1" {
            return None;
        }
        let text = |b: &[u8]| -> Option<String> {
            let end = b.iter().position(|c| *c == 0).unwrap_or(b.len());
            String::from_utf8(b.get(..end)?.to_vec()).ok()
        };
        let layer = text(data.get(32..96)?)?;
        let len = usize::try_from(u32::from_le_bytes(data.get(104..108)?.try_into().ok()?)).ok()?;
        if *data.get(112)? != 0 {
            return None;
        }
        let selection = if len == 0 {
            String::new()
        } else {
            // The length counts the text's NUL as its last byte: a text without it, or with an early
            // NUL before more bytes (which `text` would silently cut short), is not this layout.
            let b = data.get(113..113usize.checked_add(len)?)?;
            let (last, text_bytes) = b.split_last()?;
            if *last != 0 || text_bytes.contains(&0) {
                return None;
            }
            text(b)?
        };
        Some(Self { layer, selection })
    }
}

#[cfg(test)]
mod tests {
    use super::CryptomatteData;

    /// Settings data in the layout [`CryptomatteData::parse`] reads, with `noise` in the bytes it
    /// does not read.
    fn crypto(layer: &str, selection: &str, noise: u8) -> Vec<u8> {
        let mut d = b"cry1".to_vec();
        d.resize(32, noise);
        d.extend(layer.as_bytes());
        d.resize(96, 0);
        d.extend(1u32.to_le_bytes());
        d.extend([noise; 4]);
        let len = if selection.is_empty() { 0 } else { selection.len() + 1 };
        d.extend(u32::try_from(len).unwrap_or(0).to_le_bytes());
        d.extend([noise; 4]);
        d.push(0);
        d.extend(selection.as_bytes());
        d.push(0);
        // Left-overs of an earlier, longer text.
        d.extend(b" <de");
        d
    }

    #[test]
    fn cryptomatte_settings_are_read() {
        for noise in [0, 0x5a] {
            let d = CryptomatteData::parse(&crypto("Layer_A", "<0000002a>, <deadbeef>", noise));
            assert_eq!(d, Some(CryptomatteData { layer: "Layer_A".into(), selection: "<0000002a>, <deadbeef>".into() }), "noise {noise}");
        }
    }

    #[test]
    fn a_selection_whose_nul_is_not_its_last_byte_is_refused() {
        // An early NUL followed by more declared bytes would shorten the selection silently.
        let mut d = crypto("Layer_A", "<0000002a>, <deadbeef>", 0);
        d[113 + 4] = 0;
        assert_eq!(CryptomatteData::parse(&d), None);
        // No NUL at the end of the declared length.
        let mut d = crypto("Layer_A", "<0000002a>", 0);
        d[113 + 10] = b'x';
        assert_eq!(CryptomatteData::parse(&d), None);
    }

    #[test]
    fn the_default_cryptomatte_settings_are_empty() {
        let mut d = b"cry1".to_vec();
        d.resize(116, 0);
        assert_eq!(CryptomatteData::parse(&d), Some(CryptomatteData::default()));
        // What After Effects 2026 wrote for a fresh effect (plug-in 2.6.6, sample 1400
        // `crypto-00-defaults`, the `pard` default and the instance's record alike): an empty text
        // of length 1 and its hash, twice.
        let mut fresh = b"cry1".to_vec();
        fresh.resize(96, 0);
        for _ in 0..2 {
            fresh.extend(1u32.to_le_bytes());
            fresh.extend(0x514e_28b7u32.to_le_bytes());
        }
        fresh.extend([0; 4]);
        assert_eq!(fresh.len(), 116);
        assert_eq!(CryptomatteData::parse(&fresh), Some(CryptomatteData::default()));
    }

    /// The Settings data After Effects 2026 wrote for the two presets of sample 1400 (plug-in
    /// 2.6.6, `crypto-06-preset-object-box` and `crypto-07-preset-material-paint`: a layer chosen
    /// and one object or material picked in the plug-in's UI), byte for byte: 127 bytes, the `u32`
    /// at 108 the MurmurHash3 of the text with its NUL, three zero bytes after the text.
    #[test]
    fn the_presets_of_our_sample_are_read() {
        for (layer, selection, hash) in [("CryptoObject", "<11a97486>", 0x2788_0767u32), ("CryptoMaterial", "<a5deeb9e>", 0xf737_4a5b)] {
            let mut d = b"cry1".to_vec();
            d.resize(32, 0);
            d.extend(layer.as_bytes());
            d.resize(96, 0);
            for v in [1, 0x514e_28b7, 11, hash] {
                d.extend(u32::to_le_bytes(v));
            }
            d.push(0);
            d.extend(selection.as_bytes());
            d.extend([0; 4]);
            assert_eq!(d.len(), 127);
            assert_eq!(CryptomatteData::parse(&d), Some(CryptomatteData { layer: layer.into(), selection: selection.into() }), "{layer}");
        }
    }

    #[test]
    fn damaged_cryptomatte_settings_are_not_read() {
        let good = crypto("L", "<00000001>", 0);
        let mut bad_magic = good.clone();
        bad_magic[0] = b'x';
        let mut past_end = good.clone();
        past_end[104..108].copy_from_slice(&u32::MAX.to_le_bytes());
        let mut no_zero = good.clone();
        no_zero[112] = b'<';
        let mut not_utf8 = good.clone();
        not_utf8[32] = 0xff;
        // The length covers the text without its NUL (`<00000001>` is 10 bytes, its NUL the 11th).
        let mut no_nul = good.clone();
        no_nul[104..108].copy_from_slice(&10u32.to_le_bytes());
        // No zero byte at 112 with an empty text.
        let mut empty_no_zero = crypto("L", "", 0);
        empty_no_zero[112] = b'<';
        for (what, d) in [
            ("magic", bad_magic),
            ("length", past_end),
            ("zero byte", no_zero),
            ("utf-8", not_utf8),
            ("no NUL", no_nul),
            ("empty text, zero byte", empty_no_zero),
            ("short", good[..100].to_vec()),
            ("empty", Vec::new()),
        ] {
            assert_eq!(CryptomatteData::parse(&d), None, "{what}");
        }
    }
}
