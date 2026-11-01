use super::{EffectcraftApp, LayerId, Mat3, Pos2, Rect, Tick, Tool, ViewerMap, ov, vec2};

/// What a drag in the viewer is doing.
#[derive(Clone, Debug)]
pub(super) enum Gesture {
    /// Per layer: start position and the parent-space change per comp pixel of drag in x and y.
    Move {
        layers: Vec<(LayerId, [f64; 3], [f64; 3], [f64; 3])>,
        start: [f64; 2],
        /// The grabbed layer's feature nearest the pointer (comp px at the start): it snaps.
        snap_src: Option<[f64; 2]>,
    },
    Camera {
        tool: Tool,
    },
    /// A bounding-box handle: scales about the anchor point so the grabbed point follows the
    /// pointer (corners both axes, edges one), relative to the layer as the drag began; the
    /// handle snaps.
    Scale {
        layer: LayerId,
        start_scale: [f64; 3],
        /// The grabbed point relative to the anchor, in layer pixels.
        start_local: [f64; 2],
        /// The handle's point relative to the anchor, in layer pixels.
        handle: [f64; 2],
        anchor: [f64; 2],
        axes: [bool; 2],
        /// Layer pixels → comp as the drag began, and its inverse.
        l2c: Mat3,
        inv: Mat3,
        /// The other selected layers and their scales as the drag began: they scale by the same
        /// ratio about their own anchor points, as in After Effects (#491).
        others: Vec<(LayerId, [f64; 3])>,
    },
    Rotate {
        layer: LayerId,
        center: Pos2,
        /// The pointer's angle about the centre on the last frame (degrees, as atan2 gives it).
        last_angle: f64,
        /// The turn so far, summed frame by frame so it keeps counting past a full revolution in
        /// either direction (After Effects: 1x +0.0°, −1x −90.0°…) instead of wrapping at ±180°.
        turned: f64,
        start_rot: f64,
        /// The other selected layers and their rotations: they turn by the same angle.
        others: Vec<(LayerId, f64)>,
    },
    Anchor {
        layer: LayerId,
        start_anchor: [f64; 3],
        start_pos: [f64; 3],
        start: [f64; 2],
        inv: Mat3,
        l2p: Mat3,
        /// The layer's own snap targets as the drag began (its box and path vertices).
        own: Vec<effectcraft_engine::viewer::SnapTarget>,
        /// Alt held as the drag began: the anchor point moves alone, so the layer shifts.
        anchor_only: bool,
    },
    Pan {
        start_pan: [f32; 2],
    },
    Create {
        tool: Tool,
        start: [f64; 2],
    },
    Marquee {
        start: Pos2,
        /// Active paths; empty means layer selection rather than point selection.
        paths: Vec<(LayerId, u64)>,
    },
    /// Dragging selected mask / shape path vertices: press point, the grabbed vertex (comp px,
    /// it snaps), the comp-space move applied so far and comp → path space.
    Vertices {
        start: [f64; 2],
        src: [f64; 2],
        applied: [f64; 2],
        layer: LayerId,
    },
    /// Dragging a vertex's Bezier tangent (both sides symmetric unless Alt breaks them).
    Tangent {
        layer: LayerId,
        mask: u64,
        index: usize,
        out: bool,
        vertex: [f64; 2],
        inv: Mat3,
    },
    /// Dragging a Puppet pin (comp → layer matrix inverse).
    PuppetPin {
        layer: LayerId,
        pin: u64,
        inv: Mat3,
    },
    /// Rotating an Advanced / Bend pin by its ring: the pin's screen centre, the pointer's last
    /// angle (radians) and the rotation so far (°, accumulated so it can pass a full turn).
    PuppetRotate {
        layer: LayerId,
        pin: u64,
        center: Pos2,
        last: f32,
        rotation: f64,
    },
    /// Scaling an Advanced / Bend pin by its square: the pin's screen centre, the press distance
    /// from it and the scale at the press (1 = 100 %).
    PuppetScale {
        layer: LayerId,
        pin: u64,
        center: Pos2,
        start: f32,
        scale: f64,
    },
    /// Puppet tools: a marquee selecting pins (dragged outside the layer's art, or Alt-dragged).
    PinMarquee {
        start: Pos2,
    },
    /// Object Matte tool: a box around the object (screen point where the drag began).
    ObjectMatteBox {
        start: Pos2,
    },
    /// Puppet tool Record mode (⌘/Ctrl-drag a pin): the drag is sampled in real time (seconds
    /// since it began, effect space) and becomes keyframes on release (`puppet.recordPin`); the
    /// current time runs along while recording.
    PuppetRecord {
        layer: LayerId,
        pin: u64,
        /// Other selected pins of the layer, moving with the dragged one.
        others: Vec<u64>,
        inv: Mat3,
        began: f64,
        cti: Tick,
        samples: Vec<(f64, [f64; 2])>,
    },
    /// Pen tool: pulling the tangents of the vertex just placed.
    Pen {
        layer: LayerId,
        mask: u64,
        index: usize,
        vertex: [f64; 2],
        inv: Mat3,
        press: Pos2,
    },
    /// Dragging a track point's feature/search region, a corner or its attach point.
    Track {
        layer: LayerId,
        tracker: u64,
        drag: crate::panels::tracker::Drag,
    },
    /// Motion paths, free transform, Mask Feather, guides and the region of interest
    /// (`viewer_overlays`).
    Overlay(ov::Drag),
}

/// Pen tool path in progress: (layer, mask uid).
pub(super) fn pen_id() -> egui::Id {
    egui::Id::unique("viewer-pen")
}

pub(super) const HANDLE: f32 = 7.0;

/// How close (screen points) the pointer must be to a path vertex or tangent handle to grab it.
const VERTEX_GRAB: f32 = 6.0;

/// The reach of a path vertex at `pos` (#1042): a vertex marker sitting under, or just beside, a
/// bounding-box handle would lose the press to the handle (the handle reaches further than the
/// vertex), so wherever a box handle is in range the vertex reaches as far as the handle does.
/// Vertices then win in the handle's whole zone, as in After Effects.
pub(super) fn vertex_grab_radius(pos: Pos2, handles: impl IntoIterator<Item = Pos2>) -> f32 {
    if handles.into_iter().any(|h| h.distance(pos) < HANDLE + 2.0) { HANDLE + 2.0 } else { VERTEX_GRAB }
}

/// Shown when a Roto Brush tool is used in the Composition panel (#753).
pub(super) const ROTO_HINT: &str = "The Roto Brush works in the Layer panel. Double-click the layer to open it.";

/// Region of interest handles on screen: corners (0–3, clockwise from top-left) then edge
/// midpoints (4 top, 5 right, 6 bottom, 7 left).
pub(super) fn roi_handles(r: Rect) -> [Pos2; 8] {
    [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.center_top(), r.right_center(), r.center_bottom(), r.left_center()]
}

/// A drag starting on a region of interest handle: the ROI redrawn from the opposite corner (or
/// edge, with the other axis kept).
pub(super) fn roi_drag(app: &EffectcraftApp, map: &ViewerMap, comp_rect: Rect, zoom: f32, press: Pos2) -> Option<ov::Drag> {
    let [rx, ry, rw, rh] = app.session.state.region_of_interest?;
    let r = Rect::from_min_size(comp_rect.min + vec2(rx as f32 * zoom, ry as f32 * zoom), vec2(rw as f32 * zoom, rh as f32 * zoom));
    let i = roi_handles(r).iter().position(|h| h.distance(press) < 6.0)?;
    let (x0, y0, x1, y1) = (rx, ry, rx + rw, ry + rh);
    let _ = map;
    Some(match i {
        0 => ov::Drag::Roi { start: [x1, y1], keep: [None, None] },
        1 => ov::Drag::Roi { start: [x0, y1], keep: [None, None] },
        2 => ov::Drag::Roi { start: [x0, y0], keep: [None, None] },
        3 => ov::Drag::Roi { start: [x1, y0], keep: [None, None] },
        4 => ov::Drag::Roi { start: [x1, y1], keep: [Some(x0), None] },
        5 => ov::Drag::Roi { start: [x0, y0], keep: [None, Some(y1)] },
        6 => ov::Drag::Roi { start: [x0, y0], keep: [Some(x1), None] },
        _ => ov::Drag::Roi { start: [x1, y0], keep: [None, Some(y1)] },
    })
}

/// Pointer and gesture origin for one constrained path-vertex update.
pub(super) struct VertexMove<'a> {
    pub ctx: &'a egui::Context,
    pub start: [f64; 2],
    pub src: [f64; 2],
    pub applied: [f64; 2],
    pub layer: LayerId,
    pub cpt: [f64; 2],
    pub mods: egui::Modifiers,
    pub merge: &'a str,
}

pub(super) fn move_vertices(app: &mut EffectcraftApp, ectx: &super::EvalCtx, map: &ViewerMap, frame: VertexMove<'_>) -> [f64; 2] {
    let VertexMove { ctx, start, src, applied, layer, cpt, mods, merge } = frame;
    let want = super::ov::constrain_move([cpt[0] - start[0], cpt[1] - start[1]], mods.shift);
    let source = [src[0] + want[0], src[1] + want[1]];
    let correction = if mods.shift {
        let axis = if want[0].abs() >= want[1].abs() { super::vt::SnapAxis::Horizontal } else { super::vt::SnapAxis::Vertical };
        super::vt::snap_along_axis(app, ctx, ectx, map, &super::vt::SnapQuery { exclude: &[layer], sources: &[source], mods, extra: &[], axis: Some(axis) })
    } else {
        super::vt::snap(app, ctx, ectx, map, &[layer], &[source], mods)
    };
    let total = [want[0] + correction[0], want[1] + correction[1]];
    let delta = [total[0] - applied[0], total[1] - applied[1]];
    let mut completed = applied;
    if delta != [0.0; 2] {
        let paths: std::collections::BTreeSet<_> = app.session.state.selected_vertices.iter().map(|v| (v.layer, v.mask)).collect();
        let paths: Vec<_> = paths.into_iter().map(|(layer, mask)| serde_json::json!({"layer": layer.0, "mask": mask})).collect();
        if let Err(e) = app.session.execute("path.freeTransform", serde_json::json!({"paths": paths, "space": "comp", "offset": delta, "merge": merge})) {
            app.ui.status = e.to_string();
        } else {
            completed = total;
        }
    }
    completed
}
