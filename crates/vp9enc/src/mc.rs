//! Inter prediction exactly as the decoder forms it: motion vector clamping (spec 8.5.2.2) and
//! the unscaled block inter prediction process (8.5.2.4) with the regular 8-tap filters and
//! reference samples clamped to the reference frame's visible area.

use crate::tables::SUBPEL_REGULAR;
use rayon::prelude::*;

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
        // Every pass runs over rows in parallel (each sample is a pure function of its inputs).
        raw.par_chunks_mut(rs).enumerate().for_each(|(y, row)| {
            let sy = (y as i32 - off).clamp(0, r.h as i32 - 1) as usize;
            let src = &r.data[sy * r.stride..sy * r.stride + r.w];
            for (x, d) in row.iter_mut().enumerate() {
                *d = src[(x as i32 - off).clamp(0, r.w as i32 - 1) as usize];
            }
        });
        // Horizontal phases over every raw row (inner columns).
        let hpass = |fx: usize| -> Vec<u8> {
            let f = &SUBPEL_REGULAR[fx];
            let mut out = vec![0u8; pw * rh];
            out.par_chunks_mut(pw).enumerate().for_each(|(y, o)| {
                let row = &raw[y * rs..y * rs + rs];
                for (x, d) in o.iter_mut().enumerate() {
                    let s = &row[x + M - 3..x + M + 5];
                    let acc: i32 = 64 + (0..8).map(|t| f[t] * s[t] as i32).sum::<i32>();
                    *d = (acc >> 7).clamp(0, 255) as u8;
                }
            });
            out
        };
        let h: Vec<Vec<u8>> = (0..4)
            .into_par_iter()
            .map(|k| if k == 0 { (0..rh).flat_map(|y| raw[y * rs + M..y * rs + M + pw].iter().copied()).collect() } else { hpass(k * 4) })
            .collect();
        // Phase (fy, fx) at index fy * 4 + fx.
        let planes = (0..16)
            .into_par_iter()
            .map(|i| {
                let (fy, hp) = (i / 4, &h[i % 4]);
                if fy == 0 {
                    return hp[M * pw..(M + ph) * pw].to_vec();
                }
                let f = &SUBPEL_REGULAR[fy * 4];
                let mut out = vec![0u8; pw * ph];
                out.par_chunks_mut(pw).enumerate().for_each(|(y, o)| {
                    for (x, d) in o.iter_mut().enumerate() {
                        let mut acc = 64;
                        for t in 0..8 {
                            acc += f[t] * hp[(y + M + t - 3) * pw + x] as i32;
                        }
                        *d = (acc >> 7).clamp(0, 255) as u8;
                    }
                });
                out
            })
            .collect();
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
