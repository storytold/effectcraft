use super::item::{ItemId, LayerId};
use super::settings::Marker;
use super::{BlendMode, Deserialize, Label, PropGroup, Serialize, Tick};

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
