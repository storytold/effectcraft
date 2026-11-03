//! Inter prediction exactly as the decoder forms it: motion vector clamping (spec 8.5.2.2) and
//! the unscaled block inter prediction process (8.5.2.4) with the regular 8-tap filters and
//! reference samples clamped to the reference frame's visible area.

use crate::tables::SUBPEL_REGULAR;

/// A motion vector in 1/8 luma samples.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mv {
    pub row: i32,
    pub col: i32,
}

impl Mv {
    pub const ZERO: Mv = Mv { row: 0, col: 0 };
    pub fn new(row: i32, col: i32) -> Mv {
        Mv { row, col }
    }
    pub fn minus(self, o: Mv) -> Mv {
        Mv::new(self.row - o.row, self.col - o.col)
    }
}

/// A reference plane: `w`×`h` visible samples at `stride`.
pub struct RefPlane<'a> {
    pub data: &'a [u8],
    pub stride: usize,
    pub w: usize,
    pub h: usize,
}

/// The block's motion vector for `plane` in 1/16 samples of that plane, clamped as the
/// decoder clamps it. `n8`: block size in 8×8 units.
pub fn clamp_for_plane(mv: Mv, r: usize, c: usize, n8: usize, mi_rows: usize, mi_cols: usize, chroma: bool) -> (i32, i32) {
    let s = chroma as i32;
    let (r, c, b) = (r as i32, c as i32, n8 as i32);
    let to_top = (-(r * 8 * 16)) >> s;
    let to_bottom = (((mi_rows as i32 - b - r) * 8) * 16) >> s;
    let to_left = (-(c * 8 * 16)) >> s;
    let to_right = (((mi_cols as i32 - b - c) * 8) * 16) >> s;
    let spel = (4 + ((b * 8) >> s)) << 4;
    let row = ((2 * mv.row) >> s).clamp(to_top - spel, (to_bottom + spel - 16).max(to_top - spel));
    let col = ((2 * mv.col) >> s).clamp(to_left - spel, (to_right + spel - 16).max(to_left - spel));
    (row, col)
}

/// Predict a `w`×`h` block whose top-left sample is at (`x16`, `y16`) (1/16 samples) into `out`
/// (stride `w`).
pub fn predict(r: &RefPlane, x16: i32, y16: i32, w: usize, h: usize, out: &mut [u8]) {
    let (x0, y0) = (x16 >> 4, y16 >> 4);
    let (fx, fy) = ((x16 & 15) as usize, (y16 & 15) as usize);
    let (lx, ly) = (r.w as i32 - 1, r.h as i32 - 1);
    let inside = x0 - 3 >= 0 && y0 - 3 >= 0 && x0 + w as i32 + 4 <= lx && y0 + h as i32 + 4 <= ly;
    let at = |x: i32, y: i32| -> i32 {
        if inside { r.data[y as usize * r.stride + x as usize] as i32 } else { r.data[y.clamp(0, ly) as usize * r.stride + x.clamp(0, lx) as usize] as i32 }
    };
    let fh = &SUBPEL_REGULAR[fx];
    let fv = &SUBPEL_REGULAR[fy];
    if inside && w <= 16 && h <= 16 {
        // Rows straight from the reference (no edge clamping needed).
        let (xs, ys) = ((x0 - 3) as usize, (y0 - 3) as usize);
        let row = |i: usize| &r.data[(ys + i) * r.stride + xs..];
        if fx == 0 && fy == 0 {
            for i in 0..h {
                out[i * w..i * w + w].copy_from_slice(&row(i + 3)[3..3 + w]);
            }
            return;
        }
        let mut tmp = [0u8; 23 * 16];
        for i in 0..h + 7 {
            let s = row(i);
            filt8(std::array::from_fn(|t| &s[t..]), fh, &mut tmp[i * w..i * w + w]);
        }
        for i in 0..h {
            filt8(std::array::from_fn(|t| &tmp[(i + t) * w..]), fv, &mut out[i * w..i * w + w]);
        }
        return;
    }
    if fx == 0 && fy == 0 {
        for i in 0..h {
            for j in 0..w {
                out[i * w + j] = at(x0 + j as i32, y0 + i as i32) as u8;
            }
        }
        return;
    }
    // Horizontal pass over h + 7 rows (from y0 - 3), then vertical.
    let mut tmp = [0u8; 23 * 16];
    for i in 0..h + 7 {
        let y = y0 - 3 + i as i32;
        for j in 0..w {
            let x = x0 + j as i32 - 3;
            let mut acc = 64;
            for t in 0..8 {
                acc += fh[t] * at(x + t as i32, y);
            }
            tmp[i * w + j] = (acc >> 7).clamp(0, 255) as u8;
        }
    }
    for i in 0..h {
        for j in 0..w {
            let mut acc = 64;
            for t in 0..8 {
                acc += fv[t] * tmp[(i + t) * w + j] as i32;
            }
            out[i * w + j] = (acc >> 7).clamp(0, 255) as u8;
        }
    }
}

/// The reference luma pre-interpolated at the 16 quarter-sample phases the encoder uses (motion
/// vectors are even in 1/8 units: fractions 0, 4, 8, 12 in 1/16), over the visible area plus a
/// border. Each phase plane holds exactly what [`predict`] returns at that phase (the decoder's
/// separable filter with edge clamping), so motion search reads predictions directly.
pub struct PhasePlanes {
    border: usize,
    stride: usize,
    rows: usize,
    planes: Vec<Vec<u8>>,
}

impl PhasePlanes {
    pub fn new(r: &RefPlane, border: usize) -> PhasePlanes {
        const M: usize = 4;
        let (pw, ph) = (r.w + 2 * border, r.h + 2 * border);
        // Edge-replicated source with M more samples on every side for the filter taps.
        let rs = pw + 2 * M;
        let rh = ph + 2 * M;
        let off = (border + M) as i32;
        let mut raw = vec![0u8; rs * rh];
        let o = off as usize;
        for (y, row) in raw.chunks_exact_mut(rs).enumerate() {
            let sy = (y as i32 - off).clamp(0, r.h as i32 - 1) as usize;
            let src = &r.data[sy * r.stride..sy * r.stride + r.w];
            // Edge replication: src[clamp(x - off, 0, w - 1)].
            let (left, rest) = row.split_at_mut(o.min(rs));
            left.fill(src[0]);
            let n = r.w.min(rest.len());
            rest[..n].copy_from_slice(&src[..n]);
            rest[n..].fill(src[r.w - 1]);
        }
        // Horizontal phases over every raw row (inner columns).
        let hpass = |fx: usize| -> Vec<u8> {
            let f = &SUBPEL_REGULAR[fx];
            let mut out = vec![0u8; pw * rh];
            for (row, o) in raw.chunks_exact(rs).zip(out.chunks_exact_mut(pw)) {
                filt8(std::array::from_fn(|t| &row[M - 3 + t..]), f, o);
            }
            out
        };
        let unfiltered: Vec<u8> = (0..rh).flat_map(|y| raw[y * rs + M..y * rs + M + pw].iter().copied()).collect();
        let h: Vec<Vec<u8>> = (0..4).map(|k| if k == 0 { unfiltered.clone() } else { hpass(k * 4) }).collect();
        let mut planes = Vec::with_capacity(16);
        for fy in 0..4 {
            for hp in &h {
                if fy == 0 {
                    planes.push(hp[M * pw..(M + ph) * pw].to_vec());
                    continue;
                }
                let f = &SUBPEL_REGULAR[fy * 4];
                let mut out = vec![0u8; pw * ph];
                for (y, o) in out.chunks_exact_mut(pw).enumerate() {
                    filt8(std::array::from_fn(|t| &hp[(y + M + t - 3) * pw..]), f, o);
                }
                planes.push(out);
            }
        }
        PhasePlanes { border, stride: pw, rows: ph, planes }
    }

    /// The prediction of the `n`×`n` block at (`x16`, `y16`) (fractions multiples of 4) as rows of
    /// a phase plane: (plane, offset of the first sample, stride). `None` beyond the border.
    pub fn block(&self, x16: i32, y16: i32, n: usize) -> Option<(&[u8], usize, usize)> {
        let (x0, y0) = ((x16 >> 4) + self.border as i32, (y16 >> 4) + self.border as i32);
        let (fx, fy) = ((x16 & 15) as usize, (y16 & 15) as usize);
        if fx % 4 != 0 || fy % 4 != 0 || x0 < 0 || y0 < 0 || x0 as usize + n > self.stride || y0 as usize + n > self.rows {
            return None;
        }
        Some((&self.planes[(fy / 4) * 4 + fx / 4], y0 as usize * self.stride + x0 as usize, self.stride))
    }
}

/// `out[x] = clip(Round2(sum_t f[t] * s[t][x], 7))` for 8-bit samples. Lanes are 16-bit sums
/// kept modulo 2^16: with the regular filter every sum plus the rounding term lies in
/// [-13706, 46474], so wrapped non-negative sums stay below 0xC000 and negative ones (clipped
/// to 0) land above it (the decoder's `narrow8`).
#[inline]
fn filt8(s: [&[u8]; 8], f: &[i32; 8], out: &mut [u8]) {
    let n = out.len();
    let mut x = 0;
    while x + 32 <= n {
        filt_chunk::<32>(&s, f, out, x);
        x += 32;
    }
    for w in [16, 8, 4] {
        if x + w <= n {
            match w {
                16 => filt_chunk::<16>(&s, f, out, x),
                8 => filt_chunk::<8>(&s, f, out, x),
                _ => filt_chunk::<4>(&s, f, out, x),
            }
            x += w;
        }
    }
    for j in x..n {
        let acc: i32 = 64 + (0..8).map(|t| f[t] * s[t][j] as i32).sum::<i32>();
        out[j] = (acc >> 7).clamp(0, 255) as u8;
    }
}

#[inline(always)]
fn filt_chunk<const W: usize>(s: &[&[u8]; 8], f: &[i32; 8], out: &mut [u8], x: usize) {
    let mut acc = [64u16; W];
    for t in 0..8 {
        let c = f[t] as u16;
        let Some(p) = s[t].get(x..).and_then(|p| p.first_chunk::<W>()) else { return };
        for j in 0..W {
            acc[j] = acc[j].wrapping_add(c.wrapping_mul(p[j] as u16));
        }
    }
    if let Some(o) = out.get_mut(x..).and_then(|o| o.first_chunk_mut::<W>()) {
        for j in 0..W {
            o[j] = if acc[j] < 0xC000 { (acc[j] >> 7).min(255) as u8 } else { 0 };
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn phase_planes_match_predict() {
        let (w, h) = (37usize, 29usize);
        let data: Vec<u8> = (0..w * h).map(|i| ((i * 7919) % 251) as u8).collect();
        let r = RefPlane { data: &data, stride: w, w, h };
        let pp = PhasePlanes::new(&r, 24);
        let mut out = [0u8; 64];
        for y16 in (-20 * 16..(h as i32 + 10) * 16).step_by(4 * 7) {
            for x16 in (-20 * 16..(w as i32 + 10) * 16).step_by(4 * 5) {
                let Some((p, o, s)) = pp.block(x16, y16, 8) else { continue };
                predict(&r, x16, y16, 8, 8, &mut out);
                for i in 0..8 {
                    assert_eq!(&p[o + i * s..o + i * s + 8], &out[i * 8..i * 8 + 8], "({x16}, {y16}) row {i}");
                }
            }
        }
    }

    use super::*;

    #[test]
    fn whole_sample_and_linear_ramps() {
        let (w, h) = (40usize, 30usize);
        let data: Vec<u8> = (0..w * h).map(|i| ((i % w) * 4 + (i / w)) as u8).collect();
        let r = RefPlane { data: &data, stride: w, w, h };
        let mut out = [0u8; 64];
        predict(&r, 10 << 4, 8 << 4, 8, 8, &mut out);
        assert_eq!(out[0], data[8 * w + 10]);
        // Half a sample right on a linear ramp: exact midpoint.
        predict(&r, (10 << 4) + 8, 8 << 4, 8, 8, &mut out);
        assert_eq!(out[0], data[8 * w + 10] + 2);
        // Off the left edge: clamped to column 0.
        predict(&r, -(20 << 4), 0, 4, 4, &mut out);
        assert_eq!(out[0], data[0]);
    }
}
