//! The `postMessage` bridge to a same-origin parent page (`?host=parent`), for pages that embed
//! EffectCraft in an `<iframe>` (such as a Nextcloud app). Messages are plain objects with a
//! `type`:
//!
//! - `effectcraft:ready` (to the parent): `{ version }`, the app is listening; send a file now.
//! - `effectcraft:open` (from the parent): `{ name, path, blob }` (or `bytes`: an `ArrayBuffer`
//!   or `Uint8Array`). `path` is where the file lives on the parent's side ("/Motion/intro.mp4");
//!   it is kept in the file table under [`PREFIX`]. A project opens; anything else is imported,
//!   as a picked or dropped file is, into a composition made from it.
//! - `effectcraft:need` (to the parent): `{ paths }`, footage an opened project refers to that is
//!   on the parent's side and not here yet.
//! - `effectcraft:file` (from the parent): `{ path, blob }` one of them, or `{ path, error }` when
//!   it can't be had (File ▸ Dependencies ▸ Find Missing Footage then lists it).
//! - `effectcraft:save` (to the parent): `{ name, bytes }` (a transferred `ArrayBuffer`) for every
//!   file EffectCraft would otherwise download (saved projects, Render Queue outputs): `name` is
//!   the parent's path for a file under [`PREFIX`], else just the file name (a new project, a
//!   render), for the parent to ask where it goes. The parent stores it and reports failures
//!   itself.
//! - `effectcraft:dirty` (to the parent): `{ dirty }` whenever unsaved changes appear or go away.
//! - `effectcraft:failed` (to the parent): `{ error }` when the app couldn't start.
//!
//! Only the parent window on the page's own origin is heard or answered. In host mode neither the
//! last visit's project nor the demo opens, the session isn't kept in browser storage, and the
//! parent guards leaving the page. Files from the parent stay out of browser storage too.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::Arc;

use effectcraft_engine::project::{ItemKind, Project};
use wasm_bindgen::JsCast as _;
use wasm_bindgen::JsValue;
use wasm_bindgen::closure::Closure;
use wasm_bindgen_futures::JsFuture;

use crate::files;

/// Where the parent's files live in the file table: "/Motion/intro.mp4" there is
/// "/host/Motion/intro.mp4" here.
pub const PREFIX: &str = "/host";

/// How long to wait for a project's footage to arrive before opening it anyway.
const FOOTAGE_WAIT_MS: f64 = 30.0 * 60.0 * 1000.0;

thread_local! {
    static BRIDGE: RefCell<Option<Bridge>> = const { RefCell::new(None) };
    /// Footage asked of the parent (by path here): `true` once answered.
    static NEEDED: RefCell<HashMap<String, bool>> = RefCell::new(HashMap::new());
    /// The unsaved-changes state last reported to the parent.
    static REPORTED: Cell<Option<bool>> = const { Cell::new(None) };
    /// The file name an untitled project saves under: the footage it was made from.
    static SUGGESTED: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// The name an untitled project gets when it is first saved.
const UNTITLED: &str = "Untitled Project";

#[derive(Clone)]
struct Bridge {
    window: web_sys::Window,
    parent: web_sys::Window,
    origin: String,
}

impl Bridge {
    /// `None` when the page isn't framed (there is no parent to talk to).
    fn new() -> Option<Self> {
        let window = web_sys::window()?;
        let parent = window.parent().ok().flatten()?;
        if js_sys::Object::is(&parent, &window) {
            return None;
        }
        let origin = window.location().origin().ok()?;
        Some(Self { window, parent, origin })
    }

    fn post(&self, fields: &[(&str, JsValue)]) -> bool {
        message(fields).is_some_and(|m| self.parent.post_message(&m, &self.origin).is_ok())
    }
}

fn requested() -> bool {
    web_sys::window()
        .and_then(|w| w.location().search().ok())
        .and_then(|s| web_sys::UrlSearchParams::new_with_str(&s).ok())
        .and_then(|p| p.get("host"))
        .is_some_and(|h| h == "parent")
}

/// Whether the page runs embedded (`?host=parent` inside a frame).
pub fn embedded() -> bool {
    requested() && Bridge::new().is_some()
}

/// Whether the bridge is running ([`start`] succeeded).
pub fn active() -> bool {
    BRIDGE.with(|b| b.borrow().is_some())
}

/// Embedded: hear files from the parent and say the app is ready. Call once the app runs.
pub fn start(app: &mut effectcraft_ui_egui::EffectcraftApp) -> bool {
    if !requested() {
        return false;
    }
    let Some(bridge) = Bridge::new() else { return false };
    // An untitled project made from footage the parent sent saves under the footage's name.
    app.hooks.pick_save = Some(Box::new(|name: &str| {
        let suggested = SUGGESTED.with(|s| s.borrow().clone()).filter(|_| name.is_empty() || name.starts_with(UNTITLED));
        Some(files::path_for(suggested.as_deref().unwrap_or(if name.is_empty() { "Untitled Project.ecproj" } else { name })))
    }));
    listen(&bridge);
    bridge.post(&[("type", "effectcraft:ready".into()), ("version", env!("CARGO_PKG_VERSION").into())]);
    BRIDGE.with(|b| *b.borrow_mut() = Some(bridge));
    true
}

/// Embedded and EffectCraft couldn't start: tell the parent.
pub fn failed(error: &str) {
    if requested()
        && let Some(bridge) = Bridge::new()
    {
        bridge.post(&[("type", "effectcraft:failed".into()), ("error", error.into())]);
    }
}

/// After each frame: tell the parent when unsaved changes appear or go away.
pub fn report_dirty(dirty: bool) {
    if REPORTED.with(Cell::get) == Some(dirty) {
        return;
    }
    let sent = BRIDGE.with(|b| b.borrow().as_ref().is_some_and(|b| b.post(&[("type", "effectcraft:dirty".into()), ("dirty", dirty.into())])));
    if sent {
        REPORTED.with(|r| r.set(Some(dirty)));
    }
}

/// The name the parent gets for a file written at `path` here.
fn parent_name(path: &str) -> String {
    match path.strip_prefix(PREFIX).filter(|p| p.starts_with('/')) {
        Some(theirs) => theirs.to_string(),
        None => path.rsplit('/').next().filter(|n| !n.is_empty()).unwrap_or("effectcraft-output").to_string(),
    }
}

/// Hand a written file to the parent ([`files`] calls this instead of downloading).
pub fn post_save(path: &str, data: &[u8]) -> Result<(), String> {
    let bridge = BRIDGE.with(|b| b.borrow().clone()).ok_or("the page that embeds EffectCraft is gone")?;
    let buffer = js_sys::Uint8Array::from(data).buffer();
    let message = message(&[("type", "effectcraft:save".into()), ("name", parent_name(path).into()), ("bytes", buffer.clone().into())])
        .ok_or("couldn't build the message")?;
    bridge.parent.post_message_with_transfer(&message, &bridge.origin, &js_sys::Array::of1(&buffer)).map_err(|e| format!("{e:?}"))
}

fn listen(bridge: &Bridge) {
    let parent = bridge.parent.clone();
    let origin = bridge.origin.clone();
    let on_message = Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |event: web_sys::MessageEvent| {
        if event.origin() != origin || !event.source().is_some_and(|s| js_sys::Object::is(&s, &parent)) {
            return;
        }
        let data = event.data();
        match string(&data, "type").as_deref() {
            Some("effectcraft:open") => {
                let Some(blob) = blob(&data) else { return };
                let name = string(&data, "name").filter(|n| !n.trim().is_empty()).unwrap_or_else(|| "Footage".to_string());
                let theirs = string(&data, "path").filter(|p| p.starts_with('/')).unwrap_or_else(|| format!("/{name}"));
                wasm_bindgen_futures::spawn_local(open(format!("{PREFIX}{theirs}"), blob));
            }
            Some("effectcraft:file") => {
                let Some(theirs) = string(&data, "path").filter(|p| p.starts_with('/')) else { return };
                let here = format!("{PREFIX}{theirs}");
                let blob = blob(&data);
                wasm_bindgen_futures::spawn_local(async move {
                    if let Some(blob) = blob
                        && let Ok(bytes) = read(&blob).await
                    {
                        files::STORE.put_file(&here, bytes.into(), false);
                    }
                    NEEDED.with(|n| {
                        if let Some(done) = n.borrow_mut().get_mut(&here) {
                            *done = true;
                        }
                    });
                    crate::repaint();
                });
            }
            _ => {}
        }
    });
    if bridge.window.add_event_listener_with_callback("message", on_message.as_ref().unchecked_ref()).is_ok() {
        // The listener lives as long as the page.
        on_message.forget();
    }
}

async fn read(blob: &web_sys::Blob) -> Result<Vec<u8>, String> {
    let buffer = JsFuture::from(blob.array_buffer()).await.map_err(|e| format!("{e:?}"))?;
    Ok(js_sys::Uint8Array::new(&buffer).to_vec())
}

/// Open a file the parent sent (kept at `here`): a project with its footage, or media to import.
async fn open(here: String, blob: web_sys::Blob) {
    let bytes = match read(&blob).await {
        Ok(b) => b,
        Err(e) => return crate::post(move |app| app.ui.status = format!("Couldn't read {here}: {e}")),
    };
    if here.to_ascii_lowercase().ends_with(".ecproj") {
        let wanted = String::from_utf8(bytes.clone()).ok().and_then(|text| Project::from_json(&text).ok()).map(|p| missing_footage(&p)).unwrap_or_default();
        if !wanted.is_empty() {
            fetch_footage(wanted).await;
        }
    }
    let project = here.to_ascii_lowercase().ends_with(".ecproj");
    if !project {
        let name = here.rsplit('/').next().unwrap_or(&here);
        let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
        SUGGESTED.with(|s| *s.borrow_mut() = Some(format!("{stem}.ecproj")));
    }
    files::STORE.put_file(&here, Arc::from(bytes), false);
    crate::post(move |app| {
        crate::open_or_import(app, vec![here]);
        if !project {
            // A composition made from the footage (it is selected after the import), as File ▸ New
            // Comp from Selection makes it, and nothing to save yet: the new project holds only
            // what the user opened.
            if app.session.execute("file.newCompFromSelection", serde_json::json!({})).is_ok() {
                app.session.mark_saved();
            }
        }
    });
}

/// The footage files (each frame of a sequence) a project refers to on the parent's side that
/// aren't here yet.
fn missing_footage(p: &Project) -> Vec<String> {
    let mut wanted = Vec::new();
    for item in p.items.values() {
        let ItemKind::Footage(f) = &item.kind else { continue };
        for path in std::iter::once(&f.path).chain(f.sequence.iter()) {
            if path.starts_with(&format!("{PREFIX}/")) && files::get(path).is_none() && !wanted.contains(path) {
                wanted.push(path.clone());
            }
        }
    }
    wanted
}

/// Ask the parent for `wanted` (paths here) and wait until each has arrived or can't be had.
async fn fetch_footage(wanted: Vec<String>) {
    NEEDED.with(|n| {
        let mut n = n.borrow_mut();
        for path in &wanted {
            n.insert(path.clone(), false);
        }
    });
    let count = wanted.len();
    crate::post(move |app| app.ui.status = format!("Loading {count} footage file(s) from the server…"));
    let paths = js_sys::Array::from_iter(wanted.iter().map(|p| JsValue::from_str(&parent_name(p))));
    let asked = BRIDGE.with(|b| b.borrow().as_ref().is_some_and(|b| b.post(&[("type", "effectcraft:need".into()), ("paths", paths.into())])));
    let start = js_sys::Date::now();
    while asked && js_sys::Date::now() - start < FOOTAGE_WAIT_MS && NEEDED.with(|n| wanted.iter().any(|p| n.borrow().get(p) == Some(&false))) {
        sleep(100).await;
    }
    NEEDED.with(|n| {
        let mut n = n.borrow_mut();
        for path in &wanted {
            n.remove(path);
        }
    });
}

async fn sleep(ms: i32) {
    let p = js_sys::Promise::new(&mut |resolve, _| {
        if let Some(w) = web_sys::window() {
            let _ = w.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms);
        }
    });
    let _ = JsFuture::from(p).await;
}

fn message(fields: &[(&str, JsValue)]) -> Option<js_sys::Object> {
    let m = js_sys::Object::new();
    for (key, value) in fields {
        js_sys::Reflect::set(&m, &JsValue::from_str(key), value).ok()?;
    }
    Some(m)
}

fn string(data: &JsValue, key: &str) -> Option<String> {
    js_sys::Reflect::get(data, &JsValue::from_str(key)).ok()?.as_string()
}

/// The file in a message: `blob` (a `Blob`), or `bytes` (an `ArrayBuffer` or `Uint8Array`).
fn blob(data: &JsValue) -> Option<web_sys::Blob> {
    if let Ok(b) = js_sys::Reflect::get(data, &JsValue::from_str("blob"))
        && let Ok(blob) = b.dyn_into::<web_sys::Blob>()
    {
        return Some(blob);
    }
    let bytes = js_sys::Reflect::get(data, &JsValue::from_str("bytes")).ok()?;
    let array = if let Some(a) = bytes.dyn_ref::<js_sys::Uint8Array>() {
        a.clone()
    } else if bytes.is_instance_of::<js_sys::ArrayBuffer>() {
        js_sys::Uint8Array::new(&bytes)
    } else {
        return None;
    };
    web_sys::Blob::new_with_u8_array_sequence(&js_sys::Array::of1(&array)).ok()
}
