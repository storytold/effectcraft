//! The selection-aware Closed menu action. Explicit edits retain `mask.setClosed`'s API.

use effectcraft_keyframe::Value as KV;
use effectcraft_project::{GroupKind, Layer, LayerId, Node, PropGroup, Uid};
use effectcraft_time::Tick;
use serde_json::{Value, json};

use super::super::{bad, layer_mut};
use crate::{EngineError, Result, Session, VertexRef};

const MAX_DEPTH: usize = 128;

struct Target {
    layer: LayerId,
    route: Vec<usize>,
    closed: bool,
    editable: bool,
}

struct Selection<'a> {
    vertices: &'a [VertexRef],
    props: &'a [(LayerId, Uid)],
    time: Tick,
}

/// A route uses checked child access and never recurses through an input-controlled tree.
fn group_at_mut<'a>(mut group: &'a mut PropGroup, route: &[usize]) -> Option<&'a mut PropGroup> {
    for &index in route {
        group = group.children.get_mut(index)?.as_group_mut()?;
    }
    Some(group)
}

/// The selected paths on the selected layers, else every path of those layers (as the other
/// Mask and Shape Path commands do). Locked layers and masks count as matches, so selecting one
/// never falls back to its siblings, and are then left out.
fn targets(s: &Session) -> Result<Vec<Target>> {
    let comp = s.active_comp().ok_or(EngineError::NoComp)?;
    let selection = Selection { vertices: &s.state.selected_vertices, props: &s.state.selected_props, time: s.time() };
    let mut targets = Vec::new();
    for whole_layers in [false, true] {
        // Only selected layers: navigation and insertion can retain point/property selections
        // on other layers, and every match below is per layer.
        for layer in comp.layers.iter().filter(|l| s.state.selected_layers.contains(&l.id)) {
            collect(&selection, layer, &layer.props, &mut Vec::new(), whole_layers, layer.switches.locked, &mut targets)?;
        }
        if !targets.is_empty() {
            break;
        }
    }
    targets.retain(|target| target.editable);
    if targets.is_empty() {
        return Err(bad("path.toggleClosed", "select an unlocked mask or Bezier shape path"));
    }
    Ok(targets)
}

fn collect(
    selection: &Selection,
    layer: &Layer,
    group: &PropGroup,
    route: &mut Vec<usize>,
    inherited: bool,
    locked: bool,
    targets: &mut Vec<Target>,
) -> Result<()> {
    if route.len() > MAX_DEPTH {
        return Err(bad("path.toggleClosed", "path group nesting exceeds 128 levels"));
    }
    let locked = locked || matches!(group.kind, GroupKind::Mask { locked: true, .. });
    let selected = inherited || selection.props.contains(&(layer.id, group.uid));
    if super::super::mask::is_path_group(group)
        && let Some(prop) = group.get("path")
        && let KV::Path(path) = prop.value_at(layer.layer_time(selection.time))
    {
        // Selecting any of a path group's properties (Mask Path, Feather, Opacity…) or its points
        // selects the path.
        let selected = selected
            || group.children.iter().any(|c| matches!(c, Node::Prop(p) if selection.props.contains(&(layer.id, p.uid))))
            || selection.vertices.iter().any(|v| v.layer == layer.id && v.mask == group.uid && v.index < path.vertices.len());
        if selected {
            targets.push(Target { layer: layer.id, route: route.clone(), closed: path.closed, editable: !locked });
        }
        return Ok(());
    }
    for (index, child) in group.children.iter().enumerate() {
        let Some(child) = child.as_group() else { continue };
        route.push(index);
        collect(selection, layer, child, route, selected, locked, targets)?;
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
            // As `mask.setClosed`: a path read from a file may have fewer tangents than vertices (#1250).
            path.fit_tangents();
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
