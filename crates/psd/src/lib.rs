//! Photoshop document (PSD / PSB) reader, implemented from Adobe's public *Photoshop File Formats
//! Specification* (see README.md), and a minimal writer ([`mod@write`]) for fixtures.
//!
//! [`Psd::parse`] reads the structure (header, layer records and their additional information,
//! the layer tree) without decoding pixels; [`Psd::composite`] decodes the merged image and
//! [`Psd::layer_pixels`] one layer (with its layer mask applied to alpha) on demand. Pixels come
//! out as straight (unpremultiplied) RGBA `f32` in 0..1, display-encoded (32-bit documents,
//! which are linear, are encoded to sRGB).

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod descriptor;
pub mod engine_data;
pub mod placed;
mod read;
pub mod write;

use std::borrow::Cow;
use std::sync::Arc;

pub use descriptor::{DValue, Descriptor};
use read::Reader;

mod limits;
mod model;
mod parse;
mod pixels;

#[cfg(test)]
use limits::{Budget, decode_pixels};
pub use limits::{Error, Result};
pub use model::{
    Adjustment, ChannelInfo, ColorMode, Layer, LayerMask, LinkedFile, MAX_GROUP_DEPTH, Node, Pixels, Rect, Section, SmartObject, SubPath, TextInfo, VectorMask,
};
pub use parse::is_psd;
pub(crate) use pixels::pack_bits;
#[cfg(test)]
use pixels::{lab_to_rgb, srgb_encode, unpack_bits};

/// A parsed Photoshop document.
#[derive(Clone, Debug)]
pub struct Psd {
    /// PSB (large document format).
    pub psb: bool,
    pub width: u32,
    pub height: u32,
    pub channels: u16,
    pub depth: u16,
    pub mode: ColorMode,
    /// Bottom of the stack first (file order).
    pub layers: Vec<Layer>,
    /// The merged image's first extra channel is transparency.
    pub merged_alpha: bool,
    /// Indexed colour palette (256 R, then G, then B).
    pub palette: Vec<u8>,
    /// Files embedded for smart objects.
    pub linked: Vec<LinkedFile>,
    /// Repairs made while reading: damage that was skipped instead of failing the document
    /// (for example an unreadable layer section, in which case only the merged image is kept).
    pub warnings: Vec<String>,
    data: Arc<[u8]>,
    merged_offset: usize,
}

#[cfg(test)]
mod memory_tests;
#[cfg(test)]
mod tests;

/// Test-only call counters (per thread), to prove what the optimised paths avoid doing.
#[cfg(test)]
pub(crate) mod counters;
