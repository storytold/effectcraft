//! The selection-aware Closed menu action. Explicit edits retain `mask.setClosed`'s API.

use effectcraft_keyframe::Value as KV;
use effectcraft_project::{GroupKind, Layer, LayerId, PropGroup, Uid};
use effectcraft_time::Tick;
use serde_json::{Value, json};

use super::super::{bad, layer_mut};
use crate::{EngineError, Result, Session, VertexRef};

const MAX_DEPTH: usize = 128;

struct Target {
    layer: LayerId,
    route: Vec<usize>,
    closed: bool,
}

struct Selection {
    vertices: Vec<VertexRef>,
    props: Vec<(LayerId, Uid)>,
    time: Tick,
}

/// A route uses checked child access and never recurses through an input-controlled tree.
fn group_at_mut<'a>(mut group: &'a mut PropGroup, route: &[usize]) -> Option<&'a mut PropGroup> {
    for &index in route {
        group = group.children.get_mut(index)?.as_group_mut()?;
    }
    Some(group)
}

fn targets(s: &Session) -> Result<Vec<Target>> {
    let comp = s.active_comp().ok_or(EngineError::NoComp)?;
    // Layer navigation and insertion can retain point/property selections on other layers.
    let selection = Selection {
        vertices: s.state.selected_vertices.iter().filter(|v| s.state.selected_layers.contains(&v.layer)).copied().collect(),
        props: s.state.selected_props.iter().filter(|(lid, _)| s.state.selected_layers.contains(lid)).copied().collect(),
        time: s.time(),
    };
    let mut targets = Vec::new();
    for layer in comp.layers.iter() {
        let selected_layer = if !selection.vertices.is_empty() {
            selection.vertices.iter().any(|v| v.layer == layer.id)
        } else if !selection.props.is_empty() {
            selection.props.iter().any(|(lid, _)| *lid == layer.id)
        } else {
            s.state.selected_layers.contains(&layer.id)
        };
        if !selected_layer || layer.switches.locked {
            continue;
        }
        collect(&selection, layer, &layer.props, &mut Vec::new(), selection.props.is_empty(), &mut targets)?;
    }
    if targets.is_empty() {
        return Err(bad("path.toggleClosed", "select an unlocked mask or Bezier shape path"));
    }
    Ok(targets)
}

fn collect(selection: &Selection, layer: &Layer, group: &PropGroup, route: &mut Vec<usize>, inherited: bool, targets: &mut Vec<Target>) -> Result<()> {
    if route.len() > MAX_DEPTH {
        return Err(bad("path.toggleClosed", "path group nesting exceeds 128 levels"));
    }
    if matches!(group.kind, GroupKind::Mask { locked: true, .. }) {
        return Ok(());
    }
    let selected = inherited || selection.props.contains(&(layer.id, group.uid));
    if super::super::mask::is_path_group(group)
        && let Some(prop) = group.get("path")
        && let KV::Path(path) = prop.value_at(layer.layer_time(selection.time))
    {
        let selected = if selection.vertices.is_empty() {
            selected || selection.props.contains(&(layer.id, prop.uid))
        } else {
            selection.vertices.iter().any(|v| v.layer == layer.id && v.mask == group.uid && v.index < path.vertices.len())
        };
        if selected {
            targets.push(Target { layer: layer.id, route: route.clone(), closed: path.closed });
        }
        return Ok(());
    }
    for (index, child) in group.children.iter().enumerate() {
        let Some(child) = child.as_group() else { continue };
        route.push(index);
        collect(selection, layer, child, route, selected, targets)?;
        route.pop();
    }
    Ok(())
}

pub(crate) fn selected_paths_closed(s: &Session) -> Option<bool> {
    targets(s).ok().map(|targets| targets.iter().all(|target| target.closed))
}

pub(super) fn has_paths(s: &Session) -> std::result::Result<(), String> {
    targets(s).map(|_| ()).map_err(|error| error.to_string())
}

pub(super) fn toggle_closed(s: &mut Session, _: &Value) -> Result<Value> {
    let targets = targets(s)?;
    let closed = !targets.iter().all(|target| target.closed);
    let cid = s.active_comp_id().ok_or(EngineError::NoComp)?;
    let time = s.time();
    s.edit("Closed", None, |project, _| {
        for target in &targets {
            let layer = layer_mut(project, cid, target.layer)?;
            let time = layer.layer_time(time);
            let group = group_at_mut(&mut layer.props, &target.route).ok_or_else(|| bad("path.toggleClosed", "the selected path no longer exists"))?;
            let roto = super::is_roto(group);
            let prop = group.get_mut("path").ok_or_else(|| bad("path.toggleClosed", "the selected group has no path"))?;
            let KV::Path(mut path) = prop.value_at(time) else { return Err(bad("path.toggleClosed", "the selected value is not a path")) };
            if path.closed == closed {
                continue;
            }
            path.closed = closed;
            if roto {
                super::roto_smooth(&mut path);
            }
            prop.set_value_at(time, KV::Path(path));
        }
        Ok(())
    })?;
    Ok(json!({"paths": targets.len(), "closed": closed}))
}
