//! Files in the browser: a virtual file table in place of the file system, `<input type=file>`
//! pickers for Open/Import, and downloads for what the user saves and renders.
//!
//! Picked or dropped files are read into the table under `/<name>`; the engine, the media pool and
//! the exporter read and write through it ([`WebServices`], [`WebImporter`], [`export_sink`]).
//! The table is the persistent [`Store`]'s file namespace: imported media, saved projects and
//! auto-saves survive reloads ([`crate::persist`]); render outputs don't (they download).

use std::sync::{Arc, LazyLock, Mutex};

use effectcraft_engine::Services;
use effectcraft_engine::project::Footage;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

use crate::store::Store;

/// The page's store (each worker has its own, unpersisted).
pub static STORE: LazyLock<Arc<Store>> = LazyLock::new(|| {
    let s = Arc::new(Store::default());
    let _ = s.clock.set(js_sys::Date::now);
    s
});
/// Render outputs written since the last [`flush_downloads`].
static OUTPUTS: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// Saves that write to browser storage only (no download): `api.saveToBrowser`.
static QUIET: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Store a file in the table (persisted).
pub fn put(path: &str, data: Arc<[u8]>) {
    STORE.put_file(path, data, true);
}

pub fn get(path: &str) -> Option<Arc<[u8]>> {
    STORE.file(path)
}

/// A file's bytes and version (new with every write: workers resend a file whose version they
/// haven't seen).
pub fn get_versioned(path: &str) -> Option<(Arc<[u8]>, u64)> {
    STORE.file_versioned(path)
}

/// `(path, size)` of every file in the table.
pub fn list() -> Vec<(String, usize)> {
    STORE.files().into_iter().map(|(p, n, _, _)| (p, n)).collect()
}

/// The next write of `path` goes to browser storage only (no download).
pub fn save_quietly(path: &str) {
    QUIET.lock().unwrap_or_else(|e| e.into_inner()).push(path.to_string());
}

/// A rendered file (from the page's exporter or a worker): kept in the table unpersisted and
/// downloaded at the next [`flush_downloads`].
pub fn add_output(path: &str, data: Arc<[u8]>) {
    STORE.put_file(path, data, false);
    OUTPUTS.lock().unwrap_or_else(|e| e.into_inner()).push(path.to_string());
}

/// The table path for a picked/dropped file name.
pub fn path_for(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    format!("/{base}")
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().filter(|n| !n.is_empty()).unwrap_or("effectcraft-output")
}

fn mime(name: &str) -> &'static str {
    match name.rsplit('.').next().unwrap_or("").to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "tif" | "tiff" => "image/tiff",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        "zip" => "application/zip",
        "ecproj" => "application/json",
        _ => "application/octet-stream",
    }
}

/// Offer `data` as a download named `name`.
pub fn download(name: &str, data: &[u8]) -> Result<(), JsValue> {
    let doc = web_sys::window().and_then(|w| w.document()).ok_or("no document")?;
    let parts = js_sys::Array::of1(&js_sys::Uint8Array::from(data));
    let bag = web_sys::BlobPropertyBag::new();
    bag.set_type(mime(name));
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &bag)?;
    let url = web_sys::Url::create_object_url_with_blob(&blob)?;
    let a: web_sys::HtmlAnchorElement = doc.create_element("a")?.dyn_into()?;
    a.set_href(&url);
    a.set_download(name);
    a.style().set_property("display", "none")?;
    doc.body().ok_or("no body")?.append_child(&a)?;
    a.click();
    a.remove();
    // Revoke once the browser has started the download.
    let revoke = Closure::once_into_js(move || {
        let _ = web_sys::Url::revoke_object_url(&url);
    });
    if let Some(w) = web_sys::window() {
        let _ = w.set_timeout_with_callback_and_timeout_and_arguments_0(revoke.unchecked_ref(), 60_000);
    }
    Ok(())
}

/// Engine file access (projects): reads from the table; writes go to the table (persisted) and
/// download, except [`save_quietly`] saves.
pub struct WebServices;

impl Services for WebServices {
    fn read_file(&self, path: &str) -> std::io::Result<Vec<u8>> {
        get(path)
            .map(|d| d.to_vec())
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, format!("{path}: open it with File ▸ Open (or drop it on the page)")))
    }
    fn write_file(&self, path: &str, data: &[u8]) -> std::io::Result<()> {
        put(path, data.into());
        let quiet = {
            let mut q = QUIET.lock().unwrap_or_else(|e| e.into_inner());
            let n = q.len();
            q.retain(|p| p != path);
            q.len() != n
        };
        if quiet || crate::is_worker() {
            return Ok(());
        }
        download(file_name(path), data).map_err(|e| std::io::Error::other(format!("download failed: {e:?}")))
    }
    fn store_file(&self, path: &str, data: &[u8]) -> std::io::Result<()> {
        put(path, data.into());
        // A job worker's files (Content-Aware Fill) go to the page's storage.
        if crate::is_worker() {
            crate::worker::post_store(path, data);
        }
        Ok(())
    }
    /// The table's files in `dir` (picked and dropped files sit in `/`): picking or dropping a
    /// numbered file finds the other frames of its image sequence loaded before.
    fn list_dir(&self, dir: &str) -> Vec<String> {
        let dir = dir.trim_end_matches('/');
        list().into_iter().map(|(p, _)| p).filter(|p| p.rsplit_once('/').is_some_and(|(d, _)| d == dir)).collect()
    }
    fn is_dir(&self, _path: &str) -> bool {
        false
    }
}

/// Media import from the table (the bytes are also handed to the media pool, which decodes them).
pub struct WebImporter {
    pub pool: Arc<effectcraft_media::MediaPool>,
}

impl effectcraft_engine::Importer for WebImporter {
    fn probe(&self, path: &str) -> Result<Footage, String> {
        let bytes = get(path).ok_or_else(|| format!("{path}: not loaded (import it with File ▸ Import or drop it on the page)"))?;
        self.pool.add_bytes(path, bytes.clone());
        effectcraft_media::probe_bytes(path, bytes).map_err(|e| e.to_string())
    }
    fn register(&self, path: &str, data: &[u8]) {
        self.pool.add_bytes(path, get(path).unwrap_or_else(|| data.into()));
    }
    fn add_frames(&self, paths: &[String]) {
        for p in paths {
            if let Some(bytes) = get(p) {
                self.pool.add_bytes(p, bytes);
            }
        }
    }
}

/// Render Queue output: files land in the table and are downloaded after the render
/// ([`flush_downloads`]; several files, e.g. an image sequence, as one `.zip`).
pub fn export_sink() -> Arc<effectcraft_export::Sink> {
    Arc::new(|path: &str, data: Vec<u8>| add_output(path, data.into()))
}

/// Download the render outputs written since the last call.
pub fn flush_downloads() {
    let paths = std::mem::take(&mut *OUTPUTS.lock().unwrap_or_else(|e| e.into_inner()));
    let mut out: Vec<(String, Arc<[u8]>)> = paths.into_iter().filter_map(|p| get(&p).map(|d| (file_name(&p).to_string(), d))).collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out.dedup_by(|a, b| a.0 == b.0);
    let r = match out.len() {
        0 => return,
        1 => download(&out[0].0, &out[0].1),
        n => {
            let first = &out[0].0;
            let stem = first.rsplit_once('.').map_or(first.as_str(), |(s, _)| s).trim_end_matches(|c: char| c.is_ascii_digit() || c == '_');
            let stem = if stem.is_empty() { "render" } else { stem };
            log::info!("render: {n} files → {stem}.zip");
            download(&format!("{stem}.zip"), &zip_store(&out))
        }
    };
    if let Err(e) = r {
        log::warn!("download failed: {e:?}");
    }
}

/// A `.zip` with the files stored uncompressed (PKWARE APPNOTE 6.3: local headers, central
/// directory, end record; no data descriptors, no ZIP64).
pub fn zip_store(entries: &[(String, Arc<[u8]>)]) -> Vec<u8> {
    fn crc32(d: &[u8]) -> u32 {
        let mut c = !0u32;
        for &b in d {
            c ^= b as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            }
        }
        !c
    }
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in entries {
        let (crc, n, off) = (crc32(data), data.len() as u32, out.len() as u32);
        let common = |v: &mut Vec<u8>| {
            v.extend_from_slice(&20u16.to_le_bytes()); // version needed
            v.extend_from_slice(&0x0800u16.to_le_bytes()); // flags: UTF-8 names
            v.extend_from_slice(&0u16.to_le_bytes()); // stored
            v.extend_from_slice(&0u16.to_le_bytes()); // time
            v.extend_from_slice(&0x21u16.to_le_bytes()); // date: 1980-01-01
            v.extend_from_slice(&crc.to_le_bytes());
            v.extend_from_slice(&n.to_le_bytes());
            v.extend_from_slice(&n.to_le_bytes());
            v.extend_from_slice(&(name.len() as u16).to_le_bytes());
            v.extend_from_slice(&0u16.to_le_bytes()); // extra
        };
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        common(&mut out);
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);
        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes()); // version made by
        common(&mut central);
        central.extend_from_slice(&[0; 6]); // comment len, disk, internal attrs
        central.extend_from_slice(&0u32.to_le_bytes()); // external attrs
        central.extend_from_slice(&off.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }
    let (cd_off, cd_len, count) = (out.len() as u32, central.len() as u32, entries.len() as u16);
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&[0; 4]); // disk numbers
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&cd_len.to_le_bytes());
    out.extend_from_slice(&cd_off.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

/// Show the browser's file picker; `then` gets the picked files' table paths once they are read.
pub fn pick(accept: &str, multiple: bool, then: impl FnOnce(Vec<String>) + 'static) -> Result<(), JsValue> {
    let doc = web_sys::window().and_then(|w| w.document()).ok_or("no document")?;
    let input: web_sys::HtmlInputElement = doc.create_element("input")?.dyn_into()?;
    input.set_type("file");
    input.set_accept(accept);
    input.set_multiple(multiple);
    let inp = input.clone();
    let on_change = Closure::once_into_js(move || {
        let Some(list) = inp.files() else { return };
        read_files((0..list.length()).filter_map(|i| list.get(i)).collect(), then);
    });
    input.add_event_listener_with_callback("change", on_change.unchecked_ref())?;
    input.click();
    Ok(())
}

/// Read browser files into the table (asynchronously); `then` gets their table paths.
pub fn read_files(picked: Vec<web_sys::File>, then: impl FnOnce(Vec<String>) + 'static) {
    wasm_bindgen_futures::spawn_local(async move {
        let mut paths = Vec::new();
        for f in picked {
            match wasm_bindgen_futures::JsFuture::from(f.array_buffer()).await {
                Ok(buf) => {
                    let path = path_for(&f.name());
                    put(&path, js_sys::Uint8Array::new(&buf).to_vec().into());
                    paths.push(path);
                }
                Err(e) => log::warn!("cannot read {}: {e:?}", f.name()),
            }
        }
        then(paths);
    });
}
