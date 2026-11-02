//! Angle fields must cancel on Escape and reject non-finite numeric input.

use effectcraft_engine::Session;
use effectcraft_engine::project::LayerId;
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::dock::PanelKind;
use egui::{Event, Key, Modifiers, Pos2, pos2, vec2};
use egui_kittest::Harness;
use serde_json::json;

#[derive(Clone, Copy, Debug)]
enum Panel {
    EffectControls,
    Timeline,
    Properties,
}

impl Panel {
    fn kind(self) -> PanelKind {
        match self {
            Self::EffectControls => PanelKind::EffectControls,
            Self::Timeline => PanelKind::Timeline,
            Self::Properties => PanelKind::Properties,
        }
    }

    fn prefix(self) -> &'static str {
        match self {
            Self::EffectControls => "effectControls",
            Self::Timeline => "timeline",
            Self::Properties => "properties",
        }
    }
}

fn harness(panel: Panel, angle: f64) -> (Harness<'static, EffectcraftApp>, u64, u64) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Angles", "width": 320, "height": 180, "duration": 4})).unwrap();
    let layer = s.execute("layer.newSolid", json!({"name": "Plate", "color": "#40b080"})).unwrap()["layer"].as_u64().unwrap();
    let uid = if matches!(panel, Panel::EffectControls) {
        let fx = s.execute("effect.apply", json!({"layer": layer, "effect": "CC Bend It"})).unwrap()["effects"][0].as_u64().unwrap();
        s.active_comp().unwrap().layer(LayerId(layer)).unwrap().props.find_group(fx).unwrap().get("bend").unwrap().uid
    } else {
        s.active_comp().unwrap().layer(LayerId(layer)).unwrap().props.prop("transform/rotation").unwrap().uid
    };
    s.execute("prop.set", json!({"layer": layer, "prop": uid, "value": angle})).unwrap();
    let mut app = EffectcraftApp::new(s);
    app.show_panel(panel.kind());
    app.toggle_maximize(panel.kind());
    app.ui.timeline.layer_reveal.insert(layer, vec!["rotation".into()]);
    app.ui.timeline.open_layers.insert(layer);
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_eframe(|_| app);
    h.run_steps(4);
    (h, layer, uid)
}

fn rect(h: &Harness<'_, EffectcraftApp>, id: &str) -> egui::Rect {
    let e = h.state().auto.find(id).unwrap_or_else(|| panic!("no {id}"));
    egui::Rect::from_min_size(pos2(e.rect[0], e.rect[1]), vec2(e.rect[2], e.rect[3]))
}

fn component(h: &Harness<'_, EffectcraftApp>, panel: Panel, uid: u64, revolutions: bool) -> Pos2 {
    let base = format!("{}.prop.{uid}", panel.prefix());
    if revolutions {
        return rect(h, &format!("{base}.revolutions")).center();
    }
    let value = rect(h, &format!("{base}.value"));
    if matches!(panel, Panel::EffectControls) {
        // Effect Controls' legacy value target includes both components.
        pos2(value.max.x - 12.0, value.center().y)
    } else {
        value.center()
    }
}

fn click(h: &mut Harness<'_, EffectcraftApp>, p: Pos2) {
    h.input_mut().events.push(Event::PointerMoved(p));
    h.input_mut().events.push(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
}

fn key(h: &mut Harness<'_, EffectcraftApp>, key: Key, modifiers: Modifiers) {
    h.input_mut().events.push(Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers });
    h.input_mut().events.push(Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers });
    h.run_steps(2);
}

fn begin_edit(h: &mut Harness<'_, EffectcraftApp>, p: Pos2, text: &str) {
    click(h, p);
    assert!(h.ctx.egui_wants_keyboard_input(), "the numeric editor has focus");
    key(h, Key::A, Modifiers::COMMAND);
    h.input_mut().events.push(Event::Text(text.into()));
    h.step();
}

fn angle(h: &Harness<'_, EffectcraftApp>, layer: u64, uid: u64) -> f64 {
    h.state().session.active_comp().unwrap().layer(LayerId(layer)).unwrap().props.find(uid).unwrap().value.as_f64()
}

#[test]
fn escape_cancels_revolution_edits() {
    cancel_edits(true);
}

#[test]
fn escape_cancels_degree_edits() {
    cancel_edits(false);
}

fn cancel_edits(revolutions: bool) {
    for panel in [Panel::EffectControls, Panel::Timeline, Panel::Properties] {
        let (mut h, layer, uid) = harness(panel, 390.5);
        let undo = h.state().session.history.undo.len();
        let p = component(&h, panel, uid, revolutions);
        begin_edit(&mut h, p, "3");
        key(&mut h, Key::Escape, Modifiers::NONE);
        assert_eq!(angle(&h, layer, uid), 390.5, "{panel:?}: Escape cancels");
        assert_eq!(h.state().session.history.undo.len(), undo, "cancellation adds no undo step");
        assert!(!h.ctx.egui_wants_keyboard_input(), "Escape releases the keyboard");
    }
}

#[test]
fn nonfinite_revolutions_do_not_change_the_angle() {
    invalid_edits(true);
}

#[test]
fn nonfinite_degrees_do_not_change_the_angle() {
    invalid_edits(false);
}

fn invalid_edits(revolutions: bool) {
    for panel in [Panel::EffectControls, Panel::Timeline, Panel::Properties] {
        for text in ["invalid", "NaN", "inf", "-inf", "1e999"] {
            let (mut h, layer, uid) = harness(panel, 390.5);
            let undo = h.state().session.history.undo.len();
            let p = component(&h, panel, uid, revolutions);
            begin_edit(&mut h, p, text);
            key(&mut h, Key::Enter, Modifiers::NONE);
            assert_eq!(angle(&h, layer, uid), 390.5, "{panel:?}: reject {text}");
            assert_eq!(h.state().session.history.undo.len(), undo, "invalid input adds no undo step");
        }
    }
}

#[test]
fn valid_angles_commit_on_enter_with_undo() {
    commit_edits(false);
}

#[test]
fn valid_angles_commit_on_click_away_with_undo() {
    commit_edits(true);
}

/// Clicking a value selects all of it: typing replaces it without selecting it first (#170).
#[test]
fn typing_replaces_the_clicked_value() {
    for panel in [Panel::EffectControls, Panel::Timeline, Panel::Properties] {
        for (revolutions, text, expected) in [(true, "3", 1110.5), (false, "45.5", 405.5)] {
            let (mut h, layer, uid) = harness(panel, 390.5);
            let p = component(&h, panel, uid, revolutions);
            click(&mut h, p);
            h.input_mut().events.push(Event::Text(text.into()));
            h.step();
            key(&mut h, Key::Enter, Modifiers::NONE);
            assert_eq!(angle(&h, layer, uid), expected, "{panel:?}");
        }
    }
}

fn commit_edits(click_away: bool) {
    for panel in [Panel::EffectControls, Panel::Timeline, Panel::Properties] {
        for (revolutions, text, expected) in [(true, "3", 1110.5), (false, "45.5", 405.5)] {
            let (mut h, layer, uid) = harness(panel, 390.5);
            let undo = h.state().session.history.undo.len();
            let p = component(&h, panel, uid, revolutions);
            begin_edit(&mut h, p, text);
            if click_away {
                click(&mut h, pos2(1200.0, 900.0));
            } else {
                key(&mut h, Key::Enter, Modifiers::NONE);
            }
            assert_eq!(angle(&h, layer, uid), expected, "{panel:?}");
            assert_eq!(h.state().session.history.undo.len(), undo + 1);
            h.state_mut().session.execute("edit.undo", json!({})).unwrap();
            assert_eq!(angle(&h, layer, uid), 390.5);
        }
    }
}
