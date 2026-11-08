//! `window.effectcraft`: the page command API (`craft-web-api`, storytold/craft-libs#413) over the
//! desktop control channel (`docs/control-protocol.md`), for agents and browser tests.
//!
//! The crate owns `request`, `execute`, `commands`, `uiInspect` and `screenshot`, as promises with a
//! timeout (EffectCraft has no `document.inspect`, so no `docInspect`); [`install`] is EffectCraft's part: a `Request` that queues a
//! `ControlRequest` for the app, which answers it on its next frame, and waits for the reply.
//! EffectCraft's control request has no abandon state, so a call the page gave up on still runs when
//! the app reaches it, as it does for a desktop client that timed out. `web/index.html` adds
//! EffectCraft's own functions (`files`, `addFile`, `info`, `saveToBrowser`, …) to the same object.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use craft_web_api::methods::Methods;
use craft_web_api::page::{PageApi, Request};
use effectcraft_ui_egui::ControlRequest;
use serde_json::{Value, json};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

thread_local! {
    static INFO: RefCell<Value> = const { RefCell::new(Value::Null) };
}

/// How often a waiting call looks for the app's reply, in milliseconds (about a frame).
const POLL_MS: i32 = 16;

/// Put the crate's functions on `window.effectcraft` and return the queue the app drains each frame
/// (hand it to `with_control`). An existing `window.effectcraft` keeps the functions it already has.
pub(crate) fn install(ctx: egui::Context) -> Receiver<ControlRequest> {
    let (queue, requests) = channel::<ControlRequest>();
    let request: Request = Rc::new(move |method, params| {
        let (control, reply) = ControlRequest::new(method, params);
        if queue.send(control).is_err() {
            return Box::pin(async { json!({"ok": false, "error": "the app is not running"}) });
        }
        ctx.request_repaint();
        Box::pin(wait(reply))
    });
    if let Err(e) = Rc::new(PageApi::new("effectcraft", Methods { doc_inspect: None, ..Methods::default() }, request)).install() {
        log::warn!("couldn't install window.effectcraft: {e:?}");
    }
    requests
}

/// The app's reply, polled about once a frame.
async fn wait(reply: Receiver<Value>) -> Value {
    loop {
        match reply.try_recv() {
            Ok(reply) => return reply,
            Err(TryRecvError::Empty) => sleep(POLL_MS).await,
            Err(TryRecvError::Disconnected) => return json!({"ok": false, "error": "the app dropped the request"}),
        }
    }
}

async fn sleep(ms: i32) {
    let promise = js_sys::Promise::new(&mut |resolve, _| {
        if let Some(window) = web_sys::window() {
            let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms);
        }
    });
    let _ = JsFuture::from(promise).await;
}

/// Environment facts reported by `effectcraft.info()`.
pub fn set_info(key: &str, v: Value) {
    INFO.with(|i| {
        let mut i = i.borrow_mut();
        if !i.is_object() {
            *i = json!({});
        }
        i[key] = v;
    });
}

fn to_js(v: &Value) -> JsValue {
    js_sys::JSON::parse(&v.to_string()).unwrap_or(JsValue::NULL)
}

fn from_js(v: &JsValue) -> Value {
    if v.is_undefined() || v.is_null() {
        return json!({});
    }
    js_sys::JSON::stringify(v).ok().and_then(|s| s.as_string()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(json!({}))
}

/// Files in the in-memory file table: `[{path, size}]` (picked, dropped, saved and rendered files).
#[wasm_bindgen]
pub fn files() -> JsValue {
    to_js(&Value::Array(crate::files::list().into_iter().map(|(path, size)| json!({"path": path, "size": size})).collect()))
}

/// A file's bytes from the table (e.g. a render), or `undefined`.
#[wasm_bindgen(js_name = readFile)]
pub fn read_file(path: String) -> JsValue {
    crate::files::get(&path).map(|d| js_sys::Uint8Array::from(&d[..]).into()).unwrap_or(JsValue::UNDEFINED)
}

/// Put bytes into the file table under `/<name>` and open (`.ecproj`) or import them. Resolves
/// with the table path.
#[wasm_bindgen(js_name = addFile)]
pub fn add_file(name: String, data: js_sys::Uint8Array) -> String {
    let path = crate::files::path_for(&name);
    crate::files::put(&path, data.to_vec().into());
    let p = path.clone();
    crate::post(move |app| crate::open_or_import(app, vec![p]));
    path
}

/// Backend and build facts.
#[wasm_bindgen]
pub fn info() -> JsValue {
    let mut v = INFO.with(|i| i.borrow().clone());
    if !v.is_object() {
        v = json!({});
    }
    v["version"] = json!(env!("CARGO_PKG_VERSION"));
    v["threads"] = json!(false);
    v["workers"] = from_js(&crate::worker::stats());
    v["audio"] = from_js(&crate::audio::state());
    if let Some(e) = crate::persist::last_error() {
        v["storage"]["error"] = json!(e);
    }
    to_js(&v)
}

/// Save the project to browser storage without downloading it: `path` (default: its current
/// path, or `/<project name>.ecproj`). Resolves with `{path, bytes}`.
#[wasm_bindgen(js_name = saveToBrowser)]
pub fn save_to_browser(path: Option<String>) -> js_sys::Promise {
    js_sys::Promise::new(&mut |resolve, reject| {
        let path = path.clone();
        crate::post(move |app| {
            let p = path.or_else(|| app.session.path.clone()).unwrap_or_else(|| format!("/{}.ecproj", app.session.project_name()));
            let p = if p.starts_with('/') { p } else { format!("/{p}") };
            crate::files::save_quietly(&p);
            match app.session.execute("file.saveAs", json!({"path": p})) {
                Ok(v) => {
                    let _ = resolve.call1(&JsValue::NULL, &to_js(&v));
                }
                Err(e) => {
                    let _ = reject.call1(&JsValue::NULL, &JsValue::from_str(&e.to_string()));
                }
            }
        });
    })
}

/// What browser storage holds: `{backend, usage, quota, persisted, files: [{path, size,
/// modified}], config: [name]}` (render outputs, which are not stored, are left out).
#[wasm_bindgen(js_name = listStored)]
pub async fn list_stored() -> JsValue {
    let est = wasm_bindgen_futures::JsFuture::from(crate::persist::store_estimate()).await.map(|v| from_js(&v)).unwrap_or(json!({}));
    let files: Vec<Value> = crate::files::STORE
        .files()
        .into_iter()
        .filter(|f| f.3)
        .map(|(path, size, modified, _)| json!({"path": path, "size": size, "modified": modified}))
        .collect();
    let config: Vec<Value> =
        crate::files::STORE.lock().list(crate::store::CONFIG).into_iter().map(|(k, ..)| json!(k.trim_start_matches(crate::store::CONFIG))).collect();
    let mut v = est;
    v["files"] = json!(files);
    v["config"] = json!(config);
    v["pending"] = json!(crate::files::STORE.lock().has_pending());
    to_js(&v)
}

/// Remove a stored file (media, project, auto-save). Resolves with whether it existed.
#[wasm_bindgen(js_name = removeStored)]
pub fn remove_stored(path: String) -> bool {
    crate::files::STORE.remove_file(&path)
}

/// Resolves once every change is written to browser storage; rejects when a write failed (it is
/// tried again).
#[wasm_bindgen]
pub fn flush() -> js_sys::Promise {
    crate::persist::flushed()
}
