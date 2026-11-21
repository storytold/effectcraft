//! Camera and light layers (`docs/aep-format.md` 3.12; cases 251-256, 342-346).
//!
//! Both are written from a layer block of our own cases: the default two-node camera 256 and the
//! spot (254, its Intensity record removed) and point (252) lights. The layer header keeps the
//! case's type (`ldta@128` 2 camera, 1 light), source id (0 / `0xffffffff`), 3D bit and light type
//! (`ldta@136` 1 spot, 2 point); the exporter sets the auto-orient bit (`ldta@38` 0x40: a two-node
//! camera and a spot light orient towards their point of interest, 251 vs 256). The point of
//! interest is the Anchor Point in pixels and the Position an explicit value (253: 76 / 77 = 0),
//! both through the Transform writer. Options records are cloned from the one-change cases; keyed
//! options take their keyed form ([`keyed`], lab camlight-keys).

use crate::Chunk;
use effectcraft_keyframe::{Interp, Value};
use effectcraft_project::{Layer, LightKind};

use crate::export::keys::{replace_cdat, spatial_record, timed_keys};
use crate::export::plan::{CompPlan, EaseSlot, KeyPlan};
use crate::export::report::{Collector, LossCategory};
use crate::export::synth::{GROUP_END, is_tdmn, key_list, tdmn};
use crate::export::tree::{R, child_data, kids_mut, list_mut, put_f64, put_u8, split_group, tpl};

/// A camera's options in the file's units (pixels, percent, a switch).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CameraPlan {
    pub two_node: bool,
    pub zoom: f64,
    pub dof: bool,
    pub focus: f64,
    pub aperture: f64,
    pub blur: f64,
    /// Keyed options: match name and keys ([`plan_camera_keys`]).
    pub keys: Vec<(&'static str, Vec<KeyPlan>)>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LightPlan {
    /// `ldta@136`: 1 spot, 2 point (252, 254).
    pub kind: u32,
    pub intensity: f64,
    /// `[A, R, G, B]` 0..255 (343).
    pub color: [f64; 4],
    pub cone_angle: f64,
    pub cone_feather: f64,
    /// Keyed options: match name and keys ([`plan_light_keys`]).
    pub keys: Vec<(&'static str, Vec<KeyPlan>)>,
}

/// The finite scalar at `path`; a property there that is not one is reported (its default is
/// written), an absent one is not.
fn scalar(layer: &Layer, path: &str, what: &str, col: &mut Collector) -> Option<f64> {
    match layer.props.prop(path).map(|p| &p.value) {
        Some(Value::Scalar(v)) if v.is_finite() => Some(*v),
        None => None,
        Some(_) => {
            col.lose(LossCategory::InvalidValues, format!("{what}: {path} is not a number, the default is written"));
            None
        }
    }
}

fn has_expr(layer: &Layer, path: &str) -> bool {
    layer.props.prop(path).is_some_and(|p| p.expr.as_ref().is_some_and(|e| !e.text.is_empty()))
}

fn has_keys_or_expr(layer: &Layer, path: &str) -> bool {
    layer.props.prop(path).is_some_and(|p| !p.keys.is_empty() || p.expr.as_ref().is_some_and(|e| !e.text.is_empty()))
}

/// The camera's options; expressions on them are reported (the static value is written), keys are
/// planned once the layer's time base is known ([`plan_camera_keys`]).
pub(crate) fn plan_camera(layer: &Layer, comp_width: u32, what: &str, col: &mut Collector) -> CameraPlan {
    let default_zoom = f64::from(comp_width) * 50.0 / 36.0;
    if matches!(layer.auto_orient, effectcraft_project::AutoOrient::AlongPath | effectcraft_project::AutoOrient::TowardsCamera) {
        col.lose(LossCategory::Cameras, format!("{what}: the camera's Auto-Orient {:?} is not written (written as a one-node camera)", layer.auto_orient));
    }
    for id in ["zoom", "dof", "focusDistance", "aperture", "blurLevel"] {
        if has_expr(layer, &format!("cameraOptions/{id}")) {
            col.lose(LossCategory::Cameras, format!("{what}: an expression on the camera's {id} (its keys or static value are written)"));
        }
    }
    let iris_default = [
        ("irisShape", 0.0),
        ("irisRotation", 0.0),
        ("irisRoundness", 0.0),
        ("irisAspectRatio", 1.0),
        ("irisDiffractionFringe", 0.0),
        ("highlightGain", 0.0),
        ("highlightThreshold", 255.0),
        ("highlightSaturation", 0.0),
    ];
    for (id, d) in iris_default {
        let v = match layer.props.prop(&format!("cameraOptions/{id}")).map(|p| &p.value) {
            Some(Value::Scalar(v)) => Some(*v),
            Some(Value::Enum(e)) => Some(f64::from(*e)),
            _ => None,
        };
        if v.is_some_and(|v| (v - d).abs() > 1e-9) || has_keys_or_expr(layer, &format!("cameraOptions/{id}")) {
            col.lose(LossCategory::Cameras, format!("{what}: the camera's {id} is not written (not sampled)"));
        }
    }
    let dof = matches!(layer.props.prop("cameraOptions/dof").map(|p| &p.value), Some(Value::Bool(true)));
    CameraPlan {
        two_node: layer.auto_orient == effectcraft_project::AutoOrient::TowardsPointOfInterest,
        zoom: scalar(layer, "cameraOptions/zoom", what, col).unwrap_or(default_zoom),
        dof,
        focus: scalar(layer, "cameraOptions/focusDistance", what, col).unwrap_or(default_zoom),
        aperture: scalar(layer, "cameraOptions/aperture", what, col).unwrap_or(25.3 * f64::from(comp_width) / 1920.0),
        blur: scalar(layer, "cameraOptions/blurLevel", what, col).unwrap_or(100.0),
        keys: Vec::new(),
    }
}

/// The light's options, or `None` (reported) for a type without a sampled case.
pub(crate) fn plan_light(layer: &Layer, kind: LightKind, what: &str, col: &mut Collector) -> Option<LightPlan> {
    let code = match kind {
        LightKind::Spot => 1,
        LightKind::Point => 2,
        other => {
            col.lose(LossCategory::Lights, format!("{what}: a {} light is not written (not sampled)", other.label()));
            return None;
        }
    };
    // A spot light is written oriented towards its point of interest (a point light has no
    // orientation that shows).
    if code == 1 && layer.auto_orient != effectcraft_project::AutoOrient::TowardsPointOfInterest {
        col.lose(LossCategory::Lights, format!("{what}: the light's Auto-Orient {:?} is written as towards the point of interest", layer.auto_orient));
    }
    for id in ["intensity", "color", "coneAngle", "coneFeather"] {
        if has_expr(layer, &format!("lightOptions/{id}")) {
            col.lose(LossCategory::Lights, format!("{what}: an expression on the light's {id} (its keys or static value are written)"));
        }
    }
    let defaults = [("radius", 500.0), ("falloffDistance", 500.0), ("shadowDarkness", 100.0), ("shadowDiffusion", 0.0)];
    for (id, d) in defaults {
        if scalar(layer, &format!("lightOptions/{id}"), what, col).is_some_and(|v| (v - d).abs() > 1e-9)
            || has_keys_or_expr(layer, &format!("lightOptions/{id}"))
        {
            col.lose(LossCategory::Lights, format!("{what}: the light's {id} is not written (not sampled)"));
        }
    }
    // Options that are not written at all: a value other than the default, or keys or an expression
    // (which change the render even when the static value is the default).
    if matches!(layer.props.prop("lightOptions/falloff").map(|p| &p.value), Some(Value::Enum(e)) if *e != 0) || has_keys_or_expr(layer, "lightOptions/falloff")
    {
        col.lose(LossCategory::Lights, format!("{what}: the light's falloff type is not written (not sampled)"));
    }
    if matches!(layer.props.prop("lightOptions/castsShadows").map(|p| &p.value), Some(Value::Bool(true)))
        || has_keys_or_expr(layer, "lightOptions/castsShadows")
    {
        col.lose(LossCategory::Lights, format!("{what}: Casts Shadows is not written (not sampled)"));
    }
    let color = match layer.props.prop("lightOptions/color").map(|p| &p.value) {
        Some(Value::Color(c)) => {
            let out = [255.0, c[0] * 255.0, c[1] * 255.0, c[2] * 255.0].map(|v| if v.is_finite() { v.clamp(0.0, 255.0) } else { 255.0 });
            if c.iter().take(3).zip(&out[1..]).any(|(v, o)| (v * 255.0).to_bits() != o.to_bits()) {
                col.lose(LossCategory::Lights, format!("{what}: a light colour channel outside 0 to 1 is written as the nearest limit"));
            }
            out
        }
        _ => [255.0; 4],
    };
    Some(LightPlan {
        kind: code,
        intensity: scalar(layer, "lightOptions/intensity", what, col).unwrap_or(100.0),
        color,
        cone_angle: scalar(layer, "lightOptions/coneAngle", what, col).unwrap_or(90.0),
        cone_feather: scalar(layer, "lightOptions/coneFeather", what, col).unwrap_or(50.0),
        keys: Vec::new(),
    })
}

/// How a keyed option stores its values (lab camlight-keys).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Unit {
    /// A number as shown (pixels, percent, degrees): one-component plain keys, ease in shown
    /// units per second.
    Plain,
    /// Depth of Field: 0 / 1, held keys (After Effects holds a switch's keys).
    Switch,
    /// The light colour: `[A, R, G, B]` 0..255 in colour keys, written linear.
    Color,
}

/// Keyed camera options: property id, match name, unit.
const CAMERA_KEYED: &[(&str, &str, Unit)] = &[
    ("zoom", "ADBE Camera Zoom", Unit::Plain),
    ("dof", "ADBE Camera Depth of Field", Unit::Switch),
    ("focusDistance", "ADBE Camera Focus Distance", Unit::Plain),
    ("aperture", "ADBE Camera Aperture", Unit::Plain),
    ("blurLevel", "ADBE Camera Blur Level", Unit::Plain),
];

/// Keyed light options.
const LIGHT_KEYED: &[(&str, &str, Unit)] = &[
    ("intensity", "ADBE Light Intensity", Unit::Plain),
    ("color", "ADBE Light Color", Unit::Color),
    ("coneAngle", "ADBE Light Cone Angle", Unit::Plain),
    ("coneFeather", "ADBE Light Cone Feather 2", Unit::Plain),
];

/// The keys of the camera's options in the layer's time base `cp`.
pub(crate) fn plan_camera_keys(layer: &Layer, cp: &CompPlan, what: &str, col: &mut Collector) -> Vec<(&'static str, Vec<KeyPlan>)> {
    plan_option_keys(layer, "cameraOptions", CAMERA_KEYED, cp, what, col)
}

/// The keys of a spot or point light's options in the layer's time base `cp` (a point light has
/// no cone properties).
pub(crate) fn plan_light_keys(layer: &Layer, cp: &CompPlan, what: &str, col: &mut Collector) -> Vec<(&'static str, Vec<KeyPlan>)> {
    plan_option_keys(layer, "lightOptions", LIGHT_KEYED, cp, what, col)
}

fn plan_option_keys(
    layer: &Layer,
    group: &str,
    table: &[(&str, &'static str, Unit)],
    cp: &CompPlan,
    what: &str,
    col: &mut Collector,
) -> Vec<(&'static str, Vec<KeyPlan>)> {
    let mut out = Vec::new();
    for (id, name, unit) in table {
        let Some(prop) = layer.props.prop(&format!("{group}/{id}")).filter(|p| !p.keys.is_empty()) else { continue };
        let here = format!("{what}: {id}");
        let mut keys = Vec::with_capacity(prop.keys.len());
        let mut beyond = false;
        for (u, k) in timed_keys(&prop.keys, &cp.tb, &here, "key", col) {
            let Ok(time) = i32::try_from(u) else {
                col.lose(LossCategory::InvalidValues, format!("{here}: key time out of range, key dropped"));
                continue;
            };
            let value = match (unit, &k.value) {
                (Unit::Plain, Value::Scalar(v)) if v.is_finite() => vec![*v],
                (Unit::Switch, Value::Bool(b)) => vec![f64::from(u8::from(*b))],
                (Unit::Switch, Value::Scalar(v)) if v.is_finite() => vec![if *v != 0.0 { 1.0 } else { 0.0 }],
                (Unit::Color, Value::Color(c)) => {
                    let out = [255.0, c[0] * 255.0, c[1] * 255.0, c[2] * 255.0].map(|v| if v.is_finite() { v.clamp(0.0, 255.0) } else { 255.0 });
                    if c.iter().take(3).zip(&out[1..]).any(|(v, o)| (v * 255.0).to_bits() != o.to_bits()) {
                        col.lose(LossCategory::Lights, format!("{here}: a colour channel outside 0 to 1 is written as the nearest limit"));
                    }
                    out.to_vec()
                }
                _ => {
                    col.lose(LossCategory::InvalidValues, format!("{here}: a key value that cannot be stored, key dropped"));
                    continue;
                }
            };
            let mut side = |i: Interp| -> u8 {
                match (unit, i) {
                    (Unit::Switch, _) | (_, Interp::Hold) => 3,
                    (_, Interp::Linear) => 1,
                    (Unit::Plain, Interp::Bezier) => 2,
                    (Unit::Color, Interp::Bezier) => {
                        col.lose(
                            LossCategory::ApproximatedKeyframes,
                            format!("{here}: bezier ease of a colour key is written as linear (only speed 0 / influence 0 is sampled)"),
                        );
                        1
                    }
                }
            };
            let (in_interp, out_interp) = (side(k.in_interp), side(k.out_interp));
            let mut ease = |eases: &[effectcraft_keyframe::Ease], on: bool| -> Vec<EaseSlot> {
                if !on {
                    return vec![EaseSlot::default()];
                }
                let e = crate::export::plan::key_ease(eases, 0);
                let speed = if e.speed.is_finite() && e.speed.abs() <= crate::export::plan::LIMIT {
                    e.speed
                } else {
                    beyond = true;
                    0.0
                };
                vec![EaseSlot { speed, influence: if e.influence.is_finite() { e.influence.clamp(0.001, 1.0) } else { 1.0 / 3.0 } }]
            };
            let (in_ease, out_ease) = (ease(&k.in_ease, in_interp == 2), ease(&k.out_ease, out_interp == 2));
            if k.roving {
                col.lose(LossCategory::ApproximatedKeyframes, format!("{here}: a roving key is written as a normal key"));
            }
            keys.push(KeyPlan {
                time,
                value,
                in_interp,
                out_interp,
                in_ease,
                out_ease,
                spatial_in: [0.0; 3],
                spatial_out: [0.0; 3],
                spatial_auto: true,
                roving: false,
                label: k.label.min(16),
            });
        }
        crate::export::plan::report_beyond(beyond, &here, col);
        if !keys.is_empty() {
            col.exported.keyframes = col.exported.keyframes.saturating_add(keys.len());
            out.push((*name, keys));
        }
    }
    out
}

/// The keyed form of the option record `rec` (lab camlight-keys): `tdb4` byte 5 loses its static
/// bit, byte 68 marks the keys and byte 79 is cleared (the colour's static record sets it); the key
/// list takes the place of `cdat`. One-component options take 48-byte plain keys, the colour
/// 152-byte colour keys.
fn keyed(mut rec: Chunk<'static>, name: &str, keys: &[KeyPlan]) -> R<Chunk<'static>> {
    let color = name == "ADBE Light Color";
    {
        let tdb4 = child_data(&mut rec, b"tdb4")?;
        let b5 = *tdb4.get(5).ok_or_else(|| tpl("short tdb4"))?;
        put_u8(tdb4, 5, b5 & !1)?;
        put_u8(tdb4, 68, 1)?;
        put_u8(tdb4, 79, 0)?;
    }
    let size = if color { 56 + 24 * 4 } else { 8 + 40 };
    let mut ldat = Vec::with_capacity(size * keys.len());
    for k in keys {
        ldat.extend(if color { spatial_record(k, 4, false, size)? } else { crate::export::emit::plain_record(1, k, size)? });
    }
    let count = u32::try_from(keys.len()).map_err(|_| tpl("too many keys"))?;
    let size = u32::try_from(size).map_err(|_| tpl("record size"))?;
    replace_cdat(&mut rec, key_list(size, count, ldat))?;
    Ok(rec)
}

/// Replaces the records of `set` that have keys with their keyed form.
fn with_keys(set: &mut [(&str, Chunk<'static>)], keys: &[(&'static str, Vec<KeyPlan>)]) -> R<()> {
    for (name, rec) in set.iter_mut() {
        if let Some((_, k)) = keys.iter().find(|(n, _)| n == name) {
            *rec = keyed(rec.clone(), name, k)?;
        }
    }
    Ok(())
}

/// The option records: per match name the `tdbs` After Effects wrote when it was changed alone.
pub(crate) struct OptionProtos {
    pub records: Vec<(String, Chunk<'static>)>,
}

impl OptionProtos {
    fn get(&self, name: &str) -> R<Chunk<'static>> {
        self.records.iter().find(|(n, _)| n == name).map(|(_, c)| c.clone()).ok_or_else(|| tpl("camera / light option prototype missing"))
    }
}

/// The order of the properties inside the Camera and Light Options groups (After Effects' own,
/// as in 342 and 254 / 343: a record only where the value is not the default).
const CAMERA_ORDER: &[&str] =
    &["ADBE Camera Zoom", "ADBE Camera Depth of Field", "ADBE Camera Focus Distance", "ADBE Camera Aperture", "ADBE Camera Blur Level"];
const LIGHT_ORDER: &[&str] = &[
    "ADBE Light Env Atom",
    "ADBE Light Backgd Visible",
    "ADBE Light Backgd Opacity",
    "ADBE Light Backgd Blur",
    "ADBE Light Intensity",
    "ADBE Light Color",
    "ADBE Light Cone Angle",
    "ADBE Light Cone Feather 2",
    "ADBE Light Falloff Start",
    "ADBE Light Falloff Distance",
    "ADBE Casts Shadows",
];

/// Sets (or inserts) the records of an options group (`tdmn` named `group`) of a layer.
fn put_group(layr: &mut Chunk<'static>, group: &str, order: &[&str], set: Vec<(&str, Chunk<'static>)>, remove: &[&str]) -> R<()> {
    let root = list_mut(layr, b"tdgp")?;
    let kids = kids_mut(root)?;
    let at = kids.iter().position(|c| is_tdmn(c, group)).ok_or_else(|| tpl("options group missing"))?;
    let g = kids.get_mut(at + 1).ok_or_else(|| tpl("options group without a list"))?;
    let gk = kids_mut(g)?;
    // An atom group (Light Env Atom) carries its own Group End inside its list.
    let (header, mut pairs) = split_group(gk)?;
    pairs.retain(|(n, _)| !remove.contains(&n.as_str()));
    for (name, rec) in set {
        let entry = vec![tdmn(name), rec];
        match pairs.iter_mut().find(|(n, _)| n == name) {
            Some(slot) => slot.1 = entry,
            None => pairs.push((name.to_string(), entry)),
        }
    }
    let rank = |n: &str| if n == GROUP_END { usize::MAX } else { order.iter().position(|g| *g == n).unwrap_or(order.len()) };
    pairs.sort_by_key(|(n, _)| rank(n));
    gk.extend(header);
    gk.extend(pairs.into_iter().flat_map(|(_, v)| v));
    Ok(())
}

/// A copy of `proto` with the plain value `v` (`cdat` @0).
fn with_value(mut proto: Chunk<'static>, v: f64) -> R<Chunk<'static>> {
    put_f64(child_data(&mut proto, b"cdat")?, 0, v)?;
    Ok(proto)
}

pub(crate) fn put_camera(layr: &mut Chunk<'static>, protos: &OptionProtos, p: &CameraPlan) -> R<()> {
    // Every option is written explicitly (After Effects' defaults depend on the composition
    // width); Zoom and Blur Level take the plain-double layout of Focus Distance (342).
    let plain = protos.get("ADBE Camera Focus Distance")?;
    let mut set = vec![
        ("ADBE Camera Zoom", with_value(plain.clone(), p.zoom)?),
        ("ADBE Camera Depth of Field", with_value(protos.get("ADBE Camera Depth of Field")?, if p.dof { 1.0 } else { 0.0 })?),
        ("ADBE Camera Focus Distance", with_value(plain.clone(), p.focus)?),
        ("ADBE Camera Aperture", with_value(protos.get("ADBE Camera Aperture")?, p.aperture)?),
        ("ADBE Camera Blur Level", with_value(plain, p.blur)?),
    ];
    with_keys(&mut set, &p.keys)?;
    put_group(layr, "ADBE Camera Options Group", CAMERA_ORDER, set, &[])
}

pub(crate) fn put_light(layr: &mut Chunk<'static>, protos: &OptionProtos, p: &LightPlan) -> R<()> {
    let mut set = Vec::new();
    let mut remove = Vec::new();
    let keyed_here = |name: &str| p.keys.iter().any(|(n, _)| *n == name);
    if (p.intensity - 100.0).abs() > 1e-9 || keyed_here("ADBE Light Intensity") {
        set.push(("ADBE Light Intensity", with_value(protos.get("ADBE Light Intensity")?, p.intensity)?));
    } else {
        remove.push("ADBE Light Intensity");
    }
    if p.color != [255.0; 4] || keyed_here("ADBE Light Color") {
        let mut rec = protos.get("ADBE Light Color")?;
        let d = child_data(&mut rec, b"cdat")?;
        for (i, v) in p.color.iter().enumerate() {
            put_f64(d, i * 8, *v)?;
        }
        set.push(("ADBE Light Color", rec));
    }
    // A spot light stores its cone only when it differs (343-345); a point light keeps the inert
    // records of its block (252).
    if p.kind == 1 {
        if (p.cone_angle - 90.0).abs() > 1e-9 || keyed_here("ADBE Light Cone Angle") {
            set.push(("ADBE Light Cone Angle", with_value(protos.get("ADBE Light Cone Angle")?, p.cone_angle)?));
        }
        if (p.cone_feather - 50.0).abs() > 1e-9 || keyed_here("ADBE Light Cone Feather 2") {
            set.push(("ADBE Light Cone Feather 2", with_value(protos.get("ADBE Light Cone Feather 2")?, p.cone_feather)?));
        }
    }
    with_keys(&mut set, &p.keys)?;
    put_group(layr, "ADBE Light Options Group", LIGHT_ORDER, set, &remove)
}
