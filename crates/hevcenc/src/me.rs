//! Motion search reference: the previous reconstructed luma plane padded by edge replication, plus
//! half-sample planes interpolated with the 8-tap half-sample filter. Quarter-sample positions are
//! approximated by averaging neighbouring half-grid samples. Used for estimation only; the coded
//! prediction always uses the normative interpolation in `inter`.

use crate::tables::LUMA_FILTER;
use rayon::prelude::*;

const MARGIN: i32 = 80;

pub struct MeRef {
    /// [integer, horizontal half, vertical half, diagonal half] samples, each `stride` x `rows`.
    planes: [Vec<u16>; 4],
    stride: usize,
    rows: usize,
    w: i32,
    h: i32,
}

impl MeRef {
    /// Built row-parallel (every sample is a pure function of the padded plane, so the result
    /// doesn't depend on the thread count); the horizontal and vertical half planes in parallel.
    pub fn new(y: &[u16], w: usize, h: usize, bd: u32) -> MeRef {
        let m = MARGIN as usize;
        let (stride, rows) = (w + 2 * m, h + 2 * m);
        let mut pad = vec![0u16; stride * rows];
        pad.par_chunks_mut(stride).enumerate().for_each(|(r, row)| {
            let sy = (r as i32 - MARGIN).clamp(0, h as i32 - 1) as usize;
            let src = &y[sy * w..sy * w + w];
            row[..m].fill(src[0]);
            row[m..m + w].copy_from_slice(src);
            row[m + w..].fill(src[w - 1]);
        });
        let max = (1i32 << bd) - 1;
        let f = &LUMA_FILTER[2];
        let filt_h = |p: &[u16]| -> Vec<u16> {
            let mut o = vec![0u16; stride * rows];
            o.par_chunks_mut(stride).enumerate().for_each(|(r, out)| {
                let row = &p[r * stride..r * stride + stride];
                for (x, d) in out.iter_mut().enumerate() {
                    let mut s = 0i32;
                    for k in 0..8 {
                        let xx = (x as i32 - 3 + k as i32).clamp(0, stride as i32 - 1) as usize;
                        s += f[k] as i32 * row[xx] as i32;
                    }
                    *d = ((s + 32) >> 6).clamp(0, max) as u16;
                }
            });
            o
        };
        let filt_v = |p: &[u16]| -> Vec<u16> {
            let mut o = vec![0u16; stride * rows];
            o.par_chunks_mut(stride).enumerate().for_each_init(
                || vec![0i32; stride],
                |acc, (r, out)| {
                    acc.fill(0);
                    for k in 0..8 {
                        let rr = (r as i32 - 3 + k as i32).clamp(0, rows as i32 - 1) as usize;
                        let src = &p[rr * stride..rr * stride + stride];
                        let coef = f[k] as i32;
                        for (a, &s) in acc.iter_mut().zip(src) {
                            *a += coef * s as i32;
                        }
                    }
                    for (d, &a) in out.iter_mut().zip(acc.iter()) {
                        *d = ((a + 32) >> 6).clamp(0, max) as u16;
                    }
                },
            );
            o
        };
        // The diagonal plane filters the horizontal one vertically.
        let ((hp, dp), vp) = rayon::join(
            || {
                let hp = filt_h(&pad);
                let dp = filt_v(&hp);
                (hp, dp)
            },
            || filt_v(&pad),
        );
        MeRef { planes: [pad, hp, vp, dp], stride, rows, w: w as i32, h: h as i32 }
    }

    /// Integer motion vector range keeping an n x n block (plus filter margin) inside the padding.
    pub fn int_range(&self, x: i32, y: i32, n: usize) -> (i32, i32, i32, i32) {
        let n = n as i32;
        let lo_x = -MARGIN + 4 - x;
        let hi_x = self.w + MARGIN - n - 4 - x;
        let lo_y = -MARGIN + 4 - y;
        let hi_y = self.h + MARGIN - n - 4 - y;
        (lo_x.max(-256), hi_x.min(256), lo_y.max(-256), hi_y.min(256))
    }

    /// SAD of the source block against the integer-position reference block at (rx, ry).
    pub fn sad(&self, src: &[u16], ss: usize, rx: i32, ry: i32, n: usize) -> u32 {
        let p = &self.planes[0];
        let base = (ry + MARGIN) as usize * self.stride + (rx + MARGIN) as usize;
        let mut s = 0u32;
        for r in 0..n {
            let a = &src[r * ss..r * ss + n];
            let b = &p[base + r * self.stride..base + r * self.stride + n];
            s += a.iter().zip(b).map(|(&x, &y)| (x as i32 - y as i32).unsigned_abs()).sum::<u32>();
        }
        s
    }

    /// Approximate luma prediction of an n x n block at (x, y) with quarter-sample vector `mv`.
    pub fn approx_pred(&self, x: i32, y: i32, n: usize, mv: [i16; 2], out: &mut [u16]) {
        // per axis: one or two (half-sample fraction, integer offset) taps
        let axis = |v: i32| -> [(usize, i32); 2] {
            if v & 1 == 0 {
                let hv = v >> 1;
                [((hv & 1) as usize, hv >> 1); 2]
            } else {
                let (a, b) = ((v - 1) >> 1, (v + 1) >> 1);
                [((a & 1) as usize, a >> 1), ((b & 1) as usize, b >> 1)]
            }
        };
        let (mx, my) = (mv[0] as i32, mv[1] as i32);
        let ax = axis(mx);
        let ay = axis(my);
        let two = mx & 1 != 0 || my & 1 != 0;
        let taps = [(ax[0], ay[0]), (ax[1], ay[1])];
        let ntaps = if two { 2 } else { 1 };
        let mut first = true;
        for &((fx, ox), (fy, oy)) in taps.iter().take(ntaps) {
            let plane = &self.planes[fx + 2 * fy];
            let (bx, by) = (x + ox + MARGIN, y + oy + MARGIN);
            let inside = bx >= 0 && by >= 0 && bx as usize + n <= self.stride && by as usize + n <= self.rows;
            for r in 0..n {
                let dst = &mut out[r * n..r * n + n];
                if inside {
                    let o = (by as usize + r) * self.stride + bx as usize;
                    let src = &plane[o..o + n];
                    if first {
                        dst.copy_from_slice(src);
                    } else {
                        for (d, &s) in dst.iter_mut().zip(src) {
                            *d = ((*d as u32 + s as u32 + 1) >> 1) as u16;
                        }
                    }
                } else {
                    let yy = (by + r as i32).clamp(0, self.rows as i32 - 1) as usize;
                    for (c, d) in dst.iter_mut().enumerate() {
                        let xx = (bx + c as i32).clamp(0, self.stride as i32 - 1) as usize;
                        let s = plane[yy * self.stride + xx];
                        *d = if first { s } else { ((*d as u32 + s as u32 + 1) >> 1) as u16 };
                    }
                }
            }
            first = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The serial construction as it was before rows went parallel.
    fn serial_planes(y: &[u16], w: usize, h: usize, bd: u32) -> [Vec<u16>; 4] {
        let m = MARGIN as usize;
        let (stride, rows) = (w + 2 * m, h + 2 * m);
        let mut pad = vec![0u16; stride * rows];
        for r in 0..rows {
            let sy = (r as i32 - MARGIN).clamp(0, h as i32 - 1) as usize;
            let src = &y[sy * w..sy * w + w];
            let row = &mut pad[r * stride..r * stride + stride];
            row[..m].fill(src[0]);
            row[m..m + w].copy_from_slice(src);
            row[m + w..].fill(src[w - 1]);
        }
        let max = (1i32 << bd) - 1;
        let f = &LUMA_FILTER[2];
        let filt_h = |p: &[u16]| -> Vec<u16> {
            let mut o = vec![0u16; stride * rows];
            for r in 0..rows {
                for x in 0..stride {
                    let mut s = 0i32;
                    for k in 0..8 {
                        let xx = (x as i32 - 3 + k as i32).clamp(0, stride as i32 - 1) as usize;
                        s += f[k] as i32 * p[r * stride + xx] as i32;
                    }
                    o[r * stride + x] = ((s + 32) >> 6).clamp(0, max) as u16;
                }
            }
            o
        };
        let filt_v = |p: &[u16]| -> Vec<u16> {
            let mut o = vec![0u16; stride * rows];
            for r in 0..rows {
                for x in 0..stride {
                    let mut s = 0i32;
                    for k in 0..8 {
                        let rr = (r as i32 - 3 + k as i32).clamp(0, rows as i32 - 1) as usize;
                        s += f[k] as i32 * p[rr * stride + x] as i32;
                    }
                    o[r * stride + x] = ((s + 32) >> 6).clamp(0, max) as u16;
                }
            }
            o
        };
        let hp = filt_h(&pad);
        let vp = filt_v(&pad);
        let dp = filt_v(&hp);
        [pad, hp, vp, dp]
    }

    #[test]
    fn parallel_planes_match_serial() {
        for (w, h, bd) in [(8usize, 8usize, 8u32), (72, 40, 8), (130, 66, 10)] {
            let max = (1u32 << bd) - 1;
            let y: Vec<u16> = (0..w * h).map(|i| ((i as u32 * 7919 + (i / w) as u32 * 31) % (max + 1)) as u16).collect();
            let me = MeRef::new(&y, w, h, bd);
            assert!(me.planes == serial_planes(&y, w, h, bd), "{w}x{h} {bd}-bit");
        }
    }
}
