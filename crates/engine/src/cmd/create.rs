//! Layer ▸ Create: Convert to Editable Text (Photoshop type layers), Create Shapes from Text,
//! Create Masks from Text, Create Shapes from Vector Layer (SVG footage) and Create 3D Layer
//! Instance (a 2D copy of a 3D model layer that 2D effects can be applied to).

use effectcraft_color::Label;
use effectcraft_keyframe::Value as KV;
use effectcraft_project::build::{self, Ids};
use effectcraft_project::{ItemKind, LayerSource, Solid};
use serde_json::{Value, json};

use super::{CommandSpec, bad, has_layers, layers_p, str_p};
use crate::{EngineError, Result, Session, cmd};

const OPS: &str = "editableText|shapesFromText|masksFromText|shapesFromVector|instance3d";

fn create(s: &mut Session, p: &Value) -> Result<Value> {
    let op = str_p(p, "op").ok_or_else(|| bad("layer.create", format!("missing `op` ({OPS})")))?;
    let (cid, lids) = layers_p(s, p)?;
    if lids.is_empty() {
        return Err(bad("layer.create", "select a layer first"));
    }
    let time = s.time();
    let mut created = vec![];
    let mut skipped = vec![];
    // All the selected models in one edit: one undo (or redo) removes (or restores) every
    // instance at once, as one command in the History panel.
    if op == "instance3d" {
        // After Effects' Create 3D Layer Instance (Layer ▸ Create): 3D model layers take
        // no masks or effects, so a comp-sized solid goes at the top of the layer stack
        // with a 3D Layer Instance effect referencing the model — a 2D copy of its render,
        // through the comp's camera, that 2D effects can be applied to. The model is switched
        // off, as After Effects does (back on to keep its shadow).
        let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?.clone();
        let mut eligible = vec![];
        for lid in &lids {
            match comp.layer(*lid) {
                Some(src) if src.source.is_model() => eligible.push((*lid, src.clone())),
                Some(src) => skipped.push(format!("{}: not a 3D model layer", src.name)),
                None => {}
            }
        }
        if !eligible.is_empty() {
            let ids = s.edit("Create 3D Layer Instance", None, |proj, st| {
                let mut ids = vec![];
                for (lid, src) in &eligible {
                    let name = proj.comp(cid).ok_or(EngineError::NoComp)?.unique_layer_name(&format!("{} Instance", src.name));
                    let folder = proj.folder_named("Solids").unwrap_or_else(|| proj.add_item("Solids", Label::Yellow, None, ItemKind::Folder));
                    let sid = proj.add_item(
                        &name,
                        src.label,
                        Some(folder),
                        ItemKind::Solid(Solid { color: [1.0, 1.0, 1.0], width: comp.width, height: comp.height, pixel_aspect: 1.0 }),
                    );
                    let mut l = build::layer(proj, &comp, &name, LayerSource::Solid { item: sid }, (comp.width, comp.height), None);
                    // A soloed source keeps its Solo switch with video off, and any_solo hides
                    // the non-soloed copy: the copy takes the Solo over to stay visible.
                    l.switches.solo = src.switches.solo;
                    // A 3D Layer Instance effect referencing the 3D layer: the solid shows its
                    // render (the 2D copy).
                    let fx = effectcraft_effects::find("ec.3d.instance3d").ok_or_else(|| bad("layer.create", "the 3D Layer Instance effect is missing"))?;
                    let mut g = effectcraft_effects::instantiate(fx, &mut Ids(&mut proj.next_id), fx.name, [comp.width as f64, comp.height as f64]);
                    if let Some(pr) = g.get_mut("layer") {
                        pr.value = KV::Layer(Some(lid.0));
                    }
                    if let Some(fxg) = l.props.sub_mut("effects") {
                        fxg.children.push(g.into());
                    }
                    // The model is switched off (After Effects turns the referenced layer off);
                    // the copies go at the top of the stack, selected.
                    let c = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
                    if let Some(m) = c.layers.iter_mut().find(|x| x.id == *lid) {
                        m.switches.video = false;
                    }
                    let id = l.id;
                    c.layers.insert(0, l);
                    ids.push(id);
                }
                st.selected_layers = ids.clone();
                Ok(ids)
            })?;
            created.extend(ids.iter().map(|i| i.0));
        }
        if created.is_empty() && !skipped.is_empty() {
            return Err(bad("layer.create", skipped.join("; ")));
        }
        return Ok(json!({"layers": created, "skipped": skipped}));
    }
    for lid in lids {
        let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?.clone();
        let Some(src) = comp.layer(lid).cloned() else { continue };
        let footage = src.source.item().and_then(|i| match &s.project.item(i)?.kind {
            ItemKind::Footage(f) => Some(f.clone()),
            _ => None,
        });
        match op {
            "shapesFromVector" => {
                let Some(f) = footage.filter(crate::vector::is_vector) else {
                    skipped.push(format!("{}: not a vector (SVG, PDF, AI or EPS) footage layer", src.name));
                    continue;
                };
                let bytes = s.services.read_file(&f.path).map_err(|e| EngineError::Other(format!("cannot read {}: {e}", f.path)))?;
                let doc = crate::vector::vector_doc(&f.path, &bytes, f.layer.as_ref(), f.page)
                    .unwrap_or_else(|| Err(format!("{}: not a vector file", f.path)))
                    .map_err(EngineError::Other)?;
                // Images become footage layers (PNG files written next to the source and
                // imported), parented to the shape layer below it.
                let (images, masks) = crate::vector::vector_images(&doc);
                if masks > 0 {
                    skipped.push(format!("{}: {masks} soft mask(s) not converted (shape layers have no soft masks)", src.name));
                }
                let mut image_items = vec![];
                for (i, img) in images.iter().enumerate() {
                    match image_footage(s, &f.path, i + 1, img) {
                        Ok(item) => image_items.push((item, i)),
                        Err(e) => skipped.push(format!("{}: image {} not converted ({e})", src.name, i + 1)),
                    }
                    if img.clipped {
                        skipped.push(format!("{}: image {} is clipped or masked; its footage layer is not", src.name, i + 1));
                    }
                }
                let mut notes = vec![];
                let ids = s.edit("Create Shapes from Vector Layer", None, |proj, st| {
                    // Without images: one shape layer. With images: the shapes between them
                    // in layers of their own, so the stack keeps the document's paint order.
                    let segments = if image_items.is_empty() { vec![doc.clone()] } else { crate::vector::split_at_images(&doc) };
                    let top = crate::vector::shapes_from_vector(proj, &comp, &src, segments.last().unwrap_or(&doc));
                    // Top to bottom: the last segment, image n, segment n − 1, … image 1, segment 0.
                    let mut stack = vec![];
                    for k in (0..segments.len().saturating_sub(1)).rev() {
                        if let Some((item, i)) = image_items.iter().find(|(_, i)| *i == k) {
                            let (il, note) = crate::vector::image_layer(proj, &comp, *item, &images[*i], &top);
                            notes.extend(note);
                            stack.push(il);
                        }
                        let seg = &segments[k];
                        if seg.root.children.is_empty() {
                            continue;
                        }
                        let mut sl = crate::vector::shapes_from_vector(proj, &comp, &src, seg);
                        sl.name = format!("{} {}", top.name, k + 1);
                        stack.push(sl);
                    }
                    let mut ids = vec![top.id];
                    ids.extend(stack.iter().filter(|l| matches!(l.source, LayerSource::Shape)).map(|l| l.id));
                    // Each directly above the source: the top layer first.
                    insert_above(proj, cid, lid, top, true)?;
                    for l in stack {
                        insert_above(proj, cid, lid, l, true)?;
                    }
                    st.selected_layers = vec![ids[0]];
                    Ok(ids)
                })?;
                skipped.extend(notes);
                created.extend(ids.iter().map(|i| i.0));
            }
            "shapesFromText" => {
                if !matches!(src.source, LayerSource::Text) {
                    skipped.push(format!("{}: not a text layer", src.name));
                    continue;
                }
                let snapshot = s.project.clone();
                let id = s.edit("Create Shapes from Text", None, |proj, st| {
                    let l = crate::vector::shapes_from_text(proj, &comp, &snapshot, cid, &src, time)
                        .ok_or_else(|| bad("layer.create", "the text layer has no glyphs"))?;
                    let id = l.id;
                    insert_above(proj, cid, lid, l, true)?;
                    st.selected_layers = vec![id];
                    Ok(id)
                })?;
                created.push(id.0);
            }
            "masksFromText" => {
                if !matches!(src.source, LayerSource::Text) {
                    skipped.push(format!("{}: not a text layer", src.name));
                    continue;
                }
                let outlines = crate::vector::text_outlines_in_comp(&s.project, cid, &comp, &src, time);
                if outlines.is_empty() {
                    skipped.push(format!("{}: no glyphs", src.name));
                    continue;
                }
                let id = s.edit("Create Masks from Text", None, |proj, st| {
                    let name = comp.unique_layer_name(&format!("{} Outlines", src.name));
                    let folder = proj.folder_named("Solids").unwrap_or_else(|| proj.add_item("Solids", Label::Yellow, None, ItemKind::Folder));
                    let sid = proj.add_item(
                        &name,
                        Label::Red,
                        Some(folder),
                        ItemKind::Solid(Solid { color: [1.0, 1.0, 1.0], width: comp.width, height: comp.height, pixel_aspect: 1.0 }),
                    );
                    let mut l = crate::vector::masks_layer(proj, &comp, &name, outlines, sid);
                    l.in_point = src.in_point;
                    l.out_point = src.out_point;
                    let id = l.id;
                    insert_above(proj, cid, lid, l, true)?;
                    st.selected_layers = vec![id];
                    Ok(id)
                })?;
                created.push(id.0);
            }
            "editableText" => {
                let Some((f, sl)) = footage.and_then(|f| f.layer.clone().map(|l| (f, l))) else {
                    skipped.push(format!("{}: not a Photoshop layer", src.name));
                    continue;
                };
                let bytes = s.services.read_file(&f.path).map_err(|e| EngineError::Other(format!("cannot read {}: {e}", f.path)))?;
                let psd = effectcraft_psd::Psd::parse(bytes).map_err(|e| EngineError::Other(e.to_string()))?;
                let id = s.edit("Convert to Editable Text", None, |proj, st| {
                    let l = crate::psd_import::editable_text(proj, &comp, &src, &psd, sl.index as usize)
                        .ok_or_else(|| bad("layer.create", format!("{}: the Photoshop layer has no text", src.name)))?;
                    let id = l.id;
                    let c = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
                    let at = c.layers.iter().position(|x| x.id == lid).ok_or(EngineError::NoComp)?;
                    c.layers[at] = l;
                    st.selected_layers = vec![id];
                    Ok(id)
                })?;
                created.push(id.0);
            }
            other => return Err(bad("layer.create", format!("`{other}` is not available yet (supported: {OPS})"))),
        }
    }
    if created.is_empty() && !skipped.is_empty() {
        return Err(bad("layer.create", skipped.join("; ")));
    }
    Ok(json!({"layers": created, "skipped": skipped}))
}

/// An image of a vector file written as `<file> Image <n>.png` next to it and imported: the
/// footage item.
fn image_footage(s: &mut Session, source: &str, n: usize, img: &crate::vector::VectorImage) -> Result<effectcraft_project::ItemId> {
    let src = std::path::Path::new(source);
    let stem = src.file_stem().map(|x| x.to_string_lossy().into_owned()).unwrap_or_else(|| "Vector".into());
    let name = format!("{stem} Image {n}.png");
    let path = match src.parent().filter(|p| !p.as_os_str().is_empty()) {
        Some(dir) => dir.join(&name).to_string_lossy().into_owned(),
        None => name,
    };
    let png = super::comp_more::encode_png(&img.rgba, img.width, img.height).map_err(EngineError::Other)?;
    s.services.write_file(&path, &png).map_err(|e| EngineError::Other(format!("cannot write {path}: {e}")))?;
    let r = super::file::import(s, &json!({"paths": [path]}))?;
    if let Some(e) = r["errors"].as_array().and_then(|e| e.first()) {
        return Err(EngineError::Other(e.as_str().unwrap_or("import failed").to_string()));
    }
    r["items"].get(0).and_then(Value::as_u64).map(effectcraft_project::ItemId).ok_or_else(|| EngineError::Other(format!("{path} was not imported")))
}

/// Insert `l` directly above layer `below`; optionally switch the source's video off (as After
/// Effects does after converting).
fn insert_above(
    proj: &mut effectcraft_project::Project,
    cid: effectcraft_project::ItemId,
    below: effectcraft_project::LayerId,
    l: effectcraft_project::Layer,
    hide_source: bool,
) -> Result<()> {
    let c = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
    let at = c.layers.iter().position(|x| x.id == below).unwrap_or(0);
    if hide_source && let Some(src) = c.layers.get_mut(at) {
        src.switches.video = false;
    }
    c.layers.insert(at, l);
    Ok(())
}

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!("layer.create", "Create", [], None, "{op: editableText|shapesFromText|masksFromText|shapesFromVector|instance3d, layers?}", has_layers, create)]
}
