use super::settings::yes;
use super::{ColorSpace, Deserialize, FrameRate, Serialize, Tick};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FootageKind {
    #[default]
    Video,
    Still,
    Sequence,
    Audio,
    /// A 3D model (glTF 2.0 / OBJ), placed as a model layer.
    Model,
    /// A data file (JSON, CSV, TSV) for data-driven animation: read by expressions through
    /// `footage(name).sourceData` / `sourceText` / `dataValue`; it has no pixels.
    Data,
}

/// Interpret Footage ▸ Fields and Pulldown ▸ Separate Fields.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FieldOrder {
    /// Progressive frames.
    #[default]
    Off,
    /// Interlaced, upper (odd) field first: each field becomes a frame at twice the rate.
    UpperFirst,
    LowerFirst,
}

impl FieldOrder {
    pub fn label(self) -> &'static str {
        match self {
            FieldOrder::Off => "Off",
            FieldOrder::UpperFirst => "Upper Field First",
            FieldOrder::LowerFirst => "Lower Field First",
        }
    }
    pub fn parse(s: &str) -> Option<FieldOrder> {
        match s.to_ascii_lowercase().replace([' ', '_', '-'], "").as_str() {
            "off" | "none" | "progressive" => Some(FieldOrder::Off),
            "upper" | "upperfirst" | "upperfieldfirst" => Some(FieldOrder::UpperFirst),
            "lower" | "lowerfirst" | "lowerfieldfirst" => Some(FieldOrder::LowerFirst),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AlphaMode {
    #[default]
    Straight,
    Premultiplied,
    Ignore,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Footage {
    pub path: String,
    pub kind: FootageKind,
    pub width: u32,
    pub height: u32,
    pub pixel_aspect: f64,
    pub frame_rate: FrameRate,
    /// Native rate before "Conform to frame rate".
    #[serde(default)]
    pub native_rate: Option<FrameRate>,
    pub duration: Tick,
    pub has_video: bool,
    pub has_audio: bool,
    pub alpha: AlphaMode,
    #[serde(default)]
    pub premul_color: [f32; 3],
    #[serde(default = "one")]
    pub loop_count: u32,
    #[serde(default)]
    pub codec: String,
    #[serde(default)]
    pub missing: bool,
    /// Image sequence files (when kind = Sequence).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sequence: Vec<String>,
    /// Colour profile (from the file's metadata, or Interpret Footage). `None` = sRGB.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_profile: Option<ColorSpace>,
    /// One layer of a layered file (Photoshop) instead of its merged image.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layer: Option<SourceLayer>,
    /// Interpret Footage ▸ Fields and Pulldown ▸ Separate Fields.
    #[serde(default, skip_serializing_if = "is_default")]
    pub fields: FieldOrder,
    /// Interpret Footage ▸ Alpha ▸ Invert Alpha.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub invert_alpha: bool,
    /// Interpret Footage ▸ Color ▸ Interpret As Linear Light: the file's values are linear.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub linear_light: bool,
    /// Interpret Footage ▸ Color ▸ Preserve RGB: the file's values enter the composition
    /// unconverted. Float footage (OpenEXR, float TIFF) is not sRGB-encoded on decode, and the
    /// colour profile, Interpret As Linear Light and the working space don't apply.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub preserve_rgb: bool,
    /// Data footage ([`FootageKind::Data`]): the file's text (JSON, CSV or TSV), kept in the
    /// project so expressions read it everywhere (and on the web).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    /// PDF / Illustrator footage: the page shown (0-based; File ▸ Import ▸ Page).
    #[serde(default, skip_serializing_if = "is_default")]
    pub page: u32,
    /// Image sequences imported with Force Alphabetical Order: the files play one after another
    /// and their numbering doesn't count (otherwise a gap in it shows a placeholder, as in After
    /// Effects; [`sequence`]).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub alphabetical: bool,
    /// Interpret Footage ▸ Start Timecode ▸ Override Start: the frame (at the footage's frame
    /// rate) its timecode shows at its first frame. `None` = Use Source File Timecode (0: file
    /// timecode isn't read). It only relabels source time, as in After Effects.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_timecode: Option<i64>,
}

fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

impl Default for Footage {
    fn default() -> Self {
        Footage {
            path: String::new(),
            kind: FootageKind::Still,
            width: 0,
            height: 0,
            pixel_aspect: 1.0,
            frame_rate: FrameRate::FPS_30,
            native_rate: None,
            duration: Tick::ZERO,
            has_video: false,
            has_audio: false,
            alpha: AlphaMode::Straight,
            premul_color: [0.0; 3],
            loop_count: 1,
            codec: String::new(),
            missing: false,
            sequence: vec![],
            color_profile: None,
            fields: FieldOrder::Off,
            invert_alpha: false,
            linear_light: false,
            preserve_rgb: false,
            data: None,
            layer: None,
            page: 0,
            alphabetical: false,
            start_timecode: None,
        }
    }
}

impl Footage {
    /// Interpret As Linear Light applies (Preserve RGB passes the file's values through).
    pub fn linear_light_applies(&self) -> bool {
        self.linear_light && !self.preserve_rgb
    }

    /// How long the footage plays: its duration times Interpret Footage ▸ Loop.
    pub fn looped_duration(&self) -> Tick {
        Tick(self.duration.0.saturating_mul(self.loop_count.max(1) as i64))
    }

    /// Source time as the footage's timecode: its frame `frame` from Start Timecode
    /// (`0:00:41:20`), as the Footage panel shows it.
    pub fn timecode(&self, frame: i64) -> String {
        effectcraft_time::format_timecode_ae(frame.saturating_add(self.start_timecode.unwrap_or(0)), self.frame_rate, false)
    }

    /// The data format of data footage: `json`, `csv` or `tsv` (from the file extension).
    pub fn data_format(&self) -> Option<&'static str> {
        if self.kind != FootageKind::Data {
            return None;
        }
        let ext = std::path::Path::new(&self.path).extension().and_then(std::ffi::OsStr::to_str).unwrap_or("").to_ascii_lowercase();
        Some(match ext.as_str() {
            "csv" => "csv",
            "tsv" | "tab" => "tsv",
            _ => "json",
        })
    }
}

/// A proxy: a stand-in file used instead of a footage item or composition (File ▸ Set Proxy /
/// Create Proxy), when its Use Proxy switch is on and the render's Proxy Use allows it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Proxy {
    /// The proxy's footage (path, size, rate…; File ▸ Interpret Footage ▸ Proxy edits it).
    pub footage: Footage,
    /// Use Proxy (the Project panel's proxy indicator: filled = in use, hollow = set but off).
    #[serde(default = "yes")]
    pub enabled: bool,
}

/// A layer of a layered still (a Photoshop document, or a PDF / Illustrator / EPS file) used as
/// footage (File ▸ Import ▸ Composition / Composition – Retain Layer Sizes, or Choose Layer).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceLayer {
    /// Layer record index in the file (bottom of the stack = 0).
    pub index: u32,
    pub name: String,
    /// The footage is the layer's own bounds (Retain Layer Sizes) rather than the document size.
    #[serde(default)]
    pub layer_size: bool,
    /// A Photoshop smart object: the unique id of its embedded file (linked layer data), which
    /// is the footage instead of the layer's pixels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedded: Option<String>,
    /// A smart object with a perspective quad or a warp: the footage is its embedded file baked
    /// as placed (warped and pinned to its corners) over the placed bounds in document space.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub placed: bool,
}

fn one() -> u32 {
    1
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Solid {
    pub color: [f32; 3],
    pub width: u32,
    pub height: u32,
    pub pixel_aspect: f64,
}
