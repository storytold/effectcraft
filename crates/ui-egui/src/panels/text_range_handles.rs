//! A text layer's Range Selector Start and End in the Composition viewer (#1011). With the
//! selector, its animator or a property in the animator selected, each enabled Range Selector of
//! the animator shows two bars on the text, at Start + Offset and End + Offset (where the
//! renderer applies the range), as in After Effects. With the Selection tool a drag sets Start or
//! End with `prop.set`, leaving Offset alone: one undo step per drag, and on an animated
//! property a keyframe at the current time. A bar whose place falls outside the text (a negative
//! Offset, say) is pinned to its end, and dragging it writes the value for where it's dropped.
//!
//! The bars follow the static layout on purpose: an animator that moves the characters (a
//! fly-in, say) would otherwise carry them out of reach. Vertical text and text on a path have
//! none.
//!
//! Automation ids: `viewer.textRange.<uid>` (the Start / End property's uid).

use effectcraft_engine::geom::Mat3;
use effectcraft_engine::project::{Layer, LayerSource, PropGroup};
use effectcraft_engine::render::EvalCtx;
use effectcraft_engine::render::text::{range_bars, range_value_of, source_text};
use effectcraft_engine::text::kurbo::Point;
use effectcraft_engine::text::layout_doc;
use egui::{Color32, Pos2, Rect, Sense, Stroke, Vec2};
use serde_json::json;

use super::fx_widgets as fw;
use super::gradient_handles::selected_in;
use crate::EffectcraftApp;
use crate::state::Tool;

/// Draw the selected Range Selectors' bars and, with the Selection tool, drag them. `l2c` is the
/// viewer's layer→comp matrix. Called after the viewer's own gestures, so a bar takes the press
/// before the layer would move.
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
    let interactive = app.ui.tool == Tool::Selection;
    let selected_props = app.session.state.selected_props.clone();
    let selected_layers = app.session.state.selected_layers.clone();
    let editing = app.session.state.text_edit.as_ref().map(|e| e.layer);
    let mut actions: Vec<(String, serde_json::Value)> = vec![];
    for layer in ectx
        .comp
        .layers
        .iter()
        .filter(|l| selected_layers.contains(&l.id) && l.is_active_at(ectx.time) && matches!(l.source, LayerSource::Text) && editing != Some(l.id))
    {
        let Some(text) = layer.props.sub("text") else { continue };
        if text.sub("pathOptions").is_some_and(|po| ectx.e(layer, po, "path") != 0) {
            continue;
        }
        let sels: Vec<&PropGroup> = text
            .sub("animators")
            .into_iter()
            .flat_map(|a| a.groups())
            .filter(|a| a.enabled && selected_in(a, layer.id, &selected_props))
            .filter_map(|a| a.sub("selectors"))
            .flat_map(|s| s.groups())
            .filter(|s| s.enabled && s.match_id == "rangeSelector")
            .collect();
        if sels.is_empty() {
            continue;
        }
        let Some(doc) = source_text(ectx, layer) else { continue };
        let lay = layout_doc(&doc);
        let m = l2c(ectx, layer);
        // The layer's bounding-box handles, registered earlier this frame: the viewer grabs one
        // within 9 px of a press, so a bar leaves it the pointer there (Start at 0% of two-line
        // text meets the left side's handle) and takes it in the rest of its zone.
        let prefix = format!("viewer.handle.{}.", layer.id.0);
        let box_handles: Vec<Pos2> = app
            .auto
            .elements
            .iter()
            .filter(|e| e.id.starts_with(&prefix))
            .map(|e| Pos2::new(e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0))
            .collect();
        let to_screen = |p: Point| map.to_screen(fw::layer_to_comp(&m, [p.x, p.y]));
        for sel in sels {
            let (based, index, r) = range_bars(ectx, layer, sel, &lay);
            let n = lay.units(based);
            let bars: Vec<_> = [("start", r.start), ("end", r.end)]
                .into_iter()
                .filter_map(|(k, f)| Some((sel.get(k)?, lay.range_bar(based, (f + r.offset).clamp(0.0, 1.0))?)))
                .collect();
            let [(_, [_, b0, _]), (_, [_, b1, _])] = bars[..] else { continue };
            // Along the baseline on screen (the layer may be rotated or flipped).
            let along = (to_screen(Point::new(b0.x + 1.0, b0.y)) - to_screen(b0)).normalized();
            let first = (to_screen(b1) - to_screen(b0)).dot(along) >= 0.0;
            for (k, (pr, [top, base, bottom])) in bars.iter().enumerate() {
                let (st, sp, sb) = (to_screen(*top), to_screen(*base), to_screen(*bottom));
                let up = (st - sb).normalized();
                // The bar further along the line points back, the other forward: into the range.
                let inward = if (k == 0) == first { along } else { -along };
                // The triangle stands on the baseline, clear of the box's side handles (half way
                // up the box) and its bottom ones (at the descenders).
                let tri = [sp, sp + up * 8.0, sp + up * 4.0 + inward * 6.0];
                // A short grab zone around the triangle, so the line above doesn't steal the
                // layer's scale handles.
                let hr = Rect::from_two_pos(sp, sp + up * 8.0 + inward * 6.0).expand(3.0);
                let id = egui::Id::new(("viewer-text-range", pr.uid));
                let on_box_handle = ctx.pointer_latest_pos().is_some_and(|p| box_handles.iter().any(|c| c.distance(p) < 9.0));
                let free = ctx.is_being_dragged(id) || !on_box_handle;
                let resp = (interactive && free).then(|| ui.interact(hr, id, Sense::drag()));
                if interactive {
                    app.auto.add(&format!("viewer.textRange.{}", pr.uid), hr, &pr.name);
                }
                let hot = resp.as_ref().is_some_and(|r| r.hovered() || r.dragged());
                let col = if hot { t.accent } else { Color32::WHITE };
                draw_bar(painter, st, sb, &tri, Vec2::new(1.0, 1.0), Color32::from_black_alpha(160));
                draw_bar(painter, st, sb, &tri, Vec2::ZERO, col);
                let Some(resp) = resp else { continue };
                if resp.hovered() {
                    ctx.set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                    resp.clone().on_hover_text(format!("{}: {}", sel.name, pr.name));
                }
                // Keep the grab point's distance from the bar, so the bar doesn't jump to the pointer.
                let grab = id.with("grab");
                if resp.drag_started()
                    && let Some(pos) = ctx.input(|i| i.pointer.press_origin())
                {
                    ctx.data_mut(|d| d.insert_temp(grab, pos - sp));
                }
                if resp.dragged()
                    && let Some(pos) = resp.interact_pointer_pos()
                    && let Some(lp) = fw::comp_to_layer(&m, map.to_comp(pos - ctx.data(|d| d.get_temp::<Vec2>(grab)).unwrap_or_default()))
                    && let Some(f) = lay.range_fraction_at(based, Point::new(lp[0], lp[1]))
                {
                    let key = super::effect_controls::gesture_key(ui, id, resp.drag_started());
                    let value = range_value_of(f, r.offset, index, n);
                    actions.push(("prop.set".into(), json!({"layer": layer.id.0, "prop": pr.uid, "value": value, "merge": key})));
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

fn draw_bar(painter: &egui::Painter, top: Pos2, bottom: Pos2, tri: &[Pos2; 3], d: Vec2, col: Color32) {
    painter.line_segment([top + d, bottom + d], Stroke::new(1.5, col));
    painter.add(egui::Shape::convex_polygon(tri.iter().map(|p| *p + d).collect(), col, Stroke::NONE));
}
