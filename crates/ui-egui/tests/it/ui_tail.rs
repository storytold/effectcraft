//! Headless checks for the M13.5 long-tail UI: horizontal scrolling of the Timeline outline and
//! Project panel columns (egui_kittest, UI logic only).

use effectcraft_engine::Session;
use effectcraft_ui_egui::EffectcraftApp;
use egui::{Event, Pos2, Rect, pos2, vec2};
use egui_kittest::Harness;
use serde_json::json;

fn harness(w: f32) -> Harness<'static, EffectcraftApp> {
    harness_at(vec2(w, 900.0))
}

fn harness_at(size: egui::Vec2) -> Harness<'static, EffectcraftApp> {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Tail", "width": 640, "height": 360, "duration": 4})).unwrap();
    s.execute("layer.newSolid", json!({"name": "Plate", "color": "#406080", "width": 640, "height": 360})).unwrap();
    let mut h = Harness::builder().with_size(size).build_eframe(|_| EffectcraftApp::new(s));
    h.run_steps(3);
    h
}

fn rect(h: &Harness<'_, EffectcraftApp>, id: &str) -> Rect {
    let e = h.state().auto.find(id).unwrap_or_else(|| panic!("no {id}"));
    Rect::from_min_size(pos2(e.rect[0], e.rect[1]), vec2(e.rect[2], e.rect[3]))
}

pub(crate) fn drag(h: &mut Harness<'_, EffectcraftApp>, from: Pos2, to: Pos2) {
    h.input_mut().events.push(Event::PointerMoved(from));
    h.input_mut().events.push(Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
    h.step();
    for i in 1..=8 {
        h.input_mut().events.push(Event::PointerMoved(from + (to - from) * (i as f32 / 8.0)));
        h.step();
    }
    h.input_mut().events.push(Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
    h.run_steps(2);
}

#[test]
fn timeline_outline_columns_scroll_horizontally() {
    let mut h = harness(1100.0);
    // Every column on: wider than the outline pane.
    let ctx = h.ctx.clone();
    for c in ["keys", "comment", "modes", "parent", "in", "out", "duration", "stretch"] {
        effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "timeline.column", json!({"column": c, "visible": true})).unwrap();
    }
    h.run_steps(3);
    let bar = rect(&h, "timeline.outlineScroll");
    let name0 = rect(&h, "timeline.header.name").min.x;
    assert_eq!(h.state().ui.timeline.outline_scroll, 0.0);
    drag(&mut h, pos2(bar.min.x + 8.0, bar.center().y), pos2(bar.max.x + 50.0, bar.center().y));
    h.run_steps(2);
    let s = h.state().ui.timeline.outline_scroll;
    assert!(s > 50.0, "{s}");
    let name1 = rect(&h, "timeline.header.name").min.x;
    assert!((name0 - name1 - s).abs() < 1.0, "the columns moved by the scroll: {name0} → {name1} ({s})");
    // The scroll is clamped to the overflow and resets when the columns fit again.
    for c in ["keys", "comment", "modes", "parent", "in", "out", "duration", "stretch"] {
        effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "timeline.column", json!({"column": c, "visible": false})).unwrap();
    }
    h.run_steps(3);
    assert_eq!(h.state().ui.timeline.outline_scroll, 0.0);
    assert!(h.state().auto.find("timeline.outlineScroll").is_none());
}

fn click(h: &mut Harness<'_, EffectcraftApp>, at: Pos2) {
    for pressed in [true, false] {
        h.input_mut().events.push(Event::PointerMoved(at));
        h.step();
        h.input_mut().events.push(Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() });
        h.step();
    }
    h.run_steps(2);
}

fn styles(h: &mut Harness<'_, EffectcraftApp>) -> Vec<String> {
    let v = h.state_mut().session.execute("layer.style.list", json!({})).unwrap();
    v["styles"].as_array().unwrap().iter().map(|s| s["style"].as_str().unwrap().to_string()).collect()
}

#[test]
fn layer_style_dialog_adds_previews_and_cancels() {
    let mut h = harness(1400.0);
    let lid = h.state().session.active_comp().unwrap().layers[0].id.0;
    h.state_mut().session.execute("layer.select", json!({"layers": [lid]})).unwrap();
    let ctx = h.ctx.clone();
    // Layer ▸ Layer Styles ▸ Drop Shadow (from the menu): adds the style and opens the dialog on it.
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "layer.style.dropShadow", json!({})).unwrap();
    h.run_steps(3);
    assert_eq!(h.state().dialog, Some(effectcraft_ui_egui::Dialog::LayerStyles));
    assert_eq!(styles(&mut h), ["dropShadow"]);
    assert!(h.state().auto.find("dialog.layerStyle.prop.dropShadow/distance").is_some(), "the style's controls are shown");
    // Live preview: the controls edit the real property (here through the same command).
    h.state_mut().session.execute("prop.set", json!({"layer": lid, "path": "layerStyles/dropShadow/distance", "value": 25, "merge": "x"})).unwrap();
    // Pick Stroke in the list (its page: not applied), then Cancel: everything done in the
    // dialog is rolled back. (Dialog buttons are driven through the dialog's functions:
    // kittest pointer events don't reach Foreground areas here.)
    effectcraft_ui_egui::panels::layer_styles_dialog::select(h.state_mut(), "stroke");
    h.run_steps(2);
    assert_eq!(h.state().auto.find("dialog.layerStyle.page").unwrap().label, "stroke");
    assert!(h.state().auto.find("dialog.layerStyle.prop.dropShadow/distance").is_none());
    assert!(h.state().auto.find("dialog.layerStyle.cancel").is_some());
    effectcraft_ui_egui::panels::layer_styles_dialog::finish(h.state_mut(), false);
    h.run_steps(2);
    assert!(h.state().dialog.is_none());
    assert!(styles(&mut h).is_empty(), "Cancel removes the style added from the menu");
    // Layer Style Options… on Stroke, add it (the list check box runs layer.style.add), OK keeps it.
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "layer.style.options", json!({"style": "stroke"})).unwrap();
    h.run_steps(3);
    assert!(h.state().auto.find("dialog.layerStyle.prop.stroke/size").is_none(), "not applied yet");
    h.state_mut().session.execute("layer.style.add", json!({"style": "stroke"})).unwrap();
    h.run_steps(3);
    assert!(h.state().auto.find("dialog.layerStyle.prop.stroke/size").is_some(), "its controls appear");
    effectcraft_ui_egui::panels::layer_styles_dialog::finish(h.state_mut(), true);
    assert!(h.state().dialog.is_none());
    assert_eq!(styles(&mut h), ["stroke"]);
}

/// The Layer Style dialog on the solid, opened from Layer ▸ Layer Styles ▸ Drop Shadow (which
/// adds the style) at `scale` % UI Scale. Returns the layer.
fn open_drop_shadow(h: &mut Harness<'_, EffectcraftApp>, scale: u32) -> u64 {
    let lid = h.state().session.active_comp().unwrap().layers[0].id.0;
    h.state_mut().session.execute("layer.select", json!({"layers": [lid]})).unwrap();
    h.state_mut().set_pref("appearance.uiScale", json!(scale)).unwrap();
    h.run_steps(3);
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "layer.style.dropShadow", json!({})).unwrap();
    // (Its layout settles a few frames after it opens.)
    h.run_steps(5);
    lid
}

/// `id` whole: inside the window, and painted with none of it clipped away.
fn in_view(h: &Harness<'_, EffectcraftApp>, id: &str) -> Pos2 {
    let r = rect(h, id);
    // (A shape it paints: the topmost lying inside it, not the panels behind the dialog.)
    let clip = h.output().shapes.iter().rev().find(|c| matches!(&c.shape, egui::Shape::Rect(s) if r.expand(1.0).contains_rect(s.rect))).map(|c| c.clip_rect);
    assert!(h.ctx.content_rect().contains_rect(r) && clip.is_some_and(|c| c.contains_rect(r)), "{id} is cut off: {r:?} in {clip:?}");
    r.center()
}

fn style_value(h: &mut Harness<'_, EffectcraftApp>, lid: u64, path: &str) -> f64 {
    h.state_mut().session.execute("prop.get", json!({"layer": lid, "path": format!("layerStyles/dropShadow/{path}")})).unwrap()["value"].as_f64().unwrap()
}

/// #1062: the Layer Style dialog's OK and Cancel stay in view below the list and the style's
/// controls, at 100 % and 200 % UI Scale (they were laid out past its bottom edge, under the fold
/// of its scroll area, so a click on them did nothing until the dialog was scrolled). OK keeps an
/// edit; Cancel puts the project and its undo history back as they were before the dialog.
#[test]
fn layer_style_buttons_stay_in_view() {
    for scale in [100, 200] {
        for ok in [true, false] {
            let mut h = harness(1400.0);
            let before = (h.state().session.project.clone(), h.state().session.history.undo.len());
            let lid = open_drop_shadow(&mut h, scale);
            let buttons = [in_view(&h, "dialog.layerStyle.ok"), in_view(&h, "dialog.layerStyle.cancel")];
            // An edit, as the dialog's controls make them.
            h.state_mut()
                .session
                .execute("prop.set", json!({"layer": lid, "path": "layerStyles/dropShadow/distance", "value": 40, "merge": "layer-style-dialog"}))
                .unwrap();
            h.run_steps(2);
            click(&mut h, buttons[if ok { 0 } else { 1 }]);
            let case = format!("{scale} %, {}", if ok { "OK" } else { "Cancel" });
            assert_eq!(h.state().dialog, None, "{case} closed the dialog");
            if ok {
                assert_eq!(style_value(&mut h, lid, "distance"), 40.0, "{case} kept the edit");
            } else {
                let after = (&h.state().session.project, h.state().session.history.undo.len());
                assert!(after.0.same_content(&before.0) && after.1 == before.1, "{case} put back the project and its history");
            }
        }
    }
}

/// #2057: Gradient Overlay's gradient has an Edit Gradient… button in the Layer Style dialog. It
/// opens the Gradient Editor on that property, and closing the editor goes back to the dialog.
#[test]
fn layer_style_gradient_opens_the_gradient_editor() {
    for ok in [true, false] {
        let mut h = harness(1400.0);
        let lid = h.state().session.active_comp().unwrap().layers[0].id.0;
        h.state_mut().session.execute("layer.select", json!({"layers": [lid]})).unwrap();
        let ctx = h.ctx.clone();
        effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "layer.style.gradientOverlay", json!({})).unwrap();
        h.run_steps(5);
        let edit = in_view(&h, "dialog.layerStyle.value.gradientOverlay/colors");
        click(&mut h, edit);
        assert_eq!(h.state().dialog, Some(effectcraft_ui_egui::Dialog::GradientEditor), "Edit Gradient… opened the editor");
        effectcraft_ui_egui::panels::gradient_editor::finish(h.state_mut(), ok, &ctx);
        h.run_steps(2);
        assert_eq!(h.state().dialog, Some(effectcraft_ui_egui::Dialog::LayerStyles), "closing the editor (OK {ok}) went back to the dialog");
        // The dialog closes as before once the editor is gone.
        effectcraft_ui_egui::panels::layer_styles_dialog::finish(h.state_mut(), true);
        h.run_steps(2);
        assert_eq!(h.state().dialog, None);
    }
}

/// #1062: with the buttons kept at the bottom, the style's controls still scroll both ways, so
/// every one of them can be reached in the smallest window (960×600) at 200 % UI Scale, and in a
/// dialog opened in a large window that then narrows: here Noise, near the end of Drop Shadow,
/// is scrolled to, seen whole and dragged.
#[test]
fn layer_style_controls_stay_reachable_in_a_small_window() {
    for narrowed in [false, true] {
        let mut h = harness_at(if narrowed { vec2(1600.0, 1000.0) } else { vec2(960.0, 600.0) });
        let lid = open_drop_shadow(&mut h, if narrowed { 100 } else { 200 });
        if narrowed {
            h.set_size(vec2(960.0, 600.0));
            h.state_mut().set_pref("appearance.uiScale", json!(200)).unwrap();
            h.run_steps(5);
        }
        // Scroll the controls (under their heading) to their far end, both ways.
        let over = rect(&h, "dialog.layerStyle.page").left_bottom() + vec2(10.0, 16.0);
        h.input_mut().events.push(Event::PointerMoved(over));
        h.input_mut().events.push(Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: vec2(-2000.0, -2000.0),
            modifiers: Default::default(),
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Wheel,
        });
        h.run_steps(5);
        let at = in_view(&h, "dialog.layerStyle.value.dropShadow/noise");
        let slider = rect(&h, "dialog.layerStyle.value.dropShadow/noise").left_center();
        drag(&mut h, slider + vec2(8.0, 0.0), slider + vec2(60.0, 0.0));
        assert!(style_value(&mut h, lid, "noise") > 0.0, "narrowed {narrowed}: Noise was dragged (at {at:?})");
        assert_eq!(h.state().dialog, Some(effectcraft_ui_egui::Dialog::LayerStyles));
    }
}

/// #1389: on a layer with no styles, Blending Options shows its controls instead of an empty
/// page. Opening the dialog adds nothing; the first change creates the Layer Styles group, and
/// the group and the whole drag are one undo step.
#[test]
fn layer_style_blending_options_edit_a_layer_without_styles() {
    let mut h = harness(1400.0);
    let lid = h.state().session.active_comp().unwrap().layers[0].id.0;
    h.state_mut().session.execute("layer.select", json!({"layers": [lid]})).unwrap();
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "layer.style.options", json!({})).unwrap();
    h.run_steps(5);
    let has_group = |h: &Harness<'_, EffectcraftApp>| h.state().session.active_comp().unwrap().layers[0].layer_styles().is_some();
    let undo = h.state().session.history.undo.len();
    assert_eq!(h.state().auto.find("dialog.layerStyle.page").unwrap().label, "blendingOptions");
    let fill = "dialog.layerStyle.value.blendingOptions/advancedBlending/fillOpacity";
    assert!(h.state().auto.find("dialog.layerStyle.value.blendingOptions/globalLightAngle").is_some(), "Global Light is shown");
    let at = in_view(&h, fill);
    assert!(!has_group(&h), "opening the page adds nothing");
    drag(&mut h, at, at - vec2(40.0, 0.0));
    let v =
        h.state_mut().session.execute("prop.get", json!({"layer": lid, "path": "layerStyles/blendingOptions/advancedBlending/fillOpacity"})).unwrap()["value"]
            .as_f64()
            .unwrap();
    assert!(v < 100.0, "Fill Opacity was dragged: {v}");
    assert!(styles(&mut h).is_empty(), "no style was added");
    assert_eq!(h.state().session.history.undo.len(), undo + 1, "the group and the drag are one undo step");
    assert_eq!(h.state().dialog, Some(effectcraft_ui_egui::Dialog::LayerStyles));
    effectcraft_ui_egui::panels::layer_styles_dialog::finish(h.state_mut(), true);
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    assert!(!has_group(&h), "one undo removes the group");
}

#[test]
fn layer_style_dialog_moves_by_its_title() {
    let mut h = harness(1400.0);
    let lid = h.state().session.active_comp().unwrap().layers[0].id.0;
    h.state_mut().session.execute("layer.select", json!({"layers": [lid]})).unwrap();
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "layer.style.dropShadow", json!({})).unwrap();
    h.run_steps(3);
    let list0 = rect(&h, "dialog.layerStyle.list.blendingOptions");
    // The title row sits just above the style list.
    let grab = pos2(list0.center().x, list0.min.y - 20.0);
    drag(&mut h, grab, grab + vec2(-150.0, 60.0));
    h.run_steps(2);
    let list1 = rect(&h, "dialog.layerStyle.list.blendingOptions");
    assert!((list1.min - list0.min - vec2(-150.0, 60.0)).length() < 2.0, "the dialog followed the drag: {:?} → {:?}", list0.min, list1.min);
    assert_eq!(h.state().dialog, Some(effectcraft_ui_egui::Dialog::LayerStyles), "dragging does not close it");
}

#[test]
fn project_panel_columns_scroll_horizontally() {
    let mut h = harness(1400.0);
    for c in ["type", "size", "duration", "fps", "path", "comment"] {
        effectcraft_ui_egui::panels::project::set_column(&mut h.state_mut().ui.project_columns, c, true);
    }
    h.run_steps(3);
    let bar = rect(&h, "project.hscroll");
    let x0 = rect(&h, "project.sort.comment").min.x;
    let name0 = rect(&h, "project.sort.name").min.x;
    drag(&mut h, pos2(bar.min.x + 4.0, bar.center().y), pos2(bar.max.x + 40.0, bar.center().y));
    h.run_steps(2);
    let s = h.state().ui.project_hscroll;
    assert!(s > 20.0, "{s}");
    assert!(rect(&h, "project.sort.comment").min.x < x0 - 20.0);
    assert_eq!(rect(&h, "project.sort.name").min.x, name0, "Name stays frozen");
}

/// Hovering an icon-only Timeline column header explains it, and the header's own right-click
/// column menu still opens under the icon.
#[test]
fn timeline_header_icons_explain_themselves_on_hover() {
    use egui_kittest::kittest::Queryable;
    let mut h = harness(1400.0);
    // English whatever the system language: the menu item checked below is translated.
    h.state_mut().session.prefs.general.language = "en".into();
    h.run_steps(3);
    for (id, tip) in [
        ("timeline.header.av.solo", "Solo: while any layer is soloed, only the soloed layers render"),
        ("timeline.header.av.video", "Video: shows or hides the layer's picture in the composition"),
        ("timeline.header.switches.shy", "Shy: the layer leaves the Timeline while Hide Shy Layers is on"),
        ("timeline.header.switches.threeD", "3D Layer: the layer moves and turns in depth and reacts to cameras and lights"),
    ] {
        // The tooltip is a node of its own, next to any the widget's accessible text makes.
        let before = h.query_all_by_label(tip).count();
        let at = rect(&h, id).center();
        h.event(Event::PointerMoved(at));
        h.run_steps(60);
        assert!(h.query_all_by_label(tip).count() > before, "{id}: hovering shows {tip:?}");
        h.event(Event::PointerMoved(pos2(5.0, 5.0)));
        h.run_steps(5);
    }
    // The header's accessible node carries the text as its description (screen readers).
    {
        use egui_kittest::kittest::NodeT;
        let tip = "Solo: while any layer is soloed, only the soloed layers render";
        let described = h.query_all_by_label("Solo").any(|n| n.accesskit_node().description().as_deref() == Some(tip));
        assert!(described, "the Solo header's node describes it");
    }
    // A layer's own switch shows the same text.
    let lid = h.state().session.active_comp().unwrap().layers[0].id.0;
    let lock = "Lock: keeps the layer from being selected or changed";
    let before = h.query_all_by_label(lock).count();
    let at = rect(&h, &format!("timeline.layer.{lid}.lock")).center();
    h.event(Event::PointerMoved(at));
    h.run_steps(60);
    assert!(h.query_all_by_label(lock).count() > before, "the layer's Lock switch shows its text");
    // Right-click on an icon still opens the header's column menu.
    let at = rect(&h, "timeline.header.av.solo").center();
    h.event(Event::PointerMoved(at));
    h.step();
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos: at, button: egui::PointerButton::Secondary, pressed, modifiers: Default::default() });
        h.step();
    }
    h.run_steps(3);
    assert!(h.query_by_label("Hide This").is_some(), "the column menu opens under a header icon");
}

/// The Project panel's Name column edge drags to resize the column, and a double-click on the
/// edge fits the names again.
#[test]
fn project_name_column_edge_resizes_and_fits_again() {
    let mut h = harness(1400.0);
    h.state_mut().ui.start_screen = false;
    h.run_steps(3);
    let before = rect(&h, "project.nameColumn.edge").center();
    drag(&mut h, before, before + vec2(80.0, 0.0));
    assert!(h.state().ui.project_name_w.is_some());
    let edge = rect(&h, "project.nameColumn.edge").center();
    assert!((edge.x - (before.x + 80.0)).abs() < 2.0, "{edge:?} after dragging from {before:?}");
    h.input_mut().events.push(Event::PointerMoved(edge));
    for _ in 0..2 {
        h.input_mut().events.push(Event::PointerButton { pos: edge, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
        h.input_mut().events.push(Event::PointerButton { pos: edge, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
    }
    h.run_steps(3);
    assert_eq!(h.state().ui.project_name_w, None);
    assert!((rect(&h, "project.nameColumn.edge").center().x - before.x).abs() < 1.0);
}
