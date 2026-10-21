//! The Object Matte tool in the Composition viewer (After Effects 26.2's Object Matte, from its
//! public description): click an object to isolate it, or drag a box around it. The layer under
//! the pointer gets an Object Matte effect (`objectMatte.apply`), or its Object Matte is
//! re-prompted (the selected one, else its last); Shift adds another. Every prompt is one undo
//! step, and the effect is selected so its point controls show in the viewer.
//!
//! While hovering, a crosshair and the outline of the layer that would get the matte preview the
//! click: segmenting on every pointer move would be far too slow for a hover preview (a model
//! pass or a graph cut per move), so the matte itself appears once the prompt is set.

use effectcraft_engine::geom::vec2 as gv2;
use effectcraft_engine::project::Layer;
use effectcraft_engine::render::EvalCtx;
use egui::{Color32, Pos2, Rect, Stroke, StrokeKind};
use serde_json::json;

use super::viewer::{ViewerMap, fx2c, layers_at};
use crate::EffectcraftApp;

/// Drags shorter than this (screen points) are clicks: a point prompt, no box.
pub const MIN_BOX: f32 = 4.0;

/// Whether Object Matte can work on `layer` (footage, solid or composition layers with pixels).
fn eligible(layer: &Layer) -> bool {
    layer.has_video() && !layer.switches.adjustment && layer.effects().is_some()
}

/// The layer a prompt at comp point `cpt` goes to: a selected one under it, else the topmost.
pub fn target<'a>(app: &EffectcraftApp, ectx: &EvalCtx<'a>, cpt: [f64; 2]) -> Option<&'a Layer> {
    let hits: Vec<&Layer> = layers_at(ectx, cpt).filter(|l| eligible(l)).collect();
    let selected = &app.session.state.selected_layers;
    hits.iter().find(|l| selected.contains(&l.id)).or(hits.first()).copied()
}

/// The layer's effect-space rectangle (its pixels) on screen.
fn outline(ectx: &EvalCtx, map: &ViewerMap, layer: &Layer) -> [Pos2; 4] {
    let [w, h] = ectx.effect_bounds(layer).0;
    let m = fx2c(ectx, layer);
    [[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]].map(|p| {
        let c = m.apply(gv2(p[0], p[1]));
        map.to_screen([c.x, c.y])
    })
}

/// Hover preview at `hp` (screen): the outline of the layer that would get the matte and a
/// crosshair where the object point would go.
pub fn hover(app: &mut EffectcraftApp, painter: &egui::Painter, map: &ViewerMap, ectx: &EvalCtx, hp: Pos2) {
    let accent = app.tokens.accent;
    let cpt = map.to_comp(hp);
    if let Some(layer) = target(app, ectx, cpt) {
        let q = outline(ectx, map, layer);
        let mut pts = q.to_vec();
        pts.push(q[0]);
        painter.add(egui::Shape::line(pts, Stroke::new(1.5, accent)));
        let r = Rect::from_points(&q);
        app.auto.add("viewer.objectMatte.target", r, &layer.name);
    }
    let (gap, arm) = (4.0, 11.0);
    for (dx, dy) in [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
        let d = egui::vec2(dx, dy);
        for (w, c) in [(3.0, Color32::from_black_alpha(160)), (1.5, Color32::WHITE)] {
            painter.line_segment([hp + d * gap, hp + d * arm], Stroke::new(w, c));
        }
    }
    painter.circle_stroke(hp, 2.0, Stroke::new(1.5, accent));
    app.auto.add("viewer.objectMatte.crosshair", Rect::from_center_size(hp, egui::vec2(2.0 * arm, 2.0 * arm)), "Object Matte point");
}

/// The box being dragged (screen `a`–`b`).
pub fn draw_box(app: &mut EffectcraftApp, painter: &egui::Painter, a: Pos2, b: Pos2) {
    let r = Rect::from_two_pos(a, b);
    let accent = app.tokens.accent;
    painter.rect_stroke(r, 0.0, Stroke::new(2.5, Color32::from_black_alpha(140)), StrokeKind::Middle);
    painter.rect_stroke(r, 0.0, Stroke::new(1.5, accent), StrokeKind::Middle);
    painter.circle_filled(r.center(), 2.5, accent);
    app.auto.add("viewer.objectMatte.box", r, "Object Matte box");
}

/// Set the prompt from a click at `a` (screen), or a box dragged from `a` to `b`: on the
/// target layer's Object Matte (re-prompted), or a new one with `add` or when it has none.
pub fn apply(app: &mut EffectcraftApp, ectx: &EvalCtx, map: &ViewerMap, a: Pos2, b: Option<Pos2>, add: bool) {
    let boxed = b.filter(|b| (b.x - a.x).abs() >= MIN_BOX && (b.y - a.y).abs() >= MIN_BOX);
    let center = boxed.map_or(a, |b| Rect::from_two_pos(a, b).center());
    let Some(layer) = target(app, ectx, map.to_comp(center)) else {
        app.ui.status = "Object Matte: click an object on a footage, solid or composition layer".into();
        return;
    };
    let Some(inv) = fx2c(ectx, layer).inverse() else { return };
    let to_layer = |p: Pos2| {
        let c = map.to_comp(p);
        let l = inv.apply(gv2(c[0], c[1]));
        [l.x, l.y]
    };
    let point = to_layer(center);
    let mut q = json!({"layer": layer.id.0, "point": point});
    if let Some(b) = boxed {
        // The box in layer pixels: the bounds of its corners (the layer may be turned).
        let r = Rect::from_two_pos(a, b);
        let c = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()].map(to_layer);
        let (x0, x1) = c.iter().map(|p| p[0]).fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| (lo.min(v), hi.max(v)));
        let (y0, y1) = c.iter().map(|p| p[1]).fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| (lo.min(v), hi.max(v)));
        q["box"] = json!([x0, y0, x1, y1]);
    }
    if !add && let Some(uid) = existing(app, layer) {
        q["effect"] = json!(uid);
    }
    let lid = layer.id;
    match app.session.execute("objectMatte.apply", q) {
        Ok(r) => {
            if let Some(uid) = r["effect"].as_u64() {
                let _ = app.session.execute("prop.select", json!({"layer": lid.0, "prop": uid}));
            }
        }
        Err(e) => app.ui.status = e.to_string(),
    }
}

/// The Object Matte a prompt on `layer` re-prompts: the selected one, else the layer's last.
fn existing(app: &EffectcraftApp, layer: &Layer) -> Option<u64> {
    let fx = layer.effects()?;
    let om: Vec<u64> = fx.groups().filter(|g| is_object_matte(g)).map(|g| g.uid).collect();
    let sel = app.session.state.selected_props.iter().rev().find(|(l, u)| *l == layer.id && om.contains(u)).map(|(_, u)| *u);
    sel.or(om.last().copied())
}

fn is_object_matte(g: &effectcraft_engine::project::PropGroup) -> bool {
    matches!(&g.kind, effectcraft_engine::project::GroupKind::Effect { effect } if effect == effectcraft_engine::effects::object_matte::ID)
}
