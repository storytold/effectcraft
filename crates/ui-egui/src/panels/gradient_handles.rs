//! A shape layer's Gradient Fill / Gradient Stroke start and end points in the Composition viewer
//! (#612). With the gradient, or a property in it, selected, its Start Point and End Point show as
//! draggable ⊕ controls with a guide between them, as in After Effects. A drag sets the point with
//! `prop.set`: one undo step per drag, and on an animated point a keyframe at the current time.
//!
//! The points are in the space of the shape group holding the gradient, so they follow that
//! group's transform (and the groups around it) as well as the layer's.
//!
//! Automation ids: `viewer.gradientPoint.<uid>` (the Start Point / End Point property's uid).

use effectcraft_engine::geom::Mat3;
use effectcraft_engine::project::{Layer, LayerId, Node, PropGroup, Property};
use effectcraft_engine::render::EvalCtx;
use egui::{Color32, Rect, Sense, Stroke, vec2};
use serde_json::json;

use super::fx_widgets as fw;
use crate::EffectcraftApp;

/// Gradient Fill / Gradient Stroke groups in shape contents `g`, through nested groups.
fn gradient_paints<'a>(g: &'a PropGroup, depth: usize, out: &mut Vec<&'a PropGroup>) {
    if depth >= 16 {
        return;
    }
    for child in g.groups().filter(|c| c.enabled) {
        match child.match_id.as_str() {
            "gfill" | "gstroke" => out.push(child),
            "group" | "contents" => gradient_paints(child, depth + 1, out),
            _ => {}
        }
    }
}

/// `g` or something inside it is selected on `layer`.
pub(crate) fn selected_in(g: &PropGroup, layer: LayerId, selected: &[(LayerId, u64)]) -> bool {
    fn contains(g: &PropGroup, uid: u64, depth: usize) -> bool {
        g.uid == uid || (depth < 16 && g.children.iter().any(|n| n.uid() == uid || matches!(n, Node::Group(c) if contains(c, uid, depth + 1))))
    }
    selected.iter().any(|(l, uid)| *l == layer && contains(g, *uid, 0))
}

/// Draw and drag the selected gradients' start and end points. `l2c` is the viewer's layer→comp
/// matrix (it knows the 3D view). Called after the viewer's own gestures, so a handle takes the
/// press before the layer would move.
pub fn viewer_hook(
    app: &mut EffectcraftApp,
    ui: &mut egui::Ui,
    painter: &egui::Painter,
    map: &super::viewer::ViewerMap,
    ectx: &EvalCtx,
    l2c: &dyn Fn(&EvalCtx, &Layer) -> Mat3,
) {
    if !app.ui.viewer.show_layer_controls {
        return;
    }
    let t = app.tokens;
    let ctx = ui.ctx().clone();
    let selected_props = app.session.state.selected_props.clone();
    let selected_layers = app.session.state.selected_layers.clone();
    let mut actions: Vec<(String, serde_json::Value)> = vec![];
    for layer in ectx.comp.layers.iter().filter(|l| selected_layers.contains(&l.id) && l.is_active_at(ectx.time)) {
        let Some(contents) = layer.props.sub("contents") else { continue };
        let mut paints = vec![];
        gradient_paints(contents, 0, &mut paints);
        if paints.is_empty() {
            continue;
        }
        let m = l2c(ectx, layer);
        for g in paints.into_iter().filter(|g| selected_in(g, layer.id, &selected_props)) {
            // Gradient (group) space → comp: the enclosing groups' transforms, then the layer's.
            let pm = m * effectcraft_engine::render::shapes::item_matrix(ectx, layer, g.uid).unwrap_or(Mat3::IDENTITY);
            let pts: Vec<&Property> = ["start", "end"].iter().filter_map(|k| g.get(k)).collect();
            let screen: Vec<egui::Pos2> = pts.iter().map(|pr| map.to_screen(fw::layer_to_comp(&pm, ectx.value(layer, pr).as_vec2()))).collect();
            if let [a, b] = screen[..] {
                painter.line_segment([a, b], Stroke::new(1.0, Color32::from_white_alpha(110)));
            }
            for (pr, s) in pts.iter().zip(screen) {
                let hr = Rect::from_center_size(s, vec2(16.0, 16.0));
                let id = egui::Id::unique(("viewer-gradient-pt", pr.uid));
                let resp = ui.interact(hr, id, Sense::drag());
                let col = if resp.hovered() || resp.dragged() { t.accent } else { Color32::WHITE };
                fw::draw_crosshair(painter, s + vec2(1.0, 1.0), 5.0, Color32::from_black_alpha(160));
                fw::draw_crosshair(painter, s, 5.0, col);
                app.auto.add(&format!("viewer.gradientPoint.{}", pr.uid), hr, &pr.name);
                if resp.hovered() {
                    ctx.set_cursor_icon(egui::CursorIcon::Move);
                    resp.clone().on_hover_text(format!("{}: {}", g.name, pr.name));
                }
                if resp.dragged()
                    && let Some(pos) = resp.interact_pointer_pos()
                    && let Some(lp) = fw::comp_to_layer(&pm, map.to_comp(pos))
                {
                    let key = super::effect_controls::gesture_key(ui, id, resp.drag_started());
                    actions.push(("prop.set".into(), json!({"layer": layer.id.0, "prop": pr.uid, "value": [lp[0], lp[1]], "merge": key})));
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
