//! The Media Browser in the browser ([`effectcraft_engine::media_browser::Browser`]).
//!
//! It browses a virtual tree:
//!
//! - **Browser Storage** (`/Browser Storage/…`): every file in the persistent file table (OPFS /
//!   IndexedDB): imported media, uploaded files and folders, saved projects. Files added with
//!   **Add Files…** / **Upload Folder…** (plain `<input type=file>`, so every browser) are copied
//!   there and stay across visits: the recent list.
//! - **Folders** (`/Folders/<name>/…`): folders opened with **Open Folder…** through the File
//!   System Access API (`showDirectoryPicker`, Chromium browsers). Only their listing is read;
//!   a file's bytes are read when it is imported ([`Browser::fetch`]) and then kept in the file
//!   table. The folder handles are remembered (IndexedDB), so after a reload the folders come
//!   back once the browser grants access again (**Reconnect <name>**).

use std::sync::{LazyLock, Mutex};

use effectcraft_engine::media_browser::{Browser, Entry, VirtualTree};
use serde_json::{Value, json};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

#[wasm_bindgen(module = "/js/host.js")]
extern "C" {
    #[wasm_bindgen(js_name = browseCanPickFolder)]
    fn can_pick_folder() -> bool;
    #[wasm_bindgen(js_name = browsePickFolder)]
    fn pick_folder() -> js_sys::Promise;
    #[wasm_bindgen(js_name = browseRestore)]
    fn restore_folders() -> js_sys::Promise;
    #[wasm_bindgen(js_name = browseReconnect)]
    fn reconnect(name: &str) -> js_sys::Promise;
    #[wasm_bindgen(js_name = browseForget)]
    fn forget(name: &str) -> js_sys::Promise;
    #[wasm_bindgen(js_name = browseRead)]
    fn read(path: &str) -> js_sys::Promise;
    #[wasm_bindgen(js_name = browsePickFiles)]
    fn pick_files(directory: bool) -> js_sys::Promise;
}

const STORAGE: &str = "/Browser Storage";
const FOLDERS: &str = "/Folders";

#[derive(Default)]
struct State {
    /// Opened folders' files (`/Folders/…`).
    folders: VirtualTree,
    /// Opened folder names, and remembered ones waiting for access.
    open: Vec<String>,
    disconnected: Vec<String>,
}

static STATE: LazyLock<Mutex<State>> = LazyLock::new(Mutex::default);

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The web app's Media Browser.
pub struct WebBrowser;

/// The stored files as a tree under `/Browser Storage` (rebuilt per listing: storage changes).
fn storage_tree() -> VirtualTree {
    let mut t = VirtualTree::default();
    t.add_dir(STORAGE);
    for (path, size, modified, persisted) in crate::files::STORE.files() {
        if !persisted || path.starts_with(FOLDERS) {
            continue;
        }
        t.add_file(&format!("{STORAGE}{path}"), &path, size as u64, Some((modified / 1000.0) as u64));
    }
    t
}

/// Add an opened folder's listing (`[{path, dir, size, modified}]`, paths `name/…`).
fn add_folder(v: &Value) -> Option<String> {
    let name = v["name"].as_str()?.to_string();
    let mut st = state();
    st.folders.remove(&format!("{FOLDERS}/{name}"));
    st.folders.add_dir(&format!("{FOLDERS}/{name}"));
    for e in v["entries"].as_array().into_iter().flatten() {
        let Some(rel) = e["path"].as_str() else { continue };
        let path = format!("{FOLDERS}/{rel}");
        if e["dir"].as_bool() == Some(true) {
            st.folders.add_dir(&path);
        } else {
            st.folders.add_file(&path, &path, e["size"].as_u64().unwrap_or(0), e["modified"].as_u64());
        }
    }
    st.open.retain(|n| *n != name);
    st.open.push(name.clone());
    st.disconnected.retain(|n| *n != name);
    Some(name)
}

fn to_value(v: &JsValue) -> Value {
    js_sys::JSON::stringify(v).ok().and_then(|s| s.as_string()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null)
}

/// Run a promise; on success, `then` gets its value (as JSON) on the UI thread.
fn spawn(p: js_sys::Promise, what: &'static str, then: impl FnOnce(&mut effectcraft_ui_egui::EffectcraftApp, Value) + 'static) {
    wasm_bindgen_futures::spawn_local(async move {
        match wasm_bindgen_futures::JsFuture::from(p).await {
            Ok(v) => {
                let v = to_value(&v);
                crate::post(move |app| then(app, v));
            }
            Err(e) => {
                let msg = crate::persist::js_err(e);
                // Cancelling a picker is not an error.
                if !msg.contains("abort") && !msg.contains("AbortError") {
                    crate::post(move |app| app.ui.status = format!("{what}: {msg}"));
                }
            }
        }
    });
}

fn go(app: &mut effectcraft_ui_egui::EffectcraftApp, path: &str) {
    if let Err(e) = app.session.execute("mediaBrowser.go", json!({"path": path})) {
        app.ui.status = e.to_string();
    }
}

/// Bring back the folders opened on earlier visits (those the browser still grants access to).
pub fn restore() {
    spawn(restore_folders(), "Media Browser", |_, v| {
        for f in v.as_array().into_iter().flatten() {
            if f["granted"].as_bool() == Some(true) {
                add_folder(f);
            } else if let Some(n) = f["name"].as_str() {
                let mut st = state();
                if !st.disconnected.iter().any(|d| d == n) {
                    st.disconnected.push(n.to_string());
                }
            }
        }
    });
}

impl Browser for WebBrowser {
    fn home(&self) -> String {
        STORAGE.into()
    }

    fn places(&self) -> Vec<(String, String)> {
        let mut v = vec![("Browser Storage".to_string(), STORAGE.to_string())];
        v.extend(state().open.iter().map(|n| (n.clone(), format!("{FOLDERS}/{n}"))));
        v
    }

    fn list(&self, dir: &str, importable_only: bool) -> Result<Vec<Entry>, String> {
        let dir = if dir.len() > 1 { dir.trim_end_matches('/') } else { dir };
        if dir == STORAGE || dir.starts_with(&format!("{STORAGE}/")) {
            return storage_tree().list(dir, importable_only);
        }
        if dir == "/" {
            let mut t = VirtualTree::default();
            t.add_dir(STORAGE);
            t.add_dir(FOLDERS);
            return t.list("/", false);
        }
        let st = state();
        if dir == FOLDERS && st.open.is_empty() {
            return Ok(vec![]);
        }
        st.folders.list(dir, importable_only).map_err(|e| {
            let name = dir.trim_start_matches(FOLDERS).trim_start_matches('/').split('/').next().unwrap_or("");
            if st.disconnected.iter().any(|d| d == name) { format!("{name}: click Reconnect {name} to allow access again") } else { e }
        })
    }

    fn parent(&self, dir: &str) -> Option<String> {
        let d = dir.trim_end_matches('/');
        match d.rfind('/') {
            Some(0) if d.len() > 1 => Some("/".into()),
            Some(i) if i > 0 => Some(d[..i].to_string()),
            _ => None,
        }
    }

    fn fetch(&self, paths: &[String], params: &Value) -> bool {
        let missing: Vec<String> = paths.iter().filter(|p| p.starts_with(&format!("{FOLDERS}/")) && crate::files::get(p).is_none()).cloned().collect();
        if missing.is_empty() {
            return true;
        }
        let params = params.clone();
        wasm_bindgen_futures::spawn_local(async move {
            for p in &missing {
                match wasm_bindgen_futures::JsFuture::from(read(p)).await {
                    Ok(b) => crate::files::put(p, js_sys::Uint8Array::new(&b).to_vec().into()),
                    Err(e) => {
                        let msg = format!("{p}: {}", e.as_string().unwrap_or_else(|| "cannot read the file (reconnect its folder)".into()));
                        crate::post(move |app| app.ui.status = msg);
                        return;
                    }
                }
            }
            crate::post(move |app| match app.session.execute("mediaBrowser.import", params) {
                Ok(r) => {
                    if let Some(errs) = r["errors"].as_array().filter(|e| !e.is_empty()) {
                        app.ui.status = errs.iter().filter_map(|e| e.as_str()).collect::<Vec<_>>().join("; ");
                    }
                }
                Err(e) => app.ui.status = e.to_string(),
            });
        });
        false
    }

    fn actions(&self) -> Vec<(String, String)> {
        let mut v = vec![];
        if can_pick_folder() {
            v.push(("openFolder".to_string(), "Open Folder…".to_string()));
        }
        v.push(("addFiles".into(), "Add Files…".into()));
        v.push(("uploadFolder".into(), "Upload Folder…".into()));
        for n in &state().disconnected {
            v.push((format!("reconnect:{n}"), format!("Reconnect {n}")));
        }
        v
    }

    fn action(&self, id: &str) -> Result<Value, String> {
        match id {
            "openFolder" => {
                if !can_pick_folder() {
                    return Err("this browser cannot open folders (use Add Files… or Upload Folder…)".into());
                }
                spawn(pick_folder(), "Open Folder", |app, v| {
                    if let Some(n) = add_folder(&v) {
                        go(app, &format!("{FOLDERS}/{n}"));
                    }
                });
                Ok(json!({"pending": true}))
            }
            "addFiles" | "uploadFolder" => {
                let what = if id == "uploadFolder" { "Upload Folder…" } else { "Add Files…" };
                let p = pick_files(id == "uploadFolder");
                wasm_bindgen_futures::spawn_local(async move {
                    let list = match wasm_bindgen_futures::JsFuture::from(p).await {
                        Ok(list) => js_sys::Array::from(&list),
                        Err(e) => {
                            let msg = crate::persist::js_err(e);
                            crate::post(move |app| app.ui.status = format!("{what}: {msg}"));
                            return;
                        }
                    };
                    // Files that can't be read are reported, not dropped silently (#226).
                    let (mut n, mut unread) = (0, vec![]);
                    for f in list.iter() {
                        let get = |key: &str| js_sys::Reflect::get(&f, &key.into()).ok();
                        let Some(path) = get("path").and_then(|v| v.as_string()) else { continue };
                        match get("bytes").and_then(|b| b.dyn_into::<js_sys::Uint8Array>().ok()) {
                            Some(bytes) => {
                                crate::files::put(&format!("/{}", path.trim_start_matches('/')), bytes.to_vec().into());
                                n += 1;
                            }
                            None => unread.push(match get("error").and_then(|e| e.as_string()).filter(|e| !e.is_empty()) {
                                Some(why) => format!("{path} ({why})"),
                                None => path,
                            }),
                        }
                    }
                    // Cancelled: nothing to report.
                    if n == 0 && unread.is_empty() {
                        return;
                    }
                    crate::post(move |app| {
                        app.ui.status = format!("{n} file(s) added to Browser Storage");
                        if !unread.is_empty() {
                            app.ui.status += &format!(". Could not read {}: try {what} again", unread.join(", "));
                        }
                        go(app, STORAGE);
                    });
                });
                Ok(json!({"pending": true}))
            }
            _ => {
                if let Some(name) = id.strip_prefix("reconnect:") {
                    spawn(reconnect(name), "Reconnect", |app, v| {
                        if let Some(n) = add_folder(&v) {
                            go(app, &format!("{FOLDERS}/{n}"));
                        }
                    });
                    return Ok(json!({"pending": true}));
                }
                if let Some(name) = id.strip_prefix("forget:") {
                    let _ = forget(name);
                    let mut st = state();
                    st.open.retain(|n| n != name);
                    st.disconnected.retain(|n| n != name);
                    st.folders.remove(&format!("{FOLDERS}/{name}"));
                    return Ok(json!({"forgotten": name}));
                }
                Err(format!("unknown Media Browser action `{id}`"))
            }
        }
    }
}
