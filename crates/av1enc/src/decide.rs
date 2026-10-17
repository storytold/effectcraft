//! Mode decision and reconstruction (not normative): partitions, intra modes, motion vectors,
//! transform coefficients. Every prediction and reconstruction step uses the normative
//! processes, so the reconstruction built here is exactly what a decoder produces; the syntax
//! is written afterwards by `writer.rs`.

use crate::pred::*;
use crate::tables::*;
use crate::transform::{forward_2d, inverse_2d};

/// Decisions for one coded block.
#[derive(Clone)]
pub(crate) struct Leaf {
    pub bsize: usize,
    pub is_inter: bool,
    pub y_mode: usize,
    pub uv_mode: usize,
    pub tx_type_y: usize,
    /// Motion vector [row, col] in 1/8 luma samples (inter blocks).
    pub mv: [i32; 2],
    pub skip: bool,
    /// Quantised coefficients of the single transform block of each plane (raster order).
    pub coefs: [Vec<i32>; 3],
}

pub(crate) enum Node {
    Out,
    Leaf(Box<Leaf>),
    Split(Box<[Node; 4]>),
}

/// A reconstructed frame kept as a reference.
pub(crate) struct RefFrame {
    pub planes: [Plane; 3],
    /// Motion vectors chosen for the frame (per 4x4), used as search candidates.
    pub mvs: Vec<[i32; 2]>,
    /// The frame's loop filter level.
    pub lf_level: u32,
    /// The CDFs saved at the end of the frame (loaded by the next frame).
    pub cdfs: Box<crate::cdf::Cdfs>,
}

pub(crate) struct FrameGeom {
    pub width: usize,
    pub height: usize,
    pub mi_cols: usize,
    pub mi_rows: usize,
    pub sb_cols: usize,
    pub sb_rows: usize,
    pub tiles: TileLayout,
}

/// Uniformly spaced tiles (tile_info( ) with uniform_tile_spacing_flag = 1), chosen from the
/// frame size alone so that the bitstream never depends on the machine or the thread count.
pub(crate) struct TileLayout {
    /// TileColsLog2 / TileRowsLog2.
    pub cols_log2: usize,
    pub rows_log2: usize,
    /// minLog2TileCols, maxLog2TileCols, minLog2TileRows (for this cols_log2), maxLog2TileRows.
    pub min_cols_log2: usize,
    pub max_cols_log2: usize,
    pub min_rows_log2: usize,
    pub max_rows_log2: usize,
    /// Tile width and height in superblocks (the last column / row may be smaller).
    pub w_sb: usize,
    pub h_sb: usize,
    /// TileCols, TileRows.
    pub cols: usize,
    pub rows: usize,
}

/// Tiles are split into columns while each column keeps at least this many superblocks
/// (1024 samples: 1080p stays one tile, 3840x2160 gets two columns, 7680x4320 four). Each tile restarts
/// its CDF adaptation and loses the neighbours across its borders; two columns at 1080p cost
/// about 0.6 % in size for little speed, since decisions are already parallel.
const MIN_TILE_WIDTH_SB: usize = 16;
/// MAX_TILE_WIDTH and MAX_TILE_AREA in 64x64 superblocks, MAX_TILE_COLS / MAX_TILE_ROWS.
const MAX_TILE_WIDTH_SB: usize = 4096 >> 6;
const MAX_TILE_AREA_SB: usize = (4096 * 2304) >> 12;
const MAX_TILE_COLS: usize = 64;
const MAX_TILE_ROWS: usize = 64;

/// Superblocks per tile along a side of `n` superblocks split into `1 << log2` tiles.
fn span(n: usize, log2: usize) -> usize {
    n.div_ceil(1 << log2).max(1)
}

/// tile_log2( blkSize, target ): the smallest k with blkSize << k >= target.
pub(crate) fn tile_log2(blk: usize, target: usize) -> usize {
    let mut k = 0;
    while blk.checked_shl(k as u32).is_some_and(|v| v < target) {
        k += 1;
    }
    k
}

impl TileLayout {
    fn new(sb_cols: usize, sb_rows: usize) -> TileLayout {
        let min_cols_log2 = tile_log2(MAX_TILE_WIDTH_SB, sb_cols);
        let max_cols_log2 = tile_log2(1, sb_cols.min(MAX_TILE_COLS));
        let mut cols_log2 = min_cols_log2;
        while cols_log2 < max_cols_log2 && span(sb_cols, cols_log2 + 1) >= MIN_TILE_WIDTH_SB {
            cols_log2 += 1;
        }
        TileLayout::with_log2(sb_cols, sb_rows, cols_log2, 0)
    }

    /// A layout with at least `cols_log2` / `rows_log2` (raised to the minimum the frame size
    /// requires and capped at the maximum it allows).
    pub fn with_log2(sb_cols: usize, sb_rows: usize, cols_log2: usize, rows_log2: usize) -> TileLayout {
        let min_cols_log2 = tile_log2(MAX_TILE_WIDTH_SB, sb_cols);
        let max_cols_log2 = tile_log2(1, sb_cols.min(MAX_TILE_COLS));
        let max_rows_log2 = tile_log2(1, sb_rows.min(MAX_TILE_ROWS));
        let min_tiles_log2 = min_cols_log2.max(tile_log2(MAX_TILE_AREA_SB, sb_rows * sb_cols));
        let cols_log2 = cols_log2.clamp(min_cols_log2, max_cols_log2.max(min_cols_log2));
        let min_rows_log2 = min_tiles_log2.saturating_sub(cols_log2);
        let mut rows_log2 = rows_log2.clamp(min_rows_log2, max_rows_log2.max(min_rows_log2));
        // the rounded-up tile size may still exceed MAX_TILE_AREA
        while rows_log2 < max_rows_log2 && span(sb_cols, cols_log2) * span(sb_rows, rows_log2) > MAX_TILE_AREA_SB {
            rows_log2 += 1;
        }
        let (w_sb, h_sb) = (span(sb_cols, cols_log2), span(sb_rows, rows_log2));
        TileLayout {
            cols_log2,
            rows_log2,
            min_cols_log2,
            max_cols_log2,
            min_rows_log2,
            max_rows_log2,
            w_sb,
            h_sb,
            cols: sb_cols.div_ceil(w_sb).max(1),
            rows: sb_rows.div_ceil(h_sb).max(1),
        }
    }

    pub fn count(&self) -> usize {
        self.cols * self.rows
    }
}

/// The 4x4 rows r0..r1 and columns c0..c1 of a tile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TileRect {
    pub r0: usize,
    pub r1: usize,
    pub c0: usize,
    pub c1: usize,
}

impl FrameGeom {
    pub fn new(width: usize, height: usize) -> FrameGeom {
        let mi_cols = 2 * width.div_ceil(8);
        let mi_rows = 2 * height.div_ceil(8);
        let (sb_cols, sb_rows) = (mi_cols.div_ceil(16), mi_rows.div_ceil(16));
        FrameGeom { width, height, mi_cols, mi_rows, sb_cols, sb_rows, tiles: TileLayout::new(sb_cols, sb_rows) }
    }
    /// The geometry with a given tile split (see `TileLayout::with_log2`).
    pub fn with_tiles(width: usize, height: usize, cols_log2: usize, rows_log2: usize) -> FrameGeom {
        let mut g = FrameGeom::new(width, height);
        g.tiles = TileLayout::with_log2(g.sb_cols, g.sb_rows, cols_log2, rows_log2);
        g
    }
    /// Tile `n` (raster order).
    pub fn tile(&self, n: usize) -> TileRect {
        let t = &self.tiles;
        let (row, col) = (n / t.cols.max(1), n % t.cols.max(1));
        let (r0, c0) = (row * t.h_sb * 16, col * t.w_sb * 16);
        TileRect { r0, r1: (r0 + t.h_sb * 16).min(self.mi_rows), c0, c1: (c0 + t.w_sb * 16).min(self.mi_cols) }
    }
    /// The tile holding the 4x4 unit (r, c).
    pub fn tile_at(&self, r: usize, c: usize) -> TileRect {
        let t = &self.tiles;
        let row = (r / 16 / t.h_sb).min(t.rows.saturating_sub(1));
        let col = (c / 16 / t.w_sb).min(t.cols.saturating_sub(1));
        self.tile(row * t.cols + col)
    }
    /// Padded plane size (whole superblocks).
    pub fn padded(&self, plane: usize) -> (usize, usize) {
        let s = (plane > 0) as usize;
        ((self.sb_cols * 64) >> s, (self.sb_rows * 64) >> s)
    }
    /// Visible plane size.
    pub fn visible(&self, plane: usize) -> (usize, usize) {
        if plane == 0 { (self.width, self.height) } else { (self.width.div_ceil(2), self.height.div_ceil(2)) }
    }
}

pub(crate) fn tx_size_for(log2: u32) -> usize {
    match log2 {
        2 => TX_4X4,
        3 => TX_8X8,
        4 => TX_16X16,
        5 => TX_32X32,
        _ => TX_64X64,
    }
}

/// get_tx_set( ) for the square sizes used here with reduced_tx_set = 1.
pub(crate) fn tx_set(tx_sz: usize, is_inter: bool) -> usize {
    let sqr_up = TX_SIZE_SQR_UP[tx_sz] as usize;
    if sqr_up > TX_32X32 {
        return TX_SET_DCTONLY;
    }
    if is_inter {
        TX_SET_INTER_3
    } else if sqr_up == TX_32X32 {
        TX_SET_DCTONLY
    } else {
        TX_SET_INTRA_2
    }
}

pub(crate) fn tx_in_set(set: usize, is_inter: bool, tx_type: usize) -> bool {
    if is_inter { TX_TYPE_IN_SET_INTER[set][tx_type] == 1 } else { TX_TYPE_IN_SET_INTRA[set][tx_type] == 1 }
}

/// The chroma transform type of an intra block (compute_tx_type for plane > 0).
pub(crate) fn intra_uv_tx_type(uv_mode: usize, uv_tx: usize) -> usize {
    let t = MODE_TO_TXFM[uv_mode] as usize;
    if tx_in_set(tx_set(uv_tx, false), false, t) { t } else { DCT_DCT }
}

struct TxResult {
    nonzero: bool,
    dist: f64,
    bits: f64,
}

/// Largest transform block (32x32) and largest prediction block (64x64) in samples.
const TX_MAX: usize = 32 * 32;
const BLK_MAX: usize = 64 * 64;

/// Grows `v` to at least `n` elements (buffers taken out of the decider come back empty when a
/// path forgot to return them; this keeps every later use in bounds).
fn ready<T: Copy + Default>(v: &mut Vec<T>, n: usize) {
    if v.len() < n {
        v.resize(n, T::default());
    }
}

/// Scratch buffers of one transform trial, reused so that a trial allocates nothing.
#[derive(Default)]
struct TxBufs {
    res: Vec<i32>,
    coef: Vec<f32>,
    deq: Vec<i32>,
    resid: Vec<i32>,
    /// Reconstruction of `code_tx`.
    out: Vec<u16>,
}

impl TxBufs {
    fn ready(&mut self) {
        ready(&mut self.res, TX_MAX);
        ready(&mut self.coef, TX_MAX);
        ready(&mut self.deq, TX_MAX);
        ready(&mut self.resid, TX_MAX);
        ready(&mut self.out, TX_MAX);
    }
}

/// Scratch buffers of one block decision (predictions, the current and the best luma trial,
/// the motion search block), reused across blocks.
#[derive(Default)]
struct LeafBufs {
    pred: Vec<u16>,
    best_pred: Vec<u16>,
    levels: [Vec<i32>; 2],
    recon: [Vec<u16>; 2],
    search: Vec<u16>,
}

impl LeafBufs {
    fn ready(&mut self) {
        ready(&mut self.pred, BLK_MAX);
        ready(&mut self.best_pred, TX_MAX);
        for i in 0..2 {
            ready(&mut self.levels[i], TX_MAX);
            ready(&mut self.recon[i], TX_MAX);
        }
        ready(&mut self.search, BLK_MAX);
    }
}

/// Saved encoder state of a block area (reconstruction, BlockDecoded flags).
struct Snapshot {
    planes: [Vec<u16>; 3],
    decoded: Box<[[[bool; 35]; 35]; 3]>,
}

/// Decides one superblock row. The row keeps its own strip of the reconstruction and of the
/// decision motion vectors: local row 0 holds the last row of the strip above (copied in by
/// `load_above` before each superblock), local rows 1.. are the row's own. Decisions only read
/// that one row above, up to the superblock above-right, so rows can be decided in a wavefront
/// (each row two superblocks behind the one above) with exactly the serial result.
pub(crate) struct Decider<'a> {
    g: &'a FrameGeom,
    bd: u32,
    intra_only: bool,
    src: &'a [Plane; 3],
    sb_row: usize,
    /// The tile of the superblock being decided (intra edges stop at its borders).
    tile: TileRect,
    /// Reconstruction strip per plane ((64 >> ss) + 1 rows).
    rec: [Plane; 3],
    refr: Option<&'a RefFrame>,
    decoded: Box<[[[bool; 35]; 35]; 3]>,
    /// Decision motion vectors per 4x4 (search candidates), 17 rows of the strip.
    mvs: Vec<[i32; 2]>,
    dc_q: f32,
    ac_q: f32,
    dc_qi: i64,
    ac_qi: i64,
    lambda: f64,
    lambda_sad: f64,
    /// Half-sample interpolations of the reference luma (visible area): (8, 0), (0, 8), (8, 8).
    half: &'a [Vec<u16>; 3],
    tx: TxBufs,
    leaf_bufs: LeafBufs,
    /// Released snapshots, reused by `snapshot`.
    snaps: Vec<Snapshot>,
}

const MODE_BITS_INTRA: f64 = 3.0;

/// The superblock column that row `sbr` decides in wavefront step `t` (rows trail the row above
/// by two superblocks).
fn wave_col(t: usize, sbr: usize, sb_cols: usize) -> Option<usize> {
    let c = t.checked_sub(2 * sbr)?;
    (c < sb_cols).then_some(c)
}

/// The decisions of a frame: superblock trees in raster order, the reconstruction (padded
/// planes, before the loop filter) and the decision motion vectors per 4x4. Superblock rows run
/// in parallel as a wavefront; the result is identical to deciding the superblocks one after
/// another in raster order, whatever the thread count.
pub(crate) fn decide_frame(
    g: &FrameGeom,
    bd: u32,
    qidx: u8,
    intra_only: bool,
    src: &[Plane; 3],
    refr: Option<&RefFrame>,
) -> (Vec<Node>, [Plane; 3], Vec<[i32; 2]>) {
    use rayon::prelude::*;
    let half: [Vec<u16>; 3] = match refr {
        Some(rf) => {
            let sub = |(fx, fy): (usize, usize)| subpel_plane(&rf.planes[0], g.width, g.height, fx, fy, bd);
            let (h0, (h1, h2)) = rayon::join(|| sub((8, 0)), || rayon::join(|| sub((0, 8)), || sub((8, 8))));
            [h0, h1, h2]
        }
        None => Default::default(),
    };
    let mut rows: Vec<Decider> = (0..g.sb_rows).map(|r| Decider::new(g, bd, qidx, intra_only, src, refr, &half, r)).collect();
    let mut nodes: Vec<Node> = (0..g.sb_rows * g.sb_cols).map(|_| Node::Out).collect();
    let steps = g.sb_cols + 2 * g.sb_rows.saturating_sub(1);
    for t in 0..steps {
        for r in 1..rows.len() {
            if let Some(c) = wave_col(t, r, g.sb_cols)
                && let (above, [cur, ..]) = rows.split_at_mut(r)
                && let Some(prev) = above.last()
            {
                cur.load_above(prev, c);
            }
        }
        let done: Vec<(usize, Node)> =
            rows.par_iter_mut().enumerate().filter_map(|(r, d)| wave_col(t, r, g.sb_cols).map(|c| (r * g.sb_cols + c, d.superblock(c)))).collect();
        for (i, node) in done {
            if let Some(slot) = nodes.get_mut(i) {
                *slot = node;
            }
        }
    }
    let mut rec = std::array::from_fn(|p| {
        let (w, h) = g.padded(p);
        Plane::new(w, h)
    });
    let mut mvs = vec![[0, 0]; g.mi_cols * g.mi_rows];
    for d in &rows {
        d.store(&mut rec, &mut mvs);
    }
    (nodes, rec, mvs)
}

impl<'a> Decider<'a> {
    #[allow(clippy::too_many_arguments)]
    fn new(
        g: &'a FrameGeom,
        bd: u32,
        qidx: u8,
        intra_only: bool,
        src: &'a [Plane; 3],
        refr: Option<&'a RefFrame>,
        half: &'a [Vec<u16>; 3],
        sb_row: usize,
    ) -> Self {
        let bdi = ((bd - 8) >> 1) as usize;
        let dc_qi = DC_QLOOKUP[bdi][qidx as usize] as i64;
        let ac_qi = AC_QLOOKUP[bdi][qidx as usize] as i64;
        // Lagrangian multiplier in squared-sample units of the coded bit depth.
        let step = ac_qi as f64 / 8.0;
        let lambda = 0.25 * step * step;
        let rec = std::array::from_fn(|p| {
            let (w, _) = g.padded(p);
            Plane::new(w, (64 >> (p > 0) as usize) + 1)
        });
        Decider {
            g,
            bd,
            intra_only,
            src,
            sb_row,
            tile: g.tile_at(sb_row * 16, 0),
            rec,
            refr,
            decoded: Box::new([[[false; 35]; 35]; 3]),
            mvs: vec![[0, 0]; g.mi_cols * 17],
            dc_q: dc_qi as f32,
            ac_q: ac_qi as f32,
            dc_qi,
            ac_qi,
            lambda,
            lambda_sad: lambda.sqrt(),
            half,
            tx: TxBufs::default(),
            leaf_bufs: LeafBufs::default(),
            snaps: Vec::new(),
        }
    }

    /// Strip row of the sample row `y` (frame coordinates) of plane `p`.
    #[inline(always)]
    fn ry(&self, p: usize, y: usize) -> usize {
        (y + 1).saturating_sub((self.sb_row * 64) >> (p > 0) as usize)
    }

    /// Strip index of the motion vector of 4x4 (r, c).
    #[inline(always)]
    fn mv_idx(&self, r: usize, c: usize) -> usize {
        (r + 1).saturating_sub(self.sb_row * 16) * self.g.mi_cols + c
    }

    /// Copies what superblock `sbc` of this row reads from the row above (its last
    /// reconstructed row and motion vectors, from superblock `sbc - 1` to `sbc + 1`).
    fn load_above(&mut self, above: &Decider, sbc: usize) {
        for p in 0..3 {
            let s = (p > 0) as usize;
            let (w, _) = self.g.padded(p);
            let x0 = (sbc.saturating_sub(1) * 64) >> s;
            let x1 = (((sbc + 2) * 64) >> s).min(w);
            let last = 64 >> s;
            let (src, dst) = (&above.rec[p], &mut self.rec[p]);
            if let (Some(a), Some(d)) = (src.data.get(last * src.stride + x0..last * src.stride + x1), dst.data.get_mut(x0..x1)) {
                d.copy_from_slice(a);
            }
        }
        let cols = self.g.mi_cols;
        let (c0, c1) = (sbc.saturating_sub(1) * 16, ((sbc + 2) * 16).min(cols));
        if let (Some(a), Some(d)) = (above.mvs.get(16 * cols + c0..16 * cols + c1), self.mvs.get_mut(c0..c1)) {
            d.copy_from_slice(a);
        }
    }

    /// Copies the row's reconstruction and motion vectors into the frame.
    fn store(&self, rec: &mut [Plane; 3], mvs: &mut [[i32; 2]]) {
        for p in 0..3 {
            let s = (p > 0) as usize;
            let h = 64 >> s;
            let y0 = self.sb_row * h;
            let (src, dst) = (&self.rec[p], &mut rec[p]);
            let n = src.stride.min(dst.stride);
            for i in 0..h {
                if let (Some(a), Some(d)) =
                    (src.data.get((i + 1) * src.stride..(i + 1) * src.stride + n), dst.data.get_mut((y0 + i) * dst.stride..(y0 + i) * dst.stride + n))
                {
                    d.copy_from_slice(a);
                }
            }
        }
        let cols = self.g.mi_cols;
        let r0 = self.sb_row * 16;
        let rows = 16.min(self.g.mi_rows.saturating_sub(r0));
        if let (Some(a), Some(d)) = (self.mvs.get(cols..(rows + 1) * cols), mvs.get_mut(r0 * cols..(r0 + rows) * cols)) {
            d.copy_from_slice(a);
        }
    }

    /// Decides superblock `sbc` of the row (reconstruction is written to `rec`).
    fn superblock(&mut self, sbc: usize) -> Node {
        let (r, c) = (self.sb_row * 16, sbc * 16);
        self.tile = self.g.tile_at(r, c);
        // clear_block_decoded_flags( r, c, sbSize4 )
        for plane in 0..3 {
            let s = (plane > 0) as usize;
            let sb_w4 = (self.tile.c1 as isize - c as isize) >> s;
            let sb_h4 = (self.tile.r1 as isize - r as isize) >> s;
            let bd = &mut self.decoded[plane];
            for y in -1..=((16 >> s) as isize) {
                for x in -1..=((16 >> s) as isize) {
                    let v = if y < 0 && x < sb_w4 { true } else { x < 0 && y < sb_h4 };
                    bd[(y + 1) as usize][(x + 1) as usize] = v;
                }
            }
            bd[(16 >> s) + 1][0] = false;
        }
        self.partition(r, c, BLOCK_64X64, None).0
    }

    fn decoded_at(&self, plane: usize, y: isize, x: isize) -> bool {
        let (yy, xx) = (y + 1, x + 1);
        if yy < 0 || xx < 0 || yy > 34 || xx > 34 {
            return false;
        }
        self.decoded[plane][yy as usize][xx as usize]
    }

    /// Marks the transform block of `plane` covering the block at (r, c) as decoded.
    fn mark_decoded(&mut self, plane: usize, r: usize, c: usize, n4: usize) {
        let s = (plane > 0) as usize;
        let by = (r & 15) >> s;
        let bx = (c & 15) >> s;
        let step = (n4 >> s).max(1);
        for i in 0..step {
            for j in 0..step {
                if by + i + 1 < 35 && bx + j + 1 < 35 {
                    self.decoded[plane][by + i + 1][bx + j + 1] = true;
                }
            }
        }
    }

    /// Saves the block area (a released snapshot's buffers are reused).
    fn snapshot(&mut self, r: usize, c: usize, n4: usize) -> Snapshot {
        let mut snap = self.snaps.pop().unwrap_or_else(|| Snapshot { planes: Default::default(), decoded: Box::new([[[false; 35]; 35]; 3]) });
        for (p, v) in snap.planes.iter_mut().enumerate() {
            let s = (p > 0) as usize;
            let (x0, y0, n) = ((c * 4) >> s, self.ry(p, (r * 4) >> s), (n4 * 4) >> s);
            let pl = &self.rec[p];
            v.clear();
            for y in y0..y0 + n {
                v.extend_from_slice(&pl.data[y * pl.stride + x0..y * pl.stride + x0 + n]);
            }
        }
        *snap.decoded = *self.decoded;
        snap
    }

    fn release(&mut self, snap: Snapshot) {
        self.snaps.push(snap);
    }

    fn restore(&mut self, snap: &Snapshot, r: usize, c: usize, n4: usize) {
        for p in 0..3 {
            let s = (p > 0) as usize;
            let (x0, y0, n) = ((c * 4) >> s, self.ry(p, (r * 4) >> s), (n4 * 4) >> s);
            let pl = &mut self.rec[p];
            for (i, y) in (y0..y0 + n).enumerate() {
                pl.data[y * pl.stride + x0..y * pl.stride + x0 + n].copy_from_slice(&snap.planes[p][i * n..i * n + n]);
            }
        }
        self.decoded.clone_from(&snap.decoded);
    }

    fn set_mvs(&mut self, r: usize, c: usize, n4: usize, mv: [i32; 2]) {
        for y in r..(r + n4).min(self.g.mi_rows) {
            for x in c..(c + n4).min(self.g.mi_cols) {
                let i = self.mv_idx(y, x);
                self.mvs[i] = mv;
            }
        }
    }

    fn partition(&mut self, r: usize, c: usize, bsize: usize, parent_mv: Option<[i32; 2]>) -> (Node, f64) {
        let g = self.g;
        if r >= g.mi_rows || c >= g.mi_cols {
            return (Node::Out, 0.0);
        }
        let n4 = NUM_4X4_BLOCKS_WIDE[bsize] as usize;
        let half = n4 >> 1;
        let has_rows = r + half < g.mi_rows;
        let has_cols = c + half < g.mi_cols;
        let can_none = has_rows && has_cols && (bsize != BLOCK_64X64 || !self.intra_only);
        let can_split = bsize > BLOCK_8X8;
        let sub = PARTITION_SUBSIZE[PARTITION_SPLIT][bsize] as usize;
        if !can_none {
            return self.split(r, c, sub, half, parent_mv);
        }
        let before = self.snapshot(r, c, n4);
        let (leaf, cost_none) = self.leaf(r, c, bsize, parent_mv);
        let cost_none = cost_none + self.lambda * 1.0;
        // a cheap skip block rarely gains from splitting
        if !can_split || (leaf.skip && cost_none < self.lambda * 8.0) {
            self.release(before);
            return (Node::Leaf(Box::new(leaf)), cost_none);
        }
        let after_none = self.snapshot(r, c, n4);
        self.restore(&before, r, c, n4);
        self.release(before);
        let pm = if leaf.is_inter { Some(leaf.mv) } else { parent_mv };
        let (node, cost_split) = self.split(r, c, sub, half, pm);
        let cost_split = cost_split + self.lambda * 2.0;
        let out = if cost_split < cost_none {
            (node, cost_split)
        } else {
            self.restore(&after_none, r, c, n4);
            self.set_mvs(r, c, n4, if leaf.is_inter { leaf.mv } else { [0, 0] });
            (Node::Leaf(Box::new(leaf)), cost_none)
        };
        self.release(after_none);
        out
    }

    fn split(&mut self, r: usize, c: usize, sub: usize, half: usize, pm: Option<[i32; 2]>) -> (Node, f64) {
        let (n0, c0) = self.partition(r, c, sub, pm);
        let (n1, c1) = self.partition(r, c + half, sub, pm);
        let (n2, c2) = self.partition(r + half, c, sub, pm);
        let (n3, c3) = self.partition(r + half, c + half, sub, pm);
        (Node::Split(Box::new([n0, n1, n2, n3])), c0 + c1 + c2 + c3)
    }

    /// Best coding of the block as a single (NONE partition) block.
    fn leaf(&mut self, r: usize, c: usize, bsize: usize, parent_mv: Option<[i32; 2]>) -> (Leaf, f64) {
        let n4 = NUM_4X4_BLOCKS_WIDE[bsize] as usize;
        if self.intra_only {
            let res = self.intra_leaf(r, c, bsize);
            self.set_mvs(r, c, n4, [0, 0]);
            return res;
        }
        let before = self.snapshot(r, c, n4);
        let (inter, cost_inter) = self.inter_leaf(r, c, bsize, parent_mv);
        if bsize == BLOCK_64X64 || inter.skip {
            self.release(before);
            self.set_mvs(r, c, n4, inter.mv);
            return (inter, cost_inter);
        }
        let after_inter = self.snapshot(r, c, n4);
        self.restore(&before, r, c, n4);
        self.release(before);
        let (intra, cost_intra) = self.intra_leaf(r, c, bsize);
        let out = if cost_intra < cost_inter {
            self.set_mvs(r, c, n4, [0, 0]);
            (intra, cost_intra)
        } else {
            self.restore(&after_inter, r, c, n4);
            self.set_mvs(r, c, n4, inter.mv);
            (inter, cost_inter)
        };
        self.release(after_inter);
        out
    }

    fn intra_leaf(&mut self, r: usize, c: usize, bsize: usize) -> (Leaf, f64) {
        let mut lb = std::mem::take(&mut self.leaf_bufs);
        lb.ready();
        let out = self.intra_leaf_with(&mut lb, r, c, bsize);
        self.leaf_bufs = lb;
        out
    }

    fn intra_leaf_with(&mut self, lb: &mut LeafBufs, r: usize, c: usize, bsize: usize) -> (Leaf, f64) {
        let n4 = NUM_4X4_BLOCKS_WIDE[bsize] as usize;
        let log2 = (n4 * 4).trailing_zeros();
        let (sbr, sbc) = ((r & 15) as isize, (c & 15) as isize);
        let bd = self.bd;
        let n = 1usize << log2;
        // luma
        // edges are read in strip coordinates: y and max_y shift by the strip origin
        let y_org = self.sb_row as i32 * 64 - 1;
        let e = IntraEdge {
            x: c * 4,
            y: self.ry(0, r * 4),
            have_left: c > self.tile.c0,
            have_above: r > self.tile.r0,
            have_above_right: self.decoded_at(0, sbr - 1, sbc + n4 as isize),
            have_below_left: self.decoded_at(0, sbr + n4 as isize, sbc - 1),
            log2,
            max_x: (self.g.mi_cols * 4) as i32 - 1,
            max_y: (self.g.mi_rows * 4) as i32 - 1 - y_org,
            bit_depth: bd,
        };
        let ed = edges(&self.rec[0], &e);
        // modes ranked by SATD, then a rate-distortion choice of mode and transform type among
        // the best three
        let mut ranked: [(f64, usize); INTRA_MODES_USED.len()] = INTRA_MODES_USED.map(|mode| {
            predict_intra(&ed, &e, mode, &mut lb.pred);
            let sad = self.satd(0, c * 4, r * 4, n, &lb.pred[..n * n]) as f64;
            (sad + self.lambda_sad * if mode == DC_PRED { 1.0 } else { MODE_BITS_INTRA }, mode)
        });
        ranked.sort_by(|a, b| a.0.total_cmp(&b.0));
        let tx_y = tx_size_for(log2);
        let mut tb = std::mem::take(&mut self.tx);
        tb.ready();
        // trials alternate between two level / reconstruction buffers: `cur` is written, the
        // other one holds the best trial so far
        let mut cur = 0;
        let mut best: Option<(f64, usize, usize, TxResult)> = None;
        for &(_, mode) in ranked.iter().take(3) {
            predict_intra(&ed, &e, mode, &mut lb.pred);
            let alt = MODE_TO_TXFM[mode] as usize;
            let types = [DCT_DCT, alt];
            let n_types = if alt != DCT_DCT && tx_in_set(tx_set(tx_y, false), false, alt) { 2 } else { 1 };
            for &t in &types[..n_types] {
                let [l0, l1] = &mut lb.levels;
                let [r0, r1] = &mut lb.recon;
                let (levels, recon) = if cur == 0 { (l0, r0) } else { (l1, r1) };
                let res = self.try_tx(&mut tb, 0, c * 4, r * 4, log2, t, &lb.pred[..n * n], true, &mut levels[..n * n], &mut recon[..n * n]);
                let type_bits = if !res.nonzero || tx_set(tx_y, false) == TX_SET_DCTONLY {
                    0.0
                } else if t == DCT_DCT {
                    1.0
                } else {
                    2.0
                };
                let mode_bits = if mode == DC_PRED { 1.0 } else { MODE_BITS_INTRA };
                let cost = res.dist + self.lambda * (res.bits + type_bits + mode_bits);
                if best.as_ref().is_none_or(|b| cost < b.0) {
                    lb.best_pred[..n * n].copy_from_slice(&lb.pred[..n * n]);
                    best = Some((cost, mode, t, res));
                    cur ^= 1;
                }
            }
        }
        self.tx = tb;
        // Every ranked mode tries DCT_DCT, so `best` is always set; without a candidate the block
        // can't be coded intra (an infinite cost keeps it from being chosen).
        let Some((_, y_mode, tx_type_y, ty)) = best else {
            let leaf =
                Leaf { bsize, is_inter: false, y_mode: DC_PRED, uv_mode: DC_PRED, tx_type_y: DCT_DCT, mv: [0, 0], skip: true, coefs: Default::default() };
            return (leaf, f64::INFINITY);
        };
        let bi = cur ^ 1;
        self.put(0, c * 4, r * 4, n, if ty.nonzero { &lb.recon[bi][..n * n] } else { &lb.best_pred[..n * n] });
        let y_levels = lb.levels[bi][..n * n].to_vec();
        let tx_type_y = if ty.nonzero { tx_type_y } else { DCT_DCT };
        self.mark_decoded(0, r, c, n4);
        // chroma (one mode for both planes)
        let cn = n >> 1;
        let clog2 = log2 - 1;
        let (csbr, csbc) = (sbr >> 1, sbc >> 1);
        let cy_org = self.sb_row as i32 * 32 - 1;
        let ce: [IntraEdge; 2] = std::array::from_fn(|i| IntraEdge {
            x: c * 2,
            y: self.ry(1, r * 2),
            have_left: c > self.tile.c0,
            have_above: r > self.tile.r0,
            have_above_right: self.decoded_at(1 + i, csbr - 1, csbc + (n4 >> 1) as isize),
            have_below_left: self.decoded_at(1 + i, csbr + (n4 >> 1) as isize, csbc - 1),
            log2: clog2,
            max_x: (self.g.mi_cols * 2) as i32 - 1,
            max_y: (self.g.mi_rows * 2) as i32 - 1 - cy_org,
            bit_depth: bd,
        });
        let ced = [edges(&self.rec[1], &ce[0]), edges(&self.rec[2], &ce[1])];
        let mut best_uv = (f64::MAX, DC_PRED);
        for &mode in INTRA_MODES_USED.iter() {
            let mut sad = 0u64;
            for i in 0..2 {
                predict_intra(&ced[i], &ce[i], mode, &mut lb.pred);
                sad += self.satd(1 + i, c * 2, r * 2, cn, &lb.pred[..cn * cn]);
            }
            let cost = sad as f64 + self.lambda_sad * if mode == DC_PRED { 1.0 } else { 2.5 };
            if cost < best_uv.0 {
                best_uv = (cost, mode);
            }
        }
        let uv_mode = best_uv.1;
        let uv_tx = tx_size_for(clog2);
        let uv_type = intra_uv_tx_type(uv_mode, uv_tx);
        let mut uv: [(TxResult, Vec<i32>); 2] = std::array::from_fn(|_| (TxResult { nonzero: false, dist: 0.0, bits: 0.0 }, Vec::new()));
        for (i, slot) in uv.iter_mut().enumerate() {
            predict_intra(&ced[i], &ce[i], uv_mode, &mut lb.pred);
            *slot = self.code_tx(1 + i, c * 2, r * 2, clog2, uv_type, &lb.pred[..cn * cn], true);
            self.mark_decoded(1 + i, r, c, n4);
        }
        let [(u, u_levels), (v, v_levels)] = uv;
        let skip = !ty.nonzero && !u.nonzero && !v.nonzero;
        let dist = ty.dist + u.dist + v.dist;
        let bits = if skip { 1.0 } else { ty.bits + u.bits + v.bits + 1.0 } + MODE_BITS_INTRA + 2.0;
        let leaf = Leaf { bsize, is_inter: false, y_mode, uv_mode, tx_type_y, mv: [0, 0], skip, coefs: [y_levels, u_levels, v_levels] };
        (leaf, dist + self.lambda * bits)
    }

    fn inter_leaf(&mut self, r: usize, c: usize, bsize: usize, parent_mv: Option<[i32; 2]>) -> (Leaf, f64) {
        let n4 = NUM_4X4_BLOCKS_WIDE[bsize] as usize;
        let n = n4 * 4;
        let log2 = n.trailing_zeros();
        // Inter blocks are only decided with a reference frame; without one, code intra.
        let Some(refr) = self.refr else { return self.intra_leaf(r, c, bsize) };
        let mut lb = std::mem::take(&mut self.leaf_bufs);
        lb.ready();
        let (mv, mv_bits) = self.motion_search(r, c, n, parent_mv, &mut lb.search);
        let mut tx: [Option<(TxResult, Vec<i32>)>; 3] = [None, None, None];
        let mut dist = 0.0;
        for p in 0..3 {
            let s = (p > 0) as usize;
            let (vw, vh) = self.g.visible(p);
            let pn = n >> s;
            let (x, y) = ((c * 4) >> s, (r * 4) >> s);
            let pred = &mut lb.pred[..pn * pn];
            predict_inter(&refr.planes[p], vw as i32 - 1, vh as i32 - 1, x, y, pn, pn, mv, s, self.bd, pred);
            if bsize == BLOCK_64X64 {
                // coded as a skip block: the prediction is the reconstruction
                dist += self.sse(p, x, y, pn, pred);
                self.put(p, x, y, pn, pred);
            } else {
                let t = self.code_tx(p, x, y, log2 - s as u32, DCT_DCT, pred, false);
                dist += t.0.dist;
                tx[p] = Some(t);
            }
            self.mark_decoded(p, r, c, n4);
        }
        self.leaf_bufs = lb;
        let mode_bits = if mv == [0, 0] { 1.5 } else { 2.0 + mv_bits };
        let skip_leaf = || Leaf { bsize, is_inter: true, y_mode: NEWMV, uv_mode: DC_PRED, tx_type_y: DCT_DCT, mv, skip: true, coefs: Default::default() };
        if bsize == BLOCK_64X64 {
            return (skip_leaf(), dist + self.lambda * (mode_bits + 1.0));
        }
        // One transform block per plane was coded above.
        let [Some((y, y_levels)), Some((u, u_levels)), Some((v, v_levels))] = tx else { return (skip_leaf(), f64::INFINITY) };
        let skip = !y.nonzero && !u.nonzero && !v.nonzero;
        let bits = mode_bits + if skip { 1.0 } else { 1.0 + [y.bits, u.bits, v.bits].iter().sum::<f64>() };
        let leaf = Leaf { bsize, is_inter: true, y_mode: NEWMV, uv_mode: DC_PRED, tx_type_y: DCT_DCT, mv, skip, coefs: [y_levels, u_levels, v_levels] };
        (leaf, dist + self.lambda * bits)
    }

    /// Full-sample motion search: candidate vectors then a shrinking diamond search.
    /// Returns the vector and an estimate of its coding cost in bits.
    fn motion_search(&self, r: usize, c: usize, n: usize, parent_mv: Option<[i32; 2]>, buf: &mut [u16]) -> ([i32; 2], f64) {
        let g = self.g;
        let Some(refr) = self.refr else { return ([0, 0], 0.0) };
        let mi = |rr: usize, cc: usize| self.mvs[self.mv_idx(rr, cc)];
        let mut pred_mv = [0, 0];
        // at most six candidates: zero, left, above, above-right, parent, co-located
        let mut cand_buf = [[0i32; 2]; 6];
        let mut n_cands = 1;
        let mut push = |m: [i32; 2]| {
            if let Some(slot) = cand_buf.get_mut(n_cands) {
                *slot = m;
                n_cands += 1;
            }
        };
        if c > 0 {
            pred_mv = mi(r, c - 1);
            push(pred_mv);
        }
        if r > 0 {
            push(mi(r - 1, c));
            let cr = c + n / 4;
            if cr < g.mi_cols {
                push(mi(r - 1, cr));
            }
        }
        if let Some(p) = parent_mv {
            push(p);
        }
        push(refr.mvs[r * g.mi_cols + c]);
        let cands = &cand_buf[..n_cands];
        let (x0, y0) = ((c * 4) as i32, (r * 4) as i32);
        let (w, h) = (g.width as i32, g.height as i32);
        // keep the referenced block within 64 samples of the picture
        let lim_x = (-x0 - n as i32 - 64, w - x0 + 64);
        let lim_y = (-y0 - n as i32 - 64, h - y0 + 64);
        let valid = |m: [i32; 2]| {
            let (dy, dx) = (m[0] >> 3, m[1] >> 3);
            dx >= lim_x.0 && dx <= lim_x.1 && dy >= lim_y.0 && dy <= lim_y.1
        };
        let mv_bits = |m: [i32; 2]| -> f64 {
            let comp = |d: i32| if d == 0 { 0.5 } else { 2.0 + 2.0 * ((d.abs() / 8) as f64 + 1.0).log2() };
            comp(m[0] - pred_mv[0]) + comp(m[1] - pred_mv[1])
        };
        let cost =
            |m: [i32; 2]| -> f64 { self.sad_ref(&refr.planes[0], x0 + (m[1] >> 3), y0 + (m[0] >> 3), c * 4, r * 4, n) as f64 + self.lambda_sad * mv_bits(m) };
        let mut best = ([0, 0], f64::MAX);
        for &m in cands {
            let m = [m[0] & !7, m[1] & !7];
            if !valid(m) {
                continue;
            }
            let cst = cost(m);
            if cst < best.1 {
                best = (m, cst);
            }
        }
        let mut step = 8 * if n >= 32 {
            16
        } else if n == 16 {
            8
        } else {
            4
        };
        while step >= 8 {
            let mut moved = true;
            let mut iters = 0;
            while moved && iters < 16 {
                moved = false;
                iters += 1;
                let center = best.0;
                for d in [[-step, 0], [step, 0], [0, -step], [0, step]] {
                    let m = [center[0] + d[0], center[1] + d[1]];
                    if !valid(m) {
                        continue;
                    }
                    let cst = cost(m);
                    if cst < best.1 {
                        best = (m, cst);
                        moved = true;
                    }
                }
            }
            step >>= 1;
        }
        // half- then quarter-sample refinement (plus the exact neighbour candidates)
        let buf = &mut buf[..n * n];
        buf.fill(0);
        let sub_cost = |m: [i32; 2], buf: &mut [u16]| -> f64 {
            self.search_block(m, c * 4, r * 4, n, buf);
            self.sad(0, c * 4, r * 4, n, buf) as f64 + self.lambda_sad * mv_bits(m)
        };
        for &m in cands {
            if m[0] & 7 != 0 || m[1] & 7 != 0 {
                let m = [m[0] & !1, m[1] & !1];
                if valid(m) {
                    let cst = sub_cost(m, buf);
                    if cst < best.1 {
                        best = (m, cst);
                    }
                }
            }
        }
        for step in [4, 2] {
            let center = best.0;
            for d in [[-1, -1], [-1, 0], [-1, 1], [0, -1], [0, 1], [1, -1], [1, 0], [1, 1]] {
                let m = [center[0] + d[0] * step, center[1] + d[1] * step];
                if !valid(m) {
                    continue;
                }
                let cst = sub_cost(m, buf);
                if cst < best.1 {
                    best = (m, cst);
                }
            }
        }
        (best.0, mv_bits(best.0))
    }

    /// Approximate luma prediction for motion search: exact at whole and half-sample
    /// positions (precomputed planes), averaged neighbours at quarter-sample positions.
    fn search_block(&self, mv: [i32; 2], x: usize, y: usize, n: usize, out: &mut [u16]) {
        let Some(refr) = self.refr else { return };
        let (w, h) = (self.g.width as i32, self.g.height as i32);
        // per axis: up to two (integer offset, half) taps
        let axis = |v: i32| -> ([(i32, bool); 2], usize) {
            let p = 2 * v; // 1/16 units
            let (i, f) = (p >> 4, p & 15);
            match f {
                0 => ([(i, false), (i, false)], 1),
                8 => ([(i, true), (i, true)], 1),
                4 => ([(i, false), (i, true)], 2),
                _ => ([(i, true), (i + 1, false)], 2),
            }
        };
        let (ax, nx) = axis(mv[1]);
        let (ay, ny) = axis(mv[0]);
        let taps = nx.max(ny);
        let fetch = |k: usize, xx: i32, yy: i32| -> u16 {
            let (ox, hx) = ax[k.min(nx - 1)];
            let (oy, hy) = ay[k.min(ny - 1)];
            let sx = (xx + ox).clamp(0, w - 1) as usize;
            let sy = (yy + oy).clamp(0, h - 1) as usize;
            match (hx, hy) {
                (false, false) => refr.planes[0].at(sx, sy),
                (true, false) => self.half[0][sy * w as usize + sx],
                (false, true) => self.half[1][sy * w as usize + sx],
                (true, true) => self.half[2][sy * w as usize + sx],
            }
        };
        let vw = n.min(self.g.width.saturating_sub(x));
        let vh = n.min(self.g.height.saturating_sub(y));
        // fast path: every tap inside the picture
        let (x0, y0) = (x as i32, y as i32);
        let inside = (0..taps).all(|k| {
            let (ox, _) = ax[k.min(nx - 1)];
            let (oy, _) = ay[k.min(ny - 1)];
            x0 + ox >= 0 && y0 + oy >= 0 && x0 + ox + vw as i32 <= w && y0 + oy + vh as i32 <= h
        });
        if inside {
            let src = |k: usize| -> (&[u16], usize, usize) {
                let (ox, hx) = ax[k.min(nx - 1)];
                let (oy, hy) = ay[k.min(ny - 1)];
                let (sx, sy) = ((x0 + ox) as usize, (y0 + oy) as usize);
                match (hx, hy) {
                    (false, false) => (&refr.planes[0].data, refr.planes[0].stride, sy * refr.planes[0].stride + sx),
                    (true, false) => (&self.half[0], w as usize, sy * w as usize + sx),
                    (false, true) => (&self.half[1], w as usize, sy * w as usize + sx),
                    (true, true) => (&self.half[2], w as usize, sy * w as usize + sx),
                }
            };
            let (a, sa, oa) = src(0);
            if taps == 1 {
                for i in 0..vh {
                    out[i * n..i * n + vw].copy_from_slice(&a[oa + i * sa..oa + i * sa + vw]);
                }
            } else {
                let (b, sb, ob) = src(1);
                for i in 0..vh {
                    let (ra, rb) = (&a[oa + i * sa..oa + i * sa + vw], &b[ob + i * sb..ob + i * sb + vw]);
                    for (o, (&p, &q)) in out[i * n..i * n + vw].iter_mut().zip(ra.iter().zip(rb)) {
                        *o = ((p as u32 + q as u32 + 1) >> 1) as u16;
                    }
                }
            }
            return;
        }
        for i in 0..vh {
            for j in 0..vw {
                let (xx, yy) = (x0 + j as i32, y0 + i as i32);
                out[i * n + j] = if taps == 1 { fetch(0, xx, yy) } else { ((fetch(0, xx, yy) as u32 + fetch(1, xx, yy) as u32 + 1) >> 1) as u16 };
            }
        }
    }

    /// SAD of the source block at (x, y) (luma) against the reference block at (rx, ry).
    fn sad_ref(&self, refp: &Plane, rx: i32, ry: i32, x: usize, y: usize, n: usize) -> u64 {
        let (vw, vh) = (self.g.width, self.g.height);
        let w = n.min(vw.saturating_sub(x));
        let h = n.min(vh.saturating_sub(y));
        let src = &self.src[0];
        let (lx, ly) = (vw as i32 - 1, vh as i32 - 1);
        let mut sad = 0u64;
        let inside = rx >= 0 && ry >= 0 && rx + w as i32 - 1 <= lx && ry + h as i32 - 1 <= ly;
        for i in 0..h {
            let s = &src.row(y + i)[x..x + w];
            let rr = refp.row((ry + i as i32).clamp(0, ly) as usize);
            if inside {
                let rrow = &rr[rx as usize..rx as usize + w];
                sad += s.iter().zip(rrow).map(|(&a, &b)| (a as i32 - b as i32).unsigned_abs()).sum::<u32>() as u64;
            } else {
                for j in 0..w {
                    let b = rr[(rx + j as i32).clamp(0, lx) as usize];
                    sad += (s[j] as i32 - b as i32).unsigned_abs() as u64;
                }
            }
        }
        sad
    }

    /// Sum of absolute Hadamard-transformed differences (4x4 or 8x8 sub-blocks) of a prediction
    /// against the source, scaled to about the size of the SAD.
    fn satd(&self, p: usize, x: usize, y: usize, n: usize, pred: &[u16]) -> u64 {
        let src = &self.src[p];
        let k = n.min(8);
        let mut total = 0u64;
        let mut d = [0i32; 64];
        for by in (0..n).step_by(k) {
            for bx in (0..n).step_by(k) {
                for i in 0..k {
                    let row = &src.row(y + by + i)[x + bx..x + bx + k];
                    for j in 0..k {
                        d[i * k + j] = row[j] as i32 - pred[(by + i) * n + bx + j] as i32;
                    }
                }
                for i in 0..k {
                    hadamard(&mut d[i * k..i * k + k]);
                }
                let mut col = [0i32; 8];
                let mut s = 0u64;
                for j in 0..k {
                    for i in 0..k {
                        col[i] = d[i * k + j];
                    }
                    hadamard(&mut col[..k]);
                    s += col[..k].iter().map(|v| v.unsigned_abs() as u64).sum::<u64>();
                }
                total += s / (k as u64 / 2);
            }
        }
        total
    }

    /// SAD of a prediction against the source over the visible part of the block.
    fn sad(&self, p: usize, x: usize, y: usize, n: usize, pred: &[u16]) -> u64 {
        let (vw, vh) = self.g.visible(p);
        let w = n.min(vw.saturating_sub(x));
        let h = n.min(vh.saturating_sub(y));
        let src = &self.src[p];
        let mut s = 0u64;
        for i in 0..h {
            let row = &src.row(y + i)[x..x + w];
            s += row.iter().zip(&pred[i * n..i * n + w]).map(|(&a, &b)| (a as i32 - b as i32).unsigned_abs()).sum::<u32>() as u64;
        }
        s
    }

    fn sse(&self, p: usize, x: usize, y: usize, n: usize, blk: &[u16]) -> f64 {
        let (vw, vh) = self.g.visible(p);
        let w = n.min(vw.saturating_sub(x));
        let h = n.min(vh.saturating_sub(y));
        let src = &self.src[p];
        let mut s = 0u64;
        for i in 0..h {
            let row = &src.row(y + i)[x..x + w];
            s += row
                .iter()
                .zip(&blk[i * n..i * n + w])
                .map(|(&a, &b)| {
                    let d = a as i64 - b as i64;
                    (d * d) as u64
                })
                .sum::<u64>();
        }
        s as f64
    }

    fn put(&mut self, p: usize, x: usize, y: usize, n: usize, blk: &[u16]) {
        let y = self.ry(p, y);
        let pl = &mut self.rec[p];
        for i in 0..n {
            let o = (y + i) * pl.stride + x;
            pl.data[o..o + n].copy_from_slice(&blk[i * n..i * n + n]);
        }
    }

    /// Transform, quantise and reconstruct one square transform block of `plane` at (x, y)
    /// over the prediction `pred`; the reconstruction is written to `rec`. Returns the result
    /// and the quantised levels.
    fn code_tx(&mut self, p: usize, x: usize, y: usize, log2: u32, tx_type: usize, pred: &[u16], intra: bool) -> (TxResult, Vec<i32>) {
        let n = 1usize << log2;
        let mut levels = vec![0i32; n * n];
        let mut tb = std::mem::take(&mut self.tx);
        tb.ready();
        let mut out = std::mem::take(&mut tb.out);
        let res = self.try_tx(&mut tb, p, x, y, log2, tx_type, pred, intra, &mut levels, &mut out[..n * n]);
        self.put(p, x, y, n, if res.nonzero { &out[..n * n] } else { pred });
        tb.out = out;
        self.tx = tb;
        (res, levels)
    }

    /// Transform and quantise one block into `levels` (n x n); when the result is nonzero its
    /// reconstruction is written to `recon` (otherwise the reconstruction is the prediction).
    /// Nothing is written to `rec`.
    #[allow(clippy::too_many_arguments)]
    fn try_tx(
        &self,
        tb: &mut TxBufs,
        p: usize,
        x: usize,
        y: usize,
        log2: u32,
        tx_type: usize,
        pred: &[u16],
        intra: bool,
        levels: &mut [i32],
        recon: &mut [u16],
    ) -> TxResult {
        let n = 1usize << log2;
        let nn = n * n;
        let tx_sz = tx_size_for(log2);
        let bd = self.bd;
        let src = &self.src[p];
        let res = &mut tb.res[..nn];
        for i in 0..n {
            let row = &src.row(y + i)[x..x + n];
            for j in 0..n {
                res[i * n + j] = row[j] as i32 - pred[i * n + j] as i32;
            }
        }
        let coef = &mut tb.coef[..nn];
        forward_2d(res, coef, n, tx_type);
        // dead-zone quantiser: rounding offset below one half
        let rnd = if intra { 0.38f32 } else { 0.30f32 };
        levels.fill(0);
        let mut nonzero = false;
        for (k, (&cf, l)) in coef.iter().zip(levels.iter_mut()).enumerate() {
            let q = if k == 0 { self.dc_q } else { self.ac_q };
            let a = (cf.abs() / q + rnd).floor() as i32;
            if a != 0 {
                *l = if cf < 0.0 { -a.min(1 << 16) } else { a.min(1 << 16) };
                nonzero = true;
            }
        }
        let zero_dist = self.sse(p, x, y, n, pred);
        if !nonzero {
            return TxResult { nonzero, dist: zero_dist, bits: 1.0 };
        }
        self.reconstruct(levels, tx_sz, tx_type, pred, n, bd, &mut tb.deq[..nn], &mut tb.resid[..nn], recon);
        let dist = self.sse(p, x, y, n, recon);
        let bits = estimate_bits(levels, tx_sz);
        if dist + self.lambda * bits >= zero_dist + self.lambda {
            levels.fill(0);
            return TxResult { nonzero: false, dist: zero_dist, bits: 1.0 };
        }
        TxResult { nonzero: true, dist, bits }
    }

    /// Dequantisation (7.12.3) and the inverse transform, added to the prediction, into `out`.
    #[allow(clippy::too_many_arguments)]
    fn reconstruct(&self, levels: &[i32], tx_sz: usize, tx_type: usize, pred: &[u16], n: usize, bd: u32, deq: &mut [i32], resid: &mut [i32], out: &mut [u16]) {
        let denom: i64 = if tx_sz == TX_32X32 { 2 } else { 1 };
        let lim = 1i64 << (7 + bd);
        deq.fill(0);
        for (k, (&l, d)) in levels.iter().zip(deq.iter_mut()).enumerate() {
            if l == 0 {
                continue;
            }
            let q = if k == 0 { self.dc_qi } else { self.ac_qi };
            let dq = l as i64 * q;
            let mag = (dq.abs() & 0xFF_FFFF) / denom;
            *d = (if dq < 0 { -mag } else { mag }).clamp(-lim, lim - 1) as i32;
        }
        inverse_2d(deq, resid, tx_sz, tx_type, bd);
        let max = (1i32 << bd) - 1;
        for (o, (&p, &r)) in out.iter_mut().zip(pred.iter().zip(resid.iter())).take(n * n) {
            *o = (p as i32 + r).clamp(0, max) as u16;
        }
    }
}

/// Unnormalised Walsh-Hadamard transform of 4 or 8 values, in place.
fn hadamard(v: &mut [i32]) {
    let n = v.len();
    let mut h = 1;
    while h < n {
        for i in (0..n).step_by(2 * h) {
            for j in i..i + h {
                let (a, b) = (v[j], v[j + h]);
                v[j] = a + b;
                v[j + h] = a - b;
            }
        }
        h *= 2;
    }
}

/// Rough coefficient coding cost in bits.
fn estimate_bits(levels: &[i32], tx_sz: usize) -> f64 {
    let scan = scan_for(tx_sz);
    let mut eob = 0;
    for (i, &pos) in scan.iter().enumerate() {
        if levels[pos as usize] != 0 {
            eob = i + 1;
        }
    }
    if eob == 0 {
        return 1.0;
    }
    let mut bits = 3.0 + (eob as f64).log2();
    for &pos in &scan[..eob] {
        let a = levels[pos as usize].unsigned_abs();
        bits += match a {
            0 => 0.7,
            1 => 2.5,
            _ => 3.5 + 1.6 * (a as f64).log2(),
        };
    }
    bits
}

pub(crate) fn scan_for(tx_sz: usize) -> &'static [u16] {
    match tx_sz {
        TX_4X4 => &DEFAULT_SCAN_4X4,
        TX_8X8 => &DEFAULT_SCAN_8X8,
        TX_16X16 => &DEFAULT_SCAN_16X16,
        _ => &DEFAULT_SCAN_32X32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(w: usize, h: usize) -> (usize, usize, usize, usize) {
        let t = FrameGeom::new(w, h).tiles;
        (t.cols, t.rows, t.w_sb, t.h_sb)
    }

    #[test]
    fn tile_layout_from_frame_size() {
        assert_eq!(layout(1, 1), (1, 1, 1, 1));
        assert_eq!(layout(1920, 1080), (1, 1, 30, 17), "1080p is one tile");
        assert_eq!(layout(2048, 1080), (2, 1, 16, 17));
        assert_eq!(layout(3840, 2160), (2, 1, 30, 34));
        assert_eq!(layout(4096, 2304), (4, 1, 16, 36));
        assert_eq!(layout(7680, 4320), (4, 1, 30, 68));
        // over MAX_TILE_WIDTH, a partial last column
        assert_eq!(layout(4200, 72), (4, 1, 17, 2));
        // over MAX_TILE_AREA in one column: two tile rows
        assert_eq!(layout(1920, 4928), (1, 2, 30, 39));
        assert_eq!(layout(16384, 2176), (16, 1, 16, 34));
        for (w, h) in [(16384, 2176), (7680, 4320), (1920, 4928), (4200, 72), (64, 16384), (16384, 64)] {
            let t = FrameGeom::new(w, h).tiles;
            assert!(t.w_sb <= MAX_TILE_WIDTH_SB && t.w_sb * t.h_sb <= MAX_TILE_AREA_SB, "{w}x{h}");
            assert!(t.cols <= MAX_TILE_COLS && t.rows <= MAX_TILE_ROWS, "{w}x{h}");
        }
    }

    #[test]
    fn tiles_cover_the_frame() {
        let g = FrameGeom::with_tiles(320, 200, 2, 1);
        assert_eq!((g.tiles.cols, g.tiles.rows), (3, 2));
        assert_eq!(g.tile(2), TileRect { r0: 0, r1: 32, c0: 64, c1: 80 });
        assert_eq!(g.tile(5), TileRect { r0: 32, r1: 50, c0: 64, c1: 80 });
        assert_eq!(g.tile_at(49, 79), g.tile(5));
        assert_eq!(g.tile_at(31, 63), g.tile(1));
    }
}
