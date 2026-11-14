//! DPX (SMPTE 268M) and Kodak Cineon stills, the film-scan interchange formats, read with
//! `craft-dpx`; this module maps a frame onto footage and 16-bit pixels.
//!
//! Code values are shown as stored (a log scan looks flat, as in After Effects without a Cineon
//! conversion); the codec names the transfer (`log`, `linear`, …). Y'CbCr elements and transposed
//! images are refused with an error naming them; so is everything `craft-dpx` does not read.

use effectcraft_project::{AlphaMode, Footage};

use crate::{MediaError, Result};

/// File extensions of DPX and Cineon files.
pub(crate) const EXTENSIONS: &[&str] = &["dpx", "cin"];

/// Largest image accepted (8192 × 8192): bigger headers are refused before allocating.
const MAX_PIXELS: u64 = 1 << 26;

fn limits() -> craft_dpx::Limits {
    let mut l = craft_dpx::Limits::default();
    l.max_pixels = MAX_PIXELS;
    l
}

fn error(path: &str, e: craft_dpx::Error) -> MediaError {
    match e {
        craft_dpx::Error::Unsupported(_) => MediaError::Unsupported(format!("{path}: {e}")),
        _ => MediaError::Decode(format!("{path}: {e}")),
    }
}

/// The header of a DPX or Cineon file this module shows (`None` for other formats).
fn info(path: &str, bytes: &[u8]) -> Result<Option<craft_dpx::Info>> {
    if craft_dpx::sniff(bytes).is_none() {
        return Ok(None);
    }
    let info = craft_dpx::probe(bytes, &limits()).map_err(|e| error(path, e))?;
    if info.layout.is_ycbcr() {
        return Err(MediaError::Unsupported(format!("{path}: DPX with Y'CbCr samples is not supported")));
    }
    if info.flips().is_none() {
        return Err(MediaError::Unsupported(format!("{path}: DPX orientation {} (transposed image) is not supported", info.orientation)));
    }
    Ok(Some(info))
}

fn layout_name(info: &craft_dpx::Info) -> &'static str {
    match info.layout {
        craft_dpx::Layout::Luma => "luma",
        craft_dpx::Layout::Rgb => "RGB",
        _ if info.descriptor == 52 => "ABGR",
        _ => "RGBA",
    }
}

/// The transfer characteristic for the codec name, empty when unstated.
fn transfer(info: &craft_dpx::Info) -> &'static str {
    if info.kind == craft_dpx::Kind::Cineon {
        return "log";
    }
    match info.transfer {
        Some(1) => "log (printing density)",
        Some(2) => "linear",
        Some(3) => "log",
        Some(5 | 6) => "Rec. 709",
        Some(13) => "ADX",
        _ => "",
    }
}

/// Footage for a DPX or Cineon file (`None` for other formats).
pub(crate) fn probe(path: &str, bytes: &[u8]) -> Result<Option<Footage>> {
    let Some(h) = info(path, bytes)? else { return Ok(None) };
    let alpha = h.layout.has_alpha();
    let mut f = crate::probe::still_footage(path, h.width, h.height, image::ImageFormat::Png, alpha);
    f.alpha = if alpha { AlphaMode::Straight } else { AlphaMode::Ignore };
    f.pixel_aspect =
        h.pixel_aspect.map(|(x, y)| f64::from(x) / f64::from(y)).filter(|a| (0.1..=10.0).contains(a) && h.kind == craft_dpx::Kind::Dpx).unwrap_or(1.0);
    let name = if h.kind == craft_dpx::Kind::Dpx { "DPX" } else { "Cineon" };
    f.codec = format!("{name} {}-bit {}", h.bits, layout_name(&h));
    let t = transfer(&h);
    if !t.is_empty() {
        f.codec = format!("{}, {t}", f.codec);
    }
    Ok(Some(f))
}

/// The picture of a DPX or Cineon file as 16-bit samples, upright (`None` for other formats).
pub(crate) fn decode(path: &str, bytes: &[u8]) -> Result<Option<image::DynamicImage>> {
    let Some(h) = info(path, bytes)? else { return Ok(None) };
    let (flip_x, flip_y) = h.flips().unwrap_or_default();
    let frame = craft_dpx::decode(bytes, &limits()).map_err(|e| error(path, e))?;
    let (w, ht, nc) = (h.width as usize, h.height as usize, h.layout.components());
    let row_len = w.checked_mul(nc).ok_or_else(|| MediaError::Decode(format!("{path}: image dimensions overflow")))?;
    let mut out = frame.codes;
    if flip_y {
        for y in 0..ht / 2 {
            for x in 0..row_len {
                out.swap(y * row_len + x, (ht - 1 - y) * row_len + x);
            }
        }
    }
    if flip_x {
        for row in out.chunks_exact_mut(row_len) {
            for x in 0..w / 2 {
                let (a, b) = (x * nc, (w - 1 - x) * nc);
                for c in 0..nc {
                    row.swap(a + c, b + c);
                }
            }
        }
    }
    out.iter_mut().for_each(|c| *c = craft_dpx::widen(*c, h.bits));
    let (w32, h32) = (h.width, h.height);
    let img = match h.layout {
        craft_dpx::Layout::Luma => image::ImageBuffer::from_raw(w32, h32, out).map(image::DynamicImage::ImageLuma16),
        craft_dpx::Layout::Rgb => image::ImageBuffer::from_raw(w32, h32, out).map(image::DynamicImage::ImageRgb16),
        _ => image::ImageBuffer::from_raw(w32, h32, out).map(image::DynamicImage::ImageRgba16),
    };
    img.map(Some).ok_or_else(|| MediaError::Decode(format!("{path}: image buffer size mismatch")))
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
        assert!(info("x", b"not a dpx").expect("other").is_none());
        let good = {
            let mut a = dpx_header(4, 4, 50, 10, 1, 0);
            a.extend(std::iter::repeat_n(0x5a, 64));
            a
        };
        assert!(decode("g.dpx", &good).expect("ok").is_some());
        for n in [4, 8, 600, 790, 805, 2047, 2048, 2100] {
            assert!(decode("t.dpx", good.get(..n).expect("cut")).is_err(), "{n}");
        }
        // huge, zero, Y'CbCr, unsupported depth, transposed
        for (w, h, d, bits, p, o) in
            [(u32::MAX, u32::MAX, 50, 10, 1, 0), (0, 4, 50, 10, 1, 0), (4, 4, 102, 10, 1, 0), (4, 4, 50, 32, 0, 0), (4, 4, 50, 10, 1, 5)]
        {
            let mut b = dpx_header(w, h, d, bits, p, o);
            b.extend(std::iter::repeat_n(0, 64));
            assert!(probe("h.dpx", &b).is_err() && decode("h.dpx", &b).is_err(), "{w}x{h} {d} {bits} {p} {o}");
        }
        // 8192 × 8193 is past this module's own cap, which is below craft-dpx's default.
        let mut big = dpx_header(8192, 8193, 6, 8, 0, 0);
        big.extend(std::iter::repeat_n(0, 64));
        assert!(probe("big.dpx", &big).is_err());
        // run-length encoding and offsets past the end are refused
        let mut rle = good.clone();
        rle[806..808].copy_from_slice(&1u16.to_be_bytes());
        assert!(matches!(probe("r.dpx", &rle), Err(MediaError::Unsupported(_))));
        let mut far = good.clone();
        far[808..812].copy_from_slice(&u32::MAX.wrapping_sub(1).to_be_bytes());
        assert!(probe("f.dpx", &far).is_err());
    }
}
