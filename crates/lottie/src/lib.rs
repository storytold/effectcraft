//! Lottie JSON import and export.
//!
//! Implemented from the public Lottie format description (the lottie-spec 1.0 schema and the
//! LottieFiles format documentation); no player or exporter source was used.
//!
//! **Export** ([`export_comp`]) turns a composition into a Lottie document: nested compositions
//! become precomp assets, still footage becomes embedded base64 image assets, and shape, solid,
//! null, precomp, image and text layers map to their Lottie layer types (text optionally as glyph
//! shapes). Transforms (with parenting, separated position and anchor), keyframes (temporal ease
//! converted from speed/influence to normalised Bezier `i`/`o` tangents, hold keys, spatial
//! `ti`/`to` tangents), shape contents, masks, track mattes, blend modes, time remapping, layer
//! timing and composition markers are exported. Anything Lottie cannot express (most effects,
//! cameras and lights, layer styles, audio, video footage…) is listed in
//! [`ExportResult::warnings`].
//!
//! **Import** ([`import`]) is the inverse mapping into a new composition.
//!
//! Keyframe times in Lottie are frames of the layer's local time, which is Visualab Effects's layer
//! time; layer `ip`/`op`/`st` are composition frames and `sr` is the stretch factor.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod anim;
mod base64;
mod export;
mod import;
mod shapes;
mod text;
pub mod zip;

use effectcraft_color::BlendMode;
use effectcraft_time::{FrameRate, TICKS_PER_SECOND, Tick};

pub use export::{ExportOptions, ExportResult, export_comp, to_dotlottie};
pub use import::{ImportResult, ImportedImage, import};

#[derive(Debug, thiserror::Error)]
pub enum LottieError {
    #[error("not a Lottie document: {0}")]
    Parse(String),
    #[error("{0}")]
    Invalid(String),
}

/// Frame ↔ tick conversion at a frame rate.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Timebase {
    pub rate: FrameRate,
}

impl Timebase {
    /// Frame number (fractional) of a time.
    pub fn frame(&self, t: Tick) -> f64 {
        let f = t.0 as f64 * self.rate.num as f64 / (self.rate.den as f64 * TICKS_PER_SECOND as f64);
        // Keep exact frame numbers clean (no 23.999999).
        if (f - f.round()).abs() < 1e-6 { f.round() } else { f }
    }
    pub fn tick(&self, f: f64) -> Tick {
        Tick((f * TICKS_PER_SECOND as f64 * self.rate.den as f64 / self.rate.num as f64).round() as i64)
    }
}

/// Lottie layer / shape blend modes (`bm`) in order.
const LOTTIE_BLEND: [BlendMode; 18] = [
    BlendMode::Normal,
    BlendMode::Multiply,
    BlendMode::Screen,
    BlendMode::Overlay,
    BlendMode::Darken,
    BlendMode::Lighten,
    BlendMode::ColorDodge,
    BlendMode::ColorBurn,
    BlendMode::HardLight,
    BlendMode::SoftLight,
    BlendMode::Difference,
    BlendMode::Exclusion,
    BlendMode::Hue,
    BlendMode::Saturation,
    BlendMode::Color,
    BlendMode::Luminosity,
    BlendMode::Add,
    BlendMode::HardMix,
];

/// Lottie `bm` of a blend mode (`None` = Lottie has no equivalent).
pub(crate) fn blend_to_lottie(m: BlendMode) -> Option<u32> {
    LOTTIE_BLEND.iter().position(|b| *b == m).map(|i| i as u32).or(match m {
        BlendMode::LinearDodge => Some(16),
        BlendMode::ClassicColorDodge => Some(6),
        BlendMode::ClassicColorBurn => Some(7),
        BlendMode::ClassicDifference => Some(10),
        _ => None,
    })
}

pub(crate) fn blend_from_lottie(bm: u64) -> BlendMode {
    LOTTIE_BLEND.get(bm as usize).copied().unwrap_or_default()
}

#[cfg(test)]
mod tests;
