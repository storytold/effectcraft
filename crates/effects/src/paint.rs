//! Paint: the Brush, Clone Stamp and Eraser tools' strokes.
//!
//! Paint is non-destructive, as in After Effects: strokes live in the layer's **Paint** effect
//! as property groups, so every stroke property animates and the layer source is never changed.
//!
//! ```text
//! Paint                      (effect `ec.paint.paint`; Paint on Transparent)
//!   Brush 1                  (match `brush` | `clone` | `eraser`)
//!     Path                   (the recorded stroke, effect space, animatable)
//!     Stroke Options         (Start, End, Color, Diameter, Angle, Hardness, Roundness, Spacing,
//!                             Channels, Opacity, Flow; clone: Clone Source, Clone Position,
//!                             Clone Time / Clone Time Shift)
//!     Transform: Brush 1     (Anchor Point, Position, Scale, Rotation)
//! ```
//!
//! Hidden stroke properties hold what the Paint panel decided when the stroke was drawn: blending
//! mode, the stroke's time span (Duration: Constant, Write On, Single Frame, Custom), pen
//! pressure per path vertex with the brush dynamics, and eraser mode.
//!
//! A stroke is rendered as a row of brush-tip dabs along its path between Start and End, spaced
//! by Spacing (a percentage of the diameter). A dab is an ellipse (Diameter × Roundness, rotated
//! by Angle) whose edge falls off with Hardness; Flow builds coverage dab by dab and Opacity caps
//! the stroke. Strokes composite in order onto the layer (or onto transparency): brushes paint a
//! colour, clone strokes copy pixels from a source layer (optionally at another time) offset by
//! Clone Position − the stroke's first point, and erasers remove the layer and paint ("Layer
//! Source & Paint"), only earlier paint ("Paint Only"), or only the previous stroke ("Last
//! Stroke Only").

use effectcraft_color::{BlendMode, blend_pixel, luminance};
use effectcraft_keyframe::{ShapePath, Value};
use effectcraft_project::build::Ids;
use effectcraft_project::{GroupKind, Node, ParamUi, PropGroup, Property};
use effectcraft_raster::{Image, Px};
use rayon::prelude::*;

use crate::{Buf, EffectCtx, EffectSpec, Params, p, popup, slider};

/// Effect id of the Paint effect.
pub const ID: &str = "ec.paint.paint";

/// Paint panel blending modes (AE's paint mode list).
pub const MODES: [BlendMode; 22] = [
    BlendMode::Normal,
    BlendMode::Darken,
    BlendMode::Multiply,
    BlendMode::ColorBurn,
    BlendMode::LinearBurn,
    BlendMode::Add,
    BlendMode::Lighten,
    BlendMode::Screen,
    BlendMode::ColorDodge,
    BlendMode::LinearDodge,
    BlendMode::Overlay,
    BlendMode::SoftLight,
    BlendMode::HardLight,
    BlendMode::LinearLight,
    BlendMode::VividLight,
    BlendMode::PinLight,
    BlendMode::Difference,
    BlendMode::Exclusion,
    BlendMode::Hue,
    BlendMode::Saturation,
    BlendMode::Color,
    BlendMode::Luminosity,
];

pub const CHANNELS: [&str; 3] = ["RGBA", "RGB", "Alpha"];
pub const DURATIONS: [&str; 4] = ["Constant", "Write On", "Single Frame", "Custom"];
pub const ERASE_MODES: [&str; 3] = ["Layer Source & Paint", "Paint Only", "Last Stroke Only"];

/// "Forever" for a stroke's out time (layer seconds).
pub const FOREVER: f64 = 1.0e9;

/// What a stroke does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum StrokeKind {
    #[default]
    Brush,
    Clone,
    Eraser,
}

impl StrokeKind {
    pub fn match_id(self) -> &'static str {
        match self {
            StrokeKind::Brush => "brush",
            StrokeKind::Clone => "clone",
            StrokeKind::Eraser => "eraser",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            StrokeKind::Brush => "Brush",
            StrokeKind::Clone => "Clone",
            StrokeKind::Eraser => "Eraser",
        }
    }
    pub fn from_name(s: &str) -> Option<StrokeKind> {
        match s.to_ascii_lowercase().as_str() {
            "brush" => Some(StrokeKind::Brush),
            "clone" | "clonestamp" => Some(StrokeKind::Clone),
            "eraser" | "erase" => Some(StrokeKind::Eraser),
            _ => None,
        }
    }
}

/// A brush tip (Brushes panel).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BrushTip {
    pub name: &'static str,
    pub diameter: f64,
    /// Degrees.
    pub angle: f64,
    /// Percent.
    pub roundness: f64,
    pub hardness: f64,
    pub spacing: f64,
}

/// The Brushes panel's preset tips: original Visualab Effects data (a size ladder of hard and soft
/// round tips plus a few flat calligraphic tips).
pub const BRUSH_PRESETS: &[BrushTip] = &[
    BrushTip { name: "Hard Round 1 px", diameter: 1.0, angle: 0.0, roundness: 100.0, hardness: 100.0, spacing: 25.0 },
    BrushTip { name: "Hard Round 3 px", diameter: 3.0, angle: 0.0, roundness: 100.0, hardness: 100.0, spacing: 25.0 },
    BrushTip { name: "Hard Round 5 px", diameter: 5.0, angle: 0.0, roundness: 100.0, hardness: 100.0, spacing: 25.0 },
    BrushTip { name: "Hard Round 9 px", diameter: 9.0, angle: 0.0, roundness: 100.0, hardness: 100.0, spacing: 25.0 },
    BrushTip { name: "Hard Round 13 px", diameter: 13.0, angle: 0.0, roundness: 100.0, hardness: 100.0, spacing: 25.0 },
    BrushTip { name: "Hard Round 19 px", diameter: 19.0, angle: 0.0, roundness: 100.0, hardness: 100.0, spacing: 25.0 },
    BrushTip { name: "Soft Round 5 px", diameter: 5.0, angle: 0.0, roundness: 100.0, hardness: 0.0, spacing: 25.0 },
    BrushTip { name: "Soft Round 9 px", diameter: 9.0, angle: 0.0, roundness: 100.0, hardness: 0.0, spacing: 25.0 },
    BrushTip { name: "Soft Round 13 px", diameter: 13.0, angle: 0.0, roundness: 100.0, hardness: 0.0, spacing: 25.0 },
    BrushTip { name: "Soft Round 17 px", diameter: 17.0, angle: 0.0, roundness: 100.0, hardness: 0.0, spacing: 25.0 },
    BrushTip { name: "Soft Round 21 px", diameter: 21.0, angle: 0.0, roundness: 100.0, hardness: 0.0, spacing: 25.0 },
    BrushTip { name: "Soft Round 27 px", diameter: 27.0, angle: 0.0, roundness: 100.0, hardness: 0.0, spacing: 25.0 },
    BrushTip { name: "Soft Round 35 px", diameter: 35.0, angle: 0.0, roundness: 100.0, hardness: 0.0, spacing: 25.0 },
    BrushTip { name: "Soft Round 45 px", diameter: 45.0, angle: 0.0, roundness: 100.0, hardness: 0.0, spacing: 25.0 },
    BrushTip { name: "Soft Round 65 px", diameter: 65.0, angle: 0.0, roundness: 100.0, hardness: 0.0, spacing: 25.0 },
    BrushTip { name: "Soft Round 100 px", diameter: 100.0, angle: 0.0, roundness: 100.0, hardness: 0.0, spacing: 25.0 },
    BrushTip { name: "Soft Round 200 px", diameter: 200.0, angle: 0.0, roundness: 100.0, hardness: 0.0, spacing: 25.0 },
    BrushTip { name: "Soft Round 300 px", diameter: 300.0, angle: 0.0, roundness: 100.0, hardness: 0.0, spacing: 25.0 },
    BrushTip { name: "Medium Round 24 px", diameter: 24.0, angle: 0.0, roundness: 100.0, hardness: 50.0, spacing: 20.0 },
    BrushTip { name: "Flat 20 px 45°", diameter: 20.0, angle: 45.0, roundness: 25.0, hardness: 90.0, spacing: 10.0 },
    BrushTip { name: "Flat 40 px -30°", diameter: 40.0, angle: -30.0, roundness: 15.0, hardness: 80.0, spacing: 8.0 },
    BrushTip { name: "Chisel 12 px", diameter: 12.0, angle: 90.0, roundness: 35.0, hardness: 100.0, spacing: 12.0 },
];

pub fn specs() -> Vec<EffectSpec> {
    vec![EffectSpec {
        id: ID,
        name: "Paint",
        category: "Paint",
        params: vec![p("on_transparent", "Paint on Transparent", Value::Bool(false), ParamUi::Checkbox)],
        render,
        gpu: false,
        float: true,
    }]
}

/// Everything needed to build one stroke group (see [`stroke_group`]). Defaults are a white
/// 10 px hard round brush.
#[derive(Clone, Debug, PartialEq)]
pub struct StrokeSpec {
    pub kind: StrokeKind,
    /// Effect-space points.
    pub points: Vec<[f64; 2]>,
    /// Pen pressure per point (0–1); empty = full pressure.
    pub pressure: Vec<f64>,
    pub color: [f64; 4],
    pub diameter: f64,
    pub angle: f64,
    pub hardness: f64,
    pub roundness: f64,
    pub spacing: f64,
    /// Index into [`CHANNELS`].
    pub channels: u32,
    pub opacity: f64,
    pub flow: f64,
    /// Index into [`MODES`].
    pub mode: u32,
    /// Brush dynamics: pressure drives size (down to `min_size` %), opacity, flow.
    pub size_pressure: bool,
    pub min_size: f64,
    pub opacity_pressure: bool,
    pub flow_pressure: bool,
    /// Index into [`ERASE_MODES`] (erasers).
    pub erase_mode: u32,
    /// Stroke the "Last Stroke Only" eraser applies to (group uid).
    pub target: Option<u64>,
    /// Clone source layer (None = this layer).
    pub clone_source: Option<u64>,
    /// Layer-space point copied to the stroke's first point.
    pub clone_position: [f64; 2],
    /// Seconds added to the current time when sampling the source.
    pub clone_time_shift: f64,
    /// Lock Source Time: always sample the source at `clone_time` (layer seconds).
    pub lock_time: bool,
    pub clone_time: f64,
    /// Visible span in layer seconds `[in, out)`.
    pub in_time: f64,
    pub out_time: f64,
    /// Index into [`DURATIONS`] (informational: the span and End keys encode it).
    pub duration: u32,
}

impl Default for StrokeSpec {
    fn default() -> Self {
        StrokeSpec {
            kind: StrokeKind::Brush,
            points: vec![],
            pressure: vec![],
            color: [1.0, 1.0, 1.0, 1.0],
            diameter: 10.0,
            angle: 0.0,
            hardness: 100.0,
            roundness: 100.0,
            spacing: 25.0,
            channels: 0,
            opacity: 100.0,
            flow: 100.0,
            mode: 0,
            size_pressure: true,
            min_size: 1.0,
            opacity_pressure: false,
            flow_pressure: false,
            erase_mode: 0,
            target: None,
            clone_source: None,
            clone_position: [0.0; 2],
            clone_time_shift: 0.0,
            lock_time: false,
            clone_time: 0.0,
            in_time: 0.0,
            out_time: FOREVER,
            duration: 0,
        }
    }
}

fn pct() -> ParamUi {
    slider(0.0, 100.0, 0.0, 100.0, 1)
}

fn hidden(ids: &mut Ids, m: &str, v: Value) -> Property {
    let mut p = ids.prop(m, m, v).with_ui(ParamUi::Hidden);
    p.static_only = true;
    p
}

/// Build a stroke group (`Brush 3`, `Clone 1`, `Eraser 2`…) for the Paint effect.
pub fn stroke_group(ids: &mut Ids, name: &str, s: &StrokeSpec) -> PropGroup {
    let mut g = ids.group(s.kind.match_id(), name);
    g.kind = GroupKind::Indexed;
    let path = ShapePath::polygon(&s.points, false);
    g.children.push(ids.prop("path", "Path", Value::Path(path)).with_ui(ParamUi::Path).into());
    g.children.push(hidden(ids, "mode", Value::Enum(s.mode)).into());
    g.children.push(hidden(ids, "in", Value::Scalar(s.in_time)).into());
    g.children.push(hidden(ids, "out", Value::Scalar(s.out_time)).into());
    g.children.push(hidden(ids, "duration", Value::Enum(s.duration)).into());
    let pr: Vec<String> = s.pressure.iter().map(|p| format!("{:.3}", p.clamp(0.0, 1.0))).collect();
    g.children.push(hidden(ids, "pressure", Value::Str(pr.join(","))).into());
    if s.kind == StrokeKind::Eraser {
        g.children.push(hidden(ids, "erase", Value::Enum(s.erase_mode)).into());
        g.children.push(hidden(ids, "target", Value::Scalar(s.target.unwrap_or(0) as f64)).into());
    }

    let mut o = ids.group("stroke_options", "Stroke Options");
    o.children.push(ids.prop("start", "Start", Value::Scalar(0.0)).with_ui(pct()).into());
    o.children.push(ids.prop("end", "End", Value::Scalar(100.0)).with_ui(pct()).into());
    if s.kind == StrokeKind::Brush {
        o.children.push(ids.prop("color", "Color", Value::Color(s.color)).with_ui(ParamUi::Color).into());
    }
    o.children.push(ids.prop("diameter", "Diameter", Value::Scalar(s.diameter)).with_ui(slider(0.0, 2500.0, 1.0, 200.0, 1)).into());
    o.children.push(ids.prop("angle", "Angle", Value::Scalar(s.angle)).with_ui(ParamUi::Angle).into());
    o.children.push(ids.prop("hardness", "Hardness", Value::Scalar(s.hardness)).with_ui(pct()).into());
    o.children.push(ids.prop("roundness", "Roundness", Value::Scalar(s.roundness)).with_ui(pct()).into());
    o.children.push(ids.prop("spacing", "Spacing", Value::Scalar(s.spacing)).with_ui(slider(1.0, 1000.0, 1.0, 200.0, 1)).into());
    if s.kind != StrokeKind::Eraser {
        let mut c = ids.prop("channels", "Channels", Value::Enum(s.channels)).with_ui(popup(&CHANNELS));
        c.hold_only = true;
        o.children.push(c.into());
    }
    o.children.push(ids.prop("opacity", "Opacity", Value::Scalar(s.opacity)).with_ui(pct()).into());
    o.children.push(ids.prop("flow", "Flow", Value::Scalar(s.flow)).with_ui(pct()).into());
    if s.kind == StrokeKind::Clone {
        let mut src = ids.prop("clone_source", "Clone Source", Value::Layer(s.clone_source)).with_ui(ParamUi::Layer);
        src.hold_only = true;
        o.children.push(src.into());
        o.children.push(ids.prop("clone_position", "Clone Position", Value::Vec2(s.clone_position)).with_ui(ParamUi::Point).spatial().into());
        if s.lock_time {
            o.children.push(ids.prop("clone_time", "Clone Time", Value::Scalar(s.clone_time)).into());
        } else {
            o.children.push(ids.prop("clone_time_shift", "Clone Time Shift", Value::Scalar(s.clone_time_shift)).into());
        }
        o.children.push(hidden(ids, "lock_time", Value::Bool(s.lock_time)).into());
    }
    o.children.push(hidden(ids, "size_pressure", Value::Bool(s.size_pressure)).into());
    o.children.push(hidden(ids, "min_size", Value::Scalar(s.min_size)).into());
    o.children.push(hidden(ids, "opacity_pressure", Value::Bool(s.opacity_pressure)).into());
    o.children.push(hidden(ids, "flow_pressure", Value::Bool(s.flow_pressure)).into());
    g.children.push(o.into());

    let first = s.points.first().copied().unwrap_or([0.0; 2]);
    let mut t = ids.group("transform", &format!("Transform: {name}"));
    t.children.push(ids.prop("anchor", "Anchor Point", Value::Vec2(first)).with_ui(ParamUi::Point).spatial().into());
    t.children.push(ids.prop("position", "Position", Value::Vec2(first)).with_ui(ParamUi::Point).spatial().into());
    t.children.push(ids.prop("scale", "Scale", Value::Vec2([100.0, 100.0])).with_ui(ParamUi::Percent).into());
    t.children.push(ids.prop("rotation", "Rotation", Value::Scalar(0.0)).with_ui(ParamUi::Angle).into());
    g.children.push(t.into());
    g
}

/// The stroke groups of a Paint effect instance.
pub fn strokes(fx: &PropGroup) -> impl Iterator<Item = &PropGroup> {
    fx.groups().filter(|g| matches!(g.match_id.as_str(), "brush" | "clone" | "eraser"))
}

fn static_f(g: &PropGroup, m: &str, d: f64) -> f64 {
    g.get(m).map(|p| p.value.as_f64()).unwrap_or(d)
}

/// What the layer cache must know about a Paint effect at layer time `t` (seconds): which
/// strokes are visible (their spans are not property values), and whether the result reads
/// other times or layers (clone strokes with a source layer, a time shift or a locked time).
pub fn cache_key(fx: &PropGroup, t: f64) -> (Vec<bool>, bool) {
    let mut vis = vec![];
    let mut reads_other = false;
    for s in strokes(fx) {
        vis.push(s.enabled && visible(static_f(s, "in", 0.0), static_f(s, "out", FOREVER), t));
        if s.match_id == "clone"
            && let Some(o) = s.sub("stroke_options")
        {
            let other = o.get("clone_source").is_some_and(|p| p.value.as_layer().is_some() || !p.keys.is_empty());
            let shift = o.get("clone_time_shift").is_some_and(|p| p.value.as_f64() != 0.0 || !p.keys.is_empty());
            let lock = o.get("lock_time").is_some_and(|p| p.value.as_bool());
            reads_other |= other || shift || lock;
        }
    }
    (vis, reads_other)
}

fn visible(tin: f64, tout: f64, t: f64) -> bool {
    t >= tin - 1e-6 && t < tout - 1e-6
}

// ------------------------------------------------------------------------------------------
// Evaluated strokes.

/// A stroke evaluated at the render time.
#[derive(Clone, Debug)]
pub struct Stroke {
    pub kind: StrokeKind,
    pub uid: u64,
    pub path: ShapePath,
    pub pressure: Vec<f64>,
    pub mode: BlendMode,
    pub in_time: f64,
    pub out_time: f64,
    pub start: f64,
    pub end: f64,
    pub color: [f32; 4],
    pub diameter: f64,
    pub angle: f64,
    pub hardness: f64,
    pub roundness: f64,
    pub spacing: f64,
    pub channels: u32,
    pub opacity: f64,
    pub flow: f64,
    pub size_pressure: bool,
    pub min_size: f64,
    pub opacity_pressure: bool,
    pub flow_pressure: bool,
    pub erase_mode: u32,
    pub target: u64,
    pub clone_source: Option<u64>,
    pub clone_position: [f64; 2],
    pub clone_time_shift: f64,
    pub lock_time: bool,
    pub clone_time: f64,
    pub anchor: [f64; 2],
    pub position: [f64; 2],
    pub scale: [f64; 2],
    pub rotation: f64,
}

fn fd(p: &Params, k: &str, d: f64) -> f64 {
    p.get(k).map(Value::as_f64).unwrap_or(d)
}

/// Parse the strokes out of flattened Paint parameters (enabled ones, in order).
pub fn parse(params: &Params) -> Vec<Stroke> {
    let mut out = vec![];
    for pre in params.groups("") {
        let kind = match params.s(&format!("{pre}@match")) {
            "brush" => StrokeKind::Brush,
            "clone" => StrokeKind::Clone,
            "eraser" => StrokeKind::Eraser,
            _ => continue,
        };
        if !params.get(&format!("{pre}@enabled")).is_none_or(Value::as_bool) {
            continue;
        }
        let k = |m: &str| format!("{pre}{m}");
        let path = params.get(&k("path")).and_then(Value::as_path).cloned().unwrap_or_default();
        let pressure = params.s(&k("pressure")).split(',').filter_map(|x| x.trim().parse::<f64>().ok()).collect();
        let o = params.group(&pre, "stroke_options").unwrap_or_else(|| format!("{pre}#0/"));
        let ok = |m: &str| format!("{o}{m}");
        let tr = params.group(&pre, "transform").unwrap_or_else(|| format!("{pre}#0/"));
        let tk = |m: &str| format!("{tr}{m}");
        let first = path.vertices.first().copied().unwrap_or([0.0; 2]);
        let mode = MODES.get(params.get(&k("mode")).map(Value::as_enum).unwrap_or(0) as usize).copied().unwrap_or(BlendMode::Normal);
        let sc = params.get(&tk("scale")).map(Value::as_vec2).unwrap_or([100.0, 100.0]);
        out.push(Stroke {
            kind,
            uid: fd(params, &k("@uid"), 0.0) as u64,
            pressure,
            mode,
            in_time: fd(params, &k("in"), 0.0),
            out_time: fd(params, &k("out"), FOREVER),
            start: fd(params, &ok("start"), 0.0),
            end: fd(params, &ok("end"), 100.0),
            color: params.get(&ok("color")).map(Value::as_color).unwrap_or([1.0; 4]),
            diameter: fd(params, &ok("diameter"), 10.0),
            angle: fd(params, &ok("angle"), 0.0),
            hardness: fd(params, &ok("hardness"), 100.0),
            roundness: fd(params, &ok("roundness"), 100.0),
            spacing: fd(params, &ok("spacing"), 25.0),
            channels: params.get(&ok("channels")).map(Value::as_enum).unwrap_or(0),
            opacity: fd(params, &ok("opacity"), 100.0),
            flow: fd(params, &ok("flow"), 100.0),
            size_pressure: params.get(&ok("size_pressure")).is_some_and(Value::as_bool),
            min_size: fd(params, &ok("min_size"), 1.0),
            opacity_pressure: params.get(&ok("opacity_pressure")).is_some_and(Value::as_bool),
            flow_pressure: params.get(&ok("flow_pressure")).is_some_and(Value::as_bool),
            erase_mode: params.get(&k("erase")).map(Value::as_enum).unwrap_or(0),
            target: fd(params, &k("target"), 0.0) as u64,
            clone_source: params.get(&ok("clone_source")).and_then(Value::as_layer),
            clone_position: params.get(&ok("clone_position")).map(Value::as_vec2).unwrap_or(first),
            clone_time_shift: fd(params, &ok("clone_time_shift"), 0.0),
            lock_time: params.get(&ok("lock_time")).is_some_and(Value::as_bool),
            clone_time: fd(params, &ok("clone_time"), 0.0),
            anchor: params.get(&tk("anchor")).map(Value::as_vec2).unwrap_or(first),
            position: params.get(&tk("position")).map(Value::as_vec2).unwrap_or(first),
            scale: [sc[0] / 100.0, sc[1] / 100.0],
            rotation: fd(params, &tk("rotation"), 0.0),
            path,
        });
    }
    out
}

impl Stroke {
    /// Stroke transform: layer point of a path point.
    pub fn xf(&self, p: [f64; 2]) -> [f64; 2] {
        let (s, c) = self.rotation.to_radians().sin_cos();
        let d = [(p[0] - self.anchor[0]) * self.scale[0], (p[1] - self.anchor[1]) * self.scale[1]];
        [self.position[0] + d[0] * c - d[1] * s, self.position[1] + d[0] * s + d[1] * c]
    }

    /// The flattened, transformed path in layer space with the pressure at each point.
    pub fn polyline(&self) -> Vec<([f64; 2], f64)> {
        let p = &self.path;
        let n = p.vertices.len();
        let pr = |i: usize| if self.pressure.len() == n { self.pressure[i] } else { 1.0 };
        let mut out = vec![];
        if n == 0 {
            return out;
        }
        out.push((self.xf(p.vertices[0]), pr(0)));
        let segs = if p.closed { n } else { n - 1 };
        for i in 0..segs {
            let j = (i + 1) % n;
            let a = p.vertices[i];
            let b = p.vertices[j];
            let o = p.out_tangents.get(i).copied().unwrap_or([0.0; 2]);
            let t = p.in_tangents.get(j).copied().unwrap_or([0.0; 2]);
            if o == [0.0; 2] && t == [0.0; 2] {
                out.push((self.xf(b), pr(j)));
                continue;
            }
            let c1 = [a[0] + o[0], a[1] + o[1]];
            let c2 = [b[0] + t[0], b[1] + t[1]];
            for k in 1..=16 {
                let u = k as f64 / 16.0;
                let v = 1.0 - u;
                let q = [
                    v * v * v * a[0] + 3.0 * v * v * u * c1[0] + 3.0 * v * u * u * c2[0] + u * u * u * b[0],
                    v * v * v * a[1] + 3.0 * v * v * u * c1[1] + 3.0 * v * u * u * c2[1] + u * u * u * b[1],
                ];
                out.push((self.xf(q), pr(i) + (pr(j) - pr(i)) * u));
            }
        }
        out
    }
}

/// One brush-tip imprint, in buffer pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dab {
    pub x: f64,
    pub y: f64,
    /// Major radius in pixels.
    pub r: f64,
    /// Minor / major (roundness).
    pub ratio: f64,
    pub cos: f64,
    pub sin: f64,
    /// 0–1.
    pub hardness: f64,
    /// Flow weight (0–1).
    pub w: f64,
}

impl Dab {
    /// Coverage of the pixel whose centre is at `(px, py)`.
    #[inline]
    pub fn coverage(&self, px: f64, py: f64) -> f64 {
        let dx = px - self.x;
        let dy = py - self.y;
        let r = self.r.max(1e-6);
        let rm = (r * self.ratio).max(1e-6);
        let u = (dx * self.cos + dy * self.sin) / r;
        let v = (-dx * self.sin + dy * self.cos) / rm;
        let dist = (u * u + v * v).sqrt();
        // Anti-aliased edge, measured in pixels along the smaller axis.
        let aa = (0.5 - (dist - 1.0) * rm.max(0.5)).clamp(0.0, 1.0);
        let h = self.hardness.clamp(0.0, 1.0);
        if h >= 0.999 || dist <= h {
            return aa;
        }
        let x = ((dist - h) / (1.0 - h)).clamp(0.0, 1.0);
        (1.0 - x * x * (3.0 - 2.0 * x)) * aa
    }
}

/// The dabs of a stroke between Start and End, in pixels of a buffer with `scale` and `offset`.
pub fn dabs(s: &Stroke, scale: f64, offset: [f64; 2]) -> Vec<Dab> {
    let pts = s.polyline();
    if pts.is_empty() {
        return vec![];
    }
    let xs = ((s.scale[0].abs() + s.scale[1].abs()) * 0.5).max(1e-6);
    let d = s.diameter.max(0.0) * xs;
    if d <= 0.0 {
        return vec![];
    }
    let mut cum = vec![0.0];
    for w in pts.windows(2) {
        let l = ((w[1].0[0] - w[0].0[0]).powi(2) + (w[1].0[1] - w[0].0[1]).powi(2)).sqrt();
        cum.push(cum.last().copied().unwrap_or(0.0) + l);
    }
    let total = *cum.last().unwrap_or(&0.0);
    let (a, b) = ((s.start / 100.0).clamp(0.0, 1.0), (s.end / 100.0).clamp(0.0, 1.0));
    if b <= a {
        return vec![];
    }
    let step = (d * s.spacing / 100.0).max(0.1);
    let (l0, l1) = (a * total, b * total);
    let (sin, cos) = s.angle.to_radians().sin_cos();
    let at = |l: f64| -> ([f64; 2], f64) {
        let i = cum.partition_point(|c| *c <= l).clamp(1, cum.len().max(2) - 1).min(pts.len() - 1);
        if pts.len() == 1 {
            return pts[0];
        }
        let (c0, c1) = (cum[i - 1], cum[i]);
        let u = if c1 > c0 { ((l - c0) / (c1 - c0)).clamp(0.0, 1.0) } else { 0.0 };
        let (p0, q0) = pts[i - 1];
        let (p1, q1) = pts[i];
        ([p0[0] + (p1[0] - p0[0]) * u, p0[1] + (p1[1] - p0[1]) * u], q0 + (q1 - q0) * u)
    };
    let mut out = vec![];
    let mut l = l0;
    loop {
        let (q, pr) = at(l);
        let pr = pr.clamp(0.0, 1.0);
        let size = if s.size_pressure { (s.min_size / 100.0 + (1.0 - s.min_size / 100.0) * pr).clamp(0.0, 1.0) } else { 1.0 };
        let mut w = (s.flow / 100.0).clamp(0.0, 1.0);
        if s.flow_pressure {
            w *= pr;
        }
        if s.opacity_pressure {
            w *= pr;
        }
        out.push(Dab {
            x: q[0] * scale + offset[0],
            y: q[1] * scale + offset[1],
            r: d * 0.5 * size * scale,
            ratio: (s.roundness / 100.0).clamp(0.01, 1.0),
            cos,
            sin,
            hardness: s.hardness / 100.0,
            w,
        });
        if total <= 0.0 {
            break;
        }
        l += step;
        if l > l1 + 1e-9 {
            break;
        }
    }
    out
}

/// A coverage map limited to a pixel rectangle.
#[derive(Clone, Debug, Default)]
pub struct Coverage {
    pub x0: i64,
    pub y0: i64,
    pub w: usize,
    pub h: usize,
    pub data: Vec<f32>,
}

impl Coverage {
    #[inline]
    pub fn at(&self, x: i64, y: i64) -> f32 {
        let (u, v) = (x - self.x0, y - self.y0);
        if u < 0 || v < 0 || u >= self.w as i64 || v >= self.h as i64 { 0.0 } else { self.data[v as usize * self.w + u as usize] }
    }
    /// Multiply by `1 - other` where they overlap.
    fn erase_by(&mut self, other: &Coverage) {
        let w = self.w;
        let (x0, y0) = (self.x0, self.y0);
        self.data.par_chunks_mut(w.max(1)).enumerate().for_each(|(j, row)| {
            for (i, c) in row.iter_mut().enumerate() {
                let e = other.at(x0 + i as i64, y0 + j as i64);
                *c *= 1.0 - e;
            }
        });
    }
}

/// Rasterize a stroke's coverage (Flow builds up dab by dab; Opacity caps the stroke).
pub fn coverage(s: &Stroke, dabs: &[Dab], width: u32, height: u32) -> Coverage {
    if dabs.is_empty() {
        return Coverage::default();
    }
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for d in dabs {
        x0 = x0.min(d.x - d.r - 1.0);
        y0 = y0.min(d.y - d.r - 1.0);
        x1 = x1.max(d.x + d.r + 1.0);
        y1 = y1.max(d.y + d.r + 1.0);
    }
    let x0 = (x0.floor() as i64).max(0);
    let y0 = (y0.floor() as i64).max(0);
    let x1 = (x1.ceil() as i64).min(width as i64);
    let y1 = (y1.ceil() as i64).min(height as i64);
    if x1 <= x0 || y1 <= y0 {
        return Coverage::default();
    }
    let (w, h) = ((x1 - x0) as usize, (y1 - y0) as usize);
    let opacity = (s.opacity / 100.0).clamp(0.0, 1.0) as f32;
    let mut data = vec![0.0f32; w * h];
    data.par_chunks_mut(w).enumerate().for_each(|(j, row)| {
        let py = (y0 + j as i64) as f64 + 0.5;
        let mut keep = vec![1.0f64; w];
        for d in dabs {
            if (py - d.y).abs() > d.r + 1.0 {
                continue;
            }
            let xa = ((d.x - d.r - 1.0).floor() as i64).max(x0);
            let xb = ((d.x + d.r + 1.0).ceil() as i64).min(x1);
            for x in xa..xb {
                let c = d.coverage(x as f64 + 0.5, py);
                if c > 0.0 {
                    keep[(x - x0) as usize] *= 1.0 - d.w * c;
                }
            }
        }
        for (o, k) in row.iter_mut().zip(keep) {
            *o = opacity * (1.0 - k as f32);
        }
    });
    Coverage { x0, y0, w, h, data }
}

/// Composite straight colour `col` (alpha `a`) with coverage `k` into `dst` honouring the
/// stroke's mode and channels.
#[inline]
fn paint_px(dst: &mut Px, col: [f32; 3], a: f32, k: f32, mode: BlendMode, channels: u32) {
    let k = (k * a).clamp(0.0, 1.0);
    if k <= 0.0 {
        return;
    }
    match channels {
        // RGB: colour only, alpha unchanged.
        1 => {
            let da = dst[3];
            if da <= 0.0 {
                return;
            }
            let dc = [dst[0] / da, dst[1] / da, dst[2] / da];
            let mixed = blend_pixel(mode, [dc[0], dc[1], dc[2], 1.0], [col[0], col[1], col[2], 1.0], 0.0);
            for i in 0..3 {
                dst[i] = (dc[i] + (mixed[i] - dc[i]) * k) * da;
            }
        }
        // Alpha: the colour's luminance becomes the layer's alpha.
        2 => {
            let da = dst[3];
            let lum = luminance(col[0], col[1], col[2]);
            let na = da + (lum - da) * k;
            let f = if da > 0.0 { na / da } else { 0.0 };
            for c in dst.iter_mut().take(3) {
                *c *= f;
            }
            dst[3] = na;
        }
        _ => {
            *dst = blend_pixel(mode, *dst, [col[0] * k, col[1] * k, col[2] * k, k], 0.0);
        }
    }
}

/// Where a clone stroke samples: its source buffer.
fn clone_source(ctx: &EffectCtx, s: &Stroke, own: &Buf) -> Option<Buf> {
    let t = if s.lock_time { s.clone_time } else { ctx.time + s.clone_time_shift };
    let same_time = !s.lock_time && s.clone_time_shift == 0.0;
    match s.clone_source {
        None => {
            if same_time {
                Some(own.clone())
            } else {
                ctx.env.host.and_then(|h| h.self_at(t, 0)).or_else(|| Some(own.clone()))
            }
        }
        Some(id) => {
            let host = ctx.env.host?;
            if same_time {
                host.layer(id, false).map(|l| l.buf)
            } else {
                let comp_t = ctx.env.comp_time + (t - ctx.time);
                host.layer_at(id, comp_t, false).map(|l| l.buf)
            }
        }
    }
}

fn render(ctx: &EffectCtx, buf: Buf) -> Buf {
    let all = parse(ctx.params);
    let strokes: Vec<&Stroke> = all.iter().filter(|s| visible(s.in_time, s.out_time, ctx.time)).collect();
    let on_transparent = ctx.params.b("on_transparent");
    if strokes.is_empty() && !on_transparent {
        return buf;
    }
    paint_strokes(ctx, buf, &strokes, on_transparent)
}

/// Composite `strokes` (in order) onto `buf`.
pub fn paint_strokes(ctx: &EffectCtx, buf: Buf, strokes: &[&Stroke], on_transparent: bool) -> Buf {
    let (w, h) = (buf.img.width, buf.img.height);
    let mut covs: Vec<Coverage> = strokes.par_iter().map(|s| coverage(s, &dabs(s, buf.scale, buf.offset), w, h)).collect();
    // Paint Only erasers remove earlier paint; Last Stroke Only erasers remove the previous
    // (or targeted) brush / clone stroke.
    for j in 0..strokes.len() {
        let s = strokes[j];
        if s.kind != StrokeKind::Eraser || s.erase_mode == 0 {
            continue;
        }
        let e = covs[j].clone();
        if s.erase_mode == 1 {
            for i in 0..j {
                if strokes[i].kind != StrokeKind::Eraser {
                    covs[i].erase_by(&e);
                }
            }
        } else {
            let target = (0..j).rev().find(|i| if s.target != 0 { strokes[*i].uid == s.target } else { strokes[*i].kind != StrokeKind::Eraser });
            if let Some(i) = target {
                covs[i].erase_by(&e);
            }
        }
    }
    let mut out = if on_transparent { Image::new(w, h) } else { buf.img.clone() };
    for (s, cov) in strokes.iter().zip(&covs) {
        if cov.w == 0 {
            continue;
        }
        let x0 = cov.x0;
        let y0 = cov.y0;
        let cw = cov.w;
        match s.kind {
            StrokeKind::Eraser => {
                if s.erase_mode != 0 {
                    continue;
                }
                out.rows_mut().for_each(|(y, row)| {
                    let j = y as i64 - y0;
                    if j < 0 || j >= cov.h as i64 {
                        return;
                    }
                    for i in 0..cw {
                        let k = 1.0 - cov.data[j as usize * cw + i];
                        let px = &mut row[(x0 + i as i64) as usize];
                        for c in px.iter_mut() {
                            *c *= k;
                        }
                    }
                });
            }
            StrokeKind::Brush => {
                let col = [s.color[0], s.color[1], s.color[2]];
                out.rows_mut().for_each(|(y, row)| {
                    let j = y as i64 - y0;
                    if j < 0 || j >= cov.h as i64 {
                        return;
                    }
                    for i in 0..cw {
                        let k = cov.data[j as usize * cw + i];
                        paint_px(&mut row[(x0 + i as i64) as usize], col, 1.0, k, s.mode, s.channels);
                    }
                });
            }
            StrokeKind::Clone => {
                let Some(src) = clone_source(ctx, s, &buf) else { continue };
                let first = s.polyline().first().map(|p| p.0).unwrap_or([0.0; 2]);
                let off = [s.clone_position[0] - first[0], s.clone_position[1] - first[1]];
                let (bs, bo) = (buf.scale.max(1e-9), buf.offset);
                out.rows_mut().for_each(|(y, row)| {
                    let j = y as i64 - y0;
                    if j < 0 || j >= cov.h as i64 {
                        return;
                    }
                    for i in 0..cw {
                        let k = cov.data[j as usize * cw + i];
                        if k <= 0.0 {
                            continue;
                        }
                        let x = x0 + i as i64;
                        let q = [(x as f64 + 0.5 - bo[0]) / bs + off[0], (y as f64 + 0.5 - bo[1]) / bs + off[1]];
                        let sp = src.img.sample_bilinear(q[0] * src.scale + src.offset[0], q[1] * src.scale + src.offset[1]);
                        let a = sp[3];
                        if a <= 0.0 {
                            continue;
                        }
                        paint_px(&mut row[x as usize], [sp[0] / a, sp[1] / a, sp[2] / a], a, k, s.mode, s.channels);
                    }
                });
            }
        }
    }
    Buf { img: out, ..buf }
}

/// Is `g` a Paint effect instance?
pub fn is_paint(g: &PropGroup) -> bool {
    matches!(&g.kind, GroupKind::Effect { effect } if effect == ID)
}

/// Next stroke name for a kind (`Brush 4`).
pub fn next_name(fx: &PropGroup, kind: StrokeKind) -> String {
    let n = fx.children.iter().filter(|c| matches!(c, Node::Group(g) if g.match_id == kind.match_id())).count();
    format!("{} {}", kind.label(), n + 1)
}

#[cfg(test)]
mod tests;
