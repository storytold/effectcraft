//! Effect Controls: the selected layer's effects with all parameters, in After Effects'
//! conventions:
//!
//! - effect headers with the fx switch, twirl, Reset and About…; click selects the effect
//!   (Edit ▸ Copy / Paste / Duplicate and Delete then act on it), drag reorders the stack, the
//!   context menu copies, pastes, duplicates, resets and removes;
//! - stopwatch per parameter and a keyframe navigator (◀ ◆ ▶) on animated ones;
//! - sliders and angle dials twirl down; angles read `0x+45.0°`;
//! - point parameters have the crosshair button (then click in the Composition viewer) and an
//!   on-viewer ⊕ control while their effect is selected (see [`viewer_hook`]);
//! - colour parameters have a swatch and an eyedropper that samples the composited frame;
//! - layer parameters have a layer popup and a Source / Masks / Effects & Masks popup; mask
//!   ("Path") parameters list the layer's masks;
//! - Curves has its graph editor, Levels its histogram with input/output triangles, Auto
//!   Levels a histogram; Lumetri's curves, Colorama's output cycle, Glow's colour map and
//!   Reshape's correspondence points have visual editors ([`super::fx_editors`]).

use effectcraft_engine::geom::Mat3;
use effectcraft_engine::keyframe::Value;
use effectcraft_engine::project::{GroupKind, Layer, Node, ParamUi, PropGroup, Property};
use effectcraft_engine::render::EvalCtx;
use egui::{Align2, Color32, Rect, Sense, Stroke, pos2, vec2};
use serde_json::json;

use super::fx_widgets as fw;
use crate::icons::{self, Icon};
use crate::state::FxPick;
use crate::theme::Tokens;
use crate::{EffectcraftApp, widgets};

type Actions = Vec<(String, serde_json::Value)>;

const ROW: f32 = 20.0;
const DIAL_ROW: f32 = 64.0;
// The effect title starts at 44 pt. Each level's names sit one tree step farther in;
// reserve the same icon gutter for properties and groups so sibling names align.
const PARAM_NAME_X: f32 = 58.0;
const TREE_INDENT: f32 = 14.0;

fn selected_layer(app: &EffectcraftApp) -> Option<Layer> {
    let comp = app.session.active_comp()?;
    let id = app.session.state.selected_layers.first()?;
    comp.layer(*id).cloned()
}

/// An effect was just applied (`effect.apply` / `effect.applyLast` with `params`): Effect
/// Controls comes up (opened if closed, brought to the front, the focus left where it is) on the
/// layer the effect went to, with the new effect selected, as in After Effects.
pub fn reveal_applied(app: &mut EffectcraftApp, params: &serde_json::Value) {
    // `effect.apply` selects the new effect on (the last of) the layers it went to.
    let target = app.session.state.selected_props.last().map(|(l, _)| *l);
    if let Some(l) = target
        && !app.session.state.selected_layers.contains(&l)
        && params.get("comp").is_none()
        && app.session.active_comp().is_some_and(|c| c.layer(l).is_some())
    {
        // Show that layer (dropped on an unselected one), keeping the new effect selected.
        let props = app.session.state.selected_props.clone();
        match app.session.execute("layer.select", json!({"layers": [l.0]})) {
            Ok(_) => app.session.state.selected_props = props,
            Err(e) => app.ui.status = e.to_string(),
        }
    }
    app.raise_panel(crate::dock::PanelKind::EffectControls);
}

/// An undo-merge key for one gesture on `id`: a new key each time a drag starts, so separate
/// drags are separate undo steps while one drag is one step.
pub(super) fn gesture_key(ui: &egui::Ui, id: egui::Id, started: bool) -> String {
    let kid = id.with("gesture");
    let n: u64 = if started {
        let n = ui.data(|d| d.get_temp::<u64>(kid)).unwrap_or(0) + 1;
        ui.data_mut(|d| d.insert_temp(kid, n));
        n
    } else {
        ui.data(|d| d.get_temp::<u64>(kid)).unwrap_or(0)
    };
    format!("{id:?}-{n}")
}

fn triangle(p: &egui::Painter, c: egui::Pos2, left: bool, col: Color32) {
    let (dx, h) = (if left { -3.5 } else { 3.5 }, 4.0);
    p.add(egui::Shape::convex_polygon(vec![pos2(c.x + dx, c.y), pos2(c.x - dx, c.y - h), pos2(c.x - dx, c.y + h)], col, Stroke::NONE));
}

/// ◀ ◆ ▶ at the right end of an animated parameter's row.
fn key_navigator(
    app: &mut EffectcraftApp,
    ui: &mut egui::Ui,
    p: &egui::Painter,
    layer: &Layer,
    prop: &Property,
    ectx: &EvalCtx,
    r: Rect,
    actions: &mut Actions,
) {
    let t = app.tokens;
    let cy = r.center().y;
    let uid = prop.uid;
    let half = ectx.comp.frame_duration().0 / 2;
    let on_key = prop.keys.iter().any(|k| (layer.comp_time(k.time).0 - ectx.time.0).abs() <= half);
    for (what, dx) in [("prev", -44.0), ("key", -32.0), ("next", -20.0)] {
        let cr = Rect::from_center_size(pos2(r.max.x + dx, cy), vec2(12.0, 16.0));
        let resp = ui.interact(cr, egui::Id::new(("ec-nav", uid, what)), Sense::click());
        let col = if resp.hovered() { t.text } else { t.text_dim };
        match what {
            "prev" => triangle(p, cr.center(), true, col),
            "next" => triangle(p, cr.center(), false, col),
            _ => {
                let kr = Rect::from_center_size(cr.center(), vec2(10.0, 10.0));
                if on_key {
                    icons::paint(p, kr, Icon::Keyframe, t.keyframe);
                } else {
                    let c = kr.center();
                    let d = 4.5;
                    p.add(egui::Shape::closed_line(
                        vec![pos2(c.x, c.y - d), pos2(c.x + d, c.y), pos2(c.x, c.y + d), pos2(c.x - d, c.y)],
                        Stroke::new(1.0, col),
                    ));
                }
            }
        }
        app.auto.add(&format!("effectControls.prop.{uid}.{what}Key"), cr, &prop.name);
        if resp.clicked() {
            match what {
                "prev" => actions.push(("time.go".into(), json!({"to": "prevKey", "prop": uid}))),
                "next" => actions.push(("time.go".into(), json!({"to": "nextKey", "prop": uid}))),
                _ => actions.push(("prop.toggleKey".into(), json!({"layer": layer.id.0, "prop": uid}))),
            }
        }
    }
}

/// Toggle a viewer pick (crosshair / eyedropper) for a parameter.
fn toggle_pick(app: &mut EffectcraftApp, kind: &str, layer: &Layer, prop: &Property) {
    let pick = FxPick { kind: kind.into(), layer: layer.id.0, prop: prop.uid, name: prop.name.clone() };
    if app.ui.fx_pick.as_ref() == Some(&pick) {
        app.ui.fx_pick = None;
    } else {
        app.ui.fx_pick = Some(pick);
        app.ui.status = if kind == "point" {
            format!("Click in the Composition panel to set {}", prop.name)
        } else {
            format!("Click in the Composition panel to sample {}", prop.name)
        };
    }
}

/// One property row: stopwatch, name, value control (and the keyframe navigator). Returns the
/// value fields of a vector, one per dimension, for the pick whip.
#[allow(clippy::too_many_arguments)]
fn prop_row(
    app: &mut EffectcraftApp,
    ui: &mut egui::Ui,
    p: &egui::Painter,
    layer: &Layer,
    group: &PropGroup,
    prop: &Property,
    ectx: &EvalCtx,
    r: Rect,
    indent: f32,
    actions: &mut Actions,
) -> Vec<Rect> {
    let t = app.tokens;
    let mut dims = vec![];
    let cy = r.center().y;
    let uid = prop.uid;
    let swr = Rect::from_center_size(pos2(r.min.x + indent, cy), vec2(14.0, 14.0));
    if !prop.static_only && !matches!(prop.ui, ParamUi::Hidden) {
        let resp = ui.interact(swr, egui::Id::new(("ec-sw", uid)), Sense::click());
        icons::paint(
            p,
            swr,
            Icon::Stopwatch,
            if prop.is_animated() {
                t.hot_text
            } else if resp.hovered() {
                t.text
            } else {
                t.text_dim
            },
        );
        app.auto.add(&format!("effectControls.prop.{uid}.stopwatch"), swr, &prop.name);
        if resp.clicked() {
            actions.push(("prop.toggleAnimation".into(), json!({"layer": layer.id.0, "prop": uid})));
        }
    }
    if prop.is_animated() {
        key_navigator(app, ui, p, layer, prop, ectx, r, actions);
    }
    let value = ectx.value(layer, prop);
    // Keep a shared value column while names and their icons follow the group hierarchy.
    // Extremely deep groups still leave a little name space before their value.
    let vx = (r.min.x + r.width() * 0.48).max(r.min.x + PARAM_NAME_X + 124.0).max(swr.max.x + 40.0);
    // A vector's values end before the keyframe navigator: in a narrow panel they move left over
    // the name (cut short) instead of running under the navigator (#271).
    let vx = match &value {
        Value::Vec2(_) | Value::Vec3(_) => {
            let point = if matches!(prop.ui, ParamUi::Point | ParamUi::Point3) { 22.0 } else { 0.0 };
            let fields: f32 = value
                .components()
                .iter()
                .take(shown_dims(layer, prop, &value))
                .map(|v| p.layout_no_wrap(format!("{v:.1}"), Tokens::ui(12.0), t.hot_text).size().x + 12.0)
                .sum();
            let right = r.max.x - if prop.is_animated() { 52.0 } else { 8.0 };
            vx.min(right - point - fields + 8.0).max(swr.max.x + 60.0)
        }
        _ => vx,
    };
    let name_clip = Rect::from_min_max(pos2(swr.max.x + 6.0, r.min.y), pos2(vx - 6.0, r.max.y));
    app.auto.add(&format!("effectControls.prop.{uid}.name"), name_clip, &prop.name);
    p.with_clip_rect(name_clip.intersect(p.clip_rect())).text(pos2(swr.max.x + 6.0, cy), Align2::LEFT_CENTER, &prop.name, Tokens::ui(12.0), t.text);
    let merge = format!("ec-{uid}");
    let set =
        |actions: &mut Actions, v: serde_json::Value| actions.push(("prop.set".into(), json!({"layer": layer.id.0, "prop": uid, "value": v, "merge": merge})));
    let twirl_open = |app: &mut EffectcraftApp, ui: &mut egui::Ui| {
        let open = app.ui.fx_slider_open.contains(&uid);
        let tw = Rect::from_center_size(pos2(swr.min.x - 9.0, cy), vec2(10.0, 10.0));
        if widgets::twirl(ui, tw, open, egui::Id::new(("ec-stw", uid)), &t).clicked() {
            if open {
                app.ui.fx_slider_open.remove(&uid);
            } else {
                app.ui.fx_slider_open.insert(uid);
            }
        }
        app.auto.add(&format!("effectControls.prop.{uid}.twirl"), tw, &prop.name);
    };
    match &value {
        Value::Scalar(v) if matches!(prop.ui, ParamUi::Mask) => {
            // A mask of this layer ("Path" popup).
            let mut opts = vec!["None".to_string()];
            if let Some(m) = layer.masks() {
                opts.extend(m.groups().map(|g| g.name.clone()));
            }
            let i = v.round().max(0.0) as usize;
            let label = opts.get(i).cloned().unwrap_or_else(|| format!("Mask {i}"));
            let dr = Rect::from_min_size(pos2(vx, cy - 9.0), vec2((r.max.x - vx - 56.0).clamp(80.0, 200.0), 18.0));
            let pop = egui::Id::new(("ec-mpop", uid));
            if widgets::dropdown(ui, dr, &label, &t, egui::Id::new(("ec-m", uid))).clicked() {
                widgets::open_popup(ui, pop);
            }
            app.auto.add(&format!("effectControls.prop.{uid}.value"), dr, &prop.name);
            if let Some(ni) = widgets::popup_menu(ui, pop, dr.left_bottom(), &opts, Some(i)) {
                set(actions, json!(ni as f64));
            }
        }
        Value::Scalar(v) if matches!(prop.ui, ParamUi::Angle) => {
            let (rr, dr, nv) = fw::angle_field(ui, pos2(vx, cy - 9.0), egui::Id::new(("ec-v", uid)), *v, 1, &t);
            app.auto.add(&format!("effectControls.prop.{uid}.value"), rr.union(dr), &fw::format_angle(*v, 1));
            app.auto.add(&format!("effectControls.prop.{uid}.revolutions"), rr, &prop.name);
            if let Some(nv) = nv {
                set(actions, json!(nv));
            }
            twirl_open(app, ui);
        }
        Value::Scalar(v) => {
            let (min, max, dec) = match &prop.ui {
                ParamUi::Slider { min, max, decimals, .. } => (*min, *max, *decimals as usize),
                _ => (-1e9, 1e9, 1),
            };
            let suffix = match prop.ui {
                ParamUi::Percent => "%",
                _ => "",
            };
            let speed = match &prop.ui {
                ParamUi::Slider { slider_min, slider_max, .. } => ((slider_max - slider_min) / 300.0).clamp(0.001, 50.0),
                _ => 0.5,
            };
            let (vr, nv, _) = widgets::hot_number_at(
                ui,
                pos2(vx, cy - 9.0),
                egui::Id::new(("ec-v", uid)),
                *v,
                speed,
                (min, max),
                dec.max(if dec == 0 { 0 } else { 1 }),
                suffix,
                &t,
            );
            app.auto.add(&format!("effectControls.prop.{uid}.value"), vr, &prop.name);
            if let Some(nv) = nv {
                set(actions, json!(nv));
            }
            // AE hides a slider param's slider until its twirl is opened (see `slider_row`).
            if matches!(prop.ui, ParamUi::Slider { .. }) {
                twirl_open(app, ui);
            }
        }
        Value::Vec2(_) | Value::Vec3(_) => {
            let c = value.components();
            let n = shown_dims(layer, prop, &value);
            let mut x = vx;
            if matches!(prop.ui, ParamUi::Point | ParamUi::Point3) {
                let cr = Rect::from_center_size(pos2(x + 8.0, cy), vec2(18.0, 18.0));
                let active = app.ui.fx_pick.as_ref().is_some_and(|k| k.prop == uid && k.kind == "point");
                if fw::crosshair_button(ui, cr, active, egui::Id::new(("ec-xh", uid)), &t)
                    .on_hover_text(crate::i18n::tr("Click, then click in the Composition panel"))
                    .clicked()
                {
                    toggle_pick(app, "point", layer, prop);
                }
                app.auto.add(&format!("effectControls.prop.{uid}.crosshair"), cr, &prop.name);
                x += 22.0;
            }
            for d in 0..n.min(c.len()) {
                let (vr, nv, _) = widgets::hot_number_at(ui, pos2(x, cy - 9.0), egui::Id::new(("ec-v", uid, d)), c[d], 1.0, (-1e9, 1e9), 1, "", &t);
                app.auto.add(&format!("effectControls.prop.{uid}.value.{d}"), vr, &prop.name);
                dims.push(vr);
                if let Some(nv) = nv {
                    let mut nc = c.clone();
                    nc[d] = nv;
                    set(actions, json!(nc));
                }
                x = vr.max.x + 8.0;
            }
        }
        Value::Color(c) => {
            let sr = Rect::from_min_size(pos2(vx, cy - 8.0), vec2(30.0, 16.0));
            let pop = egui::Id::new(("ec-cpop", uid));
            if widgets::swatch(ui, sr, [c[0] as f32, c[1] as f32, c[2] as f32, 1.0], egui::Id::new(("ec-c", uid)), &t).clicked() {
                widgets::open_popup(ui, pop);
            }
            app.auto.add(&format!("effectControls.prop.{uid}.value"), sr, &prop.name);
            let er = Rect::from_center_size(pos2(sr.max.x + 14.0, cy), vec2(18.0, 18.0));
            let active = app.ui.fx_pick.as_ref().is_some_and(|k| k.prop == uid && k.kind == "color");
            if fw::eyedropper_button(ui, er, active, egui::Id::new(("ec-eye", uid)), &t)
                .on_hover_text(crate::i18n::tr("Eyedropper: click, then click in the Composition panel"))
                .clicked()
            {
                toggle_pick(app, "color", layer, prop);
            }
            app.auto.add(&format!("effectControls.prop.{uid}.eyedropper"), er, &prop.name);
            let mut rgb = [c[0] as f32, c[1] as f32, c[2] as f32];
            if crate::header::color_popup(ui, pop, sr.left_bottom(), &mut rgb) {
                set(actions, json!([rgb[0], rgb[1], rgb[2], 1.0]));
            }
        }
        Value::Bool(b) => {
            let cr = Rect::from_min_size(pos2(vx, cy - 8.0), vec2(16.0, 16.0));
            if widgets::checkbox(ui, cr, *b, &t, egui::Id::new(("ec-b", uid))).clicked() {
                set(actions, json!(!b));
            }
            p.text(pos2(cr.max.x + 6.0, cy), Align2::LEFT_CENTER, &prop.name, Tokens::ui(12.0), t.text_dim);
            app.auto.add(&format!("effectControls.prop.{uid}.value"), cr, &prop.name);
        }
        Value::Enum(i) => {
            if let ParamUi::Popup { options } = &prop.ui {
                let widest = options.iter().map(|o| p.layout_no_wrap(o.clone(), Tokens::ui(11.5), t.text).size().x).fold(0.0, f32::max);
                let dr = Rect::from_min_size(pos2(vx, cy - 9.0), vec2((widest + 30.0).clamp(80.0, (r.max.x - vx - 56.0).max(80.0)), 18.0));
                let pop = egui::Id::new(("ec-epop", uid));
                if widgets::dropdown(ui, dr, options.get(*i as usize).map(String::as_str).unwrap_or(""), &t, egui::Id::new(("ec-e", uid))).clicked() {
                    widgets::open_popup(ui, pop);
                }
                app.auto.add(&format!("effectControls.prop.{uid}.value"), dr, &prop.name);
                if let Some(ni) = widgets::popup_menu(ui, pop, dr.left_bottom(), options, Some(*i as usize)) {
                    set(actions, json!(ni));
                }
            }
        }
        Value::Layer(l) => {
            let comp = app.session.active_comp_arc();
            if let Some(comp) = comp {
                let dr = Rect::from_min_size(pos2(vx, cy - 9.0), vec2(((r.max.x - vx) * 0.55).clamp(90.0, 170.0), 18.0));
                let idx_of = |id: u64| comp.layers.iter().position(|x| x.id.0 == id);
                let name = l.and_then(|id| idx_of(id).map(|i| format!("{}. {}", i + 1, comp.layers[i].name))).unwrap_or_else(|| "None".into());
                let pop = egui::Id::new(("ec-lpop", uid));
                if widgets::dropdown(ui, dr, &name, &t, egui::Id::new(("ec-l", uid))).clicked() {
                    widgets::open_popup(ui, pop);
                }
                app.auto.add(&format!("effectControls.prop.{uid}.value"), dr, &prop.name);
                let mut opts = vec!["None".to_string()];
                opts.extend(comp.layers.iter().enumerate().map(|(i, l)| format!("{}. {}", i + 1, l.name)));
                let cur = l.and_then(idx_of).map(|i| i + 1).unwrap_or(0);
                if let Some(i) = widgets::popup_menu(ui, pop, dr.left_bottom(), &opts, Some(cur)) {
                    set(actions, if i == 0 { serde_json::Value::Null } else { json!(comp.layers[i - 1].id.0) });
                }
                // Source / Masks / Effects & Masks (the hidden companion parameter).
                if let Some(src) = group.get(&effectcraft_engine::effects::layer_source_id(&prop.match_id)) {
                    let si = src.value.as_enum() as usize;
                    let opts: Vec<String> = effectcraft_engine::effects::LAYER_SOURCE_OPTIONS.iter().map(|s| s.to_string()).collect();
                    let sr = Rect::from_min_size(pos2(dr.max.x + 6.0, cy - 9.0), vec2(((r.max.x - dr.max.x) - 64.0).clamp(70.0, 120.0), 18.0));
                    let spop = egui::Id::new(("ec-lspop", uid));
                    if widgets::dropdown(ui, sr, opts.get(si).map(String::as_str).unwrap_or(""), &t, egui::Id::new(("ec-ls", uid))).clicked() {
                        widgets::open_popup(ui, spop);
                    }
                    app.auto.add(&format!("effectControls.prop.{uid}.source"), sr, &src.name);
                    if let Some(ni) = widgets::popup_menu(ui, spop, sr.left_bottom(), &opts, Some(si)) {
                        actions.push(("prop.set".into(), json!({"layer": layer.id.0, "prop": src.uid, "value": ni})));
                    }
                }
            }
        }
        // EXtractoR's red / green / blue / alpha: a popup of the footage's OpenEXR channels (#295).
        Value::Str(s) if super::fx_editors::is_extractor_channel(group, prop) => {
            let dr = Rect::from_min_size(pos2(vx, cy - 9.0), vec2((r.max.x - vx - 56.0).clamp(80.0, 200.0), 18.0));
            super::fx_editors::channel_popup(app, ui, layer, prop, s, dr, actions);
        }
        Value::Gradient(g) => {
            let gr = Rect::from_min_size(pos2(vx, cy - 7.0), vec2((r.max.x - vx - 56.0).clamp(60.0, 180.0), 14.0));
            let n = 48;
            for k in 0..n {
                let f = k as f64 / (n - 1) as f64;
                let c = gradient_at(g, f);
                let x0 = gr.min.x + gr.width() * k as f32 / n as f32;
                let x1 = gr.min.x + gr.width() * (k + 1) as f32 / n as f32;
                p.rect_filled(Rect::from_min_max(pos2(x0, gr.min.y), pos2(x1, gr.max.y)), 0.0, c);
            }
            p.rect_stroke(gr, 2.0, Stroke::new(1.0, t.field_border), egui::StrokeKind::Inside);
            app.auto.add(&format!("effectControls.prop.{uid}.value"), gr, &prop.name);
        }
        _ => {}
    }
    dims
}

/// How many of a vector parameter's values show (a 3D point's Z only on a 3D layer).
fn shown_dims(layer: &Layer, prop: &Property, value: &Value) -> usize {
    let n = value.components().len();
    if prop.shown_dims > 0 && !(layer.is_3d() && n == 3) { prop.shown_dims as usize } else { n }
}

/// Colour of a gradient at `f` (colour stops only).
fn gradient_at(g: &effectcraft_engine::keyframe::Gradient, f: f64) -> Color32 {
    let stops = &g.colors;
    let c = if stops.is_empty() {
        [0.0, 0.0, 0.0, 1.0]
    } else if f <= stops[0].0 {
        stops[0].1
    } else if f >= stops[stops.len() - 1].0 {
        stops[stops.len() - 1].1
    } else {
        let i = stops.iter().position(|s| s.0 > f).unwrap_or(stops.len() - 1).max(1);
        let (a, b) = (stops[i - 1], stops[i]);
        let k = ((f - a.0) / (b.0 - a.0).max(1e-9)) as f32;
        [0, 1, 2, 3].map(|j| a.1[j] + (b.1[j] - a.1[j]) * k)
    };
    Color32::from_rgb((c[0].clamp(0.0, 1.0) * 255.0) as u8, (c[1].clamp(0.0, 1.0) * 255.0) as u8, (c[2].clamp(0.0, 1.0) * 255.0) as u8)
}

/// The twirled-open slider under a slider param: track, knob and the slider range's ends.
#[allow(clippy::too_many_arguments)]
fn slider_row(
    app: &mut EffectcraftApp,
    ui: &mut egui::Ui,
    p: &egui::Painter,
    layer: &Layer,
    prop: &Property,
    ectx: &EvalCtx,
    r: Rect,
    x0: f32,
    actions: &mut Actions,
) {
    let t = app.tokens;
    let ParamUi::Slider { slider_min, slider_max, decimals, .. } = prop.ui else { return };
    let Value::Scalar(v) = ectx.value(layer, prop) else { return };
    let uid = prop.uid;
    let tr = Rect::from_min_max(pos2(r.min.x + x0, r.min.y + 8.0), pos2(r.max.x - 16.0, r.min.y + 12.0));
    if tr.width() < 40.0 {
        return;
    }
    p.rect_filled(tr, 2.0, t.field_bg);
    let f = ((v - slider_min) / (slider_max - slider_min)).clamp(0.0, 1.0) as f32;
    p.rect_filled(Rect::from_min_max(tr.min, pos2(tr.min.x + tr.width() * f, tr.max.y)), 2.0, t.accent.gamma_multiply(0.7));
    p.circle_filled(pos2(tr.min.x + tr.width() * f, tr.center().y), 5.0, t.text);
    let dec = decimals as usize;
    p.text(pos2(tr.min.x, tr.max.y + 4.0), Align2::LEFT_TOP, format!("{slider_min:.dec$}"), Tokens::ui(10.5), t.text_dim);
    p.text(pos2(tr.max.x, tr.max.y + 4.0), Align2::RIGHT_TOP, format!("{slider_max:.dec$}"), Tokens::ui(10.5), t.text_dim);
    let sresp = ui.interact(tr.expand2(vec2(4.0, 6.0)), egui::Id::new(("ec-slider", uid)), Sense::click_and_drag());
    app.auto.add(&format!("effectControls.prop.{uid}.slider"), tr, &prop.name);
    if (sresp.dragged() || sresp.clicked())
        && let Some(pt) = sresp.interact_pointer_pos()
    {
        let f = ((pt.x - tr.min.x) / tr.width()).clamp(0.0, 1.0) as f64;
        actions.push((
            "prop.set".into(),
            json!({"layer": layer.id.0, "prop": uid, "value": slider_min + (slider_max - slider_min) * f, "merge": format!("ec-{uid}")}),
        ));
    }
}

/// The twirled-open angle dial under an angle param.
#[allow(clippy::too_many_arguments)]
fn dial_row(app: &mut EffectcraftApp, ui: &mut egui::Ui, layer: &Layer, prop: &Property, ectx: &EvalCtx, r: Rect, x0: f32, actions: &mut Actions) {
    let t = app.tokens;
    let Value::Scalar(v) = ectx.value(layer, prop) else { return };
    let uid = prop.uid;
    let d = (r.height() - 12.0).min(52.0);
    let dr = Rect::from_min_size(pos2(r.min.x + x0 + 40.0, r.min.y + 6.0), vec2(d, d));
    let id = egui::Id::new(("ec-dial", uid));
    let (resp, nv) = fw::angle_dial(ui, dr, v, id, &t);
    app.auto.add(&format!("effectControls.prop.{uid}.dial"), dr, &prop.name);
    if let Some(nv) = nv {
        let key = gesture_key(ui, id, resp.drag_started() || resp.clicked());
        actions.push(("prop.set".into(), json!({"layer": layer.id.0, "prop": uid, "value": nv, "merge": key})));
    }
}

/// The effect instance a row belongs to, for conditional visibility
/// ([`effectcraft_engine::effects::catalog::param_shown`]): effect id, the instance's root
/// group and the spec-id path of the group being drawn (`""` or `cameraPosition/`).
struct FxScope<'a> {
    effect: &'a str,
    root: &'a PropGroup,
    path: String,
}

impl<'a> FxScope<'a> {
    fn sub(&self, m: &str) -> FxScope<'a> {
        FxScope { effect: self.effect, root: self.root, path: format!("{}{m}/", self.path) }
    }
    /// Whether child `m` of the current group is shown.
    fn shown(&self, layer: &Layer, ectx: &EvalCtx, m: &str) -> bool {
        let root = self.root;
        let value = |id: &str| -> Option<Value> {
            let mut g = root;
            let mut parts = id.split('/').peekable();
            while let Some(part) = parts.next() {
                if parts.peek().is_none() {
                    return g.get(part).map(|p| ectx.value(layer, p));
                }
                g = g.sub(part)?;
            }
            None
        };
        effectcraft_engine::effects::catalog::param_shown(self.effect, &format!("{}{m}", self.path), &value)
    }
}

#[allow(clippy::too_many_arguments)]
fn group_rows(
    app: &mut EffectcraftApp,
    ui: &mut egui::Ui,
    p: &egui::Painter,
    layer: &Layer,
    g: &PropGroup,
    ectx: &EvalCtx,
    rect: Rect,
    y: &mut f32,
    depth: usize,
    fx: &FxScope,
    actions: &mut Actions,
) {
    let t = app.tokens;
    for c in &g.children {
        let r = Rect::from_min_size(pos2(rect.min.x, *y), vec2(rect.width(), ROW));
        match c {
            Node::Prop(pr) => {
                if matches!(pr.ui, ParamUi::Hidden) || pr.three_d_only && !layer.is_3d() || !fx.shown(layer, ectx, &pr.match_id) {
                    continue;
                }
                *y += ROW;
                let indent = PARAM_NAME_X - 13.0 + TREE_INDENT * depth as f32;
                let visible = !(r.max.y < rect.min.y || r.min.y > rect.max.y);
                if visible {
                    app.auto.add(&format!("effectControls.row.{}", pr.uid), r, &pr.name);
                    let dims = prop_row(app, ui, p, layer, g, pr, ectx, r, indent, actions);
                    // A Timeline pick whip can be dropped here (After Effects).
                    super::timeline::whip_target(ui.ctx(), r.intersect(rect), layer.id.0, pr.uid, dims);
                }
                if app.ui.fx_slider_open.contains(&pr.uid) {
                    if matches!(pr.ui, ParamUi::Angle) {
                        let sr = Rect::from_min_size(pos2(rect.min.x, *y), vec2(rect.width(), DIAL_ROW));
                        *y += DIAL_ROW;
                        if sr.max.y >= rect.min.y && sr.min.y <= rect.max.y {
                            dial_row(app, ui, layer, pr, ectx, sr, indent + 22.0, actions);
                        }
                    } else if matches!(pr.ui, ParamUi::Slider { .. }) {
                        let sr = Rect::from_min_size(pos2(rect.min.x, *y), vec2(rect.width(), 34.0));
                        *y += 34.0;
                        if sr.max.y >= rect.min.y && sr.min.y <= rect.max.y {
                            slider_row(app, ui, p, layer, pr, ectx, sr, indent + 22.0, actions);
                        }
                    }
                }
            }
            Node::Group(sg) => {
                // Paint strokes live in the Timeline only (AE's Paint shows Paint on Transparent).
                if effectcraft_engine::effects::paint::is_paint(g) || !fx.shown(layer, ectx, &sg.match_id) {
                    continue;
                }
                *y += ROW;
                let open = !app.ui.fx_closed.contains(&sg.uid);
                let tw = Rect::from_center_size(pos2(r.min.x + PARAM_NAME_X - 12.0 + TREE_INDENT * depth as f32, r.center().y), vec2(12.0, 12.0));
                if r.max.y >= rect.min.y && r.min.y <= rect.max.y {
                    app.auto.add(&format!("effectControls.row.{}", sg.uid), r, &sg.name);
                    app.auto.add(&format!("effectControls.group.{}.twirl", sg.uid), tw, &sg.name);
                    if widgets::twirl(ui, tw, open, egui::Id::new(("ec-g", sg.uid)), &t).clicked() {
                        if open {
                            app.ui.fx_closed.insert(sg.uid);
                        } else {
                            app.ui.fx_closed.remove(&sg.uid);
                        }
                    }
                    let name = p.text(pos2(tw.max.x + 6.0, r.center().y), Align2::LEFT_CENTER, &sg.name, Tokens::ui(12.0), t.text);
                    app.auto.add(&format!("effectControls.group.{}.name", sg.uid), name, &sg.name);
                }
                if open {
                    let sub = fx.sub(&sg.match_id);
                    // A visual editor for the group's hidden parameters (Lumetri curves, …).
                    let eh = super::fx_editors::inline_height(fx.effect, &sub.path, rect.width());
                    if eh > 0.0 {
                        let er = Rect::from_min_size(pos2(rect.min.x, *y), vec2(rect.width(), eh));
                        *y += eh;
                        if er.max.y >= rect.min.y && er.min.y <= rect.max.y {
                            super::fx_editors::inline_editor(app, ui, p, layer, fx.root, sg, fx.effect, &sub.path, ectx, er, actions);
                        }
                    }
                    group_rows(app, ui, p, layer, sg, ectx, rect, y, depth + 1, &sub, actions);
                }
            }
        }
    }
}

/// Warp Stabilizer's Analyze / Cancel buttons and analysis status.
fn warp_editor(app: &mut EffectcraftApp, ui: &mut egui::Ui, p: &egui::Painter, layer: &Layer, g: &PropGroup, r: Rect, actions: &mut Actions) {
    let t = app.tokens;
    let running = app.session.warp_target().is_some_and(|(_, l, u)| l == layer.id && u == g.uid) && app.session.is_warp_analyzing();
    let busy = app.session.is_warp_analyzing();
    let b1 = Rect::from_min_size(pos2(r.min.x + 24.0, r.min.y + 4.0), vec2(84.0, 22.0));
    let b2 = Rect::from_min_size(pos2(b1.max.x + 8.0, b1.min.y), vec2(84.0, 22.0));
    if widgets::text_button(ui, b1, "Analyze", false, &t, egui::Id::new(("warp-analyze", g.uid))).clicked() && !busy {
        actions.push(("warp.analyze".into(), json!({"layer": layer.id.0, "effect": g.uid})));
    }
    app.auto.add(&format!("effectControls.warp.{}.analyze", g.uid), b1, "Analyze");
    if running {
        if widgets::text_button(ui, b2, "Cancel", false, &t, egui::Id::new(("warp-cancel", g.uid))).clicked() {
            actions.push(("warp.cancel".into(), json!({})));
        }
        app.auto.add(&format!("effectControls.warp.{}.cancel", g.uid), b2, "Cancel");
    }
    let analysed = matches!(g.get(effectcraft_engine::effects::warp_stab::ANALYSIS).map(|p| &p.value), Some(Value::Str(s)) if !s.is_empty());
    let status = match app.session.warp_progress() {
        Some(pr) if running => pr.banner(),
        _ if analysed => "Analysis complete".to_string(),
        _ if app.session.warp_pending.iter().any(|(_, l, u)| *l == layer.id && *u == g.uid) => "Waiting to analyze".to_string(),
        _ => "Not analyzed: click Analyze".to_string(),
    };
    p.text(pos2(r.min.x + 24.0, r.min.y + 38.0), Align2::LEFT_CENTER, status, Tokens::ui(11.5), t.text_dim);
}

/// Roto Brush & Refine Edge: Freeze / Unfreeze and Propagate buttons and the segmentation status.
fn roto_editor(app: &mut EffectcraftApp, ui: &mut egui::Ui, p: &egui::Painter, layer: &Layer, g: &PropGroup, r: Rect, actions: &mut Actions) {
    use effectcraft_engine::effects::roto as fx;
    let t = app.tokens;
    let params = effectcraft_engine::effects::flatten_params(g, &mut |pr| pr.value.clone());
    let frozen = fx::is_frozen(&params);
    let d = fx::data(&params);
    let running = app.session.roto_target().is_some_and(|(_, l, u, _)| l == layer.id && u == g.uid) && app.session.is_roto_running();
    let b1 = Rect::from_min_size(pos2(r.min.x + 24.0, r.min.y + 4.0), vec2(84.0, 22.0));
    let b2 = Rect::from_min_size(pos2(b1.max.x + 8.0, b1.min.y), vec2(84.0, 22.0));
    let label = if frozen { "Unfreeze" } else { "Freeze" };
    if widgets::text_button(ui, b1, label, frozen, &t, egui::Id::new(("roto-freeze-ec", g.uid))).clicked() && !running {
        let id = if frozen { "roto.unfreeze" } else { "roto.freeze" };
        actions.push((id.into(), json!({"layer": layer.id.0, "effect": g.uid})));
    }
    app.auto.add(&format!("effectControls.roto.{}.freeze", g.uid), b1, label);
    let (l2, id2) = if running { ("Stop", "roto.cancel") } else { ("Propagate", "roto.propagate") };
    if widgets::text_button(ui, b2, l2, false, &t, egui::Id::new(("roto-prop-ec", g.uid))).clicked() && !frozen {
        actions.push((id2.into(), json!({"layer": layer.id.0, "effect": g.uid})));
    }
    app.auto.add(&format!("effectControls.roto.{}.propagate", g.uid), b2, l2);
    let status = match (app.session.roto_progress(), d.base) {
        (Some(pr), _) if running => pr.banner(),
        (_, None) => "No strokes: paint with the Roto Brush tool (Alt+W) in the Layer panel".to_string(),
        (_, Some(b)) => format!("Base frame {b}, span {}–{}{}", d.span[0], d.span[1], if frozen { " (frozen)" } else { "" }),
    };
    p.text(pos2(r.min.x + 24.0, r.min.y + 38.0), Align2::LEFT_CENTER, status, Tokens::ui(11.5), t.text_dim);
}

// ---------------------------------------------------------------------------------------------
// Curves / Levels editors

/// Height of the custom editor shown under an effect's header (0 when none).
fn editor_height(effect: &str, g: &PropGroup, width: f32) -> f32 {
    match effect {
        super::fx_editors::GLOW | super::fx_editors::RESHAPE | super::fx_editors::EXTRACTOR => super::fx_editors::header_height(effect, g, width),
        "ec.color.curves" => 34.0 + curves_size(width) + 26.0,
        effectcraft_engine::effects::warp_stab::ID => 50.0,
        effectcraft_engine::effects::camera_tracker::ID => super::camera_tracker_ui::EDITOR_HEIGHT,
        effectcraft_engine::effects::roto::ID => 50.0,
        "ec.color.levels" | "ec.color.levelsic" => 34.0 + 80.0 + 58.0,
        "ec.color.autolevels" | "ec.color.autocontrast" | "ec.color.autocolor" => 24.0 + 80.0 + 12.0,
        _ => 0.0,
    }
}

fn curves_size(width: f32) -> f32 {
    (width - 72.0).clamp(120.0, 256.0)
}

/// Cached histogram of the displayed frame (recomputed when the frame changes).
fn frame_histogram(app: &mut EffectcraftApp, ctx: &egui::Context) -> Option<std::sync::Arc<[[u32; 256]; 5]>> {
    let img = app.viewer_pixels()?;
    let key = std::sync::Arc::as_ptr(&img) as usize;
    let id = egui::Id::new("ec-histogram");
    if let Some((k, h)) = ctx.data(|d| d.get_temp::<(usize, std::sync::Arc<[[u32; 256]; 5]>)>(id))
        && k == key
    {
        return Some(h);
    }
    let h = std::sync::Arc::new(fw::histogram(&img));
    ctx.data_mut(|d| d.insert_temp(id, (key, h.clone())));
    Some(h)
}

fn channel_color(ch: usize) -> Color32 {
    match ch {
        1 => Color32::from_rgb(0xe0, 0x50, 0x50),
        2 => Color32::from_rgb(0x50, 0xc8, 0x60),
        3 => Color32::from_rgb(0x50, 0x80, 0xf0),
        4 => Color32::from_gray(0x90),
        _ => Color32::from_gray(0xb8),
    }
}

/// A "Channel: [RGB ▾]" popup; returns the chosen channel.
#[allow(clippy::too_many_arguments)]
fn channel_popup(
    app: &mut EffectcraftApp,
    ui: &mut egui::Ui,
    p: &egui::Painter,
    x: f32,
    cy: f32,
    cur: usize,
    names: &[&str],
    id: egui::Id,
    auto: &str,
) -> Option<usize> {
    let t = app.tokens;
    p.text(pos2(x, cy), Align2::LEFT_CENTER, crate::i18n::tr("Channel:"), Tokens::ui(12.0), t.text_dim);
    let dr = Rect::from_min_size(pos2(x + 58.0, cy - 9.0), vec2(80.0, 18.0));
    let pop = id.with("pop");
    if widgets::dropdown(ui, dr, names.get(cur).copied().unwrap_or(""), &t, id).clicked() {
        widgets::open_popup(ui, pop);
    }
    app.auto.add(auto, dr, "Channel");
    let opts: Vec<String> = names.iter().map(|s| s.to_string()).collect();
    widgets::popup_menu(ui, pop, dr.left_bottom(), &opts, Some(cur))
}

/// Curves: channel popup, Bezier/pencil mode, Reset, and the graph bound to the channel's
/// hidden point-list parameter.
#[allow(clippy::too_many_arguments)]
fn curves_editor(app: &mut EffectcraftApp, ui: &mut egui::Ui, p: &egui::Painter, layer: &Layer, g: &PropGroup, ectx: &EvalCtx, r: Rect, actions: &mut Actions) {
    let t = app.tokens;
    let euid = g.uid;
    // The channel lives in the effect's hidden Channel parameter (UI state for instances
    // saved before it existed).
    let chan_prop = g.get("channel");
    let ch = match chan_prop {
        Some(pr) => ectx.value(layer, pr).as_enum() as usize,
        None => app.ui.fx_curve_channel.get(&euid).copied().unwrap_or(0),
    }
    .min(4);
    let x0 = r.min.x + 30.0;
    let cy = r.min.y + 17.0;
    let names: Vec<&str> = fw::CURVE_CHANNELS.iter().map(|c| c.1).collect();
    if let Some(n) = channel_popup(app, ui, p, x0, cy, ch, &names, egui::Id::new(("ec-cch", euid)), &format!("effectControls.effect.{euid}.curves.channel")) {
        match chan_prop {
            Some(pr) => actions.push(("prop.set".into(), json!({"layer": layer.id.0, "prop": pr.uid, "value": n}))),
            None => {
                app.ui.fx_curve_channel.insert(euid, n);
            }
        }
    }
    let pencil = app.ui.fx_curve_pencil.contains(&euid);
    let mr = Rect::from_min_size(pos2(x0 + 146.0, cy - 9.0), vec2(56.0, 18.0));
    if widgets::dropdown(ui, mr, if pencil { "Pencil" } else { "Bezier" }, &t, egui::Id::new(("ec-cmode", euid))).clicked() {
        if pencil {
            app.ui.fx_curve_pencil.remove(&euid);
        } else {
            app.ui.fx_curve_pencil.insert(euid);
        }
    }
    app.auto.add(&format!("effectControls.effect.{euid}.curves.mode"), mr, "Curve mode");
    let Some(prop) = g.get(fw::CURVE_CHANNELS[ch].0) else { return };
    let cur = match ectx.value(layer, prop) {
        Value::Str(s) => fw::CurvePoints::parse(&s),
        _ => fw::CurvePoints::identity(),
    };
    let rr = Rect::from_min_size(pos2(mr.max.x + 8.0, cy - 9.0), vec2(44.0, 18.0));
    let rresp = ui.interact(rr, egui::Id::new(("ec-creset", euid)), Sense::click());
    p.text(rr.center(), Align2::CENTER_CENTER, crate::i18n::tr("Reset"), Tokens::ui(11.5), if rresp.hovered() { t.hot_text } else { t.text_dim });
    app.auto.add(&format!("effectControls.effect.{euid}.curves.reset"), rr, "Reset curve");
    if rresp.clicked() {
        actions.push(("prop.set".into(), json!({"layer": layer.id.0, "prop": prop.uid, "value": fw::CurvePoints::identity().format()})));
    }
    let size = curves_size(r.width());
    let gr = Rect::from_min_size(pos2(x0 + 8.0, r.min.y + 38.0), vec2(size, size));
    // Input/output gradients along the axes.
    let col = channel_color(ch);
    p.rect_filled(Rect::from_min_max(pos2(gr.min.x, gr.max.y + 3.0), pos2(gr.max.x, gr.max.y + 8.0)), 0.0, col.gamma_multiply(0.5));
    let id = egui::Id::new(("ec-curves", euid, ch));
    let (resp, edit) = fw::curves_graph(ui, gr, &cur, ch, pencil, id, &t);
    app.auto.add(&format!("effectControls.effect.{euid}.curves.graph"), gr, "Curves");
    let started = resp.drag_started() || resp.clicked();
    if let Some(np) = edit.changed {
        let key = gesture_key(ui, id, started);
        actions.push(("prop.set".into(), json!({"layer": layer.id.0, "prop": prop.uid, "value": np.format(), "merge": key})));
    }
    p.text(
        pos2(gr.min.x, gr.max.y + 18.0),
        Align2::LEFT_CENTER,
        if pencil { "Drag to draw the curve" } else { "Click adds a point; drag one off to remove" },
        Tokens::ui(10.5),
        t.text_faint,
    );
}

/// Levels parameter ids for a channel: input black, input white, gamma, output black, output white.
fn levels_ids(effect: &str, ch: usize) -> [String; 5] {
    if effect == "ec.color.levels" {
        return effectcraft_engine::effects::levels_channel_ids(ch);
    }
    // Levels (Individual Controls) nests each channel's controls in its twirl-down group.
    let pre = fw::LEVELS_CHANNELS[ch.min(4)].0;
    ["InBlack", "InWhite", "Gamma", "OutBlack", "OutWhite"].map(|s| format!("{pre}/{pre}{s}"))
}

/// Levels: (channel popup,) histogram, input black/gamma/white triangles, output bar with
/// black/white triangles.
#[allow(clippy::too_many_arguments)]
fn levels_editor(
    app: &mut EffectcraftApp,
    ui: &mut egui::Ui,
    p: &egui::Painter,
    layer: &Layer,
    g: &PropGroup,
    effect: &str,
    ectx: &EvalCtx,
    r: Rect,
    actions: &mut Actions,
) {
    let t = app.tokens;
    let euid = g.uid;
    let x0 = r.min.x + 30.0;
    let cy = r.min.y + 17.0;
    // Levels keeps its channel in the Channel parameter; Levels (Individual Controls) shows
    // every channel and the popup only picks the histogram.
    let ch = if effect == "ec.color.levelsic" {
        app.ui.fx_levels_channel.get(&euid).copied().unwrap_or(0).min(4)
    } else {
        g.get("channel").map(|pr| ectx.value(layer, pr).as_enum() as usize).unwrap_or(0).min(4)
    };
    if effect == "ec.color.levelsic" {
        let names: Vec<&str> = fw::LEVELS_CHANNELS.iter().map(|c| c.1).collect();
        if let Some(n) = channel_popup(app, ui, p, x0, cy, ch, &names, egui::Id::new(("ec-lch", euid)), &format!("effectControls.effect.{euid}.levels.channel"))
        {
            app.ui.fx_levels_channel.insert(euid, n);
        }
    } else {
        p.text(pos2(x0, cy), Align2::LEFT_CENTER, crate::i18n::tr("Histogram"), Tokens::ui(12.0), t.text_dim);
    }
    let w = (r.width() - 72.0).clamp(120.0, 256.0);
    let hr = Rect::from_min_size(pos2(x0 + 8.0, r.min.y + 34.0), vec2(w, 80.0));
    match frame_histogram(app, ui.ctx()) {
        // Channel 0 (RGB) shows luminance; the others their own channel.
        Some(h) => fw::draw_histogram(p, hr, &h[ch], channel_color(ch)),
        None => {
            p.rect_filled(hr, 0.0, Color32::from_rgb(0x1c, 0x1c, 0x1c));
        }
    }
    app.auto.add(&format!("effectControls.effect.{euid}.levels.histogram"), hr, "Histogram");
    let ids = levels_ids(effect, ch);
    let val = |id: &str| g.prop(id).map(|pr| ectx.value(layer, pr).as_f64()).unwrap_or(0.0);
    let (ib, iw, gm, ob, ow) = (val(&ids[0]), val(&ids[1]), val(&ids[2]), val(&ids[3]), val(&ids[4]));
    let mut set = |ui: &egui::Ui, id: &str, v: f64, started: bool| {
        if let Some(pr) = g.prop(id) {
            let key = gesture_key(ui, egui::Id::new(("ec-lv", euid, id)), started);
            actions.push(("prop.set".into(), json!({"layer": layer.id.0, "prop": pr.uid, "value": v, "merge": key})));
        }
    };
    let track = Rect::from_min_size(pos2(hr.min.x, hr.max.y + 1.0), vec2(w, 10.0));
    let started = |ui: &egui::Ui| ui.input(|i| i.pointer.any_pressed());
    let (r0, nv) = fw::level_handle(ui, track, ib, Color32::BLACK, egui::Id::new(("ec-lib", euid, ch)), &t);
    app.auto.add(&format!("effectControls.effect.{euid}.levels.inBlack"), r0, "Input Black");
    if let Some(v) = nv {
        set(ui, &ids[0], v.min(iw - 0.005), started(ui));
    }
    let gx = fw::gamma_handle(ib, iw, gm);
    let (r1, nv) = fw::level_handle(ui, track, gx, Color32::from_gray(128), egui::Id::new(("ec-lg", euid, ch)), &t);
    app.auto.add(&format!("effectControls.effect.{euid}.levels.gamma"), r1, "Gamma");
    if let Some(v) = nv {
        set(ui, &ids[2], (fw::gamma_from_handle(ib, iw, v) * 100.0).round() / 100.0, started(ui));
    }
    let (r2, nv) = fw::level_handle(ui, track, iw, Color32::WHITE, egui::Id::new(("ec-liw", euid, ch)), &t);
    app.auto.add(&format!("effectControls.effect.{euid}.levels.inWhite"), r2, "Input White");
    if let Some(v) = nv {
        set(ui, &ids[1], v.max(ib + 0.005), started(ui));
    }
    // Output: a black→white bar with its two triangles.
    let bar = Rect::from_min_size(pos2(hr.min.x, track.max.y + 10.0), vec2(w, 9.0));
    let n = 32;
    for k in 0..n {
        let v = (k as f32 / (n - 1) as f32 * 255.0) as u8;
        let x0 = bar.min.x + bar.width() * k as f32 / n as f32;
        let x1 = bar.min.x + bar.width() * (k + 1) as f32 / n as f32;
        p.rect_filled(Rect::from_min_max(pos2(x0, bar.min.y), pos2(x1, bar.max.y)), 0.0, Color32::from_gray(v));
    }
    let otrack = Rect::from_min_size(pos2(bar.min.x, bar.max.y + 1.0), vec2(w, 10.0));
    let (r3, nv) = fw::level_handle(ui, otrack, ob, Color32::BLACK, egui::Id::new(("ec-lob", euid, ch)), &t);
    app.auto.add(&format!("effectControls.effect.{euid}.levels.outBlack"), r3, "Output Black");
    if let Some(v) = nv {
        set(ui, &ids[3], v, started(ui));
    }
    let (r4, nv) = fw::level_handle(ui, otrack, ow, Color32::WHITE, egui::Id::new(("ec-low", euid, ch)), &t);
    app.auto.add(&format!("effectControls.effect.{euid}.levels.outWhite"), r4, "Output White");
    if let Some(v) = nv {
        set(ui, &ids[4], v, started(ui));
    }
}

/// Auto Levels / Auto Contrast / Auto Color: the histogram they work from.
fn histogram_only(app: &mut EffectcraftApp, ui: &mut egui::Ui, p: &egui::Painter, g: &PropGroup, r: Rect) {
    let t = app.tokens;
    let x0 = r.min.x + 30.0;
    p.text(pos2(x0, r.min.y + 12.0), Align2::LEFT_CENTER, crate::i18n::tr("Histogram"), Tokens::ui(12.0), t.text_dim);
    let w = (r.width() - 72.0).clamp(120.0, 256.0);
    let hr = Rect::from_min_size(pos2(x0 + 8.0, r.min.y + 24.0), vec2(w, 80.0));
    match frame_histogram(app, ui.ctx()) {
        Some(h) => {
            fw::draw_histogram(p, hr, &h[1], channel_color(1).gamma_multiply(0.6));
            fw::draw_histogram_overlay(p, hr, &h[2], channel_color(2).gamma_multiply(0.6));
            fw::draw_histogram_overlay(p, hr, &h[3], channel_color(3).gamma_multiply(0.6));
        }
        None => {
            p.rect_filled(hr, 0.0, Color32::from_rgb(0x1c, 0x1c, 0x1c));
        }
    }
    app.auto.add(&format!("effectControls.effect.{}.histogram", g.uid), hr, "Histogram");
}

// ---------------------------------------------------------------------------------------------

pub fn show(app: &mut EffectcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let p = ui.painter().with_clip_rect(rect);
    let ctx = ui.ctx().clone();
    let Some(layer) = selected_layer(app) else {
        p.text(rect.center(), Align2::CENTER_CENTER, crate::i18n::tr("Select a layer to see its effects"), Tokens::ui(12.0), t.text_faint);
        return;
    };
    let Some(cid) = app.session.active_comp_id() else { return };
    let comp = app.session.project.comp_arc(cid).unwrap_or_else(|| effectcraft_engine::project::Comp::new(1, 1, Default::default(), Default::default()).into());
    let comp_name = app.session.project.item(cid).map(|i| i.name.clone()).unwrap_or_default();
    let snap_project = app.session.project.clone();
    let snap_expr = app.session.expr.clone();
    let ectx = EvalCtx { project: &snap_project, comp_id: cid, comp: &comp, time: app.session.time(), expr: snap_expr.as_deref(), footage: None };
    let hdr = Rect::from_min_size(rect.min, vec2(rect.width(), 24.0));
    p.text(pos2(hdr.min.x + 10.0, hdr.center().y), Align2::LEFT_CENTER, format!("{} • {}", comp_name, layer.name), Tokens::ui(11.5), t.text_dim);
    p.line_segment([hdr.left_bottom(), hdr.right_bottom()], Stroke::new(1.0, t.separator));
    let body = Rect::from_min_max(pos2(rect.min.x, hdr.max.y), rect.max);
    let scroll = widgets::PanelScroll::begin(ui, egui::Id::new("ec-scroll"), body);
    let mut y = body.min.y + 4.0 - scroll.offset;
    let mut actions: Actions = vec![];
    let bp = p.with_clip_rect(body);
    // Rows scroll under the header: clip their widgets (and hit tests) to the body.
    let panel_clip = ui.clip_rect();
    ui.set_clip_rect(body.intersect(panel_clip));
    let fx: Vec<PropGroup> = layer.effects().map(|f| f.groups().cloned().collect()).unwrap_or_default();
    if fx.is_empty() {
        bp.text(
            pos2(body.center().x, body.min.y + 40.0),
            Align2::CENTER_CENTER,
            "No effects. Drag one from Effects & Presets.",
            Tokens::ui(12.0),
            t.text_faint,
        );
    }
    // Clicking empty space deselects the effects.
    let bg = ui.interact(body, egui::Id::new("ec-bg"), Sense::click());
    if bg.clicked() {
        app.session.state.selected_props.retain(|(l, u)| !(*l == layer.id && fx.iter().any(|g| g.uid == *u)));
    }
    let has_clip = !app.session.state.effect_clipboard.is_empty();
    let drag_id = egui::Id::new("ec-drag-effect");
    let dragging: Option<u64> = ctx.data(|d| d.get_temp(drag_id));
    // Header tops, for drag-to-reorder.
    let mut tops: Vec<(u64, f32)> = vec![];
    for (fi, g) in fx.iter().enumerate() {
        let GroupKind::Effect { effect } = &g.kind else { continue };
        let r = Rect::from_min_size(pos2(body.min.x, y), vec2(body.width(), ROW + 2.0));
        tops.push((g.uid, r.min.y));
        y += ROW + 2.0;
        let open = !app.ui.fx_closed.contains(&g.uid);
        let selected = app.session.state.selected_props.iter().any(|(l, u)| *l == layer.id && *u == g.uid);
        bp.rect_filled(r, 0.0, if selected { t.fx_header_active } else { t.fx_header });
        let fxr = Rect::from_center_size(pos2(r.min.x + 14.0, r.center().y), vec2(16.0, 16.0));
        if widgets::icon_toggle(ui, fxr, Icon::Fx, g.enabled, &t, egui::Id::new(("ec-fx", g.uid)), Sense::click()).clicked() {
            actions.push(("effect.toggle".into(), json!({"layer": layer.id.0, "effect": g.uid})));
        }
        app.auto.add(&format!("effectControls.effect.{}.fx", g.uid), fxr, &g.name);
        let tw = Rect::from_center_size(pos2(r.min.x + 32.0, r.center().y), vec2(12.0, 12.0));
        if widgets::twirl(ui, tw, open, egui::Id::new(("ec-tw", g.uid)), &t).clicked() {
            if open {
                app.ui.fx_closed.insert(g.uid);
            } else {
                app.ui.fx_closed.remove(&g.uid);
            }
        }
        app.auto.add(&format!("effectControls.effect.{}.twirl", g.uid), tw, &g.name);
        let name = bp.text(pos2(tw.max.x + 6.0, r.center().y), Align2::LEFT_CENTER, &g.name, Tokens::semibold(12.0), t.text);
        app.auto.add(&format!("effectControls.effect.{}.name", g.uid), name, &g.name);
        let reset = Rect::from_min_size(pos2(r.max.x - 96.0, r.min.y + 4.0), vec2(40.0, 18.0));
        let rresp = ui.interact(reset, egui::Id::new(("ec-reset", g.uid)), Sense::click());
        bp.text(reset.center(), Align2::CENTER_CENTER, crate::i18n::tr("Reset"), Tokens::ui(11.5), if rresp.hovered() { t.hot_text } else { t.text_dim });
        app.auto.add(&format!("effectControls.effect.{}.reset", g.uid), reset, "Reset");
        if rresp.clicked() {
            actions.push(("effect.reset".into(), json!({"layer": layer.id.0, "effect": g.uid})));
        }
        // About…: the effect's name and category (AE's About dialog).
        let about = Rect::from_min_size(pos2(r.max.x - 50.0, r.min.y + 4.0), vec2(44.0, 18.0));
        let aresp = ui.interact(about, egui::Id::new(("ec-about", g.uid)), Sense::click());
        bp.text(about.center(), Align2::CENTER_CENTER, crate::i18n::tr("About..."), Tokens::ui(11.5), if aresp.hovered() { t.hot_text } else { t.text_dim });
        app.auto.add(&format!("effectControls.effect.{}.about", g.uid), about, "About");
        let apop = egui::Id::new(("ec-about-pop", g.uid));
        if aresp.clicked() {
            widgets::open_popup(ui, apop);
        }
        if let Some(spec) = effectcraft_engine::effects::find(effect) {
            let lines = vec![spec.name.to_string(), format!("Category: {}", spec.category), "EffectCraft built-in effect (MIT OR Apache-2.0)".to_string()];
            let _ = widgets::popup_menu(ui, apop, about.left_bottom(), &lines, None);
        }
        let hresp = ui.interact(
            Rect::from_min_max(pos2(tw.max.x, r.min.y), pos2(reset.min.x - 4.0, r.max.y)),
            egui::Id::new(("ec-hdr", g.uid)),
            Sense::click_and_drag(),
        );
        app.auto.add(&format!("effectControls.effect.{}", g.uid), r, &g.name);
        if hresp.clicked() || hresp.drag_started() || hresp.secondary_clicked() {
            let add = ui.input(|i| i.modifiers.shift || i.modifiers.command);
            if add && hresp.clicked() {
                if selected {
                    app.session.state.selected_props.retain(|(l, u)| !(*l == layer.id && *u == g.uid));
                } else {
                    app.session.state.selected_props.push((layer.id, g.uid));
                }
            } else if !selected || hresp.clicked() {
                app.session.state.selected_props = vec![(layer.id, g.uid)];
            }
        }
        if hresp.drag_started() {
            ctx.data_mut(|d| d.insert_temp(drag_id, g.uid));
        }
        hresp.context_menu(|ui| {
            let lid = layer.id.0;
            let mut item = |ui: &mut egui::Ui, label: &str, enabled: bool, cmd: &str, params: serde_json::Value| {
                if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
                    actions.push((cmd.into(), params));
                    ui.close();
                }
            };
            item(ui, "Copy", true, "effect.copy", json!({"layer": lid, "effect": g.uid}));
            item(ui, "Paste", has_clip, "effect.paste", json!({"layers": [lid]}));
            item(ui, "Duplicate", true, "effect.duplicate", json!({"layer": lid, "effect": g.uid}));
            ui.separator();
            item(ui, "Reset", true, "effect.reset", json!({"layer": lid, "effect": g.uid}));
            item(ui, "Remove", true, "effect.remove", json!({"layer": lid, "effect": g.uid}));
            ui.separator();
            item(ui, "Move Up", fi > 0, "effect.reorder", json!({"layer": lid, "effect": g.uid, "index": fi.max(1)}));
            item(ui, "Move Down", fi + 1 < fx.len(), "effect.reorder", json!({"layer": lid, "effect": g.uid, "index": fi + 2}));
        });
        // What the effect can't render as asked (an OCIO config it can't read, transforms it
        // passes through), under its name.
        if let Some(w) = effectcraft_engine::effects::warning(g, &mut |pr| ectx.value(&layer, pr)) {
            let galley = bp.layout(format!("⚠ {w}"), Tokens::ui(11.0), t.warning, (body.width() - 20.0).max(60.0));
            let wr = Rect::from_min_size(pos2(body.min.x + 10.0, y + 3.0), galley.size());
            bp.galley(wr.min, galley, t.warning);
            app.auto.add(&format!("effectControls.effect.{}.warning", g.uid), wr, &w);
            y += wr.height() + 6.0;
        }
        if open {
            let eh = editor_height(effect, g, body.width());
            if eh > 0.0 {
                let er = Rect::from_min_size(pos2(body.min.x, y), vec2(body.width(), eh));
                y += eh;
                if er.max.y >= body.min.y && er.min.y <= body.max.y {
                    match effect.as_str() {
                        "ec.color.curves" => curves_editor(app, ui, &bp, &layer, g, &ectx, er, &mut actions),
                        effectcraft_engine::effects::warp_stab::ID => warp_editor(app, ui, &bp, &layer, g, er, &mut actions),
                        effectcraft_engine::effects::camera_tracker::ID => super::camera_tracker_ui::editor(app, ui, &bp, &layer, g, er, &mut actions),
                        effectcraft_engine::effects::roto::ID => roto_editor(app, ui, &bp, &layer, g, er, &mut actions),
                        "ec.color.levels" | "ec.color.levelsic" => levels_editor(app, ui, &bp, &layer, g, effect, &ectx, er, &mut actions),
                        super::fx_editors::GLOW | super::fx_editors::RESHAPE | super::fx_editors::EXTRACTOR => {
                            super::fx_editors::header_editor(app, ui, &bp, &layer, g, effect, &ectx, er, &mut actions)
                        }
                        _ => histogram_only(app, ui, &bp, g, er),
                    }
                }
            }
            let scope = FxScope { effect: effect.as_str(), root: g, path: String::new() };
            group_rows(app, ui, &bp, &layer, g, &ectx, body, &mut y, 0, &scope, &mut actions);
        }
        y += 4.0;
    }
    // Drag an effect header to reorder: insertion line between headers.
    if let Some(du) = dragging {
        if let Some(pos) = ctx.pointer_interact_pos() {
            let slot = tops.iter().position(|(_, ty)| pos.y < *ty + (ROW + 2.0) / 2.0).unwrap_or(tops.len());
            let ly = tops.get(slot).map(|(_, ty)| *ty).unwrap_or(y - 4.0);
            bp.line_segment([pos2(body.min.x + 4.0, ly), pos2(body.max.x - 4.0, ly)], Stroke::new(2.0, t.accent));
            if ctx.input(|i| i.pointer.any_released()) {
                if let Some(from) = tops.iter().position(|(u, _)| *u == du) {
                    let to = if slot > from { slot - 1 } else { slot };
                    if to != from {
                        actions.push(("effect.reorder".into(), json!({"layer": layer.id.0, "effect": du, "index": to + 1})));
                    }
                }
                ctx.data_mut(|d| d.remove::<u64>(drag_id));
            }
        }
        if !ctx.input(|i| i.pointer.any_down()) {
            ctx.data_mut(|d| d.remove::<u64>(drag_id));
        }
    }
    // Right-click anywhere a control has no menu of its own: the Effect menu, which applies to
    // the selected layers (After Effects). Drawn after the rows, whose menus open first.
    let menu_id = bg.id.with("effect-menu");
    let open = ui.input(|i| i.pointer.secondary_clicked())
        && ui.rect_contains_pointer(body)
        && (!egui::Popup::is_any_open(&ctx) || egui::Popup::is_id_open(&ctx, menu_id));
    egui::Popup::context_menu(&bg).id(menu_id).open_memory(open.then_some(egui::SetOpenCommand::Bool(true))).show(|ui| {
        ui.set_min_width(200.0);
        actions.extend(crate::menus::menu_contents(app, ui, "Effect"));
    });
    ui.set_clip_rect(panel_clip);
    let content_h = y + scroll.offset - body.min.y;
    scroll.end(ui, &mut app.auto, "effectControls.scroll", content_h, &t);
    // Drop effects here.
    if let Some(payload) = egui::DragAndDrop::payload::<crate::panels::DragPayload>(&ctx)
        && ui.rect_contains_pointer(rect)
        && let crate::panels::DragPayload::Effect(e) = payload.as_ref()
    {
        p.rect_stroke(rect, 0.0, Stroke::new(2.0, t.accent), egui::StrokeKind::Inside);
        if ctx.input(|i| i.pointer.any_released()) {
            actions.push(("effect.apply".into(), json!({"effect": e, "layers": [layer.id.0]})));
            egui::DragAndDrop::clear_payload(&ctx);
        }
    }
    if ui.input(|i| i.pointer.primary_down()) && actions.iter().any(|(id, _)| id == "prop.set") {
        app.ui.viewer.property_interacting = true;
        ctx.request_repaint();
    }
    for (id, params) in actions {
        if let Err(e) = crate::menus::invoke(app, &ctx, &id, params) {
            app.ui.status = e;
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Viewer hook

/// Effect Controls' part of the Composition viewer, called by the viewer after its own
/// interaction (so these controls sit on top):
///
/// - while a crosshair / eyedropper pick is armed, the next click in the viewer sets the point
///   parameter (converted to the layer's space) or samples the frame's colour; Esc cancels;
/// - otherwise, the point parameters of the selected effects are drawn as draggable ⊕ controls.
///
/// `l2c` is the viewer's layer→comp matrix (it knows the 3D view).
pub fn viewer_hook(
    app: &mut EffectcraftApp,
    ui: &mut egui::Ui,
    painter: &egui::Painter,
    map: &super::viewer::ViewerMap,
    ectx: &EvalCtx,
    l2c: &dyn Fn(&EvalCtx, &Layer) -> Mat3,
) {
    let t = app.tokens;
    let ctx = ui.ctx().clone();
    let mut actions: Actions = vec![];
    if let Some(pick) = app.ui.fx_pick.clone() {
        let Some(layer) = ectx.comp.layer(effectcraft_engine::project::LayerId(pick.layer)).cloned() else {
            app.ui.fx_pick = None;
            return;
        };
        // Keyers pick from their input (the shown frame is already keyed): `effect.pickColor`.
        let keyer = (pick.kind == "color")
            .then(|| layer.effects()?.groups().find(|g| g.find(pick.prop).is_some()))
            .flatten()
            .filter(|g| effectcraft_engine::effects::find(&g.match_id).is_some_and(|s| s.category == "Keying"))
            .map(|g| g.uid);
        if pick.kind == "color" && keyer.is_none() {
            // GPU frames: read the shown frame back for sampling.
            app.viewer_pixels();
        }
        let resp = ui.interact(map.area, egui::Id::new("viewer-fx-pick"), Sense::click());
        app.auto.add("viewer.fxPick", map.area, &pick.name);
        let hint = if pick.kind == "point" { format!("Click to set {}", pick.name) } else { format!("Click to sample {}", pick.name) };
        let chip = Rect::from_min_size(map.area.min + vec2(10.0, 10.0), vec2(220.0, 22.0));
        painter.rect_filled(chip, 11.0, Color32::from_black_alpha(180));
        painter.text(chip.center(), Align2::CENTER_CENTER, hint, Tokens::ui(11.5), t.text);
        if let Some(hp) = resp.hover_pos() {
            ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
            let c = map.to_comp(hp);
            app.pointer_comp = Some([c[0] as f32, c[1] as f32]);
            if pick.kind == "color"
                && keyer.is_none()
                && let Some(img) = &app.viewer_image
                && let Some(s) = fw::sample_frame(img, super::viewer_tools::shown_region(&ctx, map.comp), c)
            {
                let sw = Rect::from_min_size(hp + vec2(14.0, 14.0), vec2(26.0, 18.0));
                painter.rect_filled(sw, 2.0, Color32::from_rgb((s[0] * 255.0) as u8, (s[1] * 255.0) as u8, (s[2] * 255.0) as u8));
                painter.rect_stroke(sw, 2.0, Stroke::new(1.0, Color32::WHITE), egui::StrokeKind::Outside);
            }
        }
        if resp.clicked()
            && let Some(pos) = resp.interact_pointer_pos()
        {
            let c = map.to_comp(pos);
            // Effect points and the keyer's input are in effect space.
            let m = super::viewer::from_effect_space(ectx, &layer, l2c(ectx, &layer));
            if pick.kind == "point" {
                if let Some(lp) = fw::comp_to_layer(&m, c) {
                    actions.push(("prop.set".into(), json!({"layer": pick.layer, "prop": pick.prop, "value": [lp[0], lp[1]]})));
                }
            } else if let Some(fx) = keyer {
                // Ctrl/Cmd+click averages the 5 × 5 pixels around the point.
                if let Some(lp) = fw::comp_to_layer(&m, c) {
                    let average = ui.input(|i| i.modifiers.command);
                    actions.push((
                        "effect.pickColor".into(),
                        json!({"layer": pick.layer, "effect": fx, "prop": pick.prop, "x": lp[0], "y": lp[1], "average": average}),
                    ));
                }
            } else if let Some(img) = &app.viewer_image
                && let Some(s) = fw::sample_frame(img, super::viewer_tools::shown_region(&ctx, map.comp), c)
            {
                actions.push(("prop.set".into(), json!({"layer": pick.layer, "prop": pick.prop, "value": [s[0], s[1], s[2], 1.0]})));
            }
            app.ui.fx_pick = None;
            app.ui.status.clear();
        }
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            app.ui.fx_pick = None;
            app.ui.status.clear();
        }
    } else if app.ui.viewer.show_layer_controls {
        // ⊕ controls for the point parameters of the selected effects.
        let sel = app.session.state.selected_props.clone();
        let selected = app.session.state.selected_layers.clone();
        for layer in ectx.comp.layers.iter().filter(|l| selected.contains(&l.id) && l.is_active_at(ectx.time)) {
            let Some(fx) = layer.effects() else { continue };
            let m = l2c(ectx, layer);
            // Masks are in layer space, effect points in effect space.
            let fm = super::viewer::from_effect_space(ectx, layer, m);
            for g in fx.groups().filter(|g| g.enabled && sel.iter().any(|(l, u)| *l == layer.id && *u == g.uid)) {
                if matches!(&g.kind, GroupKind::Effect { effect } if effect == super::fx_editors::RESHAPE) {
                    super::fx_editors::reshape_overlay(app, ui, painter, map, ectx, layer, g, &m, &mut actions);
                }
                let pts: Vec<&Property> = g.props().filter(|p| matches!(p.ui, ParamUi::Point)).collect();
                let screen: Vec<egui::Pos2> = pts
                    .iter()
                    .map(|pr| {
                        let v = ectx.value(layer, pr).as_vec2();
                        map.to_screen(fw::layer_to_comp(&fm, v))
                    })
                    .collect();
                // Gradient Ramp and friends: a guide between start and end points.
                if screen.len() == 2 && matches!(&g.kind, GroupKind::Effect { effect } if effect.contains("gradient") || effect.contains("ramp")) {
                    painter.line_segment([screen[0], screen[1]], Stroke::new(1.0, Color32::from_white_alpha(110)));
                }
                for (pr, s) in pts.iter().zip(screen) {
                    let hr = Rect::from_center_size(s, vec2(16.0, 16.0));
                    let id = egui::Id::new(("viewer-fx-pt", pr.uid));
                    let resp = ui.interact(hr, id, Sense::drag());
                    let col = if resp.hovered() || resp.dragged() { t.accent } else { Color32::WHITE };
                    fw::draw_crosshair(painter, s + vec2(1.0, 1.0), 5.0, Color32::from_black_alpha(160));
                    fw::draw_crosshair(painter, s, 5.0, col);
                    app.auto.add(&format!("viewer.effectPoint.{}", pr.uid), hr, &pr.name);
                    if resp.hovered() {
                        ctx.set_cursor_icon(egui::CursorIcon::Move);
                        resp.clone().on_hover_text(format!("{}: {}", g.name, pr.name));
                    }
                    if resp.dragged()
                        && let Some(pos) = resp.interact_pointer_pos()
                        && let Some(lp) = fw::comp_to_layer(&fm, map.to_comp(pos))
                    {
                        let key = gesture_key(ui, id, resp.drag_started());
                        actions.push(("prop.set".into(), json!({"layer": layer.id.0, "prop": pr.uid, "value": [lp[0], lp[1]], "merge": key})));
                    }
                }
            }
        }
    }
    for (id, params) in actions {
        if let Err(e) = crate::menus::invoke(app, &ctx, &id, params) {
            app.ui.status = e;
        }
    }
}
