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
                let mut apply = [false; LANES];
                for (l, a) in apply.iter_mut().enumerate().take(n) {
                    let is_tx_edge = tx_ok && !(odd_last && x + (l << sx) + 8 >= mi_cols * 8);
                    *a = is_block_edge || is_tx_edge;
                }
                if apply.iter().any(|&a| a) {
                    let pos = py * stride + px;
                    if pass == 0 {
                        filter_group::<false>(&mut p.rec, pos, stride, 1, apply, wide, lim);
                    } else {
                        filter_group::<true>(&mut p.rec, pos, 1, stride, apply, wide, lim);
                    }
                }
                i += 8;
            }
        }
    }
}

const LANES: usize = 8;

/// [`filter_lane`] on the 8 lanes of a group at once (lane `l` has its q0 at `pos + l * along`;
/// samples across the edge are `step` apart; `ROWS`: `along` is 1), for lanes whose `apply` is
/// set. Every lane is computed with both filters and the result selected per lane (0 / -1
/// masks), so the arithmetic vectorizes; lanes left alone are written back unchanged.
#[inline(always)]
fn filter_group<const ROWS: bool>(d: &mut [u8], pos: usize, along: usize, step: usize, apply: [bool; LANES], wide: bool, lim: Limits) {
    type T = i16;
    let mut v = [[0 as T; LANES]; 8];
    let start = pos - 4 * step;
    if ROWS {
        for (k, vk) in v.iter_mut().enumerate() {
            let Some(row) = d.get(start + k * step..).and_then(|r| r.first_chunk::<LANES>()) else { return };
            *vk = row.map(|s| s as T);
        }
    } else {
        for l in 0..LANES {
            let Some(line) = d.get(start + l * along..).and_then(|r| r.first_chunk::<8>()) else { return };
            for k in 0..8 {
                v[k][l] = line[k] as T;
            }
        }
    }
    let (limit, blimit, thresh) = (lim.limit as T, lim.blimit as T, lim.thresh as T);
    let le = |a: T, b: T| -((a <= b) as T);
    let sel = |m: T, a: T, b: T| (a & m) | (b & !m);
    let (p3, p2, p1, p0, q0, q1, q2, q3) = (&v[0], &v[1], &v[2], &v[3], &v[4], &v[5], &v[6], &v[7]);
    let apply: [T; LANES] = apply.map(|a| -(a as T));
    let can_flat = -(wide as T);
    let mut mask = [0 as T; LANES];
    let mut hev = [0 as T; LANES];
    let mut flat = [0 as T; LANES];
    for l in 0..LANES {
        let d10 = (p1[l] - p0[l]).abs();
        let e10 = (q1[l] - q0[l]).abs();
        mask[l] = apply[l]
            & le((p3[l] - p2[l]).abs(), limit)
            & le((p2[l] - p1[l]).abs(), limit)
            & le(d10, limit)
            & le(e10, limit)
            & le((q2[l] - q1[l]).abs(), limit)
            & le((q3[l] - q2[l]).abs(), limit)
            & le((p0[l] - q0[l]).abs() * 2 + (p1[l] - q1[l]).abs() / 2, blimit);
        hev[l] = !(le(d10, thresh) & le(e10, thresh));
        flat[l] = can_flat
            & mask[l]
            & le(d10, 1)
            & le(e10, 1)
            & le((p2[l] - p0[l]).abs(), 1)
            & le((q2[l] - q0[l]).abs(), 1)
            & le((p3[l] - p0[l]).abs(), 1)
            & le((q3[l] - q0[l]).abs(), 1);
    }
    if mask.iter().fold(0, |a, &m| a | m) == 0 {
        return;
    }
    let mut out = v;
    let c = |x: T| x.clamp(-128, 127);
    for l in 0..LANES {
        let (ps1, ps0, qs0, qs1) = (p1[l] - 128, p0[l] - 128, q0[l] - 128, q1[l] - 128);
        let filter = c((c(ps1 - qs1) & hev[l]) + 3 * (qs0 - ps0));
        let f1 = c(filter + 4) >> 3;
        let f2 = c(filter + 3) >> 3;
        let f = (f1 + 1) >> 1;
        let narrow = mask[l] & !flat[l];
        let outer = narrow & !hev[l];
        out[4][l] = sel(narrow, c(qs0 - f1) + 128, out[4][l]);
        out[3][l] = sel(narrow, c(ps0 + f2) + 128, out[3][l]);
        out[5][l] = sel(outer, c(qs1 - f) + 128, out[5][l]);
        out[2][l] = sel(outer, c(ps1 + f) + 128, out[2][l]);
    }
    if flat.iter().fold(0, |a, &f| a | f) != 0 {
        // 8-tap: positions p2..q2 (indices 1..7), each the rounded mean of a 7-wide window
        // (indices clamped to 0..8) plus itself, as a sliding sum.
        let s = |k: isize| &v[k.clamp(0, 7) as usize];
        let mut sum = [0 as T; LANES];
        for j in -2isize..=4 {
            let a = s(j);
            for l in 0..LANES {
                sum[l] += a[l];
            }
        }
        for k in 1usize..7 {
            let ki = k as isize;
            let (cur, add, sub) = (s(ki), s(ki + 4), s(ki - 3));
            for l in 0..LANES {
                let w8 = (sum[l] + cur[l] + 4) >> 3;
                out[k][l] = sel(flat[l], w8, out[k][l]);
                sum[l] += add[l] - sub[l];
            }
        }
    }
    if ROWS {
        for (k, ok) in out.iter().enumerate().take(7).skip(1) {
            if let Some(row) = d.get_mut(start + k * step..).and_then(|r| r.first_chunk_mut::<LANES>()) {
                for l in 0..LANES {
                    row[l] = ok[l] as u8;
                }
            }
        }
    } else {
        for l in 0..LANES {
            if let Some(line) = d.get_mut(start + l * along..).and_then(|r| r.first_chunk_mut::<8>()) {
                for k in 1..7 {
                    line[k] = out[k][l] as u8;
                }
            }
        }
    }
}

/// Filter mask, narrow (4-tap) and 8-tap flat filters of spec 8.8.5 across one edge position;
/// `pos` is q0, samples across the edge are `step` apart.
#[cfg(test)]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_matches_per_lane_filter() {
        let mut seed = 0x1234_5678u32;
        let mut rnd = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        for case in 0..20000 {
            let stride = 24;
            let base: Vec<u8> = {
                let flatish = case % 3 == 0;
                let c = (rnd() % 256) as i32;
                (0..stride * 24)
                    .map(|_| if flatish { (c + (rnd() % 3) as i32 - 1).clamp(0, 255) as u8 } else { (c + (rnd() % 40) as i32 - 20).clamp(0, 255) as u8 })
                    .collect()
            };
            let l = (rnd() % 64) as i32;
            let lim = Limits { limit: l.max(1), blimit: 2 * (l + 2) + l.max(1), thresh: l >> 4 };
            let wide = rnd() & 1 == 1;
            let apply: [bool; LANES] = std::array::from_fn(|_| rnd() % 4 != 0);
            for pass in 0..2 {
                let (mut a, mut b) = (base.clone(), base.clone());
                let pos = 8 * stride + 8;
                for (l, &ap) in apply.iter().enumerate() {
                    if ap {
                        let (p, st) = if pass == 0 { (pos + l * stride, 1) } else { (pos + l, stride) };
                        filter_lane(&mut a, p, st, wide, lim);
                    }
                }
                if pass == 0 {
                    filter_group::<false>(&mut b, pos, stride, 1, apply, wide, lim);
                } else {
                    filter_group::<true>(&mut b, pos, 1, stride, apply, wide, lim);
                }
                assert_eq!(a, b, "case {case} pass {pass}");
            }
        }
    }
}
