//! Clean-room pure-Rust AV1 encoder (Main profile, 4:2:0, 8 and 10 bit), implemented from the
//! AV1 Bitstream & Decoding Process Specification v1.0.0 with Errata 1.
//!
//! Key frames and single-reference inter frames (LAST_FRAME = the previous frame), 64x64
//! superblocks split down to 8x8 by rate-distortion decisions, intra modes DC / V / H / Paeth /
//! smooth, full-sample motion search with the spec's motion vector prediction, DCT / ADST
//! transforms up to 32x32, one tile per frame, in-loop filters off. See the crate README.
//!
//! The public API below is the contract `crates/export` codes against; keep it stable.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod bitw;
mod cdf;
mod decide;
mod loopfilter;
mod pred;
mod symbol;
mod tables;
mod transform;
mod writer;

use bitw::{BitWriter, obu};
use decide::{Decider, FrameGeom, RefFrame};
use loopfilter::{LfInfo, loop_filter};
use pred::Plane;
use tables::AC_QLOOKUP;
use writer::TileWriter;

/// Rate control.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RateControl {
    /// Constant quantizer index (`base_q_idx`, 1–255; lower is better).
    ConstantQ(u8),
    /// Average bitrate target in kbit/s (frame-level quantizer adaptation).
    Bitrate { kbps: u32 },
}

/// Encoder settings. Main profile (seq_profile 0), 4:2:0.
#[derive(Clone, Debug, PartialEq)]
pub struct EncoderConfig {
    pub width: u32,
    pub height: u32,
    pub fps_num: u32,
    pub fps_den: u32,
    /// 8 or 10.
    pub bit_depth: u8,
    /// `seq_level_idx` (e.g. 8 = level 4.0); `None` picks the lowest level that fits.
    pub level_idx: Option<u8>,
    pub rate: RateControl,
    /// Frames between key frames (1 = all key frames).
    pub keyint: u32,
    /// BT.709 limited range is signalled unless this is set.
    pub full_range: bool,
}

impl EncoderConfig {
    pub fn new(width: u32, height: u32, fps_num: u32, fps_den: u32) -> Self {
        EncoderConfig { width, height, fps_num, fps_den, bit_depth: 8, level_idx: None, rate: RateControl::ConstantQ(100), keyint: 60, full_range: false }
    }
}

/// One input picture, 4:2:0, samples at the configured bit depth.
#[derive(Clone, Copy, Debug)]
pub struct Frame<'a> {
    pub y: &'a [u16],
    pub u: &'a [u16],
    pub v: &'a [u16],
    pub y_stride: usize,
    pub uv_stride: usize,
}

/// One temporal unit as stored in MP4 / WebM samples: OBUs with `obu_has_size_field` = 1 and
/// no temporal delimiter (key frames start with the sequence header OBU).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Packet {
    pub data: Vec<u8>,
    pub keyframe: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    InvalidConfig(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::InvalidConfig(s) => write!(f, "invalid AV1 encoder config: {s}"),
        }
    }
}

impl std::error::Error for Error {}

const OBU_SEQUENCE_HEADER: u8 = 1;
const OBU_FRAME: u8 = 6;

/// Annex A level limits: (seq_level_idx, MaxPicSize, MaxHSize, MaxVSize, MaxDisplayRate,
/// MaxBitrate (Main tier, Mbit/s)).
const LEVELS: [(u8, u64, u32, u32, u64, u64); 14] = [
    (0, 147_456, 2048, 1152, 4_423_680, 1_500_000),
    (1, 278_784, 2816, 1584, 8_363_520, 3_000_000),
    (4, 665_856, 4352, 2448, 19_975_680, 6_000_000),
    (5, 1_065_024, 5504, 3096, 31_950_720, 10_000_000),
    (8, 2_359_296, 6144, 3456, 70_778_880, 12_000_000),
    (9, 2_359_296, 6144, 3456, 141_557_760, 20_000_000),
    (12, 8_912_896, 8192, 4352, 267_386_880, 30_000_000),
    (13, 8_912_896, 8192, 4352, 534_773_760, 40_000_000),
    (14, 8_912_896, 8192, 4352, 1_069_547_520, 60_000_000),
    (15, 8_912_896, 8192, 4352, 1_069_547_520, 60_000_000),
    (16, 35_651_584, 16384, 8704, 1_069_547_520, 60_000_000),
    (17, 35_651_584, 16384, 8704, 2_139_095_040, 100_000_000),
    (18, 35_651_584, 16384, 8704, 4_278_190_080, 160_000_000),
    (19, 35_651_584, 16384, 8704, 4_278_190_080, 160_000_000),
];

/// The lowest level whose limits fit the stream (31 = no level limits when none does).
fn pick_level(cfg: &EncoderConfig) -> u8 {
    let pic = cfg.width as u64 * cfg.height as u64;
    let rate = pic * cfg.fps_num as u64 / cfg.fps_den.max(1) as u64;
    let bitrate = match cfg.rate {
        RateControl::Bitrate { kbps } => kbps as u64 * 1000,
        RateControl::ConstantQ(_) => 0,
    };
    for &(idx, max_pic, max_h, max_v, max_rate, max_br) in LEVELS.iter() {
        if pic <= max_pic && cfg.width <= max_h && cfg.height <= max_v && rate <= max_rate && bitrate <= max_br {
            return idx;
        }
    }
    31
}

/// Frame-level rate control state for `RateControl::Bitrate`.
struct RateState {
    /// Bits per frame at the target rate.
    frame_bits: f64,
    /// Quantizer index for inter frames (continuous).
    q: f64,
    /// Key frame quantizer index minus the inter frame one.
    key_off: f64,
    /// Quantizer index steps per halving of the frame size, measured from consecutive inter
    /// frames (the size/quantizer curve is much steeper than the default on noisy content).
    slope: f64,
    /// Quantizer index and size (in bits) of the previous inter frame.
    prev_inter: Option<(f64, f64)>,
    /// Accumulated (spent - budget) bits.
    debt: f64,
}

/// Quantizer index steps per halving of the frame size before any measurement.
const RC_SLOPE: f64 = 28.0;
/// Fraction of the quantizer step the size error asks for that is applied per frame.
const RC_GAIN: f64 = 0.8;
/// Frames over which an accumulated deviation from the budget is paid back.
const RC_PAYBACK: f64 = 16.0;

impl RateState {
    /// The nominal bit budget and the starting quantizer index of a key or inter frame. A key
    /// frame gets a larger share, and the inter frames of the group get the rest, so that a whole
    /// group of `keyint` frames adds up to `keyint * frame_bits`.
    fn frame_budget(&self, keyint: u32, key: bool) -> (f64, f64) {
        let keyint = keyint.max(1) as f64;
        let boost = (1.0 + (keyint.clamp(1.0, 60.0) - 1.0).sqrt() * 0.75).min(0.5 * keyint + 0.5);
        if key {
            (self.frame_bits * boost, (self.q + self.key_off).clamp(1.0, 255.0))
        } else if keyint > 1.0 {
            (self.frame_bits * (keyint - boost) / (keyint - 1.0), self.q)
        } else {
            (self.frame_bits, self.q)
        }
    }

    /// Adapts the quantizers to the size of the frame just coded at `q_used` against `target`.
    fn update(&mut self, key: bool, first: bool, q_used: f64, bits: f64, target: f64) {
        let err = (bits / target).log2().clamp(-1.5, 1.5);
        if first && key {
            // four passes ran: the key frame quantizer is known, the inter one follows from the offset
            self.q = (q_used - self.key_off).clamp(1.0, 255.0);
        } else if key {
            self.key_off = q_used - self.q + RC_GAIN * self.slope * err;
        } else {
            if let Some((pq, pbits)) = self.prev_inter {
                let (dq, dr) = (q_used - pq, (pbits / bits).log2());
                if dq.abs() >= 3.0 && dr.abs() >= 0.15 && dq * dr > 0.0 {
                    self.slope = (0.6 * self.slope + 0.4 * (dq / dr)).clamp(3.0, 45.0);
                }
            }
            self.prev_inter = Some((q_used, bits));
            self.q = (q_used + RC_GAIN * self.slope * err).clamp(1.0, 255.0);
        }
    }
}

/// Most passes on the first frame, and how far from the target (octaves, 3 %) its size may be
/// before it is accepted.
const RC_FIRST_PASSES: usize = 4;
const RC_FIRST_TOLERANCE: f64 = 0.04;
/// Quantizer index steps per halving of the size, for the first correction of the first frame
/// (the starting quantizer comes from the bits per sample; the first pass shows how far off it
/// is, and from there on the passes are placed by `next_probe_q`).
const RC_FIRST_SLOPE: f64 = 30.0;
/// The same for a key frame after the first: coded again when its size is more than 0.25 octaves
/// (19 %) off the target.
const RC_KEY_PASSES: usize = 3;
const RC_KEY_TOLERANCE: f64 = 0.25;
/// Octaves of frame size per octave of quantizer step assumed for the first correction of a frame.
const RC_PROBE_SLOPE: f64 = 1.0;
/// Largest change of the quantizer step (octaves) between two passes on one frame.
const RC_PROBE_MAX_STEP: f64 = 2.0;

/// The quantizer index whose step size is closest to `2^x`.
fn q_for_step(bit_depth: u8, x: f64) -> u8 {
    let table = &AC_QLOOKUP[((bit_depth - 8) / 2) as usize];
    let mut best = 1;
    for q in 1..=255usize {
        if ((table[q] as f64).log2() - x).abs() < ((table[best] as f64).log2() - x).abs() {
            best = q;
        }
    }
    best as u8
}

/// The next quantizer to try for a frame, from the `(quantizer, log2(size / target))`
/// pairs coded so far. The size follows the quantizer step (not the index, which is linear in the
/// step only below about 60) in a near log-log line: false position between the closest pair of
/// passes that enclose the target, otherwise a step from the latest one with the slope of the
/// last two. `None` when no untried quantizer lies between the enclosing passes.
fn next_probe_q(bit_depth: u8, tried: &[(u8, f64)]) -> Option<u8> {
    let table = &AC_QLOOKUP[((bit_depth - 8) / 2) as usize];
    let x = |q: u8| (table[q as usize] as f64).log2();
    let over = tried.iter().filter(|t| t.1 > 0.0).max_by_key(|t| t.0);
    let under = tried.iter().filter(|t| t.1 <= 0.0).min_by_key(|t| t.0);
    if let (Some(&(q_lo, e_lo)), Some(&(q_hi, e_hi))) = (over, under) {
        if q_hi - q_lo < 2 {
            return None;
        }
        let next = x(q_lo) + (x(q_hi) - x(q_lo)) * e_lo / (e_lo - e_hi);
        return Some(q_for_step(bit_depth, next).clamp(q_lo + 1, q_hi - 1));
    }
    let &(q, err) = tried.last()?;
    let mut slope = RC_PROBE_SLOPE;
    if let [.., (q0, e0), (q1, e1)] = tried
        && (x(*q1) - x(*q0)).abs() > 0.05
        && (e0 - e1) / (x(*q1) - x(*q0)) > 0.0
    {
        slope = ((e0 - e1) / (x(*q1) - x(*q0))).clamp(0.25, 3.0);
    }
    Some(q_for_step(bit_depth, x(q) + (err / slope).clamp(-RC_PROBE_MAX_STEP, RC_PROBE_MAX_STEP)))
}

/// AV1 encoder. Frames come out in input order (no hidden/reordered frames).
pub struct Encoder {
    cfg: EncoderConfig,
    geom: FrameGeom,
    level: u8,
    seq_obu: Vec<u8>,
    frame_num: u64,
    refr: Option<RefFrame>,
    rc: Option<RateState>,
}

impl Encoder {
    pub fn new(cfg: EncoderConfig) -> Result<Encoder, Error> {
        if cfg.width == 0 || cfg.height == 0 {
            return Err(Error::InvalidConfig("empty frame".into()));
        }
        if cfg.width > 4096 || cfg.height > 4096 {
            return Err(Error::InvalidConfig(format!("{}x{} exceeds the single-tile limit (4096 samples per side)", cfg.width, cfg.height)));
        }
        if cfg.bit_depth != 8 && cfg.bit_depth != 10 {
            return Err(Error::InvalidConfig(format!("bit depth {} (8 or 10 supported)", cfg.bit_depth)));
        }
        if cfg.fps_num == 0 || cfg.fps_den == 0 {
            return Err(Error::InvalidConfig("frame rate".into()));
        }
        let geom = FrameGeom::new(cfg.width as usize, cfg.height as usize);
        if geom.sb_cols * geom.sb_rows > 2304 {
            return Err(Error::InvalidConfig(format!("{}x{} exceeds the single-tile area limit (MAX_TILE_AREA)", cfg.width, cfg.height)));
        }
        if let RateControl::Bitrate { kbps } = cfg.rate
            && kbps == 0
        {
            return Err(Error::InvalidConfig("zero bitrate".into()));
        }
        let level = cfg.level_idx.unwrap_or_else(|| pick_level(&cfg));
        if level > 31 {
            return Err(Error::InvalidConfig(format!("seq_level_idx {level}")));
        }
        let rc = match cfg.rate {
            RateControl::ConstantQ(_) => None,
            RateControl::Bitrate { kbps } => {
                let frame_bits = kbps as f64 * 1000.0 * cfg.fps_den as f64 / cfg.fps_num as f64;
                // initial guess from the bits per sample
                let bpp = frame_bits / (cfg.width as f64 * cfg.height as f64);
                let q = (110.0 - 40.0 * (bpp / 0.1).log2()).clamp(20.0, 230.0);
                Some(RateState { frame_bits, q, key_off: -24.0, slope: RC_SLOPE, prev_inter: None, debt: 0.0 })
            }
        };
        let mut enc = Encoder { cfg, geom, level, seq_obu: Vec::new(), frame_num: 0, refr: None, rc };
        let payload = enc.sequence_header_payload();
        obu(OBU_SEQUENCE_HEADER, &payload, &mut enc.seq_obu);
        Ok(enc)
    }

    pub fn config(&self) -> &EncoderConfig {
        &self.cfg
    }

    /// The sequence header OBU (with size field).
    pub fn sequence_header_obu(&self) -> Vec<u8> {
        self.seq_obu.clone()
    }

    /// AV1CodecConfigurationRecord (`av1C` payload, also the WebM CodecPrivate).
    pub fn av1c(&self) -> Vec<u8> {
        let high = (self.cfg.bit_depth > 8) as u8;
        let mut v = vec![
            0x81,                              // marker (1), version (1)
            self.level & 0x1f,                 // seq_profile 0, seq_level_idx_0
            (high << 6) | (1 << 3) | (1 << 2), // tier 0, high_bitdepth, twelve_bit 0, mono 0, ss_x 1, ss_y 1, csp 0
            0,                                 // no initial_presentation_delay
        ];
        v.extend_from_slice(&self.seq_obu);
        v
    }

    /// sequence_header_obu( ) payload (5.5) including trailing bits.
    fn sequence_header_payload(&self) -> Vec<u8> {
        let c = &self.cfg;
        let mut w = BitWriter::new();
        w.f(3, 0); // seq_profile
        w.f(1, 0); // still_picture
        w.f(1, 0); // reduced_still_picture_header
        w.f(1, 0); // timing_info_present_flag
        w.f(1, 0); // initial_display_delay_present_flag
        w.f(5, 0); // operating_points_cnt_minus_1
        w.f(12, 0); // operating_point_idc[ 0 ]
        w.f(5, self.level as u32); // seq_level_idx[ 0 ]
        if self.level > 7 {
            w.f(1, 0); // seq_tier[ 0 ]
        }
        let wbits = 32 - (c.width - 1).max(1).leading_zeros();
        let hbits = 32 - (c.height - 1).max(1).leading_zeros();
        w.f(4, wbits - 1);
        w.f(4, hbits - 1);
        w.f(wbits, c.width - 1);
        w.f(hbits, c.height - 1);
        w.f(1, 0); // frame_id_numbers_present_flag
        w.f(1, 0); // use_128x128_superblock
        w.f(1, 0); // enable_filter_intra
        w.f(1, 0); // enable_intra_edge_filter
        w.f(1, 0); // enable_interintra_compound
        w.f(1, 0); // enable_masked_compound
        w.f(1, 0); // enable_warped_motion
        w.f(1, 0); // enable_dual_filter
        w.f(1, 0); // enable_order_hint
        w.f(1, 0); // seq_choose_screen_content_tools
        w.f(1, 0); // seq_force_screen_content_tools
        w.f(1, 0); // enable_superres
        w.f(1, 0); // enable_cdef
        w.f(1, 0); // enable_restoration
        // color_config( )
        w.f(1, (c.bit_depth > 8) as u32); // high_bitdepth
        w.f(1, 0); // mono_chrome
        w.f(1, 1); // color_description_present_flag
        w.f(8, 1); // color_primaries: BT.709
        w.f(8, 1); // transfer_characteristics: BT.709
        w.f(8, 1); // matrix_coefficients: BT.709
        w.f(1, c.full_range as u32); // color_range
        w.f(2, 0); // chroma_sample_position: unknown
        w.f(1, 0); // separate_uv_delta_q
        w.f(1, 0); // film_grain_params_present
        w.trailing_bits();
        w.into_bytes()
    }

    /// uncompressed_header( ) (5.9) for a shown KEY_FRAME or an INTER_FRAME referencing slot 0.
    fn frame_header(&self, w: &mut BitWriter, key: bool, qidx: u8, lf_level: u32) {
        w.f(1, 0); // show_existing_frame
        w.f(2, if key { 0 } else { 1 }); // frame_type
        w.f(1, 1); // show_frame
        if !key {
            w.f(1, 0); // error_resilient_mode
        }
        w.f(1, 0); // disable_cdf_update
        w.f(1, 0); // frame_size_override_flag
        if !key {
            w.f(3, 0); // primary_ref_frame: LAST_FRAME's saved CDFs
            w.f(8, 1); // refresh_frame_flags: slot 0 becomes LAST for the next frame
            for _ in 0..7 {
                w.f(3, 0); // ref_frame_idx[ i ]
            }
            w.f(1, 0); // render_and_frame_size_different
            w.f(1, 0); // allow_high_precision_mv
            w.f(1, 0); // is_filter_switchable
            w.f(2, 0); // interpolation_filter = EIGHTTAP
            w.f(1, 0); // is_motion_mode_switchable
        } else {
            w.f(1, 0); // render_and_frame_size_different
        }
        w.f(1, 0); // disable_frame_end_update_cdf
        // tile_info( ): uniform spacing, one tile
        let g = &self.geom;
        let tile_log2 = |blk: usize, target: usize| {
            let mut k = 0;
            while (blk << k) < target {
                k += 1;
            }
            k
        };
        w.f(1, 1); // uniform_tile_spacing_flag
        if tile_log2(1, g.sb_cols.min(64)) > 0 {
            w.f(1, 0); // increment_tile_cols_log2
        }
        if tile_log2(1, g.sb_rows.min(64)) > 0 {
            w.f(1, 0); // increment_tile_rows_log2
        }
        // quantization_params( )
        w.f(8, qidx as u32);
        w.f(1, 0); // DeltaQYDc
        w.f(1, 0); // DeltaQUDc
        w.f(1, 0); // DeltaQUAc
        w.f(1, 0); // using_qmatrix
        w.f(1, 0); // segmentation_enabled
        w.f(1, 0); // delta_q_present (base_q_idx > 0)
        // loop_filter_params( ): the same level for every plane and direction
        w.f(6, lf_level);
        w.f(6, lf_level);
        if lf_level != 0 {
            w.f(6, lf_level);
            w.f(6, lf_level);
        }
        w.f(3, 0); // loop_filter_sharpness
        w.f(1, 0); // loop_filter_delta_enabled
        w.f(1, 0); // tx_mode_select (TX_MODE_LARGEST)
        if !key {
            w.f(1, 0); // reference_select
        }
        w.f(1, 1); // reduced_tx_set
        if !key {
            for _ in 0..7 {
                w.f(1, 0); // is_global
            }
        }
    }

    /// Copies a picture into superblock-padded planes (edge replication).
    fn load_source(&self, frame: &Frame) -> [Plane; 3] {
        let maxv = (1u16 << self.cfg.bit_depth) - 1;
        std::array::from_fn(|p| {
            let (vw, vh) = self.geom.visible(p);
            let (pw, ph) = self.geom.padded(p);
            let (data, stride) = match p {
                0 => (frame.y, frame.y_stride),
                1 => (frame.u, frame.uv_stride),
                _ => (frame.v, frame.uv_stride),
            };
            assert!(stride >= vw && data.len() >= stride * (vh - 1) + vw, "plane {p} is too small for {vw}x{vh}");
            let mut pl = Plane::new(pw, ph);
            for y in 0..ph {
                let sy = y.min(vh - 1);
                let src = &data[sy * stride..sy * stride + vw];
                let row = &mut pl.data[y * pw..y * pw + pw];
                for (x, d) in row.iter_mut().enumerate() {
                    *d = src[x.min(vw - 1)].min(maxv);
                }
            }
            pl
        })
    }

    /// Codes one frame at `qidx`; returns the frame OBU and the reconstruction.
    fn encode_frame(&self, src: &[Plane; 3], key: bool, qidx: u8) -> (Vec<u8>, RefFrame) {
        let g = &self.geom;
        let refr = if key { None } else { self.refr.as_ref() };
        let mut dec = Decider::new(g, self.cfg.bit_depth as u32, qidx, key, src, refr);
        let mut tw = TileWriter::new(g, key, qidx, refr.map(|r| &*r.cdfs));
        for sbr in 0..g.sb_rows {
            tw.start_sb_row();
            for sbc in 0..g.sb_cols {
                let node = dec.superblock(sbr * 16, sbc * 16);
                tw.superblock(sbr * 16, sbc * 16, &node);
            }
        }
        let lf_level = self.pick_filter_level(&dec.rec, &tw.lf_info(), src, qidx);
        loop_filter(&mut dec.rec, &tw.lf_info(), [lf_level; 4], self.cfg.bit_depth as u32, false, 1);
        let (tile, cdfs) = tw.finish();
        let mut w = BitWriter::new();
        self.frame_header(&mut w, key, qidx, lf_level);
        w.byte_align();
        // tile_group_obu( ): a single tile, no tile_start_and_end_present_flag
        let mut payload = w.into_bytes();
        payload.extend_from_slice(&tile);
        let mut out = Vec::with_capacity(payload.len() + 8);
        obu(OBU_FRAME, &payload, &mut out);
        let rf = RefFrame { planes: dec.rec, mvs: dec.mvs, lf_level, cdfs };
        (out, rf)
    }

    /// The loop filter level minimising the luma error (searched around a guess from the
    /// quantizer, on the luma of a subset of the superblock rows).
    fn pick_filter_level(&self, rec: &[Plane; 3], info: &LfInfo, src: &[Plane; 3], qidx: u8) -> u32 {
        let (w, h) = (self.geom.width, self.geom.height);
        let sb_row_step = if self.geom.sb_rows >= 9 { 3 } else { 1 };
        let sse = |pl: &Plane| -> u64 {
            let mut s = 0u64;
            for y in 0..h {
                if !(y >> 6).is_multiple_of(sb_row_step) {
                    continue;
                }
                for (a, b) in pl.row(y)[..w].iter().zip(&src[0].row(y)[..w]) {
                    let d = *a as i64 - *b as i64;
                    s += (d * d) as u64;
                }
            }
            s
        };
        let bd = self.cfg.bit_depth as u32;
        let mut cache: Vec<(u32, u64)> = vec![(0, sse(&rec[0]))];
        // one working copy of the luma plane for all candidate levels
        let mut planes = [rec[0].clone(), Plane::new(0, 0), Plane::new(0, 0)];
        let mut eval = |lvl: u32| -> u64 {
            if let Some(&(_, e)) = cache.iter().find(|(l, _)| *l == lvl) {
                return e;
            }
            planes[0].data.copy_from_slice(&rec[0].data);
            loop_filter(&mut planes, info, [lvl; 4], bd, true, sb_row_step);
            let e = sse(&planes[0]);
            cache.push((lvl, e));
            e
        };
        let guess = ((qidx as u32 * 3) / 16).clamp(2, 50);
        let mut best = (0u32, eval(0));
        let mut step = (guess / 2).max(1);
        let mut center = guess;
        let e = eval(center);
        if e < best.1 {
            best = (center, e);
        }
        while step >= 2 {
            for lvl in [center.saturating_sub(step), (center + step).min(63)] {
                let e = eval(lvl);
                if e < best.1 {
                    best = (lvl, e);
                }
            }
            center = best.0.max(1);
            step /= 2;
        }
        best.0
    }

    /// Encodes one frame.
    pub fn encode(&mut self, frame: &Frame) -> Packet {
        let key = self.frame_num.is_multiple_of(self.cfg.keyint.max(1) as u64);
        let src = self.load_source(frame);
        let (frame_obu, rf) = match self.cfg.rate {
            RateControl::ConstantQ(q) => self.encode_frame(&src, key, q.max(1)),
            RateControl::Bitrate { .. } => match self.rc.take() {
                Some(mut rc) => {
                    let out = self.encode_rate_controlled(&src, key, &mut rc);
                    self.rc = Some(rc);
                    out
                }
                // The state is created with the encoder for bitrate mode; without it, encode at a
                // middle quantizer rather than fail.
                None => self.encode_frame(&src, key, 128),
            },
        };
        self.refr = Some(rf);
        self.frame_num += 1;
        let mut data = Vec::with_capacity(frame_obu.len() + self.seq_obu.len());
        if key {
            data.extend_from_slice(&self.seq_obu);
        }
        data.extend_from_slice(&frame_obu);
        Packet { data, keyframe: key }
    }

    fn encode_rate_controlled(&mut self, src: &[Plane; 3], key: bool, rc: &mut RateState) -> (Vec<u8>, RefFrame) {
        let first = self.frame_num == 0;
        let (budget, q_start) = rc.frame_budget(self.cfg.keyint, key);
        // pay back the accumulated deviation over the next RC_PAYBACK frames
        let target = (budget - rc.debt / RC_PAYBACK).clamp(budget * 0.35, budget * 2.0);
        let mut q = q_start;
        let mut best: Option<(Vec<u8>, RefFrame, f64, f64)> = None;
        // The first frame is coded up to four times to find its quantizer (it fixes the
        // quantizers of everything after it), a later key frame up to three times (it is a large
        // part of the budget of its group). A frame is coded again only while its size is far from
        // the target and an untried quantizer is left to try; other frames are coded once.
        let (max_passes, tolerance) = if first {
            (RC_FIRST_PASSES, RC_FIRST_TOLERANCE)
        } else if key {
            (RC_KEY_PASSES, RC_KEY_TOLERANCE)
        } else {
            (1, 0.0)
        };
        let mut tried: Vec<(u8, f64)> = Vec::new();
        for _ in 0..max_passes {
            let qi = q.round().clamp(1.0, 255.0) as u8;
            if tried.iter().any(|t| t.0 == qi) {
                break;
            }
            let (obu, rf) = self.encode_frame(src, key, qi);
            let bits = obu.len() as f64 * 8.0;
            let err = (bits / target).log2();
            tried.push((qi, err));
            if best.as_ref().is_none_or(|b| err.abs() < (b.2 / target).log2().abs()) {
                best = Some((obu, rf, bits, qi as f64));
            }
            if err.abs() <= tolerance {
                break;
            }
            let next = if first && tried.len() == 1 {
                Some((q + RC_FIRST_SLOPE * err).clamp(1.0, 255.0))
            } else {
                next_probe_q(self.cfg.bit_depth, &tried).map(f64::from)
            };
            match next {
                Some(next) => q = next,
                None => break,
            }
        }
        // At least one pass ran, so `best` is set.
        let Some((obu, rf, bits, q_used)) = best else { return self.encode_frame(src, key, q.round().clamp(1.0, 255.0) as u8) };
        rc.debt += bits - budget;
        rc.update(key, first, q_used, bits, target);
        (obu, rf)
    }

    /// The loop filter level of the last encoded frame. For tests.
    #[doc(hidden)]
    pub fn last_loop_filter_level(&self) -> Option<u32> {
        self.refr.as_ref().map(|r| r.lf_level)
    }

    /// The reconstruction of the last encoded frame (visible area, planes Y, U, V), equal to
    /// what a decoder outputs. For tests.
    #[doc(hidden)]
    pub fn last_reconstruction(&self) -> Option<[Vec<u16>; 3]> {
        let rf = self.refr.as_ref()?;
        Some(std::array::from_fn(|p| {
            let (vw, vh) = self.geom.visible(p);
            let pl = &rf.planes[p];
            let mut v = Vec::with_capacity(vw * vh);
            for y in 0..vh {
                v.extend_from_slice(&pl.row(y)[..vw]);
            }
            v
        }))
    }
}
