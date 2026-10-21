//! Layer menu.

use effectcraft_color::{BlendMode, Label};
use effectcraft_keyframe::{Justify, TextDoc, Value as KV};
use effectcraft_project::build::{self, Ids};
use effectcraft_project::{
    Comp, FrameBlend, GroupKind, ItemId, ItemKind, Layer, LayerId, LayerSource, MaskMode, MatteKind, Node, Project, PropGroup, Quality, Solid, TrackMatte,
    limits,
};
use effectcraft_time::Tick;
use serde_json::{Value, json};

use super::{CommandSpec, b_p, bad, comp_id, f_p, has_comp, has_layers, layer_mut, layer_p, layers_p, merge_p, resolve_layer, str_p, unlocked};
use crate::{EngineError, Result, Session, cmd};

pub(crate) fn color_p(p: &Value, k: &str) -> Option<[f32; 3]> {
    match p.get(k)? {
        Value::String(s) => effectcraft_color::Rgba::from_hex(s).map(|c| [c.r, c.g, c.b]),
        Value::Array(a) => {
            let g = |i: usize| a.get(i).and_then(Value::as_f64).unwrap_or(0.0) as f32;
            Some([g(0), g(1), g(2)])
        }
        _ => None,
    }
}

/// Insert a new layer above the selection (or at the top) and select it.
pub(crate) fn insert_layer(proj: &mut Project, st: &mut crate::EditorState, cid: ItemId, layer: Layer) -> Result<LayerId> {
    insert_layer_at(proj, st, cid, layer, None)
}

/// `index`: an `index` parameter, the layer's 1-based place in the stack (1 = top; past the
/// bottom = the bottom), as 0-based.
pub(crate) fn index_p(p: &Value, cmd: &str) -> Result<Option<usize>> {
    match p.get("index") {
        None | Some(Value::Null) => Ok(None),
        Some(v) => {
            v.as_u64().filter(|i| *i >= 1).map(|i| Some(usize::try_from(i - 1).unwrap_or(usize::MAX))).ok_or_else(|| bad(cmd, "index: 1-based stack position"))
        }
    }
}

/// A `position` parameter: `[x, y]` in comp pixels (finite).
pub(crate) fn position_p(p: &Value, cmd: &str) -> Result<Option<[f64; 2]>> {
    match p.get("position") {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_array()
            .and_then(|a| Some([a.first()?.as_f64()?, a.get(1)?.as_f64()?]))
            .filter(|c| c.iter().all(|x| x.is_finite()))
            .map(Some)
            .ok_or_else(|| bad(cmd, "position: [x, y] in comp pixels")),
    }
}

/// Put a new layer's Position at `pos` (comp pixels).
pub(crate) fn place(l: &mut Layer, pos: Option<[f64; 2]>) {
    if let Some([x, y]) = pos
        && let Some(pr) = l.props.prop_mut("transform/position")
    {
        let z = match pr.value {
            KV::Vec3(v) => v[2],
            _ => 0.0,
        };
        pr.value = KV::Vec3([x, y, z]);
    }
}

/// [`insert_layer`] at stack position `at` (0 = top) instead of above the selected layer.
pub(crate) fn insert_layer_at(proj: &mut Project, st: &mut crate::EditorState, cid: ItemId, mut layer: Layer, at: Option<usize>) -> Result<LayerId> {
    // Text and shape layers in Advanced 3D comps get Geometry Options (extrusion, bevels).
    if proj.comp(cid).is_some_and(|c| c.renderer == effectcraft_project::Renderer::Advanced3D) {
        super::model3d::add_geometry_options(&mut proj.next_id, &mut layer);
    }
    let comp = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
    let at = match at {
        Some(i) => i.min(comp.layers.len()),
        None => st.selected_layers.first().and_then(|id| comp.layers.iter().position(|l| l.id == *id)).unwrap_or(0),
    };
    let id = layer.id;
    comp.layers.insert(at, layer);
    st.selected_layers = vec![id];
    st.selected_props.clear();
    st.selected_keys.clear();
    Ok(id)
}

fn solids_folder(proj: &mut Project) -> ItemId {
    proj.folder_named("Solids").unwrap_or_else(|| proj.add_item("Solids", Label::Yellow, None, ItemKind::Folder))
}

fn new_solid_like(s: &mut Session, p: &Value, adjustment: bool) -> Result<Value> {
    let cid = comp_id(s, p)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?.clone();
    let color = color_p(p, "color").unwrap_or(if adjustment { [1.0, 1.0, 1.0] } else { [0.85, 0.2, 0.2] });
    // 1 to 30,000 px a side (#788): the layer is sized from the same numbers as its solid.
    let w = p.get("width").and_then(Value::as_u64).map(limits::size).unwrap_or(comp.width);
    let h = p.get("height").and_then(Value::as_u64).map(limits::size).unwrap_or(comp.height);
    let pixel_aspect = pixel_aspect_p(p)?.unwrap_or(comp.pixel_aspect);
    let default_name = if adjustment { "Adjustment Layer 1".to_string() } else { "Solid 1".to_string() };
    let name = str_p(p, "name").map(str::to_string).unwrap_or(default_name);
    let id = s.edit(if adjustment { "New Adjustment Layer" } else { "New Solid" }, None, |proj, st| {
        let folder = solids_folder(proj);
        let sid = proj.add_item(&name, Label::Red, Some(folder), ItemKind::Solid(Solid { color, width: w, height: h, pixel_aspect }));
        let mut l = build::layer(proj, &comp, &name, LayerSource::Solid { item: sid }, (w, h), None);
        if adjustment {
            l.switches.adjustment = true;
            l.label = Label::Purple;
        }
        insert_layer(proj, st, cid, l)
    })?;
    Ok(json!({"layer": id.0}))
}

/// `pixelAspect` (a positive number), when given.
fn pixel_aspect_p(p: &Value) -> Result<Option<f64>> {
    match p.get("pixelAspect").and_then(Value::as_f64) {
        Some(v) if !(v.is_finite() && v > 0.0) => Err(bad("layer.settings", "`pixelAspect` must be a positive number")),
        v => Ok(v.map(limits::pixel_aspect)),
    }
}

fn new_solid(s: &mut Session, p: &Value) -> Result<Value> {
    new_solid_like(s, p, false)
}
fn new_adjustment(s: &mut Session, p: &Value) -> Result<Value> {
    new_solid_like(s, p, true)
}

/// Parameters of `layer.setText` / `layer.newText` that aren't text attributes.
const NOT_ATTRS: &[&str] = &["layer", "layers", "comp", "merge", "text", "range", "position", "edit"];

/// Apply `layer.setText` keys to `d`: `text` replaces characters `r` (or everything), then each
/// character / paragraph / document attribute applies to `r` (or everything).
fn apply_text_params(cmd: &str, p: &Value, d: &mut TextDoc, r: Option<std::ops::Range<usize>>) -> Result<()> {
    let mut r = r;
    if let Some(t) = str_p(p, "text") {
        match r.clone() {
            Some(rr) => {
                d.replace_range(rr.clone(), t, None);
                r = Some(rr.start..rr.start + t.chars().count());
            }
            None => d.set_text(t),
        }
    }
    let Some(obj) = p.as_object() else { return Ok(()) };
    for (k, v) in obj {
        if NOT_ATTRS.contains(&k.as_str()) {
            continue;
        }
        if !d.set_attr(k, v, r.clone()).map_err(|e| bad(cmd, e))? {
            return Err(bad(cmd, format!("unknown text attribute {k}")));
        }
    }
    Ok(())
}

fn text_range_p(p: &Value) -> Option<std::ops::Range<usize>> {
    let a = p.get("range")?.as_array()?;
    let g = |i: usize| a.get(i).and_then(Value::as_u64).map(|x| x as usize);
    let (x, y) = (g(0)?, g(1).or(g(0))?);
    Some(x.min(y)..x.max(y))
}

fn new_text(s: &mut Session, p: &Value) -> Result<Value> {
    let cid = comp_id(s, p)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?.clone();
    let mut doc = TextDoc { text: "Text".into(), justify: Justify::Center, ..Default::default() };
    // A paragraph box given in comp space: the layer sits at its centre.
    let bx = p.get("box").and_then(Value::as_array).map(|a| [0, 1, 2, 3].map(|i| a.get(i).and_then(Value::as_f64).unwrap_or(0.0)));
    let mut q = p.clone();
    if let Some(o) = q.as_object_mut() {
        o.remove("box");
        o.remove("name");
    }
    let layer_name = str_p(p, "name").map(str::to_string);
    apply_text_params("layer.newText", &q, &mut doc, None)?;
    let mut pos = position_p(p, "layer.newText")?;
    if let Some([x, y, w, h]) = bx {
        let (w, h) = (w.abs().max(1.0), h.abs().max(1.0));
        doc.box_size = Some([w, h]);
        doc.box_pos = [-w / 2.0, -h / 2.0];
        pos = Some([x + w / 2.0, y + h / 2.0]);
        if p.get("justify").is_none() {
            doc.set_attr("justify", &json!("left"), None).map_err(|e| bad("layer.newText", e))?;
        }
    }
    let edit = b_p(p, "edit").unwrap_or(false);
    let id = s.edit("New Text Layer", None, |proj, st| {
        let name: String = layer_name.clone().unwrap_or_else(|| doc.text.lines().next().unwrap_or("").chars().take(40).collect());
        let mut l = build::layer(proj, &comp, if name.is_empty() { "Text" } else { &name }, LayerSource::Text, (comp.width, comp.height), None);
        if let Some(pr) = l.props.prop_mut("text/sourceText") {
            pr.value = KV::Text(Box::new(doc.clone()));
        }
        if let Some(pos) = pos
            && let Some(pr) = l.props.prop_mut("transform/position")
        {
            pr.value = KV::Vec3([pos[0], pos[1], 0.0]);
        }
        let n = doc.char_len();
        let id = insert_layer(proj, st, cid, l)?;
        if edit {
            st.text_edit = Some(super::text_edit::TextEdit { layer: id, anchor: 0, caret: n, created: true, ..Default::default() });
        }
        Ok(id)
    })?;
    Ok(json!({"layer": id.0}))
}

fn new_shape(s: &mut Session, p: &Value) -> Result<Value> {
    let cid = comp_id(s, p)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?.clone();
    let kind = str_p(p, "kind").unwrap_or("none").to_string();
    let size = p
        .get("size")
        .and_then(Value::as_array)
        .map(|a| [a.first().and_then(Value::as_f64).unwrap_or(200.0), a.get(1).and_then(Value::as_f64).unwrap_or(200.0)])
        .unwrap_or([300.0, 300.0]);
    // A fill, and a stroke only with a width (not the Tools bar's paint).
    let paint = super::shape_tool::paint_p(p, &Default::default(), "layer.newShape")?;
    let pos = p
        .get("position")
        .and_then(|v| v.as_array())
        .map(|a| [a.first().and_then(Value::as_f64).unwrap_or(0.0), a.get(1).and_then(Value::as_f64).unwrap_or(0.0)]);
    let name = str_p(p, "name").unwrap_or("Shape Layer 1").to_string();
    let id = s.edit("New Shape Layer", None, |proj, st| {
        let lid = super::shape_tool::new_shape_layer(proj, st, &comp, cid, Some(&name), pos)?;
        let mut next = proj.next_id;
        let g = super::shape_tool::shape_group(&mut Ids(&mut next), &kind, size, &paint);
        proj.next_id = next;
        if let Some(g) = g {
            super::shape_tool::add_to_contents(proj, cid, lid, g, [0.0, 0.0], "layer.newShape")?;
        }
        Ok(lid)
    })?;
    Ok(json!({"layer": id.0}))
}

fn new_simple(s: &mut Session, p: &Value, src: LayerSource, name: &str, label: &str) -> Result<Value> {
    let cid = comp_id(s, p)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?.clone();
    let name = str_p(p, "name").unwrap_or(name).to_string();
    let id = s.edit(label, None, |proj, st| {
        let l = build::layer(proj, &comp, &name, src, (comp.width, comp.height), None);
        insert_layer(proj, st, cid, l)
    })?;
    Ok(json!({"layer": id.0}))
}

fn new_null(s: &mut Session, p: &Value) -> Result<Value> {
    new_simple(s, p, LayerSource::Null, "Null 1", "New Null Object")
}

fn add_item(s: &mut Session, p: &Value) -> Result<Value> {
    let cid = comp_id(s, p)?;
    let item = match p.get("item") {
        Some(Value::Number(n)) => ItemId(n.as_u64().unwrap_or(0)),
        Some(Value::String(name)) => s.project.find_by_name(name).map(|i| i.id).ok_or_else(|| bad("layer.addItem", format!("no item `{name}`")))?,
        _ => *s.state.project_selection.first().ok_or_else(|| bad("layer.addItem", "missing `item`"))?,
    };
    if s.project.comp_contains(item, cid) {
        return Err(bad("layer.addItem", "a composition can't contain itself"));
    }
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?.clone();
    let it = s.project.item(item).ok_or_else(|| bad("layer.addItem", "no such item"))?.clone();
    let (src, size, dur) = match &it.kind {
        ItemKind::Comp(c) => (LayerSource::Comp { item }, (c.width, c.height), Some(c.duration)),
        ItemKind::Footage(f) if f.kind == effectcraft_project::FootageKind::Still => {
            // `duration`, else Settings ▸ Import ▸ Still Footage: length of the composition or a
            // duration.
            let d = f_p(p, "duration")
                .filter(|d| d.is_finite() && *d > 0.0)
                .or((s.prefs.import.still_footage == "seconds").then_some(s.prefs.import.still_seconds))
                .map(Tick::from_seconds_f64);
            (LayerSource::Footage { item }, (f.width, f.height), d)
        }
        ItemKind::Footage(f) if f.kind == effectcraft_project::FootageKind::Data => {
            return Err(bad("layer.addItem", "data files can't be layers; read them in expressions with footage(\"name\").sourceData"));
        }
        ItemKind::Footage(f) if f.kind == effectcraft_project::FootageKind::Model => {
            return super::model3d::new_model(
                s,
                &serde_json::json!({"comp": cid.0, "item": item.0, "time": f_p(p, "time"), "index": p.get("index"), "position": p.get("position")}),
            );
        }
        ItemKind::Footage(f) => (LayerSource::Footage { item }, (f.width, f.height), Some(f.looped_duration())),
        ItemKind::Solid(so) => (LayerSource::Solid { item }, (so.width, so.height), None),
        ItemKind::Folder => return Err(bad("layer.addItem", "folders can't be layers")),
    };
    let index = index_p(p, "layer.addItem")?;
    let position = position_p(p, "layer.addItem")?;
    let fr = comp.frame_rate;
    // Settings ▸ General ▸ Create Layers at Composition Start Time (off: at the current time).
    let default_start = if s.prefs.general.create_layers_at_comp_start || s.active_comp_id() != Some(cid) { Tick::ZERO } else { s.time() };
    let start = fr.snap_nearest(f_p(p, "time").map(Tick::from_seconds_f64).unwrap_or(default_start));
    let id = s.edit("Add Footage to Comp", None, |proj, st| {
        let mut l = build::layer(proj, &comp, &it.name, src, size, dur);
        l.start_time = start;
        l.in_point = start;
        // Footage and precomps keep their full length past the comp's end (After Effects); items
        // without a duration (solids, comp-length stills) end with the comp.
        let end = dur.map_or((start + comp.duration).min(comp.duration), |d| start + d);
        l.out_point = fr.snap_nearest(end).max(start + fr.frame_duration());
        place(&mut l, position);
        insert_layer_at(proj, st, cid, l, index)
    })?;
    Ok(json!({"layer": id.0}))
}

fn select(s: &mut Session, p: &Value) -> Result<Value> {
    let cid = comp_id(s, p)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let ids: Vec<LayerId> = match p.get("layers").or(p.get("layer")) {
        Some(Value::Array(a)) => a.iter().filter_map(|v| resolve_layer(comp, v)).collect(),
        Some(v) => resolve_layer(comp, v).into_iter().collect(),
        None => vec![],
    };
    let add = b_p(p, "add").unwrap_or(false);
    let toggle = b_p(p, "toggle").unwrap_or(false);
    if toggle {
        for id in ids {
            if let Some(i) = s.state.selected_layers.iter().position(|l| *l == id) {
                s.state.selected_layers.remove(i);
            } else {
                s.state.selected_layers.push(id);
            }
        }
    } else if add {
        for id in ids {
            if !s.state.selected_layers.contains(&id) {
                s.state.selected_layers.push(id);
            }
        }
    } else {
        s.state.selected_layers = ids;
        s.state.selected_props.clear();
        s.state.selected_keys.clear();
        let keep = s.state.selected_layers.clone();
        s.state.selected_vertices.retain(|v| keep.contains(&v.layer));
    }
    Ok(json!(s.state.selected_layers.iter().map(|l| l.0).collect::<Vec<_>>()))
}

fn select_step(s: &mut Session, p: &Value, dir: i64) -> Result<Value> {
    let comp = s.active_comp().ok_or(EngineError::NoComp)?;
    if comp.layers.is_empty() {
        return Ok(Value::Null);
    }
    let cur = s.state.selected_layers.first().and_then(|id| comp.layers.iter().position(|l| l.id == *id));
    let n = comp.layers.len() as i64;
    // The next layer that can be selected (locked and hidden shy layers are stepped over); at
    // the end of the stack the selection stays.
    let start = match cur {
        Some(i) => i as i64 + dir,
        None if dir > 0 => 0,
        None => n - 1,
    };
    let next =
        std::iter::successors(Some(start), |i| Some(i + dir)).take_while(|i| (0..n).contains(i)).find(|i| super::selectable(comp, &comp.layers[*i as usize]));
    let Some(i) = next.or(cur.map(|i| i as i64)) else { return Ok(Value::Null) };
    let id = comp.layers[i as usize].id;
    let add = b_p(p, "add").unwrap_or(false);
    if add {
        if !s.state.selected_layers.contains(&id) {
            s.state.selected_layers.insert(0, id);
        }
    } else {
        s.state.selected_layers = vec![id];
    }
    Ok(json!(id.0))
}

fn select_next(s: &mut Session, p: &Value) -> Result<Value> {
    select_step(s, p, 1)
}
fn select_prev(s: &mut Session, p: &Value) -> Result<Value> {
    select_step(s, p, -1)
}

fn set_switch(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, ids) = layers_p(s, p)?;
    let name = str_p(p, "switch").ok_or_else(|| bad("layer.setSwitch", "missing `switch`"))?.to_string();
    let v = b_p(p, "value");
    let label = format!("Layer Switch ({name})");
    if b_p(p, "others") == Some(true) {
        return set_switch_others(s, cid, &ids, &name, v, &label);
    }
    // `merge`: a switch dragged over several layers in the Timeline is one undo step.
    let r = s.edit(&label, merge_p(p), |proj, _| {
        let comp = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        let mut last = false;
        // Toggle relative to the first layer so a multi-selection ends up consistent.
        let first = comp.layers.iter().find(|l| ids.contains(&l.id)).map(|l| switch_value(l, &name)).unwrap_or(Some(false));
        let Some(first) = first else { return Err(bad("layer.setSwitch", format!("unknown switch `{name}`"))) };
        let target = v.unwrap_or(!first);
        for l in comp.layers.iter_mut().filter(|l| ids.contains(&l.id)) {
            set_switch_value(l, &name, target);
            last = target;
        }
        Ok(last)
    })?;
    Ok(json!(r))
}

/// `layer.setSwitch {others: true}` (Alt-click on an A/V feature in the Timeline): the named
/// layers stand alone, shown / heard / soloed / unlocked while every other layer the switch
/// applies to is the opposite; when they already do, the other layers come back. `value` sets
/// the other layers directly.
fn set_switch_others(s: &mut Session, cid: ItemId, ids: &[LayerId], name: &str, value: Option<bool>, label: &str) -> Result<Value> {
    let r = s.edit(label, None, |proj, _| {
        let comp = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        // The state that sets a layer apart: unlocked for Lock, on for the rest.
        let active = !matches!(name, "lock" | "locked");
        // Track mattes keep their video off by design.
        let mattes: Vec<LayerId> =
            if matches!(name, "video" | "eye") { comp.layers.iter().filter_map(|l| l.track_matte.map(|m| m.layer)).collect() } else { Vec::new() };
        let applies = |l: &Layer| match name {
            "audio" => l.props.sub("audio").is_some(),
            "lock" | "locked" => true,
            // Nulls have a Video switch too (it hides their outline in the viewer).
            "video" | "eye" => (l.source.is_av() || matches!(l.source, LayerSource::Null)) && !mattes.contains(&l.id),
            _ => l.source.is_av() && !mattes.contains(&l.id),
        };
        let mut alone = true;
        for l in comp.layers.iter().filter(|l| applies(l)) {
            let Some(on) = switch_value(l, name) else { return Err(bad("layer.setSwitch", format!("unknown switch `{name}`"))) };
            alone &= on == (ids.contains(&l.id) == active);
        }
        let others = value.unwrap_or(alone == active);
        for l in comp.layers.iter_mut().filter(|l| applies(l)) {
            let v = if ids.contains(&l.id) { active } else { others };
            set_switch_value(l, name, v);
        }
        Ok(others)
    })?;
    Ok(json!(r))
}

fn switch_value(l: &Layer, name: &str) -> Option<bool> {
    let sw = &l.switches;
    Some(match name {
        "video" | "eye" => sw.video,
        "audio" => sw.audio,
        "solo" => sw.solo,
        "lock" | "locked" => sw.locked,
        "shy" => sw.shy,
        "collapse" => sw.collapse,
        "quality" => sw.quality == Quality::Best,
        "fx" | "effects" => sw.effects,
        "frameBlend" => sw.frame_blend != FrameBlend::Off,
        "motionBlur" => sw.motion_blur,
        "adjustment" => sw.adjustment,
        "threeD" | "3d" | "3D" => sw.three_d,
        "guide" => sw.guide,
        "preserveTransparency" => l.preserve_transparency,
        _ => return None,
    })
}

fn set_switch_value(l: &mut Layer, name: &str, v: bool) {
    let sw = &mut l.switches;
    match name {
        "video" | "eye" => sw.video = v,
        "audio" => sw.audio = v,
        "solo" => sw.solo = v,
        "lock" | "locked" => sw.locked = v,
        "shy" => sw.shy = v,
        "collapse" => sw.collapse = v,
        "quality" => sw.quality = if v { Quality::Best } else { Quality::Draft },
        "fx" | "effects" => sw.effects = v,
        "frameBlend" => sw.frame_blend = if v { FrameBlend::FrameMix } else { FrameBlend::Off },
        "motionBlur" => sw.motion_blur = v,
        "adjustment" => sw.adjustment = v,
        "threeD" | "3d" | "3D" => {
            if sw.three_d && !v {
                drop_3d_values(l);
            }
            l.switches.three_d = v;
        }
        "guide" => sw.guide = v,
        "preserveTransparency" => l.preserve_transparency = v,
        _ => {}
    }
}

/// Turning the 3D switch off discards Z values, Orientation and X/Y Rotation (as in AE).
fn drop_3d_values(l: &mut Layer) {
    let Some(tr) = l.transform_mut() else { return };
    for (m, keep) in [("position", 0.0), ("anchor", 0.0), ("scale", 100.0)] {
        if let Some(pr) = tr.get_mut(m) {
            let flat = |v: &mut KV| {
                if let KV::Vec3(a) = v {
                    a[2] = keep;
                }
            };
            flat(&mut pr.value);
            for k in &mut pr.keys {
                flat(&mut k.value);
            }
        }
    }
    for (m, zero) in [("orientation", KV::Vec3([0.0; 3])), ("rotationX", KV::Scalar(0.0)), ("rotationY", KV::Scalar(0.0))] {
        if let Some(pr) = tr.get_mut(m) {
            pr.keys.clear();
            pr.expr = None;
            pr.value = zero;
        }
    }
}

fn rename(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, lid) = layer_p(s, p, "layer.rename")?;
    let name = str_p(p, "name").ok_or_else(|| bad("layer.rename", "missing `name`"))?.to_string();
    s.edit("Rename Layer", None, |proj, _| {
        layer_mut(proj, cid, lid)?.name = name.clone();
        Ok(())
    })?;
    Ok(Value::Null)
}

/// The Timeline's Comment column.
fn set_comment(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, ids) = layers_p(s, p)?;
    let text = str_p(p, "comment").ok_or_else(|| bad("layer.setComment", "missing `comment`"))?.to_string();
    s.edit("Layer Comment", None, |proj, _| {
        let comp = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        for l in comp.layers.iter_mut().filter(|l| ids.contains(&l.id)) {
            l.comment = text.clone();
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

fn blend(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, ids) = layers_p(s, p)?;
    let mode = str_p(p, "mode").and_then(BlendMode::from_name);
    let step = p.get("step").and_then(Value::as_i64).unwrap_or(0);
    if mode.is_none() && step == 0 {
        return Err(bad("layer.setBlendMode", "need `mode` (e.g. Multiply) or `step` ±1"));
    }
    s.edit("Blending Mode", None, |proj, _| {
        let comp = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        for l in comp.layers.iter_mut().filter(|l| ids.contains(&l.id)) {
            l.blend_mode = match mode {
                Some(m) => m,
                None if step > 0 => l.blend_mode.next(),
                None => l.blend_mode.previous(),
            };
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

fn track_matte(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, lid) = layer_p(s, p, "layer.setTrackMatte")?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let matte = match p.get("matte") {
        None | Some(Value::Null) => None,
        Some(v) => Some(resolve_layer(comp, v).ok_or_else(|| bad("layer.setTrackMatte", "no such matte layer"))?),
    };
    let kind = str_p(p, "kind")
        .map(|k| match k.to_ascii_lowercase().replace([' ', '_', '-'], "").as_str() {
            "alphainverted" | "alphainv" => MatteKind::AlphaInverted,
            "luma" => MatteKind::Luma,
            "lumainverted" | "lumainv" => MatteKind::LumaInverted,
            _ => MatteKind::Alpha,
        })
        .unwrap_or(MatteKind::Alpha);
    s.edit("Track Matte", None, |proj, _| {
        let comp = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        if let Some(m) = matte {
            if m == lid {
                return Err(bad("layer.setTrackMatte", "a layer can't be its own matte"));
            }
            if let Some(ml) = comp.layer_mut(m) {
                ml.switches.video = false;
            }
        }
        comp.layer_mut(lid).ok_or(EngineError::NoComp)?.track_matte = matte.map(|layer| TrackMatte { layer, kind });
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Re-express a layer in its new parent space without moving its anchor.
pub(crate) struct ParentFix {
    pub layer: LayerId,
    /// Maps Position from the old parent's space into the new one's.
    m: effectcraft_geom::Mat3,
    /// Added to Rotation.
    rotation: f64,
    /// Multiply Scale.
    scale: [f64; 2],
    /// 3D AV layers retain their world-space anchor, including Z.
    position_3d: Option<effectcraft_geom::Mat4>,
    /// A uniform relative parent transform can preserve static Orientation and Scale.
    pose_3d: Option<([f64; 3], f64)>,
}

impl ParentFix {
    /// Re-express the layer's Position, Rotation and Scale (every key) in the new parent's space.
    pub fn apply(&self, l: &mut Layer) {
        let Some(tr) = l.props.sub_mut("transform") else { return };
        if let Some(m) = self.position_3d {
            // Spatial Position speed measures distance along its path. Its parent-space
            // unit changes even when the child's Orientation cannot be re-expressed.
            let position_speed = uniform_parent_pose(m, [0.0; 3]).map(|(_, factor)| factor.abs());
            let map_position = |v: &KV| {
                let q = m.apply(effectcraft_geom::Vec3::from(v.as_vec3()));
                KV::Vec3([q.x, q.y, q.z])
            };
            // Read all separated axes before writing any of them.
            let names = ["positionX", "positionY", "positionZ"];
            if tr.get("positionX").is_some() {
                let axes = names.map(|name| tr.get(name).cloned());
                let current = axes.each_ref().map(|pr| pr.as_ref().map(|pr| pr.value.as_f64()).unwrap_or(0.0));
                let q = m.apply(effectcraft_geom::Vec3::from(current));
                for (i, name) in names.into_iter().enumerate() {
                    if let Some(pr) = tr.get_mut(name) {
                        pr.value = KV::Scalar([q.x, q.y, q.z].get(i).copied().unwrap_or(0.0));
                        for key in &mut pr.keys {
                            let p = axes.each_ref().map(|axis| axis.as_ref().map(|axis| axis.value_at(key.time).as_f64()).unwrap_or(0.0));
                            let q = m.apply(effectcraft_geom::Vec3::from(p));
                            key.value = KV::Scalar([q.x, q.y, q.z].get(i).copied().unwrap_or(0.0));
                        }
                    }
                }
            }
            if let Some(pr) = tr.get_mut("position") {
                pr.value = map_position(&pr.value);
                for key in &mut pr.keys {
                    key.value = map_position(&key.value);
                    // Spatial tangents are vectors, so the parent's translation is excluded.
                    for tangent in [&mut key.spatial_in, &mut key.spatial_out] {
                        let v = m.apply_vec(effectcraft_geom::Vec3::from(*tangent));
                        *tangent = [v.x, v.y, v.z];
                    }
                }
                if let Some(factor) = position_speed
                    .filter(|_| pr.spatial)
                    .filter(|factor| pr.keys.iter().flat_map(|key| key.in_ease.iter().chain(&key.out_ease)).all(|ease| (ease.speed * *factor).is_finite()))
                {
                    for key in &mut pr.keys {
                        for ease in key.in_ease.iter_mut().chain(&mut key.out_ease) {
                            ease.speed *= factor;
                        }
                    }
                }
            }
            // Cameras and lights aim with Point of Interest: move it by the same parent-space
            // change, so a two-node rig keeps its world frame (#1010, #1020).
            if let Some(pr) = tr.get_mut("poi") {
                pr.value = map_position(&pr.value);
                for key in &mut pr.keys {
                    key.value = map_position(&key.value);
                    for tangent in [&mut key.spatial_in, &mut key.spatial_out] {
                        let v = m.apply_vec(effectcraft_geom::Vec3::from(*tangent));
                        *tangent = [v.x, v.y, v.z];
                    }
                }
                if let Some(factor) = position_speed
                    .filter(|_| pr.spatial)
                    .filter(|factor| pr.keys.iter().flat_map(|key| key.in_ease.iter().chain(&key.out_ease)).all(|ease| (ease.speed * *factor).is_finite()))
                {
                    for key in &mut pr.keys {
                        for ease in key.in_ease.iter_mut().chain(&mut key.out_ease) {
                            ease.speed *= factor;
                        }
                    }
                }
            }
            if let Some((orientation, factor)) = self.pose_3d {
                if let Some(pr) = tr.get_mut("orientation") {
                    pr.value = KV::Vec3(orientation);
                }
                if let Some(pr) = tr.get_mut("scale") {
                    let scaled = |value: &KV| KV::Vec3(value.as_vec3().map(|v| v * factor));
                    pr.value = scaled(&pr.value);
                    for key in &mut pr.keys {
                        key.value = scaled(&key.value);
                        // Scale's temporal speeds use the same units as its values.
                        for ease in key.in_ease.iter_mut().chain(&mut key.out_ease) {
                            ease.speed *= factor;
                        }
                    }
                }
            }
            return;
        }
        let map = |pr: &mut effectcraft_project::Property, f: &dyn Fn(&KV) -> KV| {
            pr.value = f(&pr.value);
            for key in &mut pr.keys {
                key.value = f(&key.value);
            }
        };
        let m = self.m;
        if tr.get("positionX").is_some() {
            // Separated dimensions: X and Y move together, key by key at the same times.
            // Everything is read before anything is written.
            let [x, y] = [tr.get("positionX"), tr.get("positionY")].map(|p| p.map(|p| p.value.as_f64()).unwrap_or(0.0));
            let q = m.apply(effectcraft_geom::vec2(x, y));
            let keys_x: Vec<(Tick, f64)> = tr.get("positionX").map(|p| p.keys.iter().map(|k| (k.time, k.value.as_f64())).collect()).unwrap_or_default();
            let keys_y: Vec<(Tick, f64)> = tr.get("positionY").map(|p| p.keys.iter().map(|k| (k.time, k.value.as_f64())).collect()).unwrap_or_default();
            let at = |keys: &[(Tick, f64)], name: &str, t: Tick, tr: &PropGroup| {
                keys.iter().find(|k| k.0 == t).map(|k| k.1).unwrap_or_else(|| tr.get(name).map(|p| p.value_at(t).as_f64()).unwrap_or(0.0))
            };
            let (xs, ys): (Vec<f64>, Vec<f64>) = (
                keys_x.iter().map(|&(t, x)| m.apply(effectcraft_geom::vec2(x, at(&keys_y, "positionY", t, tr))).x).collect(),
                keys_y.iter().map(|&(t, y)| m.apply(effectcraft_geom::vec2(at(&keys_x, "positionX", t, tr), y)).y).collect(),
            );
            for (name, value, vals) in [("positionX", q.x, xs), ("positionY", q.y, ys)] {
                if let Some(pr) = tr.get_mut(name) {
                    pr.value = KV::Scalar(value);
                    for (k, v) in pr.keys.iter_mut().zip(vals) {
                        k.value = KV::Scalar(v);
                    }
                }
            }
        } else if let Some(pr) = tr.get_mut("position") {
            map(pr, &|v| {
                let c = v.as_vec3();
                let q = m.apply(effectcraft_geom::vec2(c[0], c[1]));
                KV::Vec3([q.x, q.y, c[2]])
            });
        }
        let (dr, k) = (self.rotation, self.scale);
        if let Some(pr) = tr.get_mut("rotation") {
            map(pr, &|v| KV::Scalar(v.as_f64() + dr));
        }
        if let Some(pr) = tr.get_mut("scale") {
            map(pr, &|v| {
                let c = v.as_vec3();
                KV::Vec3([c[0] * k[0], c[1] * k[1], c[2]])
            });
        }
    }
}

/// A uniform factor commutes with authored axis Rotation and Scale; prepend its orthogonal
/// rotation to static Orientation using the existing Rz * Ry * Rx order.
fn uniform_parent_pose(m: effectcraft_geom::Mat4, orientation: [f64; 3]) -> Option<([f64; 3], f64)> {
    use effectcraft_geom::{Mat4, Vec3, vec3};
    if !orientation.iter().all(|v| v.is_finite()) {
        return None;
    }
    let a = m.0;
    let columns = [vec3(a[0][0], a[1][0], a[2][0]), vec3(a[0][1], a[1][1], a[2][1]), vec3(a[0][2], a[1][2], a[2][2])];
    let magnitude = columns[0].x.hypot(columns[0].y).hypot(columns[0].z);
    if !magnitude.is_finite() || magnitude <= 0.0 {
        return None;
    }
    let c = columns.map(|c| c / magnitude);
    if c.iter().any(|c| (c.dot(*c) - 1.0).abs() > 1e-9) || c[0].dot(c[1]).abs() > 1e-9 || c[0].dot(c[2]).abs() > 1e-9 || c[1].dot(c[2]).abs() > 1e-9 {
        return None;
    }
    let cross = vec3(c[1].y * c[2].z - c[1].z * c[2].y, c[1].z * c[2].x - c[1].x * c[2].z, c[1].x * c[2].y - c[1].y * c[2].x);
    let factor = if c[0].dot(cross) < 0.0 { -magnitude } else { magnitude };
    let mut rotation = Mat4::IDENTITY;
    for row in 0..3 {
        for col in 0..3 {
            rotation.0[row][col] = a[row][col] / factor;
        }
    }
    if (0..3).all(|row| (0..3).all(|col| (rotation.0[row][col] - if row == col { 1.0 } else { 0.0 }).abs() < 1e-12)) {
        return Some((orientation, factor));
    }
    let r = (rotation * Mat4::orientation(Vec3::from(orientation))).0;
    // atan2 rather than asin: near ±90 degrees asin loses precision, while cos(y) stays exact.
    let cy = r[0][0].hypot(r[1][0]);
    let y = (-r[2][0]).atan2(cy);
    let (x, z) = if cy > 1e-9 { (r[2][1].atan2(r[2][2]), r[1][0].atan2(r[0][0])) } else { ((-r[1][2]).atan2(r[1][1]), 0.0) };
    Some(([x.to_degrees(), y.to_degrees(), z.to_degrees()], factor))
}

/// A parent change retains 2D transforms and the world-space anchor of 3D layers (Position,
/// and Point of Interest for two-node cameras and lights). Position is
/// re-expressed in the new parent's space (`None`: the composition), evaluated at the current
/// time.
pub(crate) fn parent_fixes(s: &Session, cid: ItemId, ids: &[LayerId], parent: Option<LayerId>) -> Vec<ParentFix> {
    let Some(comp) = s.project.comp(cid) else { return vec![] };
    let ectx =
        effectcraft_render::EvalCtx { project: &s.project, comp_id: cid, comp, time: s.time(), expr: s.expr.as_deref(), footage: Some(s.footage.as_ref()) };
    let space = |id: Option<LayerId>| -> effectcraft_geom::Mat3 {
        id.and_then(|i| comp.layer(i)).filter(|l| !l.is_3d()).map(|l| ectx.layer_to_comp(l).0).unwrap_or(effectcraft_geom::Mat3::IDENTITY)
    };
    let decompose = |m: &effectcraft_geom::Mat3| {
        let a = m.0;
        let sx = (a[0][0] * a[0][0] + a[1][0] * a[1][0]).sqrt();
        let det = a[0][0] * a[1][1] - a[0][1] * a[1][0];
        (a[1][0].atan2(a[0][0]).to_degrees(), sx, if sx > 1e-12 { det / sx } else { 0.0 })
    };
    let new_m = space(parent);
    let Some(inv) = new_m.inverse() else { return vec![] };
    let (rn, sxn, syn) = decompose(&new_m);
    let mut fixes: Vec<_> = comp
        .layers
        .iter()
        .filter(|l| ids.contains(&l.id) && !l.is_3d() && l.parent != parent)
        .map(|l| {
            let old_m = space(l.parent);
            let (ro, sxo, syo) = decompose(&old_m);
            let scale = [if sxn.abs() > 1e-12 { sxo / sxn } else { 1.0 }, if syn.abs() > 1e-12 { syo / syn } else { 1.0 }];
            ParentFix { layer: l.id, m: inv * old_m, rotation: ro - rn, scale, position_3d: None, pose_3d: None }
        })
        .collect();
    let space_3d = |id: Option<LayerId>| {
        id.and_then(|id| comp.layer(id)).map_or(effectcraft_geom::Mat4::IDENTITY, |layer| {
            // Parent transforms do not inherit the parent's source pixel-aspect stretch.
            let world = ectx.world_matrix(layer);
            let par = ectx.par_ratio(layer);
            let a = layer.transform().map(|tr| ectx.v3(layer, tr, "anchor", [0.0; 3])).unwrap_or([0.0; 3]);
            let anchor = effectcraft_geom::vec3(a[0], a[1], if layer.is_3d() { a[2] } else { 0.0 });
            world
                * effectcraft_geom::Mat4::translate(anchor)
                * effectcraft_geom::Mat4::scale(effectcraft_geom::vec3(1.0 / par, 1.0, 1.0))
                * effectcraft_geom::Mat4::translate(-anchor)
        })
    };
    if let Some(inv) = space_3d(parent).inverse().filter(|m| m.0.iter().flatten().all(|v| v.is_finite())) {
        // Cameras and lights compensate like 3D layers (Position, and Point of Interest for
        // two-node rigs), so parenting keeps the world frame (#1010, #1020). They have no
        // Scale, so `pose_3d` below stays `None` and Orientation is kept as authored.
        for layer in comp.layers.iter().filter(|l| ids.contains(&l.id) && l.is_3d() && l.parent != parent) {
            let m = inv * space_3d(layer.parent);
            if m.0.iter().flatten().all(|v| v.is_finite()) {
                let pose_3d = layer.transform().and_then(|tr| {
                    let orientation = tr.get("orientation")?;
                    let scale = tr.get("scale")?;
                    // Animated Euler Orientation and auto-orient cannot be re-expressed by
                    // prepending a constant Euler value. Expression-driven values stay authored.
                    if !orientation.keys.is_empty()
                        || orientation.expr.is_some()
                        || scale.expr.is_some()
                        || layer.auto_orient != effectcraft_project::AutoOrient::Off
                    {
                        return None;
                    }
                    let pose = uniform_parent_pose(m, orientation.value.as_vec3())?;
                    let factor = pose.1;
                    let values_finite = std::iter::once(&scale.value)
                        .chain(scale.keys.iter().map(|key| &key.value))
                        .all(|value| value.as_vec3().iter().all(|v| (v * factor).is_finite()));
                    let speeds_finite = scale.keys.iter().flat_map(|key| key.in_ease.iter().chain(&key.out_ease)).all(|ease| (ease.speed * factor).is_finite());
                    (values_finite && speeds_finite).then_some(pose)
                });
                fixes.push(ParentFix { layer: layer.id, m: effectcraft_geom::Mat3::IDENTITY, rotation: 0.0, scale: [1.0; 2], position_3d: Some(m), pose_3d });
            }
        }
    }
    fixes
}

/// Validate the proposed ancestor chain before any authored edit, including when compensation
/// is disabled. Bound both unique ancestors and linear lookup/selection work on loaded graphs.
fn check_parent_chain(comp: &Comp, ids: &[LayerId], parent: Option<LayerId>, max_visits: usize, max_work: usize) -> Result<()> {
    let mut seen = std::collections::HashSet::new();
    let mut work = 0usize;
    let mut cur = parent;
    while let Some(id) = cur {
        if seen.len() >= max_visits {
            return Err(bad("layer.setParent", "parent chain exceeds the safety visit limit"));
        }
        if !seen.insert(id) {
            return Err(bad("layer.setParent", "parent chain contains an existing cycle"));
        }
        for selected in ids {
            if work >= max_work {
                return Err(bad("layer.setParent", "parent chain exceeds the safety work limit"));
            }
            work += 1;
            if *selected == id {
                return Err(bad("layer.setParent", "parenting would create a cycle"));
            }
        }
        let mut ancestor = None;
        for layer in &comp.layers {
            if work >= max_work {
                return Err(bad("layer.setParent", "parent chain exceeds the safety work limit"));
            }
            work += 1;
            if layer.id == id {
                ancestor = Some(layer);
                break;
            }
        }
        let layer = ancestor.ok_or_else(|| bad("layer.setParent", "parent chain refers to a missing layer"))?;
        cur = layer.parent;
    }
    Ok(())
}

fn set_parent(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, ids) = layers_p(s, p)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let parent = match p.get("parent") {
        None | Some(Value::Null) => None,
        Some(v) => Some(resolve_layer(comp, v).ok_or_else(|| bad("layer.setParent", "no such parent layer"))?),
    };
    // A loaded cycle need not include a selected child. Validate before compensation or edit;
    // do not impose the renderer's 64-ancestor truncation as a new ordinary hierarchy limit.
    check_parent_chain(comp, &ids, parent, 4096, 1_048_576)?;
    // Like AE's pick-whip, parenting keeps the layer where it is: its transform is re-expressed
    // in the new parent's space (Position, Rotation, Scale), unless `compensate: false`.
    let compensate = b_p(p, "compensate").unwrap_or(true);
    let fixes = if compensate { parent_fixes(s, cid, &ids, parent) } else { vec![] };
    s.edit("Parent", None, |proj, _| {
        let comp = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        for l in comp.layers.iter_mut().filter(|l| ids.contains(&l.id)) {
            l.parent = parent;
            if let Some(f) = fixes.iter().find(|f| f.layer == l.id) {
                f.apply(l);
            }
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

fn timing(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, ids) = layers_p(s, p)?;
    let t = s.time();
    let op = str_p(p, "op").unwrap_or("set").to_string();
    // Layer times stay on frame boundaries of the comp (as in AE).
    let fr = s.project.comp(cid).ok_or(EngineError::NoComp)?.frame_rate;
    let snap = |k: &str| f_p(p, k).map(|v| fr.snap_nearest(Tick::from_seconds_f64(v)));
    let (delta, start, inp, outp) = (snap("delta"), snap("start"), snap("in"), snap("out"));
    // Relative trims: each layer moves its own in/out point by the same amount, so dragging an
    // edge of several selected layers trims them all together (as in AE).
    let (in_delta, out_delta) = (snap("inDelta"), snap("outDelta"));
    s.edit("Layer Timing", merge_p(p), |proj, _| {
        let comp = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        let fd = comp.frame_duration();
        let comp_dur = comp.duration;
        for l in comp.layers.iter_mut().filter(|l| ids.contains(&l.id)) {
            let shift = |l: &mut Layer, d: Tick| {
                l.start_time += d;
                l.in_point += d;
                l.out_point += d;
            };
            match op.as_str() {
                "moveInToTime" => {
                    let d = t - l.in_point;
                    shift(l, d);
                }
                "moveOutToTime" => {
                    let d = t - l.out_point;
                    shift(l, d);
                }
                "moveInToStart" => {
                    let d = Tick::ZERO - l.in_point;
                    shift(l, d);
                }
                "moveOutToEnd" => {
                    let d = comp_dur - l.out_point;
                    shift(l, d);
                }
                "trimInToTime" => l.in_point = t.min(l.out_point - fd),
                "trimOutToTime" => l.out_point = t.max(l.in_point + fd),
                _ => {
                    if let Some(d) = delta {
                        shift(l, d);
                    }
                    if let Some(st) = start {
                        let d = st - l.start_time;
                        shift(l, d);
                    }
                    if let Some(i) = inp {
                        l.in_point = i.min(l.out_point - fd);
                    }
                    if let Some(o) = outp {
                        l.out_point = o.max(l.in_point + fd);
                    }
                    // Stops at the comp start/end (a point already past it doesn't jump onto
                    // it), keeping at least one frame of layer.
                    if let Some(d) = in_delta {
                        l.in_point = (l.in_point + d).max(Tick::ZERO.min(l.in_point)).min(l.out_point - fd);
                    }
                    if let Some(d) = out_delta {
                        l.out_point = (l.out_point + d).min(comp_dur.max(l.out_point)).max(l.in_point + fd);
                    }
                }
            }
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// `p` with `key` set.
fn with(p: &Value, key: &str, v: Value) -> Value {
    let mut p = if p.is_object() { p.clone() } else { json!({}) };
    p[key] = v;
    p
}

/// Move the layers `frames` frames in time (Alt+Page Up / Page Down, with Shift ten frames).
fn move_frames(s: &mut Session, p: &Value, frames: i64) -> Result<Value> {
    let (cid, _) = layers_p(s, p)?;
    let delta = s.project.comp(cid).ok_or(EngineError::NoComp)?.frame_rate.tick_of(frames).seconds();
    timing(s, &with(p, "delta", json!(delta)))
}

/// Slip edit: move a layer's source (footage or precomp) under its fixed in and out points, like
/// dragging the source-duration bar in After Effects. `delta` in seconds or `frames`; positive
/// moves the source later. Clamped so the source still covers the in–out span. Keyframes and
/// layer markers are in layer time, so they travel with the source.
fn slip(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, ids) = layers_p(s, p)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let fr = comp.frame_rate;
    let delta = match (p.get("frames").and_then(Value::as_i64), f_p(p, "delta")) {
        (Some(f), _) => fr.tick_of(f),
        (None, Some(d)) => fr.snap_nearest(Tick::from_seconds_f64(d)),
        _ => return Err(bad("layer.slip", "delta (seconds) or frames required")),
    };
    // Allowed shift per layer: the source span [lo, hi] in comp time must keep covering in..out.
    let mut shifts: Vec<(LayerId, Tick)> = Vec::new();
    for l in comp.layers.iter().filter(|l| ids.contains(&l.id)) {
        if l.props.get("timeRemap").is_some() {
            continue;
        }
        let dur = match &l.source {
            LayerSource::Footage { item } => match s.project.item(*item).map(|i| &i.kind) {
                Some(ItemKind::Footage(f)) if f.kind != effectcraft_project::FootageKind::Still => Some(f.looped_duration()),
                _ => None,
            },
            LayerSource::Comp { item } => s.project.comp(*item).map(|c| c.duration),
            _ => None,
        };
        let Some(dur) = dur else { continue };
        let (a, b) = (l.comp_time(Tick::ZERO), l.comp_time(dur));
        let (lo, hi) = (a.min(b), a.max(b));
        let (min_d, max_d) = (l.out_point - hi, l.in_point - lo);
        if min_d > max_d {
            continue;
        }
        shifts.push((l.id, delta.max(min_d).min(max_d)));
    }
    if shifts.is_empty() {
        return Err(bad("layer.slip", "select a footage or precomp layer (without time remapping)"));
    }
    s.edit("Slip Edit", merge_p(p), |proj, _| {
        let comp = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        for (id, d) in &shifts {
            if let Some(l) = comp.layer_mut(*id) {
                l.start_time += *d;
            }
        }
        Ok(())
    })?;
    Ok(json!({"slipped": shifts.iter().map(|(id, d)| json!({"layer": id.0, "delta": d.seconds()})).collect::<Vec<_>>()}))
}

fn arrange(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, ids) = layers_p(s, p)?;
    let ids = unlocked(s, cid, ids, "layer.arrange")?;
    let how = str_p(p, "to").unwrap_or("front").to_string();
    let index = p.get("index").and_then(Value::as_u64).map(|i| i as usize);
    // `above`: put the layers right above that one (the timeline's drag to reorder).
    let above = match p.get("above") {
        Some(v) => {
            let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
            let a = resolve_layer(comp, v).ok_or_else(|| bad("layer.arrange", "no such layer for `above`"))?;
            if ids.contains(&a) {
                return Err(bad("layer.arrange", "`above` must be a layer that isn't being moved"));
            }
            Some(a)
        }
        None => None,
    };
    s.edit("Arrange Layers", None, |proj, _| {
        let comp = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        // Bring Forward / Send Backward move each layer one step past the next layer that isn't
        // moving (a run of selected layers moves as one; one already at the end stays).
        if above.is_none() && index.is_none() && matches!(how.as_str(), "forward" | "backward") {
            let picked = |l: &Layer| ids.contains(&l.id);
            let n = comp.layers.len();
            if how == "forward" {
                for i in 1..n {
                    if picked(&comp.layers[i]) && !picked(&comp.layers[i - 1]) {
                        let run_end = (i..n).find(|j| !picked(&comp.layers[*j])).unwrap_or(n);
                        comp.layers[i - 1..run_end].rotate_left(1);
                    }
                }
            } else {
                for i in (0..n.saturating_sub(1)).rev() {
                    if picked(&comp.layers[i]) && !picked(&comp.layers[i + 1]) {
                        let run_start = (0..=i).rev().find(|j| !picked(&comp.layers[*j])).map_or(0, |j| j + 1);
                        comp.layers[run_start..=i + 1].rotate_right(1);
                    }
                }
            }
            return Ok(());
        }
        let mut picked: Vec<Layer> = Vec::new();
        comp.layers.retain(|l| {
            if ids.contains(&l.id) {
                picked.push(l.clone());
                false
            } else {
                true
            }
        });
        let at = match (how.as_str(), index) {
            _ if above.is_some() => comp.layers.iter().position(|l| Some(l.id) == above).unwrap_or(comp.layers.len()),
            (_, Some(i)) => i.saturating_sub(1).min(comp.layers.len()),
            ("back", _) => comp.layers.len(),
            _ => 0,
        };
        for (k, l) in picked.into_iter().enumerate() {
            comp.layers.insert((at + k).min(comp.layers.len()), l);
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Layer ▸ Pre-compose. `mode: move` (default; "Move all attributes into the new composition")
/// nests the selected layers as they are; `mode: leave` ("Leave all attributes in", one footage,
/// solid or precomp layer) nests only the layer's source, sized to it, and keeps the layer's
/// transform, masks, effects and timing in the current comp. `adjustDuration` (move) trims the
/// new comp to the selected layers' span; `open` opens the new comp.
fn precompose(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "layer.precompose";
    let (cid, ids) = layers_p(s, p)?;
    if ids.is_empty() {
        return Err(bad(c, "no layers"));
    }
    let name = str_p(p, "name").unwrap_or("Pre-comp 1").to_string();
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?.clone();
    let leave = match str_p(p, "mode").unwrap_or("move") {
        "move" => false,
        "leave" => true,
        x => return Err(bad(c, format!("mode: move|leave, got `{x}`"))),
    };
    let adjust = b_p(p, "adjustDuration").unwrap_or(false);
    if leave {
        let [lid] = ids[..] else { return Err(bad(c, "\"Leave all attributes\" needs exactly one layer")) };
        let layer = comp.layer(lid).ok_or(EngineError::NoComp)?.clone();
        let Some(src_item) = layer.source.item() else {
            return Err(bad(c, "\"Leave all attributes\" is available for footage, solid and precomp layers only"));
        };
        let (w, h) = effectcraft_render::source_size(&s.project, &layer);
        let (w, h) = if w == 0 || h == 0 { (comp.width, comp.height) } else { (w, h) };
        let dur = match s.project.item(src_item).map(|i| &i.kind) {
            Some(ItemKind::Comp(nc)) => nc.duration,
            Some(ItemKind::Footage(f)) if f.duration > Tick(0) => f.duration,
            _ => comp.duration,
        };
        let new = s.edit("Pre-compose", None, |proj, st| {
            let mut inner = comp.nested_like(w, h, dur);
            let mut il = build::layer(proj, &inner, &layer.name, layer.source.clone(), (w, h), Some(dur));
            il.name = layer.name.clone();
            inner.layers.push(il);
            let iid = proj.add_item(&name, Label::Sandstone, None, ItemKind::Comp(inner.into()));
            let l = layer_mut(proj, cid, lid)?;
            l.source = LayerSource::Comp { item: iid };
            l.name = name.clone();
            st.selected_layers = vec![lid];
            Ok((iid, lid))
        })?;
        if b_p(p, "open").unwrap_or(false) {
            s.open_comp(new.0);
        }
        return Ok(json!({"comp": new.0.0, "layer": new.1.0}));
    }
    let span = {
        let sel: Vec<&effectcraft_project::Layer> = comp.layers.iter().filter(|l| ids.contains(&l.id)).collect();
        let a = sel.iter().map(|l| l.in_point).min().unwrap_or(Tick(0)).max(Tick(0));
        let b = sel.iter().map(|l| l.out_point).max().unwrap_or(comp.duration).min(comp.duration);
        (a, b)
    };
    let (offset, dur) = if adjust && span.1 > span.0 { (span.0, span.1 - span.0) } else { (Tick(0), comp.duration) };
    let new = s.edit("Pre-compose", None, |proj, st| {
        let mut inner = comp.nested_like(comp.width, comp.height, dur);
        inner.layers = comp.layers.iter().filter(|l| ids.contains(&l.id)).cloned().collect();
        for l in &mut inner.layers {
            if l.parent.is_some_and(|p| !ids.contains(&p)) {
                l.parent = None;
            }
            if l.track_matte.is_some_and(|m| !ids.contains(&m.layer)) {
                l.track_matte = None;
            }
            l.start_time -= offset;
            l.in_point -= offset;
            l.out_point -= offset;
        }
        let iid = proj.add_item(&name, Label::Sandstone, None, ItemKind::Comp(inner.into()));
        let mut l = build::layer(proj, &comp, &name, LayerSource::Comp { item: iid }, (comp.width, comp.height), Some(dur));
        l.name = name.clone();
        l.start_time = offset;
        l.in_point = offset;
        l.out_point = offset + dur;
        let c = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        let at = c.layers.iter().position(|l| ids.contains(&l.id)).unwrap_or(0);
        c.layers.retain(|l| !ids.contains(&l.id));
        let lid = l.id;
        c.layers.insert(at.min(c.layers.len()), l);
        st.selected_layers = vec![lid];
        Ok((iid, lid))
    })?;
    if b_p(p, "open").unwrap_or(false) {
        s.open_comp(new.0);
    }
    Ok(json!({"comp": new.0.0, "layer": new.1.0}))
}

fn add_mask(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, lid) = layer_p(s, p, "layer.addMask")?;
    let layer = s.project.comp(cid).and_then(|c| c.layer(lid)).ok_or(EngineError::NoComp)?.clone();
    let (w, h) = effectcraft_render::source_size(&s.project, &layer);
    let (w, h) = if w == 0 { (400.0, 300.0) } else { (w as f64, h as f64) };
    let shape = str_p(p, "shape").unwrap_or("rect");
    let r = p.get("rect").and_then(Value::as_array).map(|a| [0, 1, 2, 3].map(|i| a.get(i).and_then(Value::as_f64).unwrap_or(0.0)));
    let (cx, cy, rw, rh) = match r {
        Some([x, y, rw, rh]) => (x + rw / 2.0, y + rh / 2.0, rw, rh),
        None => (if w > 0.0 { w / 2.0 } else { 0.0 }, h / 2.0, w * 0.6, h * 0.6),
    };
    // The shape tools' masks: rectangle, ellipse, rounded rectangle, polygon or star.
    let path = super::shape_tool::mask_path(shape, [cx, cy], [rw, rh])
        .ok_or_else(|| bad("layer.addMask", format!("unknown shape `{shape}`; one of rect, ellipse, rounded, polygon, star")))?;
    let mode = str_p(p, "mode").and_then(MaskMode::from_name).unwrap_or(MaskMode::Add);
    let cycle = s.prefs.appearance.cycle_mask_colors;
    let uid = s.edit("New Mask", None, |proj, _| {
        let mut next = proj.next_id;
        let l = layer_mut(proj, cid, lid)?;
        let masks = l.props.sub_mut("masks").ok_or_else(|| bad("layer.addMask", "this layer can't have masks"))?;
        let n = masks.children.len();
        let g = build::mask(&mut Ids(&mut next), &format!("Mask {}", n + 1), path, mode, crate::prefs::mask_color(cycle, n));
        let uid = g.uid;
        masks.children.push(g.into());
        proj.next_id = next;
        Ok(uid)
    })?;
    Ok(json!({"mask": uid}))
}

fn mask_props(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, lid) = layer_p(s, p, "layer.setMask")?;
    let target = p.get("mask").cloned().unwrap_or(json!(1));
    let mode = str_p(p, "mode").and_then(MaskMode::from_name);
    let inverted = b_p(p, "inverted");
    s.edit("Mask Settings", None, |proj, _| {
        let l = layer_mut(proj, cid, lid)?;
        let masks = l.props.sub_mut("masks").ok_or_else(|| bad("layer.setMask", "the layer has no masks (add one with layer.addMask)"))?;
        let g = match &target {
            Value::Number(n) => {
                let n = n.as_u64().unwrap_or(1);
                if let Some(i) = masks.children.iter().position(|c| c.uid() == n) {
                    masks.children[i].as_group_mut()
                } else {
                    masks.children.get_mut(n.saturating_sub(1) as usize).and_then(|c| c.as_group_mut())
                }
            }
            Value::String(name) => masks.children.iter_mut().find(|c| c.name() == name).and_then(|c| c.as_group_mut()),
            _ => None,
        }
        .ok_or_else(|| bad("layer.setMask", "no such mask"))?;
        if let GroupKind::Mask { mode: m, inverted: inv, .. } = &mut g.kind {
            if let Some(mm) = mode {
                *m = mm;
            }
            if let Some(i) = inverted {
                *inv = i;
            }
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

fn add_shape_item(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, lid) = layer_p(s, p, "layer.addShapeItem")?;
    let kind = str_p(p, "kind").ok_or_else(|| bad("layer.addShapeItem", "missing `kind`"))?.to_string();
    // `group`: a uid or a property path (`contents/group`).
    let group_uid = match p.get("group") {
        Some(Value::String(path)) => Some(
            s.project
                .comp(cid)
                .and_then(|c| c.layer(lid))
                .and_then(|l| l.props.group(path))
                .map(|g| g.uid)
                .ok_or_else(|| bad("layer.addShapeItem", format!("no group `{path}`")))?,
        ),
        v => v.and_then(Value::as_u64),
    };
    let uid = s.edit("Add Shape Item", None, |proj, _| {
        let mut next = proj.next_id;
        let mut ids = Ids(&mut next);
        // Front / Bevel / Side / Back ▸ property: into the group's surface item (one per group).
        if let Some((surface, prop)) = build::shape_surface_kind(&kind) {
            let target = shape_item_target(proj, cid, lid, group_uid)?;
            let item = match target.children.iter().position(|c| matches!(c, Node::Group(g) if g.match_id == surface)) {
                Some(i) => i,
                None => {
                    let g = build::shape_surface(&mut ids, surface).ok_or_else(|| bad("layer.addShapeItem", format!("unknown kind `{kind}`")))?;
                    target.children.push(g.into());
                    target.children.len() - 1
                }
            };
            let Some(Node::Group(g)) = target.children.get_mut(item) else { return Err(bad("layer.addShapeItem", "no such group")) };
            if g.get(prop).is_none() {
                let p = build::shape_surface_prop(&mut ids, prop).ok_or_else(|| bad("layer.addShapeItem", format!("unknown kind `{kind}`")))?;
                g.children.push(p.into());
            }
            let uid = g.uid;
            proj.next_id = next;
            return Ok(uid);
        }
        let g = match kind.as_str() {
            "group" => build::shape_group(&mut ids, "Group 1", vec![]),
            "rect" | "rectangle" => build::shape_rect(&mut ids, [200.0, 200.0], [0.0, 0.0], 0.0),
            "ellipse" => build::shape_ellipse(&mut ids, [200.0, 200.0], [0.0, 0.0]),
            "star" => build::shape_star(&mut ids, true, 5.0, [0.0, 0.0], 100.0, 50.0),
            "polygon" => build::shape_star(&mut ids, false, 5.0, [0.0, 0.0], 100.0, 0.0),
            "fill" => build::shape_fill(&mut ids, [1.0, 0.0, 0.0, 1.0]),
            "stroke" => build::shape_stroke(&mut ids, [1.0, 1.0, 1.0, 1.0], 2.0),
            "path" => build::shape_path(&mut ids, Default::default()),
            "gfill" | "gradientFill" => build::shape_gradient_fill(&mut ids, false, [-100.0, 0.0], [100.0, 0.0], Default::default()),
            "gstroke" | "gradientStroke" => build::shape_gradient_stroke(&mut ids, false, [-100.0, 0.0], [100.0, 0.0], Default::default(), 2.0),
            "trim" | "trimPaths" => build::shape_trim(&mut ids, 0.0, 100.0, 0.0),
            "repeater" => build::shape_repeater(&mut ids, 3.0, [100.0, 0.0]),
            k => build::shape_simple_op(&mut ids, k).ok_or_else(|| bad("layer.addShapeItem", format!("unknown kind `{k}`")))?,
        };
        let uid = g.uid;
        proj.next_id = next;
        shape_item_target(proj, cid, lid, group_uid)?.children.push(g.into());
        Ok(uid)
    })?;
    // The new item's property path, ready for prop.set / add_keyframe (`contents/trim#2/end`).
    let path = s.project.comp(cid).and_then(|c| c.layer(lid)).and_then(|l| l.props.match_path_of(uid));
    Ok(json!({"uid": uid, "path": path}))
}

/// The contents a new shape item goes into: the shape group `group_uid` (its Contents), else the
/// layer's Contents.
fn shape_item_target(proj: &mut Project, cid: ItemId, lid: LayerId, group_uid: Option<u64>) -> Result<&mut PropGroup> {
    let l = layer_mut(proj, cid, lid)?;
    let contents = l.props.sub_mut("contents").ok_or_else(|| bad("layer.addShapeItem", "not a shape layer"))?;
    Ok(match group_uid {
        Some(u) => contents
            .find_group_mut(u)
            .and_then(|g| if g.match_id == "contents" { Some(g) } else { g.sub_mut("contents") })
            .ok_or_else(|| bad("layer.addShapeItem", "no such group"))?,
        None => contents,
    })
}

fn set_text(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, lid) = layer_p(s, p, "layer.setText")?;
    // The Character panel's recent fonts (Settings ▸ Type ▸ Number of Recent Fonts to Display).
    if let Some(f) = str_p(p, "font") {
        s.prefs.push_recent_font(f);
        s.save_prefs();
    }
    let t = s.time();
    let range = text_range_p(p);
    // Character attributes with only a caret (no selected text) set the insertion style.
    if let Some(r) = range.clone().filter(|r| r.is_empty() && str_p(p, "text").is_none())
        && s.state.text_edit.as_ref().is_some_and(|e| e.layer == lid)
        && let Some(doc) = super::text_edit::layer_doc(s, lid)
        && doc.char_len() > 0
        && let Some(obj) = p.as_object()
        && let Some(e) = s.state.text_edit.as_mut()
    {
        let mut st = e.pending.clone().unwrap_or_else(|| doc.insertion_style(r.start));
        let mut any = false;
        for (k, v) in obj {
            any |= effectcraft_keyframe::text_doc::apply_char_attr(&mut st, k, v).map_err(|m| bad("layer.setText", m))?;
        }
        if any {
            e.pending = Some(st);
        }
        s.bump();
    }
    s.edit("Edit Text", merge_p(p), |proj, _| {
        let l = layer_mut(proj, cid, lid)?;
        let lt = l.layer_time(t);
        let pr = l.props.prop_mut("text/sourceText").ok_or_else(|| bad("layer.setText", "not a text layer"))?;
        let mut doc = match pr.value_at(lt) {
            KV::Text(d) => *d,
            KV::Str(t) => TextDoc::plain(&t),
            _ => TextDoc::default(),
        };
        apply_text_params("layer.setText", p, &mut doc, range.clone())?;
        pr.set_value_at(lt, KV::Text(Box::new(doc.clone())));
        if let Some(first) = doc.text.lines().next()
            && str_p(p, "text").is_some()
            && !first.is_empty()
            && (l.name.starts_with("Text") || l.name.is_empty())
        {
            l.name = first.chars().take(40).collect();
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Move Position by `(dx, dy)`, including every key. Separated dimensions move X and Y as well
/// as the combined property the channels were split from.
pub(crate) fn offset_position(tr: &mut PropGroup, dx: f64, dy: f64) {
    if dx == 0.0 && dy == 0.0 {
        return;
    }
    if tr.get("positionX").is_some() {
        for (name, d) in [("positionX", dx), ("positionY", dy)] {
            if let Some(pr) = tr.get_mut(name) {
                pr.value = KV::Scalar(pr.value.as_f64() + d);
                for k in &mut pr.keys {
                    k.value = KV::Scalar(k.value.as_f64() + d);
                }
            }
        }
    }
    if let Some(pr) = tr.get_mut("position") {
        let shift = |v: &KV| match v {
            KV::Vec2(a) => KV::Vec2([a[0] + dx, a[1] + dy]),
            other => {
                let a = other.as_vec3();
                KV::Vec3([a[0] + dx, a[1] + dy, a[2]])
            }
        };
        pr.value = shift(&pr.value);
        for k in &mut pr.keys {
            k.value = shift(&k.value);
        }
    }
}

/// Set a layer's Position at layer time `lt` to `f(current)`: X/Y/Z Position when its dimensions
/// are separated.
pub(crate) fn update_position(tr: &mut PropGroup, lt: Tick, f: impl Fn([f64; 3]) -> [f64; 3]) {
    const SEPARATE: [&str; 3] = ["positionX", "positionY", "positionZ"];
    if tr.get("positionX").is_some() {
        let v = f(SEPARATE.map(|m| tr.get(m).map(|p| p.value_at(lt).as_f64()).unwrap_or(0.0)));
        for (m, x) in SEPARATE.iter().zip(v) {
            if let Some(pr) = tr.get_mut(m) {
                pr.set_value_at(lt, KV::Scalar(x));
            }
        }
    } else if let Some(pr) = tr.get_mut("position") {
        let v = f(pr.value_at(lt).as_vec3());
        pr.set_value_at(lt, KV::Vec3(v));
    }
}

/// A Transform property's default (Reset) by match id: the anchor at the centre of the source
/// (`w`×`h`), the position at the comp's (`cw`×`ch`), 100% scale and opacity, no rotation.
pub(crate) fn transform_default(m: &str, (w, h): (f64, f64), (cw, ch): (f64, f64)) -> Option<KV> {
    Some(match m {
        "anchor" => KV::Vec3([w / 2.0, h / 2.0, 0.0]),
        "position" => KV::Vec3([cw / 2.0, ch / 2.0, 0.0]),
        "positionX" => KV::Scalar(cw / 2.0),
        "positionY" => KV::Scalar(ch / 2.0),
        "positionZ" | "rotation" | "rotationX" | "rotationY" => KV::Scalar(0.0),
        "scale" => KV::Vec3([100.0; 3]),
        "orientation" => KV::Vec3([0.0; 3]),
        "opacity" => KV::Scalar(100.0),
        _ => return None,
    })
}

fn transform_op(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, ids) = layers_p(s, p)?;
    let ids = unlocked(s, cid, ids, "layer.transform")?;
    let op = str_p(p, "op").unwrap_or("reset").to_string();
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?.clone();
    let t = s.time();
    let sizes: Vec<(LayerId, (u32, u32))> =
        comp.layers.iter().filter(|l| ids.contains(&l.id)).map(|l| (l.id, effectcraft_render::source_size(&s.project, l))).collect();
    s.edit("Transform", None, |proj, _| {
        for (lid, (w, h)) in &sizes {
            let l = layer_mut(proj, cid, *lid)?;
            let lt = l.layer_time(t);
            let (cw, ch) = (comp.width as f64, comp.height as f64);
            let (w, h) = (*w as f64, *h as f64);
            let Some(tr) = l.transform_mut() else { continue };
            let set = |tr: &mut PropGroup, m: &str, v: KV| {
                if let Some(pr) = tr.get_mut(m) {
                    pr.set_value_at(lt, v);
                }
            };
            match op.as_str() {
                "reset" => {
                    for m in ["anchor", "scale", "rotation", "rotationX", "rotationY", "orientation", "opacity"] {
                        if let Some(v) = transform_default(m, (w, h), (cw, ch)) {
                            set(tr, m, v);
                        }
                    }
                    update_position(tr, lt, |_| [cw / 2.0, ch / 2.0, 0.0]);
                }
                // Centred in the frame; a 3D layer keeps its depth.
                "center" => update_position(tr, lt, |p| [cw / 2.0, ch / 2.0, p[2]]),
                "fit" | "fitWidth" | "fitHeight" if w > 0.0 && h > 0.0 => {
                    let (sx, sy) = match op.as_str() {
                        "fitWidth" => (cw / w, cw / w),
                        "fitHeight" => (ch / h, ch / h),
                        _ => (cw / w, ch / h),
                    };
                    let z = |m: &str, d: f64| tr.get(m).map(|p| p.value_at(lt).as_vec3()[2]).unwrap_or(d);
                    let (sz, az) = (z("scale", 100.0), z("anchor", 0.0));
                    set(tr, "scale", KV::Vec3([sx * 100.0, sy * 100.0, sz]));
                    update_position(tr, lt, |p| [cw / 2.0, ch / 2.0, p[2]]);
                    set(tr, "anchor", KV::Vec3([w / 2.0, h / 2.0, az]));
                }
                "flipH" | "flipV" => {
                    let cur = tr.get("scale").map(|p| p.value_at(lt).as_vec3()).unwrap_or([100.0; 3]);
                    let v = if op == "flipH" { [-cur[0], cur[1], cur[2]] } else { [cur[0], -cur[1], cur[2]] };
                    set(tr, "scale", KV::Vec3(v));
                }
                _ => {}
            }
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Layer ▸ Layer Settings: cameras and lights open their own settings; solids (and adjustment
/// layers, which are solids) edit their Solid Settings. With `affectAll: false`, a solid other
/// layers also use is copied into a new solid for this layer instead of being changed for all of
/// them (After Effects' "Affect all layers that use this solid"). `name` renames the layer and
/// its solid.
fn layer_settings(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, lid) = layer_p(s, p, "layer.settings")?;
    // Cameras and lights open their own settings (Camera Settings / Light Settings).
    match s.project.comp(cid).and_then(|c| c.layer(lid)).map(|l| l.source.clone()) {
        Some(LayerSource::Camera) => return s.execute("layer.cameraSettings", with_layer(p, lid)),
        Some(LayerSource::Light { .. }) => return s.execute("layer.lightSettings", with_layer(p, lid)),
        _ => {}
    }
    let color = color_p(p, "color");
    let w = p.get("width").and_then(Value::as_u64).map(limits::size);
    let h = p.get("height").and_then(Value::as_u64).map(limits::size);
    let pixel_aspect = pixel_aspect_p(p)?;
    let name = str_p(p, "name").map(str::trim).filter(|n| !n.is_empty()).map(str::to_string);
    let affect_all = p.get("affectAll").and_then(Value::as_bool).unwrap_or(true);
    let solid = s.edit("Layer Settings", None, |proj, _| {
        let src = layer_mut(proj, cid, lid)?.source.clone();
        if let Some(n) = &name {
            layer_mut(proj, cid, lid)?.name = n.clone();
        }
        let LayerSource::Solid { item } = src else { return Ok(None) };
        let shared = proj.comps().flat_map(|(_, c)| &c.layers).filter(|l| l.source == src).count() > 1;
        let target = if shared && !affect_all {
            // This layer gets its own copy of the solid.
            let it = proj.item(item).ok_or_else(|| bad("layer.settings", "the layer's solid is missing"))?.clone();
            let copy = proj.add_item(name.as_deref().unwrap_or(&it.name), it.label, it.parent, it.kind);
            layer_mut(proj, cid, lid)?.source = LayerSource::Solid { item: copy };
            copy
        } else {
            item
        };
        let it = proj.item_mut(target).ok_or_else(|| bad("layer.settings", "the layer's solid is missing"))?;
        if let Some(n) = &name {
            it.name = n.clone();
        }
        if let ItemKind::Solid(so) = &mut it.kind {
            so.color = color.unwrap_or(so.color);
            so.width = w.unwrap_or(so.width);
            so.height = h.unwrap_or(so.height);
            so.pixel_aspect = pixel_aspect.unwrap_or(so.pixel_aspect);
        }
        Ok(Some(target))
    })?;
    Ok(match solid {
        Some(item) => json!({"layer": lid.0, "solid": item.0}),
        None => json!({"layer": lid.0}),
    })
}

/// Layer Settings applies to solids (and adjustment layers), nulls, cameras and lights.
fn has_layer_settings(s: &Session) -> std::result::Result<(), String> {
    has_layers(s)?;
    let l = s.active_comp().zip(s.state.selected_layers.first()).and_then(|(c, l)| c.layer(*l)).ok_or("select a layer first")?;
    match l.source {
        LayerSource::Solid { .. } | LayerSource::Null | LayerSource::Camera | LayerSource::Light { .. } => Ok(()),
        _ => Err("this kind of layer has no settings".into()),
    }
}

fn with_layer(p: &Value, lid: LayerId) -> Value {
    let mut p = if p.is_object() { p.clone() } else { json!({}) };
    p["layer"] = json!(lid.0);
    p
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "layer.newText",
            "Text",
            ["Layer", "New"],
            Some("Cmd+Alt+Shift+T"),
            "{text?, name? (layer name; default the text), position? [x,y], box? [x,y,w,h] (paragraph text, comp space), vertical?, edit? (start editing), font?, style?, size?, fill?, stroke?, applyFill?, applyStroke?, strokeWidth?, tracking?, leading?, baselineShift?, hScale?, vScale?, tsume?, fauxBold?, fauxItalic?, allCaps?, smallCaps?, baseline?, superscript?, subscript?, kerning?, ligatures?, justify?, indentLeft?, indentRight?, indentFirst?, spaceBefore?, spaceAfter?, direction?, composer?, hangingPunctuation?, strokeOverFill?} (attributes as layer.setText)",
            has_comp,
            new_text
        ),
        cmd!("layer.newSolid", "Solid...", ["Layer", "New"], Some("Cmd+Y"), "{name?, color? #hex|[r,g,b], width?, height?, pixelAspect?}", has_comp, new_solid),
        cmd!("layer.newNull", "Null Object", ["Layer", "New"], Some("Cmd+Alt+Shift+Y"), "{name?}", has_comp, new_null),
        cmd!(
            "layer.newShape",
            "Shape Layer",
            ["Layer", "New"],
            None,
            "{kind?: rect|rounded|ellipse|star|polygon|none, name?, size?, fill?, fillType?, fillBlend?, fillOpacity?, stroke?, strokeType?, strokeBlend?, strokeOpacity?, strokeWidth?, position?}",
            has_comp,
            new_shape
        ),
        cmd!("layer.newAdjustment", "Adjustment Layer", ["Layer", "New"], Some("Cmd+Alt+Y"), "{name?}", has_comp, new_adjustment),
        cmd!(
            "layer.settings",
            "Layer Settings...",
            ["Layer"],
            Some("Cmd+Shift+Y"),
            "{layer?, name?, color?, width?, height?, pixelAspect?, affectAll?: bool (default true; false gives this layer its own copy of a solid other layers use)} → {layer, solid?}",
            has_layer_settings,
            layer_settings
        ),
        cmd!(
            "layer.addItem",
            "Add Footage to Comp",
            ["File"],
            Some("Cmd+/"),
            "{item: id|name, time? (s, the In point), index? (1-based stack position; default above the selected layer), position? ([x, y] comp px; default the centre), duration? (s, for a still)}",
            has_comp,
            add_item
        ),
        cmd!("layer.select", "Select Layers", [], None, "{layers: [id|name|#n], add?, toggle?}", has_comp, select),
        cmd!("layer.selectNext", "Select Next Layer", [], Some("Cmd+ArrowDown"), "{add?}", has_comp, select_next),
        cmd!("layer.selectPrevious", "Select Previous Layer", [], Some("Cmd+ArrowUp"), "{add?}", has_comp, select_prev),
        cmd!("layer.extendSelectionNext", "Extend Selection to Next Layer", [], Some("Cmd+Shift+ArrowDown"), "{}", has_comp, |s, _| select_step(
            s,
            &json!({"add": true}),
            1
        )),
        cmd!("layer.extendSelectionPrevious", "Extend Selection to Previous Layer", [], Some("Cmd+Shift+ArrowUp"), "{}", has_comp, |s, _| select_step(
            s,
            &json!({"add": true}),
            -1
        )),
        cmd!(
            "layer.setSwitch",
            "Layer Switch",
            [],
            None,
            "{layers?, switch: video|audio|solo|lock|shy|collapse|quality|fx|frameBlend|motionBlur|adjustment|threeD|guide|preserveTransparency, value?, merge?, others? (the named layers alone: the other layers get the opposite state, or come back)}",
            has_layers,
            set_switch
        ),
        cmd!("layer.rename", "Rename", [], Some("Enter"), "{layer?, name}", has_layers, rename),
        cmd!("layer.setComment", "Layer Comment", [], None, "{layers?, comment}", has_layers, set_comment),
        cmd!("layer.setBlendMode", "Blending Mode", [], None, "{layers?, mode?: Normal|Multiply|Screen|…, step?: ±1}", has_layers, blend),
        cmd!(
            "layer.setTrackMatte",
            "Track Matte",
            [],
            None,
            "{layer?, matte: layer|null, kind?: alpha|alphaInverted|luma|lumaInverted}",
            has_layers,
            track_matte
        ),
        cmd!("layer.setParent", "Parent", [], None, "{layers?, parent: layer|null, compensate?: bool}", has_layers, set_parent),
        cmd!(
            "layer.timing",
            "Layer Timing",
            [],
            None,
            "{layers?, op?: moveInToTime|moveOutToTime|moveInToStart|moveOutToEnd|trimInToTime|trimOutToTime, delta?, start?, in?, out?, inDelta?, outDelta? (seconds, per layer), merge?}",
            has_layers,
            timing
        ),
        cmd!("layer.moveInToStart", "Move Layer In Point to Beginning of Comp", [], Some("Alt+Home"), "{layers?}", has_layers, |s, p| timing(
            s,
            &with(p, "op", json!("moveInToStart"))
        )),
        cmd!("layer.moveOutToEnd", "Move Layer Out Point to End of Comp", [], Some("Alt+End"), "{layers?}", has_layers, |s, p| timing(
            s,
            &with(p, "op", json!("moveOutToEnd"))
        )),
        cmd!("layer.moveEarlier", "Move Layers 1 Frame Earlier", [], Some("Alt+PageUp"), "{layers?}", has_layers, |s, p| move_frames(s, p, -1)),
        cmd!("layer.moveLater", "Move Layers 1 Frame Later", [], Some("Alt+PageDown"), "{layers?}", has_layers, |s, p| move_frames(s, p, 1)),
        cmd!("layer.moveEarlier10", "Move Layers 10 Frames Earlier", [], Some("Alt+Shift+PageUp"), "{layers?}", has_layers, |s, p| move_frames(s, p, -10)),
        cmd!("layer.moveLater10", "Move Layers 10 Frames Later", [], Some("Alt+Shift+PageDown"), "{layers?}", has_layers, |s, p| move_frames(s, p, 10)),
        cmd!("layer.slip", "Slip Edit", [], None, "{layers?, delta? (seconds) | frames?, merge?}", has_layers, slip),
        cmd!("layer.slipBack", "Slip Edit 1 Frame Earlier", [], None, "{layers?}", has_layers, |s, p| {
            let mut p = p.clone();
            p["frames"] = json!(-1);
            slip(s, &p)
        }),
        cmd!("layer.slipForward", "Slip Edit 1 Frame Later", [], None, "{layers?}", has_layers, |s, p| {
            let mut p = p.clone();
            p["frames"] = json!(1);
            slip(s, &p)
        }),
        cmd!("layer.arrange", "Arrange", ["Layer", "Arrange"], None, "{layers?, to: front|forward|backward|back, index?, above?: layer}", has_layers, arrange),
        cmd!(
            "layer.precompose",
            "Pre-compose...",
            ["Layer"],
            Some("Cmd+Shift+C"),
            "{layers?, name?, mode?: move|leave, adjustDuration?, open?}",
            has_layers,
            precompose
        ),
        cmd!(
            "layer.addMask",
            "New Mask",
            ["Layer", "Mask"],
            Some("Cmd+Shift+N"),
            "{layer?, shape?: rect|ellipse|rounded|polygon|star, rect? [x,y,w,h] (layer space; a polygon or star fits its width), mode?}",
            has_layers,
            add_mask
        ),
        cmd!("layer.setMask", "Mask Mode", [], None, "{layer?, mask: index|uid|name, mode?, inverted?}", has_layers, mask_props),
        cmd!(
            "layer.addShapeItem",
            "Add (Shape)",
            [],
            None,
            "{layer?, kind: group|rect|ellipse|star|polygon|path|fill|stroke|gfill|gstroke|trim|repeater|round|offset|pucker|twist|zigzag|wiggle|merge|frontColor|bevelColor|sideColor|backColor, group?: uid|path} → {uid, path} (frontColor…backColor: the group's Front / Bevel / Side / Back item, which colours that surface of every extruded paint in the group in Advanced 3D)",
            has_layers,
            add_shape_item
        ),
        cmd!(
            "layer.setText",
            "Edit Text",
            [],
            None,
            "{layer?, range?: [start, end] (characters; default all), text?, font?, style?, size?, fill?, stroke?, applyFill?, applyStroke?, strokeWidth?, tracking?, leading?: px|\"auto\", baselineShift? px, hScale? %, vScale? %, tsume? %, fauxBold?, fauxItalic?, allCaps?, smallCaps?, baseline?: normal|superscript|subscript, superscript?, subscript?, kerning?: metrics|optical|number, ligatures?, discretionaryLigatures? (OpenType), contextualAlternates?, stylisticAlternates?, stylisticSets?: [1–20], ss01?, ss02?, ss03?, ss04?, ss05?, ss06?, ss07?, ss08?, ss09?, ss10?, ss11?, ss12?, ss13?, ss14?, ss15?, ss16?, ss17?, ss18?, ss19?, ss20?, swash?, titling?, ordinals?, fractions?, allSmallCaps?, figureStyle?: default|lining|oldStyle, figureWidth?: default|proportional|tabular, figures?, variations?: {tag: value} (variable font axes; null resets), justify?: left|center|right|justifyLeft|justifyCenter|justifyRight|justifyAll, indentLeft?, indentRight?, indentFirst?, spaceBefore?, spaceAfter?, direction?: ltr|rtl, composer?: everyLine|singleLine, hangingPunctuation?, strokeOverFill?, box?: [x,y,w,h]|null (layer space), vertical?}",
            has_layers,
            set_text
        ),
        cmd!(
            "layer.transform",
            "Transform",
            ["Layer", "Transform"],
            None,
            "{layers?, op: reset|center|fit|fitWidth|fitHeight|flipH|flipV}",
            has_layers,
            transform_op
        ),
    ]
}

#[allow(dead_code)]
fn unused(_: &Comp) {}

#[cfg(test)]
mod parent_chain_tests {
    use super::*;
    use std::sync::Arc;

    fn fixture() -> (Session, u64, u64, u64) {
        let mut s = Session::default();
        s.execute("comp.new", json!({"name":"Parent safety","width":16,"height":16,"duration":1})).unwrap();
        let a = s.execute("layer.newSolid", json!({"name":"A"})).unwrap()["layer"].as_u64().unwrap();
        let b = s.execute("layer.newSolid", json!({"name":"B"})).unwrap()["layer"].as_u64().unwrap();
        let c = s.execute("layer.newSolid", json!({"name":"C"})).unwrap()["layer"].as_u64().unwrap();
        for id in [a, b, c] {
            s.execute("prop.set", json!({"layer":id,"path":"transform/anchor","value":[0,0,0]})).unwrap();
        }
        s.execute("prop.set", json!({"layer":a,"path":"transform/position","value":[100,200,0]})).unwrap();
        s.execute("prop.set", json!({"layer":c,"path":"transform/position","value":[10,20,0]})).unwrap();
        s.history.undo.clear();
        (s, a, b, c)
    }
    fn malformed_parent(s: &mut Session, child: u64, parent: u64) {
        let cid = s.active_comp_id().unwrap();
        Arc::make_mut(&mut s.project).comp_mut(cid).unwrap().layer_mut(LayerId(child)).unwrap().parent = Some(LayerId(parent));
    }
    fn assert_atomic_error(s: &mut Session, child: u64, parent: u64, compensate: bool, message: &str) {
        let before = s.project.clone();
        let revision = s.revision;
        let undo = s.history.undo.len();
        let err = s.execute("layer.setParent", json!({"layers":[child],"parent":parent,"compensate":compensate})).unwrap_err();
        assert!(err.to_string().contains(message), "{err}");
        assert!(Arc::ptr_eq(&before, &s.project));
        assert_eq!(s.revision, revision);
        assert_eq!(s.history.undo.len(), undo);
    }
    #[test]
    fn loaded_cycle_excluding_selected_child_is_rejected_atomically() {
        for compensate in [true, false] {
            let (mut s, a, b, c) = fixture();
            // Simulate a malformed loaded project. Never execute this on the unguarded baseline.
            malformed_parent(&mut s, a, b);
            malformed_parent(&mut s, b, a);
            assert_atomic_error(&mut s, c, a, compensate, "existing cycle");
        }
    }
    #[test]
    fn dangling_loaded_parent_is_rejected_atomically() {
        for compensate in [true, false] {
            let (mut s, a, _, c) = fixture();
            malformed_parent(&mut s, a, u64::MAX);
            assert_atomic_error(&mut s, c, a, compensate, "missing layer");
        }
    }
    #[test]
    fn selected_descendant_and_direct_self_are_rejected() {
        let (mut s, a, _, c) = fixture();
        assert_atomic_error(&mut s, c, c, false, "create a cycle");
        s.execute("layer.setParent", json!({"layers":[a],"parent":c,"compensate":false})).unwrap();
        assert_atomic_error(&mut s, c, a, true, "create a cycle");
    }
    #[test]
    fn ordinary_parent_and_unparent_keep_explicit_false_and_undo_semantics() {
        for compensate in [true, false] {
            let (mut s, a, _, c) = fixture();
            let old = s.project.clone();
            let world = |s: &Session| {
                let cid = s.active_comp_id().unwrap();
                let comp = s.active_comp().unwrap();
                let ctx = effectcraft_render::EvalCtx::new(&s.project, cid, comp, s.time());
                ctx.world_matrix(comp.layer(LayerId(c)).unwrap()).apply(effectcraft_geom::Vec3::ZERO)
            };
            let before = world(&s);
            s.execute("layer.setParent", json!({"layers":[c],"parent":a,"compensate":compensate})).unwrap();
            let expected = if compensate { before } else { before + effectcraft_geom::vec3(100.0, 200.0, 0.0) };
            assert!((world(&s) - expected).length() < 1e-9);
            assert_eq!(s.active_comp().unwrap().layer(LayerId(c)).unwrap().parent, Some(LayerId(a)));
            assert_eq!(s.history.undo.len(), 1);
            let parented = s.project.clone();
            assert!(s.undo());
            assert!(Arc::ptr_eq(&s.project, &old));
            assert!(s.redo());
            assert!(Arc::ptr_eq(&s.project, &parented));
            s.execute("layer.setParent", json!({"layers":[c],"parent":null,"compensate":compensate})).unwrap();
            assert_eq!(s.active_comp().unwrap().layer(LayerId(c)).unwrap().parent, None);
            assert!(s.undo());
            assert!(Arc::ptr_eq(&s.project, &parented));
        }
    }
    #[test]
    fn ancestor_and_lookup_work_budgets_fail_closed() {
        let (mut s, a, b, c) = fixture();
        malformed_parent(&mut s, a, b);
        let comp = s.active_comp().unwrap();
        // New solids are inserted above the selection: C,B,A. Walking A->B scans
        // three then two layers, and compares the one selected child twice: seven work units.
        assert_eq!(comp.layers.iter().map(|l| l.id).collect::<Vec<_>>(), vec![LayerId(c), LayerId(b), LayerId(a)]);
        assert!(check_parent_chain(comp, &[LayerId(c)], Some(LayerId(a)), 2, 7).is_ok());
        assert!(check_parent_chain(comp, &[LayerId(c)], Some(LayerId(a)), 2, 6).unwrap_err().to_string().contains("work limit"));
        assert!(check_parent_chain(comp, &[LayerId(c)], Some(LayerId(a)), 2, 256).is_ok());
        assert!(check_parent_chain(comp, &[LayerId(c)], Some(LayerId(a)), 1, 256).unwrap_err().to_string().contains("visit limit"));
        assert!(check_parent_chain(comp, &[LayerId(c)], Some(LayerId(a)), 2, 1).unwrap_err().to_string().contains("work limit"));
        assert!(check_parent_chain(comp, &[LayerId(c)], None, 0, 0).is_ok(), "unparenting does not traverse ancestors");
    }

    #[test]
    fn well_formed_chain_beyond_renderer_depth_is_not_unnecessarily_rejected() {
        let (s, _, _, c) = fixture();
        let mut comp = s.active_comp().unwrap().clone();
        let template = comp.layers.first().unwrap().clone();
        for n in 0..80 {
            let mut layer = template.clone();
            layer.id = LayerId(10_000 + n);
            layer.parent = (n < 79).then_some(LayerId(10_001 + n));
            comp.layers.push(layer);
        }
        assert!(check_parent_chain(&comp, &[LayerId(c)], Some(LayerId(10_000)), 4096, 1_048_576).is_ok());
    }
}
