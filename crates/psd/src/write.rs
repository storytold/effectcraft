//! A minimal Photoshop document writer, per the same specification as the reader: RGB, CMYK or
//! Grayscale, 8 or 16 bits, raw or RLE channels, pixel layers with layer masks, groups and any
//! additional-information blocks (helpers below build text, effects, fill and adjustment
//! blocks). Used to generate test fixtures; not a full exporter. The file itself is laid out and
//! encoded by craft-psd ([`craft_psd::PsdFile::to_bytes`]).

use craft_psd::layer::{LayerInfo, LayerRecord};
use craft_psd::tagged::{SectionType, TaggedBlock};
use craft_psd::{
    BlendMode, BlendingRanges, ChannelData, Compression, GlobalLayerMask, Header, ImageData, LayerFlags, LayerInfoPlacement, MaskData, PsdFile, Version,
};

use crate::descriptor::{self, put_u16, put_u32, put_utf16, DValue, Descriptor};
use crate::limits::finite_sample;
use crate::{engine_data, ColorMode, Rect, Section};

/// A layer mask for [`WLayer`].
#[derive(Clone, Debug)]
pub struct WMask {
    pub rect: Rect,
    /// `rect.width() * rect.height()` values in 0..1.
    pub data: Vec<f32>,
    pub default_color: u8,
    pub disabled: bool,
}

/// A layer to write.
#[derive(Clone, Debug)]
pub struct WLayer {
    pub name: String,
    pub rect: Rect,
    /// Straight RGBA, `rect.width() * rect.height()` pixels.
    pub pixels: Vec<[f32; 4]>,
    pub blend: [u8; 4],
    pub opacity: u8,
    pub hidden: bool,
    pub clipping: bool,
    pub mask: Option<WMask>,
    pub section: Section,
    /// Extra additional-information blocks (key, data).
    pub blocks: Vec<([u8; 4], Vec<u8>)>,
}

impl WLayer {
    /// A pixel layer.
    pub fn pixels(name: &str, rect: Rect, pixels: Vec<[f32; 4]>) -> WLayer {
        WLayer {
            name: name.into(),
            rect,
            pixels,
            blend: *b"norm",
            opacity: 255,
            hidden: false,
            clipping: false,
            mask: None,
            section: Section::Layer,
            blocks: vec![],
        }
    }
    /// A layer filled with one colour.
    pub fn solid(name: &str, rect: Rect, rgba: [f32; 4]) -> WLayer {
        let n = (rect.width() * rect.height()) as usize;
        WLayer::pixels(name, rect, vec![rgba; n])
    }
    /// The top record of a group (its children come *before* it in file order, after a
    /// [`WLayer::group_end`]).
    pub fn group(name: &str, open: bool, blend: [u8; 4]) -> WLayer {
        let mut l = WLayer::pixels(name, Rect::default(), vec![]);
        l.section = if open { Section::OpenFolder } else { Section::ClosedFolder };
        l.blend = blend;
        l
    }
    /// The hidden record that closes a group (written below its children).
    pub fn group_end() -> WLayer {
        let mut l = WLayer::pixels("</Layer group>", Rect::default(), vec![]);
        l.section = Section::Divider;
        l
    }
    /// A layer without pixels carrying `blocks` (adjustment and fill layers, type layers).
    pub fn empty(name: &str, blocks: Vec<([u8; 4], Vec<u8>)>) -> WLayer {
        let mut l = WLayer::pixels(name, Rect::default(), vec![]);
        l.blocks = blocks;
        l
    }
    pub fn with_blend(mut self, key: &[u8; 4]) -> WLayer {
        self.blend = *key;
        self
    }
    pub fn with_opacity(mut self, o: u8) -> WLayer {
        self.opacity = o;
        self
    }
    pub fn with_block(mut self, b: ([u8; 4], Vec<u8>)) -> WLayer {
        self.blocks.push(b);
        self
    }
}

/// A document to write.
#[derive(Clone, Debug)]
pub struct WDoc {
    pub width: u32,
    pub height: u32,
    /// 8, 16 or 32 (32-bit samples are written as floats).
    pub depth: u16,
    /// Rgb, Cmyk or Grayscale.
    pub mode: ColorMode,
    pub rle: bool,
    /// Bottom of the stack first (file order).
    pub layers: Vec<WLayer>,
    /// Merged image (straight RGBA, document-sized); `None` composites visible pixel layers
    /// with Normal blending.
    pub composite: Option<Vec<[f32; 4]>>,
    /// Embedded files for smart objects: (unique id, file name, type code, bytes), written as
    /// linked layer data (`lnk2`).
    pub linked: Vec<(String, String, [u8; 4], Vec<u8>)>,
}

impl WDoc {
    pub fn new(width: u32, height: u32) -> WDoc {
        WDoc { width, height, depth: 8, mode: ColorMode::Rgb, rle: true, layers: vec![], composite: None, linked: vec![] }
    }
}

/// Colour channels for a straight RGB pixel in the document mode.
fn mode_channels(mode: ColorMode, p: [f32; 4]) -> Vec<f32> {
    match mode {
        ColorMode::Cmyk => {
            // Stored values are inverted ink amounts: k = max, c = r / max (reader: r = c * k).
            let m = p[0].max(p[1]).max(p[2]);
            if m <= 0.0 {
                vec![0.0, 0.0, 0.0, 0.0]
            } else {
                vec![p[0] / m, p[1] / m, p[2] / m, m]
            }
        }
        ColorMode::Grayscale => vec![p[0]],
        _ => vec![p[0], p[1], p[2]],
    }
}

fn encode_samples(v: &[f32], depth: u16) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * usize::from(depth.div_ceil(8)));
    for &x in v {
        // Integer depths are stored in 0..1. 32-bit documents keep finite samples
        // outside that range (HDR); NaN and infinities use the reader's mapping.
        let x = if depth == 32 { finite_sample(x) } else { x.clamp(0.0, 1.0) };
        if depth == 32 {
            out.extend_from_slice(&x.to_be_bytes());
        } else if depth == 16 {
            put_u16(&mut out, (x * 65535.0).round() as u16);
        } else {
            out.push((x * 255.0).round() as u8);
        }
    }
    out
}

/// The layer and merged-image compression of `doc`.
fn compression(doc: &WDoc) -> Compression {
    if doc.rle {
        Compression::Rle
    } else {
        Compression::Raw
    }
}

/// One channel of a `w`×`h` plane, encoded by craft-psd. An empty plane is written raw.
fn channel(id: i16, v: &[f32], w: usize, h: usize, doc: &WDoc) -> craft_psd::Result<ChannelData> {
    let c = if w == 0 || h == 0 { Compression::Raw } else { compression(doc) };
    let samples = encode_samples(v, doc.depth);
    // A PSD RLE row holds at most 65535 bytes; a wider plane is written raw.
    ChannelData::encode(id, c, &samples, w, h, doc.depth, Version::Psd)
        .or_else(|_| ChannelData::encode(id, Compression::Raw, &samples, w, h, doc.depth, Version::Psd))
}

/// The `luni` block: the UTF-16 length and code units, unpadded, as this writer has always
/// written it (`TaggedBlock::unicode_name` adds a terminator, which would change exported files).
fn unicode_name(name: &str) -> TaggedBlock {
    let units: Vec<u16> = name.encode_utf16().collect();
    let mut data = (units.len() as u32).to_be_bytes().to_vec();
    data.extend(units.iter().flat_map(|u| u.to_be_bytes()));
    TaggedBlock::new(*b"luni", data)
}

/// A legacy Pascal name: ASCII, anything else as `?`, at most 255 bytes.
fn legacy_name(s: &str) -> Vec<u8> {
    s.bytes().map(|c| if c < 0x80 { c } else { b'?' }).take(255).collect()
}

fn psd_rect(r: Rect) -> craft_psd::Rect {
    craft_psd::Rect { top: r.top, left: r.left, bottom: r.bottom, right: r.right }
}

/// The layer record of `l`: pixels, mask, name, section and blocks.
fn record(l: &WLayer, doc: &WDoc) -> craft_psd::Result<LayerRecord> {
    let (w, h) = (l.rect.width() as usize, l.rect.height() as usize);
    let n = w * h;
    let alpha: Vec<f32> = l.pixels.iter().take(n).map(|p| p[3]).collect();
    let mut channels = vec![channel(-1, &alpha, w, h, doc)?];
    let planes: Vec<Vec<f32>> = l.pixels.iter().take(n).map(|p| mode_channels(doc.mode, *p)).collect();
    for c in 0..doc.mode.color_channels() {
        let plane: Vec<f32> = planes.iter().map(|v| v[c]).collect();
        channels.push(channel(c as i16, &plane, w, h, doc)?);
    }
    let mut mask = MaskData::None;
    if let Some(m) = &l.mask {
        channels.push(channel(-2, &m.data, m.rect.width() as usize, m.rect.height() as usize, doc)?);
        mask = MaskData::Mask(craft_psd::LayerMask::new(psd_rect(m.rect), m.default_color, if m.disabled { 2 } else { 0 }));
    }
    let mut flags = LayerFlags(8);
    flags.set_hidden(l.hidden);
    let mut blocks = vec![unicode_name(&l.name)];
    let kind = match l.section {
        Section::Layer => None,
        Section::OpenFolder => Some(SectionType::OpenFolder),
        Section::ClosedFolder => Some(SectionType::ClosedFolder),
        Section::Divider => Some(SectionType::BoundingDivider),
    };
    if let Some(kind) = kind {
        blocks.push(TaggedBlock::section_divider(kind, Some(BlendMode::from_key(l.blend)), None));
    }
    blocks.extend(l.blocks.iter().map(|(k, d)| TaggedBlock::new(*k, d.clone())));
    Ok(LayerRecord {
        rect: psd_rect(l.rect),
        channels,
        blend_mode: BlendMode::from_key(l.blend),
        opacity: l.opacity,
        clipping: u8::from(l.clipping),
        flags,
        filler: 0,
        mask,
        blending_ranges: BlendingRanges::default(),
        name: legacy_name(&l.name),
        blocks,
        extra_trailing: vec![],
    })
}

/// Normal "over" of the visible pixel layers (groups ignored), straight alpha.
fn flatten(doc: &WDoc) -> Vec<[f32; 4]> {
    let (w, h) = (doc.width as i32, doc.height as i32);
    let mut acc = vec![[0.0f32; 4]; (w * h) as usize];
    for l in doc.layers.iter().filter(|l| !l.hidden && l.section == Section::Layer) {
        let lw = l.rect.width() as i32;
        for (i, p) in l.pixels.iter().enumerate() {
            let (x, y) = (l.rect.left + i as i32 % lw.max(1), l.rect.top + i as i32 / lw.max(1));
            if x < 0 || y < 0 || x >= w || y >= h {
                continue;
            }
            let a = p[3] * l.opacity as f32 / 255.0;
            let d = &mut acc[(y * w + x) as usize];
            let oa = a + d[3] * (1.0 - a);
            for c in 0..3 {
                d[c] = if oa > 0.0 { (p[c] * a + d[c] * d[3] * (1.0 - a)) / oa } else { 0.0 };
            }
            d[3] = oa;
        }
    }
    acc
}

/// Serialise a document with craft-psd.
pub fn write(doc: &WDoc) -> Vec<u8> {
    // Every input `WDoc` describes is in range for the writer; a failure is a bug in the caller.
    #[allow(clippy::expect_used)]
    build(doc).and_then(|f| f.to_bytes()).expect("a WDoc is always writable")
}

fn build(doc: &WDoc) -> craft_psd::Result<PsdFile> {
    let cc = doc.mode.color_channels();
    let header = Header::new(Version::Psd, doc.width, doc.height, (cc + 1) as u16, doc.depth, craft_psd::ColorMode::from_u16(doc.mode.to_u16()));
    let layers = doc.layers.iter().map(|l| record(l, doc)).collect::<craft_psd::Result<Vec<_>>>()?;
    let has_layers = !layers.is_empty();
    let mut global_blocks = vec![];
    if has_layers && !doc.linked.is_empty() {
        global_blocks.push(TaggedBlock::new(*b"lnk2", linked_block(&doc.linked)));
    }
    // Merged image.
    let comp = doc.composite.clone().unwrap_or_else(|| flatten(doc));
    let planes: Vec<Vec<f32>> = comp.iter().map(|p| mode_channels(doc.mode, *p)).collect();
    let mut merged = vec![];
    for c in 0..cc {
        merged.extend(encode_samples(&planes.iter().map(|v| v[c]).collect::<Vec<_>>(), doc.depth));
    }
    merged.extend(encode_samples(&comp.iter().map(|p| p[3]).collect::<Vec<_>>(), doc.depth));
    let image_data = ImageData::encode(compression(doc), &merged, &header).or_else(|_| ImageData::encode(Compression::Raw, &merged, &header))?;
    Ok(PsdFile {
        header,
        color_mode_data: vec![],
        resources: vec![],
        layer_info: has_layers.then_some(LayerInfo { merged_alpha: true, layers, padding: None }),
        // 16- and 32-bit layers go in an `Lr16` / `Lr32` block, as Photoshop writes them.
        layer_info_placement: match doc.depth {
            16 | 32 if has_layers => {
                LayerInfoPlacement::GlobalBlock { index: 0, signature: *b"8BIM", key: craft_psd::tiff::layer_info_key(doc.depth), padding: None }
            }
            _ => LayerInfoPlacement::Section,
        },
        global_layer_mask: has_layers.then(GlobalLayerMask::default),
        global_blocks,
        layer_mask_trailing: vec![],
        image_data,
        write_as_psb: false,
    })
}

/// A Unicode string without the terminating NUL (the layer-name `luni` form).
fn unicode_plain(s: &str) -> Vec<u8> {
    let mut out = vec![];
    put_utf16(&mut out, s, false);
    out
}

/// Linked layer data records (`liFD`, version 7) for embedded files.
fn linked_block(files: &[(String, String, [u8; 4], Vec<u8>)]) -> Vec<u8> {
    let mut out = vec![];
    for (uuid, name, ty, data) in files {
        let mut rec = vec![];
        rec.extend_from_slice(b"liFD");
        put_u32(&mut rec, 7);
        rec.push(uuid.len() as u8);
        rec.extend_from_slice(uuid.as_bytes());
        rec.extend_from_slice(&unicode_plain(name));
        rec.extend_from_slice(ty);
        rec.extend_from_slice(b"8BIM");
        rec.extend_from_slice(&(data.len() as u64).to_be_bytes());
        rec.push(0); // no file-open descriptor
        rec.extend_from_slice(data);
        put_u32(&mut rec, 0); // child document id (empty unicode string)
        rec.extend_from_slice(&0f64.to_be_bytes()); // asset modification time
        rec.push(0); // asset locked state
        out.extend_from_slice(&(rec.len() as u64).to_be_bytes());
        out.extend_from_slice(&rec);
        while !out.len().is_multiple_of(4) {
            out.push(0);
        }
    }
    out
}

// ---------------------------------------------------------------- block helpers

/// `SoLd`: a placed layer showing linked file `uuid` (content `size` pixels) with its corners at
/// `quad` (top-left, top-right, bottom-right, bottom-left).
pub fn smart_object_block(uuid: &str, quad: [[f64; 2]; 4], size: [f64; 2]) -> ([u8; 4], Vec<u8>) {
    smart_object_block_warped(uuid, quad, size, None)
}

/// A `warp` descriptor: a named style (bend and distortions in %) over `bounds` (left, top,
/// right, bottom), or with `mesh` (16 points, content pixels) a custom envelope.
pub fn warp_descriptor(style: &str, bend: f64, h: f64, v: f64, bounds: [f64; 4], mesh: Option<&[[f64; 2]]>) -> Descriptor {
    let mut d = Descriptor::new("warp")
        .with("warpStyle", DValue::Enum("warpStyle".into(), style.into()))
        .with("warpValue", DValue::Double(bend))
        .with("warpPerspective", DValue::Double(h))
        .with("warpPerspectiveOther", DValue::Double(v))
        .with("warpRotate", DValue::Enum("Ornt".into(), "Hrzn".into()))
        .with(
            "bounds",
            DValue::Descriptor(
                Descriptor::new("Rctn")
                    .with("Top ", DValue::UnitFloat("#Pxl".into(), bounds[1]))
                    .with("Left", DValue::UnitFloat("#Pxl".into(), bounds[0]))
                    .with("Btom", DValue::UnitFloat("#Pxl".into(), bounds[3]))
                    .with("Rght", DValue::UnitFloat("#Pxl".into(), bounds[2])),
            ),
        )
        .with("uOrder", DValue::Integer(4))
        .with("vOrder", DValue::Integer(4));
    if let Some(m) = mesh {
        let pts = Descriptor::new("rationalPoint")
            .with("Hrzn", DValue::UnitFloats("#Pxl".into(), m.iter().map(|p| p[0]).collect()))
            .with("Vrtc", DValue::UnitFloats("#Pxl".into(), m.iter().map(|p| p[1]).collect()));
        d = d
            .with("customEnvelopeWarp", DValue::Descriptor(Descriptor::new("customEnvelopeWarp").with("meshPoints", DValue::ObjectArray(m.len() as u32, pts))));
    }
    d
}

/// `SoLd` with an optional warp; `quad` may be non-affine (perspective).
pub fn smart_object_block_warped(uuid: &str, quad: [[f64; 2]; 4], size: [f64; 2], warp: Option<Descriptor>) -> ([u8; 4], Vec<u8>) {
    let list = |v: &[f64]| DValue::List(v.iter().map(|x| DValue::Double(*x)).collect());
    let t: Vec<f64> = quad.iter().flat_map(|p| [p[0], p[1]]).collect();
    let mut d = Descriptor::new("null")
        .with("Idnt", DValue::Text(uuid.to_string()))
        .with("placed", DValue::Text(uuid.to_string()))
        .with("Trnf", list(&t))
        .with("nonAffineTransform", list(&t))
        .with("Sz  ", DValue::Descriptor(Descriptor::new("Pnt ").with("Wdth", DValue::Double(size[0])).with("Hght", DValue::Double(size[1]))));
    if let Some(w) = warp {
        d = d.with("warp", DValue::Descriptor(w));
    }
    let mut out = b"soLD".to_vec();
    put_u32(&mut out, 4);
    put_u32(&mut out, 16);
    descriptor::write_descriptor(&mut out, &d);
    (*b"SoLd", out)
}

/// `TySh`: a type layer with one style run; the text origin sits at `origin`.
pub fn text_block(text: &str, origin: [f64; 2], font: &str, size: f64, color: [f64; 3], justification: u8) -> ([u8; 4], Vec<u8>) {
    let mut d = vec![];
    put_u16(&mut d, 1);
    for v in [1.0, 0.0, 0.0, 1.0, origin[0], origin[1]] {
        d.extend_from_slice(&f64::to_be_bytes(v));
    }
    put_u16(&mut d, 50);
    put_u32(&mut d, 16);
    let desc = Descriptor::new("TxLr")
        .with("Txt ", DValue::Text(text.replace('\n', "\r")))
        .with("textGridding", DValue::Enum("textGridding".into(), "None".into()))
        .with("Ornt", DValue::Enum("Ornt".into(), "Hrzn".into()))
        .with("EngineData", DValue::Raw(engine_data::minimal(text, font, size, color, justification)));
    descriptor::write_descriptor(&mut d, &desc);
    // Warp: version, descriptor version, an empty warp descriptor.
    put_u16(&mut d, 1);
    put_u32(&mut d, 16);
    descriptor::write_descriptor(&mut d, &Descriptor::new("warp").with("warpStyle", DValue::Enum("warpStyle".into(), "warpNone".into())));
    for _ in 0..4 {
        put_u32(&mut d, 0);
    }
    (*b"TySh", d)
}

/// `lfx2`: layer effects from an effects descriptor (`null` class with `DrSh`, `FrFX`…).
pub fn effects_block(desc: &Descriptor) -> ([u8; 4], Vec<u8>) {
    let mut d = vec![];
    put_u32(&mut d, 0);
    put_u32(&mut d, 16);
    descriptor::write_descriptor(&mut d, desc);
    (*b"lfx2", d)
}

/// `SoCo`: solid colour fill.
pub fn solid_color_block(rgb: [f64; 3]) -> ([u8; 4], Vec<u8>) {
    let mut d = vec![];
    put_u32(&mut d, 16);
    descriptor::write_descriptor(&mut d, &Descriptor::new("null").with("Clr ", DValue::Descriptor(Descriptor::rgb(rgb))));
    (*b"SoCo", d)
}

/// `hue2`: Hue/Saturation (master).
pub fn hue_sat_block(hue: i16, sat: i16, light: i16) -> ([u8; 4], Vec<u8>) {
    let mut d = vec![];
    put_u16(&mut d, 2);
    d.extend_from_slice(&[0, 0]);
    for v in [0i16, 0, 0, hue, sat, light] {
        d.extend_from_slice(&v.to_be_bytes());
    }
    // Six colour ranges (unused).
    d.resize(d.len() + 6 * 24, 0);
    (*b"hue2", d)
}

/// `levl`: Levels (composite record).
pub fn levels_block(in_black: i16, in_white: i16, out_black: i16, out_white: i16, gamma: f64) -> ([u8; 4], Vec<u8>) {
    let mut d = vec![];
    put_u16(&mut d, 2);
    for v in [in_black, in_white, out_black, out_white, (gamma * 100.0).round() as i16] {
        d.extend_from_slice(&v.to_be_bytes());
    }
    for _ in 0..28 {
        for v in [0i16, 255, 0, 255, 100] {
            d.extend_from_slice(&v.to_be_bytes());
        }
    }
    (*b"levl", d)
}

pub fn invert_block() -> ([u8; 4], Vec<u8>) {
    (*b"nvrt", vec![])
}

pub fn brightness_block(brightness: i16, contrast: i16) -> ([u8; 4], Vec<u8>) {
    let mut d = vec![];
    for v in [brightness, contrast, 127] {
        d.extend_from_slice(&v.to_be_bytes());
    }
    d.push(0);
    (*b"brit", d)
}

pub fn exposure_block(exposure: f32, offset: f32, gamma: f32) -> ([u8; 4], Vec<u8>) {
    let mut d = vec![];
    put_u16(&mut d, 1);
    for v in [exposure, offset, gamma] {
        d.extend_from_slice(&v.to_be_bytes());
    }
    (*b"expA", d)
}

/// `vmsk`: a vector mask from closed polygons/curves given as (in, anchor, out) knots in pixels.
pub fn vector_mask_block(subpaths: &[Vec<[[f64; 2]; 3]>], width: u32, height: u32, inverted: bool) -> ([u8; 4], Vec<u8>) {
    let mut d = vec![];
    put_u32(&mut d, 3);
    put_u32(&mut d, inverted as u32);
    // Path fill rule record.
    put_u16(&mut d, 6);
    d.resize(d.len() + 24, 0);
    let fx = |v: f64| ((v * (1 << 24) as f64).round() as i32).to_be_bytes();
    for sp in subpaths {
        put_u16(&mut d, 0);
        put_u16(&mut d, sp.len() as u16);
        d.resize(d.len() + 22, 0);
        for k in sp {
            put_u16(&mut d, 1);
            for p in k {
                d.extend_from_slice(&fx(p[1] / height as f64));
                d.extend_from_slice(&fx(p[0] / width as f64));
            }
        }
    }
    (*b"vmsk", d)
}
