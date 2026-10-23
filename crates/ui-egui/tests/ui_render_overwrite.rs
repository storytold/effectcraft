//! Rendering over files that already exist asks first (#1743): the Render button lists what is
//! there and offers Replace or Cancel (Escape cancels), and anything that starts a render through
//! the UI asks the same way, while an agent running the command is refused until it passes
//! `overwrite: true`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use effectcraft_engine::project::render_queue::{OutputFormat, RenderStatus};
use effectcraft_engine::{ExportJob, ExportResult, Exporter, Session};
use effectcraft_ui_egui::control::{self, ControlRequest, Outcome};
use effectcraft_ui_egui::dock::PanelKind;
use effectcraft_ui_egui::{Dialog, EffectcraftApp};
use egui::{Event, Key, Modifiers, pos2};
use egui_kittest::Harness;
use serde_json::{Value, json};

const EARLIER: &[u8] = b"AN EARLIER RENDER";
const RENDERED: &[u8] = b"RENDERED";

/// Writes `RENDERED` at the job's path, like the real exporter writes its file.
#[derive(Default)]
struct Writer {
    jobs: Mutex<Vec<String>>,
}

impl Exporter for Writer {
    fn formats(&self) -> Vec<OutputFormat> {
        OutputFormat::ALL.to_vec()
    }
    fn export(&self, job: &ExportJob, _: &mut dyn FnMut(u64, u64) -> bool) -> Result<ExportResult, String> {
        self.jobs.lock().unwrap().push(job.path.to_string());
        std::fs::write(job.path, RENDERED).map_err(|e| e.to_string())?;
        Ok(ExportResult { path: job.path.to_string(), ..Default::default() })
    }
}

struct Rig {
    h: Harness<'static, EffectcraftApp>,
    writer: Arc<Writer>,
    dir: PathBuf,
}

impl Rig {
    /// A queue of one item rendering to `main.mp4` in a fresh folder; with `earlier`, a file is
    /// there already.
    fn new(name: &str, earlier: bool) -> Rig {
        let dir = std::env::temp_dir().join("effectcraft-ui-overwrite").join(format!("{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let writer = Arc::new(Writer::default());
        let mut s = Session { exporter: Some(writer.clone()), ..Default::default() };
        s.execute("comp.new", json!({"name": "Main", "width": 16, "height": 16, "frameRate": 10, "duration": 0.5})).unwrap();
        s.execute("renderQueue.add", json!({"output": dir.join("main.mp4").to_string_lossy()})).unwrap();
        if earlier {
            std::fs::write(dir.join("main.mp4"), EARLIER).unwrap();
        }
        let mut app = Some(EffectcraftApp::new(s));
        let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| app.take().expect("app"));
        h.state_mut().show_panel(PanelKind::RenderQueue);
        h.run_steps(4);
        Rig { h, writer, dir }
    }

    fn click(&mut self, id: &str) {
        // Let a dialog opened by the previous input finish its first (sizing) frame, so its
        // buttons are where the automation tree says.
        self.h.run_steps(2);
        let e = self.h.state().auto.find(id).unwrap_or_else(|| panic!("no {id}")).clone();
        let p = pos2(e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0);
        self.h.input_mut().events.push(Event::PointerMoved(p));
        self.h.input_mut().events.push(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
        self.h.step();
        self.h.input_mut().events.push(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
        self.h.run_steps(2);
    }

    fn key(&mut self, key: Key) {
        self.h.input_mut().events.push(Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
        self.h.input_mut().events.push(Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::NONE });
        self.h.run_steps(2);
    }

    /// Let a background render finish.
    fn finish(&mut self) {
        let t0 = std::time::Instant::now();
        while self.h.state().session.is_rendering() {
            assert!(t0.elapsed().as_secs() < 20, "the render did not finish");
            self.h.run_steps(1);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        self.h.run_steps(2);
    }

    fn file(&self) -> Vec<u8> {
        std::fs::read(self.dir.join("main.mp4")).unwrap()
    }

    fn status(&self) -> RenderStatus {
        self.h.state().session.project.render_queue[0].status.clone()
    }

    fn exported(&self) -> usize {
        self.writer.jobs.lock().unwrap().len()
    }

    /// A control-channel request, answered at once.
    fn send(&mut self, method: &str, params: Value) -> Value {
        let ctx = self.h.ctx.clone();
        let (req, _rx) = ControlRequest::new(method, params);
        match control::handle(self.h.state_mut(), &ctx, &req) {
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

/// With nothing in the way the Render button just renders.
#[test]
fn render_with_nothing_in_the_way_does_not_ask() {
    let mut r = Rig::new("free", false);
    r.click("renderQueue.render");
    assert_eq!(r.h.state().dialog, None);
    r.finish();
    assert_eq!(r.file(), RENDERED);
    assert_eq!(r.status(), RenderStatus::Done);
}

#[test]
fn render_over_an_existing_file_asks_and_cancel_keeps_it() {
    let mut r = Rig::new("cancel", true);
    for how in ["button", "escape"] {
        r.click("renderQueue.render");
        assert_eq!(r.h.state().dialog, Some(Dialog::Overwrite), "{how}");
        for id in ["dialog.overwrite.replace", "dialog.overwrite.cancel"] {
            assert!(r.h.state().auto.find(id).is_some(), "{id}");
        }
        assert_eq!(r.file(), EARLIER, "nothing is written while it asks");
        if how == "button" {
            r.click("dialog.overwrite.cancel");
        } else {
            r.key(Key::Escape);
        }
        assert_eq!(r.h.state().dialog, None, "{how}");
        assert_eq!(r.file(), EARLIER, "{how} keeps the file");
        assert_eq!(r.exported(), 0, "{how}");
        assert_eq!(r.status(), RenderStatus::Queued, "{how}: still queued for another try");
    }
}

#[test]
fn replace_renders_over_the_existing_file() {
    let mut r = Rig::new("replace", true);
    r.click("renderQueue.render");
    assert_eq!(r.h.state().dialog, Some(Dialog::Overwrite));
    r.click("dialog.overwrite.replace");
    assert_eq!(r.h.state().dialog, None);
    r.finish();
    assert_eq!(r.file(), RENDERED);
    assert_eq!(r.status(), RenderStatus::Done);
    assert_eq!(r.exported(), 1);
    // The answer was for that render only: rendering it again (the file is there again) asks again.
    r.h.state_mut().session.execute("renderQueue.setRender", json!({"index": 1, "render": false})).unwrap();
    r.h.state_mut().session.execute("renderQueue.setRender", json!({"index": 1, "render": true})).unwrap();
    r.h.run_steps(2);
    r.click("renderQueue.render");
    assert_eq!(r.h.state().dialog, Some(Dialog::Overwrite), "asks again");
}

/// Replace answers for the files the prompt listed. An agent can point the queue at another file while it is open
/// (`engine.execute` of `renderQueue.setOutput`); the answer must not carry over to that file.
#[test]
fn replace_does_not_cover_an_output_that_changed_while_the_prompt_was_open() {
    let mut r = Rig::new("changed", true);
    let other = r.dir.join("other.mp4");
    std::fs::write(&other, EARLIER).unwrap();
    r.click("renderQueue.render");
    assert_eq!(r.h.state().dialog, Some(Dialog::Overwrite));
    let v = r.send("engine.execute", json!({"command": "renderQueue.setOutput", "params": {"path": other.to_string_lossy()}}));
    assert_eq!(v["ok"], true, "{v}");
    r.click("dialog.overwrite.replace");
    // the answer was about main.mp4: nothing was replaced, and the prompt asks again, now about other.mp4
    assert_eq!(r.exported(), 0, "nothing was rendered");
    assert_eq!(std::fs::read(&other).unwrap(), EARLIER, "the new file was not replaced");
    assert_eq!(r.h.state().dialog, Some(Dialog::Overwrite), "it asks again");
    // answering for what it shows now replaces that file
    r.click("dialog.overwrite.replace");
    assert_eq!(r.h.state().dialog, None);
    r.finish();
    assert_eq!(std::fs::read(&other).unwrap(), RENDERED);
    assert_eq!(r.exported(), 1);
}

/// If what the prompt listed is gone by the time Replace is pressed, the render just runs.
#[test]
fn replace_with_nothing_left_to_replace_just_renders() {
    let mut r = Rig::new("gone", true);
    r.click("renderQueue.render");
    assert_eq!(r.h.state().dialog, Some(Dialog::Overwrite));
    std::fs::remove_file(r.dir.join("main.mp4")).unwrap();
    r.click("dialog.overwrite.replace");
    assert_eq!(r.h.state().dialog, None, "nothing to ask about");
    r.finish();
    assert_eq!(r.file(), RENDERED);
    assert_eq!(r.exported(), 1);
}

/// Anything that starts a render through the UI asks like the button; the command run as an
/// agent runs it (`engine.execute`) is refused instead, and replaces the file when told to.
#[test]
fn the_menu_asks_and_the_control_channel_refuses() {
    let mut r = Rig::new("channels", true);
    let v = r.send("engine.execute", json!({"command": "renderQueue.render", "params": {}}));
    assert_eq!(v["ok"], false, "{v}");
    let error = v["error"].as_str().unwrap_or_default();
    assert!(error.contains("already exists") && error.contains("overwrite"), "{v}");
    assert_eq!(r.h.state().dialog, None, "engine.execute never opens a dialog");
    assert_eq!(r.file(), EARLIER);

    let v = r.send("ui.menu.invoke", json!({"id": "renderQueue.render"}));
    assert_eq!(v["result"], json!({"dialog": "overwrite"}), "{v}");
    assert_eq!(r.h.state().dialog, Some(Dialog::Overwrite));
    assert_eq!(r.file(), EARLIER);
    r.key(Key::Escape);
    assert_eq!(r.h.state().dialog, None);

    let v = r.send("engine.execute", json!({"command": "renderQueue.render", "params": {"overwrite": true}}));
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(r.file(), RENDERED);
}

/// Footage is not something to ask about: the item fails with the reason and nothing is written.
#[test]
fn an_output_that_is_footage_fails_without_asking() {
    let mut r = Rig::new("footage", false);
    std::fs::write(r.dir.join("main.mp4"), b"SOURCE FOOTAGE").unwrap();
    let footage = effectcraft_engine::project::Footage {
        path: r.dir.join("main.mp4").to_string_lossy().into_owned(),
        kind: effectcraft_engine::project::FootageKind::Video,
        has_video: true,
        ..Default::default()
    };
    std::sync::Arc::make_mut(&mut r.h.state_mut().session.project).add_item(
        "main.mp4",
        Default::default(),
        None,
        effectcraft_engine::project::ItemKind::Footage(footage),
    );
    r.h.run_steps(2);
    r.click("renderQueue.render");
    assert_eq!(r.h.state().dialog, None, "no Replace for footage");
    assert_eq!(r.file(), b"SOURCE FOOTAGE");
    assert_eq!(r.exported(), 0);
    let RenderStatus::Failed(why) = r.status() else { panic!("{:?}", r.status()) };
    assert!(why.contains("footage"), "{why}");
    assert!(r.h.state().ui.status.contains("footage"), "the status bar says why: {}", r.h.state().ui.status);
}

/// Two queue items that write one file are refused as a whole: Replace could not help (the render
/// would replace its own output), so the prompt does not open, even with the file there already,
/// and the status bar names the two items.
#[test]
fn two_items_writing_one_file_are_refused_without_asking_to_replace() {
    for earlier in [false, true] {
        let mut r = Rig::new(&format!("clash-{earlier}"), earlier);
        let out = r.dir.join("main.mp4").to_string_lossy().into_owned();
        r.h.state_mut().session.execute("renderQueue.add", json!({"output": out})).unwrap();
        r.h.run_steps(2);
        r.click("renderQueue.render");
        assert_eq!(r.h.state().dialog, None, "earlier: {earlier}: no Replace for a clash");
        assert_eq!(r.exported(), 0, "earlier: {earlier}");
        assert_eq!(r.status(), RenderStatus::Queued, "earlier: {earlier}");
        assert_eq!(r.h.state().session.project.render_queue.len(), 2);
        assert_eq!(std::fs::read(r.dir.join("main.mp4")).ok(), earlier.then(|| EARLIER.to_vec()), "earlier: {earlier}: nothing was written");
        let status = r.h.state().ui.status.clone();
        assert!(status.contains("#1") && status.contains("#2") && status.contains("main.mp4"), "the status bar says why: {status}");
        // The menu path reports the same refusal instead of opening a dialog.
        let v = r.send("ui.menu.invoke", json!({"id": "renderQueue.render"}));
        assert_eq!(v["ok"], false, "{v}");
        assert!(v["error"].as_str().unwrap_or_default().contains("main.mp4"), "{v}");
        assert_eq!(r.h.state().dialog, None);
    }
}
