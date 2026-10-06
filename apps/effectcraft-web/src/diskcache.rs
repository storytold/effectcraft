//! The disk cache (Settings ▸ Disk ▸ Disk Cache) in the browser: viewer frames kept in the
//! Origin Private File System between visits, with the size limit from the settings and
//! least-recently-used eviction.
//!
//! - **Writes** happen in the frame workers: a render request carries the frame's disk-cache key
//!   (`effectcraft_engine::remote::FrameMsg::Render`), the worker posts the frame, then writes
//!   the sealed entry (`disk_cache::frame_entry`: LZ4, checksum) with a synchronous access
//!   handle into a temporary file moved into place (`js/host.js` `cacheWrite`), and reports
//!   `FrameReply::Stored`.
//! - **The index** lives here, on the page ([`DiskIndex`], shared with the desktop's cache):
//!   rebuilt at startup from the folder listing (oldest files first), updated by the workers'
//!   `Stored` replies and by reads; eviction deletes the least recently used files.
//! - **Reads** are asynchronous: a frame whose key is indexed is read and decoded here instead
//!   of rendering it (`crate::frames`); a damaged or missing file is a miss and is forgotten.
//!
//! - **Layer buffers** too (`layers/<32 hex>.ecc`): a layer lookup happens in the middle of a
//!   synchronous render, where the browser's file system can't be read, so the frame workers
//!   back their layer caches with a `PrefetchStore`. They report each frame's layer-cache
//!   misses; before the next request the page names the recent misses that are on disk
//!   ([`layer_prefetch`], `disk_cache::LayerPrefetch`), and the worker reads those entries
//!   into memory before rendering, so the lookups find them. Slow buffers are written by the
//!   worker after its frame and indexed here like frames (one least-recently-used order for
//!   both kinds).

use std::cell::RefCell;

use effectcraft_engine::render::disk_cache::{self, DiskIndex, Kind, LayerPrefetch};
use serde_json::{Value, json};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

#[wasm_bindgen(module = "/js/host.js")]
extern "C" {
    #[wasm_bindgen(js_name = cacheList)]
    fn cache_list() -> js_sys::Promise;
    #[wasm_bindgen(js_name = cacheRead)]
    fn cache_read_limited(name: &str, max_bytes: usize) -> js_sys::Promise;
    #[wasm_bindgen(js_name = cacheWrite)]
    pub(crate) fn cache_write(name: &str, bytes: js_sys::Uint8Array) -> js_sys::Promise;
    #[wasm_bindgen(js_name = cacheDelete)]
    fn cache_delete(names: js_sys::Array) -> js_sys::Promise;
    #[wasm_bindgen(js_name = cacheClear)]
    fn cache_clear() -> js_sys::Promise;
}

/// Reject oversized OPFS files before JavaScript allocates their bytes or Rust copies them.
pub(crate) fn cache_read(name: &str) -> js_sys::Promise {
    cache_read_limited(name, disk_cache::MAX_CACHE_FILE_BYTES)
}

#[derive(Default)]
struct State {
    index: DiskIndex,
    /// The folder was listed (lookups before then miss).
    loaded: bool,
    /// The Origin Private File System works here.
    available: bool,
    /// Settings ▸ Disk ▸ Enable Disk Cache.
    enabled: bool,
    max_bytes: u64,
    hits: u64,
    misses: u64,
    writes: u64,
    evictions: u64,
    error: Option<String>,
    /// Recent layer-cache misses of the frame workers (what to prefetch).
    prefetch: LayerPrefetch,
    layer_hits: u64,
    layer_writes: u64,
}

thread_local! {
    static ST: RefCell<State> = RefCell::new(State { enabled: true, max_bytes: 1 << 30, ..Default::default() });
}

fn js_err(e: JsValue) -> String {
    e.as_string().or_else(|| js_sys::Reflect::get(&e, &"message".into()).ok().and_then(|m| m.as_string())).unwrap_or_else(|| format!("{e:?}"))
}

/// List the cache folder and build the index (in the background).
pub fn init() {
    wasm_bindgen_futures::spawn_local(async {
        match JsFuture::from(cache_list()).await {
            Ok(list) => {
                let mut entries: Vec<(f64, Kind, u128, u64)> = js_sys::Array::from(&list)
                    .iter()
                    .filter_map(|e| {
                        let get = |k: &str| js_sys::Reflect::get(&e, &k.into()).ok();
                        let (kind, key) = parse_name(&get("name")?.as_string()?)?;
                        Some((get("modified")?.as_f64().unwrap_or(0.0), kind, key, get("size")?.as_f64().unwrap_or(0.0) as u64))
                    })
                    .collect();
                entries.sort_by(|a, b| a.0.total_cmp(&b.0));
                ST.with(|s| {
                    let mut s = s.borrow_mut();
                    for (_, kind, key, bytes) in entries {
                        s.index.insert(kind, key, bytes);
                    }
                    s.loaded = true;
                    s.available = true;
                });
                evict();
            }
            Err(e) => {
                let e = js_err(e);
                log::info!("disk cache unavailable: {e}");
                ST.with(|s| s.borrow_mut().error = Some(e));
            }
        }
    });
}

/// An entry's name in `js/host.js`'s cache folders (`layers/` for layer buffers).
pub(crate) fn name_of(kind: Kind, key: u128) -> String {
    match kind {
        Kind::Frame => disk_cache::entry_name(key),
        Kind::Layer => format!("layers/{}", disk_cache::entry_name(key)),
    }
}

fn parse_name(name: &str) -> Option<(Kind, u128)> {
    match name.strip_prefix("layers/") {
        Some(n) => Some((Kind::Layer, disk_cache::parse_entry_name(n)?)),
        None => Some((Kind::Frame, disk_cache::parse_entry_name(name)?)),
    }
}

/// Layer entries (32 hex digits) a frame worker should read before its next frame: recent
/// misses that are on disk.
pub fn layer_prefetch(max: usize) -> Vec<String> {
    if !active() {
        return vec![];
    }
    ST.with(|s| {
        let s = s.borrow();
        s.prefetch.plan(&s.index, max).into_iter().map(|k| format!("{k:032x}")).collect()
    })
}

/// A frame worker's layer-cache misses and the buffers it served from what it fetched.
pub fn note_layers(misses: &[String], hits: &[String]) {
    let parse = |v: &[String]| v.iter().filter_map(|k| u128::from_str_radix(k, 16).ok()).collect::<Vec<u128>>();
    ST.with(|s| {
        let mut s = s.borrow_mut();
        s.prefetch.note_misses(parse(misses));
        for k in parse(hits) {
            s.index.touch(Kind::Layer, k);
            s.layer_hits += 1;
        }
    });
}

/// Apply the settings: on/off and the limit (`max_gb`, at most half the origin's quota when it
/// is known).
pub fn configure(enabled: bool, max_gb: u32, quota: Option<u64>) {
    let mut max = (max_gb.max(1) as u64) << 30;
    if let Some(q) = quota.filter(|q| *q > 0) {
        max = max.min(q / 2);
    }
    let changed = ST.with(|s| {
        let mut s = s.borrow_mut();
        let changed = s.enabled != enabled || s.max_bytes != max;
        s.enabled = enabled;
        s.max_bytes = max;
        changed
    });
    if changed {
        evict();
    }
}

/// The cache is usable: listed, available and enabled.
pub fn active() -> bool {
    ST.with(|s| {
        let s = s.borrow();
        s.loaded && s.available && s.enabled
    })
}

/// The frame of `key` is cached.
pub fn contains(key: u128) -> bool {
    active() && ST.with(|s| s.borrow().index.contains(Kind::Frame, key))
}

/// A lookup missed (counted for the statistics).
pub fn note_miss() {
    ST.with(|s| s.borrow_mut().misses += 1);
}

/// Read and decode a cached frame; `None` (forgotten and deleted) when the file is missing or
/// damaged.
pub async fn read(key: u128) -> Option<disk_cache::Frame8> {
    let name = disk_cache::entry_name(key);
    let bytes = JsFuture::from(cache_read(&name)).await.ok().and_then(|b| b.dyn_into::<js_sys::Uint8Array>().ok()).map(|b| b.to_vec());
    let frame = bytes.as_deref().and_then(disk_cache::read_frame_entry);
    ST.with(|s| {
        let mut s = s.borrow_mut();
        if frame.is_some() {
            s.hits += 1;
            s.index.touch(Kind::Frame, key);
        } else {
            s.misses += 1;
            s.index.remove(Kind::Frame, key);
        }
    });
    if frame.is_none() && bytes.is_some() {
        delete(vec![name]);
    }
    frame
}

/// A worker stored the frame (or, with `layer`, the layer buffer) of `key` (`bytes` on disk).
pub fn stored(key: u128, bytes: u64, layer: bool) {
    ST.with(|s| {
        let mut s = s.borrow_mut();
        s.index.insert(if layer { Kind::Layer } else { Kind::Frame }, key, bytes);
        s.writes += 1;
        if layer {
            s.layer_writes += 1;
        }
    });
    evict();
}

fn evict() {
    let victims = ST.with(|s| {
        let mut s = s.borrow_mut();
        let max = if s.enabled { s.max_bytes } else { u64::MAX };
        let v = s.index.evict(max);
        s.evictions += v.len() as u64;
        v
    });
    if !victims.is_empty() {
        delete(victims.into_iter().map(|(kind, k)| name_of(kind, k)).collect());
    }
}

fn delete(names: Vec<String>) {
    let arr = js_sys::Array::new();
    for n in names {
        arr.push(&JsValue::from_str(&n));
    }
    wasm_bindgen_futures::spawn_local(async move {
        let _ = JsFuture::from(cache_delete(arr)).await;
    });
}

/// Delete every cached frame; returns (entries, bytes) removed.
pub fn clear() -> (u64, u64) {
    let r = ST.with(|s| {
        let mut s = s.borrow_mut();
        let r = (s.index.len() as u64, s.index.total());
        s.index.clear();
        s.prefetch.clear();
        r
    });
    wasm_bindgen_futures::spawn_local(async {
        if let Err(e) = JsFuture::from(cache_clear()).await {
            log::warn!("disk cache: clearing failed: {}", js_err(e));
        }
    });
    r
}

/// `cache.diskStats` / `storage.info().diskCache`.
pub fn stats() -> Value {
    ST.with(|s| {
        let s = s.borrow();
        json!({
            "enabled": s.enabled && s.available,
            "available": s.available,
            "loaded": s.loaded,
            "folder": "Origin Private File System: effectcraft-cache/v1 (frames, layers)",
            "entries": s.index.len(),
            "frames": s.index.keys(Kind::Frame).len(),
            "layers": s.index.keys(Kind::Layer).len(),
            "layerHits": s.layer_hits,
            "layerWrites": s.layer_writes,
            "bytes": s.index.total(),
            "maxBytes": s.max_bytes,
            "hits": s.hits,
            "misses": s.misses,
            "writes": s.writes,
            "evictions": s.evictions,
            "error": s.error,
        })
    })
}
