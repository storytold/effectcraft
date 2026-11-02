//! The control channel's `render.frame` and `ui.screenshot` write a PNG to the `path` they are
//! given, and never over a file the project reads (#1743): footage, a proxy or the project file.

use std::path::PathBuf;

use effectcraft_engine::Session;
use effectcraft_engine::color::Label;
use effectcraft_engine::project::{Footage, FootageKind, ItemKind};
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::control::{self, ControlRequest, Outcome};
use egui_kittest::Harness;
use serde_json::{Value, json};

const SOURCE: &[u8] = b"SOURCE FOOTAGE";

struct Rig {
    h: Harness<'static, EffectcraftApp>,
    dir: PathBuf,
}

impl Rig {
    /// A project saved as `Project.ecproj` in a fresh folder, reading `clip.png` beside it.
    fn new(name: &str) -> Rig {
        let dir = std::env::temp_dir().join("effectcraft-ui-export-overwrite").join(format!("{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("clip.png"), SOURCE).unwrap();
        let mut s = Session::default();
        s.execute("comp.new", json!({"name": "Main", "width": 16, "height": 16, "frameRate": 10, "duration": 0.5})).unwrap();
        s.execute("file.saveAs", json!({"path": dir.join("Project.ecproj").to_string_lossy()})).unwrap();
        let clip = Footage {
            path: dir.join("clip.png").to_string_lossy().into_owned(),
            kind: FootageKind::Still,
            width: 16,
            height: 16,
            has_video: true,
            ..Default::default()
        };
        std::sync::Arc::make_mut(&mut s.project).add_item("clip.png", Label::Aqua, None, ItemKind::Footage(clip));
        let mut app = Some(EffectcraftApp::new(s));
        let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| app.take().expect("app"));
        h.run_steps(2);
        Rig { h, dir }
    }

    fn text(&self, file: &str) -> String {
        self.dir.join(file).to_string_lossy().into_owned()
    }

    fn read(&self, file: &str) -> Vec<u8> {
        std::fs::read(self.dir.join(file)).unwrap()
    }

    /// A control-channel request, answered at once or not.
    fn send(&mut self, method: &str, params: Value) -> Outcome {
        let ctx = self.h.ctx.clone();
        let (req, _rx) = ControlRequest::new(method, params);
        let now = self.h.state().now();
        control::handle(self.h.state_mut(), &ctx, &req, now)
    }

    /// The reply of a request that is answered at once.
    fn reply(&mut self, method: &str, params: Value) -> Value {
        match self.send(method, params) {
            Outcome::Done(v) => v,
            _ => panic!("{method}: no immediate reply"),
        }
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn render_frame_never_writes_over_a_file_the_project_reads() {
    let mut r = Rig::new("render-frame");
    let project = r.read("Project.ecproj");
    for (file, what) in [("clip.png", "footage"), ("Project.ecproj", "project")] {
        let v = r.reply("render.frame", json!({"path": r.text(file)}));
        let error = v["error"].as_str().unwrap_or_default();
        assert!(v["ok"] == false && error.contains(file) && error.contains(what), "{file}: {v}");
    }
    assert_eq!(r.read("clip.png"), SOURCE, "the footage is as it was");
    assert_eq!(r.read("Project.ecproj"), project, "the project file is as it was");
    // Anywhere else it writes the PNG.
    let v = r.reply("render.frame", json!({"path": r.text("frame.png")}));
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(&r.read("frame.png")[..8], b"\x89PNG\r\n\x1a\n");
    let v = r.reply("render.frame", json!({"path": r.text("inline.png"), "base64": true, "write": true}));
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(v["result"]["png"].as_str(), Some(control::base64(&r.read("inline.png")).as_str()));
}

#[test]
fn screenshot_never_writes_over_a_file_the_project_reads() {
    let mut r = Rig::new("screenshot");
    let project = r.read("Project.ecproj");
    for (file, what) in [("clip.png", "footage"), ("Project.ecproj", "project")] {
        let Outcome::Done(v) = r.send("ui.screenshot", json!({"path": r.text(file)})) else { panic!("{file}: the screenshot was taken for it") };
        let error = v["error"].as_str().unwrap_or_default();
        assert!(v["ok"] == false && error.contains(file) && error.contains(what), "{file}: {v}");
    }
    assert_eq!(r.read("clip.png"), SOURCE, "the footage is as it was");
    assert_eq!(r.read("Project.ecproj"), project, "the project file is as it was");
    // Anywhere else it is taken (the frame that carries it comes later).
    assert!(matches!(r.send("ui.screenshot", json!({"path": r.text("shot.png")})), Outcome::Screenshot { .. }));
}
