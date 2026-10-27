//! Viewer frames rendered by another engine instance (the browser's frame worker).
//!
//! The page keeps no frame threads (a wasm build without shared memory has one thread per
//! instance), so the web app renders viewer frames in a long-lived Web Worker that holds a
//! replica of the project. The page sends the replica **project diffs** rather than the whole
//! project per edit, then render requests; the worker answers each with the frame's pixels.
//!
//! - [`diff`] / [`patch`]: a structural JSON diff of the serialized project (object members
//!   added, changed or removed; arrays element-wise when their length is unchanged, else
//!   replaced). Lossless (unlike JSON Merge Patch, `null` values survive).
//! - [`Mirror`] (page side): what the worker holds; turns the next project revision into a
//!   [`FrameMsg::Project`] the first time and [`FrameMsg::Patch`] for subsequent diffs.
//! - [`FrameServer`] (worker side): applies syncs, relayed Roto Brush segmentations and render
//!   requests, and renders frames as premultiplied RGBA8 ([`rgba8_premultiplied`]).
//!
//! - GPU frames: a worker with its own WebGPU device ([`FrameServer::accel`]) renders through
//!   the GPU compositor and GPU effects. Its readbacks can't be waited for, so a frame renders
//!   in passes ([`effectcraft_render::Accelerator::frame_begin`]): [`FrameServer::handle`]
//!   runs the first, and while [`FrameServer::waiting`] the transport awaits the device and
//!   calls [`FrameServer::resume`] for the next. Backend Auto picks CPU or GPU per comp from
//!   the frames' total times ([`effectcraft_render::AutoPick`]).
//! - Disk cache: a render request may carry the frame's disk-cache key ([`FrameMsg::Render`]
//!   `disk`); the transport stores the finished frame under it (the browser: in the Origin
//!   Private File System) and reports [`FrameReply::Stored`].
//! - Layer buffers on disk: with `layers`, the server's layer cache is backed by a
//!   [`PrefetchStore`] ([`FrameServer::set_layer_store`]): before the request the transport
//!   fetches the entries named in `prefetch` (recent misses that are on disk, chosen by the
//!   page, `disk_cache::LayerPrefetch`), so the lookups in the middle of the render find them;
//!   the reply lists the layer keys that missed and those served from the store
//!   ([`FrameReply::Frame`] `layer_misses` / `layer_hits`), and slow buffers queue up for the
//!   transport to write after the frame ([`PrefetchStore::take_writes`], reported as
//!   [`FrameReply::Stored`] with `layer`).
//!
//! The transport (structured-clone messages, transferable pixel buffers) is the web crate's;
//! everything here is portable and tested natively.

use std::sync::Arc;

use effectcraft_project::{ItemId, Project};
use effectcraft_render::{Accelerator, AutoKey, Backend, ExprHost, FootageSource, Image, LayerCache, LayerStore, PrefetchStore, RenderOpts, Renderer};
use effectcraft_time::Tick;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::offload::SegData;

/// One step of a path into a JSON document.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Step {
    Key(String),
    Index(usize),
}

/// One change: set the value at `path` (creating or replacing the member/element), or remove
/// the object member at `path` (`value` absent).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Op {
    pub path: Vec<Step>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
}

/// The changes that turn `old` into `new` (empty when equal).
pub fn diff(old: &Value, new: &Value) -> Vec<Op> {
    let mut ops = vec![];
    diff_into(old, new, &mut vec![], &mut ops);
    ops
}

fn diff_into(old: &Value, new: &Value, path: &mut Vec<Step>, ops: &mut Vec<Op>) {
    if old == new {
        return;
    }
    match (old, new) {
        (Value::Object(a), Value::Object(b)) => {
            for (k, va) in a {
                path.push(Step::Key(k.clone()));
                match b.get(k) {
                    Some(vb) => diff_into(va, vb, path, ops),
                    None => ops.push(Op { path: path.clone(), value: None }),
                }
                path.pop();
            }
            for (k, vb) in b {
                if !a.contains_key(k) {
                    path.push(Step::Key(k.clone()));
                    ops.push(Op { path: path.clone(), value: Some(vb.clone()) });
                    path.pop();
                }
            }
        }
        (Value::Array(a), Value::Array(b)) if a.len() == b.len() => {
            for (i, (va, vb)) in a.iter().zip(b).enumerate() {
                path.push(Step::Index(i));
                diff_into(va, vb, path, ops);
                path.pop();
            }
        }
        _ => ops.push(Op { path: path.clone(), value: Some(new.clone()) }),
    }
}

/// Apply [`diff`]'s changes to `doc`.
pub fn patch(doc: &mut Value, ops: &[Op]) -> Result<(), String> {
    for op in ops {
        let Some((last, parents)) = op.path.split_last() else {
            *doc = op.value.clone().ok_or("cannot remove the document")?;
            continue;
        };
        let mut cur = &mut *doc;
        for st in parents {
            cur = match (st, cur) {
                (Step::Key(k), Value::Object(m)) => m.get_mut(k).ok_or_else(|| format!("no member `{k}`"))?,
                (Step::Index(i), Value::Array(a)) => a.get_mut(*i).ok_or_else(|| format!("no element {i}"))?,
                _ => return Err("path does not match the document".into()),
            };
        }
        match (last, cur, &op.value) {
            (Step::Key(k), Value::Object(m), Some(v)) => {
                m.insert(k.clone(), v.clone());
            }
            (Step::Key(k), Value::Object(m), None) => {
                m.remove(k);
            }
            (Step::Index(i), Value::Array(a), Some(v)) => *a.get_mut(*i).ok_or_else(|| format!("no element {i}"))? = v.clone(),
            _ => return Err("path does not match the document".into()),
        }
    }
    Ok(())
}

/// Page → frame worker.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum FrameMsg {
    /// The whole project (JSON) at `revision`.
    Project { revision: u64, project: String },
    /// Changes from the worker's revision `from` to `revision`.
    Patch { from: u64, revision: u64, ops: Vec<Op> },
    /// Render a frame of the current project. `disk`: the frame's disk-cache key (32 hex
    /// digits) when the transport should store it.
    Render {
        id: u64,
        revision: u64,
        comp: ItemId,
        time: Tick,
        opts: Box<RenderOpts>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        disk: Option<String>,
        /// Back the layer cache with the disk cache's layer buffers (see the module docs).
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        layers: bool,
        /// Layer entries (32 hex digits) the transport fetches before rendering.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        prefetch: Vec<String>,
    },
    /// Roto Brush segmentations computed elsewhere (a propagation worker).
    Segs { segs: Vec<SegData> },
    /// Drop cached layer buffers (Edit ▸ Purge).
    Purge,
}

/// Frame worker → page. A [`FrameReply::Frame`]'s pixels travel next to the message (a
/// transferable buffer in the browser).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum FrameReply {
    /// Premultiplied RGBA8 pixels of request `id`, rendered in `ms` milliseconds (`passes`
    /// render passes, `gpu`: through the GPU accelerator).
    Frame {
        id: u64,
        width: u32,
        height: u32,
        ms: f64,
        #[serde(default)]
        passes: u32,
        #[serde(default)]
        gpu: bool,
        /// Layer-cache lookups that missed the store (salted keys, 32 hex digits).
        #[serde(default, rename = "layerMisses", skip_serializing_if = "Vec::is_empty")]
        layer_misses: Vec<String>,
        /// Layer buffers served from the store (fetched from disk).
        #[serde(default, rename = "layerHits", skip_serializing_if = "Vec::is_empty")]
        layer_hits: Vec<String>,
    },
    /// The frame (or, with `layer`, a layer buffer) was stored in the disk cache under `key`
    /// (`bytes` on disk).
    Stored {
        key: String,
        bytes: u64,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        layer: bool,
    },
    /// The worker's renderer: its GPU adapter, or why it has none.
    Info {
        #[serde(default)]
        gpu: Option<String>,
        #[serde(default)]
        error: Option<String>,
    },
    /// Request `id` was not rendered (`stale`: the worker holds another revision).
    Failed { id: u64, error: String, stale: bool },
    /// A sync could not be applied: the page must send the whole project again.
    Resync { error: String },
}

/// The page's record of a worker's replica.
#[derive(Default)]
pub struct Mirror {
    sent: Option<(u64, Value)>,
}

impl Mirror {
    /// The revision the worker holds.
    pub fn revision(&self) -> Option<u64> {
        self.sent.as_ref().map(|s| s.0)
    }

    /// Forget the replica (the next sync sends the whole project).
    pub fn reset(&mut self) {
        self.sent = None;
    }

    /// The message that brings the worker to `revision` of `project` (`None` when it has it).
    pub fn sync(&mut self, revision: u64, project: &Project) -> Option<FrameMsg> {
        if self.revision() == Some(revision) {
            return None;
        }
        let v = serde_json::to_value(project).unwrap_or(Value::Null);
        let msg = match self.sent.take() {
            Some((from, old)) => FrameMsg::Patch { from, revision, ops: diff(&old, &v) },
            None => FrameMsg::Project { revision, project: v.to_string() },
        };
        self.sent = Some((revision, v));
        Some(msg)
    }
}

/// Premultiplied f32 → premultiplied RGBA8 (colour clamped to alpha), as the viewer shows it.
pub fn rgba8_premultiplied(img: &Image) -> Vec<u8> {
    let mut out = Vec::with_capacity(img.data.len() * 4);
    for p in &img.data {
        let a = p[3].clamp(0.0, 1.0);
        let c = |v: f32| (v.clamp(0.0, a) * 255.0 + 0.5) as u8;
        out.extend_from_slice(&[c(p[0]), c(p[1]), c(p[2]), (a * 255.0 + 0.5) as u8]);
    }
    out
}

/// The worker's side: a project replica and what rendering needs.
pub struct FrameServer {
    pub footage: Arc<dyn FootageSource>,
    pub expr: Option<Arc<dyn ExprHost>>,
    pub cache: Arc<LayerCache>,
    /// The worker's own accelerator (a GPU device; see [`FrameServer::set_accel`]).
    accel: Option<Arc<dyn Accelerator>>,
    doc: Value,
    project: Option<Arc<Project>>,
    revision: Option<u64>,
    /// The frame being rendered in passes.
    pending: Option<Pending>,
    /// The disk cache's layer buffers (see the module docs) and the project's salt.
    layer_store: Option<Arc<PrefetchStore>>,
    salt: u64,
    /// Size and content hash for footage paths with no native metadata (the browser file table).
    virtual_id: Option<Arc<dyn Fn(&str) -> Option<(u64, u64)> + Send + Sync>>,
}

/// A frame whose readbacks are in flight.
struct Pending {
    id: u64,
    project: Arc<Project>,
    comp: ItemId,
    time: Tick,
    opts: RenderOpts,
    t0: web_time::Instant,
    passes: u32,
    /// Backend Auto's choice: (timing key, GPU).
    auto: Option<(AutoKey, bool)>,
}

/// Passes before a frame gives up on deferred readbacks and renders on the CPU.
pub const MAX_PASSES: u32 = 24;

impl FrameServer {
    pub fn new(footage: Arc<dyn FootageSource>, expr: Option<Arc<dyn ExprHost>>) -> FrameServer {
        FrameServer {
            footage,
            expr,
            cache: Arc::new(LayerCache::default()),
            accel: None,
            doc: Value::Null,
            project: None,
            revision: None,
            pending: None,
            layer_store: None,
            salt: 0,
            virtual_id: None,
        }
    }

    /// Identity of virtual footage files, used when native metadata cannot describe the path.
    /// Called from `FrameServer::load`, not from the UI thread's cache lookup.
    pub fn set_virtual_id(&mut self, id: Option<Arc<dyn Fn(&str) -> Option<(u64, u64)> + Send + Sync>>) {
        self.virtual_id = id;
    }

    /// The store behind the layer cache when a request asks for `layers` (the browser: the
    /// disk cache's layer buffers, fetched before the request).
    pub fn set_layer_store(&mut self, store: Option<Arc<PrefetchStore>>) {
        self.layer_store = store;
    }

    pub fn layer_store(&self) -> Option<&Arc<PrefetchStore>> {
        self.layer_store.as_ref()
    }

    /// The salt of the replica's layer keys on disk (keys are `(salt, key)`).
    pub fn salt(&self) -> u64 {
        self.salt
    }

    /// Render with `accel` when the request's backend asks for it (the worker's GPU). Its miss
    /// gate keeps placeholder buffers of unfinished passes out of the layer cache.
    pub fn set_accel(&mut self, accel: Option<Arc<dyn Accelerator>>) {
        self.cache.set_gate(accel.as_ref().and_then(|a| a.miss_gate()));
        self.accel = accel;
    }

    pub fn accel(&self) -> Option<&Arc<dyn Accelerator>> {
        self.accel.as_ref()
    }

    pub fn revision(&self) -> Option<u64> {
        self.revision
    }

    pub fn project(&self) -> Option<&Arc<Project>> {
        self.project.as_ref()
    }

    /// A frame is waiting for readbacks: await the device, then [`FrameServer::resume`].
    pub fn waiting(&self) -> bool {
        self.pending.is_some()
    }

    fn load(&mut self, revision: u64) -> Result<(), String> {
        let p = Project::from_json(&self.doc.to_string()).map_err(|e| e.to_string())?;
        self.salt = effectcraft_render::disk_cache::footage_salt_with(&p, self.virtual_id.as_deref());
        self.cache.set_store_salt(self.salt);
        self.project = Some(Arc::new(p));
        self.revision = Some(revision);
        Ok(())
    }

    /// Handle one message; returns the replies with their pixels. A render that needs more
    /// passes returns nothing yet ([`FrameServer::waiting`]).
    pub fn handle(&mut self, msg: FrameMsg) -> Vec<(FrameReply, Option<Vec<u8>>)> {
        match msg {
            FrameMsg::Project { revision, project } => {
                let r = serde_json::from_str(&project).map_err(|e| e.to_string()).and_then(|v| {
                    self.doc = v;
                    self.load(revision)
                });
                match r {
                    Ok(()) => vec![],
                    Err(e) => self.lost(e),
                }
            }
            FrameMsg::Patch { from, revision, ops } => {
                if self.revision != Some(from) {
                    return self.lost(format!("the replica is at {:?}, the patch starts at {from}", self.revision));
                }
                match patch(&mut self.doc, &ops).and_then(|_| self.load(revision)) {
                    Ok(()) => vec![],
                    Err(e) => self.lost(e),
                }
            }
            FrameMsg::Segs { segs } => {
                for s in &segs {
                    s.store();
                }
                vec![]
            }
            FrameMsg::Purge => {
                self.cache.clear();
                if let Some(st) = &self.layer_store {
                    st.clear();
                }
                vec![]
            }
            FrameMsg::Render { id, revision, comp, time, opts, layers, .. } => {
                let store = self.layer_store.clone().filter(|_| layers);
                self.cache.set_store(store.map(|s| (s as Arc<dyn LayerStore>, self.salt)));
                let fail = |error: String, stale: bool| vec![(FrameReply::Failed { id, error, stale }, None)];
                if self.revision != Some(revision) {
                    return fail(format!("the replica is at {:?}, not {revision}", self.revision), true);
                }
                let Some(p) = self.project.clone() else { return fail("no project".into(), true) };
                if p.comp(comp).is_none() {
                    return fail(format!("no composition {}", comp.0), false);
                }
                let mut opts = *opts;
                let mut auto = None;
                if let Some(a) = &self.accel {
                    // Auto: CPU or GPU for the whole frame (all its passes), timed as a whole.
                    if opts.backend == Backend::Auto {
                        if !p.settings.gpu_acceleration {
                            opts.backend = Backend::Cpu;
                        } else if opts.roi.is_none()
                            && let Some(pick) = a.auto_pick()
                        {
                            let key = AutoKey::new(comp, opts.scale, false);
                            let gpu = pick.choose(key);
                            opts.backend = if gpu { Backend::Gpu } else { Backend::Cpu };
                            auto = Some((key, gpu));
                        }
                    }
                    a.frame_begin();
                }
                self.pending = Some(Pending { id, project: p, comp, time, opts, t0: web_time::Instant::now(), passes: 0, auto });
                self.resume()
            }
        }
    }

    /// Run the next pass of the frame in progress; its reply once it is done.
    pub fn resume(&mut self) -> Vec<(FrameReply, Option<Vec<u8>>)> {
        let Some(mut pr) = self.pending.take() else { return vec![] };
        pr.passes += 1;
        let accel = self.accel.clone().filter(|_| pr.opts.backend != Backend::Cpu);
        let give_up = pr.passes > MAX_PASSES;
        let mut r = Renderer::new(&pr.project, self.footage.as_ref(), pr.opts);
        r.expr = self.expr.as_deref();
        r.cache = Some(&self.cache);
        if !give_up {
            r.accel = accel.as_deref();
        }
        if let Some(a) = r.accel {
            a.pass_begin();
        }
        let img = r.comp_frame(pr.comp, pr.time);
        if r.accel.is_some_and(|a| a.pass_missed()) {
            self.pending = Some(pr);
            return vec![];
        }
        let ms = pr.t0.elapsed().as_secs_f64() * 1000.0;
        let gpu = r.accel.is_some() && r.active_accel().is_some();
        let hex = |v: Vec<u128>| v.into_iter().map(|k| format!("{k:032x}")).collect::<Vec<_>>();
        let (layer_misses, layer_hits) = match &self.layer_store {
            Some(st) => (hex(st.take_misses()), hex(st.take_hits())),
            None => (vec![], vec![]),
        };
        if let (Some((key, chose)), Some(a)) = (pr.auto, &self.accel)
            && let Some(pick) = a.auto_pick()
        {
            pick.record(key, chose, ms);
        }
        vec![(
            FrameReply::Frame { id: pr.id, width: img.width, height: img.height, ms, passes: pr.passes, gpu, layer_misses, layer_hits },
            Some(rgba8_premultiplied(&img)),
        )]
    }

    fn lost(&mut self, error: String) -> Vec<(FrameReply, Option<Vec<u8>>)> {
        self.doc = Value::Null;
        self.project = None;
        self.revision = None;
        vec![(FrameReply::Resync { error }, None)]
    }
}

#[cfg(test)]
mod tests;
