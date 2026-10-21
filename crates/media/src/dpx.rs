//! DPX (SMPTE 268M) and Kodak Cineon stills, the film-scan interchange formats, read from the
//! public specifications.
//!
//! Supported: luma, RGB, RGBA and ABGR (DPX) or one / three channels (Cineon), 8-, 10-, 12- and
//! 16-bit unsigned samples, 10-bit filled into 32-bit words with the pad bits low (DPX method A,
//! Cineon "32-bit left justified") or high (method B, "right justified"), 12-bit in 16-bit words,
//! big- and little-endian files, the four unrotated image orientations and the pixel aspect ratio.
//! Code values are shown as stored (a log scan looks flat, as in After Effects without a Cineon
//! conversion); the codec names the transfer (`log`, `linear`, …). Anything else (run-length
//! encoding, signed or float samples, Y'CbCr, packed 10/12-bit, transposed images) is refused
//! with an error naming it. Every field is read with bounds checks.

use effectcraft_project::{AlphaMode, Footage};
use rayon::prelude::*;

use crate::{MediaError, Result};

/// File extensions of DPX and Cineon files.
pub(crate) const EXTENSIONS: &[&str] = &["dpx", "cin"];

/// Largest image accepted (8192 × 8192): bigger headers are refused before allocating.
const MAX_PIXELS: u64 = 1 << 26;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Dpx,
    Cineon,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Layout {
    Luma,
    Rgb,
    Rgba,
    Abgr,
}

impl Layout {
    fn components(self) -> usize {
        match self {
            Layout::Luma => 1,
            Layout::Rgb => 3,
            Layout::Rgba | Layout::Abgr => 4,
        }
    }
    fn name(self) -> &'static str {
        match self {
            Layout::Luma => "luma",
            Layout::Rgb => "RGB",
            Layout::Rgba => "RGBA",
            Layout::Abgr => "ABGR",
        }
    }
}

/// How samples are stored in a line.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Packing {
    /// One byte per sample.
    Bytes8,
    /// One 16-bit word per sample.
    Words16,
    /// Three 10-bit samples per 32-bit word, first in the high bits, two pad bits low.
    Filled10A,
    /// Three 10-bit samples per 32-bit word, two pad bits high.
    Filled10B,
    /// One 12-bit sample per 16-bit word, four pad bits low.
    Filled12A,
    /// One 12-bit sample per 16-bit word, four pad bits high.
    Filled12B,
}

impl Packing {
    fn max(self) -> u32 {
        match self {
            Packing::Bytes8 => 0xff,
            Packing::Words16 => 0xffff,
            Packing::Filled10A | Packing::Filled10B => 0x3ff,
            Packing::Filled12A | Packing::Filled12B => 0xfff,
        }
    }
    fn bits(self) -> u32 {
        match self {
            Packing::Bytes8 => 8,
            Packing::Words16 => 16,
            Packing::Filled10A | Packing::Filled10B => 10,
            Packing::Filled12A | Packing::Filled12B => 12,
        }
    }
    /// Bytes holding `n` samples, padded to a 32-bit word (lines start on word boundaries).
    fn line_bytes(self, n: u64) -> u64 {
        match self {
            Packing::Bytes8 => n.div_ceil(4) * 4,
            Packing::Words16 | Packing::Filled12A | Packing::Filled12B => (n * 2).div_ceil(4) * 4,
            Packing::Filled10A | Packing::Filled10B => n.div_ceil(3) * 4,
        }
    }
}

/// What a header says about the picture.
#[derive(Clone, Debug)]
struct Header {
    kind: Kind,
    big_endian: bool,
    width: u32,
    height: u32,
    layout: Layout,
    packing: Packing,
    /// Byte offset of the first line.
    offset: u64,
    /// Bytes from one line's start to the next.
    stride: u64,
    flip_x: bool,
    flip_y: bool,
    pixel_aspect: f64,
    /// Transfer characteristic for the codec name (`log`, `linear`, …), empty when unstated.
    transfer: &'static str,
}

/// Bounds-checked reads in the file's byte order.
struct Reader<'a> {
    b: &'a [u8],
    big: bool,
}

impl Reader<'_> {
    fn u8(&self, o: usize) -> Option<u8> {
        self.b.get(o).copied()
    }
    fn u16(&self, o: usize) -> Option<u16> {
        let s: [u8; 2] = self.b.get(o..o.checked_add(2)?)?.try_into().ok()?;
        Some(if self.big { u16::from_be_bytes(s) } else { u16::from_le_bytes(s) })
    }
    fn u32(&self, o: usize) -> Option<u32> {
        let s: [u8; 4] = self.b.get(o..o.checked_add(4)?)?.try_into().ok()?;
        Some(if self.big { u32::from_be_bytes(s) } else { u32::from_le_bytes(s) })
    }
}

const UNDEFINED32: u32 = u32::MAX;

/// DPX or Cineon, and the byte order, from the magic number.
fn sniff(bytes: &[u8]) -> Option<(Kind, bool)> {
    match bytes.get(..4)? {
        b"SDPX" => Some((Kind::Dpx, true)),
        b"XPDS" => Some((Kind::Dpx, false)),
        [0x80, 0x2a, 0x5f, 0xd7] => Some((Kind::Cineon, true)),
        [0xd7, 0x5f, 0x2a, 0x80] => Some((Kind::Cineon, false)),
        _ => None,
    }
}

fn unsupported(path: &str, what: impl std::fmt::Display) -> MediaError {
    MediaError::Unsupported(format!("{path}: {what}"))
}

fn truncated(path: &str, kind: Kind) -> MediaError {
    let name = if kind == Kind::Dpx { "DPX" } else { "Cineon" };
    MediaError::Decode(format!("{path}: truncated {name} header"))
}

fn parse(path: &str, bytes: &[u8]) -> Result<Option<Header>> {
    let Some((kind, big_endian)) = sniff(bytes) else { return Ok(None) };
    let r = Reader { b: bytes, big: big_endian };
    let h = match kind {
        Kind::Dpx => parse_dpx(path, &r)?,
        Kind::Cineon => parse_cineon(path, &r)?,
    };
    let pixels = u64::from(h.width) * u64::from(h.height);
    if h.width == 0 || h.height == 0 || pixels > MAX_PIXELS {
        return Err(MediaError::Decode(format!("{path}: image size {} × {} out of range", h.width, h.height)));
    }
    let need = h.offset.saturating_add(h.stride.saturating_mul(u64::from(h.height)));
    if (bytes.len() as u64) < need {
        return Err(MediaError::Decode(format!("{path}: truncated image data ({} of {need} bytes)", bytes.len())));
    }
    Ok(Some(h))
}

fn parse_dpx(path: &str, r: &Reader) -> Result<Header> {
    let t = || truncated(path, Kind::Dpx);
    let image_offset = r.u32(4).ok_or_else(t)?;
    let orientation = r.u16(768).ok_or_else(t)?;
    let elements = r.u16(770).ok_or_else(t)?;
    let width = r.u32(772).ok_or_else(t)?;
    let height = r.u32(776).ok_or_else(t)?;
    // The first image element (the picture; further elements are alternative views).
    let sign = r.u32(780).ok_or_else(t)?;
    let descriptor = r.u8(800).ok_or_else(t)?;
    let transfer = r.u8(801).ok_or_else(t)?;
    let bits = r.u8(803).ok_or_else(t)?;
    let packing = r.u16(804).ok_or_else(t)?;
    let encoding = r.u16(806).ok_or_else(t)?;
    let data_offset = r.u32(808).ok_or_else(t)?;
    let eol_padding = r.u32(812).ok_or_else(t)?;
    if elements == 0 || elements == u16::MAX {
        return Err(unsupported(path, "DPX file without an image element"));
    }
    if sign == 1 {
        return Err(unsupported(path, "DPX with signed samples is not supported"));
    }
    if encoding == 1 {
        return Err(unsupported(path, "run-length encoded DPX is not supported"));
    }
    let layout = match descriptor {
        6 => Layout::Luma,
        50 => Layout::Rgb,
        51 => Layout::Rgba,
        52 => Layout::Abgr,
        100..=103 => return Err(unsupported(path, "DPX with Y'CbCr samples is not supported")),
        d => return Err(unsupported(path, format!("DPX image element descriptor {d} is not supported (luma, RGB, RGBA and ABGR are)"))),
    };
    let packing = match (bits, packing) {
        (8, _) => Packing::Bytes8,
        (16, _) => Packing::Words16,
        (10, 1) => Packing::Filled10A,
        (10, 2) => Packing::Filled10B,
        (12, 1) => Packing::Filled12A,
        (12, 2) => Packing::Filled12B,
        (10 | 12, _) => return Err(unsupported(path, format!("DPX with packed {bits}-bit samples (packing {packing}) is not supported"))),
        (b, _) => return Err(unsupported(path, format!("DPX with {b}-bit samples is not supported (8, 10, 12 and 16 are)"))),
    };
    let (flip_x, flip_y) = match orientation {
        0 | u16::MAX => (false, false),
        1 => (true, false),
        2 => (false, true),
        3 => (true, true),
        o => return Err(unsupported(path, format!("DPX orientation {o} (transposed image) is not supported"))),
    };
    let offset = if data_offset == 0 || data_offset == UNDEFINED32 { image_offset } else { data_offset };
    let eol = if eol_padding == UNDEFINED32 { 0 } else { eol_padding };
    let samples = u64::from(width) * layout.components() as u64;
    // Pixel aspect ratio (orientation header), when stated.
    let pixel_aspect = match (r.u32(1628), r.u32(1632)) {
        (Some(h), Some(v)) if h != 0 && v != 0 && h != UNDEFINED32 && v != UNDEFINED32 => {
            let a = f64::from(h) / f64::from(v);
            if (0.1..=10.0).contains(&a) { a } else { 1.0 }
        }
        _ => 1.0,
    };
    let transfer = match transfer {
        1 => "log (printing density)",
        2 => "linear",
        3 => "log",
        5 | 6 => "Rec. 709",
        13 => "ADX",
        _ => "",
    };
    Ok(Header {
        kind: Kind::Dpx,
        big_endian: r.big,
        width,
        height,
        layout,
        packing,
        offset: u64::from(offset),
        stride: packing.line_bytes(samples) + u64::from(eol),
        flip_x,
        flip_y,
        pixel_aspect,
        transfer,
    })
}

fn parse_cineon(path: &str, r: &Reader) -> Result<Header> {
    let t = || truncated(path, Kind::Cineon);
    let offset = r.u32(4).ok_or_else(t)?;
    let orientation = r.u8(192).ok_or_else(t)?;
    let channels = r.u8(193).ok_or_else(t)?;
    // Channel 0's description; every channel must match it.
    let bits = r.u8(198).ok_or_else(t)?;
    let width = r.u32(200).ok_or_else(t)?;
    let height = r.u32(204).ok_or_else(t)?;
    let interleave = r.u8(680).ok_or_else(t)?;
    let packing = r.u8(681).ok_or_else(t)?;
    let signed = r.u8(682).ok_or_else(t)?;
    let eol_padding = r.u32(684).ok_or_else(t)?;
    let layout = match channels {
        1 => Layout::Luma,
        3 => Layout::Rgb,
        c => return Err(unsupported(path, format!("Cineon with {c} channels is not supported (1 and 3 are)"))),
    };
    for c in 1..usize::from(channels) {
        let base = 196 + 28 * c;
        if r.u8(base + 2) != Some(bits) || r.u32(base + 4) != Some(width) || r.u32(base + 8) != Some(height) {
            return Err(unsupported(path, "Cineon channels of different sizes or depths are not supported"));
        }
    }
    if interleave != 0 {
        return Err(unsupported(path, "Cineon with line or channel interleave is not supported"));
    }
    if signed == 1 {
        return Err(unsupported(path, "Cineon with signed samples is not supported"));
    }
    let packing = match (bits, packing) {
        (8, _) => Packing::Bytes8,
        (16, _) => Packing::Words16,
        (10, 5) => Packing::Filled10A,
        (10, 6) => Packing::Filled10B,
        (10, p) => return Err(unsupported(path, format!("Cineon with 10-bit packing {p} is not supported (32-bit words are)"))),
        (b, _) => return Err(unsupported(path, format!("Cineon with {b}-bit samples is not supported (8, 10 and 16 are)"))),
    };
    let (flip_x, flip_y) = match orientation {
        0 | u8::MAX => (false, false),
        1 => (false, true),
        2 => (true, false),
        3 => (true, true),
        o => return Err(unsupported(path, format!("Cineon orientation {o} (transposed image) is not supported"))),
    };
    let eol = if eol_padding == UNDEFINED32 { 0 } else { eol_padding };
    let samples = u64::from(width) * layout.components() as u64;
    Ok(Header {
        kind: Kind::Cineon,
        big_endian: r.big,
        width,
        height,
        layout,
        packing,
        offset: u64::from(offset),
        stride: packing.line_bytes(samples) + u64::from(eol),
        flip_x,
        flip_y,
        pixel_aspect: 1.0,
        transfer: "log",
    })
}

/// Footage for a DPX or Cineon file (`None` for other formats).
pub(crate) fn probe(path: &str, bytes: &[u8]) -> Result<Option<Footage>> {
    let Some(h) = parse(path, bytes)? else { return Ok(None) };
    let alpha = matches!(h.layout, Layout::Rgba | Layout::Abgr);
    let mut f = crate::probe::still_footage(path, h.width, h.height, image::ImageFormat::Png, alpha);
    f.alpha = if alpha { AlphaMode::Straight } else { AlphaMode::Ignore };
    f.pixel_aspect = h.pixel_aspect;
    let name = if h.kind == Kind::Dpx { "DPX" } else { "Cineon" };
    f.codec = format!("{name} {}-bit {}", h.packing.bits(), h.layout.name());
    if !h.transfer.is_empty() {
        f.codec = format!("{}, {}", f.codec, h.transfer);
    }
    Ok(Some(f))
}

/// The picture of a DPX or Cineon file as 16-bit samples (`None` for other formats).
pub(crate) fn decode(path: &str, bytes: &[u8]) -> Result<Option<image::DynamicImage>> {
    let Some(h) = parse(path, bytes)? else { return Ok(None) };
    let (w, ht) = (h.width as usize, h.height as usize);
    // Output components: luma, RGB or RGBA (ABGR reordered).
    let nc = h.layout.components();
    let row_len = w * nc;
    let mut out = vec![0u16; row_len * ht];
    let max = h.packing.max();
    out.par_chunks_mut(row_len).enumerate().for_each(|(y, row)| {
        let sy = if h.flip_y { ht - 1 - y } else { y };
        let start = h.offset.saturating_add(h.stride.saturating_mul(sy as u64));
        let line = usize::try_from(start).ok().and_then(|s| bytes.get(s..)).unwrap_or(&[]);
        for x in 0..w {
            let sx = if h.flip_x { w - 1 - x } else { x };
            for c in 0..nc {
                // ABGR stores alpha first and colour reversed.
                let sc = if h.layout == Layout::Abgr { 3 - c } else { c };
                let v = sample(line, h.packing, h.big_endian, sx * nc + sc);
                if let Some(o) = row.get_mut(x * nc + c) {
                    *o = ((u64::from(v.min(max)) * 0xffff + u64::from(max) / 2) / u64::from(max)) as u16;
                }
            }
        }
    });
    let (w32, h32) = (h.width, h.height);
    let img = match h.layout {
        Layout::Luma => image::ImageBuffer::from_raw(w32, h32, out).map(image::DynamicImage::ImageLuma16),
        Layout::Rgb => image::ImageBuffer::from_raw(w32, h32, out).map(image::DynamicImage::ImageRgb16),
        Layout::Rgba | Layout::Abgr => image::ImageBuffer::from_raw(w32, h32, out).map(image::DynamicImage::ImageRgba16),
    };
    img.map(Some).ok_or_else(|| MediaError::Decode(format!("{path}: image buffer size mismatch")))
}

/// Sample `i` of a line (0 past its end; the header check makes that unreachable for valid
/// headers).
#[inline]
fn sample(line: &[u8], packing: Packing, big: bool, i: usize) -> u32 {
    let word16 = |o: usize| -> u32 {
        match line.get(o..o + 2) {
            Some(&[a, b]) => u32::from(if big { u16::from_be_bytes([a, b]) } else { u16::from_le_bytes([a, b]) }),
            _ => 0,
        }
    };
    let word32 = |o: usize| -> u32 {
        match line.get(o..o + 4) {
            Some(&[a, b, c, d]) => {
                if big {
                    u32::from_be_bytes([a, b, c, d])
                } else {
                    u32::from_le_bytes([a, b, c, d])
                }
            }
            _ => 0,
        }
    };
    match packing {
        Packing::Bytes8 => line.get(i).copied().map_or(0, u32::from),
        Packing::Words16 => word16(i * 2),
        Packing::Filled12A => word16(i * 2) >> 4,
        Packing::Filled12B => word16(i * 2) & 0xfff,
        Packing::Filled10A => (word32(i / 3 * 4) >> (22 - 10 * (i % 3) as u32)) & 0x3ff,
        Packing::Filled10B => (word32(i / 3 * 4) >> (20 - 10 * (i % 3) as u32)) & 0x3ff,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal big-endian DPX header for `w` × `h` with descriptor, bits and packing.
    fn dpx_header(w: u32, h: u32, descriptor: u8, bits: u8, packing: u16, orientation: u16) -> Vec<u8> {
        let mut b = vec![0u8; 2048];
        b[..4].copy_from_slice(b"SDPX");
        b[4..8].copy_from_slice(&2048u32.to_be_bytes());
        b[768..770].copy_from_slice(&orientation.to_be_bytes());
        b[770..772].copy_from_slice(&1u16.to_be_bytes());
        b[772..776].copy_from_slice(&w.to_be_bytes());
        b[776..780].copy_from_slice(&h.to_be_bytes());
        b[800] = descriptor;
        b[801] = 3;
        b[803] = bits;
        b[804..806].copy_from_slice(&packing.to_be_bytes());
        b
    }

    #[test]
    fn ten_bit_methods_a_and_b_and_orientation() {
        // 2 × 1 RGB, method A: R G B in bits 31..22, 21..12, 11..2.
        let mut a = dpx_header(2, 1, 50, 10, 1, 0);
        for px in [(1023u32, 512u32, 0u32), (0, 1, 1023)] {
            a.extend_from_slice(&((px.0 << 22) | (px.1 << 12) | (px.2 << 2)).to_be_bytes());
        }
        let img = decode("a.dpx", &a).expect("decode").expect("dpx").to_rgb16();
        assert_eq!(img.get_pixel(0, 0).0, [65535, 32800, 0]);
        assert_eq!(img.get_pixel(1, 0).0, [0, 64, 65535]);
        let f = probe("a.dpx", &a).expect("probe").expect("dpx");
        assert_eq!(f.codec, "DPX 10-bit RGB, log");
        // method B: pad bits high
        let mut b = dpx_header(1, 1, 50, 10, 2, 1);
        b.extend_from_slice(&((1023u32 << 20) | (512 << 10)).to_be_bytes());
        let img = decode("b.dpx", &b).expect("decode").expect("dpx").to_rgb16();
        assert_eq!(img.get_pixel(0, 0).0, [65535, 32800, 0]);
        // orientation 2: bottom-to-top lines
        let mut c = dpx_header(1, 2, 6, 8, 0, 2);
        c.extend_from_slice(&[10, 0, 0, 0, 200, 0, 0, 0]);
        let img = decode("c.dpx", &c).expect("decode").expect("dpx").to_luma16();
        assert_eq!((img.get_pixel(0, 0).0[0], img.get_pixel(0, 1).0[0]), (200 * 257, 10 * 257));
    }

    #[test]
    fn hostile_headers_never_panic() {
        assert!(parse("x", b"not a dpx").expect("other").is_none());
        let good = {
            let mut a = dpx_header(4, 4, 50, 10, 1, 0);
            a.extend(std::iter::repeat_n(0x5a, 64));
            a
        };
        assert!(decode("g.dpx", &good).expect("ok").is_some());
        for n in [4, 8, 600, 790, 805, 2047, 2048, 2100] {
            assert!(decode("t.dpx", good.get(..n).expect("cut")).is_err(), "{n}");
        }
        // huge, zero, unsupported
        for (w, h, d, bits, p, o) in
            [(u32::MAX, u32::MAX, 50, 10, 1, 0), (0, 4, 50, 10, 1, 0), (4, 4, 102, 10, 1, 0), (4, 4, 50, 10, 0, 0), (4, 4, 50, 32, 0, 0), (4, 4, 50, 10, 1, 5)]
        {
            let mut b = dpx_header(w, h, d, bits, p, o);
            b.extend(std::iter::repeat_n(0, 64));
            assert!(probe("h.dpx", &b).is_err() && decode("h.dpx", &b).is_err());
        }
        // run-length encoding and offsets past the end are refused
        let mut rle = good.clone();
        rle[806..808].copy_from_slice(&1u16.to_be_bytes());
        assert!(matches!(probe("r.dpx", &rle), Err(MediaError::Unsupported(_))));
        let mut far = good.clone();
        far[808..812].copy_from_slice(&u32::MAX.wrapping_sub(1).to_be_bytes());
        assert!(probe("f.dpx", &far).is_err());
    }
}
