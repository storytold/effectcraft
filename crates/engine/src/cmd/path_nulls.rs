//! Window ▸ Create Nulls From Paths: our own panel (and these commands) linking a mask or shape
//! path to null layers with expressions, in three ways:
//!
//! - **Points Follow Nulls** (`paths.pointsFollowNulls`): one null per vertex, placed on it; the
//!   path's expression rebuilds it from the nulls (`createPath` of each null's position brought
//!   into the path's layer with `fromComp`), so moving a null moves its vertex.
//! - **Nulls Follow Points** (`paths.nullsFollowPoints`): one null per vertex whose Position
//!   expression follows the vertex (`toComp(path.points()[i])`), for attaching things to an
//!   animated path.
//! - **Trace Path** (`paths.tracePath`): one null with a Progress slider (keyframed 0 → 100 % over
//!   the layer's span, optionally looping) whose Position follows `pointOnPath(progress)`.
//!
//! The path is `path`/`prop` of `layer`, else the selected path property, else the layer's first
//! mask or shape path. Each command is one undo step.
//!
//! **Puppet pins** rig the same way (controllers drive the pins, or props ride along with a pin):
//! with `pins`, selected pins, or a layer that has Puppet pins but no path, Points Follow Nulls
//! makes a null per Position / Advanced pin and gives the pin's Position an expression following
//! it (`fromComp` of the null's anchor, so parented nulls rig limbs), and Nulls Follow Points makes
//! nulls whose Position follows each pin (`toComp` of its Position).

use effectcraft_effects::puppet::PinKind;
use effectcraft_geom::vec2;
use effectcraft_keyframe::Value as KV;
use effectcraft_project::{Comp, ItemId, Layer, LayerId, Uid};
use serde_json::{Value, json};

use super::{CommandSpec, b_p, bad, comp_id, has_layers};
use crate::{EngineError, Result, Session, cmd};

fn quote(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| format!("\"{s}\""))
}

/// The target path property: (comp, layer, uid).
fn target(s: &Session, p: &Value, cmd: &str) -> Result<(ItemId, LayerId, Uid)> {
    if p.get("path").is_some() || p.get("prop").is_some() {
        let r = super::prop::prop_ref(s, p, cmd)?;
        let is_path = s.project.comp(r.0).and_then(|c| c.layer(r.1)).and_then(|l| l.props.find(r.2)).is_some_and(|pr| matches!(pr.value, KV::Path(_)));
        return if is_path { Ok(r) } else { Err(bad(cmd, "that property is not a mask or shape path")) };
    }
    let cid = comp_id(s, p)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    // A selected path property.
    for (l, u) in super::selected_leaf_props(s) {
        if comp.layer(l).and_then(|ly| ly.props.find(u)).is_some_and(|pr| matches!(pr.value, KV::Path(_))) {
            return Ok((cid, l, u));
        }
    }
    // The first path of `layer` (or of the first selected layer).
    let lid = match p.get("layer") {
        Some(_) => super::layer_p(s, p, cmd)?.1,
        None => *s.state.selected_layers.first().ok_or_else(|| bad(cmd, "select a layer with a mask or shape path"))?,
    };
    let layer = comp.layer(lid).ok_or(EngineError::NoComp)?;
    let mut found = None;
    layer.props.walk("", &mut |_, pr| {
        if found.is_none() && matches!(pr.value, KV::Path(_)) && !matches!(pr.ui, effectcraft_project::ParamUi::Hidden) {
            found = Some(pr.uid);
        }
    });
    found.map(|u| (cid, lid, u)).ok_or_else(|| bad(cmd, format!("`{}` has no mask or shape path", layer.name)))
}

/// What the commands need about the path: its vertices in comp space (now), the path value, the
/// expression reference to it and its layer's name.
struct PathInfo {
    comp_pts: Vec<[f64; 2]>,
    path: effectcraft_keyframe::ShapePath,
    reference: String,
    layer_name: String,
    prop_name: String,
    span: (f64, f64),
}

fn info(s: &Session, cid: ItemId, lid: LayerId, uid: Uid, cmd: &str) -> Result<PathInfo> {
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let layer = comp.layer(lid).ok_or(EngineError::NoComp)?;
    let pr = layer.props.find(uid).ok_or_else(|| bad(cmd, "no such property"))?;
    let t = s.time();
    let ctx = effectcraft_render::EvalCtx { project: &s.project, comp_id: cid, comp, time: t, expr: s.expr.as_deref(), footage: Some(s.footage.as_ref()) };
    let KV::Path(path) = ctx.value(layer, pr) else { return Err(bad(cmd, "that property is not a path")) };
    if path.vertices.is_empty() {
        return Err(bad(cmd, "the path has no vertices"));
    }
    let m = ctx.layer_to_comp(layer).0;
    let comp_pts = path
        .vertices
        .iter()
        .map(|v| {
            let q = m.apply(vec2(v[0], v[1]));
            [q.x, q.y]
        })
        .collect();
    let reference = from_other_layer(comp, layer, uid).ok_or_else(|| bad(cmd, "can't reference that path"))?;
    let prop_name = layer.props.name_path_of(uid).and_then(|n| n.split('/').rev().nth(1).map(str::to_string)).unwrap_or_else(|| pr.name.clone());
    Ok(PathInfo { comp_pts, path, reference, layer_name: layer.name.clone(), prop_name, span: (layer.in_point.seconds(), layer.out_point.seconds()) })
}

/// An expression reference to `uid` of `layer` as another layer (the nulls) sees it.
fn from_other_layer(comp: &Comp, layer: &Layer, uid: Uid) -> Option<String> {
    let mut other = layer.clone();
    other.id = LayerId(u64::MAX);
    other.name = "\u{0}".into();
    super::link::reference(comp, &other, layer, uid, true)
}

/// A Puppet pin to rig: its layer, name, Position property, where it is now (comp space) and the
/// reference to its Position from another layer.
struct PinRig {
    layer: LayerId,
    layer_name: String,
    name: String,
    position: Uid,
    comp_pt: [f64; 2],
    reference: String,
}

/// The pins to rig, when the command targets Puppet pins: `pins` / `pin`, else the selected pins,
/// else (no path given) every pin of a layer that has pins but no mask or shape path. Bend,
/// Starch and Overlap pins have no Position and are skipped.
fn pin_targets(s: &Session, p: &Value, cmd: &str) -> Result<Option<(ItemId, Vec<PinRig>)>> {
    if p.get("path").is_some() || p.get("prop").is_some() {
        return Ok(None);
    }
    let named = p.get("pins").is_some() || p.get("pin").is_some();
    let pins = if named || !super::puppet::selected_pins(s).is_empty() {
        super::puppet::pins_p(s, p, cmd)?
    } else {
        if target(s, p, cmd).is_ok() {
            return Ok(None);
        }
        let cid = comp_id(s, p)?;
        let lid = match p.get("layer") {
            Some(_) => super::layer_p(s, p, cmd)?.1,
            None => match s.state.selected_layers.first() {
                Some(l) => *l,
                None => return Ok(None),
            },
        };
        let all: Vec<_> =
            s.project.comp(cid).and_then(|c| c.layer(lid)).map(|l| super::puppet::layer_pins(l).map(|g| (cid, lid, g.uid)).collect()).unwrap_or_default();
        if all.is_empty() {
            return Ok(None);
        }
        all
    };
    let cid = pins.first().map(|x| x.0).ok_or_else(|| bad(cmd, "no puppet pins"))?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let ctx =
        effectcraft_render::EvalCtx { project: &s.project, comp_id: cid, comp, time: s.time(), expr: s.expr.as_deref(), footage: Some(s.footage.as_ref()) };
    let mut out = vec![];
    for (_, lid, uid) in pins {
        let Some(layer) = comp.layer(lid) else { continue };
        let Some(g) = layer.props.find_group(uid) else { continue };
        if !PinKind::from_index(g.get("kind").map(|k| k.value.as_enum()).unwrap_or(0)).moves() {
            continue;
        }
        let Some(pos) = g.get("position") else { continue };
        let v = ctx.value(layer, pos).as_vec2();
        let q = ctx.layer_to_comp(layer).0.apply(vec2(v[0], v[1]));
        let reference = from_other_layer(comp, layer, pos.uid).ok_or_else(|| bad(cmd, "can't reference that pin"))?;
        out.push(PinRig { layer: lid, layer_name: layer.name.clone(), name: g.name.clone(), position: pos.uid, comp_pt: [q.x, q.y], reference });
    }
    if out.is_empty() {
        return Err(bad(cmd, "no Position or Advanced puppet pins to rig (Bend, Starch and Overlap pins have no Position)"));
    }
    Ok(Some((cid, out)))
}

/// A new null at `at` (comp space) named `name`, made unique in the comp (expressions find
/// layers by name). Returns (layer id, name).
fn new_null(s: &mut Session, cid: ItemId, name: &str, at: [f64; 2]) -> Result<(u64, String)> {
    let taken = |n: &str| s.project.comp(cid).is_some_and(|c| c.layers.iter().any(|l| l.name == n));
    let mut name = name.to_string();
    let base = name.clone();
    let mut k = 2;
    while taken(&name) {
        name = format!("{base} {k}");
        k += 1;
    }
    let r = s.execute("layer.newNull", json!({"comp": cid.0, "name": name}))?;
    let id = r.get("layer").and_then(Value::as_u64).ok_or_else(|| EngineError::Other("no null layer".into()))?;
    s.execute("prop.set", json!({"comp": cid.0, "layer": id, "path": "transform/position", "value": at}))?;
    Ok((id, name))
}

fn nulls_follow_points(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "paths.nullsFollowPoints";
    if let Some((cid, pins)) = pin_targets(s, p, C)? {
        let nulls = super::app_more::grouped(s, "Nulls Follow Points", |s| {
            let mut out = vec![];
            for pin in &pins {
                let (id, _) = new_null(s, cid, &format!("{}: {}", pin.layer_name, pin.name), pin.comp_pt)?;
                let e = format!("var src = thisComp.layer({});\nvar p = src.toComp({});\n[p[0], p[1]]", quote(&pin.layer_name), pin.reference);
                s.execute("prop.setExpression", json!({"comp": cid.0, "layer": id, "path": "transform/position", "expression": e}))?;
                out.push(id);
            }
            Ok(out)
        })?;
        return Ok(json!({"nulls": nulls, "pins": pins.iter().map(|p| p.position).collect::<Vec<_>>()}));
    }
    let (cid, lid, uid) = target(s, p, C)?;
    let pi = info(s, cid, lid, uid, C)?;
    let nulls = super::app_more::grouped(s, "Nulls Follow Points", |s| {
        let mut out = vec![];
        for (i, at) in pi.comp_pts.iter().enumerate() {
            let (id, _) = new_null(s, cid, &format!("{}: {} [{}]", pi.layer_name, pi.prop_name, i + 1), *at)?;
            let e = format!("var src = thisComp.layer({});\nvar p = src.toComp({}.points()[{i}]);\n[p[0], p[1]]", quote(&pi.layer_name), pi.reference);
            s.execute("prop.setExpression", json!({"comp": cid.0, "layer": id, "path": "transform/position", "expression": e}))?;
            out.push(id);
        }
        Ok(out)
    })?;
    Ok(json!({"nulls": nulls, "layer": lid.0, "prop": uid}))
}

fn points_follow_nulls(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "paths.pointsFollowNulls";
    if let Some((cid, pins)) = pin_targets(s, p, C)? {
        let nulls = super::app_more::grouped(s, "Points Follow Nulls", |s| {
            let mut out = vec![];
            for pin in &pins {
                let (id, name) = new_null(s, cid, &format!("{}: {}", pin.layer_name, pin.name), pin.comp_pt)?;
                let e = format!("var n = thisComp.layer({});\nvar q = fromComp(n.toComp(n.transform.anchorPoint));\n[q[0], q[1]]", quote(&name));
                s.execute("prop.setExpression", json!({"comp": cid.0, "layer": pin.layer.0, "prop": pin.position, "expression": e}))?;
                out.push(id);
            }
            Ok(out)
        })?;
        return Ok(json!({"nulls": nulls, "pins": pins.iter().map(|p| p.position).collect::<Vec<_>>()}));
    }
    let (cid, lid, uid) = target(s, p, C)?;
    let pi = info(s, cid, lid, uid, C)?;
    let nulls = super::app_more::grouped(s, "Points Follow Nulls", |s| {
        let mut out = vec![];
        let mut names = vec![];
        for (i, at) in pi.comp_pts.iter().enumerate() {
            let name = format!("{}: {} [{}]", pi.layer_name, pi.prop_name, i + 1);
            let (id, name) = new_null(s, cid, &name, *at)?;
            out.push(id);
            names.push(quote(&name));
        }
        let arr = |v: &[[f64; 2]]| serde_json::to_string(v).unwrap_or_else(|_| "[]".into());
        let e = format!(
            "var nulls = [{}];\nvar pts = [];\nfor (var i = 0; i < nulls.length; i++) {{\n  var n = thisComp.layer(nulls[i]);\n  var q = fromComp(n.toComp(n.transform.anchorPoint));\n  pts.push([q[0], q[1]]);\n}}\ncreatePath(pts, {}, {}, {})",
            names.join(", "),
            arr(&pi.path.in_tangents),
            arr(&pi.path.out_tangents),
            pi.path.closed
        );
        s.execute("prop.setExpression", json!({"comp": cid.0, "layer": lid.0, "prop": uid, "expression": e}))?;
        Ok(out)
    })?;
    Ok(json!({"nulls": nulls, "layer": lid.0, "prop": uid}))
}

fn trace_path(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "paths.tracePath";
    let (cid, lid, uid) = target(s, p, C)?;
    let pi = info(s, cid, lid, uid, C)?;
    let looping = b_p(p, "loop").unwrap_or(false);
    let nid = super::app_more::grouped(s, "Trace Path", |s| {
        let (id, _) = new_null(s, cid, &format!("Trace {}: {}", pi.layer_name, pi.prop_name), pi.comp_pts[0])?;
        s.execute("effect.apply", json!({"comp": cid.0, "layers": [id], "effect": "ec.control.slider"}))?;
        let fx = s.project.comp(cid).and_then(|c| c.layer(LayerId(id))).and_then(|l| l.props.group("effects/#1")).map(|g| g.uid);
        if let Some(fx) = fx {
            s.execute("prop.renameGroup", json!({"comp": cid.0, "layer": id, "prop": fx, "name": "Progress"}))?;
        }
        let (a, b) = pi.span;
        s.execute("prop.addKey", json!({"comp": cid.0, "layer": id, "path": "effects/#1/slider", "time": a, "value": 0.0}))?;
        s.execute("prop.addKey", json!({"comp": cid.0, "layer": id, "path": "effects/#1/slider", "time": b.max(a), "value": 100.0}))?;
        if looping {
            s.execute("prop.setExpression", json!({"comp": cid.0, "layer": id, "path": "effects/#1/slider", "expression": "loopOut(\"cycle\")"}))?;
        }
        let e = format!(
            "var src = thisComp.layer({});\nvar p = src.toComp({}.pointOnPath(effect(\"Progress\")(\"Slider\") / 100));\n[p[0], p[1]]",
            quote(&pi.layer_name),
            pi.reference
        );
        s.execute("prop.setExpression", json!({"comp": cid.0, "layer": id, "path": "transform/position", "expression": e}))?;
        Ok(id)
    })?;
    Ok(json!({"null": nid, "layer": lid.0, "prop": uid}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "paths.pointsFollowNulls",
            "Points Follow Nulls",
            [],
            None,
            "{layer?, path?|prop?, pins?: [uid | name…]} → a null per vertex (or Position / Advanced puppet pin); the path (pins) follow them (expressions)",
            has_layers,
            points_follow_nulls
        ),
        cmd!(
            "paths.nullsFollowPoints",
            "Nulls Follow Points",
            [],
            None,
            "{layer?, path?|prop?, pins?: [uid | name…]} → a null per vertex (or puppet pin) following it (Position expressions)",
            has_layers,
            nulls_follow_points
        ),
        cmd!(
            "paths.tracePath",
            "Trace Path",
            [],
            None,
            "{layer?, path?|prop?, loop?: bool} → a null moving along the path (Progress slider)",
            has_layers,
            trace_path
        ),
    ]
}
