//! Layer menu: quality / sampling / frame blending, switches, transform dialogs, masks on existing
//! layers, markers, track matte shortcuts, open/reveal, and Keyframe Assistant ▸ Sequence Layers.

use effectcraft_keyframe::{Keyframe, ShapePath, Value as KV};
use effectcraft_project::{FrameBlend, GroupKind, ItemId, LayerId, LayerSource, Marker, MaskMode, MatteKind, Node, Quality, Sampling, TrackMatte, Uid};
use effectcraft_time::Tick;
use serde_json::{Value, json};

use super::{CommandSpec, b_p, bad, comp_id, f_p, frontend, has_comp, has_layers, layer_mut, layer_p, layers_p, merge_p, str_p, time_p};
use crate::{EngineError, Result, Session, cmd};

// ---------------------------------------------------------------- enablement

fn has_masks(s: &Session) -> std::result::Result<(), String> {
    has_layers(s)?;
    let c = s.active_comp().ok_or("no composition")?;
    let any = s.state.selected_layers.iter().filter_map(|l| c.layer(*l)).any(|l| l.masks().is_some_and(|m| !m.children.is_empty()));
    if any { Ok(()) } else { Err("the selected layers have no masks".into()) }
}

fn has_markers(s: &Session) -> std::result::Result<(), String> {
    has_layers(s)?;
    let c = s.active_comp().ok_or("no composition")?;
    if s.state.selected_layers.iter().filter_map(|l| c.layer(*l)).any(|l| !l.markers.is_empty()) {
        Ok(())
    } else {
        Err("the selected layers have no markers".into())
    }
}

fn has_source_item(s: &Session) -> std::result::Result<(), String> {
    has_layers(s)?;
    let c = s.active_comp().ok_or("no composition")?;
    if s.state.selected_layers.iter().filter_map(|l| c.layer(*l)).any(|l| l.source.item().is_some()) {
        Ok(())
    } else {
        Err("the selected layer has no source item".into())
    }
}

fn has_footage_layer(s: &Session) -> std::result::Result<(), String> {
    has_layers(s)?;
    let c = s.active_comp().ok_or("no composition")?;
    let any = s.state.selected_layers.iter().filter_map(|l| c.layer(*l)).any(|l| matches!(l.source, LayerSource::Footage { .. }));
    if any { Ok(()) } else { Err("select a footage layer".into()) }
}

fn has_multiple_layers(s: &Session) -> std::result::Result<(), String> {
    has_comp(s)?;
    if s.state.selected_layers.len() >= 2 { Ok(()) } else { Err("select two or more layers".into()) }
}

// ---------------------------------------------------------------- quality / sampling / blending

fn quality(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, ids) = layers_p(s, p)?;
    let q = match str_p(p, "quality").unwrap_or("best").to_ascii_lowercase().as_str() {
        "best" => Quality::Best,
        "draft" => Quality::Draft,
        "wireframe" => Quality::Wireframe,
        q => return Err(bad("layer.quality", format!("quality must be best|draft|wireframe, not `{q}`"))),
    };
    s.edit("Layer Quality", None, |proj, _| {
        for l in proj.comp_mut(cid).ok_or(EngineError::NoComp)?.layers.iter_mut().filter(|l| ids.contains(&l.id)) {
            l.switches.quality = q;
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

fn sampling(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, ids) = layers_p(s, p)?;
    let q = match str_p(p, "sampling").unwrap_or("bilinear").to_ascii_lowercase().as_str() {
        "bilinear" => Sampling::Bilinear,
        "bicubic" => Sampling::Bicubic,
        q => return Err(bad("layer.sampling", format!("sampling must be bilinear|bicubic, not `{q}`"))),
    };
    // `toggle`: layers that all have it already get the other one.
    let all = s.project.comp(cid).is_some_and(|c| c.layers.iter().filter(|l| ids.contains(&l.id)).all(|l| l.switches.sampling == q));
    let q = match q {
        Sampling::Bilinear if all && b_p(p, "toggle").unwrap_or(false) => Sampling::Bicubic,
        Sampling::Bicubic if all && b_p(p, "toggle").unwrap_or(false) => Sampling::Bilinear,
        q => q,
    };
    s.edit("Layer Sampling", None, |proj, _| {
        for l in proj.comp_mut(cid).ok_or(EngineError::NoComp)?.layers.iter_mut().filter(|l| ids.contains(&l.id)) {
            l.switches.sampling = q;
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

fn frame_blending(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, ids) = layers_p(s, p)?;
    let m = match str_p(p, "mode").unwrap_or("off") {
        "off" => FrameBlend::Off,
        "frameMix" => FrameBlend::FrameMix,
        "pixelMotion" => FrameBlend::PixelMotion,
        m => return Err(bad("layer.frameBlending", format!("mode must be off|frameMix|pixelMotion, not `{m}`"))),
    };
    s.edit("Frame Blending", None, |proj, _| {
        for l in proj.comp_mut(cid).ok_or(EngineError::NoComp)?.layers.iter_mut().filter(|l| ids.contains(&l.id)) {
            l.switches.frame_blend = m;
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

// ---------------------------------------------------------------- switches

fn hide_other_video(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, ids) = layers_p(s, p)?;
    s.edit("Hide Other Video", None, |proj, _| {
        // Every layer has a Video switch (nulls, cameras and lights included, as the Timeline's
        // eye cell shows): the selection stays shown.
        for l in proj.comp_mut(cid).ok_or(EngineError::NoComp)?.layers.iter_mut().filter(|l| !ids.contains(&l.id)) {
            l.switches.video = false;
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

fn show_all_video(s: &mut Session, p: &Value) -> Result<Value> {
    let cid = comp_id(s, p)?;
    s.edit("Show All Video", None, |proj, _| {
        let c = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        // Track mattes stay hidden (their video switch is off by design).
        let mattes: Vec<LayerId> = c.layers.iter().filter_map(|l| l.track_matte.map(|m| m.layer)).collect();
        for l in c.layers.iter_mut().filter(|l| !mattes.contains(&l.id)) {
            l.switches.video = true;
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

fn unlock_all(s: &mut Session, p: &Value) -> Result<Value> {
    let cid = comp_id(s, p)?;
    s.edit("Unlock All Layers", None, |proj, _| {
        for l in proj.comp_mut(cid).ok_or(EngineError::NoComp)?.layers.iter_mut() {
            l.switches.locked = false;
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

fn expressions(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, ids) = layers_p(s, p)?;
    let on = b_p(p, "enabled").unwrap_or(true);
    let n = s.edit(if on { "Enable Expressions" } else { "Disable Expressions" }, None, |proj, _| {
        let mut n = 0;
        for l in proj.comp_mut(cid).ok_or(EngineError::NoComp)?.layers.iter_mut().filter(|l| ids.contains(&l.id)) {
            l.props.walk_mut(&mut |pr| {
                if let Some(e) = &mut pr.expr {
                    e.enabled = on;
                    n += 1;
                }
            });
        }
        Ok(n)
    })?;
    Ok(json!({"expressions": n}))
}

// ---------------------------------------------------------------- transform

fn transform_member(prop: &str) -> Option<&'static str> {
    Some(match prop {
        "anchor" | "anchorPoint" => "anchor",
        "position" => "position",
        "scale" => "scale",
        "orientation" => "orientation",
        "rotation" => "rotation",
        "opacity" => "opacity",
        _ => return None,
    })
}

fn set_transform(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, ids) = layers_p(s, p)?;
    let prop = str_p(p, "prop").ok_or_else(|| bad("layer.setTransform", "missing `prop`"))?;
    let m = transform_member(prop).ok_or_else(|| bad("layer.setTransform", "prop: anchor|position|scale|orientation|rotation|opacity"))?;
    let v = p.get("value").ok_or_else(|| bad("layer.setTransform", "missing `value`"))?.clone();
    let t = s.time();
    let label = match m {
        "anchor" => "Anchor Point",
        "position" => "Position",
        "scale" => "Scale",
        "orientation" => "Orientation",
        "rotation" => "Rotation",
        _ => "Opacity",
    };
    s.edit(label, merge_p(p), |proj, _| {
        for lid in &ids {
            let l = layer_mut(proj, cid, *lid)?;
            let lt = l.layer_time(t);
            // Separate Dimensions: the picture follows X/Y/Z Position, not the combined property.
            if m == "position" && l.transform().is_some_and(|tr| tr.get("positionX").is_some()) {
                let cur = l
                    .transform()
                    .map(|tr| {
                        [
                            tr.get("positionX").map(|p| p.value_at(lt).as_f64()).unwrap_or(0.0),
                            tr.get("positionY").map(|p| p.value_at(lt).as_f64()).unwrap_or(0.0),
                            tr.get("positionZ").map(|p| p.value_at(lt).as_f64()).unwrap_or(0.0),
                        ]
                    })
                    .unwrap_or([0.0; 3]);
                // Two numbers keep the current z.
                let j = match &v {
                    Value::Array(a) if a.len() == 2 => json!([a[0], a[1], cur[2]]),
                    _ => v.clone(),
                };
                let nv = KV::Vec3(cur).coerce_json(&j).ok_or_else(|| bad("layer.setTransform", format!("can't use {v} for {label}")))?;
                let xyz = nv.as_vec3();
                let Some(tr) = l.transform_mut() else { continue };
                super::layer::update_position(tr, lt, |_| xyz);
                if let Some(pr) = tr.get_mut("position") {
                    pr.set_value_at(lt, KV::Vec3(xyz));
                }
                continue;
            }
            let Some(pr) = l.transform_mut().and_then(|tr| tr.get_mut(m)) else { continue };
            let cur = pr.value_at(lt);
            // Two numbers for a 3D value keep the current z.
            let j = match (&cur, &v) {
                (KV::Vec3(c), Value::Array(a)) if a.len() == 2 => json!([a[0], a[1], c[2]]),
                _ => v.clone(),
            };
            let nv = cur.coerce_json(&j).ok_or_else(|| bad("layer.setTransform", format!("can't use {v} for {label}")))?;
            pr.set_value_at(lt, nv);
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

fn center_anchor(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, ids) = layers_p(s, p)?;
    let ids = super::unlocked(s, cid, ids, "layer.centerAnchor")?;
    let t = s.time();
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let ctx = effectcraft_render::EvalCtx::new(&s.project, cid, comp, t);
    let mut centers = vec![];
    for lid in &ids {
        let Some(l) = comp.layer(*lid) else { continue };
        if let Some([x0, y0, x1, y1]) = effectcraft_render::content_bounds(&ctx, l) {
            centers.push((*lid, [(x0 + x1) / 2.0, (y0 + y1) / 2.0]));
        }
    }
    s.edit("Center Anchor Point", None, |proj, _| {
        for (lid, c) in &centers {
            let l = layer_mut(proj, cid, *lid)?;
            let lt = l.layer_time(t);
            let Some(tr) = l.transform_mut() else { continue };
            let get = |tr: &effectcraft_project::PropGroup, m: &str, d: [f64; 3]| tr.get(m).map(|p| p.value_at(lt).as_vec3()).unwrap_or(d);
            let anchor = get(tr, "anchor", [0.0; 3]);
            let scale = get(tr, "scale", [100.0; 3]);
            let rot = tr.get("rotation").map(|p| p.value_at(lt).as_f64()).unwrap_or(0.0).to_radians();
            let (dx, dy) = ((c[0] - anchor[0]) * scale[0] / 100.0, (c[1] - anchor[1]) * scale[1] / 100.0);
            let (sn, cs) = rot.sin_cos();
            let shift = [dx * cs - dy * sn, dx * sn + dy * cs];
            if let Some(a) = tr.get_mut("anchor") {
                a.set_value_at(lt, KV::Vec3([c[0], c[1], anchor[2]]));
            }
            // Shift every key so the layer stays put over its whole animation (X and Y Position
            // when the dimensions are separated).
            let offset = |pr: &mut effectcraft_project::Property, f: &dyn Fn(&KV) -> KV| {
                pr.value = f(&pr.value);
                for k in &mut pr.keys {
                    k.value = f(&k.value);
                }
            };
            if tr.get("positionX").is_some() {
                for (m, d) in [("positionX", shift[0]), ("positionY", shift[1])] {
                    if let Some(pr) = tr.get_mut(m) {
                        offset(pr, &|v| KV::Scalar(v.as_f64() + d));
                    }
                }
            } else if let Some(pr) = tr.get_mut("position") {
                offset(pr, &|v| {
                    let c = v.as_vec3();
                    KV::Vec3([c[0] + shift[0], c[1] + shift[1], c[2]])
                });
            }
        }
        Ok(())
    })?;
    Ok(json!({"layers": centers.len()}))
}

// ---------------------------------------------------------------- masks

/// Masks the command applies to: `mask` (index / uid / name) on `layer`, else selected masks,
/// else every mask of the selected layers.
pub(crate) fn target_masks(s: &Session, p: &Value) -> Result<(ItemId, Vec<(LayerId, Uid)>)> {
    let (cid, ids) = layers_p(s, p)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let mut out = vec![];
    if let Some(m) = p.get("mask") {
        for lid in &ids {
            let Some(masks) = comp.layer(*lid).and_then(|l| l.masks()) else { continue };
            let g = match m {
                Value::Number(n) => {
                    let n = n.as_u64().unwrap_or(1);
                    masks.children.iter().find(|c| c.uid() == n).or_else(|| masks.children.get(n.saturating_sub(1) as usize))
                }
                Value::String(name) => masks.children.iter().find(|c| c.name() == name),
                _ => None,
            };
            if let Some(g) = g {
                out.push((*lid, g.uid()));
            }
        }
        return Ok((cid, out));
    }
    for (lid, uid) in &s.state.selected_props {
        let Some(masks) = comp.layer(*lid).and_then(|l| l.masks()) else { continue };
        for g in masks.groups() {
            if g.uid == *uid || g.find(*uid).is_some() {
                out.push((*lid, g.uid));
            }
        }
    }
    if out.is_empty() {
        for lid in &ids {
            if let Some(masks) = comp.layer(*lid).and_then(|l| l.masks()) {
                out.extend(masks.groups().map(|g| (*lid, g.uid)));
            }
        }
    }
    out.dedup();
    Ok((cid, out))
}

fn edit_masks(
    s: &mut Session,
    p: &Value,
    label: &str,
    mut f: impl FnMut(&mut effectcraft_project::PropGroup, (f64, f64), Tick) -> Result<()>,
) -> Result<Value> {
    let (cid, targets) = target_masks(s, p)?;
    if targets.is_empty() {
        return Err(EngineError::Other(format!(
            "{label}: no masks to change (the layer has none, or `mask` matched none): add one with layer.addMask {{layer, shape?: rect|ellipse, rect?: [x,y,w,h]}}"
        )));
    }
    let t = s.time();
    let sizes: Vec<(LayerId, (f64, f64))> = targets
        .iter()
        .filter_map(|(lid, _)| s.project.comp(cid)?.layer(*lid).map(|l| (*lid, effectcraft_render::source_size(&s.project, l))))
        .map(|(l, (w, h))| (l, (w as f64, h as f64)))
        .collect();
    let n = targets.len();
    s.edit(label, merge_p(p), |proj, _| {
        for (lid, uid) in &targets {
            let size = sizes.iter().find(|(l, _)| l == lid).map(|x| x.1).unwrap_or((0.0, 0.0));
            let l = layer_mut(proj, cid, *lid)?;
            let lt = l.layer_time(t);
            let Some(g) = l.props.sub_mut("masks").and_then(|m| m.find_group_mut(*uid)) else { continue };
            f(g, size, lt)?;
        }
        Ok(())
    })?;
    Ok(json!({"masks": n}))
}

fn mask_kind(g: &mut effectcraft_project::PropGroup) -> Option<(&mut MaskMode, &mut bool, &mut bool)> {
    match &mut g.kind {
        GroupKind::Mask { mode, inverted, locked, .. } => Some((mode, inverted, locked)),
        _ => None,
    }
}

fn mask_set(s: &mut Session, p: &Value) -> Result<Value> {
    let field = str_p(p, "field").ok_or_else(|| bad("layer.mask.set", "missing `field` (feather|opacity|expansion)"))?.to_string();
    if !matches!(field.as_str(), "feather" | "opacity" | "expansion") {
        return Err(bad("layer.mask.set", "field: feather|opacity|expansion"));
    }
    let v = p.get("value").ok_or_else(|| bad("layer.mask.set", "missing `value`"))?.clone();
    let label = match field.as_str() {
        "feather" => "Mask Feather",
        "opacity" => "Mask Opacity",
        _ => "Mask Expansion",
    };
    edit_masks(s, p, label, |g, _, lt| {
        let pr = g.get_mut(&field).ok_or_else(|| bad("layer.mask.set", "mask has no such property"))?;
        let cur = pr.value_at(lt);
        let j = match (&cur, &v) {
            (KV::Vec2(_), Value::Number(n)) => json!([n, n]),
            _ => v.clone(),
        };
        let nv = cur.coerce_json(&j).ok_or_else(|| bad("layer.mask.set", format!("can't use {v} for {label}")))?;
        pr.set_value_at(lt, nv);
        Ok(())
    })
}

fn mask_shape(s: &mut Session, p: &Value) -> Result<Value> {
    let r = p.get("rect").and_then(Value::as_array).filter(|a| a.len() == 4).map(|a| [0, 1, 2, 3].map(|i| a[i].as_f64().unwrap_or(0.0)));
    let r = r.ok_or_else(|| bad("layer.mask.shape", "missing `rect` [left, top, width, height] (layer px)"))?;
    let ellipse = str_p(p, "shape") == Some("ellipse");
    edit_masks(s, p, "Mask Shape", |g, _, lt| {
        let (cx, cy) = (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
        let path = if ellipse { ShapePath::ellipse([cx, cy], r[2], r[3]) } else { ShapePath::rect([cx, cy], r[2], r[3]) };
        if let Some(pr) = g.get_mut("path") {
            pr.set_value_at(lt, KV::Path(path));
        }
        Ok(())
    })
}

fn mask_reset(s: &mut Session, p: &Value) -> Result<Value> {
    edit_masks(s, p, "Reset Mask", |g, (w, h), _| {
        let (w, h) = if w > 0.0 { (w, h) } else { (400.0, 300.0) };
        let reset = |g: &mut effectcraft_project::PropGroup, m: &str, v: KV| {
            if let Some(pr) = g.get_mut(m) {
                pr.keys.clear();
                pr.expr = None;
                pr.value = v;
            }
        };
        reset(g, "path", KV::Path(ShapePath::rect([w / 2.0, h / 2.0], w, h)));
        reset(g, "feather", KV::Vec2([0.0, 0.0]));
        reset(g, "opacity", KV::Scalar(100.0));
        reset(g, "expansion", KV::Scalar(0.0));
        if let Some((mode, inv, _)) = mask_kind(g) {
            *mode = MaskMode::Add;
            *inv = false;
        }
        Ok(())
    })
}

fn mask_remove(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, targets) = target_masks(s, p)?;
    // Without an explicit `mask`, Remove Mask only removes selected masks (AE), else the first.
    let selected: Vec<(LayerId, Uid)> = targets
        .iter()
        .copied()
        .filter(|(l, u)| {
            p.get("mask").is_some()
                || s.state.selected_props.iter().any(|(sl, su)| {
                    sl == l
                        && (su == u || s.active_comp().and_then(|c| c.layer(*l)).and_then(|ly| ly.props.find_group(*u)).is_some_and(|g| g.find(*su).is_some()))
                })
        })
        .collect();
    let remove = if selected.is_empty() { targets.into_iter().take(1).collect() } else { selected };
    remove_masks(s, cid, remove, "Remove Mask")
}

fn remove_masks(s: &mut Session, cid: ItemId, remove: Vec<(LayerId, Uid)>, label: &str) -> Result<Value> {
    let n = remove.len();
    s.edit(label, None, |proj, st| {
        for (lid, uid) in &remove {
            if let Some(m) = layer_mut(proj, cid, *lid)?.props.sub_mut("masks") {
                m.children.retain(|c| c.uid() != *uid);
            }
        }
        st.selected_props.retain(|(l, u)| !remove.iter().any(|(rl, ru)| rl == l && ru == u));
        Ok(())
    })?;
    Ok(json!({"removed": n}))
}

fn mask_remove_all(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, ids) = layers_p(s, p)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let all: Vec<(LayerId, Uid)> =
        ids.iter().filter_map(|l| comp.layer(*l)).flat_map(|l| l.masks().into_iter().flat_map(|m| m.groups().map(|g| (l.id, g.uid)))).collect();
    remove_masks(s, cid, all, "Remove All Masks")
}

fn mask_mode(s: &mut Session, p: &Value) -> Result<Value> {
    let m =
        str_p(p, "mode").and_then(MaskMode::from_name).ok_or_else(|| bad("layer.mask.mode", "mode: None|Add|Subtract|Intersect|Lighten|Darken|Difference"))?;
    edit_masks(s, p, "Mask Mode", |g, _, _| {
        if let Some((mode, ..)) = mask_kind(g) {
            *mode = m;
        }
        Ok(())
    })
}

fn mask_invert(s: &mut Session, p: &Value) -> Result<Value> {
    let v = b_p(p, "value");
    edit_masks(s, p, "Mask Inverted", |g, _, _| {
        if let Some((_, inv, _)) = mask_kind(g) {
            *inv = v.unwrap_or(!*inv);
        }
        Ok(())
    })
}

fn mask_lock(s: &mut Session, p: &Value) -> Result<Value> {
    let v = b_p(p, "value");
    edit_masks(s, p, "Mask Locked", |g, _, _| {
        if let Some((_, _, locked)) = mask_kind(g) {
            *locked = v.unwrap_or(!*locked);
        }
        Ok(())
    })
}

fn mask_unlock_all(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, ids) = layers_p(s, p)?;
    s.edit("Unlock All Masks", None, |proj, _| {
        for lid in &ids {
            if let Some(m) = layer_mut(proj, cid, *lid)?.props.sub_mut("masks") {
                for c in &mut m.children {
                    if let Node::Group(g) = c
                        && let Some((_, _, locked)) = mask_kind(g)
                    {
                        *locked = false;
                    }
                }
            }
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

fn mask_lock_others(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, keep) = target_masks(s, p)?;
    let layers: Vec<LayerId> = keep.iter().map(|(l, _)| *l).collect();
    s.edit("Lock Other Masks", None, |proj, _| {
        for lid in &layers {
            if let Some(m) = layer_mut(proj, cid, *lid)?.props.sub_mut("masks") {
                for c in &mut m.children {
                    let uid = c.uid();
                    if let Node::Group(g) = c
                        && let Some((_, _, locked)) = mask_kind(g)
                    {
                        *locked = !keep.contains(&(*lid, uid));
                    }
                }
            }
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

// ---------------------------------------------------------------- markers

fn add_marker(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, ids) = layers_p(s, p)?;
    let t = time_p(s, p, s.project.comp(cid));
    let comment = str_p(p, "comment").unwrap_or_default().to_string();
    if ids.is_empty() {
        s.edit("Add Marker", None, |proj, _| {
            let c = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
            c.markers.push(Marker { time: t, comment, ..Default::default() });
            c.markers.sort_by_key(|m| m.time);
            Ok(())
        })?;
        return Ok(json!({"time": t.seconds()}));
    }
    s.edit("Add Marker", None, |proj, _| {
        for lid in &ids {
            let l = layer_mut(proj, cid, *lid)?;
            if l.markers_locked {
                continue;
            }
            // Layer markers are stored in layer time.
            let lt = l.layer_time(t);
            l.markers.retain(|m| m.time != lt);
            l.markers.push(Marker { time: lt, comment: comment.clone(), ..Default::default() });
            l.markers.sort_by_key(|m| m.time);
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

fn markers_lock(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, ids) = layers_p(s, p)?;
    let v = b_p(p, "value");
    let r = s.edit("Lock Markers", None, |proj, _| {
        let c = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        let target = v.unwrap_or_else(|| !c.layers.iter().find(|l| ids.contains(&l.id)).is_some_and(|l| l.markers_locked));
        for l in c.layers.iter_mut().filter(|l| ids.contains(&l.id)) {
            l.markers_locked = target;
        }
        Ok(target)
    })?;
    Ok(json!(r))
}

fn delete_all_markers(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, ids) = layers_p(s, p)?;
    s.edit("Delete All Markers", None, |proj, _| {
        for l in proj.comp_mut(cid).ok_or(EngineError::NoComp)?.layers.iter_mut().filter(|l| ids.contains(&l.id) && !l.markers_locked) {
            l.markers.clear();
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

// ---------------------------------------------------------------- track matte

fn track_matte_op(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, ids) = layers_p(s, p)?;
    let op = str_p(p, "op").ok_or_else(|| bad("layer.trackMatte", "missing `op`"))?.to_string();
    let kind = match op.as_str() {
        "alpha" => Some(MatteKind::Alpha),
        "alphaInverted" => Some(MatteKind::AlphaInverted),
        "luma" => Some(MatteKind::Luma),
        "lumaInverted" => Some(MatteKind::LumaInverted),
        "none" | "above" | "below" => None,
        o => return Err(bad("layer.trackMatte", format!("op: none|alpha|alphaInverted|luma|lumaInverted|above|below, not `{o}`"))),
    };
    s.edit("Track Matte", None, |proj, _| {
        let c = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        let order: Vec<LayerId> = c.layers.iter().map(|l| l.id).collect();
        let mut hide = vec![];
        for lid in &ids {
            let i = order.iter().position(|x| x == lid).unwrap_or(0);
            let l = c.layer_mut(*lid).ok_or(EngineError::NoComp)?;
            let cur_kind = l.track_matte.map(|m| m.kind).unwrap_or_default();
            l.track_matte = match op.as_str() {
                "none" => None,
                "above" | "below" => {
                    let j = if op == "above" { i.checked_sub(1) } else { Some(i + 1) };
                    match j.and_then(|j| order.get(j)) {
                        Some(m) => Some(TrackMatte { layer: *m, kind: cur_kind }),
                        None => return Err(bad("layer.trackMatte", format!("there is no layer {op}"))),
                    }
                }
                _ => match (l.track_matte, i.checked_sub(1).and_then(|j| order.get(j))) {
                    (Some(m), _) => Some(TrackMatte { layer: m.layer, kind: kind.unwrap_or_default() }),
                    (None, Some(above)) => Some(TrackMatte { layer: *above, kind: kind.unwrap_or_default() }),
                    (None, None) => return Err(bad("layer.trackMatte", "the top layer has no layer above to use as a matte")),
                },
            };
            if let Some(m) = l.track_matte {
                hide.push(m.layer);
            }
        }
        for m in hide {
            if let Some(ml) = c.layer_mut(m) {
                ml.switches.video = false;
            }
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

// ---------------------------------------------------------------- open / reveal

fn source_item(s: &Session, p: &Value, cmd: &str) -> Result<ItemId> {
    let (cid, lid) = layer_p(s, p, cmd)?;
    s.project.comp(cid).and_then(|c| c.layer(lid)).and_then(|l| l.source.item()).ok_or_else(|| bad(cmd, "the layer has no source item"))
}

fn open_source(s: &mut Session, p: &Value) -> Result<Value> {
    let item = source_item(s, p, "layer.openSource")?;
    if s.project.comp(item).is_some() {
        s.open_comp(item);
        return Ok(json!({"comp": item.0}));
    }
    frontend(s, "window.panel", &json!({"panel": "layer"}))
}

/// Folder containing a file as a `file://` URL.
pub(crate) fn folder_url(path: &str) -> String {
    let dir = std::path::Path::new(path).parent().map(|d| d.to_string_lossy().to_string()).unwrap_or_default();
    file_url(&dir)
}

/// Extensions of folders that macOS opens as an app, plug-in or other bundle rather than showing
/// their contents.
const BUNDLE_EXTENSIONS: &[&str] = &[
    "app",
    "appex",
    "bundle",
    "component",
    "dext",
    "driver",
    "framework",
    "kext",
    "mdimporter",
    "mpkg",
    "osax",
    "pkg",
    "plugin",
    "prefpane",
    "qlgenerator",
    "saver",
    "service",
    "systemextension",
    "vst",
    "vst3",
    "wdgt",
    "workflow",
    "xpc",
];

/// Whether the system file manager would open folder `dir` as something other than a folder: a
/// bundle (`Tool.app`), or a Windows folder with a `{CLSID}` extension.
fn opens_as_bundle(dir: &std::path::Path) -> bool {
    let ext = dir.extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase();
    ext.starts_with('{') || BUNDLE_EXTENSIONS.contains(&ext.as_str())
}

/// The folder of the footage file `stored` (a path stored in the project) as a `file://` URL for
/// Reveal in Finder. A path that is not followed ([`crate::doc_path`]), or a folder that is not a
/// plain folder after following links, is refused.
pub(crate) fn reveal_url(s: &Session, stored: &str) -> std::result::Result<String, String> {
    let path = s.stored_path(stored).map_err(|why| crate::doc_path::refusal(stored, why))?;
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).ok_or_else(|| format!("{stored} is not in a folder"))?;
    let real = std::fs::canonicalize(dir).map_err(|e| format!("cannot show the folder of {stored}: {e}"))?;
    if !real.is_dir() || opens_as_bundle(dir) || opens_as_bundle(&real) {
        return Err(format!("the folder of {stored} is not a plain folder"));
    }
    Ok(file_url(&dir.to_string_lossy()))
}

/// `path` as a `file://` URL: backslashes become `/`, a Windows drive path gets the third slash
/// (`file:///C:/…`), and every byte that isn't a plain path character is percent-encoded (spaces,
/// `#`, `%`, `?`, non-ASCII as UTF-8), so the URL names exactly this file.
pub(crate) fn file_url(path: &str) -> String {
    use std::fmt::Write as _;
    let path = path.replace('\\', "/");
    let mut url = String::from(if path.starts_with('/') { "file://" } else { "file:///" });
    for b in path.bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~:@!$&'()*+,;=".contains(&b) {
            url.push(char::from(b));
        } else {
            let _ = write!(url, "%{b:02X}");
        }
    }
    url
}

fn reveal_in_finder(s: &mut Session, p: &Value) -> Result<Value> {
    let item = source_item(s, p, "layer.revealInFinder")?;
    let path = match s.project.item(item).map(|i| &i.kind) {
        Some(effectcraft_project::ItemKind::Footage(f)) if !f.path.is_empty() => f.path.clone(),
        _ => return Err(bad("layer.revealInFinder", "the layer's source is not a file")),
    };
    let url = reveal_url(s, &path).map_err(|e| bad("layer.revealInFinder", e))?;
    s.events.push(crate::Event::OpenUrl(url.clone()));
    Ok(json!({"url": url}))
}

fn reveal_source(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, ids) = layers_p(s, p)?;
    let mut items: Vec<ItemId> = Vec::new();
    {
        let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
        for layer in ids.iter().filter_map(|id| comp.layer(*id)) {
            if let Some(item) = layer.source.item()
                && s.project.item(item).is_some()
                && !items.contains(&item)
            {
                items.push(item);
            }
        }
    }
    if items.is_empty() {
        return Err(bad("layer.revealSource", "the selected layers have no source item"));
    }
    s.state.project_selection = items.clone();
    let raw: Vec<u64> = items.iter().map(|item| item.0).collect();
    frontend(s, "project.reveal", &json!({"items": raw}))?;
    Ok(json!({"item": items[0].0, "items": raw}))
}

fn reveal_comp(s: &mut Session, p: &Value) -> Result<Value> {
    let cid = comp_id(s, p)?;
    s.state.project_selection = vec![cid];
    frontend(s, "project.reveal", &json!({"items": [cid.0]}))?;
    Ok(json!({"item": cid.0, "items": [cid.0]}))
}

impl Session {
    /// Expressions of comp `cid` that fail at its current time: `{layer, prop, name, expression,
    /// error}` (Reveal Expression Errors, the viewer's expression error banner).
    pub fn expression_errors(&self, cid: effectcraft_project::ItemId) -> Vec<Value> {
        let t = self.time_of(cid);
        let Some(comp) = self.project.comp(cid) else { return vec![] };
        let Some(host) = &self.expr else { return vec![] };
        let mut errors = vec![];
        for l in &comp.layers {
            let mut has = vec![];
            l.props.walk("", &mut |_, pr| {
                if pr.has_expression() {
                    has.push(pr);
                }
            });
            for pr in has {
                let text = pr.expr.as_ref().map(|e| e.text.clone()).unwrap_or_default();
                let mut ctx = effectcraft_render::EvalCtx::new(&self.project, cid, comp, t);
                ctx.expr = Some(host.as_ref());
                if let Err(e) = host.eval(&ctx, l, pr, &pr.value_at(l.layer_time(t))) {
                    errors.push(json!({"layer": l.id.0, "prop": pr.uid, "name": pr.name, "expression": text, "error": e}));
                }
            }
        }
        errors
    }
}

fn reveal_expression_errors(s: &mut Session, p: &Value) -> Result<Value> {
    let cid = comp_id(s, p)?;
    s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let errors = s.expression_errors(cid);
    let props: Vec<Value> = errors.iter().map(|e| json!({"layer": e["layer"], "prop": e["prop"]})).collect();
    frontend(s, "timeline.revealProps", &json!({"props": props}))?;
    if errors.is_empty() {
        s.toast("No expression errors");
    }
    Ok(json!({"errors": errors}))
}

// ---------------------------------------------------------------- sequence layers

fn sequence(s: &mut Session, p: &Value) -> Result<Value> {
    let cid = comp_id(s, p)?;
    // Selection order (AE sequences in the order layers were selected).
    let ids: Vec<LayerId> = match p.get("layers") {
        Some(_) => layers_p(s, p)?.1,
        None => s.state.selected_layers.clone(),
    };
    if ids.len() < 2 {
        return Err(bad("layer.sequence", "select two or more layers"));
    }
    let overlap = b_p(p, "overlap").unwrap_or(false);
    let dur = if overlap { Tick::from_seconds_f64(f_p(p, "duration").unwrap_or(1.0)) } else { Tick::ZERO };
    let transition = str_p(p, "transition").unwrap_or("off").to_string();
    s.edit("Sequence Layers", None, |proj, _| {
        let c = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        let mut next: Option<Tick> = None;
        for (k, lid) in ids.iter().enumerate() {
            let l = c.layer_mut(*lid).ok_or(EngineError::NoComp)?;
            if let Some(at) = next {
                let d = at - l.in_point;
                l.start_time += d;
                l.in_point += d;
                l.out_point += d;
            }
            if overlap && transition != "off" {
                let (a, b) = (l.layer_time(l.in_point), l.layer_time(l.in_point + dur));
                let (oa, ob) = (l.layer_time(l.out_point - dur), l.layer_time(l.out_point));
                let last = k + 1 == ids.len();
                if let Some(op) = l.transform_mut().and_then(|tr| tr.get_mut("opacity")) {
                    op.keys.clear();
                    if k > 0 {
                        op.keys.push(Keyframe::new(a, KV::Scalar(0.0)));
                        op.keys.push(Keyframe::new(b, KV::Scalar(100.0)));
                    }
                    if transition == "crossDissolve" && !last {
                        op.keys.push(Keyframe::new(oa, KV::Scalar(100.0)));
                        op.keys.push(Keyframe::new(ob, KV::Scalar(0.0)));
                    }
                    op.keys.sort_by_key(|k| k.time);
                    op.keys.dedup_by_key(|k| k.time);
                }
            }
            next = Some(l.out_point - dur);
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!("layer.quality", "Quality", [], None, "{layers?, quality: best|draft|wireframe}", has_layers, quality),
        cmd!("layer.sampling", "Sampling", [], None, "{layers?, sampling: bilinear|bicubic, toggle?}", has_layers, sampling),
        cmd!("layer.frameBlending", "Frame Blending", [], None, "{layers?, mode: off|frameMix|pixelMotion}", has_layers, frame_blending),
        cmd!("layer.hideOtherVideo", "Hide Other Video", ["Layer", "Switches"], Some("Cmd+Alt+Shift+V"), "{layers?}", has_layers, hide_other_video),
        cmd!("layer.showAllVideo", "Show All Video", ["Layer", "Switches"], None, "{comp?}", has_comp, show_all_video),
        cmd!("layer.unlockAll", "Unlock All Layers", ["Layer", "Switches"], Some("Cmd+Shift+L"), "{comp?}", has_comp, unlock_all),
        cmd!("layer.expressions", "Enable/Disable Expressions", [], None, "{layers?, enabled: bool}", has_layers, expressions),
        cmd!(
            "layer.setTransform",
            "Transform Value",
            [],
            None,
            "{layers?, prop: anchor|position|scale|orientation|rotation|opacity, value}",
            has_layers,
            set_transform
        ),
        cmd!(
            "layer.centerAnchor",
            "Center Anchor Point in Layer Content",
            ["Layer", "Transform"],
            Some("Cmd+Alt+Home"),
            "{layers?}",
            has_layers,
            center_anchor
        ),
        cmd!(
            "layer.mask.shape",
            "Mask Shape...",
            ["Layer", "Mask"],
            Some("Cmd+Shift+M"),
            "{layer?, mask?, rect: [x, y, w, h], shape?: rect|ellipse}",
            has_masks,
            mask_shape
        ),
        cmd!("layer.mask.set", "Mask Settings", [], None, "{layer?, mask?, field: feather|opacity|expansion, value}", has_masks, mask_set),
        cmd!("layer.mask.reset", "Reset Mask", ["Layer", "Mask"], None, "{layer?, mask?}", has_masks, mask_reset),
        cmd!("layer.mask.remove", "Remove Mask", ["Layer", "Mask"], None, "{layer?, mask?}", has_masks, mask_remove),
        cmd!("layer.mask.removeAll", "Remove All Masks", ["Layer", "Mask"], None, "{layers?}", has_masks, mask_remove_all),
        cmd!("layer.mask.mode", "Mask Mode", [], None, "{layer?, mask?, mode: None|Add|Subtract|Intersect|Lighten|Darken|Difference}", has_masks, mask_mode),
        cmd!("layer.mask.invert", "Inverted", ["Layer", "Mask"], Some("Cmd+Shift+I"), "{layer?, mask?, value?}", has_masks, mask_invert),
        cmd!("layer.mask.lock", "Locked", ["Layer", "Mask"], None, "{layer?, mask?, value?}", has_masks, mask_lock),
        cmd!("layer.mask.unlockAll", "Unlock All Masks", ["Layer", "Mask"], None, "{layers?}", has_masks, mask_unlock_all),
        cmd!("layer.mask.lockOthers", "Lock Other Masks", ["Layer", "Mask"], None, "{layer?, mask?}", has_masks, mask_lock_others),
        cmd!("layer.addMarker", "Add Marker", ["Layer", "Markers"], None, "{layers?, time?, comment?}", has_comp, add_marker),
        cmd!("layer.markersLock", "Lock Markers", ["Layer", "Markers"], None, "{layers?, value?}", has_layers, markers_lock),
        cmd!("layer.deleteAllMarkers", "Delete All Markers", ["Layer", "Markers"], None, "{layers?}", has_markers, delete_all_markers),
        cmd!("layer.trackMatte", "Track Matte", [], None, "{layers?, op: none|alpha|alphaInverted|luma|lumaInverted|above|below}", has_layers, track_matte_op),
        cmd!("layer.openSource", "Open Layer Source", ["Layer"], None, "{layer?}", has_source_item, open_source),
        cmd!("layer.revealInFinder", "Reveal in Finder", ["Layer"], None, "{layer?}", has_footage_layer, reveal_in_finder),
        cmd!("layer.revealSource", "Reveal Layer Source in Project", ["Layer", "Reveal"], None, "{layers?}", has_source_item, reveal_source),
        cmd!("comp.revealInProject", "Reveal Composition in Project", ["Layer", "Reveal"], None, "{comp?}", has_comp, reveal_comp),
        cmd!("layer.revealExpressionErrors", "Reveal Expression Errors", ["Layer", "Reveal"], None, "{comp?}", has_comp, reveal_expression_errors),
        cmd!(
            "layer.sequence",
            "Sequence Layers...",
            ["Animation", "Keyframe Assistant"],
            None,
            "{layers? (in order), overlap?: bool, duration? (s), transition?: off|dissolveFront|crossDissolve}",
            has_multiple_layers,
            sequence
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::{file_url, folder_url};

    #[test]
    fn file_urls_are_well_formed() {
        assert_eq!(file_url("C:\\Users\\Ann\\Footage\\clip 1.mov"), "file:///C:/Users/Ann/Footage/clip%201.mov");
        assert_eq!(file_url("/home/ann/a#b%c?d.mov"), "file:///home/ann/a%23b%25c%3Fd.mov");
        assert_eq!(file_url("/Users/ann/Café/x.mov"), "file:///Users/ann/Caf%C3%A9/x.mov");
        assert_eq!(file_url("/plain/path-1_2.~(x).mov"), "file:///plain/path-1_2.~(x).mov", "plain characters stay");
    }

    #[test]
    fn folder_url_names_the_containing_folder() {
        assert_eq!(folder_url("/media/My Clips/a.mov"), "file:///media/My%20Clips");
    }

    /// Reveal in Finder shows plain folders only: a bundle the system would open as an app, or a
    /// link to one, is refused.
    #[test]
    fn reveal_refuses_folders_that_open_as_apps() {
        let d = std::env::temp_dir().join(format!("ec-reveal-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        for f in ["Clips", "Tool.app", "Shell.{20D04FE0-3AEA-1069-A2D8-08002B30309D}"] {
            std::fs::create_dir_all(d.join(f)).unwrap();
        }
        let s = crate::Session::default();
        let reveal = |folder: &str| super::reveal_url(&s, &d.join(folder).join("a.mov").to_string_lossy());
        assert_eq!(reveal("Clips"), Ok(super::file_url(&d.join("Clips").to_string_lossy())));
        assert!(reveal("Tool.app").is_err());
        assert!(reveal("Shell.{20D04FE0-3AEA-1069-A2D8-08002B30309D}").is_err());
        assert!(reveal("Gone").is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(d.join("Tool.app"), d.join("Footage")).unwrap();
            assert!(reveal("Footage").is_err(), "a link to a bundle");
        }
        let _ = std::fs::remove_dir_all(&d);
    }
}
