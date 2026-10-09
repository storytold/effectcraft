//! Footage import for EffectCraft (layer **L3**: it implements `effectcraft_render::FootageSource`).
//!
//! - [`probe`] / [`probe_bytes`]: inspect a file and describe it as an [`effectcraft_project::Footage`]
//!   (video, still, image sequence or audio; size, rate, duration, streams, codec, alpha).
//! - [`MediaPool`]: decodes footage frames on demand into [`effectcraft_raster::Image`]s
//!   (premultiplied f32, sRGB/Rec.709-encoded values, as the compositor expects) with a
//!   memory-budgeted LRU frame cache, one decoder per source, and the decoder kept positioned for
//!   sequential playback. Also audio ([`MediaPool::audio_samples`]) and thumbnails.
//!
//! Video containers (MP4/MOV, Matroska/WebM) and codecs (H.264, HEVC, VP9, AV1, ProRes, DNxHD,
//! MJPEG; AAC, Opus, PCM, MP3/FLAC/Vorbis) come from FilmCraft's pure-Rust crates (git dependency,
//! pinned; `plan/adr/0001`). FilmCraft types never leave this crate. Stills use the `image` crate
//! (PNG, JPEG, GIF, WebP, TIFF, BMP, OpenEXR); Photoshop documents (merged image or one layer) come
//! from `effectcraft-psd` and SVG from `effectcraft-svg` (rasterised at any scale).

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod convert;
pub mod exr_channels;
mod layered;
pub use layered::vector_doc;
mod pool;
mod probe;

pub use pool::{DEFAULT_BUDGET, MediaPool, PoolStats};
pub use probe::{DEFAULT_SEQUENCE_RATE, probe, probe_bytes, probe_model};

/// Errors from probing or decoding footage.
#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    #[error("I/O error: {0}")]
    Io(String),
    #[error("unsupported media: {0}")]
    Unsupported(String),
    #[error("decode error: {0}")]
    Decode(String),
}

pub type Result<T> = std::result::Result<T, MediaError>;

impl From<filmcraft_media::MediaError> for MediaError {
    fn from(e: filmcraft_media::MediaError) -> Self {
        use filmcraft_media::MediaError as F;
        match e {
            F::Unsupported(s) => MediaError::Unsupported(s),
            F::Io(s) | F::Offline(s) => MediaError::Io(s),
            other => MediaError::Decode(other.to_string()),
        }
    }
}

/// File extensions recognised as stills.
pub const STILL_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "tif", "tiff", "bmp", "exr", "psd", "psb", "svg", "pdf", "ai", "eps"];
/// File extensions recognised as audio-only files.
pub const AUDIO_EXTENSIONS: &[&str] = filmcraft_media::AUDIO_EXTENSIONS;
/// File extensions recognised as movies.
pub const VIDEO_EXTENSIONS: &[&str] = &["mp4", "m4v", "mov", "mkv", "webm"];

/// File extensions recognised as 3D models (Advanced 3D model layers).
pub const MODEL_EXTENSIONS: &[&str] = &["gltf", "glb", "obj"];

/// Whether `path` has an extension we can import.
pub fn is_importable(path: &std::path::Path) -> bool {
    let ext = ext_of(path);
    MODEL_EXTENSIONS.contains(&ext.as_str())
        || STILL_EXTENSIONS.contains(&ext.as_str())
        || AUDIO_EXTENSIONS.contains(&ext.as_str())
        || VIDEO_EXTENSIONS.contains(&ext.as_str())
}

pub(crate) fn ext_of(path: &std::path::Path) -> String {
    path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase()
}

pub(crate) fn rate_from_fc(r: filmcraft_time::FrameRate) -> effectcraft_time::FrameRate {
    if r.num > 0 && r.den > 0 { effectcraft_time::FrameRate::new(r.num, r.den) } else { effectcraft_time::FrameRate::FPS_30 }
}
