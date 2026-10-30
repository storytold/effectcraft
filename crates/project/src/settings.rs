use super::item::{ItemId, LayerId};
use super::{BTreeMap, ColorSpace, Deserialize, Label, Serialize, Tick};

/// The project file schema. 2: effects on layers without a source rectangle (shape, text)
/// measure positions from the top-left of their comp-sized bounds (effect space), not from
/// the layer's origin; projects from schema 1 are converted when opened (#227).
pub const SCHEMA_VERSION: u32 = 2;

/// The first schema in which effect positions are in effect space (see [`SCHEMA_VERSION`]).
pub const SCHEMA_EFFECT_SPACE: u32 = 2;

/// The EffectCraft version, written into project files as `savedBy`.
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The EffectCraft version that wrote a project file (its `savedBy`), when it says.
pub fn saved_by(text: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct Meta {
        #[serde(rename = "savedBy", default)]
        saved_by: Option<String>,
    }
    serde_json::from_str::<Meta>(text).ok()?.saved_by
}

/// The compositions that were open when a project file was saved and their current times
/// (`activeComp`, `openComps` and `compTimes` next to `savedBy`), so it reopens where it was left
/// (#748, #1714). Editor state rather than project
/// content: opening a comp doesn't make the project unsaved, and it isn't part of
/// [`Project`]. Older files have none; older versions ignore it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedView {
    /// The comp shown in the viewer and the Timeline.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_comp: Option<ItemId>,
    /// The open comps (Timeline tabs), in tab order.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub open_comps: Vec<ItemId>,
    /// Each comp's current time, so a project reopens on the frame it was saved at rather than
    /// at 0, where its layers may not have started yet (#1714).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub comp_times: BTreeMap<ItemId, Tick>,
}

/// The view a project file was saved with ([`SavedView`]); empty for older files. Values that
/// aren't ids are ignored (whether an id still names a comp is up to the caller).
pub fn saved_view(text: &str) -> SavedView {
    #[derive(Deserialize)]
    struct Meta {
        #[serde(rename = "activeComp", default)]
        active_comp: Option<serde_json::Value>,
        #[serde(rename = "openComps", default)]
        open_comps: Option<serde_json::Value>,
        #[serde(rename = "compTimes", default)]
        comp_times: Option<serde_json::Value>,
    }
    let Ok(m) = serde_json::from_str::<Meta>(text) else { return SavedView::default() };
    SavedView {
        active_comp: m.active_comp.as_ref().and_then(serde_json::Value::as_u64).map(ItemId),
        open_comps: m
            .open_comps
            .as_ref()
            .and_then(serde_json::Value::as_array)
            .map_or_else(Vec::new, |a| a.iter().filter_map(serde_json::Value::as_u64).map(ItemId).collect()),
        comp_times: m
            .comp_times
            .as_ref()
            .and_then(serde_json::Value::as_object)
            .map_or_else(BTreeMap::new, |o| o.iter().filter_map(|(k, v)| Some((ItemId(k.parse().ok()?), Tick(v.as_i64()?)))).collect()),
    }
}

/// Whether version `a` (`major.minor.patch`, pre-release suffixes ignored) is newer than `b`.
pub fn is_newer_version(a: &str, b: &str) -> bool {
    let parts = |v: &str| -> [u64; 3] {
        let mut n = v.split(['-', '+']).next().unwrap_or("").split('.').map(|x| x.parse().unwrap_or(0));
        [n.next().unwrap_or(0), n.next().unwrap_or(0), n.next().unwrap_or(0)]
    };
    parts(a) > parts(b)
}

#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    #[error("no item {0:?}")]
    NoItem(ItemId),
    #[error("item {0:?} is not a composition")]
    NotComp(ItemId),
    #[error("no layer {0:?}")]
    NoLayer(LayerId),
    #[error("no property `{0}`")]
    NoProperty(String),
    #[error("{0}")]
    Invalid(String),
}

/// Project bit depth.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum BitDepth {
    #[default]
    Bpc8,
    Bpc16,
    Bpc32,
}

impl BitDepth {
    pub fn label(self) -> &'static str {
        match self {
            BitDepth::Bpc8 => "8 bpc",
            BitDepth::Bpc16 => "16 bpc",
            BitDepth::Bpc32 => "32 bpc",
        }
    }
    pub fn next(self) -> BitDepth {
        match self {
            BitDepth::Bpc8 => BitDepth::Bpc16,
            BitDepth::Bpc16 => BitDepth::Bpc32,
            BitDepth::Bpc32 => BitDepth::Bpc8,
        }
    }
    pub fn is_float(self) -> bool {
        self == BitDepth::Bpc32
    }
    /// Quantisation levels per channel of the integer depths (8 bpc: 255; 16 bpc: 32768, After
    /// Effects' "15 + 1" bit range), `None` for 32 bpc float.
    pub fn levels(self) -> Option<f32> {
        match self {
            BitDepth::Bpc8 => Some(255.0),
            BitDepth::Bpc16 => Some(32768.0),
            BitDepth::Bpc32 => None,
        }
    }
    pub fn bits(self) -> u32 {
        match self {
            BitDepth::Bpc8 => 8,
            BitDepth::Bpc16 => 16,
            BitDepth::Bpc32 => 32,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TimeDisplayStyle {
    #[default]
    Timecode,
    Frames,
    /// Feet + Frames, 35 mm film (16 frames per foot).
    Feet35,
    /// Feet + Frames, 16 mm film (40 frames per foot).
    Feet16,
}

impl TimeDisplayStyle {
    /// Frames per foot of the Feet + Frames styles.
    pub fn frames_per_foot(self) -> Option<i64> {
        match self {
            TimeDisplayStyle::Feet35 => Some(16),
            TimeDisplayStyle::Feet16 => Some(40),
            _ => None,
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            TimeDisplayStyle::Timecode => "timecode",
            TimeDisplayStyle::Frames => "frames",
            TimeDisplayStyle::Feet35 => "feet35",
            TimeDisplayStyle::Feet16 => "feet16",
        }
    }
    pub fn parse(s: &str) -> Option<TimeDisplayStyle> {
        let k: String = s.chars().filter(char::is_ascii_alphanumeric).collect::<String>().to_ascii_lowercase();
        match k.as_str() {
            "timecode" => Some(TimeDisplayStyle::Timecode),
            "frames" => Some(TimeDisplayStyle::Frames),
            "feet35" | "feetframes" | "feetandframes" | "feet35mm" | "35mm" | "feetframes35mm" => Some(TimeDisplayStyle::Feet35),
            "feet16" | "feet16mm" | "16mm" | "feetframes16mm" => Some(TimeDisplayStyle::Feet16),
            _ => None,
        }
    }
}

/// Project Settings ▸ Color ▸ Color Engine.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ColorEngine {
    /// Built-in colour management (ICC-style working spaces).
    #[default]
    Adobe,
    /// OCIO managed, with the built-in configuration (ACES working spaces).
    Ocio,
}

/// Project Settings ▸ Color ▸ HDR: how over-range (HDR) values reach a standard-dynamic-range
/// display or output.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum HdrMode {
    /// Values above 1.0 clip.
    #[default]
    Clip,
    /// Highlights above 80% are companded (an exponential knee) into the remaining range.
    Compand,
    /// Extended Reinhard tone mapping of luminance (white at 4.0).
    ToneMap,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProjectSettings {
    pub bit_depth: BitDepth,
    /// Linearize Working Space: the working space uses linear light (1.0 gamma) for everything
    /// (sources, effects, blending). Needs a working space.
    pub linearize: bool,
    /// Working Space (colour management). `None` = no colour management.
    #[serde(default)]
    pub working_space: Option<ColorSpace>,
    /// Blend Colors Using 1.0 Gamma: layers blend in linear light (sources and effects stay in
    /// the working space's encoding).
    #[serde(default)]
    pub blend_linear: bool,
    pub time_display: TimeDisplayStyle,
    /// Frame numbering starts at 0 (or 1).
    pub frame_start: i64,
    pub audio_sample_rate: u32,
    /// Video Rendering and Effects ▸ Use: Mercury GPU Acceleration (`true`, the default; used
    /// when a GPU adapter exists) or Mercury Software Only (`false`, the CPU compositor).
    #[serde(default = "yes")]
    pub gpu_acceleration: bool,
    /// The project's comment (Metadata panel).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub comment: String,
    /// Color Engine (Adobe-style built-in or OCIO with ACES working spaces).
    #[serde(default)]
    pub color_engine: ColorEngine,
    /// HDR handling for standard-dynamic-range output (display and renders).
    #[serde(default)]
    pub hdr: HdrMode,
    /// The space the top-level comp is output in (renders and the viewer's source). `None` =
    /// sRGB; Rec. 2100 PQ / HLG give HDR output.
    #[serde(default)]
    pub output_space: Option<ColorSpace>,
    /// Working Gamma, 2.2 or 2.4 (`app.project.workingGamma`): with no working space, the curve
    /// Interpret As Linear Light footage is encoded with and Blend Colors Using 1.0 Gamma
    /// linearizes with: the sRGB curve at 2.2, a pure 2.4 power curve at 2.4. Ignored with a
    /// working space (as in After Effects).
    #[serde(default = "gamma_22")]
    pub working_gamma: f64,
}

fn gamma_22() -> f64 {
    2.2
}

impl ProjectSettings {
    /// Working Gamma is 2.4 (anything else reads as 2.2).
    pub fn working_gamma_24(&self) -> bool {
        (self.working_gamma - 2.4).abs() < 0.05
    }
}

impl Default for ProjectSettings {
    fn default() -> Self {
        ProjectSettings {
            bit_depth: BitDepth::Bpc8,
            linearize: false,
            working_space: None,
            blend_linear: false,
            time_display: TimeDisplayStyle::Timecode,
            frame_start: 0,
            audio_sample_rate: 48_000,
            gpu_acceleration: true,
            comment: String::new(),
            color_engine: ColorEngine::Adobe,
            hdr: HdrMode::Clip,
            output_space: None,
            working_gamma: 2.2,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Marker {
    pub time: Tick,
    #[serde(default)]
    pub duration: Tick,
    #[serde(default)]
    pub comment: String,
    #[serde(default)]
    pub label: Label,
    #[serde(default)]
    pub chapter: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub protected: bool,
    /// Web link frame target (`_blank`, a frame name…).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub frame_target: String,
    /// Flash/video cue point (Event or Navigation) with name and parameters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cue_point: Option<CuePoint>,
}

/// A marker's cue point (Composition/Layer Marker dialog ▸ Cue Point).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CuePoint {
    pub name: String,
    /// Navigation (true) or Event cue point.
    #[serde(default)]
    pub navigation: bool,
    /// (name, value) parameter pairs.
    #[serde(default)]
    pub params: Vec<(String, String)>,
}

pub(crate) fn yes() -> bool {
    true
}
