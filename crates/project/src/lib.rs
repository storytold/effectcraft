//! The EffectCraft document model.
//!
//! `Project` → items (folders, compositions, footage, solids) → `Comp` → `Layer` → property tree
//! ([`props`]). Compositions sit behind `Arc` so undo snapshots share everything that did not
//! change.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod build;
pub mod essential;
mod file_check;
pub mod limits;
pub mod props;
pub mod render_queue;
pub mod render_templates;
pub mod sequence;
pub mod styles;
pub mod tracking;

use std::collections::BTreeMap;
use std::sync::Arc;

pub use effectcraft_color::ColorSpace;
use effectcraft_color::{BlendMode, Label};
use effectcraft_time::{FrameRate, Tick};
pub use props::{Expression, FeatherFalloff, GroupKind, MaskMode, MaskMotionBlur, Node, ParamUi, PropGroup, Property, Uid, parse_path};
use serde::{Deserialize, Serialize};

pub use effectcraft_keyframe as keyframe;
pub use effectcraft_keyframe::{Keyframe, Value};

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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ItemId(pub u64);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct LayerId(pub u64);

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
        let k: String = s.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase();
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

/// 3D renderer of a composition.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Renderer {
    #[default]
    Classic3D,
    Advanced3D,
}

/// A composition's Resolution (the Composition panel's Resolution/Down Sample Factor popup, View ▸
/// Resolution): how many of its pixels the viewer renders. Auto follows the magnification.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Resolution {
    #[default]
    Auto,
    Full,
    Half,
    Third,
    Quarter,
    /// View ▸ Resolution ▸ Custom: render every n-th pixel (2..=40).
    Custom(u8),
}

impl Resolution {
    pub const ALL: [Resolution; 5] = [Resolution::Auto, Resolution::Full, Resolution::Half, Resolution::Third, Resolution::Quarter];
    pub fn label(self) -> &'static str {
        match self {
            Resolution::Auto => "Auto",
            Resolution::Full => "Full",
            Resolution::Half => "Half",
            Resolution::Third => "Third",
            Resolution::Quarter => "Quarter",
            Resolution::Custom(_) => "Custom",
        }
    }
    pub fn is_auto(&self) -> bool {
        *self == Resolution::Auto
    }
    /// The down-sample factor (render every n-th pixel); `None` for Auto.
    pub fn factor(self) -> Option<u8> {
        match self {
            Resolution::Auto => None,
            Resolution::Full => Some(1),
            Resolution::Half => Some(2),
            Resolution::Third => Some(3),
            Resolution::Quarter => Some(4),
            Resolution::Custom(n) => Some(n),
        }
    }
    /// The resolution that renders every `n`-th pixel (1 = Full … 4 = Quarter; up to 40).
    pub fn from_factor(n: u64) -> Resolution {
        match n.clamp(1, 40) {
            1 => Resolution::Full,
            2 => Resolution::Half,
            3 => Resolution::Third,
            4 => Resolution::Quarter,
            n => Resolution::Custom(n as u8),
        }
    }
    /// Render scale for a viewer magnification (and display pixel density).
    pub fn scale(self, zoom: f32, ppp: f32) -> f64 {
        match self.factor() {
            Some(n) => 1.0 / f64::from(n.clamp(1, 40)),
            // As in After Effects, Auto renders the pixels the magnification needs: the coarsest
            // whole factor that still gives every screen pixel a rendered one (Full above 50 %,
            // Half down to 33.3 %, Third down to 25 %, Quarter below), never fewer (#417).
            None => {
                let n = (1.0 / (zoom * ppp) as f64 + 1e-3).floor();
                if n >= 1.0 { 1.0 / n.min(4.0) } else { 1.0 }
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Comp {
    pub width: u32,
    pub height: u32,
    pub pixel_aspect: f64,
    pub frame_rate: FrameRate,
    pub duration: Tick,
    /// Start timecode/frame shown for time 0.
    pub display_start: Tick,
    pub background: [f32; 3],
    pub work_area: (Tick, Tick),
    /// Layer #1 first (top of the stack).
    pub layers: Vec<Layer>,
    #[serde(default)]
    pub markers: Vec<Marker>,
    pub shutter_angle: f64,
    pub shutter_phase: f64,
    /// Samples Per Frame: the minimum motion-blur samples (and the count for 3D layers).
    pub motion_blur_samples: u32,
    /// Adaptive Sample Limit (Composition Settings ▸ Advanced): the most samples a 2D layer gets
    /// when it moves far within the shutter.
    #[serde(default = "adaptive_limit_default")]
    pub motion_blur_adaptive_limit: u32,
    #[serde(default)]
    pub renderer: Renderer,
    /// Comp switches (timeline toolbar).
    #[serde(default)]
    pub hide_shy: bool,
    #[serde(default = "yes")]
    pub enable_motion_blur: bool,
    #[serde(default = "yes")]
    pub enable_frame_blending: bool,
    #[serde(default)]
    pub draft_3d: bool,
    /// Composition Settings ▸ Advanced ▸ Preserve frame rate when nested or in render queue:
    /// nested in another comp, or rendered at another rate, it shows only its own frames.
    #[serde(default)]
    pub preserve_frame_rate: bool,
    /// Composition Settings ▸ Advanced ▸ Preserve resolution when nested: nested in a comp
    /// previewed at a lower resolution, it still renders at full size.
    #[serde(default)]
    pub preserve_resolution: bool,
    #[serde(default)]
    pub poster_time: Tick,
    /// Global Light for layer styles (Layer ▸ Layer Styles ▸ Blending Options).
    #[serde(default)]
    pub global_light: styles::GlobalLight,
    /// Viewer guides (View ▸ Add Guide…), in comp pixels.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub guides: Vec<Guide>,
    /// Essential Graphics: the controls this comp exposes to instances and templates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub essential: Option<essential::EssentialGraphics>,
    /// The Composition panel's Resolution, saved with the project as in After Effects. It changes
    /// what the viewer renders, not the comp's pixels ([`Comp::same_pixels`]), and stays out of
    /// the undo history.
    #[serde(default, skip_serializing_if = "Resolution::is_auto")]
    pub resolution: Resolution,
}

/// A viewer guide line: vertical guides sit at an x position, horizontal ones at a y position.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Guide {
    pub vertical: bool,
    /// Comp pixels, or percent of the comp's width (vertical) / height (horizontal) when
    /// [`Guide::percent`] is set.
    pub position: f64,
    /// A percentage guide (After Effects 26.5): `position` is a percent of the comp's size, so the
    /// guide keeps its relative place when the comp is resized. Absent in older projects.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub percent: bool,
}

impl Guide {
    /// The comp's extent along this guide's axis, never zero.
    fn extent(&self, width: u32, height: u32) -> f64 {
        f64::from(if self.vertical { width } else { height }).max(1.0)
    }

    /// The guide's position in comp pixels for a comp of this size.
    pub fn px(&self, width: u32, height: u32) -> f64 {
        if self.percent { self.position / 100.0 * self.extent(width, height) } else { self.position }
    }

    /// Place the guide at `px` comp pixels, keeping its unit.
    pub fn set_px(&mut self, px: f64, width: u32, height: u32) {
        self.position = if self.percent { px / self.extent(width, height) * 100.0 } else { px };
    }

    /// Switch between pixels and percent without moving the guide on the comp.
    pub fn set_percent(&mut self, percent: bool, width: u32, height: u32) {
        let px = self.px(width, height);
        self.percent = percent;
        self.set_px(px, width, height);
    }
}

fn yes() -> bool {
    true
}

fn adaptive_limit_default() -> u32 {
    128
}

impl Comp {
    pub fn new(width: u32, height: u32, frame_rate: FrameRate, duration: Tick) -> Comp {
        Comp {
            width,
            height,
            pixel_aspect: 1.0,
            frame_rate,
            duration,
            display_start: Tick::ZERO,
            background: [0.0, 0.0, 0.0],
            work_area: (Tick::ZERO, duration),
            layers: vec![],
            markers: vec![],
            shutter_angle: 180.0,
            shutter_phase: -90.0,
            motion_blur_samples: 16,
            motion_blur_adaptive_limit: 128,
            renderer: Renderer::Classic3D,
            hide_shy: false,
            enable_motion_blur: true,
            enable_frame_blending: true,
            draft_3d: false,
            preserve_frame_rate: false,
            preserve_resolution: false,
            poster_time: Tick::ZERO,
            guides: vec![],
            global_light: styles::GlobalLight::default(),
            essential: None,
            resolution: Resolution::Auto,
        }
    }
    /// Whether the composition has a frame at its time `t`: nested, it shows nothing before it
    /// starts or after it ends (a precomp layer trimmed or remapped past it).
    pub fn covers(&self, t: Tick) -> bool {
        t >= Tick::ZERO && t < self.duration
    }
    /// A new, empty composition with this one's settings (frame rate, pixel aspect, background,
    /// motion blur, frame blending and the 3D renderer), as Pre-compose makes.
    pub fn nested_like(&self, width: u32, height: u32, duration: Tick) -> Comp {
        Comp {
            pixel_aspect: self.pixel_aspect,
            background: self.background,
            shutter_angle: self.shutter_angle,
            shutter_phase: self.shutter_phase,
            motion_blur_samples: self.motion_blur_samples,
            motion_blur_adaptive_limit: self.motion_blur_adaptive_limit,
            renderer: self.renderer,
            enable_motion_blur: self.enable_motion_blur,
            enable_frame_blending: self.enable_frame_blending,
            preserve_frame_rate: self.preserve_frame_rate,
            preserve_resolution: self.preserve_resolution,
            ..Comp::new(width, height, self.frame_rate, duration)
        }
    }
    pub fn layer(&self, id: LayerId) -> Option<&Layer> {
        self.layers.iter().find(|l| l.id == id)
    }
    pub fn layer_mut(&mut self, id: LayerId) -> Option<&mut Layer> {
        self.layers.iter_mut().find(|l| l.id == id)
    }
    /// 1-based layer number (#) of a layer.
    pub fn index_of(&self, id: LayerId) -> Option<usize> {
        self.layers.iter().position(|l| l.id == id).map(|i| i + 1)
    }
    pub fn layer_by_name(&self, name: &str) -> Option<&Layer> {
        self.layers.iter().find(|l| l.name == name)
    }
    pub fn frame_duration(&self) -> Tick {
        self.frame_rate.frame_duration()
    }
    /// Any 3D layer present (camera/lights or a 3D switch).
    pub fn has_3d(&self) -> bool {
        self.layers.iter().any(|l| l.switches.three_d || matches!(l.source, LayerSource::Camera | LayerSource::Light { .. }))
    }
    /// The active camera at `t`: the topmost enabled camera layer active at that time.
    pub fn active_camera(&self, t: Tick) -> Option<&Layer> {
        self.layers.iter().find(|l| matches!(l.source, LayerSource::Camera) && l.switches.video && l.is_active_at(t))
    }
    /// Unique layer name with a numeric suffix (`Shape Layer 2`).
    pub fn unique_layer_name(&self, base: &str) -> String {
        if !self.layers.iter().any(|l| l.name == base) {
            return base.to_string();
        }
        let stem = base.trim_end_matches(|c: char| c.is_ascii_digit()).trim_end();
        (2..).map(|i| format!("{stem} {i}")).find(|n| !self.layers.iter().any(|l| &l.name == n)).unwrap_or_default()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LightKind {
    Parallel,
    Spot,
    #[default]
    Point,
    Ambient,
    Environment,
}

impl LightKind {
    pub const ALL: [LightKind; 5] = [LightKind::Parallel, LightKind::Spot, LightKind::Point, LightKind::Ambient, LightKind::Environment];
    pub fn label(self) -> &'static str {
        match self {
            LightKind::Parallel => "Parallel",
            LightKind::Spot => "Spot",
            LightKind::Point => "Point",
            LightKind::Ambient => "Ambient",
            LightKind::Environment => "Environment",
        }
    }
}

/// A parametric 3D primitive (Layer ▸ New ▸ Cube/Sphere/Plane/Torus/Cone/Cylinder).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PrimitiveKind {
    #[default]
    Cube,
    Sphere,
    Plane,
    Torus,
    Cone,
    Cylinder,
}

impl PrimitiveKind {
    pub const ALL: [PrimitiveKind; 6] =
        [PrimitiveKind::Cube, PrimitiveKind::Sphere, PrimitiveKind::Plane, PrimitiveKind::Torus, PrimitiveKind::Cone, PrimitiveKind::Cylinder];
    pub fn label(self) -> &'static str {
        match self {
            PrimitiveKind::Cube => "Cube",
            PrimitiveKind::Sphere => "Sphere",
            PrimitiveKind::Plane => "Plane",
            PrimitiveKind::Torus => "Torus",
            PrimitiveKind::Cone => "Cone",
            PrimitiveKind::Cylinder => "Cylinder",
        }
    }
    pub fn parse(s: &str) -> Option<PrimitiveKind> {
        PrimitiveKind::ALL.into_iter().find(|k| k.label().eq_ignore_ascii_case(s.trim()))
    }
}

/// What a layer shows.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum LayerSource {
    /// Footage item (video, still, sequence, audio).
    Footage {
        item: ItemId,
    },
    /// Nested composition (precomp).
    Comp {
        item: ItemId,
    },
    /// Solid item (solids also back adjustment layers).
    Solid {
        item: ItemId,
    },
    Text,
    Shape,
    Null,
    Camera,
    Light {
        kind: LightKind,
    },
    /// An imported 3D model (a footage item of kind [`FootageKind::Model`]; Advanced 3D).
    Model {
        item: ItemId,
    },
    /// A parametric 3D primitive (Advanced 3D).
    Primitive {
        kind: PrimitiveKind,
    },
}

impl LayerSource {
    pub fn item(&self) -> Option<ItemId> {
        match self {
            LayerSource::Footage { item } | LayerSource::Comp { item } | LayerSource::Solid { item } | LayerSource::Model { item } => Some(*item),
            _ => None,
        }
    }
    pub fn type_name(&self) -> &'static str {
        match self {
            LayerSource::Footage { .. } => "Footage",
            LayerSource::Comp { .. } => "Composition",
            LayerSource::Solid { .. } => "Solid",
            LayerSource::Text => "Text",
            LayerSource::Shape => "Shape",
            LayerSource::Null => "Null",
            LayerSource::Camera => "Camera",
            LayerSource::Light { .. } => "Light",
            LayerSource::Model { .. } => "3D Model",
            LayerSource::Primitive { .. } => "3D Primitive",
        }
    }
    /// Layers with pixels (cameras, lights and nulls have none).
    pub fn is_av(&self) -> bool {
        !matches!(self, LayerSource::Camera | LayerSource::Light { .. } | LayerSource::Null)
    }
    /// A 3D model or primitive layer (meshes; drawn by the Advanced 3D renderer).
    pub fn is_model(&self) -> bool {
        matches!(self, LayerSource::Model { .. } | LayerSource::Primitive { .. })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Quality {
    #[default]
    Best,
    Draft,
    Wireframe,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Sampling {
    #[default]
    Bilinear,
    Bicubic,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FrameBlend {
    #[default]
    Off,
    FrameMix,
    PixelMotion,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Switches {
    pub video: bool,
    pub audio: bool,
    pub solo: bool,
    pub locked: bool,
    pub shy: bool,
    /// Collapse transformations (precomps) / continuously rasterize (vector layers).
    pub collapse: bool,
    pub quality: Quality,
    pub sampling: Sampling,
    pub effects: bool,
    pub frame_blend: FrameBlend,
    pub motion_blur: bool,
    pub adjustment: bool,
    pub three_d: bool,
    pub guide: bool,
}

impl Default for Switches {
    fn default() -> Self {
        Switches {
            video: true,
            audio: true,
            solo: false,
            locked: false,
            shy: false,
            collapse: false,
            quality: Quality::Best,
            sampling: Sampling::Bilinear,
            effects: true,
            frame_blend: FrameBlend::Off,
            motion_blur: false,
            adjustment: false,
            three_d: false,
            guide: false,
        }
    }
}

// What a frame's pixels depend on. Caches key on it: the Audio, Lock and Shy switches and Hide
// Shy Layers don't change pixels, so toggling them keeps cached frames (#103).

impl Switches {
    /// These switches with Audio, Lock and Shy at their defaults.
    pub fn pixels(&self) -> Switches {
        Switches { audio: true, locked: false, shy: false, ..self.clone() }
    }
}

impl Layer {
    /// Draws the same pixels as `o`: equal but for the Audio, Lock and Shy switches.
    pub fn same_pixels(&self, o: &Layer) -> bool {
        // Every field (no `..`: a new field must be considered here); the property tree last.
        let Layer {
            id,
            name,
            source,
            label,
            comment,
            start_time,
            in_point,
            out_point,
            stretch,
            switches,
            blend_mode,
            preserve_transparency,
            track_matte,
            parent,
            markers,
            markers_locked,
            auto_orient,
            environment,
            environment_background,
            props,
        } = self;
        *id == o.id
            && *name == o.name
            && *source == o.source
            && *label == o.label
            && *comment == o.comment
            && *start_time == o.start_time
            && *in_point == o.in_point
            && *out_point == o.out_point
            && *stretch == o.stretch
            && switches.pixels() == o.switches.pixels()
            && *blend_mode == o.blend_mode
            && *preserve_transparency == o.preserve_transparency
            && *track_matte == o.track_matte
            && *parent == o.parent
            && *markers == o.markers
            && *markers_locked == o.markers_locked
            && *auto_orient == o.auto_orient
            && *environment == o.environment
            && *environment_background == o.environment_background
            && *props == o.props
    }
}

impl Comp {
    /// Draws the same pixels as `o`: equal but for its layers' Audio, Lock and Shy switches, Hide
    /// Shy Layers and the viewer's Resolution.
    pub fn same_pixels(&self, o: &Comp) -> bool {
        // Every field (no `..`: a new field must be considered here); the layers last.
        let Comp {
            width,
            height,
            pixel_aspect,
            frame_rate,
            duration,
            display_start,
            background,
            work_area,
            layers,
            markers,
            shutter_angle,
            shutter_phase,
            motion_blur_samples,
            motion_blur_adaptive_limit,
            renderer,
            hide_shy: _,
            enable_motion_blur,
            enable_frame_blending,
            draft_3d,
            preserve_frame_rate,
            preserve_resolution,
            poster_time,
            global_light,
            guides,
            essential,
            resolution: _,
        } = self;
        *width == o.width
            && *height == o.height
            && *pixel_aspect == o.pixel_aspect
            && *frame_rate == o.frame_rate
            && *duration == o.duration
            && *display_start == o.display_start
            && *background == o.background
            && *work_area == o.work_area
            && *markers == o.markers
            && *shutter_angle == o.shutter_angle
            && *shutter_phase == o.shutter_phase
            && *motion_blur_samples == o.motion_blur_samples
            && *motion_blur_adaptive_limit == o.motion_blur_adaptive_limit
            && *renderer == o.renderer
            && *enable_motion_blur == o.enable_motion_blur
            && *enable_frame_blending == o.enable_frame_blending
            && *draft_3d == o.draft_3d
            && *preserve_frame_rate == o.preserve_frame_rate
            && *preserve_resolution == o.preserve_resolution
            && *poster_time == o.poster_time
            && *global_light == o.global_light
            && *guides == o.guides
            && *essential == o.essential
            && layers.len() == o.layers.len()
            && layers.iter().zip(&o.layers).all(|(a, b)| a.same_pixels(b))
    }

    /// Draws the same pixels as `o` once plain markers are ignored: they change no pixels. Protected
    /// time markers with a duration set the timing (Responsive Design), so those still count.
    pub fn same_pixels_ignoring_markers(&self, o: &Comp) -> bool {
        let protected =
            |m: &[Marker]| -> Vec<(Tick, Tick)> { m.iter().filter(|m| m.protected && m.duration > Tick::ZERO).map(|m| (m.time, m.duration)).collect() };
        if protected(&self.markers) != protected(&o.markers) {
            return false;
        }
        let mut c = self.clone();
        c.markers = o.markers.clone();
        for (a, b) in c.layers.iter_mut().zip(&o.layers) {
            a.markers = b.markers.clone();
            a.markers_locked = b.markers_locked;
        }
        c.same_pixels(o)
    }

    /// The comp with those switches and the Resolution at their defaults (borrowed when they
    /// already are), for keys that hash the whole comp.
    pub fn pixel_form(&self) -> std::borrow::Cow<'_, Comp> {
        if !self.hide_shy && self.resolution.is_auto() && self.layers.iter().all(|l| l.switches == l.switches.pixels()) {
            return std::borrow::Cow::Borrowed(self);
        }
        let mut c = self.clone();
        c.hide_shy = false;
        c.resolution = Resolution::Auto;
        for l in &mut c.layers {
            l.switches = l.switches.pixels();
        }
        std::borrow::Cow::Owned(c)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum MatteKind {
    #[default]
    Alpha,
    AlphaInverted,
    Luma,
    LumaInverted,
}

impl MatteKind {
    pub const ALL: [MatteKind; 4] = [MatteKind::Alpha, MatteKind::AlphaInverted, MatteKind::Luma, MatteKind::LumaInverted];
    pub fn label(self) -> &'static str {
        match self {
            MatteKind::Alpha => "Alpha Matte",
            MatteKind::AlphaInverted => "Alpha Inverted Matte",
            MatteKind::Luma => "Luma Matte",
            MatteKind::LumaInverted => "Luma Inverted Matte",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackMatte {
    pub layer: LayerId,
    pub kind: MatteKind,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AutoOrient {
    #[default]
    Off,
    AlongPath,
    TowardsCamera,
    TowardsPointOfInterest,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    pub id: LayerId,
    pub name: String,
    pub source: LayerSource,
    pub label: Label,
    #[serde(default)]
    pub comment: String,
    /// Comp time where the layer's time 0 sits.
    pub start_time: Tick,
    /// Visible span in comp time.
    pub in_point: Tick,
    pub out_point: Tick,
    /// Time stretch in percent (100 = normal, negative = reversed).
    pub stretch: f64,
    pub switches: Switches,
    pub blend_mode: BlendMode,
    #[serde(default)]
    pub preserve_transparency: bool,
    #[serde(default)]
    pub track_matte: Option<TrackMatte>,
    #[serde(default)]
    pub parent: Option<LayerId>,
    #[serde(default)]
    pub markers: Vec<Marker>,
    /// Layer ▸ Markers ▸ Lock Markers.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub markers_locked: bool,
    #[serde(default)]
    pub auto_orient: AutoOrient,
    /// Layer ▸ Environment Layer: the layer is the environment (image-based light and
    /// reflections) of Advanced 3D comps instead of being drawn.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub environment: bool,
    /// Layer ▸ Light ▸ Create Environment Light Background Layer: the layer's (equirectangular)
    /// image is drawn as the 3D scene's backdrop, seen through the camera, instead of as a card.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub environment_background: bool,
    /// The property tree (Masks, Effects, Transform, Text, Contents, Camera/Light options…).
    pub props: PropGroup,
}

impl Layer {
    pub fn is_active_at(&self, t: Tick) -> bool {
        t >= self.in_point && t < self.out_point
    }
    /// Layer time for a comp time (stretch-aware).
    pub fn layer_time(&self, comp_t: Tick) -> Tick {
        let d = comp_t - self.start_time;
        if (self.stretch - 100.0).abs() < 1e-9 { d } else { Tick((d.0 as f64 * 100.0 / self.stretch) as i64) }
    }
    /// Comp time for a layer time.
    pub fn comp_time(&self, layer_t: Tick) -> Tick {
        if (self.stretch - 100.0).abs() < 1e-9 { self.start_time + layer_t } else { self.start_time + Tick((layer_t.0 as f64 * self.stretch / 100.0) as i64) }
    }
    pub fn transform(&self) -> Option<&PropGroup> {
        self.props.sub("transform")
    }
    pub fn transform_mut(&mut self) -> Option<&mut PropGroup> {
        self.props.sub_mut("transform")
    }
    pub fn effects(&self) -> Option<&PropGroup> {
        self.props.sub("effects")
    }
    pub fn masks(&self) -> Option<&PropGroup> {
        self.props.sub("masks")
    }
    pub fn is_camera(&self) -> bool {
        matches!(self.source, LayerSource::Camera)
    }
    pub fn is_light(&self) -> bool {
        matches!(self.source, LayerSource::Light { .. })
    }
    /// The layer participates in 3D (3D switch, or cameras/lights which are always 3D).
    pub fn is_3d(&self) -> bool {
        self.switches.three_d || self.is_camera() || self.is_light()
    }
    pub fn has_video(&self) -> bool {
        self.source.is_av() && self.switches.video
    }
}

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
        let ext = std::path::Path::new(&self.path).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
#[allow(clippy::large_enum_variant)]
pub enum ItemKind {
    Folder,
    Comp(Arc<Comp>),
    Footage(Footage),
    Solid(Solid),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Item {
    pub id: ItemId,
    pub name: String,
    pub label: Label,
    #[serde(default)]
    pub comment: String,
    /// Containing folder (None = project root).
    #[serde(default)]
    pub parent: Option<ItemId>,
    pub kind: ItemKind,
    /// The item's proxy (footage and compositions).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy: Option<Box<Proxy>>,
}

impl Item {
    pub fn type_name(&self) -> &'static str {
        match &self.kind {
            ItemKind::Folder => "Folder",
            ItemKind::Comp(_) => "Composition",
            ItemKind::Footage(f) => match f.kind {
                FootageKind::Video => "Video",
                FootageKind::Still => "Image",
                FootageKind::Sequence => "Image Sequence",
                FootageKind::Audio => "Audio",
                FootageKind::Model => "3D Model",
                FootageKind::Data => "Data",
            },
            ItemKind::Solid(_) => "Solid",
        }
    }
    pub fn as_comp(&self) -> Option<&Comp> {
        if let ItemKind::Comp(c) = &self.kind { Some(c) } else { None }
    }
    pub fn is_folder(&self) -> bool {
        matches!(self.kind, ItemKind::Folder)
    }
    /// (width, height) for visual items.
    pub fn dimensions(&self) -> Option<(u32, u32)> {
        match &self.kind {
            ItemKind::Comp(c) => Some((c.width, c.height)),
            ItemKind::Footage(f) if f.has_video => Some((f.width, f.height)),
            ItemKind::Solid(s) => Some((s.width, s.height)),
            _ => None,
        }
    }
    pub fn duration(&self) -> Option<Tick> {
        match &self.kind {
            ItemKind::Comp(c) => Some(c.duration),
            ItemKind::Footage(f) if f.kind != FootageKind::Still => Some(f.looped_duration()),
            _ => None,
        }
    }
    pub fn frame_rate(&self) -> Option<FrameRate> {
        match &self.kind {
            ItemKind::Comp(c) => Some(c.frame_rate),
            ItemKind::Footage(f) if f.kind == FootageKind::Video || f.kind == FootageKind::Sequence => Some(f.frame_rate),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub schema: u32,
    pub settings: ProjectSettings,
    pub items: BTreeMap<ItemId, Item>,
    pub next_id: u64,
    /// The Render Queue (Composition ▸ Add to Render Queue).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub render_queue: Vec<render_queue::RenderQueueItem>,
    /// Render Settings / Output Module templates and their defaults.
    #[serde(default, skip_serializing_if = "render_templates::RenderTemplates::is_default")]
    pub render_templates: render_templates::RenderTemplates,
    /// Render Queue preferences (Notify, storage overflow folders).
    #[serde(default, skip_serializing_if = "render_queue::RenderQueuePrefs::is_default")]
    pub render_prefs: render_queue::RenderQueuePrefs,
}

impl Default for Project {
    fn default() -> Self {
        Project {
            schema: SCHEMA_VERSION,
            settings: ProjectSettings::default(),
            items: BTreeMap::new(),
            next_id: 1,
            render_queue: Vec::new(),
            render_templates: Default::default(),
            render_prefs: Default::default(),
        }
    }
}

impl Project {
    /// Allocate an id (items, layers and property uids share one counter).
    pub fn alloc(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }
    pub fn item(&self, id: ItemId) -> Option<&Item> {
        self.items.get(&id)
    }
    pub fn item_mut(&mut self, id: ItemId) -> Option<&mut Item> {
        self.items.get_mut(&id)
    }
    pub fn comp(&self, id: ItemId) -> Option<&Comp> {
        self.items.get(&id)?.as_comp()
    }
    /// A shared handle on a comp: a snapshot that costs a reference count, not a deep copy of
    /// its layers (frontends hold one across a frame instead of cloning the comp).
    pub fn comp_arc(&self, id: ItemId) -> Option<Arc<Comp>> {
        match &self.items.get(&id)?.kind {
            ItemKind::Comp(c) => Some(c.clone()),
            _ => None,
        }
    }
    /// Mutable comp (copy-on-write).
    pub fn comp_mut(&mut self, id: ItemId) -> Option<&mut Comp> {
        match &mut self.items.get_mut(&id)?.kind {
            ItemKind::Comp(c) => Some(Arc::make_mut(c)),
            _ => None,
        }
    }
    /// Adds an item; its size, frame rate and the like are brought within [`limits`] first.
    pub fn add_item(&mut self, name: &str, label: Label, parent: Option<ItemId>, mut kind: ItemKind) -> ItemId {
        limits::repair_kind(name, &mut kind, &mut vec![]);
        let id = ItemId(self.alloc());
        self.items.insert(id, Item { id, name: name.into(), label, comment: String::new(), parent, kind, proxy: None });
        id
    }
    /// Children of a folder (None = root), folders first then by name.
    pub fn children(&self, folder: Option<ItemId>) -> Vec<&Item> {
        let mut v: Vec<&Item> = self.items.values().filter(|i| i.parent == folder).collect();
        v.sort_by(|a, b| b.is_folder().cmp(&a.is_folder()).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
        v
    }
    pub fn comps(&self) -> impl Iterator<Item = (&ItemId, &Comp)> {
        self.items.iter().filter_map(|(id, i)| i.as_comp().map(|c| (id, c)))
    }
    pub fn find_by_name(&self, name: &str) -> Option<&Item> {
        self.items.values().find(|i| i.name == name)
    }
    /// A numbered name for a copy of `base` that no project item has (Edit ▸ Duplicate): its
    /// trailing number is dropped and the lowest free one from 2 is added (`Comp 1` → `Comp 2`,
    /// `Name` → `Name 2`, then `Name 3`). Items for which `ignore` holds don't count.
    pub fn numbered_item_name(&self, base: &str, ignore: impl Fn(&Item) -> bool) -> String {
        let stem = base.trim_end_matches(|c: char| c.is_ascii_digit()).trim_end();
        let taken = |n: &str| self.items.values().any(|i| i.name == n && !ignore(i));
        (2u64..).map(|k| format!("{stem} {k}")).find(|n| !taken(n)).unwrap_or_default()
    }
    /// `base` when no project item has that name, else the next free numbered name as
    /// [`Project::numbered_item_name`] gives it. Items for which `ignore` holds don't count.
    pub fn unique_item_name(&self, base: &str, ignore: impl Fn(&Item) -> bool) -> String {
        if self.items.values().any(|i| i.name == base && !ignore(i)) { self.numbered_item_name(base, ignore) } else { base.to_string() }
    }
    pub fn folder_named(&self, name: &str) -> Option<ItemId> {
        self.items.values().find(|i| i.is_folder() && i.name == name).map(|i| i.id)
    }
    /// Find the comp that contains `layer`.
    pub fn comp_of_layer(&self, layer: LayerId) -> Option<ItemId> {
        self.comps().find(|(_, c)| c.layer(layer).is_some()).map(|(id, _)| *id)
    }
    /// The compositions whose layers use `item` as their source (footage, solid, 3D model or
    /// precomp), each with how many of its layers do, ordered by comp name then id. Only direct
    /// layers count, as in After Effects' Project panel ("used 3 times"): a solid inside a precomp
    /// is used by the precomp, not by the comps that nest it. The total is the sum of the counts.
    pub fn item_users(&self, item: ItemId) -> Vec<(ItemId, usize)> {
        let mut users: Vec<(ItemId, String, usize)> = self
            .items
            .iter()
            .filter_map(|(id, it)| {
                let comp = it.as_comp()?;
                let n = comp.layers.iter().filter(|l| l.source.item() == Some(item)).count();
                (n > 0).then(|| (*id, it.name.to_lowercase(), n))
            })
            .collect();
        users.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
        users.into_iter().map(|(id, _, n)| (id, n)).collect()
    }
    /// Whether comp `inner` is (transitively) nested inside comp `outer` (to prevent cycles).
    pub fn comp_contains(&self, outer: ItemId, inner: ItemId) -> bool {
        // A walk with a stack and a visited set rather than recursion (#1237): a damaged project
        // may hold a precomp cycle that doesn't pass `inner` (recursion never ended), a long
        // chain must not use up the call stack, and comps shared by many paths are visited once
        // (recursion re-walked them for every path).
        if outer == inner {
            return true;
        }
        let mut seen = std::collections::HashSet::new();
        let mut stack = vec![outer];
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            let Some(c) = self.comp(id) else { continue };
            for l in &c.layers {
                if let LayerSource::Comp { item } = l.source {
                    if item == inner {
                        return true;
                    }
                    if !seen.contains(&item) {
                        stack.push(item);
                    }
                }
            }
        }
        false
    }
    /// Make sure `next_id` exceeds every id in the project (after loading or merging).
    pub fn fix_next_id(&mut self) {
        let mut m = self.next_id;
        for (id, it) in &self.items {
            // Saturating: a damaged file's ids must not overflow (the import then refuses it).
            m = m.max(id.0.saturating_add(1));
            if let ItemKind::Comp(c) = &it.kind {
                for l in &c.layers {
                    m = m.max(l.id.0.saturating_add(1)).max(l.props.max_uid().saturating_add(1));
                }
            }
        }
        self.next_id = m;
    }
    /// Whether `other` holds the same content. An allocated id counts (an edit may hand it out
    /// beyond the project, so it must stay taken). Shared compositions compare by pointer first,
    /// so after a copy-on-write edit only the comps it touched are compared field by field.
    pub fn same_content(&self, other: &Project) -> bool {
        let item_eq = |a: &Item, b: &Item| {
            let kind = match (&a.kind, &b.kind) {
                (ItemKind::Comp(x), ItemKind::Comp(y)) => Arc::ptr_eq(x, y) || x == y,
                (x, y) => x == y,
            };
            kind && a.id == b.id && a.name == b.name && a.label == b.label && a.comment == b.comment && a.parent == b.parent && a.proxy == b.proxy
        };
        self.schema == other.schema
            && self.next_id == other.next_id
            && self.settings == other.settings
            && self.items.len() == other.items.len()
            && self.items.iter().zip(&other.items).all(|((ka, a), (kb, b))| ka == kb && item_eq(a, b))
            && self.render_queue == other.render_queue
            && self.render_templates == other.render_templates
            && self.render_prefs == other.render_prefs
    }
    /// The project file's text: the project with the version that wrote it (`savedBy`), checked
    /// to read back, so a save never writes a file that can't be opened. Values JSON can't hold
    /// (NaN or infinity, written as `null`, and nesting too deep to parse) are refused on every
    /// save; debug builds also read the text back.
    pub fn to_file_json(&self) -> Result<String, ProjectError> {
        self.to_file_json_with_view(&SavedView::default())
    }
    /// [`Project::to_file_json`] that also records the open compositions ([`SavedView`]).
    pub fn to_file_json_with_view(&self, view: &SavedView) -> Result<String, ProjectError> {
        if self.schema > SCHEMA_VERSION {
            return Err(ProjectError::Invalid(format!(
                "the project can't be saved: it holds a value a project file can't store (project schema {} is newer than this version supports ({SCHEMA_VERSION}))",
                self.schema
            )));
        }
        #[derive(Serialize)]
        struct File<'a> {
            #[serde(rename = "savedBy")]
            saved_by: &'a str,
            #[serde(flatten)]
            view: &'a SavedView,
            #[serde(flatten)]
            project: &'a Project,
        }
        let file = File { saved_by: APP_VERSION, view, project: self };
        file_check::check(&file)
            .map_err(|e| ProjectError::Invalid(format!("the project can't be saved: it holds a value a project file can't store ({e})")))?;
        let text = serde_json::to_string_pretty(&file).map_err(|e| ProjectError::Invalid(format!("the project can't be saved: {e}")))?;
        #[cfg(debug_assertions)]
        Project::from_json(&text)
            .map_err(|e| ProjectError::Invalid(format!("the project can't be saved: it holds a value a project file can't store ({e})")))?;
        Ok(text)
    }
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }
    /// [`Project::to_json`] without indentation: smaller and faster (auto-saves).
    pub fn to_json_compact(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
    pub fn from_json(s: &str) -> Result<Project, ProjectError> {
        Project::from_json_repaired(s).map(|(p, _)| p)
    }
    /// [`Project::from_json`], also returning one line per value that was out of [`limits`] and
    /// was corrected (a composition of a billion pixels, a zero frame rate…), so a corrupt or
    /// hostile file still opens, within bounds, and the user can be told what changed.
    pub fn from_json_repaired(s: &str) -> Result<(Project, Vec<String>), ProjectError> {
        let mut p: Project = serde_json::from_str(s).map_err(|e| ProjectError::Invalid(e.to_string()))?;
        if p.schema > SCHEMA_VERSION {
            return Err(ProjectError::Invalid(format!("project schema {} is newer than this version supports ({SCHEMA_VERSION})", p.schema)));
        }
        p.fix_next_id();
        let mut notes = vec![];
        for item in p.items.values_mut() {
            limits::repair_item(item, &mut notes);
        }
        Ok((p, notes))
    }
}

#[cfg(test)]
mod tests;
