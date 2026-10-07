//! The Layer panel: one layer in its own (layer) space, where the Brush, Clone Stamp and Eraser
//! tools paint. Double-clicking a footage or solid layer (or Layer ▸ Open Layer) opens it.
//!
//! The image is the layer's source and masks followed by its effects up to the View menu's
//! choice (by default the last Paint effect, as After Effects shows "Paint"). Dragging with a
//! paint tools records the stroke's points in layer space and, on release, runs `paint.stroke`
//! at the current time (the drag's duration feeds Write On). Alt/Option-click with the Clone
//! Stamp sets the clone source.
//!
//! With a Roto Brush & Refine Edge effect the panel shows the effect's input with the matte
//! overlay, the segmentation span bar and the view / Freeze buttons; the Roto Brush and Refine
//! Edge tools paint their strokes here (see [`super::roto_tool`]).

use effectcraft_engine::effects::paint;
use effectcraft_engine::project::{Layer, LayerId};
use effectcraft_engine::render::{EvalCtx, RenderOpts, Renderer};
use egui::{Align2, Color32, Pos2, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use serde_json::json;

use crate::state::Tool;
use crate::theme::Tokens;
use crate::{EffectcraftApp, widgets};

/// A stroke being drawn.
#[derive(Clone, Debug)]
struct Drag {
    layer: u64,
    points: Vec<[f64; 2]>,
    started: f64,
}

/// Cached texture of the rendered layer: (revision, layer, time, view, scale) → texture and the
/// layer-space rectangle it covers.
#[derive(Clone)]
struct Tex {
    key: (u64, u64, i64, usize, u32),
    tex: egui::TextureHandle,
    /// Layer-space rect of the image `[x0, y0, x1, y1]`.
    rect: [f64; 4],
}

fn tex_id() -> egui::Id {
    egui::Id::new("layer-panel-tex")
}

/// The layer shown: the opened one, else the first selected layer.
pub fn current_layer(app: &EffectcraftApp) -> Option<Layer> {
    let comp = app.session.active_comp()?;
    let id = app.ui.layer_panel.map(LayerId).filter(|id| comp.layer(*id).is_some()).or_else(|| app.session.state.selected_layers.first().copied())?;
    comp.layer(id).cloned()
}

/// Number of effects shown by default: the Roto Brush's input while a Roto Brush tool is active
/// (or when there is no Paint effect), else through the last Paint effect (none when there
/// isn't one).
fn default_view(layer: &Layer, tool: Tool) -> usize {
    let paint = layer.effects().and_then(|fx| fx.groups().enumerate().filter(|(_, g)| paint::is_paint(g)).map(|(i, _)| i + 1).last());
    match super::roto_tool::roto_index(layer) {
        Some(r) if tool.roto_kind(false).is_some() || paint.is_none() => r,
        _ => paint.unwrap_or(0),
    }
}

pub fn show(app: &mut EffectcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let ctx = ui.ctx().clone();
    let p = ui.painter().with_clip_rect(rect);
    let (Some(cid), Some(layer)) = (app.session.active_comp_id(), current_layer(app)) else {
        p.rect_filled(rect, 0.0, t.pasteboard);
        p.text(rect.center(), Align2::CENTER_CENTER, "Double-click a layer to open it in the Layer panel", Tokens::ui(12.0), t.text_faint);
        app.auto.add("layerPanel.empty", rect, "Layer panel (empty)");
        return;
    };
    let comp = app.session.project.comp_arc(cid).unwrap_or_else(|| effectcraft_engine::project::Comp::new(1, 1, Default::default(), Default::default()).into());
    let time = app.session.time();
    let fx_count = layer.effects().map(|f| f.groups().count()).unwrap_or(0);
    let view = app.ui.layer_view.map(|v| v.min(fx_count)).unwrap_or_else(|| default_view(&layer, app.ui.tool));
    let roto = super::roto_tool::roto_index(&layer);

    // Title strip.
    let top = Rect::from_min_size(rect.min, vec2(rect.width(), 24.0));
    p.rect_filled(top, 0.0, t.panel_bg);
    p.text(pos2(top.min.x + 10.0, top.center().y), Align2::LEFT_CENTER, format!("Layer: {}", layer.name), Tokens::semibold(12.0), t.text);
    app.auto.add("layerPanel.title", top, &layer.name);
    // Bottom bar: View menu.
    let bar = Rect::from_min_max(pos2(rect.min.x, rect.max.y - 30.0), rect.max);
    p.rect_filled(bar, 0.0, t.panel_bg);
    p.text(pos2(bar.min.x + 10.0, bar.center().y), Align2::LEFT_CENTER, "View:", Tokens::ui(12.0), t.text_dim);
    let mut names = vec!["Masks".to_string()];
    if let Some(fx) = layer.effects() {
        names.extend(fx.groups().map(|g| g.name.clone()));
    }
    let vr = Rect::from_min_size(pos2(bar.min.x + 46.0, bar.center().y - 10.0), vec2(150.0, 20.0));
    let pop = egui::Id::new("layer-view-pop");
    if widgets::dropdown(ui, vr, &names[view.min(names.len() - 1)], &t, egui::Id::new("layer-view")).clicked() {
        widgets::open_popup(ui, pop);
    }
    app.auto.add("layerPanel.view", vr, "View");
    if let Some(i) = widgets::popup_menu(ui, pop, vr.left_bottom(), &names, Some(view)) {
        app.ui.layer_view = Some(i);
    }
    let tc = super::timecode(&app.session, &comp, time);
    p.text(pos2(bar.max.x - 10.0, bar.center().y), Align2::RIGHT_CENTER, tc, Tokens::mono(12.0), t.timecode);
    // Roto Brush: view / Freeze buttons and the segmentation span bar.
    let mut canvas_bottom = bar.min.y;
    if roto.is_some() {
        super::roto_tool::bar_buttons(app, ui, &p, bar, vr.max.x + 12.0, &layer);
        let sb = Rect::from_min_max(pos2(rect.min.x, bar.min.y - 18.0), pos2(rect.max.x, bar.min.y));
        super::roto_tool::span_bar(app, ui, &p, sb, &comp, &layer);
        canvas_bottom = sb.min.y;
    }

    // Canvas.
    let area = Rect::from_min_max(pos2(rect.min.x, top.max.y), pos2(rect.max.x, canvas_bottom));
    p.rect_filled(area, 0.0, t.pasteboard);
    let (w, h) = effectcraft_engine::render::source_size(&app.session.project, &layer);
    let (lw, lh) = if w == 0 { (comp.width as f64, comp.height as f64) } else { (w as f64, h as f64) };
    let zoom = ((area.width() - 40.0) as f64 / lw).min((area.height() - 40.0) as f64 / lh).max(0.01);
    let origin = area.center() - vec2((lw * zoom / 2.0) as f32, (lh * zoom / 2.0) as f32);
    let to_screen = |q: [f64; 2]| pos2(origin.x + (q[0] * zoom) as f32, origin.y + (q[1] * zoom) as f32);
    let to_layer = |s: Pos2| [((s.x - origin.x) as f64) / zoom, ((s.y - origin.y) as f64) / zoom];
    let frame = Rect::from_min_max(to_screen([0.0, 0.0]), to_screen([lw, lh]));
    let painter = p.with_clip_rect(area);
    super::viewer::checker(&painter, frame);

    // Render (synchronously; the layer cache makes repeats cheap).
    let ppp = ctx.pixels_per_point() as f64;
    let scale = (zoom * ppp).clamp(0.05, 1.0);
    let scale = if scale > 0.75 {
        1.0
    } else if scale > 0.4 {
        0.5
    } else {
        0.25
    };
    let key = (app.session.revision, layer.id.0, time.0, view, (scale * 1000.0) as u32);
    let cached: Option<Tex> = ctx.data(|d| d.get_temp(tex_id()));
    let tex = match cached {
        Some(c) if c.key == key => Some(c),
        _ => {
            let ectx = EvalCtx { project: &app.session.project, comp_id: cid, comp: &comp, time, expr: app.session.expr.as_deref(), footage: None };
            let mut r = Renderer::new(&app.session.project, &*app.session.footage, RenderOpts { scale, ..Default::default() });
            r.expr = app.session.expr.as_deref();
            r.cache = Some(&app.session.layer_cache);
            r.layer_input(&ectx, &layer, view).and_then(|buf| {
                if !crate::frames::presentation_check(&ctx, [buf.img.width as usize, buf.img.height as usize], &mut app.ui.status) {
                    return None;
                }
                let img = crate::frames::to_color_image(&buf.img);
                let x0 = -buf.offset[0] / buf.scale;
                let y0 = -buf.offset[1] / buf.scale;
                let rect = [x0, y0, x0 + buf.img.width as f64 / buf.scale, y0 + buf.img.height as f64 / buf.scale];
                let tex = ctx.load_texture("layer-panel", img, egui::TextureOptions::LINEAR);
                let c = Tex { key, tex, rect };
                ctx.data_mut(|d| d.insert_temp(tex_id(), c.clone()));
                Some(c)
            })
        }
    };
    if let Some(tx) = &tex
        && crate::frames::presentation_check(&ctx, tx.tex.size(), &mut app.ui.status)
    {
        let r = Rect::from_min_max(to_screen([tx.rect[0], tx.rect[1]]), to_screen([tx.rect[2], tx.rect[3]]));
        painter.image(tx.tex.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    }
    if roto.is_some_and(|r| r == view) {
        super::roto_tool::draw_overlay(app, &ctx, &painter, &comp, &layer, scale, &to_screen);
    }
    painter.rect_stroke(frame, 0.0, Stroke::new(1.0, Color32::from_black_alpha(160)), StrokeKind::Outside);
    app.auto.add("layerPanel.canvas", frame, &layer.name);
    app.auto.add("layerPanel.area", area, "Layer panel");

    // Interaction: Roto Brush / Refine Edge.
    if app.ui.tool.roto_kind(false).is_some() {
        super::roto_tool::interact(app, ui, &painter, area, &layer, zoom, &to_screen, &to_layer);
        return;
    }
    // Interaction: paint tools.
    let resp = ui.interact(area, egui::Id::new("layer-panel-interact"), Sense::click_and_drag());
    let kind = app.ui.tool.paint_kind();
    let o = app.session.state.paint.clone();
    let did = egui::Id::new("layer-panel-drag");
    if let Some(hp) = resp.hover_pos() {
        let lp = to_layer(hp);
        app.pointer_comp = Some([lp[0] as f32, lp[1] as f32]);
        if kind.is_some() {
            ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
            // Brush tip outline.
            let rad = (o.diameter * 0.5 * zoom) as f32;
            let ang = (o.angle as f32).to_radians();
            let rnd = (o.roundness / 100.0).clamp(0.01, 1.0) as f32;
            let pts: Vec<Pos2> = (0..48)
                .map(|i| {
                    let a = i as f32 / 48.0 * std::f32::consts::TAU;
                    let (x, y) = (a.cos() * rad, a.sin() * rad * rnd);
                    hp + vec2(x * ang.cos() - y * ang.sin(), x * ang.sin() + y * ang.cos())
                })
                .collect();
            painter.add(egui::Shape::closed_line(pts, Stroke::new(1.0, Color32::from_white_alpha(200))));
            // Clone source overlay.
            if app.ui.tool == Tool::Clone
                && o.show_overlay
                && let (Some(tx), Some(src)) = (&tex, o.clone_point)
            {
                let off = o.clone_offset.unwrap_or([src[0] - lp[0], src[1] - lp[1]]);
                let r = Rect::from_min_max(to_screen([tx.rect[0] - off[0], tx.rect[1] - off[1]]), to_screen([tx.rect[2] - off[0], tx.rect[3] - off[1]]));
                let a = (o.overlay_opacity / 100.0 * 255.0).clamp(0.0, 255.0) as u8;
                painter.with_clip_rect(frame).image(tx.tex.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::from_white_alpha(a));
            }
        }
    }
    if let Some(src) = o.clone_point.filter(|_| app.ui.tool == Tool::Clone && o.clone_source.is_none_or(|l| l == layer.id.0)) {
        let s = to_screen(src);
        painter.line_segment([s - vec2(6.0, 0.0), s + vec2(6.0, 0.0)], Stroke::new(1.0, Color32::WHITE));
        painter.line_segment([s - vec2(0.0, 6.0), s + vec2(0.0, 6.0)], Stroke::new(1.0, Color32::WHITE));
    }
    let mods = ui.input(|i| i.modifiers);
    let now = ui.input(|i| i.time);
    // Alt/Option-click with the Clone Stamp sets the source.
    if app.ui.tool == Tool::Clone
        && mods.alt
        && resp.clicked()
        && let Some(pos) = resp.interact_pointer_pos()
    {
        let lp = to_layer(pos);
        let _ = app.session.execute("paint.setCloneSource", json!({"layer": layer.id.0, "point": lp}));
        return;
    }
    if let Some(kind) = kind
        && !mods.alt
    {
        if resp.drag_started()
            && let Some(pos) = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos())
        {
            ctx.data_mut(|d| d.insert_temp(did, Drag { layer: layer.id.0, points: vec![to_layer(pos)], started: now }));
        }
        if let Some(mut d) = ctx.data(|d| d.get_temp::<Drag>(did)) {
            if let Some(pos) = resp.interact_pointer_pos() {
                let lp = to_layer(pos);
                let last = d.points.last().copied().unwrap_or(lp);
                if (lp[0] - last[0]).hypot(lp[1] - last[1]) * zoom >= 1.0 {
                    d.points.push(lp);
                    ctx.data_mut(|dd| dd.insert_temp(did, d.clone()));
                }
            }
            // Live preview of the stroke.
            let col = if kind == "eraser" {
                Color32::from_white_alpha(140)
            } else {
                Color32::from_rgba_unmultiplied((o.color[0] * 255.0) as u8, (o.color[1] * 255.0) as u8, (o.color[2] * 255.0) as u8, 200)
            };
            let width = ((o.diameter * zoom) as f32).max(1.0);
            let pts: Vec<Pos2> = d.points.iter().map(|q| to_screen(*q)).collect();
            if pts.len() == 1 {
                painter.circle_filled(pts[0], width / 2.0, col);
            } else {
                painter.add(egui::Shape::line(pts, Stroke::new(width, col)));
            }
            if resp.drag_stopped() {
                ctx.data_mut(|dd| dd.remove::<Drag>(did));
                let pts: Vec<[f64; 2]> = d.points.clone();
                let r = app.session.execute("paint.stroke", json!({"layer": d.layer, "kind": kind, "points": pts, "duration": (now - d.started).max(0.0)}));
                if let Err(e) = r {
                    app.ui.status = e.to_string();
                }
            }
        }
        if resp.clicked()
            && let Some(pos) = resp.interact_pointer_pos()
        {
            let r = app.session.execute("paint.stroke", json!({"layer": layer.id.0, "kind": kind, "points": [to_layer(pos)], "duration": 0.0}));
            if let Err(e) = r {
                app.ui.status = e.to_string();
            }
        }
    }
}
