//! Control-channel clicks and drags hold their modifiers for the whole gesture: egui takes
//! `i.modifiers` from `ModifiersChanged` events, not from the pointer events, and synthetic
//! events arrive one per frame, so a Shift-click must hold Shift from its press to its release.

use effectcraft_engine::Session;
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::control::{self, ControlRequest};
use serde_json::{Value, json};

/// Feed the queued synthetic input frame by frame; per frame: (has a press/release, the
/// modifiers egui holds in that frame, which only `ModifiersChanged` events change).
fn frames(app: &mut EffectcraftApp) -> Vec<(bool, egui::Modifiers)> {
    let mut out = vec![];
    let mut held = egui::Modifiers::NONE;
    for _ in 0..64 {
        let mut raw = egui::RawInput::default();
        app.feed_synthetic(&mut raw);
        if raw.events.is_empty() {
            break;
        }
        let mut button = false;
        for e in &raw.events {
            match e {
                egui::Event::ModifiersChanged(m) => held = *m,
                egui::Event::PointerButton { .. } => button = true,
                _ => {}
            }
        }
        out.push((button, held));
    }
    out
}

fn send(app: &mut EffectcraftApp, method: &str, params: Value) {
    let ctx = egui::Context::default();
    let (req, _rx) = ControlRequest::new(method, params);
    let _ = control::handle(app, &ctx, &req);
}

#[test]
fn shift_click_holds_shift_from_press_to_release() {
    let mut app = EffectcraftApp::new(Session::default());
    send(&mut app, "ui.click", json!({"x": 10, "y": 10, "modifiers": {"shift": true}}));
    let f = frames(&mut app);
    let buttons: Vec<_> = f.iter().filter(|(b, _)| *b).collect();
    assert_eq!(buttons.len(), 2, "press and release: {f:?}");
    assert!(buttons.iter().all(|(_, m)| m.shift), "Shift on the press and the release: {f:?}");
    // Afterwards the modifiers are released again.
    send(&mut app, "ui.click", json!({"x": 10, "y": 10}));
    assert!(frames(&mut app).iter().all(|(_, m)| !m.shift), "a plain click after it has no Shift");
}

#[test]
fn shift_drag_holds_shift_through_the_moves() {
    let mut app = EffectcraftApp::new(Session::default());
    send(&mut app, "ui.drag", json!({"from": {"x": 10, "y": 10}, "to": {"x": 90, "y": 40}, "steps": 4, "modifiers": {"shift": true}}));
    let f = frames(&mut app);
    let first = f.iter().position(|(b, _)| *b).expect("press");
    let last = f.iter().rposition(|(b, _)| *b).expect("release");
    assert!(last > first + 1, "moves between press and release: {f:?}");
    assert!(f[first..=last].iter().all(|(_, m)| m.shift), "Shift held for the whole drag: {f:?}");
}
