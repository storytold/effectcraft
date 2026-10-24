//! Headless checks for the Object Matte tool in the Composition viewer: the hover preview, a
//! click (point prompt), a drag (box prompt) re-prompting the same effect, Shift adding another,
//! one undo step each (egui_kittest, UI logic only).

use effectcraft_engine::Session;
use effectcraft_engine::effects::flatten_params;
use effectcraft_engine::effects::object_matte as om;
use effectcraft_engine::keyframe::Value;
use effectcraft_engine::project::{GroupKind, LayerId, PropGroup};
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::dock::PanelKind;
use effectcraft_ui_egui::state::Tool;
use egui::{Event, Modifiers, Pos2, Rect, pos2, vec2};
use egui_kittest::Harness;
use serde_json::json;

fn harness() -> Harness<'static, EffectcraftApp> {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Matte", "width": 640, "height": 360, "duration": 4})).unwrap();
    s.execute("layer.newSolid", json!({"name": "Plate", "color": "#406080", "width": 640, "height": 360})).unwrap();
    s.execute("layer.newSolid", json!({"name": "Box", "color": "#e04020", "width": 80, "height": 80})).unwrap();
    s.execute("edit.deselectAll", json!({})).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(move |_| EffectcraftApp::new(s));
    h.run_steps(3);
    h
}

fn rect(h: &Harness<'_, EffectcraftApp>, id: &str) -> Rect {
    let e = h.state().auto.find(id).unwrap_or_else(|| panic!("no {id}"));
    Rect::from_min_size(pos2(e.rect[0], e.rect[1]), vec2(e.rect[2], e.rect[3]))
}

/// Comp pixel → screen point.
fn screen(h: &Harness<'_, EffectcraftApp>, p: [f32; 2]) -> Pos2 {
    let c = rect(h, "viewer.comp");
    let z = c.width() / 640.0;
    c.min + vec2(p[0] * z, p[1] * z)
}

fn press(h: &mut Harness<'_, EffectcraftApp>, p: Pos2, pressed: bool, modifiers: Modifiers) {
    h.input_mut().events.push(Event::ModifiersChanged(modifiers));
    h.input_mut().events.push(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed, modifiers });
    h.step();
}

fn click(h: &mut Harness<'_, EffectcraftApp>, p: Pos2, modifiers: Modifiers) {
    h.input_mut().events.push(Event::PointerMoved(p));
    h.step();
    press(h, p, true, modifiers);
    press(h, p, false, modifiers);
    h.input_mut().events.push(Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(2);
}

fn drag(h: &mut Harness<'_, EffectcraftApp>, from: Pos2, to: Pos2) -> Option<Rect> {
    h.input_mut().events.push(Event::PointerMoved(from));
    press(h, from, true, Modifiers::NONE);
    let mut drawn = None;
    for i in 1..=8 {
        h.input_mut().events.push(Event::PointerMoved(from + (to - from) * (i as f32 / 8.0)));
        h.step();
        drawn = h.state().auto.find("viewer.objectMatte.box").map(|e| Rect::from_min_size(pos2(e.rect[0], e.rect[1]), vec2(e.rect[2], e.rect[3])));
    }
    press(h, to, false, Modifiers::NONE);
    h.run_steps(2);
    drawn
}

fn box_layer(h: &Harness<'_, EffectcraftApp>) -> LayerId {
    h.state().session.active_comp().unwrap().layers.iter().find(|l| l.name == "Box").unwrap().id
}

/// The Box layer's Object Matte effects.
fn mattes(h: &Harness<'_, EffectcraftApp>) -> Vec<PropGroup> {
    let comp = h.state().session.active_comp().unwrap();
    let l = comp.layer(box_layer(h)).unwrap();
    l.effects().unwrap().groups().filter(|g| matches!(&g.kind, GroupKind::Effect { effect } if effect == om::ID)).cloned().collect()
}

fn v2(g: &PropGroup, id: &str) -> [f64; 2] {
    flatten_params(g, &mut |p| p.value.clone()).get(id).map(Value::as_vec2).unwrap()
}

fn near(a: [f64; 2], b: [f64; 2]) -> bool {
    (a[0] - b[0]).abs() < 1.5 && (a[1] - b[1]).abs() < 1.5
}

#[test]
fn object_matte_tool_prompts_with_a_click_and_a_box() {
    let mut h = harness();
    assert_eq!(Tool::from_name("objectMatte"), Some(Tool::ObjectMatte));
    assert!(Tool::SLOTS.iter().any(|s| s.contains(&Tool::RotoBrush) && s.contains(&Tool::ObjectMatte)));
    h.state_mut().ui.tool = Tool::ObjectMatte;
    h.run_steps(2);
    // Hover: a crosshair at the pointer and the outline of the layer that would get the matte.
    let center = screen(&h, [320.0, 180.0]);
    h.input_mut().events.push(Event::PointerMoved(center));
    h.run_steps(2);
    assert!(rect(&h, "viewer.objectMatte.crosshair").contains(center));
    let target = h.state().auto.find("viewer.objectMatte.target").expect("no target outline");
    assert_eq!(target.label, "Box");
    assert!(mattes(&h).is_empty(), "hovering must not apply anything");
    let undo0 = h.state().session.history.undo.len();

    // Click: a point prompt in layer pixels on the layer under the pointer, selected.
    click(&mut h, center, Modifiers::NONE);
    let m = mattes(&h);
    assert_eq!(m.len(), 1);
    assert!(near(v2(&m[0], "objectPoint"), [40.0, 40.0]), "{:?}", v2(&m[0], "objectPoint"));
    let uid = m[0].uid;
    let st = &h.state().session.state;
    assert!(st.selected_props.contains(&(box_layer(&h), uid)), "the new effect is selected");
    assert_eq!(h.state().session.history.undo.len(), undo0 + 1);

    // Drag: a box (drawn while dragging) re-prompts the same effect; its centre is the point.
    // (It starts clear of the selected effect's point controls, which take drags of their own.)
    let (a, b) = (screen(&h, [286.0, 150.0]), screen(&h, [356.0, 214.0]));
    let drawn = drag(&mut h, a, b).expect("no box drawn while dragging");
    assert!(drawn.width() > 20.0);
    let m = mattes(&h);
    assert_eq!(m.len(), 1);
    assert_eq!(m[0].uid, uid);
    let ps = flatten_params(&m[0], &mut |p| p.value.clone());
    assert_eq!(ps.get("useBox"), Some(&Value::Bool(true)));
    assert!(near(v2(&m[0], "boxTopLeft"), [6.0, 10.0]), "{:?}", v2(&m[0], "boxTopLeft"));
    assert!(near(v2(&m[0], "boxBottomRight"), [76.0, 74.0]), "{:?}", v2(&m[0], "boxBottomRight"));
    assert!(near(v2(&m[0], "objectPoint"), [41.0, 42.0]), "{:?}", v2(&m[0], "objectPoint"));
    assert_eq!(h.state().session.history.undo.len(), undo0 + 2);

    // Shift-click adds a second Object Matte.
    let p = screen(&h, [310.0, 170.0]);
    click(&mut h, p, Modifiers::SHIFT);
    let m = mattes(&h);
    assert_eq!(m.len(), 2);
    assert_eq!(m[1].name, "Object Matte 2");
    assert!(near(v2(&m[1], "objectPoint"), [30.0, 30.0]));

    // Undo takes the prompts back one at a time.
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    assert_eq!(mattes(&h).len(), 1);
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    assert_eq!(flatten_params(&mattes(&h)[0], &mut |p| p.value.clone()).get("useBox"), Some(&Value::Bool(false)));
}

/// Effect Controls: the Object Matte's Track and Freeze buttons run the background job and its
/// status line shows where it is; the UI keeps drawing frames while it runs and finishes the
/// job itself (a Freeze writes its mattes without any other job running).
#[test]
fn object_matte_effect_controls_track_and_freeze() {
    let mut h = harness();
    h.state_mut().show_panel(PanelKind::EffectControls);
    h.state_mut().ui.tool = Tool::ObjectMatte;
    h.run_steps(2);
    let center = screen(&h, [320.0, 180.0]);
    click(&mut h, center, Modifiers::NONE);
    let uid = mattes(&h)[0].uid;
    h.run_steps(2);
    let status = |h: &Harness<'_, EffectcraftApp>| h.state().auto.find(&format!("effectControls.objectMatte.{uid}.status")).map(|e| e.label.clone());
    assert!(status(&h).is_some(), "no Object Matte status line");
    // Freeze: a background job (the frames keep coming), then the effect is frozen.
    let b = rect(&h, &format!("effectControls.objectMatte.{uid}.freeze"));
    // Let any automatic tracking finish first (one job at a time).
    let frozen = |h: &Harness<'_, EffectcraftApp>| om::is_frozen(&flatten_params(&mattes(h)[0], &mut |p| p.value.clone()));
    let t = std::time::Instant::now();
    while h.state().session.is_roto_running() {
        assert!(t.elapsed().as_secs() < 120);
        h.step();
    }
    click(&mut h, b.center(), Modifiers::NONE);
    let t = std::time::Instant::now();
    while !frozen(&h) {
        assert!(
            t.elapsed().as_secs() < 20,
            "never frozen: {:?} / {} / running {} / {:?}",
            status(&h),
            h.state().ui.status,
            h.state().session.is_roto_running(),
            h.state().session.roto_progress()
        );
        h.step();
    }
    h.run_steps(2);
    assert!(status(&h).is_some_and(|s| s.starts_with("Frozen")), "{:?}", status(&h));
    assert_eq!(h.state().auto.find(&format!("effectControls.objectMatte.{uid}.freeze")).map(|e| e.label.as_str()), Some("Unfreeze"));
}
