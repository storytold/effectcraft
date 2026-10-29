//! The optimised decode / mask / placed-bake paths against the implementations they replaced
//! (kept below verbatim as references), and counters for the work they no longer do.

use std::borrow::Cow;

use crate::counters::{self, INSIDE, QUAD, SCRATCH};
use crate::placed::{Warp, square_to_quad};
use crate::write::*;
use crate::*;

// ------------------------------------------------------------------ the replaced code

fn ref_unpredict(depth: u16, v: &mut [u8], w: usize, h: usize) -> usize {
    let mut allocs = 0;
    match depth {
        16 => {
            for row in v.chunks_exact_mut(w * 2).take(h) {
                let mut prev = 0u16;
                for c in row.as_chunks_mut::<2>().0 {
                    let x = u16::from_be_bytes([c[0], c[1]]).wrapping_add(prev);
                    c.copy_from_slice(&x.to_be_bytes());
                    prev = x;
                }
            }
        }
        32 => {
            for row in v.chunks_exact_mut(w * 4).take(h) {
                for i in 1..row.len() {
                    row[i] = row[i].wrapping_add(row[i - 1]);
                }
                let planar = row.to_vec();
                allocs += 1;
                for x in 0..w {
                    for b in 0..4 {
                        row[x * 4 + b] = planar[b * w + x];
                    }
                }
            }
        }
        _ => {
            for row in v.chunks_exact_mut(w).take(h) {
                for i in 1..row.len() {
                    row[i] = row[i].wrapping_add(row[i - 1]);
                }
            }
        }
    }
    allocs
}

fn ref_decode_channel(p: &Psd, offset: usize, len: usize, w: usize, h: usize) -> Result<Vec<f32>> {
    let end = offset.checked_add(len).filter(|&e| e <= p.data.len()).ok_or(Error::Truncated)?;
    let mut r = Reader::at(&p.data[..end], offset);
    let comp = r.u16()?;
    decode_pixels(w, h)?;
    let rowb = p.row_bytes(w);
    let raw = match comp {
        0 => r.bytes(rowb * h)?.to_vec(),
        1 => {
            let mut counts = Vec::with_capacity(h.min(r.remaining() / 2));
            for _ in 0..h {
                counts.push(if p.psb { r.u32()? as usize } else { r.u16()? as usize });
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
                ref_unpredict(p.depth, &mut v, w, h);
            }
            v
        }
        c => return Err(Error::Unsupported(format!("compression {c}"))),
    };
    Ok(p.ref_samples(&raw, w, h))
}

fn ref_composite(p: &Psd) -> Result<Pixels> {
    let (w, h) = (p.width as usize, p.height as usize);
    decode_pixels(w, h)?;
    let mut r = Reader::at(&p.data, p.merged_offset);
    let comp = r.u16()?;
    let nch = p.channels as usize;
    let cc = p.mode.color_channels();
    let want = (cc + usize::from(p.merged_alpha)).min(nch);
    let rowb = p.row_bytes(w);
    let mut planes: Vec<Option<Vec<f32>>> = vec![None; nch];
    match comp {
        0 => {
            for pl in planes.iter_mut().take(want) {
                *pl = Some(p.ref_samples(r.bytes(rowb * h)?, w, h));
            }
        }
        1 => {
            let mut counts = Vec::with_capacity((nch * h).min(r.remaining() / 2));
            for _ in 0..nch * h {
                counts.push(if p.psb { r.u32()? as usize } else { r.u16()? as usize });
            }
            for (c, pl) in planes.iter_mut().enumerate().take(want) {
                let mut out = Vec::with_capacity(rowb * h);
                for row in 0..h {
                    unpack_bits(r.bytes(counts[c * h + row])?, rowb, &mut out)?;
                }
                *pl = Some(p.ref_samples(&out, w, h));
            }
        }
        2 | 3 => {
            let rest = r.bytes(r.remaining())?;
            let v = miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(rest, want * rowb * h).map_err(|e| Error::Invalid(format!("zip: {e:?}")))?;
            for (c, pl) in planes.iter_mut().enumerate().take(want) {
                let mut plane = v.get(c * rowb * h..(c + 1) * rowb * h).ok_or(Error::Truncated)?.to_vec();
                if comp == 3 {
                    ref_unpredict(p.depth, &mut plane, w, h);
                }
                *pl = Some(p.ref_samples(&plane, w, h));
            }
        }
        c => return Err(Error::Unsupported(format!("compression {c}"))),
    }
    let alpha = if p.merged_alpha && nch > cc { planes[cc].take() } else { None };
    let mut data = Vec::with_capacity(w * h);
    for i in 0..w * h {
        let [r, g, b] = p.ref_to_rgb(&planes, i);
        let a = alpha.as_ref().map_or(1.0, |a| a[i]);
        data.push([r, g, b, a]);
    }
    Ok(Pixels { width: w as u32, height: h as u32, data })
}

fn ref_layer_pixels(p: &Psd, index: usize, canvas: bool) -> Result<Pixels> {
    let l = p.layers.get(index).ok_or_else(|| Error::Invalid(format!("no layer {index}")))?;
    let doc = Rect { top: 0, left: 0, bottom: p.height as i32, right: p.width as i32 };
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
        let mut planes: Vec<Option<Vec<f32>>> = vec![None; p.mode.color_channels()];
        let mut alpha = None;
        if l.has_pixels() {
            for c in &l.channels {
                match c.id {
                    -1 => alpha = Some(ref_decode_channel(p, c.offset, c.len, sw, sh)?),
                    id if id >= 0 && (id as usize) < planes.len() => planes[id as usize] = Some(ref_decode_channel(p, c.offset, c.len, sw, sh)?),
                    _ => {}
                }
            }
        }
        let fill = l.fill.map(|c| c.map(|v| v as f32));
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
                    _ => (p.ref_to_rgb(&planes, i), alpha.as_ref().map_or(1.0, |a| a[i])),
                };
                out[dy * ow + dx] = [rgb[0], rgb[1], rgb[2], a];
            }
        }
    }
    if let Some(m) = l.mask.filter(|m| !m.disabled) {
        let mc = l.channels.iter().find(|c| c.id == -2);
        let (mw, mh) = (m.rect.width() as usize, m.rect.height() as usize);
        let mask = match mc {
            Some(c) if mw > 0 && mh > 0 => Some(ref_decode_channel(p, c.offset, c.len, mw, mh)?),
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

fn ref_map(so: &SmartObject, p: [f64; 2], w: f64, h: f64) -> [f64; 2] {
    let p = match &so.warp {
        Some(wp) => wp.apply(p, w, h),
        None => p,
    };
    let m = square_to_quad(so.quad);
    let (u, v) = (p[0] / w.max(1e-9), p[1] / h.max(1e-9));
    let z = m[2][0] * u + m[2][1] * v + m[2][2];
    let z = if z.abs() < 1e-12 { 1e-12 } else { z };
    [(m[0][0] * u + m[0][1] * v + m[0][2]) / z, (m[1][0] * u + m[1][1] * v + m[1][2]) / z]
}

fn ref_placed_bounds(so: &SmartObject, w: f64, h: f64) -> [f64; 4] {
    let n = 32;
    let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
    for j in 0..=n {
        for i in 0..=n {
            let p = ref_map(so, [i as f64 / n as f64 * w, j as f64 / n as f64 * h], w, h);
            b = [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])];
        }
    }
    [b[0].floor(), b[1].floor(), b[2].ceil(), b[3].ceil()]
}

fn ref_placed_size(so: &SmartObject, w: f64, h: f64, k: f64) -> (u32, u32) {
    let bb = ref_placed_bounds(so, w, h);
    (((bb[2] - bb[0]) * k).ceil().clamp(1.0, 16384.0) as u32, ((bb[3] - bb[1]) * k).ceil().clamp(1.0, 16384.0) as u32)
}

fn ref_render_placed(so: &SmartObject, content: &Pixels, k: f64, size: Option<(u32, u32)>) -> Option<(Pixels, [f64; 2])> {
    let (cw, ch) = (content.width as f64, content.height as f64);
    if content.width == 0 || content.height == 0 {
        return None;
    }
    let bb = ref_placed_bounds(so, cw, ch);
    let (ow, oh) = match size {
        Some((w, h)) => (w.max(1), h.max(1)),
        None => ref_placed_size(so, cw, ch, k),
    };
    let (ow, oh) = (ow as usize, oh as usize);
    let n = 48usize;
    let mut dst = Vec::with_capacity((n + 1) * (n + 1));
    let mut src = Vec::with_capacity((n + 1) * (n + 1));
    for j in 0..=n {
        for i in 0..=n {
            let s = [i as f64 / n as f64 * cw, j as f64 / n as f64 * ch];
            let d = ref_map(so, s, cw, ch);
            dst.push([(d[0] - bb[0]) * k, (d[1] - bb[1]) * k]);
            src.push(s);
        }
    }
    let mut out = vec![[0.0f32; 4]; ow * oh];
    let sample = |x: f64, y: f64| -> [f32; 4] {
        let (fx, fy) = ((x - 0.5).clamp(0.0, cw - 1.0), (y - 0.5).clamp(0.0, ch - 1.0));
        let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(content.width as usize - 1), (y0 + 1).min(content.height as usize - 1));
        let (a, b) = ((fx - x0 as f64) as f32, (fy - y0 as f64) as f32);
        let at = |x: usize, y: usize| {
            let p = content.data[y * content.width as usize + x];
            [p[0] * p[3], p[1] * p[3], p[2] * p[3], p[3]]
        };
        let (p00, p10, p01, p11) = (at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1));
        let mut o = [0.0; 4];
        for c in 0..4 {
            o[c] = (p00[c] * (1.0 - a) + p10[c] * a) * (1.0 - b) + (p01[c] * (1.0 - a) + p11[c] * a) * b;
        }
        o
    };
    let idx = |i: usize, j: usize| j * (n + 1) + i;
    for j in 0..n {
        for i in 0..n {
            for tri in [[idx(i, j), idx(i + 1, j), idx(i + 1, j + 1)], [idx(i, j), idx(i + 1, j + 1), idx(i, j + 1)]] {
                let [a, b, c] = tri.map(|t| dst[t]);
                let den = (b[1] - c[1]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[1] - c[1]);
                if den.abs() < 1e-12 {
                    continue;
                }
                let x0 = a[0].min(b[0]).min(c[0]).floor().max(0.0) as usize;
                let x1 = (a[0].max(b[0]).max(c[0]).ceil() as usize).min(ow);
                let y0 = a[1].min(b[1]).min(c[1]).floor().max(0.0) as usize;
                let y1 = (a[1].max(b[1]).max(c[1]).ceil() as usize).min(oh);
                for y in y0..y1 {
                    for x in x0..x1 {
                        let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
                        let l1 = ((b[1] - c[1]) * (px - c[0]) + (c[0] - b[0]) * (py - c[1])) / den;
                        let l2 = ((c[1] - a[1]) * (px - c[0]) + (a[0] - c[0]) * (py - c[1])) / den;
                        let l3 = 1.0 - l1 - l2;
                        if l1 < -1e-9 || l2 < -1e-9 || l3 < -1e-9 {
                            continue;
                        }
                        let [sa, sb, sc] = tri.map(|t| src[t]);
                        let sx = sa[0] * l1 + sb[0] * l2 + sc[0] * l3;
                        let sy = sa[1] * l1 + sb[1] * l2 + sc[1] * l3;
                        out[y * ow + x] = sample(sx, sy);
                    }
                }
            }
        }
    }
    for p in &mut out {
        if p[3] > 0.0 {
            for c in 0..3 {
                p[c] /= p[3];
            }
        }
    }
    Some((Pixels { width: ow as u32, height: oh as u32, data: out }, [bb[0], bb[1]]))
}

// ------------------------------------------------------------------ fixtures

fn bits(p: &Pixels) -> Vec<[u32; 4]> {
    p.data.iter().map(|px| px.map(f32::to_bits)).collect()
}

/// Deterministic samples in 0..1 for a `w`×`h` plane, as file bytes at `depth`.
fn synth_plane(depth: u16, w: usize, h: usize, seed: usize) -> Vec<u8> {
    let mut out = vec![];
    for i in 0..w * h {
        let v = ((i * 7 + seed * 13) % 101) as f32 / 100.0;
        match depth {
            8 => out.push((v * 255.0).round() as u8),
            16 => out.extend(((v * 65535.0).round() as u16).to_be_bytes()),
            _ => out.extend(v.to_be_bytes()),
        }
    }
    out
}

/// The delta coding that `comp` 3 undoes (the inverse of `Psd::unpredict`).
fn predict(raw: &[u8], w: usize, depth: u16) -> Vec<u8> {
    let mut out = raw.to_vec();
    match depth {
        16 => {
            for row in out.chunks_exact_mut(w * 2) {
                let mut prev = 0u16;
                for c in row.as_chunks_mut::<2>().0 {
                    let x = u16::from_be_bytes([c[0], c[1]]);
                    c.copy_from_slice(&x.wrapping_sub(prev).to_be_bytes());
                    prev = x;
                }
            }
        }
        32 => {
            for row in out.chunks_exact_mut(w * 4) {
                let inter = row.to_vec();
                for x in 0..w {
                    for b in 0..4 {
                        row[b * w + x] = inter[x * 4 + b];
                    }
                }
                for i in (1..row.len()).rev() {
                    row[i] = row[i].wrapping_sub(row[i - 1]);
                }
            }
        }
        _ => {
            for row in out.chunks_exact_mut(w) {
                for i in (1..row.len()).rev() {
                    row[i] = row[i].wrapping_sub(row[i - 1]);
                }
            }
        }
    }
    out
}

/// A pixel-data stream (compression field first) of consecutive `w`×`h` planes.
fn stream(planes: &[Vec<u8>], w: usize, h: usize, depth: u16, comp: u16) -> Vec<u8> {
    let mut out = comp.to_be_bytes().to_vec();
    let rowb = planes.first().map_or(0, |p| p.len() / h.max(1));
    match comp {
        0 => planes.iter().for_each(|p| out.extend(p)),
        1 => {
            let rows: Vec<Vec<u8>> = planes.iter().flat_map(|p| p.chunks(rowb.max(1)).map(pack_bits)).collect();
            rows.iter().for_each(|r| out.extend((r.len() as u16).to_be_bytes()));
            rows.iter().for_each(|r| out.extend(r));
        }
        _ => {
            let all: Vec<u8> = planes.iter().flat_map(|p| if comp == 3 { predict(p, w, depth) } else { p.clone() }).collect();
            out.extend(miniz_oxide::deflate::compress_to_vec_zlib(&all, 6));
        }
    }
    out
}

/// A small RGB document with a background, a hidden box and a masked layer (8 bit, raw).
fn base() -> Psd {
    let mut d = WDoc::new(40, 30);
    d.rle = false;
    let grad: Vec<[f32; 4]> = (0..1200).map(|i| [(i % 40) as f32 / 39.0, (i / 40) as f32 / 29.0, 0.25, 1.0]).collect();
    let bg = WLayer::pixels("Background", Rect::new(0, 0, 40, 30), grad);
    let mut red = WLayer::solid("Red", Rect::new(5, 6, 10, 8), [1.0, 0.0, 0.0, 0.5]);
    red.hidden = true;
    let mut masked = WLayer::solid("Masked", Rect::new(20, 10, 12, 12), [0.0, 0.5, 1.0, 1.0]);
    let data = (0..144).map(|i| if i % 12 < 6 { 1.0 } else { 0.25 }).collect();
    masked.mask = Some(WMask { rect: Rect::new(20, 10, 12, 12), data, default_color: 0, disabled: false });
    d.layers = vec![bg, red, masked];
    Psd::parse(write(&d)).unwrap()
}

/// `p` with every channel and the merged image re-encoded at `depth` with compression `comp`.
fn reencode(p: &Psd, depth: u16, comp: u16) -> Psd {
    let mut z = p.clone();
    z.depth = depth;
    let mut data = p.data.to_vec();
    for (n, (l, zl)) in p.layers.iter().zip(&mut z.layers).enumerate() {
        for (c, zc) in l.channels.iter().zip(&mut zl.channels) {
            let r = if c.id == -2 { l.mask.map(|m| m.rect).unwrap_or(l.rect) } else { l.rect };
            let (w, h) = (r.width() as usize, r.height() as usize);
            zc.offset = data.len();
            data.extend(stream(&[synth_plane(depth, w, h, n * 5 + (c.id + 2) as usize)], w, h, depth, comp));
            zc.len = data.len() - zc.offset;
        }
    }
    let (w, h) = (p.width as usize, p.height as usize);
    let planes: Vec<Vec<u8>> = (0..usize::from(p.channels)).map(|c| synth_plane(depth, w, h, 100 + c)).collect();
    z.merged_offset = data.len();
    data.extend(stream(&planes, w, h, depth, comp));
    z.data = data.into();
    z
}

fn variants() -> impl Iterator<Item = (u16, u16)> {
    [8u16, 16, 32].into_iter().flat_map(|d| [0u16, 1, 2, 3].into_iter().map(move |c| (d, c)))
}

// ------------------------------------------------------------------ decoding

#[test]
fn unpredict_matches_the_reference_and_allocates_one_scratch_row() {
    for depth in [8u16, 16, 32] {
        let p = Psd { depth, ..base() };
        let (w, h) = (13usize, 9usize);
        let bps = usize::from(depth / 8);
        let mut a: Vec<u8> = (0..w * h * bps).map(|i| (i * 31 % 251) as u8).collect();
        let mut b = a.clone();
        counters::take(&SCRATCH);
        p.unpredict(&mut a, w, h);
        let scratch = counters::take(&SCRATCH);
        let old_allocs = ref_unpredict(depth, &mut b, w, h);
        assert_eq!(a, b, "{depth} bit");
        // The reference copied every row; the new code uses one scratch row (32 bit only).
        assert_eq!((scratch, old_allocs), if depth == 32 { (1, h) } else { (0, 0) }, "{depth} bit");
    }
}

#[test]
fn raw_planes_are_borrowed_and_zip_planes_are_unpredicted_in_place() {
    let p = base();
    // Raw: the sample bytes are a view into the file data.
    let q = reencode(&p, 8, 0);
    let c = q.layers[0].channels[0];
    let mut r = Reader::at(&q.data, c.offset);
    let comp = r.u16().unwrap();
    let plane = q.plane_bytes(comp, &mut r, 40, 30, &mut Budget::new(u64::MAX)).unwrap();
    let Cow::Borrowed(b) = plane else { panic!("raw plane was copied") };
    let file = q.data.as_ptr_range();
    assert!(file.contains(&b.as_ptr()) && b.len() == 1200);
    // ZIP: the plane handed out is a sub-slice of the inflated buffer, not a copy.
    let mut v: Vec<u8> = (0..3 * 40 * 30).map(|i| (i % 255) as u8).collect();
    let range = v.as_ptr_range();
    for comp in [2, 3] {
        let got = q.zip_plane(comp, &mut v, 1, 40, 30).unwrap();
        assert_eq!(got.len(), 1200);
        assert!(range.contains(&got.as_ptr()), "compression {comp}");
    }
    assert!(q.zip_plane(2, &mut v, 3, 40, 30).is_err());
}

#[test]
fn decode_and_composite_match_the_reference() {
    let p = base();
    for (depth, comp) in variants() {
        let q = reencode(&p, depth, comp);
        let what = format!("{depth} bit, compression {comp}");
        assert_eq!(bits(&q.composite().unwrap()), bits(&ref_composite(&q).unwrap()), "composite {what}");
        for i in 0..q.layers.len() {
            for canvas in [false, true] {
                assert_eq!(bits(&q.layer_pixels(i, canvas).unwrap()), bits(&ref_layer_pixels(&q, i, canvas).unwrap()), "layer {i} canvas {canvas}, {what}");
            }
        }
    }
}

#[test]
fn errors_match_the_reference_on_damaged_data() {
    let p = base();
    for (depth, comp) in variants() {
        let mut q = reencode(&p, depth, comp);
        // Cut the file in the middle of the merged image / the last channel.
        let cut = q.data.len() - 7;
        q.data = q.data[..cut].into();
        assert_eq!(q.composite().map(|p| bits(&p)), ref_composite(&q).map(|p| bits(&p)), "{depth} bit, compression {comp}");
        assert_eq!(q.layer_pixels(2, true).map(|p| bits(&p)), ref_layer_pixels(&q, 2, true).map(|p| bits(&p)), "{depth} bit, compression {comp}");
    }
}

// ------------------------------------------------------------------ the layer mask

fn rect_at(left: i32, top: i32) -> Rect {
    Rect::new(left, top, 12, 12)
}

#[test]
fn layer_mask_matches_the_reference_for_every_mask_shape() {
    let p = base();
    for (depth, comp) in [(8, 1), (16, 2), (32, 3), (8, 0)] {
        let q = reencode(&p, depth, comp);
        // Moved so it overlaps the layer / canvas partly, wholly, or not at all (the data is 12×12).
        for (left, top) in [(20, 10), (14, 4), (-5, -3), (34, 24), (25, 12), (300, 300), (-300, 5)] {
            for default_color in [0u8, 255, 90] {
                for inverted in [false, true] {
                    for disabled in [false, true] {
                        for with_data in [true, false] {
                            let mut r = q.clone();
                            r.layers[2].mask = Some(LayerMask { rect: rect_at(left, top), default_color, disabled, inverted });
                            if !with_data {
                                r.layers[2].channels.retain(|c| c.id != -2);
                            }
                            for canvas in [false, true] {
                                let what = format!(
                                    "{depth} bit comp {comp}, mask at ({left},{top}) default {default_color} inverted {inverted} disabled {disabled} data {with_data} canvas {canvas}"
                                );
                                assert_eq!(bits(&r.layer_pixels(2, canvas).unwrap()), bits(&ref_layer_pixels(&r, 2, canvas).unwrap()), "{what}");
                            }
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn layer_mask_evaluates_inside_per_row_and_skips_no_ops() {
    let q = reencode(&base(), 8, 0);
    let with = |default_color, inverted, data: bool| {
        let mut r = q.clone();
        r.layers[2].mask = Some(LayerMask { rect: rect_at(20, 10), default_color, disabled: false, inverted });
        if !data {
            r.layers[2].channels.retain(|c| c.id != -2);
        }
        r
    };
    // With mask data: one inside test per output row (the reference tested every pixel: 40 × 30).
    counters::take(&INSIDE);
    with(0, false, true).layer_pixels(2, true).unwrap();
    assert_eq!(counters::take(&INSIDE), 30);
    // Own bounds: 12 rows.
    with(0, false, true).layer_pixels(2, false).unwrap();
    assert_eq!(counters::take(&INSIDE), 12);
    // No mask data: no per-pixel or per-row test at all (a constant factor, skipped when 1.0).
    with(255, false, false).layer_pixels(2, true).unwrap();
    with(0, true, false).layer_pixels(2, true).unwrap();
    with(0, false, false).layer_pixels(2, true).unwrap();
    assert_eq!(counters::take(&INSIDE), 0);
    // A no-op mask factor leaves the alpha untouched.
    let plain = with(255, false, false).layer_pixels(2, false).unwrap();
    let mut none = q.clone();
    none.layers[2].mask = None;
    assert_eq!(bits(&plain), bits(&none.layer_pixels(2, false).unwrap()));
}

// ------------------------------------------------------------------ placed layers

fn warps() -> Vec<Option<Warp>> {
    let mut mesh = vec![];
    for j in 0..4 {
        for i in 0..4 {
            mesh.push([i as f64 * 20.0 / 3.0 + if j == 1 || j == 2 { 2.0 } else { 0.0 }, j as f64 * 10.0 / 3.0]);
        }
    }
    vec![
        None,
        Some(Warp { style: "warpArc".into(), bend: 40.0, bounds: [0.0, 0.0, 20.0, 10.0], ..Default::default() }),
        Some(Warp { style: "warpCustom".into(), bounds: [0.0, 0.0, 20.0, 10.0], cols: 1, rows: 1, mesh, ..Default::default() }),
    ]
}

fn content(w: u32, h: u32) -> Pixels {
    Pixels {
        width: w,
        height: h,
        data: (0..w * h).map(|i| [(i % w) as f32 / w as f32, (i / w) as f32 / h as f32, 0.5, if i % 7 == 0 { 0.4 } else { 1.0 }]).collect(),
    }
}

#[test]
fn placed_bake_matches_the_reference() {
    let quads = [
        [[10.0, 10.0], [50.0, 0.0], [50.0, 40.0], [10.0, 30.0]],
        [[0.0, 0.0], [20.0, 0.0], [20.0, 10.0], [0.0, 10.0]],
        [[3.0, 4.0], [33.0, 9.0], [28.0, 29.0], [-2.0, 21.0]],
        [[0.0, 0.0], [0.0, 0.0], [0.0, 0.0], [0.0, 0.0]],
    ];
    let c = content(20, 10);
    for quad in quads {
        for warp in warps() {
            let so = SmartObject { uuid: "x".into(), quad, size: None, warp };
            let what = format!("{quad:?} {:?}", so.warp.as_ref().map(|w| &w.style));
            assert_eq!(so.placed_bounds(20.0, 10.0), ref_placed_bounds(&so, 20.0, 10.0), "{what}");
            for k in [0.5, 1.0, 2.0] {
                assert_eq!(so.placed_size(20.0, 10.0, k), ref_placed_size(&so, 20.0, 10.0, k), "{what} k {k}");
                for size in [None, Some((30, 25)), Some((0, 0))] {
                    let got = so.render_placed(&c, k, size);
                    let want = ref_render_placed(&so, &c, k, size);
                    assert_eq!(
                        got.as_ref().map(|(p, o)| (p.width, p.height, bits(p), *o)),
                        want.as_ref().map(|(p, o)| (p.width, p.height, bits(p), *o)),
                        "{what} k {k} size {size:?}"
                    );
                }
            }
            for p in [[0.0, 0.0], [10.0, 5.0], [20.0, 10.0], [7.5, 2.25]] {
                assert_eq!(so.map(p, 20.0, 10.0), ref_map(&so, p, 20.0, 10.0), "{what} {p:?}");
            }
        }
    }
    assert!(SmartObject { uuid: "x".into(), quad: [[0.0; 2]; 4], size: None, warp: None }.render_placed(&content(0, 0), 1.0, None).is_none());
}

#[test]
fn placed_bake_solves_the_quad_once() {
    let so = SmartObject { uuid: "x".into(), quad: [[10.0, 10.0], [50.0, 0.0], [50.0, 40.0], [10.0, 30.0]], size: None, warp: None };
    counters::take(&QUAD);
    so.placed_bounds(20.0, 10.0);
    assert_eq!(counters::take(&QUAD), 1, "33×33 mapped points, one solve (was 1089)");
    so.placed_size(20.0, 10.0, 1.0);
    assert_eq!(counters::take(&QUAD), 1);
    // The bounds, the size and the 49×49 grid of one bake share a single solve (was 1089 × 2 + 2401).
    so.render_placed(&content(20, 10), 1.0, None).unwrap();
    assert_eq!(counters::take(&QUAD), 1);
    so.render_placed(&content(20, 10), 1.0, Some((40, 40))).unwrap();
    assert_eq!(counters::take(&QUAD), 1);
    // A single `map` still solves for itself.
    so.map([1.0, 1.0], 20.0, 10.0);
    assert_eq!(counters::take(&QUAD), 1);
}
