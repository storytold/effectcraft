//! Angle values read `{rev}x ±deg°` (After Effects); the revolutions part is its own field
//! (issue #93): it has an automation id, typing N sets N whole turns keeping the degrees, and
//! dragging it scrubs in 360° steps (one undo step per drag). Checked on layer Rotation in the
//! Timeline and on an Angle Control in Effect Controls.

use effectcraft_engine::Session;
use effectcraft_engine::project::LayerId;
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::dock::PanelKind;
use egui::{Event, Pos2, Rect, pos2, vec2};
use egui_kittest::Harness;
use serde_json::json;

const ROTATION: &str = "transform/rotation";

/// A comp with one solid (Rotation 30°) carrying an Angle Control (30°), selected, its Rotation
/// revealed in the Timeline, Effect Controls shown. Returns the harness, the layer, the
/// Rotation uid and the Angle Control's angle uid.
fn harness() -> (Harness<'static, EffectcraftApp>, LayerId, u64, u64) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Angles", "width": 320, "height": 180, "frameRate": 30, "duration": 4})).unwrap();
    s.execute("layer.newSolid", json!({"name": "Box", "color": "#e04020", "width": 80, "height": 80})).unwrap();
    let id = s.active_comp().unwrap().layers[0].id;
    s.execute("prop.set", json!({"layer": id.0, "path": ROTATION, "value": 30.0})).unwrap();
    let fx = s.execute("effect.apply", json!({"layers": [id.0], "effect": "Angle Control"})).unwrap()["effects"][0].as_u64().unwrap();
    let l = s.active_comp().unwrap().layer(id).unwrap().clone();
    let rot = l.props.prop(ROTATION).unwrap().uid;
    let angle = l.effects().unwrap().groups().find(|g| g.uid == fx).and_then(|g| g.get("angle")).map(|p| p.uid).unwrap();
    s.execute("prop.set", json!({"layer": id.0, "prop": angle, "value": 30.0})).unwrap();
    s.execute("layer.select", json!({"layers": [id.0]})).unwrap();
    s.state.selected_props.clear();
    let mut app = EffectcraftApp::new(s);
    app.show_panel(PanelKind::EffectControls);
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| app);
    h.run_steps(3);
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "timeline.reveal.rotation", json!({})).unwrap();
    h.run_steps(4);
    (h, id, rot, angle)
}

fn value(h: &Harness<'_, EffectcraftApp>, id: LayerId, uid: u64) -> f64 {
    let l = h.state().session.active_comp().unwrap().layer(id).unwrap().clone();
    let p = l.props.find(uid).expect("property");
    p.value.components()[0]
}

fn rect(h: &Harness<'_, EffectcraftApp>, auto: &str) -> Option<Rect> {
    h.state()
        .auto
        .elements
        .iter()
        .chain(h.state().auto.previous.iter())
        .find(|e| e.id == auto)
        .map(|e| Rect::from_min_size(pos2(e.rect[0], e.rect[1]), vec2(e.rect[2], e.rect[3])))
}

fn click(h: &mut Harness<'_, EffectcraftApp>, pos: Pos2) {
    h.input_mut().events.push(Event::PointerMoved(pos));
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
    h.run_steps(2);
}

/// Click the field, clear it, type `text`, Enter.
fn type_into(h: &mut Harness<'_, EffectcraftApp>, auto: &str, text: &str) {
    let r = rect(h, auto).unwrap_or_else(|| panic!("{auto} registered"));
    click(h, r.center());
    for _ in 0..8 {
        h.key_press(egui::Key::Backspace);
        h.step();
    }
    h.event(Event::Text(text.into()));
    h.step();
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
}

/// Press at `from`, move `dx` points right in small steps (a real drag), release.
fn drag(h: &mut Harness<'_, EffectcraftApp>, auto: &str, dx: f32) {
    let from = rect(h, auto).unwrap_or_else(|| panic!("{auto} registered")).center();
    let to = from + vec2(dx, 0.0);
    h.input_mut().events.push(Event::PointerMoved(from));
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
    h.step();
    for i in 1..=24 {
        h.input_mut().events.push(Event::PointerMoved(from + (to - from) * (i as f32 / 24.0)));
        h.step();
    }
    h.input_mut().events.push(Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
    h.run_steps(3);
}

fn whole_turns_from(start: f64, now: f64) -> f64 {
    let turns = (now - start) / 360.0;
    assert!((turns - turns.round()).abs() < 1e-9, "{start}° → {now}° is not whole turns");
    turns.round()
}

fn check(field: &str, rot_or_angle: fn(u64, u64) -> u64) {
    let (mut h, id, rot, angle) = harness();
    let uid = rot_or_angle(rot, angle);
    let revs = format!("{field}.{uid}.revolutions");
    let degs = format!("{field}.{uid}.value");
    assert!(rect(&h, &revs).is_some(), "{revs} registered");
    let (rr, dr) = (rect(&h, &revs).unwrap(), rect(&h, &degs).unwrap());
    assert!(rr.max.x <= dr.min.x + 1.0, "the revolutions field sits left of the degrees field: {rr:?} {dr:?}");

    // Typing 3 gives three turns plus the 30° that were there.
    type_into(&mut h, &revs, "3");
    assert_eq!(value(&h, id, uid), 3.0 * 360.0 + 30.0, "{field}: typed 3x");
    // Typing -1 reads -1x-30° (AE's display of -390°).
    type_into(&mut h, &revs, "-1");
    assert_eq!(value(&h, id, uid), -390.0, "{field}: typed -1x");
    type_into(&mut h, &revs, "0");
    assert_eq!(value(&h, id, uid), -30.0, "{field}: typed 0x keeps -30°");
    type_into(&mut h, &revs, "2");
    assert_eq!(value(&h, id, uid), 750.0, "{field}: typed 2x");

    // Dragging right adds whole turns, left takes them away; one undo step per drag.
    // (Back-to-back edits of one field share its merge key, as for every hot number, so each
    // drag starts from a fresh undo step here.)
    h.state_mut().session.history.merge_key = None;
    let undo = h.state().session.history.undo.len();
    drag(&mut h, &revs, 60.0);
    let up = whole_turns_from(750.0, value(&h, id, uid));
    assert!(up >= 2.0, "{field}: a 60 pt drag adds turns ({up})");
    assert_eq!(h.state().session.history.undo.len(), undo + 1, "{field}: one undo step per drag");
    let at = value(&h, id, uid);
    h.state_mut().session.history.merge_key = None;
    drag(&mut h, &revs, -60.0);
    let down = whole_turns_from(at, value(&h, id, uid));
    assert!(down <= -2.0, "{field}: a drag left takes turns away ({down})");
    assert_eq!(h.state().session.history.undo.len(), undo + 2, "{field}: one undo step per drag");
}

#[test]
fn timeline_rotation_revolutions_are_editable() {
    check("timeline.prop", |rot, _| rot);
}

#[test]
fn effect_controls_angle_revolutions_are_editable() {
    check("effectControls.prop", |_, angle| angle);
}
