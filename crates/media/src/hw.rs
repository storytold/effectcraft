//! Hardware video decoding (Settings ▸ Import ▸ Enable Hardware Accelerated Decoding).
//!
//! The decoders themselves are not here: the desktop app registers the operating system's
//! decoders (VideoToolbox, Media Foundation, NVDEC, VA-API) in front of FilmCraft's own through
//! `filmcraft-platform`, the one FilmCraft crate with OS FFI. This crate only flips FilmCraft's
//! process-wide switch and reads its counters, so it stays pure Rust, free of `unsafe` and builds
//! for wasm32 (where nothing is registered and everything decodes in software). Streams a hardware
//! decoder declines, and decoders that fail mid-stream, decode in software.

use effectcraft_render::HardwareDecoding;

/// Let registered hardware decoders take the video decoders created from now on (`true`, the
/// default), or decode everything in software. Returns whether the setting changed.
pub fn set_hardware_decoding(on: bool) -> bool {
    let changed = filmcraft_codecs::hw::hardware_decoding() != on;
    filmcraft_codecs::hw::set_hardware_decoding(on);
    changed
}

/// The setting and the counters so far.
pub fn hardware_decoding() -> HardwareDecoding {
    let s = filmcraft_codecs::hw::hw_stats();
    HardwareDecoding {
        enabled: filmcraft_codecs::hw::hardware_decoding(),
        backend: filmcraft_codecs::hw::hw_backend().map(str::to_string),
        frames: s.frames,
        sessions: s.sessions,
        declined: s.declined,
        fallbacks: s.fallbacks,
    }
}
