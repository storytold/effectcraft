//! `./effectcraft-config.json`, fetched once per page load and shared by whoever reads a key.

use std::cell::RefCell;

use wasm_bindgen::{JsCast, closure::Closure};
use wasm_bindgen_futures::JsFuture;

use crate::startup::{self, Startup};

/// Longest the request and the body read may take (ms); a stalled server must not hold up start-up.
const TIMEOUT_MS: i32 = 5_000;

thread_local! {
    /// `None`: not fetched yet; `Some(None)`: no usable file.
    static FILE: RefCell<Option<Option<Vec<u8>>>> = const { RefCell::new(None) };
}

/// The config file's bytes, or `None` when it is missing, too large, unreadable or the request
/// takes longer than [`TIMEOUT_MS`] (the timer aborts the request and covers the body read).
async fn fetch() -> Option<Vec<u8>> {
    let win = web_sys::window()?;
    let ctl = web_sys::AbortController::new().ok()?;
    let init = web_sys::RequestInit::new();
    // Always ask the server: the file is the operator's switch, never a cached copy.
    init.set_cache(web_sys::RequestCache::NoCache);
    init.set_signal(Some(&ctl.signal()));
    let on_timeout = Closure::once(move || ctl.abort());
    let timer = win.set_timeout_with_callback_and_timeout_and_arguments_0(on_timeout.as_ref().unchecked_ref(), TIMEOUT_MS).ok()?;
    let out = read(&win, &init).await;
    win.clear_timeout_with_handle(timer);
    out
}

/// Fetch the file and read its body chunk by chunk, giving up as soon as it passes `CONFIG_MAX`.
async fn read(win: &web_sys::Window, init: &web_sys::RequestInit) -> Option<Vec<u8>> {
    let req = web_sys::Request::new_with_str_and_init("effectcraft-config.json", init).ok()?;
    let resp: web_sys::Response = JsFuture::from(win.fetch_with_request(&req)).await.ok()?.dyn_into().ok()?;
    if resp.status() != 200 {
        return None;
    }
    let reader: web_sys::ReadableStreamDefaultReader = resp.body()?.get_reader().unchecked_into();
    let mut out = Vec::new();
    loop {
        let chunk = JsFuture::from(reader.read()).await.ok()?;
        if js_sys::Reflect::get(&chunk, &"done".into()).ok()?.as_bool() == Some(true) {
            return Some(out);
        }
        let bytes: js_sys::Uint8Array = js_sys::Reflect::get(&chunk, &"value".into()).ok()?.dyn_into().ok()?;
        if out.len().saturating_add(bytes.length() as usize) > startup::CONFIG_MAX {
            // Stop the download: the limit is enforced while reading, not after the whole body arrived.
            let _ = reader.cancel();
            return None;
        }
        out.extend_from_slice(&bytes.to_vec());
    }
}

/// The config file's bytes (`None` when there is none), fetched on the first call.
pub async fn bytes() -> Option<Vec<u8>> {
    if let Some(done) = FILE.with(|f| f.borrow().clone()) {
        return done;
    }
    let got = fetch().await;
    FILE.with(|f| *f.borrow_mut() = Some(got.clone()));
    got
}

/// The configured startup behaviour.
pub async fn startup() -> Startup {
    let s = bytes().await.map(|b| startup::parse(&b)).unwrap_or_default();
    log::info!("startup: {s:?}");
    s
}
