//! The `postMessage` bridge to a same-origin parent page (`?host=parent`), for pages that embed
//! EffectCraft in an `<iframe>` (such as a file manager):
//!
//! - `effectcraft:ready` (to the parent): `{ version }`, the app is listening; send a file now.
//! - `effectcraft:open` (from the parent): `{ name, bytes }`, bytes an `ArrayBuffer` or
//!   `Uint8Array`. It opens like a file picked with File ▸ Open / Import or dropped on the page:
//!   a `.ecproj` opens as the project, anything else is imported as footage.
//! - `effectcraft:save` (to the parent): `{ name, bytes }` (a transferred `ArrayBuffer`) for every
//!   file EffectCraft would otherwise download: Save / Save As (the `.ecproj`, which marks the
//!   project clean), exported frames and Render Queue outputs (several files as one `.zip`).
//! - `effectcraft:dirty` (to the parent): `{ dirty }` whenever unsaved changes appear or go away.
//! - `effectcraft:failed` (to the parent): `{ error }` when the app couldn't start, or a
//!   document sent with `effectcraft:open` is over the size limit (2 GiB) and wasn't opened.
//!
//! Only the parent window on the page's own origin is heard or answered. In host mode the page
//! starts on a blank project (no demo, no restored session, no session snapshot) and the parent
//! guards leaving the page. EffectCraft has no user name, so `?author=` is ignored.

use std::cell::{Cell, RefCell};

use wasm_bindgen::JsCast as _;
use wasm_bindgen::JsValue;
use wasm_bindgen::closure::Closure;

/// Largest document taken from the parent page, checked before its bytes are copied in: the
/// same 2 GiB ceiling the FBX importer holds an import to (`MAX_IMPORT_BYTES` in
/// `effectcraft-model`), well inside a tab's 4 GiB wasm32 heap.
const MAX_OPEN_BYTES: u32 = 1 << 31;

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
}

/// Connect to the parent when the page is framed; false when it isn't.
pub fn connect() -> bool {
    let Some(bridge) = Bridge::new() else { return false };
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

/// Report the project's unsaved-changes state when it changed (called every frame).
pub fn dirty(dirty: bool) {
    if REPORTED.with(Cell::get) == Some(dirty) {
        return;
    }
    if with(|b| b.send(&[("type", "effectcraft:dirty".into()), ("dirty", dirty.into())])) == Some(true) {
        REPORTED.with(|r| r.set(Some(dirty)));
    }
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

    fn post_save(&self, name: &str, bytes: &[u8]) -> Result<(), JsValue> {
        let buffer = js_sys::Uint8Array::from(bytes).buffer();
        let message = message(&[("type", "effectcraft:save".into()), ("name", name.into()), ("bytes", buffer.clone().into())]).ok_or("couldn't build the message")?;
        self.parent.post_message_with_transfer(&message, &self.origin, &js_sys::Array::of1(&buffer))
    }

    fn listen(&self) {
        let parent = self.parent.clone();
        let origin = self.origin.clone();
        let on_message = Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |event: web_sys::MessageEvent| {
            if event.origin() != origin || !event.source().is_some_and(|s| js_sys::Object::is(&s, &parent)) {
                return;
            }
            let data = event.data();
            if string(&data, "type").as_deref() != Some("effectcraft:open") {
                return;
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
