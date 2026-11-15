//! Sampled images: `image`, `imagemask` and `colorimage` (PostScript Language Reference §4.10),
//! in the operand and the dictionary forms, reading from strings, procedures, `currentfile` and
//! its filters.

use std::sync::Arc;

use effectcraft_svg::{Image as SvgImage, Node};

use super::scanner::Src;
use super::value::{Filter, Flow, ImageCs, V, matrix_of};
use super::{Affine, Interp, Rc, RefCell, is_white};
use crate::color::cmyk_to_rgb;
use crate::object::{ascii_hex, ascii85, lzw, run_length};

/// The longest side and the largest area of an image decoded (larger ones are refused rather
/// than exhausting memory).
const MAX_SIDE: usize = 16384;
const MAX_PIXELS: usize = 64 << 20;
/// The most bytes a chain of filters decodes.
const MAX_DECODED: usize = 1 << 30;

struct Params {
    width: usize,
    height: usize,
    bits: usize,
    matrix: Affine,
    decode: Option<Vec<f64>>,
    sources: Vec<V>,
    cs: ImageCs,
    /// A stencil (`imagemask`): `true` when sample 1 paints.
    stencil: Option<bool>,
}

fn reads_file(v: &V) -> bool {
    match v {
        V::File | V::Filter(_) => true,
        V::Proc(p) => p.iter().any(|x| match x {
            V::Name(n) => &**n == "currentfile",
            V::File | V::Filter(_) => true,
            _ => false,
        }),
        _ => false,
    }
}

impl Interp {
    /// Pop the operands of `n`.
    fn image_params(&mut self, n: &str) -> Option<Params> {
        if n != "colorimage" && matches!(self.st.last(), Some(V::Dict(_))) {
            let V::Dict(d) = self.pop() else { return None };
            let d = d.borrow();
            let int = |k: &str| d.get(k).and_then(V::num).filter(|v| v.is_finite() && *v >= 0.0).map(|v| v as usize);
            // A stencil's [1 0] paints where the sample is 1.
            let stencil = (n == "imagemask").then(|| d.get("Decode").and_then(V::items).unwrap_or_default().first().and_then(V::num) == Some(1.0));
            let sources = match (d.get("DataSource"), d.get("MultipleDataSources")) {
                (Some(ds), Some(V::Bool(true))) => ds.items().unwrap_or_default(),
                (Some(ds), _) => vec![ds.clone()],
                _ => vec![],
            };
            return Some(Params {
                width: int("Width")?,
                height: int("Height")?,
                bits: if stencil.is_some() { 1 } else { int("BitsPerComponent")? },
                matrix: d.get("ImageMatrix").and_then(matrix_of)?,
                decode: if stencil.is_some() { None } else { d.get("Decode").and_then(V::items).map(|v| v.iter().filter_map(V::num).collect()) },
                sources,
                cs: self.gs.image_cs.clone(),
                stencil,
            });
        }
        let (sources, cs) = if n == "colorimage" {
            let ncomp = self.popn().max(1.0) as usize;
            let multi = matches!(self.pop(), V::Bool(true));
            let mut s: Vec<V> = (0..if multi { ncomp.min(4) } else { 1 }).map(|_| self.pop()).collect();
            s.reverse();
            let cs = match ncomp {
                1 => ImageCs::Gray,
                3 => ImageCs::Rgb,
                4 => ImageCs::Cmyk,
                _ => ImageCs::Other,
            };
            (s, cs)
        } else {
            (vec![self.pop()], ImageCs::Gray)
        };
        let matrix = matrix_of(&self.pop());
        let third = self.pop();
        let height = self.popn();
        let width = self.popn();
        let sized = |v: f64| (v.is_finite() && v >= 0.0).then_some(v as usize);
        let (bits, stencil) = match (n, third) {
            ("imagemask", V::Bool(b)) => (1, Some(b)),
            (_, V::Num(b)) if b.is_finite() && b >= 0.0 => (b as usize, None),
            _ => return None,
        };
        Some(Params { width: sized(width)?, height: sized(height)?, bits, matrix: matrix?, decode: None, sources, cs, stencil })
    }

    /// Up to `want` bytes of a filter. Its data is decoded the first time it is read: the data
    /// follows the operator that reads it, not the `filter` that was set up before it.
    pub(super) fn filter_take(&mut self, f: &Rc<RefCell<Filter>>, want: usize, src: &mut Src) -> Vec<u8> {
        let mut f = f.borrow_mut();
        if f.data.is_none() {
            f.data = Some(decode_chain(&f.names, src).unwrap_or_default());
        }
        let pos = f.pos;
        let out: Vec<u8> = f.data.as_deref().and_then(|d| d.get(pos..)).map(|d| d[..want.min(d.len())].to_vec()).unwrap_or_default();
        f.pos = pos + out.len();
        out
    }

    /// `file string readstring` / `readhexstring` while an image reads its data.
    pub(super) fn readstring(&mut self, hex: bool, src: &mut Src) {
        let target = self.pop();
        let file = self.pop();
        let want = match &target {
            V::Str(s) => s.borrow().len(),
            _ => 0,
        };
        let got = match &file {
            V::Filter(f) => self.filter_take(f, want, src),
            V::File if hex => src.read_hex(want),
            V::File => src.read_bytes(want),
            _ => vec![],
        };
        let full = got.len() == want;
        if let V::Str(s) = &target {
            let mut s = s.borrow_mut();
            let n = got.len().min(s.len());
            s[..n].copy_from_slice(&got[..n]);
        }
        self.push(V::Str(Rc::new(RefCell::new(got))));
        self.push(V::Bool(full));
    }

    /// `readstring` reads real data from a filter always, and from the file while an image
    /// reads it (elsewhere the file is the program being run).
    pub(super) fn reads_real_data(&self) -> bool {
        self.reading_image || matches!(self.st.iter().rev().nth(1), Some(V::Filter(_)))
    }

    /// One read from an image's data source (empty at the end of the data).
    fn read_source(&mut self, s: &V, want: usize, src: &mut Src) -> Result<Vec<u8>, Flow> {
        Ok(match s {
            V::Filter(f) => self.filter_take(f, want, src),
            V::File => src.read_bytes(want),
            V::Str(t) => t.borrow().clone(),
            V::Proc(_) => {
                self.exec_obj(s.clone(), src)?;
                match self.pop() {
                    V::Str(t) => t.borrow().clone(),
                    _ => vec![],
                }
            }
            _ => vec![],
        })
    }

    /// `image`, `imagemask` and `colorimage`.
    pub(super) fn image_op(&mut self, n: &str, src: &mut Src) -> Result<(), Flow> {
        let Some(p) = self.image_params(n) else {
            self.b.skip("image");
            return Ok(());
        };
        let from_file = p.sources.iter().any(reads_file);
        // The one whitespace character that ended the operator's name.
        if from_file && src.d.get(src.pos).copied().is_some_and(is_white) {
            src.pos += if src.d[src.pos..].starts_with(b"\r\n") { 2 } else { 1 };
        }
        match self.decode_image(&p, src)? {
            Some(rgba) => self.b.push(Node::Image(SvgImage {
                name: "Image".into(),
                transform: self.gs.ctm * p.matrix.inverse(),
                width: p.width as u32,
                height: p.height as u32,
                rgba: Arc::new(rgba),
                opacity: 1.0,
            })),
            None => {
                if from_file && !p.sources.iter().any(|s| matches!(s, V::Filter(_))) {
                    src.skip_image_data();
                }
                self.b.skip("image");
            }
        }
        Ok(())
    }

    fn decode_image(&mut self, p: &Params, src: &mut Src) -> Result<Option<Vec<u8>>, Flow> {
        let ncomp = if p.stencil.is_some() { 1 } else { p.cs.components() };
        let valid = ncomp > 0
            && matches!(p.bits, 1 | 2 | 4 | 8 | 12 | 16)
            && (1..=MAX_SIDE).contains(&p.width)
            && (1..=MAX_SIDE).contains(&p.height)
            && p.width * p.height <= MAX_PIXELS
            && p.matrix.determinant().abs() > 0.0
            && (p.sources.len() == 1 || p.sources.len() == ncomp);
        if !valid {
            return Ok(None);
        }
        let planar = p.sources.len() > 1;
        let row_samples = if planar { p.width } else { p.width * ncomp };
        let row_bytes = (row_samples * p.bits).div_ceil(8);
        let need = row_bytes * p.height;
        let mut planes = vec![Vec::<u8>::new(); p.sources.len()];
        self.reading_image = true;
        let filled = self.fill_planes(&p.sources, &mut planes, need, src);
        self.reading_image = false;
        filled?;
        if planes.iter().any(Vec::is_empty) {
            return Ok(None);
        }
        for pl in &mut planes {
            pl.resize(need, 0);
        }
        let max = ((1u32 << p.bits) - 1) as f64;
        let decode: Vec<f64> = match &p.decode {
            Some(d) if d.len() >= ncomp * 2 => d.clone(),
            _ if matches!(p.cs, ImageCs::Indexed { .. }) => vec![0.0, max],
            _ => [0.0, 1.0].repeat(ncomp),
        };
        let mut rgba = vec![0u8; p.width * p.height * 4];
        let mut comps = [0.0f64; 4];
        let to8 = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        let color = self.gs.color.map(to8);
        for (i, px) in rgba.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let (y, x) = (i / p.width, i % p.width);
            for (c, slot) in comps.iter_mut().enumerate().take(ncomp) {
                let (plane, index) = if planar { (c, x) } else { (0, x * ncomp + c) };
                let s = planes.get(plane).map_or(0, |d| sample(d, y * row_bytes, index, p.bits)) as f64;
                let (lo, hi) = (decode.get(c * 2).copied().unwrap_or(0.0), decode.get(c * 2 + 1).copied().unwrap_or(1.0));
                *slot = lo + s * (hi - lo) / max;
            }
            if let Some(paint_one) = p.stencil {
                px[..3].copy_from_slice(&color);
                px[3] = if (comps[0] >= 0.5) == paint_one { 255 } else { 0 };
            } else {
                px[..3].copy_from_slice(&p.cs.to_rgb(&comps[..ncomp]).map(to8));
                px[3] = 255;
            }
        }
        Ok(Some(rgba))
    }

    /// Read every source round by round until each plane has `need` bytes or runs dry.
    fn fill_planes(&mut self, sources: &[V], planes: &mut [Vec<u8>], need: usize, src: &mut Src) -> Result<(), Flow> {
        loop {
            self.spend(1)?;
            let mut progress = false;
            for (s, pl) in sources.iter().zip(planes.iter_mut()) {
                if pl.len() >= need {
                    continue;
                }
                let got = self.read_source(s, need - pl.len(), src)?;
                let keep = got.len().min(need - pl.len());
                progress |= keep > 0;
                pl.extend_from_slice(&got[..keep]);
            }
            if !progress {
                return Ok(());
            }
        }
    }
}

/// Sample `index` of the row at byte `row` of `data`.
fn sample(data: &[u8], row: usize, index: usize, bits: usize) -> u32 {
    let bit = index * bits;
    let at = |i: usize| data.get(row + i).copied().unwrap_or(0) as u32;
    match bits {
        8 => at(index),
        16 => (at(index * 2) << 8) | at(index * 2 + 1),
        12 => {
            let i = bit / 8;
            if bit.is_multiple_of(8) { (at(i) << 4) | (at(i + 1) >> 4) } else { ((at(i) & 15) << 8) | at(i + 1) }
        }
        _ => (at(bit / 8) >> (8 - bits - bit % 8)) & ((1 << bits) - 1),
    }
}

/// Decode a chain of filters reading from the current file, innermost first. The first must
/// know where its data ends in the file.
fn decode_chain(names: &[String], src: &mut Src) -> Option<Vec<u8>> {
    let first = names.first()?;
    while src.d.get(src.pos).copied().is_some_and(is_white) {
        src.pos += 1;
    }
    let rest = src.d.get(src.pos..)?;
    let find = |needle: &[u8]| rest.windows(needle.len()).position(|w| w == needle).map(|p| p + needle.len());
    let len = match first.as_str() {
        "ASCII85Decode" => find(b"~>").unwrap_or(rest.len()),
        "ASCIIHexDecode" => find(b">").unwrap_or(rest.len()),
        "RunLengthDecode" => run_length_extent(rest),
        _ => return None,
    };
    src.pos += len;
    let data = rest.get(..len)?;
    decode_bytes(names, data)
}

/// Run `data` through a chain of filters.
pub(super) fn decode_bytes(names: &[String], data: &[u8]) -> Option<Vec<u8>> {
    let mut data = data.to_vec();
    for name in names {
        data = match name.as_str() {
            "ASCII85Decode" => ascii85(&data),
            "ASCIIHexDecode" => ascii_hex(&data, MAX_DECODED)?,
            "RunLengthDecode" => run_length(&data, MAX_DECODED)?,
            "LZWDecode" => lzw(&data, true, MAX_DECODED)?,
            _ => return None,
        };
    }
    Some(data)
}

/// The length of run-length encoded data, its end-of-data byte included.
fn run_length_extent(d: &[u8]) -> usize {
    let mut i = 0usize;
    while let Some(&l) = d.get(i) {
        i += match l {
            128 => return i + 1,
            0..=127 => l as usize + 2,
            _ => 2,
        };
    }
    d.len()
}

/// Whether a filter of this name can be decoded.
pub(super) fn known_filter(name: &str) -> bool {
    matches!(name, "ASCII85Decode" | "ASCIIHexDecode" | "RunLengthDecode" | "LZWDecode")
}

pub(super) fn cs_from(v: &V) -> ImageCs {
    let items = v.items().unwrap_or_else(|| vec![v.clone()]);
    match items.first().map(V::key).as_deref() {
        Some("DeviceGray" | "CalGray") => ImageCs::Gray,
        Some("DeviceRGB" | "CalRGB") => ImageCs::Rgb,
        Some("DeviceCMYK") => ImageCs::Cmyk,
        Some("Indexed") => {
            let base = items.get(1).map(cs_from).unwrap_or(ImageCs::Other);
            let hival = items.get(2).and_then(V::num).unwrap_or(0.0).clamp(0.0, 4095.0) as usize;
            match (items.get(3), base.components()) {
                (Some(V::Str(l)), 1 | 3 | 4) => ImageCs::Indexed { base: Rc::new(base), hival, lookup: Rc::new(l.borrow().clone()) },
                _ => ImageCs::Other,
            }
        }
        _ => ImageCs::Other,
    }
}

impl ImageCs {
    pub(super) fn components(&self) -> usize {
        match self {
            ImageCs::Gray | ImageCs::Indexed { .. } => 1,
            ImageCs::Rgb => 3,
            ImageCs::Cmyk => 4,
            ImageCs::Other => 0,
        }
    }

    fn to_rgb(&self, c: &[f64]) -> [f64; 3] {
        let g = |i: usize| c.get(i).copied().unwrap_or(0.0).clamp(0.0, 1.0);
        match self {
            ImageCs::Gray => [g(0); 3],
            ImageCs::Rgb => [g(0), g(1), g(2)],
            ImageCs::Cmyk => cmyk_to_rgb(g(0), g(1), g(2), g(3)),
            ImageCs::Indexed { base, hival, lookup } => {
                let i = (c.first().copied().unwrap_or(0.0).round().max(0.0) as usize).min(*hival);
                let n = base.components();
                let v: Vec<f64> = (0..n).map(|k| lookup.get(i * n + k).copied().unwrap_or(0) as f64 / 255.0).collect();
                base.to_rgb(&v)
            }
            ImageCs::Other => [0.0; 3],
        }
    }
}
