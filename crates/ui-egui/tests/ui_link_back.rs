//! The `Back to FilmCraft` header button. A click must run `link.back` and the screen must say what
//! happened. A refusal that leaves the screen unchanged looks like a dead button.

use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};

use craft_ipc::server::{Activate, Activation, ControlSink, FrameOpen, Frames, Server, ServerConfig};
use craft_ipc::{LinkError, RuntimeDir};
use effectcraft_engine::Session;
use effectcraft_project::CraftLinkBinding;
use effectcraft_ui_egui::EffectcraftApp;
use egui::{Event, Pos2, pos2};
use egui_kittest::Harness;
use serde_json::{Map, Value, json};

const LINK: &str = "44444444-4444-4444-8444-444444444444";

struct Temp(std::path::PathBuf, RuntimeDir);

impl Temp {
    fn new(name: &str) -> Self {
        let path = std::path::PathBuf::from(format!("/tmp/ec-uib-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        let dir = RuntimeDir::at(&path).expect("runtime dir");
        Self(path, dir)
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

type Log = Arc<Mutex<Vec<String>>>;

struct Sink(Log, Value);

impl ControlSink for Sink {
    fn submit(&self, method: String, _params: Value) -> Option<Receiver<Value>> {
        self.0.lock().ok()?.push(method);
        let (tx, rx) = channel();
        tx.send(self.1.clone()).ok()?;
        Some(rx)
    }
}

struct Act(Log, Activation);

impl Activate for Act {
    fn activate(&self, _token: Option<String>, _pid: Option<u32>, _open: Option<Value>) -> Activation {
        if let Ok(mut log) = self.0.lock() {
            log.push("app.activate".into());
        }
        self.1
    }
}

struct NoFrames;

impl Frames for NoFrames {
    fn open(&self, _p: &Map<String, Value>) -> Result<FrameOpen, LinkError> {
        Err(LinkError::NotReady("none".into()))
    }
    fn render(&self, _t: &str, _h: &Value) -> Result<(Value, Vec<u8>), LinkError> {
        Err(LinkError::NotReady("none".into()))
    }
    fn close_ticket(&self, _t: &str) {}
}

fn peer(runtime: &RuntimeDir, focus: Value, answer: Activation) -> (Server, Log) {
    let log: Log = Arc::new(Mutex::new(Vec::new()));
    let config = ServerConfig {
        app: "filmcraft".into(),
        app_version: "0.0.0-test".into(),
        instance: None,
        runtime: Some(runtime.clone()),
        tcp_port: None,
        sink: Arc::new(Sink(log.clone(), focus)),
        activate: Arc::new(Act(log.clone(), answer)),
        frames: Arc::new(NoFrames),
    };
    (Server::start(config).expect("fake filmcraft"), log)
}

/// A result path the link accepts: `<link_id>.mov` in a directory that holds the `.ecproj` sidecar.
fn result_target(tag: &str) -> String {
    let dir = std::env::temp_dir().join(format!("ec-uib-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("result dir");
    std::fs::write(dir.join(format!("{LINK}.ecproj")), b"{}").expect("sidecar");
    dir.join(format!("{LINK}.mov")).to_string_lossy().into_owned()
}

/// A harness whose active comp is bound to a link, with Back installed against `runtime`.
fn harness(runtime: &RuntimeDir, result: &str) -> Harness<'static, EffectcraftApp> {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Linked", "width": 64, "height": 36, "duration": 1})).unwrap();
    s.execute("layer.newSolid", json!({"name": "Plate", "color": "#406080", "width": 64, "height": 36})).unwrap();
    let comp = s.active_comp_id().unwrap();
    s.project_mut().craft_links.insert(
        LINK.into(),
        CraftLinkBinding { link_id: LINK.into(), comp, source: json!({"app": "filmcraft", "doc_id": "0", "item": "1"}), result_path: result.into(), epoch: 0 },
    );
    effectcraft_link::install_back_command(&mut s, Some(runtime.clone()), Arc::new(effectcraft_link::EguiActivation::new(egui::Context::default()))).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(move |_| EffectcraftApp::new(s));
    h.run_steps(3);
    h
}

fn click(h: &mut Harness<'_, EffectcraftApp>, p: Pos2) {
    h.input_mut().events.push(Event::PointerMoved(p));
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
    h.run_steps(3);
}

fn press_back(h: &mut Harness<'_, EffectcraftApp>) -> String {
    let e = h.state().auto.find("header.link.back").expect("the Back button is drawn for a bound comp").rect;
    click(h, pos2(e[0] + e[2] / 2.0, e[1] + e[3] / 2.0));
    h.state().auto.find("toast").map(|t| t.label.clone()).unwrap_or_default()
}

#[test]
fn clicking_back_without_filmcraft_says_why() {
    let rt = Temp::new("none");
    let out = result_target("none");
    let mut h = harness(&rt.1, &out);
    let said = press_back(&mut h);
    assert!(said.contains("FilmCraft is not running"), "the click gave no visible reason: {said:?}");
}

#[test]
fn clicking_back_reaches_filmcraft() {
    let rt = Temp::new("peer");
    let (_server, log) = peer(&rt.1, json!({}), Activation::Yes);
    let out = result_target("peer");
    let mut h = harness(&rt.1, &out);
    let said = press_back(&mut h);
    assert_eq!(*log.lock().unwrap(), ["link.focus", "app.activate"], "toast: {said:?}");
    assert!(said.contains("Result sent to FilmCraft"), "{said:?}");
}

#[test]
fn clicking_back_when_filmcraft_refuses_says_why() {
    let rt = Temp::new("refuse");
    let focus = json!({"ok": false, "error": {"code": "link_unknown", "message": "no item carries that link", "data": {}}});
    let (_server, _log) = peer(&rt.1, focus, Activation::Yes);
    let out = result_target("refuse");
    let mut h = harness(&rt.1, &out);
    let said = press_back(&mut h);
    assert!(said.contains("no item carries that link"), "{said:?}");
}

#[test]
fn a_raise_the_compositor_refused_says_to_switch_windows() {
    let rt = Temp::new("noraise");
    let (_server, _log) = peer(&rt.1, json!({}), Activation::No);
    let out = result_target("noraise");
    let mut h = harness(&rt.1, &out);
    let said = press_back(&mut h);
    assert!(said.contains("Switch to its window"), "FilmCraft did not come forward and the screen did not say so: {said:?}");
}

#[test]
fn a_raise_nobody_confirmed_does_not_claim_the_window_came_forward() {
    let rt = Temp::new("requested");
    let (_server, _log) = peer(&rt.1, json!({}), Activation::Requested);
    let out = result_target("requested");
    let mut h = harness(&rt.1, &out);
    let said = press_back(&mut h);
    assert!(said.contains("Result sent to FilmCraft"), "{said:?}");
    assert!(said.contains("if it stays behind"), "a request is not a confirmed raise: {said:?}");
    assert!(!said.contains("did not come to the front"), "{said:?}");
}

/// The render blocks the UI thread. The screen has to say so before it blocks.
#[test]
fn the_click_is_acknowledged_before_the_render_runs() {
    let rt = Temp::new("ack");
    let out = result_target("ack");
    let mut h = harness(&rt.1, &out);
    let e = h.state().auto.find("header.link.back").unwrap().rect;
    let p = pos2(e[0] + e[2] / 2.0, e[1] + e[3] / 2.0);
    h.input_mut().events.push(Event::PointerMoved(p));
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
    h.step();
    assert!(h.state().ui.status.contains("Rendering the result"), "status after the click: {:?}", h.state().ui.status);
}
