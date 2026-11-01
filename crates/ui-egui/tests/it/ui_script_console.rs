//! Window ▸ Script Console: the panel shows, runs JavaScript through `script.run` (persistent
//! context) and logs output, results and errors (egui_kittest, UI logic only).

use crate::gpu_test::RenderSerial;
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::dock::PanelKind;
use egui_kittest::Harness;

fn click(h: &mut Harness<'_, EffectcraftApp>, id: &str) {
    let e = h.state().auto.find(id).unwrap_or_else(|| panic!("no {id}")).clone();
    let c = egui::pos2(e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0);
    h.input_mut().events.push(egui::Event::PointerMoved(c));
    for pressed in [true, false] {
        h.input_mut().events.push(egui::Event::PointerButton { pos: c, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() });
    }
    h.run_steps(2);
}

#[test]
fn script_console_runs_scripts() {
    let s = effectcraft_host::session();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.state_mut().show_panel(PanelKind::ScriptConsole);
    h.run_steps(3);
    for id in ["scriptConsole.input", "scriptConsole.output", "scriptConsole.run", "scriptConsole.clear"] {
        assert!(h.state().auto.find(id).is_some(), "missing {id}");
    }
    h.state_mut().ui.script_console.input = "var c = app.project.items.addComp('Console', 320, 180, 1, 2, 24); writeLn('made'); c.name".into();
    h.run_steps(1);
    click(&mut h, "scriptConsole.run");
    assert!(h.state().session.project.items.values().any(|i| i.name == "Console"));
    // Variables persist between runs; errors are logged with their line.
    h.state_mut().ui.script_console.input = "c.numLayers + 5".into();
    click(&mut h, "scriptConsole.run");
    h.state_mut().ui.script_console.input = "nope()".into();
    click(&mut h, "scriptConsole.run");
    let log: Vec<(String, String)> = h.state().ui.script_console.log.iter().map(|l| (l.kind.clone(), l.text.clone())).collect();
    assert!(log.contains(&("output".into(), "made".into())), "{log:?}");
    assert!(log.contains(&("result".into(), "\"Console\"".into())), "{log:?}");
    assert!(log.contains(&("result".into(), "5".into())), "{log:?}");
    assert!(log.iter().any(|(k, t)| k == "error" && t.contains("line 1") && t.contains("nope")), "{log:?}");
    assert!(h.state_mut().session.undo(), "console edits are undoable");
    click(&mut h, "scriptConsole.clear");
    assert!(h.state().ui.script_console.log.is_empty());
}

/// Headless look at the panel (wgpu offscreen); run with
/// `SC_SNAPSHOT=/abs/out.png cargo test -p effectcraft-ui-egui --test it ui_script_console:: -- --ignored`.
#[test]
#[ignore]
fn script_console_snapshot() {
    let s = effectcraft_host::session();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.state_mut().show_panel(PanelKind::ScriptConsole);
    h.run_steps(2);
    for code in ["var c = app.project.items.addComp('Console', 320, 180, 1, 2, 24); writeLn('made ' + c.name); c.id", "c.layers.addText('Hi').name", "nope()"] {
        h.state_mut().ui.script_console.input = code.into();
        h.run_steps(1);
        click(&mut h, "scriptConsole.run");
    }
    h.state_mut().ui.script_console.input = "app.project.numItems".into();
    h.run_steps(3);
    let img = h.render_serialized().expect("render");
    let out = std::env::var("SC_SNAPSHOT").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/test-out/script_console.png").into());
    if let Some(d) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(d).unwrap();
    }
    img.save(&out).unwrap();
}
