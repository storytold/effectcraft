//! Deblocking loop filter (spec 7.14), mirrored from the decoder process so that the encoder's
//! references equal a decoder's. Segmentation and loop filter deltas are off and the sharpness
//! is 0 in the frames this encoder writes.

use crate::pred::Plane;
use crate::tables::{MAX_LOOP_FILTER, NUM_4X4_BLOCKS_HIGH, NUM_4X4_BLOCKS_WIDE, SUBSAMPLED_SIZE, TX_HEIGHT, TX_WIDTH};

/// Mode info the loop filter reads (per 4x4 luma unit, row stride `mi_cols`), and the
/// transform size per 4x4 unit of each plane (`LoopfilterTxSizes`, row stride `mi_cols + 32`).
pub(crate) struct LfInfo<'a> {
    pub mi_cols: usize,
    pub mi_rows: usize,
    pub width: usize,
    pub height: usize,
    pub mi_size: &'a [u8],
    pub skip: &'a [bool],
    pub is_inter: &'a [bool],
    pub tx: [&'a [u8]; 3],
}

/// Applies the loop filter with `loop_filter_level[ 0..4 ]` to `planes`. For level search
/// (`luma_only`), only plane 0 and only every `sb_row_step`-th superblock row are filtered.
pub(crate) fn loop_filter(planes: &mut [Plane; 3], info: &LfInfo, level: [u32; 4], bit_depth: u32, luma_only: bool, sb_row_step: usize) {
    if level[0] == 0 && level[1] == 0 {
        return;
    }
    let nplanes = if luma_only { 1 } else { 3 };
    for (plane, pl) in planes.iter_mut().enumerate().take(nplanes) {
        if plane > 0 && level[1 + plane] == 0 {
            continue;
        }
        for pass in 0..2 {
            let step = if plane == 0 { 1 } else { 2 };
            let mut row = 0;
            while row < info.mi_rows {
                if !(row >> 4).is_multiple_of(sb_row_step) {
                    row += step;
                    continue;
                }
                let mut col = 0;
                while col < info.mi_cols {
                    edge(pl, info, level, bit_depth, plane, pass, (row, col));
                    col += step;
                }
                row += step;
            }
        }
    }
}

fn edge(pl: &mut Plane, info: &LfInfo, level: [u32; 4], bd: u32, plane: usize, pass: usize, position: (usize, usize)) {
    let (row, col) = position;
    let s = (plane > 0) as usize;
    let (dx, dy) = if pass == 0 { (1usize, 0usize) } else { (0, 1) };
    let x = col * 4;
    let y = row * 4;
    let row = row | s;
    let col = col | s;
    let on_screen = !(x >= info.width || y >= info.height || (pass == 0 && x == 0) || (pass == 1 && y == 0));
    if !on_screen {
        return;
    }
    let xp = x >> s;
    let yp = y >> s;
    let prev_row = row - (dy << s);
    let prev_col = col - (dx << s);
    let i = row * info.mi_cols + col;
    let mi_size = info.mi_size[i] as usize;
    let tx_stride = info.mi_cols + 32;
    let tx_sz = info.tx[plane][(row >> s) * tx_stride + (col >> s)] as usize;
    let plane_size = SUBSAMPLED_SIZE[mi_size][s][s] as usize;
    let skip = info.skip[i];
    let is_intra = !info.is_inter[i];
    let prev_tx_sz = info.tx[plane][(prev_row >> s) * tx_stride + (prev_col >> s)] as usize;
    let is_block_edge = if pass == 0 {
        xp.is_multiple_of(4 * NUM_4X4_BLOCKS_WIDE[plane_size] as usize)
    } else {
        yp.is_multiple_of(4 * NUM_4X4_BLOCKS_HIGH[plane_size] as usize)
    };
    let is_tx_edge = if pass == 0 { xp.is_multiple_of(TX_WIDTH[tx_sz] as usize) } else { yp.is_multiple_of(TX_HEIGHT[tx_sz] as usize) };
    let apply_filter = is_tx_edge && (is_block_edge || !skip || is_intra);
    let base_size = if pass == 0 { TX_WIDTH[prev_tx_sz].min(TX_WIDTH[tx_sz]) } else { TX_HEIGHT[prev_tx_sz].min(TX_HEIGHT[tx_sz]) } as usize;
    let filter_size = if plane == 0 { 16.min(base_size) } else { 8.min(base_size) };
    // 7.14.4 with no deltas, segmentation or sharpness: the same strength for every block
    let lvl = level[if plane == 0 { pass } else { plane + 1 }].min(MAX_LOOP_FILTER as u32) as i32;
    if !apply_filter || lvl == 0 {
        return;
    }
    let limit = lvl.max(1);
    let blimit = 2 * (lvl + 2) + limit;
    let thresh = lvl >> 4;
    let strength = FilterStrength { limit, blimit, thresh };
    for k in 0..4 {
        sample_filter(pl, (xp + dy * k, yp + dx * k), plane, &strength, (dx as isize, dy as isize), filter_size, bd);
    }
}

/// Thresholds derived from the loop-filter level (spec 7.14.4).
struct FilterStrength {
    limit: i32,
    blimit: i32,
    thresh: i32,
}

#[inline]
fn sample_filter(pl: &mut Plane, position: (usize, usize), plane: usize, strength: &FilterStrength, direction: (isize, isize), filter_size: usize, bd: u32) {
    let (x, y) = position;
    let (dx, dy) = direction;
    let FilterStrength { limit, blimit, thresh } = *strength;
    let stride = pl.stride as isize;
    let step = dy * stride + dx;
    let base = y as isize * stride + x as isize;
    let reach: isize = if filter_size >= 16 { 7 } else { 4 };
    let mut v = [0i32; 16];
    for k in -reach..reach {
        v[(k + 8) as usize] = pl.data[(base + k * step) as usize] as i32;
    }
    let at = |k: isize| -> i32 { v[(k + 8) as usize] };
    let (q0, q1, q2, q3) = (at(0), at(1), at(2), at(3));
    let (p0, p1, p2, p3) = (at(-1), at(-2), at(-3), at(-4));
    let d = &mut pl.data;
    let sh = bd - 8;
    let thresh_bd = thresh << sh;
    let hev = (p1 - p0).abs() > thresh_bd || (q1 - q0).abs() > thresh_bd;
    let filter_len = if filter_size == 4 {
        4
    } else if plane != 0 {
        6
    } else if filter_size == 8 {
        8
    } else {
        16
    };
    let limit_bd = limit << sh;
    let blimit_bd = blimit << sh;
    let mut mask = (p1 - p0).abs() > limit_bd || (q1 - q0).abs() > limit_bd || (p0 - q0).abs() * 2 + (p1 - q1).abs() / 2 > blimit_bd;
    if filter_len >= 6 {
        mask |= (p2 - p1).abs() > limit_bd || (q2 - q1).abs() > limit_bd;
    }
    if filter_len >= 8 {
        mask |= (p3 - p2).abs() > limit_bd || (q3 - q2).abs() > limit_bd;
    }
    if mask {
        return;
    }
    let t_bd = 1 << sh;
    let flat = if filter_size >= 8 {
        let mut m = (p1 - p0).abs() > t_bd || (q1 - q0).abs() > t_bd || (p2 - p0).abs() > t_bd || (q2 - q0).abs() > t_bd;
        if filter_len >= 8 {
            m |= (p3 - p0).abs() > t_bd || (q3 - q0).abs() > t_bd;
        }
        !m
    } else {
        false
    };
    let flat2 = if filter_size >= 16 {
        let (q4, q5, q6, p4, p5, p6) = (at(4), at(5), at(6), at(-5), at(-6), at(-7));
        !((p6 - p0).abs() > t_bd
            || (q6 - q0).abs() > t_bd
            || (p5 - p0).abs() > t_bd
            || (q5 - q0).abs() > t_bd
            || (p4 - p0).abs() > t_bd
            || (q4 - q0).abs() > t_bd)
    } else {
        false
    };
    if filter_size == 4 || !flat {
        // narrow filter (7.14.6.3)
        let half = 1i32 << (bd - 1);
        let c = |v: i32| v.clamp(-half, half - 1);
        let off = 0x80 << sh;
        let (ps1, ps0, qs0, qs1) = (p1 - off, p0 - off, q0 - off, q1 - off);
        let mut filter = if hev { c(ps1 - qs1) } else { 0 };
        filter = c(filter + 3 * (qs0 - ps0));
        let filter1 = c(filter + 4) >> 3;
        let filter2 = c(filter + 3) >> 3;
        d[base as usize] = (c(qs0 - filter1) + off) as u16;
        d[(base - step) as usize] = (c(ps0 + filter2) + off) as u16;
        if !hev {
            let f = (filter1 + 1) >> 1;
            d[(base + step) as usize] = (c(qs1 - f) + off) as u16;
            d[(base - 2 * step) as usize] = (c(ps1 + f) + off) as u16;
        }
    } else {
        // wide filter (7.14.6.4)
        let log2 = if filter_size == 8 || !flat2 { 3 } else { 4 };
        let n: isize = if log2 == 4 {
            6
        } else if plane == 0 {
            3
        } else {
            2
        };
        let n2: isize = if log2 == 3 && plane == 0 { 0 } else { 1 };
        let mut f = [0i32; 12];
        for i in -n..n {
            let mut t = 0;
            for j in -n..=n {
                let p = (i + j).clamp(-(n + 1), n);
                let tap = if j.abs() <= n2 { 2 } else { 1 };
                t += at(p) * tap;
            }
            f[(i + n) as usize] = (t + (1 << (log2 - 1))) >> log2;
        }
        for i in -n..n {
            d[(base + i * step) as usize] = f[(i + n) as usize] as u16;
        }
    }
}
