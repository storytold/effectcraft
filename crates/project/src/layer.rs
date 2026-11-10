use super::item::{ItemId, LayerId};
use super::settings::Marker;
use super::{BlendMode, Deserialize, GroupKind, Label, Node, ParamUi, PropGroup, Property, Serialize, Tick, Value};

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

// What a frame's pixels depend on. Caches key on it: the Lock and Shy switches and Hide Shy
// Layers never change pixels, and the Audio switch, the Audio group and audio effects change them
// only when something draws the layer's audio (a waveform or spectrum picks it, see
// [`crate::Project::heard_layers`]), so editing those keeps cached frames (#103).

/// Is `id` an audio effect (applied by the audio mix rather than the compositor)?
pub fn is_audio_effect(id: &str) -> bool {
    id.starts_with("ec.audio.")
}

/// Does effect `id` read its own layer at other times (`EffectHost::self_at`, `layer_at` on
/// itself): the time effects (Echo, Posterize Time, Timewarp…), simulations, temporal smoothing
/// and analysis (Remove Grain, Auto Levels, Color Stabilizer's reference frame, Warp Stabilizer,
/// Roto Brush…) and Clone strokes with a time offset? On a precomp layer, a change at one nested
/// time then reaches frames of the layer far from where it shows. Plug-in effects (every id
/// outside `ec.`) count too: plug-in API 2 reads the input at other times (`PluginHost`).
// ponytail: hand-kept list of the effects that call `self_at`; a new one must be added here.
pub fn reads_other_frames(id: &str) -> bool {
    !id.starts_with("ec.") || id.starts_with("ec.time.") || id.starts_with("ec.sim.") || READS_OTHER_FRAMES.contains(&id)
}

/// The effects outside `ec.time.` and `ec.sim.` that read their own layer at other times.
const READS_OTHER_FRAMES: &[&str] = &[
    "ec.noise.removegrain",
    "ec.color.autolevels",
    "ec.color.autocontrast",
    "ec.color.autocolor",
    "ec.color.shadowhighlight",
    "ec.color.colorstabilizer",
    "ec.matte.refinesoft",
    "ec.matte.refinehard",
    "ec.matte.rotobrush",
    "ec.matte.objectmatte",
    "ec.blur.camerashakedeblur",
    "ec.distort.rollingshutterrepair",
    "ec.distort.warpstabilizer",
    "ec.key.ccsimplewireremoval",
    "ec.paint.paint",
];

impl Switches {
    /// These switches with Audio, Lock and Shy at their defaults.
    pub fn pixels(&self) -> Switches {
        Switches { audio: true, locked: false, shy: false, ..self.clone() }
    }

    /// The switches that change the pixels of a layer: [`Self::pixels`], keeping Audio when
    /// something draws the layer's audio (`heard`).
    pub fn picture(&self, heard: bool) -> Switches {
        Switches { audio: if heard { self.audio } else { true }, ..self.pixels() }
    }
}

/// A node only the audio mix reads: the layer's Audio group, an audio effect.
fn audio_only(n: &Node, root: bool) -> bool {
    match n {
        Node::Group(g) => (root && g.match_id == "audio") || matches!(&g.kind, GroupKind::Effect { effect } if is_audio_effect(effect)),
        Node::Prop(_) => false,
    }
}

/// `a` and `b` are equal but for their audio-only nodes ([`audio_only`]: the root's Audio group,
/// the effects in its Effects group that are audio effects).
fn same_picture_props(a: &PropGroup, b: &PropGroup) -> bool {
    fn head(a: &PropGroup, b: &PropGroup) -> bool {
        // Every field (no `..`: a new field must be considered here).
        let PropGroup { uid, match_id, name, kind, enabled, label, children: _ } = a;
        *uid == b.uid && *match_id == b.match_id && *name == b.name && *kind == b.kind && *enabled == b.enabled && *label == b.label
    }
    fn children(a: &PropGroup, b: &PropGroup, root: bool) -> bool {
        let mut x = a.children.iter().filter(|n| !audio_only(n, root));
        let mut y = b.children.iter().filter(|n| !audio_only(n, root));
        loop {
            match (x.next(), y.next()) {
                (None, None) => return true,
                (Some(Node::Group(g)), Some(Node::Group(h))) if root && g.match_id == "effects" && h.match_id == "effects" => {
                    if !(head(g, h) && children(g, h, false)) {
                        return false;
                    }
                }
                (Some(m), Some(n)) if m == n => {}
                _ => return false,
            }
        }
    }
    head(a, b) && children(a, b, true)
}

/// `g` (a layer's property tree) without its audio-only nodes ([`same_picture_props`]).
fn picture_props(g: &PropGroup) -> PropGroup {
    let mut g = g.clone();
    g.children.retain(|n| !audio_only(n, true));
    for n in &mut g.children {
        if let Node::Group(e) = n
            && e.match_id == "effects"
        {
            e.children.retain(|n| !audio_only(n, false));
        }
    }
    g
}

impl Layer {
    /// Draws the same pixels as `o`: equal but for the Audio, Lock and Shy switches and what only
    /// the audio mix reads (nothing draws its audio).
    pub fn same_pixels(&self, o: &Layer) -> bool {
        self.same_pixels_with(o, true, false)
    }

    /// [`Self::same_pixels`], with the markers counting only when `with_markers` (they are read by
    /// expressions alone, so a project without any can edit them freely) and the audio (the Audio
    /// switch, the Audio group, audio effects) only when `heard`: something draws it.
    pub fn same_pixels_with(&self, o: &Layer, with_markers: bool, heard: bool) -> bool {
        // Every field (no `..`: a new field must be considered here); the property tree last.
        let Layer {
            id,
            name,
            // Whether the name follows the text changes no pixels until the name changes.
            name_follows_text: _,
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
            && switches.picture(heard) == o.switches.picture(heard)
            && *blend_mode == o.blend_mode
            && *preserve_transparency == o.preserve_transparency
            && *track_matte == o.track_matte
            && *parent == o.parent
            && (!with_markers || *markers == o.markers)
            && *markers_locked == o.markers_locked
            && *auto_orient == o.auto_orient
            && *environment == o.environment
            && *environment_background == o.environment_background
            && (if heard { *props == o.props } else { same_picture_props(props, &o.props) })
    }

    /// The comp-time spans where `self` draws differently than `o` when the two differ only in
    /// the keyframes of Transform properties (a key's value, ease or interpolation moved in the
    /// Timeline or Graph Editor): from two keys before the first changed key to two after the
    /// last (auto-Bezier and continuous tangents reach the neighbouring segments), to the
    /// layer's in or out point past the first or last key (values hold there). `None` for any
    /// other difference, a static value, an expression on the layer (it can read the keyed
    /// property at other times), roving or muted keys, a time remap or a time effect (it reads other
    /// times): those reach the layer's whole span.
    pub fn keyframe_spans(&self, o: &Layer, with_markers: bool, heard: bool) -> Option<Vec<(Tick, Tick)>> {
        let transform = |l: &Layer| l.props.group("transform").cloned();
        let (t1, t0) = (transform(self)?, transform(o)?);
        let reads_time = |l: &Layer| {
            l.props.get("timeRemap").is_some()
                || l.props.group("effects").is_some_and(|e| {
                    e.children.iter().any(|n| matches!(n, Node::Group(f) if matches!(&f.kind, GroupKind::Effect { effect } if reads_other_frames(effect))))
                })
        };
        let has_expr = |l: &Layer| {
            let mut any = false;
            l.props.walk("", &mut |_, p| any |= p.has_expression());
            any
        };
        if reads_time(self) || reads_time(o) || has_expr(self) || has_expr(o) {
            return None;
        }
        // Everything but the Transform group draws the same.
        let rest = |l: &Layer| {
            let mut l = l.clone();
            l.props.children.retain(|n| !matches!(n, Node::Group(g) if g.match_id == "transform"));
            l
        };
        let head = |g: &PropGroup| PropGroup { children: vec![], ..g.clone() };
        if !rest(self).same_pixels_with(&rest(o), with_markers, heard) || head(&t1) != head(&t0) || t1.children.len() != t0.children.len() {
            return None;
        }
        let mut spans = Vec::new();
        for (n1, n0) in t1.children.iter().zip(&t0.children) {
            if n1 == n0 {
                continue;
            }
            let (Node::Prop(p1), Node::Prop(p0)) = (n1, n0) else { return None };
            let bare = |p: &Property| Property { keys: vec![], value: Value::Scalar(0.0), ..p.clone() };
            // (muted keys are skipped: the keys around an edit that play can be any distance away)
            if bare(p1) != bare(p0) || p1.keys.is_empty() || p0.keys.is_empty() || p1.keys.iter().chain(&p0.keys).any(|k| k.roving || k.muted) {
                return None;
            }
            // Keys only one side has as they are (added, removed, edited, moved).
            let changed: Vec<Tick> =
                p1.keys.iter().filter(|k| !p0.keys.contains(k)).chain(p0.keys.iter().filter(|k| !p1.keys.contains(k))).map(|k| k.time).collect();
            let (Some(first), Some(last)) = (changed.iter().min(), changed.iter().max()) else { continue };
            // Two keys before the first changed one and two after the last, on either side (the
            // first or last key when there are fewer: values hold beyond it). Past the first or
            // last key the change reaches the in or out point (`None`).
            let mut lo = Some(Tick(i64::MAX));
            let mut hi = Some(Tick(i64::MIN));
            for keys in [&p1.keys, &p0.keys] {
                let before = keys.iter().filter(|k| k.time < *first).count();
                let back = if before == 0 { None } else { keys.get(before.saturating_sub(2)) };
                lo = lo.zip(back).map(|(a, k)| a.min(k.time));
                let upto = keys.iter().filter(|k| k.time <= *last).count();
                let ahead = if upto >= keys.len() { None } else { keys.get(upto.saturating_add(1).min(keys.len().saturating_sub(1))) };
                hi = hi.zip(ahead).map(|(a, k)| a.max(k.time));
            }
            let (x, y) = (lo.map_or(self.in_point, |t| self.comp_time(t)), hi.map_or(self.out_point, |t| self.comp_time(t)));
            let (x, y) = (x.min(y).max(self.in_point), x.max(y).min(self.out_point));
            if x <= y {
                spans.push((x, y));
            }
        }
        Some(spans)
    }

    /// This layer with what doesn't change its pixels at its defaults (see
    /// [`Self::same_pixels_with`]), for keys that hash it; `None` when it already is.
    pub fn picture_form(&self, heard: bool) -> Option<Layer> {
        let switches = self.switches.picture(heard);
        let audio = !heard
            && (self.props.children.iter().any(|n| audio_only(n, true))
                || self
                    .props
                    .children
                    .iter()
                    .any(|n| matches!(n, Node::Group(e) if e.match_id == "effects" && e.children.iter().any(|n| audio_only(n, false)))));
        if switches == self.switches && !audio {
            return None;
        }
        let mut l = Layer { switches, ..self.clone() };
        if audio {
            l.props = picture_props(&self.props);
        }
        Some(l)
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

/// A stretch written as `null` (a NaN or infinite stretch: JSON holds neither, and auto-saves
/// write it so) reads as NaN, so the project still opens and loading brings it to 100 % with a
/// note ([`crate::limits::stretch`], #3247) instead of the whole file being refused. An infinite
/// stretch therefore also reopens at 100 %, not at ±9900 %.
fn stretch_or_nan<'de, D: serde::Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
    Ok(Option::<f64>::deserialize(d)?.unwrap_or(f64::NAN))
}

/// `t × mul / div` in ticks, within [`Tick::MIN`]..=[`Tick::MAX`] (the cast saturates, NaN is 0),
/// so a layer time of an extreme stretch can't overflow later tick arithmetic.
fn scaled(t: Tick, mul: f64, div: f64) -> Tick {
    Tick(((t.0 as f64 * mul / div) as i64).clamp(Tick::MIN.0, Tick::MAX.0))
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    pub id: LayerId,
    pub name: String,
    /// A text layer's name follows the first line of its Source Text until the user renames it
    /// (After Effects). `Some(true)`: it follows, `Some(false)`: the user named it. `None` (older
    /// projects, layers made before this was recorded): see [`Layer::name_follows_text()`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name_follows_text: Option<bool>,
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
    #[serde(deserialize_with = "stretch_or_nan")]
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

/// The name a text layer takes from its text: the first 40 characters of the first line. A line
/// ends at a line feed or at a bare carriage return (After Effects' paragraph break).
pub fn text_layer_name(text: &str) -> String {
    text.split(['\r', '\n']).next().unwrap_or("").chars().take(40).collect()
}

/// A text layer's default name when its text gives none: "Text", numbered "Text 2", "Text 3"…
fn is_default_text_name(name: &str) -> bool {
    name == "Text" || name.strip_prefix("Text ").is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
}

impl Layer {
    /// Whether this text layer's name follows its Source Text, whose first line gives the name
    /// `current` (see [`text_layer_name`]). Without a recorded state (an older project), a name
    /// that is empty, the text's own, or the default "Text"/"Text 2"… follows; any other name was
    /// given by the user.
    pub fn name_follows_text(&self, current: &str) -> bool {
        self.name_follows_text.unwrap_or_else(|| self.name.is_empty() || self.name == current || is_default_text_name(&self.name))
    }

    /// The layers this layer reads: its parent, track matte, layer parameters (Displacement Map,
    /// Audio Spectrum's Audio Layer…) and tracker targets.
    pub fn referenced_layers(&self) -> Vec<LayerId> {
        fn walk(g: &PropGroup, depth: usize, out: &mut Vec<LayerId>) {
            if depth > 64 {
                return;
            }
            if let GroupKind::Tracker { settings } = &g.kind
                && let Some(t) = settings.target
            {
                out.push(t);
            }
            for c in &g.children {
                match c {
                    Node::Prop(p) => {
                        for v in std::iter::once(&p.value).chain(p.keys.iter().map(|k| &k.value)) {
                            if let Value::Layer(Some(id)) = v {
                                out.push(LayerId(*id));
                            }
                        }
                    }
                    Node::Group(sub) => walk(sub, depth + 1, out),
                }
            }
        }
        let mut out: Vec<LayerId> = self.parent.iter().copied().chain(self.track_matte.iter().map(|m| m.layer)).collect();
        walk(&self.props, 0, &mut out);
        out
    }

    /// Layers this one picks in an effect or tracker (an Audio Spectrum's Audio Layer, a
    /// Displacement Map's map layer…), itself included: [`Self::referenced_layers`] without the
    /// parent and the track matte.
    pub fn picked_layers(&self) -> Vec<LayerId> {
        let skip = usize::from(self.parent.is_some()) + usize::from(self.track_matte.is_some());
        self.referenced_layers().into_iter().skip(skip).collect()
    }

    pub fn is_active_at(&self, t: Tick) -> bool {
        t >= self.in_point && t < self.out_point
    }

    /// The time stretch the layer plays at: [`Layer::stretch`] within
    /// [`limits::stretch`](crate::limits::stretch) (a damaged value that got past loading, 0 or
    /// NaN, never divides the layer's time by zero).
    pub fn effective_stretch(&self) -> f64 {
        crate::limits::stretch(self.stretch)
    }

    /// Layer time for a comp time (stretch-aware).
    pub fn layer_time(&self, comp_t: Tick) -> Tick {
        let d = comp_t - self.start_time;
        let s = self.effective_stretch();
        if (s - 100.0).abs() < 1e-9 { d } else { scaled(d, 100.0, s) }
    }

    /// Comp time for a layer time.
    pub fn comp_time(&self, layer_t: Tick) -> Tick {
        let s = self.effective_stretch();
        if (s - 100.0).abs() < 1e-9 { self.start_time + layer_t } else { self.start_time + scaled(layer_t, s, 100.0) }
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

    /// The labels popup property `p` of this layer offers: Text Path Options ▸ Path lists None and
    /// the layer's masks (value k = mask k), every other popup its own options. `None` when `p` is
    /// not a popup.
    pub fn popup_options<'a>(&'a self, p: &'a Property) -> Option<std::borrow::Cow<'a, [String]>> {
        if self.props.prop("text/pathOptions/path").is_some_and(|q| q.uid == p.uid) {
            let masks = self.masks().into_iter().flat_map(|m| m.groups().map(|g| g.name.clone()));
            return Some(std::iter::once("None".to_string()).chain(masks).collect::<Vec<_>>().into());
        }
        match &p.ui {
            ParamUi::Popup { options } => Some(options.as_slice().into()),
            _ => None,
        }
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
