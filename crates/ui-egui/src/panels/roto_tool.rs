//! Roto Brush and Refine Edge tools in the Layer panel: green foreground strokes, Alt/Option red
//! background strokes, Refine Edge strokes along soft edges; the matte overlay (Alpha Boundary in
//! pink, Alpha, Alpha Overlay), the segmentation span bar and the Freeze button.

use effectcraft_engine::effects::roto as fx;
use effectcraft_engine::project::{Comp, Layer};
use effectcraft_engine::render::{EvalCtx, RenderOpts, Renderer};
use egui::{Align2, Color32, Pos2, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use serde_json::json;

use crate::theme::Tokens;
use crate::{EffectcraftApp, widgets};

/// Position of the layer's first Roto Brush & Refine Edge effect.
pub fn roto_index(layer: &Layer) -> Option<usize> {
    layer.effects()?.groups().position(|g| matches!(&g.kind, effectcraft_engine::project::GroupKind::Effect { effect } if effect == fx::ID))
}

fn roto_group(layer: &Layer) -> Option<&effectcraft_engine::project::PropGroup> {
    layer.effects()?.groups().nth(roto_index(layer)?)
}

/// A stroke being drawn.
#[derive(Clone, Debug)]
struct Drag {
    layer: u64,
    kind: &'static str,
    points: Vec<[f64; 2]>,
}

#[derive(Clone)]
struct Overlay {
    key: (u64, u64, i64, u32, String),
    tex: egui::TextureHandle,
    rect: [f64; 4],
}

fn c32(c: [f64; 3], a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied((c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8, a)
}

/// Draw the matte overlay of the Roto Brush on `layer` (the panel shows the effect's input).
pub fn draw_overlay(
    app: &mut EffectcraftApp,
    ctx: &egui::Context,
    painter: &egui::Painter,
    comp: &Comp,
    layer: &Layer,
    scale: f64,
    to_screen: &dyn Fn([f64; 2]) -> Pos2,
) {
    let Some(idx) = roto_index(layer) else { return };
    let o = app.session.state.roto.clone();
    if o.view == "none" {
        return;
    }
    let Some(cid) = app.session.active_comp_id() else { return };
    let time = app.session.time();
    let key = (
        app.session.revision,
        layer.id.0,
        time.0,
        (scale * 1000.0) as u32,
        format!("{}{:?}{:?}{}", o.view, o.overlay_color, o.boundary_color, o.overlay_opacity),
    );
    let id = egui::Id::new("roto-overlay");
    let cached: Option<Overlay> = ctx.data(|d| d.get_temp(id));
    let ov = match cached {
        Some(c) if c.key == key => Some(c),
        _ => {
            let ectx = EvalCtx { project: &app.session.project, comp_id: cid, comp, time, expr: app.session.expr.as_deref(), footage: None };
            let mut r = Renderer::new(&app.session.project, &*app.session.footage, RenderOpts { scale, ..Default::default() });
            r.expr = app.session.expr.as_deref();
            r.cache = Some(&app.session.layer_cache);
            let (Some(inp), Some(out)) = (r.layer_input(&ectx, layer, idx), r.layer_input(&ectx, layer, idx + 1)) else { return };
            let (w, h) = (inp.img.width as usize, inp.img.height as usize);
            let a_at = |x: i64, y: i64| -> f32 {
                // Output pixel at the same layer point.
                let lx = (x as f64 + 0.5 - inp.offset[0]) / inp.scale;
                let ly = (y as f64 + 0.5 - inp.offset[1]) / inp.scale;
                let (ox, oy) = out.to_px([lx, ly]);
                out.img.get(ox.floor() as i64, oy.floor() as i64)[3]
            };
            let alpha: Vec<f32> = (0..w * h).map(|i| a_at((i % w) as i64, (i / w) as i64)).collect();
            let inside = |x: i64, y: i64| x >= 0 && y >= 0 && x < w as i64 && y < h as i64 && alpha[y as usize * w + x as usize] >= 0.5;
            let mut px = vec![Color32::TRANSPARENT; w * h];
            let bc = c32(o.boundary_color, 255);
            let oc = c32(o.overlay_color, (o.overlay_opacity / 100.0 * 255.0).clamp(0.0, 255.0) as u8);
            // View Search Region: the band around the boundary (yellow).
            let search = roto_group(layer).filter(|g| g.prop("rotoBrushPropagation/viewSearchRegion").is_some_and(|p| p.value.as_bool())).map(|g| {
                let r = g.prop("rotoBrushPropagation/searchRadius").map(|p| p.value.as_f64()).unwrap_or(15.0) * scale;
                let bin: Vec<u8> = alpha.iter().map(|a| (*a >= 0.5) as u8).collect();
                let sd = effectcraft_engine::track::roto::matting::signed_distance(&bin, w, h);
                (sd, r as f32)
            });
            for y in 0..h as i64 {
                for x in 0..w as i64 {
                    let i = y as usize * w + x as usize;
                    let a = alpha[i];
                    px[i] = match o.view.as_str() {
                        "alpha" => {
                            let v = (a.clamp(0.0, 1.0) * 255.0) as u8;
                            Color32::from_rgb(v, v, v)
                        }
                        "alphaOverlay" if a < 0.5 => oc,
                        "alphaOverlay" => Color32::TRANSPARENT,
                        _ => {
                            let me = a >= 0.5;
                            let edge = me && [(-1, 0), (1, 0), (0, -1), (0, 1)].iter().any(|(dx, dy)| !inside(x + dx, y + dy));
                            if edge { bc } else { Color32::TRANSPARENT }
                        }
                    };
                    if let Some((sd, r)) = &search {
                        let d = sd[i].abs();
                        if (d - r).abs() < 0.75 {
                            px[i] = Color32::from_rgb(255, 220, 0);
                        }
                    }
                }
            }
            let img = egui::ColorImage { size: [w, h], pixels: px, source_size: egui::Vec2::new(w as f32, h as f32) };
            let x0 = -inp.offset[0] / inp.scale;
            let y0 = -inp.offset[1] / inp.scale;
            let rect = [x0, y0, x0 + w as f64 / inp.scale, y0 + h as f64 / inp.scale];
            if !crate::frames::presentation_check(ctx, img.size, &mut app.ui.status) {
                return;
            }
            let tex = ctx.load_texture("roto-overlay", img, egui::TextureOptions::NEAREST);
            let c = Overlay { key, tex, rect };
            ctx.data_mut(|d| d.insert_temp(id, c.clone()));
            Some(c)
        }
    };
    if let Some(ov) = ov
        && crate::frames::presentation_check(ctx, ov.tex.size(), &mut app.ui.status)
    {
        let r = Rect::from_min_max(to_screen([ov.rect[0], ov.rect[1]]), to_screen([ov.rect[2], ov.rect[3]]));
        painter.image(ov.tex.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    }
}

/// Pointer handling for the Roto Brush / Refine Edge tools. Returns true when it handled input.
pub fn interact(
    app: &mut EffectcraftApp,
    ui: &mut egui::Ui,
    painter: &egui::Painter,
    area: Rect,
    layer: &Layer,
    zoom: f64,
    to_screen: &dyn Fn([f64; 2]) -> Pos2,
    to_layer: &dyn Fn(Pos2) -> [f64; 2],
) -> bool {
    let ctx = ui.ctx().clone();
    let mods = ui.input(|i| i.modifiers);
    let Some(kind) = app.ui.tool.roto_kind(mods.alt) else { return false };
    let resp = ui.interact(area, egui::Id::new("layer-panel-roto"), Sense::click_and_drag());
    let o = app.session.state.roto.clone();
    let refine = kind.starts_with("refine");
    let dia = if refine { o.refine_diameter } else { o.diameter };
    let col = match kind {
        "fg" => Color32::from_rgb(40, 220, 60),
        "bg" => Color32::from_rgb(235, 50, 40),
        "refine" => Color32::from_rgb(110, 200, 255),
        _ => Color32::from_rgb(255, 160, 60),
    };
    if let Some(hp) = resp.hover_pos() {
        let lp = to_layer(hp);
        app.pointer_comp = Some([lp[0] as f32, lp[1] as f32]);
        ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
        let r = (dia * 0.5 * zoom) as f32;
        painter.circle_stroke(hp, r, Stroke::new(1.5, col));
        let sign = if kind == "bg" || kind == "refineErase" { "−" } else { "+" };
        painter.text(hp, Align2::CENTER_CENTER, sign, Tokens::semibold(12.0), col);
    }
    let did = egui::Id::new("layer-panel-roto-drag");
    if resp.drag_started()
        && let Some(pos) = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos())
    {
        ctx.data_mut(|d| d.insert_temp(did, Drag { layer: layer.id.0, kind, points: vec![to_layer(pos)] }));
    }
    if let Some(mut d) = ctx.data(|d| d.get_temp::<Drag>(did)) {
        if let Some(pos) = resp.interact_pointer_pos() {
            let lp = to_layer(pos);
            let last = d.points.last().copied().unwrap_or(lp);
            if (lp[0] - last[0]).hypot(lp[1] - last[1]) * zoom >= 1.5 {
                d.points.push(lp);
                ctx.data_mut(|dd| dd.insert_temp(did, d.clone()));
            }
        }
        let width = ((dia * zoom) as f32).max(1.0);
        let pts: Vec<Pos2> = d.points.iter().map(|q| to_screen(*q)).collect();
        let c = col.gamma_multiply(0.6);
        if pts.len() == 1 {
            painter.circle_filled(pts[0], width / 2.0, c);
        } else {
            painter.add(egui::Shape::line(pts, Stroke::new(width, c)));
        }
        if resp.drag_stopped() {
            ctx.data_mut(|dd| dd.remove::<Drag>(did));
            run_stroke(app, d.layer, d.kind, &d.points, dia / 2.0);
        }
    } else if resp.clicked()
        && let Some(pos) = resp.interact_pointer_pos()
    {
        run_stroke(app, layer.id.0, kind, &[to_layer(pos)], dia / 2.0);
    }
    true
}

fn run_stroke(app: &mut EffectcraftApp, layer: u64, kind: &str, points: &[[f64; 2]], radius: f64) {
    let r = app.session.execute("roto.stroke", json!({"layer": layer, "kind": kind, "points": points, "radius": radius}));
    if let Err(e) = r {
        app.ui.status = e.to_string();
    }
}

/// The segmentation span bar (above the panel's bottom bar): the layer's frames, the span, the
/// base frame (yellow), computed frames (green) and stroke frames. Click to go to a frame; drag
/// the span's ends to change it.
pub fn span_bar(app: &mut EffectcraftApp, ui: &mut egui::Ui, p: &egui::Painter, r: Rect, comp: &Comp, layer: &Layer) {
    let t = app.tokens;
    let Some(g) = roto_group(layer) else { return };
    let Some(cid) = app.session.active_comp_id() else { return };
    let uid = g.uid;
    let params = effectcraft_engine::effects::flatten_params(g, &mut |p| p.value.clone());
    let d = fx::data(&params);
    p.rect_filled(r, 0.0, t.panel_bg);
    let lim = effectcraft_engine::roto::frame_limits(comp, layer);
    let n = (lim[1] - lim[0] + 1).max(1) as f32;
    let track = Rect::from_min_max(pos2(r.min.x + 10.0, r.min.y + 4.0), pos2(r.max.x - 10.0, r.max.y - 4.0));
    let x_of = |f: f64| track.min.x + ((f - lim[0] as f64) / n as f64) as f32 * track.width();
    let f_of = |x: f32| (lim[0] as f64 + ((x - track.min.x) / track.width()).clamp(0.0, 0.9999) as f64 * n as f64).floor() as i64;
    p.rect_filled(track, 2.0, t.pasteboard);
    if let Some(base) = d.base {
        let sp = Rect::from_min_max(pos2(x_of(d.span[0] as f64), track.min.y), pos2(x_of(d.span[1] as f64 + 1.0), track.max.y));
        let frozen = fx::is_frozen(&params);
        p.rect_filled(sp, 2.0, if frozen { Color32::from_rgb(60, 110, 190) } else { Color32::from_rgb(90, 90, 96) });
        for (f, done) in app.session.roto_computed(cid, layer.id, uid) {
            if done {
                let cr = Rect::from_min_max(pos2(x_of(f as f64), track.max.y - 4.0), pos2(x_of(f as f64 + 1.0), track.max.y));
                p.rect_filled(cr, 0.0, Color32::from_rgb(70, 200, 90));
            }
        }
        for f in d.stroke_frames() {
            let x = x_of(f as f64 + 0.5);
            p.line_segment([pos2(x, track.min.y), pos2(x, track.min.y + 5.0)], Stroke::new(2.0, Color32::WHITE));
        }
        let bx = x_of(base as f64 + 0.5);
        p.rect_filled(Rect::from_center_size(pos2(bx, track.center().y), vec2(6.0, track.height())), 1.0, Color32::from_rgb(240, 200, 40));
    }
    // Current time.
    let fps = comp.frame_rate.as_f64();
    let cur = fx::frame_of(layer.layer_time(app.session.time()).seconds(), fps);
    let cx = x_of(cur as f64 + 0.5);
    p.line_segment([pos2(cx, r.min.y), pos2(cx, r.max.y)], Stroke::new(1.0, t.timecode));
    p.rect_stroke(track, 2.0, Stroke::new(1.0, Color32::from_black_alpha(140)), StrokeKind::Outside);
    app.auto.add("layerPanel.rotoSpan", track, "Segmentation span");
    let resp = ui.interact(track, egui::Id::new("roto-span-bar"), Sense::click_and_drag());
    let hid = egui::Id::new("roto-span-handle");
    if resp.drag_started()
        && let (Some(pos), Some(_)) = (resp.interact_pointer_pos(), d.base)
    {
        let (a, b) = (x_of(d.span[0] as f64), x_of(d.span[1] as f64 + 1.0));
        let h = if (pos.x - a).abs() < 6.0 {
            1
        } else if (pos.x - b).abs() < 6.0 {
            2
        } else {
            0
        };
        ui.ctx().data_mut(|m| m.insert_temp(hid, h));
    }
    let handle: i32 = ui.ctx().data(|m| m.get_temp(hid)).unwrap_or(0);
    if let Some(pos) = resp.interact_pointer_pos() {
        let f = f_of(pos.x);
        if handle == 0 && (resp.clicked() || resp.dragged()) {
            let t = layer.comp_time(comp.frame_rate.tick_of(f));
            let _ = app.session.execute("time.set", json!({"time": t.seconds()}));
        }
        if handle != 0 && resp.dragged() {
            p.line_segment([pos2(pos.x, track.min.y), pos2(pos.x, track.max.y)], Stroke::new(2.0, Color32::from_rgb(240, 200, 40)));
        }
        if handle != 0 && resp.drag_stopped() {
            let k = if handle == 1 { "start" } else { "end" };
            if let Err(e) = app.session.execute("roto.span", json!({"layer": layer.id.0, "effect": uid, k: f})) {
                app.ui.status = e.to_string();
            }
        }
    }
    if resp.drag_stopped() {
        ui.ctx().data_mut(|m| m.remove::<i32>(hid));
    }
}

/// View-mode toggles and Freeze / Propagate buttons in the Layer panel's bottom bar, from `x`.
pub fn bar_buttons(app: &mut EffectcraftApp, ui: &mut egui::Ui, p: &egui::Painter, bar: Rect, x: f32, layer: &Layer) {
    let t = app.tokens;
    let mut x = x;
    let view = app.session.state.roto.view.clone();
    for (id, label) in [("alpha", "Alpha"), ("alphaBoundary", "Boundary"), ("alphaOverlay", "Overlay")] {
        let r = Rect::from_min_size(pos2(x, bar.center().y - 10.0), vec2(66.0, 20.0));
        let on = view == id;
        if widgets::text_button(ui, r, label, on, &t, egui::Id::new(("roto-view", id))).clicked() {
            let v = if on { "none" } else { id };
            let _ = app.session.execute("roto.options", json!({"view": v}));
        }
        app.auto.add(&format!("layerPanel.roto.view.{id}"), r, label);
        x += 70.0;
    }
    let Some(g) = roto_group(layer) else { return };
    let uid = g.uid;
    let params = effectcraft_engine::effects::flatten_params(g, &mut |p| p.value.clone());
    let frozen = fx::is_frozen(&params);
    let running = app.session.is_roto_running();
    x += 8.0;
    let r = Rect::from_min_size(pos2(x, bar.center().y - 10.0), vec2(74.0, 20.0));
    let label = if frozen { "Unfreeze" } else { "Freeze" };
    if widgets::text_button(ui, r, label, frozen, &t, egui::Id::new(("roto-freeze", uid))).clicked() {
        let id = if frozen { "roto.unfreeze" } else { "roto.freeze" };
        if let Err(e) = app.session.execute(id, json!({"layer": layer.id.0, "effect": uid})) {
            app.ui.status = e.to_string();
        }
    }
    app.auto.add("layerPanel.roto.freeze", r, label);
    x += 80.0;
    if running {
        let r = Rect::from_min_size(pos2(x, bar.center().y - 10.0), vec2(60.0, 20.0));
        if widgets::text_button(ui, r, "Stop", false, &t, egui::Id::new("roto-stop")).clicked() {
            let _ = app.session.execute("roto.cancel", json!({}));
        }
        app.auto.add("layerPanel.roto.stop", r, "Stop");
        if let Some(pr) = app.session.roto_progress() {
            p.text(pos2(r.max.x + 8.0, bar.center().y), Align2::LEFT_CENTER, pr.banner(), Tokens::ui(11.5), t.text_dim);
        }
    }
}
