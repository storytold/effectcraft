//! Dropping onto the Composition viewer: footage, comps and solids from the
//! Project panel and files from the Media Browser become layers centred where they are dropped
//! (#85), and an effect or animation preset from Effects & Presets goes on the layer under the
//! pointer, outlined while it is dragged (#88). Alt/Option-dropping a Project item replaces the source of
//! the layer under the pointer instead, as in the Timeline (#2173). Agents do the same with `layer.addItem {position}`,
//! `mediaBrowser.import {position}`, `effect.apply {layers}` and the preset commands.

use effectcraft_engine::project::{ItemId, ItemKind};
use effectcraft_engine::render::EvalCtx;
use egui::{Rect, Stroke, StrokeKind, vec2};
use serde_json::{Value, json};

use super::DragPayload;
use super::viewer::{ViewerMap, layer_at, layer_at_including_locked, layer_quad};
use crate::EffectcraftApp;

/// While something is dragged over the viewer, show where it goes; drop it on release.
pub(crate) fn show(app: &mut EffectcraftApp, ui: &egui::Ui, painter: &egui::Painter, map: &ViewerMap, ectx: &EvalCtx) {
    let ctx = ui.ctx().clone();
    let Some(payload) = egui::DragAndDrop::payload::<DragPayload>(&ctx) else { return };
    if matches!(*payload, DragPayload::Property { .. }) {
        return;
    }
    let Some(ptr) = ctx.input(|i| i.pointer.hover_pos()).filter(|_| ui.rect_contains_pointer(map.area)) else { return };
    let stroke = Stroke::new(2.0, app.tokens.accent);
    let at = map.to_comp(ptr);
    // Alt/Option-drop of a Project item replaces the source of the layer under the pointer (After
    // Effects), as in the Timeline; it never adds a layer.
    let alt_item = ctx.input(|i| i.modifiers.alt) && matches!(payload.as_ref(), DragPayload::Item(_));
    let actions: Vec<(&str, Value)> = match payload.as_ref() {
        DragPayload::Effect(effect) => layer_at(ectx, at)
            .map(|l| {
                if let Some((_, q, _)) = layer_quad(ectx, l) {
                    painter.add(egui::Shape::closed_line(q.iter().map(|c| map.to_screen(*c)).collect(), stroke));
                }
                ("effect.apply", json!({"effect": effect, "layers": [l.id.0]}))
            })
            .into_iter()
            .collect(),
        DragPayload::Preset { preset, .. } => layer_at(ectx, at)
            .map(|l| {
                if let Some((_, q, _)) = layer_quad(ectx, l) {
                    painter.add(egui::Shape::closed_line(q.iter().map(|c| map.to_screen(*c)).collect(), stroke));
                }
                preset.apply_to(l.id.0)
            })
            .into_iter()
            .collect(),
        DragPayload::Item(item) if alt_item => alt_replace(app, ectx, painter, map, stroke, at, *item).into_iter().collect(),
        DragPayload::Item(item) => {
            // The layer's frame, centred on the pointer.
            if let Some([w, h]) = app.session.project.item(ItemId(*item)).and_then(|it| frame_size(&it.kind, ectx.comp.pixel_aspect)) {
                painter.rect_stroke(Rect::from_center_size(ptr, vec2(w, h) * map.zoom), 0.0, Stroke::new(1.0, app.tokens.accent), StrokeKind::Middle);
            }
            // New layers go above the current selection, so insert in reverse
            // to preserve the order of the dragged Project items.
            crate::panels::project_drop_items(app, *item).into_iter().rev().map(|item| ("layer.addItem", json!({"item": item, "position": at}))).collect()
        }
        DragPayload::Files(paths) => vec![("mediaBrowser.import", json!({"paths": paths, "addToComp": true, "position": at}))],
        DragPayload::Property { .. } => vec![],
    };
    // Layer drops outline the target layer; the others outline the whole viewer.
    let layer_drop = alt_item || matches!(payload.as_ref(), DragPayload::Effect(_) | DragPayload::Preset { .. });
    if !layer_drop && !actions.is_empty() {
        painter.rect_stroke(map.area, 0.0, stroke, StrokeKind::Inside);
    }
    if ctx.input(|i| i.pointer.any_released()) {
        egui::DragAndDrop::clear_payload(&ctx);
        for (id, params) in actions {
            if let Err(e) = crate::menus::invoke(app, &ctx, id, params) {
                app.ui.status = e;
            }
        }
    }
}

/// The size in comp pixels of a layer made from an item of `kind` (`None`: nothing to see).
fn frame_size(kind: &ItemKind, comp_pixel_aspect: f64) -> Option<[f32; 2]> {
    let (w, h, pixel_aspect) = match kind {
        ItemKind::Comp(c) => (c.width, c.height, c.pixel_aspect),
        ItemKind::Footage(f) if f.has_video => (f.width, f.height, f.pixel_aspect),
        ItemKind::Solid(s) => (s.width, s.height, s.pixel_aspect),
        _ => return None,
    };
    let stretch = if comp_pixel_aspect > 0.0 && pixel_aspect > 0.0 { pixel_aspect / comp_pixel_aspect } else { 1.0 };
    (w > 0 && h > 0).then_some([(w as f64 * stretch) as f32, h as f32])
}

/// The `layer.replaceSource` for an Alt/Option-drop of Project item `item` at comp point `at`: the
/// topmost layer there, outlined, if it can take the item as its source (a locked one cannot).
fn alt_replace(
    app: &EffectcraftApp,
    ectx: &EvalCtx,
    painter: &egui::Painter,
    map: &ViewerMap,
    stroke: Stroke,
    at: [f64; 2],
    item: u64,
) -> Option<(&'static str, Value)> {
    let layer = layer_at_including_locked(ectx, at).filter(|l| super::timeline::replace_source_target(app, ectx.comp_id, ItemId(item), l.id, ectx.comp))?;
    if let Some((_, q, _)) = layer_quad(ectx, layer) {
        painter.add(egui::Shape::closed_line(q.iter().map(|c| map.to_screen(*c)).collect(), stroke));
    }
    Some(("layer.replaceSource", json!({"layers": [layer.id.0], "item": item, "fixExpressions": false})))
}
