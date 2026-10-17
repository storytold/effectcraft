//! Hardware video decoding (Settings ▸ Import ▸ Enable Hardware Accelerated Decoding).
//!
//! FilmCraft's `filmcraft-platform` puts the operating system's H.264 / HEVC decoders
//! (VideoToolbox on macOS, Media Foundation / DXVA on Windows, NVDEC and VA-API on Linux) in front
//! of FilmCraft's software decoders. All OS FFI stays in that crate; EffectCraft only registers it
//! here and flips the setting through `effectcraft-media`. Registering is harmless: while the
//! setting is off, for streams the hardware can't take, and when no driver is present, the software
//! decoders run, and a hardware decoder that fails mid-stream switches to software.

/// Register the hardware decoders on a background thread: probing the drivers (loading libva or
/// the NVIDIA driver, opening a render node) must not hold up the window. Footage opened before it
/// finishes decodes in software until it is reopened.
pub fn register_in_background() {
    let spawned = std::thread::Builder::new().name("ec-hw-decode".into()).spawn(|| {
        let t0 = std::time::Instant::now();
        let available = filmcraft_platform::register();
        log::info!("hardware video decoding: {available:?} ({:.0} ms)", t0.elapsed().as_secs_f64() * 1e3);
    });
    if let Err(e) = spawned {
        log::warn!("hardware video decoding not registered: {e}");
    }
}
