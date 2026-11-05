//! Tile syntax writer (spec 5.11 with the 8.3 CDF selection). It keeps the same per-4x4 mode
//! info, coefficient contexts and adaptive CDFs as a decoder and writes the decisions taken by
//! `decide.rs` as symbols. The context derivations and the motion vector prediction process
//! (7.10.2) mirror the decoder exactly, since symbols and coded motion vector differences
//! depend on them.

use crate::cdf::Cdfs;
use crate::decide::{FrameGeom, Leaf, Node, intra_uv_tx_type, scan_for, tx_in_set, tx_set, tx_size_for};
use crate::loopfilter::LfInfo;
use crate::symbol::SymbolWriter;
use crate::tables::{
    ADST_ADST, ADST_DCT, ALTREF_FRAME, ALTREF2_FRAME, BLOCK_8X8, BLOCK_64X64, BR_CDF_SIZE, BWDREF_FRAME, CLASS0_SIZE, COEFF_BASE_CTX_OFFSET, COEFF_BASE_RANGE,
    D67_PRED, DC_PRED, DCT_ADST, DCT_DCT, DEFAULT_INTRA_FRAME_Y_MODE_CDF, GLOBALMV, GOLDEN_FRAME, INTRA_FRAME, INTRA_MODE_CONTEXT, LAST_FRAME, LAST2_FRAME,
    LAST3_FRAME, MAG_REF_OFFSET_WITH_TX_CLASS, MAX_ANGLE_DELTA, MAX_REF_MV_STACK_SIZE, MI_HEIGHT_LOG2, MI_WIDTH_LOG2, MV_BORDER, MV_CLASS_0, NEARESTMV, NEARMV,
    NEWMV, NUM_4X4_BLOCKS_HIGH, NUM_4X4_BLOCKS_WIDE, NUM_BASE_LEVELS, PARTITION_HORZ, PARTITION_HORZ_4, PARTITION_HORZ_A, PARTITION_HORZ_B, PARTITION_NONE,
    PARTITION_SPLIT, PARTITION_SUBSIZE, PARTITION_VERT, PARTITION_VERT_4, PARTITION_VERT_A, PARTITION_VERT_B, REF_CAT_LEVEL, SIG_REF_DIFF_OFFSET, SIZE_GROUP,
    SUBSAMPLED_SIZE, TX_32X32, TX_CLASS_2D, TX_HEIGHT, TX_HEIGHT_LOG2, TX_SIZE_SQR, TX_SIZE_SQR_UP, TX_TYPE_INTER_INV_SET3, TX_TYPE_INTRA_INV_SET2, TX_WIDTH,
    TX_WIDTH_LOG2, V_PRED,
};

type MvP = [i32; 2];

/// Mode info per 4x4 (MiSizes, YModes, RefFrames, Mvs, ...).
struct Mi {
    cols: usize,
    mi_size: Vec<u8>,
    y_mode: Vec<u8>,
    ref_frame: Vec<[i8; 2]>,
    mv: Vec<MvP>,
    skip: Vec<bool>,
    is_inter: Vec<bool>,
    tx_type: Vec<u8>,
    written: Vec<bool>,
}

impl Mi {
    #[inline(always)]
    fn idx(&self, r: usize, c: usize) -> usize {
        r * self.cols + c
    }
}

/// Output of find_mv_stack.
#[derive(Default)]
struct MvStack {
    ref_stack_mv: [MvP; MAX_REF_MV_STACK_SIZE + 1],
    weight_stack: [u32; MAX_REF_MV_STACK_SIZE + 1],
    num_mv_found: usize,
    new_mv_count: usize,
    found_match: bool,
    close_matches: usize,
    total_matches: usize,
    new_mv_context: usize,
    ref_mv_context: usize,
    zero_mv_context: usize,
    drl_ctx_stack: [usize; MAX_REF_MV_STACK_SIZE + 1],
}

pub(crate) struct TileWriter<'a> {
    g: &'a FrameGeom,
    intra_only: bool,
    qidx: u8,
    sw: SymbolWriter,
    cdf: Box<Cdfs>,
    kf_y_mode: [[[u16; 14]; 5]; 5],
    mi: Mi,
    above_level: [Vec<u8>; 3],
    above_dc: [Vec<u8>; 3],
    left_level: [Vec<u8>; 3],
    left_dc: [Vec<u8>; 3],
    /// LoopfilterTxSizes per plane (row stride mi_cols + 32).
    lf_tx: [Vec<u8>; 3],
    /// Scratch for the levels coded so far in `coeffs`.
    quant: Vec<i32>,
    // current block
    r: usize,
    c: usize,
    bsize: usize,
    avail_u: bool,
    avail_l: bool,
    mvs: MvStack,
}

fn block_size_px(bs: usize) -> usize {
    4 * NUM_4X4_BLOCKS_WIDE[bs] as usize
}

fn is_directional(mode: usize) -> bool {
    (V_PRED..=D67_PRED).contains(&mode)
}

impl<'a> TileWriter<'a> {
    /// `init`: the CDFs saved by the primary reference frame (load_cdfs), or None for the
    /// defaults (init_non_coeff_cdfs / init_coeff_cdfs).
    pub fn new(g: &'a FrameGeom, intra_only: bool, qidx: u8, init: Option<&Cdfs>) -> Self {
        let n = g.mi_cols * g.mi_rows;
        TileWriter {
            g,
            intra_only,
            qidx,
            sw: SymbolWriter::new(false),
            cdf: match init {
                Some(c) => {
                    let mut c = Box::new(c.clone());
                    c.clear_counts();
                    c
                }
                None => Cdfs::new(qidx as u32),
            },
            kf_y_mode: DEFAULT_INTRA_FRAME_Y_MODE_CDF,
            mi: Mi {
                cols: g.mi_cols,
                mi_size: vec![0; n],
                y_mode: vec![0; n],
                ref_frame: vec![[0, -1]; n],
                mv: vec![[0, 0]; n],
                skip: vec![false; n],
                is_inter: vec![false; n],
                tx_type: vec![0; n],
                written: vec![false; n],
            },
            above_level: std::array::from_fn(|_| vec![0; g.mi_cols + 64]),
            above_dc: std::array::from_fn(|_| vec![0; g.mi_cols + 64]),
            left_level: std::array::from_fn(|_| vec![0; g.mi_rows + 64]),
            left_dc: std::array::from_fn(|_| vec![0; g.mi_rows + 64]),
            lf_tx: std::array::from_fn(|_| vec![0; (g.mi_cols + 32) * (g.mi_rows + 32)]),
            quant: Vec::new(),
            r: 0,
            c: 0,
            bsize: 0,
            avail_u: false,
            avail_l: false,
            mvs: MvStack::default(),
        }
    }

    /// The tile data and the final CDFs (saved for the frame-end CDF update).
    pub fn finish(self) -> (Vec<u8>, Box<Cdfs>) {
        (self.sw.finish(), self.cdf)
    }

    /// The mode info the loop filter needs.
    pub fn lf_info(&self) -> LfInfo<'_> {
        LfInfo {
            mi_cols: self.g.mi_cols,
            mi_rows: self.g.mi_rows,
            width: self.g.width,
            height: self.g.height,
            mi_size: &self.mi.mi_size,
            skip: &self.mi.skip,
            is_inter: &self.mi.is_inter,
            tx: [&self.lf_tx[0], &self.lf_tx[1], &self.lf_tx[2]],
        }
    }

    /// Start of a superblock row (clear_left_context).
    pub fn start_sb_row(&mut self) {
        for p in 0..3 {
            self.left_level[p].iter_mut().for_each(|v| *v = 0);
            self.left_dc[p].iter_mut().for_each(|v| *v = 0);
        }
    }

    pub fn superblock(&mut self, r: usize, c: usize, node: &Node) {
        self.partition(r, c, BLOCK_64X64, node);
    }

    fn partition(&mut self, r: usize, c: usize, bsize: usize, node: &Node) {
        let (mi_rows, mi_cols) = (self.g.mi_rows, self.g.mi_cols);
        if r >= mi_rows || c >= mi_cols {
            return;
        }
        let avail_u = r > 0;
        let avail_l = c > 0;
        let n4 = NUM_4X4_BLOCKS_WIDE[bsize] as usize;
        let half = n4 >> 1;
        let has_rows = r + half < mi_rows;
        let has_cols = c + half < mi_cols;
        let split = matches!(node, Node::Split(_));
        let bsl = MI_WIDTH_LOG2[bsize] as usize;
        let above = avail_u && (MI_WIDTH_LOG2[self.mi.mi_size[self.mi.idx(r - 1, c)] as usize] as usize) < bsl;
        let left = avail_l && (MI_HEIGHT_LOG2[self.mi.mi_size[self.mi.idx(r, c - 1)] as usize] as usize) < bsl;
        let ctx = left as usize * 2 + above as usize;
        let partition = if split { PARTITION_SPLIT } else { PARTITION_NONE };
        if has_rows && has_cols {
            let cdf: &mut [u16] = match bsl {
                1 => &mut self.cdf.partition_w8[ctx],
                2 => &mut self.cdf.partition_w16[ctx],
                3 => &mut self.cdf.partition_w32[ctx],
                _ => &mut self.cdf.partition_w64[ctx],
            };
            self.sw.symbol(partition, cdf);
        } else if has_cols || has_rows {
            assert!(split, "blocks crossing the frame edge are split");
            let pcdf: &[u16] = match bsl {
                2 => &self.cdf.partition_w16[ctx],
                3 => &self.cdf.partition_w32[ctx],
                _ => &self.cdf.partition_w64[ctx],
            };
            let pr = |k: usize| pcdf[k] as i32 - if k == 0 { 0 } else { pcdf[k - 1] as i32 };
            let psum = if has_cols {
                // split_or_horz
                pr(PARTITION_VERT) + pr(PARTITION_SPLIT) + pr(PARTITION_HORZ_A) + pr(PARTITION_VERT_A) + pr(PARTITION_VERT_B) + pr(PARTITION_VERT_4)
            } else {
                // split_or_vert
                pr(PARTITION_HORZ) + pr(PARTITION_SPLIT) + pr(PARTITION_HORZ_A) + pr(PARTITION_HORZ_B) + pr(PARTITION_VERT_A) + pr(PARTITION_HORZ_4)
            };
            let cdf = [((1i32 << 15) - psum) as u16, 1 << 15, 0];
            self.sw.symbol_fixed(1, &cdf);
        } else {
            assert!(split);
        }
        match node {
            Node::Split(ch) => {
                let sub = PARTITION_SUBSIZE[PARTITION_SPLIT][bsize] as usize;
                self.partition(r, c, sub, &ch[0]);
                self.partition(r, c + half, sub, &ch[1]);
                self.partition(r + half, c, sub, &ch[2]);
                self.partition(r + half, c + half, sub, &ch[3]);
            }
            Node::Leaf(leaf) => {
                debug_assert_eq!(leaf.bsize, bsize);
                self.block(r, c, leaf);
            }
            // Blocks inside the frame are always decided; nothing is coded outside it.
            Node::Out => debug_assert!(false, "undecided block inside the frame"),
        }
    }

    fn block(&mut self, r: usize, c: usize, b: &Leaf) {
        self.r = r;
        self.c = c;
        self.bsize = b.bsize;
        self.avail_u = r > 0;
        self.avail_l = c > 0;
        let bsize = b.bsize;
        let n4 = NUM_4X4_BLOCKS_WIDE[bsize] as usize;
        // skip
        let mut ctx = 0;
        if self.avail_u {
            ctx += self.mi.skip[self.mi.idx(r - 1, c)] as usize;
        }
        if self.avail_l {
            ctx += self.mi.skip[self.mi.idx(r, c - 1)] as usize;
        }
        let mut y_mode = b.y_mode;
        if self.intra_only {
            self.sw.symbol(b.skip as usize, &mut self.cdf.skip[ctx]);
            let above = if self.avail_u { self.mi.y_mode[self.mi.idx(r - 1, c)] as usize } else { DC_PRED };
            let left = if self.avail_l { self.mi.y_mode[self.mi.idx(r, c - 1)] as usize } else { DC_PRED };
            let actx = INTRA_MODE_CONTEXT[above] as usize;
            let lctx = INTRA_MODE_CONTEXT[left] as usize;
            self.sw.symbol(b.y_mode, &mut self.kf_y_mode[actx][lctx]);
            self.intra_angle_and_uv(b);
        } else {
            self.sw.symbol(b.skip as usize, &mut self.cdf.skip[ctx]);
            // is_inter
            let left = if self.avail_l { self.mi.ref_frame[self.mi.idx(r, c - 1)] } else { [INTRA_FRAME as i8, -1] };
            let above = if self.avail_u { self.mi.ref_frame[self.mi.idx(r - 1, c)] } else { [INTRA_FRAME as i8, -1] };
            let left_intra = left[0] <= INTRA_FRAME as i8;
            let above_intra = above[0] <= INTRA_FRAME as i8;
            let ctx = if self.avail_u && self.avail_l {
                if left_intra && above_intra { 3 } else { (left_intra || above_intra) as usize }
            } else if self.avail_u || self.avail_l {
                2 * (if self.avail_u { above_intra } else { left_intra }) as usize
            } else {
                0
            };
            self.sw.symbol(b.is_inter as usize, &mut self.cdf.is_inter[ctx]);
            if b.is_inter {
                y_mode = self.inter_block_mode_info(b, left, above);
            } else {
                let ctx = SIZE_GROUP[bsize] as usize;
                self.sw.symbol(b.y_mode, &mut self.cdf.y_mode[ctx]);
                self.intra_angle_and_uv(b);
            }
        }
        // mode info stored before the residual
        let (rows, cols) = (n4.min(self.g.mi_rows - r), n4.min(self.g.mi_cols - c));
        for y in 0..rows {
            for x in 0..cols {
                let i = self.mi.idx(r + y, c + x);
                self.mi.y_mode[i] = y_mode as u8;
                self.mi.ref_frame[i] = if b.is_inter { [LAST_FRAME as i8, -1] } else { [INTRA_FRAME as i8, -1] };
                self.mi.written[i] = true;
                if b.is_inter {
                    self.mi.mv[i] = b.mv;
                }
            }
        }
        if b.skip {
            // reset_block_context( )
            for plane in 0..3 {
                let s = (plane > 0) as usize;
                for i in (c >> s)..((c + n4) >> s) {
                    self.above_level[plane][i] = 0;
                    self.above_dc[plane][i] = 0;
                }
                for i in (r >> s)..((r + n4) >> s) {
                    self.left_level[plane][i] = 0;
                    self.left_dc[plane][i] = 0;
                }
            }
        } else {
            let log2 = (n4 * 4).trailing_zeros();
            assert!(log2 <= 5, "64x64 blocks are coded as skip blocks");
            for plane in 0..3 {
                let s = (plane > 0) as u32;
                self.coeffs(plane, (c * 4) >> s, (r * 4) >> s, tx_size_for(log2 - s), b, &b.coefs[plane]);
            }
        }
        for y in 0..rows {
            for x in 0..cols {
                let i = self.mi.idx(r + y, c + x);
                self.mi.is_inter[i] = b.is_inter;
                self.mi.skip[i] = b.skip;
                self.mi.mi_size[i] = bsize as u8;
            }
        }
        // LoopfilterTxSizes: one transform block per plane
        let log2 = (n4 * 4).trailing_zeros();
        let stride = self.g.mi_cols + 32;
        for plane in 0..3 {
            let s = (plane > 0) as usize;
            let tx = tx_size_for(log2 - s as u32) as u8;
            let (r0, c0, m) = (r >> s, c >> s, n4 >> s);
            for y in r0..r0 + m {
                self.lf_tx[plane][y * stride + c0..y * stride + c0 + m].iter_mut().for_each(|v| *v = tx);
            }
        }
    }

    fn intra_angle_and_uv(&mut self, b: &Leaf) {
        if self.bsize >= BLOCK_8X8 && is_directional(b.y_mode) {
            self.sw.symbol(MAX_ANGLE_DELTA, &mut self.cdf.angle_delta[b.y_mode - V_PRED]);
        }
        let bs = block_size_px(self.bsize);
        if bs <= 32 {
            self.sw.symbol(b.uv_mode, &mut self.cdf.uv_mode_cfl_allowed[b.y_mode]);
        } else {
            self.sw.symbol(b.uv_mode, &mut self.cdf.uv_mode_cfl_not_allowed[b.y_mode]);
        }
        if self.bsize >= BLOCK_8X8 && is_directional(b.uv_mode) {
            self.sw.symbol(MAX_ANGLE_DELTA, &mut self.cdf.angle_delta[b.uv_mode - V_PRED]);
        }
    }

    /// read_ref_frames (LAST_FRAME), find_mv_stack, the inter mode, DRL and motion vector.
    /// Returns the inter mode (YMode).
    fn inter_block_mode_info(&mut self, b: &Leaf, left: [i8; 2], above: [i8; 2]) -> usize {
        let count = |ft: i8| -> usize {
            let mut n = 0;
            if self.avail_u {
                n += (above[0] == ft) as usize + (above[1] == ft) as usize;
            }
            if self.avail_l {
                n += (left[0] == ft) as usize + (left[1] == ft) as usize;
            }
            n
        };
        let cmp = |a: usize, b: usize| match a.cmp(&b) {
            std::cmp::Ordering::Less => 0,
            std::cmp::Ordering::Equal => 1,
            std::cmp::Ordering::Greater => 2,
        };
        let (last, last2, last3, gold) = (count(LAST_FRAME as i8), count(LAST2_FRAME as i8), count(LAST3_FRAME as i8), count(GOLDEN_FRAME as i8));
        let (bwd, alt2, alt) = (count(BWDREF_FRAME as i8), count(ALTREF2_FRAME as i8), count(ALTREF_FRAME as i8));
        let ctx_p1 = cmp(last + last2 + last3 + gold, bwd + alt2 + alt);
        let ctx_comp_ref = cmp(last + last2, last3 + gold);
        let ctx_comp_ref_p1 = cmp(last, last2);
        self.sw.symbol(0, &mut self.cdf.single_ref[ctx_p1][0]);
        self.sw.symbol(0, &mut self.cdf.single_ref[ctx_comp_ref][2]);
        self.sw.symbol(0, &mut self.cdf.single_ref[ctx_comp_ref_p1][3]);
        self.find_mv_stack();
        let mv = b.mv;
        let s = &self.mvs;
        let mode = if mv == [0, 0] {
            GLOBALMV
        } else if mv == s.ref_stack_mv[0] {
            NEARESTMV
        } else if mv == s.ref_stack_mv[1] {
            NEARMV
        } else {
            NEWMV
        };
        let (nctx, zctx, rctx) = (s.new_mv_context, s.zero_mv_context, s.ref_mv_context);
        self.sw.symbol((mode != NEWMV) as usize, &mut self.cdf.new_mv[nctx]);
        if mode != NEWMV {
            self.sw.symbol((mode != GLOBALMV) as usize, &mut self.cdf.zero_mv[zctx]);
            if mode != GLOBALMV {
                self.sw.symbol((mode == NEARMV) as usize, &mut self.cdf.ref_mv[rctx]);
            }
        }
        // DRL: ref_mv_idx 0 for NEWMV, 1 for NEARMV
        if mode == NEWMV {
            if self.mvs.num_mv_found > 1 {
                let ctx = self.mvs.drl_ctx_stack[0];
                self.sw.symbol(0, &mut self.cdf.drl_mode[ctx]);
            }
        } else if mode == NEARMV && self.mvs.num_mv_found > 2 {
            let ctx = self.mvs.drl_ctx_stack[1];
            self.sw.symbol(0, &mut self.cdf.drl_mode[ctx]);
        }
        if mode == NEWMV {
            let pred = self.mvs.ref_stack_mv[0];
            self.write_mv([mv[0] - pred[0], mv[1] - pred[1]]);
        }
        mode
    }

    fn write_mv(&mut self, diff: MvP) {
        let joint = ((diff[0] != 0) as usize) << 1 | (diff[1] != 0) as usize;
        // MV_JOINT_ZERO, MV_JOINT_HNZVZ (col only), MV_JOINT_HZVNZ (row only), MV_JOINT_HNZVNZ
        self.sw.symbol(joint, &mut self.cdf.mv_joint);
        for (comp, &d) in diff.iter().enumerate() {
            if d != 0 {
                self.write_mv_component(comp, d);
            }
        }
    }

    fn write_mv_component(&mut self, comp: usize, v: i32) {
        debug_assert!(v % 2 == 0, "quarter-sample precision");
        self.sw.symbol((v < 0) as usize, &mut self.cdf.mv_sign[comp]);
        let m = v.unsigned_abs() - 1; // odd: the implied hp bit is 1
        let fr = ((m >> 1) & 3) as usize;
        let int = m >> 3;
        if int < CLASS0_SIZE as u32 {
            self.sw.symbol(MV_CLASS_0, &mut self.cdf.mv_class[comp]);
            self.sw.symbol(int as usize, &mut self.cdf.mv_class0_bit[comp]);
            self.sw.symbol(fr, &mut self.cdf.mv_class0_fr[comp][int as usize]);
        } else {
            let class = 31 - int.leading_zeros();
            let d = int - (1 << class);
            self.sw.symbol(class as usize, &mut self.cdf.mv_class[comp]);
            for i in 0..class as usize {
                self.sw.symbol(((d >> i) & 1) as usize, &mut self.cdf.mv_bit[comp][i]);
            }
            self.sw.symbol(fr, &mut self.cdf.mv_fr[comp]);
        }
    }

    // ---- coefficients ----------------------------------------------------------------------

    fn coeffs(&mut self, plane: usize, start_x: usize, start_y: usize, tx_sz: usize, b: &Leaf, levels: &[i32]) {
        let x4 = start_x >> 2;
        let y4 = start_y >> 2;
        let w4 = TX_WIDTH[tx_sz] as usize >> 2;
        let h4 = TX_HEIGHT[tx_sz] as usize >> 2;
        let tx_sz_ctx = (TX_SIZE_SQR[tx_sz] as usize + TX_SIZE_SQR_UP[tx_sz] as usize + 1) >> 1;
        let ptype = (plane > 0) as usize;
        let scan = scan_for(tx_sz);
        let mut eob = 0;
        for (i, &pos) in scan.iter().enumerate() {
            if levels[pos as usize] != 0 {
                eob = i + 1;
            }
        }
        let ctx = self.all_zero_ctx(plane, tx_sz, x4, y4, w4, h4);
        self.sw.symbol((eob == 0) as usize, &mut self.cdf.txb_skip[tx_sz_ctx][ctx]);
        let mut cul_level = 0u32;
        let mut dc_category = 0u8;
        if eob == 0 {
            if plane == 0 {
                self.set_tx_types(x4, y4, tx_sz, DCT_DCT);
            }
        } else {
            if plane == 0 {
                self.write_transform_type(x4, y4, tx_sz, b);
            }
            let plane_tx_type = self.compute_tx_type(plane, tx_sz, x4, y4, b);
            debug_assert!(plane_tx_type == DCT_DCT || matches!(plane_tx_type, ADST_DCT | DCT_ADST | ADST_ADST));
            let eob_multisize = (TX_WIDTH_LOG2[tx_sz] as usize).min(5) + (TX_HEIGHT_LOG2[tx_sz] as usize).min(5) - 4;
            let eob_pt = if eob < 2 { eob } else { (32 - ((eob - 1) as u32).leading_zeros()) as usize + 1 };
            let ectx = 0; // TX_CLASS_2D
            let s = eob_pt - 1;
            match eob_multisize {
                0 => self.sw.symbol(s, &mut self.cdf.eob_pt_16[ptype][ectx]),
                1 => self.sw.symbol(s, &mut self.cdf.eob_pt_32[ptype][ectx]),
                2 => self.sw.symbol(s, &mut self.cdf.eob_pt_64[ptype][ectx]),
                3 => self.sw.symbol(s, &mut self.cdf.eob_pt_128[ptype][ectx]),
                4 => self.sw.symbol(s, &mut self.cdf.eob_pt_256[ptype][ectx]),
                5 => self.sw.symbol(s, &mut self.cdf.eob_pt_512[ptype]),
                _ => self.sw.symbol(s, &mut self.cdf.eob_pt_1024[ptype]),
            }
            if eob_pt >= 3 {
                let eob_shift = eob_pt - 3;
                let offset = eob - ((1 << (eob_pt - 2)) + 1);
                self.sw.symbol((offset >> eob_shift) & 1, &mut self.cdf.eob_extra[tx_sz_ctx][ptype][eob_pt - 3]);
                for i in 1..eob_pt - 2 {
                    let sh = eob_pt - 2 - 1 - i;
                    self.sw.bool((offset >> sh) & 1 == 1);
                }
            }
            let bwl = TX_WIDTH_LOG2[tx_sz] as usize;
            let height = TX_HEIGHT[tx_sz] as usize;
            let width = 1usize << bwl;
            // Quant[] as the decoder sees it while parsing: the levels coded so far (capped)
            let mut quant = std::mem::take(&mut self.quant);
            quant.clear();
            quant.resize(width * height, 0);
            for c in (0..eob).rev() {
                let pos = scan[c] as usize;
                let a = levels[pos].unsigned_abs().min(15) as i32;
                if c == eob - 1 {
                    let ctx = coeff_base_eob_ctx(c, bwl, height);
                    self.sw.symbol((a.min(3) - 1) as usize, &mut self.cdf.coeff_base_eob[tx_sz_ctx][ptype][ctx]);
                } else {
                    let ctx = coeff_base_ctx(&quant, tx_sz, bwl, width, height, pos);
                    self.sw.symbol(a.min(3) as usize, &mut self.cdf.coeff_base[tx_sz_ctx][ptype][ctx]);
                }
                if a > NUM_BASE_LEVELS as i32 {
                    let ctx = coeff_br_ctx(&quant, bwl, height, pos);
                    let mut rem = a - 3;
                    for _ in 0..(COEFF_BASE_RANGE / (BR_CDF_SIZE - 1)) {
                        let br = rem.min(3);
                        self.sw.symbol(br as usize, &mut self.cdf.coeff_br[tx_sz_ctx.min(TX_32X32)][ptype][ctx]);
                        rem -= br;
                        if br < 3 {
                            break;
                        }
                    }
                }
                quant[pos] = a;
            }
            self.quant = quant;
            for c in 0..eob {
                let pos = scan[c] as usize;
                let v = levels[pos];
                if v != 0 {
                    if c == 0 {
                        let ctx = self.dc_sign_ctx(plane, x4, y4, w4, h4);
                        self.sw.symbol((v < 0) as usize, &mut self.cdf.dc_sign[ptype][ctx]);
                    } else {
                        self.sw.bool(v < 0);
                    }
                }
                let a = v.unsigned_abs();
                if a > (NUM_BASE_LEVELS + COEFF_BASE_RANGE) as u32 {
                    // Golomb code of a - 15
                    let x = a - 15 + 1;
                    let length = 32 - x.leading_zeros();
                    for _ in 0..length - 1 {
                        self.sw.bool(false);
                    }
                    self.sw.bool(true);
                    for i in (0..length - 1).rev() {
                        self.sw.bool((x >> i) & 1 == 1);
                    }
                }
                if pos == 0 && a > 0 {
                    dc_category = if v < 0 { 1 } else { 2 };
                }
                cul_level += a & 0xFFFFF;
            }
            cul_level = cul_level.min(63);
        }
        for i in 0..w4 {
            if x4 + i < self.above_level[plane].len() {
                self.above_level[plane][x4 + i] = cul_level as u8;
                self.above_dc[plane][x4 + i] = dc_category;
            }
        }
        for i in 0..h4 {
            if y4 + i < self.left_level[plane].len() {
                self.left_level[plane][y4 + i] = cul_level as u8;
                self.left_dc[plane][y4 + i] = dc_category;
            }
        }
    }

    fn write_transform_type(&mut self, x4: usize, y4: usize, tx_sz: usize, b: &Leaf) {
        // transform_type( )
        let set = tx_set(tx_sz, b.is_inter);
        let t = b.tx_type_y;
        if set > 0 && self.qidx > 0 {
            let sqr = TX_SIZE_SQR[tx_sz] as usize;
            if b.is_inter {
                // The decision only picks types of this set (DCT_DCT otherwise).
                let s = TX_TYPE_INTER_INV_SET3.iter().position(|&v| v as usize == t).unwrap_or_default();
                self.sw.symbol(s, &mut self.cdf.inter_tx_type_set3[sqr]);
            } else {
                let s = TX_TYPE_INTRA_INV_SET2.iter().position(|&v| v as usize == t).unwrap_or_default();
                self.sw.symbol(s, &mut self.cdf.intra_tx_type_set2[sqr][b.y_mode]);
            }
        } else {
            assert_eq!(t, DCT_DCT);
        }
        self.set_tx_types(x4, y4, tx_sz, t);
    }

    fn set_tx_types(&mut self, x4: usize, y4: usize, tx_sz: usize, t: usize) {
        for i in 0..(TX_WIDTH[tx_sz] as usize >> 2) {
            for j in 0..(TX_HEIGHT[tx_sz] as usize >> 2) {
                if y4 + j < self.g.mi_rows && x4 + i < self.g.mi_cols {
                    let k = self.mi.idx(y4 + j, x4 + i);
                    self.mi.tx_type[k] = t as u8;
                }
            }
        }
    }

    fn compute_tx_type(&self, plane: usize, tx_sz: usize, x4: usize, y4: usize, b: &Leaf) -> usize {
        if TX_SIZE_SQR_UP[tx_sz] as usize > TX_32X32 {
            return DCT_DCT;
        }
        if plane == 0 {
            return self.mi.tx_type[self.mi.idx(y4, x4)] as usize;
        }
        if b.is_inter {
            let set = tx_set(tx_sz, true);
            let xx = self.c.max(x4 << 1);
            let yy = self.r.max(y4 << 1);
            let t = self.mi.tx_type[self.mi.idx(yy, xx)] as usize;
            return if tx_in_set(set, true, t) { t } else { DCT_DCT };
        }
        intra_uv_tx_type(b.uv_mode, tx_sz)
    }

    fn all_zero_ctx(&self, plane: usize, tx_sz: usize, x4: usize, y4: usize, w4: usize, h4: usize) -> usize {
        let s = (plane > 0) as usize;
        let max_x4 = self.g.mi_cols >> s;
        let max_y4 = self.g.mi_rows >> s;
        let w = TX_WIDTH[tx_sz] as usize;
        let h = TX_HEIGHT[tx_sz] as usize;
        let bsize = SUBSAMPLED_SIZE[self.bsize][s][s] as usize;
        let bw = block_size_px(bsize);
        let bh = 4 * NUM_4X4_BLOCKS_HIGH[bsize] as usize;
        let al = &self.above_level[plane];
        let ll = &self.left_level[plane];
        if plane == 0 {
            let mut top = 0u32;
            let mut left = 0u32;
            for k in 0..w4 {
                if x4 + k < max_x4 {
                    top = top.max(al[x4 + k] as u32);
                }
            }
            for k in 0..h4 {
                if y4 + k < max_y4 {
                    left = left.max(ll[y4 + k] as u32);
                }
            }
            top = top.min(255);
            left = left.min(255);
            if bw == w && bh == h {
                0
            } else if top == 0 && left == 0 {
                1
            } else if top == 0 || left == 0 {
                2 + (top.max(left) > 3) as usize
            } else if top.max(left) <= 3 {
                4
            } else if top.min(left) <= 3 {
                5
            } else {
                6
            }
        } else {
            let ad = &self.above_dc[plane];
            let ld = &self.left_dc[plane];
            let mut above = 0u8;
            let mut left = 0u8;
            for i in 0..w4 {
                if x4 + i < max_x4 {
                    above |= al[x4 + i] | ad[x4 + i];
                }
            }
            for i in 0..h4 {
                if y4 + i < max_y4 {
                    left |= ll[y4 + i] | ld[y4 + i];
                }
            }
            let mut ctx = (above != 0) as usize + (left != 0) as usize + 7;
            if bw * bh > w * h {
                ctx += 3;
            }
            ctx
        }
    }

    fn dc_sign_ctx(&self, plane: usize, x4: usize, y4: usize, w4: usize, h4: usize) -> usize {
        let s = (plane > 0) as usize;
        let max_x4 = self.g.mi_cols >> s;
        let max_y4 = self.g.mi_rows >> s;
        let mut dc_sign = 0i32;
        for k in 0..w4 {
            if x4 + k < max_x4 {
                match self.above_dc[plane][x4 + k] {
                    1 => dc_sign -= 1,
                    2 => dc_sign += 1,
                    _ => {}
                }
            }
        }
        for k in 0..h4 {
            if y4 + k < max_y4 {
                match self.left_dc[plane][y4 + k] {
                    1 => dc_sign -= 1,
                    2 => dc_sign += 1,
                    _ => {}
                }
            }
        }
        match dc_sign.signum() {
            -1 => 1,
            1 => 2,
            _ => 0,
        }
    }

    // ---- motion vector prediction (7.10.2), single reference LAST_FRAME --------------------

    fn inside(&self, r: isize, c: isize) -> bool {
        c >= 0 && c < self.g.mi_cols as isize && r >= 0 && r < self.g.mi_rows as isize
    }

    fn find_mv_stack(&mut self) {
        let n4 = NUM_4X4_BLOCKS_WIDE[self.bsize] as isize;
        let (bw4, bh4) = (n4, n4);
        self.mvs = MvStack::default();
        self.scan_row(-1);
        let mut found_above = self.mvs.found_match;
        self.mvs.found_match = false;
        self.scan_col(-1);
        let mut found_left = self.mvs.found_match;
        self.mvs.found_match = false;
        if bw4.max(bh4) <= 16 {
            self.scan_point(-1, bw4);
        }
        if self.mvs.found_match {
            found_above = true;
        }
        self.mvs.close_matches = found_above as usize + found_left as usize;
        let num_nearest = self.mvs.num_mv_found;
        let num_new = self.mvs.new_mv_count;
        for idx in 0..num_nearest {
            self.mvs.weight_stack[idx] += REF_CAT_LEVEL as u32;
        }
        self.mvs.zero_mv_context = 0;
        // use_ref_frame_mvs = 0: no temporal candidates
        self.scan_point(-1, -1);
        if self.mvs.found_match {
            found_above = true;
        }
        self.mvs.found_match = false;
        self.scan_row(-3);
        if self.mvs.found_match {
            found_above = true;
        }
        self.mvs.found_match = false;
        self.scan_col(-3);
        if self.mvs.found_match {
            found_left = true;
        }
        self.mvs.found_match = false;
        if bh4 > 1 {
            self.scan_row(-5);
        }
        if self.mvs.found_match {
            found_above = true;
        }
        self.mvs.found_match = false;
        if bw4 > 1 {
            self.scan_col(-5);
        }
        if self.mvs.found_match {
            found_left = true;
        }
        self.mvs.total_matches = found_above as usize + found_left as usize;
        let n = self.mvs.num_mv_found;
        self.sort_stack(0, num_nearest);
        self.sort_stack(num_nearest, n);
        if self.mvs.num_mv_found < 2 {
            self.extra_search();
        }
        self.context_and_clamping(num_new);
    }

    fn scan_row(&mut self, delta_row: isize) {
        let bw4 = NUM_4X4_BLOCKS_WIDE[self.bsize] as isize;
        let (mi_row, mi_col) = (self.r as isize, self.c as isize);
        let end4 = bw4.min(self.g.mi_cols as isize - mi_col).min(16);
        let mut delta_col = 0;
        let use_step16 = bw4 >= 16;
        let mut delta_row = delta_row;
        if delta_row.abs() > 1 {
            delta_row += mi_row & 1;
            delta_col = 1 - (mi_col & 1);
        }
        let mut i = 0;
        while i < end4 {
            let mv_row = mi_row + delta_row;
            let mv_col = mi_col + delta_col + i;
            if !self.inside(mv_row, mv_col) {
                break;
            }
            let k = self.mi.idx(mv_row as usize, mv_col as usize);
            let mut len = bw4.min(NUM_4X4_BLOCKS_WIDE[self.mi.mi_size[k] as usize] as isize);
            if delta_row.abs() > 1 {
                len = len.max(2);
            }
            if use_step16 {
                len = len.max(4);
            }
            self.add_ref_mv_candidate(mv_row as usize, mv_col as usize, len as u32 * 2);
            i += len;
        }
    }

    fn scan_col(&mut self, delta_col: isize) {
        let bh4 = NUM_4X4_BLOCKS_HIGH[self.bsize] as isize;
        let (mi_row, mi_col) = (self.r as isize, self.c as isize);
        let end4 = bh4.min(self.g.mi_rows as isize - mi_row).min(16);
        let mut delta_row = 0;
        let use_step16 = bh4 >= 16;
        let mut delta_col = delta_col;
        if delta_col.abs() > 1 {
            delta_row = 1 - (mi_row & 1);
            delta_col += mi_col & 1;
        }
        let mut i = 0;
        while i < end4 {
            let mv_row = mi_row + delta_row + i;
            let mv_col = mi_col + delta_col;
            if !self.inside(mv_row, mv_col) {
                break;
            }
            let k = self.mi.idx(mv_row as usize, mv_col as usize);
            let mut len = bh4.min(NUM_4X4_BLOCKS_HIGH[self.mi.mi_size[k] as usize] as isize);
            if delta_col.abs() > 1 {
                len = len.max(2);
            }
            if use_step16 {
                len = len.max(4);
            }
            self.add_ref_mv_candidate(mv_row as usize, mv_col as usize, len as u32 * 2);
            i += len;
        }
    }

    fn scan_point(&mut self, delta_row: isize, delta_col: isize) {
        let mv_row = self.r as isize + delta_row;
        let mv_col = self.c as isize + delta_col;
        if self.inside(mv_row, mv_col) && self.mi.written[self.mi.idx(mv_row as usize, mv_col as usize)] {
            self.add_ref_mv_candidate(mv_row as usize, mv_col as usize, 4);
        }
    }

    fn add_ref_mv_candidate(&mut self, mv_row: usize, mv_col: usize, weight: u32) {
        let i = self.mi.idx(mv_row, mv_col);
        if !self.mi.is_inter[i] {
            return;
        }
        let rf = self.mi.ref_frame[i];
        for cand_list in 0..2 {
            if rf[cand_list] == LAST_FRAME as i8 {
                self.search_stack(i, cand_list, weight);
            }
        }
    }

    fn search_stack(&mut self, i: usize, cand_list: usize, weight: u32) {
        debug_assert_eq!(cand_list, 0);
        let cand_mode = self.mi.y_mode[i] as usize;
        // gm_type is IDENTITY: the stored motion vector is used
        let mut cand = self.mi.mv[i];
        lower_mv_precision(&mut cand);
        if cand_mode == NEWMV {
            self.mvs.new_mv_count += 1;
        }
        self.mvs.found_match = true;
        let n = self.mvs.num_mv_found;
        if let Some(idx) = (0..n).find(|&k| self.mvs.ref_stack_mv[k] == cand) {
            self.mvs.weight_stack[idx] += weight;
        } else if n < MAX_REF_MV_STACK_SIZE {
            self.mvs.ref_stack_mv[n] = cand;
            self.mvs.weight_stack[n] = weight;
            self.mvs.num_mv_found += 1;
        }
    }

    fn sort_stack(&mut self, start: usize, end: usize) {
        let mut end = end;
        let s = &mut self.mvs;
        while end > start {
            let mut new_end = start;
            for idx in start + 1..end {
                if s.weight_stack[idx - 1] < s.weight_stack[idx] {
                    s.weight_stack.swap(idx - 1, idx);
                    s.ref_stack_mv.swap(idx - 1, idx);
                    new_end = idx;
                }
            }
            end = new_end;
        }
    }

    fn extra_search(&mut self) {
        let (mi_row, mi_col) = (self.r as isize, self.c as isize);
        let n4 = NUM_4X4_BLOCKS_WIDE[self.bsize] as isize;
        let w4 = n4.min(16).min(self.g.mi_cols as isize - mi_col);
        let h4 = n4.min(16).min(self.g.mi_rows as isize - mi_row);
        let num4x4 = w4.min(h4);
        for pass in 0..2 {
            let mut idx = 0;
            while idx < num4x4 && self.mvs.num_mv_found < 2 {
                let (mv_row, mv_col) = if pass == 0 { (mi_row - 1, mi_col + idx) } else { (mi_row + idx, mi_col - 1) };
                if !self.inside(mv_row, mv_col) {
                    break;
                }
                let k = self.mi.idx(mv_row as usize, mv_col as usize);
                // add_extra_mv_candidate( ) (no sign bias: a single forward reference)
                let rfs = self.mi.ref_frame[k];
                for &cand_ref in rfs.iter().take(2) {
                    if cand_ref > INTRA_FRAME as i8 {
                        let cand = self.mi.mv[k];
                        let n = self.mvs.num_mv_found;
                        if !(0..n).any(|j| self.mvs.ref_stack_mv[j] == cand) {
                            self.mvs.ref_stack_mv[n] = cand;
                            self.mvs.weight_stack[n] = 2;
                            self.mvs.num_mv_found += 1;
                        }
                    }
                }
                let sz = self.mi.mi_size[k] as usize;
                idx += if pass == 0 { NUM_4X4_BLOCKS_WIDE[sz] } else { NUM_4X4_BLOCKS_HIGH[sz] } as isize;
            }
        }
        for idx in self.mvs.num_mv_found..2 {
            self.mvs.ref_stack_mv[idx] = [0, 0]; // GlobalMvs[ 0 ]
        }
    }

    fn context_and_clamping(&mut self, num_new: usize) {
        let bs = block_size_px(self.bsize) as i32;
        let n = self.mvs.num_mv_found;
        for idx in 0..n {
            let mut z = 0;
            if idx + 1 < n {
                let w0 = self.mvs.weight_stack[idx];
                let w1 = self.mvs.weight_stack[idx + 1];
                if w0 >= REF_CAT_LEVEL as u32 {
                    if w1 < REF_CAT_LEVEL as u32 {
                        z = 1;
                    }
                } else {
                    z = 2;
                }
            }
            self.mvs.drl_ctx_stack[idx] = z;
        }
        let n4 = NUM_4X4_BLOCKS_WIDE[self.bsize] as i32;
        for idx in 0..n {
            let mut mv = self.mvs.ref_stack_mv[idx];
            let border_r = MV_BORDER as i32 + bs * 8;
            let to_top = -((self.r as i32 * 4) * 8);
            let to_bottom = ((self.g.mi_rows as i32 - n4 - self.r as i32) * 4) * 8;
            mv[0] = mv[0].clamp(to_top - border_r, to_bottom + border_r);
            let to_left = -((self.c as i32 * 4) * 8);
            let to_right = ((self.g.mi_cols as i32 - n4 - self.c as i32) * 4) * 8;
            mv[1] = mv[1].clamp(to_left - border_r, to_right + border_r);
            self.mvs.ref_stack_mv[idx] = mv;
        }
        let (close, total) = (self.mvs.close_matches, self.mvs.total_matches);
        if close == 0 {
            self.mvs.new_mv_context = total.min(1);
            self.mvs.ref_mv_context = total;
        } else if close == 1 {
            self.mvs.new_mv_context = 3 - num_new.min(1);
            self.mvs.ref_mv_context = 2 + total;
        } else {
            self.mvs.new_mv_context = 5 - num_new.min(1);
            self.mvs.ref_mv_context = 5;
        }
    }
}

/// lower_mv_precision( ) with allow_high_precision_mv = 0 and force_integer_mv = 0.
fn lower_mv_precision(mv: &mut MvP) {
    for c in mv.iter_mut() {
        if *c & 1 != 0 {
            *c += if *c > 0 { -1 } else { 1 };
        }
    }
}

fn coeff_base_eob_ctx(c: usize, bwl: usize, height: usize) -> usize {
    if c == 0 {
        0
    } else if c <= (height << bwl) / 8 {
        1
    } else if c <= (height << bwl) / 4 {
        2
    } else {
        3
    }
}

/// Context of coeff_base for TX_CLASS_2D (8.3.2).
fn coeff_base_ctx(quant: &[i32], tx_sz: usize, bwl: usize, width: usize, height: usize, pos: usize) -> usize {
    let row = pos >> bwl;
    let col = pos - (row << bwl);
    let mut mag = 0i32;
    for off in SIG_REF_DIFF_OFFSET[TX_CLASS_2D].iter() {
        let ref_row = row + off[0] as usize;
        let ref_col = col + off[1] as usize;
        if ref_row < height && ref_col < width {
            mag += quant[(ref_row << bwl) + ref_col].abs().min(3);
        }
    }
    let ctx = ((mag + 1) >> 1).min(4) as usize;
    if row == 0 && col == 0 {
        return 0;
    }
    ctx + COEFF_BASE_CTX_OFFSET[tx_sz][row.min(4)][col.min(4)] as usize
}

/// Context of coeff_br for TX_CLASS_2D.
fn coeff_br_ctx(quant: &[i32], bwl: usize, txh: usize, pos: usize) -> usize {
    let txw = 1usize << bwl;
    let row = pos >> bwl;
    let col = pos - (row << bwl);
    let mut mag = 0i32;
    for off in MAG_REF_OFFSET_WITH_TX_CLASS[TX_CLASS_2D].iter() {
        let ref_row = row + off[0] as usize;
        let ref_col = col + off[1] as usize;
        if ref_row < txh && ref_col < txw {
            mag += quant[ref_row * txw + ref_col].min((COEFF_BASE_RANGE + NUM_BASE_LEVELS + 1) as i32);
        }
    }
    let mag = ((mag + 1) >> 1).min(6) as usize;
    if pos == 0 {
        mag
    } else if row < 2 && col < 2 {
        mag + 7
    } else {
        mag + 14
    }
}
