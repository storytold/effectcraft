//! Masks and bezier paths.
//!
//! A layer's masks are in `tdgp[ADBE Mask Parade]`: one `mkif` chunk (mode and invert switch) next
//! to one `tdgp[ADBE Mask Atom]` per mask. The atom holds the optional properties feather, opacity
//! and expansion (stored only when not at their defaults) and `om-s[ADBE Mask Shape]`: the path
//! property (`tdbs`, with a key list when animated) and `omks` with one `LIST shap` per keyframe (a
//! single one when the path is static).

use crate::Chunk;
use crate::bytes::{be_f32, be_u32, until_nul};
use crate::typed::{KeyList, Layer, Project, PropGroup, PropNode, Property};
use crate::values::{Tracked, read};

/// The mask mode (`mkif` `u32` at byte 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaskMode {
    /// `0` (329)
    None,
    /// `1`
    Add,
    /// `2`
    Subtract,
    /// `3`
    Intersect,
    /// `4`
    Lighten,
    /// `5`
    Darken,
    /// `6`
    Difference,
    /// Any other value (not sampled).
    Other(u32),
}

impl MaskMode {
    /// The mode for a stored id.
    pub fn from_id(id: u32) -> Self {
        match id {
            0 => Self::None,
            1 => Self::Add,
            2 => Self::Subtract,
            3 => Self::Intersect,
            4 => Self::Lighten,
            5 => Self::Darken,
            6 => Self::Difference,
            other => Self::Other(other),
        }
    }
}

/// A vertex of a bezier path with its two control points as absolute positions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PathVertex {
    pub position: [f64; 2],
    /// The control point of the incoming segment.
    pub in_handle: [f64; 2],
    /// The control point of the outgoing segment.
    pub out_handle: [f64; 2],
}

impl PathVertex {
    /// The in tangent: the in handle minus the vertex (what the scripting API calls `inTangents`).
    pub fn in_tangent(&self) -> [f64; 2] {
        [self.in_handle[0] - self.position[0], self.in_handle[1] - self.position[1]]
    }

    /// The out tangent: the out handle minus the vertex (`outTangents`).
    pub fn out_tangent(&self) -> [f64; 2] {
        [self.out_handle[0] - self.position[0], self.out_handle[1] - self.position[1]]
    }
}

/// One path shape (`LIST shap`): a bounding box and the control points normalised inside it.
///
/// The `shph` chunk holds a magic word whose last byte carries the flags (`0x01` closed, `0x09` open:
/// bit `0x08`), the box (left, top, right, bottom) as four `f32`, and the byte at offset 20 (`1` in
/// every sample). The box is the box of the vertices **and** their handles. The `list` holds three
/// points per vertex, each as two `f32` in [0, 1] of the box, in the order `[vertex i, out handle i,
/// in handle i + 1]`; the last record is the in handle of vertex 0. For masks the box is normalised
/// to the **layer** size (the size of the layer's source, not the composition's: on a 100x100 solid
/// a 60x40 mask rectangle at the centre has the box `0.2, 0.3, 0.8, 0.7`, 309); for the paths of shape layers it is in pixels. Handles equal their vertex when a
/// vertex has no tangents; a tangent is the handle minus the vertex, without a sign change.
#[derive(Debug, Clone, PartialEq)]
pub struct ShapePath {
    /// Left, top, right, bottom.
    pub bbox: [f32; 4],
    /// The byte at offset 20 of `shph` (`1` in every sample, also for the open path).
    pub closed_flag: u8,
    /// True for an open path: bit `0x08` of the last byte of the first `shph` word.
    pub open: bool,
    /// The normalised points as stored, three per vertex.
    pub points: Vec<[f32; 2]>,
}

impl ShapePath {
    /// Decodes a `LIST shap`; `None` when `shph` or the point list is malformed.
    pub fn from_chunk(shap: &Chunk<'_>) -> Option<Self> {
        let shph = shap.child(b"shph")?.bytes()?;
        let bbox = [be_f32(shph, 4)?, be_f32(shph, 8)?, be_f32(shph, 12)?, be_f32(shph, 16)?];
        let closed_flag = *shph.get(20)?;
        let open = shph.get(3)? & 0x08 != 0;
        let list = KeyList::from_chunk(shap.child_list(b"list")?)?;
        if list.header.record_size != 8 {
            return None;
        }
        let points = list.records().map(|r| Some([be_f32(r, 0)?, be_f32(r, 4)?])).collect::<Option<Vec<_>>>()?;
        Some(Self { bbox, closed_flag, open, points })
    }

    /// The number of vertices (`points / 3`).
    pub fn vertex_count(&self) -> usize {
        self.points.len() / 3
    }

    /// The vertices with absolute control points: `box.left + u * (box.right - box.left)` (and the
    /// same for y), multiplied by `scale` (the layer size in pixels for a mask, `(1, 1)` for a
    /// shape layer path).
    pub fn vertices(&self, scale: (f64, f64)) -> Vec<PathVertex> {
        let [left, top, right, bottom] = self.bbox.map(f64::from);
        let abs = |p: Option<&[f32; 2]>| -> [f64; 2] {
            let [u, v] = p.copied().unwrap_or([0.0, 0.0]).map(f64::from);
            [(left + u * (right - left)) * scale.0, (top + v * (bottom - top)) * scale.1]
        };
        let n = self.vertex_count();
        let total = n.saturating_mul(3);
        (0..n)
            .map(|i| {
                let base = i.saturating_mul(3);
                PathVertex {
                    position: abs(self.points.get(base)),
                    out_handle: abs(self.points.get(base.saturating_add(1))),
                    // The in handle of vertex i is the record before vertex i (the last one for vertex 0).
                    in_handle: abs(self.points.get(base.saturating_add(total).saturating_sub(1) % total.max(1))),
                }
            })
            .collect()
    }
}

/// One mask of a layer.
#[derive(Debug, Clone, PartialEq)]
pub struct Mask {
    /// The mask name (`Mask 1`).
    pub name: Option<String>,
    /// The mode, from this mask's `mkif` (there is one `mkif` per mask, directly before its atom).
    pub mode: Option<MaskMode>,
    /// The invert switch (`mkif` byte 0).
    pub inverted: Option<bool>,
    /// The running mask id counter of the layer (`mkif` `u32` at byte 8): 1 for the first mask
    /// created on a layer, +1 per mask created later on the same layer (undo does not reset it).
    /// A session counter, not a stable id.
    pub counter: Option<u32>,
    /// Feather in pixels `[x, y]`; absent = `[0, 0]`.
    pub feather: Tracked<[f64; 2]>,
    /// Opacity in percent (stored as a fraction); absent = 100.
    pub opacity_percent: Tracked<f64>,
    /// Expansion in pixels; absent = 0.
    pub expansion: Tracked<f64>,
    /// True when the path has keyframes.
    pub path_animated: bool,
    /// The path keyframe times in seconds (empty for a static path).
    pub path_key_seconds: Vec<f64>,
    /// The path shapes: one for a static path, one per keyframe otherwise.
    pub shapes: Vec<ShapePath>,
}

fn mask_from_atom(atom: &PropGroup<'_, '_>, mkif: Option<&[u8]>) -> Mask {
    let opacity = read(atom, "ADBE Mask Opacity", [100.0], 100.0);
    let expansion = read(atom, "ADBE Mask Offset", [0.0], 1.0);
    let mut mask = Mask {
        name: atom.user_name(),
        mode: mkif.and_then(|m| be_u32(m, 4)).map(MaskMode::from_id),
        inverted: mkif.and_then(|m| m.first()).map(|b| *b != 0),
        counter: mkif.and_then(|m| be_u32(m, 8)),
        feather: read(atom, "ADBE Mask Feather", [0.0, 0.0], 1.0),
        opacity_percent: Tracked { value: opacity.value[0], source: opacity.source },
        expansion: Tracked { value: expansion.value[0], source: expansion.source },
        path_animated: false,
        path_key_seconds: Vec::new(),
        shapes: Vec::new(),
    };
    if let Some(PropNode::Other(oms)) = atom.get("ADBE Mask Shape") {
        if let Some(prop) = oms.child_list(b"tdbs").and_then(Property::from_chunk) {
            mask.path_animated = prop.is_animated();
            if let (Some(keys), Some(scale)) = (prop.keys(), prop.time_scale()) {
                mask.path_key_seconds = keys.keyframes().filter_map(|k| k.seconds(scale)).collect();
            }
        }
        if let Some(omks) = oms.child_list(b"omks") {
            mask.shapes = omks.children().iter().filter(|c| c.is_list(b"shap")).filter_map(ShapePath::from_chunk).collect();
        }
    }
    mask
}

impl<'f, 'a> Project<'f, 'a> {
    /// The vertices of every path shape of a mask in pixels of the layer: mask boxes are normalised
    /// to the **layer** size (the size of its source), not the composition's. `None` when the layer
    /// has no source with a size.
    pub fn mask_vertices_px(&self, layer: &Layer<'f, 'a>, mask: &Mask) -> Option<Vec<Vec<PathVertex>>> {
        let (width, height) = self.layer_source_size(layer)?;
        Some(mask.shapes.iter().map(|s| s.vertices((f64::from(width), f64::from(height)))).collect())
    }
}

/// One mask with the views of its records, for readers that need more than [`Mask`]: the
/// keyframes and expressions of the mask properties (in [`MaskAtom::atom`]: `ADBE Mask Feather`,
/// `ADBE Mask Opacity`, `ADBE Mask Offset`) and the path keyframes paired with their shapes.
#[derive(Debug, Clone)]
pub struct MaskAtom<'f, 'a> {
    /// The decoded mask (as [`Layer::masks`] returns it).
    pub mask: Mask,
    /// The atom group (`tdgp[ADBE Mask Atom]`).
    pub atom: PropGroup<'f, 'a>,
    /// The path property (`om-s[ADBE Mask Shape]/tdbs`): its key list holds one 64-byte record
    /// per path keyframe (time, interpolation, label and roving flags; 093).
    pub path: Option<Property<'f, 'a>>,
    /// Every `LIST shap` of `omks` in file order, `None` where one is malformed, so that index `i`
    /// stays the shape of path keyframe `i` (one entry for a static path).
    pub path_shapes: Vec<Option<ShapePath>>,
}

impl<'f, 'a> Layer<'f, 'a> {
    /// The layer's masks, in stack order.
    pub fn masks(&self) -> Vec<Mask> {
        self.mask_atoms().into_iter().map(|m| m.mask).collect()
    }

    /// The layer's masks with the views of their records, in stack order.
    pub fn mask_atoms(&self) -> Vec<MaskAtom<'f, 'a>> {
        let Some(root) = self.properties() else { return Vec::new() };
        let Some(PropNode::Group(parade)) = root.get("ADBE Mask Parade") else { return Vec::new() };
        // Per mask: `tdmn` (Mask Atom), `mkif`, then the atom's `tdgp`.
        let mut masks = Vec::new();
        let (mut name, mut mkif): (Option<&[u8]>, Option<&[u8]>) = (None, None);
        for c in parade.chunk().children() {
            if c.is(b"tdmn") {
                name = c.bytes();
            } else if c.is(b"mkif") {
                mkif = c.bytes();
            } else if c.is_list(b"tdgp") {
                let is_atom = name.take().is_some_and(|n| until_nul(n) == b"ADBE Mask Atom");
                if is_atom && let Some(atom) = PropGroup::from_chunk(c) {
                    let mask = mask_from_atom(&atom, mkif.take());
                    let (mut path, mut path_shapes) = (None, Vec::new());
                    if let Some(PropNode::Other(oms)) = atom.get("ADBE Mask Shape") {
                        path = oms.child_list(b"tdbs").and_then(Property::from_chunk);
                        if let Some(omks) = oms.child_list(b"omks") {
                            path_shapes = omks.children().iter().filter(|c| c.is_list(b"shap")).map(ShapePath::from_chunk).collect();
                        }
                    }
                    masks.push(MaskAtom { mask, atom, path, path_shapes });
                }
            }
        }
        masks
    }
}
