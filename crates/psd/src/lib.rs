//! Photoshop document (PSD / PSB) reader, implemented from Adobe's public *Photoshop File Formats
//! Specification* (see README.md), and a minimal writer ([`write`]) for fixtures.
//!
//! [`Psd::parse`] reads the structure (header, layer records and their additional information,
//! the layer tree) without decoding pixels; [`Psd::composite`] decodes the merged image and
//! [`Psd::layer_pixels`] one layer (with its layer mask applied to alpha) on demand. Pixels come
//! out as straight (unpremultiplied) RGBA `f32` in 0..1, display-encoded (32-bit documents,
//! which are linear, are encoded to sRGB).

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod descriptor;
pub mod engine_data;
pub mod placed;
mod read;
pub mod write;

use std::sync::Arc;

pub use descriptor::{DValue, Descriptor};
use read::Reader;

#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    NotPsd,
    Truncated,
    Unsupported(String),
    Invalid(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NotPsd => write!(f, "not a Photoshop document"),
            Error::Truncated => write!(f, "truncated Photoshop document"),
            Error::Unsupported(s) => write!(f, "unsupported Photoshop feature: {s}"),
            Error::Invalid(s) => write!(f, "invalid Photoshop document: {s}"),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// The most pixels one decoded image (the composite, a layer or a mask) may have: 2^28, e.g.
/// 16 384 × 16 384, 4 GiB as float RGBA. Bounds read from a file are untrusted, so a few bytes
/// can't make the reader allocate more than that.
const MAX_DECODE_PIXELS: u64 = 1 << 28;

/// `w × h` as a pixel count, or an error when it exceeds [`MAX_DECODE_PIXELS`].
fn decode_pixels(w: usize, h: usize) -> Result<usize> {
    match (w as u64).checked_mul(h as u64) {
        Some(n) if n <= MAX_DECODE_PIXELS => Ok(n as usize),
        _ => Err(Error::Unsupported(format!("{w}×{h} pixels is too large to decode"))),
    }
}

/// Whether `bytes` start like a Photoshop document.
pub fn is_psd(bytes: &[u8]) -> bool {
    bytes.len() >= 6 && &bytes[..4] == b"8BPS" && matches!(u16::from_be_bytes([bytes[4], bytes[5]]), 1 | 2)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorMode {
    Bitmap,
    Grayscale,
    Indexed,
    Rgb,
    Cmyk,
    Multichannel,
    Duotone,
    Lab,
}

impl ColorMode {
    pub fn from_u16(v: u16) -> Option<ColorMode> {
        Some(match v {
            0 => ColorMode::Bitmap,
            1 => ColorMode::Grayscale,
            2 => ColorMode::Indexed,
            3 => ColorMode::Rgb,
            4 => ColorMode::Cmyk,
            7 => ColorMode::Multichannel,
            8 => ColorMode::Duotone,
            9 => ColorMode::Lab,
            _ => return None,
        })
    }
    pub fn to_u16(self) -> u16 {
        match self {
            ColorMode::Bitmap => 0,
            ColorMode::Grayscale => 1,
            ColorMode::Indexed => 2,
            ColorMode::Rgb => 3,
            ColorMode::Cmyk => 4,
            ColorMode::Multichannel => 7,
            ColorMode::Duotone => 8,
            ColorMode::Lab => 9,
        }
    }
    /// Number of colour channels (before alpha).
    pub fn color_channels(self) -> usize {
        match self {
            ColorMode::Rgb | ColorMode::Lab => 3,
            ColorMode::Cmyk => 4,
            _ => 1,
        }
    }
}

/// A rectangle in document pixels (`right`/`bottom` exclusive).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub top: i32,
    pub left: i32,
    pub bottom: i32,
    pub right: i32,
}

impl Rect {
    pub fn new(left: i32, top: i32, width: i32, height: i32) -> Rect {
        Rect { top, left, bottom: top + height, right: left + width }
    }
    pub fn width(&self) -> u32 {
        (i64::from(self.right) - i64::from(self.left)).clamp(0, i64::from(u32::MAX)) as u32
    }
    pub fn height(&self) -> u32 {
        (i64::from(self.bottom) - i64::from(self.top)).clamp(0, i64::from(u32::MAX)) as u32
    }
    pub fn is_empty(&self) -> bool {
        self.width() == 0 || self.height() == 0
    }
}

/// One channel of a layer: id (0.. colour, -1 transparency, -2 layer mask, -3 real user mask) and
/// where its data (compression + image data) sits in the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChannelInfo {
    pub id: i16,
    pub offset: usize,
    pub len: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LayerMask {
    pub rect: Rect,
    /// Value outside the mask rectangle (0 or 255).
    pub default_color: u8,
    pub disabled: bool,
    pub inverted: bool,
}

/// Layer kind from the section divider setting (`lsct`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Section {
    #[default]
    Layer,
    OpenFolder,
    ClosedFolder,
    /// The hidden "</Layer group>" record that closes a group (below its children).
    Divider,
}

/// A type layer (`TySh`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TextInfo {
    /// Text with `\n` line breaks.
    pub text: String,
    /// Affine transform (xx, xy, yx, yy, tx, ty): the text origin sits at (tx, ty).
    pub transform: [f64; 6],
    pub style: engine_data::TextStyle,
    /// Text bounds (left, top, right, bottom) relative to the origin, when stored.
    pub bounds: Option<[f64; 4]>,
}

/// Adjustment layers (best effort).
#[derive(Clone, Debug, PartialEq)]
pub enum Adjustment {
    Invert,
    BrightnessContrast {
        brightness: f64,
        contrast: f64,
        legacy: bool,
    },
    HueSaturation {
        hue: f64,
        saturation: f64,
        lightness: f64,
        colorize: bool,
    },
    Levels {
        in_black: f64,
        in_white: f64,
        gamma: f64,
        out_black: f64,
        out_white: f64,
    },
    Exposure {
        exposure: f64,
        offset: f64,
        gamma: f64,
    },
    /// Another adjustment (its tagged-block key, e.g. `curv`, `blnc`).
    Other(String),
}

/// A vector mask path: sub-paths of (in tangent, vertex, out tangent) points in document pixels.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VectorMask {
    pub inverted: bool,
    pub disabled: bool,
    pub subpaths: Vec<SubPath>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SubPath {
    pub closed: bool,
    /// Per knot: [control point preceding, anchor, control point leaving] as (x, y).
    pub knots: Vec<[[f64; 2]; 3]>,
}

/// A placed layer (smart object): `SoLd` / `SoLE` (placed layer data) or the older `PlLd`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SmartObject {
    /// Unique id of the linked file ([`Psd::linked`]) the layer shows.
    pub uuid: String,
    /// Where the content's corners sit in document pixels: top-left, top-right, bottom-right,
    /// bottom-left.
    pub quad: [[f64; 2]; 4],
    /// The content's size in pixels (`Sz  `), when recorded.
    pub size: Option<[f64; 2]>,
    /// The placed layer's warp (`warp`), when it has one ([`placed`]).
    pub warp: Option<placed::Warp>,
}

impl SmartObject {
    /// The quad as an affine placement of a `w`×`h` content rectangle: (centre, scale x, scale
    /// y, rotation in degrees). Perspective and skew are dropped.
    pub fn placement(&self, w: f64, h: f64) -> ([f64; 2], f64, f64, f64) {
        let q = self.quad;
        let centre = [(q[0][0] + q[1][0] + q[2][0] + q[3][0]) / 4.0, (q[0][1] + q[1][1] + q[2][1] + q[3][1]) / 4.0];
        let top = [q[1][0] - q[0][0], q[1][1] - q[0][1]];
        let left = [q[3][0] - q[0][0], q[3][1] - q[0][1]];
        let sx = top[0].hypot(top[1]) / w.max(1e-9);
        let sy = left[0].hypot(left[1]) / h.max(1e-9);
        // Mirrored content (left edge turning the other way) keeps a negative y scale.
        let cross = top[0] * left[1] - top[1] * left[0];
        (centre, sx, if cross < 0.0 { -sy } else { sy }, top[1].atan2(top[0]).to_degrees())
    }
}

/// A file embedded in (or linked from) the document: linked layer data (`lnk2`, `lnkD`,
/// `lnk3`, `lnkE`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LinkedFile {
    pub uuid: String,
    pub name: String,
    /// File type code (`8BPS`, `PNGf`, `JPEG`…), may be blank.
    pub file_type: String,
    /// Embedded data (absent for externally linked files).
    pub data: Option<std::ops::Range<usize>>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Layer {
    /// Index in [`Psd::layers`] (file order: bottom of the stack first).
    pub index: usize,
    pub name: String,
    pub id: Option<u32>,
    pub rect: Rect,
    /// Blend mode key (`norm`, `mul `, `pass`…).
    pub blend: [u8; 4],
    pub opacity: u8,
    /// Fill opacity (`iOpa`), 255 when absent.
    pub fill_opacity: u8,
    /// Clipped to the layer below.
    pub clipping: bool,
    pub hidden: bool,
    pub protect_transparency: bool,
    pub section: Section,
    pub channels: Vec<ChannelInfo>,
    pub mask: Option<LayerMask>,
    pub text: Option<TextInfo>,
    pub adjustment: Option<Adjustment>,
    /// Solid colour fill layer (`SoCo`), RGB 0..1.
    pub fill: Option<[f64; 3]>,
    /// Layer effects (`lfx2` descriptor).
    pub effects: Option<Descriptor>,
    pub vector_mask: Option<VectorMask>,
    /// Smart object placement.
    pub smart_object: Option<SmartObject>,
    /// Keys of every additional-information block the layer carries.
    pub keys: Vec<[u8; 4]>,
}

impl Layer {
    pub fn is_group(&self) -> bool {
        matches!(self.section, Section::OpenFolder | Section::ClosedFolder)
    }
    pub fn blend_key(&self) -> &str {
        std::str::from_utf8(&self.blend).unwrap_or("norm")
    }
    pub fn has_pixels(&self) -> bool {
        !self.rect.is_empty() && self.channels.iter().any(|c| c.id >= 0)
    }
}

/// The layer tree, top of the stack first.
#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Layer(usize),
    Group { layer: usize, children: Vec<Node> },
}

/// Decoded pixels: straight RGBA, row-major.
#[derive(Clone, Debug, PartialEq)]
pub struct Pixels {
    pub width: u32,
    pub height: u32,
    pub data: Vec<[f32; 4]>,
}

/// A parsed Photoshop document.
#[derive(Clone, Debug)]
pub struct Psd {
    /// PSB (large document format).
    pub psb: bool,
    pub width: u32,
    pub height: u32,
    pub channels: u16,
    pub depth: u16,
    pub mode: ColorMode,
    /// Bottom of the stack first (file order).
    pub layers: Vec<Layer>,
    /// The merged image's first extra channel is transparency.
    pub merged_alpha: bool,
    /// Indexed colour palette (256 R, then G, then B).
    pub palette: Vec<u8>,
    /// Files embedded for smart objects.
    pub linked: Vec<LinkedFile>,
    data: Arc<[u8]>,
    merged_offset: usize,
}

impl Psd {
    pub fn parse(data: impl Into<Arc<[u8]>>) -> Result<Psd> {
        let data: Arc<[u8]> = data.into();
        let mut r = Reader::new(&data);
        if r.bytes(4).ok() != Some(b"8BPS") {
            return Err(Error::NotPsd);
        }
        let version = r.u16()?;
        let psb = match version {
            1 => false,
            2 => true,
            _ => return Err(Error::NotPsd),
        };
        r.skip(6)?;
        let channels = r.u16()?;
        let height = r.u32()?;
        let width = r.u32()?;
        let depth = r.u16()?;
        let mode_v = r.u16()?;
        let mode = ColorMode::from_u16(mode_v).ok_or_else(|| Error::Unsupported(format!("colour mode {mode_v}")))?;
        if !matches!(depth, 1 | 8 | 16 | 32) {
            return Err(Error::Unsupported(format!("{depth}-bit")));
        }
        if width == 0 || height == 0 || width > 300_000 || height > 300_000 {
            return Err(Error::Invalid(format!("size {width}x{height}")));
        }
        // Colour mode data.
        let n = r.u32()? as usize;
        let palette = r.bytes(n)?.to_vec();
        // Image resources.
        let n = r.u32()? as usize;
        r.skip(n)?;
        // Layer and mask information.
        let mut psd = Psd {
            psb,
            width,
            height,
            channels,
            depth,
            mode,
            layers: vec![],
            merged_alpha: false,
            palette,
            linked: vec![],
            data: data.clone(),
            merged_offset: 0,
        };
        let lm_len = r.length(psb)?;
        let lm_end = r.pos.checked_add(lm_len).filter(|&e| e <= data.len()).ok_or(Error::Truncated)?;
        if lm_len > 0 {
            let li_len = r.length(psb)?;
            let li_end = r.pos.checked_add(li_len).filter(|&e| e <= lm_end).ok_or(Error::Truncated)?;
            if li_len > 0 {
                psd.layer_info(&data, r.pos, li_end)?;
            }
            r.pos = li_end;
            if r.pos + 4 <= lm_end {
                let g = r.u32()? as usize;
                r.pos = (r.pos + g).min(lm_end);
            }
            // Global additional layer information (16/32-bit layer info lives here).
            while let Some((key, start, end)) = next_block(&data, &mut r.pos, lm_end, psb) {
                if matches!(&key, b"lnk2" | b"lnkD" | b"lnk3" | b"lnkE") {
                    psd.linked_files(&data, start, end);
                }
                if matches!(&key, b"Lr16" | b"Lr32" | b"Layr") && psd.layers.is_empty() && end > start {
                    // The block holds the layer info, normally without its length field; accept
                    // both layouts.
                    if psd.layer_info(&data, start, end).is_err() || psd.layers.is_empty() {
                        psd.layers.clear();
                        let skip = if psb { 8 } else { 4 };
                        if start + skip < end && psd.layer_info(&data, start + skip, end).is_err() {
                            psd.layers.clear();
                        }
                    }
                }
                r.pos = end;
            }
        }
        psd.merged_offset = lm_end;
        Ok(psd)
    }

    /// The embedded bytes of linked file `uuid`.
    pub fn linked_data(&self, uuid: &str) -> Option<&[u8]> {
        let f = self.linked.iter().find(|f| f.uuid == uuid)?;
        self.data.get(f.data.clone()?)
    }

    /// Linked layer data records (spec: "Linked Layer"): length, kind (`liFD` embedded data,
    /// `liFE` external file, `liFA` alias), version, unique id, original file name, type,
    /// creator, data length, optional descriptors, then the data. A damaged record ends the list.
    fn linked_files(&mut self, data: &[u8], start: usize, end: usize) {
        let mut pos = start;
        while pos + 8 <= end {
            let mut r = Reader::at(&data[..end], pos);
            let Ok(len) = r.u64() else { break };
            let body = r.pos;
            let Some(rec_end) = body.checked_add(len as usize).filter(|e| *e <= end) else { break };
            let mut r = Reader::at(&data[..rec_end], body);
            let rec = (|| -> Result<LinkedFile> {
                let kind = r.tag()?;
                let version = r.u32()?;
                let n = r.u8()? as usize;
                let uuid = String::from_utf8_lossy(r.bytes(n)?).into_owned();
                let name = descriptor::read_unicode(&mut r)?;
                let file_type = String::from_utf8_lossy(&r.tag()?).trim().to_string();
                let _creator = r.tag()?;
                let dlen = r.u64()? as usize;
                if r.u8()? != 0 {
                    let _v = r.u32()?;
                    descriptor::read_descriptor(&mut r)?;
                }
                let mut out = LinkedFile { uuid, name, file_type, data: None };
                match &kind {
                    b"liFD" => {
                        let s = r.pos;
                        r.skip(dlen)?;
                        out.data = Some(s..s + dlen);
                    }
                    b"liFE" => {
                        let _v = r.u32()?;
                        descriptor::read_descriptor(&mut r)?;
                        if version > 3 {
                            r.skip(4 + 4 + 8)?;
                        }
                        let _size = r.u64()?;
                        if version > 2 && r.remaining() >= dlen && dlen > 0 {
                            let s = r.pos;
                            out.data = Some(s..s + dlen);
                        }
                    }
                    _ => {}
                }
                Ok(out)
            })();
            match rec {
                Ok(f) => self.linked.push(f),
                Err(_) => break,
            }
            pos = rec_end.div_ceil(4) * 4;
        }
    }

    fn layer_info(&mut self, data: &[u8], start: usize, end: usize) -> Result<()> {
        let mut r = Reader::at(&data[..end], start);
        let count = r.i16()?;
        self.merged_alpha = count < 0;
        let n = count.unsigned_abs() as usize;
        let mut layers = Vec::with_capacity(n);
        for i in 0..n {
            layers.push(self.layer_record(&mut r, i)?);
        }
        // Channel image data follows in record order.
        for l in &mut layers {
            for c in &mut l.channels {
                c.offset = r.pos;
                r.skip(c.len)?;
            }
        }
        self.layers = layers;
        Ok(())
    }

    fn layer_record(&self, r: &mut Reader, index: usize) -> Result<Layer> {
        let psb = self.psb;
        let (top, left, bottom, right) = (r.i32()?, r.i32()?, r.i32()?, r.i32()?);
        let nch = r.u16()? as usize;
        if nch > 64 {
            return Err(Error::Invalid("channel count".into()));
        }
        let mut channels = Vec::with_capacity(nch);
        for _ in 0..nch {
            let id = r.i16()?;
            let len = r.length(psb)?;
            channels.push(ChannelInfo { id, offset: 0, len });
        }
        if r.bytes(4)? != b"8BIM" {
            return Err(Error::Invalid("layer record signature".into()));
        }
        let blend = r.tag()?;
        let opacity = r.u8()?;
        let clipping = r.u8()? != 0;
        let flags = r.u8()?;
        r.skip(1)?;
        let extra = r.u32()? as usize;
        let extra_end = r.pos.checked_add(extra).filter(|&e| e <= r.data.len()).ok_or(Error::Truncated)?;
        let mut l = Layer {
            index,
            rect: Rect { top, left, bottom, right },
            blend,
            opacity,
            fill_opacity: 255,
            clipping,
            hidden: flags & 2 != 0,
            protect_transparency: flags & 1 != 0,
            channels,
            ..Default::default()
        };
        // Layer mask data.
        let mlen = r.u32()? as usize;
        let mstart = r.pos;
        if mlen >= 18 {
            let rect = Rect { top: r.i32()?, left: r.i32()?, bottom: r.i32()?, right: r.i32()? };
            let default_color = r.u8()?;
            let mflags = r.u8()?;
            l.mask = Some(LayerMask { rect, default_color, disabled: mflags & 2 != 0, inverted: mflags & 4 != 0 });
        }
        r.pos = mstart + mlen;
        // Blending ranges.
        let blen = r.u32()? as usize;
        r.skip(blen)?;
        l.name = r.pascal(4)?;
        let mut pos = r.pos;
        let data = r.data;
        while let Some((key, start, end)) = next_block(data, &mut pos, extra_end, psb) {
            l.keys.push(key);
            let block = &data[start..end];
            // A malformed block only loses that block's information.
            let _ = self.layer_block(&mut l, &key, block);
            pos = end;
        }
        r.pos = extra_end;
        Ok(l)
    }

    fn layer_block(&self, l: &mut Layer, key: &[u8; 4], b: &[u8]) -> Result<()> {
        let mut r = Reader::new(b);
        match key {
            b"luni" => {
                let name = descriptor::read_unicode(&mut r)?;
                if !name.is_empty() {
                    l.name = name;
                }
            }
            b"lyid" => l.id = Some(r.u32()?),
            b"iOpa" => l.fill_opacity = r.u8()?,
            b"lsct" | b"lsdk" => {
                l.section = match r.u32()? {
                    1 => Section::OpenFolder,
                    2 => Section::ClosedFolder,
                    3 => Section::Divider,
                    _ => Section::Layer,
                };
                if b.len() >= 12 && &b[4..8] == b"8BIM" {
                    l.blend.copy_from_slice(&b[8..12]);
                }
            }
            b"TySh" => {
                let _ver = r.u16()?;
                let mut t = [0.0; 6];
                for v in &mut t {
                    *v = r.f64()?;
                }
                let _text_ver = r.u16()?;
                let _desc_ver = r.u32()?;
                let d = descriptor::read_descriptor(&mut r)?;
                let text = d.text("Txt ").unwrap_or_default().replace("\r\n", "\n").replace('\r', "\n");
                let text = text.strip_suffix('\n').map(str::to_string).unwrap_or(text);
                let style = d.raw("EngineData").map(|e| engine_data::text_style(&engine_data::parse(e))).unwrap_or_default();
                let bounds =
                    d.obj("bounds").or_else(|| d.obj("boundingBox")).and_then(|o| Some([o.num("Left")?, o.num("Top ")?, o.num("Rght")?, o.num("Btom")?]));
                l.text = Some(TextInfo { text, transform: t, style, bounds });
            }
            b"lfx2" => {
                let _ver = r.u32()?;
                let _dver = r.u32()?;
                l.effects = Some(descriptor::read_descriptor(&mut r)?);
            }
            b"SoCo" => {
                let _ver = r.u32()?;
                let d = descriptor::read_descriptor(&mut r)?;
                l.fill = d.color("Clr ");
            }
            b"nvrt" => l.adjustment = Some(Adjustment::Invert),
            b"brit" => {
                let brightness = r.i16()? as f64;
                let contrast = r.i16()? as f64;
                if l.adjustment.is_none() {
                    l.adjustment = Some(Adjustment::BrightnessContrast { brightness, contrast, legacy: true });
                }
            }
            b"CgEd" => {
                let _ver = r.u32()?;
                let d = descriptor::read_descriptor(&mut r)?;
                l.adjustment = Some(Adjustment::BrightnessContrast {
                    brightness: d.num("Brgh").unwrap_or(0.0),
                    contrast: d.num("Cntr").unwrap_or(0.0),
                    legacy: d.bool("useLegacy").unwrap_or(false),
                });
            }
            b"hue2" | b"hue " => {
                let _ver = r.u16()?;
                let colorize = r.u8()? != 0;
                r.skip(1)?;
                let ch = (r.i16()? as f64, r.i16()? as f64, r.i16()? as f64);
                let m = (r.i16()? as f64, r.i16()? as f64, r.i16()? as f64);
                let (hue, saturation, lightness) = if colorize { ch } else { m };
                l.adjustment = Some(Adjustment::HueSaturation { hue, saturation, lightness, colorize });
            }
            b"levl" => {
                let _ver = r.u16()?;
                let (ib, iw, ob, ow, g) = (r.i16()?, r.i16()?, r.i16()?, r.i16()?, r.i16()?);
                l.adjustment =
                    Some(Adjustment::Levels { in_black: ib as f64, in_white: iw as f64, gamma: g as f64 / 100.0, out_black: ob as f64, out_white: ow as f64 });
            }
            b"expA" => {
                let _ver = r.u16()?;
                let exposure = r.f32()? as f64;
                let offset = r.f32()? as f64;
                let gamma = r.f32()? as f64;
                l.adjustment = Some(Adjustment::Exposure { exposure, offset, gamma });
            }
            b"curv" | b"blnc" | b"post" | b"thrs" | b"selc" | b"mixr" | b"phfl" | b"vibA" | b"blwh" | b"grdm" | b"clrL" if l.adjustment.is_none() => {
                l.adjustment = Some(Adjustment::Other(String::from_utf8_lossy(key).into_owned()));
            }
            b"SoLd" | b"SoLE" => {
                let _id = r.tag()?;
                let _ver = r.u32()?;
                let _dver = r.u32()?;
                let d = descriptor::read_descriptor(&mut r)?;
                let uuid = d.text("Idnt").unwrap_or_default().to_string();
                let nums = |k: &str| -> Option<Vec<f64>> {
                    match d.get(k)? {
                        DValue::List(l) => Some(
                            l.iter()
                                .filter_map(|v| match v {
                                    DValue::Double(x) | DValue::UnitFloat(_, x) => Some(*x),
                                    DValue::Integer(i) => Some(*i as f64),
                                    _ => None,
                                })
                                .collect(),
                        ),
                        DValue::UnitFloats(_, v) => Some(v.clone()),
                        _ => None,
                    }
                };
                let t = nums("Trnf").filter(|t| t.len() == 8).unwrap_or_else(|| vec![0.0; 8]);
                // The non-affine transform when present (perspective), else the transform.
                let t = nums("nonAffineTransform").filter(|t| t.len() == 8).unwrap_or(t);
                let size = d.obj("Sz  ").and_then(|s| Some([s.num("Wdth")?, s.num("Hght")?]));
                let warp = d.obj("warp").and_then(placed::Warp::read);
                l.smart_object = Some(SmartObject { uuid, quad: [[t[0], t[1]], [t[2], t[3]], [t[4], t[5]], [t[6], t[7]]], size, warp });
            }
            b"PlLd" if l.smart_object.is_none() => {
                let _id = r.tag()?;
                let _ver = r.u32()?;
                let n = r.u8()? as usize;
                let uuid = String::from_utf8_lossy(r.bytes(n)?).into_owned();
                let _page = r.u32()?;
                let _pages = r.u32()?;
                let _aa = r.u32()?;
                let _ty = r.u32()?;
                let mut t = [0.0; 8];
                for v in &mut t {
                    *v = r.f64()?;
                }
                l.smart_object = Some(SmartObject { uuid, quad: [[t[0], t[1]], [t[2], t[3]], [t[4], t[5]], [t[6], t[7]]], size: None, warp: None });
            }
            b"vmsk" | b"vsms" => {
                let _ver = r.u32()?;
                let flags = r.u32()?;
                l.vector_mask = Some(self.vector_paths(&mut r, flags)?);
            }
            _ => {}
        }
        Ok(())
    }

    fn vector_paths(&self, r: &mut Reader, flags: u32) -> Result<VectorMask> {
        let (w, h) = (self.width as f64, self.height as f64);
        let mut vm = VectorMask { inverted: flags & 1 != 0, disabled: flags & 4 != 0, subpaths: vec![] };
        let fixed = |r: &mut Reader| -> Result<f64> { Ok(r.i32()? as f64 / (1 << 24) as f64) };
        while r.remaining() >= 26 {
            let sel = r.u16()?;
            match sel {
                0 | 3 => {
                    let _n = r.u16()?;
                    r.skip(22)?;
                    vm.subpaths.push(SubPath { closed: sel == 0, knots: vec![] });
                }
                1 | 2 | 4 | 5 => {
                    let mut pts = [[0.0; 2]; 3];
                    for p in &mut pts {
                        let y = fixed(r)?;
                        let x = fixed(r)?;
                        *p = [x * w, y * h];
                    }
                    if let Some(sp) = vm.subpaths.last_mut() {
                        sp.knots.push(pts);
                    }
                }
                _ => r.skip(24)?,
            }
        }
        vm.subpaths.retain(|s| !s.knots.is_empty());
        Ok(vm)
    }

    /// The layer tree, top of the stack first.
    pub fn tree(&self) -> Vec<Node> {
        let mut stack: Vec<Vec<Node>> = vec![vec![]];
        for l in &self.layers {
            match l.section {
                Section::Divider => stack.push(vec![]),
                Section::OpenFolder | Section::ClosedFolder => {
                    let children = if stack.len() > 1 { stack.pop().unwrap_or_default() } else { vec![] };
                    if let Some(top) = stack.last_mut() {
                        top.push(Node::Group { layer: l.index, children });
                    }
                }
                Section::Layer => {
                    if let Some(top) = stack.last_mut() {
                        top.push(Node::Layer(l.index));
                    }
                }
            }
        }
        while stack.len() > 1 {
            let c = stack.pop().unwrap_or_default();
            if let Some(top) = stack.last_mut() {
                top.extend(c);
            }
        }
        fn rev(v: &mut Vec<Node>) {
            v.reverse();
            for n in v {
                if let Node::Group { children, .. } = n {
                    rev(children);
                }
            }
        }
        let mut out = stack.pop().unwrap_or_default();
        rev(&mut out);
        out
    }

    fn bytes_per_sample(&self) -> usize {
        match self.depth {
            16 => 2,
            32 => 4,
            _ => 1,
        }
    }

    /// Decode one channel of `w`×`h` samples at `offset` (compression field first). Values 0..1.
    fn decode_channel(&self, offset: usize, len: usize, w: usize, h: usize) -> Result<Vec<f32>> {
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
        Ok(self.samples(&raw, w, h))
    }

    fn row_bytes(&self, w: usize) -> usize {
        if self.depth == 1 { w.div_ceil(8) } else { w * self.bytes_per_sample() }
    }

    fn unpredict(&self, v: &mut [u8], w: usize, h: usize) {
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
                for row in v.chunks_exact_mut(w * 4).take(h) {
                    for i in 1..row.len() {
                        row[i] = row[i].wrapping_add(row[i - 1]);
                    }
                    // Bytes are stored plane by plane (all first bytes, then all second bytes…).
                    let planar = row.to_vec();
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

    fn samples(&self, raw: &[u8], w: usize, h: usize) -> Vec<f32> {
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
            32 => raw.as_chunks::<4>().0.iter().take(w * h).map(|c| f32::from_be_bytes([c[0], c[1], c[2], c[3]])).collect(),
            _ => raw.iter().take(w * h).map(|&b| b as f32 / 255.0).collect(),
        }
    }

    /// Convert colour channels (in the document's mode) to RGB.
    fn to_rgb(&self, ch: &[Option<Vec<f32>>], i: usize) -> [f32; 3] {
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
    pub fn composite(&self) -> Result<Pixels> {
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
                    *p = Some(self.samples(r.bytes(rowb * h)?, w, h));
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
                    *p = Some(self.samples(&out, w, h));
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
                    *p = Some(self.samples(&plane, w, h));
                }
            }
            c => return Err(Error::Unsupported(format!("compression {c}"))),
        }
        let alpha = if self.merged_alpha && nch > cc { planes[cc].take() } else { None };
        let mut data = Vec::with_capacity(w * h);
        for i in 0..w * h {
            let [r, g, b] = self.to_rgb(&planes, i);
            let a = alpha.as_ref().map_or(1.0, |a| a[i]);
            data.push([r, g, b, a]);
        }
        Ok(Pixels { width: w as u32, height: h as u32, data })
    }

    /// One layer's pixels with its layer mask (and vector-less fill colour) applied to alpha.
    /// `canvas`: document-sized with the layer in place; otherwise the layer's own bounds.
    pub fn layer_pixels(&self, index: usize, canvas: bool) -> Result<Pixels> {
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
                        -1 => alpha = Some(self.decode_channel(c.offset, c.len, sw, sh)?),
                        id if id >= 0 && (id as usize) < planes.len() => planes[id as usize] = Some(self.decode_channel(c.offset, c.len, sw, sh)?),
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
                        _ => (self.to_rgb(&planes, i), alpha.as_ref().map_or(1.0, |a| a[i])),
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
                Some(c) if mw > 0 && mh > 0 => Some(self.decode_channel(c.offset, c.len, mw, mh)?),
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

/// The next additional-information block at `*pos` (signature `8BIM`/`8B64`, key, length):
/// returns (key, data start, data end). Skips up to 3 padding bytes before a signature.
fn next_block(data: &[u8], pos: &mut usize, end: usize, psb: bool) -> Option<([u8; 4], usize, usize)> {
    let mut p = *pos;
    for _ in 0..4 {
        if p + 12 > end {
            return None;
        }
        if &data[p..p + 4] == b"8BIM" || &data[p..p + 4] == b"8B64" {
            break;
        }
        p += 1;
    }
    if p + 12 > end || !(&data[p..p + 4] == b"8BIM" || &data[p..p + 4] == b"8B64") {
        return None;
    }
    let mut key = [0u8; 4];
    key.copy_from_slice(&data[p + 4..p + 8]);
    let wide =
        psb && matches!(&key, b"LMsk" | b"Lr16" | b"Lr32" | b"Layr" | b"Mt16" | b"Mt32" | b"Mtrn" | b"Alph" | b"FMsk" | b"lnk2" | b"FEid" | b"FXid" | b"PxSD");
    let mut r = Reader::at(&data[..end], p + 8);
    let len = r.length(wide).ok()?;
    let start = r.pos;
    let stop = start.checked_add(len)?;
    if stop > end {
        return None;
    }
    *pos = stop;
    Some((key, start, stop))
}

/// PackBits decoding of one row (appends exactly `row` bytes, zero-padded).
fn unpack_bits(src: &[u8], row: usize, out: &mut Vec<u8>) -> Result<()> {
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

fn srgb_encode(v: f32) -> f32 {
    if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
}

/// CIE L*a*b* (D50) to sRGB (D65 via the Bradford-adapted matrix), clamped.
fn lab_to_rgb(l: f32, a: f32, b: f32) -> [f32; 3] {
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

#[cfg(test)]
mod tests;
