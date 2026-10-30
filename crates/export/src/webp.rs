//! Animated WebP (RFC 9649, "WebP Image Format", the extended file format).
//!
//! The file is a RIFF `WEBP` with a `VP8X` header (animation flag; alpha flag with RGB + Alpha),
//! an `ANIM` chunk (background colour, loop count) and one `ANMF` chunk per frame. Each frame's
//! image is, by the Output Module's compression:
//!
//! * **Lossy** (the default): a `VP8 ` key frame from `effectcraft-vp8enc` at the module's
//!   quality, with an `ALPH` chunk before it when the frame has any transparency (§2.7.1.2): the
//!   alpha plane compressed with the lossless format (a `VP8L` image-stream without its header),
//!   through a prediction filter when that codes smaller, and below alpha quality 100 reduced to
//!   fewer levels first (full transparency and full opacity always stay exact).
//! * **Lossless**: a `VP8L` bitstream from the `image` crate's pure-Rust encoder.
//!
//! Both lossless streams are lifted out of the still file that encoder writes.
//!
//! A frame stores only the area that changed since the frame before it (its corner snapped to
//! the even offsets the format stores), drawn over the canvas without blending, so what it
//! holds replaces what was there, alpha included. A run of identical frames is one frame shown
//! for the run's length, so a hold costs a few bytes. Durations are whole milliseconds, with the
//! rounding error carried so long animations keep their timing (as GIF's centiseconds are).

use std::io::{Seek, SeekFrom, Write};

use effectcraft_project::Comp;
use effectcraft_project::render_queue::Channels;
use image::ExtendedColorType;
use rayon::prelude::*;

use crate::{Cx, ExportError, Report, Result, State, batch_size, io};

/// Largest frame side: `VP8L` stores width − 1 and height − 1 in 14 bits.
pub(crate) const MAX_SIDE: u32 = 1 << 14;
/// Largest lossy frame side: the VP8 key frame header stores the size itself in 14 bits.
pub(crate) const MAX_SIDE_LOSSY: u32 = effectcraft_vp8enc::MAX_SIDE;
/// `ALPH` header bits: compressed with the lossless format; level reduction applied (P = 1); the
/// filter method sits in bits 2–3.
const ALPH_LOSSLESS: u8 = 0x01;
const ALPH_LEVEL_REDUCTION: u8 = 0x10;

/// How frames are compressed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Codec {
    Lossless,
    /// VP8 at `quality` (1–100); alpha beside it at `alpha_quality` (100 = exact).
    Lossy {
        quality: u8,
        alpha_quality: u8,
    },
}

impl Codec {
    /// The largest side a frame can have: 16384 px for VP8L, 16383 px for VP8 (both 14-bit fields).
    fn max_side(self) -> u32 {
        match self {
            Codec::Lossless => MAX_SIDE,
            Codec::Lossy { .. } => MAX_SIDE_LOSSY,
        }
    }
}
/// Largest frame duration in milliseconds (24 bits).
const MAX_DURATION: u32 = (1 << 24) - 1;
/// `ANMF` flags: do not blend (bit 1 set), do not dispose (bit 0 clear).
const NO_BLEND: u8 = 0b10;
/// `VP8X` flags: alpha (`L`) and animation (`A`).
const ALPHA_FLAG: u8 = 0x10;
const ANIMATION_FLAG: u8 = 0x02;

/// An area of the canvas, in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// Render the time span as an animated WebP: frames that repeat are merged and each one stores only the area that changed.
pub(crate) async fn webp(cx: &Cx<'_>, comp: &Comp, w: u32, h: u32, st: &mut State<'_>) -> Result<Report> {
    let codec = if cx.output.webp_lossless {
        Codec::Lossless
    } else {
        Codec::Lossy { quality: cx.output.quality.clamp(1, 100), alpha_quality: cx.output.webp_alpha_quality.clamp(1, 100) }
    };
    let max = codec.max_side();
    if w == 0 || h == 0 || w > max || h > max {
        return Err(ExportError::Unsupported(format!("WebP frames are limited to {max} px a side (this one is {w}×{h})")));
    }
    let rate = cx.settings.rate(comp);
    let mut clock = Clock::new(rate.num as f64, rate.den as f64)?;
    let channels = cx.output.channels;
    let alpha = channels == Channels::Rgba;
    let background = if alpha {
        [0; 4]
    } else {
        let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        // Blue, green, red, alpha.
        [c(comp.background[2]), c(comp.background[1]), c(comp.background[0]), 255]
    };
    let path = cx.place(cx.path, 1);
    let mut file = cx.create(&path)?;
    let mut anim = Anim::start(&mut file, w, h, alpha, background, if cx.output.gif_loop { 0 } else { 1 })?;
    let full = Rect { x: 0, y: 0, w, h };
    let mut prev: Option<Vec<u8>> = None;
    let mut pending: Option<Frame> = None;
    let batch = batch_size();
    let mut i = 0;
    while i < st.total {
        let end = (i + batch).min(st.total);
        let frames: Vec<Vec<u8>> = cx
            .frames(comp, (i..end).collect(), |_, img| {
                let mut px = cx.pixels(&img, comp, channels, w, h);
                clear_transparent(&mut px);
                px
            })
            .await;
        // What changed, frame by frame in order; the changed areas then encode in parallel.
        let mut plan = Vec::with_capacity(frames.len());
        for px in frames {
            let rect = match &prev {
                None => Some(full),
                Some(p) => changed(p, &px, w, h)?,
            };
            let area = match rect {
                Some(r) => Some((r, crop(&px, w, r)?)),
                None => None,
            };
            let corner: [u8; 4] = px.get(0..4).and_then(|c| c.try_into().ok()).ok_or_else(|| ExportError::Encode("empty WebP frame".into()))?;
            plan.push((area, corner, clock.next()));
            prev = Some(px);
        }
        let encoded: Vec<Result<Option<Frame>>> = plan
            .par_iter()
            .map(|(area, _, ms)| match area {
                Some((r, px)) => encode_frame(px, r.w, r.h, codec).map(|data| Some(Frame { rect: *r, data, ms: *ms })),
                None => Ok(None),
            })
            .collect();
        for ((_, corner, ms), frame) in plan.iter().zip(encoded) {
            match (frame?, &mut pending) {
                (Some(f), _) => {
                    if let Some(done) = pending.replace(f) {
                        anim.frame(&done)?;
                    }
                }
                // The same picture again: the frame on screen stays up longer.
                (None, Some(p)) if p.ms.checked_add(*ms).is_some_and(|t| t <= MAX_DURATION) => p.ms += ms,
                // A hold longer than a frame can last (4.6 hours) goes on in a new one-pixel
                // frame that changes nothing.
                (None, _) => {
                    let still = Frame { rect: Rect { x: 0, y: 0, w: 1, h: 1 }, data: encode_frame(corner, 1, 1, codec)?, ms: *ms };
                    if let Some(done) = pending.replace(still) {
                        anim.frame(&done)?;
                    }
                }
            }
        }
        st.advance(end - i)?;
        i = end;
    }
    if let Some(done) = pending {
        anim.frame(&done)?;
    }
    anim.finish()?;
    let bytes = file.finish()?;
    Ok(Report { path, frames: 0, width: w, height: h, seconds: 0.0, bytes, audio: false, log: None, overflow: vec![] })
}

/// One `ANMF` frame: where it goes, its image chunks and how long it shows.
pub(crate) struct Frame {
    pub rect: Rect,
    pub data: Vec<u8>,
    pub ms: u32,
}

/// Frame durations in whole milliseconds, carrying the rounding error: 60 fps gives 17, 17, 16…
pub(crate) struct Clock {
    ms_per_frame: f64,
    clock: f64,
    emitted: u64,
}

impl Clock {
    /// A clock for `num`/`den` frames per second; an error when that rate has no millisecond duration.
    pub(crate) fn new(num: f64, den: f64) -> Result<Clock> {
        let ms_per_frame = 1000.0 * den / num;
        if !ms_per_frame.is_finite() || ms_per_frame <= 0.0 {
            return Err(ExportError::Unsupported(format!("frame rate {num}/{den} can't be timed in milliseconds")));
        }
        Ok(Clock { ms_per_frame, clock: 0.0, emitted: 0 })
    }

    /// The next frame's duration: at least 1 ms (a 0 ms frame is a "default" some players
    /// replace with their own) and at most what a frame can store.
    pub(crate) fn next(&mut self) -> u32 {
        self.clock += self.ms_per_frame;
        let d = (self.clock.round() as u64).saturating_sub(self.emitted).max(1);
        self.emitted = self.emitted.saturating_add(d);
        d.min(MAX_DURATION as u64) as u32
    }
}

/// Fully transparent pixels drop their colour, which nobody sees: they compress better and
/// compare equal however the renderer left them.
pub(crate) fn clear_transparent(px: &mut [u8]) {
    for p in px.chunks_exact_mut(4) {
        if let [r, g, b, 0] = p {
            (*r, *g, *b) = (0, 0, 0);
        }
    }
}

/// The area of `cur` that differs from `prev` (both `w`×`h` RGBA), its corner moved to even
/// coordinates as `ANMF` offsets are stored halved; `None` when they are the same.
pub(crate) fn changed(prev: &[u8], cur: &[u8], w: u32, h: u32) -> Result<Option<Rect>> {
    let row = w as usize * 4;
    let short = || ExportError::Encode("WebP frame is smaller than its size".into());
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
    for y in 0..h {
        let span = y as usize * row..(y as usize + 1) * row;
        let (a, b) = (prev.get(span.clone()).ok_or_else(short)?, cur.get(span).ok_or_else(short)?);
        if a == b {
            continue;
        }
        let mut xs = a.chunks_exact(4).zip(b.chunks_exact(4)).enumerate().filter(|(_, (p, q))| p != q).map(|(x, _)| x as u32);
        let Some(first) = xs.next() else { continue };
        let last = xs.next_back().unwrap_or(first);
        (x0, x1) = (x0.min(first), x1.max(last));
        (y0, y1) = (y0.min(y), y);
    }
    if x0 == u32::MAX {
        return Ok(None);
    }
    let (x, y) = (x0 & !1, y0 & !1);
    Ok(Some(Rect { x, y, w: x1 + 1 - x, h: y1 + 1 - y }))
}

/// The pixels of `r` in a `w`-wide RGBA frame.
pub(crate) fn crop(px: &[u8], w: u32, r: Rect) -> Result<Vec<u8>> {
    let outside = || ExportError::Encode("WebP frame area outside the frame".into());
    let (row, n, x) = (w as usize * 4, r.w as usize * 4, r.x as usize * 4);
    let mut out = Vec::with_capacity(n.saturating_mul(r.h as usize).min(px.len()));
    for y in r.y as usize..r.y.saturating_add(r.h) as usize {
        let start = y.checked_mul(row).and_then(|s| s.checked_add(x)).ok_or_else(outside)?;
        out.extend_from_slice(px.get(start..start.saturating_add(n)).ok_or_else(outside)?);
    }
    Ok(out)
}

/// `px` (`w`×`h` RGBA) as a frame's image chunks, headers and padding included.
pub(crate) fn encode_frame(px: &[u8], w: u32, h: u32, codec: Codec) -> Result<Vec<u8>> {
    let n = (w as usize).checked_mul(h as usize).and_then(|n| n.checked_mul(4));
    let max = codec.max_side();
    if w == 0 || h == 0 || w > max || h > max || n != Some(px.len()) {
        return Err(ExportError::Encode(format!("WebP frame of {w}×{h} with {} bytes", px.len())));
    }
    match codec {
        Codec::Lossless => encode_lossless(px, w, h),
        Codec::Lossy { quality, alpha_quality } => encode_lossy(px, w, h, quality, alpha_quality),
    }
}

/// A `VP8 ` chunk, after an `ALPH` chunk when any pixel is not opaque (§2.7.1.2).
fn encode_lossy(px: &[u8], w: u32, h: u32, quality: u8, alpha_quality: u8) -> Result<Vec<u8>> {
    let frame = effectcraft_vp8enc::encode_rgba(px, w, h, &effectcraft_vp8enc::Vp8Params { quality, loop_filter: None })
        .map_err(|e| ExportError::Encode(e.to_string()))?;
    let mut out = Vec::with_capacity(frame.data.len() + 16);
    if !px.chunks_exact(4).all(|p| p.get(3) == Some(&255)) {
        let mut alpha: Vec<u8> = px.chunks_exact(4).map(|p| p[3]).collect();
        let levels = alpha_levels(alpha_quality);
        reduce_levels(&mut alpha, levels);
        // The lossless format predicts on its own, and a filter's residuals can defeat that as
        // easily as help it, so the filter whose residuals look cheapest is kept only when it
        // actually codes smaller than no filter.
        let best = (1..4u8)
            .map(|m| (m, filter_alpha(&alpha, w as usize, h as usize, m)))
            .min_by(|(_, a), (_, b)| entropy(a).total_cmp(&entropy(b)))
            .ok_or_else(|| ExportError::Encode("no alpha filter".into()))?;
        let plain = alpha_stream(&alpha, w, h)?;
        let filtered = alpha_stream(&best.1, w, h)?;
        let (method, chunk) = if filtered.len() < plain.len() { (best.0, filtered) } else { (0, plain) };
        let header = ALPH_LOSSLESS | (method << 2) | if levels < 256 { ALPH_LEVEL_REDUCTION } else { 0 };
        put_chunk(&mut out, b"ALPH", &[&[header], image_stream(&chunk)?])?;
    }
    put_chunk(&mut out, b"VP8 ", &[&frame.data])?;
    Ok(out)
}

/// An alpha plane (or its filter residuals) as a `VP8L` chunk: grey, as the decoder takes the
/// green channel, so the lossless format's subtract-green transform leaves one channel to code.
fn alpha_stream(alpha: &[u8], w: u32, h: u32) -> Result<Vec<u8>> {
    let grey: Vec<u8> = alpha.iter().flat_map(|&a| [a; 3]).collect();
    let mut still = Vec::new();
    image::codecs::webp::WebPEncoder::new_lossless(&mut still).encode(&grey, w, h, ExtendedColorType::Rgb8).map_err(|e| ExportError::Encode(e.to_string()))?;
    vp8l_chunk(&still)
}

/// Alpha levels kept at an alpha quality: all 256 at 100, fewer below on a logarithmic scale
/// (about 147 at 90, 64 at 75, 16 at 50, 4 at 25).
pub(crate) fn alpha_levels(quality: u8) -> u32 {
    if quality >= 100 { 256 } else { 256f64.powf(f64::from(quality.max(1)) / 100.0).round().clamp(2.0, 256.0) as u32 }
}

/// Alpha rounded to `levels` evenly spaced values; 0 and 255 stay as they are.
pub(crate) fn reduce_levels(alpha: &mut [u8], levels: u32) {
    if !(2..256).contains(&levels) {
        return;
    }
    let n = levels - 1;
    for a in alpha {
        let k = (u32::from(*a) * n + 127) / 255;
        *a = ((k * 255 + n / 2) / n) as u8;
    }
}

/// The alpha plane as filter method `method`'s residuals (RFC 9649 §2.7.1.2): each value less its
/// prediction (none, left, above or gradient), modulo 256, with the edge rules the decoder applies.
pub(crate) fn filter_alpha(alpha: &[u8], w: usize, h: usize, method: u8) -> Vec<u8> {
    if alpha.len() != w * h || method == 0 {
        return alpha.to_vec();
    }
    let at = |x: usize, y: usize| alpha.get(y * w + x).copied().unwrap_or(0);
    let mut out = Vec::with_capacity(alpha.len());
    for y in 0..h {
        for x in 0..w {
            let pred = match (x, y) {
                (0, 0) => 0,
                (0, _) if method != 2 => at(0, y - 1),
                (_, 0) if method != 1 => at(x - 1, 0),
                _ => match method {
                    1 => at(x - 1, y),
                    2 => at(x, y - 1),
                    _ => (i32::from(at(x - 1, y)) + i32::from(at(x, y - 1)) - i32::from(at(x - 1, y - 1))).clamp(0, 255) as u8,
                },
            };
            out.push(at(x, y).wrapping_sub(pred));
        }
    }
    out
}

/// Bits per value the values' histogram promises: how compressible a filter's residuals are.
fn entropy(v: &[u8]) -> f64 {
    let mut hist = [0u32; 256];
    for &b in v {
        hist[usize::from(b)] += 1;
    }
    let n = v.len().max(1) as f64;
    hist.iter()
        .filter(|&&c| c > 0)
        .map(|&c| {
            let p = f64::from(c) / n;
            -p * p.log2()
        })
        .sum()
}

/// The image-stream of a `VP8L` chunk: its data after the 5-byte image header (the signature and
/// the packed width, height, alpha hint and version), which `ALPH` leaves out (RFC 9649 §3.4).
pub(crate) fn image_stream(chunk: &[u8]) -> Result<&[u8]> {
    let size =
        chunk.get(4..8).and_then(|s| <[u8; 4]>::try_from(s).ok()).map(u32::from_le_bytes).ok_or_else(|| ExportError::Encode("short VP8L chunk".into()))?
            as usize;
    if chunk.get(8) != Some(&0x2f) {
        return Err(ExportError::Encode("VP8L chunk without its signature".into()));
    }
    chunk.get(13..8usize.saturating_add(size)).ok_or_else(|| ExportError::Encode("VP8L chunk shorter than its size".into()))
}

/// Appends a chunk made of `parts`, with its header and the pad byte of an odd size.
fn put_chunk(out: &mut Vec<u8>, fourcc: &[u8; 4], parts: &[&[u8]]) -> Result<()> {
    let size: usize = parts.iter().map(|p| p.len()).sum();
    let size32 = u32::try_from(size).map_err(|_| ExportError::Unsupported("a WebP frame is limited to 4 GB".into()))?;
    out.extend_from_slice(fourcc);
    out.extend_from_slice(&size32.to_le_bytes());
    for p in parts {
        out.extend_from_slice(p);
    }
    if size % 2 == 1 {
        out.push(0);
    }
    Ok(())
}

/// A lossless `VP8L` chunk. Opaque frames go to the encoder as RGB, which it compresses better.
fn encode_lossless(px: &[u8], w: u32, h: u32) -> Result<Vec<u8>> {
    let opaque = px.chunks_exact(4).all(|p| p.get(3) == Some(&255));
    let (data, color) = if opaque {
        (px.chunks_exact(4).flat_map(|p| p.iter().take(3).copied()).collect(), ExtendedColorType::Rgb8)
    } else {
        (px.to_vec(), ExtendedColorType::Rgba8)
    };
    let mut still = Vec::new();
    image::codecs::webp::WebPEncoder::new_lossless(&mut still).encode(&data, w, h, color).map_err(|e| ExportError::Encode(e.to_string()))?;
    vp8l_chunk(&still)
}

/// The `VP8L` chunk of a still WebP file (header, data and the pad byte of an odd size).
pub(crate) fn vp8l_chunk(file: &[u8]) -> Result<Vec<u8>> {
    let bad = |what: &str| ExportError::Encode(format!("WebP encoder output: {what}"));
    if file.get(0..4) != Some(b"RIFF") || file.get(8..12) != Some(b"WEBP") {
        return Err(bad("not a RIFF WEBP file"));
    }
    let mut pos = 12usize;
    while let Some([f0, f1, f2, f3, s0, s1, s2, s3]) = file.get(pos..pos.saturating_add(8)).and_then(|h| <[u8; 8]>::try_from(h).ok()) {
        let size = u32::from_le_bytes([s0, s1, s2, s3]) as usize;
        let end = pos.checked_add(8).and_then(|p| p.checked_add(size)).ok_or_else(|| bad("chunk size overflows"))?;
        if [f0, f1, f2, f3] == *b"VP8L" {
            let mut chunk = file.get(pos..end).ok_or_else(|| bad("VP8L chunk runs past the end"))?.to_vec();
            if size % 2 == 1 {
                chunk.push(0);
            }
            return Ok(chunk);
        }
        pos = end.saturating_add(size % 2);
    }
    Err(bad("no VP8L chunk"))
}

/// The container being written: its RIFF size is filled in by [`Anim::finish`].
pub(crate) struct Anim<W: Write + Seek> {
    out: W,
    /// Bytes written so far, the RIFF header included.
    written: u64,
}

/// `v`'s low 24 bits, little-endian, as WebP's 3-byte fields.
fn put24(v: u32) -> [u8; 3] {
    let [a, b, c, _] = v.to_le_bytes();
    [a, b, c]
}

impl<W: Write + Seek> Anim<W> {
    /// The RIFF, `VP8X` and `ANIM` headers for a `w`×`h` canvas. `background` is blue, green,
    /// red, alpha; `loops` 0 plays forever.
    pub(crate) fn start(out: W, w: u32, h: u32, alpha: bool, background: [u8; 4], loops: u16) -> Result<Anim<W>> {
        let mut a = Anim { out, written: 0 };
        let flags = ANIMATION_FLAG | if alpha { ALPHA_FLAG } else { 0 };
        a.put(b"RIFF")?;
        a.put(&0u32.to_le_bytes())?;
        a.put(b"WEBP")?;
        a.put(b"VP8X")?;
        a.put(&10u32.to_le_bytes())?;
        a.put(&[flags, 0, 0, 0])?;
        a.put(&put24(w.saturating_sub(1)))?;
        a.put(&put24(h.saturating_sub(1)))?;
        a.put(b"ANIM")?;
        a.put(&6u32.to_le_bytes())?;
        a.put(&background)?;
        a.put(&loops.to_le_bytes())?;
        Ok(a)
    }

    /// An `ANMF` chunk: the frame's offset (stored halved), size, duration and data, written without blending.
    pub(crate) fn frame(&mut self, f: &Frame) -> Result<()> {
        let size = u32::try_from(f.data.len().saturating_add(16)).map_err(|_| self.too_large())?;
        let r = f.rect;
        self.put(b"ANMF")?;
        self.put(&size.to_le_bytes())?;
        self.put(&put24(r.x / 2))?;
        self.put(&put24(r.y / 2))?;
        self.put(&put24(r.w.saturating_sub(1)))?;
        self.put(&put24(r.h.saturating_sub(1)))?;
        self.put(&put24(f.ms.min(MAX_DURATION)))?;
        self.put(&[NO_BLEND])?;
        self.put(&f.data)
    }

    /// Fill in the RIFF size.
    pub(crate) fn finish(mut self) -> Result<()> {
        let size = u32::try_from(self.written.saturating_sub(8)).map_err(|_| self.too_large())?;
        self.out.seek(SeekFrom::Start(4)).map_err(io)?;
        self.out.write_all(&size.to_le_bytes()).map_err(io)?;
        self.out.seek(SeekFrom::End(0)).map_err(io)?;
        Ok(())
    }

    /// Write `b`, refusing anything that would take the RIFF size past 32 bits.
    fn put(&mut self, b: &[u8]) -> Result<()> {
        let written = self.written.saturating_add(b.len() as u64);
        // The RIFF size (the file less its first 8 bytes) is 32 bits.
        if written.saturating_sub(8) > u32::MAX as u64 {
            return Err(self.too_large());
        }
        self.out.write_all(b).map_err(io)?;
        self.written = written;
        Ok(())
    }

    /// The error for a file past the RIFF size limit.
    fn too_large(&self) -> ExportError {
        ExportError::Unsupported("a WebP file is limited to 4 GB: shorten the time span or lower the resolution".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// A `w`×`h` RGBA buffer of colour `c`.
    fn solid(w: u32, h: u32, c: [u8; 4]) -> Vec<u8> {
        c.repeat((w * h) as usize)
    }

    /// Set the pixel at (`x`, `y`) of a `w`-wide RGBA buffer to `c`.
    fn set(px: &mut [u8], w: u32, x: u32, y: u32, c: [u8; 4]) {
        let i = ((y * w + x) * 4) as usize;
        px[i..i + 4].copy_from_slice(&c);
    }

    /// Rounded durations carry their error, so 60 fps totals 100 ms over six frames.
    #[test]
    fn clock_carries_rounding() {
        let mut c = Clock::new(60.0, 1.0).unwrap();
        let d: Vec<u32> = (0..6).map(|_| c.next()).collect();
        assert_eq!(d.iter().sum::<u32>(), 100);
        assert!(d.iter().all(|&v| v == 16 || v == 17), "{d:?}");
        let mut c = Clock::new(30000.0, 1001.0).unwrap();
        assert_eq!((0..2997).map(|_| c.next() as u64).sum::<u64>(), 100_000, "100 s of 29.97");
        // Faster than a millisecond: every frame still lasts one.
        let mut c = Clock::new(4000.0, 1.0).unwrap();
        assert_eq!((0..4).map(|_| c.next()).collect::<Vec<_>>(), [1, 1, 1, 1]);
        assert!(Clock::new(0.0, 1.0).is_err());
        assert!(Clock::new(f64::NAN, 1.0).is_err());
    }

    /// The changed area is `None` for equal frames and otherwise starts on an even pixel (offsets are stored halved).
    #[test]
    fn changed_area_snaps_to_even_corner() {
        let (w, h) = (8, 6);
        let a = solid(w, h, [10, 20, 30, 255]);
        assert_eq!(changed(&a, &a, w, h).unwrap(), None);
        let mut b = a.clone();
        set(&mut b, w, 3, 3, [255, 0, 0, 255]);
        set(&mut b, w, 5, 4, [255, 0, 0, 255]);
        assert_eq!(changed(&a, &b, w, h).unwrap(), Some(Rect { x: 2, y: 2, w: 4, h: 3 }));
        let mut c = a.clone();
        set(&mut c, w, 0, 0, [0, 0, 0, 255]);
        set(&mut c, w, 7, 5, [0, 0, 0, 255]);
        assert_eq!(changed(&a, &c, w, h).unwrap(), Some(Rect { x: 0, y: 0, w, h }));
    }

    /// Buffers smaller than their size claims are errors.
    #[test]
    fn short_buffers_are_errors_not_panics() {
        assert!(changed(&[0; 8], &[0; 4], 2, 1).is_err());
        assert!(crop(&[0; 8], 2, Rect { x: 0, y: 0, w: 2, h: 2 }).is_err());
        assert!(encode_frame(&[0; 8], 2, 2, Codec::Lossless).is_err());
        assert!(encode_frame(&[0; 8], 2, 2, Codec::Lossy { quality: 90, alpha_quality: 100 }).is_err());
        assert!(encode_frame(&[], 0, 0, Codec::Lossless).is_err());
        assert!(encode_frame(&[0; 4], MAX_SIDE + 1, 1, Codec::Lossless).is_err());
        assert!(encode_frame(&[0; 4], MAX_SIDE_LOSSY + 1, 1, Codec::Lossy { quality: 90, alpha_quality: 100 }).is_err());
        assert!(image_stream(b"VP8L\x09\0\0\0\x2f").is_err());
        assert!(vp8l_chunk(b"RIFF\0\0\0\0WEBPVP8L\xff\xff\xff\x7f").is_err());
        assert!(vp8l_chunk(b"RIFF\0\0\0\0WEBPVP8X\x0a\0\0\0").is_err());
        assert!(vp8l_chunk(b"not webp").is_err());
    }

    /// Fully transparent pixels are cleared to zero; any other alpha keeps its colour.
    #[test]
    fn transparent_pixels_lose_their_colour() {
        let mut px = vec![9, 8, 7, 0, 9, 8, 7, 1];
        clear_transparent(&mut px);
        assert_eq!(px, [0, 0, 0, 0, 9, 8, 7, 1]);
    }

    /// Cropping copies just the rect's pixels, row by row.
    #[test]
    fn crop_takes_the_rect() {
        let w = 4;
        let px: Vec<u8> = (0..4 * 3 * 4).map(|v| v as u8).collect();
        let c = crop(&px, w, Rect { x: 2, y: 1, w: 2, h: 2 }).unwrap();
        assert_eq!(c.len(), 16);
        assert_eq!(&c[0..8], &px[24..32]);
        assert_eq!(&c[8..16], &px[40..48]);
    }

    /// A lossless frame chunk has an even size and decodes back to its pixels.
    #[test]
    fn frame_chunk_is_even_and_decodes() {
        let (w, h) = (5, 3);
        let mut px = solid(w, h, [200, 100, 50, 128]);
        set(&mut px, w, 1, 1, [0, 255, 0, 255]);
        let chunk = encode_frame(&px, w, h, Codec::Lossless).unwrap();
        assert_eq!(&chunk[0..4], b"VP8L");
        assert_eq!(chunk.len() % 2, 0);
        // As a still file again, it decodes to the same pixels.
        let mut file = b"RIFF".to_vec();
        file.extend_from_slice(&(4 + chunk.len() as u32).to_le_bytes());
        file.extend_from_slice(b"WEBP");
        file.extend_from_slice(&chunk);
        let img = image::load_from_memory_with_format(&file, image::ImageFormat::WebP).unwrap().to_rgba8();
        assert_eq!(img.as_raw(), &px);
    }

    /// The chunks of a frame's image data: (fourcc, payload).
    fn chunks(data: &[u8]) -> Vec<([u8; 4], Vec<u8>)> {
        let mut out = Vec::new();
        let mut pos = 0;
        while pos + 8 <= data.len() {
            let size = u32::from_le_bytes(data[pos + 4..pos + 8].try_into().unwrap()) as usize;
            out.push((data[pos..pos + 4].try_into().unwrap(), data[pos + 8..pos + 8 + size].to_vec()));
            pos += 8 + size + size % 2;
        }
        out
    }

    /// As a still WebP file: `VP8X` with the alpha flag, then the frame's chunks.
    fn still(data: &[u8], w: u32, h: u32) -> Vec<u8> {
        let mut body = b"WEBPVP8X".to_vec();
        body.extend_from_slice(&10u32.to_le_bytes());
        body.extend_from_slice(&[ALPHA_FLAG, 0, 0, 0]);
        body.extend_from_slice(&put24(w - 1));
        body.extend_from_slice(&put24(h - 1));
        body.extend_from_slice(data);
        let mut f = b"RIFF".to_vec();
        f.extend_from_slice(&(body.len() as u32).to_le_bytes());
        f.extend_from_slice(&body);
        f
    }

    /// A lossy frame with transparency is an `ALPH` chunk followed by its `VP8 ` chunk.
    #[test]
    fn lossy_frames_carry_their_alpha_beside_them() {
        let (w, h) = (24, 20);
        let mut px = solid(w, h, [40, 80, 200, 255]);
        for y in 0..h {
            for x in 0..w {
                // A soft-edged hole: transparent at the left, half at the middle.
                let a = (x * 255 / (w - 1)) as u8;
                set(&mut px, w, x, y, [40, 80, 200, a]);
            }
        }
        clear_transparent(&mut px);
        let data = encode_frame(&px, w, h, Codec::Lossy { quality: 90, alpha_quality: 100 }).unwrap();
        let c = chunks(&data);
        assert_eq!(c.iter().map(|(f, _)| *f).collect::<Vec<_>>(), [*b"ALPH", *b"VP8 "], "ALPH before the bitstream");
        assert_eq!(c[0].1[0] & 0x03, ALPH_LOSSLESS, "compressed with the lossless format");
        assert_eq!(c[0].1[0] & ALPH_LEVEL_REDUCTION, 0, "exact at alpha quality 100");
        let img = image::load_from_memory_with_format(&still(&data, w, h), image::ImageFormat::WebP).unwrap().to_rgba8();
        for (got, want) in img.as_raw().chunks_exact(4).zip(px.chunks_exact(4)) {
            assert_eq!(got[3], want[3], "alpha is lossless");
            if want[3] == 255 {
                assert!(got.iter().zip(want).take(3).all(|(a, b)| a.abs_diff(*b) <= 12), "{got:?} vs {want:?}");
            }
        }
    }

    /// The decoder's side of §2.7.1.2: residuals back to alpha.
    fn unfilter(res: &[u8], w: usize, h: usize, method: u8) -> Vec<u8> {
        let mut a = vec![0u8; w * h];
        for y in 0..h {
            for x in 0..w {
                let pred = if method == 0 || (x == 0 && y == 0) {
                    0
                } else if x == 0 && (method == 1 || method == 3) {
                    a[(y - 1) * w]
                } else if y == 0 && (method == 2 || method == 3) {
                    a[x - 1]
                } else {
                    match method {
                        1 => a[y * w + x - 1],
                        2 => a[(y - 1) * w + x],
                        _ => (a[y * w + x - 1] as i32 + a[(y - 1) * w + x] as i32 - a[(y - 1) * w + x - 1] as i32).clamp(0, 255) as u8,
                    }
                };
                a[y * w + x] = res[y * w + x].wrapping_add(pred);
            }
        }
        a
    }

    /// Each alpha filter (none, horizontal, vertical, gradient) is undone exactly.
    #[test]
    fn alpha_filters_undo_exactly() {
        let (w, h) = (13, 9);
        let alpha: Vec<u8> = (0..w * h).map(|i| ((i * 37) ^ (i / 3)) as u8).collect();
        for m in 0..4 {
            assert_eq!(unfilter(&filter_alpha(&alpha, w, h, m), w, h, m), alpha, "method {m}");
        }
        // A smooth ramp leaves a horizontal filter almost nothing to code.
        let ramp: Vec<u8> = (0..w * h).map(|i| (i % w * 19) as u8).collect();
        assert!(entropy(&filter_alpha(&ramp, w, h, 1)) < entropy(&ramp));
    }

    /// Fewer alpha levels at lower quality, always keeping 0 and 255.
    #[test]
    fn alpha_levels_keep_the_ends() {
        assert_eq!(alpha_levels(100), 256);
        assert_eq!((alpha_levels(75), alpha_levels(50), alpha_levels(1)), (64, 16, 2));
        let mut a: Vec<u8> = (0..=255).collect();
        reduce_levels(&mut a, 16);
        assert_eq!((a[0], a[255]), (0, 255));
        let mut distinct = a.clone();
        distinct.dedup();
        assert_eq!(distinct.len(), 16);
    }

    /// Alpha reduced to fewer levels decodes to exactly those levels.
    #[test]
    fn reduced_alpha_decodes_to_the_reduced_values() {
        let (w, h) = (24, 20);
        let mut px = solid(w, h, [200, 60, 90, 255]);
        for y in 0..h {
            for x in 0..w {
                set(&mut px, w, x, y, [200, 60, 90, ((x * 11 + y * 7) % 256) as u8]);
            }
        }
        clear_transparent(&mut px);
        let data = encode_frame(&px, w, h, Codec::Lossy { quality: 90, alpha_quality: 50 }).unwrap();
        let c = chunks(&data);
        assert_eq!(c[0].1[0] & ALPH_LEVEL_REDUCTION, ALPH_LEVEL_REDUCTION, "level reduction is signalled");
        let mut want: Vec<u8> = px.chunks_exact(4).map(|p| p[3]).collect();
        reduce_levels(&mut want, alpha_levels(50));
        let img = image::load_from_memory_with_format(&still(&data, w, h), image::ImageFormat::WebP).unwrap().to_rgba8();
        let got: Vec<u8> = img.as_raw().chunks_exact(4).map(|p| p[3]).collect();
        assert_eq!(got, want);
    }

    /// An opaque lossy frame is a `VP8 ` chunk alone.
    #[test]
    fn opaque_lossy_frames_have_no_alpha_chunk() {
        let data = encode_frame(&solid(16, 16, [9, 200, 30, 255]), 16, 16, Codec::Lossy { quality: 75, alpha_quality: 100 }).unwrap();
        assert_eq!(chunks(&data).iter().map(|(f, _)| *f).collect::<Vec<_>>(), [*b"VP8 "]);
    }

    /// `image_stream` finds the bitstream right after the chunk header.
    #[test]
    fn the_image_stream_is_the_chunk_after_its_header() {
        let chunk = encode_frame(&solid(3, 3, [1, 2, 3, 255]), 3, 3, Codec::Lossless).unwrap();
        let stream = image_stream(&chunk).unwrap();
        let size = u32::from_le_bytes(chunk[4..8].try_into().unwrap()) as usize;
        assert_eq!(stream.len(), size - 5);
        assert_eq!(stream, &chunk[13..8 + size]);
    }

    /// The `VP8X` and `ANIM` headers carry the flags, canvas size, background and loop count, and `finish` fills in the RIFF size.
    #[test]
    fn container_headers_and_size() {
        let mut buf = Cursor::new(Vec::new());
        let mut a = Anim::start(&mut buf, 300, 200, true, [1, 2, 3, 4], 0).unwrap();
        let data = encode_frame(&solid(2, 2, [255, 0, 0, 255]), 2, 2, Codec::Lossless).unwrap();
        a.frame(&Frame { rect: Rect { x: 6, y: 4, w: 2, h: 2 }, data: data.clone(), ms: 40 }).unwrap();
        a.finish().unwrap();
        let f = buf.into_inner();
        assert_eq!(&f[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(f[4..8].try_into().unwrap()) as usize, f.len() - 8);
        assert_eq!(&f[8..16], b"WEBPVP8X");
        assert_eq!(f[20], ALPHA_FLAG | ANIMATION_FLAG);
        assert_eq!(&f[24..30], &[43, 1, 0, 199, 0, 0], "canvas 300×200, stored less one");
        assert_eq!(&f[30..38], b"ANIM\x06\0\0\0");
        assert_eq!(&f[38..44], &[1, 2, 3, 4, 0, 0], "background, loop forever");
        assert_eq!(&f[44..48], b"ANMF");
        assert_eq!(u32::from_le_bytes(f[48..52].try_into().unwrap()) as usize, 16 + data.len());
        assert_eq!(&f[52..67], &[3, 0, 0, 2, 0, 0, 1, 0, 0, 1, 0, 0, 40, 0, 0], "x/2, y/2, w−1, h−1, 40 ms");
        assert_eq!(f[67], NO_BLEND);
        assert_eq!(&f[68..], &data[..]);
    }
}
