use std::ops::ControlFlow;

use super::common::{Actions, effect_active, gesture_key};
use super::rows::FxScope;
use super::{
    Align2, Color32, EffectcraftApp, EvalCtx, GroupKind, Layer, Mat3, Node, ParamUi, PropGroup, Property, Rect, Sense, Stroke, Tokens, fw, json, vec2,
};

// ---------------------------------------------------------------------------------------------
// Viewer hook

/// Collect point controls through visible parameter groups. Hidden optional slots do not
/// leave stray viewport handles. The depth cap also bounds malformed project trees.
fn visible_points<'a>(g: &'a PropGroup, scope: &FxScope<'_>, layer: &Layer, ectx: &EvalCtx, depth: usize, out: &mut Vec<&'a Property>) {
    if depth > 16 {
        return;
    }
    for node in &g.children {
        match node {
            Node::Prop(p) if matches!(p.ui, ParamUi::Point) && scope.shown(layer, ectx, &p.match_id) => out.push(p),
            Node::Group(child) if child.enabled && scope.shown(layer, ectx, &child.match_id) => {
                visible_points(child, &scope.sub(&child.match_id), layer, ectx, depth + 1, out)
            }
            _ => {}
        }
    }
}

/// An armed crosshair / eyedropper pick: the hint chip, the cursor preview, and the click that
/// sets or samples. `Break` when the picked layer is gone; `Continue` carries the point preview.
fn fx_pick_session(
    app: &mut EffectcraftApp,
    ui: &mut egui::Ui,
    painter: &egui::Painter,
    map: &crate::panels::viewer::ViewerMap,
    ectx: &EvalCtx,
    l2c: &dyn Fn(&EvalCtx, &Layer) -> Mat3,
    actions: &mut Actions,
) -> ControlFlow<(), Option<egui::Pos2>> {
    let t = app.tokens;
    let ctx = ui.ctx().clone();
    let mut point_preview = None;
    let Some(pick) = app.ui.fx_pick.clone() else {
        return ControlFlow::Continue(None);
    };
    let Some(layer) = ectx.comp.layer(effectcraft_engine::project::LayerId(pick.layer)).cloned() else {
        app.ui.fx_pick = None;
        return ControlFlow::Break(());
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
    let resp = ui.interact(map.area, egui::Id::unique("viewer-fx-pick"), Sense::click());
    app.auto.add("viewer.fxPick", map.area, &pick.name);
    let hint = if pick.kind == "point" { format!("Click to set {}", pick.name) } else { format!("Click to sample {}", pick.name) };
    let chip = Rect::from_min_size(map.area.min + vec2(10.0, 10.0), vec2(220.0, 22.0));
    painter.rect_filled(chip, 11.0, Color32::from_black_alpha(180));
    painter.text(chip.center(), Align2::CENTER_CENTER, hint, Tokens::ui(11.5), t.text);
    if let Some(hp) = resp.hover_pos() {
        ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
        let c = map.to_comp(hp);
        app.pointer_comp = Some([c[0] as f32, c[1] as f32]);
        if pick.kind == "point" && fw::comp_to_layer(&l2c(ectx, &layer), c).is_some() {
            point_preview = Some(hp);
        }
        if pick.kind == "color"
            && keyer.is_none()
            && let Some(img) = &app.viewer_image
            && let Some(s) = fw::sample_frame(img, crate::panels::viewer_tools::shown_region(&ctx, map.comp), c)
        {
            let sw = Rect::from_min_size(hp + vec2(14.0, 14.0), vec2(26.0, 18.0));
            painter.rect_filled(sw, 2.0, Color32::from_rgb((s[0] * 255.0) as u8, (s[1] * 255.0) as u8, (s[2] * 255.0) as u8));
            painter.rect_stroke(sw, 2.0, Stroke::new(1.0, Color32::WHITE), egui::StrokeKind::Outside);
        }
    }
    // No frame to sample yet (it is still rendering, or its readback is pending): the click
    // leaves the eyedropper armed instead of using it up without a colour.
    let unsampled = pick.kind == "color" && keyer.is_none() && app.viewer_image.is_none();
    if resp.clicked()
        && !unsampled
        && let Some(pos) = resp.interact_pointer_pos()
    {
        let c = map.to_comp(pos);
        // Effect points and the keyer's input are in effect space.
        let m = crate::panels::viewer::from_effect_space(ectx, &layer, l2c(ectx, &layer));
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
            && let Some(s) = fw::sample_frame(img, crate::panels::viewer_tools::shown_region(&ctx, map.comp), c)
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

    ControlFlow::Continue(point_preview)
}

/// Shared hint chip and sampled-color swatch for text and popup eyedroppers.
fn color_pick_preview(app: &EffectcraftApp, painter: &egui::Painter, map: &crate::panels::viewer::ViewerMap, hint: &str, pointer: Option<egui::Pos2>) {
    let chip = Rect::from_min_size(map.area.min + vec2(10.0, 10.0), vec2(220.0, 22.0));
    painter.rect_filled(chip, 11.0, Color32::from_black_alpha(180));
    painter.text(chip.center(), Align2::CENTER_CENTER, hint, Tokens::ui(11.5), app.tokens.text);
    if let Some(pos) = pointer
        && let Some(sample) = crate::panels::viewer_tools::sample_displayed(app, painter.ctx(), map.to_comp(pos))
    {
        let swatch = Rect::from_min_size(pos + vec2(14.0, 14.0), vec2(26.0, 18.0));
        painter.rect_filled(swatch, 2.0, Color32::from_rgb((sample[0] * 255.0) as u8, (sample[1] * 255.0) as u8, (sample[2] * 255.0) as u8));
        painter.rect_stroke(swatch, 2.0, Stroke::new(1.0, Color32::WHITE), egui::StrokeKind::Outside);
    }
}

/// An armed text-colour pick: sample the shown frame on click, Esc cancels.
fn text_color_pick(
    app: &mut EffectcraftApp,
    ui: &mut egui::Ui,
    painter: &egui::Painter,
    map: &crate::panels::viewer::ViewerMap,
    ectx: &EvalCtx,
    actions: &mut Actions,
) {
    let ctx = ui.ctx().clone();
    let Some(pick) = app.ui.text_color_pick.clone() else {
        return;
    };
    if ectx.comp.layer(effectcraft_engine::project::LayerId(pick.layer)).is_none() {
        app.ui.text_color_pick = None;
        app.ui.status.clear();
    } else {
        app.viewer_pixels();
        let resp = ui.interact(map.area, egui::Id::unique("viewer-text-color-pick"), Sense::click());
        app.auto.add("viewer.textColorPick", map.area, &pick.field);
        color_pick_preview(app, painter, map, &crate::i18n::tr_args("Click to sample text {}", &[&pick.field]), resp.hover_pos());
        if let Some(pos) = resp.hover_pos() {
            ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
            let c = map.to_comp(pos);
            app.pointer_comp = Some([c[0] as f32, c[1] as f32]);
        }
        if resp.clicked()
            && let Some(pos) = resp.interact_pointer_pos()
            && let Some(s) = crate::panels::viewer_tools::sample_displayed(app, &ctx, map.to_comp(pos))
        {
            let mut params = json!({"layer": pick.layer});
            params[&pick.field] = json!([s[0], s[1], s[2]]);
            if let Some(range) = pick.range {
                params["range"] = json!(range);
            }
            actions.push(("layer.setText".into(), params));
            app.ui.text_color_pick = None;
            app.ui.status.clear();
        }
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            app.ui.text_color_pick = None;
            app.ui.status.clear();
        }
    }
}

/// A colour picker's eyedropper is armed (its popup stays open): the viewer shows the colour under
/// the pointer, and the next press on it samples the frame into the picker. The press is read from
/// the pointer, since a modal dialog over the viewer takes its clicks.
fn popup_eyedropper(app: &mut EffectcraftApp, ui: &mut egui::Ui, painter: &egui::Painter, map: &crate::panels::viewer::ViewerMap) {
    let ctx = ui.ctx().clone();
    if !crate::color_picker::eyedropper_armed(&ctx) {
        return;
    }
    let _ = ui.interact(map.area, egui::Id::unique("viewer-popup-eyedropper"), Sense::click());
    // The sampling surface, apart from the picker's button that arms it (`colorPicker.eyedropper`).
    app.auto.add("viewer.colorPickerEyedropper", map.area, "Sample color");
    ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
    app.viewer_pixels();
    color_pick_preview(app, painter, map, crate::i18n::tr("Click to sample color"), crate::color_picker::eyedropper_point(&ctx));
    if let Some(pos) = crate::color_picker::sample_press(&ctx)
        && let Some(s) = crate::panels::viewer_tools::sample_displayed(app, &ctx, map.to_comp(pos))
    {
        crate::color_picker::deliver_sample(&ctx, [s[0], s[1], s[2]]);
    }
}

/// Effect Controls' part of the Composition viewer, called by the viewer after its own
/// interaction (so these controls sit on top):
///
/// - while a crosshair pick is armed, its handle previews the cursor position without editing
///   the property; the next click sets the point in layer space, and Esc cancels;
/// - an eyedropper pick samples the frame's colour on click;
/// - otherwise, the point parameters of the selected effects are drawn as draggable ⊕ controls.
///
/// `l2c` is the viewer's layer→comp matrix (it knows the 3D view).
pub fn viewer_hook(
    app: &mut EffectcraftApp,
    ui: &mut egui::Ui,
    painter: &egui::Painter,
    map: &crate::panels::viewer::ViewerMap,
    ectx: &EvalCtx,
    l2c: &dyn Fn(&EvalCtx, &Layer) -> Mat3,
) {
    let t = app.tokens;
    let ctx = ui.ctx().clone();
    crate::color_picker::note_viewer(&ctx, map.area);
    let mut actions: Actions = vec![];
    let point_preview = match fx_pick_session(app, ui, painter, map, ectx, l2c, &mut actions) {
        ControlFlow::Break(()) => return,
        ControlFlow::Continue(p) => p,
    };
    text_color_pick(app, ui, painter, map, ectx, &mut actions);
    popup_eyedropper(app, ui, painter, map);
    let picking = app.ui.fx_pick.is_some() || app.ui.text_color_pick.is_some();
    let point_pick = app.ui.fx_pick.clone().filter(|p| p.kind == "point");
    if app.ui.viewer.show_layer_controls || point_pick.is_some() {
        // ⊕ controls for the point parameters of the selected effects.
        let mut sel = app.session.state.selected_props.clone();
        if let Some(pick) = &point_pick {
            sel.push((effectcraft_engine::project::LayerId(pick.layer), pick.prop));
        }
        let selected = app.session.state.selected_layers.clone();
        for layer in ectx
            .comp
            .layers
            .iter()
            .filter(|l| (selected.contains(&l.id) || point_pick.as_ref().is_some_and(|p| p.layer == l.id.0)) && l.is_active_at(ectx.time))
        {
            let Some(fx) = layer.effects() else { continue };
            let m = l2c(ectx, layer);
            // Masks are in layer space, effect points in effect space.
            let fm = crate::panels::viewer::from_effect_space(ectx, layer, m);
            let picked_prop = point_pick.as_ref().filter(|p| p.layer == layer.id.0).map(|p| p.prop);
            for g in fx.groups().filter(|g| g.enabled && effect_active(g, layer.id, &sel)) {
                // Puppet owns its pin handles and hit testing. Generic crosshairs would
                // overlap the pins and steal their selection/rotation/scale gestures.
                if matches!(&g.kind, GroupKind::Effect { effect } if effect == effectcraft_engine::effects::puppet::ID) {
                    continue;
                }
                if !picking
                    && app.ui.viewer.show_layer_controls
                    && matches!(&g.kind, GroupKind::Effect { effect } if effect == crate::panels::fx_editors::RESHAPE)
                {
                    crate::panels::fx_editors::reshape_overlay(app, ui, painter, map, ectx, layer, g, &m, &mut actions);
                }
                if !picking
                    && app.ui.viewer.show_layer_controls
                    && matches!(&g.kind, GroupKind::Effect { effect } if effect == crate::panels::fx_editors::MESH_WARP)
                {
                    crate::panels::fx_editors::mesh_warp_overlay(app, ui, painter, map, ectx, layer, g, &fm, &mut actions);
                }
                let scope = FxScope { effect: &g.match_id, root: g, path: String::new() };
                let mut pts = Vec::new();
                visible_points(g, &scope, layer, ectx, 0, &mut pts);
                // An explicit pick stays visible even when View > Show Layer Controls is off.
                if !app.ui.viewer.show_layer_controls {
                    pts.retain(|pr| picked_prop == Some(pr.uid));
                }
                let screen: Vec<egui::Pos2> = pts
                    .iter()
                    .map(|pr| {
                        if picked_prop == Some(pr.uid)
                            && let Some(pos) = point_preview
                        {
                            return pos;
                        }
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
                    let id = egui::Id::unique(("viewer-fx-pt", pr.uid));
                    // The picker owns the click. Preview handles must not steal it or start a drag.
                    let resp = (!picking).then(|| ui.interact(hr, id, Sense::drag()));
                    let col = if picked_prop == Some(pr.uid) || resp.as_ref().is_some_and(|r| r.hovered() || r.dragged()) { t.accent } else { Color32::WHITE };
                    fw::draw_crosshair(painter, s + vec2(1.0, 1.0), 5.0, Color32::from_black_alpha(160));
                    fw::draw_crosshair(painter, s, 5.0, col);
                    app.auto.add(&format!("viewer.effectPoint.{}", pr.uid), hr, &pr.name);
                    let Some(resp) = resp else { continue };
                    if resp.hovered() {
                        ctx.set_cursor_icon(egui::CursorIcon::Move);
                        resp.clone().on_hover_text(format!("{}: {}", crate::i18n::effects::group(&g.match_id, &g.name), pr.name));
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
