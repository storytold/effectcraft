//! Long menus and lists scroll instead of running off the window, with a scroll bar to drag
//! (egui_kittest, UI logic only).

use effectcraft_engine::Session;
use effectcraft_ui_egui::EffectcraftApp;
use egui::{Event, Pos2, Rect, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;

fn harness(w: f32, h: f32) -> Harness<'static, EffectcraftApp> {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    let mut h = Harness::builder().with_size(vec2(w, h)).build_eframe(|_| EffectcraftApp::new(s));
    h.run_steps(3);
    h
}

fn rect(h: &Harness<'_, EffectcraftApp>, id: &str) -> Rect {
    let e = h.state().auto.find(id).unwrap_or_else(|| panic!("no {id}"));
    Rect::from_min_size(pos2(e.rect[0], e.rect[1]), vec2(e.rect[2], e.rect[3]))
}

fn click_at(h: &mut Harness<'_, EffectcraftApp>, p: Pos2) {
    h.input_mut().events.push(Event::PointerMoved(p));
    h.input_mut().events.push(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
    h.run_steps(2);
}

fn wheel(h: &mut Harness<'_, EffectcraftApp>, at: Pos2, dy: f32) {
    h.input_mut().events.push(Event::PointerMoved(at));
    h.input_mut().events.push(Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: vec2(0.0, dy),
        modifiers: Default::default(),
        phase: egui::TouchPhase::Move,
    });
    h.run_steps(20);
}

/// Edit is taller than a short window: it scrolls (with the wheel, and a scroll bar that always
/// shows) so its last entries can be reached and clicked, instead of running off the bottom of
/// the window (#269).
#[test]
fn a_menu_taller_than_the_window_scrolls() {
    let mut h = harness(1200.0, 520.0);
    assert!(!h.ctx.global_style().spacing.scroll.floating, "scroll bars show without hovering the list");
    let edit = rect(&h, "menu.Edit");
    click_at(&mut h, edit.center());
    let last = h.query_by_label_contains("Keyboard Shortcuts").expect("the Edit menu is open").rect();
    assert!(last.max.y > 520.0, "Edit is taller than the window ({last:?})");
    let first = h.query_by_label_contains("Quick Apply").expect("Quick Apply").rect();
    wheel(&mut h, first.center(), -2000.0);
    let last = h.query_by_label_contains("Keyboard Shortcuts").expect("still open").rect();
    assert!(last.min.y >= 0.0 && last.max.y <= 520.0, "scrolled into the window ({last:?})");
    click_at(&mut h, last.center());
    assert_eq!(h.state().dialog, Some(effectcraft_ui_egui::Dialog::Shortcuts), "the entry takes the click");
}

fn drag(h: &mut Harness<'_, EffectcraftApp>, from: Pos2, to: Pos2) {
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

/// A scroll bar's "scroll/max" automation label.
fn scrolled(h: &Harness<'_, EffectcraftApp>, id: &str) -> (f32, f32) {
    let label = h.state().auto.find(id).unwrap_or_else(|| panic!("no {id}")).label.clone();
    let (s, max) = label.split_once('/').unwrap_or_else(|| panic!("{id}: {label}"));
    (s.parse().unwrap(), max.parse().unwrap())
}

/// Every panel that scrolls its own rows shows a scroll bar while they overflow, and dragging it
/// scrolls them: without a mouse wheel or touchpad there was no way down most panels (#271).
#[test]
fn panels_taller_than_their_space_have_a_scroll_bar_to_drag() {
    use effectcraft_ui_egui::dock::PanelKind;
    // (The Project panel's list starts below its item preview and search: a taller window.)
    for (panel, id, height) in [
        (PanelKind::EffectControls, "effectControls.scroll", 240.0),
        (PanelKind::Properties, "properties.scroll", 240.0),
        (PanelKind::EffectsPresets, "effects.scroll", 240.0),
        (PanelKind::Project, "project.vscroll", 330.0),
        (PanelKind::Timeline, "timeline.scroll", 240.0),
        (PanelKind::RenderQueue, "renderQueue.scroll", 240.0),
        (PanelKind::History, "history.scroll", 240.0),
        (PanelKind::Preview, "preview.scroll", 240.0),
        (PanelKind::Character, "character.scroll", 240.0),
        (PanelKind::Paragraph, "paragraph.scroll", 240.0),
        (PanelKind::Info, "info.scroll", 240.0),
    ] {
        let mut h = harness(1200.0, height);
        let app = h.state_mut();
        let lid = app.session.active_comp().unwrap().layers[0].id.0;
        app.session.execute("layer.select", json!({"layers": [lid]})).unwrap();
        for e in ["slider", "angle", "checkbox", "color", "point"] {
            app.session.execute("effect.apply", json!({"effect": format!("ec.control.{e}"), "layers": [lid]})).unwrap();
        }
        for _ in 0..6 {
            app.session.execute("renderQueue.add", json!({})).unwrap();
        }
        app.show_panel(panel);
        app.ui.maximized = Some(panel);
        h.run_steps(3);
        let (_, max) = scrolled(&h, id);
        assert!(max > 0.0, "{id}: the rows overflow");
        let bar = rect(&h, id);
        // A press on the track moves the thumb there (History starts at its current state, the
        // last); then the thumb drags to the end.
        click_at(&mut h, pos2(bar.center().x, bar.min.y + 2.0));
        assert_eq!(scrolled(&h, id).0, 0.0, "{id}: at the top");
        drag(&mut h, pos2(bar.center().x, bar.min.y + 6.0), pos2(bar.center().x, bar.max.y + 40.0));
        assert_eq!(scrolled(&h, id).0, max, "{id}: dragged to the end");
    }
}

/// In a narrow Effect Controls panel an animated point's values stay clear of its keyframe
/// navigator (â—€ â—† â–¶), which was drawn over them (#271).
#[test]
fn effect_values_stay_clear_of_the_keyframe_navigator() {
    use effectcraft_ui_egui::dock::PanelKind;
    let mut h = harness(1600.0, 1000.0);
    let app = h.state_mut();
    let lid = app.session.active_comp().unwrap().layers[0].id.0;
    app.session.execute("layer.select", json!({"layers": [lid]})).unwrap();
    app.session.execute("effect.apply", json!({"effect": "ec.control.point", "layers": [lid]})).unwrap();
    let path = "effects/#1/point";
    app.session.execute("prop.toggleAnimation", json!({"layer": lid, "path": path})).unwrap();
    app.session.execute("prop.set", json!({"layer": lid, "path": path, "value": [123456.7, 123456.7]})).unwrap();
    let uid = app.session.active_comp().unwrap().layer(effectcraft_engine::project::LayerId(lid)).unwrap().props.prop(path).unwrap().uid;
    assert!(app.edit_layout(|l| l.float(PanelKind::EffectControls, [300.0, 150.0, 330.0, 500.0])));
    h.run_steps(3);
    let value = rect(&h, &format!("effectControls.prop.{uid}.value.1"));
    let nav = rect(&h, &format!("effectControls.prop.{uid}.prevKey"));
    assert!(value.max.x <= nav.min.x, "the value ({value:?}) ends before the navigator ({nav:?})");
    assert!(value.min.x > rect(&h, &format!("effectControls.prop.{uid}.stopwatch")).max.x, "and after the stopwatch");
}

/// The Settings dialog fits its width: its page column was wider than the dialog, so the whole
/// dialog scrolled sideways a little (#284).
#[test]
fn settings_dialog_does_not_scroll_sideways() {
    let mut h = harness(1400.0, 900.0);
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "app.settings", json!({"page": "general"})).unwrap();
    h.run_steps(3);
    let (ok, page) = (rect(&h, "settings.ok"), rect(&h, "settings.page.general"));
    h.input_mut().events.push(Event::PointerMoved(page.center()));
    h.input_mut().events.push(Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: vec2(-400.0, 0.0),
        modifiers: Default::default(),
        phase: egui::TouchPhase::Move,
    });
    h.run_steps(20);
    assert_eq!((rect(&h, "settings.ok"), rect(&h, "settings.page.general")), (ok, page), "nothing moved sideways");
}
