//! The in-loop deblocking filter (spec 8.8), as the decoder applies it to a frame coded with 4×4
//! transforms, 8×8 / 16×16 blocks, no segmentation and no reference/mode deltas: superblocks in
//! raster order, for each plane the vertical edges (pass 0) then the horizontal edges (pass 1).
//! Transform edges inside a skipped inter block are left alone; every 4×4 edge uses the 4-tap
//! filter except edges on a 32-sample boundary, which get the 8-tap (flat) filter.

use crate::{BLOCK_16X16, Mi, Plane};

#[derive(Clone, Copy)]
struct Limits {
    limit: i32,
    blimit: i32,
    thresh: i32,
}

/// Mode-info grid and thresholds shared by every superblock of this frame.
struct FrameFilter<'a> {
    mi: &'a [Mi],
    mi_rows: usize,
    mi_cols: usize,
    limits: Limits,
}

/// Filter the reconstructed planes (Y, U, V, or a prefix of them) in place at `level` (0 = off;
/// sharpness 0).
pub fn filter_frame(planes: &mut [Plane], mi: &[Mi], mi_rows: usize, mi_cols: usize, level: u8) {
    if level == 0 {
        return;
    }
    let l = level as i32;
    let limit = l.max(1);
    let lim = Limits { limit, blimit: 2 * (l + 2) + limit, thresh: l >> 4 };
    let filter = FrameFilter { mi, mi_rows, mi_cols, limits: lim };
    for sbr in 0..mi_rows.div_ceil(8) {
        for sbc in 0..mi_cols.div_ceil(8) {
            for (plane, p) in planes.iter_mut().enumerate() {
                for pass in 0..2 {
                    filter.sb_plane(p, plane, pass, sbr * 8, sbc * 8);
                }
            }
        }
    }
}

impl FrameFilter<'_> {
    fn sb_plane(&self, p: &mut Plane, plane: usize, pass: usize, row: usize, col: usize) {
        let Self { mi, mi_rows, mi_cols, limits: lim } = *self;
        let (sx, sy) = if plane > 0 { (1, 1) } else { (0, 0) };
        let (sub, edge_len) = if pass == 0 { (sx, 64 >> sy) } else { (sy, 64 >> sx) };
        let stride = p.w;
        for edge in 0..(16usize >> sub) {
            let mut i = 0;
            while i < edge_len {
                let (x, y) = if pass == 0 { (col * 8 + edge * (4 << sx), row * 8 + (i << sy)) } else { (col * 8 + (i << sx), row * 8 + edge * (4 << sy)) };
                if x >= 8 * mi_cols || y >= 8 * mi_rows || (pass == 0 && x == 0) || (pass == 1 && y == 0) {
                    i += 8;
                    continue;
                }
                let lc = ((x >> 3) >> sx) << sx;
                let lr = ((y >> 3) >> sy) << sy;
                let m = mi[lr * mi_cols + lc];
                let size = if sub == 0 { m.size } else { m.size.max(BLOCK_16X16) };
                let n8 = if size >= BLOCK_16X16 { 2 } else { 1 };
                let is_block_edge = if pass == 0 { x & (8 * n8 - 1) == 0 } else { y & (8 * n8 - 1) == 0 };
                let tx_ok = !m.inter || !m.skip;
                if !is_block_edge && !tx_ok {
                    i += 8;
                    continue;
                }
                let wide = edge % 8 == 0;
                let n = if pass == 0 { (8 * mi_rows - y).div_ceil(1 << sy) } else { (8 * mi_cols - x).div_ceil(1 << sx) }.min(8).min(edge_len - i);
                // A transform edge is not filtered in the last odd chroma column.
                let odd_last = pass == 1 && sx == 1 && mi_cols & 1 == 1 && edge & 1 == 1;
                let (px, py) = (x >> sx, y >> sy);
                for l in 0..n {
                    let is_tx_edge = tx_ok && !(odd_last && x + (l << sx) + 8 >= mi_cols * 8);
                    if !(is_block_edge || is_tx_edge) {
                        continue;
                    }
                    let (pos, step) = if pass == 0 { ((py + l) * stride + px, 1) } else { (py * stride + px + l, stride) };
                    filter_lane(&mut p.rec, pos, step, wide, lim);
                }
                i += 8;
            }
        }
    }
}

/// Filter mask, narrow (4-tap) and 8-tap flat filters of spec 8.8.5 across one edge position;
/// `pos` is q0, samples across the edge are `step` apart.
fn filter_lane(d: &mut [u8], pos: usize, step: usize, wide: bool, lim: Limits) {
    let at = |k: isize| d[(pos as isize + k * step as isize) as usize] as i32;
    let (p3, p2, p1, p0) = (at(-4), at(-3), at(-2), at(-1));
    let (q0, q1, q2, q3) = (at(0), at(1), at(2), at(3));
    let Limits { limit, blimit, thresh } = lim;
    let mask = (p3 - p2).abs() <= limit
        && (p2 - p1).abs() <= limit
        && (p1 - p0).abs() <= limit
        && (q1 - q0).abs() <= limit
        && (q2 - q1).abs() <= limit
        && (q3 - q2).abs() <= limit
        && (p0 - q0).abs() * 2 + (p1 - q1).abs() / 2 <= blimit;
    if !mask {
        return;
    }
    let flat =
        wide && (p1 - p0).abs() <= 1 && (q1 - q0).abs() <= 1 && (p2 - p0).abs() <= 1 && (q2 - q0).abs() <= 1 && (p3 - p0).abs() <= 1 && (q3 - q0).abs() <= 1;
    let mut set = |k: isize, v: i32| d[(pos as isize + k * step as isize) as usize] = v as u8;
    if flat {
        // 8-tap: positions p2..q2, each the rounded mean of a 7-wide clamped window plus itself.
        let s = [p3, p2, p1, p0, q0, q1, q2, q3];
        let px = |k: isize| s[(k.clamp(-4, 3) + 4) as usize];
        for k in -3isize..3 {
            let mut sum = px(k);
            for j in -3..=3 {
                sum += px(k + j);
            }
            set(k, (sum + 4) >> 3);
        }
        return;
    }
    let c = |v: i32| v.clamp(-128, 127);
    let hev = (p1 - p0).abs() > thresh || (q1 - q0).abs() > thresh;
    let (ps1, ps0, qs0, qs1) = (p1 - 128, p0 - 128, q0 - 128, q1 - 128);
    let filter = c(if hev { c(ps1 - qs1) } else { 0 } + 3 * (qs0 - ps0));
    let f1 = c(filter + 4) >> 3;
    let f2 = c(filter + 3) >> 3;
    set(0, c(qs0 - f1) + 128);
    set(-1, c(ps0 + f2) + 128);
    if !hev {
        let f = (f1 + 1) >> 1;
        set(1, c(qs1 - f) + 128);
        set(-2, c(ps1 + f) + 128);
    }
}
