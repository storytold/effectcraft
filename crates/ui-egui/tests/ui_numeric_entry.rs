//! CPU egui numeric entry through the real widget and authored property command; no frame worker.
use effectcraft_engine::{Session, project::LayerId};
use effectcraft_ui_egui::{
    theme::{ThemeKind, Tokens},
    widgets,
};
use egui::{Event, Key, Modifiers, PointerButton, Rect, pos2};
use egui_kittest::Harness;
use serde_json::json;

struct State {
    session: Session,
    layer: u64,
    rect: Rect,
}
fn value(state: &State) -> f64 {
    state.session.active_comp().unwrap().layer(LayerId(state.layer)).unwrap().props.prop("transform/opacity").unwrap().value.as_f64()
}
fn harness() -> Harness<'static, State> {
    let mut session = Session::default();
    session.execute("comp.new", json!({"name":"Numeric", "width":16,"height":16,"duration":1})).unwrap();
    let layer = session.execute("layer.newSolid", json!({"name":"A"})).unwrap()["layer"].as_u64().unwrap();
    session.execute("prop.set", json!({"layer":layer,"path":"transform/opacity","value":40})).unwrap();
    session.history.undo.clear();
    let state = State { session, layer, rect: Rect::NOTHING };
    let mut h = Harness::builder().with_size(egui::vec2(320.0, 100.0)).build_ui_state(
        |ui, state| {
            let (rect, change, _) = widgets::hot_number_at(
                ui,
                pos2(20.0, 20.0),
                egui::Id::new("numeric-entry-test"),
                value(state),
                0.5,
                (0.0, 100.0),
                1,
                "%",
                &Tokens::for_kind(ThemeKind::Dark),
            );
            state.rect = rect;
            if let Some(v) = change {
                state.session.execute("prop.set", json!({"layer":state.layer,"path":"transform/opacity","value":v})).unwrap();
            }
            ui.put(Rect::from_min_size(pos2(200.0, 20.0), egui::vec2(80.0, 25.0)), egui::Button::new("Elsewhere"));
        },
        state,
    );
    h.run_steps(3);
    h
}
fn click(h: &mut Harness<'_, State>, pos: egui::Pos2) {
    h.event(Event::PointerMoved(pos));
    h.step();
    h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(2);
}
fn type_text(h: &mut Harness<'_, State>, text: &str) {
    let pos = h.state().rect.center();
    click(h, pos);
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.event(Event::Text(text.into()));
    h.run_steps(2);
    assert_eq!(value(h.state()), 40.0, "Typing must not author a live value");
}
#[test]
fn arithmetic_enter_commits_once_and_clamps_with_real_undo() {
    for (text, expected) in [("2*3", 6.0), ("4/2", 2.0), ("2e2", 100.0), ("-(2+3)", 0.0), (" 2 + 3 * 4 ", 14.0)] {
        let mut h = harness();
        type_text(&mut h, text);
        h.key_press(Key::Enter);
        h.run_steps(3);
        assert_eq!(value(h.state()), expected, "{text}");
        assert_eq!(h.state().session.history.undo.len(), 1, "{text}");
        assert!(h.state_mut().session.undo());
        assert_eq!(value(h.state()), 40.0);
        assert!(h.state_mut().session.redo());
        assert_eq!(value(h.state()), expected);
    }
}
#[test]
fn invalid_arithmetic_focus_loss_and_escape_never_author() {
    for text in ["1/0", "2*", "1e309", "(2+3", "NaN"] {
        let mut h = harness();
        type_text(&mut h, text);
        click(&mut h, pos2(240.0, 32.0));
        h.run_steps(3);
        assert_eq!(value(h.state()), 40.0, "{text}");
        assert!(h.state().session.history.undo.is_empty(), "{text}");
    }
    let mut oversized = harness();
    type_text(&mut oversized, &format!("{}2*3", " ".repeat(510)));
    oversized.key_press(Key::Enter);
    oversized.run_steps(3);
    assert_eq!(value(oversized.state()), 40.0);
    assert!(oversized.state().session.history.undo.is_empty());
    let mut h = harness();
    type_text(&mut h, "2*3");
    h.key_press(Key::Escape);
    h.run_steps(3);
    click(&mut h, pos2(240.0, 32.0));
    assert_eq!(value(h.state()), 40.0);
    assert!(h.state().session.history.undo.is_empty());
}

#[test]
fn valid_click_away_commits_suffixed_comma_arithmetic_and_input_boundary() {
    for (text, expected) in [("2,5*2%".to_owned(), 5.0), (format!("{}2*3", " ".repeat(509)), 6.0)] {
        let mut h = harness();
        type_text(&mut h, &text);
        click(&mut h, pos2(240.0, 32.0));
        h.run_steps(3);
        assert_eq!(value(h.state()), expected);
        assert_eq!(h.state().session.history.undo.len(), 1);
        assert!(h.state_mut().session.undo());
        assert_eq!(value(h.state()), 40.0);
        assert!(h.state_mut().session.redo());
        assert_eq!(value(h.state()), expected);
    }
}

#[test]
fn escape_releases_focus_and_reopen_discards_arithmetic() {
    cancel_and_reopen(false);
}

#[test]
fn escape_and_click_away_same_frame_cancel_before_reopen() {
    cancel_and_reopen(true);
}

fn cancel_and_reopen(click_away: bool) {
    let mut h = harness();
    type_text(&mut h, "8*9");
    assert!(h.ctx.egui_wants_keyboard_input());
    h.event(Event::Key { key: Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    let away = pos2(240.0, 32.0);
    if click_away {
        h.event(Event::PointerMoved(away));
        h.event(Event::PointerButton { pos: away, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    }
    h.step();
    h.event(Event::Key { key: Key::Escape, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::NONE });
    if click_away {
        h.event(Event::PointerButton { pos: away, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    }
    h.run_steps(3);
    assert_eq!(value(h.state()), 40.0);
    assert!(h.state().session.history.undo.is_empty());
    assert!(!h.ctx.egui_wants_keyboard_input(), "Escape must release the keyboard");
    let p = h.state().rect.center();
    click(&mut h, p);
    assert!(h.ctx.egui_wants_keyboard_input());
    h.key_press(Key::End);
    h.event(Event::Text("+2".into()));
    h.run_steps(2);
    h.key_press(Key::Enter);
    h.run_steps(3);
    assert_eq!(value(h.state()), 42.0, "reopen must use authored 40, not canceled 8*9");
    assert_eq!(h.state().session.history.undo.len(), 1);
    assert!(h.state_mut().session.undo());
    assert_eq!(value(h.state()), 40.0);
    assert!(h.state_mut().session.redo());
    assert_eq!(value(h.state()), 42.0);
}

#[test]
fn clicking_numeric_value_selects_all_for_arithmetic_without_select_all_shortcut() {
    for click_away in [false, true] {
        let mut h = harness();
        let pos = h.state().rect.center();
        click(&mut h, pos);
        h.event(Event::Text("2*3".into()));
        h.run_steps(2);
        let buffer = h.ctx.data(|d| d.get_temp::<String>(egui::Id::new("numeric-entry-test").with("editing")));
        assert_eq!(buffer.as_deref(), Some("2*3"), "typing after opening must replace authored40 without Ctrl/Cmd+A");
        assert_eq!(value(h.state()), 40.0, "typing must not author before commit");
        assert!(h.state().session.history.undo.is_empty());
        if click_away {
            click(&mut h, pos2(240.0, 32.0));
        } else {
            h.key_press(Key::Enter);
        }
        h.run_steps(3);
        assert_eq!(value(h.state()), 6.0);
        assert_eq!(h.state().session.history.undo.len(), 1);
        assert!(h.state_mut().session.undo());
        assert_eq!(value(h.state()), 40.0);
        assert!(h.state_mut().session.redo());
        assert_eq!(value(h.state()), 6.0);
    }
}
