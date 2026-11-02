//! Image XObjects and inline images (ISO 32000-1 §8.9): sample decoding (1–16 bits per
//! component, `/Decode` arrays), colour spaces (through [`crate::color`]: Indexed lookups and
//! ICCBased profiles by their `/Alternate` space), DCT (JPEG, through the pure-Rust `zune-jpeg`)
//! and the general filters, stencil masks (`/ImageMask`), soft masks (`/SMask`) and colour-key
//! and explicit masks (`/Mask`). The result is straight RGBA8.

use crate::color::{Cs, color_space};
use crate::object::{Dict, File, Obj, decode_stream};

pub(crate) struct Decoded {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// The longest side of a decoded image (larger images are refused rather than exhausting
/// memory).
const MAX_SIDE: usize = 16384;

fn get<'a>(file: &'a File, d: &'a Dict, long: &str, short: &str) -> Option<&'a Obj> {
    file.get(d, long).or_else(|| file.get(d, short))
}

fn filters(file: &File, d: &Dict) -> Vec<String> {
    match get(file, d, "Filter", "F").map(|f| file.resolve(f)) {
        Some(Obj::Name(n)) => vec![n.clone()],
        Some(Obj::Array(a)) => a.iter().filter_map(|f| file.resolve(f).name().map(str::to_string)).collect(),
        _ => vec![],
    }
}

/// Inline-image abbreviations (§8.9.7) expanded to the full keys and names.
pub(crate) fn expand_inline(d: &Dict) -> Dict {
    let mut out = Dict::new();
    for (k, v) in d {
        let key = match k.as_str() {
            "BPC" => "BitsPerComponent",
            "CS" => "ColorSpace",
            "D" => "Decode",
            "DP" => "DecodeParms",
            "F" => "Filter",
            "H" => "Height",
            "IM" => "ImageMask",
            "I" => "Interpolate",
            "W" => "Width",
            other => other,
        };
        let name = |n: &str| -> String {
            match n {
                "G" => "DeviceGray",
                "RGB" => "DeviceRGB",
                "CMYK" => "DeviceCMYK",
                "I" => "Indexed",
                "AHx" => "ASCIIHexDecode",
                "A85" => "ASCII85Decode",
                "LZW" => "LZWDecode",
                "Fl" => "FlateDecode",
                "RL" => "RunLengthDecode",
                "CCF" => "CCITTFaxDecode",
                "DCT" => "DCTDecode",
                other => other,
            }
            .to_string()
        };
        let v = match v {
            Obj::Name(n) => Obj::Name(name(n)),
            Obj::Array(a) => Obj::Array(a.iter().map(|x| if let Obj::Name(n) = x { Obj::Name(name(n)) } else { x.clone() }).collect()),
            other => other.clone(),
        };
        out.insert(key.to_string(), v);
    }
    out
}

/// The number of bytes an unfiltered inline image occupies.
pub(crate) fn inline_len(file: &File, d: &Dict, res: Option<&Dict>) -> Option<usize> {
    if !filters(file, d).is_empty() {
        return None;
    }
    let w = file.get_num(d, "Width")? as usize;
    let h = file.get_num(d, "Height")? as usize;
    let mask = matches!(file.get(d, "ImageMask"), Some(Obj::Bool(true)));
    let bpc = if mask { 1 } else { file.get_num(d, "BitsPerComponent").unwrap_or(8.0) as usize };
    let n = if mask { 1 } else { file.get(d, "ColorSpace").map(|c| color_space(file, c, res).components()).unwrap_or(1) };
    // `Width`/`Height` come straight from the file and can be enormous: fall back to the
    // `EI` scan (the caller's `None` branch) instead of overflowing the arithmetic.
    let row = w.checked_mul(n)?.checked_mul(bpc)?.div_ceil(8);
    row.checked_mul(h)
}

/// Decode an image's samples (all filters, including DCT). Returns (bytes, components from
/// a JPEG when it decided them itself).
fn samples(file: &File, d: &Dict, raw: &[u8]) -> Result<(Vec<u8>, Option<usize>), String> {
    let fs = filters(file, d);
    match fs.last().map(String::as_str) {
        Some("DCTDecode" | "DCT") => {
            let mut d2 = d.clone();
            d2.remove("F");
            let rest: Vec<Obj> = fs[..fs.len() - 1].iter().map(|n| Obj::Name(n.clone())).collect();
            d2.insert("Filter".into(), Obj::Array(rest));
            d2.remove("DecodeParms");
            d2.remove("DP");
            let jpeg = decode_stream(file, &d2, raw).ok_or("image (filters)")?;
            jpeg_decode(&jpeg).map(|(b, n)| (b, Some(n)))
        }
        Some(f @ ("JPXDecode" | "JBIG2Decode")) => Err(format!("image ({f})")),
        _ => decode_stream(file, d, raw).map(|b| (b, None)).ok_or_else(|| "image (filters)".to_string()),
    }
}

fn jpeg_decode(data: &[u8]) -> Result<(Vec<u8>, usize), String> {
    use zune_jpeg::JpegDecoder;
    use zune_jpeg::zune_core::colorspace::ColorSpace;
    use zune_jpeg::zune_core::options::DecoderOptions;
    let mut probe = JpegDecoder::new(std::io::Cursor::new(data));
    probe.decode_headers().map_err(|_| "image (JPEG)".to_string())?;
    let n = probe.input_colorspace().map(|c| c.num_components()).unwrap_or(3);
    let out = match n {
        1 => ColorSpace::Luma,
        4 => ColorSpace::CMYK,
        _ => ColorSpace::RGB,
    };
    let opts = DecoderOptions::default().jpeg_set_out_colorspace(out).set_max_width(MAX_SIDE).set_max_height(MAX_SIDE);
    let mut dec = JpegDecoder::new_with_options(std::io::Cursor::new(data), opts);
    let mut px = dec.decode().map_err(|_| "image (JPEG)".to_string())?;
    // Adobe CMYK JPEGs (an APP14 "Adobe" marker) store inverted values.
    if n == 4 && data.windows(5).take(4096).any(|w| w == b"Adobe") {
        for v in &mut px {
            *v = 255 - *v;
        }
    }
    Ok((px, n.max(1)))
}

/// Unpack `w`×`h` samples of `n` components at `bpc` bits into 0..=max integers.
fn unpack(data: &[u8], w: usize, h: usize, n: usize, bpc: usize) -> Vec<u16> {
    let row_bytes = (w * n * bpc).div_ceil(8);
    let mut out = Vec::with_capacity(w * h * n);
    for y in 0..h {
        let row = data.get(y * row_bytes..((y + 1) * row_bytes).min(data.len())).unwrap_or(&[]);
        for i in 0..w * n {
            let v = match bpc {
                8 => row.get(i).copied().unwrap_or(0) as u16,
                16 => u16::from_be_bytes([row.get(2 * i).copied().unwrap_or(0), row.get(2 * i + 1).copied().unwrap_or(0)]),
                1 | 2 | 4 => {
                    let bit = i * bpc;
                    let byte = row.get(bit / 8).copied().unwrap_or(0);
                    ((byte >> (8 - bpc - bit % 8)) & ((1u8 << bpc) - 1)) as u16
                }
                _ => 0,
            };
            out.push(v);
        }
    }
    out
}

/// Nearest-neighbour resample of a single-channel 8-bit mask to `w`×`h`.
fn resample(m: &[u8], mw: usize, mh: usize, w: usize, h: usize) -> Vec<u8> {
    if mw == w && mh == h {
        return m.to_vec();
    }
    let mut out = Vec::with_capacity(w * h);
    for y in 0..h {
        let sy = (y * mh / h.max(1)).min(mh.saturating_sub(1));
        for x in 0..w {
            let sx = (x * mw / w.max(1)).min(mw.saturating_sub(1));
            out.push(m.get(sy * mw + sx).copied().unwrap_or(255));
        }
    }
    out
}

/// A stencil or soft mask stream as 8-bit coverage.
fn mask_image(file: &File, o: &Obj, soft: bool) -> Option<(Vec<u8>, usize, usize)> {
    let Obj::Stream(d, raw) = file.resolve(o) else { return None };
    let w = file.get_num(d, "Width")? as usize;
    let h = file.get_num(d, "Height")? as usize;
    if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE {
        return None;
    }
    let (data, jpeg_n) = samples(file, d, raw).ok()?;
    if jpeg_n.is_some() {
        return Some((data.into_iter().step_by(jpeg_n.unwrap_or(1)).take(w * h).collect(), w, h));
    }
    let bpc = if soft { file.get_num(d, "BitsPerComponent").unwrap_or(8.0) as usize } else { 1 };
    // Like `decode()`: only 1/2/4/8/16-bit depths are supported. Anything else (notably
    // `bpc >= 32`, where `1u32 << bpc` below would overflow) means "no usable mask".
    if !matches!(bpc, 1 | 2 | 4 | 8 | 16) {
        return None;
    }
    let max = ((1u32 << bpc) - 1).max(1) as f64;
    let dec = file.get(d, "Decode").map(|x| file.nums(x)).filter(|v| v.len() >= 2);
    let v = unpack(&data, w, h, 1, bpc);
    let out = v
        .into_iter()
        .map(|s| {
            let t = s as f64 / max;
            let t = match &dec {
                Some(dd) => dd[0] + t * (dd[1] - dd[0]),
                None => t,
            };
            // Explicit (stencil) masks: sample 1 masks out with the default Decode.
            let a = if soft { t } else { 1.0 - t };
            (a.clamp(0.0, 1.0) * 255.0).round() as u8
        })
        .collect();
    Some((out, w, h))
}

/// Decode an image. `fill` is the current fill colour (stencil masks paint with it).
pub(crate) fn decode(file: &File, d: &Dict, raw: &[u8], res: Option<&Dict>, fill: [f64; 3]) -> Result<Decoded, String> {
    let w = file.get_num(d, "Width").unwrap_or(0.0) as usize;
    let h = file.get_num(d, "Height").unwrap_or(0.0) as usize;
    if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE || w * h > 64 << 20 {
        return Err("image (size)".into());
    }
    let stencil = matches!(file.get(d, "ImageMask"), Some(Obj::Bool(true)));
    let (data, jpeg_n) = samples(file, d, raw)?;
    let decode_arr = file.get(d, "Decode").map(|x| file.nums(x)).unwrap_or_default();
    let mut rgba = vec![0u8; w * h * 4];
    if stencil {
        let v = unpack(&data, w, h, 1, 1);
        let paint_on = if decode_arr.first().copied().unwrap_or(0.0) > 0.5 { 1 } else { 0 };
        let c = fill.map(|x| (x.clamp(0.0, 1.0) * 255.0).round() as u8);
        for (i, s) in v.into_iter().enumerate() {
            rgba[i * 4..i * 4 + 3].copy_from_slice(&c);
            rgba[i * 4 + 3] = if s == paint_on { 255 } else { 0 };
        }
        return Ok(Decoded { width: w as u32, height: h as u32, rgba });
    }
    let cs = match file.get(d, "ColorSpace") {
        Some(c) => color_space(file, c, res),
        None if jpeg_n == Some(4) => Cs::Cmyk,
        None if jpeg_n == Some(1) => Cs::Gray,
        None => Cs::Rgb,
    };
    let n = match jpeg_n {
        Some(n) => n,
        None => cs.components().max(1),
    };
    let bpc = if jpeg_n.is_some() { 8 } else { file.get_num(d, "BitsPerComponent").unwrap_or(8.0) as usize };
    if !matches!(bpc, 1 | 2 | 4 | 8 | 16) {
        return Err(format!("image ({bpc} bits per component)"));
    }
    let max = ((1u32 << bpc) - 1) as f64;
    let raw_samples = unpack(&data, w, h, n, bpc);
    // Decode ranges per component.
    let indexed = matches!(cs, Cs::Indexed { .. });
    let range = |k: usize| -> (f64, f64) {
        match (decode_arr.get(2 * k), decode_arr.get(2 * k + 1)) {
            (Some(a), Some(b)) => (*a, *b),
            _ if indexed => (0.0, max),
            _ => (0.0, 1.0),
        }
    };
    let ranges: Vec<(f64, f64)> = (0..n).map(range).collect();
    // Fast paths: a lookup per 8-bit value for Gray / RGB / CMYK / Indexed.
    let mut comp = vec![0.0f64; n];
    // Indexed images: one colour per palette entry.
    let palette: Option<Vec<[f64; 3]>> = match &cs {
        Cs::Indexed { hival, .. } if n == 1 => Some((0..=*hival).map(|i| cs.to_rgb(&[i as f64])).collect()),
        _ => None,
    };
    for i in 0..w * h {
        if let Some(pal) = &palette {
            let s = raw_samples[i] as f64 / max;
            let (a, b) = ranges[0];
            let idx = (a + s * (b - a)).round().clamp(0.0, (pal.len() - 1) as f64) as usize;
            let c = pal[idx];
            rgba[i * 4..i * 4 + 4].copy_from_slice(&[(c[0] * 255.0).round() as u8, (c[1] * 255.0).round() as u8, (c[2] * 255.0).round() as u8, 255]);
            continue;
        }
        for k in 0..n {
            let s = raw_samples[i * n + k] as f64 / max;
            let (a, b) = ranges[k];
            comp[k] = a + s * (b - a);
        }
        let c = cs.to_rgb(&comp);
        let o = i * 4;
        rgba[o] = (c[0] * 255.0).round() as u8;
        rgba[o + 1] = (c[1] * 255.0).round() as u8;
        rgba[o + 2] = (c[2] * 255.0).round() as u8;
        rgba[o + 3] = 255;
    }
    // Colour-key masking: samples inside every [min max] pair are transparent.
    match file.get(d, "Mask").map(|m| file.resolve(m)) {
        Some(Obj::Array(a)) => {
            let keys: Vec<f64> = a.iter().filter_map(|x| file.resolve(x).num()).collect();
            if keys.len() >= 2 * n {
                for i in 0..w * h {
                    if (0..n).all(|k| {
                        let v = raw_samples[i * n + k] as f64;
                        v >= keys[2 * k] && v <= keys[2 * k + 1]
                    }) {
                        rgba[i * 4 + 3] = 0;
                    }
                }
            }
        }
        Some(m @ Obj::Stream(..)) => {
            if let Some((m, mw, mh)) = mask_image(file, m, false) {
                for (i, a) in resample(&m, mw, mh, w, h).into_iter().enumerate() {
                    rgba[i * 4 + 3] = a;
                }
            }
        }
        _ => {}
    }
    if let Some(sm) = file.get(d, "SMask")
        && let Some((m, mw, mh)) = mask_image(file, sm, true)
    {
        for (i, a) in resample(&m, mw, mh, w, h).into_iter().enumerate() {
            rgba[i * 4 + 3] = ((rgba[i * 4 + 3] as u32 * a as u32) / 255) as u8;
        }
    }
    Ok(Decoded { width: w as u32, height: h as u32, rgba })
}
