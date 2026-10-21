//! Tests for the decode memory budget, with the previous decoder (one `Vec<f32>` per channel and a
//! result vector, no budget) kept as the reference the new one must reproduce bit for bit.

use super::write::*;
use super::*;

/// The decoder as it was before the budget: copied unchanged except for the names.
impl Psd {
    /// Decode one channel of `w`×`h` samples at `offset` (compression field first). Values 0..1.
    fn ref_decode_channel(&self, offset: usize, len: usize, w: usize, h: usize) -> Result<Vec<f32>> {
        let end = offset.checked_add(len).filter(|&e| e <= self.data.len()).ok_or(Error::Truncated)?;
        let mut r = Reader::at(&self.data[..end], offset);
        let comp = r.u16()?;
        decode_pixels(w, h)?;
        let rowb = self.row_bytes(w);
        let raw = match comp {
            0 => r.bytes(rowb * h)?.to_vec(),
            1 => {
                let mut counts = Vec::with_capacity(h.min(r.remaining() / 2));
                for _ in 0..h {
                    counts.push(if self.psb { r.u32()? as usize } else { r.u16()? as usize });
                }
                let mut out = Vec::with_capacity(rowb * h);
                for c in counts {
                    unpack_bits(r.bytes(c)?, rowb, &mut out)?;
                }
                out
            }
            2 | 3 => {
                let rest = r.bytes(r.remaining())?;
                let mut v = miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(rest, rowb * h).map_err(|e| Error::Invalid(format!("zip: {e:?}")))?;
                v.resize(rowb * h, 0);
                if comp == 3 {
                    self.unpredict(&mut v, w, h);
                }
                v
            }
            c => return Err(Error::Unsupported(format!("compression {c}"))),
        };
        Ok(self.ref_samples(&raw, w, h))
    }

    pub(crate) fn ref_samples(&self, raw: &[u8], w: usize, h: usize) -> Vec<f32> {
        match self.depth {
            1 => {
                let rowb = w.div_ceil(8);
                let mut out = Vec::with_capacity(w * h);
                for y in 0..h {
                    for x in 0..w {
                        let b = raw.get(y * rowb + x / 8).copied().unwrap_or(0);
                        // 1 = black.
                        out.push(if b & (0x80 >> (x % 8)) != 0 { 0.0 } else { 1.0 });
                    }
                }
                out
            }
            16 => raw.as_chunks::<2>().0.iter().take(w * h).map(|c| u16::from_be_bytes([c[0], c[1]]) as f32 / 65535.0).collect(),
            // As the previous decoder after the non-finite fix: NaN reads as 0, +inf as 1, -inf as 0.
            32 => raw
                .as_chunks::<4>()
                .0
                .iter()
                .take(w * h)
                .map(|c| {
                    let v = f32::from_be_bytes([c[0], c[1], c[2], c[3]]);
                    if v.is_nan() || v == f32::NEG_INFINITY {
                        0.0
                    } else if v == f32::INFINITY {
                        1.0
                    } else {
                        v
                    }
                })
                .collect(),
            _ => raw.iter().take(w * h).map(|&b| b as f32 / 255.0).collect(),
        }
    }

    /// Convert colour channels (in the document's mode) to RGB.
    pub(crate) fn ref_to_rgb(&self, ch: &[Option<Vec<f32>>], i: usize) -> [f32; 3] {
        let g = |k: usize| ch.get(k).and_then(|c| c.as_ref()).map_or(0.0, |c| c[i]);
        let rgb = match self.mode {
            ColorMode::Rgb => [g(0), g(1), g(2)],
            ColorMode::Cmyk => {
                // Stored inverted (1 = no ink).
                let k = g(3);
                [g(0) * k, g(1) * k, g(2) * k]
            }
            ColorMode::Indexed if self.palette.len() >= 768 => {
                let idx = (g(0) * 255.0).round() as usize & 255;
                [self.palette[idx] as f32 / 255.0, self.palette[256 + idx] as f32 / 255.0, self.palette[512 + idx] as f32 / 255.0]
            }
            ColorMode::Lab => lab_to_rgb(g(0) * 100.0, g(1) * 255.0 - 128.0, g(2) * 255.0 - 128.0),
            _ => [g(0); 3],
        };
        if self.depth == 32 && self.mode != ColorMode::Lab { rgb.map(srgb_encode) } else { rgb }
    }

    /// The merged (flattened) image.
    fn ref_composite(&self) -> Result<Pixels> {
        let (w, h) = (self.width as usize, self.height as usize);
        decode_pixels(w, h)?;
        let mut r = Reader::at(&self.data, self.merged_offset);
        let comp = r.u16()?;
        let nch = self.channels as usize;
        let cc = self.mode.color_channels();
        let want = (cc + usize::from(self.merged_alpha)).min(nch);
        let rowb = self.row_bytes(w);
        let mut planes: Vec<Option<Vec<f32>>> = vec![None; nch];
        match comp {
            0 => {
                for p in planes.iter_mut().take(want) {
                    *p = Some(self.ref_samples(r.bytes(rowb * h)?, w, h));
                }
            }
            1 => {
                let mut counts = Vec::with_capacity((nch * h).min(r.remaining() / 2));
                for _ in 0..nch * h {
                    counts.push(if self.psb { r.u32()? as usize } else { r.u16()? as usize });
                }
                for (c, p) in planes.iter_mut().enumerate().take(want) {
                    let mut out = Vec::with_capacity(rowb * h);
                    for row in 0..h {
                        unpack_bits(r.bytes(counts[c * h + row])?, rowb, &mut out)?;
                    }
                    *p = Some(self.ref_samples(&out, w, h));
                }
            }
            2 | 3 => {
                let rest = r.bytes(r.remaining())?;
                let v = miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(rest, want * rowb * h).map_err(|e| Error::Invalid(format!("zip: {e:?}")))?;
                for (c, p) in planes.iter_mut().enumerate().take(want) {
                    let mut plane = v.get(c * rowb * h..(c + 1) * rowb * h).ok_or(Error::Truncated)?.to_vec();
                    if comp == 3 {
                        self.unpredict(&mut plane, w, h);
                    }
                    *p = Some(self.ref_samples(&plane, w, h));
                }
            }
            c => return Err(Error::Unsupported(format!("compression {c}"))),
        }
        let alpha = if self.merged_alpha && nch > cc { planes[cc].take() } else { None };
        let mut data = Vec::with_capacity(w * h);
        for i in 0..w * h {
            let [r, g, b] = self.ref_to_rgb(&planes, i);
            let a = alpha.as_ref().map_or(1.0, |a| a[i]);
            data.push([r, g, b, a]);
        }
        Ok(Pixels { width: w as u32, height: h as u32, data })
    }

    /// One layer's pixels with its layer mask (and vector-less fill colour) applied to alpha.
    /// `canvas`: document-sized with the layer in place; otherwise the layer's own bounds.
    fn ref_layer_pixels(&self, index: usize, canvas: bool) -> Result<Pixels> {
        let l = self.layers.get(index).ok_or_else(|| Error::Invalid(format!("no layer {index}")))?;
        let doc = Rect { top: 0, left: 0, bottom: self.height as i32, right: self.width as i32 };
        // Fill layers often have no pixel data: they cover the canvas (or the mask).
        let fill_rect =
            if l.fill.is_some() && !l.has_pixels() { l.mask.filter(|m| !m.rect.is_empty() && m.default_color == 0).map_or(doc, |m| m.rect) } else { l.rect };
        let out_rect = if canvas {
            doc
        } else if fill_rect.is_empty() {
            Rect { top: 0, left: 0, bottom: 1, right: 1 }
        } else {
            fill_rect
        };
        let (ow, oh) = (out_rect.width() as usize, out_rect.height() as usize);
        let mut out = vec![[0.0f32; 4]; decode_pixels(ow, oh)?];
        let src = fill_rect;
        let (sw, sh) = (src.width() as usize, src.height() as usize);
        if sw > 0 && sh > 0 {
            let mut planes: Vec<Option<Vec<f32>>> = vec![None; self.mode.color_channels()];
            let mut alpha = None;
            if l.has_pixels() {
                for c in &l.channels {
                    match c.id {
                        -1 => alpha = Some(self.ref_decode_channel(c.offset, c.len, sw, sh)?),
                        id if id >= 0 && (id as usize) < planes.len() => planes[id as usize] = Some(self.ref_decode_channel(c.offset, c.len, sw, sh)?),
                        _ => {}
                    }
                }
            }
            let fill = l.fill.map(|c| c.map(|v| v as f32));
            // The source rows / columns that land inside the output (64-bit: file bounds can span
            // the whole i32 range).
            let span = |s0: i32, n: usize, o0: i32, on: usize| {
                let off = i64::from(o0) - i64::from(s0);
                let lo = off.clamp(0, n as i64);
                let hi = (off + on as i64).clamp(lo, n as i64);
                (lo as usize..hi as usize, off)
            };
            let (ys, oy) = span(src.top, sh, out_rect.top, oh);
            let (xs, ox) = span(src.left, sw, out_rect.left, ow);
            for y in ys {
                let dy = (y as i64 - oy) as usize;
                for x in xs.clone() {
                    let dx = (x as i64 - ox) as usize;
                    let i = y * sw + x;
                    let (rgb, a) = match fill {
                        Some(f) if !l.has_pixels() => (f, 1.0),
                        _ => (self.ref_to_rgb(&planes, i), alpha.as_ref().map_or(1.0, |a| a[i])),
                    };
                    out[dy * ow + dx] = [rgb[0], rgb[1], rgb[2], a];
                }
            }
        }
        // Layer mask: multiplies alpha (also outside the mask rect, by its default colour).
        if let Some(m) = l.mask.filter(|m| !m.disabled) {
            let mc = l.channels.iter().find(|c| c.id == -2);
            let (mw, mh) = (m.rect.width() as usize, m.rect.height() as usize);
            let mask = match mc {
                Some(c) if mw > 0 && mh > 0 => Some(self.ref_decode_channel(c.offset, c.len, mw, mh)?),
                _ => None,
            };
            let def = m.default_color as f32 / 255.0;
            for y in 0..oh {
                let py = out_rect.top + y as i32;
                for x in 0..ow {
                    let px = out_rect.left + x as i32;
                    let inside = py >= m.rect.top && py < m.rect.bottom && px >= m.rect.left && px < m.rect.right;
                    let mut v = match (&mask, inside) {
                        (Some(mk), true) => mk[(py - m.rect.top) as usize * mw + (px - m.rect.left) as usize],
                        _ => def,
                    };
                    if m.inverted {
                        v = 1.0 - v;
                    }
                    out[y * ow + x][3] *= v;
                }
            }
        }
        Ok(Pixels { width: ow as u32, height: oh as u32, data: out })
    }
}

// ---------------------------------------------------------------------------------------------
// Tests

fn grad(w: u32, h: u32) -> Vec<[f32; 4]> {
    (0..w * h)
        .map(|i| [(i % w) as f32 / (w - 1).max(1) as f32, (i / w) as f32 / (h - 1).max(1) as f32, 0.25 + (i % 7) as f32 / 16.0, 0.3 + (i % 5) as f32 / 8.0])
        .collect()
}

/// Layers covering: full canvas pixels with alpha, a partly off-canvas layer, a masked layer with
/// an inverted mask and a default colour, a solid layer.
fn doc(depth: u16, mode: ColorMode, rle: bool) -> WDoc {
    let mut d = WDoc::new(40, 30);
    d.depth = depth;
    d.mode = mode;
    d.rle = rle;
    let bg = WLayer::pixels("bg", Rect::new(0, 0, 40, 30), grad(40, 30));
    let off = WLayer::pixels("off", Rect::new(-5, -4, 18, 13), grad(18, 13));
    let solid = WLayer::solid("solid", Rect::new(30, 20, 15, 15), [0.2, 0.6, 0.9, 1.0]);
    let mut masked = WLayer::pixels("masked", Rect::new(10, 8, 12, 12), grad(12, 12));
    masked.mask =
        Some(WMask { rect: Rect::new(12, 10, 12, 12), data: (0..144).map(|i| (i % 12) as f32 / 11.0).collect(), default_color: 255, disabled: false });
    d.layers = vec![bg, off, solid, masked];
    d
}

fn assert_same_as_reference(p: &Psd, what: &str) {
    assert_eq!(p.composite().unwrap(), p.ref_composite().unwrap(), "composite {what}");
    for i in 0..p.layers.len() {
        for canvas in [false, true] {
            assert_eq!(p.layer_pixels(i, canvas).unwrap(), p.ref_layer_pixels(i, canvas).unwrap(), "layer {i} canvas={canvas} {what}");
        }
    }
}

#[test]
fn pixels_match_the_one_buffer_per_channel_reference() {
    for depth in [8u16, 16] {
        for mode in [ColorMode::Rgb, ColorMode::Grayscale, ColorMode::Cmyk] {
            for rle in [true, false] {
                let p = Psd::parse(write(&doc(depth, mode, rle))).unwrap();
                assert_same_as_reference(&p, &format!("{depth}-bit {mode:?} rle={rle}"));
            }
        }
    }
}

/// A copy of `bytes` with a different size, depth and merged-image section (`comp`, then `body`).
pub(crate) fn with_merged(bytes: &[u8], size: Option<(u32, u32)>, depth: Option<u16>, comp: u16, body: &[u8]) -> Vec<u8> {
    let offset = Psd::parse(bytes.to_vec()).unwrap().merged_offset;
    let mut v = bytes[..offset].to_vec();
    if let Some((w, h)) = size {
        v[14..18].copy_from_slice(&h.to_be_bytes());
        v[18..22].copy_from_slice(&w.to_be_bytes());
    }
    if let Some(d) = depth {
        v[22..24].copy_from_slice(&d.to_be_bytes());
    }
    v.extend_from_slice(&comp.to_be_bytes());
    v.extend_from_slice(body);
    v
}

/// Channel planes for a 6×5 image: values that exercise the whole range.
fn planes(nch: usize, bytes_per_sample: usize, n: usize) -> Vec<u8> {
    let mut v = vec![];
    for c in 0..nch {
        for i in 0..n {
            let t = ((i * 37 + c * 91) % 251) as f32 / 250.0;
            match bytes_per_sample {
                1 => v.push((t * 255.0) as u8),
                2 => v.extend_from_slice(&((t * 65535.0) as u16).to_be_bytes()),
                _ => v.extend_from_slice(&(t * 1.5 - 0.1).to_be_bytes()),
            }
        }
    }
    v
}

#[test]
fn merged_image_matches_the_reference_for_32_bit_and_zip() {
    for mode in [ColorMode::Rgb, ColorMode::Grayscale, ColorMode::Cmyk] {
        let mut small = WDoc::new(6, 5);
        small.mode = mode;
        small.rle = false;
        let base = write(&small);
        let info = Psd::parse(base.clone()).unwrap();
        // The merged image only reads the colour channels and, when flagged, the alpha channel.
        let nch = (mode.color_channels() + usize::from(info.merged_alpha)).min(info.channels as usize);
        let n = 30;
        // 32-bit float, raw.
        let p = Psd::parse(with_merged(&base, None, Some(32), 0, &planes(nch, 4, n))).unwrap();
        assert_eq!(p.depth, 32);
        assert_eq!(p.composite().unwrap(), p.ref_composite().unwrap(), "32-bit {mode:?}");
        // ZIP without and with prediction, 8 and 16 bit; 32-bit ZIP without prediction.
        for (depth, bps) in [(8u16, 1usize), (16, 2), (32, 4)] {
            let raw = planes(nch, bps, n);
            let zip = miniz_oxide::deflate::compress_to_vec_zlib(&raw, 6);
            let p = Psd::parse(with_merged(&base, None, Some(depth), 2, &zip)).unwrap();
            assert_eq!(p.composite().unwrap(), p.ref_composite().unwrap(), "zip {depth}-bit {mode:?}");
            if depth == 32 {
                continue;
            }
            // Prediction: each row stores differences to its left neighbour.
            let mut pred = raw.clone();
            for plane in pred.chunks_exact_mut(n * bps) {
                for row in plane.chunks_exact_mut(6 * bps) {
                    if bps == 1 {
                        for i in (1..6).rev() {
                            row[i] = row[i].wrapping_sub(row[i - 1]);
                        }
                    } else {
                        for i in (1..6).rev() {
                            let a = u16::from_be_bytes([row[i * 2], row[i * 2 + 1]]);
                            let b = u16::from_be_bytes([row[i * 2 - 2], row[i * 2 - 1]]);
                            row[i * 2..i * 2 + 2].copy_from_slice(&a.wrapping_sub(b).to_be_bytes());
                        }
                    }
                }
            }
            let zip = miniz_oxide::deflate::compress_to_vec_zlib(&pred, 6);
            let p = Psd::parse(with_merged(&base, None, Some(depth), 3, &zip)).unwrap();
            assert_eq!(p.composite().unwrap(), p.ref_composite().unwrap(), "zip+prediction {depth}-bit {mode:?}");
        }
    }
}

#[test]
fn budget_counts_the_sum_of_the_buffers_of_a_decode() {
    let mut b = Budget::new(100);
    b.charge(60).unwrap();
    assert!(b.charge(41).is_err(), "60 + 41 is over 100 although each alone fits");
    b.check(40).unwrap();
    assert!(b.check(41).is_err());
    b.release(30);
    b.charge(70).unwrap();
    assert!(b.vec::<u8>(1).unwrap_err().to_string().contains("memory budget"));
    assert!(Budget::new(u64::MAX).vec::<u64>(usize::MAX / 2).is_err(), "a size that cannot be reserved is an error, not an abort");

    // The same for a real decode: the result (16 bytes a pixel) and a channel's bytes together.
    let p = Psd::parse(write(&doc(8, ColorMode::Rgb, true))).unwrap();
    let n = 40 * 30;
    let peak = 16 * n + n;
    assert!(p.composite_with_budget(peak).is_ok());
    for limit in [peak - 1, 16 * n, 16 * n - 1, 1000, 0] {
        let e = p.composite_with_budget(limit).unwrap_err();
        assert!(e.to_string().contains("memory budget"), "{limit}: {e}");
    }
    // A layer: the result plus its channel; the mask comes after the channels are gone.
    let bg = p.layers.iter().position(|l| l.name == "bg").unwrap();
    assert!(p.layer_pixels_with_budget(bg, true, 16 * n + n).is_ok());
    assert!(p.layer_pixels_with_budget(bg, true, 16 * n).unwrap_err().to_string().contains("memory budget"));
    assert_eq!(p.layer_pixels_with_budget(bg, true, 16 * n + n).unwrap(), p.layer_pixels(bg, true).unwrap());
}

#[test]
fn a_huge_header_with_a_tiny_zip_stream_is_refused_before_it_allocates() {
    // 16384 × 16384, RGB + alpha: a few hundred bytes of zlib that inflate to gigabytes of zeros.
    let mut small = WDoc::new(2, 2);
    small.rle = false;
    let base = write(&small);
    let zeros = vec![0u8; 1 << 20];
    let stream = miniz_oxide::deflate::compress_to_vec_zlib(&zeros, 9);
    let bytes = with_merged(&base, Some((16384, 16384)), None, 2, &stream);
    assert!(bytes.len() < 8192);
    let p = Psd::parse(bytes).unwrap();
    assert_eq!((p.width, p.height), (16384, 16384));
    let e = p.composite().unwrap_err();
    assert!(e.to_string().contains("memory budget"), "{e}");
    // The same file with the compression types that read straight from the file.
    for comp in [0u16, 1, 3] {
        let p = Psd::parse(with_merged(&base, Some((16384, 16384)), None, comp, &stream)).unwrap();
        let e = p.composite().unwrap_err();
        assert!(e.to_string().contains("memory budget"), "comp {comp}: {e}");
    }
    // And a smaller image under a small budget: the stream is not inflated.
    let p = Psd::parse(with_merged(&base, Some((4096, 4096)), None, 2, &stream)).unwrap();
    assert!(p.composite_with_budget(1 << 20).unwrap_err().to_string().contains("memory budget"));
}
