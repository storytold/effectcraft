use super::{
    EngineError, Ids, ItemId, ItemKind, Justify, KV, Label, Layer, LayerId, LayerSource, Project, Result, Session, Solid, TextDoc, Tick, Value, b_p, bad,
    build, comp_id, f_p, json, limits, str_p,
};

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
        crate::cmd::model3d::add_geometry_options(&mut proj.next_id, &mut layer);
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
pub(super) fn pixel_aspect_p(p: &Value) -> Result<Option<f64>> {
    match p.get("pixelAspect").and_then(Value::as_f64) {
        Some(v) if !(v.is_finite() && v > 0.0) => Err(bad("layer.settings", "`pixelAspect` must be a positive number")),
        v => Ok(v.map(limits::pixel_aspect)),
    }
}

pub(super) fn new_solid(s: &mut Session, p: &Value) -> Result<Value> {
    new_solid_like(s, p, false)
}

pub(super) fn new_adjustment(s: &mut Session, p: &Value) -> Result<Value> {
    new_solid_like(s, p, true)
}

/// Parameters of `layer.setText` / `layer.newText` that aren't text attributes.
const NOT_ATTRS: &[&str] = &["layer", "layers", "comp", "merge", "text", "range", "position", "edit"];

/// Apply `layer.setText` keys to `d`: `text` replaces characters `r` (or everything), then each
/// character / paragraph / document attribute applies to `r` (or everything).
pub(super) fn apply_text_params(cmd: &str, p: &Value, d: &mut TextDoc, r: Option<std::ops::Range<usize>>) -> Result<()> {
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

pub(super) fn text_range_p(p: &Value) -> Option<std::ops::Range<usize>> {
    let a = p.get("range")?.as_array()?;
    let g = |i: usize| a.get(i).and_then(Value::as_u64).map(|x| x as usize);
    let (x, y) = (g(0)?, g(1).or(g(0))?);
    Some(x.min(y)..x.max(y))
}

/// `layer.newText`: adds a Text layer to the comp at the CTI and returns `{layer}`.
///
/// The document starts as centre-justified "Text", with the optional text parameters applied.
/// With `box` `[x, y, w, h]`, `(x, y)` is the paragraph box's top-left in comp space. Its width
/// and height are `w.abs().max(1.0)` and `h.abs().max(1.0)`; the layer position is the box centre.
/// It is left justified unless `justify` is given. The layer is named by `name`, else the first 40
/// characters of the first text line, else "Text". `edit: true` also opens a new editing
/// session with anchor `0` and the caret after the text, selecting all initial text so the next
/// insertion replaces it. The session has `created: true`; leaving it empty keeps the layer, as After Effects does.
pub(super) fn new_text(s: &mut Session, p: &Value) -> Result<Value> {
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
            st.text_edit = Some(crate::cmd::text_edit::TextEdit {
                layer: id,
                anchor: 0,
                caret: n,
                created: true,
                session: crate::cmd::text_edit::next_edit_session(),
                ..Default::default()
            });
        }
        Ok(id)
    })?;
    Ok(json!({"layer": id.0}))
}

pub(super) fn new_shape(s: &mut Session, p: &Value) -> Result<Value> {
    let cid = comp_id(s, p)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?.clone();
    let kind = str_p(p, "kind").unwrap_or("none").to_string();
    let size = p
        .get("size")
        .and_then(Value::as_array)
        .map(|a| [a.first().and_then(Value::as_f64).unwrap_or(200.0), a.get(1).and_then(Value::as_f64).unwrap_or(200.0)])
        .unwrap_or([300.0, 300.0]);
    // A fill, and a stroke only with a width (not the Tools bar's paint).
    let paint = crate::cmd::shape_tool::paint_p(p, &Default::default(), "layer.newShape")?;
    let pos = p
        .get("position")
        .and_then(|v| v.as_array())
        .map(|a| [a.first().and_then(Value::as_f64).unwrap_or(0.0), a.get(1).and_then(Value::as_f64).unwrap_or(0.0)]);
    let name = str_p(p, "name").unwrap_or("Shape Layer 1").to_string();
    let id = s.edit("New Shape Layer", None, |proj, st| {
        let lid = crate::cmd::shape_tool::new_shape_layer(proj, st, &comp, cid, Some(&name), pos)?;
        let mut next = proj.next_id;
        let g = crate::cmd::shape_tool::shape_group(&mut Ids(&mut next), &kind, size, &paint);
        proj.next_id = next;
        if let Some(g) = g {
            crate::cmd::shape_tool::add_to_contents(proj, cid, lid, g, [0.0, 0.0], "layer.newShape")?;
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

pub(super) fn new_null(s: &mut Session, p: &Value) -> Result<Value> {
    new_simple(s, p, LayerSource::Null, "Null 1", "New Null Object")
}

pub(super) fn add_item(s: &mut Session, p: &Value) -> Result<Value> {
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
            return crate::cmd::model3d::new_model(
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
