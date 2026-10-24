//! Layer and composition markers, and Time Remapping.
//!
//! Markers (aep-format.md 3.11; 122, 123, 249, 250, 347-349, 509, 510): the time is a key of the
//! `mrst` property in its own time scale (layer time for layer markers, as EffectCraft keeps them),
//! the duration counts 1/600 s, the strings are comment, chapter, URL, frame target and cue point
//! name, then the cue type (event 4, navigation 5) and the cue parameters; the label is a byte of
//! the marker header (347).
//!
//! Time Remapping (aep-format.md 3.4; 319, 354): an enabled remap is a keyed `ADBE Time Remapping`
//! property whose plain one-component keys are layer time -> source time in seconds; it becomes
//! EffectCraft's `timeRemap` property (seconds, keys in layer time) at the top of the layer's
//! properties, where the engine's Enable Time Remapping puts it.

use effectcraft_aep::{Chunk, CueType, Layer as AepLayer, Marker as AepMarker, PropNode, Property as AepProp};
use effectcraft_color::Label;
use effectcraft_keyframe::Value;
use effectcraft_project::build::Ids;
use effectcraft_project::{CuePoint, Marker, Node, ParamUi, PropGroup};
use effectcraft_time::Tick;

use crate::props::{Kind, import_expression, import_keys};
use crate::report::{Collector, LossCategory};
use crate::time::TimeMap;

/// The time scale of the marker list below `holder` (the layer, or the composition's `SecL`).
fn marker_scale(holder: &Chunk<'_>) -> Option<u32> {
    holder.find_list(b"mrst")?.child_list(b"tdbs").and_then(AepProp::from_chunk)?.time_scale().filter(|s| *s != 0)
}

/// The markers of a layer (`holder` = the layer chunk) or a composition (`holder` = its `SecL`).
pub(crate) fn import_markers(list: Vec<AepMarker>, holder: &Chunk<'_>, what: &str, tm: &TimeMap, col: &mut Collector) -> Vec<Marker> {
    let scale = marker_scale(holder).unwrap_or(tm.timescale);
    let mut out: Vec<Marker> = Vec::with_capacity(list.len());
    for m in list {
        let time = tm.units(i64::from(m.time_units), scale);
        let duration = match m.duration_seconds {
            Some(d) if d.is_finite() && (0.0..=1e7).contains(&d) => Tick::from_seconds_f64(d),
            Some(_) => {
                col.lose(LossCategory::DamagedData, format!("{what}: a marker duration is not valid, 0 used"));
                Tick::ZERO
            }
            None => Tick::ZERO,
        };
        let label = m.label.and_then(|l| Label::ALL.get(usize::from(l)).copied()).unwrap_or(Label::None);
        let navigation = match m.cue_type {
            Some(CueType::Navigation) => true,
            Some(CueType::Event) | None => false,
            Some(CueType::Other(code)) => {
                col.lose(LossCategory::Markers, format!("{what}: marker at {:.3} s has the cue type {code}, read as an event cue", time.seconds()));
                false
            }
        };
        let cue_point = (!m.cue_point_name.is_empty() || !m.cue_params.is_empty()).then(|| CuePoint {
            name: m.cue_point_name.clone(),
            navigation,
            params: m.cue_params.clone(),
        });
        out.push(Marker {
            time,
            duration,
            comment: m.comment,
            label,
            chapter: m.chapter,
            url: m.url,
            protected: false,
            frame_target: m.frame_target,
            cue_point,
        });
        col.imported.markers += 1;
    }
    out.sort_by_key(|m| m.time);
    out
}

/// The Time Remapping of `layer`, as EffectCraft's `timeRemap` property; `None` when the remap is
/// off. `default_seconds` is the value before the first key (the source time at the in point).
pub(crate) fn import_time_remap(layer: &AepLayer<'_, '_>, name: &str, tm: &TimeMap, ids: &mut Ids<'_>, col: &mut Collector) -> Option<Node> {
    let Some(PropNode::Property(p)) = layer.properties().and_then(|g| g.get("ADBE Time Remapping")) else { return None };
    if !p.is_animated() && p.flags() != Some(1) && p.expression().is_none() {
        return None;
    }
    let mut prop = ids.prop("timeRemap", "Time Remap", Value::Scalar(0.0)).with_ui(ParamUi::Number);
    if let Some(v) = p.static_value().and_then(|v| v.first().copied()).filter(|v| v.is_finite()) {
        prop.value = Value::Scalar(v);
    }
    if p.is_animated() {
        let before = col.imported.keyframes;
        import_keys(&p, Kind::TimeRemap, name, "ADBE Time Remapping", tm, &mut prop, col);
        if col.imported.keyframes == before {
            col.lose(LossCategory::TimeRemapping, format!("{name}: Time Remapping has no readable keyframe, not imported"));
            return None;
        }
    }
    import_expression(&p, &mut prop, col);
    col.imported.time_remaps += 1;
    Some(Node::Prop(prop))
}

/// Puts the `timeRemap` property first in the layer's properties.
pub(crate) fn insert_time_remap(props: &mut PropGroup, node: Node) {
    props.children.insert(0, node);
}
