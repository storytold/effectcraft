//! Effects & Presets: copy the configured folder path through the contents menu.

use crate::gpu_test::RenderSerial;
use effectcraft_engine::Session;
use effectcraft_ui_egui::panels::effects_presets::PresetFile;
use effectcraft_ui_egui::{ControlRequest, EffectcraftApp, control, dock::PanelKind};
use egui::{Event, Modifiers, OutputCommand, PointerButton, pos2};
use egui_kittest::Harness;
use serde_json::json;

fn expected_folder() -> String {
    let path = std::env::var("EC_PRESETS_DIR").unwrap_or_else(|_| {
        let home = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).unwrap();
        std::path::Path::new(&home).join("Documents/EffectCraft/Presets").to_str().unwrap().to_string()
    });
    if path.is_empty() { path } else { std::path::absolute(path).unwrap().to_str().unwrap().to_string() }
}

fn click(h: &mut Harness<'_, EffectcraftApp>, id: &str) -> Vec<String> {
    let e = h.state().auto.find(id).unwrap_or_else(|| panic!("missing {id}"));
    let pos = pos2(e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0);
    let mut copied = Vec::new();
    h.input_mut().events.push(Event::PointerMoved(pos));
    h.step();
    for pressed in [true, false] {
        h.input_mut().events.push(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        h.step();
        for command in &h.output().platform_output.commands {
            if let OutputCommand::CopyText(text) = command {
                copied.push(text.clone());
            }
        }
    }
    copied
}

#[test]
fn presets_menu_copies_the_configured_folder_without_editing_the_project() {
    let expected = expected_folder();
    for language in ["en", "ja"] {
        // The `ja` run draws Japanese menu text, which needs an installed CJK face.
        let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).allow_missing_glyphs().build_eframe(|_| {
            let mut app = EffectcraftApp::new(Session::default());
            app.session.prefs.general.language = language.into();
            app.ui.start_screen = false;
            app.show_panel(PanelKind::EffectsPresets);
            app
        });
        h.run_steps(3);
        let before = serde_json::to_value(&h.state().session.project).unwrap();
        let revision = h.state().session.revision;
        assert!(h.state().auto.find("effects.copyPresetsFolderPath").is_none());
        click(&mut h, "effects.menu");
        h.run_steps(3);
        if let Ok(path) = std::env::var("PRESETS_FOLDER_SNAPSHOT") {
            h.render_serialized().unwrap().save(path).unwrap();
        }
        assert_eq!(click(&mut h, "effects.copyPresetsFolderPath"), [expected.as_str()]);
        h.run_steps(2);
        assert!(h.state().auto.find("effects.copyPresetsFolderPath").is_none());
        assert_eq!(serde_json::to_value(&h.state().session.project).unwrap(), before);
        assert_eq!(h.state().session.revision, revision);
    }
}

#[test]
fn presets_folder_control_command_returns_the_path_or_a_configuration_error() {
    let mut app = EffectcraftApp::new(Session::default());
    let ctx = egui::Context::default();
    let expected = expected_folder();
    let mut reply = None;
    let mut output = ctx.run_ui(Default::default(), |ui| {
        let (request, _rx) = ControlRequest::new("ui.menu.invoke", json!({"id": "effects.copyPresetsFolderPath"}));
        let now = app.now();
        reply = match control::handle(&mut app, ui.ctx(), &request, now) {
            control::Outcome::Done(value) => Some(value),
            _ => panic!("expected an immediate reply"),
        };
    });
    output.textures_delta.clear();
    let copied: Vec<_> = output
        .platform_output
        .commands
        .iter()
        .filter_map(|command| match command {
            OutputCommand::CopyText(text) => Some(text.as_str()),
            _ => None,
        })
        .collect();
    if expected.is_empty() {
        assert_eq!(reply, Some(json!({"ok": false, "error": "No presets folder is configured"})));
        assert!(copied.is_empty());
        assert_eq!(app.ui.status, "No presets folder is configured");
    } else {
        assert_eq!(reply, Some(json!({"ok": true, "result": {"path": expected}})));
        assert_eq!(copied, [expected.as_str()]);
    }
}

#[test]
fn shared_preset_rows_search_apply_and_undo_the_selected_definition() {
    struct Files(std::path::PathBuf);
    impl Drop for Files {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let files = Files(std::env::temp_dir().join(format!(
        "ec-preset-rows-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    )));
    std::fs::create_dir(&files.0).unwrap();
    let mut entries = Vec::new();
    for (name, value) in [("Custom Cache A", 35.0), ("Custom Cache B", 70.0)] {
        let path = files.0.join(format!("{name}.ecpreset"));
        let preset = json!({"effectcraftAnimationPreset": 1, "name": name, "properties": [{"path": "transform/opacity", "value": {"t": "Scalar", "v": value}}], "effects": []});
        std::fs::write(&path, serde_json::to_vec(&preset).unwrap()).unwrap();
        entries.push(PresetFile { name: name.into(), path: path.to_string_lossy().into_owned() });
    }
    let mut session = Session::default();
    session.prefs.general.language = "en".into();
    session.execute("comp.new", json!({"width": 64, "height": 36, "duration": 2})).unwrap();
    session.execute("layer.newText", json!({"text": "Cache"})).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).with_step_dt(1.0 / 60.0).build_eframe(|_| {
        let mut app = EffectcraftApp::new(session);
        app.ui.start_screen = false;
        app.show_panel(PanelKind::EffectsPresets);
        app
    });
    h.run_steps(3);
    let shared = std::sync::Arc::new(entries);
    let now = h.ctx.input(|i| i.time);
    h.ctx.data_mut(|d| d.insert_temp(egui::Id::unique("fx-user-presets"), (now, shared.clone())));
    let search = |h: &mut Harness<'_, EffectcraftApp>, text: &str| {
        click(h, "effects.search");
        h.input_mut().events.push(Event::Key { key: egui::Key::A, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::COMMAND });
        h.input_mut().events.push(Event::Text(text.into()));
        h.run_steps(40);
    };
    search(&mut h, "Typewriter");
    assert!(h.state().auto.find("effects.preset.text.typewriter").is_some());
    assert!(h.state().auto.find("effects.preset.file.Custom Cache A").is_none());
    click(&mut h, "effects.preset.text.typewriter");
    click(&mut h, "effects.preset.text.typewriter");
    h.run_steps(3);
    assert_eq!(h.state().session.active_comp().unwrap().layers[0].props.group("text/animators").unwrap().groups().count(), 1);
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    for (name, expected) in [("Custom Cache A", 35.0), ("Custom Cache B", 70.0)] {
        search(&mut h, name);
        let id = format!("effects.preset.file.{name}");
        let history = h.state().session.history.undo.len();
        assert_eq!(h.state().session.active_comp().unwrap().layers[0].props.prop("transform/opacity").unwrap().value.as_f64(), 100.0);
        click(&mut h, &id);
        click(&mut h, &id);
        h.run_steps(3);
        assert_eq!(h.state().session.active_comp().unwrap().layers[0].props.prop("transform/opacity").unwrap().value.as_f64(), expected);
        assert_eq!(h.state().session.history.undo.len(), history + 1);
        if let Ok(dir) = std::env::var("PRESET_CACHE_SNAPSHOTS") {
            h.render_serialized().unwrap().save(format!("{dir}/{name}.png")).unwrap();
        }
        h.state_mut().session.execute("edit.undo", json!({})).unwrap();
        h.run_steps(2);
        assert_eq!(h.state().session.active_comp().unwrap().layers[0].props.prop("transform/opacity").unwrap().value.as_f64(), 100.0);
    }
    let (_, cached) = h.ctx.data(|d| d.get_temp::<(f64, std::sync::Arc<Vec<PresetFile>>)>(egui::Id::unique("fx-user-presets"))).unwrap();
    assert!(std::sync::Arc::ptr_eq(&shared, &cached));
    assert_eq!(shared.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["Custom Cache A", "Custom Cache B"]);
}
