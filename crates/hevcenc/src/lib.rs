//! Clean-room pure-Rust HEVC (H.265) encoder, implemented from ITU-T Rec. H.265 (ISO/IEC 23008-2).
//!
//! Main and Main 10 profiles, 4:2:0, one slice per picture: IDR pictures and single-reference P
//! pictures (no reordering), 64x64 CTUs with rate-distortion coding-unit decisions (8x8 .. 64x64),
//! intra (35 modes, NxN at 8x8), merge / skip and AMVP inter prediction with quarter-sample motion,
//! DCT / DST transforms and CABAC. The deblocking filter is on; SAO is not used.
//!
//! ```
//! use effectcraft_hevcenc::{Encoder, EncoderConfig, Frame};
//! let mut enc = Encoder::new(EncoderConfig::new(64, 48, 30, 1)).unwrap();
//! let (y, c) = (vec![128u16; 64 * 48], vec![128u16; 32 * 24]);
//! let packet = enc.encode(&Frame { y: &y, u: &c, v: &c, y_stride: 64, uv_stride: 32 });
//! assert!(packet.keyframe && !packet.data.is_empty());
//! let _hvcc = enc.hvcc();
//! ```

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod bitstream;
mod cabac;
mod deblock;
mod frame;
mod inter;
mod intra;
mod me;
mod params;
#[rustfmt::skip]
mod tables;
mod transform;

use bitstream::{BitWriter, NAL_IDR_N_LP, NAL_TRAIL_R, nal_unit};
use frame::{FrameCoder, SliceKind};
use params::{LOG2_MAX_POC_LSB, LOG2_MIN_CB, MAX_MERGE_CAND, SeqParams};

/// Profile written in the parameter sets.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Profile {
    /// Main: 8-bit 4:2:0.
    #[default]
    Main,
    /// Main 10: 10-bit 4:2:0.
    Main10,
}

impl Profile {
    pub fn bit_depth(self) -> u8 {
        match self {
            Profile::Main => 8,
            Profile::Main10 => 10,
        }
    }
    fn idc(self) -> u8 {
        match self {
            Profile::Main => 1,
            Profile::Main10 => 2,
        }
    }
}

/// Rate control.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RateControl {
    /// Constant QP (0–51; lower is better).
    ConstantQp(u8),
    /// Average bitrate target in kbit/s (frame-level QP adaptation).
    Bitrate { kbps: u32 },
}

/// Encoder settings.
#[derive(Clone, Debug, PartialEq)]
pub struct EncoderConfig {
    /// Picture size; must be even (4:2:0 cropping is in units of two luma samples).
    pub width: u32,
    pub height: u32,
    /// Frame rate numerator / denominator (VUI timing and rate control).
    pub fps_num: u32,
    pub fps_den: u32,
    pub profile: Profile,
    /// `general_level_idc` (30 × level, e.g. 93 = level 3.1); `None` picks the lowest level that fits.
    pub level_idc: Option<u8>,
    pub rate: RateControl,
    /// Frames between IDR pictures (1 = all intra, 0 = only the first picture).
    pub keyint: u32,
    /// BT.709 limited range is signalled in the VUI unless this is set.
    pub full_range: bool,
}

impl EncoderConfig {
    pub fn new(width: u32, height: u32, fps_num: u32, fps_den: u32) -> Self {
        EncoderConfig {
            width,
            height,
            fps_num,
            fps_den,
            profile: Profile::Main,
            level_idc: None,
            rate: RateControl::ConstantQp(28),
            keyint: 60,
            full_range: false,
        }
    }
}

/// One input picture, 4:2:0, samples at the profile's bit depth (8-bit values 0–255 for Main).
/// Luma is `width` x `height`, chroma `width / 2` x `height / 2`.
#[derive(Clone, Copy, Debug)]
pub struct Frame<'a> {
    pub y: &'a [u16],
    pub u: &'a [u16],
    pub v: &'a [u16],
    pub y_stride: usize,
    pub uv_stride: usize,
}

/// One coded picture (an access unit): NAL units, each prefixed by a 4-byte big-endian length
/// (ISO/IEC 14496-15 sample format; parameter sets are only in [`Encoder::hvcc`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Packet {
    pub data: Vec<u8>,
    /// IDR picture.
    pub keyframe: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    InvalidConfig(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::InvalidConfig(s) => write!(f, "invalid HEVC encoder config: {s}"),
        }
    }
}

impl std::error::Error for Error {}

/// Frame-level rate control for [`RateControl::Bitrate`]: a bits ≈ X / Qstep model per picture type
/// and a buffer that spreads over- / undershoot over the following pictures.
struct RateCtl {
    target: f64,
    /// sum(actual) - sum(target), bits.
    buffer: f64,
    /// Complexity X = bits * Qstep for I and P pictures.
    x: [Option<f64>; 2],
    last_qp: [i32; 2],
    init_qp: i32,
}

fn qstep(qp: i32) -> f64 {
    2f64.powf((qp as f64 - 4.0) / 6.0)
}

impl RateCtl {
    fn new(kbps: u32, cfg: &EncoderConfig) -> Self {
        let fps = cfg.fps_num as f64 / cfg.fps_den.max(1) as f64;
        let target = kbps as f64 * 1000.0 / fps.max(0.001);
        let bpp = target / (cfg.width as f64 * cfg.height as f64);
        let init_qp = (28.0 - 6.0 * (bpp / 0.08).log2()).round().clamp(10.0, 46.0) as i32;
        RateCtl { target, buffer: 0.0, x: [None, None], last_qp: [init_qp - 3, init_qp], init_qp }
    }

    fn qp(&self, idr: bool) -> i32 {
        let t = idr as usize ^ 1; // 0 = I, 1 = P
        let tp = (self.target - self.buffer / 12.0).clamp(0.3 * self.target, 2.0 * self.target);
        let want = if idr { 4.0 * tp } else { tp };
        let x = match (self.x[t], self.x[0]) {
            (Some(x), _) => x,
            (None, Some(xi)) => xi / 5.0,
            (None, None) => return if idr { self.init_qp - 3 } else { self.init_qp },
        };
        let qp = (4.0 + 6.0 * (x / want).log2()).round() as i32;
        qp.clamp(self.last_qp[t] - 6, self.last_qp[t] + 6).clamp(1, 51)
    }

    fn update(&mut self, idr: bool, qp: i32, bits: f64) {
        let t = idr as usize ^ 1;
        let x = bits * qstep(qp);
        self.x[t] = Some(match self.x[t] {
            Some(old) => 0.5 * old + 0.5 * x,
            None => x,
        });
        self.last_qp[t] = qp;
        self.buffer += bits - self.target;
    }
}

/// HEVC encoder. Pictures come out in input order (no reordering: I and P only).
pub struct Encoder {
    cfg: EncoderConfig,
    seq: SeqParams,
    sets: [Vec<u8>; 3],
    frames: u64,
    /// POC of the previous picture (relative to the last IDR).
    poc: u32,
    reference: Option<[Vec<u16>; 3]>,
    recon: Option<[Vec<u16>; 3]>,
    rc: Option<RateCtl>,
}

impl Encoder {
    pub fn new(cfg: EncoderConfig) -> Result<Encoder, Error> {
        if cfg.width == 0 || cfg.height == 0 {
            return Err(Error::InvalidConfig("empty frame".into()));
        }
        if !cfg.width.is_multiple_of(2) || !cfg.height.is_multiple_of(2) {
            return Err(Error::InvalidConfig(format!("{}x{}: 4:2:0 HEVC needs even dimensions", cfg.width, cfg.height)));
        }
        if cfg.width > 16384 || cfg.height > 16384 {
            return Err(Error::InvalidConfig("picture larger than 16384 samples".into()));
        }
        if cfg.fps_num == 0 || cfg.fps_den == 0 {
            return Err(Error::InvalidConfig("zero frame rate".into()));
        }
        match cfg.rate {
            RateControl::ConstantQp(q) if q > 51 => return Err(Error::InvalidConfig(format!("QP {q} > 51"))),
            RateControl::Bitrate { kbps: 0 } => return Err(Error::InvalidConfig("zero bitrate".into())),
            _ => {}
        }
        let min_cb = 1u32 << LOG2_MIN_CB;
        let (coded_width, coded_height) = (cfg.width.div_ceil(min_cb) * min_cb, cfg.height.div_ceil(min_cb) * min_cb);
        let kbps = match cfg.rate {
            RateControl::Bitrate { kbps } => Some(kbps),
            RateControl::ConstantQp(_) => None,
        };
        let level_idc = cfg.level_idc.unwrap_or_else(|| params::choose_level(coded_width, coded_height, cfg.fps_num, cfg.fps_den, kbps));
        let seq = SeqParams {
            coded_width,
            coded_height,
            width: cfg.width,
            height: cfg.height,
            bit_depth: cfg.profile.bit_depth() as u32,
            profile_idc: cfg.profile.idc(),
            level_idc,
            full_range: cfg.full_range,
            fps_num: cfg.fps_num,
            fps_den: cfg.fps_den,
        };
        let sets = [params::vps(&seq), params::sps(&seq), params::pps()];
        let rc = kbps.map(|k| RateCtl::new(k, &cfg));
        Ok(Encoder { cfg, seq, sets, frames: 0, poc: 0, reference: None, recon: None, rc })
    }

    pub fn config(&self) -> &EncoderConfig {
        &self.cfg
    }

    /// VPS, SPS, PPS NAL units (2-byte NAL header, emulation prevention, no start codes or length
    /// prefixes).
    pub fn parameter_sets(&self) -> [Vec<u8>; 3] {
        self.sets.clone()
    }

    /// HEVCDecoderConfigurationRecord (`hvcC` payload) with the parameter sets.
    pub fn hvcc(&self) -> Vec<u8> {
        params::hvcc(&self.seq, &self.sets)
    }

    /// `general_level_idc` written in the parameter sets.
    pub fn level_idc(&self) -> u8 {
        self.seq.level_idc
    }

    /// Reconstruction of the last encoded picture (what a decoder outputs), cropped planes
    /// `[Y, Cb, Cr]` with strides `width` and `width / 2`. For tests.
    #[doc(hidden)]
    pub fn last_reconstruction(&self) -> Option<&[Vec<u16>; 3]> {
        self.recon.as_ref()
    }

    /// Copy the input into coded-size planes, replicating the right / bottom edges.
    fn pad_input(&self, f: &Frame) -> [Vec<u16>; 3] {
        let max = (1u32 << self.seq.bit_depth) - 1;
        let mut out: [Vec<u16>; 3] = Default::default();
        for (c, plane) in out.iter_mut().enumerate() {
            let (src, stride, w, h, cw, ch) = if c == 0 {
                (f.y, f.y_stride, self.seq.width, self.seq.height, self.seq.coded_width, self.seq.coded_height)
            } else {
                let s = if c == 1 { f.u } else { f.v };
                (s, f.uv_stride, self.seq.width / 2, self.seq.height / 2, self.seq.coded_width / 2, self.seq.coded_height / 2)
            };
            let (w, h, cw, ch) = (w as usize, h as usize, cw as usize, ch as usize);
            assert!(stride >= w && src.len() >= (h - 1) * stride + w, "HEVC encoder: plane {c} smaller than {w}x{h} (stride {stride})");
            plane.reserve(cw * ch);
            for y in 0..ch {
                let row = &src[y.min(h - 1) * stride..][..w];
                plane.extend(row.iter().map(|&v| (v as u32).min(max) as u16));
                let last = plane.last().copied().unwrap_or(0);
                plane.extend(std::iter::repeat_n(last, cw - w));
            }
        }
        out
    }

    /// Encodes one picture.
    pub fn encode(&mut self, frame: &Frame) -> Packet {
        let src = self.pad_input(frame);
        let keyint = self.cfg.keyint as u64;
        let idr = self.reference.is_none() || (keyint > 0 && self.frames.is_multiple_of(keyint));
        self.poc = if idr { 0 } else { self.poc + 1 };
        let qp = match (&self.rc, self.cfg.rate) {
            (Some(rc), _) => rc.qp(idr),
            (None, RateControl::ConstantQp(q)) => q as i32,
            (None, RateControl::Bitrate { .. }) => 28,
        };
        let (w, h) = (self.seq.coded_width as usize, self.seq.coded_height as usize);
        let bd = self.seq.bit_depth;
        let kind = if idr { SliceKind::I } else { SliceKind::P };
        let me = if idr { None } else { self.reference.as_ref().map(|r| me::MeRef::new(&r[0], w, h, bd)) };
        let refp = if idr { None } else { self.reference.as_ref() };
        let mut fc = FrameCoder::new(frame::FrameConfig { bit_depth: bd, width: w, height: h, kind, slice_qp: qp }, &src, refp, me.as_ref());

        // slice_segment_header() (7.3.6.1)
        let mut bw = BitWriter::new();
        bw.put(1, 1); // first_slice_segment_in_pic_flag
        if idr {
            bw.put(0, 1); // no_output_of_prior_pics_flag
        }
        bw.ue(0); // slice_pic_parameter_set_id
        bw.ue(if idr { 2 } else { 1 }); // slice_type I / P
        if !idr {
            bw.put(self.poc & ((1 << LOG2_MAX_POC_LSB) - 1), LOG2_MAX_POC_LSB); // slice_pic_order_cnt_lsb
            bw.put(1, 1); // short_term_ref_pic_set_sps_flag (the only SPS set, no index coded)
            bw.put(0, 1); // num_ref_idx_active_override_flag
            bw.ue(5 - MAX_MERGE_CAND as u32); // five_minus_max_num_merge_cand
        }
        bw.se(qp - 26); // slice_qp_delta (init_qp = 26)
        // byte_alignment()
        bw.put(1, 1);
        bw.align_zero();

        let ctx = cabac::init_contexts(qp, if idr { 0 } else { 1 });
        let mut enc = cabac::CabacEncoder::new(bw, ctx);
        fc.encode(&mut enc);
        fc.deblock();
        let rbsp = enc.finish().into_bytes();
        let nal = nal_unit(if idr { NAL_IDR_N_LP } else { NAL_TRAIL_R }, &rbsp);
        let mut data = Vec::with_capacity(nal.len() + 4);
        data.extend_from_slice(&(nal.len() as u32).to_be_bytes());
        data.extend_from_slice(&nal);

        if let Some(rc) = &mut self.rc {
            rc.update(idr, qp, data.len() as f64 * 8.0);
        }
        // output picture: cropped reconstruction
        let rec = std::mem::take(&mut fc.rec);
        let (ow, oh) = (self.seq.width as usize, self.seq.height as usize);
        let crop = |p: &[u16], stride: usize, w: usize, h: usize| -> Vec<u16> { (0..h).flat_map(|y| p[y * stride..y * stride + w].iter().copied()).collect() };
        self.recon = Some([crop(&rec[0], w, ow, oh), crop(&rec[1], w / 2, ow / 2, oh / 2), crop(&rec[2], w / 2, ow / 2, oh / 2)]);
        self.reference = Some(rec);
        self.frames += 1;
        Packet { data, keyframe: idr }
    }
}
