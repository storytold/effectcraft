//! Keyframe color labels: Label a keyframe (Keyframe ▸ Label ▸ colour) and Edit ▸ Select Keyframe
//! Label Group (On Selected Layers / On All Layers / Visible Keyframes on …).

use effectcraft_color::Label;
use serde_json::{Value, json};

use super::{CommandSpec, bad, has_comp, has_keys, str_p};
use crate::{EngineError, KeyRef, Result, Session, cmd};

fn label_p(p: &Value, c: &str) -> Result<u8> {
    match p.get("label") {
        Some(Value::String(s)) => {
            let l = if s.eq_ignore_ascii_case("none") { Some(Label::None) } else { Label::from_name(s) };
            let l = l.ok_or_else(|| bad(c, format!("unknown label `{s}`")))?;
            Ok(Label::ALL.iter().position(|x| *x == l).unwrap_or(0) as u8)
        }
        Some(Value::Number(n)) => Ok(n.as_u64().filter(|i| (*i as usize) < Label::ALL.len()).ok_or_else(|| bad(c, "label index 0–16"))? as u8),
        _ => Err(bad(c, "missing `label` (name or 0–16)")),
    }
}

fn set_label(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "keys.setLabel";
    let label = label_p(p, c)?;
    let n = super::prop::edit_keys(s, "Keyframe Label", None, |keys, i, _| {
        keys[i].label = label;
        true
    })?;
    Ok(json!({"label": Label::ALL[label as usize].name(), "keys": n}))
}

/// Select every keyframe that shares a label with a selected keyframe, on the selected or all
/// layers. `visible` (property uids revealed in the Timeline) limits the "Visible Keyframes"
/// scopes; without it every property counts as visible.
fn select_label_group(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "keys.selectLabelGroup";
    let scope = str_p(p, "scope").unwrap_or("selected");
    let comp = s.active_comp().ok_or(EngineError::NoComp)?;
    let labels: Vec<u8> =
        s.state.selected_keys.iter().filter_map(|k| comp.layer(k.layer)?.props.find(k.prop)?.keys.iter().find(|x| x.time == k.time).map(|x| x.label)).collect();
    if labels.is_empty() {
        return Err(bad(c, "select a keyframe first"));
    }
    let only_selected = match scope {
        "selected" | "visibleSelected" => true,
        "all" | "visibleAll" => false,
        _ => return Err(bad(c, "scope: selected|all|visibleSelected|visibleAll")),
    };
    let visible: Option<Vec<u64>> =
        (scope.starts_with("visible")).then(|| p.get("visible").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).collect())).flatten();
    let key_layers: Vec<_> = s.state.selected_keys.iter().map(|k| k.layer).collect();
    let mut sel = Vec::new();
    for l in &comp.layers {
        if only_selected && !s.state.selected_layers.contains(&l.id) && !key_layers.contains(&l.id) {
            continue;
        }
        l.props.walk("", &mut |_, pr| {
            if visible.as_ref().is_some_and(|v| !v.contains(&pr.uid)) {
                return;
            }
            for k in &pr.keys {
                if labels.contains(&k.label) {
                    sel.push(KeyRef { layer: l.id, prop: pr.uid, time: k.time });
                }
            }
        });
    }
    s.state.selected_keys = sel;
    Ok(json!({"keys": s.state.selected_keys.len()}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!("keys.setLabel", "Keyframe Label", [], None, "{label: name|none|0–16}", has_keys, set_label),
        cmd!(
            "keys.selectLabelGroup",
            "Select Keyframe Label Group",
            [],
            None,
            "{scope: selected|all|visibleSelected|visibleAll, visible?: [prop uid]}",
            has_comp,
            select_label_group
        ),
    ]
}
