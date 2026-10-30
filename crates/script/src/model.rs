//! Read-only queries behind the object model (`__query(kind, json)`): project, items, layers,
//! property nodes, values and keyframes as JSON shaped for the prelude. Every edit goes through
//! engine commands instead (`__exec`), so it is undoable and journaled.

use effectcraft_engine::Session;
use effectcraft_engine::color::Label;
use effectcraft_keyframe::{Interp, TextDoc, Value as KV};
use effectcraft_project::{Comp, GroupKind, ItemId, ItemKind, Layer, LayerId, LayerSource, Node, ParamUi, PropGroup};
use effectcraft_render::EvalCtx;
use effectcraft_time::Tick;
use serde_json::{Value as J, json};

use crate::matchnames;

type R = Result<J, String>;

fn u(a: &J, k: &str) -> Option<u64> {
    a.get(k).and_then(J::as_u64)
}
fn f(a: &J, k: &str) -> Option<f64> {
    a.get(k).and_then(J::as_f64)
}

fn comp_of<'a>(s: &'a Session, a: &J) -> Result<(ItemId, &'a Comp), String> {
    let id = ItemId(u(a, "comp").ok_or("missing comp")?);
    s.project.comp(id).map(|c| (id, c)).ok_or_else(|| "the composition no longer exists".to_string())
}

fn layer_of<'a>(s: &'a Session, a: &J) -> Result<(ItemId, &'a Comp, &'a Layer), String> {
    let (cid, c) = comp_of(s, a)?;
    let l = c.layer(LayerId(u(a, "layer").ok_or("missing layer")?)).ok_or("the layer no longer exists (it was deleted)")?;
    Ok((cid, c, l))
}

pub fn query(s: &mut Session, kind: &str, a: &J) -> R {
    match kind {
        "project" => Ok(project(s)),
        "item" => item(s, a),
        "layer" => layer(s, a),
        "node" => node(s, a),
        "child" => child(s, a),
        "value" => value(s, a),
        "keys" => keys(s, a),
        "textdoc" => textdoc(s, a),
        "markers" => markers(s, a),
        "sourceRect" => source_rect(s, a),
        "pointConvert" => point_convert(s, a),
        "resolveAdd" => resolve_add(s, a),
        "menuCommandId" => Ok(menu_command_id(a.get("name").and_then(J::as_str).unwrap_or(""))),
        "commandAt" => {
            let i = u(a, "index").unwrap_or(0) as usize;
            Ok(effectcraft_engine::command_specs().get(i.wrapping_sub(1)).map(|c| json!(c.id)).unwrap_or(J::Null))
        }
        "exprError" => {
            let code = a.get("code").and_then(J::as_str).unwrap_or("");
            Ok(match s.expr_check {
                Some(check) => json!(check(code).err().unwrap_or_default()),
                None => json!(""),
            })
        }
        "labels" => Ok(json!(Label::ALL.iter().map(|l| l.name()).collect::<Vec<_>>())),
        "effects" => Ok(json!(
            effectcraft_engine::effects::all()
                .into_iter()
                .map(|e| json!({"id": e.id, "name": e.name, "matchName": matchnames::effect(e.id), "category": e.category}))
                .collect::<Vec<_>>()
        )),
        _ => Err(format!("unknown query `{kind}`")),
    }
}

fn project(s: &Session) -> J {
    let p = &s.project;
    let items: Vec<u64> = p.items.keys().map(|k| k.0).collect();
    let active = s.active_comp_id().map(|c| c.0).or_else(|| (s.state.project_selection.len() == 1).then(|| s.state.project_selection[0].0));
    json!({
        "items": items,
        "active": active,
        "selection": s.state.project_selection.iter().map(|i| i.0).collect::<Vec<_>>(),
        "path": s.path,
        "dirty": s.is_dirty(),
        "bitsPerChannel": p.settings.bit_depth.bits(),
        "linearBlending": p.settings.blend_linear,
        "linearizeWorkingSpace": p.settings.linearize,
        "workingSpace": p.settings.working_space.map(|w| format!("{w:?}")).unwrap_or_default(),
        "workingGamma": if p.settings.working_gamma_24() { 2.4 } else { 2.2 },
        "timeDisplayFrames": p.settings.time_display == effectcraft_project::TimeDisplayStyle::Frames,
        "displayStartFrame": p.settings.frame_start,
        "renderQueue": p.render_queue.len(),
        "revision": s.revision,
    })
}

fn label_index(l: Label) -> usize {
    Label::ALL.iter().position(|x| *x == l).unwrap_or(0)
}

/// Layer ids selected in `comp`, including a comp that is not the active one (#1001).
fn selected_layer_ids(s: &Session, comp: effectcraft_project::ItemId) -> Vec<u64> {
    let layers = if s.active_comp_id() == Some(comp) {
        &s.state.selected_layers
    } else {
        match s.state.comp_selections.get(&comp) {
            Some(sel) => &sel.layers,
            None => return Vec::new(),
        }
    };
    let Some(c) = s.project.comp(comp) else { return Vec::new() };
    layers.iter().filter(|l| c.layer(**l).is_some()).map(|l| l.0).collect()
}

fn layer_selected(s: &Session, comp: effectcraft_project::ItemId, layer: effectcraft_project::LayerId) -> bool {
    if s.active_comp_id() == Some(comp) {
        return s.state.selected_layers.contains(&layer);
    }
    s.state.comp_selections.get(&comp).is_some_and(|sel| sel.layers.contains(&layer))
}

/// A comp's selected properties as `[layer, uid]`: the selected ones, then those with selected
/// keyframes (selecting a keyframe selects its property in After Effects, so scripts find
/// selected keys through `selectedProperties`).
fn selected_props(s: &Session, c: &effectcraft_project::Comp) -> Vec<J> {
    let mut v: Vec<(u64, u64)> = vec![];
    let keyed = s.state.selected_keys.iter().map(|k| (k.layer, k.prop));
    for (l, p) in s.state.selected_props.iter().copied().chain(keyed) {
        if c.layer(l).is_some() && !v.contains(&(l.0, p)) {
            v.push((l.0, p));
        }
    }
    v.into_iter().map(|(l, p)| json!([l, p])).collect()
}

fn item(s: &Session, a: &J) -> R {
    let id = ItemId(u(a, "id").ok_or("missing id")?);
    let it = s.project.item(id).ok_or("the item no longer exists (it was deleted)")?;
    let mut o = json!({
        "id": id.0,
        "name": it.name,
        "typeName": it.type_name(),
        "parent": it.parent.map(|p| p.0).unwrap_or(0),
        "comment": it.comment,
        "label": label_index(it.label),
        "selected": s.state.project_selection.contains(&id),
    });
    let used_in: Vec<u64> = s.project.comps().filter(|(_, c)| c.layers.iter().any(|l| l.source.item() == Some(id))).map(|(cid, _)| cid.0).collect();
    o["usedIn"] = json!(used_in);
    // The item's proxy (footage and comps): its file and whether Use Proxy is on (#813).
    o["proxy"] = match &it.proxy {
        Some(p) => json!({"file": p.footage.path, "isStill": p.footage.kind == effectcraft_project::FootageKind::Still, "enabled": p.enabled}),
        None => J::Null,
    };
    match &it.kind {
        ItemKind::Folder => {
            o["kind"] = json!("folder");
            o["children"] = json!(s.project.items.values().filter(|c| c.parent == Some(id)).map(|c| c.id.0).collect::<Vec<_>>());
        }
        ItemKind::Comp(c) => {
            let cid = id;
            o["kind"] = json!("comp");
            let sel = selected_layer_ids(s, cid);
            let fps = c.frame_rate.as_f64();
            if let Some(m) = o.as_object_mut() {
                m.extend(
                    json!({
                        "width": c.width, "height": c.height, "pixelAspect": c.pixel_aspect,
                        "duration": c.duration.seconds(), "frameRate": fps, "frameDuration": c.frame_duration().seconds(),
                        "layers": c.layers.iter().map(|l| l.id.0).collect::<Vec<_>>(),
                        "selectedLayers": sel,
                        "selectedProperties": selected_props(s, c),
                        "time": s.time_of(cid).seconds(),
                        "workAreaStart": c.work_area.0.seconds(), "workAreaDuration": (c.work_area.1 - c.work_area.0).seconds(),
                        "bgColor": c.background, "displayStartTime": c.display_start.seconds(),
                        "hideShyLayers": c.hide_shy, "motionBlur": c.enable_motion_blur, "frameBlending": c.enable_frame_blending,
                        "draft3d": c.draft_3d, "shutterAngle": c.shutter_angle, "shutterPhase": c.shutter_phase,
                        "motionBlurSamplesPerFrame": c.motion_blur_samples,
                        "motionBlurAdaptiveSampleLimit": c.motion_blur_adaptive_limit,
                        "preserveNestedFrameRate": c.preserve_frame_rate, "preserveNestedResolution": c.preserve_resolution,
                        "renderer": match c.renderer { effectcraft_project::Renderer::Classic3D => "ADBE Classic 3D", effectcraft_project::Renderer::Advanced3D => "ADBE Advanced 3d" },
                        "open": s.state.open_comps.contains(&cid), "active": s.active_comp_id() == Some(cid),
                        "hasVideo": true, "hasAudio": c.layers.iter().any(|l| l.switches.audio && matches!(l.source, LayerSource::Footage { .. } | LayerSource::Comp { .. })),
                        "posterTime": c.poster_time.seconds(),
                    })
                    .as_object()
                    .cloned()
                    .unwrap_or_default(),
                )
            }
        }
        ItemKind::Footage(fo) => {
            o["kind"] = json!("footage");
            o["file"] = json!(fo.path);
            o["width"] = json!(fo.width);
            o["height"] = json!(fo.height);
            o["pixelAspect"] = json!(fo.pixel_aspect);
            o["duration"] = json!(if fo.kind == effectcraft_project::FootageKind::Still { 0.0 } else { fo.duration.seconds() });
            o["frameRate"] = json!(if fo.kind == effectcraft_project::FootageKind::Still { 0.0 } else { fo.frame_rate.as_f64() });
            o["hasVideo"] = json!(fo.has_video);
            o["hasAudio"] = json!(fo.has_audio);
            o["footageMissing"] = json!(fo.missing);
            o["isStill"] = json!(fo.kind == effectcraft_project::FootageKind::Still);
            o["loop"] = json!(fo.loop_count);
            // Interpret Footage ▸ Conform to frame rate: 0 when the file's own rate is used (#813).
            o["conformFrameRate"] = json!(if fo.native_rate.is_some() { fo.frame_rate.as_f64() } else { 0.0 });
            o["nativeFrameRate"] = json!(fo.native_rate.unwrap_or(fo.frame_rate).as_f64());
        }
        ItemKind::Solid(so) => {
            o["kind"] = json!("solid");
            o["width"] = json!(so.width);
            o["height"] = json!(so.height);
            o["pixelAspect"] = json!(so.pixel_aspect);
            o["color"] = json!(so.color);
            o["duration"] = json!(0.0);
            o["frameRate"] = json!(0.0);
            o["hasVideo"] = json!(true);
            o["hasAudio"] = json!(false);
            o["isStill"] = json!(true);
        }
    }
    Ok(o)
}

fn layer(s: &Session, a: &J) -> R {
    let (cid, c, l) = layer_of(s, a)?;
    let kind = match &l.source {
        LayerSource::Text => "text",
        LayerSource::Shape => "shape",
        LayerSource::Camera => "camera",
        LayerSource::Light { .. } => "light",
        _ => "av",
    };
    let (w, h) = effectcraft_render::source_size(&s.project, l);
    let t = s.time_of(cid);
    Ok(json!({
        "id": l.id.0,
        "comp": cid.0,
        "kind": kind,
        "name": l.name,
        "index": c.index_of(l.id).unwrap_or(0),
        "matchName": matchnames::layer(&l.source),
        "root": l.props.uid,
        "source": l.source.item().map(|i| i.0),
        "nullLayer": matches!(l.source, LayerSource::Null),
        "lightType": match &l.source { LayerSource::Light { kind } => kind.label(), _ => "" },
        "enabled": l.switches.video,
        "audioEnabled": l.switches.audio,
        "solo": l.switches.solo,
        "locked": l.switches.locked,
        "shy": l.switches.shy,
        "collapse": l.switches.collapse,
        "quality": format!("{:?}", l.switches.quality),
        "samplingQuality": format!("{:?}", l.switches.sampling),
        "effectsActive": l.switches.effects,
        "frameBlending": format!("{:?}", l.switches.frame_blend),
        "motionBlur": l.switches.motion_blur,
        "adjustmentLayer": l.switches.adjustment,
        "threeDLayer": l.switches.three_d,
        "guideLayer": l.switches.guide,
        "preserveTransparency": l.preserve_transparency,
        "inPoint": l.in_point.seconds(),
        "outPoint": l.out_point.seconds(),
        "startTime": l.start_time.seconds(),
        "stretch": l.stretch,
        "time": l.layer_time(t).seconds(),
        "active": l.switches.video && l.is_active_at(t),
        "parent": l.parent.map(|p| p.0),
        "blendingMode": l.blend_mode.label(),
        "trackMatte": l.track_matte.map(|m| json!({"layer": m.layer.0, "kind": format!("{:?}", m.kind)})),
        "isTrackMatte": c.layers.iter().any(|x| x.track_matte.is_some_and(|m| m.layer == l.id)),
        "label": label_index(l.label),
        "comment": l.comment,
        "selected": layer_selected(s, cid, l.id),
        "autoOrient": format!("{:?}", l.auto_orient),
        "width": w, "height": h,
        "hasVideo": l.source.is_av(),
        "hasAudio": matches!(l.source, LayerSource::Footage { .. } | LayerSource::Comp { .. }) && l.props.sub("audio").is_some(),
        "timeRemapEnabled": l.props.prop("timeRemap").is_some(),
    }))
}

/// Node at the end of a uid chain below the layer root, and the chain itself.
fn chain(l: &Layer, uid: u64) -> Option<Vec<&Node>> {
    l.props.node_chain(uid)
}

/// PropertyValueType of a property.
fn value_type(p: &effectcraft_project::Property) -> &'static str {
    if matches!(p.ui, ParamUi::Mask) {
        return "MASK_INDEX";
    }
    match &p.value {
        KV::Scalar(_) | KV::Bool(_) | KV::Enum(_) => "OneD",
        KV::Vec2(_) => {
            if p.spatial {
                "TwoD_SPATIAL"
            } else {
                "TwoD"
            }
        }
        KV::Vec3(_) => {
            if p.spatial {
                "ThreeD_SPATIAL"
            } else {
                "ThreeD"
            }
        }
        KV::Color(_) => "COLOR",
        KV::Path(_) => "SHAPE",
        KV::Text(_) => "TEXT_DOCUMENT",
        KV::Layer(_) => "LAYER_INDEX",
        KV::Gradient(_) | KV::Str(_) => "CUSTOM_VALUE",
    }
}

/// Groups whose children are instances (effects, masks, shape items, animators).
fn indexed_parade(g: &PropGroup) -> bool {
    matches!(g.match_id.as_str(), "effects" | "masks" | "contents" | "animators" | "selectors" | "layerStyles" | "motionTrackers")
}

fn node_json(c: &Comp, l: &Layer, uid: u64) -> R {
    if uid == 0 || uid == l.props.uid {
        let n = l.props.children.len();
        return Ok(json!({
            "uid": l.props.uid, "kind": "group", "name": l.name, "matchName": matchnames::layer(&l.source), "matchId": "layer",
            "index": c.index_of(l.id).unwrap_or(0), "depth": 0, "parent": null, "isLayer": true, "numProperties": n,
            "children": l.props.children.iter().map(Node::uid).collect::<Vec<_>>(), "propertyType": "NAMED_GROUP",
            "enabled": l.switches.video, "canSetEnabled": true,
        }));
    }
    let ch = chain(l, uid).ok_or("the property no longer exists (it was deleted)")?;
    let node = *ch.last().ok_or("bad property")?;
    let parent_group: &PropGroup = if ch.len() >= 2 { ch[ch.len() - 2].as_group().ok_or("bad tree")? } else { &l.props };
    let index = parent_group.children.iter().position(|x| x.uid() == uid).map(|i| i + 1).unwrap_or(0);
    let parent_uid = if ch.len() >= 2 { ch[ch.len() - 2].uid() } else { l.props.uid };
    let match_name = matchnames::node(&ch);
    let instance_parent = indexed_parade(parent_group) && ch.len() >= 2 || (ch.len() == 2 && ch[0].match_id() == "contents");
    let mut o = json!({
        "uid": uid, "name": node.name(), "matchName": match_name, "matchId": node.match_id(), "index": index,
        "depth": ch.len(), "parent": parent_uid, "isLayer": false,
        "isEffect": matches!(node.as_group().map(|g| &g.kind), Some(GroupKind::Effect { .. })),
        "isMask": matches!(node.as_group().map(|g| &g.kind), Some(GroupKind::Mask { .. })),
    });
    match node {
        Node::Group(g) => {
            let instance = matches!(g.kind, GroupKind::Indexed | GroupKind::Effect { .. } | GroupKind::Mask { .. } | GroupKind::Tracker { .. });
            o["kind"] = json!("group");
            o["numProperties"] = json!(g.children.len());
            o["children"] = json!(g.children.iter().map(Node::uid).collect::<Vec<_>>());
            o["propertyType"] = json!(if indexed_parade(g) { "INDEXED_GROUP" } else { "NAMED_GROUP" });
            o["enabled"] = json!(g.enabled);
            o["canSetEnabled"] = json!(instance);
            o["instance"] = json!(instance || instance_parent);
            o["effectId"] = json!(match &g.kind {
                GroupKind::Effect { effect } => Some(effect.clone()),
                _ => None,
            });
            if let GroupKind::Mask { mode, inverted, color, locked, .. } = &g.kind {
                o["maskMode"] = json!(mode.label());
                o["inverted"] = json!(inverted);
                o["color"] = json!([color[0] as f64 / 255.0, color[1] as f64 / 255.0, color[2] as f64 / 255.0]);
                o["locked"] = json!(locked);
            }
        }
        Node::Prop(p) => {
            o["kind"] = json!("prop");
            o["propertyType"] = json!("PROPERTY");
            o["valueType"] = json!(value_type(p));
            o["type"] = json!(p.value.kind_name());
            o["dims"] = json!(p.value.dims());
            o["spatial"] = json!(p.spatial);
            o["interpolates"] = json!(p.value.interpolates());
            o["numKeys"] = json!(p.keys.len());
            o["expression"] = json!(p.expr.as_ref().map(|e| e.text.clone()).unwrap_or_default());
            o["expressionEnabled"] = json!(p.has_expression());
            o["canVaryOverTime"] = json!(!p.static_only);
            o["canSetExpression"] = json!(!p.static_only && !matches!(p.ui, ParamUi::Hidden));
            o["hidden"] = json!(matches!(p.ui, ParamUi::Hidden));
            o["enabled"] = json!(true);
            o["canSetEnabled"] = json!(false);
            o["isModified"] = json!(p.is_animated() || p.has_expression());
            if let ParamUi::Slider { min, max, .. } = p.ui {
                o["min"] = json!(min);
                o["max"] = json!(max);
            }
            if let ParamUi::Popup { options } = &p.ui {
                o["options"] = json!(options);
            }
            o["ui"] = json!(match &p.ui {
                ParamUi::Percent => "%",
                ParamUi::Angle => "degrees",
                ParamUi::Pixels | ParamUi::Point | ParamUi::Point3 => "pixels",
                _ => "",
            });
            // Position with Separate Dimensions.
            o["separated"] = json!(node.match_id() == "position" && parent_group.match_id == "transform" && parent_group.get("positionX").is_some());
            o["separationFollower"] = json!(matches!(node.match_id(), "positionX" | "positionY" | "positionZ"));
        }
    }
    Ok(o)
}

fn node(s: &Session, a: &J) -> R {
    let (_, c, l) = layer_of(s, a)?;
    node_json(c, l, u(a, "uid").unwrap_or(0))
}

/// Normalized name for loose lookups: `Anchor Point`, `anchorPoint`, `anchor_point` → `anchorpoint`.
fn loose(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_ascii_lowercase()
}

/// AE-style aliases for our match ids (attribute names scripts use).
fn alias(k: &str) -> Option<&'static str> {
    Some(match k {
        "anchorpoint" => "anchor",
        "pointofinterest" => "poi",
        "xrotation" => "rotationX",
        "yrotation" => "rotationY",
        "zrotation" => "rotation",
        "xposition" => "positionX",
        "yposition" => "positionY",
        "zposition" => "positionZ",
        "effect" | "effects" => "effects",
        "mask" | "masks" => "masks",
        "content" | "contents" => "contents",
        "maskshape" | "maskpath" => "path",
        "maskfeather" => "feather",
        // A light's options name it "ADBE Casts Shadows" in After Effects (#1063).
        "adbecastsshadows" => "castsShadows",
        "maskopacity" => "opacity",
        "maskexpansion" | "maskoffset" => "expansion",
        "sourcetext" => "sourceText",
        "timeremap" => "timeRemap",
        "audiolevels" => "levels",
        _ => return None,
    })
}

/// How many groups below a property group a string key may search (#1063: Pro Levels2's Alpha
/// controls sit one group down, under Alpha).
const NESTED_CHILD_DEPTH: u8 = 3;

/// The child of `g` that a string key names. The direct children are searched first, then the
/// groups nested below them, up to `depth` levels.
fn find_child<'a>(l: &'a Layer, g: &'a PropGroup, k: &str, depth: u8) -> Option<&'a Node> {
    let chain_of = |n: &Node| -> String {
        let mut ch = if g.uid == l.props.uid { vec![] } else { chain(l, g.uid).unwrap_or_default() };
        ch.push(n);
        matchnames::node(&ch)
    };
    let lk = loose(k);
    g.children
        .iter()
        .find(|n| chain_of(n) == k)
        .or_else(|| g.children.iter().find(|n| n.name() == k))
        .or_else(|| g.children.iter().find(|n| n.match_id() == k))
        .or_else(|| g.children.iter().find(|n| n.name().eq_ignore_ascii_case(k)))
        .or_else(|| g.children.iter().find(|n| loose(n.name()) == lk || loose(n.match_id()) == lk))
        .or_else(|| alias(&lk).and_then(|m| g.children.iter().find(|n| n.match_id() == m)))
        .or_else(|| {
            if depth == 0 {
                return None;
            }
            g.children.iter().find_map(|n| match n {
                Node::Group(sub) => find_child(l, sub, k, depth - 1),
                Node::Prop(_) => None,
            })
        })
}

fn child(s: &Session, a: &J) -> R {
    let (_, c, l) = layer_of(s, a)?;
    let uid = u(a, "uid").unwrap_or(0);
    let g: &PropGroup = if uid == 0 || uid == l.props.uid { &l.props } else { l.props.find_group(uid).ok_or("not a property group")? };
    let key = a.get("key").cloned().unwrap_or(J::Null);
    let found = match &key {
        J::Number(n) => n.as_u64().and_then(|i| g.children.get((i as usize).checked_sub(1)?)),
        J::String(k) => find_child(l, g, k, NESTED_CHILD_DEPTH),
        _ => None,
    };
    match found {
        Some(n) => node_json(c, l, n.uid()),
        None => Ok(J::Null),
    }
}

/// A value as the object model shows it: popups 1-based, checkboxes 0/1, layer references as
/// layer indices, paths as `{vertices, inTangents, outTangents, closed}`, text as the document.
fn js_value(c: &Comp, v: &KV) -> J {
    match v {
        KV::Bool(b) => json!(if *b { 1 } else { 0 }),
        KV::Enum(i) => json!(*i + 1),
        KV::Layer(l) => json!(l.and_then(|id| c.index_of(LayerId(id))).unwrap_or(0)),
        KV::Path(p) => json!({"vertices": p.vertices, "inTangents": p.in_tangents, "outTangents": p.out_tangents, "closed": p.closed}),
        KV::Text(d) => text_json(d),
        v => v.to_json(),
    }
}

/// A text document for `TextDocument`: the serialized document plus the first character's style.
fn text_json(d: &TextDoc) -> J {
    let mut doc = serde_json::to_value(d).unwrap_or(J::Null);
    let st = d.style_at(0);
    if let Some(o) = doc.as_object_mut() {
        o.insert("first".into(), effectcraft_keyframe::text_doc::char_style_json(&st));
        o.insert("para".into(), effectcraft_keyframe::text_doc::para_style_json(&d.para(0)));
    }
    doc
}

fn value(s: &Session, a: &J) -> R {
    let (cid, c, l) = layer_of(s, a)?;
    let uid = u(a, "uid").ok_or("missing uid")?;
    let p = l.props.find(uid).ok_or("the property no longer exists (it was deleted)")?;
    let t = f(a, "time").map(Tick::from_seconds_f64).unwrap_or_else(|| s.time_of(cid));
    let pre = a.get("pre").and_then(J::as_bool).unwrap_or(false);
    let v = if pre || !p.has_expression() {
        p.value_at(l.layer_time(t))
    } else {
        let mut ctx = EvalCtx::new(&s.project, cid, c, t);
        ctx.expr = s.expr.as_deref();
        // Sample expressions (`sampleImage`) read footage through the session's source; without
        // it footage layers sample as transparent (#1000).
        ctx.footage = Some(s.footage.as_ref());
        ctx.value(l, p)
    };
    Ok(js_value(c, &v))
}

fn interp(i: Interp) -> &'static str {
    match i {
        Interp::Linear => "LINEAR",
        Interp::Bezier => "BEZIER",
        Interp::Hold => "HOLD",
    }
}

fn keys(s: &Session, a: &J) -> R {
    let (cid, c, l) = layer_of(s, a)?;
    let uid = u(a, "uid").ok_or("missing uid")?;
    let p = l.props.find(uid).ok_or("the property no longer exists (it was deleted)")?;
    let sel: Vec<Tick> = s.state.selected_keys.iter().filter(|k| k.layer == l.id && k.prop == uid).map(|k| k.time).collect();
    let _ = cid;
    let dims = if p.spatial { 1 } else { p.value.dims().max(1) };
    // Each side's ease as it plays (After Effects reports linear and auto-Bezier sides this way
    // too), else as stored; influence is stored as a fraction, scripts see percent.
    let ease = |i: usize, out: bool| -> J {
        let Some(k) = p.keys.get(i) else { return json!([]) };
        let stored = if out { &k.out_ease } else { &k.in_ease };
        let v: Vec<J> = (0..dims)
            .map(|d| {
                let e =
                    effectcraft_keyframe::side_ease(&p.keys, i, d, p.spatial, out).or_else(|| stored.get(d).or(stored.first()).copied()).unwrap_or_default();
                json!({"speed": e.speed, "influence": e.influence * 100.0})
            })
            .collect();
        J::Array(v)
    };
    let key = |i: usize, k: &effectcraft_keyframe::Keyframe| -> J {
        // Spatial tangents as the motion path plays (auto-Bezier and continuous keys too).
        let (spatial_in, spatial_out) = if p.spatial { effectcraft_keyframe::spatial_tangents(&p.keys, i) } else { (k.spatial_in, k.spatial_out) };
        json!({
            "time": l.comp_time(k.time).seconds(),
            "layerTime": k.time.seconds(),
            "value": js_value(c, &k.value),
            "inInterp": interp(k.in_interp), "outInterp": interp(k.out_interp),
            "inEase": ease(i, false), "outEase": ease(i, true),
            "continuous": k.continuous, "autoBezier": k.auto_bezier, "roving": k.roving,
            "spatialIn": spatial_in, "spatialOut": spatial_out,
            "spatialContinuous": k.spatial_continuous, "spatialAutoBezier": k.spatial_auto,
            "selected": sel.contains(&k.time),
        })
    };
    // `index` (zero-based): only that key, as `{count, key}` (`key` null when out of range), so
    // looking keys up one at a time doesn't serialise the whole list each time (#814).
    if let Some(i) = u(a, "index") {
        let k = usize::try_from(i).ok().and_then(|i| p.keys.get(i).map(|k| key(i, k)));
        return Ok(json!({"count": p.keys.len(), "key": k}));
    }
    Ok(J::Array(p.keys.iter().enumerate().map(|(i, k)| key(i, k)).collect()))
}

/// Apply `layer.setText` attribute keys to the Source Text document at `time` and return the new
/// document (for `prop.set` / `prop.addKey`).
fn textdoc(s: &Session, a: &J) -> R {
    let (cid, _, l) = layer_of(s, a)?;
    let uid = u(a, "uid").ok_or("missing uid")?;
    let p = l.props.find(uid).ok_or("the property no longer exists")?;
    let t = f(a, "time").map(Tick::from_seconds_f64).unwrap_or_else(|| s.time_of(cid));
    let KV::Text(doc) = p.value_at(l.layer_time(t)) else { return Err("not a Source Text property".into()) };
    let mut d = *doc;
    if let Some(attrs) = a.get("attrs").and_then(J::as_object) {
        if let Some(text) = attrs.get("text").and_then(J::as_str) {
            d.set_text(text);
        }
        for (k, v) in attrs {
            if k == "text" || v.is_null() {
                continue;
            }
            match d.set_attr(k, v, None) {
                Ok(true) => {}
                Ok(false) => return Err(format!("TextDocument: unknown attribute `{k}`")),
                Err(e) => return Err(format!("TextDocument.{k}: {e}")),
            }
        }
    }
    serde_json::to_value(&d).map_err(|e| e.to_string())
}

fn markers(s: &Session, a: &J) -> R {
    let (_, c) = comp_of(s, a)?;
    let (list, to_comp): (&[effectcraft_project::Marker], Option<&Layer>) = match u(a, "layer") {
        Some(lid) => {
            let l = c.layer(LayerId(lid)).ok_or("the layer no longer exists")?;
            (&l.markers, Some(l))
        }
        None => (&c.markers, None),
    };
    let out: Vec<J> = list
        .iter()
        .map(|m| {
            let t = to_comp.map(|l| l.comp_time(m.time)).unwrap_or(m.time);
            json!({
                "time": t.seconds(), "duration": m.duration.seconds(), "comment": m.comment, "chapter": m.chapter, "url": m.url,
                "frameTarget": m.frame_target, "label": label_index(m.label), "protectedRegion": m.protected,
                "cuePointName": m.cue_point.as_ref().map(|c| c.name.clone()).unwrap_or_default(),
                "eventCuePoint": m.cue_point.as_ref().is_some_and(|c| !c.navigation),
                "params": m.cue_point.as_ref().map(|c| c.params.clone()).unwrap_or_default(),
            })
        })
        .collect();
    Ok(J::Array(out))
}

/// A layer's source point in comp space (`toComp`) or the reverse, through the layer's transform
/// and its parents' at comp time (#808: `sourcePointToComp`, `compPointToSource`).
fn point_convert(s: &Session, a: &J) -> R {
    let (cid, c, l) = layer_of(s, a)?;
    let t = f(a, "time").map(Tick::from_seconds_f64).unwrap_or_else(|| s.time_of(cid));
    let p = a.get("point").and_then(J::as_array).ok_or("missing point")?;
    let x = p.first().and_then(J::as_f64).unwrap_or(0.0);
    let y = p.get(1).and_then(J::as_f64).unwrap_or(0.0);
    let mut ctx = EvalCtx::new(&s.project, cid, c, t);
    ctx.expr = s.expr.as_deref();
    let (layer_space, _) = ctx.layer_to_comp(l);
    let m = if a.get("toComp").and_then(J::as_bool).unwrap_or(true) {
        layer_space
    } else {
        layer_space.inverse().ok_or("the layer's transform can't be inverted")?
    };
    let k = m.0;
    let w = k[2][0] * x + k[2][1] * y + k[2][2];
    let (ox, oy) = (k[0][0] * x + k[0][1] * y + k[0][2], k[1][0] * x + k[1][1] * y + k[1][2]);
    let (ox, oy) = if w != 0.0 && w.is_finite() { (ox / w, oy / w) } else { (ox, oy) };
    Ok(json!([ox, oy]))
}

fn source_rect(s: &Session, a: &J) -> R {
    let (cid, c, l) = layer_of(s, a)?;
    let t = f(a, "time").map(Tick::from_seconds_f64).unwrap_or_else(|| s.time_of(cid));
    let mut ctx = EvalCtx::new(&s.project, cid, c, t);
    ctx.expr = s.expr.as_deref();
    let r = match &l.source {
        LayerSource::Text => {
            let paths: Vec<_> = effectcraft_render::text::glyph_paths(&ctx, l).into_iter().map(|(p, _)| p).collect();
            effectcraft_path::bounds(&paths).map(|b| [b.y0, b.x0, b.width(), b.height()]).unwrap_or([0.0; 4])
        }
        LayerSource::Shape => match l.props.sub("contents") {
            Some(contents) => {
                let buf = effectcraft_render::shapes::render(&ctx, l, contents, 1.0);
                if buf.img.width <= 4 { [0.0; 4] } else { [2.0 - buf.offset[1], 2.0 - buf.offset[0], buf.img.width as f64 - 4.0, buf.img.height as f64 - 4.0] }
            }
            None => [0.0; 4],
        },
        _ => {
            let (w, h) = effectcraft_render::source_size(&s.project, l);
            [0.0, 0.0, w as f64, h as f64]
        }
    };
    Ok(json!({"top": r[0], "left": r[1], "width": r[2], "height": r[3]}))
}

/// What `PropertyGroup.addProperty(name)` adds to the group `uid`.
fn resolve_add(s: &Session, a: &J) -> R {
    let (_, _, l) = layer_of(s, a)?;
    let uid = u(a, "uid").unwrap_or(0);
    let name = a.get("name").and_then(J::as_str).unwrap_or("");
    let g: &PropGroup = if uid == 0 || uid == l.props.uid { &l.props } else { l.props.find_group(uid).ok_or("not a property group")? };
    let in_contents = g.match_id == "contents";
    let in_group = l.props.sub("contents").is_some_and(|c| c.uid == g.uid || c.find_group(g.uid).is_some());
    Ok(match g.match_id.as_str() {
        "effects" => {
            let id = matchnames::effect_from_match(name).map(str::to_string).or_else(|| effectcraft_engine::effects::lookup(name).map(|e| e.id.to_string()));
            match id {
                Some(id) => json!({"kind": "effect", "effect": id}),
                None => json!({"error": format!("Can not add a property with name \"{name}\" to this PropertyGroup (unknown effect)")}),
            }
        }
        "masks" => {
            if name == "ADBE Mask Atom" || loose(name) == "mask" {
                json!({"kind": "mask"})
            } else {
                json!({"error": format!("Can not add \"{name}\" to Masks (use \"ADBE Mask Atom\")")})
            }
        }
        "animators" => json!({"kind": "animator"}),
        // A stroke always holds Dash 1, Gap 1 and Offset (Dash 0 draws no dashes); adding one of
        // them returns it, Dash/Gap 2 and 3 are added as a pair (`shape.dashes.add`) (#815).
        "dashes" if in_group => match matchnames::stroke_dash(name) {
            Some(id) => match g.children.iter().find(|n| n.match_id() == id) {
                Some(n) => json!({"kind": "existing", "uid": n.uid()}),
                None => json!({"kind": "dash", "id": id, "stroke": l.props.parent_of(g.uid).map(|p| p.uid)}),
            },
            None => json!({"error": format!("Can not add \"{name}\" to Dashes")}),
        },
        _ if in_contents && in_group => match matchnames::shape_kind(name) {
            Some(k) => {
                json!({"kind": "shape", "shape": k, "group": if l.props.sub("contents").is_some_and(|c| c.uid == g.uid) { J::Null } else { json!(g.uid) }})
            }
            None => json!({"error": format!("Can not add \"{name}\" to Contents")}),
        },
        "properties" if l.props.group("text/animators").is_some_and(|a| a.find_group(g.uid).is_some()) => {
            let animator = l.props.group("text/animators").and_then(|a| a.groups().find(|x| x.find_group(g.uid).is_some()).map(|x| x.uid));
            match text_animator_property(name) {
                Some(k) => json!({"kind": "animatorProperty", "property": k, "animator": animator}),
                None => json!({"error": format!("Can not add \"{name}\" to an animator's Properties")}),
            }
        }
        "selectors" if l.props.group("text/animators").is_some_and(|a| a.find_group(g.uid).is_some()) => {
            let animator = l.props.group("text/animators").and_then(|a| a.groups().find(|x| x.find_group(g.uid).is_some()).map(|x| x.uid));
            let kind = match name {
                "ADBE Text Wiggly Selector" => "wiggly",
                "ADBE Text Expressible Selector" => "expression",
                n if loose(n).contains("wiggly") => "wiggly",
                n if loose(n).contains("expression") => "expression",
                _ => "range",
            };
            json!({"kind": "selector", "selector": kind, "animator": animator})
        }
        _ => json!({"error": format!("Can not add a property to \"{}\"", g.name)}),
    })
}

/// A text animator property (`layer.addTextAnimatorProperty` key) from an After Effects match
/// name or a display name.
fn text_animator_property(name: &str) -> Option<&'static str> {
    let table = matchnames::TEXT_ANIMATOR_PROPERTIES;
    if let Some((k, _)) = table.iter().find(|(_, m)| m.eq_ignore_ascii_case(name)) {
        return Some(k);
    }
    let n = loose(name);
    table.iter().map(|(k, _)| *k).find(|k| loose(k) == n || (n == "anchorpoint" && *k == "anchor") || (n == "trackingamount" && *k == "tracking"))
}

/// `app.findMenuCommandId(name)`: 1-based index of the command whose label matches (menu items
/// first), 0 when none does.
fn menu_command_id(name: &str) -> J {
    let norm = |s: &str| s.trim().trim_end_matches("...").trim_end_matches('…').trim().to_ascii_lowercase();
    let n = norm(name);
    let specs = effectcraft_engine::command_specs();
    let pick = specs
        .iter()
        .position(|c| !c.menu.is_empty() && norm(c.label) == n)
        .or_else(|| specs.iter().position(|c| norm(c.label) == n))
        .or_else(|| specs.iter().position(|c| c.id == name));
    json!(pick.map(|i| i + 1).unwrap_or(0))
}
