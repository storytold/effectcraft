//! The `postMessage` bridge to a same-origin parent page (`?host=parent`), for pages that embed
//! EffectCraft in an `<iframe>` (such as a file manager):
//!
//! - `effectcraft:ready` (to the parent): `{ version }`, the app is listening; send a file now.
//! - `effectcraft:open` (from the parent): `{ name, bytes }`, bytes an `ArrayBuffer` or
//!   `Uint8Array`. It opens like a file picked with File ▸ Open / Import or dropped on the page:
//!   a `.ecproj` opens as the project, anything else is imported as footage.
//! - `effectcraft:save` (to the parent): `{ id, name, bytes }` (a transferred `ArrayBuffer`) for every
//!   file EffectCraft would otherwise download: Save / Save As (the `.ecproj`, which marks the
//!   project clean), exported frames and Render Queue outputs (several files as one `.zip`).
//! - `effectcraft:saved` (from the parent): `{ id }`, the file from that `effectcraft:save` is
//!   stored. Until it arrives a project save keeps the project reported dirty; without it within
//!   30 s the app posts `effectcraft:failed { id, error }` and the project stays dirty.
//! - `effectcraft:dirty` (to the parent): `{ dirty }` whenever unsaved changes appear or go away.
//! - `effectcraft:failed` (to the parent): `{ error }` when the app couldn't start, or a
//!   document sent with `effectcraft:open` is over the size limit (1 GiB) and wasn't opened, or
//!   a save went unconfirmed.
//!
//! The bridge is installed only for a same-origin parent, or for a parent on the origin named by
//! `?host_origin=` (a bare `scheme://host[:port]` origin); messages go to that exact origin and only
//! the parent window on it is heard. A cross-origin parent without `host_origin` gets no bridge:
//! the page runs as usual and saves download. In host mode the page
//! starts on a blank project (no demo, no restored session, no session snapshot) and the parent
//! guards leaving the page. EffectCraft has no user name, so `?author=` is ignored.

use std::cell::{Cell, RefCell};

use wasm_bindgen::JsCast as _;
use wasm_bindgen::JsValue;
use wasm_bindgen::closure::Closure;

use crate::bridge::SaveAcks;

/// Largest document taken from the parent page, checked before its bytes are copied in. Opening
/// copies it twice (into a `Vec`, then into the file store's `Arc<[u8]>`), and a wasm32 `Vec`
/// holds at most `isize::MAX` (just under 2 GiB), so 1 GiB leaves room for both copies inside a
/// tab's 4 GiB heap.
const MAX_OPEN_BYTES: u32 = 1 << 30;

thread_local! {
    static BRIDGE: RefCell<Option<Bridge>> = const { RefCell::new(None) };
}

#[derive(Clone)]
pub struct Bridge {
    window: web_sys::Window,
    parent: web_sys::Window,
    origin: String,
}

thread_local! {
    /// The unsaved-changes state last reported to the parent.
    static REPORTED: Cell<Option<bool>> = const { Cell::new(None) };
    /// Saves waiting for `effectcraft:saved`.
    static ACKS: RefCell<SaveAcks> = RefCell::new(SaveAcks::default());
}

/// Connect to the parent when the page is framed by a same-origin parent, or by the parent on
/// `host_origin`; false otherwise (no bridge).
pub fn connect(host_origin: Option<&str>) -> bool {
    let Some(bridge) = Bridge::new(host_origin) else { return false };
    bridge.listen();
    BRIDGE.with(|b| *b.borrow_mut() = Some(bridge));
    true
}

fn with<R>(f: impl FnOnce(&Bridge) -> R) -> Option<R> {
    BRIDGE.with(|b| b.borrow().as_ref().map(f))
}

/// In host mode: hand `bytes` to the parent instead of downloading them.
pub fn save(name: &str, bytes: &[u8]) -> Option<Result<(), JsValue>> {
    with(|b| b.post_save(name, bytes))
}

pub fn ready() {
    with(|b| b.send(&[("type", "effectcraft:ready".into()), ("version", env!("CARGO_PKG_VERSION").into())]));
}

pub fn failed(error: &str) {
    with(|b| b.send(&[("type", "effectcraft:failed".into()), ("error", error.into())]));
}

/// Report the project's unsaved-changes state when it changed (called every frame): held dirty
/// while a project save is unconfirmed; saves the parent didn't confirm in time fail.
pub fn dirty(ctx: &egui::Context, session_dirty: bool) {
    if with(|_| ()).is_none() {
        return;
    }
    for (id, name) in ACKS.with(|a| a.borrow_mut().expire(js_sys::Date::now())) {
        with(|b| {
            b.send(&[
                ("type", "effectcraft:failed".into()),
                ("id", JsValue::from_f64(id as f64)),
                ("error", format!("{name}: the host page didn't confirm the save").into()),
            ])
        });
    }
    let (dirty, waiting) = ACKS.with(|a| {
        let a = a.borrow();
        (a.dirty(session_dirty), a.waiting())
    });
    if waiting {
        ctx.request_repaint_after(std::time::Duration::from_secs(1));
    }
    if REPORTED.with(Cell::get) == Some(dirty) {
        return;
    }
    if with(|b| b.send(&[("type", "effectcraft:dirty".into()), ("dirty", dirty.into())])) == Some(true) {
        REPORTED.with(|r| r.set(Some(dirty)));
    }
}

impl Bridge {
    /// `None` when the page isn't framed, or the parent is cross-origin and not the origin
    /// `host_origin` allows (see [`crate::bridge::target_origin`]).
    fn new(host_origin: Option<&str>) -> Option<Self> {
        let window = web_sys::window()?;
        let parent = window.parent().ok().flatten()?;
        if js_sys::Object::is(&parent, &window) {
            return None;
        }
        let own = window.location().origin().ok()?;
        // Reading a cross-origin parent's location throws: treat that as cross-origin.
        let parent_origin = parent.location().origin().ok();
        let origin = crate::bridge::target_origin(&own, parent_origin.as_deref(), host_origin)?;
        Some(Self { window, parent, origin })
    }

    fn post_save(&self, name: &str, bytes: &[u8]) -> Result<(), JsValue> {
        let buffer = js_sys::Uint8Array::from(bytes).buffer();
        let project = name.ends_with(".ecproj");
        let id = ACKS.with(|a| a.borrow_mut().posted(name, project, js_sys::Date::now()));
        let sent =
            message(&[("type", "effectcraft:save".into()), ("id", JsValue::from_f64(id as f64)), ("name", name.into()), ("bytes", buffer.clone().into())])
                .ok_or_else(|| JsValue::from_str("couldn't build the message"))
                .and_then(|m| self.parent.post_message_with_transfer(&m, &self.origin, &js_sys::Array::of1(&buffer)));
        if sent.is_err() {
            ACKS.with(|a| a.borrow_mut().failed(id));
        }
        sent
    }

    fn listen(&self) {
        let parent = self.parent.clone();
        let origin = self.origin.clone();
        let on_message = Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |event: web_sys::MessageEvent| {
            if event.origin() != origin || !event.source().is_some_and(|s| js_sys::Object::is(&s, &parent)) {
                return;
            }
            let data = event.data();
            match string(&data, "type").as_deref() {
                Some("effectcraft:open") => {}
                Some("effectcraft:saved") => {
                    let id = js_sys::Reflect::get(&data, &JsValue::from_str("id")).ok().and_then(|v| v.as_f64());
                    if let Some(id) = id.filter(|id| id.is_finite() && *id >= 0.0 && id.fract() == 0.0) {
                        ACKS.with(|a| a.borrow_mut().saved(id as u64));
                    }
                    return;
                }
                _ => return,
            }
            let name = string(&data, "name").filter(|n| !n.trim().is_empty()).unwrap_or_else(|| "Untitled Project.ecproj".to_string());
            let Some(array) = byte_array(&data) else { return };
            if array.byte_length() > MAX_OPEN_BYTES {
                failed(&format!("{name}: too large to open ({} MiB; the limit is {} MiB)", array.byte_length() >> 20, MAX_OPEN_BYTES >> 20));
                return;
            }
            let path = crate::files::path_for(&name);
            crate::files::put(&path, array.to_vec().into());
            crate::post(move |app| crate::open_or_import(app, vec![path]));
        });
        if self.window.add_event_listener_with_callback("message", on_message.as_ref().unchecked_ref()).is_ok() {
            // The listener lives as long as the page.
            on_message.forget();
        }
    }

    /// Post a message to the parent; false if it couldn't be sent.
    fn send(&self, fields: &[(&str, JsValue)]) -> bool {
        message(fields).is_some_and(|m| self.parent.post_message(&m, &self.origin).is_ok())
    }
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

/// `bytes` (an `ArrayBuffer` or `Uint8Array`) as a view, not yet copied.
fn byte_array(data: &JsValue) -> Option<js_sys::Uint8Array> {
    let bytes = js_sys::Reflect::get(data, &JsValue::from_str("bytes")).ok()?;
    if let Some(array) = bytes.dyn_ref::<js_sys::Uint8Array>() {
        Some(array.clone())
    } else if bytes.is_instance_of::<js_sys::ArrayBuffer>() {
        Some(js_sys::Uint8Array::new(&bytes))
    } else {
        None
    }
}
