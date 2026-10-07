//! Background frame rendering and the RAM preview cache.
//!
//! Frames render on a dedicated thread pool from immutable `Arc<Project>` snapshots, so the UI
//! never waits for the compositor. Results land in a memory-budgeted cache keyed by the comp's
//! *content* ([`comp_content`]: the comp and everything it uses), frame, scale, view and render
//! options; the timeline draws the cached range as the green cache bar. An edit keeps the frames
//! of every comp it doesn't touch, and undo finds the frames of the state it returns to.
//! Switches that don't change pixels (Audio, Lock, Shy, Hide Shy Layers) keep the frames too.
//! When the budget is full the least recently shown frames go first.
//!
//! wasm32 has no threads: jobs wait in the same priority queue and [`Frames::pump`] hands them
//! to a [`RemoteFrames`] (the browser's frame worker, a second engine instance fed with project
//! diffs, `effectcraft_engine::remote`) or, without one, renders them on the UI thread between
//! egui frames, within a time budget.
//!
//! With the disk cache on (Settings ▸ Media & Disk Cache), every CPU-rendered frame is also
//! written to disk under a content key (the comp's content hash, frame, scale, view and render
//! options), and a job first looks there: frames survive restarts and project reopenings. The
//! timeline shows disk-only frames as a blue cache bar.
//!
//! With Mercury GPU Acceleration (a [`Gpu`] on egui-wgpu's device and the project's renderer set
//! to the GPU) frames stay on the GPU: the compositor writes an RGBA8 texture that the viewer
//! registers with egui-wgpu and draws directly; pixels are read back only when something needs
//! them (Info panel, eyedroppers, histograms).

mod failures;

pub use failures::{FailureKind, PreviewFailure, presentation_check, presentation_image, presentation_size, same_view};

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use effectcraft_engine::project::{Comp, ItemId, ItemKind, LayerSource, Node, Project, PropGroup};
use effectcraft_engine::render::disk_cache::{self, DiskCache};
use effectcraft_engine::render::{ExprHost, FootageSource, LayerCache, RenderOpts, Renderer};
use effectcraft_engine::time::Tick;
use effectcraft_gpu::{DisplayFrame, Gpu};
use rayon::prelude::*;

/// Comp identities (and the project snapshots behind them) the RAM preview keeps frames of.
const MAX_KEEPERS: usize = 64;

/// GPU frames kept for RAM preview (bytes of video memory) at first. wgpu can't tell how much
/// video memory there is: each time the device runs out, the budget halves (to
/// [`MIN_GPU_BUDGET`]) and the frame renders on the CPU (#106).
const GPU_BUDGET: usize = 1 << 30;
const MIN_GPU_BUDGET: usize = 64 << 20;

/// A rendered viewer frame: CPU pixels or a GPU texture.
#[derive(Clone)]
pub enum FrameImage {
    Cpu(Arc<egui::ColorImage>),
    Gpu(Arc<DisplayFrame>),
}

impl FrameImage {
    pub fn size(&self) -> [usize; 2] {
        match self {
            FrameImage::Cpu(c) => c.size,
            FrameImage::Gpu(f) => [f.width as usize, f.height as usize],
        }
    }
    fn bytes(&self) -> usize {
        self.size()[0] * self.size()[1] * 4
    }
    fn is_gpu(&self) -> bool {
        matches!(self, FrameImage::Gpu(_))
    }
}

/// A viewer frame: its comp's content, the frame, scale, view and render options. `revision` is
/// the project revision it was asked for at (job bookkeeping); it is not part of the identity, so
/// a later revision with the same content finds the frame.
#[derive(Clone, Copy, Debug)]
pub struct FrameKey {
    pub revision: u64,
    /// The comp's content key ([`Frames::content_of`]).
    pub content: u64,
    pub comp: u64,
    pub frame: i64,
    /// Render scale × 1000.
    pub scale: u32,
    /// Hash of the 3D view camera (0 = the comp's active camera).
    pub view: u64,
    /// Hash of the other render options (Draft / Fast Previews, shadows, nested switches…).
    pub opts: u64,
}

impl FrameKey {
    /// Everything but the revision.
    fn id(&self) -> (u64, u64, i64, u32, u64, u64) {
        (self.content, self.comp, self.frame, self.scale, self.view, self.opts)
    }

    /// Whether `self` is a frame of the same comp content, scale, view and options as `base`.
    pub fn same_series(&self, base: &FrameKey) -> bool {
        FrameKey { frame: base.frame, ..*self } == *base
    }
}

impl PartialEq for FrameKey {
    fn eq(&self, o: &FrameKey) -> bool {
        self.id() == o.id()
    }
}

impl Eq for FrameKey {}

impl std::hash::Hash for FrameKey {
    fn hash<H: std::hash::Hasher>(&self, h: &mut H) {
        self.id().hash(h);
    }
}

struct Cache {
    map: HashMap<FrameKey, FrameImage>,
    order: VecDeque<FrameKey>,
    bytes: usize,
    /// Bytes of GPU frames (also counted in `bytes`).
    gpu_bytes: usize,
    budget: usize,
    /// Bytes of GPU frames to keep at most ([`GPU_BUDGET`]).
    gpu_budget: usize,
}

impl Cache {
    fn insert(&mut self, k: FrameKey, img: FrameImage) {
        let (sz, gpu) = (img.bytes(), img.is_gpu());
        if let Some(old) = self.map.insert(k, img) {
            self.forget(&old);
        } else {
            self.order.push_back(k);
        }
        self.bytes += sz;
        if gpu {
            self.gpu_bytes += sz;
        }
        while self.bytes > self.budget {
            let Some(old) = self.order.pop_front() else { break };
            self.remove(&old);
        }
        self.evict_gpu();
    }
    /// Video memory: drop the oldest GPU frames past their own budget.
    fn evict_gpu(&mut self) {
        while self.gpu_bytes > self.gpu_budget {
            let Some(i) = self.order.iter().position(|k| self.map.get(k).is_some_and(FrameImage::is_gpu)) else { break };
            if let Some(old) = self.order.remove(i) {
                self.remove(&old);
            }
        }
    }
    /// The GPU ran out of memory: keep half as many GPU frames from now on.
    fn gpu_out_of_memory(&mut self) {
        self.gpu_budget = (self.gpu_budget / 2).max(MIN_GPU_BUDGET);
        self.evict_gpu();
    }
    fn forget(&mut self, img: &FrameImage) {
        self.bytes -= img.bytes();
        if img.is_gpu() {
            self.gpu_bytes -= img.bytes();
        }
    }
    fn remove(&mut self, k: &FrameKey) {
        if let Some(i) = self.map.remove(k) {
            self.forget(&i);
        }
    }
    /// Shown again: last in line for eviction.
    fn touch(&mut self, k: &FrameKey) {
        if let Some(i) = self.order.iter().rposition(|o| o == k)
            && let Some(k) = self.order.remove(i)
        {
            self.order.push_back(k);
        }
    }
    /// Drop every frame of the identities in `gone`.
    fn drop_contents(&mut self, gone: &HashSet<u64>) {
        let keys: Vec<FrameKey> = self.map.keys().filter(|k| gone.contains(&k.content)).copied().collect();
        for k in &keys {
            self.remove(k);
        }
        self.order.retain(|k| !gone.contains(&k.content));
    }
    fn evict_to_budget(&mut self) {
        while self.bytes > self.budget {
            let Some(old) = self.order.pop_front() else { break };
            self.remove(&old);
        }
    }
}

/// What a render job needs (all cheap clones).
#[derive(Clone)]
pub struct RenderSource {
    pub project: Arc<Project>,
    pub footage: Arc<dyn FootageSource>,
    pub expr: Option<Arc<dyn ExprHost>>,
    pub layer_cache: Arc<LayerCache>,
    /// GPU compositor on the viewer's device (Mercury GPU Acceleration), if available.
    pub gpu: Option<Gpu>,
    /// Keep GPU frames on the GPU for the viewer (native egui-wgpu textures). Off on the
    /// desktop: Show Channel / exposure / region-of-interest drawing read the CPU frame texture,
    /// so GPU frames are read back (the compositing still runs on the GPU). On in the browser
    /// while the viewer draws plainly, because there frames can't be read back synchronously
    /// (`EffectcraftApp::gpu_display`).
    pub gpu_display: bool,
    /// The persistent disk cache, when enabled.
    pub disk: Option<Arc<DiskCache>>,
}

/// A [`comp_content`] identity and, when it was found in another project, that project
/// ([`Frames::identity`]).
type Identity = (u64, Option<Arc<Project>>);

/// Content keys of (project revision, comp), computed once per revision.
type ContentKeys = Arc<Mutex<HashMap<(u64, u64), u128>>>;

fn content_key(keys: &ContentKeys, project: &Project, revision: u64, comp: u64) -> u128 {
    if let Some(k) = keys.lock().ok().and_then(|m| m.get(&(revision, comp)).copied()) {
        return k;
    }
    let k = disk_cache::comp_content_key(project, ItemId(comp));
    if let Ok(mut m) = keys.lock() {
        if m.len() > 256 {
            m.clear();
        }
        m.insert((revision, comp), k);
    }
    k
}

/// The identity of a comp's pixels in `project`, cheap enough to take at every revision: the
/// addresses of the comps it draws (an edit replaces a comp's `Arc`: `Arc::make_mut` copies it
/// while another snapshot holds it, and [`Frames::content_of`] keeps one for every identity it
/// hands out), and by value the project settings and the footage, solids and proxies it uses.
/// With expressions in its comps (which can read anything) every item counts.
pub fn comp_content(project: &Project, comp: ItemId) -> u64 {
    comp_content_with(project, comp, |_, c| Arc::as_ptr(c))
}

/// [`comp_content`] with each comp's address given by `addr`.
fn comp_content_with(project: &Project, comp: ItemId, addr: impl Fn(ItemId, &Arc<Comp>) -> *const Comp) -> u64 {
    use std::hash::{Hash, Hasher};
    fn has_expression(g: &PropGroup) -> bool {
        g.children.iter().any(|c| match c {
            Node::Prop(p) => p.expr.is_some(),
            Node::Group(sub) => has_expression(sub),
        })
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    format!("{:?}", project.settings).hash(&mut h);
    let mut seen = BTreeSet::new();
    let mut stack = vec![comp];
    let mut expressions = false;
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        let Some(ItemKind::Comp(c)) = project.item(id).map(|i| &i.kind) else { continue };
        for l in &c.layers {
            if let LayerSource::Comp { item } | LayerSource::Footage { item } | LayerSource::Solid { item } = l.source {
                stack.push(item);
            }
            expressions = expressions || has_expression(&l.props);
        }
    }
    let ids: Vec<ItemId> = if expressions { project.items.keys().copied().collect() } else { seen.into_iter().collect() };
    for id in ids {
        let Some(it) = project.item(id) else { continue };
        id.0.hash(&mut h);
        if expressions {
            // (`comp("Name")`, `footage("Name")`)
            it.name.hash(&mut h);
        }
        match &it.kind {
            ItemKind::Comp(c) => (addr(id, c) as usize).hash(&mut h),
            k => format!("{k:?}").hash(&mut h),
        }
        format!("{:?}", it.proxy).hash(&mut h);
    }
    comp.0.hash(&mut h);
    h.finish()
}

/// Hash of render options (part of [`FrameKey`] and of the disk key).
pub fn opts_hash(o: &RenderOpts) -> u64 {
    let mut h = disk_cache::Hash128::default();
    h.write(format!("{o:?}").as_bytes());
    h.finish() as u64
}

/// Disk key of a viewer frame (render options included: region of interest, draft…).
fn frame_disk_key(keys: &ContentKeys, project: &Project, key: &FrameKey, opts_hash: u64) -> u128 {
    let content = content_key(keys, project, key.revision, key.comp);
    disk_cache::frame_key(content, key.frame, key.scale, key.view ^ opts_hash)
}

/// A frame for a [`RemoteFrames`] renderer.
pub struct RemoteJob {
    pub project: Arc<Project>,
    pub revision: u64,
    pub comp: ItemId,
    pub t: Tick,
    pub opts: RenderOpts,
    /// The frame's disk-cache key, when the renderer keeps a disk cache
    /// ([`RemoteFrames::disk`]): served from it on a hit, stored there after a render.
    pub disk_key: Option<u128>,
}

/// Premultiplied RGBA8 pixels from a [`RemoteFrames`] renderer.
pub struct RemoteFrame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// Render time (ms).
    pub ms: f64,
}

/// Called once with a remote frame (or why there is none).
pub type RemoteDone = Box<dyn FnOnce(Result<RemoteFrame, String>) + Send>;
/// Typed completion seam for future admission-aware workers; legacy strings remain retryable.
pub type CheckedRemoteDone = Box<dyn FnOnce(Result<RemoteFrame, PreviewFailure>) + Send>;

/// Renders viewer frames elsewhere (the browser's frame worker), so the UI thread never does.
pub trait RemoteFrames: Send + Sync {
    /// Frames it can render at the same time (0 when it is not usable: frames then render on
    /// the UI thread again).
    fn slots(&self) -> usize;
    /// Start rendering `job`; `done` runs when the frame arrives.
    fn start(&self, job: RemoteJob, done: RemoteDone);
    /// Legacy transports have no reliable permanent-error classification. Never inspect strings
    /// to decide that a document is unrenderable; use bounded backoff for all of them.
    fn start_checked(&self, job: RemoteJob, done: CheckedRemoteDone) {
        self.start(job, Box::new(move |r| done(r.map_err(|e| PreviewFailure::new(FailureKind::Retryable, &e)))));
    }
    /// Drop cached intermediate results (Edit ▸ Purge).
    fn purge(&self) {}
    /// It renders GPU effects (its own GPU device): frames with effects go to it rather than
    /// the UI thread's GPU compositor, which would run their effects on the CPU.
    fn gpu(&self) -> bool {
        false
    }
    /// It keeps a disk cache of frames (the browser's Origin Private File System).
    fn disk(&self) -> bool {
        false
    }
    /// The disk cache holds the frame of this key (the blue cache bar).
    fn disk_contains(&self, _key: u128) -> bool {
        false
    }
}

/// Whether a comp (or a comp it nests) has a layer with enabled effects.
pub fn comp_has_effects(project: &Project, comp: ItemId) -> bool {
    fn walk(p: &Project, comp: ItemId, depth: usize) -> bool {
        let Some(c) = p.comp(comp) else { return false };
        c.layers.iter().any(|l| {
            l.switches.effects && l.effects().is_some_and(|fx| fx.groups().any(|g| g.enabled))
                || depth < 16 && matches!(l.source, effectcraft_engine::project::LayerSource::Comp { item } if walk(p, item, depth + 1))
        })
    }
    walk(project, comp, 0)
}

pub struct Frames {
    cache: Arc<Mutex<Cache>>,
    /// Keys queued or rendering.
    inflight: Arc<Mutex<HashSet<FrameKey>>>,
    /// Pending jobs, picked by priority when a pool thread frees up (the viewer's frame first).
    queue: Arc<Mutex<Queue>>,
    #[cfg(not(target_arch = "wasm32"))]
    /// None if the OS refused the threads: jobs then go to rayon's global pool.
    pool: Option<rayon::ThreadPool>,
    ctx: Option<egui::Context>,
    /// Render time of the last viewer (urgent) frame in ms, for the Info panel / perf readout.
    pub last_ms: Arc<Mutex<f64>>,
    content_keys: ContentKeys,
    gpu_retired: Arc<AtomicBool>,
    failures: Arc<Mutex<failures::Failures>>,
    failure_clock: web_time::Instant,
    /// [`comp_content`] identities by (revision, comp), with the project that keeps an identity
    /// found by [`Frames::identity`] in another project.
    identities: Mutex<HashMap<(u64, u64), Identity>>,
    /// The last new identity of each comp and its project (see [`Frames::identity`]).
    bases: Mutex<HashMap<u64, (u64, Arc<Project>)>>,
    /// A project snapshot for each identity handed out, with when it was last asked for (see
    /// [`Frames::content_of`]); the counter.
    keepers: Mutex<(u64, HashMap<u64, (u64, Arc<Project>)>)>,
    /// Where frames render instead of the UI thread (wasm32), and how many it is rendering.
    remote: Option<Arc<dyn RemoteFrames>>,
    remote_busy: Arc<Mutex<usize>>,
}

impl Default for Frames {
    fn default() -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(2, 16);
        Frames {
            cache: Arc::new(Mutex::new(Cache { map: HashMap::new(), order: VecDeque::new(), bytes: 0, gpu_bytes: 0, budget: 3 << 30, gpu_budget: GPU_BUDGET })),
            inflight: Arc::new(Mutex::new(HashSet::new())),
            queue: Arc::new(Mutex::new(Queue::default())),
            #[cfg(not(target_arch = "wasm32"))]
            // A panicking job would abort the whole process (rayon's default for `spawn`): log it
            // and lose only that frame (the panic hook has logged the message).
            pool: rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .thread_name(|i| format!("ec-frame-{i}"))
                .panic_handler(|_| log::error!("a frame render panicked; that frame was skipped"))
                .build()
                .ok(),
            ctx: None,
            last_ms: Arc::new(Mutex::new(0.0)),
            content_keys: Arc::default(),
            gpu_retired: Arc::default(),
            identities: Mutex::default(),
            bases: Mutex::default(),
            keepers: Mutex::default(),
            remote: None,
            remote_busy: Arc::default(),
            failures: Arc::default(),
            failure_clock: web_time::Instant::now(),
        }
    }
}

/// Premultiplied f32 → egui premultiplied Color32 (alpha kept, so the viewer can show the
/// transparency grid or the comp background colour underneath).
pub fn to_color_image(img: &effectcraft_engine::render::Image) -> egui::ColorImage {
    let px: Vec<egui::Color32> = img
        .data
        .par_iter()
        .map(|p| {
            let a = p[3].clamp(0.0, 1.0);
            let c = |v: f32| (v.clamp(0.0, a) * 255.0 + 0.5) as u8;
            egui::Color32::from_rgba_premultiplied(c(p[0]), c(p[1]), c(p[2]), (a * 255.0 + 0.5) as u8)
        })
        .collect();
    egui::ColorImage::new([img.width as usize, img.height as usize], px)
}

impl Frames {
    /// Retire only GPU preview state; documents, CPU frames and running job leases survive.
    /// Publication checks the same latch while holding the cache lock, so a late GPU frame
    /// cannot restore an invalid texture after this removal.
    pub fn retire_gpu(&self) {
        self.gpu_retired.store(true, Ordering::Release);
        self.retry_failed_previews();
        if let Ok(mut cache) = self.cache.lock() {
            let keys: Vec<_> = cache.map.iter().filter_map(|(key, image)| image.is_gpu().then_some(*key)).collect();
            for key in keys {
                cache.remove(&key);
            }
            let Cache { map, order, .. } = &mut *cache;
            order.retain(|key| map.contains_key(key));
        }
        if let Some(ctx) = &self.ctx {
            ctx.request_repaint();
        }
    }

    /// Actual native frame-worker capacity, including the OS fallback pool.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn parallelism(&self) -> usize {
        self.pool.as_ref().map_or_else(rayon::current_num_threads, rayon::ThreadPool::current_num_threads)
    }
    #[cfg(target_arch = "wasm32")]
    pub fn parallelism(&self) -> usize {
        self.remote_slots().max(1)
    }
    pub fn set_context(&mut self, ctx: &egui::Context) {
        let id = egui::Id::new("preview-presentation-limit");
        let limit = ctx.input(|i| i.max_texture_side);
        let previous = ctx.data(|d| d.get_temp::<usize>(id));
        ctx.data_mut(|d| d.insert_temp(id, limit));
        if previous.is_some_and(|old| old != limit) {
            self.retry_failed_previews();
        }
        if self.ctx.is_none() {
            self.ctx = Some(ctx.clone());
        }
    }

    /// The egui context (to request repaints from callbacks).
    pub fn context(&self) -> Option<egui::Context> {
        self.ctx.clone()
    }

    /// The cached frame, which becomes the most recently used.
    pub fn get(&self, k: &FrameKey) -> Option<FrameImage> {
        let mut c = self.cache.lock().ok()?;
        let img = c.map.get(k).cloned()?;
        if let (Some(ctx), FrameImage::Cpu(cpu)) = (&self.ctx, &img)
            && let Err(error) = presentation_image(ctx, cpu)
        {
            drop(c);
            self.reject_presentation(*k, error);
            return None;
        }
        c.touch(k);
        Some(img)
    }

    /// The [`comp_content`] identity of `comp` in `project` at `revision` (taken once per
    /// revision). The project is kept while frames of that identity are cached or rendering, so
    /// the comp addresses it hashes can't be reused by other comps meanwhile.
    pub fn content_of(&self, project: &Arc<Project>, revision: u64, comp: ItemId) -> u64 {
        let known = self.identities.lock().ok().and_then(|m| m.get(&(revision, comp.0)).cloned());
        let (id, base) = known.unwrap_or_else(|| {
            let found = self.identity(project, comp);
            if let Ok(mut m) = self.identities.lock() {
                if m.len() > 256 {
                    m.clear();
                }
                m.insert((revision, comp.0), found.clone());
            }
            found
        });
        if let Ok(mut guard) = self.keepers.lock() {
            let (clock, k) = &mut *guard;
            *clock += 1;
            let now = *clock;
            k.entry(id).or_insert_with(|| (now, base.unwrap_or_else(|| project.clone()))).0 = now;
            if k.len() > MAX_KEEPERS {
                let inflight: HashSet<u64> = self.inflight.lock().map(|s| s.iter().map(|f| f.content).collect()).unwrap_or_default();
                if let Ok(mut c) = self.cache.lock() {
                    // Snapshots of identities without frames go; past the limit the least
                    // recently asked for go too, with their frames (a long drag leaves many).
                    let cached: HashSet<u64> = c.map.keys().map(|f| f.content).collect();
                    k.retain(|i, _| *i == id || cached.contains(i) || inflight.contains(i));
                    if k.len() > MAX_KEEPERS {
                        let mut ages: Vec<(u64, u64)> = k.iter().filter(|(i, _)| **i != id && !inflight.contains(i)).map(|(i, (t, _))| (*t, *i)).collect();
                        ages.sort_unstable();
                        let gone: HashSet<u64> = ages.into_iter().take(k.len().saturating_sub(MAX_KEEPERS / 2)).map(|(_, i)| i).collect();
                        k.retain(|i, _| !gone.contains(i));
                        c.drop_contents(&gone);
                    }
                }
            }
        }
        id
    }

    /// The [`comp_content`] identity of `comp` in `project`, or the comp's last new identity when
    /// every comp it draws is the same or draws the same pixels there ([`Comp::same_pixels`]:
    /// only Audio, Lock or Shy switches changed), so toggling those keeps the cached frames
    /// (#103). That identity hashes the addresses of the comps in its project, which is
    /// returned to keep them.
    fn identity(&self, project: &Arc<Project>, comp: ItemId) -> Identity {
        let plain = comp_content(project, comp);
        let Ok(mut bases) = self.bases.lock() else { return (plain, None) };
        if let Some((base_id, base)) = bases.get(&comp.0)
            && *base_id != plain
            && comp_content_with(project, comp, |id, c| match base.item(id).map(|i| &i.kind) {
                Some(ItemKind::Comp(b)) if Arc::ptr_eq(b, c) || b.same_pixels(c) => Arc::as_ptr(b),
                _ => Arc::as_ptr(c),
            }) == *base_id
        {
            return (*base_id, Some(base.clone()));
        }
        if bases.len() > MAX_KEEPERS {
            bases.clear();
        }
        bases.insert(comp.0, (plain, project.clone()));
        (plain, None)
    }

    pub fn is_cached(&self, k: &FrameKey) -> bool {
        self.cache.lock().map(|c| c.map.contains_key(k)).unwrap_or(false)
    }

    pub fn inflight(&self) -> usize {
        self.inflight.lock().map(|s| s.len()).unwrap_or(0)
    }

    /// Cached frame numbers of the series of `base` (its comp content, scale, view and render
    /// options) — for the cache bar.
    pub fn cached_frames(&self, base: &FrameKey) -> Vec<i64> {
        let Ok(c) = self.cache.lock() else { return vec![] };
        let mut v: Vec<i64> = c.map.keys().filter(|k| k.same_series(base)).map(|k| k.frame).collect();
        v.sort_unstable();
        v
    }

    /// Frames of the series of `base` in the disk cache but not in RAM — the blue cache bar.
    /// `opts` are the viewer's render options (part of the disk key).
    pub fn disk_frames(&self, src: &RenderSource, base: &FrameKey, frames: i64, opts: &RenderOpts) -> Vec<i64> {
        let remote = self.remote.as_ref().filter(|r| r.disk());
        let contains = |k: u128| match (&src.disk, remote) {
            (Some(dc), _) => dc.contains(disk_cache::Kind::Frame, k),
            (None, Some(r)) => r.disk_contains(k),
            (None, None) => false,
        };
        if src.disk.is_none() && remote.is_none() {
            return vec![];
        }
        let ram: HashSet<i64> = self.cached_frames(base).into_iter().collect();
        let oh = opts_hash(opts);
        (0..frames.clamp(0, 100_000))
            .filter(|f| !ram.contains(f))
            .filter(|f| contains(frame_disk_key(&self.content_keys, &src.project, &FrameKey { frame: *f, ..*base }, oh)))
            .collect()
    }

    /// Change the RAM preview cache budget (Settings ▸ Memory & CPU), evicting the oldest
    /// frames when it shrinks.
    pub fn set_budget(&self, bytes: usize) {
        let changed = if let Ok(mut c) = self.cache.lock() {
            let changed = c.budget != bytes;
            c.budget = bytes;
            c.evict_to_budget();
            changed
        } else {
            false
        };
        // Do not take failure state while holding cache: completion takes failure then cache.
        if changed {
            self.retry_failed_previews();
        }
    }

    pub fn budget(&self) -> usize {
        self.cache.lock().map(|c| c.budget).unwrap_or(0)
    }

    /// Forget failure backoff after an explicit retry, purge or device/worker recovery.
    /// This does not duplicate or cancel an existing in-flight job.
    pub fn retry_failed_previews(&self) {
        if let Ok(mut f) = self.failures.lock() {
            f.recover();
        }
        if let Some(ctx) = &self.ctx {
            ctx.request_repaint();
        }
    }

    /// Reject an unusable cached CPU frame without clearing a valid older view.
    pub fn reject_presentation(&self, key: FrameKey, error: PreviewFailure) {
        if let Ok(mut failures) = self.failures.lock() {
            let epoch = failures.epoch;
            failures.record(key, epoch, error, self.failure_clock.elapsed());
            if let Ok(mut cache) = self.cache.lock() {
                cache.remove(&key);
            }
        }
    }

    pub fn failure(&self, key: &FrameKey) -> Option<PreviewFailure> {
        self.failures.lock().ok()?.get(key)
    }

    pub fn clear(&self) {
        self.retry_failed_previews();
        if let Some(r) = &self.remote {
            r.purge();
        }
        if let Ok(mut c) = self.cache.lock() {
            c.map.clear();
            c.order.clear();
            c.bytes = 0;
            c.gpu_bytes = 0;
        }
    }

    /// Queue a prefetch frame (no-op if cached or already queued/rendering).
    pub fn request(&self, src: &RenderSource, key: FrameKey, comp: ItemId, t: Tick, opts: RenderOpts) {
        self.request_with(src, key, comp, t, opts, false);
    }

    /// Queue the frame the viewer is showing: it jumps ahead of every prefetch job.
    pub fn request_urgent(&self, src: &RenderSource, key: FrameKey, comp: ItemId, t: Tick, opts: RenderOpts) {
        self.request_with(src, key, comp, t, opts, true);
    }

    /// An urgent (viewer) frame is queued or rendering.
    pub fn urgent_pending(&self) -> bool {
        self.queue.lock().map(|q| q.urgent_running > 0 || q.jobs.iter().any(|j| j.urgent)).unwrap_or(false)
    }

    fn request_with(&self, src: &RenderSource, key: FrameKey, comp: ItemId, t: Tick, opts: RenderOpts, urgent: bool) {
        if let Some(delay) = self.failures.lock().ok().and_then(|f| f.delay(&key, self.failure_clock.elapsed())) {
            if let Some(ctx) = &self.ctx
                && delay != std::time::Duration::MAX
            {
                ctx.request_repaint_after(delay);
            }
            return;
        }
        if self.is_cached(&key) {
            return;
        }
        let Ok(mut q) = self.queue.lock() else { return };
        // Jobs for an older project revision can never be shown: drop them.
        let before = q.jobs.len();
        q.jobs.retain(|j| j.key.revision >= key.revision);
        let dropped = before - q.jobs.len();
        if dropped > 0
            && let Ok(mut inf) = self.inflight.lock()
        {
            inf.retain(|k| k.revision >= key.revision || q.running.contains(k));
        }
        if urgent {
            // Only the newest viewer frame is urgent; earlier ones become prefetch.
            for j in q.jobs.iter_mut() {
                j.urgent = j.key == key;
            }
        }
        {
            let Ok(mut inf) = self.inflight.lock() else { return };
            if !inf.insert(key) {
                return;
            }
        }
        q.seq += 1;
        let seq = q.seq;
        let failure_epoch = self.failures.lock().map(|f| f.epoch).unwrap_or(0);
        q.jobs.push(Job { key, src: src.clone(), comp, t, opts, urgent, seq, failure_epoch });
        drop(q);
        #[cfg(not(target_arch = "wasm32"))]
        if self.remote.is_none() {
            let w = self.worker();
            // One spawn per job; each spawn renders whichever queued job matters most right now.
            let job = move || {
                w.run_next();
            };
            match &self.pool {
                Some(pool) => pool.spawn(job),
                None => rayon::spawn(job),
            }
        }
        #[cfg(target_arch = "wasm32")]
        if let Some(ctx) = &self.ctx {
            // rendered by `pump` after this UI frame
            ctx.request_repaint();
        }
    }

    fn worker(&self) -> Worker {
        Worker {
            queue: self.queue.clone(),
            cache: self.cache.clone(),
            inflight: self.inflight.clone(),
            ctx: self.ctx.clone(),
            last: self.last_ms.clone(),
            content_keys: self.content_keys.clone(),
            gpu_retired: self.gpu_retired.clone(),
            failures: self.failures.clone(),
            failure_clock: self.failure_clock,
        }
    }

    /// Render frames with `remote` from now on (`None`: on this thread again).
    pub fn set_remote(&mut self, remote: Option<Arc<dyn RemoteFrames>>) {
        self.retry_failed_previews();
        self.remote = remote;
    }

    /// The remote renderer keeps a disk cache (the browser's).
    pub fn remote_disk(&self) -> bool {
        self.remote.as_ref().is_some_and(|r| r.disk())
    }

    /// Whether frames go to a usable [`RemoteFrames`].
    pub fn remote_active(&self) -> bool {
        self.remote.as_ref().is_some_and(|r| r.slots() > 0)
    }

    /// Frames the remote renderer renders at once (0 without one).
    pub fn remote_slots(&self) -> usize {
        self.remote.as_ref().map(|r| r.slots()).unwrap_or(0)
    }

    /// Frames being rendered remotely.
    pub fn remote_busy(&self) -> usize {
        self.remote_busy.lock().map(|b| *b).unwrap_or(0)
    }

    /// Hand queued frames to the remote renderer while it has free slots, most important first
    /// (the viewer's frame, then prefetch in request order; a viewer frame requested while every
    /// slot is busy goes to the first one that frees up). Frames that the GPU compositor can keep
    /// on the GPU render here instead (no readback). Returns how many were started.
    pub fn dispatch_remote(&self) -> usize {
        let Some(remote) = self.remote.clone() else { return 0 };
        let slots = remote.slots();
        let w = self.worker();
        let mut started = 0;
        loop {
            let busy = self.remote_busy();
            if busy >= slots {
                break;
            }
            let job = {
                let Ok(mut q) = self.queue.lock() else { break };
                let Some(i) = Queue::best(&q.jobs) else { break };
                let job = q.jobs.swap_remove(i);
                q.running.insert(job.key);
                if job.urgent {
                    q.urgent_running += 1;
                }
                job
            };
            // The GPU compositor here keeps the frame on the GPU, but runs layer effects on the
            // CPU (this thread can't wait for readbacks): frames with effects go to workers
            // that render them on their own GPU.
            let gpu_here = job.src.gpu.is_some() && job.src.gpu_display && !(remote.gpu() && comp_has_effects(&job.src.project, job.comp));
            if gpu_here {
                let t0 = web_time::Instant::now();
                if let Some(img) = w.render_gpu(&job) {
                    w.finish(&job, img, t0);
                    continue;
                }
            }
            if let Ok(mut b) = self.remote_busy.lock() {
                *b += 1;
            }
            started += 1;
            let (key, urgent) = (job.key, job.urgent);
            let fw = self.worker();
            let busy = self.remote_busy.clone();
            let t0 = web_time::Instant::now();
            let disk_key = remote.disk().then(|| frame_disk_key(&w.content_keys, &job.src.project, &key, opts_hash(&job.opts)));
            let rj = RemoteJob { project: job.src.project.clone(), revision: key.revision, comp: job.comp, t: job.t, opts: job.opts, disk_key };
            let failure_epoch = job.failure_epoch;
            remote.start_checked(
                rj,
                Box::new(move |r: Result<RemoteFrame, PreviewFailure>| {
                    if let Ok(mut b) = busy.lock() {
                        *b = b.saturating_sub(1);
                    }
                    match r {
                        Ok(f) => {
                            let budget = fw.cache.lock().map(|c| c.budget).unwrap_or(0);
                            let img = match failures::remote_image(&f, budget, fw.ctx.as_ref().map(|ctx| ctx.input(|i| i.max_texture_side))) {
                                Ok(img) => img,
                                Err(error) => {
                                    fw.failed(key, urgent, failure_epoch, error);
                                    return;
                                }
                            };
                            fw.finish_key(key, urgent, FrameImage::Cpu(Arc::new(img)), t0, failure_epoch);
                        }
                        Err(e) => {
                            // Unknown transport failures wait for bounded backoff, never permanent string-based rejection.
                            log::debug!("remote frame: {}", e.message());
                            fw.failed(key, urgent, failure_epoch, e);
                        }
                    }
                }),
            );
        }
        started
    }

    /// Render queued frames, most important first. With a usable [`RemoteFrames`] they are only
    /// handed over ([`Frames::dispatch_remote`]); otherwise they render on this thread until the
    /// queue is empty or `budget` has passed (at least one frame). Returns whether frames are
    /// still queued or rendering remotely. Only wasm32 needs this (native frames render on the
    /// pool); elsewhere it does nothing.
    pub fn pump(&self, budget: std::time::Duration) -> bool {
        if cfg!(not(target_arch = "wasm32")) {
            return false;
        }
        if self.remote_active() {
            self.dispatch_remote();
            return self.queue.lock().map(|q| !q.jobs.is_empty()).unwrap_or(false) || self.remote_busy() > 0;
        }
        let t0 = web_time::Instant::now();
        let w = self.worker();
        while w.run_next() {
            if t0.elapsed() >= budget {
                break;
            }
        }
        self.queue.lock().map(|q| !q.jobs.is_empty()).unwrap_or(false)
    }
}

/// Renders queued jobs (on a pool thread, or the UI thread on wasm32).
struct Worker {
    queue: Arc<Mutex<Queue>>,
    cache: Arc<Mutex<Cache>>,
    inflight: Arc<Mutex<HashSet<FrameKey>>>,
    ctx: Option<egui::Context>,
    last: Arc<Mutex<f64>>,
    content_keys: ContentKeys,
    gpu_retired: Arc<AtomicBool>,
    failures: Arc<Mutex<failures::Failures>>,
    failure_clock: web_time::Instant,
}

impl Worker {
    /// Render the queued job that matters most right now. `false` when the queue was empty.
    fn run_next(&self) -> bool {
        let Worker { queue, content_keys, .. } = self;
        let job = {
            let Ok(mut q) = queue.lock() else { return false };
            let Some(i) = Queue::best(&q.jobs) else { return false };
            let job = q.jobs.swap_remove(i);
            q.running.insert(job.key);
            if job.urgent {
                q.urgent_running += 1;
            }
            job
        };
        let t0 = web_time::Instant::now();
        let disk = job.src.disk.as_ref().map(|dc| (dc.clone(), frame_disk_key(content_keys, &job.src.project, &job.key, opts_hash(&job.opts))));
        let from_disk = disk.as_ref().and_then(|(dc, k)| dc.get_frame(*k));
        let disk_hit = from_disk.is_some();
        let ci = match from_disk {
            Some(f) => FrameImage::Cpu(Arc::new(egui::ColorImage::from_rgba_premultiplied([f.width as usize, f.height as usize], &f.rgba))),
            None => match self.render_guarded(&job) {
                Some(ci) => ci,
                None => {
                    // Not "in progress" forever: the viewer asks for it again.
                    self.failed(job.key, job.urgent, job.failure_epoch, PreviewFailure::new(FailureKind::Retryable, "Preview rendering panicked"));
                    return true;
                }
            },
        };
        // Publish the shared frame and wake the viewer before optional RGBA conversion
        // and compression. Persistence still runs on this worker, using the same snapshot.
        if !self.finish(&job, ci.clone(), t0) {
            return true;
        }
        if !disk_hit && let (Some((dc, k)), FrameImage::Cpu(img)) = (&disk, &ci) {
            let rgba: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
            dc.put_frame(*k, img.size[0] as u32, img.size[1] as u32, &rgba);
            if let Some(ctx) = &self.ctx {
                ctx.request_repaint();
            }
        }
        true
    }

    /// Store a finished frame and release its job.
    fn finish(&self, job: &Job, img: FrameImage, t0: web_time::Instant) -> bool {
        self.finish_key(job.key, job.urgent, img, t0, job.failure_epoch)
    }

    fn failed(&self, key: FrameKey, urgent: bool, epoch: u64, failure: PreviewFailure) {
        if let Ok(mut f) = self.failures.lock() {
            f.record(key, epoch, failure, self.failure_clock.elapsed());
        }
        self.release(key, urgent);
    }

    fn finish_key(&self, key: FrameKey, urgent: bool, img: FrameImage, t0: web_time::Instant, epoch: u64) -> bool {
        if let (Some(ctx), FrameImage::Cpu(cpu)) = (&self.ctx, &img)
            && let Err(error) = presentation_image(ctx, cpu)
        {
            self.failed(key, urgent, epoch, error);
            return false;
        }
        // Hold the generation guard through publication, so recovery/new failures cannot race
        // a late success. Release it BEFORE queue bookkeeping (request takes queue then epoch).
        let Ok(mut failures) = self.failures.lock() else {
            self.release(key, urgent);
            return false;
        };
        if epoch != failures.epoch {
            drop(failures);
            self.release(key, urgent);
            return false;
        }
        let mut published = false;
        if let Ok(mut c) = self.cache.lock()
            && (!img.is_gpu() || !self.gpu_retired.load(Ordering::Acquire))
        {
            c.insert(key, img);
            failures.succeeded(&key);
            published = true;
        }
        drop(failures);
        if published
            && urgent
            && let Ok(mut l) = self.last.lock()
        {
            *l = t0.elapsed().as_secs_f64() * 1000.0;
        }
        self.release(key, urgent);
        published
    }
    /// The job of `key` is no longer queued or running (rendered or dropped).
    fn release(&self, key: FrameKey, urgent: bool) {
        if let Ok(mut inf) = self.inflight.lock() {
            inf.remove(&key);
        }
        if let Ok(mut q) = self.queue.lock() {
            q.running.remove(&key);
            if urgent {
                q.urgent_running = q.urgent_running.saturating_sub(1);
            }
        }
        if let Some(ctx) = &self.ctx {
            ctx.request_repaint();
        }
    }

    /// [`Worker::render_job`], again on the CPU if it panicked (the panic hook has logged it);
    /// `None` if that panicked too.
    fn render_guarded(&self, job: &Job) -> Option<FrameImage> {
        let attempt = |gpu: bool| std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.render_job(job, gpu))).ok();
        attempt(true).or_else(|| {
            log::error!("a frame render panicked (frame {})", job.key.frame);
            job.src.gpu.as_ref().and_then(|_| attempt(false))
        })
    }

    /// A failed scoped attempt retries on CPU. Retired contexts cannot recover by reducing
    /// their budget; only a healthy device's allocation failure uses the upstream backoff.
    fn gpu_attempt_failed(&self, gpu: &Gpu, e: &str) {
        if gpu.context().check_health().is_err() {
            log::warn!("the GPU context is retired ({e}); the frame renders on the CPU");
            return;
        }
        log::warn!("the GPU ran out of memory ({e}); the frame renders on the CPU and fewer GPU frames are kept");
        if let Ok(mut c) = self.cache.lock() {
            c.gpu_out_of_memory();
        }
    }

    /// The frame on the GPU, kept there for the viewer, if the compositor can do it alone.
    fn render_gpu(&self, job: &Job) -> Option<FrameImage> {
        if self.gpu_retired.load(Ordering::Acquire) {
            return None;
        }
        let g = job.src.gpu.as_ref()?;
        let mut r = Renderer::new(&job.src.project, job.src.footage.as_ref(), job.opts);
        r.expr = job.src.expr.as_deref();
        r.cache = Some(&job.src.layer_cache);
        r.accel = Some(g as &dyn effectcraft_engine::render::Accelerator);
        r.active_accel()?;
        match g.within_memory(|| g.render_display(&r, job.comp, job.t)) {
            Ok(f) => f.map(|f| FrameImage::Gpu(Arc::new(f))),
            Err(e) => {
                self.gpu_attempt_failed(g, &e);
                None
            }
        }
    }

    /// Render a frame; `gpu: false` keeps it off the GPU compositor.
    fn render_job(&self, job: &Job, gpu: bool) -> FrameImage {
        let mut r = Renderer::new(&job.src.project, job.src.footage.as_ref(), job.opts);
        r.expr = job.src.expr.as_deref();
        let gpu_allowed = gpu && !self.gpu_retired.load(Ordering::Acquire);
        // A failed GPU attempt may have populated intermediate entries before its scope
        // reported failure. The CPU retry must compute its own pixels from the snapshot.
        r.cache = gpu_allowed.then_some(job.src.layer_cache.as_ref());
        let gpu = job.src.gpu.as_ref().filter(|_| gpu_allowed);
        r.accel = gpu.map(|g| g as &dyn effectcraft_engine::render::Accelerator);
        // The GPU leaves the frame in a texture for the viewer; otherwise (Software Only, no
        // adapter, or a frame the GPU cannot finish here) the CPU renders it.
        // Auto (Mercury GPU Acceleration) shows each comp on whichever compositor measured
        // faster for it (light comps composite faster on the CPU).
        if let (Some(g), Some(a)) = (gpu, r.active_accel())
            && job.src.gpu_display
        {
            // The browser cannot wait for the GPU to time it (and its CPU path is slow): always
            // the GPU there.
            let auto = if cfg!(target_arch = "wasm32") { None } else { r.frame_auto(a, job.comp, true) };
            if auto.is_none_or(|(p, key)| p.choose(key)) {
                let t0 = web_time::Instant::now();
                match g.within_memory(|| g.render_display(&r, job.comp, job.t)) {
                    Ok(Some(f)) => {
                        if let Some((p, key)) = auto {
                            // Time the GPU's work, not just its recording.
                            g.wait();
                            if g.context().check_health().is_err() {
                                return self.render_job(job, false);
                            }
                            p.record(key, true, t0.elapsed().as_secs_f64() * 1000.0);
                        }
                        return FrameImage::Gpu(Arc::new(f));
                    }
                    Ok(None) => {
                        if let Some((p, key)) = auto {
                            p.declined(key);
                        }
                    }
                    Err(e) => {
                        self.gpu_attempt_failed(g, &e);
                        return self.render_job(job, false);
                    }
                }
            }
            let t0 = web_time::Instant::now();
            let img = to_color_image(&r.comp_frame_cpu(job.comp, job.t));
            if let Some((p, key)) = auto {
                p.record(key, false, t0.elapsed().as_secs_f64() * 1000.0);
            }
            return FrameImage::Cpu(Arc::new(img));
        }
        if let Some(g) = gpu {
            // The GPU composites and the frame is read back.
            return match g.within_memory(|| r.comp_frame(job.comp, job.t)) {
                Ok(img) => FrameImage::Cpu(Arc::new(to_color_image(&img))),
                Err(e) => {
                    self.gpu_attempt_failed(g, &e);
                    self.render_job(job, false)
                }
            };
        }
        FrameImage::Cpu(Arc::new(to_color_image(&r.comp_frame(job.comp, job.t))))
    }
}

struct Job {
    key: FrameKey,
    src: RenderSource,
    comp: ItemId,
    t: Tick,
    opts: RenderOpts,
    urgent: bool,
    seq: u64,
    failure_epoch: u64,
}

#[derive(Default)]
struct Queue {
    jobs: Vec<Job>,
    running: HashSet<FrameKey>,
    urgent_running: usize,
    seq: u64,
}

impl Queue {
    /// The job that matters most: urgent first (newest), then prefetch in request order.
    fn best(jobs: &[Job]) -> Option<usize> {
        jobs.iter().enumerate().max_by_key(|(_, j)| (j.urgent, if j.urgent { j.seq as i64 } else { -(j.seq as i64) })).map(|(i, _)| i)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retirement_reaches_existing_workers_and_keeps_cpu_frames_and_completion_live() {
        let frames = Frames::default();
        let worker = frames.worker();
        let first = key(1, 0, 0);
        insert(&frames, first);
        frames.retire_gpu();
        assert!(worker.gpu_retired.load(Ordering::Acquire), "already queued workers share retirement");
        assert!(frames.is_cached(&first), "healthy materialized CPU frames survive");
        let next = key(1, 1, 0);
        frames.inflight.lock().unwrap().insert(next);
        frames.queue.lock().unwrap().running.insert(next);
        let epoch = frames.failures.lock().unwrap().epoch;
        worker.finish_key(next, false, img(), web_time::Instant::now(), epoch);
        assert!(frames.is_cached(&next), "CPU retry can still publish");
        assert_eq!(frames.inflight(), 0);
        assert!(frames.queue.lock().unwrap().running.is_empty());
    }

    fn key(content: u64, frame: i64, opts: u64) -> FrameKey {
        FrameKey { revision: content, content, comp: 1, frame, scale: 1000, view: 0, opts }
    }

    fn img() -> FrameImage {
        FrameImage::Cpu(Arc::new(egui::ColorImage::new([2, 2], vec![egui::Color32::BLACK; 4])))
    }

    fn insert(f: &Frames, k: FrameKey) {
        f.cache.lock().unwrap().insert(k, img());
    }

    /// A frame whose render panics (a GPU out of memory used to, on every frame thread) is
    /// released, not left "in progress" forever, so the viewer can ask for it again (#106).
    #[test]
    fn a_frame_whose_render_panics_is_released() {
        use effectcraft_engine::Session;
        use effectcraft_engine::project::{Footage, FootageKind};
        use effectcraft_engine::render::Image;
        use serde_json::json;
        use std::sync::atomic::{AtomicUsize, Ordering};
        #[derive(Default)]
        struct Explodes(AtomicUsize);
        impl FootageSource for Explodes {
            fn frame(&self, _: ItemId, _: &Footage, _: Tick) -> Option<Arc<Image>> {
                self.0.fetch_add(1, Ordering::SeqCst);
                panic!("a decoder bug");
            }
        }
        let mut s = Session::default();
        let cid = ItemId(s.execute("comp.new", json!({"name": "A", "width": 8, "height": 8, "duration": 1})).unwrap()["comp"].as_u64().unwrap());
        let clip = Footage { kind: FootageKind::Still, width: 8, height: 8, has_video: true, ..Default::default() };
        let item = Arc::make_mut(&mut s.project).add_item("clip.png", Default::default(), None, ItemKind::Footage(clip));
        s.execute("layer.addItem", json!({"item": item.0})).unwrap();
        let footage = Arc::new(Explodes::default());
        let src = RenderSource {
            project: s.project.clone(),
            footage: footage.clone(),
            expr: None,
            layer_cache: Arc::default(),
            gpu: None,
            gpu_display: false,
            disk: None,
        };
        let f = Frames::default();
        let key = FrameKey { revision: s.revision, content: f.content_of(&s.project, s.revision, cid), ..key(0, 0, 0) };
        let key = FrameKey { comp: cid.0, ..key };
        f.request(&src, key, cid, Tick::ZERO, RenderOpts::default());
        for _ in 0..1000 {
            if f.inflight() == 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(footage.0.load(Ordering::SeqCst) > 0, "the render reached the footage");
        assert_eq!(f.inflight(), 0, "released");
        assert!(!f.is_cached(&key));
    }

    /// Each time the GPU runs out of memory, half as many GPU frames are kept, down to a floor.
    #[test]
    fn running_out_of_video_memory_halves_the_gpu_frame_budget() {
        let f = Frames::default();
        let mut c = f.cache.lock().unwrap();
        assert_eq!(c.gpu_budget, GPU_BUDGET);
        c.gpu_out_of_memory();
        assert_eq!(c.gpu_budget, GPU_BUDGET / 2);
        for _ in 0..20 {
            c.gpu_out_of_memory();
        }
        assert_eq!(c.gpu_budget, MIN_GPU_BUDGET);
    }

    #[test]
    fn least_recently_shown_frames_go_first() {
        let f = Frames::default();
        f.set_budget(3 * 16);
        for i in 0..3 {
            insert(&f, key(1, i, 0));
        }
        // Frame 0 is shown again, so frame 1 is now the oldest.
        assert!(f.get(&key(1, 0, 0)).is_some());
        insert(&f, key(1, 3, 0));
        assert!(f.is_cached(&key(1, 0, 0)) && !f.is_cached(&key(1, 1, 0)));
        assert!(f.is_cached(&key(1, 2, 0)) && f.is_cached(&key(1, 3, 0)));
    }

    #[test]
    fn frames_are_keyed_by_content_and_series_are_kept_apart() {
        let f = Frames::default();
        insert(&f, key(1, 0, 0));
        insert(&f, key(2, 0, 0));
        insert(&f, key(2, 1, 7));
        // Other content or render options (Draft, shadows…) are another series.
        assert_eq!(f.cached_frames(&key(2, 0, 0)), vec![0]);
        assert_eq!(f.cached_frames(&key(2, 0, 7)), vec![1]);
        // A later revision with the same content finds the frame (an edit elsewhere, an undo).
        assert!(f.is_cached(&FrameKey { revision: 99, ..key(1, 0, 0) }));
        assert!(!f.is_cached(&key(3, 0, 0)));
    }

    #[test]
    fn an_edit_keeps_the_frames_of_comps_it_does_not_touch() {
        use effectcraft_engine::Session;
        use serde_json::json;
        let mut s = Session::default();
        let a = ItemId(s.execute("comp.new", json!({"name": "A", "width": 8, "height": 8, "duration": 1})).unwrap()["comp"].as_u64().unwrap());
        let b = ItemId(s.execute("comp.new", json!({"name": "B", "width": 8, "height": 8, "duration": 1})).unwrap()["comp"].as_u64().unwrap());
        let f = Frames::default();
        let at = |s: &Session, c: ItemId| f.content_of(&s.project, s.revision, c);
        let (a0, b0) = (at(&s, a), at(&s, b));
        // Edit B: A's content (and frames) stay, B's change.
        s.execute("layer.newSolid", json!({"comp": b.0, "color": "#ff0000"})).unwrap();
        assert_eq!(at(&s, a), a0);
        assert_ne!(at(&s, b), b0);
        // Undo: B is back to the content its old frames were rendered for.
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(at(&s, b), b0);
    }

    #[test]
    fn a_long_drag_keeps_a_bounded_number_of_states() {
        use effectcraft_engine::Session;
        use serde_json::json;
        let mut s = Session::default();
        let a = ItemId(s.execute("comp.new", json!({"name": "A", "width": 8, "height": 8, "duration": 1})).unwrap()["comp"].as_u64().unwrap());
        let f = Frames::default();
        let mut last = None;
        for i in 0..200 {
            s.execute("comp.settings", json!({"width": 8 + i, "merge": "drag"})).unwrap();
            let k = FrameKey { content: f.content_of(&s.project, s.revision, a), ..key(0, 0, 0) };
            insert(&f, k);
            last = Some(k);
        }
        // The snapshots and the frames kept stay bounded (a dropped identity's frames go with it,
        // so a later state that reuses its addresses can't be shown an old frame).
        assert!(f.keepers.lock().unwrap().1.len() <= MAX_KEEPERS);
        assert!(f.cache.lock().unwrap().map.len() <= MAX_KEEPERS);
        assert!(f.is_cached(&last.unwrap()), "the current state's frame stays");
    }

    #[test]
    fn an_identity_keeps_its_comps_so_an_edit_in_place_still_changes_it() {
        use effectcraft_engine::Session;
        use serde_json::json;
        let mut s = Session::default();
        let a = ItemId(s.execute("comp.new", json!({"name": "A", "width": 8, "height": 8, "duration": 1})).unwrap()["comp"].as_u64().unwrap());
        // A project nothing else holds (no undo history): make_mut would edit the comp in place,
        // at the same address, if the identity hadn't kept a snapshot holding it.
        let mut p = Arc::new((*s.project).clone());
        drop(s);
        let f = Frames::default();
        let before = f.content_of(&p, 1, a);
        let comp = Arc::make_mut(&mut p).comp_mut(a).unwrap();
        comp.width = 16;
        assert_ne!(f.content_of(&p, 2, a), before);
    }

    /// Audio, Lock and Shy (and Hide Shy Layers) don't change pixels: toggling them, in the comp
    /// or a comp it nests, keeps its frames, also when toggled back (#103).
    #[test]
    fn audio_lock_and_shy_switches_keep_the_frames() {
        use effectcraft_engine::Session;
        use serde_json::json;
        let mut s = Session::default();
        let id = |v: serde_json::Value, k: &str| v[k].as_u64().unwrap();
        let inner = ItemId(id(s.execute("comp.new", json!({"name": "Inner", "width": 8, "height": 8, "duration": 1})).unwrap(), "comp"));
        let solid = id(s.execute("layer.newSolid", json!({"comp": inner.0, "color": "#ff0000"})).unwrap(), "layer");
        let outer = ItemId(id(s.execute("comp.new", json!({"name": "Outer", "width": 8, "height": 8, "duration": 1})).unwrap(), "comp"));
        let nested = id(s.execute("layer.addItem", json!({"comp": outer.0, "item": inner.0})).unwrap(), "layer");
        let f = Frames::default();
        let at = |s: &Session| f.content_of(&s.project, s.revision, outer);
        let o0 = at(&s);
        let switch = |s: &mut Session, comp: ItemId, layer: u64, sw: &str, v: bool| {
            s.execute("layer.setSwitch", json!({"comp": comp.0, "layers": [layer], "switch": sw, "value": v})).unwrap();
        };
        for (comp, layer, sw) in [(outer, nested, "audio"), (outer, nested, "lock"), (outer, nested, "shy"), (inner, solid, "audio"), (inner, solid, "shy")] {
            // Audio off; Lock and Shy on.
            switch(&mut s, comp, layer, sw, sw != "audio");
            assert_eq!(at(&s), o0, "{sw} on in {comp:?}");
        }
        s.execute("comp.setSwitch", json!({"comp": outer.0, "switch": "hideShy", "value": true})).unwrap();
        assert_eq!(at(&s), o0, "Hide Shy Layers");
        switch(&mut s, outer, nested, "audio", true);
        switch(&mut s, outer, nested, "lock", false);
        assert_eq!(at(&s), o0, "toggled back");
        // Switches that change pixels still do.
        switch(&mut s, inner, solid, "video", false);
        let o1 = at(&s);
        assert_ne!(o1, o0, "Video");
        switch(&mut s, outer, nested, "audio", false);
        assert_eq!(at(&s), o1, "a mute after a real edit keeps that edit's frames");
        switch(&mut s, inner, solid, "video", true);
        assert_ne!(at(&s), o1);
    }
}

#[cfg(test)]
mod failure_dispatch_tests {
    use super::*;
    use effectcraft_engine::Session;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct TypedRemote {
        calls: AtomicUsize,
        good: AtomicBool,
    }
    impl RemoteFrames for TypedRemote {
        fn slots(&self) -> usize {
            1
        }
        fn start(&self, _: RemoteJob, _: RemoteDone) {
            panic!("typed route must be used");
        }
        fn start_checked(&self, _: RemoteJob, done: CheckedRemoteDone) {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.good.load(Ordering::SeqCst) {
                done(Ok(RemoteFrame { width: 1, height: 1, rgba: vec![20, 40, 60, 255], ms: 1.0 }));
            } else {
                done(Err(PreviewFailure::new(FailureKind::Content, "synthetic admission error")));
            }
        }
    }
    fn fixture() -> (Frames, RenderSource, ItemId, FrameKey, Arc<TypedRemote>) {
        let mut s = Session::default();
        let cid = ItemId(s.execute("comp.new", json!({"width": 8, "height": 8, "duration": 1})).unwrap()["comp"].as_u64().unwrap());
        let source = RenderSource {
            project: s.project.clone(),
            footage: s.footage.clone(),
            expr: None,
            layer_cache: Arc::default(),
            gpu: None,
            gpu_display: false,
            disk: None,
        };
        let remote = Arc::new(TypedRemote { calls: AtomicUsize::new(0), good: AtomicBool::new(false) });
        let mut f = Frames::default();
        f.set_remote(Some(remote.clone()));
        let key = FrameKey { revision: 1, content: 1, comp: cid.0, frame: 0, scale: 1000, view: 0, opts: 0 };
        (f, source, cid, key, remote)
    }
    #[test]
    fn repeated_paint_does_not_redispatch_a_typed_failure_and_edit_retry_success_recover() {
        let (f, src, cid, key, remote) = fixture();
        for _ in 0..1000 {
            f.request_urgent(&src, key, cid, Tick::ZERO, RenderOpts::default());
            f.dispatch_remote();
        }
        assert_eq!(remote.calls.load(Ordering::SeqCst), 1);
        assert_eq!(f.inflight(), 0);
        assert_eq!(f.remote_busy(), 0);
        assert!(!f.urgent_pending());
        assert!(!f.is_cached(&key));
        assert_eq!(f.failure(&key).unwrap().kind, FailureKind::Content);
        let edited = FrameKey { content: 2, revision: 2, ..key };
        remote.good.store(true, Ordering::SeqCst);
        f.request_urgent(&src, edited, cid, Tick::ZERO, RenderOpts::default());
        assert_eq!(f.dispatch_remote(), 1);
        assert!(f.is_cached(&edited), "changed content is not blocked by old error");
        f.retry_failed_previews();
        f.request_urgent(&src, key, cid, Tick::ZERO, RenderOpts::default());
        assert_eq!(f.dispatch_remote(), 1);
        assert!(f.is_cached(&key));
        assert!(f.failure(&key).is_none());
        assert_eq!(remote.calls.load(Ordering::SeqCst), 3);
    }
    #[test]
    fn failed_options_and_frames_do_not_block_other_requests_and_success_clears_diagnostics() {
        let (f, src, cid, key, remote) = fixture();
        f.request(&src, key, cid, Tick::ZERO, RenderOpts::default());
        f.dispatch_remote();
        remote.good.store(true, Ordering::SeqCst);
        for next in [FrameKey { frame: 1, ..key }, FrameKey { opts: 1, ..key }, FrameKey { view: 1, ..key }] {
            f.request(&src, next, cid, Tick::ZERO, RenderOpts::default());
            assert_eq!(f.dispatch_remote(), 1);
            assert!(f.is_cached(&next));
        }
        // A valid materialized success for the original request resolves its diagnostic.
        let epoch = f.failures.lock().unwrap().epoch;
        f.worker().finish_key(
            key,
            false,
            FrameImage::Cpu(Arc::new(egui::ColorImage::new([1, 1], vec![egui::Color32::WHITE]))),
            web_time::Instant::now(),
            epoch,
        );
        assert!(f.failure(&key).is_none());
        assert!(f.is_cached(&key));
        assert_eq!(remote.calls.load(Ordering::SeqCst), 4);
    }
    #[test]
    fn legacy_error_strings_use_retryable_backoff_without_permanent_string_classification() {
        struct OldRemote(AtomicUsize);
        impl RemoteFrames for OldRemote {
            fn slots(&self) -> usize {
                1
            }
            fn start(&self, _: RemoteJob, done: RemoteDone) {
                self.0.fetch_add(1, Ordering::SeqCst);
                done(Err("invalid composition; stale worker; out of memory".into()));
            }
        }
        let (mut f, src, cid, key, _) = fixture();
        let old = Arc::new(OldRemote(AtomicUsize::new(0)));
        f.set_remote(Some(old.clone()));
        f.request(&src, key, cid, Tick::ZERO, RenderOpts::default());
        f.dispatch_remote();
        assert_eq!(f.failure(&key).unwrap().kind, FailureKind::Retryable);
        // Give the ledger an explicit logical instant; no timing sleeps/flaky repaint loop.
        assert!(f.failures.lock().unwrap().delay(&key, std::time::Duration::ZERO).is_some());
        assert!(f.failures.lock().unwrap().delay(&key, std::time::Duration::from_secs(60)).is_none());
        assert_eq!(old.0.load(Ordering::SeqCst), 1);
    }
    #[test]
    fn delayed_pre_recovery_success_cannot_clear_a_new_failure_or_publish_pixels() {
        struct Held(Mutex<Option<CheckedRemoteDone>>);
        impl RemoteFrames for Held {
            fn slots(&self) -> usize {
                1
            }
            fn start(&self, _: RemoteJob, _: RemoteDone) {
                panic!("typed route")
            }
            fn start_checked(&self, _: RemoteJob, done: CheckedRemoteDone) {
                *self.0.lock().unwrap() = Some(done);
            }
        }
        let (mut f, src, cid, key, _) = fixture();
        let held = Arc::new(Held(Mutex::default()));
        f.set_remote(Some(held.clone()));
        f.request_urgent(&src, key, cid, Tick::ZERO, RenderOpts::default());
        assert_eq!(f.dispatch_remote(), 1);
        f.retry_failed_previews();
        let epoch = f.failures.lock().unwrap().epoch;
        // Model a newer typed evaluator failure while the old transport completion is held.
        // Current scheduler still prevents concurrent same-key dispatch; this deliberately
        // exercises the publication seam needed by typed/native callers and PR115 leases.
        f.worker().failed(key, true, epoch, PreviewFailure::new(FailureKind::Content, "new generation error"));
        let done = held.0.lock().unwrap().take().unwrap();
        done(Ok(RemoteFrame { width: 1, height: 1, rgba: vec![0, 0, 0, 255], ms: 1.0 }));
        assert_eq!(f.failure(&key).unwrap().message(), "new generation error");
        assert!(!f.is_cached(&key), "late pre-recovery success is not published");
        assert_eq!(f.remote_busy(), 0);
        assert_eq!(f.inflight(), 0);
    }
    #[test]
    fn actual_input_limit_accepts_boundary_and_rejects_cached_frame_after_decrease() {
        struct Boundary;
        impl RemoteFrames for Boundary {
            fn slots(&self) -> usize {
                1
            }
            fn start(&self, _: RemoteJob, done: RemoteDone) {
                done(Ok(RemoteFrame { width: 8, height: 1, rgba: vec![40; 32], ms: 1.0 }));
            }
        }
        let (mut f, src, cid, key, _) = fixture();
        let ctx = egui::Context::default();
        ctx.input_mut(|i| i.max_texture_side = 8);
        f.set_context(&ctx);
        f.set_remote(Some(Arc::new(Boundary)));
        f.request_urgent(&src, key, cid, Tick::ZERO, RenderOpts::default());
        assert_eq!(f.dispatch_remote(), 1);
        assert!(f.get(&key).is_some());
        assert_eq!(f.remote_busy(), 0);
        ctx.input_mut(|i| i.max_texture_side = 7);
        f.set_context(&ctx);
        assert!(f.get(&key).is_none(), "same key is revalidated against the live presentation limit");
        assert!(!f.is_cached(&key));
        assert_eq!(f.failure(&key).unwrap().kind, FailureKind::Retryable);
        ctx.input_mut(|i| i.max_texture_side = 8);
        f.set_context(&ctx);
        assert!(f.failure(&key).is_none(), "device limit change releases backoff without a content edit");
        f.request_urgent(&src, key, cid, Tick::ZERO, RenderOpts::default());
        assert_eq!(f.dispatch_remote(), 1);
        assert!(f.get(&key).is_some());
    }
    #[test]
    fn invalid_or_over_budget_remote_pixels_release_once_without_publication() {
        struct Bad {
            size: [u32; 2],
            bytes: Vec<u8>,
            calls: AtomicUsize,
        }
        impl RemoteFrames for Bad {
            fn slots(&self) -> usize {
                1
            }
            fn start(&self, _: RemoteJob, done: RemoteDone) {
                self.calls.fetch_add(1, Ordering::SeqCst);
                done(Ok(RemoteFrame { width: self.size[0], height: self.size[1], rgba: self.bytes.clone(), ms: 1.0 }));
            }
        }
        for (size, bytes, budget) in [([u32::MAX; 2], vec![], 16), ([2, 2], vec![0; 3], 16), ([2, 2], vec![0; 16], 8), ([9, 1], vec![0; 36], 64)] {
            let (mut f, src, cid, key, _) = fixture();
            f.set_budget(budget);
            let ctx = egui::Context::default();
            // No font frame: this exercises actual input state without a tiny font atlas.
            ctx.input_mut(|i| i.max_texture_side = 8);
            f.set_context(&ctx);
            let bad = Arc::new(Bad { size, bytes, calls: AtomicUsize::new(0) });
            f.set_remote(Some(bad.clone()));
            f.request_urgent(&src, key, cid, Tick::ZERO, RenderOpts::default());
            assert_eq!(f.dispatch_remote(), 1);
            assert!(!f.is_cached(&key));
            assert_eq!(f.failure(&key).unwrap().kind, FailureKind::Retryable);
            assert_eq!(f.inflight(), 0);
            assert_eq!(f.remote_busy(), 0);
            assert!(!f.urgent_pending());
            assert_eq!(bad.calls.load(Ordering::SeqCst), 1);
            f.retry_failed_previews();
            assert!(f.failure(&key).is_none(), "explicit retry releases failure state");
            if size == [9, 1] {
                ctx.input_mut(|i| i.max_texture_side = 16);
                f.set_context(&ctx);
                f.request_urgent(&src, key, cid, Tick::ZERO, RenderOpts::default());
                assert_eq!(f.dispatch_remote(), 1);
                assert!(f.is_cached(&key), "live limit recovery admits the formerly rejected pixels");
                assert!(f.failure(&key).is_none());
                assert_eq!(f.remote_busy(), 0);
                assert_eq!(f.inflight(), 0);
            }
        }
    }
}
