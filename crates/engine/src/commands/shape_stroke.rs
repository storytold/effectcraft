//! Shape Stroke options: the Dashes "+" / "−" buttons (Dash 2 / Gap 2, Dash 3 / Gap 3), Taper and
//! Wave. Taper and Wave are ordinary properties of the stroke (`…/stroke/taper/startLength`); the
//! commands here set several of them in one undo step and add the groups to strokes saved before
//! they existed. Also the toolbar's Fill / Stroke options on selected shape layers.

use effectcraft_keyframe::{Gradient, Value as KV};
use effectcraft_project::build::{self, EXTRA_DASHES, Ids};
use effectcraft_project::{ItemId, Layer, LayerId, LayerSource, Node, PropGroup, Uid};
use effectcraft_time::Tick;
use serde_json::{Value, json};

use super::layer::color_p;
use super::shape_tool::{self, PaintKind};
use super::{CommandSpec, bad, f_p, has_layers, layer_mut, layer_p, layers_p, merge_p};
use crate::{EngineError, Result, Session, cmd};

fn is_stroke(g: &PropGroup) -> bool {
    matches!(g.match_id.as_str(), "stroke" | "gstroke")
}

/// `{layer?, prop?: stroke uid}` → the stroke; without `prop`, the first stroke of the layer (a
/// selected stroke property wins).
fn stroke_ref(s: &Session, p: &Value, cmd: &str) -> Result<(ItemId, LayerId, Uid)> {
    let (cid, lid) = layer_p(s, p, cmd)?;
    let l = s.project.comp(cid).and_then(|c| c.layer(lid)).ok_or(EngineError::NoComp)?;
    if let Some(uid) = p.get("prop").and_then(Value::as_u64) {
        let g = l.props.find_group(uid).ok_or_else(|| bad(cmd, format!("no property group @{uid}")))?;
        if !is_stroke(g) {
            return Err(bad(cmd, format!("`{}` is not a Stroke or Gradient Stroke", g.name)));
        }
        return Ok((cid, lid, uid));
    }
    let contents = l.props.sub("contents").ok_or_else(|| bad(cmd, "not a shape layer"))?;
    // A selected stroke (or a property inside one), else the first stroke in the contents.
    let mut found = None;
    for (sl, su) in &s.state.selected_props {
        if *sl == lid
            && let Some(g) = find_stroke_containing(contents, *su)
        {
            found = Some(g);
            break;
        }
    }
    if found.is_none() {
        let mut first = None;
        walk_groups(contents, &mut |g| {
            if first.is_none() && is_stroke(g) {
                first = Some(g.uid);
            }
        });
        found = first;
    }
    found.map(|u| (cid, lid, u)).ok_or_else(|| bad(cmd, "the layer has no stroke"))
}

fn walk_groups(g: &PropGroup, f: &mut impl FnMut(&PropGroup)) {
    for sub in g.groups() {
        f(sub);
        walk_groups(sub, f);
    }
}

fn find_stroke_containing(contents: &PropGroup, uid: Uid) -> Option<Uid> {
    let mut out = None;
    walk_groups(contents, &mut |g| {
        if out.is_none() && is_stroke(g) && (g.uid == uid || g.find(uid).is_some() || g.find_group(uid).is_some()) {
            out = Some(g.uid);
        }
    });
    out
}

fn dashes_add(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "shape.dashes.add";
    let (cid, lid, uid) = stroke_ref(s, p, c)?;
    s.edit("Add Dash", None, |proj, _| {
        let mut next = proj.next_id;
        let g = layer_mut(proj, cid, lid)?.props.find_group_mut(uid).ok_or_else(|| bad(c, "stroke vanished"))?;
        let d = g.sub_mut("dashes").ok_or_else(|| bad(c, "the stroke has no Dashes group"))?;
        // Before the first "+", Dash is 0 (no dashes): the first "+" turns it on.
        let dash = d.get("dash").map(|p| p.value.as_f64()).unwrap_or(0.0);
        let mut ids = Ids(&mut next);
        let added = if dash <= 0.0 {
            if let Some(p) = d.get_mut("dash") {
                p.value = KV::Scalar(10.0);
            }
            "dash"
        } else {
            let Some((dm, gm)) = EXTRA_DASHES.iter().find(|(dm, _)| d.get(dm).is_none()) else {
                return Err(bad(c, "a stroke has at most three dash/gap pairs"));
            };
            let k = if *dm == "dash2" { 2 } else { 3 };
            // Insert before Offset, which stays last.
            let at = d.children.iter().position(|n| n.match_id() == "offset").unwrap_or(d.children.len());
            let dv = d.get("dash").map(|p| p.value.as_f64()).unwrap_or(10.0);
            let gv = d.get("gap").map(|p| p.value.as_f64()).filter(|g| *g > 0.0).unwrap_or(dv);
            d.children.insert(at, ids.prop(gm, &format!("Gap {k}"), KV::Scalar(gv)).with_ui(effectcraft_project::ParamUi::Pixels).into());
            d.children.insert(at, ids.prop(dm, &format!("Dash {k}"), KV::Scalar(dv)).with_ui(effectcraft_project::ParamUi::Pixels).into());
            dm
        };
        proj.next_id = next;
        Ok(json!({"added": added}))
    })
}

fn dashes_remove(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "shape.dashes.remove";
    let (cid, lid, uid) = stroke_ref(s, p, c)?;
    s.edit("Remove Dash", None, |proj, _| {
        let g = layer_mut(proj, cid, lid)?.props.find_group_mut(uid).ok_or_else(|| bad(c, "stroke vanished"))?;
        let d = g.sub_mut("dashes").ok_or_else(|| bad(c, "the stroke has no Dashes group"))?;
        for (dm, gm) in EXTRA_DASHES.iter().rev() {
            if d.get(dm).is_some() {
                d.children.retain(|n| n.match_id() != *dm && n.match_id() != *gm);
                return Ok(json!({"removed": dm}));
            }
        }
        // The last "−" turns dashes off.
        if let Some(p) = d.get_mut("dash") {
            p.value = KV::Scalar(0.0);
            p.keys.clear();
        }
        Ok(json!({"removed": "dash"}))
    })
}

/// Set `fields` (`param name → (match id, kind)`) of the stroke's `group`, creating the group.
fn set_group(s: &mut Session, p: &Value, c: &str, group: &str, fields: &[(&str, &str, bool)]) -> Result<Value> {
    let (cid, lid, uid) = stroke_ref(s, p, c)?;
    let t = s.time();
    let label = if group == "taper" { "Stroke Taper" } else { "Stroke Wave" };
    s.edit(label, merge_p(p), |proj, _| {
        let mut next = proj.next_id;
        {
            let g = layer_mut(proj, cid, lid)?.props.find_group_mut(uid).ok_or_else(|| bad(c, "stroke vanished"))?;
            if g.sub(group).is_none() {
                let mut ids = Ids(&mut next);
                let ng = if group == "taper" { build::stroke_taper(&mut ids) } else { build::stroke_wave(&mut ids) };
                g.children.push(ng.into());
            }
        }
        proj.next_id = next;
        let l = layer_mut(proj, cid, lid)?;
        let lt = l.layer_time(t);
        let g = l.props.find_group_mut(uid).ok_or_else(|| bad(c, "stroke vanished"))?;
        let sub = g.sub_mut(group).ok_or_else(|| bad(c, "group vanished"))?;
        let mut out = serde_json::Map::new();
        for (param, m, is_enum) in fields {
            let Some(v) = p.get(*param) else { continue };
            let pr = sub.get_mut(m).ok_or_else(|| bad(c, format!("no {m}")))?;
            let nv = if *is_enum {
                let opts: &[&str] = if *m == "units" && group == "taper" { &["pixels", "percent"] } else { &["pixels", "cycles"] };
                let i = match v {
                    Value::String(s) => {
                        opts.iter().position(|o| o.eq_ignore_ascii_case(s)).ok_or_else(|| bad(c, format!("`{param}` is one of {opts:?}")))? as u32
                    }
                    Value::Number(n) => n.as_u64().unwrap_or(0).min(opts.len() as u64 - 1) as u32,
                    _ => return Err(bad(c, format!("bad `{param}`"))),
                };
                KV::Enum(i)
            } else {
                let x = v.as_f64().ok_or_else(|| bad(c, format!("`{param}` must be a number")))?;
                let x = if let effectcraft_project::ParamUi::Slider { min, max, .. } = pr.ui { x.clamp(min, max) } else { x.max(0.0) };
                KV::Scalar(x)
            };
            pr.set_value_at(lt, nv.clone());
            out.insert(param.to_string(), nv.to_json());
        }
        Ok(json!({"stroke": uid, group: Value::Object(out)}))
    })
}

fn taper(s: &mut Session, p: &Value) -> Result<Value> {
    set_group(
        s,
        p,
        "shape.stroke.taper",
        "taper",
        &[
            ("units", "units", true),
            ("startLength", "startLength", false),
            ("endLength", "endLength", false),
            ("startWidth", "startWidth", false),
            ("endWidth", "endWidth", false),
            ("startEase", "startEase", false),
            ("endEase", "endEase", false),
        ],
    )
}

fn wave(s: &mut Session, p: &Value) -> Result<Value> {
    set_group(
        s,
        p,
        "shape.stroke.wave",
        "wave",
        &[("amount", "amount", false), ("units", "units", true), ("wavelength", "wavelength", false), ("cycles", "cycles", false), ("phase", "phase", false)],
    )
}

/// Path items a Fill or Stroke paints in their group.
const PATHS: [&str; 4] = ["rect", "ellipse", "star", "path"];

/// Shape groups nest; deeper contents are left alone (never-crash recursion bound).
pub(crate) const MAX_DEPTH: usize = 64;

/// The first Fill colour, Stroke colour and Stroke Width in `layer`'s contents at layer time
/// `lt` (what the toolbar shows for a selected shape layer).
pub fn first_paint(layer: &Layer, lt: Tick) -> (Option<[f64; 4]>, Option<[f64; 4]>, Option<f64>) {
    let (mut fill, mut stroke, mut width) = (None, None, None);
    if let Some(c) = layer.props.sub("contents") {
        walk_groups(c, &mut |g| {
            let at = |id: &str| g.get(id).map(|p| p.value_at(lt));
            let rgba = |v: KV| v.as_color().map(f64::from);
            match g.match_id.as_str() {
                "fill" if fill.is_none() => fill = at("color").map(rgba),
                "stroke" if stroke.is_none() => {
                    stroke = at("color").map(rgba);
                    width = at("width").map(|v| v.as_f64());
                }
                _ => {}
            }
        });
    }
    (fill, stroke, width)
}

/// What the toolbar's Fill and Stroke set on the selected shape layers.
#[derive(Clone, Copy, Debug, Default)]
struct PaintEdit {
    fill: Option<[f64; 4]>,
    stroke: Option<[f64; 4]>,
    width: Option<f64>,
    /// A group with a path and no stroke gets one.
    add_stroke: bool,
    fill_kind: Option<PaintKind>,
    stroke_kind: Option<PaintKind>,
}

/// The kind of the first Fill and the first Stroke in `layer`'s contents at layer time `lt`: a
/// switched-off or missing one is None (what the toolbar shows for a selected shape layer).
pub fn first_paint_kinds(layer: &Layer, lt: Tick) -> (PaintKind, PaintKind) {
    let (mut fill, mut stroke) = (None, None);
    let kind = |g: &PropGroup| match g.match_id.as_str() {
        _ if !g.enabled => PaintKind::None,
        "fill" | "stroke" => PaintKind::Solid,
        _ if g.get("type").is_some_and(|p| p.value_at(lt).as_enum() == 1) => PaintKind::Radial,
        _ => PaintKind::Linear,
    };
    if let Some(c) = layer.props.sub("contents") {
        walk_groups(c, &mut |g| match g.match_id.as_str() {
            "fill" | "gfill" if fill.is_none() => fill = Some(kind(g)),
            "stroke" | "gstroke" if stroke.is_none() => stroke = Some(kind(g)),
            _ => {}
        });
    }
    (fill.unwrap_or(PaintKind::None), stroke.unwrap_or(PaintKind::None))
}

/// The bounds (`[x, y, w, h]`) of the path items of one contents level at layer time `lt`, where
/// a new gradient's Start and End Points go; a 100 px square at the origin without paths.
fn path_bounds(c: &PropGroup, lt: Tick) -> [f64; 4] {
    let mut acc: Option<[f64; 4]> = None;
    for g in c.groups() {
        let at = |id: &str| g.get(id).map(|p| p.value_at(lt));
        let b = match g.match_id.as_str() {
            "rect" | "ellipse" => {
                let [w, h] = at("size").map(|v| v.as_vec2()).unwrap_or([100.0; 2]);
                let [x, y] = at("position").map(|v| v.as_vec2()).unwrap_or([0.0; 2]);
                Some([x - w.abs() / 2.0, y - h.abs() / 2.0, x + w.abs() / 2.0, y + h.abs() / 2.0])
            }
            "star" => {
                let r = at("outerRadius").map(|v| v.as_f64().abs()).unwrap_or(100.0);
                let [x, y] = at("position").map(|v| v.as_vec2()).unwrap_or([0.0; 2]);
                Some([x - r, y - r, x + r, y + r])
            }
            "path" => {
                let v = at("path");
                let bez = v.as_ref().and_then(|v| v.as_path()).map(effectcraft_path::to_kurbo);
                bez.and_then(|b| effectcraft_path::bounds(&[b])).map(|r| [r.x0, r.y0, r.x1, r.y1])
            }
            _ => None,
        };
        let Some(b) = b.filter(|b| b.iter().all(|v| v.is_finite())) else { continue };
        acc = Some(match acc {
            None => b,
            Some(a) => [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])],
        });
    }
    acc.map(|[x0, y0, x1, y1]| [x0, y0, x1 - x0, y1 - y0]).unwrap_or([-50.0, -50.0, 100.0, 100.0])
}

/// Fill Options / Stroke Options on one contents level (as in After Effects, with shape layers
/// selected they change the layers' paint): None switches its Fills and Gradient Fills (or
/// Strokes and Gradient Strokes) off; another kind switches them on, turns them into that kind
/// (keeping the properties both kinds have: blend mode, opacity, width, dashes…) and gives a
/// level with paths and no such item one.
fn set_kind(c: &mut PropGroup, ids: &mut Ids, lt: Tick, stroke: bool, kind: PaintKind, e: &PaintEdit) {
    let (solid, grad) = if stroke { ("stroke", "gstroke") } else { ("fill", "gfill") };
    let is_item = |g: &PropGroup| g.match_id == solid || g.match_id == grad;
    if kind == PaintKind::None {
        for n in &mut c.children {
            if let Node::Group(g) = n
                && is_item(g)
            {
                g.enabled = false;
            }
        }
        return;
    }
    let radial = kind == PaintKind::Radial;
    let color = (if stroke { e.stroke } else { e.fill }).unwrap_or([1.0; 4]);
    let width = e.width.filter(|w| *w > 0.0).unwrap_or(2.0);
    let (start, end) = shape_tool::gradient_ends(radial, path_bounds(c, lt));
    let fresh = |ids: &mut Ids| match (kind, stroke) {
        (PaintKind::Solid, false) => build::shape_fill(ids, color),
        (PaintKind::Solid, true) => build::shape_stroke(ids, color, width),
        (_, false) => build::shape_gradient_fill(ids, radial, start, end, Gradient::default()),
        (_, true) => build::shape_gradient_stroke(ids, radial, start, end, Gradient::default(), width),
    };
    let want = if kind == PaintKind::Solid { solid } else { grad };
    let mut found = false;
    for i in 0..c.children.len() {
        let Some(Node::Group(g)) = c.children.get(i) else { continue };
        if !is_item(g) {
            continue;
        }
        found = true;
        let replacement = (g.match_id != want).then(|| {
            let mut new = fresh(ids);
            new.uid = g.uid;
            new.name = super::effect::unique_name(c, &new.name);
            for n in &mut new.children {
                if let Some(old) = g.children.iter().find(|o| o.match_id() == n.match_id()) {
                    *n = old.clone();
                }
            }
            new
        });
        let Some(Node::Group(g)) = c.children.get_mut(i) else { continue };
        match replacement {
            Some(new) => *g = new,
            None => {
                g.enabled = true;
                if let Some(t) = g.get_mut("type").filter(|_| want == grad) {
                    t.set_value_at(lt, KV::Enum(radial as u32));
                }
            }
        }
    }
    if !found && c.groups().any(|g| PATHS.contains(&g.match_id.as_str())) {
        let mut new = fresh(ids);
        new.name = super::effect::unique_name(c, &new.name);
        // Paths, strokes, then fills, as new shapes have them.
        let at = if stroke {
            c.children.iter().position(|n| matches!(n.match_id(), "fill" | "gfill")).unwrap_or(c.children.len())
        } else {
            c.children.iter().rposition(|n| PATHS.contains(&n.match_id()) || matches!(n.match_id(), "stroke" | "gstroke")).map_or(c.children.len(), |i| i + 1)
        };
        c.children.insert(at, new.into());
    }
}

/// Paint one contents group and its sub-groups: the kinds first, then Fills take `fill`, Strokes
/// `stroke`, Strokes and Gradient Strokes `width`. With `add_stroke`, a group with a path and no
/// stroke gets one (before its fills, as new shapes have it).
fn paint(c: &mut PropGroup, ids: &mut Ids, lt: Tick, e: &PaintEdit, depth: usize) {
    if depth > MAX_DEPTH {
        return;
    }
    if let Some(k) = e.stroke_kind {
        set_kind(c, ids, lt, true, k, e);
    }
    if let Some(k) = e.fill_kind {
        set_kind(c, ids, lt, false, k, e);
    }
    let (fill, stroke, width) = (e.fill, e.stroke, e.width);
    if e.add_stroke && c.groups().any(|g| PATHS.contains(&g.match_id.as_str())) && !c.groups().any(is_stroke) {
        let at = c.children.iter().position(|n| matches!(n, Node::Group(g) if matches!(g.match_id.as_str(), "fill" | "gfill"))).unwrap_or(c.children.len());
        c.children.insert(at, build::shape_stroke(ids, stroke.unwrap_or([1.0; 4]), width.unwrap_or(2.0)).into());
    }
    for n in &mut c.children {
        let Node::Group(g) = n else { continue };
        let set = |g: &mut PropGroup, id: &str, v: Option<KV>| {
            if let (Some(v), Some(p)) = (v, g.get_mut(id)) {
                p.set_value_at(lt, v);
            }
        };
        match g.match_id.as_str() {
            "fill" => set(g, "color", fill.map(KV::Color)),
            "stroke" => {
                set(g, "color", stroke.map(KV::Color));
                set(g, "width", width.map(KV::Scalar));
            }
            "gstroke" => set(g, "width", width.map(KV::Scalar)),
            "group" => {
                if let Some(sub) = g.sub_mut("contents") {
                    paint(sub, ids, lt, e, depth + 1);
                }
            }
            _ => {}
        }
    }
}

/// `p`'s `fillType` / `strokeType` (none|solid|linear|radial).
fn kind_p(p: &Value, key: &str, c: &str) -> Result<Option<PaintKind>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_str().and_then(PaintKind::from_name).map(Some).ok_or_else(|| bad(c, format!("`{key}`: none|solid|linear|radial"))),
    }
}

/// The toolbar's Fill and Stroke options with shape layers selected (as in After Effects, they
/// change the layers' fills and strokes): `fillType` / `strokeType` switch every Fill / Stroke
/// off (none) or turn them into Solid Color, Linear or Radial Gradient ones (adding one where a
/// path has none), `fill` sets every Fill's colour, `stroke` every Stroke's, `strokeWidth` every
/// stroke's width. A stroke colour or a width above 0 gives the paths that have no stroke one.
/// Without values, reports the first layer's paint.
fn fill_stroke(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "shape.fillStroke";
    let (cid, lids) = layers_p(s, p)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let shapes: Vec<LayerId> = lids.into_iter().filter(|l| comp.layer(*l).is_some_and(|l| matches!(l.source, LayerSource::Shape))).collect();
    if shapes.is_empty() {
        return Err(bad(c, "select a shape layer"));
    }
    let rgba = |k: &str| color_p(p, k).map(|c| [c[0] as f64, c[1] as f64, c[2] as f64, 1.0]);
    let (fill, stroke) = (rgba("fill"), rgba("stroke"));
    let width = f_p(p, "strokeWidth").filter(|w| w.is_finite()).map(|w| w.max(0.0));
    let (fill_kind, stroke_kind) = (kind_p(p, "fillType", c)?, kind_p(p, "strokeType", c)?);
    let add_stroke = stroke_kind != Some(PaintKind::None) && width.map_or(stroke.is_some(), |w| w > 0.0);
    let e = PaintEdit { fill, stroke, width, add_stroke, fill_kind, stroke_kind };
    if fill.is_some() || stroke.is_some() || width.is_some() || fill_kind.is_some() || stroke_kind.is_some() {
        let t = s.time();
        s.edit("Fill and Stroke", merge_p(p), |proj, _| {
            let mut next = proj.next_id;
            for lid in &shapes {
                let l = layer_mut(proj, cid, *lid)?;
                let lt = l.layer_time(t);
                if let Some(contents) = l.props.sub_mut("contents") {
                    paint(contents, &mut Ids(&mut next), lt, &e, 0);
                }
            }
            proj.next_id = next;
            Ok(())
        })?;
    }
    let l = shapes.first().and_then(|id| s.project.comp(cid)?.layer(*id)).ok_or(EngineError::NoComp)?;
    let lt = l.layer_time(s.time());
    let (fill, stroke, width) = first_paint(l, lt);
    let (fill_kind, stroke_kind) = first_paint_kinds(l, lt);
    Ok(json!({"fill": fill, "stroke": stroke, "strokeWidth": width, "fillType": fill_kind.id(), "strokeType": stroke_kind.id()}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "shape.fillStroke",
            "Fill and Stroke",
            [],
            None,
            "{layers?, fillType?: none|solid|linear|radial, strokeType?, fill?: color, stroke?: color, strokeWidth?, merge?} → {fill, stroke, strokeWidth, fillType, strokeType} (first layer)",
            has_layers,
            fill_stroke
        ),
        cmd!("shape.dashes.add", "Add Dash or Gap", [], None, "{layer?, prop?: stroke uid}", has_layers, dashes_add),
        cmd!("shape.dashes.remove", "Remove Dash or Gap", [], None, "{layer?, prop?: stroke uid}", has_layers, dashes_remove),
        cmd!(
            "shape.stroke.taper",
            "Stroke Taper",
            [],
            None,
            "{layer?, prop?: stroke uid, units?: pixels|percent, startLength?, endLength?, startWidth? (%), endWidth? (%), startEase? (%), endEase? (%)}",
            has_layers,
            taper
        ),
        cmd!(
            "shape.stroke.wave",
            "Stroke Wave",
            [],
            None,
            "{layer?, prop?: stroke uid, amount? (%), units?: pixels|cycles, wavelength?, cycles?, phase? (°)}",
            has_layers,
            wave
        ),
    ]
}
