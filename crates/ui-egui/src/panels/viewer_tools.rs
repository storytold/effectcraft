//! Composition viewer tools: snapping (targets and the highlighted feedback), rulers with guides
//! dragged out of them, the displayed frame (Show Channel, exposure, snapshots, region of
//! interest, Wireframe Fast Previews) and the bottom control bar in After Effects' order.
//!
//! Edits go through engine commands (`view.*`, `comp.*`); the viewer calls in from small hooks.

use effectcraft_engine::commands::viewer_cmds::FastPreviews;
use effectcraft_engine::project::{Comp, ItemId, LayerId};
use effectcraft_engine::render::EvalCtx;
use effectcraft_engine::time::Tick;
use effectcraft_engine::viewer::{self as vw, Channel, Snap, SnapKind, SnapSource};
use egui::{Align2, Color32, Pos2, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use serde_json::json;

use super::viewer::ViewerMap;
use crate::icons::Icon;
use crate::state::Resolution;
use crate::theme::Tokens;
use crate::{EffectcraftApp, widgets};

// ---------------------------------------------------------------- snapping

/// Snap distance in screen points.
pub(crate) const SNAP_PX: f64 = 8.0;
const SNAP_COLOR: Color32 = Color32::from_rgb(0xff, 0xd2, 0x3c);

fn snap_id() -> egui::Id {
    egui::Id::new("viewer-snap")
}

/// Layer snapping is on: the Snapping checkbox (View ▸ Snapping), inverted while Cmd/Ctrl is held.
pub(crate) fn snapping_on(app: &EffectcraftApp, mods: egui::Modifiers) -> bool {
    app.session.state.snapping != mods.command
}

/// Snap dragged feature points (`sources`, comp pixels) to the comp's targets: other layers'
/// edges, corners, centres, anchor points and mask/shape vertices and the comp edges and centre
/// (when snapping is on, as the Snapping options allow), guides (View ▸ Snap to Guides) and the
/// grid (View ▸ Snap to Grid). Returns the comp-space correction (zero when nothing is near) and
/// records the feedback for this frame.
pub(crate) fn snap(
    app: &EffectcraftApp,
    ctx: &egui::Context,
    ectx: &EvalCtx,
    map: &ViewerMap,
    exclude: &[LayerId],
    sources: &[[f64; 2]],
    mods: egui::Modifiers,
) -> [f64; 2] {
    snap_with(app, ctx, ectx, map, exclude, sources, mods, &[])
}

/// [`snap`] with `extra` layer targets (the Pan Behind layer's own box).
pub(crate) fn snap_with(
    app: &EffectcraftApp,
    ctx: &egui::Context,
    ectx: &EvalCtx,
    map: &ViewerMap,
    exclude: &[LayerId],
    sources: &[[f64; 2]],
    mods: egui::Modifiers,
    extra: &[vw::SnapTarget],
) -> [f64; 2] {
    let layers = snapping_on(app, mods);
    let v = &app.ui.viewer;
    let guides = v.snap_guides && v.guides;
    let grid = v.snap_grid;
    if sources.is_empty() || !(layers || guides || grid) {
        return [0.0; 2];
    }
    let opts = vw::SnapOptions { layers, features: app.session.state.snap_features, guides, grid, grid_spacing: app.session.prefs.grids.grid_spacing };
    let mut targets = vw::targets(ectx, exclude, opts);
    if layers {
        targets.extend_from_slice(extra);
    }
    match vw::snap(sources, &targets, SNAP_PX / map.zoom.max(1e-3) as f64) {
        Some(s) => {
            let d = s.delta;
            ctx.data_mut(|dd| dd.insert_temp(snap_id(), s));
            d
        }
        None => [0.0; 2],
    }
}

/// The snap handle: the dragged layer's feature that snaps (the one nearest where it was
/// grabbed), boxed while snapping is on.
pub(crate) fn draw_handle(painter: &egui::Painter, at: Pos2) {
    painter.rect_stroke(Rect::from_center_size(at, vec2(6.0, 6.0)), 0.0, Stroke::new(1.0, SNAP_COLOR), StrokeKind::Middle);
}

/// Draw (and clear) this frame's snap feedback: the target layer's box highlighted, the guide or
/// grid line, and a cross where the feature landed.
pub(crate) fn draw_snap(ctx: &egui::Context, painter: &egui::Painter, map: &ViewerMap, ectx: &EvalCtx) {
    let Some(s) = ctx.data(|d| d.get_temp::<Snap>(snap_id())) else { return };
    ctx.data_mut(|d| d.remove::<Snap>(snap_id()));
    let stroke = Stroke::new(1.5, SNAP_COLOR);
    let area = map.area;
    for h in &s.hits {
        if let SnapSource::Layer(l) | SnapSource::Vertex(l) = h.source
            && let Some(layer) = ectx.comp.layer(l)
            && let Some(b) = effectcraft_engine::render::content_bounds(ectx, layer)
        {
            let (m, _) = ectx.layer_to_comp(layer);
            let q: Vec<Pos2> = [[b[0], b[1]], [b[2], b[1]], [b[2], b[3]], [b[0], b[3]]]
                .iter()
                .map(|p| {
                    let c = m.apply(effectcraft_engine::geom::vec2(p[0], p[1]));
                    map.to_screen([c.x, c.y])
                })
                .collect();
            painter.add(egui::Shape::closed_line(q, stroke));
        }
        match h.kind {
            SnapKind::Point => {
                let c = map.to_screen(h.pos);
                painter.rect_stroke(Rect::from_center_size(c, vec2(9.0, 9.0)), 0.0, stroke, StrokeKind::Middle);
            }
            // A line across the viewer, or along its span (to where the feature landed).
            SnapKind::VLine => {
                let x = map.to_screen(h.pos).x;
                let (a, b) = h.span.map_or((area.min.y, area.max.y), |[a, b]| (map.to_screen([0.0, a.min(s.at[1])]).y, map.to_screen([0.0, b.max(s.at[1])]).y));
                painter.line_segment([pos2(x, a), pos2(x, b)], Stroke::new(1.0, SNAP_COLOR));
            }
            SnapKind::HLine => {
                let y = map.to_screen(h.pos).y;
                let (a, b) = h.span.map_or((area.min.x, area.max.x), |[a, b]| (map.to_screen([a.min(s.at[0]), 0.0]).x, map.to_screen([b.max(s.at[0]), 0.0]).x));
                painter.line_segment([pos2(a, y), pos2(b, y)], Stroke::new(1.0, SNAP_COLOR));
            }
        }
    }
    let c = map.to_screen(s.at);
    painter.line_segment([c - vec2(8.0, 0.0), c + vec2(8.0, 0.0)], stroke);
    painter.line_segment([c - vec2(0.0, 8.0), c + vec2(0.0, 8.0)], stroke);
}

// ---------------------------------------------------------------- rulers and guides

/// Ruler thickness in points.
pub(crate) const RULER: f32 = 16.0;

/// The viewer area left for the image once the rulers (View ▸ Show Rulers) take their strips.
pub(crate) fn inset(app: &EffectcraftApp, area: Rect) -> Rect {
    if app.ui.viewer.rulers { Rect::from_min_max(area.min + vec2(RULER, RULER), area.max) } else { area }
}

fn nice(raw: f64) -> f64 {
    let mag = 10f64.powf(raw.max(1e-9).log10().floor());
    let n = raw / mag;
    mag * if n <= 1.0 {
        1.0
    } else if n <= 2.0 {
        2.0
    } else if n <= 5.0 {
        5.0
    } else {
        10.0
    }
}

/// Draw the rulers (ticks relative to the zero point, live pointer markers) and handle their
/// gestures: drag from a ruler to add a guide, drag the corner to move the zero point
/// (double-click resets it).
pub(crate) fn rulers(app: &mut EffectcraftApp, ui: &mut egui::Ui, map: &ViewerMap, outer: Rect, inner: Rect) {
    if !app.ui.viewer.rulers {
        return;
    }
    let t = app.tokens;
    let p = ui.painter().clone();
    let top = Rect::from_min_max(pos2(inner.min.x, outer.min.y), pos2(inner.max.x, inner.min.y));
    let left = Rect::from_min_max(pos2(outer.min.x, inner.min.y), pos2(inner.min.x, inner.max.y));
    let corner = Rect::from_min_max(outer.min, inner.min);
    let bg = t.panel_bg;
    for r in [top, left, corner] {
        p.rect_filled(r, 0.0, bg);
    }
    p.line_segment([top.left_bottom(), top.right_bottom()], Stroke::new(1.0, t.separator));
    p.line_segment([left.right_top(), left.right_bottom()], Stroke::new(1.0, t.separator));
    let o = app.ui.viewer.ruler_origin;
    let zoom = map.zoom as f64;
    let step = nice(64.0 / zoom.max(1e-6));
    let minor = step / if (step / nice(step / 2.0) - 2.0).abs() < 1e-9 { 2.0 } else { 5.0 };
    let tick = Stroke::new(1.0, t.text_faint);
    let font = Tokens::ui(9.0);
    let label = |v: f64| if step >= 1.0 { format!("{}", v.round() as i64) } else { format!("{v:.1}") };
    // Horizontal ruler (x).
    let pt = p.with_clip_rect(top);
    let (c0, c1) = (map.to_comp(top.left_top())[0], map.to_comp(top.right_top())[0]);
    let mut k = ((c0 - o[0]) / minor).floor() as i64;
    while (o[0] + k as f64 * minor) <= c1 {
        let cx = o[0] + k as f64 * minor;
        let x = map.to_screen([cx, 0.0]).x;
        let major = ((k as f64 * minor) / step).round() * step - k as f64 * minor;
        let is_major = major.abs() < minor * 0.01;
        pt.line_segment([pos2(x, top.max.y - if is_major { RULER - 2.0 } else { 4.0 }), pos2(x, top.max.y)], tick);
        if is_major {
            pt.text(pos2(x + 2.0, top.min.y + 1.0), Align2::LEFT_TOP, label(cx - o[0]), font.clone(), t.text_dim);
        }
        k += 1;
    }
    // Vertical ruler (y).
    let pl = p.with_clip_rect(left);
    let (r0, r1) = (map.to_comp(left.left_top())[1], map.to_comp(left.left_bottom())[1]);
    let mut k = ((r0 - o[1]) / minor).floor() as i64;
    while (o[1] + k as f64 * minor) <= r1 {
        let cy = o[1] + k as f64 * minor;
        let y = map.to_screen([0.0, cy]).y;
        let major = ((k as f64 * minor) / step).round() * step - k as f64 * minor;
        let is_major = major.abs() < minor * 0.01;
        pl.line_segment([pos2(left.max.x - if is_major { RULER - 2.0 } else { 4.0 }, y), pos2(left.max.x, y)], tick);
        if is_major {
            let g = pl.layout_no_wrap(label(cy - o[1]), font.clone(), t.text_dim);
            pl.add(egui::epaint::TextShape::new(pos2(left.min.x + 1.0, y + g.size().x + 2.0), g, t.text_dim).with_angle(-std::f32::consts::FRAC_PI_2));
        }
        k += 1;
    }
    // Live pointer markers.
    if let Some(hp) = ui.ctx().pointer_hover_pos()
        && inner.contains(hp)
    {
        let m = Stroke::new(1.0, t.accent);
        pt.line_segment([pos2(hp.x, top.min.y), pos2(hp.x, top.max.y)], m);
        pl.line_segment([pos2(left.min.x, hp.y), pos2(left.max.x, hp.y)], m);
    }
    // Zero point: drag the corner, double-click to reset.
    p.line_segment([corner.center() - vec2(4.0, 0.0), corner.center() + vec2(4.0, 0.0)], tick);
    p.line_segment([corner.center() - vec2(0.0, 4.0), corner.center() + vec2(0.0, 4.0)], tick);
    let cr = ui
        .interact(corner, egui::Id::new("vw-ruler-origin"), Sense::click_and_drag())
        .on_hover_text(crate::i18n::tr("Ruler zero point (drag to move, double-click to reset)"));
    app.auto.add("viewer.ruler.origin", corner, "Ruler zero point");
    let fg = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("vw-ruler-fg")));
    if cr.dragged()
        && let Some(hp) = cr.interact_pointer_pos()
    {
        fg.line_segment([pos2(inner.min.x, hp.y), pos2(inner.max.x, hp.y)], Stroke::new(1.0, t.accent));
        fg.line_segment([pos2(hp.x, inner.min.y), pos2(hp.x, inner.max.y)], Stroke::new(1.0, t.accent));
    }
    if cr.drag_stopped()
        && let Some(hp) = cr.interact_pointer_pos()
    {
        let c = map.to_comp(hp);
        app.ui.viewer.ruler_origin = [c[0].round(), c[1].round()];
    }
    if cr.double_clicked() {
        app.ui.viewer.ruler_origin = [0.0, 0.0];
    }
    // Guides dragged out of the rulers: the top ruler makes horizontal guides, the left one
    // vertical guides.
    for (r, vertical, id) in [(top, false, "top"), (left, true, "left")] {
        let resp = ui.interact(r, egui::Id::new(("vw-ruler", id)), Sense::drag()).on_hover_text(crate::i18n::tr("Drag to create a guide"));
        app.auto.add(&format!("viewer.ruler.{id}"), r, if vertical { "Vertical ruler" } else { "Horizontal ruler" });
        if resp.hovered() {
            ui.ctx().set_cursor_icon(if vertical { egui::CursorIcon::ResizeHorizontal } else { egui::CursorIcon::ResizeVertical });
        }
        if resp.dragged()
            && let Some(hp) = resp.interact_pointer_pos()
        {
            let s = Stroke::new(1.0, guide_color(app));
            if vertical {
                fg.line_segment([pos2(hp.x, inner.min.y), pos2(hp.x, inner.max.y)], s);
            } else {
                fg.line_segment([pos2(inner.min.x, hp.y), pos2(inner.max.x, hp.y)], s);
            }
        }
        if resp.drag_stopped()
            && let Some(hp) = resp.interact_pointer_pos()
            && inner.contains(hp)
        {
            let c = map.to_comp(hp);
            let pos = if vertical { c[0] } else { c[1] }.round();
            let r = app.session.execute("view.addGuide", json!({"orientation": if vertical { "vertical" } else { "horizontal" }, "position": pos}));
            if let Err(e) = r {
                app.ui.status = e.to_string();
            }
            app.ui.viewer.guides = true;
        }
    }
}

pub(crate) fn guide_color(app: &EffectcraftApp) -> Color32 {
    let [r, g, b] = super::viewer::hex_rgb(&app.session.prefs.grids.guide_color).unwrap_or([0x3c, 0xc8, 0xf0]);
    Color32::from_rgb(r, g, b)
}

/// The guide under a screen point (visible, unlocked guides only): (index, vertical).
pub(crate) fn guide_at(app: &EffectcraftApp, comp: &Comp, map: &ViewerMap, pos: Pos2) -> Option<(usize, bool)> {
    if !app.ui.viewer.guides || app.ui.viewer.lock_guides {
        return None;
    }
    comp.guides.iter().enumerate().find_map(|(i, g)| {
        let s = map.to_screen([g.position, g.position]);
        let d = if g.vertical { (s.x - pos.x).abs() } else { (s.y - pos.y).abs() };
        (d < 4.0).then_some((i, g.vertical))
    })
}

/// Proportional grid (Grid and guide options ▸ Proportional Grid): Settings ▸ Grids & Guides
/// horizontal × vertical cells over the comp.
pub(crate) fn proportional_grid(app: &EffectcraftApp, painter: &egui::Painter, comp_rect: Rect) {
    if !app.ui.viewer.proportional_grid {
        return;
    }
    let g = &app.session.prefs.grids;
    let [r, gg, b] = super::viewer::hex_rgb(&g.grid_color).unwrap_or([120, 160, 255]);
    let s = Stroke::new(1.0, Color32::from_rgba_unmultiplied(r, gg, b, 110));
    let (nx, ny) = (g.proportional_horizontal.max(1), g.proportional_vertical.max(1));
    for i in 1..nx {
        let x = comp_rect.min.x + comp_rect.width() * i as f32 / nx as f32;
        super::viewer::styled_line(painter, [pos2(x, comp_rect.min.y), pos2(x, comp_rect.max.y)], s, &g.grid_style);
    }
    for i in 1..ny {
        let y = comp_rect.min.y + comp_rect.height() * i as f32 / ny as f32;
        super::viewer::styled_line(painter, [pos2(comp_rect.min.x, y), pos2(comp_rect.max.x, y)], s, &g.grid_style);
    }
}

// ---------------------------------------------------------------- the displayed frame

/// Remember the region of interest the displayed texture was rendered with.
pub(crate) fn set_texture_roi(ctx: &egui::Context, roi: Option<[f64; 4]>) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::new("viewer-tex-roi"), roi));
}

fn texture_roi(ctx: &egui::Context) -> Option<[f64; 4]> {
    ctx.data(|d| d.get_temp::<Option<[f64; 4]>>(egui::Id::new("viewer-tex-roi"))).flatten()
}

/// The comp area ([x, y, w, h], comp pixels) the shown frame covers: its region of interest or
/// Extended Viewer area, else the whole `comp` (width, height).
pub(crate) fn shown_region(ctx: &egui::Context, comp: [f32; 2]) -> [f64; 4] {
    texture_roi(ctx).unwrap_or([0.0, 0.0, comp[0] as f64, comp[1] as f64])
}

/// Show Snapshot: the stored snapshot is on screen (button or F5 held).
pub(crate) fn showing_snapshot(app: &EffectcraftApp, ctx: &egui::Context) -> bool {
    let f5 = ctx.input(|i| i.key_down(egui::Key::F5)) && !ctx.egui_wants_keyboard_input();
    app.session.snapshot.is_some() && (app.session.state.viewer.show_snapshot || f5)
}

/// Show Channel and exposure, then the display colour conversion (View ▸ Use Display Color
/// Management / Simulate Output).
fn transformed(img: &egui::ColorImage, ch: Channel, colorized: bool, stops: f32, dc: Option<vw::DisplayColor>) -> egui::ColorImage {
    let mut px: Vec<[u8; 4]> = img.pixels.iter().map(|c| c.to_array()).collect();
    if let Some(dc) = dc.filter(|_| matches!(ch, Channel::Rgb | Channel::RgbStraight)) {
        dc.apply(&mut px);
    }
    vw::display_transform(&mut px, ch, colorized, stops);
    egui::ColorImage::new(img.size, px.into_iter().map(|a| Color32::from_rgba_premultiplied(a[0], a[1], a[2], a[3])).collect())
}

/// Draw the frame into `comp_rect`: the rendered frame (covering the region of interest when one
/// is set), or the snapshot while Show Snapshot is on / F5 is held, through Show Channel and the
/// exposure. Fast Previews ▸ Wireframe draws layer outlines instead.
pub(crate) fn draw_frame(app: &mut EffectcraftApp, ctx: &egui::Context, painter: &egui::Painter, comp_rect: Rect, cid: ItemId, ectx: &EvalCtx) {
    let opts = app.session.state.viewer.clone();
    let snap = showing_snapshot(app, ctx);
    // Settings ▸ Video ▸ Mirror on Computer Monitor off: playback goes to Video Preview only.
    if crate::prefs_live::main_viewer_hidden(app) && !snap {
        painter.text(comp_rect.center(), Align2::CENTER_CENTER, crate::i18n::tr("Playing on Video Preview"), Tokens::ui(13.0), Color32::from_gray(150));
        return;
    }
    if opts.fast_previews == FastPreviews::Wireframe && !snap {
        let zoom = comp_rect.width() / ectx.comp.width.max(1) as f32;
        let map = ViewerMap { origin: comp_rect.min, zoom, comp: [ectx.comp.width as f32, ectx.comp.height as f32], area: comp_rect };
        for l in ectx.comp.layers.iter().filter(|l| l.is_active_at(ectx.time) && l.has_video()) {
            let Some(b) = effectcraft_engine::render::content_bounds(ectx, l) else { continue };
            let (m, _) = super::viewer::l2c(ectx, l);
            let q: Vec<Pos2> = [[b[0], b[1]], [b[2], b[1]], [b[2], b[3]], [b[0], b[3]]]
                .iter()
                .map(|p| {
                    let c = m.apply(effectcraft_engine::geom::vec2(p[0], p[1]));
                    map.to_screen([c.x, c.y])
                })
                .collect();
            painter.add(egui::Shape::closed_line(q, Stroke::new(1.0, app.tokens.label(l.label))));
        }
        return;
    }
    let dc = vw::DisplayColor::of(&app.session);
    // The display conversion's inputs, for the texture caches.
    let custom = if opts.simulation.profile == vw::SimProfile::MyCustom { format!("{:?}", app.session.prefs.custom_rgb) } else { String::new() };
    let dc_key = format!("{:?}{:?}{}{custom}", dc.is_some(), opts.simulation, app.session.prefs.previews.display_profile)
        + &format!("{:?}{:?}{}", app.session.project.settings.working_space, app.session.project.settings.output_space, opts.display_color_management);
    let plain = opts.channel == Channel::Rgb && opts.exposure == 0.0 && dc.is_none();
    let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
    if snap {
        let Some(s) = app.session.snapshot.clone() else { return };
        let key = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            (std::sync::Arc::as_ptr(&s.image) as usize, opts.channel.id(), opts.colorized, opts.exposure.to_bits(), &dc_key).hash(&mut h);
            h.finish()
        };
        let id = egui::Id::new("viewer-snapshot-tex");
        let tex = match ctx.data(|d| d.get_temp::<(u64, egui::TextureHandle)>(id)) {
            Some((k, tex)) if k == key => tex,
            _ => {
                let img = crate::frames::to_color_image(&s.image);
                let img = if plain { img } else { transformed(&img, opts.channel, opts.colorized, opts.exposure, dc) };
                let tex = crate::frames::load_fitted(ctx, "viewer-snapshot", img, egui::TextureOptions::LINEAR);
                ctx.data_mut(|d| d.insert_temp(id, (key, tex.clone())));
                tex
            }
        };
        painter.image(tex.id(), comp_rect, uv, Color32::WHITE);
        painter.text(comp_rect.left_top() + vec2(6.0, 6.0), Align2::LEFT_TOP, crate::i18n::tr("Snapshot"), Tokens::ui(11.0), SNAP_COLOR);
        return;
    }
    // A GPU frame (the web viewer): drawn straight from its texture; Show Channel / exposure
    // read it back first (asynchronously in the browser: from the next frame on).
    let gpu = matches!((&app.viewer_shown, &app.viewer_native), (Some(s), Some(n)) if s.0 == n.0 && s.1 == n.1 && n.1.comp == cid.0);
    let rect = if gpu {
        let Some(id) = app.viewer_shown.map(|s| s.0) else { return };
        if plain || app.viewer_pixels().is_none() {
            if !plain {
                ctx.request_repaint();
            }
            painter.image(id, comp_rect, uv, Color32::WHITE);
            return;
        }
        comp_rect
    } else {
        let Some((tex, k, minify)) = app.viewer_tex.clone() else { return };
        if k.comp != cid.0 {
            return;
        }
        let zoom = comp_rect.width() as f64 / ectx.comp.width.max(1) as f64;
        let rect = match texture_roi(ctx) {
            Some([x, y, w, h]) => Rect::from_min_size(comp_rect.min + vec2((x * zoom) as f32, (y * zoom) as f32), vec2((w * zoom) as f32, (h * zoom) as f32)),
            None => comp_rect,
        };
        if plain {
            let uv = app.viewer_image.as_ref().map_or(uv, |src| fitted_uv(ctx, src.size, minify));
            painter.image(tex.id(), rect, uv, Color32::WHITE);
            return;
        }
        rect
    };
    let Some(src) = app.viewer_image.clone() else { return };
    let smooth = app.session.prefs.viewer_zoom_smooth();
    // Averaged down for the magnification, as the frame's own texture is (#417).
    let minify = super::viewer::minify_factor(smooth, src.size[0] as f64, (rect.width() * ctx.pixels_per_point()) as f64);
    let key = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (std::sync::Arc::as_ptr(&src) as usize, opts.channel.id(), opts.colorized, opts.exposure.to_bits(), &dc_key, minify).hash(&mut h);
        h.finish()
    };
    let id = egui::Id::new("viewer-display-tex");
    let tex = match ctx.data(|d| d.get_temp::<(u64, egui::TextureHandle)>(id)) {
        Some((k, tex)) if k == key => tex,
        _ => {
            let img =
                crate::frames::fit_texture_by(transformed(&src, opts.channel, opts.colorized, opts.exposure, dc), crate::frames::max_texture_side(ctx), minify);
            let tex = ctx.load_texture("viewer-display", img, super::viewer::zoom_texture_options(smooth));
            ctx.data_mut(|d| d.insert_temp(id, (key, tex.clone())));
            tex
        }
    };
    painter.image(tex.id(), rect, fitted_uv(ctx, src.size, minify), Color32::WHITE);
}

/// The texture coordinates of a frame of `size` in its texture, fitted with at least `minify`
/// (see [`crate::frames::fitted_uv`]).
fn fitted_uv(ctx: &egui::Context, size: [usize; 2], minify: usize) -> Rect {
    crate::frames::fitted_uv(size, crate::frames::fit_factor(size, crate::frames::max_texture_side(ctx), minify))
}

// ---------------------------------------------------------------- bottom bar

/// A popup list above `anchor` (selected entries highlighted); entries register
/// `viewer.<auto>.<n>` automation ids.
fn popup(app: &mut EffectcraftApp, ui: &mut egui::Ui, id: &str, anchor: Rect, items: &[(String, bool)], auto: &str) -> Option<usize> {
    let pid = egui::Id::new(id);
    if !widgets::popup_is_open(ui, pid) {
        return None;
    }
    let h: f32 = items.iter().map(|(l, _)| if l == "-" { 9.0 } else { 21.0 }).sum::<f32>() + 14.0;
    let seps = items.iter().filter(|(l, _)| l == "-").count();
    widgets::popup_list(ui, pid, anchor.left_top() - vec2(0.0, h + 4.0), anchor, items.len().saturating_sub(seps), seps, |ui| {
        ui.set_min_width(170.0);
        let mut chosen = None;
        for (i, (label, on)) in items.iter().enumerate() {
            if label == "-" {
                ui.separator();
                continue;
            }
            let r = ui.selectable_label(*on, label.as_str());
            app.auto.add(&format!("viewer.{auto}.{i}"), r.rect, label);
            if r.clicked() {
                chosen = Some(i);
            }
        }
        chosen
    })
}

fn toggle_popup(ui: &egui::Ui, id: &str) {
    let pid = egui::Id::new(id).with("open");
    let open: bool = ui.data(|d| d.get_temp(pid).unwrap_or(false));
    ui.data_mut(|d| d.insert_temp(pid, !open));
}

/// Magnification as After Effects shows it: "136.1 %", "100 %".
pub(crate) fn magnification_label(pct: f32) -> String {
    if (pct - pct.round()).abs() < 0.05 { format!("{:.0} %", pct.round()) } else { format!("{pct:.1} %") }
}

/// The resolution popup's label: the chosen factor, or the factor Auto resolves to in brackets.
pub(crate) fn resolution_label(res: Resolution, scale: f64) -> String {
    match res {
        Resolution::Auto => {
            let n = (1.0 / scale.clamp(1.0 / 40.0, 1.0)).round() as u32;
            match n {
                1 => "(Full)".into(),
                2 => "(Half)".into(),
                3 => "(Third)".into(),
                4 => "(Quarter)".into(),
                _ => format!("(1/{n})"),
            }
        }
        Resolution::Custom(n) => format!("Custom ({n})"),
        r => r.label().to_string(),
    }
}

/// The Composition panel's bottom bar, in After Effects 2026 order: magnification, resolution,
/// transparency grid, mask and shape path visibility, region of interest, grid and guide
/// options, show channel, reset exposure, exposure, take / show snapshot, Fast Previews, the 3D
/// renderer and view, and the current time.
pub(crate) fn bottom_bar(app: &mut EffectcraftApp, ui: &mut egui::Ui, bar: Rect, zoom: f32, fit: f32, time: Tick, comp: &Comp) {
    let t = app.tokens;
    let p = ui.painter().clone();
    p.rect_filled(bar, 0.0, t.panel_bg);
    p.line_segment([bar.left_top(), bar.right_top()], Stroke::new(1.0, t.separator));
    let cy = bar.center().y;
    let mut x = bar.min.x + 8.0;
    let ppp = ui.ctx().pixels_per_point();

    // Magnification.
    let r = Rect::from_min_size(pos2(x, cy - 10.0), vec2(80.0, 20.0));
    if widgets::dropdown(ui, r, &magnification_label(zoom * ppp * 100.0), &t, egui::Id::new("vw-mag"))
        .on_hover_text(crate::i18n::tr("Magnification ratio popup"))
        .clicked()
    {
        toggle_popup(ui, "vw-mag-pop");
    }
    app.auto.add("viewer.magnification", r, "Magnification");
    const MAGS: [&str; 15] = ["Fit", "Fit up to 100%", "-", "1.5%", "3.1%", "6.25%", "12.5%", "25%", "33.3%", "50%", "100%", "200%", "400%", "800%", "1600%"];
    let cur = (zoom * ppp * 100.0 * 10.0).round() / 10.0;
    let items: Vec<(String, bool)> = MAGS
        .iter()
        .map(|m| {
            let on = match *m {
                "Fit" => app.ui.viewer.zoom.is_none(),
                s => s.trim_end_matches('%').parse::<f32>().is_ok_and(|v| app.ui.viewer.zoom.is_some() && (v - cur).abs() < 0.06),
            };
            (m.to_string(), on)
        })
        .collect();
    if let Some(i) = popup(app, ui, "vw-mag-pop", r, &items, "magnification") {
        app.ui.viewer.pan = [0.0, 0.0];
        app.ui.viewer.zoom = match MAGS[i] {
            "Fit" => None,
            "Fit up to 100%" => Some(fit.min(1.0 / ppp)),
            s => s.trim_end_matches('%').parse::<f32>().ok().map(|v| v / 100.0 / ppp),
        };
    }
    x = r.max.x + 4.0;

    // Resolution / down-sample factor.
    let r = Rect::from_min_size(pos2(x, cy - 10.0), vec2(84.0, 20.0));
    let scale = app.viewer_scale(zoom, ppp);
    if widgets::dropdown(ui, r, &resolution_label(app.ui.viewer.res, scale), &t, egui::Id::new("vw-res"))
        .on_hover_text(crate::i18n::tr("Preview resolution: Auto shows full detail while paused (up to 4K), adapting during playback and dragging. Full/Half/Third/Quarter are fixed."))
        .clicked()
    {
        toggle_popup(ui, "vw-res-pop");
    }
    app.auto.add("viewer.resolution", r, "Resolution/Down Sample Factor");
    let res = app.ui.viewer.res;
    let items: Vec<(String, bool)> = vec![
        ("Full".into(), res == Resolution::Full),
        ("Half".into(), res == Resolution::Half),
        ("Third".into(), res == Resolution::Third),
        ("Quarter".into(), res == Resolution::Quarter),
        ("Custom...".into(), matches!(res, Resolution::Custom(_))),
        ("-".into(), false),
        ("Auto".into(), res == Resolution::Auto),
    ];
    if let Some(i) = popup(app, ui, "vw-res-pop", r, &items, "resolutionItem") {
        let ctx = ui.ctx().clone();
        let id = ["view.res.full", "view.res.half", "view.res.third", "view.res.quarter", "view.res.custom", "", "view.res.auto"][i];
        if let Err(e) = crate::menus::invoke(app, &ctx, id, json!({})) {
            app.ui.status = e;
        }
    }
    x = r.max.x + 6.0;

    let tog = |ui: &mut egui::Ui, auto: &mut crate::automation::Registry, x: &mut f32, icon: Icon, on: bool, id: &str, tip: &str| -> (bool, Rect) {
        let r = Rect::from_min_size(pos2(*x, cy - 11.0), vec2(22.0, 22.0));
        let resp = widgets::icon_button(ui, r, icon, on, &t, egui::Id::new(("vw-btn", id))).on_hover_text(tip);
        auto.add(&format!("viewer.{id}"), r, tip);
        *x += 24.0;
        (resp.clicked(), r)
    };
    if tog(ui, &mut app.auto, &mut x, Icon::Checker, app.ui.viewer.transparency_grid, "transparency", "Toggle Transparency Grid").0 {
        app.ui.viewer.transparency_grid = !app.ui.viewer.transparency_grid;
    }
    if tog(ui, &mut app.auto, &mut x, Icon::MaskVis, app.ui.viewer.show_masks, "masks", "Toggle Mask and Shape Path Visibility").0 {
        app.ui.viewer.show_masks = !app.ui.viewer.show_masks;
    }
    // Region of interest: with none, arm drawing (the next drag draws it); with one, clear it.
    let has_roi = app.session.state.region_of_interest.is_some();
    if tog(ui, &mut app.auto, &mut x, Icon::Region, has_roi || app.ui.viewer.roi_draw, "roi", "Region of Interest").0 {
        if has_roi {
            let _ = app.session.execute("view.setRegionOfInterest", json!({"rect": null}));
            app.ui.viewer.roi_draw = false;
        } else {
            app.ui.viewer.roi_draw = !app.ui.viewer.roi_draw;
        }
    }
    // Grid and guide options.
    let v = &app.ui.viewer;
    let any = v.grid || v.safe_margins || v.proportional_grid || (v.guides && !comp.guides.is_empty()) || v.rulers;
    let (clicked, gr) = tog(ui, &mut app.auto, &mut x, Icon::Grid, any, "grid", "Choose grid and guide options");
    if clicked {
        toggle_popup(ui, "vw-grid-pop");
    }
    let v = &app.ui.viewer;
    let items = vec![
        ("Title/Action Safe".to_string(), v.safe_margins),
        ("Proportional Grid".to_string(), v.proportional_grid),
        ("Grid".to_string(), v.grid),
        ("Guides".to_string(), v.guides),
        ("Rulers".to_string(), v.rulers),
        ("-".to_string(), false),
        ("3D Reference Axes".to_string(), app.session.prefs.three_d.show_reference_axes),
    ];
    if let Some(i) = popup(app, ui, "vw-grid-pop", gr, &items, "gridItem") {
        let v = &mut app.ui.viewer;
        match i {
            0 => v.safe_margins = !v.safe_margins,
            1 => v.proportional_grid = !v.proportional_grid,
            2 => v.grid = !v.grid,
            3 => v.guides = !v.guides,
            4 => v.rulers = !v.rulers,
            6 => app.session.prefs.three_d.show_reference_axes = !app.session.prefs.three_d.show_reference_axes,
            _ => {}
        }
    }
    // Show Channel and Color Management Settings.
    let vo = app.session.state.viewer.clone();
    let (clicked, cr) = tog(ui, &mut app.auto, &mut x, Icon::Channel, vo.channel != Channel::Rgb, "channel", "Show Channel and Color Management Settings");
    if clicked {
        toggle_popup(ui, "vw-channel-pop");
    }
    let mut items: Vec<(String, bool)> = Channel::ALL.iter().map(|c| (c.label().to_string(), vo.channel == *c)).collect();
    items.push(("-".into(), false));
    items.push(("Colorize".into(), vo.colorized));
    if let Some(i) = popup(app, ui, "vw-channel-pop", cr, &items, "channelItem") {
        let params =
            if i < Channel::ALL.len() { json!({"channel": Channel::ALL[i].id()}) } else { json!({"channel": vo.channel.id(), "colorized": !vo.colorized}) };
        let _ = app.session.execute("view.channel", params);
    }
    // Exposure.
    let exposed = vo.exposure != 0.0;
    if tog(ui, &mut app.auto, &mut x, Icon::Exposure, exposed, "resetExposure", "Reset Exposure").0 {
        let _ = app.session.execute("view.resetExposure", json!({}));
    }
    let er = Rect::from_min_size(pos2(x, cy - 10.0), vec2(42.0, 20.0));
    let mut ev = vo.exposure as f64;
    let resp = ui.put(
        er,
        egui::DragValue::new(&mut ev)
            .speed(0.05)
            .range(-40.0..=40.0)
            .fixed_decimals(1)
            .custom_formatter(|v, _| if v == 0.0 { "0.0".into() } else { format!("{v:+.1}") }),
    );
    let resp = resp.on_hover_text(crate::i18n::tr("Adjust Exposure (stops)"));
    app.auto.add("viewer.exposure", er, "Adjust Exposure");
    if resp.changed() {
        let _ = app.session.execute("view.exposure", json!({"stops": ev}));
    }
    x = er.max.x + 4.0;
    // Snapshots.
    if tog(ui, &mut app.auto, &mut x, Icon::Snapshot, false, "snapshot", "Take Snapshot").0 {
        let scale = app.viewer_scale(zoom, ppp);
        match app.session.execute("view.takeSnapshot", json!({"scale": scale.clamp(0.05, 1.0)})) {
            Ok(_) => app.ui.status = "Snapshot taken".into(),
            Err(e) => app.ui.status = e.to_string(),
        }
    }
    let showing = showing_snapshot(app, ui.ctx());
    let (_, sr) = tog(ui, &mut app.auto, &mut x, Icon::ShowSnapshot, showing, "showSnapshot", "Show Snapshot (hold)");
    // Show Snapshot works while the button is held, like F5.
    let held = ui.input(|i| i.pointer.primary_down()) && ui.input(|i| i.pointer.press_origin()).is_some_and(|o| sr.contains(o));
    if app.session.snapshot.is_some() && held != app.session.state.viewer.show_snapshot {
        let _ = app.session.execute("view.showSnapshot", json!({"value": held}));
    }
    // Fast Previews.
    let fp = vo.fast_previews;
    let (clicked, fr) = tog(ui, &mut app.auto, &mut x, Icon::Sparkle, fp != FastPreviews::Off || app.ui.viewer.fast_preview, "fastPreviews", "Fast Previews");
    if clicked {
        toggle_popup(ui, "vw-fast-pop");
    }
    let items: Vec<(String, bool)> = FastPreviews::ALL.iter().map(|m| (m.label().to_string(), fp == *m)).collect();
    if let Some(i) = popup(app, ui, "vw-fast-pop", fr, &items, "fastPreviewsItem") {
        let _ = app.session.execute("view.fastPreviewMode", json!({"mode": FastPreviews::ALL[i].id()}));
    }
    x += 4.0;
    // 3D renderer + view.
    if comp.has_3d() {
        let r = Rect::from_min_size(pos2(x, cy - 10.0), vec2(96.0, 20.0));
        let _ = widgets::dropdown(ui, r, "Classic 3D", &t, egui::Id::new("vw-3d"));
        app.auto.add("viewer.renderer3d", r, "3D Renderer");
        x = r.max.x + 4.0;
        let cid = app.session.active_comp_id();
        let cur = cid.and_then(|c| app.session.state.views3d.get(&c)).map(|v| v.current).unwrap_or_default();
        let r = Rect::from_min_size(pos2(x, cy - 10.0), vec2(118.0, 20.0));
        if widgets::dropdown(ui, r, cur.label(), &t, egui::Id::new("vw-cam")).clicked() {
            toggle_popup(ui, "vw-cam-pop");
        }
        app.auto.add("viewer.view3d", r, "3D View");
        use effectcraft_engine::render::three_d::View3D;
        let items: Vec<(String, bool)> = View3D::ALL.iter().map(|v| (v.label().to_string(), *v == cur)).collect();
        if let Some(i) = popup(app, ui, "vw-cam-pop", r, &items, "view3dItem") {
            let _ = app.session.execute("view.set3DView", json!({"view": View3D::ALL[i].id()}));
        }
        x = r.max.x + 4.0;
        // Extended Viewer (Settings ▸ 3D): custom views and Draft 3D show past the comp frame.
        let on = app.session.prefs.three_d.extended_viewer;
        if tog(ui, &mut app.auto, &mut x, Icon::ExtendedViewer, on, "extendedViewer", "Extended Viewer").0 {
            let _ = app.session.execute("view.extendedViewer", json!({}));
        }
        x += 2.0;
    }
    // Current time.
    let tc = crate::panels::timecode(&app.session, comp, time);
    let g = p.layout_no_wrap(tc, Tokens::mono(12.0), t.timecode);
    let tr = Rect::from_min_size(pos2(x + 4.0, cy - 9.0), g.size() + vec2(8.0, 4.0));
    p.galley(pos2(tr.min.x + 4.0, tr.min.y + 2.0), g, t.timecode);
    let resp = ui.interact(tr, egui::Id::new("vw-timecode"), Sense::click());
    app.auto.add("viewer.timecode", tr, "Current time");
    crate::panels::toggle_time_display(app, &resp);
    // Right: render time.
    let ms = app.frames.last_ms.lock().map(|v| *v).unwrap_or(0.0);
    if tr.max.x + 50.0 < bar.max.x {
        p.text(pos2(bar.max.x - 10.0, cy), Align2::RIGHT_CENTER, format!("{ms:.0} ms"), Tokens::ui(11.0), t.text_faint);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bar_labels_match_after_effects() {
        assert_eq!(magnification_label(136.08), "136.1 %");
        assert_eq!(magnification_label(100.0), "100 %");
        assert_eq!(magnification_label(49.97), "50 %");
        assert_eq!(resolution_label(Resolution::Auto, 1.0), "(Full)");
        assert_eq!(resolution_label(Resolution::Auto, 0.5), "(Half)");
        assert_eq!(resolution_label(Resolution::Auto, 0.125), "(1/8)");
        assert_eq!(resolution_label(Resolution::Quarter, 0.25), "Quarter");
        assert_eq!(resolution_label(Resolution::Custom(5), 0.2), "Custom (5)");
    }

    /// Auto renders the pixels the magnification needs, as in After Effects: Full above 50 %,
    /// Half down to 33.3 %, Third down to 25 %, Quarter below — never fewer pixels than the
    /// screen shows (#417: 60 % rendered at Half).
    #[test]
    fn auto_resolution_follows_the_magnification() {
        let auto = |zoom: f32, ppp: f32| resolution_label(Resolution::Auto, Resolution::Auto.scale(zoom, ppp));
        for (zoom, want) in [(2.0, "(Full)"), (1.0, "(Full)"), (0.6, "(Full)"), (0.51, "(Full)"), (0.5, "(Half)"), (0.46, "(Half)"), (0.34, "(Half)")] {
            assert_eq!(auto(zoom, 1.0), want, "{zoom}");
        }
        for (zoom, want) in [(1.0 / 3.0, "(Third)"), (0.26, "(Third)"), (0.25, "(Quarter)"), (0.1, "(Quarter)"), (0.0, "(Quarter)")] {
            assert_eq!(auto(zoom, 1.0), want, "{zoom}");
        }
        // Magnification in points on a 150 % display: 40 % shows 60 % of the comp's pixels.
        assert_eq!(auto(0.4, 1.5), "(Full)");
        assert_eq!(auto(f32::NAN, 1.0), "(Full)");
        assert_eq!(Resolution::Full.scale(0.1, 1.0), 1.0);
    }

    /// CPU frames go up averaged down to about one texel per screen pixel (More Accurate only),
    /// drawn with texture coordinates that keep each texel over its pixels (#417).
    #[test]
    fn minified_frames_keep_their_texels_in_place() {
        use crate::panels::viewer::minify_factor;
        assert_eq!(minify_factor(true, 1.0, 0.46), 2);
        assert_eq!(minify_factor(true, 1.0, 0.5), 2);
        assert_eq!(minify_factor(true, 1.0, 0.51), 1);
        assert_eq!(minify_factor(true, 0.5, 0.46), 1, "a Half frame at 46 %");
        assert_eq!(minify_factor(true, 1.0, 1.0 / 3.0), 3);
        assert_eq!(minify_factor(true, 1.0, 2.0), 1);
        assert_eq!(minify_factor(false, 1.0, 0.1), 1, "Faster keeps every pixel");
        assert_eq!(minify_factor(true, 1.0, 0.0), 256);
        assert_eq!(minify_factor(true, f64::NAN, 1.0), 1);
        // 2560 px by 3: 854 texels hold 2562 px, the image is 2560 / 2562 of them.
        let uv = crate::frames::fitted_uv([2560, 1440], 3);
        assert!((uv.max.x - 2560.0 / 2562.0).abs() < 1e-6 && uv.max.y == 1.0, "{uv:?}");
        assert_eq!(crate::frames::fitted_uv([7, 5], 1).max, egui::pos2(1.0, 1.0));
        let img =
            egui::ColorImage::new([5, 1], vec![egui::Color32::WHITE, egui::Color32::BLACK, egui::Color32::WHITE, egui::Color32::BLACK, egui::Color32::WHITE]);
        let out = crate::frames::fit_texture_by(img, 4096, 2);
        assert_eq!(out.size, [3, 1]);
        assert_eq!(out.pixels[1], egui::Color32::from_gray(128), "texels average their pixels");
        assert_eq!(out.pixels[2], egui::Color32::WHITE, "the last one only the pixels it holds");
    }
}
