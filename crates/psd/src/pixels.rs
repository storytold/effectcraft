#[cfg(test)]
use super::counters;
use super::limits::{Budget, DECODE_BUDGET, Error, Result, decode_pixels, finite_sample, inflate, release_bytes, too_large};
use super::model::{ColorMode, Layer, Pixels, Rect};
use super::{Psd, Reader};
use std::borrow::Cow;

impl Psd {
    fn bytes_per_sample(&self) -> usize {
        match self.depth {
            16 => 2,
            32 => 4,
            _ => 1,
        }
    }

    /// The raw big-endian bytes of one channel of `w`×`h` samples at `offset` (compression field
    /// first): `row_bytes(w) × h` of them with the compression and the prediction undone.
    /// Uncompressed data is borrowed from the file, not copied; any other buffer is charged to
    /// `budget` and the caller releases it (see [`release_bytes`]) once it is dropped.
    fn channel_bytes(&self, offset: usize, len: usize, w: usize, h: usize, budget: &mut Budget) -> Result<Cow<'_, [u8]>> {
        let end = offset.checked_add(len).filter(|&e| e <= self.data.len()).ok_or(Error::Truncated)?;
        let mut r = Reader::at(self.data.get(..end).ok_or(Error::Truncated)?, offset);
        let comp = r.u16()?;
        decode_pixels(w, h)?;
        self.plane_bytes(comp, &mut r, w, h, budget)
    }

    /// The raw bytes (`row_bytes(w)` × `h`, ZIP prediction undone) of one channel whose pixel data
    /// follows `r`'s position (after the compression field). Uncompressed data is borrowed, not
    /// copied; the other buffers are charged to `budget`.
    pub(crate) fn plane_bytes<'a>(&self, comp: u16, r: &mut Reader<'a>, w: usize, h: usize, budget: &mut Budget) -> Result<Cow<'a, [u8]>> {
        let rowb = self.row_bytes(w);
        let size = rowb.checked_mul(h).ok_or_else(|| too_large(w, h))?;
        Ok(match self.open_planes(comp, r, 1, 1, (w, h), budget)? {
            PlaneStream::Raw => Cow::Borrowed(r.bytes(size)?),
            PlaneStream::Rle(counts) => Cow::Owned(Self::unpack_plane(&counts, 0, r, rowb, h, budget)?),
            PlaneStream::Zip(mut v) => {
                v.resize(size, 0);
                self.zip_plane(comp, &mut v, 0, w, h)?;
                Cow::Owned(v)
            }
        })
    }

    /// Prepare the pixel data at `r` (just after the compression field) of `total` planes of
    /// `w`×`h` samples, of which the first `want` are decoded: the RLE row sizes of every plane,
    /// or the inflated ZIP data, which holds all planes one after another (charged to `budget`
    /// before it is inflated).
    fn open_planes(&self, comp: u16, r: &mut Reader, total: usize, want: usize, size: (usize, usize), budget: &mut Budget) -> Result<PlaneStream> {
        let (w, h) = size;
        let size = self.row_bytes(w).checked_mul(h).ok_or_else(|| too_large(w, h))?;
        Ok(match comp {
            0 => PlaneStream::Raw,
            1 => {
                let n = total.saturating_mul(h);
                let mut counts = Vec::with_capacity(n.min(r.remaining() / 2));
                for _ in 0..n {
                    counts.push(if self.psb { r.u32()? as usize } else { r.u16()? as usize });
                }
                PlaneStream::Rle(counts)
            }
            2 | 3 => {
                let rest = r.bytes(r.remaining())?;
                // One stream holds every plane: what it may decode to is what they need.
                let all = size.checked_mul(want).ok_or_else(|| too_large(w, h))?;
                budget.charge(all as u64)?;
                PlaneStream::Zip(inflate(rest, all)?)
            }
            c => return Err(Error::Unsupported(format!("compression {c}"))),
        })
    }

    /// The raw bytes (`row_bytes(w)` × `h`, ZIP prediction undone) of plane `c`: raw and RLE
    /// planes are read from `r` in order, raw ones borrowed from the file data, ZIP ones from the
    /// inflated buffer (un-predicted in place; already charged by `open_planes`), RLE ones charged
    /// to `budget`.
    fn decode_plane<'x, 'a: 'x>(
        &self,
        comp: u16,
        planes: &'x mut PlaneStream,
        r: &mut Reader<'a>,
        c: usize,
        size: (usize, usize),
        budget: &mut Budget,
    ) -> Result<Cow<'x, [u8]>> {
        let (w, h) = size;
        let rowb = self.row_bytes(w);
        let size = rowb.checked_mul(h).ok_or_else(|| too_large(w, h))?;
        Ok(match planes {
            PlaneStream::Raw => Cow::Borrowed(r.bytes(size)?),
            PlaneStream::Rle(counts) => Cow::Owned(Self::unpack_plane(counts, c, r, rowb, h, budget)?),
            PlaneStream::Zip(v) => Cow::Borrowed(self.zip_plane(comp, v, c, w, h)?),
        })
    }

    /// Unpack the `h` RLE rows of plane `c` (row byte counts `counts`, plane after plane) into a
    /// buffer charged to `budget`.
    fn unpack_plane(counts: &[usize], c: usize, r: &mut Reader, rowb: usize, h: usize, budget: &mut Budget) -> Result<Vec<u8>> {
        let mut out = budget.vec::<u8>(rowb.checked_mul(h).ok_or(Error::Truncated)?)?;
        for row in 0..h {
            let n = *counts.get(c * h + row).ok_or(Error::Truncated)?;
            unpack_bits(r.bytes(n)?, rowb, &mut out)?;
        }
        Ok(out)
    }

    /// Plane `c` of the inflated ZIP data of consecutive planes, un-predicted in place (`comp` 3).
    pub(crate) fn zip_plane<'v>(&self, comp: u16, v: &'v mut [u8], c: usize, w: usize, h: usize) -> Result<&'v [u8]> {
        let rowb = self.row_bytes(w);
        let plane = v.get_mut(c * rowb * h..(c + 1) * rowb * h).ok_or(Error::Truncated)?;
        if comp == 3 {
            self.unpredict(plane, w, h);
        }
        Ok(plane)
    }

    /// One channel as `w * h` floats (0..1), charged to `budget` (the caller releases it).
    fn channel_floats(&self, offset: usize, len: usize, w: usize, h: usize, budget: &mut Budget) -> Result<Vec<f32>> {
        let raw = self.channel_bytes(offset, len, w, h, budget)?;
        let mut out = budget.filled(decode_pixels(w, h)?, 0.0f32)?;
        self.for_each_sample(&raw, w, h, |i, v| {
            if let Some(o) = out.get_mut(i) {
                *o = v;
            }
        });
        release_bytes(budget, raw);
        Ok(out)
    }

    pub(crate) fn row_bytes(&self, w: usize) -> usize {
        if self.depth == 1 { w.div_ceil(8) } else { w * self.bytes_per_sample() }
    }

    pub(crate) fn unpredict(&self, v: &mut [u8], w: usize, h: usize) {
        match self.depth {
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
                // One scratch row for the whole plane.
                let mut planar = vec![0u8; w * 4];
                #[cfg(test)]
                counters::bump(&counters::SCRATCH);
                for row in v.chunks_exact_mut(w * 4).take(h) {
                    for i in 1..row.len() {
                        row[i] = row[i].wrapping_add(row[i - 1]);
                    }
                    // Bytes are stored plane by plane (all first bytes, then all second bytes…).
                    planar.copy_from_slice(row);
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
    }

    /// Hand every sample of `raw` (`w`×`h`, row-major, in order) to `f` as a float. Samples a short
    /// `raw` does not have are not reported.
    pub(crate) fn for_each_sample(&self, raw: &[u8], w: usize, h: usize, mut f: impl FnMut(usize, f32)) {
        let n = w.saturating_mul(h);
        match self.depth {
            1 => {
                let rowb = w.div_ceil(8);
                for y in 0..h {
                    for x in 0..w {
                        let b = raw.get(y * rowb + x / 8).copied().unwrap_or(0);
                        // 1 = black.
                        f(y * w + x, if b & (0x80 >> (x % 8)) != 0 { 0.0 } else { 1.0 });
                    }
                }
            }
            16 => raw.as_chunks::<2>().0.iter().take(n).enumerate().for_each(|(i, c)| f(i, u16::from_be_bytes([c[0], c[1]]) as f32 / 65535.0)),
            // NaN reads as 0, +inf as white (1.0, like the integer depths' maximum), -inf as 0.
            32 => raw.as_chunks::<4>().0.iter().take(n).enumerate().for_each(|(i, c)| f(i, finite_sample(f32::from_be_bytes([c[0], c[1], c[2], c[3]])))),
            _ => raw.iter().take(n).enumerate().for_each(|(i, &b)| f(i, b as f32 / 255.0)),
        }
    }

    /// Convert colour values (the document mode's channels, missing ones 0) to RGB.
    pub(crate) fn rgb_from(&self, v: [f32; 4]) -> [f32; 3] {
        let g = |k: usize| v.get(k).copied().unwrap_or(0.0);
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

    /// Write merged-image channel `c` (raw bytes) into the result: the first three colour channels
    /// and the alpha channel into their slot of a pixel, the fourth CMYK channel into `black`.
    fn store_merged_channel(&self, c: usize, raw: &[u8], data: &mut [[f32; 4]], black: &mut Option<Vec<f32>>) {
        let (w, h, cc) = (self.width as usize, self.height as usize, self.mode.color_channels());
        if c < cc.min(3) {
            self.for_each_sample(raw, w, h, |i, v| {
                if let Some(p) = data.get_mut(i) {
                    p[c] = v;
                }
            });
        } else if c == 3 && c < cc {
            if let Some(k) = black {
                self.for_each_sample(raw, w, h, |i, v| {
                    if let Some(o) = k.get_mut(i) {
                        *o = v;
                    }
                });
            }
        } else if c == cc && self.merged_alpha {
            self.for_each_sample(raw, w, h, |i, v| {
                if let Some(p) = data.get_mut(i) {
                    p[3] = v;
                }
            });
        }
    }

    /// The merged (flattened) image.
    pub fn composite(&self) -> Result<Pixels> {
        self.composite_with_budget(DECODE_BUDGET)
    }

    /// [`Self::composite`] with at most `limit` bytes live at once. The channels are written
    /// straight into the result, so the peak is the result (16 bytes a pixel), the CMYK black
    /// channel, and the one buffer a channel (or, for ZIP, all channels) decodes through.
    pub(crate) fn composite_with_budget(&self, limit: u64) -> Result<Pixels> {
        let mut budget = Budget::new(limit);
        let (w, h) = (self.width as usize, self.height as usize);
        let n = decode_pixels(w, h)?;
        let mut r = Reader::at(&self.data, self.merged_offset);
        let comp = r.u16()?;
        let nch = self.channels as usize;
        let cc = self.mode.color_channels();
        let want = (cc + usize::from(self.merged_alpha)).min(nch);
        let rowb = self.row_bytes(w);
        let size = rowb.checked_mul(h).ok_or_else(|| too_large(w, h))?;
        // The peak: the result, the CMYK black channel and what the channels decode through.
        let black_bytes = if self.mode == ColorMode::Cmyk && want > 3 { 4 * n as u64 } else { 0 };
        let transient = if matches!(comp, 2 | 3) { (size as u64).saturating_mul(want as u64) } else { size as u64 };
        budget.check((16 * n as u64).saturating_add(black_bytes).saturating_add(transient))?;
        // Opaque black until a channel says otherwise.
        let mut data = budget.filled(n, [0.0f32, 0.0, 0.0, 1.0])?;
        let mut black = if self.mode == ColorMode::Cmyk && want > 3 { Some(budget.filled(n, 0.0f32)?) } else { None };
        let mut stream = self.open_planes(comp, &mut r, nch, want, (w, h), &mut budget)?;
        for c in 0..want {
            let raw = self.decode_plane(comp, &mut stream, &mut r, c, (w, h), &mut budget)?;
            self.store_merged_channel(c, &raw, &mut data, &mut black);
            release_bytes(&mut budget, raw);
        }
        for (i, p) in data.iter_mut().enumerate() {
            let k = black.as_ref().and_then(|k| k.get(i)).copied().unwrap_or(0.0);
            let [r, g, b] = self.rgb_from([p[0], p[1], p[2], k]);
            *p = [r, g, b, p[3]];
        }
        Ok(Pixels { width: w as u32, height: h as u32, data })
    }

    /// One layer's pixels with its layer mask (and vector-less fill colour) applied to alpha.
    /// `canvas`: document-sized with the layer in place; otherwise the layer's own bounds.
    pub fn layer_pixels(&self, index: usize, canvas: bool) -> Result<Pixels> {
        self.layer_pixels_with_budget(index, canvas, DECODE_BUDGET)
    }

    /// [`Self::layer_pixels`] with at most `limit` bytes live at once: the result (16 bytes a
    /// pixel) with the channels written straight into it, the CMYK black channel, the one channel
    /// being decoded, and the mask.
    pub(crate) fn layer_pixels_with_budget(&self, index: usize, canvas: bool, limit: u64) -> Result<Pixels> {
        let mut budget = Budget::new(limit);
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
        let n = decode_pixels(ow, oh)?;
        let src = fill_rect;
        let (sw, sh) = (src.width() as usize, src.height() as usize);
        // The peak: the result, then either the CMYK black channel with the channel being decoded,
        // or the mask as floats with its bytes. Checked before anything is allocated.
        let plane = |w: usize, h: usize| (self.row_bytes(w) as u64).saturating_mul(h as u64);
        let mut pixels_peak = 0u64;
        if l.has_pixels() && sw > 0 && sh > 0 {
            pixels_peak = plane(sw, sh).saturating_add(if self.mode == ColorMode::Cmyk { 4 * n as u64 } else { 0 });
        }
        let mut mask_peak = 0u64;
        if let Some(m) = l.mask.filter(|m| !m.disabled) {
            let (mw, mh) = (m.rect.width() as usize, m.rect.height() as usize);
            if mw > 0 && mh > 0 && l.channels.iter().any(|c| c.id == -2) {
                mask_peak = plane(mw, mh).saturating_add((mw as u64).saturating_mul(mh as u64).saturating_mul(4));
            }
        }
        budget.check((16 * n as u64).saturating_add(pixels_peak.max(mask_peak)))?;
        let mut out = budget.filled(n, [0.0f32; 4])?;
        if sw > 0 && sh > 0 {
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
            let dest = |y: usize, x: usize| (y as i64 - oy) as usize * ow + (x as i64 - ox) as usize;
            let fill = l.fill.map(|c| c.map(|v| v as f32));
            match fill {
                Some(f) if !l.has_pixels() => {
                    for y in ys {
                        for x in xs.clone() {
                            if let Some(p) = out.get_mut(dest(y, x)) {
                                *p = [f[0], f[1], f[2], 1.0];
                            }
                        }
                    }
                }
                _ => {
                    // Opaque unless there is an alpha channel.
                    for y in ys.clone() {
                        for x in xs.clone() {
                            if let Some(p) = out.get_mut(dest(y, x)) {
                                p[3] = 1.0;
                            }
                        }
                    }
                    let cc = self.mode.color_channels();
                    let mut black: Option<Vec<f32>> = None;
                    if l.has_pixels() {
                        for c in &l.channels {
                            let slot = match c.id {
                                -1 => 3,
                                id if id >= 0 && (id as usize) < cc => id as usize,
                                _ => continue,
                            };
                            let raw = self.channel_bytes(c.offset, c.len, sw, sh, &mut budget)?;
                            // The fourth CMYK channel is the black plate, not a slot of the result.
                            let to_black = c.id == 3;
                            if to_black && black.is_none() {
                                black = Some(budget.filled(out.len(), 0.0f32)?);
                            }
                            // Samples arrive in row-major order of the source.
                            let (mut sy, mut sx) = (0usize, 0usize);
                            self.for_each_sample(&raw, sw, sh, |_, v| {
                                if ys.contains(&sy) && xs.contains(&sx) {
                                    let d = dest(sy, sx);
                                    if to_black {
                                        if let Some(o) = black.as_mut().and_then(|k| k.get_mut(d)) {
                                            *o = v;
                                        }
                                    } else if let Some(p) = out.get_mut(d) {
                                        p[slot] = v;
                                    }
                                }
                                sx += 1;
                                if sx == sw {
                                    sx = 0;
                                    sy += 1;
                                }
                            });
                            release_bytes(&mut budget, raw);
                        }
                    }
                    for y in ys {
                        for x in xs.clone() {
                            let d = dest(y, x);
                            let k = black.as_ref().and_then(|k| k.get(d)).copied().unwrap_or(0.0);
                            if let Some(p) = out.get_mut(d) {
                                let [r, g, b] = self.rgb_from([p[0], p[1], p[2], k]);
                                *p = [r, g, b, p[3]];
                            }
                        }
                    }
                    if let Some(k) = black {
                        budget.release(k.len() as u64 * 4);
                    }
                }
            }
        }
        self.apply_layer_mask(l, out_rect, &mut out, &mut budget)?;
        Ok(Pixels { width: ow as u32, height: oh as u32, data: out })
    }

    /// Apply the raster mask to alpha, including its default outside the mask bounds.
    fn apply_layer_mask(&self, l: &Layer, out_rect: Rect, out: &mut [[f32; 4]], budget: &mut Budget) -> Result<()> {
        let (ow, oh) = (out_rect.width() as usize, out_rect.height() as usize);
        // Layer mask: multiplies alpha (also outside the mask rect, by its default colour).
        if let Some(m) = l.mask.filter(|m| !m.disabled) {
            let mc = l.channels.iter().find(|c| c.id == -2);
            let (mw, mh) = (m.rect.width() as usize, m.rect.height() as usize);
            let mask = match mc {
                Some(c) if mw > 0 && mh > 0 => Some(self.channel_floats(c.offset, c.len, mw, mh, budget)?),
                _ => None,
            };
            let def = m.default_color as f32 / 255.0;
            // The factor outside the mask rect, or everywhere when there is no mask data.
            let outside = if m.inverted { 1.0 - def } else { def };
            let scale = |px: &mut [[f32; 4]], v: f32| {
                if v != 1.0 {
                    for p in px {
                        p[3] *= v;
                    }
                }
            };
            match &mask {
                None => scale(out, outside),
                Some(mk) => {
                    // The columns of the output that fall inside the mask rect (the same for every row).
                    let lo = (i64::from(m.rect.left) - i64::from(out_rect.left)).clamp(0, ow as i64);
                    let hi = (i64::from(m.rect.right) - i64::from(out_rect.left)).clamp(lo, ow as i64);
                    let (lo, hi) = (lo as usize, hi as usize);
                    for (y, row) in out.chunks_exact_mut(ow.max(1)).take(oh).enumerate() {
                        let py = i64::from(out_rect.top) + y as i64;
                        #[cfg(test)]
                        counters::bump(&counters::INSIDE);
                        if py < i64::from(m.rect.top) || py >= i64::from(m.rect.bottom) {
                            scale(row, outside);
                            continue;
                        }
                        let (left, rest) = row.split_at_mut(lo);
                        let (mid, right) = rest.split_at_mut(hi - lo);
                        scale(left, outside);
                        scale(right, outside);
                        // The mask row under `mid`: its first column is output column `lo`.
                        let base = (py - i64::from(m.rect.top)) as usize * mw + (i64::from(out_rect.left) + lo as i64 - i64::from(m.rect.left)) as usize;
                        for (i, p) in mid.iter_mut().enumerate() {
                            let v = mk.get(base + i).copied().unwrap_or(def);
                            p[3] *= if m.inverted { 1.0 - v } else { v };
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

/// The pixel data of a run of planes, as prepared by `Psd::open_planes`.
enum PlaneStream {
    Raw,
    /// RLE row byte counts of every plane, plane after plane.
    Rle(Vec<usize>),
    /// The inflated ZIP data of all planes.
    Zip(Vec<u8>),
}

/// PackBits decoding of one row (appends exactly `row` bytes, zero-padded).
pub(crate) fn unpack_bits(src: &[u8], row: usize, out: &mut Vec<u8>) -> Result<()> {
    let start = out.len();
    let mut i = 0;
    while i < src.len() && out.len() - start < row {
        let n = src[i] as i8;
        i += 1;
        if n >= 0 {
            let k = n as usize + 1;
            let s = src.get(i..i + k).ok_or(Error::Truncated)?;
            out.extend_from_slice(s);
            i += k;
        } else if n != -128 {
            let k = (1 - n as isize) as usize;
            let v = *src.get(i).ok_or(Error::Truncated)?;
            out.extend(std::iter::repeat_n(v, k));
            i += 1;
        }
    }
    out.resize(start + row, 0);
    Ok(())
}

/// PackBits encoding of one row.
pub(crate) fn pack_bits(src: &[u8]) -> Vec<u8> {
    let mut out = vec![];
    let mut i = 0;
    while i < src.len() {
        // run of identical bytes
        let mut run = 1;
        while i + run < src.len() && src[i + run] == src[i] && run < 128 {
            run += 1;
        }
        if run >= 2 {
            out.push((1 - run as isize) as i8 as u8);
            out.push(src[i]);
            i += run;
            continue;
        }
        // literal run
        let start = i;
        let mut n = 0;
        while i < src.len() && n < 128 {
            if i + 1 < src.len() && src[i + 1] == src[i] {
                break;
            }
            i += 1;
            n += 1;
        }
        if n == 0 {
            i += 1;
            n = 1;
        }
        out.push((n - 1) as u8);
        out.extend_from_slice(&src[start..start + n]);
    }
    out
}

pub(crate) fn srgb_encode(v: f32) -> f32 {
    if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
}

/// CIE L*a*b* (D50) to sRGB (D65 via the Bradford-adapted matrix), clamped.
pub(crate) fn lab_to_rgb(l: f32, a: f32, b: f32) -> [f32; 3] {
    let fy = (l + 16.0) / 116.0;
    let fx = fy + a / 500.0;
    let fz = fy - b / 200.0;
    let f = |t: f32| if t > 6.0 / 29.0 { t * t * t } else { 3.0 * (6.0f32 / 29.0).powi(2) * (t - 4.0 / 29.0) };
    let (x, y, z) = (0.964_22 * f(fx), f(fy), 0.825_21 * f(fz));
    // XYZ (D50) → linear sRGB.
    let r = 3.133_856 * x - 1.616_867 * y - 0.490_615 * z;
    let g = -0.978_768 * x + 1.916_142 * y + 0.033_454 * z;
    let bb = 0.071_945 * x - 0.228_991 * y + 1.405_243 * z;
    [r, g, bb].map(|v| srgb_encode(v.clamp(0.0, 1.0)))
}
