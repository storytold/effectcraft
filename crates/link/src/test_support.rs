//! A FilmCraft stand-in for tests: a real `craft_ipc` server on a temporary runtime directory.
//! It records every `link.focus` and `app.activate` it receives, in arrival order.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};

use craft_ipc::server::{Activate, Activation, ControlSink, FrameOpen, Frames, Server, ServerConfig};
use craft_ipc::{LinkError, RuntimeDir};
use serde_json::{Map, Value, json};

use crate::activation::{ActivationBackend, FocusResult};

/// Removes the directory when dropped.
pub struct TempRuntime {
    pub dir: RuntimeDir,
    path: PathBuf,
}

impl TempRuntime {
    pub fn new(name: &str) -> Self {
        // A unix socket path holds at most 100 bytes, so keep this short.
        let tick = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let path = PathBuf::from(format!("/tmp/ec-{name}-{}-{:x}", std::process::id(), tick & 0xffff_ffff));
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

pub type CallLog = Arc<Mutex<Vec<(String, Value)>>>;

/// What the stand-in answers.
pub struct FakeReplies {
    /// Reply object for `link.focus`.
    pub focus: Value,
    /// Answer for `app.activate`.
    pub activate: Activation,
}

impl Default for FakeReplies {
    fn default() -> Self {
        Self { focus: json!({}), activate: Activation::Requested }
    }
}

pub struct FakeFilmCraft {
    pub log: CallLog,
    _server: Server,
}

impl FakeFilmCraft {
    pub fn start(runtime: &RuntimeDir, replies: FakeReplies) -> Self {
        let log: CallLog = Arc::new(Mutex::new(Vec::new()));
        let config = ServerConfig {
            app: "filmcraft".to_string(),
            app_version: "0.0.0-test".to_string(),
            instance: None,
            runtime: Some(runtime.clone()),
            tcp_port: None,
            sink: Arc::new(Sink { log: Arc::clone(&log), focus: replies.focus }),
            activate: Arc::new(Act { log: Arc::clone(&log), answer: replies.activate }),
            frames: Arc::new(NoFrames),
        };
        let server = Server::start(config).expect("fake filmcraft server");
        Self { log, _server: server }
    }

    pub fn calls(&self) -> Vec<(String, Value)> {
        self.log.lock().expect("log").clone()
    }

    pub fn methods(&self) -> Vec<String> {
        self.calls().into_iter().map(|(method, _)| method).collect()
    }
}

struct Sink {
    log: CallLog,
    focus: Value,
}

impl ControlSink for Sink {
    fn submit(&self, method: String, params: Value) -> Option<Receiver<Value>> {
        self.log.lock().ok()?.push((method.clone(), params));
        let (tx, rx) = channel();
        let reply = if method == "link.focus" { self.focus.clone() } else { json!({"ok": false, "error": "unexpected method"}) };
        tx.send(reply).ok()?;
        Some(rx)
    }
}

struct Act {
    log: CallLog,
    answer: Activation,
}

impl Activate for Act {
    fn activate(&self, token: Option<String>, from_pid: Option<u32>, open: Option<Value>) -> Activation {
        if let Ok(mut log) = self.log.lock() {
            log.push(("app.activate".to_string(), json!({"token": token, "from_pid": from_pid, "open": open})));
        }
        self.answer
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

/// An [`ActivationBackend`] that mints a fixed token and records every call.
#[derive(Default)]
pub struct FakeBackend {
    pub minted: Option<String>,
    pub mint_calls: std::sync::atomic::AtomicUsize,
    pub calls: Mutex<Vec<String>>,
    /// Models stock winit: a token can be minted but not applied to a live window.
    pub cannot_apply: bool,
}

impl FakeBackend {
    fn log(&self, text: impl Into<String>) {
        if let Ok(mut calls) = self.calls.lock() {
            calls.push(text.into());
        }
    }

    pub fn recorded(&self) -> Vec<String> {
        self.calls.lock().expect("calls").clone()
    }
}

impl ActivationBackend for FakeBackend {
    fn mint_token(&self) -> Option<String> {
        self.mint_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.minted.clone()
    }
    fn can_apply_token(&self) -> bool {
        !self.cannot_apply
    }
    fn set_pending_token(&self, token: &str) {
        self.log(format!("pending:{token}"));
    }
    fn focus(&self) -> FocusResult {
        self.log("focus");
        FocusResult::Pending
    }
    fn request_user_attention(&self) {
        self.log("attention");
    }
    fn yield_activation_to(&self, _bundle_id: &str) -> bool {
        true
    }
    fn activate_application(&self) -> bool {
        true
    }
}
