//! Text animation: Animation ▸ Animate Text / Add Text Selector, the Timeline's "Animate:" and
//! "Add:" pop-ups, Enable Per-character 3D, and EffectCraft's own text animator presets.

use effectcraft_keyframe::{Keyframe, Value as KV};
use effectcraft_project::build::{self, Ids};
use effectcraft_project::{Layer, LayerSource, Node, PropGroup};
use effectcraft_time::Tick;
use serde_json::{Value, json};
use std::sync::OnceLock;

use super::{CommandSpec, b_p, bad, has_comp, layer_mut, layer_p, str_p};
use crate::{Result, Session, cmd, query};

/// The original text animator presets (`presets/text_animators.json`).
pub const PRESETS_JSON: &str = include_str!("../../presets/text_animators.json");

fn presets() -> &'static [Value] {
    static PRESETS: OnceLock<Vec<Value>> = OnceLock::new();
    PRESETS
        .get_or_init(|| serde_json::from_str::<Value>(PRESETS_JSON).ok().and_then(|v| v.get("presets").and_then(Value::as_array).cloned()).unwrap_or_default())
}

/// (id, name) of every preset.
pub fn preset_names() -> Vec<(String, String)> {
    presets().iter().filter_map(|p| Some((p.get("id")?.as_str()?.to_string(), p.get("name")?.as_str()?.to_string()))).collect()
}

/// Enabled when a text layer is selected.
fn has_text_layer(s: &Session) -> std::result::Result<(), String> {
    has_comp(s)?;
    let comp = s.active_comp().ok_or("no composition is open")?;
    if s.state.selected_layers.iter().any(|id| comp.layer(*id).is_some_and(|l| matches!(l.source, LayerSource::Text))) {
        Ok(())
    } else {
        Err("select a text layer first".into())
    }
}

/// Enabled when a selected text layer has an animator.
fn has_animator(s: &Session) -> std::result::Result<(), String> {
    has_text_layer(s)?;
    let comp = s.active_comp().ok_or("no composition is open")?;
    let any =
        s.state.selected_layers.iter().filter_map(|id| comp.layer(*id)).any(|l| l.props.group("text/animators").is_some_and(|a| a.groups().next().is_some()));
    if any { Ok(()) } else { Err("add a text animator first".into()) }
}

fn per_char_on(l: &Layer) -> bool {
    l.props.prop("text/perChar3d").is_some_and(|p| p.value.as_bool())
}

/// The animator a command targets: `animator` (uid or 1-based index), else the animator holding
/// a selected property, else the last one.
fn animator_uid(s: &Session, l: &Layer, p: &Value, cmd: &str) -> Result<u64> {
    let anims = l.props.group("text/animators").ok_or_else(|| bad(cmd, "not a text layer"))?;
    let list: Vec<&PropGroup> = anims.groups().collect();
    if let Some(v) = p.get("animator") {
        let n = v.as_u64().ok_or_else(|| bad(cmd, "`animator` must be a uid or 1-based index"))?;
        if let Some(g) = list.iter().find(|g| g.uid == n) {
            return Ok(g.uid);
        }
        return list.get((n as usize).wrapping_sub(1)).map(|g| g.uid).ok_or_else(|| bad(cmd, format!("no animator {n}")));
    }
    for (lid, uid) in &s.state.selected_props {
        if *lid != l.id {
            continue;
        }
        if let Some(g) = list.iter().find(|g| g.uid == *uid || g.find(*uid).is_some() || g.find_group(*uid).is_some()) {
            return Ok(g.uid);
        }
    }
    list.last().map(|g| g.uid).ok_or_else(|| bad(cmd, "the layer has no text animators"))
}

/// Add the properties of `kind` to an animator's Properties group (skipping ones it has).
fn add_props(ids: &mut Ids, anim: &mut PropGroup, kind: &str, three_d: bool) -> usize {
    let list = build::text_anim_props(ids, kind, three_d);
    if anim.sub("properties").is_none() {
        let g = ids.group("properties", "Properties");
        anim.children.push(g.into());
    }
    let Some(props) = anim.sub_mut("properties") else { return 0 };
    let mut n = 0;
    for pr in list {
        if props.get(&pr.match_id).is_none() {
            props.children.push(pr.into());
            n += 1;
        }
    }
    n
}

fn kinds_p(p: &Value) -> Vec<String> {
    match p.get("properties").or(p.get("property")) {
        Some(Value::Array(a)) => a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect(),
        Some(Value::String(x)) => vec![x.clone()],
        _ => vec!["opacity".into()],
    }
}

fn add_animator(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "layer.addTextAnimator";
    let (cid, lid) = layer_p(s, p, C)?;
    let kinds = kinds_p(p);
    if kinds.iter().any(|k| k == "perChar3d") {
        return enable_per_char(s, &json!({"layer": lid.0, "comp": cid.0}));
    }
    let uid = s.edit("Add Text Animator", None, |proj, _| {
        let mut next = proj.next_id;
        let mut ids = Ids(&mut next);
        let l = layer_mut(proj, cid, lid)?;
        let three = per_char_on(l);
        let anims = l.props.group_mut("text/animators").ok_or_else(|| bad(C, "not a text layer"))?;
        let name = str_p(p, "name").map(str::to_string).unwrap_or_else(|| format!("Animator {}", anims.children.len() + 1));
        let mut g = build::text_animator(&mut ids, &name, vec![]);
        let added: usize = kinds.iter().map(|k| add_props(&mut ids, &mut g, k, three)).sum();
        // `properties: []` adds an empty animator (scripting's addProperty("ADBE Text Animator")).
        if added == 0 && !kinds.is_empty() {
            return Err(bad(C, "unknown animator property"));
        }
        // Like After Effects, a scripted animator has no selectors until the script adds one
        // (addProperty("ADBE Text Selector")); a default 0-100% one would select every character.
        if kinds.is_empty()
            && let Some(sels) = g.sub_mut("selectors")
        {
            sels.children.clear();
        }
        let uid = g.uid;
        anims.children.push(g.into());
        proj.next_id = next;
        Ok(uid)
    })?;
    Ok(json!({"animator": uid}))
}

fn add_animator_prop(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "layer.addTextAnimatorProperty";
    let (cid, lid) = layer_p(s, p, C)?;
    let kinds = kinds_p(p);
    if let Some(k) = kinds.iter().find(|k| !build::TEXT_ANIMATOR_KINDS.iter().any(|(id, _)| id == k)) {
        return Err(bad(C, format!("unknown animator property {k}")));
    }
    let layer = s.project.comp(cid).and_then(|c| c.layer(lid)).cloned().ok_or_else(|| bad(C, "no layer"))?;
    let auid = animator_uid(s, &layer, p, C)?;
    let mut index = 0;
    s.edit("Add Text Animator Property", None, |proj, _| {
        let mut next = proj.next_id;
        let mut ids = Ids(&mut next);
        let l = layer_mut(proj, cid, lid)?;
        let three = per_char_on(l);
        let anim = l.props.find_group_mut(auid).ok_or_else(|| bad(C, "no such animator"))?;
        for k in &kinds {
            add_props(&mut ids, anim, k, three);
        }
        // The requested property's 1-based position in the Properties group. A companion (such as
        // Tracking Type) is added before it, so the first new child is not always the one asked for.
        index = kinds.first().and_then(|k| anim.sub("properties")?.children.iter().position(|n| n.match_id() == k.as_str())).map_or(0, |i| i + 1);
        proj.next_id = next;
        Ok(())
    })?;
    Ok(json!({"animator": auid, "index": index}))
}

pub(crate) fn add_selector(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "layer.addTextSelector";
    let (cid, lid) = layer_p(s, p, C)?;
    let kind = str_p(p, "kind").unwrap_or("range").to_ascii_lowercase();
    let layer = s.project.comp(cid).and_then(|c| c.layer(lid)).cloned().ok_or_else(|| bad(C, "no layer"))?;
    let auid = animator_uid(s, &layer, p, C)?;
    let uid = s.edit("Add Text Selector", None, |proj, _| {
        let mut next = proj.next_id;
        let mut ids = Ids(&mut next);
        let l = layer_mut(proj, cid, lid)?;
        let anim = l.props.find_group_mut(auid).ok_or_else(|| bad(C, "no such animator"))?;
        if anim.sub("selectors").is_none() {
            let g = ids.group("selectors", "Selectors");
            anim.children.insert(0, g.into());
        }
        let sels = anim.sub_mut("selectors").ok_or_else(|| bad(C, "the animator has no Selectors group"))?;
        let (m, label) = match kind.as_str() {
            "range" => ("rangeSelector", "Range Selector"),
            "wiggly" => ("wigglySelector", "Wiggly Selector"),
            "expression" => ("expressionSelector", "Expression Selector"),
            _ => return Err(bad(C, "kind must be range, wiggly or expression")),
        };
        let n = sels.groups().filter(|g| g.match_id == m).count() + 1;
        let g = build::text_selector(&mut ids, &kind, &format!("{label} {n}")).ok_or_else(|| bad(C, "unknown selector kind"))?;
        let uid = g.uid;
        sels.children.push(g.into());
        proj.next_id = next;
        Ok(uid)
    })?;
    Ok(json!({"selector": uid, "animator": auid}))
}

/// Show / hide Z components and X / Y rotation on every animator.
fn set_animators_3d(ids: &mut Ids, l: &mut Layer, on: bool) {
    let Some(anims) = l.props.group_mut("text/animators") else { return };
    for c in anims.children.iter_mut() {
        let Node::Group(a) = c else { continue };
        let Some(props) = a.sub_mut("properties") else { continue };
        for c in props.children.iter_mut() {
            if let Node::Prop(p) = c
                && matches!(p.match_id.as_str(), "anchor" | "position" | "scale")
            {
                p.shown_dims = if on { 3 } else { 2 };
                if let KV::Vec2(v) = p.value {
                    p.value = KV::Vec3([v[0], v[1], if p.match_id == "scale" { 100.0 } else { 0.0 }]);
                }
            }
        }
        let has_rot = props.get("rotation").is_some();
        if on && has_rot && props.get("rotationX").is_none() {
            let at = props.children.iter().position(|c| c.match_id() == "rotation").unwrap_or(0);
            let rx = ids.prop("rotationX", "X Rotation", KV::Scalar(0.0)).with_ui(effectcraft_project::ParamUi::Angle);
            let ry = ids.prop("rotationY", "Y Rotation", KV::Scalar(0.0)).with_ui(effectcraft_project::ParamUi::Angle);
            props.children.insert(at, ry.into());
            props.children.insert(at, rx.into());
        }
        if !on {
            props.children.retain(|c| !matches!(c.match_id(), "rotationX" | "rotationY"));
        }
        if let Some(r) = props.get_mut("rotation") {
            r.name = if on { "Z Rotation".into() } else { "Rotation".into() };
        }
    }
}

fn enable_per_char(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "layer.enablePerChar3D";
    let (cid, lid) = layer_p(s, p, C)?;
    let cur = s.project.comp(cid).and_then(|c| c.layer(lid)).map(per_char_on).unwrap_or(false);
    let on = b_p(p, "enabled").unwrap_or(if p.get("toggle").and_then(Value::as_bool) == Some(true) { !cur } else { true });
    s.edit(if on { "Enable Per-character 3D" } else { "Disable Per-character 3D" }, None, |proj, _| {
        let mut next = proj.next_id;
        let mut ids = Ids(&mut next);
        let l = layer_mut(proj, cid, lid)?;
        if !matches!(l.source, LayerSource::Text) {
            return Err(bad(C, "not a text layer"));
        }
        if l.props.prop("text/perChar3d").is_none() {
            let mut pc = ids.prop("perChar3d", "Per-character 3D", KV::Bool(false)).with_ui(effectcraft_project::ParamUi::Hidden);
            pc.static_only = true;
            if let Some(t) = l.props.sub_mut("text") {
                t.children.insert(1, pc.into());
            }
        }
        if let Some(pr) = l.props.prop_mut("text/perChar3d") {
            pr.value = KV::Bool(on);
        }
        // Per-character 3D makes the layer 3D (as in AE).
        if on {
            l.switches.three_d = true;
        }
        set_animators_3d(&mut ids, l, on);
        proj.next_id = next;
        Ok(())
    })?;
    Ok(json!({"perChar3d": on}))
}

/// JSON → property value shaped like `like`.
pub(super) fn json_value(v: &Value, like: &KV) -> Option<KV> {
    Some(match v {
        Value::Bool(b) => match like {
            KV::Bool(_) => KV::Bool(*b),
            _ => KV::Scalar(if *b { 1.0 } else { 0.0 }),
        },
        Value::Number(n) => {
            let x = n.as_f64()?;
            match like {
                KV::Enum(_) => KV::Enum(x.max(0.0) as u32),
                KV::Bool(_) => KV::Bool(x != 0.0),
                _ => KV::Scalar(x),
            }
        }
        Value::Array(a) => {
            let c: Vec<f64> = a.iter().filter_map(Value::as_f64).collect();
            like.with_components(&c)
        }
        _ => return None,
    })
}

fn set_path(g: &mut PropGroup, path: &str, v: &Value, keys: Option<&Value>, at: Tick, ease: bool) {
    let Some(pr) = g.prop_mut(path) else { return };
    if let Some(val) = json_value(v, &pr.value) {
        pr.value = val;
    }
    let Some(Value::Array(list)) = keys else { return };
    let mut ks = Vec::new();
    for k in list {
        let (Some(t), Some(val)) = (k.get(0).and_then(Value::as_f64), k.get(1).and_then(|x| json_value(x, &pr.value))) else { continue };
        let mut key = Keyframe::new(at + Tick::from_seconds_f64(t), val);
        if pr.hold_only {
            key = key.hold();
        } else if ease {
            key = key.eased();
        }
        ks.push(key);
    }
    if !ks.is_empty() {
        pr.value = ks[0].value.clone();
        pr.keys = ks;
    }
}

fn apply_preset(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "layer.applyTextPreset";
    let (cid, lid) = layer_p(s, p, C)?;
    let want = str_p(p, "preset").ok_or_else(|| bad(C, "`preset` id or name required"))?.to_string();
    let preset = presets()
        .iter()
        .find(|x| x.get("id").and_then(Value::as_str) == Some(&want) || x.get("name").and_then(Value::as_str).is_some_and(|n| n.eq_ignore_ascii_case(&want)))
        .ok_or_else(|| bad(C, format!("no preset {want}")))?;
    let name = preset.get("name").and_then(Value::as_str).unwrap_or("Preset").to_string();
    let now = s.time();
    let uids = s.edit(&format!("Apply Text Preset {name}"), None, |proj, _| {
        let mut next = proj.next_id;
        let mut ids = Ids(&mut next);
        let l = layer_mut(proj, cid, lid)?;
        let at = l.layer_time(now);
        let three = per_char_on(l);
        let anims_json = preset.get("animators").and_then(Value::as_array).cloned().unwrap_or_default();
        let anims = l.props.group_mut("text/animators").ok_or_else(|| bad(C, "not a text layer"))?;
        let mut uids = Vec::new();
        for a in &anims_json {
            let aname = a.get("name").and_then(Value::as_str).unwrap_or("Animator");
            let mut g = build::text_animator(&mut ids, aname, vec![]);
            let ease = a.get("ease").and_then(Value::as_bool).unwrap_or(false);
            if let Some(Value::Object(props)) = a.get("properties") {
                for (m, v) in props {
                    add_props(&mut ids, &mut g, m, three);
                    let keys = a.get("keys").and_then(|k| k.get(m));
                    if let Some(pg) = g.sub_mut("properties") {
                        set_path(pg, m, v, keys, at, ease);
                    }
                }
            }
            for (i, sj) in a.get("selectors").and_then(Value::as_array).cloned().unwrap_or_default().iter().enumerate() {
                let kind = sj.get("kind").and_then(Value::as_str).unwrap_or("range");
                let Some(sels) = g.sub_mut("selectors") else { continue };
                if !(i == 0 && kind == "range") {
                    let label = match kind {
                        "wiggly" => "Wiggly Selector 1",
                        "expression" => "Expression Selector 1",
                        _ => "Range Selector 2",
                    };
                    if let Some(sg) = build::text_selector(&mut ids, kind, label) {
                        sels.children.push(sg.into());
                    }
                }
                let Some(Node::Group(sel)) = sels.children.last_mut() else { continue };
                let ease = sj.get("ease").and_then(Value::as_bool).unwrap_or(false);
                if let Some(Value::Object(set)) = sj.get("set") {
                    for (path, v) in set {
                        set_path(sel, path, v, None, at, ease);
                    }
                }
                if let Some(Value::Object(keys)) = sj.get("keys") {
                    for (path, list) in keys {
                        let cur = sel.prop(path).map(|p| p.value.clone());
                        if let Some(cur) = cur {
                            let v = serde_json::to_value(cur.components().first().copied().unwrap_or(0.0)).unwrap_or_default();
                            set_path(sel, path, &v, Some(list), at, ease);
                        }
                    }
                }
            }
            uids.push(g.uid);
            anims.children.push(g.into());
        }
        proj.next_id = next;
        Ok(uids)
    })?;
    Ok(json!({"animators": uids}))
}

fn list_presets(_: &mut Session, _: &Value) -> Result<Value> {
    Ok(Value::Array(presets().iter().map(|p| json!({"id": p.get("id"), "name": p.get("name"), "description": p.get("description")})).collect()))
}

/// Animation ▸ Animate Text ▸ Variable Font Axes: the axes of the layer's font, or (with `axis`)
/// a new animator — or a property on `animator` — that offsets that axis (user units, weighted
/// by the selectors, added to the axis default).
fn font_axes(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "text.animatorFontAxes";
    let (cid, lid) = layer_p(s, p, C)?;
    let layer = s.project.comp(cid).and_then(|c| c.layer(lid)).cloned().ok_or_else(|| bad(C, "no layer"))?;
    let Some(KV::Text(doc)) = layer.props.prop("text/sourceText").map(|pr| pr.value_at(layer.layer_time(s.time()))) else {
        return Err(bad(C, "not a text layer"));
    };
    let face = effectcraft_text::layout_doc(&doc).glyphs.first().map(|g| g.face).unwrap_or_else(|| effectcraft_text::resolve(&doc.font, &doc.style).face);
    let axes = effectcraft_text::variable::font_axes(face);
    let list = || json!(axes.iter().map(|a| json!({"tag": a.tag, "name": a.name, "min": a.min, "default": a.default, "max": a.max})).collect::<Vec<_>>());
    let Some(want) = str_p(p, "axis") else { return Ok(json!({"font": doc.font, "axes": list()})) };
    if axes.is_empty() {
        return Err(bad(C, format!("{} {} has no variation axes (not a variable font)", doc.font, doc.style)));
    }
    let axis = axes
        .iter()
        .find(|a| a.tag == want || a.name.eq_ignore_ascii_case(want))
        .ok_or_else(|| bad(C, format!("no axis `{want}`; the font has {}", list())))?
        .clone();
    let target = match p.get("animator") {
        Some(_) => Some(animator_uid(s, &layer, p, C)?),
        None => None,
    };
    let uid = s.edit("Add Variable Font Axis", None, |proj, _| {
        let mut next = proj.next_id;
        let mut ids = Ids(&mut next);
        let prop = ids
            .prop(&format!("{}{}", effectcraft_render::text::AXIS_PREFIX, axis.tag.trim_end()), &axis.name, KV::Scalar(0.0))
            .with_ui(effectcraft_project::ParamUi::Number);
        let l = layer_mut(proj, cid, lid)?;
        let uid = match target {
            Some(a) => {
                let g = l.props.find_group_mut(a).and_then(|g| g.sub_mut("properties")).ok_or_else(|| bad(C, "no such animator"))?;
                g.children.push(prop.into());
                a
            }
            None => {
                let anims = l.props.group_mut("text/animators").ok_or_else(|| bad(C, "not a text layer"))?;
                let name = format!("Animator {}", anims.children.len() + 1);
                let g = build::text_animator(&mut ids, &name, vec![prop]);
                let uid = g.uid;
                anims.children.push(g.into());
                uid
            }
        };
        proj.next_id = next;
        Ok(uid)
    })?;
    Ok(json!({"animator": uid, "axis": axis.tag}))
}

fn list_kinds(_: &mut Session, _: &Value) -> Result<Value> {
    Ok(Value::Array(build::TEXT_ANIMATOR_KINDS.iter().filter(|(k, _)| *k != "-").map(|(k, l)| json!({"property": k, "label": l})).collect()))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "layer.addTextAnimator",
            "Animate Text",
            [],
            None,
            "{layer?, properties: [anchor|position|scale|skew|rotation|opacity|transformAll|lineAnchor|lineSpacing|characterOffset|characterValue|blur|fillColor|fillHue|fillSaturation|fillBrightness|fillOpacity|strokeColor|strokeHue|strokeSaturation|strokeBrightness|strokeOpacity|strokeWidth|tracking|frontColor|bevelColor|sideColor|backColor|perChar3d], name?}",
            has_text_layer,
            add_animator
        ),
        cmd!(
            "layer.addTextAnimatorProperty",
            "Add Property",
            [],
            None,
            "{layer?, animator?: uid|index, property: (see layer.addTextAnimator)}",
            has_animator,
            add_animator_prop
        ),
        cmd!(
            "layer.addTextSelector",
            "Add Text Selector",
            [],
            None,
            "{layer?, animator?: uid|index, kind: range|wiggly|expression}",
            has_animator,
            add_selector
        ),
        cmd!("layer.enablePerChar3D", "Enable Per-character 3D", [], None, "{layer?, enabled?: bool, toggle?: bool}", has_text_layer, enable_per_char),
        cmd!(
            "layer.applyTextPreset",
            "Apply Text Animation Preset",
            [],
            None,
            "{layer?, preset: typewriter|fadeUpCharacters|bounceInWords|trackingIn|scramble|blurIn|jitter|dropInLines}",
            has_text_layer,
            apply_preset
        ),
        cmd!(
            "text.animatorFontAxes",
            "Variable Font Axes",
            ["Animation", "Animate Text"],
            None,
            "{layer?, axis?: tag|name (omit to list the font's axes), animator?: uid|index}",
            has_text_layer,
            font_axes
        ),
        query!("text.presets", "Text Animation Presets", "{}", list_presets),
        query!("text.animatorProperties", "Text Animator Properties", "{}", list_kinds),
    ]
}
