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

/// The `egui::Id` under which the last pass's `ViewerMap` is published.
fn map_id() -> egui::Id {
    egui::Id::unique("viewer-map")
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
    ctx.data(|d| d.get_temp::<f32>(egui::Id::unique("viewer-fit"))).unwrap_or(0.5)
}

use super::viewer_overlays::{self as ov, VertexHit};
use super::viewer_tools as vt;

thread_local! {
    /// The 3D view camera of the viewer being drawn (None = the comp's active camera).
    static VIEW_CAM: std::cell::Cell<Option<CameraState>> = const { std::cell::Cell::new(None) };
}

/// Draw the Composition viewer in `rect` and handle its input for one pass.
/// Publishes the fit magnification and `ViewerMap`, requests and draws the frame, then paints the nav breadcrumbs,
/// view layout, guides/grid, layer boxes, handles, anchors, 3D gizmos, motion paths, masks, puppet pins and tracker
/// overlays and records their automation regions. Pointer and drop gestures run engine commands
/// (move/scale/rotate/anchor/camera/shape/mask/guide edits, picks, marquees); failures worth showing go to
/// `app.ui.status`, while per-frame drag updates ignore errors so a gesture is not aborted mid-drag. With no active
/// composition it draws `empty_state` and returns.
/// What the passes of one viewer frame share (see [`show`]).
struct Vw<'a> {
    t: Tokens,
    cid: effectcraft_engine::project::ItemId,
    nav: Rect,
    bar: Rect,
    err_bar: Rect,
    err_h: f32,
    pasteboard: Color32,
    area: Rect,
    outer: Rect,
    comp_rect: Rect,
    zoom: f32,
    ppp: f32,
    cw: f32,
    ch: f32,
    map: ViewerMap,
    time: Tick,
    overlays: bool,
    mods: egui::Modifiers,
    space_pan: bool,
    text_owns: bool,
    gid: egui::Id,
    track_cur: Option<(LayerId, effectcraft_engine::project::Uid)>,
    has_3d: bool,
    ctx: egui::Context,
    comp_name: String,
    p: egui::Painter,
    painter: egui::Painter,
    selected: Vec<LayerId>,
    sel_vertices: Vec<effectcraft_engine::VertexRef>,
    ft: Option<ov::FtBox>,
    comp: &'a Comp,
    gesture: Option<Gesture>,
    handle_hits: Vec<(LayerId, usize, Pos2, egui::CursorIcon)>,
    axis_hits: Vec<(LayerId, [Pos2; 2], [f64; 3], [f64; 3])>,
    vertex_hits: Vec<VertexHit>,
    vertex_marks: Vec<ov::VertexMark>,
    key_hits: Vec<ov::KeyHit>,
    motion_frames: Vec<ov::PathFrame>,
    paths: Vec<ov::PathInfo>,
    track_hits: Vec<super::tracker::Hit>,
    pin_hits: Vec<super::puppet_tool::PinHit>,
    puppet_layers: Vec<(&'a Layer, super::puppet_tool::Overlay)>,
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
    let mut v = Vw {
        t,
        cid,
        nav: Rect::NOTHING,
        bar: Rect::NOTHING,
        err_bar: Rect::NOTHING,
        err_h: 0.0,
        pasteboard: Color32::BLACK,
        area: Rect::NOTHING,
        outer: Rect::NOTHING,
        comp_rect: Rect::NOTHING,
        zoom: 1.0,
        ppp: 1.0,
        cw: 0.0,
        ch: 0.0,
        map: ViewerMap { origin: Pos2::ZERO, zoom: 1.0, comp: [0.0; 2], area: Rect::NOTHING },
        time: Tick::ZERO,
        overlays: false,
        mods: egui::Modifiers::NONE,
        space_pan: false,
        text_owns: false,
        gid: egui::Id::unique("viewer-gesture"),
        track_cur: None,
        has_3d: false,
        ctx: ctx.clone(),
        comp_name,
        painter: p.clone(),
        p,
        selected: vec![],
        sel_vertices: vec![],
        ft: None,
        comp: &comp,
        gesture: None,
        handle_hits: vec![],
        axis_hits: vec![],
        vertex_hits: vec![],
        vertex_marks: vec![],
        key_hits: vec![],
        motion_frames: vec![],
        paths: vec![],
        track_hits: vec![],
        pin_hits: vec![],
        puppet_layers: vec![],
    };
    phase_nav(app, ui, rect, &mut v);
    phase_layout(app, ui, rect, &mut v);
    phase_frame(app, &mut v);
    let snap_project = app.session.project.clone();
    let snap_expr = app.session.expr.clone();
    let ectx = EvalCtx { project: &snap_project, comp_id: cid, comp: &comp, time: v.time, expr: snap_expr.as_deref(), footage: None };
    phase_guides(app, &ectx, &mut v);
    phase_layers(app, ui, &ectx, &mut v);
    phase_scene(app, &ectx, &mut v);
    phase_rigs(app, ui, &ectx, &mut v);
    let resp = phase_menu(app, ui, &ectx, &mut v);
    phase_hover(app, ui, &ectx, &resp, &mut v);
    phase_press(app, ui, &ectx, &resp, &mut v);
    phase_dragstart(app, ui, &ectx, &resp, &mut v);
    phase_drag(app, ui, &ectx, &resp, &mut v);
    phase_stopped(app, ui, &ectx, &resp, &mut v);
    phase_clicked(app, &ectx, &resp, &mut v);
    phase_dblclick(app, &ectx, &resp, &mut v);
    phase_tail(app, ui, &ectx, &mut v);
}

fn phase_nav(app: &mut EffectcraftApp, ui: &mut egui::Ui, rect: Rect, v: &mut Vw<'_>) {
    let t = v.t;
    let cid = v.cid;
    let p = v.p.clone();
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
        let resp = ui.interact(r, egui::Id::unique(("nav", oc.0)), Sense::click());
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
    v.nav = nav;
}

fn phase_layout(app: &mut EffectcraftApp, ui: &mut egui::Ui, rect: Rect, v: &mut Vw<'_>) {
    let t = v.t;
    let cid = v.cid;
    let nav = v.nav;
    let ctx = v.ctx.clone();
    let p = v.p.clone();
    let comp = v.comp;
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
    let area = aux_views(app, ui, comp, cid, full, pasteboard);
    p.rect_filled(area, 0.0, pasteboard);
    // View ▸ Show Rulers takes a strip on the top and left.
    let outer = area;
    let area = vt::inset(app, area);

    let (cw, ch) = (comp.width as f32, comp.height as f32);
    let fit = ((area.width() - 40.0) / cw).min((area.height() - 40.0) / ch).max(0.01);
    // Fit up to 100% follows the panel's size like Fit does, and stops at 100%.
    let fit = if app.ui.viewer.fit_up_to_100 { fit.min(1.0 / ctx.pixels_per_point()) } else { fit };
    ctx.data_mut(|d| d.insert_temp(egui::Id::unique("viewer-fit"), fit));
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
    app.ui.viewer.extended = extended_region(app, comp, cid, map);
    v.bar = bar;
    v.err_bar = err_bar;
    v.err_h = err_h;
    v.pasteboard = pasteboard;
    v.area = area;
    v.outer = outer;
    v.comp_rect = comp_rect;
    v.zoom = zoom;
    v.ppp = ppp;
    v.cw = cw;
    v.ch = ch;
    v.map = map;
    v.painter = painter;
}

fn phase_frame(app: &mut EffectcraftApp, v: &mut Vw<'_>) {
    let cid = v.cid;
    let comp_rect = v.comp_rect;
    let zoom = v.zoom;
    let ppp = v.ppp;
    let ctx = v.ctx.clone();
    let painter = v.painter.clone();
    let comp = v.comp;
    // Frame.
    let scale = app.viewer_scale(zoom, ppp);
    if !app.playback.playing {
        // Queue the frame on screen before any prefetch.
        app.request_frame_urgent(cid, comp.frame_rate.frame_at(app.session.time()), scale);
    }
    crate::note_viewer_scale(app, scale);
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
    v.time = time;
}

fn phase_guides(app: &mut EffectcraftApp, ectx: &EvalCtx, v: &mut Vw<'_>) {
    let cid = v.cid;
    let area = v.area;
    let comp_rect = v.comp_rect;
    let zoom = v.zoom;
    let map = v.map;
    let ctx = v.ctx.clone();
    let comp_name = v.comp_name.clone();
    let painter = v.painter.clone();
    let comp = v.comp;
    // The frame (or snapshot) through Show Channel and exposure; ROI frames cover the region.
    vt::draw_frame(app, &ctx, &painter, comp_rect, cid, ectx);
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
    v.overlays = overlays;
}

fn phase_layers(app: &mut EffectcraftApp, ui: &mut egui::Ui, ectx: &EvalCtx, v: &mut Vw<'_>) {
    let t = v.t;
    let zoom = v.zoom;
    let cw = v.cw;
    let ch = v.ch;
    let map = v.map;
    let time = v.time;
    let ctx = v.ctx.clone();
    let painter = v.painter.clone();
    let mut handle_hits = std::mem::take(&mut v.handle_hits);
    let mut axis_hits = std::mem::take(&mut v.axis_hits);
    let mut vertex_hits = std::mem::take(&mut v.vertex_hits);
    let mut vertex_marks = std::mem::take(&mut v.vertex_marks);
    let mut key_hits = std::mem::take(&mut v.key_hits);
    let mut motion_frames = std::mem::take(&mut v.motion_frames);
    let mut paths = std::mem::take(&mut v.paths);
    let comp = v.comp;
    // Overlays + interaction.
    let selected = app.session.state.selected_layers.clone();
    // Bounding-box handles: (layer, handle, screen point, its resize cursor).
    // 3D gizmo arrows: (layer, grabbable screen segment, position change per comp px in x and y).
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
        // Nulls are rig controls: their outlines remain visible without selection (#1642).
        // Keep this separate from selected layers so they gain no handles, anchors or paths. Solo
        // scope matches picking (`selectable_layers`), so a null that can't be clicked isn't drawn.
        let any_solo = comp.layers.iter().any(|l| l.switches.solo && l.source.is_av());
        for l in comp.layers.iter().filter(|l| {
            matches!(l.source, LayerSource::Null) && !selected.contains(&l.id) && l.is_active_at(time) && l.switches.video && (!any_solo || l.switches.solo)
        }) {
            let outline = layer_outline(ectx, l);
            if outline.len() > 2 {
                let col = if label_handles { t.label(l.label) } else { t.accent };
                painter.add(egui::Shape::closed_line(outline.into_iter().map(|p| map.to_screen(p)).collect(), Stroke::new(1.0, col)));
            }
        }
        // The eye hides a layer's pixels, not its controls: a selected AV layer keeps its box,
        // handles, anchor, motion path and mask/shape paths so it stays editable over the visible
        // artwork (#1769). Nulls, cameras and lights have no pixels and disappear from the viewer
        // when hidden (#1332/#1642). Picking still uses `selectable_layers`, so a hidden layer is
        // selected from the Timeline; its controls are interactive once selected.
        for l in comp.layers.iter().filter(|l| selected.contains(&l.id) && l.is_active_at(time) && (l.switches.video || l.source.is_av())) {
            let col = if label_handles { t.label(l.label) } else { t.accent };
            // Motion path of animated position (keyframes and spatial tangents are draggable).
            ov::motion_path(app, &painter, &map, ectx, l, col, &mut key_hits, &mut motion_frames);
            // Masks and shape paths.
            if app.ui.viewer.show_masks {
                let pen = ui.data(|d| d.get_temp::<(LayerId, u64)>(pen_id()));
                let hover = ui.input(|i| i.pointer.hover_pos());
                ov::draw_paths(app, &painter, &map, ectx, l, col, &sel_vertices, pen, hover, &mut vertex_hits, &mut vertex_marks, &mut paths);
            }
            // Settings ▸ Previews ▸ Show Internal Wireframes: outlines of the layers inside a
            // selected collapsed precomp.
            if app.session.prefs.previews.show_internal_wireframes
                && l.switches.collapse
                && let effectcraft_engine::project::LayerSource::Comp { item } = &l.source
                && let Some(nc) = app.session.project.comp(*item)
            {
                let (outer, _) = l2c(ectx, l);
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
            // Path editing keeps vertices and tangents visible without the layer's transform
            // box competing with them (#1220). The path's own free-transform box is separate.
            let editing_path = paths.iter().any(|p| {
                p.layer == l.id && (app.ui.tool.is_pen() || (app.ui.tool == Tool::Selection && sel_vertices.iter().any(|v| v.layer == l.id && v.mask == p.uid)))
            });
            if editing_path {
                continue;
            }
            let Some((m, q, _)) = layer_quad(ectx, l) else {
                // A 3D layer reaching past the camera: the part of its box in front of it.
                let outline = layer_outline(ectx, l);
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
                    let pc = cam_state(ectx).projection(cw as f64, ch as f64) * w;
                    let a3 = effectcraft_engine::geom::vec3(a[0], a[1], a[2]);
                    let len = 60.0 / zoom as f64;
                    let pinv = parent_world_inverse(ectx, l);
                    for (axis, col, name) in [
                        (effectcraft_engine::geom::vec3(len, 0.0, 0.0), Color32::from_rgb(0xe0, 0x50, 0x50), "x"),
                        (effectcraft_engine::geom::vec3(0.0, len, 0.0), Color32::from_rgb(0x60, 0xd0, 0x60), "y"),
                        (effectcraft_engine::geom::vec3(0.0, 0.0, len), Color32::from_rgb(0x50, 0x8c, 0xf0), "z"),
                    ] {
                        // (clipped at the camera's near plane, #572)
                        let Some([o, e]) = project_segment(&pc, a3, a3 + axis) else { continue };
                        // An axis pointing at the camera (Z in the default front view) projects to
                        // a dot: draw it as a short diagonal stub so it can still be grabbed.
                        let mut s = [e[0] - o[0], e[1] - o[1]];
                        if (s[0] * s[0] + s[1] * s[1]).sqrt() * (zoom as f64) < 12.0 {
                            let k = 24.0 / zoom as f64;
                            s = [k, -k];
                        }
                        let (so, se) = (map.to_screen(o), map.to_screen([o[0] + s[0], o[1] + s[1]]));
                        painter.arrow(so, se - so, Stroke::new(2.0, col));
                        if !l.switches.locked {
                            let dir = pinv.apply_vec(w.apply_vec(axis));
                            let (ax, ay) = axis_drag([dir.x, dir.y, dir.z], s);
                            // The inner part stays on the anchor point, which drags the layer freely.
                            axis_hits.push((l.id, [so + (se - so) * 0.35, se], ax, ay));
                            app.auto.add(&format!("viewer.gizmo.{}.{name}", l.id.0), Rect::from_center_size(se, vec2(12.0, 12.0)), "3D axis");
                        }
                    }
                }
            }
        }
    }
    v.handle_hits = handle_hits;
    v.axis_hits = axis_hits;
    v.vertex_hits = vertex_hits;
    v.vertex_marks = vertex_marks;
    v.key_hits = key_hits;
    v.motion_frames = motion_frames;
    v.paths = paths;
    v.selected = selected;
    v.sel_vertices = sel_vertices;
}

fn phase_scene(app: &mut EffectcraftApp, ectx: &EvalCtx, v: &mut Vw<'_>) {
    let t = v.t;
    let cid = v.cid;
    let area = v.area;
    let comp_rect = v.comp_rect;
    let cw = v.cw;
    let ch = v.ch;
    let map = v.map;
    let time = v.time;
    let painter = v.painter.clone();
    let selected = v.selected.clone();
    let vertex_marks = std::mem::take(&mut v.vertex_marks);
    let mut track_hits = std::mem::take(&mut v.track_hits);
    let comp = v.comp;
    // Path points last, over the boxes' handles (#1042).
    ov::draw_vertex_marks(&painter, &vertex_marks, app.session.prefs.general.path_point_size as f32 + 1.0);

    // Settings ▸ 3D: Extended Viewer (3D layers outlined beyond the comp frame) and Show 3D
    // Reference Axes (world X/Y/Z in the corner, turning with the view).
    let has_3d = comp.layers.iter().any(|l| l.is_3d() && l.is_active_at(time));
    if has_3d && app.session.prefs.three_d.extended_viewer {
        for l in comp.layers.iter().filter(|l| l.is_3d() && l.is_active_at(time) && l.has_video()) {
            let q = layer_outline(ectx, l);
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
        let pc = cam_state(ectx).projection(cw as f64, ch as f64);
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
    let track_cur = super::tracker::viewer_track(app);
    if app.layer_controls_visible()
        && let Some((tl, tu)) = track_cur
        && selected.contains(&tl)
        && let Some(layer) = comp.layer(tl)
        && layer.is_active_at(time)
    {
        let (m, _) = l2c(ectx, layer);
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
        let (m, _) = l2c(ectx, layer);
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
        let (m, _) = l2c(ectx, layer);
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
    v.vertex_marks = vertex_marks;
    v.track_hits = track_hits;
    v.track_cur = track_cur;
    v.has_3d = has_3d;
}

fn phase_rigs(app: &mut EffectcraftApp, ui: &mut egui::Ui, ectx: &EvalCtx, v: &mut Vw<'_>) {
    let t = v.t;
    let cid = v.cid;
    let area = v.area;
    let map = v.map;
    let time = v.time;
    let ctx = v.ctx.clone();
    let painter = v.painter.clone();
    let selected = v.selected.clone();
    let paths = std::mem::take(&mut v.paths);
    let mut pin_hits = std::mem::take(&mut v.pin_hits);
    let mut puppet_layers = std::mem::take(&mut v.puppet_layers);
    let comp = v.comp;
    // Free transform box of a path (double-click a path with the Selection tool).
    let ft = ov::draw_free_transform(app, &ctx, &painter, &map, &paths);

    // Cameras and lights (wireframes) and the 3D view name.
    if comp.has_3d() {
        let through_active = VIEW_CAM.with(|c| c.get()).is_none();
        draw_rigs(app, &painter, &map, ectx, &selected, cam_state(ectx), through_active, None);
        let view = app.session.state.views3d.get(&cid).map(|v| v.current).unwrap_or_default();
        let label = match (view, comp.active_camera(time)) {
            (View3D::ActiveCamera, Some(c)) => format!("Active Camera ({})", c.name),
            (v, _) => v.label().to_string(),
        };
        painter.text(area.left_top() + vec2(12.0, 10.0), Align2::LEFT_TOP, label, Tokens::ui(12.0), t.text_dim);
    }

    // Puppet pins and meshes of the selected layers (Puppet tools).
    // The selected layers with a Puppet effect, top of the stack first (the layers the Puppet
    // tools work on).
    if app.ui.tool.puppet_kind().is_some() {
        let sel_props: Vec<u64> = app.session.state.selected_props.iter().map(|(_, u)| *u).collect();
        for lid in &selected {
            if let Some(l) = comp.layer(*lid)
                && let Some(ov) = super::puppet_tool::overlay(app, &ctx, cid, l, time)
            {
                let hover = ui.input(|i| i.pointer.hover_pos());
                pin_hits.extend(super::puppet_tool::draw(&painter, &map, ectx, l, &ov, app.session.state.puppet.show_mesh, &sel_props, hover));
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
    v.paths = paths;
    v.pin_hits = pin_hits;
    v.puppet_layers = puppet_layers;
    v.ft = ft;
}

fn phase_menu(app: &mut EffectcraftApp, ui: &mut egui::Ui, ectx: &EvalCtx, v: &mut Vw<'_>) -> egui::Response {
    let cid = v.cid;
    let area = v.area;
    let map = v.map;
    let ctx = v.ctx.clone();
    let painter = v.painter.clone();
    let comp = v.comp;
    // Project items, files and effects dropped on the viewer.
    super::viewer_drop::show(app, ui, &painter, &map, ectx);

    // Interaction.
    let resp = ui.interact(area, egui::Id::unique("viewer-interact"), Sense::click_and_drag());
    let menu_hits_id = egui::Id::unique(("viewer-menu-hits", cid.0));
    if resp.secondary_clicked()
        && let Some(pos) = resp.interact_pointer_pos()
    {
        // Picking an already selected layer preserves the group; an unselected hit
        // becomes the target, using the same visible/unlocked hit test as left-click.
        pick(app, ectx, map.to_comp(pos), false);
        let point = map.to_comp(pos);
        let hits: Vec<(u64, String)> = layers_at(ectx, point).map(|l| (l.id.0, l.name.clone())).collect();
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
                crate::widgets::menu_scroll(ui, false, |ui| {
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
                    caps.source |= c.source;
                    caps
                },
            );
            super::timeline::layer_menu(ui, &context_layers, caps, (&mut app.auto, "viewer.layerMenu"), &mut context_actions);
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
    resp
}

fn phase_hover(app: &mut EffectcraftApp, ui: &mut egui::Ui, ectx: &EvalCtx, resp: &egui::Response, v: &mut Vw<'_>) {
    let area = v.area;
    let zoom = v.zoom;
    let cw = v.cw;
    let ch = v.ch;
    let map = v.map;
    let ctx = v.ctx.clone();
    let painter = v.painter.clone();
    let handle_hits = std::mem::take(&mut v.handle_hits);
    let axis_hits = std::mem::take(&mut v.axis_hits);
    let vertex_hits = std::mem::take(&mut v.vertex_hits);
    let key_hits = std::mem::take(&mut v.key_hits);
    let motion_frames = std::mem::take(&mut v.motion_frames);
    let paths = std::mem::take(&mut v.paths);
    let track_hits = std::mem::take(&mut v.track_hits);
    let pin_hits = std::mem::take(&mut v.pin_hits);
    let comp = v.comp;
    // Path vertices and tangent handles take the press before the bounding-box handles (#1042).
    let vertex_at = |pos: Pos2, tangents: bool| -> Option<VertexHit> {
        let reach = vertex_grab_radius(pos, handle_hits.iter().map(|(_, _, h, _)| *h));
        vertex_hits.iter().rev().filter(|h| h.tangent.is_some() == tangents).find(|h| h.pos.distance(pos) < reach).copied()
    };
    let gid = egui::Id::unique("viewer-gesture");
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
                if ui.data(|d| d.get_temp::<Gesture>(egui::Id::unique("viewer-gesture"))).is_none() {
                    super::object_matte_tool::hover(app, &painter, &map, ectx, hp);
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
                } else if let Some((_, vertical)) = vt::guide_at(app, comp, &map, hp) {
                    if vertical { egui::CursorIcon::ResizeHorizontal } else { egui::CursorIcon::ResizeVertical }
                } else if vertex_at(hp, true).or_else(|| vertex_at(hp, false)).is_some() {
                    // A path point under a box handle is grabbed first: no resize cursor.
                    egui::CursorIcon::PointingHand
                } else if axis_at(&axis_hits, hp).is_some() {
                    egui::CursorIcon::Move
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
    v.handle_hits = handle_hits;
    v.axis_hits = axis_hits;
    v.vertex_hits = vertex_hits;
    v.key_hits = key_hits;
    v.motion_frames = motion_frames;
    v.paths = paths;
    v.track_hits = track_hits;
    v.pin_hits = pin_hits;
    v.gid = gid;
}

fn phase_press(app: &mut EffectcraftApp, ui: &mut egui::Ui, ectx: &EvalCtx, resp: &egui::Response, v: &mut Vw<'_>) {
    let map = v.map;
    let ctx = v.ctx.clone();
    let painter = v.painter.clone();
    let key_hits = std::mem::take(&mut v.key_hits);
    let motion_frames = std::mem::take(&mut v.motion_frames);
    let paths = std::mem::take(&mut v.paths);
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
        pen_press(app, ui, ectx, &map, &paths, ov::motion_frame_at(&motion_frames, &key_hits, pos), pos, mods);
    }
    // A pen click (press + release without a drag) ends its tangent gesture on release.
    if ui.input(|i| i.pointer.primary_released())
        && matches!(ui.data(|d| d.get_temp::<Gesture>(egui::Id::unique("viewer-gesture"))), Some(Gesture::Pen { .. }))
        && !resp.drag_stopped()
    {
        ui.data_mut(|d| d.remove::<Gesture>(egui::Id::unique("viewer-gesture")));
    }
    // On-canvas text editing (Type tool, double-click a text layer) owns its clicks and drags.
    let text_owns = super::viewer_text::hook(app, ui, &painter, &map, ectx, resp, space_pan);
    v.key_hits = key_hits;
    v.motion_frames = motion_frames;
    v.paths = paths;
    v.mods = mods;
    v.space_pan = space_pan;
    v.text_owns = text_owns;
}

fn phase_dragstart(app: &mut EffectcraftApp, ui: &mut egui::Ui, ectx: &EvalCtx, resp: &egui::Response, v: &mut Vw<'_>) {
    let comp_rect = v.comp_rect;
    let zoom = v.zoom;
    let map = v.map;
    let time = v.time;
    let mods = v.mods;
    let space_pan = v.space_pan;
    let text_owns = v.text_owns;
    let gid = v.gid;
    let track_cur = v.track_cur;
    let ft = std::mem::take(&mut v.ft);
    let handle_hits = std::mem::take(&mut v.handle_hits);
    let axis_hits = std::mem::take(&mut v.axis_hits);
    let vertex_hits = std::mem::take(&mut v.vertex_hits);
    let key_hits = std::mem::take(&mut v.key_hits);
    let paths = std::mem::take(&mut v.paths);
    let track_hits = std::mem::take(&mut v.track_hits);
    let pin_hits = std::mem::take(&mut v.pin_hits);
    let puppet_layers = std::mem::take(&mut v.puppet_layers);
    let comp = v.comp;
    let vertex_at = |pos: Pos2, tangents: bool| -> Option<VertexHit> {
        let reach = vertex_grab_radius(pos, handle_hits.iter().map(|(_, _, h, _)| *h));
        vertex_hits.iter().rev().filter(|h| h.tangent.is_some() == tangents).find(|h| h.pos.distance(pos) < reach).copied()
    };
    let puppet_layer = |app: &mut EffectcraftApp, cpt: [f64; 2]| -> Option<LayerId> {
        if puppet_layers.is_empty() {
            return pick(app, ectx, cpt, false);
        }
        super::puppet_tool::target(ectx, &puppet_layers, cpt, |l| layers_at(ectx, cpt).any(|h| h.id == l.id))
    };
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
            Tool::Selection if vt::guide_at(app, comp, &map, press).is_some() => {
                vt::guide_at(app, comp, &map, press).map(|(index, vertical)| Gesture::Overlay(ov::Drag::Guide { index, vertical }))
            }
            // Alt-drag a vertex with the Selection tool (or drag one with Convert Vertex): pull
            // new tangents out of it.
            Tool::Selection | Tool::PenConvert if (mods.alt || tool == Tool::PenConvert) && vertex_at(press, false).is_some() => {
                vertex_at(press, false).map(|h| {
                    let inv = h.l2c.inverse().unwrap_or(Mat3::IDENTITY);
                    Gesture::Pen { layer: h.layer, mask: h.mask, index: h.index, vertex: h.vertex, inv, press }
                })
            }
            Tool::MaskFeather => ov::begin_feather(app, ectx, &paths, &map, press).map(Gesture::Overlay),
            Tool::Rotate => pick(app, ectx, cpt, mods.shift).map(|l| {
                let others = selected_others(app, comp, l).into_iter().filter_map(|o| Some((o.id, ectx.f(o, o.transform()?, "rotation", 0.0)))).collect();
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
            Tool::PanBehind => pick(app, ectx, cpt, false).and_then(|l| {
                let layer = comp.layer(l)?;
                let tr = layer.transform()?;
                let (l2c, _) = l2c(ectx, layer);
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
                    own: effectcraft_engine::viewer::layer_targets(ectx, layer, false, app.session.state.snap_features),
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
                let inv = fx2c(ectx, l).inverse()?;
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
                    let (m, _) = l2c(ectx, layer);
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
                // A gizmo arrow is drawn over the box handles, so it is grabbed first: move along
                // that axis only, without snapping.
                if let Some((lid, _, ax, ay)) = axis_at(&axis_hits, press) {
                    let p0 = comp.layer(lid).and_then(|l| Some(ectx.v3(l, l.transform()?, "position", [0.0; 3])));
                    p0.map(|p0| Gesture::Move { layers: vec![(lid, p0, ax, ay)], start: cpt, snap_src: None })
                } else if let Some((lid, hi, ..)) = handle_hits.iter().find(|(_, _, h, _)| h.distance(press) < HANDLE + 2.0).cloned() {
                    let layer = comp.layer(lid).cloned();
                    layer.and_then(|layer| {
                        let tr = layer.transform()?;
                        let (l2c, _) = l2c(ectx, &layer);
                        let inv = l2c.inverse()?;
                        let a = ectx.v3(&layer, tr, "anchor", [0.0; 3]);
                        let lp = inv.apply(gv2(cpt[0], cpt[1]));
                        let hp = handle_point(effectcraft_engine::render::content_bounds(ectx, &layer)?, hi);
                        Some(Gesture::Scale {
                            layer: lid,
                            start_scale: ectx.v3(&layer, tr, "scale", [100.0; 3]),
                            start_local: [lp.x - a[0], lp.y - a[1]],
                            handle: [hp[0] - a[0], hp[1] - a[1]],
                            anchor: [a[0], a[1]],
                            axes: handle_axes(hi),
                            l2c,
                            inv,
                            others: selected_others(app, comp, lid)
                                .into_iter()
                                .filter_map(|o| Some((o.id, ectx.v3(o, o.transform()?, "scale", [100.0; 3]))))
                                .collect(),
                        })
                    })
                } else if let Some(l) = pick(app, ectx, cpt, mods.shift) {
                    let layers: Vec<(LayerId, [f64; 3], [f64; 3], [f64; 3])> = app
                        .session
                        .state
                        .selected_layers
                        .iter()
                        .filter_map(|id| comp.layer(*id))
                        .filter(|l| !l.switches.locked)
                        .filter_map(|l| {
                            let p0 = ectx.v3(l, l.transform()?, "position", [0.0; 3]);
                            let (ax, ay) = drag_axes(ectx, l);
                            Some((l.id, p0, ax, ay))
                        })
                        .collect();
                    // Snap the grabbed layer's feature nearest the pointer (AE's snap handle).
                    let snap_src = comp.layer(l).map(|layer| effectcraft_engine::viewer::layer_features(ectx, layer)).and_then(|f| {
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
    v.ft = ft;
    v.handle_hits = handle_hits;
    v.axis_hits = axis_hits;
    v.vertex_hits = vertex_hits;
    v.key_hits = key_hits;
    v.paths = paths;
    v.track_hits = track_hits;
    v.pin_hits = pin_hits;
    v.puppet_layers = puppet_layers;
}

fn phase_drag(app: &mut EffectcraftApp, ui: &mut egui::Ui, ectx: &EvalCtx, resp: &egui::Response, v: &mut Vw<'_>) {
    let t = v.t;
    let zoom = v.zoom;
    let map = v.map;
    let mods = v.mods;
    let gid = v.gid;
    let ctx = v.ctx.clone();
    let painter = v.painter.clone();
    let gesture: Option<Gesture> = ui.data(|d| d.get_temp(gid));
    if let (Some(g), Some(pos)) = (gesture.clone(), resp.interact_pointer_pos()) {
        let cpt = map.to_comp(pos);
        let merge = format!("viewer-drag-{}", ui.data(|d| d.get_temp::<u64>(egui::Id::unique("viewer-drag-n")).unwrap_or(0)));
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
                    let c = vt::snap(app, &ctx, ectx, &map, &ids, &[[src[0] + d[0], src[1] + d[1]]], mods);
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
                let f = snap_handle(f, handle, anchor, &l2c, axes, uniform, |p| vt::snap(app, &ctx, ectx, &map, &[layer], &[p], mods));
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
                let sc = vt::snap_with(app, &ctx, ectx, &map, &vt::SnapQuery { exclude: &[layer], sources: &[want], mods, extra: &own });
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
                let c = vt::snap(app, &ctx, ectx, &map, &[layer], &[[src[0] + want[0], src[1] + want[1]]], mods);
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
                    let mut snapper = |a: &EffectcraftApp, p: [f64; 2], l: LayerId| vt::snap(a, &ctx, ectx, &map, &[l], &[p], mods);
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
    v.gesture = gesture;
}

fn phase_stopped(app: &mut EffectcraftApp, ui: &mut egui::Ui, ectx: &EvalCtx, resp: &egui::Response, v: &mut Vw<'_>) {
    let area = v.area;
    let comp_rect = v.comp_rect;
    let map = v.map;
    let time = v.time;
    let mods = v.mods;
    let gid = v.gid;
    let vertex_hits = std::mem::take(&mut v.vertex_hits);
    let pin_hits = std::mem::take(&mut v.pin_hits);
    let gesture = v.gesture.clone();
    let comp = v.comp;
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
                Gesture::ObjectMatteBox { start } => super::object_matte_tool::apply(app, ectx, &map, start, Some(pos), mods.shift),
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
                    let hits: Vec<u64> = selectable_layers(comp, time)
                        .filter(|l| layer_quad(ectx, l).is_some_and(|(_, q, _)| quad_intersects_box(&q, a, b)))
                        .map(|l| l.id.0)
                        .collect();
                    let _ = app.session.execute("layer.select", json!({"layers": hits, "toggle": mods.shift}));
                }
                _ => {}
            }
        }
        ui.data_mut(|d| {
            d.remove::<Gesture>(gid);
            let n: u64 = d.get_temp(egui::Id::unique("viewer-drag-n")).unwrap_or(0);
            d.insert_temp(egui::Id::unique("viewer-drag-n"), n + 1);
        });
    }
    v.vertex_hits = vertex_hits;
    v.pin_hits = pin_hits;
}

fn phase_clicked(app: &mut EffectcraftApp, ectx: &EvalCtx, resp: &egui::Response, v: &mut Vw<'_>) {
    let area = v.area;
    let zoom = v.zoom;
    let cw = v.cw;
    let ch = v.ch;
    let map = v.map;
    let mods = v.mods;
    let text_owns = v.text_owns;
    let ctx = v.ctx.clone();
    let ft = std::mem::take(&mut v.ft);
    let handle_hits = std::mem::take(&mut v.handle_hits);
    let vertex_hits = std::mem::take(&mut v.vertex_hits);
    let key_hits = std::mem::take(&mut v.key_hits);
    let motion_frames = std::mem::take(&mut v.motion_frames);
    let paths = std::mem::take(&mut v.paths);
    let pin_hits = std::mem::take(&mut v.pin_hits);
    let puppet_layers = std::mem::take(&mut v.puppet_layers);
    let comp = v.comp;
    let vertex_at = |pos: Pos2, tangents: bool| -> Option<VertexHit> {
        let reach = vertex_grab_radius(pos, handle_hits.iter().map(|(_, _, h, _)| *h));
        vertex_hits.iter().rev().filter(|h| h.tangent.is_some() == tangents).find(|h| h.pos.distance(pos) < reach).copied()
    };
    let puppet_layer = |app: &mut EffectcraftApp, cpt: [f64; 2]| -> Option<LayerId> {
        if puppet_layers.is_empty() {
            return pick(app, ectx, cpt, false);
        }
        super::puppet_tool::target(ectx, &puppet_layers, cpt, |l| layers_at(ectx, cpt).any(|h| h.id == l.id))
    };
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
            Tool::ObjectMatte => super::object_matte_tool::apply(app, ectx, &map, pos, None, mods.shift),
            t if t.puppet_kind().is_some() => {
                if let Some((h, _)) = super::puppet_tool::hit(&pin_hits, pos) {
                    // Click selects the pin; Shift-click adds or removes it.
                    let _ = app.session.execute("puppet.selectPins", json!({"layer": h.layer.0, "pins": [h.pin], "toggle": mods.shift}));
                } else if let Some(l) = puppet_layer(app, cpt).and_then(|l| comp.layer(l))
                    && let Some(inv) = fx2c(ectx, l).inverse()
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
                if pick(app, ectx, cpt, mods.shift).is_none() {
                    let _ = app.session.execute("edit.deselectAll", json!({}));
                }
                app.ui.status = crate::i18n::tr(ROTO_HINT).to_string();
            }
            _ => {
                if pick(app, ectx, cpt, mods.shift).is_none() {
                    let _ = app.session.execute("edit.deselectAll", json!({}));
                } else if !mods.shift && !app.session.state.selected_vertices.is_empty() && ov::segment_at(&paths, &map, pos, 6.0).is_none() {
                    // A click away from the paths, even on their layer, deselects the mask
                    // points (the layer stays selected), as in After Effects (#513).
                    let _ = app.session.execute("mask.selectVertices", json!({"vertices": []}));
                }
            }
        }
    }
    v.ft = ft;
    v.handle_hits = handle_hits;
    v.vertex_hits = vertex_hits;
    v.key_hits = key_hits;
    v.motion_frames = motion_frames;
    v.paths = paths;
    v.pin_hits = pin_hits;
    v.puppet_layers = puppet_layers;
}

fn phase_dblclick(app: &mut EffectcraftApp, ectx: &EvalCtx, resp: &egui::Response, v: &mut Vw<'_>) {
    let map = v.map;
    let text_owns = v.text_owns;
    let ctx = v.ctx.clone();
    let handle_hits = std::mem::take(&mut v.handle_hits);
    let vertex_hits = std::mem::take(&mut v.vertex_hits);
    let paths = std::mem::take(&mut v.paths);
    let comp = v.comp;
    let vertex_at = |pos: Pos2, tangents: bool| -> Option<VertexHit> {
        let reach = vertex_grab_radius(pos, handle_hits.iter().map(|(_, _, h, _)| *h));
        vertex_hits.iter().rev().filter(|h| h.tangent.is_some() == tangents).find(|h| h.pos.distance(pos) < reach).copied()
    };
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
        if let Some(l) = pick(app, ectx, cpt, false).and_then(|l| comp.layer(l)) {
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
    v.handle_hits = handle_hits;
    v.vertex_hits = vertex_hits;
    v.paths = paths;
}

fn phase_tail(app: &mut EffectcraftApp, ui: &mut egui::Ui, ectx: &EvalCtx, v: &mut Vw<'_>) {
    let t = v.t;
    let cid = v.cid;
    let bar = v.bar;
    let err_bar = v.err_bar;
    let err_h = v.err_h;
    let area = v.area;
    let outer = v.outer;
    let zoom = v.zoom;
    let map = v.map;
    let time = v.time;
    let ctx = v.ctx.clone();
    let painter = v.painter.clone();
    let comp = v.comp;
    // 3D Camera Tracker points, targets and their menu (on top of the viewer's gestures while
    // the effect is selected), and its analysis banner.
    super::camera_tracker_ui::viewer_hook(app, ui, &painter, &map, ectx, &|c, l| l2c(c, l).0);
    // Effect point controls and crosshair/eyedropper picks (on top of the viewer's gestures).
    crate::panels::effect_controls::viewer_hook(app, ui, &painter, &map, ectx, &|c, l| l2c(c, l).0);
    // Selected shape gradients' start and end points (#612).
    crate::panels::gradient_handles::viewer_hook(app, ui, &painter, &map, ectx, &|c, l| l2c(c, l).0);
    // Selected text Range Selectors' Start and End bars (#1011).
    crate::panels::text_range_handles::viewer_hook(app, ui, &painter, &map, ectx, &|c, l| l2c(c, l).0);

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
        ui.ctx().data_mut(|d| d.insert_temp(egui::Id::unique("viewer-video-off-chip"), r));
    }

    vt::draw_snap(&ctx, &painter, &map, ectx);
    vt::rulers(app, ui, &map, outer, area);
    vt::bottom_bar(app, ui, bar, zoom, time, comp);
    if err_h > 0.0 {
        super::expr_bar::draw(app, ui, err_bar, cid);
    }
}
pub(crate) use crate::widgets::hex_rgb;

mod frame;
mod geometry;
mod gesture;
mod shapes;
mod tools;
mod view;

use frame::{empty_state, rect_of, show_frame};
pub(crate) use frame::{extended_region, minify_factor, snap_to_pixels, styled_line, zoom_sampler, zoom_texture_options};
#[cfg(test)]
use geometry::{box_corners, project_clipped};
use geometry::{
    cam_state, drag_ratio, handle_axes, handle_cursor, handle_point, one_axis, project_segment, quad_intersects_box, rotate_value, selected_others,
    snap_handle, turn_delta,
};
pub(crate) use geometry::{from_effect_space, fx2c, l2c, layer_outline, layer_quad};
use gesture::{Gesture, HANDLE, ROTO_HINT, pen_id, roi_drag, roi_handles, vertex_grab_radius};
use shapes::{create_shape, draw_shape_ghost, shape_drag};
pub(crate) use shapes::{layer_at, layers_at, pick, selectable_layers};
#[cfg(test)]
use tools::constrain_45;
use tools::{add_motion_key, axis_at, axis_drag, drag_axes, draw_rigs, parent_world_inverse, pen_cursor, pen_cursor_icon, pen_press, select_motion_key};
pub(crate) use view::checker;
use view::{aux_views, locked_pane};

#[cfg(test)]
mod tests;
