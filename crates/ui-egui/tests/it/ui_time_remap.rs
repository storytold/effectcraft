//! #3071: Time Remap edits are in frames, while commands and keyframes stay in seconds.

use effectcraft_engine::Session;
use effectcraft_engine::project::{LayerId, TimeDisplayStyle};
use effectcraft_ui_egui::{EffectcraftApp, dock::PanelKind};
use egui::{Event, Key, Modifiers, PointerButton, Pos2, Rect, Shape, pos2, vec2};
use egui_kittest::Harness;
use serde_json::json;

/// A 24 fps source nested in a variable-rate comp, with the first remap key at `seconds`.
fn harness(fps: f64, seconds: f64) -> (Harness<'static, EffectcraftApp>, u64, u64) {
    let mut s = Session::default();
    s.prefs.disk.disk_cache_enabled = false;
    let inner = s.execute("comp.new", json!({"name": "Source", "width": 32, "height": 32, "frameRate": 24, "duration": 8})).unwrap()["comp"].as_u64().unwrap();
    s.execute("comp.new", json!({"name": "Remap", "width": 64, "height": 64, "frameRate": fps, "duration": 8})).unwrap();
    let layer = s.execute("layer.addItem", json!({"item": inner})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.enableTimeRemap", json!({"layers": [layer]})).unwrap();
    s.execute("prop.set", json!({"layer": layer, "path": "timeRemap", "value": seconds})).unwrap();
    let uid = s.active_comp().unwrap().layer(LayerId(layer)).unwrap().props.prop("timeRemap").unwrap().uid;
    let mut app = EffectcraftApp::new(s);
    app.show_panel(PanelKind::Timeline);
    app.toggle_maximize(PanelKind::Timeline);
    app.ui.timeline.open_layers.insert(layer);
    app.ui.timeline.layer_reveal.insert(layer, vec!["timeRemap".into()]);
    let mut h = Harness::builder().with_size(vec2(1400.0, 700.0)).build_eframe(|_| app);
    h.run_steps(4);
    (h, layer, uid)
}

/// Locate the real value editor through its automation target.
fn field(h: &Harness<'_, EffectcraftApp>, uid: u64) -> Rect {
    let r = h.state().auto.find(&format!("timeline.prop.{uid}.value")).unwrap().rect;
    Rect::from_min_size(pos2(r[0], r[1]), vec2(r[2], r[3]))
}

/// Read painted value text, rather than trusting the control's automation label.
fn shown(h: &Harness<'_, EffectcraftApp>, uid: u64) -> Vec<String> {
    /// Flatten nested paint shapes while retaining only text inside this field.
    fn walk(shape: &Shape, area: Rect, out: &mut Vec<String>) {
        match shape {
            Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, area, out)),
            Shape::Text(t) if area.expand(1.0).contains_rect(t.visual_bounding_rect()) => out.push(t.galley.text().to_string()),
            _ => {}
        }
    }
    let mut out = vec![];
    for shape in &h.output().shapes {
        walk(&shape.shape, field(h, uid), &mut out);
    }
    out
}

/// Read source seconds from the key at this fixture's unchanged, zero-time playhead.
fn value(h: &Harness<'_, EffectcraftApp>, layer: u64, uid: u64) -> f64 {
    h.state().session.active_comp().unwrap().layer(LayerId(layer)).unwrap().props.find(uid).unwrap().keys[0].value.as_f64()
}

/// Move the pointer, optionally change its button state, and hold gesture modifiers.
fn pointer(h: &mut Harness<'_, EffectcraftApp>, at: Pos2, pressed: Option<bool>, modifiers: Modifiers) {
    h.event(Event::ModifiersChanged(modifiers));
    h.event(Event::PointerMoved(at));
    if let Some(pressed) = pressed {
        h.event(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers });
    }
    h.step();
}

/// Send an unmodified key press/release and settle focus changes.
fn key(h: &mut Harness<'_, EffectcraftApp>, key: Key) {
    for pressed in [true, false] {
        h.event(Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE });
    }
    h.run_steps(2);
}

/// Replace a clicked value's selected text, then commit or cancel with `commit`.
fn type_value(h: &mut Harness<'_, EffectcraftApp>, uid: u64, text: &str, commit: Key) {
    let at = field(h, uid).center();
    pointer(h, at, Some(true), Modifiers::NONE);
    pointer(h, at, Some(false), Modifiers::NONE);
    h.run_steps(2);
    assert!(h.ctx.egui_wants_keyboard_input());
    h.event(Event::Text(text.into()));
    h.step();
    key(h, commit);
}

/// Composition frames remain explicit regardless of timecode style or frame numbering.
#[test]
fn remap_displays_composition_frames_in_both_time_display_styles() {
    let (mut h, layer, uid) = harness(30.0, 1.5);
    if let Ok(path) = std::env::var("TIME_REMAP_REVIEW_PNG") {
        use crate::gpu_test::RenderSerial as _;
        h.render_serialized().unwrap().save(path).unwrap();
    }
    for style in [TimeDisplayStyle::Timecode, TimeDisplayStyle::Frames] {
        let project = std::sync::Arc::make_mut(&mut h.state_mut().session.project);
        project.settings.time_display = style;
        project.settings.frame_start = 1;
        h.run_steps(2);
        assert_eq!(shown(&h, uid), ["45.00f"]);
        assert_eq!(value(&h, layer, uid), 1.5, "drawing never converts stored values");
    }
}

/// Typed subframes convert at integer and NTSC rates and round-trip through project history.
#[test]
fn typed_frames_commit_seconds_with_undo_and_redo() {
    for fps in [24.0, 30.0, 24000.0 / 1001.0, 60.0] {
        let (mut h, layer, uid) = harness(fps, 1.5);
        let before = h.state().session.history.undo.len();
        type_value(&mut h, uid, "-12.5f", Key::Enter);
        assert!((value(&h, layer, uid) - -12.5 / fps).abs() < 1e-9, "{fps} fps");
        assert_eq!(h.state().session.history.undo.len(), before + 1);
        h.state_mut().session.execute("edit.undo", json!({})).unwrap();
        assert_eq!(value(&h, layer, uid), 1.5);
        h.state_mut().session.execute("edit.redo", json!({})).unwrap();
        assert!((value(&h, layer, uid) - -12.5 / fps).abs() < 1e-9);
    }
}

/// A measured drag uses quarter-frames per pixel and merges into one undo step.
#[test]
fn dragging_is_frame_based_and_keeps_fine_and_coarse_modifiers() {
    for (modifiers, multiplier) in [(Modifiers::NONE, 1.0), (Modifiers::COMMAND, 0.1), (Modifiers::SHIFT, 10.0)] {
        let (mut h, layer, uid) = harness(30.0, 1.5);
        let before = h.state().session.history.undo.len();
        let at = field(&h, uid).center();
        pointer(&mut h, at, Some(true), modifiers);
        pointer(&mut h, at + vec2(8.0, 0.0), None, modifiers);
        let midway = value(&h, layer, uid);
        pointer(&mut h, at + vec2(28.0, 0.0), None, modifiers);
        pointer(&mut h, at + vec2(28.0, 0.0), Some(false), modifiers);
        h.run_steps(2);
        assert!((value(&h, layer, uid) - midway - 5.0 * multiplier / 30.0).abs() < 1e-9, "{modifiers:?}");
        assert_eq!(h.state().session.history.undo.len(), before + 1, "a drag is one undo step");
        h.state_mut().session.execute("edit.undo", json!({})).unwrap();
        assert_eq!(value(&h, layer, uid), 1.5);
    }
}

/// Cancellation and rejected input must leave both source values and history untouched.
#[test]
fn escape_and_invalid_frame_input_preserve_values_and_history() {
    for (text, commit) in [("90", Key::Escape), ("NaN", Key::Enter), ("inf", Key::Enter), ("1e999", Key::Enter), ("invalid", Key::Enter)] {
        let (mut h, layer, uid) = harness(30.0, 1.5);
        let before = h.state().session.history.undo.len();
        type_value(&mut h, uid, text, commit);
        assert_eq!(value(&h, layer, uid), 1.5, "{text}");
        assert_eq!(h.state().session.history.undo.len(), before);
    }
}

/// Enter, Tab and click-away must not write the two-decimal display rounding back to a key.
#[test]
fn closing_a_rounded_frame_field_without_typing_keeps_subframe_precision() {
    for commit in [Some(Key::Enter), Some(Key::Tab), None] {
        let (mut h, layer, uid) = harness(24000.0 / 1001.0, 1.0);
        let before = h.state().session.history.undo.len();
        let at = field(&h, uid).center();
        pointer(&mut h, at, Some(true), Modifiers::NONE);
        pointer(&mut h, at, Some(false), Modifiers::NONE);
        h.run_steps(2);
        if let Some(k) = commit {
            key(&mut h, k);
        } else {
            pointer(&mut h, pos2(1200.0, 600.0), Some(true), Modifiers::NONE);
            pointer(&mut h, pos2(1200.0, 600.0), Some(false), Modifiers::NONE);
            h.run_steps(2);
        }
        assert_eq!(value(&h, layer, uid), 1.0, "{commit:?}");
        assert_eq!(h.state().session.history.undo.len(), before);
    }
}

/// Selected layers share a scrub's delta but receive the same absolute typed source time.
#[test]
fn selected_remaps_keep_their_offsets_when_dragged_and_share_typed_values() {
    let (mut h, layer, uid) = harness(30.0, 1.5);
    let s = &mut h.state_mut().session;
    let source = s.active_comp().unwrap().layer(LayerId(layer)).unwrap().source.item().unwrap();
    let other = s.execute("layer.addItem", json!({"item": source.0})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.enableTimeRemap", json!({"layers": [other]})).unwrap();
    s.execute("prop.set", json!({"layer": other, "path": "timeRemap", "value": 2.5})).unwrap();
    let other_uid = s.active_comp().unwrap().layer(LayerId(other)).unwrap().props.prop("timeRemap").unwrap().uid;
    s.execute("layer.select", json!({"layers": [layer, other]})).unwrap();
    h.run_steps(3);
    let before = h.state().session.history.undo.len();
    let at = field(&h, uid).center();
    pointer(&mut h, at, Some(true), Modifiers::NONE);
    pointer(&mut h, at + vec2(20.0, 0.0), None, Modifiers::NONE);
    pointer(&mut h, at + vec2(40.0, 0.0), None, Modifiers::NONE);
    pointer(&mut h, at + vec2(40.0, 0.0), Some(false), Modifiers::NONE);
    h.run_steps(2);
    let delta = value(&h, layer, uid) - 1.5;
    assert!(delta > 0.0);
    assert!((value(&h, other, other_uid) - 2.5 - delta).abs() < 1e-9);
    assert_eq!(h.state().session.history.undo.len(), before + 1);
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    assert_eq!(value(&h, layer, uid), 1.5);
    assert_eq!(value(&h, other, other_uid), 2.5);
    h.run_steps(2);
    type_value(&mut h, uid, "60", Key::Enter);
    assert_eq!(value(&h, layer, uid), 2.0);
    assert_eq!(value(&h, other, other_uid), 2.0);
}

/// Text-field Undo restores the original buffer without quantizing the stored source time.
#[test]
fn undoing_the_text_edit_before_committing_keeps_subframe_precision() {
    let (mut h, layer, uid) = harness(24000.0 / 1001.0, 1.0);
    let before = h.state().session.history.undo.len();
    let at = field(&h, uid).center();
    pointer(&mut h, at, Some(true), Modifiers::NONE);
    pointer(&mut h, at, Some(false), Modifiers::NONE);
    h.run_steps(2);
    h.event(Event::Text("90".into()));
    h.run_steps(2);
    h.event(Event::ModifiersChanged(Modifiers::COMMAND));
    for pressed in [true, false] {
        h.event(Event::Key { key: Key::Z, physical_key: None, pressed, repeat: false, modifiers: Modifiers::COMMAND });
    }
    h.run_steps(2);
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    key(&mut h, Key::Enter);
    assert_eq!(value(&h, layer, uid), 1.0);
    assert_eq!(h.state().session.history.undo.len(), before);
}
