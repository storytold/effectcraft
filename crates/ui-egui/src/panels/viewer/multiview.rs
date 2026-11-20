//! The panes of View ▸ Switch View Layout, as in After Effects: in a comp with 3D layers every pane
//! of a 2- or 4-view layout takes the press (in a 2D comp only the active pane does, as before).
//! The viewer's pipeline runs once, in the active pane, through that pane's view; the other panes
//! are drawn by `view::aux_views`. A press in an inactive pane makes it the active pane before the
//! pipeline runs ([`pre_activate`]), and the pipeline then takes the same press there: egui gives
//! a press to the widget id that was under the pointer in the previous pass, and each layout
//! position has its own id ([`input_id`]), held by the inactive pane's sensor and, once the pane
//! is active, by the viewer's own interaction there. Each position keeps its own picture framing
//! ([`sync_framing`]).

use effectcraft_engine::project::ItemId;
use effectcraft_engine::render::EvalCtx;
use effectcraft_engine::render::three_d::{CameraState, Views3D};
use egui::{Color32, Rect, Stroke, StrokeKind, pos2, vec2};
use serde_json::json;

use super::{VIEW_CAM, ViewerMap, axis_arrows, gv2, layer_outline, layer_quad, snap_to_pixels};
use crate::EffectcraftApp;
use crate::state::PaneFrame;

/// Space between neighbouring panes, in points.
const GAP: f32 = 2.0;

/// The input id of layout position `pos`: the sensor of the pane while it is inactive, the
/// viewer's own interaction while it is active.
pub(super) fn input_id(pos: usize) -> egui::Id {
    egui::Id::unique(("viewer-interact", pos))
}

/// The active layout position of comp `cid`'s viewer. A comp with no 3D layer keeps the viewer as
/// it was: the bottom-right pane takes the input, and a position activated while the comp had 3D
/// layers is kept for when they come back.
pub(super) fn active_pos(app: &EffectcraftApp, cid: ItemId) -> usize {
    if !app.session.project.comp(cid).is_some_and(|c| c.has_3d()) {
        return Views3D::HOME;
    }
    app.session.state.views3d.get(&cid).map_or(Views3D::HOME, Views3D::active)
}

/// The positions the layout of `views` (1, 2 or 4) shows in comp `cid`: one view shows the active
/// position.
pub(super) fn shown_positions(app: &EffectcraftApp, cid: ItemId, views: u8) -> Vec<usize> {
    if views <= 1 {
        return vec![active_pos(app, cid)];
    }
    app.session.state.views3d.get(&cid).map_or_else(|| Views3D::default().shown(views), |v| v.shown(views))
}

/// Where layout positions `shown` sit in `full` for a `layout` of 1, 2 or 4 views: one view fills
/// it; two put position 0 on the left half and 3 on the right; four put 0, 1, 2 and 3 in the
/// top-left, top-right, bottom-left and bottom-right quarters. Neighbours are [`GAP`] apart.
pub(super) fn layout_rects(full: Rect, layout: u8, shown: &[usize]) -> Vec<(usize, Rect)> {
    let (c, g) = (full.center(), GAP / 2.0);
    let (left, right, top, bottom) = (c.x - g, c.x + g, c.y - g, c.y + g);
    let rect = |pos: usize| match (layout, pos) {
        (0 | 1, _) => full,
        (2, 0) => Rect::from_min_max(full.min, pos2(left, full.max.y)),
        (2, _) => Rect::from_min_max(pos2(right, full.min.y), full.max),
        (_, 0) => Rect::from_min_max(full.min, pos2(left, top)),
        (_, 1) => Rect::from_min_max(pos2(right, full.min.y), pos2(full.max.x, top)),
        (_, 2) => Rect::from_min_max(pos2(full.min.x, bottom), pos2(left, full.max.y)),
        _ => Rect::from_min_max(pos2(right, bottom), full.max),
    };
    shown.iter().map(|&pos| (pos, rect(pos))).collect()
}

/// The panes one viewer pass laid out, read by the next pass.
#[derive(Clone, Debug)]
struct Panes {
    comp: u64,
    /// The egui pass that laid them out (`cumulative_pass_nr`).
    pass: u64,
    active: usize,
    /// Every shown pane: its layout position and its rectangle.
    panes: Vec<(usize, Rect)>,
    /// The inactive panes' corner controls: a press there is theirs, not the pane's.
    corners: Vec<Rect>,
}

fn panes_id() -> egui::Id {
    egui::Id::unique("viewer-panes")
}

/// The panes the viewer laid out in this pass or the one before. Older ones are nobody's: the
/// viewer is not drawn any more (closed, or another tab in its dock slot), and egui hands a
/// press only to widgets of the last pass.
fn published(ctx: &egui::Context) -> Option<Panes> {
    let now = ctx.cumulative_pass_nr();
    ctx.data(|d| d.get_temp::<Panes>(panes_id())).filter(|p| now.saturating_sub(p.pass) <= 1)
}

fn drawn_id() -> egui::Id {
    egui::Id::unique("viewer-panes-drawn")
}

/// Whether the viewer laid out its panes (it does in every pass it is drawn) since the last
/// call, which resets it: the main window asks once per pass of its own, after its panels. Not
/// by pass number: egui numbers each window's passes apart, and a Composition panel floated into
/// a window of its own draws in that window's passes, which run inside the main window's.
pub(super) fn take_drawn(ctx: &egui::Context) -> bool {
    ctx.data_mut(|d| d.remove_temp::<bool>(drawn_id())).unwrap_or(false)
}

/// Record this pass's panes of comp `comp` (see [`Panes`]), and that the viewer was drawn
/// ([`take_drawn`]).
pub(super) fn publish(ctx: &egui::Context, comp: ItemId, active: usize, panes: Vec<(usize, Rect)>, corners: Vec<Rect>) {
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|d| {
        d.insert_temp(panes_id(), Panes { comp: comp.0, pass, active, panes, corners });
        d.insert_temp(drawn_id(), true);
    });
}

/// A press in an inactive pane of the viewer drawn in `viewer` makes it the active pane before the
/// viewer's pipeline runs, so the pipeline, now in that pane and through its view, takes the same
/// press: it selects or drags there, and a middle-click only activates (After Effects). A press
/// on the pane's corner controls is theirs, one over a popup or a window above the viewer is not
/// the viewer's, and a second button pressed during a drag belongs to that drag. A comp with no
/// 3D layer keeps the viewer as it was: only the active pane takes input.
pub(super) fn pre_activate(app: &mut EffectcraftApp, ui: &egui::Ui, cid: ItemId, viewer: Rect) {
    if !app.session.project.comp(cid).is_some_and(|c| c.has_3d()) {
        return;
    }
    let ctx = ui.ctx();
    let press = ui.input(|i| {
        i.events.iter().find_map(|e| match e {
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary | egui::PointerButton::Secondary | egui::PointerButton::Middle,
                pressed: true,
                ..
            } => Some(*pos),
            _ => None,
        })
    });
    let (Some(at), Some(last)) = (press.filter(|p| viewer.contains(*p)), published(ctx)) else { return };
    let Some(pos) = last.panes.iter().find(|(_, r)| r.contains(at)).map(|(pos, _)| *pos) else { return };
    let shown = shown_positions(app, cid, app.session.state.view_layout.max(1));
    let inactive = pos != last.active && pos != active_pos(app, cid) && shown.contains(&pos);
    let ours = ui.is_enabled() && ctx.layer_id_at(at) == Some(ui.layer_id()) && !last.corners.iter().any(|r| r.contains(at));
    if last.comp != cid.0 || !inactive || !ours || ctx.dragged_id().is_some() {
        return;
    }
    if let Err(e) = app.session.execute("view.activatePane", json!({"position": pos, "comp": cid.0})) {
        app.ui.status = e.to_string();
    }
}

/// Each layout position of a 2- or 4-view layout keeps its own picture framing: when the active
/// position or the comp changed since the last pass, the framing in `zoom` and `pan` is stored for
/// the position it belonged to, in any layout, and in a 2- or 4-view layout the active position's
/// comes out. One view keeps its framing, as before (whichever position it shows).
pub(super) fn sync_framing(app: &mut EffectcraftApp, cid: ItemId) {
    let now = (cid.0, active_pos(app, cid));
    let several = app.session.state.view_layout > 1;
    let v = &mut app.ui.viewer;
    if let Some((_, was)) = v.framed.filter(|f| *f != now) {
        if let Some(slot) = v.pane_frames.get_mut(was) {
            *slot = PaneFrame { zoom: v.zoom, pan: v.pan };
        }
        if several {
            let next = v.pane_frames.get(now.1).copied().unwrap_or_default();
            v.zoom = next.zoom;
            v.pan = next.pan;
        }
    }
    v.framed = Some(now);
}

/// The picture's mapping in `area` for a comp of `comp` (width, height) pixels: fitted with 20
/// points around it (Fit up to 100% stops at 100%), at `frame`'s magnification and pan, and
/// the fit magnification. The active pane and the inactive ones are framed alike, on the area
/// the rulers leave them, so a pane's picture stays put when it becomes active.
pub(super) fn fit_map(area: Rect, comp: [f32; 2], frame: PaneFrame, fit_up_to_100: bool, ppp: f32) -> (ViewerMap, f32) {
    let [cw, ch] = comp;
    let fit = ((area.width() - 40.0) / cw).min((area.height() - 40.0) / ch).max(0.01);
    // Fit up to 100% follows the panel's size like Fit does, and stops at 100%.
    let fit = if fit_up_to_100 { fit.min(1.0 / ppp) } else { fit };
    let zoom = frame.zoom.unwrap_or(fit);
    let center = area.center() + vec2(frame.pan[0], frame.pan[1]);
    // On the physical pixel grid, so at 100 % every comp pixel covers exactly one screen pixel
    // instead of being blended with its neighbour (#882).
    let origin = snap_to_pixels(center - vec2(cw * zoom / 2.0, ch * zoom / 2.0), ppp);
    (ViewerMap { origin, zoom, comp, area }, fit)
}

/// The selected layers' outlines, anchor points and axis gizmos in the inactive pane at layout
/// position `pos`, through that pane's view (`view`; None = the comp's camera), so a layer or an
/// arrow can be aimed at before the press that activates the pane. Registers
/// `viewer.pane.{pos}.anchor.{layer}` and `viewer.pane.{pos}.gizmo.{layer}.{x|y|z}` where they
/// show inside the pane.
pub(super) fn draw_selection(app: &mut EffectcraftApp, painter: &egui::Painter, map: &ViewerMap, ectx: &EvalCtx, pos: usize, view: Option<CameraState>) {
    if !app.layer_controls_visible() {
        return;
    }
    let t = app.tokens;
    let label_handles = app.session.prefs.appearance.use_label_color_for_handles;
    let selected = app.session.state.selected_layers.clone();
    // The pane's camera while drawing it; the viewer's own camera after.
    let viewer_cam = VIEW_CAM.with(|c| c.replace(view));
    for l in ectx.comp.layers.iter().filter(|l| selected.contains(&l.id) && l.is_active_at(ectx.time) && (l.switches.video || l.source.is_av())) {
        let col = if label_handles { t.label(l.label) } else { t.accent };
        let Some((m, q, _)) = layer_quad(ectx, l) else {
            // A 3D layer reaching past the camera: the part of its box in front of it.
            let outline = layer_outline(ectx, l);
            if outline.len() > 2 {
                painter.add(egui::Shape::closed_line(outline.iter().map(|p| map.to_screen(*p)).collect(), Stroke::new(1.0, col)));
            }
            continue;
        };
        painter.add(egui::Shape::closed_line(q.iter().map(|p| map.to_screen(*p)).collect(), Stroke::new(1.0, col)));
        let Some(tr) = l.transform() else { continue };
        let a = ectx.v3(l, tr, "anchor", [0.0; 3]);
        let c = m.apply(gv2(a[0], a[1]));
        let s = map.to_screen([c.x, c.y]);
        painter.circle_stroke(s, 5.0, Stroke::new(1.0, Color32::WHITE));
        painter.line_segment([s - vec2(9.0, 0.0), s + vec2(9.0, 0.0)], Stroke::new(1.0, Color32::WHITE));
        painter.line_segment([s - vec2(0.0, 9.0), s + vec2(0.0, 9.0)], Stroke::new(1.0, Color32::WHITE));
        if map.area.contains(s) {
            app.auto.add(&format!("viewer.pane.{pos}.anchor.{}", l.id.0), Rect::from_center_size(s, vec2(12.0, 12.0)), "anchor point");
        }
        if !l.is_3d() {
            continue;
        }
        for g in axis_arrows(ectx, l, a, map) {
            painter.arrow(g.tail, g.tip - g.tail, Stroke::new(2.0, g.color));
            if !l.switches.locked && map.area.contains(g.tip) {
                app.auto.add(&format!("viewer.pane.{pos}.gizmo.{}.{}", l.id.0, g.name), Rect::from_center_size(g.tip, vec2(12.0, 12.0)), "3D axis");
            }
        }
    }
    VIEW_CAM.with(|c| c.set(viewer_cam));
}

/// The picture that stands in for the frame on screen while that frame is another position's:
/// right after a pane became active, until its own frame is shown (the frame on screen still is
/// the previously active pane's, another view). `Some(picture)`: draw that, the pane's last side
/// pane picture (`None`: it has none); `None`: the frame on screen is this pane's.
pub(super) fn stand_in(app: &EffectcraftApp, ctx: &egui::Context, cid: ItemId) -> Option<Option<egui::TextureId>> {
    let active = active_pos(app, cid);
    let (comp, pos) = app.ui.viewer.shown_for?;
    // Only a picture of the active pane's own view: another view's would show Top while a press
    // acts through Left (the pane was switched, and its frame is still rendering).
    (comp == cid.0 && pos != active).then(|| super::view::side_picture(ctx, cid, active, crate::app::viewer_frames::view_key(app.session.view_camera(cid))))
}

/// The active pane's thin accent border while the layout shows more than one pane of comp `cid`,
/// a comp with 3D layers (a 2D comp's viewer has none, as before). Drawn last, over the pane's
/// picture and overlays.
pub(super) fn active_border(app: &EffectcraftApp, ctx: &egui::Context, painter: &egui::Painter, cid: ItemId) {
    if !app.session.project.comp(cid).is_some_and(|c| c.has_3d()) {
        return;
    }
    let Some(shown) = published(ctx).filter(|s| s.comp == cid.0 && s.pass == ctx.cumulative_pass_nr() && s.panes.len() > 1) else { return };
    if let Some((_, r)) = shown.panes.iter().find(|(pos, _)| *pos == shown.active) {
        painter.rect_stroke(*r, 0.0, Stroke::new(1.5, app.tokens.accent), StrokeKind::Inside);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_rects_place_each_position() {
        let full = Rect::from_min_max(pos2(0.0, 0.0), pos2(102.0, 52.0));
        assert_eq!(layout_rects(full, 1, &[2]), vec![(2, full)], "one view fills the viewer, whatever position is active");
        let two = layout_rects(full, 2, &[0, 3]);
        assert_eq!(two, vec![(0, Rect::from_min_max(pos2(0.0, 0.0), pos2(50.0, 52.0))), (3, Rect::from_min_max(pos2(52.0, 0.0), pos2(102.0, 52.0)))]);
        let four = layout_rects(full, 4, &[0, 1, 2, 3]);
        let want = [
            Rect::from_min_max(pos2(0.0, 0.0), pos2(50.0, 25.0)),
            Rect::from_min_max(pos2(52.0, 0.0), pos2(102.0, 25.0)),
            Rect::from_min_max(pos2(0.0, 27.0), pos2(50.0, 52.0)),
            Rect::from_min_max(pos2(52.0, 27.0), pos2(102.0, 52.0)),
        ];
        assert_eq!(four, want.iter().copied().enumerate().collect::<Vec<_>>());
    }

    #[test]
    fn fit_map_fits_with_a_margin_then_follows_the_frame() {
        let area = Rect::from_min_size(pos2(10.0, 20.0), vec2(840.0, 400.0));
        let (map, fit) = fit_map(area, [640.0, 360.0], PaneFrame::default(), false, 1.0);
        assert_eq!(fit, 1.0, "(400 - 40) / 360");
        assert_eq!((map.zoom, map.origin), (1.0, pos2(110.0, 40.0)), "centred");
        let (map, _) = fit_map(area, [640.0, 360.0], PaneFrame { zoom: Some(0.5), pan: [3.1, -7.0] }, false, 2.0);
        assert_eq!((map.zoom, map.origin), (0.5, pos2(273.0, 123.0)), "the frame's zoom and pan, on the pixel grid");
        assert_eq!(fit_map(area, [640.0, 360.0], PaneFrame::default(), true, 2.0).1, 0.5, "Fit up to 100% stops at 100%");
    }

    /// The panes a pass laid out count in that pass and the next (where a press is routed by
    /// them), not once the viewer has stopped drawing.
    #[test]
    fn published_panes_expire_after_the_next_pass() {
        let ctx = egui::Context::default();
        let run = |ctx: &egui::Context, publish_now: bool| {
            let mut seen = None;
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                if publish_now {
                    publish(ui.ctx(), ItemId(7), 3, vec![(3, Rect::from_min_size(pos2(0.0, 0.0), vec2(10.0, 10.0)))], vec![]);
                }
                seen = published(ui.ctx()).map(|p| p.comp);
            });
            out.textures_delta.clear();
            seen
        };
        assert_eq!(run(&ctx, true), Some(7));
        assert_eq!(run(&ctx, false), Some(7), "the next pass routes its press by them");
        assert_eq!(run(&ctx, false), None, "not drawn for a pass: gone");
    }

    /// The viewer of a Composition panel floated into a window of its own draws in that window's
    /// passes (an egui viewport, which numbers its passes apart from the main window's). The
    /// check after the main window's panels must still see it drawn and keep its side panes'
    /// frames: they lost them on every frame, and in a full cache each landed frame went at once.
    #[test]
    fn a_viewer_in_a_floating_window_keeps_its_side_panes_frames() {
        use crate::frames::{FrameKey, RemoteDone, RemoteFrame, RemoteFrames, RemoteJob};
        /// A frame renderer that hands every frame back at once.
        struct Lands;
        impl RemoteFrames for Lands {
            fn slots(&self) -> usize {
                1
            }
            fn start(&self, _: RemoteJob, done: RemoteDone) {
                done(Ok(RemoteFrame { width: 2, height: 2, rgba: vec![0; 16], ms: 0.0 }));
            }
        }
        let mut app = EffectcraftApp::new(effectcraft_engine::Session::default());
        app.frames.set_remote(Some(std::sync::Arc::new(Lands)));
        let ctx = egui::Context::default();
        let window = egui::ViewportId::from_hash_of("floating-composition-panel");
        let pass = |app: &EffectcraftApp, viewport: egui::ViewportId, viewer_drawn: bool| {
            let mut viewports = egui::ViewportIdMap::default();
            viewports.insert(viewport, egui::ViewportInfo::default());
            let raw = egui::RawInput { viewport_id: viewport, viewports, ..Default::default() };
            let mut out = ctx.run_ui(raw, |ui| {
                if viewer_drawn {
                    publish(ui.ctx(), ItemId(1), 3, vec![(3, Rect::from_min_size(pos2(0.0, 0.0), vec2(10.0, 10.0)))], vec![]);
                }
                // (the main window's draw_inner, after its panels)
                if viewport == egui::ViewportId::ROOT {
                    super::super::upkeep(app, ui.ctx());
                }
            });
            out.textures_delta.clear();
        };
        // The main window has drawn passes of its own: the two windows' pass numbers differ.
        for _ in 0..3 {
            pass(&app, egui::ViewportId::ROOT, false);
        }
        // Side pane 0 waits for its frame, which has landed.
        let k = FrameKey { revision: 1, content: 1, comp: 1, frame: 0, scale: 1000, view: 7, opts: 0 };
        app.frames.request_pane(&app.render_source(), k, ItemId(1), effectcraft_engine::time::Tick::ZERO, Default::default(), 0);
        app.frames.dispatch_remote();
        assert!(app.frames.is_cached(&k));
        let kept = |app: &EffectcraftApp| {
            let budget = app.frames.budget();
            app.frames.set_budget(0);
            let kept = app.frames.is_cached(&k);
            app.frames.set_budget(budget);
            kept
        };
        // The viewer draws in its window, then the main window ends its pass.
        pass(&app, window, true);
        pass(&app, egui::ViewportId::ROOT, false);
        assert!(kept(&app), "the side pane still waits for its frame: an empty budget keeps it");
        // A frame in which the viewer is not drawn anywhere.
        pass(&app, egui::ViewportId::ROOT, false);
        assert!(!kept(&app), "the viewer was not drawn: no side pane waits for a frame");
    }
}
