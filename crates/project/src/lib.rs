//! The EffectCraft document model.
//!
//! `Project` → items (folders, compositions, footage, solids) → `Comp` → `Layer` → property tree
//! ([`props`]). Compositions sit behind `Arc` so undo snapshots share everything that did not
//! change.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod build;
pub mod essential;
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
pub use sequence::MissingFrames;
use serde::{Deserialize, Serialize};

pub use effectcraft_keyframe as keyframe;
pub use effectcraft_keyframe::{Keyframe, Value};

pub const SCHEMA_VERSION: u32 = 1;

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
}

/// A viewer guide line: vertical guides sit at an x position, horizontal ones at a y position.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Guide {
    pub vertical: bool,
    pub position: f64,
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
    /// Draws the same pixels as `o`: equal but for its layers' Audio, Lock and Shy switches and
    /// Hide Shy Layers.
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

    /// The comp with those switches at their defaults (borrowed when they already are), for
    /// keys that hash the whole comp.
    pub fn pixel_form(&self) -> std::borrow::Cow<'_, Comp> {
        if !self.hide_shy && self.layers.iter().all(|l| l.switches == l.switches.pixels()) {
            return std::borrow::Cow::Borrowed(self);
        }
        let mut c = self.clone();
        c.hide_shy = false;
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
    /// Data footage ([`FootageKind::Data`]): the file's text (JSON, CSV or TSV), kept in the
    /// project so expressions read it everywhere (and on the web).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    /// PDF / Illustrator footage: the page shown (0-based; File ▸ Import ▸ Page).
    #[serde(default, skip_serializing_if = "is_default")]
    pub page: u32,
    /// Image sequences: Interpret Footage ▸ Missing Frames ([`sequence`]).
    #[serde(default, skip_serializing_if = "is_default")]
    pub missing_frames: MissingFrames,
    /// Image sequences: Interpret Footage ▸ Start Frame, the frame number at the footage's first
    /// frame (`None` = the first file's).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_frame: Option<i64>,
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
            data: None,
            layer: None,
            page: 0,
            missing_frames: MissingFrames::Skip,
            start_frame: None,
        }
    }
}

impl Footage {
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
            ItemKind::Footage(f) if f.kind != FootageKind::Still => Some(f.duration),
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
    pub fn add_item(&mut self, name: &str, label: Label, parent: Option<ItemId>, kind: ItemKind) -> ItemId {
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
    pub fn folder_named(&self, name: &str) -> Option<ItemId> {
        self.items.values().find(|i| i.is_folder() && i.name == name).map(|i| i.id)
    }
    /// Find the comp that contains `layer`.
    pub fn comp_of_layer(&self, layer: LayerId) -> Option<ItemId> {
        self.comps().find(|(_, c)| c.layer(layer).is_some()).map(|(id, _)| *id)
    }
    /// Whether comp `inner` is (transitively) nested inside comp `outer` (to prevent cycles).
    pub fn comp_contains(&self, outer: ItemId, inner: ItemId) -> bool {
        if outer == inner {
            return true;
        }
        let Some(c) = self.comp(outer) else { return false };
        c.layers.iter().any(|l| matches!(l.source, LayerSource::Comp { item } if self.comp_contains(item, inner)))
    }
    /// Make sure `next_id` exceeds every id in the project (after loading or merging).
    pub fn fix_next_id(&mut self) {
        let mut m = self.next_id;
        for (id, it) in &self.items {
            m = m.max(id.0 + 1);
            if let ItemKind::Comp(c) = &it.kind {
                for l in &c.layers {
                    m = m.max(l.id.0 + 1).max(l.props.max_uid() + 1);
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
    /// to read back, so a save never writes a file that can't be opened (JSON has no NaN or
    /// infinity: such a value would be written as `null`).
    pub fn to_file_json(&self) -> Result<String, ProjectError> {
        #[derive(Serialize)]
        struct File<'a> {
            #[serde(rename = "savedBy")]
            saved_by: &'a str,
            #[serde(flatten)]
            project: &'a Project,
        }
        let text = serde_json::to_string_pretty(&File { saved_by: APP_VERSION, project: self })
            .map_err(|e| ProjectError::Invalid(format!("the project can't be saved: {e}")))?;
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
        let mut p: Project = serde_json::from_str(s).map_err(|e| ProjectError::Invalid(e.to_string()))?;
        if p.schema > SCHEMA_VERSION {
            return Err(ProjectError::Invalid(format!("project schema {} is newer than this version supports ({SCHEMA_VERSION})", p.schema)));
        }
        p.fix_next_id();
        Ok(p)
    }
}

#[cfg(test)]
mod tests;
