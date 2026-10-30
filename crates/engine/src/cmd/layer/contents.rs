use super::create::{apply_text_params, text_range_p};
use super::{
    EngineError, GroupKind, Ids, ItemId, KV, LayerId, MaskMode, Node, Project, PropGroup, Result, Session, TextDoc, Value, b_p, bad, build, json, layer_mut,
    layer_p, merge_p, str_p,
};

pub(super) fn add_mask(s: &mut Session, p: &Value) -> Result<Value> {
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
    let path = crate::cmd::shape_tool::mask_path(shape, [cx, cy], [rw, rh])
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

pub(super) fn mask_props(s: &mut Session, p: &Value) -> Result<Value> {
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

pub(super) fn add_shape_item(s: &mut Session, p: &Value) -> Result<Value> {
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

/// `layer.setText`: sets a text layer's Source Text and/or character attributes at the CTI.
///
/// A `font` name is also added to the recent fonts and the preferences saved. With an empty
/// `range` and no `text`, while that layer is being edited and its document is non-empty,
/// recognised character attributes are also staged in the pending insertion style without
/// changing existing characters. Paragraph/document attributes still apply through the edit;
/// the session is bumped whenever this branch runs. Document changes use [`Session::edit`],
/// merged by `merge_p`. With `text` given, a layer whose name is empty or starts with `Text`
/// takes the new first line (up to 40 characters) when that line is non-empty. Replies `null`.
pub(super) fn set_text(s: &mut Session, p: &Value) -> Result<Value> {
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
        && let Some(doc) = crate::cmd::text_edit::layer_doc(s, lid)
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
    // A changed Source Text is handled by `Session::edit`: its boundary clamps the selection and
    // starts a new IME epoch only when the CTI document actually changed, so an identical text
    // write stays a no-op and keeps the editor's owner.
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
