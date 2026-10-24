//! A VP8 key-frame encoder: the lossy picture of a WebP image or animation frame, written from
//! RFC 6386 (*VP8 Data Format and Decoding Guide*, November 2011) by mirroring its decoding
//! process (see README.md).
//!
//! * Every macroblock is predicted with the best of the four 16×16 luma modes (with the Y2
//!   Walsh–Hadamard block of DCs) or B_PRED (each 4×4 subblock with the best of its ten modes),
//!   and the best of the four 8×8 chroma modes, chosen by rate–distortion cost: squared error
//!   plus λ times the bits, the bits counted from the token and mode probabilities.
//! * Coefficients are quantized from one quality setting (0–100 → the frame's quantizer index);
//!   macroblocks left with nothing to code are skipped.
//! * The coefficient probabilities are updated per frame where the frame's own token counts pay
//!   for the update (§13.4).
//! * The loop filter level is chosen from the quantizer (it shapes only the decoded picture: intra
//!   prediction reads the unfiltered reconstruction, §12).
//!
//! The encoder reconstructs exactly what a decoder reconstructs before loop filtering — the
//! prediction, the 16-bit inverse transforms of §14 — and returns it with the frame.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod bool;
mod encode;
mod predict;
mod tables;
mod transform;
mod yuv;

pub use encode::Plane;

/// Largest width or height: the key frame header stores 14 bits.
pub const MAX_SIDE: u32 = (1 << 14) - 1;

/// What to encode with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vp8Params {
    /// 0 (smallest) to 100 (best).
    pub quality: u8,
    /// The loop filter level (0–63, 0 = off); `None` chooses one from the quantizer.
    pub loop_filter: Option<u8>,
}

impl Default for Vp8Params {
    fn default() -> Self {
        Vp8Params { quality: 90, loop_filter: None }
    }
}

/// An encoded key frame.
#[derive(Clone, Debug)]
pub struct Vp8Frame {
    /// The VP8 frame: frame tag, key frame header, first partition, token partition — what a
    /// WebP `VP8 ` chunk holds.
    pub data: Vec<u8>,
    /// What a decoder reconstructs before loop filtering, at the coded (macroblock-padded) size.
    pub y: Plane,
    pub u: Plane,
    pub v: Plane,
    /// The quantizer index used (0–127).
    pub q_index: u8,
    /// The loop filter level signalled.
    pub loop_filter: u8,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum Vp8Error {
    #[error("VP8 frames are 1 to {MAX_SIDE} pixels a side (this one is {0}×{1})")]
    Size(u32, u32),
    #[error("{0} bytes of RGBA for a {1}×{2} frame")]
    Buffer(usize, u32, u32),
    #[error("the first partition is {0} bytes, over the 19-bit limit")]
    PartitionTooLarge(usize),
}

/// Encodes straight (unpremultiplied) 8-bit RGBA as one VP8 key frame. Alpha is not part of a
/// VP8 frame (a WebP file carries it beside the frame); it weights the colour of the chroma
/// samples, so fully transparent pixels do not bleed into the edges of what is visible.
pub fn encode_rgba(rgba: &[u8], width: u32, height: u32, params: &Vp8Params) -> Result<Vp8Frame, Vp8Error> {
    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
        return Err(Vp8Error::Size(width, height));
    }
    let n = (width as usize).checked_mul(height as usize).and_then(|n| n.checked_mul(4));
    if n != Some(rgba.len()) {
        return Err(Vp8Error::Buffer(rgba.len(), width, height));
    }
    let (y, u, v) = yuv::rgba_to_yuv420(rgba, width, height);
    encode::encode(&y, &u, &v, width, height, params)
}

/// Quality → quantizer index, interpolated between these points. Fitted on rendered motion
/// graphics frames so that a quality gives about the PSNR (within half a decibel) and size the same
/// quality gives in the widely used libwebp encoder; steep at the top, where each step of quality
/// costs the most bytes.
const QUALITY_TO_Q: [(u8, u8); 13] =
    [(0, 105), (10, 76), (20, 66), (30, 58), (40, 47), (50, 42), (60, 36), (70, 31), (80, 22), (85, 15), (90, 7), (95, 2), (100, 0)];

/// The quantizer index for a quality: 100 is index 0 (the finest steps).
pub fn q_index(quality: u8) -> u8 {
    let q = quality.min(100);
    for pair in QUALITY_TO_Q.windows(2) {
        let [(q0, i0), (q1, i1)] = [pair[0], pair[1]];
        if q <= q1 {
            let t = f64::from(q - q0) / f64::from(q1 - q0);
            return (f64::from(i0) + t * (f64::from(i1) - f64::from(i0))).round() as u8;
        }
    }
    0
}
