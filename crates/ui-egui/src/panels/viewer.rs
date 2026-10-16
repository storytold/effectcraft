//! The Composition viewer: pasteboard + comp frame, magnification and resolution, transparency
//! grid, guides, layer bounding boxes with handles, anchor points, motion paths and masks, and
//! direct manipulation (move, scale, rotate, pan behind, shape/type creation, hand and zoom),
//! 3D views (Active Camera / Front / … / Custom View 3), camera tools (orbit, pan, dolly) and
//! camera/light wireframes.

use effectcraft_engine::commands::shape_tool::PaintKind;
use effectcraft_engine::effects::puppet::PinKind;
use effectcraft_engine::geom::{Mat3, vec2 as gv2};
use effectcraft_engine::project::{Comp, Layer, LayerId, LayerSource};
use effectcraft_engine::render::EvalCtx;
use effectcraft_engine::render::three_d::{self, CameraState, View3D};
use effectcraft_engine::time::Tick;
use egui::{Align2, Color32, Pos2, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use serde_json::json;

use crate::state::Tool;
use crate::theme::Tokens;
use crate::{EffectcraftApp, widgets};

/// Viewer mapping published each frame (for the control channel and other panels).
#[derive(Clone, Copy, Debug)]
pub struct ViewerMap {
    pub origin: Pos2,
    /// Points per comp pixel.
    pub zoom: f32,
    pub comp: [f32; 2],
    pub area: Rect,
}

impl ViewerMap {
    pub fn to_screen(&self, p: [f64; 2]) -> Pos2 {
        pos2(self.origin.x + p[0] as f32 * self.zoom, self.origin.y + p[1] as f32 * self.zoom)
    }
    pub fn to_comp(&self, p: Pos2) -> [f64; 2] {
        [((p.x - self.origin.x) / self.zoom) as f64, ((p.y - self.origin.y) / self.zoom) as f64]
    }
}

fn map_id() -> egui::Id {
    egui::Id::new("viewer-map")
}

pub fn comp_to_screen(ctx: &egui::Context, p: [f32; 2]) -> Option<Pos2> {
    let m: ViewerMap = ctx.data(|d| d.get_temp(map_id()))?;
    Some(m.to_screen([p[0] as f64, p[1] as f64]))
}

pub fn screen_to_comp(ctx: &egui::Context, p: Pos2) -> Option<[f64; 2]> {
    let m: ViewerMap = ctx.data(|d| d.get_temp(map_id()))?;
    Some(m.to_comp(p))
}

/// The fit magnification from the last frame.
pub fn last_fit(ctx: &egui::Context) -> f32 {
    ctx.data(|d| d.get_temp::<f32>(egui::Id::new("viewer-fit"))).unwrap_or(0.5)
}

/// What a drag in the viewer is doing.
#[derive(Clone, Debug)]
enum Gesture {
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
    },
    /// Dragging selected mask / shape path vertices: press point, the grabbed vertex (comp px,
    /// it snaps), the comp-space move applied so far and comp → path space.
    Vertices {
        start: [f64; 2],
        src: [f64; 2],
        applied: [f64; 2],
        inv: Mat3,
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
        drag: super::tracker::Drag,
    },
    /// Motion paths, free transform, Mask Feather, guides and the region of interest
    /// (`viewer_overlays`).
    Overlay(ov::Drag),
}

/// Pen tool path in progress: (layer, mask uid).
fn pen_id() -> egui::Id {
    egui::Id::new("viewer-pen")
}

use super::viewer_overlays::{self as ov, VertexHit};
use super::viewer_tools as vt;

const HANDLE: f32 = 7.0;
/// How close (screen points) the pointer must be to a path vertex or tangent handle to grab it.
const VERTEX_GRAB: f32 = 6.0;

/// The reach of a path vertex at `pos` (#1042): a vertex marker sitting under, or just beside, a
/// bounding-box handle would lose the press to the handle (the handle reaches further than the
/// vertex), so wherever a box handle is in range the vertex reaches as far as the handle does.
/// Vertices then win in the handle's whole zone, as in After Effects.
fn vertex_grab_radius(pos: Pos2, handles: impl IntoIterator<Item = Pos2>) -> f32 {
    if handles.into_iter().any(|h| h.distance(pos) < HANDLE + 2.0) { HANDLE + 2.0 } else { VERTEX_GRAB }
}
/// Shown when a Roto Brush tool is used in the Composition panel (#753).
const ROTO_HINT: &str = "The Roto Brush works in the Layer panel. Double-click the layer to open it.";

thread_local! {
    /// The 3D view camera of the viewer being drawn (None = the comp's active camera).
    static VIEW_CAM: std::cell::Cell<Option<CameraState>> = const { std::cell::Cell::new(None) };
}

/// Region of interest handles on screen: corners (0–3, clockwise from top-left) then edge
/// midpoints (4 top, 5 right, 6 bottom, 7 left).
fn roi_handles(r: Rect) -> [Pos2; 8] {
    [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.center_top(), r.right_center(), r.center_bottom(), r.left_center()]
}

/// A drag starting on a region of interest handle: the ROI redrawn from the opposite corner (or
/// edge, with the other axis kept).
fn roi_drag(app: &EffectcraftApp, map: &ViewerMap, comp_rect: Rect, zoom: f32, press: Pos2) -> Option<ov::Drag> {
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

/// The camera the viewer shows (view camera or the comp's active camera).
fn cam_state(ctx: &EvalCtx) -> CameraState {
    VIEW_CAM.with(|c| c.get()).unwrap_or_else(|| three_d::active_camera(ctx))
}

/// Layer → comp (viewer) matrix through the current 3D view.
pub(crate) fn l2c(ctx: &EvalCtx, layer: &Layer) -> (Mat3, f64) {
    match VIEW_CAM.with(|c| c.get()) {
        Some(cam) if layer.is_3d() => (three_d::layer_to_view(ctx, layer, &cam), 0.0),
        _ => ctx.layer_to_comp(layer),
    }
}

/// `l2c` (layer → comp) applied from effect space instead: effect points and Puppet pins,
/// measured from the top-left of the layer's effect bounds (see `effect_bounds`).
pub(crate) fn from_effect_space(ctx: &EvalCtx, layer: &Layer, l2c: Mat3) -> Mat3 {
    let o = ctx.effect_bounds(layer).1;
    l2c * Mat3::translate(gv2(o[0], o[1]))
}

/// Effect space → comp (viewer) matrix through the current 3D view.
pub(crate) fn fx2c(ctx: &EvalCtx, layer: &Layer) -> Mat3 {
    from_effect_space(ctx, layer, l2c(ctx, layer).0)
}

/// The selected, unlocked layers of `comp` other than `layer` (when `layer` is one of them): a
/// handle or Rotation tool drag on one changes them all.
fn selected_others<'a>(app: &EffectcraftApp, comp: &'a Comp, layer: LayerId) -> Vec<&'a Layer> {
    let sel = &app.session.state.selected_layers;
    if !sel.contains(&layer) {
        return vec![];
    }
    sel.iter().filter(|id| **id != layer).filter_map(|id| comp.layer(*id)).filter(|l| !l.switches.locked && l.transform().is_some()).collect()
}

/// The least homogeneous depth (`w`: the distance in front of a perspective camera in pixels, 1
/// for 2D layers and orthographic views) a viewer outline is drawn at: points at or behind the
/// camera would flip through infinity and draw long lines across the viewer (#572).
const NEAR_W: f64 = 0.5;

/// `p` through the homography `m` before the perspective divide: (x, y, w).
fn homogeneous(m: &Mat3, p: [f64; 2]) -> [f64; 3] {
    let r = |i: usize| m.0.get(i).map_or(0.0, |row| row[0] * p[0] + row[1] * p[1] + row[2]);
    [r(0), r(1), r(2)]
}

/// The closed polygon `poly` (layer pixels) through `m`, clipped to the part in front of the
/// camera's near plane (comp pixels; empty when none of it is).
fn project_clipped(m: &Mat3, poly: &[[f64; 2]]) -> Vec<[f64; 2]> {
    let h: Vec<[f64; 3]> = poly.iter().map(|p| homogeneous(m, *p)).collect();
    let mut out = vec![];
    for (i, a) in h.iter().enumerate() {
        let Some(b) = h.get((i + 1) % h.len()) else { continue };
        let (ina, inb) = (a[2] >= NEAR_W, b[2] >= NEAR_W);
        if ina {
            out.push(*a);
        }
        if ina != inb {
            let t = (NEAR_W - a[2]) / (b[2] - a[2]);
            out.push([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, NEAR_W]);
        }
    }
    out.iter().map(|q| [q[0] / q[2], q[1] / q[2]]).filter(|q| q[0].is_finite() && q[1].is_finite()).collect()
}

/// The segment a → b (world space) through the projection `pc`, clipped at the near plane.
fn project_segment(pc: &effectcraft_engine::geom::Mat4, a: effectcraft_engine::geom::Vec3, b: effectcraft_engine::geom::Vec3) -> Option<[[f64; 2]; 2]> {
    let h = |p: effectcraft_engine::geom::Vec3| {
        let r = |i: usize| pc.0.get(i).map_or(0.0, |row| row[0] * p.x + row[1] * p.y + row[2] * p.z + row[3]);
        [r(0), r(1), r(3)]
    };
    let (mut ha, mut hb) = (h(a), h(b));
    if ha[2] < NEAR_W && hb[2] < NEAR_W {
        return None;
    }
    let cut = |p: [f64; 3], q: [f64; 3]| {
        let t = (NEAR_W - p[2]) / (q[2] - p[2]);
        [p[0] + (q[0] - p[0]) * t, p[1] + (q[1] - p[1]) * t, NEAR_W]
    };
    if ha[2] < NEAR_W {
        ha = cut(ha, hb);
    } else if hb[2] < NEAR_W {
        hb = cut(hb, ha);
    }
    let d = |q: [f64; 3]| [q[0] / q[2], q[1] / q[2]];
    let s = [d(ha), d(hb)];
    s.iter().flatten().all(|v| v.is_finite()).then_some(s)
}

/// The content box corners (layer pixels) of `layer`.
fn box_corners(b: [f64; 4]) -> [[f64; 2]; 4] {
    [[b[0], b[1]], [b[2], b[1]], [b[2], b[3]], [b[0], b[3]]]
}

/// Layer → comp matrix and its bounds quad (in comp pixels). None for a 3D layer reaching to or
/// behind the camera (its quad would flip; [`layer_outline`] draws the part in front).
pub(crate) fn layer_quad(ctx: &EvalCtx, layer: &Layer) -> Option<(Mat3, [[f64; 2]; 4], [f64; 4])> {
    let b = effectcraft_engine::render::content_bounds(ctx, layer)?;
    let (m, _) = l2c(ctx, layer);
    let corners = box_corners(b);
    if corners.iter().any(|p| homogeneous(&m, *p)[2] < NEAR_W) {
        return None;
    }
    let pts = corners.map(|p| {
        let q = m.apply(gv2(p[0], p[1]));
        [q.x, q.y]
    });
    Some((m, pts, b))
}

/// The outline of a layer's content box in comp pixels, clipped at the camera's near plane.
pub(crate) fn layer_outline(ctx: &EvalCtx, layer: &Layer) -> Vec<[f64; 2]> {
    let Some(b) = effectcraft_engine::render::content_bounds(ctx, layer) else { return vec![] };
    project_clipped(&l2c(ctx, layer).0, &box_corners(b))
}

/// Which axes a bounding-box handle scales: corners (0–3) both; the top and bottom edges (4, 6)
/// y; the right and left edges (5, 7) x.
fn handle_axes(handle: usize) -> [bool; 2] {
    match handle {
        4 | 6 => [false, true],
        5 | 7 => [true, false],
        _ => [true, true],
    }
}

/// A bounding-box handle's point in layer pixels (`b`: the content box; handles as in
/// [`handle_axes`], the corners clockwise from the top left, then the top, right, bottom and
/// left edges' midpoints).
fn handle_point(b: [f64; 4], handle: usize) -> [f64; 2] {
    let (cx, cy) = ((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0);
    match handle {
        0 => [b[0], b[1]],
        1 => [b[2], b[1]],
        2 => [b[2], b[3]],
        3 => [b[0], b[3]],
        4 => [cx, b[1]],
        5 => [b[2], cy],
        6 => [cx, b[3]],
        _ => [b[0], cy],
    }
}

/// The resize cursor over bounding-box handle `handle` of the on-screen box `sq` (corners
/// clockwise from the top left, handles as in [`handle_point`]): along the direction the handle
/// scales, so it turns with the layer, as in After Effects. A corner points between its two
/// edges (the box's diagonal whatever its proportions), an edge handle across its edge.
fn handle_cursor(sq: &[Pos2], handle: usize) -> egui::CursorIcon {
    let at = |k: usize| sq.get(k % 4).copied().unwrap_or(Pos2::ZERO);
    let dir = if handle < 4 {
        let c = at(handle);
        (c - at(handle + 3)).normalized() + (c - at(handle + 1)).normalized()
    } else {
        let e = at(handle - 3) - at(handle - 4);
        vec2(-e.y, e.x)
    };
    // Screen y points down: 45° is the south-east diagonal.
    let a = dir.y.atan2(dir.x).to_degrees().rem_euclid(180.0);
    if !(22.5..157.5).contains(&a) {
        egui::CursorIcon::ResizeHorizontal
    } else if a < 67.5 {
        egui::CursorIcon::ResizeNwSe
    } else if a < 112.5 {
        egui::CursorIcon::ResizeVertical
    } else {
        egui::CursorIcon::ResizeNeSw
    }
}

/// The scale ratio (x, y) that puts the point grabbed at `start` (relative to the anchor, layer
/// pixels) under the pointer at `cur`. `uniform` (Shift on a corner) keeps the aspect ratio,
/// following the pointer along the handle's diagonal. Dragging past the anchor flips the layer,
/// as in After Effects; an axis whose grabbed point sits on the anchor keeps its scale.
fn drag_ratio(start: [f64; 2], cur: [f64; 2], axes: [bool; 2], uniform: bool) -> [f64; 2] {
    if uniform {
        let d2 = start[0] * start[0] + start[1] * start[1];
        let f = if d2 > 1e-6 { (cur[0] * start[0] + cur[1] * start[1]) / d2 } else { 1.0 };
        return [f, f];
    }
    [0, 1].map(|k| if axes[k] && start[k].abs() > 1e-3 { cur[k] / start[k] } else { 1.0 })
}

/// Snap a handle drag (#252): the handle (`h`, layer pixels from the anchor point `a` as the
/// drag began; `l2c` that layer space → comp) at scale ratio `f` goes through `snap` (a comp
/// point → its correction), and the ratio that puts the handle where it snapped comes back.
/// A `uniform` drag slides the handle along its line from the anchor point to the nearer snapped
/// line; otherwise each scaled axis takes the snapped point.
fn snap_handle(f: [f64; 2], h: [f64; 2], a: [f64; 2], l2c: &Mat3, axes: [bool; 2], uniform: bool, snap: impl FnOnce([f64; 2]) -> [f64; 2]) -> [f64; 2] {
    let at = |f: [f64; 2]| {
        let q = l2c.apply(gv2(a[0] + f[0] * h[0], a[1] + f[1] * h[1]));
        [q.x, q.y]
    };
    let p = at(f);
    let c = snap(p);
    let s = [p[0] + c[0], p[1] + c[1]];
    if uniform {
        let o = at([0.0; 2]);
        let d = [p[0] - o[0], p[1] - o[1]];
        let k = [0, 1].into_iter().filter(|k| c[*k] != 0.0 && d[*k].abs() > 1e-9).min_by(|x, y| c[*x].abs().total_cmp(&c[*y].abs()));
        return k.map_or(f, |k| f.map(|v| v * (s[k] - o[k]) / d[k]));
    }
    let Some(inv) = l2c.inverse().filter(|_| c != [0.0; 2]) else { return f };
    let q = inv.apply(gv2(s[0], s[1]));
    let q = [q.x - a[0], q.y - a[1]];
    [0, 1].map(|k| if axes[k] && h[k].abs() > 1e-9 { q[k] / h[k] } else { f[k] })
}

/// The turn from angle `from` to angle `to` (degrees), wrapped to (−180°, 180°]: the shortest way
/// round, so summing it frame by frame follows the pointer across atan2's ±180° seam and past full
/// revolutions. Non-finite input turns by nothing.
fn turn_delta(from: f64, to: f64) -> f64 {
    let d = to - from;
    if !d.is_finite() {
        return 0.0;
    }
    let w = (d + 180.0).rem_euclid(360.0) - 180.0;
    if w <= -180.0 { w + 360.0 } else { w }
}

/// The Rotation tool's value: the rotation as the drag began plus the turn so far (which may span
/// revolutions), snapped to 45° steps with Shift.
fn rotate_value(start_rot: f64, turned: f64, shift: bool) -> f64 {
    let r = start_rot + turned;
    let r = if r.is_finite() { r } else { start_rot };
    if shift { (r / 45.0).round() * 45.0 } else { r }
}

/// A drag `d` (comp pixels), kept to the axis it moves along most when `shift` is held.
fn one_axis(d: [f64; 2], shift: bool) -> [f64; 2] {
    if !shift {
        d
    } else if d[0].abs() > d[1].abs() {
        [d[0], 0.0]
    } else {
        [0.0, d[1]]
    }
}

fn point_in_quad(p: [f64; 2], q: &[[f64; 2]; 4]) -> bool {
    let mut sign = 0.0;
    for i in 0..4 {
        let a = q[i];
        let b = q[(i + 1) % 4];
        let c = (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]);
        if c.abs() < 1e-9 {
            continue;
        }
        if sign == 0.0 {
            sign = c.signum();
        } else if c.signum() != sign {
            return false;
        }
    }
    true
}

/// Separating-axis test: a marquee may cross a layer without containing any corner.
fn quad_intersects_box(q: &[[f64; 2]; 4], a: [f64; 2], b: [f64; 2]) -> bool {
    if q.iter().flatten().chain(a.iter()).chain(b.iter()).any(|v| !v.is_finite()) {
        return false;
    }
    let lo = [a[0].min(b[0]), a[1].min(b[1])];
    let hi = [a[0].max(b[0]), a[1].max(b[1])];
    let rect = [lo, [hi[0], lo[1]], hi, [lo[0], hi[1]]];
    let axes = [[1.0, 0.0], [0.0, 1.0]].into_iter().chain((0..4).map(|i| {
        let j = (i + 1) % 4;
        [q[j][1] - q[i][1], q[i][0] - q[j][0]]
    }));
    for axis in axes {
        let project = |p: &[f64; 2]| p[0] * axis[0] + p[1] * axis[1];
        let (qmin, qmax) = q.iter().map(project).fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| (lo.min(v), hi.max(v)));
        let (rmin, rmax) = rect.iter().map(project).fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| (lo.min(v), hi.max(v)));
        if qmax < rmin || rmax < qmin {
            return false;
        }
    }
    true
}

/// Parent-space linear inverse (to convert a comp-space drag delta into a position delta).
fn parent_inverse(ctx: &EvalCtx, layer: &Layer) -> Mat3 {
    match layer.parent.and_then(|p| ctx.comp.layer(p)) {
        Some(p) => {
            let w = ctx.world_matrix(p);
            let m = Mat3([[w.0[0][0], w.0[0][1], 0.0], [w.0[1][0], w.0[1][1], 0.0], [0.0, 0.0, 1.0]]);
            m.inverse().unwrap_or(Mat3::IDENTITY)
        }
        None => Mat3::IDENTITY,
    }
}

/// Draw the extra views of a 2- or 4-view layout and return the main view's rectangle.
fn aux_views(
    app: &mut EffectcraftApp,
    ui: &mut egui::Ui,
    comp: &effectcraft_engine::project::Comp,
    cid: effectcraft_engine::project::ItemId,
    full: Rect,
    bg: Color32,
) -> Rect {
    let n = app.session.state.view_layout;
    if n < 2 {
        return full;
    }
    let gap = 2.0;
    let views = app.session.state.views3d.get(&cid).cloned().unwrap_or_default();
    let (main, aux): (Rect, Vec<(Rect, View3D)>) = if n == 2 {
        let mid = full.center().x;
        (
            Rect::from_min_max(pos2(mid + gap / 2.0, full.min.y), full.max),
            vec![(Rect::from_min_max(full.min, pos2(mid - gap / 2.0, full.max.y)), views.pane(0))],
        )
    } else {
        let c = full.center();
        let q = |x0: f32, y0: f32, x1: f32, y1: f32| Rect::from_min_max(pos2(x0, y0), pos2(x1, y1));
        (
            q(c.x + gap / 2.0, c.y + gap / 2.0, full.max.x, full.max.y),
            vec![
                (q(full.min.x, full.min.y, c.x - gap / 2.0, c.y - gap / 2.0), views.pane(0)),
                (q(c.x + gap / 2.0, full.min.y, full.max.x, c.y - gap / 2.0), views.pane(1)),
                (q(full.min.x, c.y + gap / 2.0, c.x - gap / 2.0, full.max.y), views.pane(2)),
            ],
        )
    };
    let ctx = ui.ctx().clone();
    let p = ui.painter().clone();
    p.rect_filled(full, 0.0, Color32::from_black_alpha(200));
    let t = app.session.time();
    let (cw, ch) = (comp.width as f32, comp.height as f32);
    let share = app.session.state.share_view_options;
    // The camera and light wireframes of each view (as in After Effects, where every view of the
    // layout shows them), evaluated on a snapshot of the project.
    let snap_project = app.session.project.clone();
    let snap_expr = app.session.expr.clone();
    let ectx = EvalCtx { project: &snap_project, comp_id: cid, comp, time: t, expr: snap_expr.as_deref(), footage: None };
    let selected = app.session.state.selected_layers.clone();
    let mut picked: Option<(usize, View3D)> = None;
    for (i, (r, v)) in aux.into_iter().enumerate() {
        p.rect_filled(r, 0.0, bg);
        let fit = ((r.width() - 20.0) / cw).min((r.height() - 20.0) / ch).max(0.01);
        let cr = Rect::from_center_size(r.center(), vec2(cw * fit, ch * fit));
        let scale = (fit * ctx.pixels_per_point()).min(1.0) as f64;
        // The Active Camera pane renders through the comp's own camera; the others through their
        // view camera.
        let through_active = v == View3D::ActiveCamera;
        let cam = if through_active { three_d::active_camera(&ectx) } else { views.cam(v, comp.width as f64, comp.height as f64).state() };
        let key = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            (app.session.revision, cid.0, t.0, i, scale.to_bits(), format!("{cam:?}")).hash(&mut h);
            h.finish()
        };
        let id = egui::Id::new(("viewer-aux", i));
        let cached: Option<(u64, egui::TextureHandle)> = ctx.data(|d| d.get_temp(id));
        let tex = match cached {
            Some((k, tex)) if k == key => tex,
            _ => {
                let opts = effectcraft_engine::render::RenderOpts {
                    scale,
                    view: (comp.has_3d() && !through_active).then_some(cam),
                    draft: true,
                    ..Default::default()
                };
                let img = app.session.render(cid, t, opts);
                let tex = crate::frames::load_fitted(&ctx, format!("viewer-aux-{i}"), crate::frames::to_color_image(&img), egui::TextureOptions::LINEAR);
                ctx.data_mut(|d| d.insert_temp(id, (key, tex.clone())));
                tex
            }
        };
        let b = comp.background;
        p.rect_filled(cr, 0.0, Color32::from_rgb((b[0] * 255.0) as u8, (b[1] * 255.0) as u8, (b[2] * 255.0) as u8));
        p.image(tex.id(), cr, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        p.rect_stroke(cr, 0.0, Stroke::new(1.0, Color32::from_black_alpha(160)), StrokeKind::Outside);
        if share && app.ui.viewer.safe_margins {
            for k in [0.9, 0.8] {
                p.rect_stroke(Rect::from_center_size(cr.center(), cr.size() * k), 0.0, Stroke::new(1.0, Color32::from_white_alpha(120)), StrokeKind::Middle);
            }
        }
        if comp.has_3d() {
            let map = ViewerMap { origin: cr.min, zoom: fit, comp: [cw, ch], area: r };
            draw_rigs(app, &p.with_clip_rect(r), &map, &ectx, &selected, cam, through_active, Some(v.id()));
        }
        let label = if comp.has_3d() { v.label() } else { crate::i18n::tr("Active Camera") };
        p.text(r.left_bottom() + vec2(8.0, -8.0), Align2::LEFT_BOTTOM, label, Tokens::ui(11.0), Color32::from_white_alpha(200));
        app.auto.add(&format!("viewer.view.{}", v.id()), r, label);
        if comp.has_3d()
            && let Some(chosen) = pane_selector(app, ui, i + 1, r, v)
        {
            picked = Some((i + 1, chosen));
        }
    }
    if let Some((pane, v)) = picked
        && let Err(e) = app.session.execute("view.setPaneView", json!({"pane": pane, "view": v.id(), "comp": cid.0}))
    {
        app.ui.status = e.to_string();
    }
    main
}

/// The view selector in the top-left corner of pane `pane` (1 to 3) of a multi-view layout, a
/// popup of every 3D view like the toolbar's 3D View menu. Returns the view chosen this frame.
fn pane_selector(app: &mut EffectcraftApp, ui: &mut egui::Ui, pane: usize, area: Rect, current: View3D) -> Option<View3D> {
    let t = app.tokens;
    let r = Rect::from_min_size(area.min + vec2(6.0, 6.0), vec2(124.0, 20.0));
    let pid = egui::Id::new(("viewer-pane-view", pane));
    let response = widgets::dropdown(ui, r, crate::i18n::tr(current.label()), &t, pid);
    if response.clicked() {
        let open = widgets::popup_is_open(ui, pid);
        ui.data_mut(|d| d.insert_temp(pid.with("open"), !open));
    }
    app.auto.add_response(&format!("viewer.view.{pane}.type"), &response, "View of pane");
    if !widgets::popup_is_open(ui, pid) {
        return None;
    }
    let chosen = widgets::popup_list(ui, pid, r.left_bottom() + vec2(0.0, 2.0), r, View3D::ALL.len(), 0, |ui| {
        ui.set_min_width(124.0);
        let mut chosen = None;
        for (k, v) in View3D::ALL.iter().enumerate() {
            let item = ui.selectable_label(*v == current, crate::i18n::tr(v.label()));
            app.auto.add(&format!("viewer.view.{pane}.typeItem.{k}"), item.rect, v.label());
            if item.clicked() {
                chosen = Some(*v);
            }
        }
        chosen
    });
    chosen.filter(|v| *v != current)
}

/// View ▸ Split with New Locked Viewer: the locked viewer on the left of the Composition panel
/// (its comp and 3D view stay put whatever comp is active). Returns the main viewer's rectangle.
fn locked_pane(app: &mut EffectcraftApp, ui: &mut egui::Ui, full: Rect, bg: Color32) -> Rect {
    let Some(lv) = app.session.state.locked_viewer else { return full };
    let Some(comp) = app.session.project.comp_arc(lv.comp) else { return full };
    let name = app.session.project.item(lv.comp).map(|i| i.name.clone()).unwrap_or_default();
    let gap = 2.0;
    let mid = full.center().x;
    let r = Rect::from_min_max(full.min, pos2(mid - gap / 2.0, full.max.y));
    let main = Rect::from_min_max(pos2(mid + gap / 2.0, full.min.y), full.max);
    let ctx = ui.ctx().clone();
    let p = ui.painter().clone();
    p.rect_filled(Rect::from_min_max(pos2(r.max.x, full.min.y), pos2(main.min.x, full.max.y)), 0.0, Color32::from_black_alpha(200));
    p.rect_filled(r, 0.0, bg);
    let (cw, ch) = (comp.width as f32, comp.height as f32);
    let fit = ((r.width() - 20.0) / cw).min((r.height() - 44.0) / ch).max(0.01);
    let cr = Rect::from_center_size(r.center(), vec2(cw * fit, ch * fit));
    let scale = (fit * ctx.pixels_per_point()).min(1.0) as f64;
    let t = app.session.time_of(lv.comp);
    let cam = (lv.view != View3D::ActiveCamera && comp.has_3d())
        .then(|| app.session.state.views3d.get(&lv.comp).cloned().unwrap_or_default().cam(lv.view, comp.width as f64, comp.height as f64).state());
    let dc = effectcraft_engine::viewer::DisplayColor::of(&app.session);
    // The display conversion's inputs, keyed as in the main viewer's texture caches.
    let dc_key = format!("{cam:?}{}", super::viewer_tools::display_key(app, dc.is_some()));
    let key = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (app.session.revision, lv.comp.0, t.0, scale.to_bits(), dc_key).hash(&mut h);
        h.finish()
    };
    let id = egui::Id::new("viewer-locked");
    let cached: Option<(u64, egui::TextureHandle)> = ctx.data(|d| d.get_temp(id));
    let tex = match cached {
        Some((k, tex)) if k == key => tex,
        _ => {
            let opts = effectcraft_engine::render::RenderOpts { scale, view: cam, ..Default::default() };
            let img = app.session.render(lv.comp, t, opts);
            let mut ci = crate::frames::to_color_image(&img);
            if let Some(dc) = dc {
                let mut px: Vec<[u8; 4]> = ci.pixels.iter().map(|c| c.to_array()).collect();
                dc.apply(&mut px);
                ci = egui::ColorImage::new(ci.size, px.into_iter().map(|a| Color32::from_rgba_premultiplied(a[0], a[1], a[2], a[3])).collect());
            }
            let tex = crate::frames::load_fitted(&ctx, "viewer-locked", ci, egui::TextureOptions::LINEAR);
            ctx.data_mut(|d| d.insert_temp(id, (key, tex.clone())));
            tex
        }
    };
    let b = comp.background;
    p.rect_filled(cr, 0.0, Color32::from_rgb((b[0] * 255.0) as u8, (b[1] * 255.0) as u8, (b[2] * 255.0) as u8));
    p.image(tex.id(), cr, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    p.rect_stroke(cr, 0.0, Stroke::new(1.0, Color32::from_black_alpha(160)), StrokeKind::Outside);
    let label = crate::i18n::tr_args("Locked: {} — {}", &[&name, &(if comp.has_3d() { lv.view.label() } else { crate::i18n::tr("Active Camera") })]);
    p.text(r.left_top() + vec2(8.0, 8.0), Align2::LEFT_TOP, &label, Tokens::ui(11.0), Color32::from_white_alpha(210));
    app.auto.add("viewer.locked", r, &label);
    // Close the locked viewer.
    let close = Rect::from_min_size(pos2(r.max.x - 26.0, r.min.y + 4.0), vec2(20.0, 20.0));
    let resp = ui.interact(close, egui::Id::new("viewer-locked-close"), Sense::click());
    p.text(close.center(), Align2::CENTER_CENTER, "\u{00d7}", Tokens::ui(14.0), if resp.hovered() { Color32::WHITE } else { Color32::from_white_alpha(170) });
    app.auto.add("viewer.locked.close", close, "Close Locked Viewer");
    if resp.clicked() {
        let _ = app.session.execute("view.closeLockedViewer", serde_json::json!({}));
    }
    main
}

pub(crate) fn checker(p: &egui::Painter, r: Rect) {
    let s = 10.0;
    p.rect_filled(r, 0.0, Color32::from_gray(0xcc));
    let mut y = r.min.y;
    let mut row = 0;
    while y < r.max.y {
        let mut x = r.min.x + if row % 2 == 0 { 0.0 } else { s };
        while x < r.max.x {
            p.rect_filled(Rect::from_min_max(pos2(x, y), pos2((x + s).min(r.max.x), (y + s).min(r.max.y))), 0.0, Color32::from_gray(0x99));
            x += 2.0 * s;
        }
        y += s;
        row += 1;
    }
}

pub fn show(app: &mut EffectcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let ctx = ui.ctx().clone();
    let Some(cid) = app.session.active_comp_id() else {
        empty_state(app, ui, rect);
        return;
    };
    let comp = app.session.project.comp_arc(cid).unwrap_or_else(|| effectcraft_engine::project::Comp::new(1920, 1080, Default::default(), Tick::ZERO).into());
    let comp_name = app.session.project.item(cid).map(|i| i.name.clone()).unwrap_or_default();
    let p = ui.painter().clone();
    VIEW_CAM.with(|c| c.set(app.session.view_camera(cid)));

    // Navigator bar (open comps as breadcrumbs). After Effects shows a single breadcrumb row:
    // when the Composition Navigator (nested comp flow) is above the viewer, this row is left
    // out.
    let nav_h = if super::precomp::has_flow(app) { 0.0 } else { 24.0 };
    let nav = Rect::from_min_size(rect.min, vec2(rect.width(), nav_h));
    p.rect_filled(nav, 0.0, t.panel_bg);
    let mut x = nav.min.x + 10.0;
    let open = if nav_h > 0.0 { app.session.state.open_comps.clone() } else { vec![] };
    for oc in open {
        let name = app.session.project.item(oc).map(|i| i.name.clone()).unwrap_or_default();
        let g = p.layout_no_wrap(name.clone(), Tokens::ui(11.5), t.text);
        let r = Rect::from_min_size(pos2(x, nav.min.y + 3.0), vec2(g.size().x + 16.0, 18.0));
        let resp = ui.interact(r, egui::Id::new(("nav", oc.0)), Sense::click());
        let active = oc == cid;
        p.rect_filled(
            r,
            9.0,
            if active {
                t.row_selected
            } else if resp.hovered() {
                t.hover
            } else {
                Color32::TRANSPARENT
            },
        );
        p.galley_with_override_text_color(pos2(r.min.x + 8.0, r.center().y - g.size().y / 2.0), g, if active { t.tab_text_active } else { t.text_dim });
        app.auto.add(&format!("viewer.nav.{}", oc.0), r, &name);
        if resp.clicked() {
            app.session.open_comp(oc);
        }
        x = r.max.x + 4.0;
    }

    // Bottom control bar.
    let bar_h = 36.0;
    let bar = Rect::from_min_max(pos2(rect.min.x, rect.max.y - bar_h), rect.max);
    // The expression error bar sits above the control bar while expressions fail.
    let err_h = super::expr_bar::height(app, &ctx, cid);
    let err_bar = Rect::from_min_max(pos2(rect.min.x, bar.min.y - err_h), pos2(rect.max.x, bar.min.y));
    let full = Rect::from_min_max(pos2(rect.min.x, nav.max.y), pos2(rect.max.x, err_bar.min.y));
    let pasteboard = app.ui.viewer.pasteboard.map(|[r, g, b]| Color32::from_rgb(r, g, b)).unwrap_or(t.pasteboard);
    // View ▸ Switch View Layout: extra views (Top / Front / Right) beside the main view, which
    // keeps the overlays and the interaction.
    let full = locked_pane(app, ui, full, pasteboard);
    let area = aux_views(app, ui, &comp, cid, full, pasteboard);
    p.rect_filled(area, 0.0, pasteboard);
    // View ▸ Show Rulers takes a strip on the top and left.
    let outer = area;
    let area = vt::inset(app, area);

    let (cw, ch) = (comp.width as f32, comp.height as f32);
    let fit = ((area.width() - 40.0) / cw).min((area.height() - 40.0) / ch).max(0.01);
    // Fit up to 100% follows the panel's size like Fit does, and stops at 100%.
    let fit = if app.ui.viewer.fit_up_to_100 { fit.min(1.0 / ctx.pixels_per_point()) } else { fit };
    ctx.data_mut(|d| d.insert_temp(egui::Id::new("viewer-fit"), fit));
    let zoom = app.ui.viewer.zoom.unwrap_or(fit);
    let center = area.center() + vec2(app.ui.viewer.pan[0], app.ui.viewer.pan[1]);
    // On the physical pixel grid, so at 100 % every comp pixel covers exactly one screen pixel
    // instead of being blended with its neighbour (#882).
    let ppp = ctx.pixels_per_point();
    let origin = snap_to_pixels(center - vec2(cw * zoom / 2.0, ch * zoom / 2.0), ppp);
    let map = ViewerMap { origin, zoom, comp: [cw, ch], area };
    ctx.data_mut(|d| d.insert_temp(map_id(), map));
    let comp_rect = Rect::from_min_size(origin, vec2(cw * zoom, ch * zoom));
    let painter = p.with_clip_rect(area);
    // Settings ▸ 3D ▸ Extended Viewer: custom 3D views (and Draft 3D) render the visible
    // pasteboard too, so 3D layers reaching past the comp frame stay visible there.
    app.ui.viewer.extended = extended_region(app, &comp, cid, map);

    // Frame.
    let scale = app.viewer_scale(zoom, ppp);
    if !app.playback.playing {
        // Queue the frame on screen before any prefetch.
        app.request_frame_urgent(cid, comp.frame_rate.frame_at(app.session.time()), scale);
    }
    crate::tick_playback(app, &ctx, scale);
    let time = app.session.time();
    let frame = comp.frame_rate.frame_at(time);
    let key = app.frame_key(cid, frame, scale);
    app.request_frame_urgent(cid, frame, scale);
    // The frame on screen. While a drag or a property scrub is rendering, the exact key changes
    // with every edit and its render is still in flight; the newest completed revision of the
    // same frame keeps the picture following the pointer instead of freezing until it stops.
    if let Some((k, img)) = app.frames.get_for_viewer(&key, app.viewer_shown.as_ref().map(|(_, s)| s)) {
        let stale = app.viewer_shown.as_ref().is_none_or(|(_, s)| *s != k);
        // A CPU frame goes up again when the magnification calls for another factor.
        let minify = minify_factor(app.session.prefs.viewer_zoom_smooth(), scale, (zoom * ppp) as f64);
        let refit = !app.viewer_on_gpu() && app.viewer_tex.as_ref().is_some_and(|(_, _, m)| *m != minify);
        if stale || refit {
            show_frame(app, &ctx, k, img, minify);
        } else if let Some((_, s)) = app.viewer_shown.as_mut() {
            // The same frame at a later revision (an edit elsewhere, an undo): it shows that one.
            s.revision = k.revision;
        }
    }
    if app.ui.viewer.transparency_grid {
        checker(&painter, comp_rect);
    } else {
        let bg = comp.background;
        painter.rect_filled(comp_rect, 0.0, Color32::from_rgb((bg[0] * 255.0) as u8, (bg[1] * 255.0) as u8, (bg[2] * 255.0) as u8));
    }
    let snap_project = app.session.project.clone();
    let snap_expr = app.session.expr.clone();
    let ectx = EvalCtx { project: &snap_project, comp_id: cid, comp: &comp, time, expr: snap_expr.as_deref(), footage: None };
    // The frame (or snapshot) through Show Channel and exposure; ROI frames cover the region.
    vt::draw_frame(app, &ctx, &painter, comp_rect, cid, &ectx);
    if app.ui.viewer.extended.is_some() {
        // The comp frame outlined over the extended render.
        painter.rect_stroke(comp_rect, 0.0, Stroke::new(1.0, Color32::from_gray(150)), StrokeKind::Outside);
        app.auto.add("viewer.extendedArea", rect_of(&map, app.ui.viewer.extended), "Extended Viewer area");
    } else {
        painter.rect_stroke(comp_rect, 0.0, Stroke::new(1.0, Color32::from_black_alpha(160)), StrokeKind::Outside);
    }
    app.auto.add("viewer.comp", comp_rect, &comp_name);
    app.auto.add("viewer.area", area, "Composition viewer");

    // Guides.
    let overlays = app.overlays_visible();
    if overlays && app.ui.viewer.safe_margins {
        let g = &app.session.prefs.grids;
        let (act, title) = (1.0 - g.action_safe as f32 / 100.0, 1.0 - g.title_safe as f32 / 100.0);
        for (k, a) in [(act, 120u8), (title, 160)] {
            let r = Rect::from_center_size(comp_rect.center(), comp_rect.size() * k);
            painter.rect_stroke(r, 0.0, Stroke::new(1.0, Color32::from_white_alpha(a)), StrokeKind::Middle);
        }
        let c = comp_rect.center();
        painter.line_segment([c - vec2(10.0, 0.0), c + vec2(10.0, 0.0)], Stroke::new(1.0, Color32::from_white_alpha(140)));
        painter.line_segment([c - vec2(0.0, 10.0), c + vec2(0.0, 10.0)], Stroke::new(1.0, Color32::from_white_alpha(140)));
    }
    if overlays && app.ui.viewer.grid {
        // Settings ▸ Grids & Guides: gridline spacing, subdivisions and colour.
        let g = &app.session.prefs.grids;
        let [r, gg, b] = hex_rgb(&g.grid_color).unwrap_or([120, 160, 255]);
        let major = Stroke::new(1.0, Color32::from_rgba_unmultiplied(r, gg, b, 90));
        let minor = Stroke::new(1.0, Color32::from_rgba_unmultiplied(r, gg, b, 35));
        let step = (g.grid_spacing as f32 * zoom).max(2.0);
        let sub = g.grid_subdivisions.max(1) as f32;
        let sstep = step / sub;
        let draw_minor = sstep >= 4.0;
        // Settings ▸ Grids & Guides ▸ Grid Style (lines, dashed lines or dots).
        let style = g.grid_style.clone();
        let mut i = 0u32;
        let mut gx = comp_rect.min.x;
        while gx <= comp_rect.max.x {
            let is_major = i.is_multiple_of(g.grid_subdivisions.max(1));
            if is_major || draw_minor {
                styled_line(&painter, [pos2(gx, comp_rect.min.y), pos2(gx, comp_rect.max.y)], if is_major { major } else { minor }, &style);
            }
            gx += sstep;
            i += 1;
        }
        let mut i = 0u32;
        let mut gy = comp_rect.min.y;
        while gy <= comp_rect.max.y {
            let is_major = i.is_multiple_of(g.grid_subdivisions.max(1));
            if is_major || draw_minor {
                styled_line(&painter, [pos2(comp_rect.min.x, gy), pos2(comp_rect.max.x, gy)], if is_major { major } else { minor }, &style);
            }
            gy += sstep;
            i += 1;
        }
    }

    vt::proportional_grid(app, &painter, comp_rect);

    // Comp guides (View ▸ Show Guides) and the region of interest.
    if overlays && app.ui.viewer.guides {
        let [r, gg, b] = hex_rgb(&app.session.prefs.grids.guide_color).unwrap_or([0x3c, 0xc8, 0xf0]);
        let stroke = Stroke::new(1.0, Color32::from_rgb(r, gg, b));
        // Settings ▸ Grids & Guides ▸ Guide Style.
        let style = app.session.prefs.grids.guide_style.clone();
        for g in &comp.guides {
            if g.vertical {
                let x = comp_rect.min.x + g.px(comp.width, comp.height) as f32 * zoom;
                styled_line(&painter, [pos2(x, area.min.y), pos2(x, area.max.y)], stroke, &style);
            } else {
                let y = comp_rect.min.y + g.px(comp.width, comp.height) as f32 * zoom;
                styled_line(&painter, [pos2(area.min.x, y), pos2(area.max.x, y)], stroke, &style);
            }
        }
    }
    if let Some([rx, ry, rw, rh]) = app.session.state.region_of_interest {
        let r = Rect::from_min_size(comp_rect.min + vec2(rx as f32 * zoom, ry as f32 * zoom), vec2(rw as f32 * zoom, rh as f32 * zoom));
        painter.rect_stroke(r, 0.0, Stroke::new(1.0, Color32::WHITE), StrokeKind::Middle);
        // Resize handles: corners and edge midpoints (drag with any tool).
        for (i, c) in roi_handles(r).iter().enumerate() {
            painter.rect_filled(Rect::from_center_size(*c, vec2(5.0, 5.0)), 0.0, Color32::WHITE);
            app.auto.add(&format!("viewer.regionOfInterest.handle.{i}"), Rect::from_center_size(*c, vec2(8.0, 8.0)), "Resize the region of interest");
        }
        app.auto.add("viewer.regionOfInterest", r, "Region of interest");
    }
    // Overlays + interaction.
    let selected = app.session.state.selected_layers.clone();
    // Bounding-box handles: (layer, handle, screen point, its resize cursor).
    let mut handle_hits: Vec<(LayerId, usize, Pos2, egui::CursorIcon)> = Vec::new();
    let mut vertex_hits: Vec<VertexHit> = Vec::new();
    let mut vertex_marks: Vec<ov::VertexMark> = Vec::new();
    let mut key_hits: Vec<ov::KeyHit> = Vec::new();
    let mut motion_frames: Vec<ov::PathFrame> = Vec::new();
    let mut paths: Vec<ov::PathInfo> = Vec::new();
    let sel_vertices = app.session.state.selected_vertices.clone();
    // Pen path in progress ends when the tool changes or its mask is gone; Esc / Enter also end
    // free transform.
    let finish = ui.input(|i| i.key_pressed(egui::Key::Escape) || i.key_pressed(egui::Key::Enter)) && !ctx.egui_wants_keyboard_input();
    if app.ui.tool != Tool::Pen || finish {
        ui.data_mut(|d| d.remove::<(LayerId, u64)>(pen_id()));
    }
    if finish || app.ui.tool != Tool::Selection {
        ov::end_free_transform(&ctx);
    }
    if app.layer_controls_visible() {
        // Settings ▸ General ▸ Path Point and Handle Size; Appearance ▸ Use Label Color for
        // Layer Handles and Paths.
        let hs = app.session.prefs.general.path_point_size as f32 + 2.0;
        let label_handles = app.session.prefs.appearance.use_label_color_for_handles;
        // A layer whose Video switch is off has no box, handles or paths in the viewer (a hidden
        // null disappears from the comp, as in After Effects), even while it is selected.
        for l in comp.layers.iter().filter(|l| selected.contains(&l.id) && l.is_active_at(time) && l.switches.video) {
            let col = if label_handles { t.label(l.label) } else { t.accent };
            // Motion path of animated position (keyframes and spatial tangents are draggable).
            ov::motion_path(app, &painter, &map, &ectx, l, col, &mut key_hits, &mut motion_frames);
            // Masks and shape paths.
            if app.ui.viewer.show_masks {
                let pen = ui.data(|d| d.get_temp::<(LayerId, u64)>(pen_id()));
                let hover = ui.input(|i| i.pointer.hover_pos());
                ov::draw_paths(app, &painter, &map, &ectx, l, col, &sel_vertices, pen, hover, &mut vertex_hits, &mut vertex_marks, &mut paths);
            }
            // Settings ▸ Previews ▸ Show Internal Wireframes: outlines of the layers inside a
            // selected collapsed precomp.
            if app.session.prefs.previews.show_internal_wireframes
                && l.switches.collapse
                && let effectcraft_engine::project::LayerSource::Comp { item } = &l.source
                && let Some(nc) = app.session.project.comp(*item)
            {
                let (outer, _) = l2c(&ectx, l);
                let nctx = EvalCtx::new(&app.session.project, *item, nc, ectx.nested_time(l));
                for nl in nc.layers.iter().filter(|nl| nl.is_active_at(nctx.time) && !nl.is_3d()) {
                    if let Some((_, q, _)) = layer_quad(&nctx, nl) {
                        let sq: Vec<Pos2> = q
                            .iter()
                            .map(|p| {
                                let c = outer.apply(gv2(p[0], p[1]));
                                map.to_screen([c.x, c.y])
                            })
                            .collect();
                        painter.add(egui::Shape::closed_line(sq, Stroke::new(1.0, col.gamma_multiply(0.6))));
                    }
                }
            }
            let Some((m, q, _)) = layer_quad(&ectx, l) else {
                // A 3D layer reaching past the camera: the part of its box in front of it.
                let outline = layer_outline(&ectx, l);
                if outline.len() > 2 {
                    painter.add(egui::Shape::closed_line(outline.iter().map(|p| map.to_screen(*p)).collect(), Stroke::new(1.0, col)));
                    continue;
                }
                // Cameras, lights, empty layers: just the anchor.
                if let Some(tr) = l.transform() {
                    let pos = ectx.v3(l, tr, "position", [0.0; 3]);
                    let s = map.to_screen([pos[0], pos[1]]);
                    painter.circle_stroke(s, 6.0, Stroke::new(1.0, col));
                }
                continue;
            };
            let sq: Vec<Pos2> = q.iter().map(|p| map.to_screen(*p)).collect();
            painter.add(egui::Shape::closed_line(sq.clone(), Stroke::new(1.0, col)));
            for i in 0..8 {
                let hp = if i < 4 { sq[i] } else { sq[i - 4] + (sq[(i - 3) % 4] - sq[i - 4]) * 0.5 };
                let hr = Rect::from_center_size(hp, vec2(hs, hs));
                painter.rect_filled(hr, 0.0, col);
                painter.rect_stroke(hr, 0.0, Stroke::new(1.0, Color32::from_black_alpha(120)), StrokeKind::Outside);
                handle_hits.push((l.id, i, hp, handle_cursor(&sq, i)));
                app.auto.add(&format!("viewer.handle.{}.{i}", l.id.0), hr, "handle");
            }
            // Anchor point.
            if let Some(tr) = l.transform() {
                let a = ectx.v3(l, tr, "anchor", [0.0; 3]);
                let c = m.apply(gv2(a[0], a[1]));
                let s = map.to_screen([c.x, c.y]);
                painter.circle_stroke(s, 5.0, Stroke::new(1.0, Color32::WHITE));
                painter.line_segment([s - vec2(9.0, 0.0), s + vec2(9.0, 0.0)], Stroke::new(1.0, Color32::WHITE));
                painter.line_segment([s - vec2(0.0, 9.0), s + vec2(0.0, 9.0)], Stroke::new(1.0, Color32::WHITE));
                app.auto.add(&format!("viewer.anchor.{}", l.id.0), Rect::from_center_size(s, vec2(12.0, 12.0)), "anchor point");
                // 3D axis gizmo.
                if l.is_3d() {
                    let w = ectx.world_matrix(l);
                    let pc = cam_state(&ectx).projection(cw as f64, ch as f64) * w;
                    let a3 = effectcraft_engine::geom::vec3(a[0], a[1], a[2]);
                    let len = 60.0 / zoom as f64;
                    for (axis, col) in [
                        (effectcraft_engine::geom::vec3(len, 0.0, 0.0), Color32::from_rgb(0xe0, 0x50, 0x50)),
                        (effectcraft_engine::geom::vec3(0.0, len, 0.0), Color32::from_rgb(0x60, 0xd0, 0x60)),
                        (effectcraft_engine::geom::vec3(0.0, 0.0, len), Color32::from_rgb(0x50, 0x8c, 0xf0)),
                    ] {
                        // (clipped at the camera's near plane, #572)
                        if let Some([o, e]) = project_segment(&pc, a3, a3 + axis) {
                            painter.arrow(map.to_screen(o), map.to_screen(e) - map.to_screen(o), Stroke::new(2.0, col));
                        }
                    }
                }
            }
        }
    }

    // Path points last, over the boxes' handles (#1042).
    ov::draw_vertex_marks(&painter, &vertex_marks, app.session.prefs.general.path_point_size as f32 + 1.0);

    // Settings ▸ 3D: Extended Viewer (3D layers outlined beyond the comp frame) and Show 3D
    // Reference Axes (world X/Y/Z in the corner, turning with the view).
    let has_3d = comp.layers.iter().any(|l| l.is_3d() && l.is_active_at(time));
    if has_3d && app.session.prefs.three_d.extended_viewer {
        for l in comp.layers.iter().filter(|l| l.is_3d() && l.is_active_at(time) && l.has_video()) {
            let q = layer_outline(&ectx, l);
            if q.len() > 2 {
                let sq: Vec<Pos2> = q.iter().map(|p| map.to_screen(*p)).collect();
                let outside = sq.iter().any(|p| !comp_rect.contains(*p));
                if outside {
                    painter.add(egui::Shape::closed_line(sq, Stroke::new(1.0, t.label(l.label).gamma_multiply(0.55))));
                }
            }
        }
    }
    if has_3d && app.session.prefs.three_d.show_reference_axes {
        let pc = cam_state(&ectx).projection(cw as f64, ch as f64);
        let o3 = effectcraft_engine::geom::vec3(cw as f64 / 2.0, ch as f64 / 2.0, 0.0);
        let o = pc.apply(o3);
        let corner = pos2(area.min.x + 34.0, area.max.y - 34.0);
        for (axis, col, name) in [
            (effectcraft_engine::geom::vec3(1.0, 0.0, 0.0), Color32::from_rgb(0xe0, 0x50, 0x50), "X"),
            (effectcraft_engine::geom::vec3(0.0, 1.0, 0.0), Color32::from_rgb(0x60, 0xd0, 0x60), "Y"),
            (effectcraft_engine::geom::vec3(0.0, 0.0, 1.0), Color32::from_rgb(0x50, 0x8c, 0xf0), "Z"),
        ] {
            let e = pc.apply(o3 + axis * 10.0);
            let d = vec2((e.x - o.x) as f32, (e.y - o.y) as f32);
            let d = if d.length() > 1e-4 { d.normalized() * 22.0 } else { vec2(0.0, 0.0) };
            painter.arrow(corner, d, Stroke::new(1.5, col));
            painter.text(corner + d * 1.25, Align2::CENTER_CENTER, name, Tokens::ui(10.0), col);
        }
        app.auto.add("viewer.referenceAxes", Rect::from_center_size(corner, vec2(56.0, 56.0)), "3D reference axes");
    }

    // Track points of the current track (Tracker panel) on its layer, while that layer is selected.
    let mut track_hits = vec![];
    let track_cur = super::tracker::viewer_track(app);
    if app.layer_controls_visible()
        && let Some((tl, tu)) = track_cur
        && selected.contains(&tl)
        && let Some(layer) = comp.layer(tl)
        && layer.is_active_at(time)
    {
        let (m, _) = l2c(&ectx, layer);
        track_hits = super::tracker::draw_overlay(app, &painter, &map, &m, layer, tu, time);
    }

    // Warp Stabilizer banner over the layer being analysed ("Analyzing in background (step 1
    // of 2)…" / "Stabilizing…").
    if let Some((wc, wl, _)) = app.session.warp_target()
        && wc == cid
        && let Some(pr) = app.session.warp_progress()
        && let Some(layer) = comp.layer(wl)
        && layer.is_active_at(time)
    {
        let (m, _) = l2c(&ectx, layer);
        let (w, h) = effectcraft_engine::render::source_size(&app.session.project, layer);
        let c = m.apply(effectcraft_engine::geom::vec2(w as f64 / 2.0, h as f64 / 2.0));
        let at = map.to_screen([c.x, c.y]);
        let text = pr.banner();
        let galley = painter.layout_no_wrap(text.clone(), Tokens::medium(13.0), Color32::WHITE);
        let r = Rect::from_center_size(at, galley.size() + vec2(28.0, 14.0)).intersect(map.area);
        painter.rect_filled(r, 3.0, Color32::from_rgba_unmultiplied(0x1d, 0x4f, 0x9c, 0xe0));
        painter.galley(pos2(r.center().x - galley.size().x / 2.0, r.center().y - galley.size().y / 2.0), galley, Color32::WHITE);
        app.auto.add("viewer.warpBanner", r, &text);
    }

    // Footage that can't be decoded (no decoder for its codec / profile): say why on the layer
    // instead of leaving a black picture.
    let mut notices = 0;
    for layer in comp.layers.iter().filter(|l| l.is_active_at(time) && l.switches.video) {
        let effectcraft_engine::project::LayerSource::Footage { item } = &layer.source else { continue };
        let Some(effectcraft_engine::project::ItemKind::Footage(f)) = app.session.project.item(*item).map(|i| &i.kind) else { continue };
        let Some(why) = app.session.footage.problem(f) else { continue };
        if notices >= 3 {
            break;
        }
        let (m, _) = l2c(&ectx, layer);
        let (w, h) = effectcraft_engine::render::source_size(&app.session.project, layer);
        let c = m.apply(effectcraft_engine::geom::vec2(w as f64 / 2.0, h as f64 / 2.0));
        let at = map.to_screen([c.x, c.y]);
        let text = format!("{}: {}", layer.name, why);
        let galley = painter.layout(text.clone(), Tokens::medium(12.0), Color32::WHITE, (map.area.width() - 40.0).clamp(80.0, 420.0));
        let r = Rect::from_center_size(at + vec2(0.0, notices as f32 * 4.0), galley.size() + vec2(24.0, 16.0)).intersect(map.area);
        painter.rect_filled(r, 3.0, Color32::from_rgba_unmultiplied(0x4a, 0x1a, 0x14, 0xe6));
        painter.rect_stroke(r, 3.0, Stroke::new(1.0, t.warning), StrokeKind::Inside);
        painter.galley(pos2(r.min.x + 12.0, r.center().y - galley.size().y / 2.0), galley, Color32::WHITE);
        app.auto.add(&format!("viewer.footageProblem.{}", layer.id.0), r, &text);
        notices += 1;
    }

    // Free transform box of a path (double-click a path with the Selection tool).
    let ft = ov::draw_free_transform(app, &ctx, &painter, &map, &paths);

    // Cameras and lights (wireframes) and the 3D view name.
    if comp.has_3d() {
        let through_active = VIEW_CAM.with(|c| c.get()).is_none();
        draw_rigs(app, &painter, &map, &ectx, &selected, cam_state(&ectx), through_active, None);
        let view = app.session.state.views3d.get(&cid).map(|v| v.current).unwrap_or_default();
        let label = match (view, comp.active_camera(time)) {
            (View3D::ActiveCamera, Some(c)) => format!("Active Camera ({})", c.name),
            (v, _) => v.label().to_string(),
        };
        painter.text(area.left_top() + vec2(12.0, 10.0), Align2::LEFT_TOP, label, Tokens::ui(12.0), t.text_dim);
    }

    // Puppet pins and meshes of the selected layers (Puppet tools).
    let mut pin_hits: Vec<super::puppet_tool::PinHit> = vec![];
    // The selected layers with a Puppet effect, top of the stack first (the layers the Puppet
    // tools work on).
    let mut puppet_layers: Vec<(&Layer, super::puppet_tool::Overlay)> = vec![];
    if app.ui.tool.puppet_kind().is_some() {
        let sel_props: Vec<u64> = app.session.state.selected_props.iter().map(|(_, u)| *u).collect();
        for lid in &selected {
            if let Some(l) = comp.layer(*lid)
                && let Some(ov) = super::puppet_tool::overlay(app, &ctx, cid, l, time)
            {
                let hover = ui.input(|i| i.pointer.hover_pos());
                pin_hits.extend(super::puppet_tool::draw(&painter, &map, &ectx, l, &ov, app.session.state.puppet.show_mesh, &sel_props, hover));
                puppet_layers.push((l, ov));
            }
        }
        puppet_layers.sort_by_key(|(l, _)| comp.index_of(l.id));
        for h in &pin_hits {
            app.auto.add(&format!("viewer.puppetPin.{}", h.pin), Rect::from_center_size(h.pos, vec2(10.0, 10.0)), "Puppet pin");
            if let Some(hp) = h.handle {
                let ring = h.pos + (h.pos - hp);
                app.auto.add(&format!("viewer.puppetPin.{}.rotate", h.pin), Rect::from_center_size(ring, vec2(6.0, 6.0)), "Rotate puppet pin");
                app.auto.add(&format!("viewer.puppetPin.{}.scale", h.pin), Rect::from_center_size(hp, vec2(6.0, 6.0)), "Scale puppet pin");
            }
        }
    }
    // The layer a Puppet tool press works on: while selected layers are rigged, only those
    // ([`super::puppet_tool::target`]); otherwise the layer under the pointer, selected.
    let puppet_layer = |app: &mut EffectcraftApp, cpt: [f64; 2]| -> Option<LayerId> {
        if puppet_layers.is_empty() {
            return pick(app, &ectx, cpt, false);
        }
        super::puppet_tool::target(&ectx, &puppet_layers, cpt, |l| layers_at(&ectx, cpt).any(|h| h.id == l.id))
    };

    // Project items, files and effects dropped on the viewer.
    super::viewer_drop::show(app, ui, &painter, &map, &ectx);

    // Interaction.
    let resp = ui.interact(area, egui::Id::new("viewer-interact"), Sense::click_and_drag());
    let menu_hits_id = egui::Id::new(("viewer-menu-hits", cid.0));
    if resp.secondary_clicked()
        && let Some(pos) = resp.interact_pointer_pos()
    {
        // Picking an already selected layer preserves the group; an unselected hit
        // becomes the target, using the same visible/unlocked hit test as left-click.
        pick(app, &ectx, map.to_comp(pos), false);
        let point = map.to_comp(pos);
        let hits: Vec<(u64, String)> = layers_at(&ectx, point).map(|l| (l.id.0, l.name.clone())).collect();
        ui.data_mut(|d| d.insert_temp(menu_hits_id, hits));
    }
    let context_layers: Vec<u64> =
        app.session.state.selected_layers.iter().filter(|id| comp.layer(**id).is_some_and(|l| !l.switches.locked)).map(|id| id.0).collect();
    let mut context_actions = Vec::new();
    let copy_frame = crate::i18n::label(app, "comp.copyFrame", "Copy Frame to Clipboard");
    crate::widgets::context_menu(&resp, |ui| {
        let hits = ui.data(|d| d.get_temp::<Vec<(u64, String)>>(menu_hits_id)).unwrap_or_default();
        if !hits.is_empty() {
            ui.menu_button(crate::i18n::tr("Select"), |ui| {
                crate::widgets::menu_scroll(ui, |ui| {
                    for (id, name) in &hits {
                        if ui.button(name).clicked() {
                            context_actions.push(("layer.select".to_owned(), serde_json::json!({"layers":[id]})));
                            ui.close();
                        }
                    }
                })
            });
            ui.separator();
        }
        if context_layers.is_empty() {
            ui.label(crate::i18n::tr("No unlocked layer selected"));
        } else {
            let caps = context_layers.iter().filter_map(|id| comp.layer(effectcraft_engine::project::LayerId(*id))).fold(
                super::timeline::LayerMenuCaps::default(),
                |mut caps, layer| {
                    let c = super::timeline::LayerMenuCaps::of(&app.session.project, layer);
                    caps.vector |= c.vector;
                    caps.text |= c.text;
                    caps
                },
            );
            super::timeline::layer_menu(ui, &context_layers, caps, &mut context_actions);
        }
        ui.separator();
        if ui.button(copy_frame).clicked() {
            context_actions.push(("comp.copyFrame".to_owned(), serde_json::json!({"comp": cid.0})));
            ui.close();
        }
        // The comp this viewer shows, which is not always the active one.
        let settings = ui.button(crate::i18n::tr("Composition Settings…"));
        app.auto.add("viewer.context.settings", settings.rect, "Composition Settings…");
        if settings.clicked() {
            context_actions.push(("comp.settings".to_owned(), serde_json::json!({"comp": cid.0})));
            ui.close();
        }
    });
    for (id, params) in context_actions {
        if let Err(e) = crate::menus::invoke(app, &ctx, &id, params) {
            app.ui.status = e;
        }
    }
    // Path vertices and tangent handles take the press before the bounding-box handles (#1042).
    let vertex_at = |pos: Pos2, tangents: bool| -> Option<VertexHit> {
        let reach = vertex_grab_radius(pos, handle_hits.iter().map(|(_, _, h, _)| *h));
        vertex_hits.iter().rev().filter(|h| h.tangent.is_some() == tangents).find(|h| h.pos.distance(pos) < reach).copied()
    };
    let gid = egui::Id::new("viewer-gesture");
    // An overlay on top of the viewer (e.g. the camera tracker points) takes the hover, but the pointer is still over the viewer: keep wheel zoom working.
    let hover_pos = resp.hover_pos().or_else(|| if resp.contains_pointer() { ui.input(|i| i.pointer.hover_pos()) } else { None });
    if let Some(hp) = hover_pos {
        app.pointer_comp = Some({
            let c = map.to_comp(hp);
            [c[0] as f32, c[1] as f32]
        });
        let cursor = match app.ui.tool {
            Tool::Hand => egui::CursorIcon::Grab,
            Tool::Zoom => egui::CursorIcon::ZoomIn,
            Tool::Type | Tool::TypeVertical => egui::CursorIcon::Text,
            Tool::Orbit => egui::CursorIcon::Alias,
            Tool::PanCamera => egui::CursorIcon::Move,
            Tool::Dolly => egui::CursorIcon::ResizeVertical,
            t if t.is_pen() => {
                pen_cursor(
                    &ctx,
                    hp,
                    pen_cursor_icon(t, ov::segment_at(&paths, &map, hp, 5.0).is_some() || ov::motion_frame_at(&motion_frames, &key_hits, hp).is_some()),
                );
                egui::CursorIcon::None
            }
            t if t.is_shape() => egui::CursorIcon::Crosshair,
            _ if app.ui.viewer.roi_draw => egui::CursorIcon::Crosshair,
            Tool::ObjectMatte => {
                // The tool draws its own crosshair (and the layer it would matte) while no box
                // is being dragged.
                if ui.data(|d| d.get_temp::<Gesture>(egui::Id::new("viewer-gesture"))).is_none() {
                    super::object_matte_tool::hover(app, &painter, &map, &ectx, hp);
                }
                egui::CursorIcon::None
            }
            t if t.puppet_kind().is_some() => match super::puppet_tool::hit(&pin_hits, hp) {
                Some((_, super::puppet_tool::PinPart::Ring)) => egui::CursorIcon::Alias,
                Some((_, super::puppet_tool::PinPart::Scale)) => egui::CursorIcon::ResizeNwSe,
                Some((h, _)) if h.kind == PinKind::Bend => egui::CursorIcon::PointingHand,
                Some(_) => egui::CursorIcon::Move,
                None => egui::CursorIcon::Crosshair,
            },
            _ => {
                if super::tracker::hit_at(&track_hits, hp).is_some() || key_hits.iter().any(|h| h.pos.distance(hp) < 6.0) {
                    egui::CursorIcon::Move
                } else if let Some((_, vertical)) = vt::guide_at(app, &comp, &map, hp) {
                    if vertical { egui::CursorIcon::ResizeHorizontal } else { egui::CursorIcon::ResizeVertical }
                } else if vertex_at(hp, true).or_else(|| vertex_at(hp, false)).is_some() {
                    // A path point under a box handle is grabbed first: no resize cursor.
                    egui::CursorIcon::PointingHand
                } else if let Some((.., cursor)) = handle_hits.iter().find(|(_, _, h, _)| h.distance(hp) < HANDLE) {
                    *cursor
                } else {
                    egui::CursorIcon::Default
                }
            }
        };
        ctx.set_cursor_icon(cursor);
        // Scroll wheel zooms around the pointer.
        let scroll = ui.input(|i| i.smooth_scroll_delta.y + i.zoom_delta().ln() * 300.0);
        if scroll.abs() > 0.1 && area.contains(hp) {
            let k = (scroll / 300.0).exp();
            let nz = (zoom * k).clamp(0.01, 32.0);
            let before = map.to_comp(hp);
            let new_origin = hp - vec2(before[0] as f32 * nz, before[1] as f32 * nz);
            let new_center = new_origin + vec2(cw * nz / 2.0, ch * nz / 2.0);
            let pan = new_center - area.center();
            app.ui.viewer.zoom = Some(nz);
            app.ui.viewer.pan = [pan.x, pan.y];
        }
    } else {
        app.pointer_comp = None;
    }
    app.ui.viewer.interacting = resp.dragged();
    let mods = ui.input(|i| i.modifiers);
    let space_pan = super::space_hand(&ctx);
    // Pen tool: each press places a vertex (dragging pulls Bezier tangents); pressing on the
    // first vertex closes the path, pressing an open path's last vertex resumes it.
    if app.ui.tool == Tool::Pen
        && !space_pan
        && resp.hovered()
        && ui.input(|i| i.pointer.primary_pressed())
        && let Some(pos) = ui.input(|i| i.pointer.press_origin())
    {
        pen_press(app, ui, &ectx, &map, &paths, ov::motion_frame_at(&motion_frames, &key_hits, pos), pos, mods);
    }
    // A pen click (press + release without a drag) ends its tangent gesture on release.
    if ui.input(|i| i.pointer.primary_released())
        && matches!(ui.data(|d| d.get_temp::<Gesture>(egui::Id::new("viewer-gesture"))), Some(Gesture::Pen { .. }))
        && !resp.drag_stopped()
    {
        ui.data_mut(|d| d.remove::<Gesture>(egui::Id::new("viewer-gesture")));
    }
    // On-canvas text editing (Type tool, double-click a text layer) owns its clicks and drags.
    let text_owns = super::viewer_text::hook(app, ui, &painter, &map, &ectx, &resp, space_pan);
    if resp.drag_started()
        && !text_owns
        && let Some(pos) = resp.interact_pointer_pos()
    {
        let cpt = map.to_comp(pos);
        let press = ui.input(|i| i.pointer.press_origin()).unwrap_or(pos);
        let tool = if space_pan || resp.dragged_by(egui::PointerButton::Middle) { Tool::Hand } else { app.ui.tool };
        let g = match tool {
            Tool::Hand => Some(Gesture::Pan { start_pan: app.ui.viewer.pan }),
            // Region of Interest armed: the drag draws it.
            _ if app.ui.viewer.roi_draw => Some(Gesture::Overlay(ov::Drag::Roi { start: map.to_comp(press), keep: [None, None] })),
            // A region of interest handle: resize it from the opposite corner / edge.
            _ if roi_drag(app, &map, comp_rect, zoom, press).is_some() => roi_drag(app, &map, comp_rect, zoom, press).map(Gesture::Overlay),
            Tool::Selection if ft.as_ref().and_then(|f| ov::begin_ft_drag(f, press, &map)).is_some() => {
                ft.as_ref().and_then(|f| ov::begin_ft_drag(f, press, &map)).map(Gesture::Overlay)
            }
            Tool::Selection if key_hits.iter().any(|h| h.pos.distance(press) < 6.0) => {
                ov::begin_key_drag(app, &key_hits, press, map.to_comp(press), mods.shift).map(Gesture::Overlay)
            }
            Tool::Selection if vt::guide_at(app, &comp, &map, press).is_some() => {
                vt::guide_at(app, &comp, &map, press).map(|(index, vertical)| Gesture::Overlay(ov::Drag::Guide { index, vertical }))
            }
            // Alt-drag a vertex with the Selection tool (or drag one with Convert Vertex): pull
            // new tangents out of it.
            Tool::Selection | Tool::PenConvert if (mods.alt || tool == Tool::PenConvert) && vertex_at(press, false).is_some() => {
                vertex_at(press, false).map(|h| {
                    let inv = h.l2c.inverse().unwrap_or(Mat3::IDENTITY);
                    Gesture::Pen { layer: h.layer, mask: h.mask, index: h.index, vertex: h.vertex, inv, press }
                })
            }
            Tool::MaskFeather => ov::begin_feather(app, &ectx, &paths, &map, press).map(Gesture::Overlay),
            Tool::Rotate => pick(app, &ectx, cpt, mods.shift).map(|l| {
                let others = selected_others(app, &comp, l).into_iter().filter_map(|o| Some((o.id, ectx.f(o, o.transform()?, "rotation", 0.0)))).collect();
                let layer = comp.layer(l).cloned();
                let center = layer
                    .as_ref()
                    .and_then(|l| l.transform().map(|tr| ectx.v3(l, tr, "position", [0.0; 3])))
                    .map(|p| map.to_screen([p[0], p[1]]))
                    .unwrap_or(pos);
                let rot = layer.as_ref().and_then(|l| l.transform().map(|tr| ectx.f(l, tr, "rotation", 0.0))).unwrap_or(0.0);
                Gesture::Rotate {
                    layer: l,
                    center,
                    last_angle: ((pos.y - center.y) as f64).atan2((pos.x - center.x) as f64).to_degrees(),
                    turned: 0.0,
                    start_rot: rot,
                    others,
                }
            }),
            Tool::PanBehind => pick(app, &ectx, cpt, false).and_then(|l| {
                let layer = comp.layer(l)?;
                let tr = layer.transform()?;
                let (l2c, _) = l2c(&ectx, layer);
                let l2p = ectx.local_matrix(layer);
                let l2p = Mat3([[l2p.0[0][0], l2p.0[0][1], 0.0], [l2p.0[1][0], l2p.0[1][1], 0.0], [0.0, 0.0, 1.0]]);
                Some(Gesture::Anchor {
                    layer: l,
                    start_anchor: ectx.v3(layer, tr, "anchor", [0.0; 3]),
                    start_pos: ectx.v3(layer, tr, "position", [0.0; 3]),
                    // From where the button went down (the drag starts a few points later).
                    start: map.to_comp(press),
                    inv: l2c.inverse().unwrap_or(Mat3::IDENTITY),
                    l2p,
                    own: effectcraft_engine::viewer::layer_targets(&ectx, layer, false, app.session.state.snap_features),
                    anchor_only: mods.alt,
                })
            }),
            // From where the button went down: by the time the pointer has moved far enough to
            // count as a drag it has already left the press point, and the shape is drawn there.
            t if t.is_shape() => Some(Gesture::Create { tool: t, start: map.to_comp(press) }),
            Tool::ObjectMatte => Some(Gesture::ObjectMatteBox { start: press }),
            t if t.puppet_kind().is_some() && super::puppet_tool::hit(&pin_hits, press).is_none() => {
                // Off the pins: Alt or a press outside the art drags a marquee that selects pins.
                (mods.alt || puppet_layer(app, cpt).is_none()).then_some(Gesture::PinMarquee { start: press })
            }
            t if t.puppet_kind().is_some() => super::puppet_tool::hit(&pin_hits, press).and_then(|(h, part)| {
                use super::puppet_tool::PinPart;
                let angle = (press - h.pos).angle();
                match part {
                    PinPart::Ring => return Some(Gesture::PuppetRotate { layer: h.layer, pin: h.pin, center: h.pos, last: angle, rotation: h.rotation }),
                    PinPart::Scale => {
                        let start = h.pos.distance(press).max(1.0);
                        return Some(Gesture::PuppetScale { layer: h.layer, pin: h.pin, center: h.pos, start, scale: h.scale });
                    }
                    // A Bend pin has no position: it turns and scales by its ring only.
                    PinPart::Center if h.kind == PinKind::Bend => return None,
                    PinPart::Center => {}
                }
                let l = comp.layer(h.layer)?;
                let inv = fx2c(&ectx, l).inverse()?;
                if mods.command {
                    let lp = inv.apply(gv2(cpt[0], cpt[1]));
                    let began = ui.input(|i| i.time);
                    // Dragging one of several selected pins records them all.
                    let sel = &app.session.state.selected_props;
                    let others: Vec<u64> = if sel.contains(&(h.layer, h.pin)) {
                        pin_hits.iter().filter(|o| o.layer == h.layer && o.pin != h.pin && sel.contains(&(o.layer, o.pin))).map(|o| o.pin).collect()
                    } else {
                        vec![]
                    };
                    return Some(Gesture::PuppetRecord { layer: h.layer, pin: h.pin, others, inv, began, cti: time, samples: vec![(0.0, [lp.x, lp.y])] });
                }
                Some(Gesture::PuppetPin { layer: h.layer, pin: h.pin, inv })
            }),
            Tool::Orbit | Tool::PanCamera | Tool::Dolly => Some(Gesture::Camera { tool }),
            Tool::Selection if super::tracker::hit_at(&track_hits, press).is_some() => {
                let hit = super::tracker::hit_at(&track_hits, press);
                track_cur.zip(hit).and_then(|((tl, tu), hit)| {
                    let layer = comp.layer(tl)?;
                    let (m, _) = l2c(&ectx, layer);
                    super::tracker::begin_drag(layer, tu, time, &m, hit, map.to_comp(press)).map(|drag| Gesture::Track { layer: tl, tracker: tu, drag })
                })
            }
            Tool::Selection if vertex_at(press, true).is_some() => vertex_at(press, true).map(|h| Gesture::Tangent {
                layer: h.layer,
                mask: h.mask,
                index: h.index,
                out: h.tangent == Some(true),
                vertex: h.vertex,
                inv: h.l2c.inverse().unwrap_or(Mat3::IDENTITY),
            }),
            Tool::Selection if vertex_at(press, false).is_some() => vertex_at(press, false).map(|h| {
                let me = json!({"layer": h.layer.0, "mask": h.mask, "index": h.index});
                let selected = app.session.state.selected_vertices.iter().any(|v| v.layer == h.layer && v.mask == h.mask && v.index == h.index);
                if !selected {
                    let _ = app.session.execute("mask.selectVertices", json!({"vertices": [me], "add": mods.shift}));
                }
                let src = h.l2c.apply(gv2(h.vertex[0], h.vertex[1]));
                Gesture::Vertices {
                    start: map.to_comp(press),
                    src: [src.x, src.y],
                    applied: [0.0; 2],
                    inv: h.l2c.inverse().unwrap_or(Mat3::IDENTITY),
                    layer: h.layer,
                }
            }),
            // Everything starts where the button went down: by the time the pointer has moved
            // far enough to count as a drag it may have left a small handle.
            Tool::Selection => {
                let cpt = map.to_comp(press);
                if let Some((lid, hi, ..)) = handle_hits.iter().find(|(_, _, h, _)| h.distance(press) < HANDLE + 2.0).cloned() {
                    let layer = comp.layer(lid).cloned();
                    layer.and_then(|layer| {
                        let tr = layer.transform()?;
                        let (l2c, _) = l2c(&ectx, &layer);
                        let inv = l2c.inverse()?;
                        let a = ectx.v3(&layer, tr, "anchor", [0.0; 3]);
                        let lp = inv.apply(gv2(cpt[0], cpt[1]));
                        let hp = handle_point(effectcraft_engine::render::content_bounds(&ectx, &layer)?, hi);
                        Some(Gesture::Scale {
                            layer: lid,
                            start_scale: ectx.v3(&layer, tr, "scale", [100.0; 3]),
                            start_local: [lp.x - a[0], lp.y - a[1]],
                            handle: [hp[0] - a[0], hp[1] - a[1]],
                            anchor: [a[0], a[1]],
                            axes: handle_axes(hi),
                            l2c,
                            inv,
                            others: selected_others(app, &comp, lid)
                                .into_iter()
                                .filter_map(|o| Some((o.id, ectx.v3(o, o.transform()?, "scale", [100.0; 3]))))
                                .collect(),
                        })
                    })
                } else if let Some(l) = pick(app, &ectx, cpt, mods.shift) {
                    let layers: Vec<(LayerId, [f64; 3], [f64; 3], [f64; 3])> = app
                        .session
                        .state
                        .selected_layers
                        .iter()
                        .filter_map(|id| comp.layer(*id))
                        .filter(|l| !l.switches.locked)
                        .filter_map(|l| {
                            let p0 = ectx.v3(l, l.transform()?, "position", [0.0; 3]);
                            let (ax, ay) = drag_axes(&ectx, l);
                            Some((l.id, p0, ax, ay))
                        })
                        .collect();
                    // Snap the grabbed layer's feature nearest the pointer (AE's snap handle).
                    let snap_src = comp.layer(l).map(|layer| effectcraft_engine::viewer::layer_features(&ectx, layer)).and_then(|f| {
                        let c = map.to_comp(press);
                        f.into_iter().min_by(|a, b| {
                            let da = (a[0] - c[0]).powi(2) + (a[1] - c[1]).powi(2);
                            let db = (b[0] - c[0]).powi(2) + (b[1] - c[1]).powi(2);
                            da.total_cmp(&db)
                        })
                    });
                    (!layers.is_empty()).then_some(Gesture::Move { layers, start: map.to_comp(press), snap_src })
                } else {
                    // With path vertices on screen the marquee selects vertices (layers stay
                    // selected); otherwise it starts from a clean selection.
                    if !mods.shift && !vertex_hits.iter().any(|h| h.tangent.is_none()) {
                        let _ = app.session.execute("edit.deselectAll", json!({}));
                    }
                    Some(Gesture::Marquee { start: press })
                }
            }
            // The Roto Brush paints in the Layer panel only: say so instead of doing nothing.
            rt if rt.roto_kind(false).is_some() => {
                app.ui.status = crate::i18n::tr(ROTO_HINT).to_string();
                None
            }
            _ => None,
        };
        if let Some(g) = g {
            ui.data_mut(|d| d.insert_temp(gid, g));
        }
    }
    let gesture: Option<Gesture> = ui.data(|d| d.get_temp(gid));
    if let (Some(g), Some(pos)) = (gesture.clone(), resp.interact_pointer_pos()) {
        let cpt = map.to_comp(pos);
        let merge = format!("viewer-drag-{}", ui.data(|d| d.get_temp::<u64>(egui::Id::new("viewer-drag-n")).unwrap_or(0)));
        match g {
            Gesture::Pan { start_pan } => {
                // No total drag on the release frame: keep the pan reached so far (it used to
                // snap back to where the drag began).
                if let Some(d) = resp.total_drag_delta() {
                    app.ui.viewer.pan = [start_pan[0] + d.x, start_pan[1] + d.y];
                }
            }
            Gesture::Move { layers, start, snap_src } => {
                let mut d = one_axis([cpt[0] - start[0], cpt[1] - start[1]], mods.shift);
                if let Some(src) = snap_src {
                    let ids: Vec<LayerId> = layers.iter().map(|x| x.0).collect();
                    let c = vt::snap(app, &ctx, &ectx, &map, &ids, &[[src[0] + d[0], src[1] + d[1]]], mods);
                    d = [d[0] + c[0], d[1] + c[1]];
                    if vt::snapping_on(app, mods) {
                        vt::draw_handle(&painter, map.to_screen([src[0] + d[0], src[1] + d[1]]));
                    }
                }
                for (lid, p0, ax, ay) in layers {
                    let v = json!([p0[0] + ax[0] * d[0] + ay[0] * d[1], p0[1] + ax[1] * d[0] + ay[1] * d[1], p0[2] + ax[2] * d[0] + ay[2] * d[1]]);
                    let _ = app.session.execute("prop.set", json!({"layer": lid.0, "path": "transform/position", "value": v, "merge": merge}));
                }
            }
            Gesture::Scale { layer, start_scale, start_local, handle, anchor, axes, l2c, inv, others } => {
                let lp = inv.apply(gv2(cpt[0], cpt[1]));
                let uniform = mods.shift && axes == [true, true];
                let f = drag_ratio(start_local, [lp.x - anchor[0], lp.y - anchor[1]], axes, uniform);
                // The handle snaps (to the comp's and other layers' edges, corners and centres),
                // so a layer scales exactly to them, e.g. to the comp's size.
                let f = snap_handle(f, handle, anchor, &l2c, axes, uniform, |p| vt::snap(app, &ctx, &ectx, &map, &[layer], &[p], mods));
                for (lid, s0) in std::iter::once((layer, start_scale)).chain(others) {
                    let v = [s0[0] * f[0], s0[1] * f[1], s0[2]];
                    let _ = app.session.execute("prop.set", json!({"layer": lid.0, "path": "transform/scale", "value": v, "merge": merge}));
                }
            }
            Gesture::Rotate { layer, center, last_angle, turned, start_rot, others } => {
                // Count the turn incrementally: the absolute angle wraps at ±180°, so writing it
                // straight would jump a revolution back each time the pointer went round.
                let ang = ((pos.y - center.y) as f64).atan2((pos.x - center.x) as f64).to_degrees();
                let turned = turned + turn_delta(last_angle, ang);
                ui.data_mut(|dd| dd.insert_temp(gid, Gesture::Rotate { layer, center, last_angle: ang, turned, start_rot, others: others.clone() }));
                // Shift snaps the accumulated value (so 405° snaps to 1x +45°, not to 45°).
                let r = rotate_value(start_rot, turned, mods.shift);
                let _ = app.session.execute("prop.set", json!({"layer": layer.0, "path": "transform/rotation", "value": r, "merge": merge}));
                for (lid, r0) in others {
                    let _ =
                        app.session.execute("prop.set", json!({"layer": lid.0, "path": "transform/rotation", "value": r0 + (r - start_rot), "merge": merge}));
                }
            }
            Gesture::Anchor { layer, start_anchor, start_pos, start, inv, l2p, own, anchor_only } => {
                // Pan Behind snaps the anchor point (to its own layer's box, other layers' features,
                // guides and the grid); Shift keeps the move to one axis.
                let d = one_axis([cpt[0] - start[0], cpt[1] - start[1]], mods.shift);
                let anchor_c = inv.inverse().map(|m| m.apply(gv2(start_anchor[0], start_anchor[1]))).unwrap_or(gv2(start[0], start[1]));
                let want = [anchor_c.x + d[0], anchor_c.y + d[1]];
                let sc = vt::snap_with(app, &ctx, &ectx, &map, &[layer], &[want], mods, &own);
                let a = inv.apply(gv2(start[0], start[1]));
                let b = inv.apply(gv2(start[0] + d[0] + sc[0], start[1] + d[1] + sc[1]));
                let dl = gv2(b.x - a.x, b.y - a.y);
                let na = [start_anchor[0] + dl.x, start_anchor[1] + dl.y, start_anchor[2]];
                let _ = app.session.execute("prop.set", json!({"layer": layer.0, "path": "transform/anchor", "value": na, "merge": merge}));
                // Position follows so the layer stays put, unless Alt moves the anchor point alone.
                if !anchor_only {
                    let dp = l2p.apply_vec(dl);
                    let np = [start_pos[0] + dp.x, start_pos[1] + dp.y, start_pos[2]];
                    let _ = app.session.execute("prop.set", json!({"layer": layer.0, "path": "transform/position", "value": np, "merge": merge}));
                }
            }
            Gesture::Camera { tool } => {
                let d = resp.drag_delta();
                if d != egui::Vec2::ZERO {
                    let (dx, dy) = ((d.x / zoom) as f64, (d.y / zoom) as f64);
                    let (id, params) = match tool {
                        Tool::Orbit => ("camera.orbit", json!({"yaw": -d.x as f64 * 0.4, "pitch": d.y as f64 * 0.4, "merge": merge})),
                        Tool::PanCamera => ("camera.pan", json!({"dx": dx, "dy": dy, "merge": merge})),
                        _ => ("camera.dolly", json!({"amount": -dy * 2.0, "merge": merge})),
                    };
                    if let Err(e) = app.session.execute(id, params) {
                        app.ui.status = e.to_string();
                    }
                }
            }
            Gesture::Create { tool, start } => {
                if let Some(d) = shape_drag(app, tool, start, cpt, mods) {
                    draw_shape_ghost(&painter, &map, app, &d, &t);
                }
            }
            Gesture::Vertices { start, src, applied, inv, layer } => {
                // The grabbed vertex snaps (to other layers' vertices and features, guides, grid).
                let want = [cpt[0] - start[0], cpt[1] - start[1]];
                let c = vt::snap(app, &ctx, &ectx, &map, &[layer], &[[src[0] + want[0], src[1] + want[1]]], mods);
                let total = [want[0] + c[0], want[1] + c[1]];
                let d = inv.apply_vec(gv2(total[0] - applied[0], total[1] - applied[1]));
                if d.x != 0.0 || d.y != 0.0 {
                    let _ = app.session.execute("mask.moveVertices", json!({"delta": [d.x, d.y], "merge": merge}));
                }
                ui.data_mut(|dd| dd.insert_temp(gid, Gesture::Vertices { start, src, applied: total, inv, layer }));
            }
            Gesture::Overlay(d) => match d {
                ov::Drag::Guide { index, vertical } => {
                    let pos_v = if vertical { cpt[0] } else { cpt[1] }.round();
                    let _ = app.session.execute("view.moveGuide", json!({"index": index, "position": pos_v, "merge": merge}));
                }
                ov::Drag::Roi { start, keep } => {
                    let e = [keep[0].unwrap_or(cpt[0]), keep[1].unwrap_or(cpt[1])];
                    let r = Rect::from_two_pos(map.to_screen(start), map.to_screen(e));
                    painter.rect_stroke(r, 0.0, Stroke::new(1.0, Color32::WHITE), StrokeKind::Middle);
                }
                d => {
                    let mut snapper = |a: &EffectcraftApp, p: [f64; 2], l: LayerId| vt::snap(a, &ctx, &ectx, &map, &[l], &[p], mods);
                    if let Some(nd) = ov::update(app, &d, cpt, pos, mods, &merge, zoom, &mut snapper) {
                        ui.data_mut(|dd| dd.insert_temp(gid, Gesture::Overlay(nd)));
                    }
                }
            },
            Gesture::Tangent { layer, mask, index, out, vertex, inv } => {
                let lp = inv.apply(gv2(cpt[0], cpt[1]));
                let t = [lp.x - vertex[0], lp.y - vertex[1]];
                let mut params = json!({"layer": layer.0, "mask": mask, "index": index, "merge": merge});
                params[if out { "out" } else { "in" }] = json!(t);
                if !mods.alt {
                    params[if out { "in" } else { "out" }] = json!([-t[0], -t[1]]);
                }
                let _ = app.session.execute("mask.setVertex", params);
            }
            Gesture::PuppetRecord { layer, pin, others, inv, began, cti, mut samples } => {
                let lp = inv.apply(gv2(cpt[0], cpt[1]));
                let now = ui.input(|i| i.time) - began;
                if samples.last().is_none_or(|s| now > s.0) {
                    samples.push((now, [lp.x, lp.y]));
                }
                // The current time runs with the recording (Record Options ▸ Speed).
                let speed = app.session.state.puppet.record_speed.max(1.0) / 100.0;
                app.session.set_time(cti + Tick::from_seconds_f64(now / speed));
                painter.circle_filled(pos, 6.0, Color32::from_rgb(0xe0, 0x30, 0x30));
                painter.text(pos + vec2(10.0, -10.0), Align2::LEFT_BOTTOM, crate::i18n::tr("Recording"), Tokens::ui(11.0), Color32::from_rgb(0xff, 0x60, 0x60));
                ui.data_mut(|dd| dd.insert_temp(gid, Gesture::PuppetRecord { layer, pin, others, inv, began, cti, samples }));
                ui.ctx().request_repaint();
            }
            Gesture::PuppetRotate { layer, pin, center, last, rotation } => {
                // Accumulate the turn since the last frame (wrapped to ±180°) so a drag can go
                // past a full revolution; Shift snaps to 15°.
                let a = (pos - center).angle();
                let mut d = (a - last) as f64;
                d = (d + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU) - std::f64::consts::PI;
                let rotation = rotation + d.to_degrees();
                let deg = if mods.shift { (rotation / 15.0).round() * 15.0 } else { rotation };
                ui.data_mut(|dd| dd.insert_temp(gid, Gesture::PuppetRotate { layer, pin, center, last: a, rotation }));
                if let Err(e) = app.session.execute("puppet.setPin", json!({"layer": layer.0, "pin": pin, "rotation": deg, "merge": merge})) {
                    app.ui.status = e.to_string();
                }
            }
            Gesture::PuppetScale { layer, pin, center, start, scale } => {
                // Shift snaps the scale to 5 % steps.
                let pct = scale * (pos.distance(center) / start) as f64 * 100.0;
                let pct = if mods.shift { (pct / 5.0).round() * 5.0 } else { pct }.max(1.0);
                if let Err(e) = app.session.execute("puppet.setPin", json!({"layer": layer.0, "pin": pin, "scale": pct, "merge": merge})) {
                    app.ui.status = e.to_string();
                }
            }
            Gesture::PuppetPin { layer, pin, inv } => {
                let lp = inv.apply(gv2(cpt[0], cpt[1]));
                if let Err(e) = app.session.execute("puppet.movePin", json!({"layer": layer.0, "pin": pin, "position": [lp.x, lp.y], "merge": merge})) {
                    app.ui.status = e.to_string();
                }
            }
            Gesture::Pen { layer, mask, index, vertex, inv, press } => {
                if pos.distance(press) > 3.0 {
                    let lp = inv.apply(gv2(cpt[0], cpt[1]));
                    let t = [lp.x - vertex[0], lp.y - vertex[1]];
                    let _ = app
                        .session
                        .execute("mask.setVertex", json!({"layer": layer.0, "mask": mask, "index": index, "out": t, "in": [-t[0], -t[1]], "merge": merge}));
                }
            }
            Gesture::Track { layer, tracker, drag } => {
                let mut q = super::tracker::drag_params(&drag, cpt);
                q["layer"] = json!(layer.0);
                q["tracker"] = json!(tracker);
                q["merge"] = json!(merge);
                let _ = app.session.execute("track.setPoint", q);
            }
            Gesture::ObjectMatteBox { start } => super::object_matte_tool::draw_box(app, &painter, start, pos),
            Gesture::Marquee { start } | Gesture::PinMarquee { start } => {
                let r = Rect::from_two_pos(start, pos);
                painter.rect_filled(r, 0.0, t.accent.gamma_multiply(0.12));
                painter.rect_stroke(r, 0.0, Stroke::new(1.0, t.accent), StrokeKind::Middle);
            }
        }
    }
    if resp.drag_stopped() {
        if let (Some(g), Some(pos)) = (gesture, resp.interact_pointer_pos()) {
            match g {
                Gesture::PinMarquee { start } => {
                    // Pins inside the box, per layer; Shift adds to the selection.
                    let r = Rect::from_two_pos(start, pos);
                    let mut first = !mods.shift;
                    let mut layers: Vec<LayerId> = pin_hits.iter().map(|h| h.layer).collect();
                    layers.dedup();
                    if first && pin_hits.iter().all(|h| !r.contains(h.pos)) {
                        let _ = app.session.execute("puppet.selectPins", json!({"pins": []}));
                    }
                    for l in layers {
                        let pins: Vec<u64> = pin_hits.iter().filter(|h| h.layer == l && r.contains(h.pos)).map(|h| h.pin).collect();
                        if !pins.is_empty() {
                            let _ = app.session.execute("puppet.selectPins", json!({"layer": l.0, "pins": pins, "add": !first}));
                            first = false;
                        }
                    }
                }
                Gesture::PuppetRecord { layer, pin, others, cti, samples, .. } => {
                    let pts: Vec<serde_json::Value> = samples.iter().map(|(t, p)| json!([t, p[0], p[1]])).collect();
                    let q = json!({"layer": layer.0, "pin": pin, "pins": others, "samples": pts, "start": cti.seconds()});
                    match app.session.execute("puppet.recordPin", q) {
                        Ok(_) => {
                            for p in std::iter::once(pin).chain(others) {
                                super::puppet_tool::reveal_pin(app, layer, p);
                            }
                        }
                        Err(e) => app.ui.status = e.to_string(),
                    }
                }
                Gesture::PuppetPin { layer, pin, .. } => super::puppet_tool::reveal_pin(app, layer, pin),
                Gesture::ObjectMatteBox { start } => super::object_matte_tool::apply(app, &ectx, &map, start, Some(pos), mods.shift),
                Gesture::Create { tool, start } => {
                    let end = map.to_comp(pos);
                    create_shape(app, tool, start, end, mods);
                }
                Gesture::Overlay(ov::Drag::Roi { start, keep }) => {
                    let c = map.to_comp(pos);
                    let end = [keep[0].unwrap_or(c[0]), keep[1].unwrap_or(c[1])];
                    let (x0, y0) = (start[0].min(end[0]).max(0.0), start[1].min(end[1]).max(0.0));
                    let (x1, y1) = (start[0].max(end[0]).min(comp.width as f64), start[1].max(end[1]).min(comp.height as f64));
                    if x1 - x0 >= 2.0 && y1 - y0 >= 2.0 {
                        let rect = [x0.round(), y0.round(), (x1 - x0).round(), (y1 - y0).round()];
                        let _ = app.session.execute("view.setRegionOfInterest", json!({"rect": rect}));
                    }
                    app.ui.viewer.roi_draw = false;
                }
                // A guide dragged off the image (back to the rulers) is removed.
                Gesture::Overlay(ov::Drag::Guide { index, .. }) if !area.contains(pos) || !comp_rect.expand(40.0).contains(pos) => {
                    let _ = app.session.execute("view.removeGuide", json!({"index": index}));
                }
                Gesture::Marquee { start } if vertex_hits.iter().any(|h| h.tangent.is_none() && Rect::from_two_pos(start, pos).contains(h.pos)) => {
                    // Marquee over path vertices selects them.
                    let r = Rect::from_two_pos(start, pos);
                    let v: Vec<serde_json::Value> = vertex_hits
                        .iter()
                        .filter(|h| h.tangent.is_none() && r.contains(h.pos))
                        .map(|h| json!({"layer": h.layer.0, "mask": h.mask, "index": h.index}))
                        .collect();
                    let _ = app.session.execute("mask.selectVertices", json!({"vertices": v, "add": mods.shift}));
                }
                Gesture::Marquee { start } => {
                    let a = map.to_comp(start);
                    let b = map.to_comp(pos);
                    let hits: Vec<u64> = selectable_layers(&comp, time)
                        .filter(|l| layer_quad(&ectx, l).is_some_and(|(_, q, _)| quad_intersects_box(&q, a, b)))
                        .map(|l| l.id.0)
                        .collect();
                    let _ = app.session.execute("layer.select", json!({"layers": hits, "toggle": mods.shift}));
                }
                _ => {}
            }
        }
        ui.data_mut(|d| {
            d.remove::<Gesture>(gid);
            let n: u64 = d.get_temp(egui::Id::new("viewer-drag-n")).unwrap_or(0);
            d.insert_temp(egui::Id::new("viewer-drag-n"), n + 1);
        });
    }
    if resp.clicked()
        && !text_owns
        && let Some(pos) = resp.interact_pointer_pos()
    {
        let cpt = map.to_comp(pos);
        match app.ui.tool {
            Tool::Zoom => {
                let k = if mods.alt { 0.5 } else { 2.0 };
                let nz = (zoom * k).clamp(0.01, 32.0);
                let new_origin = pos - vec2(cpt[0] as f32 * nz, cpt[1] as f32 * nz);
                let pan = new_origin + vec2(cw * nz / 2.0, ch * nz / 2.0) - area.center();
                app.ui.viewer.zoom = Some(nz);
                app.ui.viewer.pan = [pan.x, pan.y];
            }
            Tool::Type | Tool::TypeVertical => {}
            Tool::Pen | Tool::MaskFeather => {}
            // On a motion path (After Effects): Add Vertex adds a keyframe where it is clicked,
            // Delete Vertex removes the keyframe, Convert Vertex switches it between Linear and
            // Auto Bezier spatial interpolation.
            Tool::PenAdd => {
                if let Some((l, m, seg, tt)) = ov::segment_at(&paths, &map, pos, 6.0) {
                    let _ = app.session.execute("mask.insertVertex", json!({"layer": l.0, "mask": m, "segment": seg, "t": tt}));
                } else if let Some(f) = ov::motion_frame_at(&motion_frames, &key_hits, pos) {
                    add_motion_key(app, f);
                }
            }
            Tool::PenDelete => {
                if let Some(h) = vertex_at(pos, false) {
                    let _ = app.session.execute("mask.deleteVertices", json!({"vertices": [{"layer": h.layer.0, "mask": h.mask, "index": h.index}]}));
                } else if let Some(k) = ov::motion_key_at(&key_hits, pos)
                    && select_motion_key(app, &k)
                {
                    let _ = app.session.execute("keys.delete", json!({}));
                }
            }
            Tool::PenConvert => {
                if let Some(h) = vertex_at(pos, false) {
                    let _ = app.session.execute("mask.convertVertex", json!({"layer": h.layer.0, "mask": h.mask, "index": h.index}));
                } else if let Some(k) = ov::motion_key_at(&key_hits, pos) {
                    // A key showing direction handles becomes Linear; one without, Auto Bezier.
                    let curved = key_hits.iter().any(|h| h.tangent.is_some() && h.layer == k.layer && h.prop == k.prop && h.time == k.time);
                    if select_motion_key(app, &k) {
                        let spatial = if curved { "linear" } else { "autoBezier" };
                        let _ = app.session.execute("keys.interpolation", json!({"spatial": spatial}));
                    }
                }
            }
            Tool::Selection if mods.alt && vertex_at(pos, false).is_some() => {
                if let Some(h) = vertex_at(pos, false) {
                    let _ = app.session.execute("mask.convertVertex", json!({"layer": h.layer.0, "mask": h.mask, "index": h.index}));
                }
            }
            Tool::Selection if ft.is_some() && ft.as_ref().and_then(|f| ov::begin_ft_drag(f, pos, &map)).is_none() => {
                // Clicking away from the free-transform box ends it.
                ov::end_free_transform(&ctx);
            }
            Tool::ObjectMatte => super::object_matte_tool::apply(app, &ectx, &map, pos, None, mods.shift),
            t if t.puppet_kind().is_some() => {
                if let Some((h, _)) = super::puppet_tool::hit(&pin_hits, pos) {
                    // Click selects the pin; Shift-click adds or removes it.
                    let _ = app.session.execute("puppet.selectPins", json!({"layer": h.layer.0, "pins": [h.pin], "toggle": mods.shift}));
                } else if let Some(l) = puppet_layer(app, cpt).and_then(|l| comp.layer(l))
                    && let Some(inv) = fx2c(&ectx, l).inverse()
                {
                    let lp = inv.apply(gv2(cpt[0], cpt[1]));
                    match app.session.execute("puppet.addPin", json!({"layer": l.id.0, "kind": t.puppet_kind(), "position": [lp.x, lp.y]})) {
                        Ok(r) => {
                            if let Some(pin) = r["pin"].as_u64() {
                                super::puppet_tool::reveal_pin(app, l.id, pin);
                            }
                        }
                        Err(e) => app.ui.status = e.to_string(),
                    }
                }
            }
            Tool::Selection if vertex_at(pos, false).is_some() => {
                if let Some(h) = vertex_at(pos, false) {
                    let me = json!({"layer": h.layer.0, "mask": h.mask, "index": h.index});
                    let _ = app.session.execute("mask.selectVertices", json!({"vertices": [me], "add": mods.shift, "toggle": mods.shift}));
                }
            }
            // The Roto Brush paints in the Layer panel only: select as usual and say so.
            rt if rt.roto_kind(false).is_some() => {
                if pick(app, &ectx, cpt, mods.shift).is_none() {
                    let _ = app.session.execute("edit.deselectAll", json!({}));
                }
                app.ui.status = crate::i18n::tr(ROTO_HINT).to_string();
            }
            _ => {
                if pick(app, &ectx, cpt, mods.shift).is_none() {
                    let _ = app.session.execute("edit.deselectAll", json!({}));
                } else if !mods.shift && !app.session.state.selected_vertices.is_empty() && ov::segment_at(&paths, &map, pos, 6.0).is_none() {
                    // A click away from the paths, even on their layer, deselects the mask
                    // points (the layer stays selected), as in After Effects (#513).
                    let _ = app.session.execute("mask.selectVertices", json!({"vertices": []}));
                }
            }
        }
    }
    if resp.double_clicked()
        && !text_owns
        && app.ui.tool == Tool::Selection
        && let Some(pos) = resp.interact_pointer_pos()
        && let Some((l, m)) = vertex_at(pos, false).map(|h| (h.layer, h.mask)).or_else(|| ov::segment_at(&paths, &map, pos, 6.0).map(|s| (s.0, s.1)))
    {
        // Double-click a path: all its points selected in a free-transform box (Layer ▸ Mask and
        // Shape Path ▸ Free Transform Points), so dragging any of them, or inside the box, moves
        // the whole path (After Effects).
        if let Some(p) = paths.iter().find(|p| p.layer == l && p.uid == m) {
            let all: Vec<serde_json::Value> = (0..p.sp.vertices.len()).map(|index| json!({"layer": l.0, "mask": m, "index": index})).collect();
            let _ = app.session.execute("mask.selectVertices", json!({"vertices": all}));
        }
        ov::begin_free_transform(&ctx, l, m);
    } else if resp.double_clicked()
        && !text_owns
        && !app.ui.tool.is_pen()
        && let Some(pos) = resp.interact_pointer_pos()
    {
        let cpt = map.to_comp(pos);
        let roto = app.ui.tool.roto_kind(false).is_some();
        if let Some(l) = pick(app, &ectx, cpt, false).and_then(|l| comp.layer(l)) {
            match &l.source {
                effectcraft_engine::project::LayerSource::Comp { item } => app.session.open_comp(*item),
                // With a Roto Brush tool, double-clicking a layer opens it in the Layer panel,
                // where the brush works (After Effects).
                _ if roto => {
                    let id = l.id.0;
                    match crate::menus::invoke(app, &ctx, "layer.openLayer", json!({"layer": id})) {
                        Ok(_) => app.ui.status.clear(),
                        Err(e) => app.ui.status = e,
                    }
                }
                _ => {}
            }
        }
    }
    // 3D Camera Tracker points, targets and their menu (on top of the viewer's gestures while
    // the effect is selected), and its analysis banner.
    super::camera_tracker_ui::viewer_hook(app, ui, &painter, &map, &ectx, &|c, l| l2c(c, l).0);
    // Effect point controls and crosshair/eyedropper picks (on top of the viewer's gestures).
    crate::panels::effect_controls::viewer_hook(app, ui, &painter, &map, &ectx, &|c, l| l2c(c, l).0);
    // Selected shape gradients' start and end points (#612).
    crate::panels::gradient_handles::viewer_hook(app, ui, &painter, &map, &ectx, &|c, l| l2c(c, l).0);

    // Status chips (render time / caching).
    if app.playback.playing && app.playback.waiting {
        let r = Rect::from_min_size(area.min + vec2(10.0, 10.0), vec2(150.0, 22.0));
        painter.rect_filled(r, 11.0, Color32::from_black_alpha(170));
        painter.text(r.center(), Align2::CENTER_CENTER, crate::i18n::tr("Caching frames…"), Tokens::ui(11.5), t.cache_green);
    }
    if app.video_preview_off().is_some() {
        let r = Rect::from_min_size(area.min + vec2(10.0, 10.0), vec2(330.0, 22.0));
        painter.rect_filled(r, 11.0, Color32::from_black_alpha(190));
        painter.text(r.center(), Align2::CENTER_CENTER, crate::i18n::tr("Video preview is off (Preview panel): audio only"), Tokens::ui(11.5), t.timecode);
        ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("viewer-video-off-chip"), r));
    }

    vt::draw_snap(&ctx, &painter, &map, &ectx);
    vt::rulers(app, ui, &map, outer, area);
    vt::bottom_bar(app, ui, bar, zoom, time, &comp);
    if err_h > 0.0 {
        super::expr_bar::draw(app, ui, err_bar, cid);
    }
}

/// Parent-space position change per comp pixel of drag (x and y) for a layer: 2D layers map
/// through the parent transform; 3D layers move in the view plane at their depth.
fn drag_axes(ctx: &EvalCtx, l: &Layer) -> ([f64; 3], [f64; 3]) {
    if !l.is_3d() {
        let inv = parent_inverse(ctx, l);
        let a = inv.apply_vec(gv2(1.0, 0.0));
        let b = inv.apply_vec(gv2(0.0, 1.0));
        return ([a.x, a.y, 0.0], [b.x, b.y, 0.0]);
    }
    let cam = cam_state(ctx);
    let anchor = l.transform().map(|tr| ctx.v3(l, tr, "anchor", [0.0; 3])).unwrap_or([0.0; 3]);
    let wp = ctx.world_matrix(l).apply(anchor.into());
    let k = if cam.ortho { 1.0 / cam.zoom.max(1e-9) } else { cam.depth(wp).max(1.0) / cam.zoom.max(1e-9) };
    let pinv = match l.parent.and_then(|p| ctx.comp.layer(p)) {
        Some(p) => ctx.world_matrix(p).inverse().unwrap_or_default(),
        None => effectcraft_engine::geom::Mat4::IDENTITY,
    };
    let a = pinv.apply_vec(cam.right() * k);
    let b = pinv.apply_vec(cam.down() * k);
    ([a.x, a.y, a.z], [b.x, b.y, b.z])
}

/// Wireframes of cameras (frustum, point of interest) and lights (position, direction) as seen
/// through the current 3D view.
/// Camera and light wireframes seen through `cam` (`through_active`: `cam` is the active camera,
/// which is then not drawn). `pane` names the automation ids: `None` for the main view
/// (`viewer.camera.<id>`), `Some(view)` for a view of a 2- or 4-view layout
/// (`viewer.view.<view>.camera.<id>`).
#[allow(clippy::too_many_arguments)]
fn draw_rigs(
    app: &mut EffectcraftApp,
    painter: &egui::Painter,
    map: &ViewerMap,
    ectx: &EvalCtx,
    selected: &[LayerId],
    cam: CameraState,
    through_active: bool,
    pane: Option<&str>,
) {
    use effectcraft_engine::geom::Vec3;
    use effectcraft_engine::project::{LayerSource, LightKind};
    let (w, h) = (ectx.comp.width as f64, ectx.comp.height as f64);
    let active = ectx.comp.active_camera(ectx.time).map(|c| c.id);
    let prefix = pane.map(|v| format!("viewer.view.{v}.")).unwrap_or_else(|| "viewer.".to_string());
    let proj = |p: Vec3| cam.project(w, h, p).map(|v| map.to_screen([v.x, v.y]));
    let seg = |a: Vec3, b: Vec3, stroke: Stroke| {
        if let (Some(x), Some(y)) = (proj(a), proj(b)) {
            painter.line_segment([x, y], stroke);
        }
    };
    for l in ectx.comp.layers.iter().filter(|l| (l.is_camera() || l.is_light()) && l.is_active_at(ectx.time)) {
        let sel = selected.contains(&l.id);
        let col = app.tokens.label(l.label).gamma_multiply(if sel { 1.0 } else { 0.6 });
        let stroke = Stroke::new(if sel { 1.5 } else { 1.0 }, col);
        let (eye, fwd, down) = three_d::camera::layer_frame(ectx, l);
        let poi = |l: &Layer| -> Option<Vec3> {
            let pr = l.transform()?.get("poi").filter(|_| three_d::camera::is_two_node(l))?;
            Some(three_d::camera::parent_world(ectx, l).apply(ectx.value(l, pr).as_vec3().into()))
        };
        if l.is_camera() {
            if through_active && Some(l.id) == active {
                continue;
            }
            let zoom = l.props.prop("cameraOptions/zoom").map(|p| ectx.value(l, p).as_f64()).unwrap_or(1000.0);
            let right = down.cross(fwd);
            // Frustum to a plane at a quarter of the zoom distance.
            let k = 0.25;
            let c = eye + fwd * (zoom * k);
            let corners = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)].map(|(sx, sy)| c + right * (sx * w * 0.5 * k) + down * (sy * h * 0.5 * k));
            for i in 0..4 {
                seg(eye, corners[i], stroke);
                seg(corners[i], corners[(i + 1) % 4], stroke);
            }
            if let Some(poi) = poi(l) {
                seg(eye, poi, Stroke::new(1.0, col.gamma_multiply(0.6)));
                if let Some(s) = proj(poi) {
                    painter.circle_stroke(s, 4.0, stroke);
                }
            }
            if let Some(s) = proj(eye) {
                app.auto.add(&format!("{prefix}camera.{}", l.id.0), Rect::from_center_size(s, vec2(14.0, 14.0)), &l.name);
            }
        } else {
            let LayerSource::Light { kind } = l.source else { continue };
            if kind == LightKind::Ambient {
                continue;
            }
            let Some(s) = proj(eye) else { continue };
            painter.circle_stroke(s, 6.0, stroke);
            for i in 0..8 {
                let a = i as f32 * std::f32::consts::FRAC_PI_4;
                let d = vec2(a.cos(), a.sin());
                painter.line_segment([s + d * 8.0, s + d * 12.0], stroke);
            }
            if matches!(kind, LightKind::Spot | LightKind::Parallel) {
                let target = poi(l).unwrap_or(eye + fwd * 300.0);
                seg(eye, target, Stroke::new(1.0, col.gamma_multiply(0.7)));
                if let Some(q) = proj(target) {
                    painter.circle_stroke(q, 3.0, stroke);
                }
            }
            app.auto.add(&format!("{prefix}light.{}", l.id.0), Rect::from_center_size(s, vec2(16.0, 16.0)), &l.name);
        }
    }
}

/// Pen tool press at `pos`: continue the path in progress (pressing its first vertex closes it),
/// resume an open path whose last vertex is pressed, add a vertex on a visible path's segment, or
/// start a new path: a mask on the selected footage/solid layer (or shape layer, with Tool Creates
/// Mask), a shape path (Shape n group with the Tools bar's Fill and Stroke) on the selected shape
/// layer, or a new shape layer when nothing is selected. The placed point snaps.
/// Add a Position keyframe at a motion path's frame, keeping the path where it is.
fn add_motion_key(app: &mut EffectcraftApp, f: ov::PathFrame) {
    if let Err(e) = app.session.execute("prop.addKey", json!({"layer": f.layer.0, "prop": f.prop, "time": f.time.seconds()})) {
        app.ui.status = e.to_string();
    }
}

/// Select only the motion path's keyframe `k` (what the vertex tools act on).
fn select_motion_key(app: &mut EffectcraftApp, k: &ov::KeyHit) -> bool {
    app.session.execute("keys.select", json!({"keys": [{"layer": k.layer.0, "prop": k.prop, "time": k.time.seconds()}]})).is_ok()
}

/// The pointer for a Pen slot tool: the tool's own pen, the Pen showing Add Vertex's over a path
/// it would add a point to (After Effects).
fn pen_cursor_icon(tool: Tool, over_path: bool) -> crate::icons::Icon {
    use crate::icons::Icon;
    match tool {
        Tool::Pen if over_path => Icon::PenAdd,
        Tool::PenAdd => Icon::PenAdd,
        Tool::PenDelete => Icon::PenDelete,
        Tool::PenConvert => Icon::PenConvert,
        Tool::MaskFeather => Icon::MaskFeather,
        _ => Icon::Pen,
    }
}

/// Paint pen pointer `icon` with its tip at `at`, outlined for any background.
fn pen_cursor(ctx: &egui::Context, at: Pos2, icon: crate::icons::Icon) {
    use crate::icons::Icon;
    // Where each glyph's tip sits in its 16 × 16 box.
    let tip = match icon {
        Icon::PenAdd | Icon::PenDelete | Icon::MaskFeather => vec2(6.0, 2.5),
        Icon::PenConvert => vec2(8.0, 3.5),
        _ => vec2(8.0, 1.8),
    };
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("viewer-pen-cursor")));
    let rect = Rect::from_min_size(at - tip, vec2(16.0, 16.0));
    for d in [vec2(-1.0, 0.0), vec2(1.0, 0.0), vec2(0.0, -1.0), vec2(0.0, 1.0)] {
        crate::icons::paint(&painter, rect.translate(d), icon, Color32::BLACK);
    }
    crate::icons::paint(&painter, rect, icon, Color32::WHITE);
}

#[allow(clippy::too_many_arguments)]
fn pen_press(
    app: &mut EffectcraftApp,
    ui: &mut egui::Ui,
    ectx: &EvalCtx,
    map: &ViewerMap,
    paths: &[ov::PathInfo],
    motion_frame: Option<ov::PathFrame>,
    pos: Pos2,
    mods: egui::Modifiers,
) {
    use effectcraft_engine::project::LayerSource;
    let gid = egui::Id::new("viewer-gesture");
    let current: Option<(LayerId, u64)> = ui.data(|d| d.get_temp(pen_id()));
    let c0 = map.to_comp(pos);
    let corr = vt::snap(app, ui.ctx(), ectx, map, &[], &[c0], mods);
    let c = [c0[0] + corr[0], c0[1] + corr[1]];
    if let Some((lid, uid)) = current
        && let Some(p) = ov::path_of(ectx, lid, uid)
        && let Some(inv) = p.m.inverse()
    {
        let n = p.sp.vertices.len();
        let first = p.sp.vertices.first().map(|v| {
            let q = p.m.apply(gv2(v[0], v[1]));
            map.to_screen([q.x, q.y])
        });
        if n >= 2 && first.is_some_and(|f| f.distance(pos) < 8.0) {
            let _ = app.session.execute("mask.setClosed", json!({"layer": lid.0, "mask": uid, "closed": true}));
            ui.data_mut(|d| {
                d.remove::<(LayerId, u64)>(pen_id());
                d.remove::<Gesture>(gid);
            });
            return;
        }
        // Shift keeps the new segment at a multiple of 45° from the vertex before it (#1544).
        let last = p.sp.vertices.last().map(|v| p.m.apply(gv2(v[0], v[1])));
        let c = match last {
            Some(q) if mods.shift => constrain_45([q.x, q.y], c),
            _ => c,
        };
        let lp = inv.apply(gv2(c[0], c[1]));
        let lp = [lp.x, lp.y];
        if let Ok(v) = app.session.execute("mask.addVertex", json!({"layer": lid.0, "mask": uid, "point": lp})) {
            let index = v.as_u64().unwrap_or(0) as usize;
            ui.data_mut(|d| d.insert_temp(gid, Gesture::Pen { layer: lid, mask: uid, index, vertex: lp, inv, press: pos }));
        }
        return;
    }
    // Clicking the last vertex of an open path (e.g. after Esc ended it) resumes drawing it.
    if let Some(cur) = ov::open_end_at(paths, map, pos, 8.0) {
        ui.data_mut(|d| d.insert_temp(pen_id(), cur));
        return;
    }
    // On a visible path's segment the Pen adds a vertex there; on a motion path, a keyframe.
    if let Some(f) = motion_frame {
        add_motion_key(app, f);
        return;
    }
    if let Some((l, m, seg, t)) = ov::segment_at(paths, map, pos, 5.0) {
        let _ = app.session.execute("mask.insertVertex", json!({"layer": l.0, "mask": m, "segment": seg, "t": t}));
        return;
    }
    let target = app
        .session
        .state
        .selected_layers
        .iter()
        .copied()
        .find(|id| ectx.comp.layer(*id).is_some_and(|l| !l.switches.locked && (l.masks().is_some() || matches!(l.source, LayerSource::Shape))));
    let layer = target.and_then(|id| ectx.comp.layer(id));
    let (lid, uid, inv) = match layer {
        Some(l) if !matches!(l.source, LayerSource::Shape) || app.session.state.shape_tool.creates_mask => {
            let Some(inv) = l2c(ectx, l).0.inverse() else { return };
            let lp = inv.apply(gv2(c[0], c[1]));
            let Ok(v) = app.session.execute("mask.new", json!({"layer": l.id.0, "vertices": [[lp.x, lp.y]]})) else { return };
            let Some(mask) = v["mask"].as_u64() else { return };
            (l.id, mask, inv)
        }
        Some(l) => {
            let Some(inv) = l2c(ectx, l).0.inverse() else { return };
            let lp = inv.apply(gv2(c[0], c[1]));
            let r = app.session.execute("shape.newPath", json!({"layer": l.id.0, "vertices": [[lp.x, lp.y]], "space": "layer"}));
            let Some(path) = r.ok().and_then(|v| v["path"].as_u64()) else { return };
            (l.id, path, inv)
        }
        None => {
            // A new shape layer at the comp centre (unrotated: comp → layer is a translation).
            let r = app.session.execute("shape.newPath", json!({"vertices": [c], "space": "comp"}));
            let Some(v) = r.ok() else { return };
            let (Some(l), Some(path)) = (v["layer"].as_u64(), v["path"].as_u64()) else { return };
            let inv = Mat3::translate(gv2(-(ectx.comp.width as f64) / 2.0, -(ectx.comp.height as f64) / 2.0));
            (LayerId(l), path, inv)
        }
    };
    let lp = inv.apply(gv2(c[0], c[1]));
    ui.data_mut(|d| {
        d.insert_temp(pen_id(), (lid, uid));
        d.insert_temp(gid, Gesture::Pen { layer: lid, mask: uid, index: 0, vertex: [lp.x, lp.y], inv, press: pos });
    });
}

/// `to` moved onto the nearest line through `from` at a multiple of 45° (horizontal, vertical or
/// diagonal), as far along it as `to` lies: the Pen's Shift-click.
fn constrain_45(from: [f64; 2], to: [f64; 2]) -> [f64; 2] {
    let (dx, dy) = (to[0] - from[0], to[1] - from[1]);
    if !(dx.is_finite() && dy.is_finite()) || (dx == 0.0 && dy == 0.0) {
        return to;
    }
    let step = std::f64::consts::FRAC_PI_4;
    let a = (dy.atan2(dx) / step).round() * step;
    let (ux, uy) = (a.cos(), a.sin());
    let along = dx * ux + dy * uy;
    [from[0] + ux * along, from[1] + uy * along]
}

/// Visible, unlocked layers eligible for viewer hit tests, in front-to-back order.
/// Solo scope matches the renderer: any soloed AV layer activates it at every time (as in After
/// Effects), even when hidden, locked or outside its in-out range.
pub(crate) fn selectable_layers(comp: &Comp, time: Tick) -> impl Iterator<Item = &Layer> {
    let any_solo = comp.layers.iter().any(|l| l.switches.solo && l.source.is_av());
    comp.layers
        .iter()
        .filter(move |l| l.is_active_at(time) && l.switches.video && !l.switches.locked && !l.is_camera() && !l.is_light() && (!any_solo || l.switches.solo))
}

/// Selectable layers under a comp point, front to back.
pub(crate) fn layers_at<'a, 'b>(ectx: &'b EvalCtx<'a>, cpt: [f64; 2]) -> impl Iterator<Item = &'a Layer> + 'b {
    selectable_layers(ectx.comp, ectx.time).filter(move |l| layer_quad(ectx, l).is_some_and(|(_, q, _)| point_in_quad(cpt, &q)))
}

/// Topmost selectable layer under a comp point.
pub(crate) fn layer_at<'a>(ectx: &EvalCtx<'a>, cpt: [f64; 2]) -> Option<&'a Layer> {
    layers_at(ectx, cpt).next()
}

/// The layer a press at a comp point acts on, selected (Shift toggles the topmost layer). As in
/// After Effects, a selected layer under the point comes before the layers in front of it, so it
/// can be dragged where they cover it; otherwise the topmost layer (#230).
pub(crate) fn pick(app: &mut EffectcraftApp, ectx: &EvalCtx, cpt: [f64; 2], toggle: bool) -> Option<LayerId> {
    if toggle {
        let hit = layer_at(ectx, cpt)?.id;
        let _ = app.session.execute("layer.select", json!({"layers": [hit.0], "toggle": true}));
        return Some(hit);
    }
    let selected = &app.session.state.selected_layers;
    let hits: Vec<LayerId> = layers_at(ectx, cpt).map(|l| l.id).collect();
    let hit = *hits.iter().find(|id| selected.contains(id)).or(hits.first())?;
    if !selected.contains(&hit) {
        let _ = app.session.execute("layer.select", json!({"layers": [hit.0]}));
    }
    Some(hit)
}

/// Where a shape drag is going to land: a mask on the selected layer, a group inside it, or a new
/// shape layer.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ShapeTarget {
    Mask,
    IntoLayer,
    NewLayer,
}

impl ShapeTarget {
    /// What the drag will make and where, named the way the ghost captions it.
    fn label(self, layer_name: &str) -> String {
        match self {
            ShapeTarget::Mask => format!("Mask on “{layer_name}”"),
            ShapeTarget::IntoLayer => format!("Into “{layer_name}”"),
            ShapeTarget::NewLayer => "New shape layer".to_string(),
        }
    }
}

/// A shape drag as it stands, after the Tools' normalisations: the path it will make and where it
/// will land.
struct ShapeDrag {
    /// The `shape.newShape` / `layer.addMask` kind for the tool.
    kind: &'static str,
    /// Comp-space centre and size, what the drag will make.
    centre: [f64; 2],
    size: [f64; 2],
    target: ShapeTarget,
    /// The selected layer's name and the index its next mask would take, for the ghost's caption
    /// and colour.
    layer_name: String,
    mask_index: usize,
}

/// The kind `tool` draws, as `shape.newShape` / `layer.addMask` name it.
fn shape_kind(tool: Tool) -> &'static str {
    match tool {
        Tool::Rectangle => "rect",
        Tool::RoundedRect => "rounded",
        Tool::Ellipse => "ellipse",
        Tool::Polygon => "polygon",
        _ => "star",
    }
}

/// What a shape tool drag from `start` to `end` makes, or `None` while it is too small to make
/// anything. Alt (Option) draws from the press point and Shift squares the box; the live ghost and
/// [`create_shape`] both go through this, so the drag always shows what it will commit.
fn shape_drag(app: &EffectcraftApp, tool: Tool, start: [f64; 2], end: [f64; 2], mods: egui::Modifiers) -> Option<ShapeDrag> {
    let comp = app.session.active_comp()?;
    let selected = app.session.state.selected_layers.first().copied();
    let sel = selected.and_then(|id| comp.layer(id));
    // A footage, solid or other non-shape layer selected, or a shape layer with Tool Creates
    // Mask: a mask on that layer. Otherwise the engine's own target: the first selected unlocked
    // shape layer, else a new one.
    let target = match sel {
        Some(l) if l.source.is_av() && (app.session.state.shape_tool.creates_mask || !matches!(l.source, LayerSource::Shape)) => (ShapeTarget::Mask, Some(l)),
        _ => match effectcraft_engine::commands::shape_tool::selected_shape_target(&app.session, comp).and_then(|id| comp.layer(id)) {
            Some(l) => (ShapeTarget::IntoLayer, Some(l)),
            None => (ShapeTarget::NewLayer, None),
        },
    };
    let (target, layer) = target;
    let mask_index = if target == ShapeTarget::Mask { layer.and_then(|l| l.masks()).map_or(0, |m| m.groups().count()) } else { 0 };
    let (mut w, mut h) = ((end[0] - start[0]).abs(), (end[1] - start[1]).abs());
    // Ctrl/Cmd draws from the press point as the centre, as in After Effects.
    if mods.command {
        w *= 2.0;
        h *= 2.0;
    }
    if w < 2.0 && h < 2.0 {
        return None;
    }
    if mods.shift {
        let m = w.max(h);
        w = m;
        h = m;
    }
    Some(ShapeDrag {
        kind: shape_kind(tool),
        centre: if mods.command { start } else { [start[0].min(end[0]) + w / 2.0, start[1].min(end[1]) + h / 2.0] },
        size: [w, h],
        target,
        layer_name: layer.map_or_else(String::new, |l| l.name.clone()),
        mask_index,
    })
}

/// The Tools bar's paint colour as bytes, dimmed by `alpha` so the comp shows through the ghost.
fn paint_rgb(c: [f32; 3], opacity: f64, alpha: f32) -> Color32 {
    let f = |k: f32| (k.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color32::from_rgb(f(c[0]), f(c[1]), f(c[2])).gamma_multiply((opacity.clamp(0.0, 100.0) / 100.0) as f32 * alpha)
}

/// Draw a shape drag as it will land: the tool's own path, filled with the Tools bar's Fill and
/// outlined in its Stroke (or the mask colour, for a mask), captioned with where it goes.
fn draw_shape_ghost(painter: &egui::Painter, map: &ViewerMap, app: &EffectcraftApp, d: &ShapeDrag, t: &Tokens) {
    // The engine's own geometry for the kind, so the ghost is the shape and not a stand-in for it.
    let Some(sp) = effectcraft_engine::commands::shape_tool::mask_path(d.kind, d.centre, d.size) else { return };
    let mut pts = ov::flatten(&sp, &Mat3::IDENTITY, map);
    if pts.first() == pts.last() {
        pts.pop();
    }
    if pts.len() < 3 {
        return;
    }
    let tool = &app.session.state.shape_tool;
    let (fill, line) = match d.target {
        ShapeTarget::Mask => {
            let [r, g, b] = effectcraft_engine::prefs::mask_color(app.session.prefs.appearance.cycle_mask_colors, d.mask_index);
            (Color32::from_rgb(r, g, b).gamma_multiply(0.25), Color32::from_rgb(r, g, b))
        }
        _ => {
            let stroke_on = tool.stroke.kind != PaintKind::None && tool.stroke_width > 0.0;
            (
                if tool.fill.kind == PaintKind::None { Color32::TRANSPARENT } else { paint_rgb(tool.fill.color, tool.fill.opacity, 0.45) },
                if stroke_on { paint_rgb(tool.stroke.color, tool.stroke.opacity, 1.0) } else { t.accent },
            )
        }
    };
    let Some(bounds) = bounds_of(&pts) else { return };
    // A screen-width outline reads at every zoom, where a comp-pixel hairline does not.
    painter.add(egui::Shape::Path(egui::epaint::PathShape { points: pts, closed: true, fill, stroke: Stroke::new(1.5, line).into() }));
    // Caption it with where the shape lands: a mask, a group in the selected shape layer or a
    // new layer.
    let font = Tokens::ui(11.0);
    let galley = painter.layout_no_wrap(d.target.label(&d.layer_name), font, line);
    let caption = Rect::from_min_size(pos2(bounds.left(), bounds.top() - 5.0 - galley.size().y), galley.size());
    painter.rect_filled(caption.expand(3.0), 2.0, Color32::from_black_alpha(150));
    painter.galley(caption.min, galley, line);
}

/// The bounds of points on screen, or `None` for an empty set.
fn bounds_of(pts: &[Pos2]) -> Option<Rect> {
    let first = *pts.first()?;
    let mut r = Rect::from_min_max(first, first);
    for p in pts.iter().skip(1) {
        r.extend_with(*p);
    }
    Some(r)
}

fn create_shape(app: &mut EffectcraftApp, tool: Tool, a: [f64; 2], b: [f64; 2], mods: egui::Modifiers) {
    let Some(d) = shape_drag(app, tool, a, b, mods) else { return };
    let (kind, (w, h), (cx, cy)) = (d.kind, (d.size[0], d.size[1]), (d.centre[0], d.centre[1]));
    // A mask on the selected layer, as the ghost showed.
    if d.target == ShapeTarget::Mask {
        // In layer space: invert the layer transform.
        let sel_layer = app.session.state.selected_layers.first().copied().and_then(|id| app.session.active_comp().and_then(|c| c.layer(id)).cloned());
        if let Some(l) = sel_layer
            && let Some(comp) = app.session.active_comp_arc()
        {
            let ectx = EvalCtx {
                project: &app.session.project,
                comp_id: app.session.active_comp_id().unwrap_or_default(),
                comp: &comp,
                time: app.session.time(),
                expr: None,
                footage: None,
            };
            let inv = l2c(&ectx, &l).0.inverse().unwrap_or(Mat3::IDENTITY);
            let p0 = inv.apply(gv2(cx - w / 2.0, cy - h / 2.0));
            let p1 = inv.apply(gv2(cx + w / 2.0, cy + h / 2.0));
            let rect = [p0.x.min(p1.x), p0.y.min(p1.y), (p1.x - p0.x).abs(), (p1.y - p0.y).abs()];
            if let Err(e) = app.session.execute("layer.addMask", json!({"layer": l.id.0, "shape": kind, "rect": rect})) {
                app.ui.status = e.to_string();
            }
        }
        return;
    }
    // Into the selected shape layer's Contents as a new group (as in After Effects), else a new
    // shape layer; painted with the Tools bar's Fill and Stroke.
    if let Err(e) = app.session.execute("shape.newShape", json!({"kind": kind, "size": [w, h], "position": [cx, cy]})) {
        app.ui.status = e.to_string();
    }
}

fn empty_state(app: &mut EffectcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let p = ui.painter().clone();
    p.rect_filled(rect, 0.0, t.pasteboard);
    let c = rect.center();
    p.text(c - vec2(0.0, 40.0), Align2::CENTER_CENTER, crate::i18n::tr("Composition"), Tokens::semibold(16.0), t.text);
    let b1 = Rect::from_center_size(c + vec2(0.0, 0.0), vec2(220.0, 30.0));
    if widgets::text_button(ui, b1, "New Composition", true, &t, egui::Id::new("empty-newcomp")).clicked() {
        crate::panels::dialogs::open_new_comp(app);
    }
    app.auto.add("viewer.empty.newComp", b1, "New Composition");
    let b2 = Rect::from_center_size(c + vec2(0.0, 40.0), vec2(220.0, 30.0));
    if widgets::text_button(ui, b2, "New Composition From Footage", false, &t, egui::Id::new("empty-fromfootage")).clicked() {
        let _ = crate::menus::invoke(app, &ui.ctx().clone(), "file.import", json!({}));
    }
    app.auto.add("viewer.empty.import", b2, "New Composition From Footage");
    // A Project item dropped on the empty viewer creates the first composition,
    // just like dropping it onto an empty Timeline (#684). Multiple selected
    // items use the existing New Comp from Selection dialog.
    let ctx = ui.ctx().clone();
    if let Some(crate::panels::DragPayload::Item(dragged)) = egui::DragAndDrop::payload::<crate::panels::DragPayload>(&ctx).as_deref()
        && ui.rect_contains_pointer(rect)
    {
        let items: Vec<effectcraft_engine::project::ItemId> = crate::panels::project_drop_items(app, *dragged)
            .into_iter()
            .map(effectcraft_engine::project::ItemId)
            .filter(|id| {
                app.session.project.item(*id).is_some_and(|item| match &item.kind {
                    effectcraft_engine::project::ItemKind::Footage(f) => f.kind != effectcraft_engine::project::FootageKind::Data,
                    effectcraft_engine::project::ItemKind::Solid(_) | effectcraft_engine::project::ItemKind::Comp(_) => true,
                    _ => false,
                })
            })
            .collect();
        if !items.is_empty() {
            p.rect_stroke(rect.shrink(2.0), 0.0, Stroke::new(2.0, t.accent), StrokeKind::Inside);
            app.auto.add("viewer.newCompDrop", rect, "Drop footage to create a composition");
            if ctx.input(|i| i.pointer.any_released()) {
                app.session.state.project_selection = items;
                if let Err(e) = crate::menus::invoke(app, &ctx, "file.newCompFromSelection", json!({})) {
                    app.ui.status = e;
                }
                egui::DragAndDrop::clear_payload(&ctx);
            }
        }
    }
}

/// `#rrggbb` → sRGB bytes.
/// A grid or guide line in the Grids & Guides style: `lines`, `dashed` or `dots`.
pub(crate) fn styled_line(painter: &egui::Painter, pts: [Pos2; 2], stroke: Stroke, style: &str) {
    match style {
        "dashed" => {
            painter.add(egui::Shape::dashed_line(&pts, stroke, 6.0, 4.0));
        }
        "dots" => {
            painter.add(egui::Shape::dotted_line(&pts, stroke.color, 4.0, stroke.width.max(1.0) * 0.75));
        }
        _ => {
            painter.line_segment(pts, stroke);
        }
    }
}

/// `p` (points) moved to the nearest physical pixel corner at `ppp` pixels per point.
pub(crate) fn snap_to_pixels(p: Pos2, ppp: f32) -> Pos2 {
    if !(ppp.is_finite() && ppp > 0.0 && p.x.is_finite() && p.y.is_finite()) {
        return p;
    }
    pos2((p.x * ppp).round() / ppp, (p.y * ppp).round() / ppp)
}

/// Texture filtering for Viewer Zoom Quality: More Accurate = bilinear, Faster = nearest. As in
/// After Effects, a magnified frame (above 100 %, or a lower Resolution) always shows its pixels
/// as blocks, never smoothed, in either mode (#882).
pub(crate) fn zoom_texture_options(smooth: bool) -> egui::TextureOptions {
    if smooth { egui::TextureOptions { magnification: egui::TextureFilter::Nearest, ..egui::TextureOptions::LINEAR } } else { egui::TextureOptions::NEAREST }
}

/// The sampler GPU frames (mipmapped display textures) are drawn with: More Accurate filters
/// bilinearly within and between mip levels, so a frame shown below 100 % is averaged instead of
/// skipping pixels (#417); Faster takes the nearest pixel of the full-size frame. Magnified, both
/// show the nearest pixel (#882).
pub(crate) fn zoom_sampler(smooth: bool) -> eframe::wgpu::SamplerDescriptor<'static> {
    use eframe::wgpu::{FilterMode, MipmapFilterMode, SamplerDescriptor};
    let label = Some("viewer frame");
    if smooth {
        SamplerDescriptor {
            label,
            mag_filter: FilterMode::Nearest,
            min_filter: FilterMode::Linear,
            mipmap_filter: MipmapFilterMode::Linear,
            ..Default::default()
        }
    } else {
        SamplerDescriptor { label, lod_max_clamp: 0.0, ..Default::default() }
    }
}

/// The whole factor a CPU frame is averaged down by for its texture when `pixels` of it are drawn
/// over `screen` screen pixels: about one texel per screen pixel, so bilinear filtering never
/// skips frame pixels — the CPU frames' counterpart of the GPU frames' mip levels (#417). Faster
/// keeps every pixel.
pub(crate) fn minify_factor(smooth: bool, pixels: f64, screen: f64) -> usize {
    let k = (pixels / screen + 1e-3).floor();
    if smooth && k >= 1.0 { k.min(256.0) as usize } else { 1 }
}

pub(crate) use crate::widgets::hex_rgb;

/// The Extended Viewer's render region for this frame (see [`ViewerState::extended`]):
/// Settings ▸ 3D ▸ Extended Viewer on, the comp has 3D layers and the view is a custom 3D view
/// (or Draft 3D is on), and part of the visible viewer lies outside the comp frame.
///
/// [`ViewerState::extended`]: crate::state::ViewerState::extended
pub(crate) fn extended_region(
    app: &EffectcraftApp,
    comp: &effectcraft_engine::project::Comp,
    cid: effectcraft_engine::project::ItemId,
    map: ViewerMap,
) -> Option<[f64; 4]> {
    if !app.session.prefs.three_d.extended_viewer || !comp.has_3d() || app.session.state.region_of_interest.is_some() {
        return None;
    }
    let view = app.session.state.views3d.get(&cid).map(|v| v.current).unwrap_or_default();
    if view == View3D::ActiveCamera && !comp.draft_3d {
        return None;
    }
    let a = map.to_comp(map.area.min);
    let b = map.to_comp(map.area.max);
    effectcraft_engine::render::extended_region(comp.width, comp.height, [a[0], a[1], b[0], b[1]], 1.0)
}

fn rect_of(map: &ViewerMap, r: Option<[f64; 4]>) -> Rect {
    let [x, y, w, h] = r.unwrap_or_default();
    Rect::from_min_max(map.to_screen([x, y]), map.to_screen([x + w, y + h]))
}

/// Put a rendered frame on screen: CPU pixels go into an egui texture, averaged down by `minify`
/// (see [`minify_factor`]); GPU frames are drawn straight from their wgpu texture (registered with
/// egui-wgpu, no readback).
fn show_frame(app: &mut EffectcraftApp, ctx: &egui::Context, key: crate::frames::FrameKey, img: crate::frames::FrameImage, minify: usize) {
    use crate::frames::FrameImage;
    // Settings ▸ Previews ▸ Viewer Zoom Quality.
    let smooth = app.session.prefs.viewer_zoom_smooth();
    let opts = zoom_texture_options(smooth);
    match img {
        FrameImage::Cpu(img) => {
            // Frames wider or taller than the GPU's texture limit go up averaged down too (#201).
            let max = crate::frames::max_texture_side(ctx);
            let fitted = crate::frames::fit_shared_texture_by(&img, max, minify);
            match &mut app.viewer_tex {
                Some((tex, k, m)) if tex.size() == fitted.size() => {
                    tex.set(fitted, opts);
                    (*k, *m) = (key, minify);
                }
                _ => app.viewer_tex = Some((ctx.load_texture("viewer-frame", fitted, opts), key, minify)),
            }
            app.viewer_shown = app.viewer_tex.as_ref().map(|(t, k, _)| (t.id(), *k));
            app.viewer_image = Some(img);
            vt::set_texture_roi(ctx, app.session.state.region_of_interest.or(app.ui.viewer.extended));
        }
        FrameImage::Gpu(f) => {
            let Some(rs) = &app.wgpu else { return };
            let view = f.texture.create_view(&Default::default());
            let mut ren = rs.renderer.write();
            let id = match &app.viewer_native {
                Some((id, _, _)) => {
                    ren.update_egui_texture_from_wgpu_texture_with_sampler_options(&rs.device, &view, zoom_sampler(smooth), *id);
                    *id
                }
                None => ren.register_native_texture_with_sampler_options(&rs.device, &view, zoom_sampler(smooth)),
            };
            drop(ren);
            app.viewer_native = Some((id, key, f));
            app.viewer_shown = Some((id, key));
            // Pixels are read back on demand (viewer_pixels).
            app.viewer_image = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use effectcraft_engine::geom::{Mat4, vec3};

    /// #1544: Shift-clicking with the Pen keeps the new segment horizontal, vertical or diagonal.
    #[test]
    fn pen_shift_click_keeps_the_segment_at_45_degrees() {
        let near = |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9;
        let from = [100.0, 100.0];
        assert!(near(constrain_45(from, [104.0, 180.0]), [100.0, 180.0]), "straight down");
        assert!(near(constrain_45(from, [30.0, 97.0]), [30.0, 100.0]), "left");
        assert!(near(constrain_45(from, [150.0, 46.0]), [152.0, 48.0]), "up and right, on the diagonal");
        assert!(near(constrain_45(from, from), from), "on the vertex");
        assert!(constrain_45(from, [f64::NAN, 3.0])[0].is_nan(), "a non-finite point stays as it is");
    }

    /// The Rotation tool counts past a full turn in both directions, as in After Effects: the
    /// pointer going round the anchor 1.5 turns clockwise gives 540°, then back 2 turns gives
    /// −180°, with no jump where atan2 wraps from 180° to −180°.
    #[test]
    fn rotate_tool_accumulates_revolutions() {
        // Screen y points down, so a growing atan2 angle is clockwise on screen.
        let drag = |start: f64, steps: &[f64]| {
            let (mut last, mut turned, mut out) = (start, 0.0, vec![]);
            for &a in steps {
                // atan2's range: (−180°, 180°].
                let a = (a + 180.0).rem_euclid(360.0) - 180.0;
                turned += turn_delta(last, a);
                last = a;
                out.push(turned);
            }
            (last, turned, out)
        };
        let cw: Vec<f64> = (1..=54).map(|i| i as f64 * 10.0).collect();
        let (last, turned, seq) = drag(0.0, &cw);
        assert!((turned - 540.0).abs() < 1e-9, "{turned}");
        assert!(seq.windows(2).all(|w| (w[1] - w[0] - 10.0).abs() < 1e-9), "no jump: {seq:?}");
        assert!((rotate_value(0.0, turned, false) - 540.0).abs() < 1e-9);
        let ccw: Vec<f64> = (1..=72).map(|i| 540.0 - i as f64 * 10.0).collect();
        let (_, back, seq) = drag(last, &ccw);
        assert!(seq.windows(2).all(|w| (w[1] - w[0] + 10.0).abs() < 1e-9), "no jump: {seq:?}");
        let total = turned + back;
        assert!((total + 180.0).abs() < 1e-9, "{total}");
        assert!((rotate_value(30.0, total, false) + 150.0).abs() < 1e-9);
        // Shift snaps the accumulated value, not a wrapped one.
        assert_eq!(rotate_value(0.0, 400.0, true), 405.0);
        assert_eq!(rotate_value(0.0, -400.0, true), -405.0);
        // Across the seam: 170° → −170° is +20°, and back is −20°; exactly half a turn is +180°.
        assert!((turn_delta(170.0, -170.0) - 20.0).abs() < 1e-9);
        assert!((turn_delta(-170.0, 170.0) + 20.0).abs() < 1e-9);
        assert_eq!(turn_delta(0.0, 180.0), 180.0);
        assert_eq!(turn_delta(0.0, -180.0), 180.0);
        // Never crash or poison the value on non-finite input.
        assert_eq!(turn_delta(f64::NAN, 10.0), 0.0);
        assert_eq!(turn_delta(0.0, f64::INFINITY), 0.0);
        assert_eq!(rotate_value(15.0, f64::NAN, false), 15.0);
    }

    /// #1042: a path vertex lying under, or just beside, a bounding-box handle is grabbed
    /// instead of the handle; away from every handle the reach is unchanged.
    #[test]
    fn vertex_reaches_as_far_as_a_box_handle_over_it() {
        let handle = pos2(100.0, 100.0);
        let vertex = pos2(100.0, 100.0);
        // The press lands 8 points off the vertex, still on the handle's square.
        let press = pos2(108.0, 100.0);
        let pick = |p: Pos2, handles: &[Pos2]| vertex.distance(p) < vertex_grab_radius(p, handles.iter().copied());
        assert!(handle.distance(press) < HANDLE + 2.0, "the handle would take this press");
        assert!(pick(press, &[handle]), "the vertex wins over the handle");
        assert!(!pick(press, &[]), "no handle there: the usual reach");
        assert!(pick(pos2(105.0, 100.0), &[]));
        assert!(!pick(pos2(120.0, 100.0), &[handle]), "past the handle: nothing");
    }

    /// #882: the frame's corner sits on a physical pixel at any display scaling, so 100 % maps
    /// comp pixels 1:1 onto screen pixels.
    #[test]
    fn frame_origin_snaps_to_physical_pixels() {
        for ppp in [1.0f32, 1.25, 1.5, 1.75, 2.0] {
            for p in [pos2(-201.12, 72.4), pos2(689.8, 239.75), pos2(0.3, -0.37)] {
                let s = snap_to_pixels(p, ppp);
                for v in [s.x * ppp, s.y * ppp] {
                    assert!((v - v.round()).abs() < 1e-3, "ppp {ppp}: {p:?} → {s:?}");
                }
                assert!((s - p).length() <= 0.71 / ppp, "moved by at most half a pixel: {p:?} → {s:?}");
            }
        }
        assert_eq!(snap_to_pixels(pos2(1.3, 2.0), 0.0), pos2(1.3, 2.0));
    }

    /// #882: a magnified frame shows its pixels as blocks in both Zoom Quality modes; More
    /// Accurate only smooths when the frame is shown smaller.
    #[test]
    fn magnified_frames_are_never_smoothed() {
        use eframe::wgpu::FilterMode;
        for smooth in [true, false] {
            assert_eq!(zoom_texture_options(smooth).magnification, egui::TextureFilter::Nearest);
            assert_eq!(zoom_sampler(smooth).mag_filter, FilterMode::Nearest);
        }
        assert_eq!(zoom_texture_options(true).minification, egui::TextureFilter::Linear);
        assert_eq!(zoom_sampler(true).min_filter, FilterMode::Linear);
    }

    /// A perspective-like homography: w = 100 + y (the plane crosses the camera at y = −100).
    fn tilted() -> Mat3 {
        Mat3([[100.0, 0.0, 0.0], [0.0, 100.0, 0.0], [0.0, 1.0, 100.0]])
    }

    /// #572: a box reaching behind the camera is clipped at the near plane instead of flipping
    /// through infinity into long lines.
    #[test]
    fn outlines_clip_at_the_near_plane() {
        let m = tilted();
        let front = project_clipped(&m, &box_corners([-50.0, -50.0, 50.0, 50.0]));
        assert_eq!(front.len(), 4);
        let crossing = project_clipped(&m, &box_corners([-50.0, -150.0, 50.0, 50.0]));
        assert_eq!(crossing.len(), 4, "{crossing:?}");
        // Nothing flipped through infinity: a corner behind the camera (y = −150, w = −50) would
        // land at y = +300 if it were projected as is; clipped at the near plane it runs off far
        // above (negative y) instead, and the front corners stay where they were.
        assert!(crossing.iter().all(|p| p[0].is_finite() && p[1].is_finite() && p[1] < 34.0), "{crossing:?}");
        assert!(project_clipped(&m, &box_corners([-50.0, -400.0, 50.0, -200.0])).is_empty(), "wholly behind");
    }

    #[test]
    fn gizmo_segments_clip_at_the_near_plane() {
        // w = 100 + z.
        let pc = Mat4([[100.0, 0.0, 0.0, 0.0], [0.0, 100.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 1.0, 100.0]]);
        let s = project_segment(&pc, vec3(10.0, 0.0, 0.0), vec3(10.0, 0.0, -300.0)).unwrap();
        assert_eq!(s[0], [10.0, 0.0]);
        assert!(s[1][0] > 10.0 && s[1][0].is_finite(), "toward the camera it grows, never flips: {s:?}");
        assert!(project_segment(&pc, vec3(0.0, 0.0, -200.0), vec3(0.0, 0.0, -300.0)).is_none());
    }
}
