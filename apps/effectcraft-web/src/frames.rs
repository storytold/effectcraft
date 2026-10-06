//! Viewer frames rendered in Web Workers ([`effectcraft_engine::remote`]).
//!
//! **Page side** — [`WebFrames`], the frame cache's [`RemoteFrames`]: a few long-lived frame
//! workers (`js/host.js` `frameWorkerStart`), each holding a replica of the project. Before a
//! render, the worker gets the footage files it lacks and a sync (the whole project the first
//! time, then diffs: [`Mirror`]); the frame comes back as premultiplied RGBA8 in a transferred
//! buffer. One frame per worker at a time, so the page always decides what renders next (the
//! viewer's frame before prefetch). A worker that fails is dropped; with none left, frames render
//! on the page's thread again (`Frames::pump`). With the disk cache on, a frame whose key is
//! cached is read from the Origin Private File System instead ([`crate::diskcache`]); the
//! others are stored there by the worker that renders them.
//!
//! **Worker side** — [`worker_frame_init`] / [`worker_frame`]: the worker's [`FrameServer`]
//! (footage from the worker's media pool, expressions) handles each message and posts its
//! replies. Where the browser has WebGPU in workers, the worker opens its own device
//! ([`Gpu::request_deferred`]) and renders GPU effects and the GPU compositor on it: a frame
//! takes several passes, awaiting the device's readbacks in between (they can't be waited for
//! synchronously, even in a worker), and Backend Auto picks CPU or GPU per comp from measured
//! frame times. Without WebGPU, frames render on the worker's CPU.

use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::Arc;

use effectcraft_engine::offload::footage_files;
use effectcraft_engine::remote::{FrameMsg, FrameReply, FrameServer, Mirror};
use effectcraft_gpu::Gpu;
use effectcraft_ui_egui::frames::{RemoteDone, RemoteFrame, RemoteFrames, RemoteJob};
use serde_json::{Value, json};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

#[wasm_bindgen(module = "/js/host.js")]
extern "C" {
    #[wasm_bindgen(js_name = frameWorkerStart)]
    fn frame_worker_start(base: &str, on_message: &js_sys::Function, gpu: bool) -> u32;
    #[wasm_bindgen(js_name = frameWorkerPost)]
    fn frame_worker_post(index: u32, msg: &JsValue, transfer: &js_sys::Array);
    #[wasm_bindgen(js_name = hasWebGpu)]
    pub(crate) fn has_web_gpu() -> bool;
}

/// One frame worker as the page sees it.
struct Fw {
    index: u32,
    mirror: Mirror,
    files: HashSet<String>,
    busy: Option<(u64, RemoteDone)>,
    alive: bool,
    /// Its GPU adapter (after it reported), or why it has none.
    gpu: Option<Result<String, String>>,
}

#[derive(Default)]
struct State {
    workers: Vec<Fw>,
    next_id: u64,
    rendered: u64,
    failed: u64,
    last_ms: f64,
    /// Bytes of project sync messages sent (diffs keep this small).
    sync_bytes: u64,
    syncs: u64,
    /// Frames rendered through a worker's GPU, and the passes of the last one.
    gpu_frames: u64,
    last_passes: u32,
    /// Frames served from the disk cache.
    disk_hits: u64,
}

type OnMessage = Closure<dyn FnMut(u32, JsValue, JsValue, JsValue)>;

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
    static HANDLER: RefCell<Option<OnMessage>> = const { RefCell::new(None) };
    /// The worker's frame server (worker side).
    static SERVER: RefCell<Option<FrameServer>> = const { RefCell::new(None) };
    /// The worker's GPU device (worker side).
    static GPU: RefCell<Option<Gpu>> = const { RefCell::new(None) };
}

fn obj(pairs: &[(&str, JsValue)]) -> JsValue {
    let o = js_sys::Object::new();
    for (k, v) in pairs {
        let _ = js_sys::Reflect::set(&o, &JsValue::from_str(k), v);
    }
    o.into()
}

/// The page's frame workers (see the module docs).
pub struct WebFrames;

impl WebFrames {
    /// Start `n` frame workers; `gpu`: each opens its own WebGPU device where it can.
    pub fn start(n: usize, gpu: bool) -> WebFrames {
        let on: OnMessage = Closure::new(|index: u32, json: JsValue, bytes: JsValue, error: JsValue| on_message(index, json, bytes, error));
        STATE.with(|s| {
            let mut s = s.borrow_mut();
            for _ in 0..n {
                let index = frame_worker_start(&crate::page_url(""), on.as_ref().unchecked_ref(), gpu);
                s.workers.push(Fw { index, mirror: Mirror::default(), files: HashSet::new(), busy: None, alive: true, gpu: None });
            }
        });
        HANDLER.with(|h| *h.borrow_mut() = Some(on));
        WebFrames
    }

    /// Send `job` to an idle worker.
    fn render(&self, job: RemoteJob, done: RemoteDone) {
        let r = STATE.with(|s| {
            let mut s = s.borrow_mut();
            s.next_id += 1;
            let id = s.next_id;
            let Some(k) = s.workers.iter().position(|w| w.alive && w.busy.is_none()) else { return Err((done, "no idle frame worker".to_string())) };
            // Footage the worker hasn't got yet.
            let w = &mut s.workers[k];
            for p in footage_files(&job.project) {
                if w.files.contains(&p) {
                    continue;
                }
                if let Some(d) = crate::files::get(&p) {
                    let bytes = js_sys::Uint8Array::from(&d[..]);
                    frame_worker_post(w.index, &obj(&[("type", "file".into()), ("path", p.as_str().into()), ("bytes", bytes.into())]), &js_sys::Array::new());
                    w.files.insert(p);
                }
            }
            let sync = w.mirror.sync(job.revision, &job.project);
            let index = w.index;
            let mut sent = 0;
            if let Some(m) = sync {
                let t = serde_json::to_string(&m).unwrap_or_default();
                sent = t.len() as u64;
                frame_worker_post(index, &obj(&[("type", "frame".into()), ("json", t.into())]), &js_sys::Array::new());
            }
            let disk = job.disk_key.map(|k| format!("{k:032x}"));
            // Layer buffers from the disk cache: what to read before rendering.
            let layers = crate::diskcache::active();
            let prefetch = crate::diskcache::layer_prefetch(64);
            let req = FrameMsg::Render { id, revision: job.revision, comp: job.comp, time: job.t, opts: Box::new(job.opts), disk, layers, prefetch };
            frame_worker_post(
                index,
                &obj(&[("type", "frame".into()), ("json", serde_json::to_string(&req).unwrap_or_default().into())]),
                &js_sys::Array::new(),
            );
            s.workers[k].busy = Some((id, done));
            if sent > 0 {
                s.syncs += 1;
                s.sync_bytes += sent;
            }
            Ok(())
        });
        if let Err((done, e)) = r {
            done(Err(e));
        }
    }
}

impl RemoteFrames for WebFrames {
    fn slots(&self) -> usize {
        STATE.with(|s| s.borrow().workers.iter().filter(|w| w.alive).count())
    }

    fn purge(&self) {
        purge();
    }

    fn gpu(&self) -> bool {
        STATE.with(|s| s.borrow().workers.iter().any(|w| w.alive && matches!(w.gpu, Some(Ok(_)))))
    }

    fn disk(&self) -> bool {
        crate::diskcache::active()
    }

    fn disk_contains(&self, key: u128) -> bool {
        crate::diskcache::contains(key)
    }

    fn start(&self, job: RemoteJob, done: RemoteDone) {
        // A frame in the disk cache is read, not rendered (a failed read renders it).
        if let Some(k) = job.disk_key {
            if crate::diskcache::contains(k) {
                wasm_bindgen_futures::spawn_local(async move {
                    match crate::diskcache::read(k).await {
                        Some(f) => {
                            STATE.with(|s| s.borrow_mut().disk_hits += 1);
                            done(Ok(RemoteFrame { width: f.width, height: f.height, rgba: f.rgba, ms: 0.0 }));
                        }
                        None => WebFrames.render(job, done),
                    }
                    crate::repaint();
                });
                return;
            }
            crate::diskcache::note_miss();
        }
        self.render(job, done);
    }
}

/// A message from frame worker `index`: a JSON [`FrameReply`] (with the pixels in `bytes`), or
/// `error` when the worker died.
fn on_message(index: u32, json: JsValue, bytes: JsValue, error: JsValue) {
    let done = STATE.with(|s| {
        let mut s = s.borrow_mut();
        let st = &mut *s;
        let w = st.workers.iter_mut().find(|w| w.index == index)?;
        if let Some(e) = error.as_string() {
            log::warn!("frame worker {index}: {e}; frames render on the page's thread if none is left");
            w.alive = false;
            return w.busy.take().map(|(_, d)| (d, Err(e)));
        }
        let reply: FrameReply = serde_json::from_str(&json.as_string()?).ok()?;
        match reply {
            FrameReply::Frame { id, width, height, ms, passes, gpu, layer_misses, layer_hits } => {
                crate::diskcache::note_layers(&layer_misses, &layer_hits);
                let (_, d) = w.busy.take_if(|b| b.0 == id)?;
                let rgba = bytes.dyn_into::<js_sys::Uint8Array>().map(|b| b.to_vec()).unwrap_or_default();
                st.rendered += 1;
                st.last_ms = ms;
                if gpu {
                    st.gpu_frames += 1;
                    st.last_passes = passes;
                }
                Some((d, Ok(RemoteFrame { width, height, rgba, ms })))
            }
            FrameReply::Failed { id, error, .. } => {
                st.failed += 1;
                let (_, d) = w.busy.take_if(|b| b.0 == id)?;
                Some((d, Err(error)))
            }
            FrameReply::Resync { error } => {
                log::info!("frame worker {index}: {error}: sending the whole project again");
                w.mirror.reset();
                None
            }
            FrameReply::Stored { key, bytes, layer } => {
                if let Ok(k) = u128::from_str_radix(&key, 16) {
                    crate::diskcache::stored(k, bytes, layer);
                }
                None
            }
            FrameReply::Info { gpu, error } => {
                match (&gpu, &error) {
                    (Some(g), _) => log::info!("frame worker {index}: GPU {g}"),
                    (None, e) => log::info!("frame worker {index}: CPU only ({})", e.as_deref().unwrap_or("no GPU")),
                }
                w.gpu = Some(gpu.ok_or_else(|| error.unwrap_or_default()));
                None
            }
        }
    });
    if let Some((d, r)) = done {
        d(r);
    }
    crate::repaint();
}

/// Relay Roto Brush segmentations (from a propagation worker) to the frame workers, so their
/// renders use them instead of computing them again.
pub fn relay_segs(segs: &[effectcraft_engine::offload::SegData]) {
    if segs.is_empty() {
        return;
    }
    let t = serde_json::to_string(&FrameMsg::Segs { segs: segs.to_vec() }).unwrap_or_default();
    STATE.with(|s| {
        for w in s.borrow().workers.iter().filter(|w| w.alive) {
            frame_worker_post(w.index, &obj(&[("type", "frame".into()), ("json", t.as_str().into())]), &js_sys::Array::new());
        }
    });
}

/// Ask every frame worker to drop its cached layer buffers (Edit ▸ Purge).
pub fn purge() {
    let t = serde_json::to_string(&FrameMsg::Purge).unwrap_or_default();
    STATE.with(|s| {
        for w in s.borrow().workers.iter().filter(|w| w.alive) {
            frame_worker_post(w.index, &obj(&[("type", "frame".into()), ("json", t.as_str().into())]), &js_sys::Array::new());
        }
    });
}

/// `effectcraft.info().frameWorkers`.
pub fn stats() -> Value {
    STATE.with(|s| {
        let s = s.borrow();
        let gpu: Vec<Value> = s
            .workers
            .iter()
            .map(|w| match &w.gpu {
                Some(Ok(name)) => json!(name),
                Some(Err(e)) => json!({"cpu": e}),
                None => Value::Null,
            })
            .collect();
        json!({
            "workers": s.workers.len(),
            "alive": s.workers.iter().filter(|w| w.alive).count(),
            "busy": s.workers.iter().filter(|w| w.busy.is_some()).count(),
            "rendered": s.rendered,
            "failed": s.failed,
            "lastMs": s.last_ms,
            "syncs": s.syncs,
            "syncBytes": s.sync_bytes,
            "gpu": gpu,
            "gpuFrames": s.gpu_frames,
            "lastPasses": s.last_passes,
            "diskHits": s.disk_hits,
        })
    })
}

thread_local! {
    /// `workerFrameCheck` promises waiting for their frame: id → (resolve, reject, local frame).
    #[allow(clippy::type_complexity)]
    static CHECKS: RefCell<std::collections::HashMap<u64, (js_sys::Function, js_sys::Function, (u32, u32, Vec<u8>), f64)>> = RefCell::new(Default::default());
}

/// `effectcraft.workerFrameCheck({time?, scale?, backend?})`: render the active comp's frame in
/// a frame worker (backend `gpu` by default: the worker's WebGPU device) and on the page's CPU
/// with the same render options, and compare them. Resolves with `{width, height, maxDiff,
/// meanDiff, over4, workerMs, gpuFrames}` (differences in 8-bit levels; `over4`: the fraction
/// of channels more than 4 levels apart). Rejects when no worker is idle (try again).
#[wasm_bindgen(js_name = workerFrameCheck)]
pub fn worker_frame_check(params: JsValue) -> js_sys::Promise {
    let get = |k: &str| js_sys::Reflect::get(&params, &k.into()).ok().filter(|v| !v.is_undefined());
    let time = get("time").and_then(|v| v.as_f64());
    let scale = get("scale").and_then(|v| v.as_f64()).unwrap_or(0.5);
    let backend = get("backend").and_then(|v| v.as_string()).unwrap_or_else(|| "gpu".into());
    js_sys::Promise::new(&mut |resolve, reject| {
        let backend = backend.clone();
        crate::post(move |app| {
            let s = &app.session;
            let Some(comp) = s.active_comp_id() else {
                let _ = reject.call1(&JsValue::NULL, &"no active composition".into());
                return;
            };
            let t = time.map(effectcraft_engine::time::Tick::from_seconds_f64).unwrap_or_else(|| s.time());
            let mut opts = app.frame_opts(comp, scale);
            opts.backend = match backend.as_str() {
                "cpu" => effectcraft_engine::render::Backend::Cpu,
                "auto" => effectcraft_engine::render::Backend::Auto,
                _ => effectcraft_engine::render::Backend::Gpu,
            };
            let local = {
                let mut r = effectcraft_engine::render::Renderer::new(
                    &s.project,
                    s.footage.as_ref(),
                    effectcraft_engine::render::RenderOpts { backend: effectcraft_engine::render::Backend::Cpu, ..opts },
                );
                r.expr = s.expr.as_deref();
                let img = r.comp_frame(comp, t);
                (img.width, img.height, effectcraft_engine::remote::rgba8_premultiplied(&img))
            };
            let id = STATE.with(|st| {
                let mut st = st.borrow_mut();
                st.next_id += 1;
                st.next_id
            });
            CHECKS.with(|c| c.borrow_mut().insert(id, (resolve.clone(), reject.clone(), local, js_sys::Date::now())));
            let job = RemoteJob { project: s.project.clone(), revision: s.revision, comp, t, opts, disk_key: None };
            WebFrames.render(
                job,
                Box::new(move |r: Result<RemoteFrame, String>| {
                    let Some((resolve, reject, (w, h, want), t0)) = CHECKS.with(|c| c.borrow_mut().remove(&id)) else { return };
                    match r {
                        Ok(f) if (f.width, f.height) == (w, h) && f.rgba.len() == want.len() => {
                            let (mut max, mut sum, mut over) = (0u32, 0u64, 0u64);
                            for (a, b) in f.rgba.iter().zip(&want) {
                                let d = (*a as i32 - *b as i32).unsigned_abs();
                                max = max.max(d);
                                sum += d as u64;
                                over += (d > 4) as u64;
                            }
                            let n = want.len().max(1) as f64;
                            let c = ((h / 2 * w + w / 2) * 4) as usize;
                            let v = json!({"width": w, "height": h, "maxDiff": max, "meanDiff": sum as f64 / n, "over4": over as f64 / n,
                                           "workerMs": f.ms, "wallMs": js_sys::Date::now() - t0, "gpuFrames": stats()["gpuFrames"],
                                           "center": [&f.rgba[c..c + 4], &want[c..c + 4]]});
                            let _ = resolve.call1(&JsValue::NULL, &JsValue::from_str(&v.to_string()));
                        }
                        Ok(f) => {
                            let _ = reject.call1(&JsValue::NULL, &format!("size {}x{} vs {w}x{h}", f.width, f.height).into());
                        }
                        Err(e) => {
                            let _ = reject.call1(&JsValue::NULL, &e.into());
                        }
                    }
                }),
            );
        });
    })
}

// ---------------------------------------------------------------- worker side

fn post_reply(r: &FrameReply, px: Option<&[u8]>) {
    let scope: web_sys::DedicatedWorkerGlobalScope = js_sys::global().unchecked_into();
    let t = serde_json::to_string(r).unwrap_or_default();
    let res = match px {
        Some(px) => {
            let buf = js_sys::Uint8Array::from(px).buffer();
            scope.post_message_with_transfer(
                &obj(&[("type", "frameReply".into()), ("json", t.into()), ("bytes", buf.clone().into())]),
                &js_sys::Array::of1(&buf),
            )
        }
        None => scope.post_message(&obj(&[("type", "frameReply".into()), ("json", t.into())])),
    };
    if let Err(e) = res {
        log::warn!("frame worker: postMessage: {e:?}");
    }
}

fn new_server() -> FrameServer {
    let mut s = FrameServer::new(crate::worker::pool(), Some(Arc::new(effectcraft_expr::Expressions)));
    s.set_layer_store(Some(Arc::new(effectcraft_engine::render::PrefetchStore::default())));
    s
}

/// Set up a frame worker (called once by `web/worker.js` after `workerInit`): its frame
/// server and, with `gpu` and WebGPU in workers, its own GPU device. Reports
/// [`FrameReply::Info`].
#[wasm_bindgen(js_name = workerFrameInit)]
pub async fn worker_frame_init(gpu: bool) {
    let mut server = new_server();
    let info = if !gpu {
        FrameReply::Info { gpu: None, error: Some("GPU frame workers are off (?nogpuworkers)".into()) }
    } else if !has_web_gpu() {
        FrameReply::Info { gpu: None, error: Some("no WebGPU in workers".into()) }
    } else {
        match Gpu::request_deferred().await {
            Ok(g) => {
                let name = effectcraft_engine::render::Accelerator::name(&g);
                server.set_accel(Some(Arc::new(g.clone())));
                GPU.with(|c| *c.borrow_mut() = Some(g));
                FrameReply::Info { gpu: Some(name), error: None }
            }
            Err(e) => FrameReply::Info { gpu: None, error: Some(e) },
        }
    };
    SERVER.with(|c| *c.borrow_mut() = Some(server));
    post_reply(&info, None);
}

/// Handle one frame message (a JSON [`FrameMsg`]) in a frame worker: replies are posted with
/// their pixels transferred. A frame rendering in passes awaits the GPU between them; a frame
/// with a disk-cache key is then stored ([`FrameReply::Stored`]).
#[wasm_bindgen(js_name = workerFrame)]
pub async fn worker_frame(json: String) {
    let msg: FrameMsg = match serde_json::from_str(&json) {
        Ok(m) => m,
        Err(e) => {
            log::warn!("frame worker: bad message: {e}");
            return;
        }
    };
    let (disk, layers) = match &msg {
        FrameMsg::Render { disk, layers, prefetch, .. } => {
            if *layers {
                prefetch_layers(prefetch).await;
            }
            (disk.clone(), *layers)
        }
        _ => (None, false),
    };
    let mut replies = SERVER.with(|c| c.borrow_mut().get_or_insert_with(new_server).handle(msg));
    while SERVER.with(|c| c.borrow().as_ref().is_some_and(|s| s.waiting())) {
        let Some(g) = GPU.with(|c| c.borrow().clone()) else { break };
        g.settled().await;
        replies = SERVER.with(|c| c.borrow_mut().as_mut().map(|s| s.resume()).unwrap_or_default());
    }
    for (r, px) in replies {
        post_reply(&r, px.as_deref());
        if let (FrameReply::Frame { width, height, .. }, Some(px), Some(key)) = (&r, &px, &disk)
            && effectcraft_engine::render::disk_cache::frame_cacheable(*width, *height, px)
        {
            let entry = effectcraft_engine::render::disk_cache::frame_entry(*width, *height, px);
            let name = format!("{key}.ecc");
            match JsFuture::from(crate::diskcache::cache_write(&name, js_sys::Uint8Array::from(&entry[..]))).await {
                Ok(n) => post_reply(&FrameReply::Stored { key: key.clone(), bytes: n.as_f64().unwrap_or(entry.len() as f64) as u64, layer: false }, None),
                Err(e) => log::warn!("frame worker: disk cache write failed: {e:?}"),
            }
        }
    }
    if layers {
        store_layers().await;
    }
}

/// Read the layer entries the page named into the server's store (those it lacks).
async fn prefetch_layers(keys: &[String]) {
    let Some(store) = SERVER.with(|c| c.borrow().as_ref().and_then(|s| s.layer_store().cloned())) else { return };
    for k in keys {
        let Ok(key) = u128::from_str_radix(k, 16) else { continue };
        if store.has(key) {
            continue;
        }
        let name = crate::diskcache::name_of(effectcraft_engine::render::disk_cache::Kind::Layer, key);
        if let Ok(b) = JsFuture::from(crate::diskcache::cache_read(&name)).await
            && let Ok(b) = b.dyn_into::<js_sys::Uint8Array>()
        {
            store.provide(key, &b.to_vec());
        }
    }
}

/// Write the slow layer buffers of the frame just rendered and report them.
async fn store_layers() {
    let Some(store) = SERVER.with(|c| c.borrow().as_ref().and_then(|s| s.layer_store().cloned())) else { return };
    for (key, entry) in store.take_writes() {
        let name = crate::diskcache::name_of(effectcraft_engine::render::disk_cache::Kind::Layer, key);
        match JsFuture::from(crate::diskcache::cache_write(&name, js_sys::Uint8Array::from(&entry[..]))).await {
            Ok(n) => post_reply(&FrameReply::Stored { key: format!("{key:032x}"), bytes: n.as_f64().unwrap_or(entry.len() as f64) as u64, layer: true }, None),
            Err(e) => log::warn!("frame worker: layer cache write failed: {e:?}"),
        }
    }
}
