//! Shared test rig: real `Session`, real `PluginHost`, real wiring, real wasm guests.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};

use craft_ipc::server::{Activate, Activation, ControlSink, FrameOpen, Frames, Server, ServerConfig};
use craft_ipc::{LinkError, RuntimeDir};
use craft_plugin_host::lifecycle::InstallOutcome;
use craft_plugin_host::store::Source;
use effectcraft_engine::Session;
use effectcraft_engine::color::Label;
use effectcraft_link::ActivationBackend;
use effectcraft_plugins::{HostConfig, PluginHost};
use effectcraft_project::{Comp, CraftLinkBinding, ItemId, ItemKind};
use effectcraft_time::FrameRate;
use serde_json::{Map, Value, json};

pub mod gain;
pub mod guest;

pub const EDIT: &str = "test.edit";
pub const PIXELS: &str = "test.pixels";
pub const RAW_RGB: &str = "test.raw-rgb";
pub const LINK: &str = "55555555-5555-4555-8555-555555555555";

fn leb(mut n: usize, out: &mut Vec<u8>) {
    loop {
        let byte = (n & 0x7F) as u8;
        n >>= 7;
        if n == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

/// A package: a core module from WAT text, plus a `craft.manifest` custom section.
pub fn package_from(wat_text: &str, manifest: &Value) -> Vec<u8> {
    let mut wasm = wat::parse_str(wat_text).unwrap_or_else(|e| panic!("bad WAT: {e}"));
    let name = "craft.manifest";
    let mut body = Vec::new();
    leb(name.len(), &mut body);
    body.extend_from_slice(name.as_bytes());
    body.extend_from_slice(&serde_json::to_vec(manifest).unwrap());
    wasm.push(0);
    leb(body.len(), &mut wasm);
    wasm.extend_from_slice(&body);
    wasm
}

/// The text of `tests/fixtures/<name>.wat`.
pub fn fixture_text(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(format!("{name}.wat"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Effects that run `pixels.wat`. It asks for no permission, so it installs without a prompt.
pub fn pixels_package() -> Vec<u8> {
    let manifest = json!({
        "schema": 1, "id": PIXELS, "name": PIXELS, "version": "1.0.0",
        "abi": { "craft": 1 },
        "limits": { "memory_mb": 4, "stack_kb": 128 },
        "contributions": [
            { "kind": "effect", "id": "quad", "entry": 3, "label": "Quad", "category": "Test Plug-ins", "process": { "sample": ["f32"], "alpha": "straight" } }
        ]
    });
    package_from(&fixture_text("pixels"), &manifest)
}

/// A raw RGB image reader (`raw-rgb.wat`). It asks for `fs.picked` and nothing else.
pub fn raw_rgb_package() -> Vec<u8> {
    let manifest = json!({
        "schema": 1, "id": RAW_RGB, "name": "Raw RGB image", "version": "1.0.0",
        "abi": { "craft": 1 },
        "permissions": { "required": ["fs.picked"] },
        "limits": { "memory_mb": 64 },
        "contributions": [{ "kind": "format", "id": "rgbraw", "entry": 0, "extensions": ["rgbraw"] }]
    });
    package_from(&fixture_text("raw-rgb"), &manifest)
}

/// A session with one comp open, and a plug-in host in memory wired the way the app wires it
/// (`PluginHost::start`).
pub fn rig() -> (Session, Arc<PluginHost>, ItemId) {
    rig_with_clock(None)
}

/// A clock that never moves, for the command layer's 2 ms gate rule and the plug-in host's deadlines.
/// A test on a busy machine then measures the code and not the load.
pub struct Frozen;

impl craft_command::Clock for Frozen {
    fn now_ms(&self) -> u64 {
        1_000
    }
}

impl craft_plugin_host::engine::Clock for Frozen {
    fn now(&self) -> std::time::Duration {
        std::time::Duration::from_secs(1)
    }
}

/// [`rig`] with both clocks frozen.
pub fn rig_frozen() -> (Session, Arc<PluginHost>, ItemId) {
    rig_with_clock(Some(Arc::new(Frozen)))
}

fn rig_with_clock(clock: Option<Arc<Frozen>>) -> (Session, Arc<PluginHost>, ItemId) {
    let mut session = Session::default();
    if let Some(clock) = &clock {
        session.clock = clock.clone();
    }
    let rate = FrameRate::new(24, 1);
    let mut comp = Comp::new(64, 36, rate, rate.tick_of(1));
    comp.background = [0.2, 0.2, 0.7];
    let id = session.project_mut().add_item("Link Comp", Label::Sandstone, None, ItemKind::Comp(Arc::new(comp)));
    session.open_comp(id);
    let host_clock: Option<Arc<dyn craft_plugin_host::engine::Clock>> = clock.map(|c| c as Arc<dyn craft_plugin_host::engine::Clock>);
    let host = PluginHost::start(&mut session, HostConfig { clock: host_clock, ..HostConfig::default() }).unwrap();
    (session, host, id)
}

/// Install `bytes` and accept every permission it asks for, the way the prompt's Install button does.
pub fn install(host: &PluginHost, bytes: &[u8]) -> InstallOutcome {
    match host.install_bytes(bytes, Source::File).unwrap() {
        InstallOutcome::NeedsGrant(req) => {
            let id = req.plugin_id.clone();
            host.answer(&id, true, req.optional).unwrap()
        }
        other => other,
    }
}

pub fn bind(session: &mut Session, comp: ItemId, result: &str) {
    // `link.open` writes this sidecar. The render accepts only a directory that holds it.
    if let Some(dir) = std::path::Path::new(result).parent() {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(format!("{LINK}.ecproj")), b"{}").unwrap();
    }
    session.project_mut().craft_links.insert(
        LINK.into(),
        CraftLinkBinding { link_id: LINK.into(), comp, source: json!({"app": "filmcraft", "doc_id": "0", "item": "1"}), result_path: result.into(), epoch: 0 },
    );
}

/// A runtime directory that is removed when dropped. A unix socket path holds at most 100 bytes.
pub struct TempRuntime {
    pub dir: RuntimeDir,
    path: PathBuf,
}

impl TempRuntime {
    pub fn new(name: &str) -> Self {
        let tick = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let path = PathBuf::from(format!("/tmp/ecp-{name}-{}-{:x}", std::process::id(), tick & 0xffff_ffff));
        let _ = std::fs::remove_dir_all(&path);
        let dir = RuntimeDir::at(&path).expect("runtime dir");
        Self { dir, path }
    }
}

impl Drop for TempRuntime {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// A FilmCraft stand-in: a real `craft_ipc` server that records `link.focus` and `app.activate`.
pub struct FakeFilmCraft {
    log: Arc<Mutex<Vec<String>>>,
    _server: Server,
}

impl FakeFilmCraft {
    pub fn start(runtime: &RuntimeDir) -> Self {
        let log = Arc::new(Mutex::new(Vec::new()));
        let config = ServerConfig {
            app: "filmcraft".to_string(),
            app_version: "0.0.0-test".to_string(),
            instance: None,
            runtime: Some(runtime.clone()),
            tcp_port: None,
            sink: Arc::new(Sink(Arc::clone(&log))),
            activate: Arc::new(Act(Arc::clone(&log))),
            frames: Arc::new(NoFrames),
        };
        Self { log, _server: Server::start(config).expect("fake filmcraft server") }
    }

    pub fn methods(&self) -> Vec<String> {
        self.log.lock().expect("log").clone()
    }
}

struct Sink(Arc<Mutex<Vec<String>>>);

impl ControlSink for Sink {
    fn submit(&self, method: String, _params: Value) -> Option<Receiver<Value>> {
        let reply = if method == "link.focus" { json!({}) } else { json!({"ok": false, "error": "unexpected method"}) };
        self.0.lock().ok()?.push(method);
        let (tx, rx) = channel();
        tx.send(reply).ok()?;
        Some(rx)
    }
}

struct Act(Arc<Mutex<Vec<String>>>);

impl Activate for Act {
    fn activate(&self, _token: Option<String>, _from_pid: Option<u32>, _open: Option<Value>) -> Activation {
        if let Ok(mut log) = self.0.lock() {
            log.push("app.activate".to_string());
        }
        Activation::Requested
    }
}

struct NoFrames;

impl Frames for NoFrames {
    fn open(&self, _params: &Map<String, Value>) -> Result<FrameOpen, LinkError> {
        Err(LinkError::NotReady("no frames in the test peer".into()))
    }
    fn render(&self, _ticket: &str, _header: &Value) -> Result<(Value, Vec<u8>), LinkError> {
        Err(LinkError::NotReady("no frames in the test peer".into()))
    }
    fn close_ticket(&self, _ticket: &str) {}
}

/// An activation backend that has no window to focus.
pub struct NoWindow;

impl ActivationBackend for NoWindow {
    fn mint_token(&self) -> Option<String> {
        None
    }
    fn set_pending_token(&self, _token: &str) {}
    fn focus(&self) -> effectcraft_link::activation::FocusResult {
        effectcraft_link::activation::FocusResult::Pending
    }
    fn request_user_attention(&self) {}
    fn yield_activation_to(&self, _bundle_id: &str) -> bool {
        true
    }
    fn activate_application(&self) -> bool {
        true
    }
}
