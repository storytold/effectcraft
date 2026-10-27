//! Layer ▸ Layer Styles: add / remove / show styles and the comp's Global Light.
//!
//! Styles are property groups under the layer's `layerStyles` group (see
//! [`effectcraft_project::styles`]); their values are edited with `prop.set` like any other
//! property (`layerStyles/dropShadow/distance`). Global Light is shared by every layer of a comp:
//! editing one layer's `layerStyles/blendingOptions/globalLightAngle` updates them all.

use effectcraft_project::build::Ids;
use effectcraft_project::styles::{self as st, GROUP, STYLES};
use effectcraft_project::{ItemId, LayerId, Uid};
use serde_json::{Value, json};

use super::{CommandSpec, b_p, bad, f_p, has_comp, has_layers, layer_mut, layer_p, layers_p, str_p, time_p};
use crate::{EngineError, Result, Session, cmd};

/// Selected (or given) layers that take layer styles.
fn style_layers(s: &Session, p: &Value) -> Result<(ItemId, Vec<LayerId>)> {
    let (cid, ids) = layers_p(s, p)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    Ok((cid, ids.into_iter().filter(|id| comp.layer(*id).is_some_and(|l| l.can_have_styles())).collect()))
}

fn add_styles(s: &mut Session, p: &Value, cmd: &str, styles: &[&str], enabled: bool, label: &str) -> Result<Value> {
    let (cid, ids) = style_layers(s, p)?;
    if ids.is_empty() {
        return Err(bad(cmd, "no layer that takes layer styles (footage, solid, precomp, text or shape layer)"));
    }
    let uids = s.edit(label, None, |proj, state| {
        let gl = proj.comp(cid).ok_or(EngineError::NoComp)?.global_light.clone();
        let mut out = vec![];
        let mut sel = vec![];
        for lid in &ids {
            let mut next = proj.next_id;
            let l = layer_mut(proj, cid, *lid)?;
            for id in styles {
                let had = l.layer_styles().and_then(|g| g.sub(id)).is_some();
                let uid = st::add_style(l, &mut Ids(&mut next), id, &gl, enabled).ok_or_else(|| bad(cmd, format!("unknown style `{id}`")))?;
                // Choosing a style that is already there (e.g. after Show All) switches it on.
                if had
                    && enabled
                    && let Some(g) = l.props.find_group_mut(uid)
                {
                    g.enabled = true;
                }
                if let Some(g) = l.props.sub_mut(GROUP) {
                    g.enabled = true;
                }
                out.push(json!({"layer": lid.0, "style": id, "uid": uid}));
                sel.push((*lid, uid));
            }
            proj.next_id = next;
        }
        state.selected_props = sel;
        Ok(out)
    })?;
    Ok(json!({"styles": uids}))
}

fn add(s: &mut Session, p: &Value) -> Result<Value> {
    let name = str_p(p, "style").ok_or_else(|| bad("layer.style.add", "missing `style` (e.g. dropShadow or \"Drop Shadow\")"))?;
    let id = st::style_id(name)
        .ok_or_else(|| bad("layer.style.add", format!("unknown style `{name}`; one of {}", STYLES.iter().map(|s| s.0).collect::<Vec<_>>().join(", "))))?;
    let label = STYLES.iter().find(|s| s.0 == id).map(|s| s.1).unwrap_or(id);
    add_styles(s, p, "layer.style.add", &[id], true, label)
}

macro_rules! style_cmd {
    ($fn:ident, $id:literal) => {
        fn $fn(s: &mut Session, p: &Value) -> Result<Value> {
            let label = STYLES.iter().find(|x| x.0 == $id).map(|x| x.1).unwrap_or($id);
            add_styles(s, p, concat!("layer.style.", $id), &[$id], true, label)
        }
    };
}
style_cmd!(drop_shadow, "dropShadow");
style_cmd!(inner_shadow, "innerShadow");
style_cmd!(outer_glow, "outerGlow");
style_cmd!(inner_glow, "innerGlow");
style_cmd!(bevel_emboss, "bevelEmboss");
style_cmd!(satin, "satin");
style_cmd!(color_overlay, "colorOverlay");
style_cmd!(gradient_overlay, "gradientOverlay");
style_cmd!(stroke, "stroke");

/// Show All: every style is added (new ones switched off) so they can be revealed and edited.
fn show_all(s: &mut Session, p: &Value) -> Result<Value> {
    let all: Vec<&str> = STYLES.iter().map(|s| s.0).collect();
    add_styles(s, p, "layer.style.showAll", &all, false, "Show All Layer Styles")
}

fn remove_all(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, ids) = style_layers(s, p)?;
    let n = s.edit("Remove All Layer Styles", None, |proj, state| {
        let mut n = 0;
        for lid in &ids {
            let l = layer_mut(proj, cid, *lid)?;
            if let Some(g) = l.layer_styles() {
                let gone: Vec<Uid> = std::iter::once(g.uid).chain(g.groups().map(|x| x.uid)).collect();
                l.props.children.retain(|c| c.match_id() != GROUP);
                state.selected_props.retain(|(_, u)| !gone.contains(u));
                n += 1;
            }
        }
        Ok(n)
    })?;
    Ok(json!({"layers": n}))
}

/// A style group of a layer by match id, display name, uid or the selection.
fn find_style(s: &Session, p: &Value, cmd: &str) -> Result<(ItemId, LayerId, Uid)> {
    let (cid, lid) = layer_p(s, p, cmd)?;
    let layer = s.project.comp(cid).and_then(|c| c.layer(lid)).ok_or(EngineError::NoComp)?;
    let g = layer.layer_styles().ok_or_else(|| bad(cmd, "the layer has no layer styles"))?;
    let found = match p.get("style") {
        Some(Value::Number(n)) => {
            let n = n.as_u64().unwrap_or(0);
            if n == g.uid { Some(g.uid) } else { g.groups().find(|x| x.uid == n).map(|x| x.uid) }
        }
        Some(Value::String(name)) if name == GROUP || name.eq_ignore_ascii_case("Layer Styles") => Some(g.uid),
        Some(Value::String(name)) => {
            let id = st::style_id(name).unwrap_or(name.as_str());
            g.groups().find(|x| x.match_id == id || x.name.eq_ignore_ascii_case(name)).map(|x| x.uid)
        }
        _ => s.state.selected_props.iter().find_map(|(l, u)| (*l == lid).then(|| g.groups().find(|x| x.uid == *u).map(|x| x.uid)).flatten()),
    }
    .ok_or_else(|| bad(cmd, "no such layer style"))?;
    Ok((cid, lid, found))
}

fn remove(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, lid, uid) = find_style(s, p, "layer.style.remove")?;
    s.edit("Remove Layer Style", None, |proj, state| {
        let l = layer_mut(proj, cid, lid)?;
        let whole = l.layer_styles().is_some_and(|g| g.uid == uid);
        if whole {
            l.props.children.retain(|c| c.match_id() != GROUP);
        } else if let Some(g) = l.props.sub_mut(GROUP) {
            if g.sub(st::BLENDING).is_some_and(|b| b.uid == uid) {
                return Err(bad("layer.style.remove", "Blending Options can't be removed"));
            }
            g.children.retain(|c| c.uid() != uid);
            // The last style gone removes the Layer Styles group, as in AE.
            if g.children.iter().all(|c| c.match_id() == st::BLENDING) {
                l.props.children.retain(|c| c.match_id() != GROUP);
            }
        }
        state.selected_props.retain(|(_, u)| *u != uid);
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Eye switch of a style (or of the whole Layer Styles group: `style: "layerStyles"`).
fn toggle(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, lid, uid) = find_style(s, p, "layer.style.toggle")?;
    let v = b_p(p, "value");
    let r = s.edit("Toggle Layer Style", None, |proj, _| {
        let g = layer_mut(proj, cid, lid)?.props.find_group_mut(uid).ok_or_else(|| bad("layer.style.toggle", "gone"))?;
        g.enabled = v.unwrap_or(!g.enabled);
        Ok(g.enabled)
    })?;
    Ok(json!(r))
}

fn global_light(s: &mut Session, p: &Value) -> Result<Value> {
    let cid = super::comp_id(s, p)?;
    let (angle, altitude) = (f_p(p, "angle"), f_p(p, "altitude"));
    if angle.is_none() && altitude.is_none() {
        let c = s.project.comp(cid).ok_or(EngineError::NoComp)?;
        return Ok(json!({"angle": c.global_light.angle.value.as_f64(), "altitude": c.global_light.altitude.value.as_f64()}));
    }
    let t = time_p(s, p, s.project.comp(cid));
    s.edit("Global Light", super::merge_p(p), |proj, _| {
        let c = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        st::set_global_light(c, angle, altitude, t);
        Ok(())
    })?;
    let c = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    Ok(json!({"angle": c.global_light.angle.value_at(t).as_f64(), "altitude": c.global_light.altitude.value_at(t).as_f64()}))
}

fn no_psd_styles(s: &Session) -> std::result::Result<(), String> {
    has_layers(s)?;
    Err("no Photoshop layer styles to convert (only layers imported from PSD files carry them)".into())
}

fn convert(_: &mut Session, _: &Value) -> Result<Value> {
    Err(EngineError::Other("no Photoshop layer styles to convert".into()))
}

fn list(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, lid) = layer_p(s, p, "layer.style.list")?;
    let l = s.project.comp(cid).and_then(|c| c.layer(lid)).ok_or(EngineError::NoComp)?;
    let styles: Vec<Value> = l
        .layer_styles()
        .map(|g| {
            g.groups()
                .filter(|x| x.match_id != st::BLENDING)
                .map(|x| json!({"style": x.match_id, "name": x.name, "uid": x.uid, "enabled": x.enabled}))
                .collect()
        })
        .unwrap_or_default();
    Ok(
        json!({"group": l.layer_styles().map(|g| g.uid), "enabled": l.layer_styles().map(|g| g.enabled), "styles": styles, "available": STYLES.iter().map(|s| json!({"style": s.0, "name": s.1})).collect::<Vec<_>>()}),
    )
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!("layer.style.convertToEditable", "Convert to Editable Styles", ["Layer", "Layer Styles"], None, "{layers?}", no_psd_styles, convert),
        cmd!("layer.style.showAll", "Show All", ["Layer", "Layer Styles"], None, "{layers?}", has_layers, show_all),
        cmd!("layer.style.removeAll", "Remove All", ["Layer", "Layer Styles"], None, "{layers?}", has_layers, remove_all),
        cmd!("layer.style.dropShadow", "Drop Shadow", ["Layer", "Layer Styles"], None, "{layers?}", has_layers, drop_shadow),
        cmd!("layer.style.innerShadow", "Inner Shadow", ["Layer", "Layer Styles"], None, "{layers?}", has_layers, inner_shadow),
        cmd!("layer.style.outerGlow", "Outer Glow", ["Layer", "Layer Styles"], None, "{layers?}", has_layers, outer_glow),
        cmd!("layer.style.innerGlow", "Inner Glow", ["Layer", "Layer Styles"], None, "{layers?}", has_layers, inner_glow),
        cmd!("layer.style.bevelEmboss", "Bevel and Emboss", ["Layer", "Layer Styles"], None, "{layers?}", has_layers, bevel_emboss),
        cmd!("layer.style.satin", "Satin", ["Layer", "Layer Styles"], None, "{layers?}", has_layers, satin),
        cmd!("layer.style.colorOverlay", "Color Overlay", ["Layer", "Layer Styles"], None, "{layers?}", has_layers, color_overlay),
        cmd!("layer.style.gradientOverlay", "Gradient Overlay", ["Layer", "Layer Styles"], None, "{layers?}", has_layers, gradient_overlay),
        cmd!("layer.style.stroke", "Stroke", ["Layer", "Layer Styles"], None, "{layers?}", has_layers, stroke),
        cmd!(
            "layer.style.add",
            "Add Layer Style",
            [],
            None,
            "{style: dropShadow|innerShadow|outerGlow|innerGlow|bevelEmboss|satin|colorOverlay|gradientOverlay|stroke (or display name), layers?}",
            has_layers,
            add
        ),
        cmd!("layer.style.remove", "Remove Layer Style", [], None, "{layer?, style: id|name|uid|layerStyles}", has_layers, remove),
        cmd!("layer.style.toggle", "Toggle Layer Style", [], None, "{layer?, style: id|name|uid|layerStyles, value?}", has_layers, toggle),
        cmd!(
            "layer.style.globalLight",
            "Global Light",
            [],
            None,
            "{comp?, angle? (deg), altitude? (deg, 0..90), time? (s)} — no values returns the current setting",
            has_comp,
            global_light
        ),
        crate::query!("layer.style.list", "List Layer Styles", "{layer?}", list),
    ]
}
