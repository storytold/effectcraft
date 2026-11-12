//! After Effects' numeric order of a layer's top-level property groups.
//!
//! In After Effects `thisLayer(4)` and `layer.property(4)` reach the same group on every layer of a
//! kind, whatever the layer holds: Marker is always 1, and the groups a layer has not used yet
//! (Time Remap, Motion Trackers, Layer Styles…) still take their slot. EffectCraft's tree holds only
//! the groups a layer has, without Marker, so its own positions differ and move as groups are
//! added. Rigs that address groups by number (`comp("Palette").layer(1)(4)(n)(1)`) read the wrong
//! group, or none, unless numbers go through this table.
//!
//! The order was observed in After Effects 26.3 by reading `layer.property(i).matchName` from a
//! script and `thisComp.layer(..)(i).name` from an expression on new layers of each kind; both
//! agree. 3D model and primitive layers have no table: their numbers stay EffectCraft's own.

use crate::{Layer, LayerSource, Node};

/// One top-level slot in After Effects' order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AeSlot {
    /// The group's match id in EffectCraft's tree, or `""` when EffectCraft has no such group.
    pub match_id: &'static str,
    /// The display name After Effects gives the slot.
    pub name: &'static str,
    /// After Effects' match name for the slot.
    pub match_name: &'static str,
}

const fn slot(match_id: &'static str, name: &'static str, match_name: &'static str) -> AeSlot {
    AeSlot { match_id, name, match_name }
}

/// The marker slot: number 1 on every layer. Markers are a field of [`Layer`], not a tree node.
pub const MARKER: AeSlot = slot("marker", "Marker", "ADBE Marker");

/// Footage, solid, adjustment, precomp and null layers.
const AV: &[AeSlot] = &[
    MARKER,
    slot("timeRemap", "Time Remap", "ADBE Time Remapping"),
    slot("motionTrackers", "Motion Trackers", "ADBE MTrackers"),
    slot("masks", "Masks", "ADBE Mask Parade"),
    slot("effects", "Effects", "ADBE Effect Parade"),
    slot("transform", "Transform", "ADBE Transform Group"),
    slot("layerStyles", "Layer Styles", "ADBE Layer Styles"),
    slot("", "Geometry Options", "ADBE Plane Options Group"),
    slot("geometryOptions", "Geometry Options", "ADBE Extrsn Options Group"),
    slot("materialOptions", "Material Options", "ADBE Material Options Group"),
    slot("audio", "Audio", "ADBE Audio Group"),
    slot("", "Data", "ADBE Data Group"),
    slot("", "Essential Properties", "ADBE Layer Overrides"),
    slot("", "Sets", "ADBE Layer Sets"),
    slot("", "Source Options", "ADBE Source Options Group"),
];

const TEXT: &[AeSlot] = &[
    MARKER,
    slot("text", "Text", "ADBE Text Properties"),
    slot("masks", "Masks", "ADBE Mask Parade"),
    slot("effects", "Effects", "ADBE Effect Parade"),
    slot("transform", "Transform", "ADBE Transform Group"),
    slot("layerStyles", "Layer Styles", "ADBE Layer Styles"),
    slot("geometryOptions", "Geometry Options", "ADBE Extrsn Options Group"),
    slot("materialOptions", "Material Options", "ADBE Material Options Group"),
    slot("audio", "Audio", "ADBE Audio Group"),
    slot("", "Sets", "ADBE Layer Sets"),
];

const SHAPE: &[AeSlot] = &[
    MARKER,
    slot("contents", "Contents", "ADBE Root Vectors Group"),
    slot("masks", "Masks", "ADBE Mask Parade"),
    slot("effects", "Effects", "ADBE Effect Parade"),
    slot("transform", "Transform", "ADBE Transform Group"),
    slot("layerStyles", "Layer Styles", "ADBE Layer Styles"),
    slot("geometryOptions", "Geometry Options", "ADBE Extrsn Options Group"),
    slot("materialOptions", "Material Options", "ADBE Material Options Group"),
    slot("audio", "Audio", "ADBE Audio Group"),
    slot("", "Sets", "ADBE Layer Sets"),
];

const CAMERA: &[AeSlot] =
    &[MARKER, slot("transform", "Transform", "ADBE Transform Group"), slot("cameraOptions", "Camera Options", "ADBE Camera Options Group")];

const LIGHT: &[AeSlot] = &[MARKER, slot("transform", "Transform", "ADBE Transform Group"), slot("lightOptions", "Light Options", "ADBE Light Options Group")];

/// After Effects' top-level slots for a layer of this kind, or `None` for 3D model and primitive
/// layers, which keep EffectCraft's own numbers.
pub fn ae_slots(source: &LayerSource) -> Option<&'static [AeSlot]> {
    match source {
        LayerSource::Footage { .. } | LayerSource::Comp { .. } | LayerSource::Solid { .. } | LayerSource::Null => Some(AV),
        LayerSource::Text => Some(TEXT),
        LayerSource::Shape => Some(SHAPE),
        LayerSource::Camera => Some(CAMERA),
        LayerSource::Light { .. } => Some(LIGHT),
        LayerSource::Model { .. } | LayerSource::Primitive { .. } => None,
    }
}

/// What After Effects' top-level number `index` (1-based) reaches on `layer`.
#[derive(Clone, Copy, Debug)]
pub enum AeChild<'a> {
    /// The layer's markers (number 1).
    Marker,
    /// A group or property the layer has.
    Node(&'a Node),
    /// A slot After Effects always has and this layer does not (an empty group there).
    Absent(&'static AeSlot),
}

/// The top-level node After Effects' number `index` (1-based) reaches, or `None` when the number is
/// past the last slot or the layer kind has no table ([`ae_slots`]).
pub fn ae_child(layer: &Layer, index: usize) -> Option<AeChild<'_>> {
    let s = ae_slots(&layer.source)?.get(index.checked_sub(1)?)?;
    if *s == MARKER {
        return Some(AeChild::Marker);
    }
    if s.match_id.is_empty() {
        return Some(AeChild::Absent(s));
    }
    Some(layer.props.children.iter().find(|n| n.match_id() == s.match_id).map_or(AeChild::Absent(s), AeChild::Node))
}

/// After Effects' 1-based number for the top-level node with this match id, when it has one.
pub fn ae_index(layer: &Layer, match_id: &str) -> Option<usize> {
    if match_id.is_empty() {
        return None;
    }
    ae_slots(&layer.source)?.iter().position(|s| s.match_id == match_id).map(|i| i + 1)
}

/// How many top-level properties After Effects counts on `layer` (`numProperties`), or `None` for
/// layer kinds without a table.
pub fn ae_count(layer: &Layer) -> Option<usize> {
    ae_slots(&layer.source).map(<[AeSlot]>::len)
}

/// After Effects' 1-based number for the top-level slot with this After Effects match name
/// (`ADBE Effect Parade`), when the layer kind has one.
pub fn ae_index_of_match_name(layer: &Layer, match_name: &str) -> Option<usize> {
    ae_slots(&layer.source)?.iter().position(|s| s.match_name == match_name).map(|i| i + 1)
}
