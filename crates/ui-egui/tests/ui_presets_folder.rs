//! Effects & Presets: copy the configured folder path through the contents menu.

use effectcraft_engine::Session;
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
    for pressed in [true, false] {
        h.input_mut().events.push(Event::PointerMoved(pos));
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
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| {
        let mut app = EffectcraftApp::new(Session::default());
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
        h.render().unwrap().save(path).unwrap();
    }
    assert_eq!(click(&mut h, "effects.copyPresetsFolderPath"), [expected]);
    h.run_steps(2);
    assert!(h.state().auto.find("effects.copyPresetsFolderPath").is_none());
    assert_eq!(serde_json::to_value(&h.state().session.project).unwrap(), before);
    assert_eq!(h.state().session.revision, revision);
}

#[test]
fn presets_folder_control_command_returns_the_path_or_a_configuration_error() {
    let mut app = EffectcraftApp::new(Session::default());
    let ctx = egui::Context::default();
    let expected = expected_folder();
    let mut reply = None;
    let mut output = ctx.run_ui(Default::default(), |ui| {
        let (request, _rx) = ControlRequest::new("ui.menu.invoke", json!({"id": "effects.copyPresetsFolderPath"}));
        reply = match control::handle(&mut app, ui.ctx(), &request) {
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
