use super::create::{color_p, pixel_aspect_p};
use super::{
    EngineError, ItemKind, KV, LayerId, LayerSource, PropGroup, Result, Session, Tick, Value, bad, has_layers, json, layer_mut, layer_p, layers_p, limits,
    str_p, unlocked,
};

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

pub(super) fn transform_op(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, ids) = layers_p(s, p)?;
    let ids = unlocked(s, cid, ids, "layer.transform")?;
    let op = str_p(p, "op").unwrap_or("reset").to_string();
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?.clone();
    let t = s.time();
    // A shape or text layer has no source item, so source_size is (0, 0): the fit ops size it by
    // its painted bounds at the current time instead, and anchor at the bounds' centre — content
    // bounds live in layer space, where a shape is centred on the origin and text sits above its
    // baseline, so (w/2, h/2) would push it off-centre (#2198). Reset and the other ops keep the
    // source size.
    let ectx = effectcraft_render::EvalCtx {
        expr: s.expr.as_deref(),
        footage: Some(s.footage.as_ref()),
        ..effectcraft_render::EvalCtx::new(&s.project, cid, &comp, t)
    };
    let fitting = matches!(op.as_str(), "fit" | "fitWidth" | "fitHeight");
    let sizes: Vec<(LayerId, [f64; 2], [f64; 2])> = comp
        .layers
        .iter()
        .filter(|l| ids.contains(&l.id))
        .map(|l| {
            let (w, h) = effectcraft_render::source_size(&s.project, l);
            if fitting && (w == 0 || h == 0) {
                match effectcraft_render::content_bounds(&ectx, l).filter(|b| b.iter().all(|v| v.is_finite()) && b[2] > b[0] && b[3] > b[1]) {
                    Some(b) => (l.id, [b[2] - b[0], b[3] - b[1]], [(b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0]),
                    None => (l.id, [0.0; 2], [0.0; 2]),
                }
            } else {
                (l.id, [w as f64, h as f64], [w as f64 / 2.0, h as f64 / 2.0])
            }
        })
        .collect();
    s.edit("Transform", None, |proj, _| {
        for (lid, size, anchor) in &sizes {
            let l = layer_mut(proj, cid, *lid)?;
            let lt = l.layer_time(t);
            let (cw, ch) = (comp.width as f64, comp.height as f64);
            let (w, h) = (size[0], size[1]);
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
                    // A tiny shape's bounds can overflow the scale; refuse rather than write infinity.
                    if !(sx * 100.0).is_finite() || !(sy * 100.0).is_finite() {
                        return Err(EngineError::Other(format!("layer {} is too small to fit to the composition", lid.0)));
                    }
                    let z = |m: &str, d: f64| tr.get(m).map(|p| p.value_at(lt).as_vec3()[2]).unwrap_or(d);
                    let (sz, az) = (z("scale", 100.0), z("anchor", 0.0));
                    set(tr, "scale", KV::Vec3([sx * 100.0, sy * 100.0, sz]));
                    update_position(tr, lt, |p| [cw / 2.0, ch / 2.0, p[2]]);
                    set(tr, "anchor", KV::Vec3([anchor[0], anchor[1], az]));
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
pub(super) fn layer_settings(s: &mut Session, p: &Value) -> Result<Value> {
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
pub(super) fn has_layer_settings(s: &Session) -> std::result::Result<(), String> {
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
