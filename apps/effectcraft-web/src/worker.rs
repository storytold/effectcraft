//! Renders and analyses in Web Workers ([`effectcraft_engine::offload`]).
//!
//! **Page side** — [`WorkerOffload`], the session's [`Offload`]: serializes the job, ships the
//! footage files it reads and the request to a worker from the host's pool (`js/host.js`
//! `workerRun`), and turns the worker's messages back into [`WorkerReply`]s in the job's
//! [`Inbox`]. Rendered files come back as their bytes and download like any render.
//!
//! Viewer frames use separate, long-lived workers ([`crate::frames`]); Roto Brush
//! segmentations a propagation job sends back are relayed to them.
//!
//! **Worker side** — `web/worker.js` instantiates the same wasm module (the page's compiled
//! `WebAssembly.Module`, so nothing is compiled twice) and calls [`worker_init`] and
//! [`worker_job_init`] (the worker's own WebGPU device, where workers have WebGPU), then
//! [`worker_file`] per footage file and [`worker_job`] per job: a plain engine session (footage
//! from memory, expressions, the exporter) runs it and posts replies as it goes. With a device,
//! the job's frames (Render Queue frames, analysis input frames, particles, Advanced 3D) render
//! on it in passes, the job awaiting the GPU's readbacks between them
//! ([`effectcraft_engine::offload::run_request_async`]). Each worker has its own memory: no
//! shared-memory threads, so this works on stable Rust.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use effectcraft_engine::Session;
use effectcraft_engine::offload::{Inbox, Offload, WorkerReply, WorkerRequest};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

#[wasm_bindgen(module = "/js/host.js")]
extern "C" {
    #[wasm_bindgen(js_name = workerRun)]
    fn worker_run(id: f64, json: &str, files: js_sys::Array, base: &str, on_message: &js_sys::Function);
    #[wasm_bindgen(js_name = workerCancel)]
    fn worker_cancel(id: f64);
    #[wasm_bindgen(js_name = workerStats)]
    pub fn stats() -> JsValue;
    #[wasm_bindgen(js_name = setJobWorkerGpu)]
    pub fn set_job_worker_gpu(on: bool);
}

type OnMessage = Closure<dyn FnMut(String, JsValue, JsValue, JsValue)>;

thread_local! {
    /// Message handlers of running jobs (dropped when the job ends).
    static HANDLERS: RefCell<HashMap<u64, OnMessage>> = RefCell::new(HashMap::new());
    /// The worker's session and its media pool.
    static SESSION: RefCell<Option<Session>> = const { RefCell::new(None) };
    static POOL: RefCell<Option<Arc<effectcraft_media::MediaPool>>> = const { RefCell::new(None) };
    /// The job worker's GPU device (deferred readbacks), when it has one.
    static GPU: RefCell<Option<effectcraft_gpu::Gpu>> = const { RefCell::new(None) };
}

/// The page's offload: one worker per running job (idle workers are reused).
pub struct WorkerOffload;

impl Offload for WorkerOffload {
    fn start(&self, req: WorkerRequest, inbox: Arc<Inbox>) -> Result<(), String> {
        let id = req.id;
        let files = js_sys::Array::new();
        for p in &req.files {
            if let Some((d, version)) = crate::files::get_versioned(p) {
                files.push(&js_sys::Array::of3(&JsValue::from_str(p), &js_sys::Uint8Array::from(&d[..]), &JsValue::from_f64(version as f64)));
            }
        }
        let json = serde_json::to_string(&req).map_err(|e| e.to_string())?;
        let on: OnMessage = Closure::new(move |kind: String, json: JsValue, path: JsValue, bytes: JsValue| {
            match kind.as_str() {
                "reply" => {
                    let Some(t) = json.as_string() else { return };
                    match serde_json::from_str::<WorkerReply>(&t) {
                        Ok(r) => {
                            if let WorkerReply::Segs { segs } = &r {
                                crate::frames::relay_segs(segs);
                            }
                            let done = matches!(r, WorkerReply::Done);
                            inbox.push(r);
                            if done {
                                end(id);
                            }
                        }
                        Err(e) => log::warn!("worker reply: {e}"),
                    }
                }
                "file" => {
                    if let (Some(p), Ok(b)) = (path.as_string(), bytes.dyn_into::<js_sys::Uint8Array>()) {
                        crate::files::add_output(&p, b.to_vec().into());
                    }
                }
                "store" => {
                    // A file the job keeps (Content-Aware Fill's sequence): browser storage.
                    if let (Some(p), Ok(b)) = (path.as_string(), bytes.dyn_into::<js_sys::Uint8Array>()) {
                        crate::files::put(&p, b.to_vec().into());
                    }
                }
                "unstore" => {
                    // A kept file the job no longer needs (a fill that failed partway).
                    if let Some(p) = path.as_string() {
                        crate::files::STORE.remove_file(&p);
                    }
                }
                _ => {
                    inbox.push(WorkerReply::Failed { error: json.as_string().unwrap_or_else(|| "worker failed".into()) });
                    inbox.push(WorkerReply::Done);
                    end(id);
                }
            }
            crate::repaint();
        });
        worker_run(id as f64, &json, files, &crate::page_url(""), on.as_ref().unchecked_ref());
        HANDLERS.with(|h| h.borrow_mut().insert(id, on));
        Ok(())
    }

    fn cancel(&self, id: u64) {
        worker_cancel(id as f64);
        end(id);
    }
}

/// Drop a job's handler (after the current call returns).
fn end(id: u64) {
    wasm_bindgen_futures::spawn_local(async move {
        HANDLERS.with(|h| h.borrow_mut().remove(&id));
    });
}

// ---------------------------------------------------------------- worker side

fn post(msg: &JsValue, transfer: Option<&js_sys::Array>) {
    let scope: web_sys::DedicatedWorkerGlobalScope = js_sys::global().unchecked_into();
    let r = match transfer {
        Some(t) => scope.post_message_with_transfer(msg, t),
        None => scope.post_message(msg),
    };
    if let Err(e) = r {
        log::warn!("worker: postMessage: {e:?}");
    }
}

fn obj(pairs: &[(&str, JsValue)]) -> JsValue {
    let o = js_sys::Object::new();
    for (k, v) in pairs {
        let _ = js_sys::Reflect::set(&o, &JsValue::from_str(k), v);
    }
    o.into()
}

/// Send a file the job keeps to the page's storage (`Services::store_file` in a job worker).
pub(crate) fn post_store(path: &str, data: &[u8]) {
    let bytes = js_sys::Uint8Array::from(data);
    let buf = bytes.buffer();
    post(&obj(&[("type", "store".into()), ("path", path.into()), ("bytes", buf.clone().into())]), Some(&js_sys::Array::of1(&buf)));
}

/// Tell the page to drop a file the job stored ([`post_store`]) and no longer needs
/// (`Services::remove_file` in a job worker).
pub(crate) fn post_unstore(path: &str) {
    post(&obj(&[("type", "unstore".into()), ("path", path.into())]), None);
}

/// Set up the worker's engine session (called once by `web/worker.js`).
#[wasm_bindgen(js_name = workerInit)]
pub fn worker_init() {
    crate::set_worker();
    eframe::WebLogger::init(log::LevelFilter::Info).ok();
    let pool = Arc::new(effectcraft_media::MediaPool::new());
    POOL.with(|p| *p.borrow_mut() = Some(pool.clone()));
    // Rendered files go back to the page (transferred, not copied).
    let sink: Arc<effectcraft_export::Sink> = Arc::new(|path: &str, data: Vec<u8>| {
        let bytes = js_sys::Uint8Array::from(&data[..]);
        let buf = bytes.buffer();
        post(&obj(&[("type", "file".into()), ("path", path.into()), ("bytes", buf.clone().into())]), Some(&js_sys::Array::of1(&buf)));
    });
    let s = Session {
        services: Arc::new(crate::files::WebServices),
        footage: pool.clone(),
        importer: Some(Arc::new(crate::files::WebImporter { pool })),
        expr: Some(Arc::new(effectcraft_expr::Expressions)),
        expr_check: Some(effectcraft_expr::check_syntax),
        exporter: Some(Arc::new(effectcraft_host::FileExporter { sink: Some(sink) })),
        ..Default::default()
    };
    SESSION.with(|c| *c.borrow_mut() = Some(s));
}

/// The worker's media pool (footage files sent by the page).
pub fn pool() -> Arc<effectcraft_media::MediaPool> {
    POOL.with(|p| p.borrow().clone()).unwrap_or_else(|| Arc::new(effectcraft_media::MediaPool::new()))
}

/// A footage file for the jobs that follow.
#[wasm_bindgen(js_name = workerFile)]
pub fn worker_file(path: String, bytes: js_sys::Uint8Array) {
    let data: Arc<[u8]> = bytes.to_vec().into();
    crate::files::STORE.put_file(&path, data.clone(), false);
    POOL.with(|p| {
        if let Some(p) = p.borrow().as_ref() {
            p.add_bytes(&path, data);
        }
    });
}

/// A job worker's GPU (called once by `web/worker.js` after `workerInit`): with `gpu` and
/// WebGPU in workers, its own device with deferred readbacks becomes the session's
/// accelerator. Posts `{type: "gpu", gpu?, error?}`.
#[wasm_bindgen(js_name = workerJobInit)]
pub async fn worker_job_init(gpu: bool) {
    let r = if !gpu {
        Err("GPU job workers are off (?nogpuworkers)".to_string())
    } else if !crate::frames::has_web_gpu() {
        Err("no WebGPU in workers".to_string())
    } else {
        effectcraft_gpu::Gpu::request_deferred().await
    };
    let msg = match r {
        Ok(g) => {
            let name = effectcraft_engine::render::Accelerator::name(&g);
            SESSION.with(|c| {
                if let Some(s) = c.borrow_mut().as_mut() {
                    s.accel = Some(Arc::new(g.clone()));
                }
            });
            GPU.with(|c| *c.borrow_mut() = Some(g));
            log::info!("job worker: GPU {name}");
            obj(&[("type", "gpu".into()), ("gpu", name.into())])
        }
        Err(e) => {
            log::info!("job worker: CPU only ({e})");
            obj(&[("type", "gpu".into()), ("error", e.into())])
        }
    };
    post(&msg, None);
}

/// Run one job (a JSON [`WorkerRequest`]) and post its replies, then `{type: "stats", gpu,
/// passes, readbacks, ms}` (the device's render passes and readbacks during the job).
#[wasm_bindgen(js_name = workerJob)]
pub async fn worker_job(json: String) {
    let send: effectcraft_engine::offload::Post = Rc::new(|r: WorkerReply| {
        let done = matches!(r, WorkerReply::Done);
        let t = serde_json::to_string(&r).unwrap_or_default();
        post(&obj(&[("type", "reply".into()), ("json", t.into()), ("done", done.into())]), None);
    });
    let req: WorkerRequest = match serde_json::from_str(&json) {
        Ok(r) => r,
        Err(e) => {
            send(WorkerReply::Failed { error: format!("bad request: {e}") });
            send(WorkerReply::Done);
            return;
        }
    };
    // The session leaves its slot while the job runs (the job awaits the GPU in between;
    // messages are handled one at a time, so nothing else needs it meanwhile).
    let Some(mut s) = SESSION.with(|c| c.borrow_mut().take()) else {
        send(WorkerReply::Failed { error: "worker not initialised".into() });
        send(WorkerReply::Done);
        return;
    };
    let gpu = GPU.with(|c| c.borrow().clone());
    let before = gpu.as_ref().and_then(|g| g.deferred_stats()).unwrap_or_default();
    let t0 = web_time::Instant::now();
    let kind = format!("{:?}", req.job.kind());
    // `Done` is posted last by the job; the stats go out just before it.
    let stats_sent = Rc::new(std::cell::Cell::new(false));
    let (g2, s2) = (gpu.clone(), stats_sent.clone());
    let send2: effectcraft_engine::offload::Post = Rc::new(move |r: WorkerReply| {
        if matches!(r, WorkerReply::Done) && !s2.replace(true) {
            let after = g2.as_ref().and_then(|g| g.deferred_stats()).unwrap_or_default();
            post(
                &obj(&[
                    ("type", "stats".into()),
                    ("kind", kind.as_str().into()),
                    ("gpu", JsValue::from_bool(g2.is_some())),
                    ("passes", ((after.0 - before.0) as f64).into()),
                    ("readbacks", ((after.1 - before.1) as f64).into()),
                    ("ms", (t0.elapsed().as_secs_f64() * 1000.0).into()),
                ]),
                None,
            );
        }
        send(r);
    });
    effectcraft_engine::offload::run_request_async(&mut s, req, &send2).await;
    SESSION.with(|c| *c.borrow_mut() = Some(s));
}
