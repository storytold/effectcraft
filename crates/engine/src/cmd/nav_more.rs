//! Keyboard-driven commands that have no menu entry.

use std::collections::BTreeSet;

use effectcraft_keyframe::Value as KV;
use effectcraft_project::{ItemId, LayerSource, Project};
use effectcraft_time::Tick;
use serde_json::{Value, json};

use super::{CommandSpec, always, bad, comp_id, has_comp, has_layers, layers_p};
use crate::{EngineError, Result, Session, cmd};

/// Ctrl+Alt+B: the work area spans the selected layers (their earliest In to their latest Out
/// point), or the whole composition with none selected.
fn work_area_to_selection(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, ids) = layers_p(s, p)?;
    s.edit("Work Area", None, |proj, _| {
        let c = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        let fd = c.frame_duration();
        let spans: Vec<(Tick, Tick)> = c.layers.iter().filter(|l| ids.contains(&l.id)).map(|l| (l.in_point, l.out_point)).collect();
        let (start, end) = match (spans.iter().map(|s| s.0).min(), spans.iter().map(|s| s.1).max()) {
            (Some(a), Some(b)) => (a, b),
            _ => (Tick::ZERO, c.duration),
        };
        // min/max, not clamp: the bounds cross on a comp shorter than a frame.
        let start = start.min(c.duration - fd).max(Tick::ZERO);
        c.work_area = (start, end.min(c.duration).max(start + fd));
        Ok(())
    })?;
    let c = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    Ok(json!({"start": c.work_area.0.seconds(), "end": c.work_area.1.seconds()}))
}

/// Shift+F2: no keyframe, property or property group is selected; the layers stay selected.
fn deselect_keys(s: &mut Session, _: &Value) -> Result<Value> {
    s.state.selected_keys.clear();
    s.state.selected_props.clear();
    s.state.selected_vertices.clear();
    Ok(Value::Null)
}

/// Alt+` / Alt+Shift+`: select the next (previous) mask of the layer, wrapping around; the first
/// (last) one when none is selected.
fn step_mask(s: &mut Session, p: &Value, dir: i64) -> Result<Value> {
    let cmd = if dir > 0 { "layer.mask.selectNext" } else { "layer.mask.selectPrevious" };
    let (cid, ids) = layers_p(s, p)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let (lid, masks) = ids
        .iter()
        .filter_map(|id| comp.layer(*id))
        .find_map(|l| l.masks().filter(|m| m.groups().next().is_some()).map(|m| (l.id, m)))
        .ok_or_else(|| bad(cmd, "the selected layers have no masks"))?;
    let groups: Vec<_> = masks.groups().collect();
    let n = groups.len() as i64;
    // The selected mask: its group, or a property inside it.
    let selected = |g: &effectcraft_project::PropGroup| {
        s.state.selected_props.iter().any(|(l, uid)| *l == lid && (g.find_group(*uid).is_some() || g.find(*uid).is_some()))
    };
    let i = match groups.iter().position(|g| selected(g)) {
        Some(i) => (i as i64 + dir).rem_euclid(n.max(1)),
        None if dir > 0 => 0,
        None => n - 1,
    };
    let uid = groups.get(i as usize).map(|g| g.uid).ok_or_else(|| bad(cmd, "the selected layers have no masks"))?;
    s.state.selected_props = vec![(lid, uid)];
    s.state.selected_keys.clear();
    s.state.selected_vertices.clear();
    Ok(json!({"layer": lid.0, "mask": uid, "index": i + 1}))
}

/// Alt+Shift+C: Casts Shadows on or off for the selected 3D layers (all follow the first one).
fn casts_shadows(s: &mut Session, p: &Value) -> Result<Value> {
    const PATH: &str = "materialOptions/castsShadows";
    let (cid, ids) = layers_p(s, p)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let with: Vec<_> = ids.iter().filter_map(|id| comp.layer(*id)).filter(|l| l.switches.three_d && l.props.prop(PATH).is_some()).collect();
    let first = with.first().ok_or_else(|| bad("layer.castsShadows", "select a 3D layer"))?;
    let on = match p.get("value").and_then(Value::as_bool) {
        Some(v) => v,
        None => first.props.prop(PATH).is_none_or(|pr| pr.value == KV::Enum(0)),
    };
    let layers: Vec<u64> = with.iter().map(|l| l.id.0).collect();
    let set = super::find("material.set").ok_or_else(|| bad("layer.castsShadows", "Material Options are not available"))?;
    (set.run)(s, &json!({"comp": cid.0, "layers": layers, "castsShadows": if on { "on" } else { "off" }}))?;
    Ok(json!({"castsShadows": on, "layers": layers}))
}

/// The comps connected to `from` by nesting, in either direction (its network of nested
/// compositions), `from` included.
fn hierarchy(proj: &Project, from: ItemId) -> BTreeSet<ItemId> {
    let mut edges: Vec<(ItemId, ItemId)> = Vec::new();
    for item in proj.items.values() {
        let Some(c) = item.as_comp() else { continue };
        for l in &c.layers {
            if let LayerSource::Comp { item: nested } = &l.source {
                edges.push((item.id, *nested));
            }
        }
    }
    let mut seen = BTreeSet::from([from]);
    let mut todo = vec![from];
    while let Some(c) = todo.pop() {
        for (a, b) in &edges {
            let other = if *a == c {
                *b
            } else if *b == c {
                *a
            } else {
                continue;
            };
            if seen.insert(other) {
                todo.push(other);
            }
        }
    }
    seen
}

/// Shift+Esc: activate the composition that was active most recently among those in the active
/// composition's nesting hierarchy.
fn activate_recent(s: &mut Session, p: &Value) -> Result<Value> {
    let cid = comp_id(s, p)?;
    let related = hierarchy(&s.project, cid);
    let next = s.state.recent_comps.iter().copied().find(|c| *c != cid && related.contains(c) && s.project.comp(*c).is_some());
    let Some(next) = next else { return Ok(Value::Null) };
    s.open_comp(next);
    Ok(json!({"comp": next.0}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "comp.workAreaToSelection",
            "Set Work Area to Selected Layers",
            [],
            Some("Cmd+Alt+B"),
            "{comp?, layers? (default the selected layers; the whole composition with none)}",
            has_comp,
            work_area_to_selection
        ),
        cmd!("comp.activateRecent", "Activate Most Recent Composition in Hierarchy", [], Some("Shift+Escape"), "{comp?}", has_comp, activate_recent),
        cmd!("keys.deselectAll", "Deselect All Keyframes and Properties", [], ["Shift+F2", "Cmd+Alt+Shift+A"], "{}", always, deselect_keys),
        cmd!("layer.mask.selectNext", "Select Next Mask", [], Some("Alt+`"), "{layer?}", has_layers, |s, p| step_mask(s, p, 1)),
        cmd!("layer.mask.selectPrevious", "Select Previous Mask", [], Some("Alt+Shift+`"), "{layer?}", has_layers, |s, p| step_mask(s, p, -1)),
        cmd!("layer.castsShadows", "Casts Shadows", [], Some("Alt+Shift+C"), "{layers?, value?}", has_layers, casts_shadows),
    ]
}
