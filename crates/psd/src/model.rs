use super::*;

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
        Rect { top, left, bottom: top.saturating_add(height), right: left.saturating_add(width) }
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

/// How deep groups nest in [`Psd::tree`]; deeper ones are flattened (a hostile file can nest
/// thousands, and walking or dropping that tree would overflow the stack).
pub const MAX_GROUP_DEPTH: usize = 64;

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
