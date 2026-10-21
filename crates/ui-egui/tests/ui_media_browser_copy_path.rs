use std::sync::Arc;

use effectcraft_engine::{
    Session,
    media_browser::{Browser, Entry, VirtualTree},
};
use effectcraft_ui_egui::{ControlRequest, EffectcraftApp, control, dock::PanelKind};
use egui::{Event, Modifiers, OutputCommand, PointerButton, pos2};
use egui_kittest::Harness;
use serde_json::{Value, json};

struct TestBrowser(VirtualTree);
impl Browser for TestBrowser {
    fn home(&self) -> String {
        "/Media".into()
    }
    fn list(&self, dir: &str, only: bool) -> Result<Vec<Entry>, String> {
        self.0.list(dir, only)
    }
}

fn click(h: &mut Harness<'_, EffectcraftApp>, id: &str, button: PointerButton) -> Vec<String> {
    let e = h.state().auto.find(id).unwrap_or_else(|| panic!("missing {id}"));
    let pos = pos2(e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0);
    h.input_mut().events.push(Event::PointerMoved(pos));
    h.step();
    let mut copied = Vec::new();
    for pressed in [true, false] {
        h.input_mut().events.push(Event::PointerButton { pos, button, pressed, modifiers: Modifiers::NONE });
        h.step();
        copied.extend(h.output().platform_output.commands.iter().filter_map(|c| match c {
            OutputCommand::CopyText(text) => Some(text.clone()),
            _ => None,
        }));
    }
    copied
}

#[test]
fn file_folder_and_virtual_paths_copy_from_the_context_menu_without_document_changes() {
    let cases = [
        ("作品 clip.csv", "/Users/example/Shoot 01/作品 clip.csv"),
        ("Notes.txt", "./relative notes.txt"),
        ("Virtual.csv", "browser://assets/opaque%20作品.csv?version=2"),
        ("Folder 作品", "/Media/Folder 作品"),
    ];
    let mut tree = VirtualTree::default();
    for (name, path) in cases.iter().take(3) {
        tree.add_file(&format!("/Media/{name}"), path, 1, None);
    }
    tree.add_dir("/Media/Folder 作品");
    let mut session = Session { browser: Some(Arc::new(TestBrowser(tree))), ..Default::default() };
    session.prefs.general.language = "en".into();
    session.execute("comp.new", json!({"width": 64, "height": 36, "duration": 2})).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).with_step_dt(1.0 / 60.0).build_eframe(|_| {
        let mut app = EffectcraftApp::new(session);
        app.ui.start_screen = false;
        app.show_panel(PanelKind::MediaBrowser);
        app.ui.maximized = Some(PanelKind::MediaBrowser);
        app
    });
    h.run_steps(3);
    let project = h.state().session.project.clone();
    let revision = h.state().session.revision;
    let history = h.state().session.history.undo.len();
    let browser = serde_json::to_value(&h.state().session.state.media_browser).unwrap();
    for (name, path) in cases {
        let id = format!("mediaBrowser.entry.{name}.copySourcePath");
        assert!(h.state().auto.find(&id).is_none());
        assert!(click(&mut h, &format!("mediaBrowser.entry.{name}"), PointerButton::Secondary).is_empty());
        h.run_steps(3);
        assert!(h.state().auto.find(&id).is_some());
        if name == "作品 clip.csv"
            && let Ok(file) = std::env::var("MEDIA_BROWSER_COPY_SNAPSHOT")
        {
            h.render().unwrap().save(file).unwrap();
        }
        assert_eq!(click(&mut h, &id, PointerButton::Primary), [path]);
        h.run_steps(3);
        assert!(h.state().auto.find(&id).is_none());
        assert_eq!(h.state().session.project, project);
        assert_eq!(h.state().session.revision, revision);
        assert_eq!(h.state().session.history.undo.len(), history);
        assert_eq!(serde_json::to_value(&h.state().session.state.media_browser).unwrap(), browser);
    }
}

#[test]
fn copy_path_control_command_preserves_the_path_and_rejects_bad_arguments() {
    let mut app = EffectcraftApp::new(Session::default());
    let ctx = egui::Context::default();
    let project = app.session.project.clone();
    let revision = app.session.revision;
    let history = app.session.history.undo.len();
    for params in [json!({"path": "  ./作品 clip.csv  "}), json!({}), json!({"path": ""}), json!({"path": 42}), json!({"path": null})] {
        let mut reply = None;
        let mut out = ctx.run_ui(Default::default(), |ui| {
            let (request, _rx) = ControlRequest::new("ui.menu.invoke", json!({"id": "mediaBrowser.copySourcePath", "params": params}));
            reply = match control::handle(&mut app, ui.ctx(), &request) {
                control::Outcome::Done(value) => Some(value),
                _ => panic!("expected immediate reply"),
            };
        });
        out.textures_delta.clear();
        let copied: Vec<_> = out
            .platform_output
            .commands
            .iter()
            .filter_map(|c| match c {
                OutputCommand::CopyText(text) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        if params.get("path").and_then(Value::as_str).is_some_and(|s| !s.is_empty()) {
            assert_eq!(reply, Some(json!({"ok": true, "result": {"path": "  ./作品 clip.csv  "}})));
            assert_eq!(copied, ["  ./作品 clip.csv  "]);
        } else {
            assert_eq!(reply, Some(json!({"ok": false, "error": "A source path is required"})));
            assert!(copied.is_empty());
        }
        assert_eq!(app.session.project, project);
        assert_eq!(app.session.revision, revision);
        assert_eq!(app.session.history.undo.len(), history);
    }
}
