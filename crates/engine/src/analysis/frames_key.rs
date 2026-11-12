//! What an analysis of a layer is made from, as one key shared by the Warp Stabilizer, the 3D
//! Camera Tracker and Roto Brush / Object Matte.
//!
//! The key hashes a text this module writes field by field. The source item (footage and its
//! settings, solid), the Time Remap property and the masks and effect groups (their properties,
//! keyframes and values) are destructured or matched in full, and each variant is written as a
//! fixed word: a new field or variant doesn't compile until it is decided here, written if it
//! changes the layer's frames, left out (with a reason) if it doesn't. Renaming a field or variant
//! doesn't change keys. Of the layer and of a precomp, only the fields named in `layer_frames` are
//! read (a precomp's contents are #3535). The
//! text leaves out observations (whether footage is available) and data that never changes pixels
//! (colour labels, UI hints, markers), so they never drop saved analyses (#3534, #3536).
//!
//! Keys saved by 0.22–0.24 hashed derived `Debug` text instead. [`legacy`] still recognises
//! them, and a matching saved key is kept rather than rewritten: frozen Roto Brush mattes and
//! segmentations hash the saved key itself.

use std::fmt::Write as _;

use effectcraft_color::ColorSpace;
use effectcraft_keyframe::{Ease, FeatherPoint, Gradient, Interp, Keyframe, ShapePath, Value};
use effectcraft_project::{
    AlphaMode, CineonSettings, CineonUnits, Expression, FeatherFalloff, FieldOrder, Footage, FootageKind, GroupKind, ItemId, ItemKind, Layer, LayerSource,
    LightKind, MaskMode, MaskMotionBlur, Node, PrimitiveKind, Project, PropGroup, Property, Solid, SourceLayer, Uid,
};
use effectcraft_time::{FrameRate, Tick};

pub(crate) mod legacy;

/// The version of the text, its first word: a later change to what it writes bumps it.
const VERSION: &str = "frames/1";

/// Property groups nest a few levels (effect ▸ group ▸ property); deeper trees are cut here.
const MAX_DEPTH: usize = 32;

fn fnv(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

fn hex(s: &str) -> String {
    format!("{:016x}", fnv(s))
}

/// Which signature a key is made from: today's, or one an earlier release saved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Era {
    Current,
    Legacy,
}

/// The signature of `layer`'s frames: its source (and the source item), In/Out, start time,
/// stretch, Time Remap and the comp frame rate. Warp Stabilizer and 3D Camera Tracker keys are
/// made from it.
pub(crate) fn signature(p: &Project, comp: ItemId, layer: &Layer) -> String {
    hex(&signature_text(p, comp, layer))
}

/// The text [`signature`] hashes.
pub(crate) fn signature_text(p: &Project, comp: ItemId, layer: &Layer) -> String {
    let mut t = Text::new();
    t.word(VERSION);
    t.layer_frames(p, comp, layer);
    t.0
}

/// A Roto Brush's (or Object Matte's) Input Key: the layer's frames, its masks and the effects
/// above the instance `uid`.
pub(crate) fn input_key(p: &Project, comp: ItemId, layer: &Layer, uid: Uid) -> String {
    input_key_of(signature(p, comp, layer), layer, uid)
}

/// An Input Key from a [`signature`].
pub(crate) fn input_key_of(signature: String, layer: &Layer, uid: Uid) -> String {
    let mut t = Text::new();
    t.word(VERSION);
    t.word(&signature);
    t.inputs_above(layer, uid);
    hex(&t.0)
}

/// Whether `saved`, a key saved with an analysis of `layer`, is for the layer's current frames.
/// `key` makes such a key from a signature of the given era. A key an earlier release saved for
/// the same frames matches too: keep it rather than rewrite it (analysing writes today's).
pub(crate) fn key_matches(p: &Project, comp: ItemId, layer: &Layer, saved: &str, key: impl Fn(String, Era) -> String) -> bool {
    key(signature(p, comp, layer), Era::Current) == saved || legacy::signatures(p, comp, layer).into_iter().any(|s| key(s, Era::Legacy) == saved)
}

/// The text a key is hashed from. Fields are `name=value;`; strings are quoted and escaped, so
/// no value can end its field early.
struct Text(String);

impl Text {
    fn new() -> Text {
        Text(String::new())
    }

    fn word(&mut self, w: &str) {
        self.0.push_str(w);
        self.0.push(';');
    }

    fn field(&mut self, name: &str, value: impl std::fmt::Display) {
        let _ = write!(self.0, "{name}={value};");
    }

    fn str(&mut self, name: &str, s: &str) {
        let _ = write!(self.0, "{name}={s:?};");
    }

    fn num(&mut self, name: &str, v: f64) {
        let _ = write!(self.0, "{name}={v:?};");
    }

    fn nums(&mut self, name: &str, v: &[f64]) {
        let _ = write!(self.0, "{name}={v:?};");
    }

    fn floats(&mut self, name: &str, v: &[f32]) {
        let _ = write!(self.0, "{name}={v:?};");
    }

    fn opt<T>(&mut self, name: &str, v: Option<&T>, write: impl FnOnce(&mut Text, &T)) {
        match v {
            Some(v) => {
                let _ = write!(self.0, "{name}{{");
                write(self, v);
                self.0.push_str("};");
            }
            None => self.word(&format!("{name}=none")),
        }
    }

    fn tick(&mut self, name: &str, t: Tick) {
        self.field(name, t.0);
    }

    fn rate(&mut self, name: &str, r: FrameRate) {
        let FrameRate { num, den } = r;
        let _ = write!(self.0, "{name}={num}/{den};");
    }

    /// What a layer's frames are made of, before its masks and effects.
    fn layer_frames(&mut self, p: &Project, comp: ItemId, layer: &Layer) {
        self.layer_source(&layer.source);
        if let Some(item) = layer.source.item() {
            match p.item(item).map(|i| &i.kind) {
                Some(ItemKind::Footage(f)) => self.footage(f),
                Some(ItemKind::Solid(s)) => self.solid(s),
                // A precomp's own contents aren't in the signature yet (#3535).
                Some(ItemKind::Comp(c)) => {
                    self.word("comp");
                    self.field("w", c.width);
                    self.field("h", c.height);
                    self.rate("rate", c.frame_rate);
                    self.tick("duration", c.duration);
                }
                Some(ItemKind::Folder) | None => self.word("item=none"),
            }
        }
        self.tick("in", layer.in_point);
        self.tick("out", layer.out_point);
        self.tick("start", layer.start_time);
        self.num("stretch", layer.stretch);
        self.opt("timeRemap", layer.props.get("timeRemap"), |t, pr| t.property(pr));
        self.opt("compRate", p.comp(comp).map(|c| &c.frame_rate), |t, r| t.rate("rate", *r));
    }

    fn layer_source(&mut self, s: &LayerSource) {
        match s {
            LayerSource::Footage { item } => self.field("footage", item.0),
            LayerSource::Comp { item } => self.field("comp", item.0),
            LayerSource::Solid { item } => self.field("solid", item.0),
            LayerSource::Model { item } => self.field("model", item.0),
            LayerSource::Text => self.word("text"),
            LayerSource::Shape => self.word("shape"),
            LayerSource::Null => self.word("null"),
            LayerSource::Camera => self.word("camera"),
            LayerSource::Light { kind } => self.field(
                "light",
                match kind {
                    LightKind::Parallel => "parallel",
                    LightKind::Spot => "spot",
                    LightKind::Point => "point",
                    LightKind::Ambient => "ambient",
                    LightKind::Environment => "environment",
                },
            ),
            LayerSource::Primitive { kind } => self.field(
                "primitive",
                match kind {
                    PrimitiveKind::Cube => "cube",
                    PrimitiveKind::Sphere => "sphere",
                    PrimitiveKind::Plane => "plane",
                    PrimitiveKind::Torus => "torus",
                    PrimitiveKind::Cone => "cone",
                    PrimitiveKind::Cylinder => "cylinder",
                },
            ),
        }
    }

    fn solid(&mut self, s: &Solid) {
        let Solid { color, width, height, pixel_aspect } = s;
        self.word("solid");
        self.floats("color", color);
        self.field("w", width);
        self.field("h", height);
        self.num("par", *pixel_aspect);
    }

    fn footage(&mut self, f: &Footage) {
        let Footage {
            path,
            kind,
            width,
            height,
            pixel_aspect,
            frame_rate,
            native_rate,
            duration,
            has_video,
            has_audio,
            alpha,
            premul_color,
            loop_count,
            codec,
            // Whether the file is there is an observation (`footage.check`), not an edit: an
            // offline file is the same footage, and its analyses stay valid (#3534).
            missing: _,
            sequence,
            color_profile,
            layer,
            fields,
            invert_alpha,
            linear_light,
            preserve_rgb,
            cineon,
            data,
            page,
            alphabetical,
            start_timecode,
            file_timecode,
            // Markers annotate the footage; they never change its frames.
            markers: _,
            placed_page,
        } = f;
        self.word("footage");
        self.str("path", path);
        self.field(
            "kind",
            match kind {
                FootageKind::Video => "video",
                FootageKind::Still => "still",
                FootageKind::Sequence => "sequence",
                FootageKind::Audio => "audio",
                FootageKind::Model => "model",
                FootageKind::Data => "data",
            },
        );
        self.field("w", width);
        self.field("h", height);
        self.num("par", *pixel_aspect);
        self.rate("rate", *frame_rate);
        self.opt("nativeRate", native_rate.as_ref(), |t, r| t.rate("rate", *r));
        self.tick("duration", *duration);
        self.field("video", has_video);
        self.field("audio", has_audio);
        self.field(
            "alpha",
            match alpha {
                AlphaMode::Straight => "straight",
                AlphaMode::Premultiplied => "premultiplied",
                AlphaMode::Ignore => "ignore",
            },
        );
        self.floats("matte", premul_color);
        self.field("loop", loop_count);
        self.str("codec", codec);
        self.field("frames", sequence.len());
        for s in sequence {
            self.str("file", s);
        }
        self.opt("profile", color_profile.as_ref(), |t, c| t.word(color_space(*c)));
        self.opt("layer", layer.as_ref(), |t, l| t.source_layer(l));
        self.field(
            "fields",
            match fields {
                FieldOrder::Off => "off",
                FieldOrder::UpperFirst => "upper",
                FieldOrder::LowerFirst => "lower",
            },
        );
        self.field("invertAlpha", invert_alpha);
        self.field("linearLight", linear_light);
        self.field("preserveRgb", preserve_rgb);
        self.cineon(cineon);
        self.opt("data", data.as_ref(), |t, d| t.str("text", d));
        self.field("page", page);
        self.field("alphabetical", alphabetical);
        self.opt("startTimecode", start_timecode.as_ref(), |t, v| t.field("frame", v));
        self.opt("fileTimecode", file_timecode.as_ref(), |t, v| t.field("frame", v));
        self.field("placedPage", placed_page);
    }

    fn source_layer(&mut self, l: &SourceLayer) {
        let SourceLayer { index, name, layer_size, embedded, placed } = l;
        self.field("index", index);
        // The layer is found by its index; the name only labels it.
        let _ = name;
        self.field("layerSize", layer_size);
        self.opt("embedded", embedded.as_ref(), |t, e| t.str("id", e));
        self.field("placed", placed);
    }

    fn cineon(&mut self, c: &CineonSettings) {
        let CineonSettings { log_conversion, converted_black, converted_white, black, white, gamma, rolloff, units } = c;
        self.word("cineon");
        self.field("log", log_conversion);
        self.floats("points", &[*converted_black, *converted_white, *black, *white, *gamma, *rolloff]);
        self.field(
            "units",
            match units {
                CineonUnits::TenBit => "10bit",
                CineonUnits::EightBit => "8bit",
                CineonUnits::Float => "float",
            },
        );
    }

    /// The layer's masks and the effects above the instance `uid`: what the effect's input is
    /// made of besides the layer's frames.
    fn inputs_above(&mut self, layer: &Layer, uid: Uid) {
        if let Some(m) = layer.masks() {
            self.word("masks");
            self.group(m, 0);
        }
        if let Some(fx) = layer.effects() {
            self.word("effects");
            for g in fx.groups().take_while(|g| g.uid != uid) {
                self.group(g, 0);
            }
        }
    }

    fn group(&mut self, g: &PropGroup, depth: usize) {
        // Expressions find effects and groups by name (`effect("Blur")`, `propertyGroup(1).name`).
        // The uid and colour label never change pixels.
        let PropGroup { uid: _, match_id, name, kind, enabled, label: _, children } = g;
        if depth >= MAX_DEPTH {
            self.word("tooDeep");
            return;
        }
        self.str("group", match_id);
        self.str("name", name);
        self.group_kind(kind);
        self.field("enabled", enabled);
        for c in children {
            match c {
                Node::Prop(p) => self.property(p),
                Node::Group(g) => self.group(g, depth + 1),
            }
        }
        self.word("end");
    }

    fn group_kind(&mut self, k: &GroupKind) {
        match k {
            GroupKind::Plain => self.word("plain"),
            GroupKind::Indexed => self.word("indexed"),
            // A mask's colour and lock only show it in the UI.
            GroupKind::Mask { mode, inverted, color: _, locked: _, motion_blur, feather_falloff, roto_bezier } => {
                self.field(
                    "mask",
                    match mode {
                        MaskMode::None => "none",
                        MaskMode::Add => "add",
                        MaskMode::Subtract => "subtract",
                        MaskMode::Intersect => "intersect",
                        MaskMode::Lighten => "lighten",
                        MaskMode::Darken => "darken",
                        MaskMode::Difference => "difference",
                    },
                );
                self.field("inverted", inverted);
                self.field(
                    "motionBlur",
                    match motion_blur {
                        MaskMotionBlur::SameAsLayer => "layer",
                        MaskMotionBlur::On => "on",
                        MaskMotionBlur::Off => "off",
                    },
                );
                self.field(
                    "falloff",
                    match feather_falloff {
                        FeatherFalloff::Smooth => "smooth",
                        FeatherFalloff::Linear => "linear",
                    },
                );
                self.field("rotoBezier", roto_bezier);
            }
            GroupKind::Effect { effect } => self.str("effect", effect),
            // A motion tracker's settings say how to track, not how to draw the layer.
            GroupKind::Tracker { settings: _ } => self.word("tracker"),
            GroupKind::Essential { overridden } => {
                self.word("essential");
                for u in overridden {
                    self.field("overridden", u);
                }
            }
        }
    }

    fn property(&mut self, pr: &Property) {
        // Expressions read the name (`thisProperty.name`) and seed `random`/`wiggle` with the
        // uid: the uid is written for a property with an expression. UI hints and the 2D/3D-only
        // and hold-only flags say where and how it can be edited, not what it evaluates to.
        let Property { uid, match_id, name, value, keys, expr, ui: _, spatial, hold_only: _, three_d_only: _, two_d_only: _, shown_dims: _, static_only: _ } =
            pr;
        self.str("prop", match_id);
        self.str("name", name);
        if expr.is_some() {
            self.field("uid", uid);
        }
        self.value(value);
        self.field("spatial", spatial);
        for k in keys {
            self.keyframe(k);
        }
        self.opt("expr", expr.as_ref(), |t, e| {
            let Expression { text, enabled } = e;
            t.str("text", text);
            t.field("enabled", enabled);
        });
    }

    fn keyframe(&mut self, k: &Keyframe) {
        // A key's colour label only shows it in the UI.
        let Keyframe {
            time,
            value,
            in_interp,
            out_interp,
            in_ease,
            out_ease,
            continuous,
            auto_bezier,
            spatial_in,
            spatial_out,
            spatial_auto,
            spatial_continuous,
            roving,
            label: _,
            muted,
        } = k;
        self.tick("key", *time);
        self.value(value);
        self.field("interp", format_args!("{}/{}", interp(*in_interp), interp(*out_interp)));
        for (side, eases) in [("inEase", in_ease), ("outEase", out_ease)] {
            for e in eases {
                let Ease { speed, influence } = e;
                self.nums(side, &[*speed, *influence]);
            }
        }
        self.field("continuous", continuous);
        self.field("autoBezier", auto_bezier);
        self.nums("spatialIn", spatial_in);
        self.nums("spatialOut", spatial_out);
        self.field("spatialAuto", spatial_auto);
        self.field("spatialContinuous", spatial_continuous);
        self.field("roving", roving);
        self.field("muted", muted);
    }

    fn value(&mut self, v: &Value) {
        match v {
            Value::Scalar(x) => self.num("scalar", *x),
            Value::Vec2(x) => self.nums("vec2", x),
            Value::Vec3(x) => self.nums("vec3", x),
            Value::Color(x) => self.nums("color", x),
            Value::Bool(b) => self.field("bool", b),
            Value::Enum(i) => self.field("enum", i),
            Value::Path(p) => self.shape_path(p),
            // A text document has dozens of fields and no place in what a key covers today (a
            // text layer's Source Text is not under its masks or effects): its project-file form.
            Value::Text(d) => self.str("text", &serde_json::to_string(d).unwrap_or_default()),
            Value::Layer(l) => self.opt("layer", l.as_ref(), |t, id| t.field("id", id)),
            Value::Gradient(g) => {
                let Gradient { colors, opacities } = g;
                self.word("gradient");
                for (at, c) in colors {
                    self.num("stop", *at);
                    self.floats("rgba", c);
                }
                for (at, o) in opacities {
                    self.num("stop", *at);
                    self.floats("opacity", &[*o]);
                }
            }
            Value::Str(s) => self.str("str", s),
        }
    }

    fn shape_path(&mut self, p: &ShapePath) {
        let ShapePath { vertices, in_tangents, out_tangents, closed, feather } = p;
        self.word("path");
        for (name, points) in [("v", vertices), ("in", in_tangents), ("out", out_tangents)] {
            for pt in points {
                self.nums(name, pt);
            }
        }
        self.field("closed", closed);
        for f in feather {
            let FeatherPoint { segment, t, radius, tension } = f;
            self.field("featherSegment", segment);
            self.nums("feather", &[*t, *radius, *tension]);
        }
    }
}

fn interp(i: Interp) -> &'static str {
    match i {
        Interp::Linear => "linear",
        Interp::Bezier => "bezier",
        Interp::Hold => "hold",
    }
}

fn color_space(c: ColorSpace) -> &'static str {
    match c {
        ColorSpace::Srgb => "srgb",
        ColorSpace::Rec709 => "rec709",
        ColorSpace::Rec2020 => "rec2020",
        ColorSpace::DisplayP3 => "p3",
        ColorSpace::AcesCg => "acescg",
        ColorSpace::Aces2065 => "aces2065",
        ColorSpace::Rec2100Pq => "rec2100pq",
        ColorSpace::Rec2100Hlg => "rec2100hlg",
    }
}
