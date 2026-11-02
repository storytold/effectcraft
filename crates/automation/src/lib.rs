//! # effectcraft-automation (L5)
//!
//! Makes Visualab Effects drivable by AI agents. See `docs/agents.md`.
//!
//! * [`McpServer`]: a Model Context Protocol server (JSON-RPC 2.0, newline-delimited over stdio),
//!   hand-rolled on `serde_json`; no async runtime, starts instantly.
//! * [`Backend`]: where tools run. **Headless** owns an in-process [`Session`] (no window);
//!   **bridge** forwards to a running desktop app over its JSON-lines control channel
//!   (`effectcraft --control 9877`, `docs/control-protocol.md`), adding screenshots and UI input.
//! * [`tools`]: the tool catalogue (names, descriptions, JSON schemas) and their implementations,
//!   shared by the MCP server and `effectcraft-cli`'s one-shot subcommands.
//!
//! Layering: depends on `effectcraft-engine` only (never egui); the CLI injects a fully wired
//! session (`effectcraft_host::session()`).

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod autosave;
pub mod backend;
pub mod base64;
pub mod bridge;
pub mod confine;
mod long_job;
pub mod server;
pub mod tools;

pub use backend::{Backend, Frame};
pub use bridge::BridgeClient;
pub use effectcraft_engine::Session;
pub use server::McpServer;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Missing or malformed tool arguments.
    #[error("{0}")]
    BadArgs(String),
    #[error(transparent)]
    Engine(#[from] effectcraft_engine::EngineError),
    /// Transport problem talking to the desktop app.
    #[error("bridge: {0}")]
    Bridge(String),
    /// The desktop app replied with an error.
    #[error("{0}")]
    App(String),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Encode RGBA8 pixels as PNG (fast compression), downscaled so the longest side is at most
/// `max_side` (0 = keep). Fully opaque images are written as RGB to save space.
pub fn encode_png(w: u32, h: u32, rgba: Vec<u8>, max_side: u32) -> Result<Vec<u8>> {
    use image::ImageEncoder;
    use image::codecs::png::{CompressionType, FilterType, PngEncoder};
    let mut img = image::RgbaImage::from_raw(w, h, rgba).ok_or_else(|| Error::Other("bad image buffer".into()))?;
    if max_side > 0 && w.max(h) > max_side {
        let s = max_side as f64 / w.max(h) as f64;
        let (nw, nh) = (((w as f64 * s).round() as u32).max(1), ((h as f64 * s).round() as u32).max(1));
        img = image::imageops::resize(&img, nw, nh, image::imageops::FilterType::Triangle);
    }
    let (w, h) = img.dimensions();
    let mut out = Vec::new();
    let enc = PngEncoder::new_with_quality(&mut out, CompressionType::Fast, FilterType::Adaptive);
    let raw = img.into_raw();
    let r = if raw.as_chunks::<4>().0.iter().all(|p| p[3] == 255) {
        let rgb: Vec<u8> = raw.as_chunks::<4>().0.iter().flat_map(|p| [p[0], p[1], p[2]]).collect();
        enc.write_image(&rgb, w, h, image::ExtendedColorType::Rgb8)
    } else {
        enc.write_image(&raw, w, h, image::ExtendedColorType::Rgba8)
    };
    r.map_err(|e| Error::Other(format!("png: {e}")))?;
    Ok(out)
}

/// Decode any PNG and re-encode it compressed and capped to `max_side`. Returns (png, w, h).
pub fn recompress_png(bytes: &[u8], max_side: u32) -> Result<(Vec<u8>, u32, u32)> {
    let img = image::load_from_memory(bytes).map_err(|e| Error::Other(format!("png: {e}")))?.to_rgba8();
    let (w, h) = img.dimensions();
    let png = encode_png(w, h, img.into_raw(), max_side)?;
    let (w, h) = png_size(&png).unwrap_or((w, h));
    Ok((png, w, h))
}

/// Width and height from a PNG's IHDR.
pub fn png_size(png: &[u8]) -> Option<(u32, u32)> {
    if png.len() < 24 || &png[12..16] != b"IHDR" {
        return None;
    }
    let be = |i: usize| u32::from_be_bytes([png[i], png[i + 1], png[i + 2], png[i + 3]]);
    Some((be(16), be(20)))
}

#[cfg(test)]
mod tests;
