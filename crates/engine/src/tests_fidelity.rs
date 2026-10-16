//! Render-fidelity commands: project colour settings, footage colour profile, slip edit.

use effectcraft_project::{BitDepth, ColorSpace};
use serde_json::json;

use crate::Session;

#[test]
fn project_colour_settings_are_undoable() {
    let mut s = Session::default();
    s.execute("file.projectSettings", json!({"bitDepth": 32, "workingSpace": "rec2020", "linearize": true, "blendLinear": true})).unwrap();
    let st = &s.project.settings;
    assert_eq!((st.bit_depth, st.working_space, st.linearize, st.blend_linear), (BitDepth::Bpc32, Some(ColorSpace::Rec2020), true, true));
    s.execute("file.projectSettings", json!({"workingSpace": "Display P3"})).unwrap();
    assert_eq!(s.project.settings.working_space, Some(ColorSpace::DisplayP3));
    s.execute("file.projectSettings", json!({"workingSpace": "none"})).unwrap();
    assert_eq!(s.project.settings.working_space, None);
    assert!(s.execute("file.projectSettings", json!({"workingSpace": "acescg-ish"})).is_err());
    assert!(s.undo());
    assert_eq!(s.project.settings.working_space, Some(ColorSpace::DisplayP3));
    assert!(s.undo() && s.undo());
    assert_eq!(s.project.settings.bit_depth, BitDepth::Bpc8);
    assert!(!s.project.settings.blend_linear);
    // The Project panel's depth button cycles 8 → 16 → 32 → 8.
    for want in ["16 bpc", "32 bpc", "8 bpc"] {
        assert_eq!(s.execute("file.cycleBitDepth", json!({})).unwrap(), json!(want));
    }
}

#[test]
fn footage_colour_profile_is_interpreted() {
    let mut s = Session::default();
    let id = s.execute("file.importPlaceholder", json!({"name": "clip", "width": 64, "height": 36, "frameRate": 24, "duration": 2})).unwrap();
    let item = id["item"].as_u64().or_else(|| id.as_u64()).map(effectcraft_project::ItemId).unwrap_or_else(|| *s.project.items.keys().last().unwrap());
    let profile = |s: &Session| match &s.project.item(item).unwrap().kind {
        effectcraft_project::ItemKind::Footage(f) => f.color_profile,
        _ => panic!("not footage"),
    };
    s.execute("file.interpretFootage", json!({"items": [item.0], "colorProfile": "rec2020"})).unwrap();
    assert_eq!(profile(&s), Some(ColorSpace::Rec2020));
    s.execute("file.interpretFootage", json!({"items": [item.0], "colorProfile": "auto"})).unwrap();
    assert_eq!(profile(&s), None);
    assert!(s.undo());
    assert_eq!(profile(&s), Some(ColorSpace::Rec2020));
}

#[test]
fn slip_moves_the_source_under_fixed_in_and_out() {
    use effectcraft_time::Tick;
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 100, "height": 100, "frameRate": 30, "duration": 10})).unwrap();
    let nested = s.execute("comp.new", json!({"name": "Nested", "duration": 4, "open": false})).unwrap()["comp"].as_u64().unwrap();
    s.execute("layer.addItem", json!({"item": nested})).unwrap();
    let id = s.active_comp().unwrap().layers[0].id.0;
    // Trim to 1 s .. 3 s of a 4 s source.
    s.execute("layer.timing", json!({"layers": [id], "in": 1.0, "out": 3.0})).unwrap();
    let get = |s: &Session| s.active_comp().unwrap().layers[0].clone();
    let before = get(&s);
    assert_eq!((before.start_time, before.in_point.seconds(), before.out_point.seconds()), (Tick::ZERO, 1.0, 3.0));
    // Slip the source 0.5 s later: in/out stay, the source (start) moves.
    s.execute("layer.slip", json!({"layers": [id], "delta": 0.5})).unwrap();
    let l = get(&s);
    assert_eq!((l.start_time.seconds(), l.in_point, l.out_point), (0.5, before.in_point, before.out_point));
    // The source time at comp 2 s is now 1.5 s.
    assert_eq!(l.layer_time(Tick::from_seconds_f64(2.0)).seconds(), 1.5);
    // Clamped so the source still covers in..out: at most start = in (1 s).
    let r = s.execute("layer.slip", json!({"layers": [id], "delta": 5.0})).unwrap();
    assert_eq!(get(&s).start_time.seconds(), 1.0);
    assert_eq!(r["slipped"][0]["delta"], json!(0.5));
    // Slip by a frame.
    s.execute("layer.slipBack", json!({"layers": [id]})).unwrap();
    assert!((get(&s).start_time.seconds() - (1.0 - 1.0 / 30.0)).abs() < 1e-9);
    s.execute("layer.slipForward", json!({"layers": [id]})).unwrap();
    assert_eq!(get(&s).start_time.seconds(), 1.0);
    // Undo walks back through the slips.
    assert!(s.undo() && s.undo() && s.undo() && s.undo());
    assert_eq!(get(&s).start_time, Tick::ZERO);
    // Alt+PageUp / Alt+PageDown move the layer; the one-frame slips have no default key.
    assert_eq!(crate::find_command("layer.slipBack").and_then(|c| c.shortcut), None);
    assert_eq!(crate::find_command("layer.slipForward").and_then(|c| c.shortcut), None);
    // Layers without a source duration can't slip.
    s.execute("layer.newSolid", json!({"color": "#ff0000"})).unwrap();
    assert!(s.execute("layer.slip", json!({"delta": 0.5})).is_err());
}
