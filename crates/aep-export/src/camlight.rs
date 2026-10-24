//! Camera and light layers (`docs/aep-format.md` 3.12; cases 251-256, 342-346).
//!
//! Both are written from a layer block of our own cases: the default two-node camera 256 and the
//! spot (254, its Intensity record removed) and point (252) lights. The layer header keeps the
//! case's type (`ldta@128` 2 camera, 1 light), source id (0 / `0xffffffff`), 3D bit and light type
//! (`ldta@136` 1 spot, 2 point); the exporter sets the auto-orient bit (`ldta@38` 0x40: a two-node
//! camera and a spot light orient towards their point of interest, 251 vs 256). The point of
//! interest is the Anchor Point in pixels and the Position an explicit value (253: 76 / 77 = 0),
//! both through the Transform writer. Options records are cloned from the one-change cases.

use effectcraft_aep::Chunk;
use effectcraft_keyframe::Value;
use effectcraft_project::{Layer, LightKind};

use crate::report::{Collector, LossCategory};
use crate::tree::{R, child_data, kids_mut, list_mut, put_f64, tpl};

/// A camera's options in the file's units (pixels, percent, a switch).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CameraPlan {
    pub two_node: bool,
    pub zoom: f64,
    pub dof: bool,
    pub focus: f64,
    pub aperture: f64,
    pub blur: f64,
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
}

fn scalar(layer: &Layer, path: &str) -> Option<f64> {
    match layer.props.prop(path).map(|p| &p.value) {
        Some(Value::Scalar(v)) if v.is_finite() => Some(*v),
        _ => None,
    }
}

fn has_keys_or_expr(layer: &Layer, path: &str) -> bool {
    layer.props.prop(path).is_some_and(|p| !p.keys.is_empty() || p.expr.as_ref().is_some_and(|e| !e.text.is_empty()))
}

/// The camera's options; keys and expressions on them are reported (the static value is written).
pub(crate) fn plan_camera(layer: &Layer, comp_width: u32, what: &str, col: &mut Collector) -> CameraPlan {
    let default_zoom = f64::from(comp_width) * 50.0 / 36.0;
    for id in ["zoom", "dof", "focusDistance", "aperture", "blurLevel"] {
        if has_keys_or_expr(layer, &format!("cameraOptions/{id}")) {
            col.lose(LossCategory::Cameras, format!("{what}: keys or an expression on the camera's {id} (the static value is written)"));
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
        if v.is_some_and(|v| (v - d).abs() > 1e-9) {
            col.lose(LossCategory::Cameras, format!("{what}: the camera's {id} is not written (not sampled)"));
        }
    }
    let dof = matches!(layer.props.prop("cameraOptions/dof").map(|p| &p.value), Some(Value::Bool(true)));
    CameraPlan {
        two_node: layer.auto_orient == effectcraft_project::AutoOrient::TowardsPointOfInterest,
        zoom: scalar(layer, "cameraOptions/zoom").unwrap_or(default_zoom),
        dof,
        focus: scalar(layer, "cameraOptions/focusDistance").unwrap_or(default_zoom),
        aperture: scalar(layer, "cameraOptions/aperture").unwrap_or(25.3 * f64::from(comp_width) / 1920.0),
        blur: scalar(layer, "cameraOptions/blurLevel").unwrap_or(100.0),
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
    for id in ["intensity", "color", "coneAngle", "coneFeather"] {
        if has_keys_or_expr(layer, &format!("lightOptions/{id}")) {
            col.lose(LossCategory::Lights, format!("{what}: keys or an expression on the light's {id} (the static value is written)"));
        }
    }
    let defaults = [("radius", 500.0), ("falloffDistance", 500.0), ("shadowDarkness", 100.0), ("shadowDiffusion", 0.0)];
    for (id, d) in defaults {
        if scalar(layer, &format!("lightOptions/{id}")).is_some_and(|v| (v - d).abs() > 1e-9) {
            col.lose(LossCategory::Lights, format!("{what}: the light's {id} is not written (not sampled)"));
        }
    }
    if matches!(layer.props.prop("lightOptions/falloff").map(|p| &p.value), Some(Value::Enum(e)) if *e != 0) {
        col.lose(LossCategory::Lights, format!("{what}: the light's falloff type is not written (not sampled)"));
    }
    if matches!(layer.props.prop("lightOptions/castsShadows").map(|p| &p.value), Some(Value::Bool(true))) {
        col.lose(LossCategory::Lights, format!("{what}: Casts Shadows is not written (not sampled)"));
    }
    let color = match layer.props.prop("lightOptions/color").map(|p| &p.value) {
        Some(Value::Color(c)) => [255.0, c[0] * 255.0, c[1] * 255.0, c[2] * 255.0].map(|v| if v.is_finite() { v.clamp(0.0, 255.0) } else { 255.0 }),
        _ => [255.0; 4],
    };
    Some(LightPlan {
        kind: code,
        intensity: scalar(layer, "lightOptions/intensity").unwrap_or(100.0),
        color,
        cone_angle: scalar(layer, "lightOptions/coneAngle").unwrap_or(90.0),
        cone_feather: scalar(layer, "lightOptions/coneFeather").unwrap_or(50.0),
    })
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

pub(crate) fn tdmn_chunk(name: &str) -> Chunk<'static> {
    let mut b = vec![0u8; 40];
    for (i, c) in name.bytes().take(39).enumerate() {
        b[i] = c;
    }
    Chunk::data(*b"tdmn", b)
}

fn tdmn_name(c: &Chunk<'_>) -> Option<String> {
    let b = c.bytes()?;
    let end = b.iter().position(|x| *x == 0).unwrap_or(b.len());
    std::str::from_utf8(b.get(..end)?).ok().map(str::to_string)
}

/// Sets (or inserts) the records of an options group (`tdmn` named `group`) of a layer.
fn put_group(layr: &mut Chunk<'static>, group: &str, order: &[&str], set: Vec<(&str, Chunk<'static>)>, remove: &[&str]) -> R<()> {
    let root = list_mut(layr, b"tdgp")?;
    let kids = kids_mut(root)?;
    let at = kids.iter().position(|c| c.is(b"tdmn") && tdmn_name(c).as_deref() == Some(group)).ok_or_else(|| tpl("options group missing"))?;
    let g = kids.get_mut(at + 1).ok_or_else(|| tpl("options group without a list"))?;
    let gk = kids_mut(g)?;
    let first = gk.iter().position(|c| c.is(b"tdmn")).unwrap_or(gk.len());
    let header: Vec<Chunk<'static>> = gk.drain(..first).collect();
    let mut pairs: Vec<(String, Vec<Chunk<'static>>)> = Vec::new();
    let mut it = std::mem::take(gk).into_iter();
    while let Some(m) = it.next() {
        let name = tdmn_name(&m).unwrap_or_default();
        if name == "ADBE Group End" {
            pairs.push((name, vec![m]));
            continue;
        }
        let Some(node) = it.next() else { break };
        // An atom group (Light Env Atom) carries its own Group End inside the list.
        pairs.push((name, vec![m, node]));
    }
    pairs.retain(|(n, _)| !remove.contains(&n.as_str()));
    for (name, rec) in set {
        let entry = vec![tdmn_chunk(name), rec];
        match pairs.iter_mut().find(|(n, _)| n == name) {
            Some(slot) => slot.1 = entry,
            None => pairs.push((name.to_string(), entry)),
        }
    }
    let rank = |n: &str| if n == "ADBE Group End" { usize::MAX } else { order.iter().position(|g| *g == n).unwrap_or(order.len()) };
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
    let set = vec![
        ("ADBE Camera Zoom", with_value(plain.clone(), p.zoom)?),
        ("ADBE Camera Depth of Field", with_value(protos.get("ADBE Camera Depth of Field")?, if p.dof { 1.0 } else { 0.0 })?),
        ("ADBE Camera Focus Distance", with_value(plain.clone(), p.focus)?),
        ("ADBE Camera Aperture", with_value(protos.get("ADBE Camera Aperture")?, p.aperture)?),
        ("ADBE Camera Blur Level", with_value(plain, p.blur)?),
    ];
    put_group(layr, "ADBE Camera Options Group", CAMERA_ORDER, set, &[])
}

pub(crate) fn put_light(layr: &mut Chunk<'static>, protos: &OptionProtos, p: &LightPlan) -> R<()> {
    let mut set = Vec::new();
    let mut remove = Vec::new();
    if (p.intensity - 100.0).abs() > 1e-9 {
        set.push(("ADBE Light Intensity", with_value(protos.get("ADBE Light Intensity")?, p.intensity)?));
    } else {
        remove.push("ADBE Light Intensity");
    }
    if p.color != [255.0; 4] {
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
        if (p.cone_angle - 90.0).abs() > 1e-9 {
            set.push(("ADBE Light Cone Angle", with_value(protos.get("ADBE Light Cone Angle")?, p.cone_angle)?));
        }
        if (p.cone_feather - 50.0).abs() > 1e-9 {
            set.push(("ADBE Light Cone Feather 2", with_value(protos.get("ADBE Light Cone Feather 2")?, p.cone_feather)?));
        }
    }
    put_group(layr, "ADBE Light Options Group", LIGHT_ORDER, set, &remove)
}
