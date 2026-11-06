//! Footage import for EffectCraft (layer **L3**: it implements `effectcraft_render::FootageSource`).
//!
//! - [`probe`] / [`probe_bytes`]: inspect a file and describe it as an [`effectcraft_project::Footage`]
//!   (video, still, image sequence or audio; size, rate, duration, streams, codec, alpha).
//! - [`MediaPool`]: decodes footage frames on demand into [`effectcraft_raster::Image`]s
//!   (premultiplied f32, sRGB/Rec.709-encoded values, as the compositor expects) with a
//!   memory-budgeted LRU frame cache, one decoder per source, and the decoder kept positioned for
//!   sequential playback. Also audio ([`MediaPool::audio_samples`]) and thumbnails.
//!
//! Video containers (MP4/MOV, Matroska/WebM, MXF OP1a / OP-Atom) and codecs (H.264, HEVC, VP9,
//! AV1, ProRes, DNxHD, MPEG-2, MJPEG; AAC, Opus, PCM, MP3/FLAC/Vorbis) come from FilmCraft's pure-Rust crates (git dependency,
//! pinned; `plan/adr/0001`). FilmCraft types never leave this crate. Stills use the `image` crate
//! (PNG, JPEG, GIF, WebP, TIFF, BMP, OpenEXR, Targa, Radiance HDR); Photoshop documents (merged image or one layer) come
//! from `effectcraft-psd` and SVG from `effectcraft-svg` (rasterised at any scale).

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod aac;
mod convert;
pub mod downmix;
mod dpx;
pub mod exr_channels;
mod gif_anim;
mod layered;
pub use layered::vector_doc;
mod pool;
mod probe;
mod raw;
mod stream;
mod wav_markers;
mod webp_anim;

/// Video decoder counters (time in decoders, samples decoded, seeks, cache hits), for benchmarks.
pub use filmcraft_codecs::{GopStats, gop_stats};
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
    /// The read was called off before it finished (see [`effectcraft_render::FootageSource::cancellable`]).
    #[error("cancelled")]
    Cancelled,
}

pub type Result<T> = std::result::Result<T, MediaError>;

impl From<filmcraft_media::MediaError> for MediaError {
    fn from(e: filmcraft_media::MediaError) -> Self {
        use filmcraft_media::MediaError as F;
        match e {
            F::Unsupported(s) => MediaError::Unsupported(s),
            F::Io(s) | F::Offline(s) => MediaError::Io(s),
            F::Cancelled => MediaError::Cancelled,
            other => MediaError::Decode(other.to_string()),
        }
    }
}

/// File extensions recognised as stills.
pub const STILL_EXTENSIONS: &[&str] =
    &["png", "jpg", "jpeg", "gif", "webp", "tif", "tiff", "bmp", "exr", "tga", "hdr", "psd", "psb", "svg", "pdf", "ai", "eps", "arw"];
/// File extensions recognised as audio-only files.
pub const AUDIO_EXTENSIONS: &[&str] = filmcraft_media::AUDIO_EXTENSIONS;
pub use effectcraft_project::VIDEO_EXTENSIONS;

/// File extensions recognised as 3D models (Advanced 3D model layers).
pub const MODEL_EXTENSIONS: &[&str] = &["gltf", "glb", "obj", "stl", "fbx"];

/// Whether `path` has an extension we can import.
pub fn is_importable(path: &std::path::Path) -> bool {
    let ext = ext_of(path);
    MODEL_EXTENSIONS.contains(&ext.as_str())
        || STILL_EXTENSIONS.contains(&ext.as_str())
        || AUDIO_EXTENSIONS.contains(&ext.as_str())
        || VIDEO_EXTENSIONS.contains(&ext.as_str())
        || dpx::EXTENSIONS.contains(&ext.as_str())
}

/// A whole file's bytes, refusing anything that isn't a regular file: a project can name
/// `/dev/zero` or a FIFO as footage, and reading those never ends (#2134).
pub(crate) fn read_regular_file(path: &std::path::Path) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("{} is not a file", path.display())));
    }
    let mut bytes = Vec::new();
    // A sparse file can claim far more than memory: an error, not an abort.
    bytes.try_reserve_exact(usize::try_from(meta.len()).unwrap_or(usize::MAX)).map_err(|e| std::io::Error::new(std::io::ErrorKind::OutOfMemory, e))?;
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// Read a regular file with a strict byte ceiling. Check metadata before opening to reject
/// FIFOs and devices without waiting for a producer; recheck the opened handle against races.
pub(crate) fn read_regular_file_limited(path: &std::path::Path, limit: usize) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let reject_non_file = || std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("{} is not a file", path.display()));
    let reject_too_large = || std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{} exceeds the {limit}-byte limit", path.display()));
    let max = u64::try_from(limit).unwrap_or(u64::MAX);
    let path_meta = std::fs::metadata(path)?;
    if !path_meta.is_file() {
        return Err(reject_non_file());
    }
    if path_meta.len() > max {
        return Err(reject_too_large());
    }
    let file = std::fs::File::open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(reject_non_file());
    }
    if meta.len() > max {
        return Err(reject_too_large());
    }
    let reserve = usize::try_from(meta.len()).unwrap_or(limit).min(limit);
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(reserve).map_err(|e| std::io::Error::new(std::io::ErrorKind::OutOfMemory, e))?;
    file.take(max.saturating_add(1)).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(reject_too_large());
    }
    Ok(bytes)
}
pub(crate) fn ext_of(path: &std::path::Path) -> String {
    path.extension().and_then(std::ffi::OsStr::to_str).unwrap_or("").to_ascii_lowercase()
}

/// The `image` crate format of a still: from its signature, else from its extension (Targa has
/// no signature; a `.hdr` without the Radiance one then fails with the decoder's own error).
pub(crate) fn still_format(path: &str, bytes: &[u8]) -> Option<image::ImageFormat> {
    if let Ok(fmt) = image::guess_format(bytes) {
        return Some(fmt);
    }
    match ext_of(std::path::Path::new(path)).as_str() {
        "tga" => Some(image::ImageFormat::Tga),
        "hdr" => Some(image::ImageFormat::Hdr),
        _ => None,
    }
}

pub(crate) fn rate_from_fc(r: filmcraft_time::FrameRate) -> effectcraft_time::FrameRate {
    if r.num > 0 && r.den > 0 { effectcraft_time::FrameRate::new(r.num, r.den) } else { effectcraft_time::FrameRate::FPS_30 }
}

#[cfg(test)]
mod read_tests {
    /// #2134: a folder (and on Unix a device) named as footage is an error, not an endless read.
    #[test]
    fn only_regular_files_are_read_whole() {
        let dir = std::env::temp_dir();
        assert!(super::read_regular_file(&dir).is_err());
        assert!(crate::probe::probe(&dir).is_err(), "probing a folder fails instead of reading it");
        #[cfg(unix)]
        assert!(super::read_regular_file(std::path::Path::new("/dev/zero")).is_err());
        let f = dir.join(format!("ec-read-regular-{}.bin", std::process::id()));
        std::fs::write(&f, b"abc").unwrap();
        assert_eq!(super::read_regular_file(&f).unwrap(), b"abc");
        assert_eq!(super::read_regular_file_limited(&f, 3).unwrap(), b"abc");
        assert_eq!(super::read_regular_file_limited(&f, 2).unwrap_err().kind(), std::io::ErrorKind::InvalidData);
        let _ = std::fs::remove_file(f);
    }
}
