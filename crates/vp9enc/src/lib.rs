//! A VP9 encoder (profile 0, 8-bit 4:2:0), written from the *VP9 Bitstream & Decoding Process
//! Specification* v0.6 by mirroring its decoding process in reverse (see README.md).
//!
//! * **Key frames**: 64×64 superblocks split to 8×8 intra blocks (DC / V / H / TM chosen per
//!   block), 4×4 DCT/ADST transforms (the Walsh–Hadamard transform at quality 100: lossless).
//! * **Inter frames** (error resilient, single reference: the previous frame as LAST): 16×16 or
//!   8×8 blocks with motion vectors found by a candidate + diamond search refined to quarter
//!   samples with the regular 8-tap filters; NEARESTMV / NEARMV / ZEROMV / NEWMV chosen by cost,
//!   motion vector prediction exactly as the decoder derives it; skipped blocks; 4×4 DCT residual.
//! * **In-loop deblocking** (8.8) on every lossy frame; the level is picked per frame by trial.
//! * **Probabilities**: per-frame forward updates (compressed header) of the coefficient, skip,
//!   inter mode, reference, partition and motion vector probabilities, chosen from the frame's
//!   own symbol counts (no backward adaptation: `refresh_frame_context` = 0).
//! * **Rate control**: constant quality (quality → base_q_idx), or a target bitrate with a
//!   per-frame q_idx model.
//!
//! The encoder reconstructs exactly what a decoder reconstructs (prediction, integer inverse
//! transforms, loop filter), so prediction never drifts; [`Vp9Encoder::reconstruction`] returns
//! it.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod bool;
mod lf;
mod mc;
mod probs;
mod tables;
mod tile;
mod transform;

pub use mc::Mv;
use probs::Probs;
use tables::{AC_QLOOKUP, DC_QLOOKUP};
use transform::Quant;

pub(crate) const BLOCK_8X8: u8 = 3;
pub(crate) const BLOCK_16X16: u8 = 6;

/// Encoder settings.
#[derive(Clone, Debug, PartialEq)]
pub struct EncoderConfig {
    pub width: u32,
    pub height: u32,
    /// 1..=100; 100 is lossless. With a target bitrate: the quality of the first frame.
    pub quality: u8,
    /// Full-range (0–255) rather than limited (16–235) YUV; only signalled.
    pub full_range: bool,
    /// A key frame every this many frames (1 = every frame is a key frame; 0 = only the first).
    pub keyframe_interval: u32,
    /// Target bitrate in kbit/s (`None`: constant quality).
    pub target_kbps: Option<u32>,
    /// Frames per second (rate control).
    pub frame_rate: f64,
    /// The in-loop deblocking filter.
    pub loop_filter: bool,
}

impl Default for EncoderConfig {
    fn default() -> Self {
        EncoderConfig { width: 0, height: 0, quality: 80, full_range: false, keyframe_interval: 60, target_kbps: None, frame_rate: 30.0, loop_filter: true }
    }
}

/// One coded frame.
#[derive(Clone, Debug)]
pub struct EncodedFrame {
    pub data: Vec<u8>,
    pub key: bool,
    pub q_idx: u8,
    pub filter_level: u8,
}

/// A plane being coded: source and reconstruction at superblock-aligned size.
pub(crate) struct Plane {
    /// Stride (superblock-aligned width).
    pub w: usize,
    /// Last valid column / row for intra prediction edges (the 8-aligned mode-info grid).
    pub max_x: usize,
    pub max_y: usize,
    /// Visible size.
    pub vis_w: usize,
    pub vis_h: usize,
    pub src: Vec<u8>,
    pub rec: Vec<u8>,
}

impl Plane {
    fn new(data: &[u8], vw: usize, vh: usize, w: usize, h: usize, max_x: usize, max_y: usize) -> Plane {
        let mut src = vec![0u8; w * h];
        for y in 0..h {
            let sy = y.min(vh.saturating_sub(1));
            for x in 0..w {
                let sx = x.min(vw.saturating_sub(1));
                src[y * w + x] = data.get(sy * vw + sx).copied().unwrap_or(128);
            }
        }
        Plane { w, max_x, max_y, vis_w: vw, vis_h: vh, src, rec: vec![0; w * h] }
    }
}

/// Mode info of one 8×8 position.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Mi {
    pub inter: bool,
    /// Intra mode (0..=9) or inter mode (NEARESTMV..=NEWMV).
    pub mode: u8,
    pub mv: Mv,
    /// Skip as the loop filter sees it (inter blocks without coefficients).
    pub skip: bool,
    pub size: u8,
}

/// Rate control state (target bitrate mode).
#[derive(Clone, Debug)]
struct RateControl {
    /// bits ≈ complexity / step: per frame type, from the last frame of that type.
    key_complexity: Option<f64>,
    inter_complexity: Option<f64>,
    /// Bits spent minus bits budgeted so far.
    debt: f64,
    last_inter_q: Option<u8>,
}

pub struct Vp9Encoder {
    cfg: EncoderConfig,
    base_qidx: u8,
    lossless: bool,
    frames: u64,
    since_key: u32,
    /// The reconstruction of the last frame (the LAST reference): planes at their strides.
    reference: Option<[Plane; 3]>,
    mvs: Vec<Mv>,
    rc: RateControl,
}

fn quality_to_qidx(q: u8) -> u8 {
    // 99 ≈ 3, 90 ≈ 30, 50 ≈ 150, 1 ≈ 255.
    ((100 - q.clamp(1, 99) as i32) * 255 / 85).clamp(1, 255) as u8
}

impl Vp9Encoder {
    pub fn new(cfg: EncoderConfig) -> Self {
        let q = cfg.quality.clamp(1, 100);
        let lossless = q >= 100 && cfg.target_kbps.is_none();
        let base_qidx = if lossless { 0 } else { quality_to_qidx(q) };
        Vp9Encoder {
            cfg,
            base_qidx,
            lossless,
            frames: 0,
            since_key: 0,
            reference: None,
            mvs: vec![],
            rc: RateControl { key_complexity: None, inter_complexity: None, debt: 0.0, last_inter_q: None },
        }
    }

    pub fn config(&self) -> &EncoderConfig {
        &self.cfg
    }

    /// Encode one frame from planar 8-bit 4:2:0 (`y`: width×height, `u`/`v`:
    /// ((w+1)/2)×((h+1)/2), tightly packed).
    pub fn encode_yuv420(&mut self, y: &[u8], u: &[u8], v: &[u8]) -> Vec<u8> {
        self.encode_frame(y, u, v, false).data
    }

    /// The decoder's reconstruction of the last coded frame (visible area, tightly packed Y, U,
    /// V).
    pub fn reconstruction(&self) -> Option<[Vec<u8>; 3]> {
        let r = self.reference.as_ref()?;
        Some(std::array::from_fn(|i| {
            let p = &r[i];
            (0..p.vis_h).flat_map(|y| p.rec[y * p.w..y * p.w + p.vis_w].iter().copied()).collect()
        }))
    }

    fn is_key_due(&self, force: bool) -> bool {
        force || self.reference.is_none() || self.cfg.keyframe_interval == 1 || (self.cfg.keyframe_interval > 1 && self.since_key >= self.cfg.keyframe_interval)
    }

    /// The q index of the next frame.
    fn pick_qidx(&mut self, key: bool) -> u8 {
        if self.lossless {
            return 0;
        }
        let Some(kbps) = self.cfg.target_kbps else { return self.base_qidx };
        let per_frame = kbps as f64 * 1000.0 / self.cfg.frame_rate.max(1.0);
        let interval = if self.cfg.keyframe_interval == 0 { 300.0 } else { self.cfg.keyframe_interval as f64 };
        // Key frames get a larger share of the group's budget.
        let boost = (interval / 6.0).clamp(1.0, 5.0);
        let group = per_frame * interval;
        let key_share = group * boost / (boost + interval - 1.0);
        let inter_share = (group - key_share) / (interval - 1.0).max(1.0);
        let mut target = if key { key_share } else { inter_share };
        // Pay back (or spend) the running difference over about a second.
        let horizon = self.cfg.frame_rate.max(1.0);
        target = (target - self.rc.debt / horizon).max(target * 0.25);
        let complexity = if key { self.rc.key_complexity } else { self.rc.inter_complexity.or(self.rc.key_complexity.map(|c| c / (boost * 2.0))) };
        let Some(cx) = complexity else { return self.base_qidx };
        let step = (cx / target).max(4.0);
        let mut q = (0..=255u8).min_by_key(|&q| ((AC_QLOOKUP[q as usize] as f64 - step).abs() * 16.0) as i64).unwrap_or(self.base_qidx);
        if !key && let Some(last) = self.rc.last_inter_q {
            q = q.clamp(last.saturating_sub(24), last.saturating_add(24));
        }
        q.max(1)
    }

    fn rc_update(&mut self, key: bool, qidx: u8, bytes: usize) {
        let Some(kbps) = self.cfg.target_kbps else { return };
        let bits = bytes as f64 * 8.0;
        let cx = bits * AC_QLOOKUP[qidx as usize] as f64;
        if key {
            self.rc.key_complexity = Some(cx);
        } else {
            self.rc.inter_complexity = Some(match self.rc.inter_complexity {
                Some(c) => c * 0.5 + cx * 0.5,
                None => cx,
            });
            self.rc.last_inter_q = Some(qidx);
        }
        let per_frame = kbps as f64 * 1000.0 / self.cfg.frame_rate.max(1.0);
        self.rc.debt += bits - per_frame;
    }

    /// Encode one frame; `force_key` starts a new key frame.
    pub fn encode_frame(&mut self, y: &[u8], u: &[u8], v: &[u8], force_key: bool) -> EncodedFrame {
        let (w, h) = (self.cfg.width.max(1) as usize, self.cfg.height.max(1) as usize);
        let key = self.is_key_due(force_key);
        let qidx = self.pick_qidx(key);
        let lossless = qidx == 0;
        let mi_cols = w.div_ceil(8);
        let mi_rows = h.div_ceil(8);
        let sb_cols = mi_cols.div_ceil(8);
        let sb_rows = mi_rows.div_ceil(8);
        let (pw, ph) = (sb_cols * 64, sb_rows * 64);
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        let mut planes = [
            Plane::new(y, w, h, pw, ph, mi_cols * 8 - 1, mi_rows * 8 - 1),
            Plane::new(u, cw, ch, pw / 2, ph / 2, mi_cols * 4 - 1, mi_rows * 4 - 1),
            Plane::new(v, cw, ch, pw / 2, ph / 2, mi_cols * 4 - 1, mi_rows * 4 - 1),
        ];
        // Tile columns: as few as the width allows (tile_info, 6.2.13).
        let mut min_log2 = 0;
        while (64usize << min_log2) < sb_cols {
            min_log2 += 1;
        }
        let mut max_log2 = 1;
        while (sb_cols >> max_log2) >= 4 {
            max_log2 += 1;
        }
        max_log2 -= 1;
        let tile_cols = 1usize << min_log2;
        let quant = Quant { dcq: DC_QLOOKUP[qidx as usize], acq: AC_QLOOKUP[qidx as usize], lossless, round_dc: 0.5, round_ac: 0.38 };
        let step = AC_QLOOKUP[qidx as usize] as f64 / 8.0;
        let lambda_sse = 0.35 * step * step;
        let refs = if key {
            None
        } else {
            self.reference.as_ref().map(|r| std::array::from_fn::<_, 3, _>(|i| mc::RefPlane { data: &r[i].rec, stride: r[i].w, w: r[i].vis_w, h: r[i].vis_h }))
        };
        let prev = (!key && self.mvs.len() == mi_cols * mi_rows).then_some(&self.mvs[..]);
        let phases = refs.as_ref().map(|r| mc::PhasePlanes::new(&r[0], 48));
        let fp = tile::FrameParams { key, quant, mi_cols, mi_rows, refs, phases, prev_mvs: prev, lambda_sad: lambda_sse.sqrt(), lambda_sse };
        let mut mi = vec![Mi::default(); mi_cols * mi_rows];
        let mut writers = vec![];
        for t in 0..tile_cols {
            let start = (((t * sb_cols) >> min_log2) * 8).min(mi_cols);
            let end = ((((t + 1) * sb_cols) >> min_log2) * 8).min(mi_cols);
            let mut tile = tile::Tile::new(&fp, &mut planes, &mut mi, start, end);
            tile.code();
            writers.push(tile.w);
        }
        // Forward probability updates from this frame's counts.
        let defaults = Probs::defaults();
        let mut counts = vec![[0u32; 2]; probs::NPROBS];
        for wr in &writers {
            wr.count(&mut counts);
        }
        let wanted = probs::plan_updates(&defaults, &counts, !key);
        let (compressed, used) = probs::compressed_header(&defaults, &wanted, lossless, !key);
        let tiles: Vec<Vec<u8>> = writers.iter().map(|wr| wr.emit(&used)).collect();
        drop(fp);
        // Loop filter: the level with the least error against the source.
        let level = if lossless || !self.cfg.loop_filter { 0 } else { choose_level(&mut planes, &mi, mi_rows, mi_cols, qidx, key) };
        lf::filter_frame(&mut planes, &mi, mi_rows, mi_cols, level);
        // Uncompressed header.
        let mut bw = BitWriter::default();
        bw.put(2, 2); // frame_marker
        bw.put(0, 1); // profile low bit
        bw.put(0, 1); // profile high bit
        bw.put(0, 1); // show_existing_frame
        bw.put(!key as u32, 1); // frame_type
        bw.put(1, 1); // show_frame
        if key {
            bw.put(0, 1); // error_resilient_mode
            bw.put(0x49, 8);
            bw.put(0x83, 8);
            bw.put(0x42, 8);
            bw.put(2, 3); // color_space CS_BT_709
            bw.put(self.cfg.full_range as u32, 1);
            bw.put(w as u32 - 1, 16);
            bw.put(h as u32 - 1, 16);
            bw.put(0, 1); // render_and_frame_size_different
            bw.put(0, 1); // refresh_frame_context
            bw.put(1, 1); // frame_parallel_decoding_mode
        } else {
            bw.put(1, 1); // error_resilient_mode (no reliance on earlier contexts or vectors)
            bw.put(0x01, 8); // refresh_frame_flags: slot 0 becomes the new LAST
            for _ in 0..3 {
                bw.put(0, 3); // ref_frame_idx: slot 0
                bw.put(0, 1); // sign bias
            }
            bw.put(1, 1); // found_ref: the size of LAST
            bw.put(0, 1); // render_and_frame_size_different
            bw.put(0, 1); // allow_high_precision_mv
            bw.put(0, 1); // is_filter_switchable
            bw.put(1, 2); // raw_interpolation_filter → EIGHTTAP (regular)
        }
        bw.put(0, 2); // frame_context_idx
        bw.put(level as u32, 6); // loop_filter_level
        bw.put(0, 3); // sharpness
        bw.put(0, 1); // mode_ref_delta_enabled
        bw.put(qidx as u32, 8);
        bw.put(0, 1); // delta_q_y_dc
        bw.put(0, 1); // delta_q_uv_dc
        bw.put(0, 1); // delta_q_uv_ac
        bw.put(0, 1); // segmentation_enabled
        if min_log2 < max_log2 {
            bw.put(0, 1); // no extra tile columns
        }
        bw.put(0, 1); // tile_rows_log2 = 0
        bw.put(compressed.len() as u32, 16);
        let mut out = bw.finish();
        out.extend_from_slice(&compressed);
        let n = tiles.len();
        for (i, t) in tiles.into_iter().enumerate() {
            if i + 1 < n {
                out.extend_from_slice(&(t.len() as u32).to_be_bytes());
            }
            out.extend_from_slice(&t);
        }
        // A frame mustn't end in a byte that reads as a superframe marker (0b110xxxxx, Annex B):
        // decoders would take its last bytes for a superframe index and reject the frame. The
        // last tile's bool-coded data may end in zero padding (9.2.3), so a zero byte fixes it.
        if out.last().is_some_and(|b| b & 0xe0 == 0xc0) {
            out.push(0);
        }
        // State for the next frame.
        self.mvs = mi.iter().map(|m| if m.inter { m.mv } else { Mv::ZERO }).collect();
        self.reference = Some(planes);
        self.frames += 1;
        self.since_key = if key { 1 } else { self.since_key + 1 };
        self.rc_update(key, qidx, out.len());
        EncodedFrame { data: out, key, q_idx: qidx, filter_level: level }
    }
}

/// Try a few loop filter levels on the luma reconstruction; the one closest to the source
/// (squared error over the visible area) wins (0 when filtering doesn't help).
fn choose_level(planes: &mut [Plane; 3], mi: &[Mi], mi_rows: usize, mi_cols: usize, qidx: u8, key: bool) -> u8 {
    let guess = ((qidx as f64 * if key { 0.25 } else { 0.2 }).round() as u8).clamp(1, 63);
    let sse = |p: &Plane| -> u64 {
        let mut s = 0u64;
        for y in 0..p.vis_h {
            for x in 0..p.vis_w {
                let d = p.src[y * p.w + x] as i64 - p.rec[y * p.w + x] as i64;
                s += (d * d) as u64;
            }
        }
        s
    };
    let mut best = (sse(&planes[0]), 0u8);
    let saved = planes[0].rec.clone();
    for level in [guess / 2, guess, (guess as u16 * 3 / 2).min(63) as u8] {
        if level == 0 {
            continue;
        }
        lf::filter_frame(&mut planes[..1], mi, mi_rows, mi_cols, level);
        let s = sse(&planes[0]);
        planes[0].rec.copy_from_slice(&saved);
        if s < best.0 {
            best = (s, level);
        }
    }
    best.1
}

#[derive(Default)]
struct BitWriter {
    out: Vec<u8>,
    acc: u32,
    n: u32,
}

impl BitWriter {
    fn put(&mut self, v: u32, bits: u32) {
        for i in (0..bits).rev() {
            self.acc = (self.acc << 1) | ((v >> i) & 1);
            self.n += 1;
            if self.n == 8 {
                self.out.push(self.acc as u8);
                self.acc = 0;
                self.n = 0;
            }
        }
    }
    fn finish(mut self) -> Vec<u8> {
        if self.n > 0 {
            self.out.push((self.acc << (8 - self.n)) as u8);
        }
        self.out
    }
}

// ---------------------------------------------------------------- colour conversion

/// BT.709 RGB(A) 8-bit → planar YUV 4:2:0 (limited or full range).
pub fn rgba_to_yuv420(rgba: &[u8], w: u32, h: u32, full_range: bool) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let (w, h) = (w as usize, h as usize);
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let mut y = vec![0u8; w * h];
    let mut u = vec![0u8; cw * ch];
    let mut v = vec![0u8; cw * ch];
    let (ys, yo, cs) = if full_range { (255.0, 0.0, 255.0) } else { (219.0, 16.0, 224.0) };
    let px = |x: usize, yy: usize| -> [f32; 3] {
        let i = (yy.min(h - 1) * w + x.min(w - 1)) * 4;
        [rgba[i] as f32 / 255.0, rgba[i + 1] as f32 / 255.0, rgba[i + 2] as f32 / 255.0]
    };
    let luma = |c: [f32; 3]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
    for yy in 0..h {
        for x in 0..w {
            y[yy * w + x] = (luma(px(x, yy)) * ys + yo).round().clamp(0.0, 255.0) as u8;
        }
    }
    for cy in 0..ch {
        for cx in 0..cw {
            let mut acc = [0.0f32; 3];
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let c = px(cx * 2 + dx, cy * 2 + dy);
                for k in 0..3 {
                    acc[k] += c[k] / 4.0;
                }
            }
            let l = luma(acc);
            let cb = (acc[2] - l) / 1.8556;
            let cr = (acc[0] - l) / 1.5748;
            u[cy * cw + cx] = (cb * cs + 128.0).round().clamp(0.0, 255.0) as u8;
            v[cy * cw + cx] = (cr * cs + 128.0).round().clamp(0.0, 255.0) as u8;
        }
    }
    (y, u, v)
}

/// The alpha channel as a luma plane (full range) with neutral chroma — the second VP9 stream of
/// WebM alpha.
pub fn alpha_to_yuv420(rgba: &[u8], w: u32, h: u32) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let (w, h) = (w as usize, h as usize);
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let y: Vec<u8> = rgba.as_chunks::<4>().0.iter().take(w * h).map(|p| p[3]).collect();
    (y, vec![128; cw * ch], vec![128; cw * ch])
}
