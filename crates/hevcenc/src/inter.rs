//! Fractional sample interpolation (8.5.3.3.3) and default weighted prediction (8-262), mirrored
//! from the filmcraft-hevc decoder. Reference sample fetches clamp to the picture (8-228, 8-229).

use crate::tables::{CHROMA_FILTER, LUMA_FILTER};

/// One plane of a reference picture.
#[derive(Clone, Copy)]
pub struct PlaneRef<'a> {
    pub data: &'a [u16],
    pub w: usize,
    pub h: usize,
}

impl PlaneRef<'_> {
    /// Copy a w x h window at (x0, y0) with edge replication.
    fn window(&self, x0: i32, y0: i32, w: usize, h: usize, out: &mut [i16], os: usize) {
        let (pw, ph) = (self.w as i32, self.h as i32);
        let inside_x = x0 >= 0 && x0 + w as i32 <= pw;
        for r in 0..h {
            let yy = (y0 + r as i32).clamp(0, ph - 1) as usize;
            let row = &self.data[yy * self.w..yy * self.w + self.w];
            let dst = &mut out[r * os..r * os + w];
            if inside_x {
                for (d, &s) in dst.iter_mut().zip(&row[x0 as usize..x0 as usize + w]) {
                    *d = s as i16;
                }
            } else {
                for (c, d) in dst.iter_mut().enumerate() {
                    *d = row[(x0 + c as i32).clamp(0, pw - 1) as usize] as i16;
                }
            }
        }
    }
}

/// Luma prediction samples (14-bit intermediate) of a w x h block at (x, y) displaced by the
/// quarter-sample vector `mv`, into `out` (stride w).
pub fn mc_luma(f: PlaneRef, bd: u32, x: i32, y: i32, mv: [i16; 2], size: (usize, usize), out: &mut [i16]) {
    let (w, h) = size;
    let (fx, fy) = ((mv[0] & 3) as usize, (mv[1] & 3) as usize);
    let xi = x + (mv[0] as i32 >> 2);
    let yi = y + (mv[1] as i32 >> 2);
    let shift1 = bd.min(12) - 8;
    let shift3 = 14 - bd;
    let ws = w + 7;
    let mut win = vec![0i16; ws * (h + 7)];
    if fx == 0 && fy == 0 {
        f.window(xi, yi, w, h, &mut win, ws);
        for r in 0..h {
            for c in 0..w {
                out[r * w + c] = win[r * ws + c] << shift3;
            }
        }
        return;
    }
    f.window(xi - 3, yi - 3, w + 7, h + 7, &mut win, ws);
    let hf = &LUMA_FILTER[fx];
    let vf = &LUMA_FILTER[fy];
    if fy == 0 {
        for r in 0..h {
            let row = &win[(r + 3) * ws..];
            for c in 0..w {
                let s: i32 = (0..8).map(|k| hf[k] as i32 * row[c + k] as i32).sum();
                out[r * w + c] = (s >> shift1) as i16;
            }
        }
    } else if fx == 0 {
        for r in 0..h {
            for c in 0..w {
                let s: i32 = (0..8).map(|k| vf[k] as i32 * win[(r + k) * ws + c + 3] as i32).sum();
                out[r * w + c] = (s >> shift1) as i16;
            }
        }
    } else {
        let mut tmp = vec![0i16; ws * (h + 7)];
        for r in 0..h + 7 {
            let row = &win[r * ws..];
            for c in 0..w {
                let s: i32 = (0..8).map(|k| hf[k] as i32 * row[c + k] as i32).sum();
                tmp[r * ws + c] = (s >> shift1) as i16;
            }
        }
        for r in 0..h {
            for c in 0..w {
                let s: i32 = (0..8).map(|k| vf[k] as i32 * tmp[(r + k) * ws + c] as i32).sum();
                out[r * w + c] = (s >> 6) as i16;
            }
        }
    }
}

/// Chroma prediction samples of a w x h block at chroma position (x, y) with a 1/8-sample vector.
pub fn mc_chroma(f: PlaneRef, bd: u32, x: i32, y: i32, mv: [i16; 2], size: (usize, usize), out: &mut [i16]) {
    let (w, h) = size;
    let (fx, fy) = ((mv[0] & 7) as usize, (mv[1] & 7) as usize);
    let xi = x + (mv[0] as i32 >> 3);
    let yi = y + (mv[1] as i32 >> 3);
    let shift1 = bd.min(12) - 8;
    let shift3 = 14 - bd;
    let ws = w + 7;
    let mut win = vec![0i16; ws * (h + 7)];
    if fx == 0 && fy == 0 {
        f.window(xi, yi, w, h, &mut win, ws);
        for r in 0..h {
            for c in 0..w {
                out[r * w + c] = win[r * ws + c] << shift3;
            }
        }
        return;
    }
    f.window(xi - 1, yi - 1, w + 3, h + 3, &mut win, ws);
    let hf = &CHROMA_FILTER[fx];
    let vf = &CHROMA_FILTER[fy];
    if fy == 0 {
        for r in 0..h {
            let row = &win[(r + 1) * ws..];
            for c in 0..w {
                let s: i32 = (0..4).map(|k| hf[k] as i32 * row[c + k] as i32).sum();
                out[r * w + c] = (s >> shift1) as i16;
            }
        }
    } else if fx == 0 {
        for r in 0..h {
            for c in 0..w {
                let s: i32 = (0..4).map(|k| vf[k] as i32 * win[(r + k) * ws + c + 1] as i32).sum();
                out[r * w + c] = (s >> shift1) as i16;
            }
        }
    } else {
        let mut tmp = vec![0i16; ws * (h + 7)];
        for r in 0..h + 3 {
            let row = &win[r * ws..];
            for c in 0..w {
                let s: i32 = (0..4).map(|k| hf[k] as i32 * row[c + k] as i32).sum();
                tmp[r * ws + c] = (s >> shift1) as i16;
            }
        }
        for r in 0..h {
            for c in 0..w {
                let s: i32 = (0..4).map(|k| vf[k] as i32 * tmp[(r + k) * ws + c] as i32).sum();
                out[r * w + c] = (s >> 6) as i16;
            }
        }
    }
}

/// Default weighted prediction, one list (8-262): 14-bit samples to `bd`-bit samples.
pub fn put_uni(p: &[i16], w: usize, h: usize, bd: u32, dst: &mut [u16], ds: usize) {
    let shift = 14 - bd;
    let off = if shift > 0 { 1 << (shift - 1) } else { 0 };
    let max = (1i32 << bd) - 1;
    for r in 0..h {
        for c in 0..w {
            dst[r * ds + c] = ((p[r * w + c] as i32 + off) >> shift).clamp(0, max) as u16;
        }
    }
}
