//! The Composition viewer: pasteboard + comp frame, magnification and resolution, transparency
//! grid, guides, layer bounding boxes with handles, anchor points, motion paths and masks, and
//! direct manipulation (move, scale, rotate, pan behind, shape/type creation, hand and zoom),
//! 3D views (Active Camera / Front / … / Custom View 3), camera tools (orbit, pan, dolly) and
//! camera/light wireframes.

use effectcraft_engine::effects::puppet::PinKind;
use effectcraft_engine::geom::{Mat3, vec2 as gv2};
use effectcraft_engine::project::{Comp, Layer, LayerId};
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
    /// pointer (corners both axes, edges one), relative to the layer as the drag began.
    Scale {
        layer: LayerId,
        start_scale: [f64; 3],
        /// The grabbed point relative to the anchor, in layer pixels.
        start_local: [f64; 2],
        anchor: [f64; 2],
        axes: [bool; 2],
        /// Comp → layer pixels as the drag began.
        inv: Mat3,
    },
    Rotate {
        layer: LayerId,
        center: Pos2,
        start_angle: f64,
        start_rot: f64,
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
    /// Puppet tool Record mode (⌘/Ctrl-drag a pin): the drag is sampled in real time (seconds
    /// since it began, layer space) and becomes keyframes on release (`puppet.recordPin`); the
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

/// Layer → comp matrix and its bounds quad (in comp pixels).
pub(crate) fn layer_quad(ctx: &EvalCtx, layer: &Layer) -> Option<(Mat3, [[f64; 2]; 4], [f64; 4])> {
    let b = effectcraft_engine::render::content_bounds(ctx, layer)?;
    let (m, _) = l2c(ctx, layer);
    let pts = [[b[0], b[1]], [b[2], b[1]], [b[2], b[3]], [b[0], b[3]]].map(|p| {
        let q = m.apply(gv2(p[0], p[1]));
        [q.x, q.y]
    });
    Some((m, pts, b))
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

/// The scale that puts the point grabbed at `start` (relative to the anchor, layer pixels)
/// under the pointer at `cur`, for a layer whose scale was `start_scale` when the drag began.
/// `proportional` (Shift on a corner) keeps the aspect ratio, following the pointer along the
/// handle's diagonal. Dragging past the anchor flips the layer, as in After Effects; an axis
/// whose grabbed point sits on the anchor keeps its scale.
fn drag_scale(start_scale: [f64; 3], start: [f64; 2], cur: [f64; 2], axes: [bool; 2], proportional: bool) -> [f64; 3] {
    let ratio = |k: usize| if axes[k] && start[k].abs() > 1e-3 { cur[k] / start[k] } else { 1.0 };
    let (mut fx, mut fy) = (ratio(0), ratio(1));
    if proportional && axes == [true, true] {
        let d2 = start[0] * start[0] + start[1] * start[1];
        let f = if d2 > 1e-6 { (cur[0] * start[0] + cur[1] * start[1]) / d2 } else { 1.0 };
        (fx, fy) = (f, f);
    }
    [start_scale[0] * fx, start_scale[1] * fy, start_scale[2]]
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
    let (main, aux): (Rect, Vec<(Rect, View3D)>) = if n == 2 {
        let mid = full.center().x;
        (Rect::from_min_max(pos2(mid + gap / 2.0, full.min.y), full.max), vec![(Rect::from_min_max(full.min, pos2(mid - gap / 2.0, full.max.y)), View3D::Top)])
    } else {
        let c = full.center();
        let q = |x0: f32, y0: f32, x1: f32, y1: f32| Rect::from_min_max(pos2(x0, y0), pos2(x1, y1));
        (
            q(c.x + gap / 2.0, c.y + gap / 2.0, full.max.x, full.max.y),
            vec![
                (q(full.min.x, full.min.y, c.x - gap / 2.0, c.y - gap / 2.0), View3D::Top),
                (q(c.x + gap / 2.0, full.min.y, full.max.x, c.y - gap / 2.0), View3D::Front),
                (q(full.min.x, c.y + gap / 2.0, c.x - gap / 2.0, full.max.y), View3D::Right),
            ],
        )
    };
    let ctx = ui.ctx().clone();
    let p = ui.painter().clone();
    p.rect_filled(full, 0.0, Color32::from_black_alpha(200));
    let t = app.session.time();
    let (cw, ch) = (comp.width as f32, comp.height as f32);
    let views = app.session.state.views3d.get(&cid).cloned().unwrap_or_default();
    let share = app.session.state.share_view_options;
    for (i, (r, v)) in aux.into_iter().enumerate() {
        p.rect_filled(r, 0.0, bg);
        let fit = ((r.width() - 20.0) / cw).min((r.height() - 20.0) / ch).max(0.01);
        let cr = Rect::from_center_size(r.center(), vec2(cw * fit, ch * fit));
        let scale = (fit * ctx.pixels_per_point()).min(1.0) as f64;
        let cam = views.cam(v, comp.width as f64, comp.height as f64).state();
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
                let opts = effectcraft_engine::render::RenderOpts { scale, view: comp.has_3d().then_some(cam), draft: true, ..Default::default() };
                let img = app.session.render(cid, t, opts);
                if !crate::frames::presentation_check(&ctx, [img.width as usize, img.height as usize], &mut app.ui.status) {
                    continue;
                }
                let tex = ctx.load_texture(format!("viewer-aux-{i}"), crate::frames::to_color_image(&img), egui::TextureOptions::LINEAR);
                ctx.data_mut(|d| d.insert_temp(id, (key, tex.clone())));
                tex
            }
        };
        if !crate::frames::presentation_check(&ctx, tex.size(), &mut app.ui.status) {
            continue;
        }
        let b = comp.background;
        p.rect_filled(cr, 0.0, Color32::from_rgb((b[0] * 255.0) as u8, (b[1] * 255.0) as u8, (b[2] * 255.0) as u8));
        p.image(tex.id(), cr, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        p.rect_stroke(cr, 0.0, Stroke::new(1.0, Color32::from_black_alpha(160)), StrokeKind::Outside);
        if share && app.ui.viewer.safe_margins {
            for k in [0.9, 0.8] {
                p.rect_stroke(Rect::from_center_size(cr.center(), cr.size() * k), 0.0, Stroke::new(1.0, Color32::from_white_alpha(120)), StrokeKind::Middle);
            }
        }
        let label = if comp.has_3d() { v.label() } else { "Active Camera" };
        p.text(r.left_bottom() + vec2(8.0, -8.0), Align2::LEFT_BOTTOM, label, Tokens::ui(11.0), Color32::from_white_alpha(200));
        app.auto.add(&format!("viewer.view.{}", v.id()), r, label);
    }
    main
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
    let key = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (app.session.revision, lv.comp.0, t.0, scale.to_bits(), format!("{cam:?}{}", dc.is_some())).hash(&mut h);
        h.finish()
    };
    let id = egui::Id::new("viewer-locked");
    let cached: Option<(u64, egui::TextureHandle)> = ctx.data(|d| d.get_temp(id));
    let tex = match cached {
        Some((k, tex)) if k == key => Some(tex),
        _ => {
            let opts = effectcraft_engine::render::RenderOpts { scale, view: cam, ..Default::default() };
            let img = app.session.render(lv.comp, t, opts);
            if crate::frames::presentation_check(&ctx, [img.width as usize, img.height as usize], &mut app.ui.status) {
                let mut ci = crate::frames::to_color_image(&img);
                if let Some(dc) = dc {
                    let mut px: Vec<[u8; 4]> = ci.pixels.iter().map(|c| c.to_array()).collect();
                    dc.apply(&mut px);
                    ci = egui::ColorImage::new(ci.size, px.into_iter().map(|a| Color32::from_rgba_premultiplied(a[0], a[1], a[2], a[3])).collect());
                }
                let tex = ctx.load_texture("viewer-locked", ci, egui::TextureOptions::LINEAR);
                ctx.data_mut(|d| d.insert_temp(id, (key, tex.clone())));
                Some(tex)
            } else {
                None
            }
        }
    };

    let b = comp.background;
    p.rect_filled(cr, 0.0, Color32::from_rgb((b[0] * 255.0) as u8, (b[1] * 255.0) as u8, (b[2] * 255.0) as u8));
    if let Some(tex) = tex
        && crate::frames::presentation_check(&ctx, tex.size(), &mut app.ui.status)
    {
        p.image(tex.id(), cr, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    }
    p.rect_stroke(cr, 0.0, Stroke::new(1.0, Color32::from_black_alpha(160)), StrokeKind::Outside);
    let label = format!("Locked: {name} \u{2014} {}", if comp.has_3d() { lv.view.label() } else { "Active Camera" });
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
                Color32::from_rgb(0x34, 0x3c, 0x4e)
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
    let bar_h = 30.0;
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
    ctx.data_mut(|d| d.insert_temp(egui::Id::new("viewer-fit"), fit));
    let zoom = app.ui.viewer.zoom.unwrap_or(fit);
    let center = area.center() + vec2(app.ui.viewer.pan[0], app.ui.viewer.pan[1]);
    let origin = center - vec2(cw * zoom / 2.0, ch * zoom / 2.0);
    let map = ViewerMap { origin, zoom, comp: [cw, ch], area };
    ctx.data_mut(|d| d.insert_temp(map_id(), map));
    let comp_rect = Rect::from_min_size(origin, vec2(cw * zoom, ch * zoom));
    let painter = p.with_clip_rect(area);
    // Settings ▸ 3D ▸ Extended Viewer: custom 3D views (and Draft 3D) render the visible
    // pasteboard too, so 3D layers reaching past the comp frame stay visible there.
    app.ui.viewer.extended = extended_region(app, &comp, cid, map);

    // Frame.
    let ppp = ctx.pixels_per_point();
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
    if let Some(img) = app.frames.get(&key) {
        let stale = app.viewer_shown.as_ref().is_none_or(|(_, k)| *k != key);
        if stale {
            show_frame(app, &ctx, key, img);
        } else if let Some((_, k)) = app.viewer_shown.as_mut() {
            // The same frame at a later revision (an edit elsewhere, an undo): it shows that one.
            k.revision = key.revision;
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
    if let Some((tex, shown)) = &app.viewer_tex
        && app.viewer_shown.is_some_and(|(id, _)| id == tex.id())
        && let Err(error) = crate::frames::presentation_size(&ctx, tex.size())
    {
        app.ui.status = error.message().to_owned();
        app.frames.reject_presentation(*shown, error);
        app.viewer_shown = None;
        app.viewer_tex = None;
        app.viewer_image = None;
    }
    let retained = app.viewer_shown.as_ref().is_some_and(|(_, shown)| crate::frames::same_view(&key, shown));
    vt::draw_frame(app, &ctx, &painter, comp_rect, cid, &ectx, &key);
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
                let x = comp_rect.min.x + g.position as f32 * zoom;
                styled_line(&painter, [pos2(x, area.min.y), pos2(x, area.max.y)], stroke, &style);
            } else {
                let y = comp_rect.min.y + g.position as f32 * zoom;
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
    let mut handle_hits: Vec<(LayerId, usize, Pos2)> = Vec::new();
    let mut vertex_hits: Vec<VertexHit> = Vec::new();
    let mut key_hits: Vec<ov::KeyHit> = Vec::new();
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
        for l in comp.layers.iter().filter(|l| selected.contains(&l.id) && l.is_active_at(time)) {
            let col = if label_handles { t.label(l.label) } else { t.accent };
            // Motion path of animated position (keyframes and spatial tangents are draggable).
            ov::motion_path(app, &painter, &map, &ectx, l, col, &mut key_hits);
            // Masks and shape paths.
            if app.ui.viewer.show_masks {
                let pen = ui.data(|d| d.get_temp::<(LayerId, u64)>(pen_id()));
                let hover = ui.input(|i| i.pointer.hover_pos());
                ov::draw_paths(app, &painter, &map, &ectx, l, col, &sel_vertices, pen, hover, &mut vertex_hits, &mut paths);
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
                handle_hits.push((l.id, i, hp));
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
                    let o = pc.apply(a3);
                    let len = 60.0 / zoom as f64;
                    for (axis, col) in [
                        (effectcraft_engine::geom::vec3(len, 0.0, 0.0), Color32::from_rgb(0xe0, 0x50, 0x50)),
                        (effectcraft_engine::geom::vec3(0.0, len, 0.0), Color32::from_rgb(0x60, 0xd0, 0x60)),
                        (effectcraft_engine::geom::vec3(0.0, 0.0, len), Color32::from_rgb(0x50, 0x8c, 0xf0)),
                    ] {
                        let e = pc.apply(a3 + axis);
                        painter.arrow(map.to_screen([o.x, o.y]), map.to_screen([e.x, e.y]) - map.to_screen([o.x, o.y]), Stroke::new(2.0, col));
                    }
                }
            }
        }
    }

    // Settings ▸ 3D: Extended Viewer (3D layers outlined beyond the comp frame) and Show 3D
    // Reference Axes (world X/Y/Z in the corner, turning with the view).
    let has_3d = comp.layers.iter().any(|l| l.is_3d() && l.is_active_at(time));
    if has_3d && app.session.prefs.three_d.extended_viewer {
        for l in comp.layers.iter().filter(|l| l.is_3d() && l.is_active_at(time) && l.has_video()) {
            if let Some((_, q, _)) = layer_quad(&ectx, l) {
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

    super::expr_editor::error_banner(app, ui, area);

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

    // Free transform box of a path (double-click a path with the Selection tool).
    let ft = ov::draw_free_transform(app, &ctx, &painter, &map, &paths);

    // Cameras and lights (wireframes) and the 3D view name.
    if comp.has_3d() {
        draw_rigs(app, &painter, &map, &ectx, &selected);
        let view = app.session.state.views3d.get(&cid).map(|v| v.current).unwrap_or_default();
        let label = match (view, comp.active_camera(time)) {
            (View3D::ActiveCamera, Some(c)) => format!("Active Camera ({})", c.name),
            (v, _) => v.label().to_string(),
        };
        painter.text(area.left_top() + vec2(12.0, 10.0), Align2::LEFT_TOP, label, Tokens::ui(12.0), t.text_dim);
    }

    // Puppet pins and meshes of the selected layers (Puppet tools).
    let mut pin_hits: Vec<super::puppet_tool::PinHit> = vec![];
    if app.ui.tool.puppet_kind().is_some() {
        let sel_props: Vec<u64> = app.session.state.selected_props.iter().map(|(_, u)| *u).collect();
        for lid in &selected {
            if let Some(l) = comp.layer(*lid)
                && let Some(ov) = super::puppet_tool::overlay(app, &ctx, cid, l, time)
            {
                let hover = ui.input(|i| i.pointer.hover_pos());
                pin_hits.extend(super::puppet_tool::draw(&painter, &map, &ectx, l, &ov, app.session.state.puppet.show_mesh, &sel_props, hover));
            }
        }
        for h in &pin_hits {
            app.auto.add(&format!("viewer.puppetPin.{}", h.pin), Rect::from_center_size(h.pos, vec2(10.0, 10.0)), "Puppet pin");
            if let Some(hp) = h.handle {
                let ring = h.pos + (h.pos - hp);
                app.auto.add(&format!("viewer.puppetPin.{}.rotate", h.pin), Rect::from_center_size(ring, vec2(6.0, 6.0)), "Rotate puppet pin");
                app.auto.add(&format!("viewer.puppetPin.{}.scale", h.pin), Rect::from_center_size(hp, vec2(6.0, 6.0)), "Scale puppet pin");
            }
        }
    }

    // Project items, files and effects dropped on the viewer.
    super::viewer_drop::show(app, ui, &painter, &map, &ectx);

    // Interaction.
    let resp = ui.interact(area, egui::Id::new("viewer-interact"), Sense::click_and_drag());
    preview_failure_banner(app, ui, &painter, area, &key, retained);
    let menu_hits_id = egui::Id::new(("viewer-menu-hits", cid.0));
    if resp.secondary_clicked()
        && let Some(pos) = resp.interact_pointer_pos()
    {
        // Picking an already selected layer preserves the group; an unselected hit
        // becomes the target, using the same visible/unlocked hit test as left-click.
        pick(app, &ectx, map.to_comp(pos), false);
        let point = map.to_comp(pos);
        let hits: Vec<(u64, String)> = selectable_layers(&comp, time)
            .filter(|l| layer_quad(&ectx, l).is_some_and(|(_, q, _)| point_in_quad(point, &q)))
            .map(|l| (l.id.0, l.name.clone()))
            .collect();
        ui.data_mut(|d| d.insert_temp(menu_hits_id, hits));
    }
    let context_layers: Vec<u64> =
        app.session.state.selected_layers.iter().filter(|id| comp.layer(**id).is_some_and(|l| !l.switches.locked)).map(|id| id.0).collect();
    let mut context_actions = Vec::new();
    resp.context_menu(|ui| {
        let hits = ui.data(|d| d.get_temp::<Vec<(u64, String)>>(menu_hits_id)).unwrap_or_default();
        if !hits.is_empty() {
            ui.menu_button("Select", |ui| {
                for (id, name) in &hits {
                    if ui.button(name).clicked() {
                        context_actions.push(("layer.select".to_owned(), serde_json::json!({"layers":[id]})));
                        ui.close();
                    }
                }
            });
            ui.separator();
        }
        if context_layers.is_empty() {
            ui.label("No unlocked layer selected");
        } else {
            super::timeline::layer_menu(ui, &context_layers, &mut context_actions);
        }
    });
    for (id, params) in context_actions {
        if let Err(e) = crate::menus::invoke(app, &ctx, &id, params) {
            app.ui.status = e;
        }
    }
    let gid = egui::Id::new("viewer-gesture");
    if let Some(hp) = resp.hover_pos() {
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
            t if t.is_shape() || t.is_pen() => egui::CursorIcon::Crosshair,
            _ if app.ui.viewer.roi_draw => egui::CursorIcon::Crosshair,
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
                } else if handle_hits.iter().any(|(_, _, h)| h.distance(hp) < HANDLE) {
                    egui::CursorIcon::ResizeNwSe
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
    let space_pan = ui.input(|i| i.key_down(egui::Key::Space)) && !ctx.egui_wants_keyboard_input();
    // Pen tool: each press places a vertex (dragging pulls Bezier tangents); pressing on the
    // first vertex closes the path.
    if app.ui.tool == Tool::Pen
        && !space_pan
        && resp.hovered()
        && ui.input(|i| i.pointer.primary_pressed())
        && let Some(pos) = ui.input(|i| i.pointer.press_origin())
    {
        pen_press(app, ui, &ectx, &map, &paths, pos, mods);
    }
    // A pen click (press + release without a drag) ends its tangent gesture on release.
    if ui.input(|i| i.pointer.primary_released())
        && matches!(ui.data(|d| d.get_temp::<Gesture>(egui::Id::new("viewer-gesture"))), Some(Gesture::Pen { .. }))
        && !resp.drag_stopped()
    {
        ui.data_mut(|d| d.remove::<Gesture>(egui::Id::new("viewer-gesture")));
    }
    let vertex_at = |pos: Pos2, tangents: bool| -> Option<VertexHit> {
        vertex_hits.iter().rev().filter(|h| h.tangent.is_some() == tangents).find(|h| h.pos.distance(pos) < 6.0).copied()
    };
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
                let layer = comp.layer(l).cloned();
                let center = layer
                    .as_ref()
                    .and_then(|l| l.transform().map(|tr| ectx.v3(l, tr, "position", [0.0; 3])))
                    .map(|p| map.to_screen([p[0], p[1]]))
                    .unwrap_or(pos);
                let rot = layer.as_ref().and_then(|l| l.transform().map(|tr| ectx.f(l, tr, "rotation", 0.0))).unwrap_or(0.0);
                Gesture::Rotate { layer: l, center, start_angle: ((pos.y - center.y) as f64).atan2((pos.x - center.x) as f64).to_degrees(), start_rot: rot }
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
                    own: effectcraft_engine::viewer::layer_targets(&ectx, layer, false),
                    anchor_only: mods.alt,
                })
            }),
            t if t.is_shape() => Some(Gesture::Create { tool: t, start: cpt }),
            t if t.puppet_kind().is_some() && super::puppet_tool::hit(&pin_hits, press).is_none() => {
                // Off the pins: Alt or a press outside the art drags a marquee that selects pins.
                (mods.alt || pick(app, &ectx, cpt, false).is_none()).then_some(Gesture::PinMarquee { start: press })
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
                let inv = l2c(&ectx, l).0.inverse()?;
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
                if let Some((lid, hi, _)) = handle_hits.iter().find(|(_, _, h)| h.distance(press) < HANDLE + 2.0).cloned() {
                    let layer = comp.layer(lid).cloned();
                    layer.and_then(|layer| {
                        let tr = layer.transform()?;
                        let (l2c, _) = l2c(&ectx, &layer);
                        let inv = l2c.inverse()?;
                        let a = ectx.v3(&layer, tr, "anchor", [0.0; 3]);
                        let lp = inv.apply(gv2(cpt[0], cpt[1]));
                        Some(Gesture::Scale {
                            layer: lid,
                            start_scale: ectx.v3(&layer, tr, "scale", [100.0; 3]),
                            start_local: [lp.x - a[0], lp.y - a[1]],
                            anchor: [a[0], a[1]],
                            axes: handle_axes(hi),
                            inv,
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
                }
                for (lid, p0, ax, ay) in layers {
                    let v = json!([p0[0] + ax[0] * d[0] + ay[0] * d[1], p0[1] + ax[1] * d[0] + ay[1] * d[1], p0[2] + ax[2] * d[0] + ay[2] * d[1]]);
                    let _ = app.session.execute("prop.set", json!({"layer": lid.0, "path": "transform/position", "value": v, "merge": merge}));
                }
            }
            Gesture::Scale { layer, start_scale, start_local, anchor, axes, inv } => {
                let lp = inv.apply(gv2(cpt[0], cpt[1]));
                let v = drag_scale(start_scale, start_local, [lp.x - anchor[0], lp.y - anchor[1]], axes, mods.shift);
                let _ = app.session.execute("prop.set", json!({"layer": layer.0, "path": "transform/scale", "value": v, "merge": merge}));
            }
            Gesture::Rotate { layer, center, start_angle, start_rot } => {
                let ang = ((pos.y - center.y) as f64).atan2((pos.x - center.x) as f64).to_degrees();
                let mut r = start_rot + (ang - start_angle);
                if mods.shift {
                    r = (r / 45.0).round() * 45.0;
                }
                let _ = app.session.execute("prop.set", json!({"layer": layer.0, "path": "transform/rotation", "value": r, "merge": merge}));
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
            Gesture::Create { start, .. } => {
                let a = map.to_screen(start);
                let r = Rect::from_two_pos(a, pos);
                painter.rect_stroke(r, 0.0, Stroke::new(1.0, t.accent), StrokeKind::Middle);
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
                painter.text(pos + vec2(10.0, -10.0), Align2::LEFT_BOTTOM, "Recording", Tokens::ui(11.0), Color32::from_rgb(0xff, 0x60, 0x60));
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
                    if let Err(e) = app.session.execute("puppet.recordPin", q) {
                        app.ui.status = e.to_string();
                    }
                }
                Gesture::Create { tool, start } => {
                    let end = map.to_comp(pos);
                    create_shape(app, tool, start, end, mods.shift);
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
            Tool::PenAdd => {
                if let Some((l, m, seg, tt)) = ov::segment_at(&paths, &map, pos, 6.0) {
                    let _ = app.session.execute("mask.insertVertex", json!({"layer": l.0, "mask": m, "segment": seg, "t": tt}));
                }
            }
            Tool::PenDelete => {
                if let Some(h) = vertex_at(pos, false) {
                    let _ = app.session.execute("mask.deleteVertices", json!({"vertices": [{"layer": h.layer.0, "mask": h.mask, "index": h.index}]}));
                }
            }
            Tool::PenConvert => {
                if let Some(h) = vertex_at(pos, false) {
                    let _ = app.session.execute("mask.convertVertex", json!({"layer": h.layer.0, "mask": h.mask, "index": h.index}));
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
            t if t.puppet_kind().is_some() => {
                if let Some((h, _)) = super::puppet_tool::hit(&pin_hits, pos) {
                    // Click selects the pin; Shift-click adds or removes it.
                    let _ = app.session.execute("puppet.selectPins", json!({"layer": h.layer.0, "pins": [h.pin], "toggle": mods.shift}));
                } else if let Some(l) = pick(app, &ectx, cpt, false).and_then(|l| comp.layer(l))
                    && let Some(inv) = l2c(&ectx, l).0.inverse()
                {
                    let lp = inv.apply(gv2(cpt[0], cpt[1]));
                    let r = app.session.execute("puppet.addPin", json!({"layer": l.id.0, "kind": t.puppet_kind(), "position": [lp.x, lp.y]}));
                    if let Err(e) = r {
                        app.ui.status = e.to_string();
                    }
                }
            }
            Tool::Selection if vertex_at(pos, false).is_some() => {
                if let Some(h) = vertex_at(pos, false) {
                    let me = json!({"layer": h.layer.0, "mask": h.mask, "index": h.index});
                    let _ = app.session.execute("mask.selectVertices", json!({"vertices": [me], "add": mods.shift, "toggle": mods.shift}));
                }
            }
            _ => {
                if pick(app, &ectx, cpt, mods.shift).is_none() {
                    let _ = app.session.execute("edit.deselectAll", json!({}));
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
        // Double-click a path: free transform its points (Layer ▸ Mask and Shape Path ▸ Free
        // Transform Points).
        ov::begin_free_transform(&ctx, l, m);
    } else if resp.double_clicked()
        && !text_owns
        && !app.ui.tool.is_pen()
        && let Some(pos) = resp.interact_pointer_pos()
    {
        let cpt = map.to_comp(pos);
        if let Some(l) = pick(app, &ectx, cpt, false).and_then(|l| comp.layer(l))
            && let effectcraft_engine::project::LayerSource::Comp { item } = &l.source
        {
            app.session.open_comp(*item);
        }
    }
    // 3D Camera Tracker points, targets and their menu (on top of the viewer's gestures while
    // the effect is selected), and its analysis banner.
    super::camera_tracker_ui::viewer_hook(app, ui, &painter, &map, &ectx, &|c, l| l2c(c, l).0);
    // Effect point controls and crosshair/eyedropper picks (on top of the viewer's gestures).
    crate::panels::effect_controls::viewer_hook(app, ui, &painter, &map, &ectx, &|c, l| l2c(c, l).0);

    // Status chips (render time / caching).
    if app.playback.playing && app.playback.waiting {
        let r = Rect::from_min_size(area.min + vec2(10.0, 10.0), vec2(150.0, 22.0));
        painter.rect_filled(r, 11.0, Color32::from_black_alpha(170));
        painter.text(r.center(), Align2::CENTER_CENTER, "Caching frames…", Tokens::ui(11.5), t.cache_green);
    }

    vt::draw_snap(&ctx, &painter, &map, &ectx);
    vt::rulers(app, ui, &map, outer, area);
    vt::bottom_bar(app, ui, bar, zoom, fit, time, &comp);
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
fn draw_rigs(app: &mut EffectcraftApp, painter: &egui::Painter, map: &ViewerMap, ectx: &EvalCtx, selected: &[LayerId]) {
    use effectcraft_engine::geom::Vec3;
    use effectcraft_engine::project::{LayerSource, LightKind};
    let cam = cam_state(ectx);
    let (w, h) = (ectx.comp.width as f64, ectx.comp.height as f64);
    let active = ectx.comp.active_camera(ectx.time).map(|c| c.id);
    let through_active = VIEW_CAM.with(|c| c.get()).is_none();
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
                app.auto.add(&format!("viewer.camera.{}", l.id.0), Rect::from_center_size(s, vec2(14.0, 14.0)), &l.name);
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
            app.auto.add(&format!("viewer.light.{}", l.id.0), Rect::from_center_size(s, vec2(16.0, 16.0)), &l.name);
        }
    }
}

/// Pen tool press at `pos`: continue the path in progress (pressing its first vertex closes it),
/// add a vertex on a visible path's segment, or start a new path: a mask on the selected
/// footage/solid layer, a shape path (Shape n group with Fill and Stroke) on the selected shape
/// layer, or a new shape layer when nothing is selected. The placed point snaps.
fn pen_press(app: &mut EffectcraftApp, ui: &mut egui::Ui, ectx: &EvalCtx, map: &ViewerMap, paths: &[ov::PathInfo], pos: Pos2, mods: egui::Modifiers) {
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
        let lp = inv.apply(gv2(c[0], c[1]));
        let lp = [lp.x, lp.y];
        if let Ok(v) = app.session.execute("mask.addVertex", json!({"layer": lid.0, "mask": uid, "point": lp})) {
            let index = v.as_u64().unwrap_or(0) as usize;
            ui.data_mut(|d| d.insert_temp(gid, Gesture::Pen { layer: lid, mask: uid, index, vertex: lp, inv, press: pos }));
        }
        return;
    }
    // On a visible path's segment the Pen adds a vertex there.
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
        Some(l) if !matches!(l.source, LayerSource::Shape) => {
            let Some(inv) = l2c(ectx, l).0.inverse() else { return };
            let lp = inv.apply(gv2(c[0], c[1]));
            let Ok(v) = app.session.execute("mask.new", json!({"layer": l.id.0, "vertices": [[lp.x, lp.y]]})) else { return };
            let Some(mask) = v["mask"].as_u64() else { return };
            (l.id, mask, inv)
        }
        Some(l) => {
            let Some(inv) = l2c(ectx, l).0.inverse() else { return };
            let lp = inv.apply(gv2(c[0], c[1]));
            let fill = app.ui.fill_color;
            let stroke = app.ui.stroke_color;
            let r = app.session.execute(
                "shape.newPath",
                json!({"layer": l.id.0, "vertices": [[lp.x, lp.y]], "space": "layer", "fill": [fill[0], fill[1], fill[2]], "stroke": [stroke[0], stroke[1], stroke[2]], "strokeWidth": app.ui.stroke_width}),
            );
            let Some(path) = r.ok().and_then(|v| v["path"].as_u64()) else { return };
            (l.id, path, inv)
        }
        None => {
            // A new shape layer at the comp centre (unrotated: comp → layer is a translation).
            let fill = app.ui.fill_color;
            let stroke = app.ui.stroke_color;
            let r = app.session.execute(
                "shape.newPath",
                json!({"vertices": [c], "space": "comp", "fill": [fill[0], fill[1], fill[2]], "stroke": [stroke[0], stroke[1], stroke[2]], "strokeWidth": app.ui.stroke_width}),
            );
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

/// Visible, unlocked layers eligible for viewer hit tests, in front-to-back order.
/// Solo scope matches the renderer: only active AV layers activate it, even when hidden or locked.
pub(crate) fn selectable_layers(comp: &Comp, time: Tick) -> impl Iterator<Item = &Layer> {
    let any_solo = comp.layers.iter().any(|l| l.switches.solo && l.source.is_av() && l.is_active_at(time));
    comp.layers
        .iter()
        .filter(move |l| l.is_active_at(time) && l.switches.video && !l.switches.locked && !l.is_camera() && !l.is_light() && (!any_solo || l.switches.solo))
}

/// Topmost selectable layer under a comp point.
pub(crate) fn layer_at<'a>(ectx: &EvalCtx<'a>, cpt: [f64; 2]) -> Option<&'a Layer> {
    selectable_layers(ectx.comp, ectx.time).find(|l| layer_quad(ectx, l).is_some_and(|(_, q, _)| point_in_quad(cpt, &q)))
}

/// Topmost layer under a comp point (selects it; shift toggles). Returns the hit layer.
pub(crate) fn pick(app: &mut EffectcraftApp, ectx: &EvalCtx, cpt: [f64; 2], toggle: bool) -> Option<LayerId> {
    let hit = layer_at(ectx, cpt).map(|l| l.id)?;
    if toggle {
        let _ = app.session.execute("layer.select", json!({"layers": [hit.0], "toggle": true}));
    } else if !app.session.state.selected_layers.contains(&hit) {
        let _ = app.session.execute("layer.select", json!({"layers": [hit.0]}));
    }
    Some(hit)
}

fn create_shape(app: &mut EffectcraftApp, tool: Tool, a: [f64; 2], b: [f64; 2], square: bool) {
    let mut w = (b[0] - a[0]).abs();
    let mut h = (b[1] - a[1]).abs();
    if w < 2.0 && h < 2.0 {
        return;
    }
    if square {
        let m = w.max(h);
        w = m;
        h = m;
    }
    let cx = a[0].min(b[0]) + w / 2.0;
    let cy = a[1].min(b[1]) + h / 2.0;
    let kind = match tool {
        Tool::Rectangle => "rect",
        Tool::RoundedRect => "rounded",
        Tool::Ellipse => "ellipse",
        Tool::Polygon => "polygon",
        _ => "star",
    };
    // A non-shape layer selected and "creates mask": add a mask instead.
    let sel = app.session.state.selected_layers.first().copied();
    let sel_layer = sel.and_then(|id| app.session.active_comp().and_then(|c| c.layer(id)).cloned());
    if let Some(l) = sel_layer
        && !matches!(l.source, effectcraft_engine::project::LayerSource::Shape)
        && l.source.is_av()
        && matches!(kind, "rect" | "ellipse")
    {
        // Mask in layer space: invert the layer transform.
        let comp = app.session.active_comp_arc();
        if let Some(comp) = comp {
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
            let _ = app.session.execute(
                "layer.addMask",
                json!({"layer": l.id.0, "shape": if kind == "ellipse" { "ellipse" } else { "rect" }, "rect": [p0.x.min(p1.x), p0.y.min(p1.y), (p1.x - p0.x).abs(), (p1.y - p0.y).abs()]}),
            );
            return;
        }
    }
    let fill = app.ui.fill_color;
    let stroke = app.ui.stroke_color;
    let _ = app.session.execute(
        "layer.newShape",
        json!({"kind": kind, "size": [w, h], "position": [cx, cy], "fill": [fill[0], fill[1], fill[2]], "stroke": [stroke[0], stroke[1], stroke[2]], "strokeWidth": app.ui.stroke_width}),
    );
}

fn empty_state(app: &mut EffectcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let p = ui.painter();
    p.rect_filled(rect, 0.0, t.pasteboard);
    let c = rect.center();
    p.text(c - vec2(0.0, 40.0), Align2::CENTER_CENTER, "Composition", Tokens::semibold(16.0), t.text);
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

/// Texture filtering for Viewer Zoom Quality: More Accurate = bilinear, Faster = nearest.
pub(crate) fn zoom_texture_options(smooth: bool) -> egui::TextureOptions {
    if smooth { egui::TextureOptions::LINEAR } else { egui::TextureOptions::NEAREST }
}

pub(crate) fn hex_rgb(s: &str) -> Option<[u8; 3]> {
    let h = s.trim().trim_start_matches('#');
    if h.len() != 6 || !h.is_ascii() {
        return None;
    }
    let p = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    Some([p(0)?, p(2)?, p(4)?])
}

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

/// Put a rendered frame on screen: CPU pixels go into an egui texture; GPU frames are drawn
/// straight from their wgpu texture (registered with egui-wgpu, no readback).
fn show_frame(app: &mut EffectcraftApp, ctx: &egui::Context, key: crate::frames::FrameKey, img: crate::frames::FrameImage) {
    use crate::frames::FrameImage;
    // Settings ▸ Previews ▸ Viewer Zoom Quality.
    let smooth = app.session.prefs.viewer_zoom_smooth();
    let opts = zoom_texture_options(smooth);
    match img {
        FrameImage::Cpu(img) => {
            if let Err(error) = crate::frames::presentation_image(ctx, &img) {
                app.frames.reject_presentation(key, error);
                return;
            }
            match &mut app.viewer_tex {
                Some((tex, k)) if tex.size() == img.size => {
                    tex.set((*img).clone(), opts);
                    *k = key;
                }
                _ => app.viewer_tex = Some((ctx.load_texture("viewer-frame", (*img).clone(), opts), key)),
            }
            app.viewer_shown = app.viewer_tex.as_ref().map(|(t, k)| (t.id(), *k));
            app.viewer_image = Some(img);
            vt::set_texture_roi(ctx, app.session.state.region_of_interest.or(app.ui.viewer.extended));
        }
        FrameImage::Gpu(f) => {
            let Some(rs) = &app.wgpu else { return };
            let view = f.texture.create_view(&Default::default());
            let mut ren = rs.renderer.write();
            let filter = if smooth { eframe::wgpu::FilterMode::Linear } else { eframe::wgpu::FilterMode::Nearest };
            let id = match &app.viewer_native {
                Some((id, _, _)) => {
                    ren.update_egui_texture_from_wgpu_texture(&rs.device, &view, filter, *id);
                    *id
                }
                None => ren.register_native_texture(&rs.device, &view, filter),
            };
            drop(ren);
            app.viewer_native = Some((id, key, f));
            app.viewer_shown = Some((id, key));
            // Pixels are read back on demand (viewer_pixels).
            app.viewer_image = None;
        }
    }
}

fn preview_failure_rects(area: Rect) -> Option<(Rect, Rect)> {
    let width = area.width();
    let height = area.height();
    if ![area.min.x, area.min.y, area.max.x, area.max.y, width, height].iter().all(|v| v.is_finite()) || width <= 8.0 || height <= 8.0 {
        return None;
    }
    let inner = area.shrink(4.0);
    let banner = Rect::from_min_size(inner.min, vec2(inner.width(), inner.height().min(24.0)));
    let retry = Rect::from_min_max(pos2((banner.max.x - banner.width().min(58.0)).max(banner.min.x), banner.min.y), banner.max);
    // Finite large coordinates can also lose the small button width to f32 rounding.
    if retry.width() <= 0.0 || retry.height() <= 0.0 || !area.contains_rect(banner) || !banner.contains_rect(retry) {
        return None;
    }
    Some((banner, retry))
}

fn preview_failure_banner(app: &mut EffectcraftApp, ui: &mut egui::Ui, painter: &egui::Painter, area: Rect, key: &crate::frames::FrameKey, retained: bool) {
    let Some(failure) = app.frames.failure(key) else { return };
    let Some((banner, retry)) = preview_failure_rects(area) else { return };
    let t = app.tokens;
    painter.rect_filled(banner, 2.0, t.panel_bg);
    let outdated = retained && app.viewer_shown.as_ref().is_some_and(|(_, shown)| *shown != *key);
    let label = if outdated { "Preview outdated — rendering failed" } else { "Preview rendering failed" };
    painter.with_clip_rect(Rect::from_min_max(banner.min, pos2(retry.min.x, banner.max.y))).text(
        banner.left_center() + vec2(5.0, 0.0),
        Align2::LEFT_CENTER,
        label,
        Tokens::ui(11.0),
        t.warning,
    );
    app.auto.add("viewer.preview.error", banner, label);
    let response = ui.interact(retry, egui::Id::new("preview-retry"), Sense::click()).on_hover_text(failure.message());
    let button_painter = ui.painter().with_clip_rect(ui.clip_rect().intersect(retry));
    button_painter.rect_filled(retry, 2.0, if response.hovered() { t.pressed } else { t.hover });
    button_painter.text(retry.center(), Align2::CENTER_CENTER, "Retry", Tokens::medium(12.0), t.text);
    app.auto.add("viewer.preview.retry", retry, "Retry preview rendering");
    if response.clicked() {
        app.frames.retry_failed_previews();
    }
}

#[cfg(test)]
mod preview_failure_ui_tests {
    use super::*;
    use crate::frames::{CheckedRemoteDone, FailureKind, PreviewFailure, RemoteDone, RemoteFrame, RemoteFrames, RemoteJob};
    use egui_kittest::Harness;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    // Headless tests inspect real output deltas but have no texture renderer.
    // Clear them even when an assertion unwinds, as required by egui 0.36.
    struct TestOutput(egui::FullOutput);
    impl std::ops::Deref for TestOutput {
        type Target = egui::FullOutput;
        fn deref(&self) -> &Self::Target {
            &self.0
        }
    }
    impl Drop for TestOutput {
        fn drop(&mut self) {
            self.0.textures_delta.clear();
        }
    }

    #[test]
    fn cpu_install_rechecks_live_limit_and_never_marks_rejected_pixels_shown() {
        let ctx = egui::Context::default();
        let mut app = EffectcraftApp::new(effectcraft_engine::Session::default());
        let key = crate::frames::FrameKey { revision: 1, content: 1, comp: 1, frame: 0, scale: 1000, view: 0, opts: 0 };
        let image = Arc::new(egui::ColorImage::new([1025, 1], vec![egui::Color32::WHITE; 1025]));
        let first = TestOutput(ctx.run_ui(egui::RawInput { max_texture_side: Some(2048), ..Default::default() }, |ui| {
            let ctx = ui.ctx();
            show_frame(&mut app, ctx, key, crate::frames::FrameImage::Cpu(image.clone()));
        }));
        assert!(app.viewer_shown.is_some());
        let id = app.viewer_tex.as_ref().unwrap().0.id();
        assert!(first.textures_delta.set.iter().any(|(texture, _)| *texture == id));
        let rejected = TestOutput(ctx.run_ui(egui::RawInput { max_texture_side: Some(1024), ..Default::default() }, |ui| {
            let ctx = ui.ctx();
            show_frame(&mut app, ctx, key, crate::frames::FrameImage::Cpu(image.clone()));
        }));
        assert!(!rejected.textures_delta.set.iter().any(|(texture, _)| *texture == id));
        assert_eq!(app.frames.failure(&key).unwrap().kind, FailureKind::Retryable);
        // Failed installation preserves the prior identity; normal paint suppresses it when
        // its own dimensions no longer fit. No new texture or successful key is published.
        assert_eq!(app.viewer_tex.as_ref().unwrap().0.id(), id);
        app.frames.retry_failed_previews();
        let recovered = TestOutput(ctx.run_ui(egui::RawInput { max_texture_side: Some(2048), ..Default::default() }, |ui| {
            let ctx = ui.ctx();
            show_frame(&mut app, ctx, key, crate::frames::FrameImage::Cpu(image.clone()));
        }));
        assert!(recovered.textures_delta.set.iter().any(|(texture, _)| *texture == id));
    }

    #[test]
    fn malformed_display_pixels_are_rejected_before_transform_without_new_texture() {
        let mut s = effectcraft_engine::Session::default();
        s.execute("comp.new", json!({"width":32,"height":32,"duration":1})).unwrap();
        s.state.viewer.exposure = 1.0;
        let project = s.project.clone();
        let cid = s.active_comp_id().unwrap();
        let comp = project.comp(cid).unwrap();
        let ectx = EvalCtx { project: &project, comp_id: cid, comp, time: Tick::ZERO, expr: None, footage: None };
        let mut app = EffectcraftApp::new(s);
        let key = app.frame_key(cid, 0, 1.0);
        let ctx = egui::Context::default();
        ctx.run_ui(egui::RawInput::default(), |ui| {
            let ctx = ui.ctx();
            show_frame(&mut app, ctx, key, crate::frames::FrameImage::Cpu(Arc::new(egui::ColorImage::new([1, 1], vec![Color32::WHITE]))));
        })
        .drop_without_applying_deltas();
        let old = app.viewer_shown.unwrap();
        app.viewer_image = Some(Arc::new(egui::ColorImage { size: [2, 1], pixels: vec![Color32::WHITE], source_size: vec2(2.0, 1.0) }));
        let revision = app.session.revision;
        let output = TestOutput(ctx.run_ui(egui::RawInput::default(), |root_ui| {
            let ctx = root_ui.ctx().clone();
            egui::CentralPanel::default().show(root_ui, |ui| {
                vt::draw_frame(&mut app, &ctx, ui.painter(), Rect::from_min_size(pos2(0.0, 0.0), vec2(320.0, 320.0)), cid, &ectx, &key);
            });
        }));
        assert!(app.ui.status.contains("pixel count"));
        assert!(ctx.data(|d| d.get_temp::<(u64, egui::TextureHandle)>(egui::Id::new("viewer-display-tex"))).is_none());
        assert!(!output.shapes.iter().any(|s| draws_texture(&s.shape, old.0)));
        assert_eq!(app.session.revision, revision);
        assert_eq!(app.viewer_shown.unwrap(), old, "rejected display transform does not publish replacement identity");
    }
    #[test]
    fn pending_new_main_key_keeps_old_limit_error_visible_without_poisoning_request() {
        struct Pending;
        impl RemoteFrames for Pending {
            fn slots(&self) -> usize {
                1
            }
            fn start(&self, _: RemoteJob, _: RemoteDone) {}
        }
        let mut s = effectcraft_engine::Session::default();
        s.execute("comp.new", json!({"width":32,"height":32,"duration":1})).unwrap();
        let cid = s.active_comp_id().unwrap();
        let mut app = EffectcraftApp::new(s);
        app.ui.viewer.res = crate::state::Resolution::Full;
        app.frames.set_remote(Some(Arc::new(Pending)));
        let old = app.frame_key(cid, 0, 1.0);
        let ctx = egui::Context::default();
        crate::theme::install(&ctx, &app.tokens);
        ctx.run_ui(egui::RawInput { max_texture_side: Some(2048), ..Default::default() }, |ui| {
            let ctx = ui.ctx();
            app.frames.set_context(ctx);
            show_frame(&mut app, ctx, old, crate::frames::FrameImage::Cpu(Arc::new(egui::ColorImage::new([1025, 1], vec![Color32::WHITE; 1025]))));
        })
        .drop_without_applying_deltas();
        let texture = app.viewer_shown.unwrap().0;
        app.session.execute("layer.newSolid", json!({"width":16,"height":16})).unwrap();
        let requested = app.frame_key(cid, 0, 1.0);
        assert_ne!(old, requested);
        let output = TestOutput(ctx.run_ui(egui::RawInput { max_texture_side: Some(1024), ..Default::default() }, |root_ui| {
            let ctx = root_ui.ctx().clone();
            app.frames.set_context(&ctx);
            egui::CentralPanel::default().show(root_ui, |ui| {
                show(&mut app, ui, Rect::from_min_size(pos2(0.0, 0.0), vec2(640.0, 480.0)));
            });
        }));
        assert!(app.frames.failure(&old).is_some());
        assert!(app.frames.failure(&requested).is_none(), "pending new content does not inherit old image rejection");
        assert!(app.ui.status.contains("preview texture limit"));
        assert!(app.viewer_shown.is_none());
        assert!(!output.shapes.iter().any(|s| draws_texture(&s.shape, texture)));
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct RequestIdentity {
        revision: u64,
        comp: u64,
        tick: i64,
        opts_hash: u64,
        scale_bits: u64,
    }
    struct Remote {
        fail: AtomicBool,
        calls: AtomicUsize,
        requests: std::sync::Mutex<Vec<RequestIdentity>>,
    }
    impl RemoteFrames for Remote {
        fn slots(&self) -> usize {
            1
        }
        fn start(&self, _: RemoteJob, _: RemoteDone) {
            panic!("expected typed completion");
        }
        fn start_checked(&self, job: RemoteJob, done: CheckedRemoteDone) {
            let identity = RequestIdentity {
                revision: job.revision,
                comp: job.comp.0,
                tick: job.t.0,
                opts_hash: crate::frames::opts_hash(&job.opts),
                scale_bits: job.opts.scale.to_bits(),
            };
            {
                let mut requests = self.requests.lock().unwrap();
                assert!(requests.len() < 128, "bounded fixture request trace overflow");
                requests.push(identity);
            }
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fail.load(Ordering::SeqCst) {
                done(Err(PreviewFailure::new(FailureKind::Content, "synthetic bounded renderer failure")));
            } else {
                done(Ok(RemoteFrame { width: 2, height: 2, rgba: [20, 40, 60, 255].repeat(4), ms: 1.0 }));
            }
        }
    }
    fn harness() -> (Harness<'static, EffectcraftApp>, Arc<Remote>, u64) {
        let mut s = effectcraft_engine::Session::default();
        let other = s.execute("comp.new", json!({"name": "Other", "width": 32, "height": 32, "duration": 1})).unwrap()["comp"].as_u64().unwrap();
        s.execute("comp.new", json!({"name": "Main", "width": 32, "height": 32, "duration": 1})).unwrap();
        let remote = Arc::new(Remote { fail: AtomicBool::new(false), calls: AtomicUsize::new(0), requests: std::sync::Mutex::new(Vec::with_capacity(128)) });
        let installed = remote.clone();
        let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_| {
            let mut app = EffectcraftApp::new(s);
            app.frames.set_remote(Some(installed));
            app
        });
        h.run_steps(4);
        h.state().frames.dispatch_remote();
        h.run_steps(3);
        assert!(h.state().viewer_shown.is_some(), "success installed real viewer texture");
        (h, remote, other)
    }
    fn draws_texture(shape: &egui::Shape, id: egui::TextureId) -> bool {
        match shape {
            egui::Shape::Mesh(mesh) => mesh.texture_id == id,
            egui::Shape::Vec(shapes) => shapes.iter().any(|s| draws_texture(s, id)),
            _ => false,
        }
    }
    #[test]
    fn failed_edit_keeps_same_view_pixels_visibly_outdated_and_retry_installs_success() {
        let (mut h, remote, _) = harness();
        let old = h.state().viewer_shown.unwrap();
        remote.fail.store(true, Ordering::SeqCst);
        h.state_mut().session.execute("layer.newSolid", json!({"color": "#ff8030"})).unwrap();
        let opts = h.state().frame_opts(effectcraft_project::ItemId(old.1.comp), 1.0);
        let failed_request = RequestIdentity {
            revision: h.state().session.revision,
            comp: old.1.comp,
            tick: 0,
            opts_hash: crate::frames::opts_hash(&opts),
            scale_bits: opts.scale.to_bits(),
        };
        h.step();
        h.state().frames.dispatch_remote();
        h.run_steps(3);
        assert_eq!(h.state().viewer_shown.unwrap(), old, "no failed pixels installed or marked current");
        assert!(h.output().shapes.iter().any(|s| draws_texture(&s.shape, old.0)), "last valid texture remains actually painted");
        assert_eq!(h.state().auto.find("viewer.preview.error").unwrap().label, "Preview outdated — rendering failed");
        let before_repaints = remote.requests.lock().unwrap().clone();
        assert_eq!(before_repaints.iter().filter(|request| **request == failed_request).count(), 1, "the desired edited frame failed exactly once");
        for _ in 0..20 {
            h.step();
            h.state().frames.dispatch_remote();
        }
        let after_repaints = remote.requests.lock().unwrap().clone();
        assert_eq!(after_repaints.iter().filter(|request| **request == failed_request).count(), 1, "repaint cannot repeat the exact failed admission");
        let prefetches = &after_repaints[before_repaints.len()..];
        assert!(!prefetches.is_empty(), "paused lookahead still admits independent frames");
        assert!(
            prefetches.iter().enumerate().all(|(index, request)| !before_repaints.contains(request) && !prefetches[..index].contains(request)),
            "no other content-failed frame is redispatched during repaint"
        );
        assert!(
            prefetches.iter().all(|request| {
                request.revision == failed_request.revision
                    && request.comp == failed_request.comp
                    && request.tick > failed_request.tick
                    && request.opts_hash == failed_request.opts_hash
                    && request.scale_bits == failed_request.scale_bits
            }),
            "additional repaint requests belong only to distinct later frames in the same view"
        );
        remote.fail.store(false, Ordering::SeqCst);
        let e = h.state().auto.find("viewer.preview.retry").unwrap().clone();
        let pos = egui::pos2(e.rect[0] + e.rect[2] * 0.5, e.rect[1] + e.rect[3] * 0.5);
        h.input_mut().events.extend([
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE },
            egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE },
        ]);
        h.step();
        h.step();
        h.state().frames.dispatch_remote();
        h.run_steps(3);
        assert_eq!(
            remote.requests.lock().unwrap().iter().filter(|request| **request == failed_request).count(),
            2,
            "manual Retry redispatches the exact failed frame once"
        );
        assert_ne!(h.state().viewer_shown.unwrap().1, old.1);
        assert_eq!(
            h.state().viewer_shown.unwrap().1,
            h.state().frame_key(effectcraft_project::ItemId(failed_request.comp), 0, 1.0),
            "the successful current frame is installed"
        );
        assert!(h.output().shapes.iter().any(|s| draws_texture(&s.shape, h.state().viewer_shown.unwrap().0)), "successful retry pixels are actually painted");
        assert!(h.state().auto.find("viewer.preview.error").is_none());
    }
    #[test]
    fn failed_other_comp_never_paints_the_old_compositions_texture() {
        let (mut h, remote, other) = harness();
        let old = h.state().viewer_shown.unwrap();
        remote.fail.store(true, Ordering::SeqCst);
        h.state_mut().session.execute("comp.open", json!({"comp": other})).unwrap();
        h.step();
        h.state().frames.dispatch_remote();
        h.run_steps(3);
        assert!(h.state().viewer_shown.is_none_or(|(_, k)| k.comp != other));
        assert!(!h.output().shapes.iter().any(|s| draws_texture(&s.shape, old.0)), "different comp never paints retained old pixels");
        assert_eq!(h.state().auto.find("viewer.preview.error").unwrap().label, "Preview rendering failed");
    }
    #[test]
    fn failure_banner_hit_targets_stay_inside_tiny_or_empty_viewers() {
        for [w, h] in [[0.0, 0.0], [4.0, 4.0], [9.0, 9.0], [20.0, 12.0], [80.0, 30.0], [200.0, 80.0]] {
            let area = Rect::from_min_size(pos2(10.0, 20.0), vec2(w, h));
            if let Some((banner, retry)) = preview_failure_rects(area) {
                for rect in [banner, retry] {
                    assert!(rect.width() > 0.0 && rect.height() > 0.0);
                    assert!(area.contains_rect(rect));
                    assert!([rect.min.x, rect.min.y, rect.max.x, rect.max.y].iter().all(|v| v.is_finite()));
                }
            } else {
                assert!(w <= 8.0 || h <= 8.0);
            }
        }
        for area in [
            Rect::from_min_max(pos2(-f32::MAX, 0.0), pos2(f32::MAX, 30.0)),
            Rect::from_min_max(pos2(0.0, -f32::MAX), pos2(200.0, f32::MAX)),
            Rect::from_min_max(pos2(f32::NAN, 0.0), pos2(200.0, 30.0)),
            Rect::from_min_max(pos2(0.0, 0.0), pos2(f32::INFINITY, 30.0)),
            Rect::from_min_max(pos2(1e30, 0.0), pos2(1.001e30, 30.0)),
        ] {
            assert!(preview_failure_rects(area).is_none(), "hostile/precision-lost bounds must not register malformed controls: {area:?}");
        }
    }
    #[test]
    fn wireframe_without_any_cached_frame_still_paints_layer_outlines() {
        let mut s = effectcraft_engine::Session::default();
        s.execute("comp.new", json!({"width": 32, "height": 32, "duration": 1})).unwrap();
        s.execute("layer.newSolid", json!({"width": 16, "height": 16})).unwrap();
        s.state.viewer.fast_previews = effectcraft_engine::commands::viewer_cmds::FastPreviews::Wireframe;
        let project = s.project.clone();
        let cid = s.active_comp_id().unwrap();
        let comp = project.comp(cid).unwrap();
        let ectx = EvalCtx { project: &project, comp_id: cid, comp, time: Tick::ZERO, expr: None, footage: None };
        let mut app = EffectcraftApp::new(s);
        assert!(app.viewer_shown.is_none());
        let key = app.frame_key(cid, 0, 1.0);
        let ctx = egui::Context::default();
        let output = TestOutput(ctx.run_ui(egui::RawInput::default(), |root_ui| {
            let ctx = root_ui.ctx().clone();
            egui::CentralPanel::default().show(root_ui, |ui| {
                vt::draw_frame(&mut app, &ctx, ui.painter(), Rect::from_min_size(pos2(0.0, 0.0), vec2(320.0, 320.0)), cid, &ectx, &key);
            });
        }));
        assert!(
            output.shapes.iter().any(|s| matches!(&s.shape, egui::Shape::Path(p) if p.closed && p.points.len() == 4)),
            "wireframe is independent of cached raster success"
        );
        assert!(app.viewer_shown.is_none());
    }
}
