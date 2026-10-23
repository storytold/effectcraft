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
//!   [`FrameMsg::Sync`] (a full project the first time, a diff afterwards).
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
        }
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
        self.salt = effectcraft_render::disk_cache::footage_salt(&p);
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
        // The frame is done (its `frame_begin` ran in `handle`): the GPU pool ages its sizes.
        if let Some(a) = &self.accel {
            a.frame_end();
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
mod tests {
    use super::*;
    use crate::Session;
    use serde_json::json;

    fn round<T: Serialize + for<'de> Deserialize<'de>>(v: &T) -> T {
        serde_json::from_str(&serde_json::to_string(v).unwrap()).unwrap()
    }

    #[test]
    fn diff_and_patch_round_trip() {
        let a = json!({"a": 1, "b": [1, 2, {"c": null}], "d": {"e": "x", "f": [1]}, "g": null});
        let b = json!({"a": 2, "b": [1, 3, {"c": 4}], "d": {"f": [1, 2]}, "g": null, "h": {"i": null}});
        let ops = diff(&a, &b);
        assert!(!ops.is_empty());
        let mut x = a.clone();
        patch(&mut x, &round(&ops)).unwrap();
        assert_eq!(x, b);
        // Removal and the reverse direction.
        let mut y = b.clone();
        patch(&mut y, &diff(&b, &a)).unwrap();
        assert_eq!(y, a);
        assert!(diff(&a, &a).is_empty());
        // A whole-document change.
        let mut z = json!(1);
        patch(&mut z, &diff(&json!(1), &json!([1]))).unwrap();
        assert_eq!(z, json!([1]));
        // A patch for another document fails cleanly.
        assert!(patch(&mut json!(3), &diff(&a, &b)).is_err());
    }

    fn server(s: &Session) -> FrameServer {
        FrameServer::new(s.footage.clone(), s.expr.clone())
    }

    /// Pixels the page itself would render.
    fn local(s: &Session, comp: ItemId, t: Tick, opts: RenderOpts) -> Vec<u8> {
        let mut r = Renderer::new(&s.project, s.footage.as_ref(), opts);
        r.expr = s.expr.as_deref();
        rgba8_premultiplied(&r.comp_frame(comp, t))
    }

    /// Layer buffers through the disk store: a worker's slow buffers queue up as entries; the
    /// page plans what another worker fetches before its frame (recent misses on disk); that
    /// worker's mid-render lookups find them and the pixels match.
    #[test]
    fn layer_buffers_go_through_the_disk_store() {
        use effectcraft_render::disk_cache::{DiskIndex, Kind, LayerPrefetch};
        let mut s = Session::default();
        s.execute("file.openDemoProject", json!({})).unwrap();
        let comp = s.active_comp_id().unwrap();
        let opts = RenderOpts { scale: 0.25, ..Default::default() };
        let t = Tick::from_seconds_f64(1.0);
        let req = |id: u64, layers: bool, prefetch: Vec<String>| {
            round(&FrameMsg::Render { id, revision: s.revision, comp, time: t, opts: Box::new(opts), disk: None, layers, prefetch })
        };
        let worker = || {
            let mut w = server(&s);
            let store = Arc::new(PrefetchStore::new(64 << 20).with_min_ms(0));
            w.set_layer_store(Some(store.clone()));
            let mut m = Mirror::default();
            assert!(w.handle(round(&m.sync(s.revision, &s.project).unwrap())).is_empty());
            (w, store)
        };
        let unhex = |v: &[String]| v.iter().map(|k| u128::from_str_radix(k, 16).unwrap()).collect::<Vec<u128>>();
        // Worker A: everything misses; its buffers queue up for the disk.
        let (mut a, store_a) = worker();
        let out = a.handle(req(1, true, vec![]));
        let (FrameReply::Frame { layer_misses, layer_hits, .. }, Some(px_a)) = &out[0] else { panic!("{:?}", out[0].0) };
        assert!(!layer_misses.is_empty() && layer_hits.is_empty());
        let writes = store_a.take_writes();
        assert!(!writes.is_empty());
        assert!(writes.iter().all(|(k, _)| (*k >> 64) as u64 == a.salt()), "keys are salted per project");
        let mut index = DiskIndex::default();
        let mut disk = std::collections::HashMap::new();
        for (k, e) in writes {
            index.insert(Kind::Layer, k, e.len() as u64);
            disk.insert(k, e);
        }
        index.insert(Kind::Frame, 1, 10);
        // The page: recent misses that are on disk, most recent first.
        let mut plan = LayerPrefetch::default();
        plan.note_misses(unhex(layer_misses));
        let keys = plan.plan(&index, 64);
        assert!(!keys.is_empty() && keys.len() <= index.len());
        // Worker B fetches them before its first frame: its lookups hit.
        let (mut b, store_b) = worker();
        for k in &keys {
            assert!(store_b.provide(*k, &disk[k]));
        }
        assert!(!store_b.provide(1, b"damaged"));
        let out = b.handle(req(1, true, keys.iter().map(|k| format!("{k:032x}")).collect()));
        let (FrameReply::Frame { layer_misses: mb, layer_hits: hb, .. }, Some(px_b)) = &out[0] else { panic!("{:?}", out[0].0) };
        assert_eq!(unhex(hb).len(), keys.len(), "every fetched buffer was used");
        assert!(mb.len() < layer_misses.len());
        assert_eq!(px_a, px_b);
        assert_eq!(*px_b, local(&s, comp, t, opts));
        // Nothing new to write: what B rendered itself was rendered by A too.
        assert!(store_b.take_writes().iter().all(|(k, _)| disk.contains_key(k)));
        // Without `layers` the store is not consulted.
        let (mut c, store_c) = worker();
        c.handle(req(1, false, vec![]));
        assert!(store_c.take_misses().is_empty() && store_c.take_writes().is_empty());
        // Purge empties the store.
        b.handle(round(&FrameMsg::Purge));
        assert!(!store_b.has(keys[0]));
        // LRU across frames and layers in the shared index: a touched layer outlives the frame.
        index.touch(Kind::Layer, keys[0]);
        let order = index.lru_order();
        assert_eq!(order.last(), Some(&(Kind::Layer, keys[0])));
        assert!(order.iter().position(|e| *e == (Kind::Frame, 1)).unwrap() < order.len() - 1);
    }

    #[test]
    fn replica_follows_edits_through_diffs_and_renders_the_same_frames() {
        let mut s = Session::default();
        s.execute("file.openDemoProject", json!({})).unwrap();
        let comp = s.active_comp_id().unwrap();
        let opts = RenderOpts { scale: 0.25, guides: true, ..Default::default() };
        let mut m = Mirror::default();
        let mut w = server(&s);
        let send = |m: FrameMsg, w: &mut FrameServer| w.handle(round(&m));
        // First sync: the whole project.
        let first = m.sync(s.revision, &s.project).unwrap();
        assert!(matches!(first, FrameMsg::Project { .. }));
        assert!(send(first, &mut w).is_empty());
        assert!(m.sync(s.revision, &s.project).is_none(), "nothing new");
        let t = Tick::from_seconds_f64(1.0);
        let render = |w: &mut FrameServer, rev: u64, id: u64| {
            w.handle(round(&FrameMsg::Render { id, revision: rev, comp, time: t, opts: Box::new(opts), disk: None, layers: false, prefetch: vec![] }))
        };
        let out = render(&mut w, s.revision, 1);
        let (FrameReply::Frame { id, width, height, .. }, Some(px)) = &out[0] else { panic!("{:?}", out[0].0) };
        assert_eq!((*id, *width as usize * *height as usize * 4), (1, px.len()));
        assert_eq!(*px, local(&s, comp, t, opts));
        // An edit travels as a small diff; the replica renders the edited frame.
        s.execute("layer.newSolid", json!({"color": "#ff8800", "name": "Orange"})).unwrap();
        let msg = m.sync(s.revision, &s.project).unwrap();
        let FrameMsg::Patch { ops, .. } = &msg else { panic!("expected a patch") };
        assert!(!ops.is_empty() && serde_json::to_string(ops).unwrap().len() < s.project.to_json().len() / 2);
        assert!(send(msg, &mut w).is_empty());
        assert_eq!(**w.project().unwrap(), *s.project);
        let out = render(&mut w, s.revision, 2);
        assert_eq!(out[0].1.as_deref(), Some(&local(&s, comp, t, opts)[..]));
        // A request for another revision is refused as stale.
        let out = render(&mut w, s.revision + 7, 3);
        assert!(matches!(out[0].0, FrameReply::Failed { id: 3, stale: true, .. }));
        // A patch from the wrong base asks for a resync; the mirror then sends the project.
        let bad = FrameMsg::Patch { from: 999, revision: 1000, ops: vec![] };
        assert!(matches!(send(bad, &mut w)[0].0, FrameReply::Resync { .. }));
        m.reset();
        assert!(matches!(m.sync(s.revision, &s.project), Some(FrameMsg::Project { .. })));
    }

    #[test]
    fn frame_messages_round_trip() {
        for m in [
            FrameMsg::Purge,
            FrameMsg::Segs { segs: vec![SegData { key: 7, data: "AAAA".into() }] },
            FrameMsg::Render {
                id: 3,
                revision: 4,
                comp: ItemId(2),
                time: Tick::from_seconds_f64(0.5),
                opts: Box::new(RenderOpts { scale: 0.5, draft: true, roi: Some([1.0, 2.0, 3.0, 4.0]), ..Default::default() }),
                disk: Some(format!("{:032x}", 0xabcdu128)),
                layers: true,
                prefetch: vec![format!("{:032x}", 7u128)],
            },
        ] {
            let a = serde_json::to_string(&m).unwrap();
            assert_eq!(serde_json::to_string(&round(&m)).unwrap(), a);
            assert!(a.starts_with("{\"type\":"), "{a}");
        }
        for r in [
            FrameReply::Frame { id: 1, width: 2, height: 3, ms: 4.5, passes: 2, gpu: true, layer_misses: vec!["0a".into()], layer_hits: vec![] },
            FrameReply::Stored { key: "00ff".into(), bytes: 9, layer: false },
            FrameReply::Stored { key: "00fe".into(), bytes: 8, layer: true },
            FrameReply::Info { gpu: Some("Adapter (BrowserWebGpu)".into()), error: None },
            FrameReply::Failed { id: 1, error: "x".into(), stale: true },
            FrameReply::Resync { error: "y".into() },
        ] {
            assert_eq!(round(&r), r);
        }
    }

    /// An accelerator with deferred readbacks: a GPU effect chain is "read back" one
    /// [`Deferred::settle`] after it was first asked for (computed with the CPU effects, so the
    /// finished frame equals the CPU render); until then it misses and returns its input.
    #[derive(Default)]
    struct Deferred {
        ready: std::sync::Mutex<std::collections::HashSet<u64>>,
        asked: std::sync::Mutex<std::collections::HashSet<u64>>,
        gate: Arc<std::sync::atomic::AtomicBool>,
        chains: std::sync::atomic::AtomicUsize,
        auto: effectcraft_render::AutoPick,
    }

    impl Deferred {
        fn settle(&self) {
            let asked: Vec<u64> = self.asked.lock().unwrap().drain().collect();
            self.ready.lock().unwrap().extend(asked);
        }
    }

    impl Accelerator for Deferred {
        fn name(&self) -> String {
            "deferred".into()
        }
        fn comp_frame(&self, _: &Renderer, _: ItemId, _: Tick) -> Option<Image> {
            None
        }
        fn supports_effect(&self, id: &str) -> bool {
            effectcraft_effects::GPU_EFFECTS.contains(&id)
        }
        fn effects(&self, chain: &[effectcraft_render::FxStep], buf: &effectcraft_effects::Buf, levels: Option<f32>) -> Option<effectcraft_effects::Buf> {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            for s in chain {
                s.spec.id.hash(&mut h);
                s.ctx.time.to_bits().hash(&mut h);
            }
            (buf.img.width, buf.img.height).hash(&mut h);
            for p in &buf.img.data {
                for c in p {
                    c.to_bits().hash(&mut h);
                }
            }
            let key = h.finish();
            if !self.ready.lock().unwrap().contains(&key) {
                self.asked.lock().unwrap().insert(key);
                self.gate.store(true, std::sync::atomic::Ordering::Relaxed);
                return Some(buf.clone());
            }
            self.chains.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let mut b = buf.clone();
            for s in chain {
                b = effectcraft_effects::apply(s.spec, &s.ctx, b);
                if let Some(l) = levels {
                    effectcraft_render::color::quantize(&mut b.img, l);
                }
            }
            Some(b)
        }
        fn auto_pick(&self) -> Option<&effectcraft_render::AutoPick> {
            Some(&self.auto)
        }
        fn frame_begin(&self) {
            self.ready.lock().unwrap().clear();
        }
        fn pass_begin(&self) {
            self.gate.store(false, std::sync::atomic::Ordering::Relaxed);
        }
        fn pass_missed(&self) -> bool {
            self.gate.load(std::sync::atomic::Ordering::Relaxed)
        }
        fn miss_gate(&self) -> Option<Arc<std::sync::atomic::AtomicBool>> {
            Some(self.gate.clone())
        }
    }

    #[test]
    fn deferred_readbacks_render_in_passes() {
        let mut s = Session::default();
        s.execute("file.openDemoProject", json!({})).unwrap();
        let comp = s.active_comp_id().unwrap();
        s.execute("layer.newSolid", json!({"color": "#3388ff", "name": "Blue", "width": 200, "height": 120})).unwrap();
        s.execute("effect.apply", json!({"effect": "ec.blur.gaussian"})).unwrap();
        s.execute("effect.apply", json!({"effect": "ec.stylize.glow"})).unwrap();
        s.execute("layer.newSolid", json!({"color": "#ffcc00", "name": "Yellow", "width": 80, "height": 80})).unwrap();
        s.execute("effect.apply", json!({"effect": "ec.color.levels"})).unwrap();
        let opts = RenderOpts { scale: 0.25, backend: Backend::Gpu, ..Default::default() };
        let t = Tick::from_seconds_f64(1.0);
        let mut w = server(&s);
        let acc = Arc::new(Deferred::default());
        w.set_accel(Some(acc.clone()));
        let mut m = Mirror::default();
        w.handle(round(&m.sync(s.revision, &s.project).unwrap()));
        let req = |id| round(&FrameMsg::Render { id, revision: s.revision, comp, time: t, opts: Box::new(opts), disk: None, layers: false, prefetch: vec![] });
        assert!(w.handle(req(1)).is_empty() && w.waiting(), "the first pass misses");
        let mut out = vec![];
        for _ in 0..10 {
            acc.settle();
            out = w.resume();
            if !w.waiting() {
                break;
            }
        }
        let (FrameReply::Frame { id: 1, passes, gpu: true, .. }, Some(px)) = &out[0] else { panic!("{:?}", out.first().map(|o| &o.0)) };
        assert!(*passes >= 2, "{passes}");
        assert!(acc.chains.load(std::sync::atomic::Ordering::Relaxed) >= 2);
        // The same pixels as the CPU: no placeholder survived, in the frame or the layer cache.
        assert_eq!(*px, local(&s, comp, t, RenderOpts { backend: Backend::Cpu, ..opts }));
        let out = w.handle(req(2));
        let out = if out.is_empty() {
            acc.settle();
            w.resume()
        } else {
            out
        };
        assert_eq!(out[0].1.as_deref(), Some(&px[..]));
        // CPU requests never touch the accelerator; Auto times whole frames.
        let cpu = RenderOpts { backend: Backend::Cpu, ..opts };
        let out =
            w.handle(round(&FrameMsg::Render { id: 3, revision: s.revision, comp, time: t, opts: Box::new(cpu), disk: None, layers: false, prefetch: vec![] }));
        assert!(matches!(out[0].0, FrameReply::Frame { id: 3, passes: 1, gpu: false, .. }));
        for id in 4..12 {
            let auto = RenderOpts { backend: Backend::Auto, ..opts };
            let mut out = w.handle(round(&FrameMsg::Render {
                id,
                revision: s.revision,
                comp,
                time: t,
                opts: Box::new(auto),
                disk: None,
                layers: false,
                prefetch: vec![],
            }));
            while w.waiting() {
                acc.settle();
                out = w.resume();
            }
            assert_eq!(out[0].1.as_deref(), Some(&px[..]), "frame {id}");
        }
        let st = acc.auto.stats(effectcraft_render::AutoKey::new(comp, opts.scale, false)).unwrap();
        assert!(st.cpu_frames > 0 && st.gpu_frames > 0, "{st:?}");
    }
}
