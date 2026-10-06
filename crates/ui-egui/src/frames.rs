//! Background frame rendering and the RAM preview cache.
//!
//! Frames render on a dedicated thread pool from immutable `Arc<Project>` snapshots, so the UI
//! never waits for the compositor. Results land in a memory-budgeted cache keyed by the comp's
//! *content* ([`comp_content`]: the comp and everything it uses), frame, scale, view and render
//! options; the timeline draws the cached range as the green cache bar. An edit keeps the frames
//! of every comp it doesn't touch, and undo finds the frames of the state it returns to, as in
//! After Effects. When the budget is full the least recently shown frames go first.
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

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};

use effectcraft_engine::project::{ItemId, ItemKind, LayerSource, Node, Project, PropGroup};
use effectcraft_engine::render::disk_cache::{self, DiskCache};
use effectcraft_engine::render::{ExprHost, FootageSource, LayerCache, RenderOpts, Renderer};
use effectcraft_engine::time::Tick;
use effectcraft_gpu::{DisplayFrame, Gpu};
use rayon::prelude::*;

mod latency;
pub use latency::ViewerTiming;

/// Comp identities (and the project snapshots behind them) the RAM preview keeps frames of.
const MAX_KEEPERS: usize = 64;

/// GPU frames kept for RAM preview (bytes of video memory).
const GPU_BUDGET: usize = 1 << 30;

/// Paused edits need the newest frame, rather than a pool full of obsolete snapshots.
const INTERACTIVE_LIMIT: usize = 2;
/// Dynamic viewer tabs cannot consume the four reserved auxiliary/locked pane slots.
const MAX_SECONDARY_VIEWERS: usize = 64;

/// A stable secondary pane, independent of the content it currently displays.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SecondarySlot {
    Top,
    Front,
    Right,
    LockedSplit,
    Viewer(u32),
}

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
        // Video memory: drop the oldest GPU frames past their own budget.
        while self.gpu_bytes > GPU_BUDGET {
            let Some(i) = self.order.iter().position(|k| self.map.get(k).is_some_and(FrameImage::is_gpu)) else { break };
            if let Some(old) = self.order.remove(i) {
                self.remove(&old);
            }
        }
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
            ItemKind::Comp(c) => (Arc::as_ptr(c) as usize).hash(&mut h),
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
    // The key may carry an exact secondary-view Tick in addition to the render options.
    // Hash the fields separately: XOR would cancel the primary key's equal option hashes.
    let mut variant = disk_cache::Hash128::default();
    variant.write_u64(key.view);
    variant.write_u64(opts_hash);
    variant.write_u64(key.opts);
    disk_cache::frame_key(content, key.frame, key.scale, variant.finish() as u64)
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

/// Renders viewer frames elsewhere (the browser's frame worker), so the UI thread never does.
pub trait RemoteFrames: Send + Sync {
    /// Frames it can render at the same time (0 when it is not usable: frames then render on
    /// the UI thread again).
    fn slots(&self) -> usize;
    /// Start rendering `job`; `done` runs when the frame arrives.
    fn start(&self, job: RemoteJob, done: RemoteDone);
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
    latency: Arc<Mutex<latency::Latency>>,
    content_keys: ContentKeys,
    /// [`comp_content`] identities by (revision, comp).
    identities: Mutex<HashMap<(u64, u64), u64>>,
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
            cache: Arc::new(Mutex::new(Cache { map: HashMap::new(), order: VecDeque::new(), bytes: 0, gpu_bytes: 0, budget: 3 << 30 })),
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
            latency: Arc::default(),
            content_keys: Arc::default(),
            identities: Mutex::default(),
            keepers: Mutex::default(),
            remote: None,
            remote_busy: Arc::default(),
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
        c.touch(k);
        Some(img)
    }

    /// The [`comp_content`] identity of `comp` in `project` at `revision` (taken once per
    /// revision). The project is kept while frames of that identity are cached or rendering, so
    /// the comp addresses it hashes can't be reused by other comps meanwhile.
    pub fn content_of(&self, project: &Arc<Project>, revision: u64, comp: ItemId) -> u64 {
        let known = self.identities.lock().ok().and_then(|m| m.get(&(revision, comp.0)).copied());
        let id = known.unwrap_or_else(|| {
            let id = comp_content(project, comp);
            if let Ok(mut m) = self.identities.lock() {
                if m.len() > 256 {
                    m.clear();
                }
                m.insert((revision, comp.0), id);
            }
            id
        });
        if let Ok(mut guard) = self.keepers.lock() {
            let (clock, k) = &mut *guard;
            *clock += 1;
            let now = *clock;
            k.entry(id).or_insert_with(|| (now, project.clone())).0 = now;
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
        if let Ok(mut c) = self.cache.lock() {
            c.budget = bytes;
            c.evict_to_budget();
        }
    }

    pub fn budget(&self) -> usize {
        self.cache.lock().map(|c| c.budget).unwrap_or(0)
    }

    pub fn clear(&self) {
        if let Some(r) = &self.remote {
            r.purge();
        }
        // Call the remote hook before locking: it may discard callbacks and release jobs.
        // Keep live leases occupied until completion, but reject their pre-purge pixels.
        let Ok(mut q) = self.queue.lock() else { return };
        for running in q.running.values_mut() {
            running.invalidated = true;
        }
        let dropped = q.prune(|_| true);
        self.forget_queued(&q, dropped);
        q.desired_secondary.clear();
        q.secondary_seen.clear();
        q.secondary_served.clear();
        if let Ok(mut c) = self.cache.lock() {
            c.map.clear();
            c.order.clear();
            c.bytes = 0;
            c.gpu_bytes = 0;
            if let Ok(mut timing) = self.latency.lock() {
                *timing = latency::Latency::default();
            }
        }
    }

    /// Latest request-to-viewer-install sample. This does not measure physical presentation.
    pub fn viewer_timing(&self) -> ViewerTiming {
        self.latency.lock().map(|t| t.snapshot()).unwrap_or_default()
    }

    /// A successful texture installation, or reuse of the already installed desired texture.
    pub(crate) fn viewer_installed(&self, key: FrameKey) {
        if let Ok(mut timing) = self.latency.lock() {
            timing.installed(key, web_time::Instant::now());
        }
    }

    /// Queue a prefetch frame (no-op if cached or already queued/rendering).
    pub fn request(&self, src: &RenderSource, key: FrameKey, comp: ItemId, t: Tick, opts: RenderOpts) {
        self.request_with(src, key, comp, t, opts, RequestKind::Prefetch);
    }

    /// Queue the frame the viewer is showing: it jumps ahead of every prefetch job.
    pub fn request_urgent(&self, src: &RenderSource, key: FrameKey, comp: ItemId, t: Tick, opts: RenderOpts) {
        self.request_viewer(src, key, comp, t, opts, false);
    }

    /// A paused viewer edit or scrub: retain only its newest queued request and bound
    /// simultaneous interactive renders. Playback uses [`Self::request_urgent`] instead.
    pub fn request_interactive(&self, src: &RenderSource, key: FrameKey, comp: ItemId, t: Tick, opts: RenderOpts) {
        self.request_viewer(src, key, comp, t, opts, true);
    }

    /// Begin collecting the secondary panes visible during this UI paint.
    pub fn begin_secondary_paint(&self) {
        if let Ok(mut q) = self.queue.lock() {
            q.secondary_seen.clear();
        }
    }

    /// Retire hidden panes without canceling another pane's shared frame or a running lease.
    pub fn end_secondary_paint(&self) {
        let Ok(mut q) = self.queue.lock() else { return };
        let seen = q.secondary_seen.clone();
        let previous_slots = q.desired_secondary.len();
        q.desired_secondary.retain(|slot, _| seen.contains(slot));
        q.secondary_served.retain(|slot, _| seen.contains(slot));
        let wanted: HashSet<_> = q.desired_secondary.values().copied().collect();
        let dropped = q.prune(|j| j.secondary && !wanted.contains(&j.key));
        self.forget_queued(&q, dropped);
        let wake = q.best().is_some();
        let freed_slots = q.desired_secondary.len() < previous_slots;
        drop(q);
        if freed_slots && let Some(ctx) = &self.ctx {
            ctx.request_repaint();
        }
        if wake {
            self.wake_worker();
        }
    }

    /// Keep the newest queued frame for this pane, sharing identical work with other panes.
    #[allow(clippy::too_many_arguments)]
    pub fn request_secondary(&self, src: &RenderSource, slot: SecondarySlot, key: FrameKey, comp: ItemId, t: Tick, opts: RenderOpts) {
        self.request_with(src, key, comp, t, opts, RequestKind::Secondary(slot));
    }

    /// An installed pane texture still satisfies this demand after raw RAM eviction.
    /// Register its visibility and retire obsolete queued work without recreating pixels.
    pub fn retain_secondary(&self, slot: SecondarySlot, key: FrameKey) {
        let Ok(mut q) = self.queue.lock() else { return };
        if !q.set_secondary(slot, key) {
            return;
        }
        let wanted: HashSet<_> = q.desired_secondary.values().copied().collect();
        let dropped = q.prune(|j| j.secondary && !wanted.contains(&j.key));
        self.forget_queued(&q, dropped);
        let wake = q.best().is_some();
        drop(q);
        if wake {
            self.wake_worker();
        }
    }

    fn request_viewer(&self, src: &RenderSource, key: FrameKey, comp: ItemId, t: Tick, opts: RenderOpts, interactive: bool) {
        let requested = web_time::Instant::now();
        let cached = self.is_cached(&key);
        if let Ok(mut timing) = self.latency.lock() {
            timing.request(key, cached, requested);
        }
        self.request_with(src, key, comp, t, opts, if interactive { RequestKind::Interactive } else { RequestKind::Viewer });
    }

    /// An urgent (viewer) frame is queued or rendering.
    pub fn urgent_pending(&self) -> bool {
        self.queue.lock().map(|q| q.urgent_running > 0 || q.jobs.iter().any(|j| j.urgent)).unwrap_or(false)
    }

    fn request_with(&self, src: &RenderSource, key: FrameKey, comp: ItemId, t: Tick, opts: RenderOpts, kind: RequestKind) {
        let urgent = matches!(kind, RequestKind::Viewer | RequestKind::Interactive);
        let interactive = matches!(kind, RequestKind::Interactive);
        let secondary = matches!(kind, RequestKind::Secondary(_));
        let Ok(mut q) = self.queue.lock() else { return };
        if let RequestKind::Secondary(slot) = kind
            && !q.set_secondary(slot, key)
        {
            return;
        }
        // Coalesce before cache/dedup returns: revisiting an already available frame
        // must also cancel that pane's previous pending edit. Shared demand survives.
        let wanted: HashSet<_> = q.desired_secondary.values().copied().collect();
        let dropped = q.prune(|j| {
            !wanted.contains(&j.key) && (j.secondary || !secondary && j.key != key && (j.key.revision < key.revision || interactive && j.interactive))
        });
        self.forget_queued(&q, dropped);
        if urgent {
            // Only the newest viewer frame is urgent; earlier ones become prefetch.
            for j in q.jobs.iter_mut() {
                j.urgent = j.key == key;
                if j.urgent {
                    j.interactive = interactive;
                }
            }
        }
        if !secondary && let Some(job) = q.jobs.iter_mut().find(|j| j.key == key) {
            // A primary/prefetch request now owns the job independently of any pane.
            job.secondary = false;
        }
        if self.is_cached(&key) {
            let wake = q.best().is_some();
            drop(q);
            if wake {
                self.wake_worker();
            }
            return;
        }
        let inserted = {
            let Ok(mut inf) = self.inflight.lock() else { return };
            inf.insert(key)
        };
        if !inserted {
            // Promotion can make a queued job eligible after all earlier spawn attempts
            // returned at the interactive limit. Running jobs already own their drain.
            let wake = q.jobs.iter().any(|j| j.key == key) && q.best().is_some();
            drop(q);
            if wake {
                self.wake_worker();
            }
            return;
        }
        q.seq = q.seq.wrapping_add(1);
        let seq = q.seq;
        q.jobs.push(Job { key, src: src.clone(), comp, t, opts, urgent, interactive, secondary, seq });
        drop(q);
        self.wake_worker();
    }

    /// Called with the queue locked, preserving queue-to-inflight lock order.
    fn forget_queued(&self, q: &Queue, dropped: Vec<FrameKey>) {
        if !dropped.is_empty()
            && let Ok(mut inf) = self.inflight.lock()
        {
            for key in dropped {
                if !q.running.contains_key(&key) {
                    inf.remove(&key);
                }
            }
        }
    }

    fn wake_worker(&self) {
        #[cfg(not(target_arch = "wasm32"))]
        if self.remote.is_none() {
            let w = self.worker();
            // One spawn per job; each spawn renders whichever queued job matters most right now.
            let job = move || w.drain();
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
            latency: self.latency.clone(),
            content_keys: self.content_keys.clone(),
        }
    }

    /// Render frames with `remote` from now on (`None`: on this thread again).
    pub fn set_remote(&mut self, remote: Option<Arc<dyn RemoteFrames>>) {
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
                let Some(job) = q.pop() else { break };
                job
            };
            let mut guard = w.guard(&job);
            if let Ok(mut timing) = self.latency.lock() {
                timing.started(job.key, web_time::Instant::now());
            }
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
                guard.remote_busy = Some(self.remote_busy.clone());
            }
            started += 1;
            let (key, urgent, seq) = (job.key, job.urgent, job.seq);
            let fw = self.worker();
            let t0 = web_time::Instant::now();
            let disk_key = remote.disk().then(|| frame_disk_key(&w.content_keys, &job.src.project, &key, opts_hash(&job.opts)));
            let rj = RemoteJob { project: job.src.project.clone(), revision: key.revision, comp: job.comp, t: job.t, opts: job.opts, disk_key };
            remote.start(
                rj,
                Box::new(move |r: Result<RemoteFrame, String>| {
                    // Dropping a lost callback or unwinding its completion also releases
                    // the slot. The sequence protects a later retry of the same content.
                    let _guard = guard;
                    match r {
                        Ok(f) => {
                            let size = [f.width as usize, f.height as usize];
                            let expected = size[0].checked_mul(size[1]).and_then(|n| n.checked_mul(4));
                            if f.width == 0 || f.height == 0 || expected != Some(f.rgba.len()) {
                                log::warn!("remote frame rejected: invalid dimensions or RGBA byte count");
                                return;
                            }
                            let img = egui::ColorImage::from_rgba_premultiplied(size, &f.rgba);
                            fw.finish_key(key, seq, urgent, FrameImage::Cpu(Arc::new(img)), t0);
                        }
                        Err(e) => {
                            // Not rendered (a stale revision, a lost worker): the viewer asks again.
                            log::debug!("remote frame: {e}");
                            fw.release(key, seq);
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
#[derive(Clone)]
struct Worker {
    queue: Arc<Mutex<Queue>>,
    cache: Arc<Mutex<Cache>>,
    inflight: Arc<Mutex<HashSet<FrameKey>>>,
    ctx: Option<egui::Context>,
    last: Arc<Mutex<f64>>,
    latency: Arc<Mutex<latency::Latency>>,
    content_keys: ContentKeys,
}

impl Worker {
    #[cfg(not(target_arch = "wasm32"))]
    fn drain(&self) {
        loop {
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.run_next())) {
                Ok(true) => {}
                Ok(false) => break,
                Err(_) => log::error!("a frame render panicked; that frame was skipped"),
            }
        }
    }

    fn guard(&self, job: &Job) -> JobGuard {
        JobGuard { worker: self.clone(), key: job.key, seq: job.seq, remote_busy: None }
    }

    /// Render the queued job that matters most right now. `false` when the queue was empty.
    fn run_next(&self) -> bool {
        let Worker { queue, content_keys, .. } = self;
        let job = {
            let Ok(mut q) = queue.lock() else { return false };
            let Some(job) = q.pop() else { return false };
            job
        };
        let _guard = self.guard(&job);
        let t0 = web_time::Instant::now();
        if let Ok(mut timing) = self.latency.lock() {
            timing.started(job.key, t0);
        }
        let disk = job.src.disk.as_ref().map(|dc| (dc.clone(), frame_disk_key(content_keys, &job.src.project, &job.key, opts_hash(&job.opts))));
        let from_disk = disk.as_ref().and_then(|(dc, k)| dc.get_frame(*k));
        let disk_hit = from_disk.is_some();
        let ci = match from_disk {
            Some(f) => FrameImage::Cpu(Arc::new(egui::ColorImage::from_rgba_premultiplied([f.width as usize, f.height as usize], &f.rgba))),
            None => self.render_job(&job),
        };
        // Publish the shared frame and wake the viewer before optional RGBA conversion
        // conversion. The disk writer compresses the owned bytes using the same snapshot.
        let published = self.finish(&job, ci.clone(), t0);
        if published
            && !disk_hit
            && let (Some((dc, k)), FrameImage::Cpu(img)) = (&disk, &ci)
            && let Some((width, height, rgba)) = cache_rgba(img)
        {
            dc.put_frame_owned(*k, width, height, rgba);
            if let Some(ctx) = &self.ctx {
                ctx.request_repaint();
            }
        }
        true
    }

    /// Store a finished frame and release its job.
    fn finish(&self, job: &Job, img: FrameImage, t0: web_time::Instant) -> bool {
        self.finish_key(job.key, job.seq, job.urgent, img, t0)
    }

    fn finish_key(&self, key: FrameKey, seq: u64, urgent: bool, img: FrameImage, t0: web_time::Instant) -> bool {
        // Match publication against the live lease under the same queue lock that clear
        // uses to invalidate it. This prevents a late callback from repopulating RAM.
        let Ok(q) = self.queue.lock() else { return false };
        let valid = q.running.get(&key).is_some_and(|r| r.seq == seq && !r.invalidated);
        let mut published = false;
        if valid && let Ok(mut c) = self.cache.lock() {
            c.insert(key, img);
            published = true;
            if urgent && let Ok(mut l) = self.last.lock() {
                *l = t0.elapsed().as_secs_f64() * 1000.0;
            }
            // Publish pixels and readiness under the same cache lock: the UI must not
            // install a result before its preparation timestamp is visible.
            if let Ok(mut timing) = self.latency.lock() {
                timing.ready(key, web_time::Instant::now());
            }
        }
        drop(q);
        self.release(key, seq);
        published
    }

    /// The job of `key` is no longer queued or running (rendered or dropped).
    fn release(&self, key: FrameKey, seq: u64) {
        if let Ok(mut q) = self.queue.lock() {
            if !q.running.get(&key).is_some_and(|r| r.seq == seq) {
                return;
            }
            if let Some(running) = q.running.remove(&key) {
                if running.urgent {
                    q.urgent_running = q.urgent_running.saturating_sub(1);
                }
                if let Ok(mut inf) = self.inflight.lock() {
                    inf.remove(&key);
                }
            }
        }
        if let Some(ctx) = &self.ctx {
            ctx.request_repaint();
        }
    }

    /// The frame on the GPU, kept there for the viewer, if the compositor can do it alone.
    fn render_gpu(&self, job: &Job) -> Option<FrameImage> {
        let g = job.src.gpu.as_ref()?;
        let mut r = Renderer::new(&job.src.project, job.src.footage.as_ref(), job.opts);
        r.expr = job.src.expr.as_deref();
        r.cache = Some(&job.src.layer_cache);
        r.accel = Some(g as &dyn effectcraft_engine::render::Accelerator);
        r.active_accel()?;
        g.render_display(&r, job.comp, job.t).map(|f| FrameImage::Gpu(Arc::new(f)))
    }

    fn render_job(&self, job: &Job) -> FrameImage {
        let mut r = Renderer::new(&job.src.project, job.src.footage.as_ref(), job.opts);
        r.expr = job.src.expr.as_deref();
        r.cache = Some(&job.src.layer_cache);
        r.accel = job.src.gpu.as_ref().map(|g| g as &dyn effectcraft_engine::render::Accelerator);
        // The GPU leaves the frame in a texture for the viewer; otherwise (Software Only, no
        // adapter, or a frame the GPU cannot finish here) the CPU renders it.
        // Auto (Mercury GPU Acceleration) shows each comp on whichever compositor measured
        // faster for it (light comps composite faster on the CPU).
        if let (Some(g), Some(a)) = (&job.src.gpu, r.active_accel())
            && job.src.gpu_display
        {
            // The browser cannot wait for the GPU to time it (and its CPU path is slow): always
            // the GPU there.
            let auto = if cfg!(target_arch = "wasm32") { None } else { r.frame_auto(a, job.comp, true) };
            if auto.is_none_or(|(p, key)| p.choose(key)) {
                let t0 = web_time::Instant::now();
                if let Some(f) = g.render_display(&r, job.comp, job.t) {
                    if let Some((p, key)) = auto {
                        // Time the GPU's work, not just its recording.
                        g.wait();
                        p.record(key, true, t0.elapsed().as_secs_f64() * 1000.0);
                    }
                    return FrameImage::Gpu(Arc::new(f));
                }
                if let Some((p, key)) = auto {
                    p.declined(key);
                }
            }
            let t0 = web_time::Instant::now();
            let img = to_color_image(&r.comp_frame_cpu(job.comp, job.t));
            if let Some((p, key)) = auto {
                p.record(key, false, t0.elapsed().as_secs_f64() * 1000.0);
            }
            return FrameImage::Cpu(Arc::new(img));
        }
        FrameImage::Cpu(Arc::new(to_color_image(&r.comp_frame(job.comp, job.t))))
    }
}

/// Release exactly this dispatch on success, unwind, or a discarded remote callback.
struct JobGuard {
    worker: Worker,
    key: FrameKey,
    seq: u64,
    remote_busy: Option<Arc<Mutex<usize>>>,
}

impl Drop for JobGuard {
    fn drop(&mut self) {
        if let Some(busy) = &self.remote_busy
            && let Ok(mut busy) = busy.lock()
        {
            *busy = busy.saturating_sub(1);
        }
        self.worker.release(self.key, self.seq);
        // Completion may already have released the job and repainted while its remote
        // slot was still busy. Wake again after freeing capacity, even for a stale lease.
        if self.remote_busy.is_some()
            && let Some(ctx) = &self.worker.ctx
        {
            ctx.request_repaint();
        }
    }
}

enum RequestKind {
    Prefetch,
    Viewer,
    Interactive,
    Secondary(SecondarySlot),
}

struct Job {
    key: FrameKey,
    src: RenderSource,
    comp: ItemId,
    t: Tick,
    opts: RenderOpts,
    urgent: bool,
    interactive: bool,
    /// This job may be discarded once no secondary pane desires it. Primary and
    /// prefetch ownership is independent of its current scheduling priority.
    secondary: bool,
    seq: u64,
}

#[derive(Default)]
struct Queue {
    jobs: Vec<Job>,
    running: HashMap<FrameKey, Running>,
    urgent_running: usize,
    seq: u64,
    desired_secondary: HashMap<SecondarySlot, FrameKey>,
    secondary_seen: HashSet<SecondarySlot>,
    /// Scheduling age belongs to the pane, so replacing a pending frame does not
    /// restart its wait behind panes visited earlier in each UI paint.
    secondary_served: HashMap<SecondarySlot, u64>,
    secondary_clock: u64,
}

struct Running {
    seq: u64,
    urgent: bool,
    interactive: bool,
    secondary: bool,
    invalidated: bool,
}

impl Queue {
    fn set_secondary(&mut self, slot: SecondarySlot, key: FrameKey) -> bool {
        if matches!(slot, SecondarySlot::Viewer(_))
            && !self.desired_secondary.contains_key(&slot)
            && self.desired_secondary.keys().filter(|s| matches!(s, SecondarySlot::Viewer(_))).count() >= MAX_SECONDARY_VIEWERS
        {
            return false;
        }
        self.desired_secondary.insert(slot, key);
        self.secondary_seen.insert(slot);
        self.secondary_served.entry(slot).or_insert(0);
        true
    }

    fn desired_secondary(&self, key: &FrameKey) -> bool {
        self.desired_secondary.values().any(|desired| desired == key)
    }

    fn secondary_age(&self, key: &FrameKey) -> u64 {
        self.desired_secondary
            .iter()
            .filter(|(_, desired)| *desired == key)
            .map(|(slot, _)| self.secondary_served.get(slot).copied().unwrap_or(0))
            .min()
            .unwrap_or(u64::MAX)
    }

    fn served_secondary(&mut self, key: &FrameKey) {
        if !self.desired_secondary(key) {
            return;
        }
        // Preserve relative ages even at counter exhaustion, without changing dispatch
        // sequence numbers (which independently protect running-job leases).
        if self.secondary_clock == u64::MAX {
            let mut ages: Vec<u64> = self.secondary_served.values().copied().filter(|age| *age > 0).collect();
            ages.sort_unstable();
            ages.dedup();
            for age in self.secondary_served.values_mut().filter(|age| **age > 0) {
                *age = ages.partition_point(|old| *old < *age) as u64 + 1;
            }
            self.secondary_clock = ages.len() as u64;
        }
        self.secondary_clock += 1;
        for (slot, desired) in &self.desired_secondary {
            if desired == key {
                self.secondary_served.insert(*slot, self.secondary_clock);
            }
        }
    }

    fn priority(&self, job: &Job) -> u8 {
        if job.urgent { 2 } else { u8::from(self.desired_secondary(&job.key)) }
    }

    fn prune(&mut self, obsolete: impl Fn(&Job) -> bool) -> Vec<FrameKey> {
        let mut dropped = Vec::new();
        self.jobs.retain(|job| {
            if obsolete(job) {
                dropped.push(job.key);
                false
            } else {
                true
            }
        });
        dropped
    }

    /// Primary frames first, then visible secondary panes, then speculative prefetch.
    /// At most one secondary render leaves room for a new primary interactive frame.
    fn best(&self) -> Option<usize> {
        let interactive_slots = self.running.values().filter(|r| r.interactive).count() < INTERACTIVE_LIMIT;
        let secondary_slot = !self.running.values().any(|r| r.secondary);
        self.jobs
            .iter()
            .enumerate()
            .filter(|(_, j)| {
                let secondary = !j.urgent && self.desired_secondary(&j.key);
                (!(j.interactive || secondary) || interactive_slots) && (!secondary || secondary_slot)
            })
            .max_by(|(_, a), (_, b)| {
                self.priority(a)
                    .cmp(&self.priority(b))
                    .then_with(|| if self.priority(a) == 1 { self.secondary_age(&b.key).cmp(&self.secondary_age(&a.key)) } else { std::cmp::Ordering::Equal })
                    .then_with(|| if a.urgent { a.seq.cmp(&b.seq) } else { b.seq.cmp(&a.seq) })
            })
            .map(|(i, _)| i)
    }

    fn pop(&mut self) -> Option<Job> {
        let i = self.best()?;
        let job = self.jobs.swap_remove(i);
        let secondary = !job.urgent && self.desired_secondary(&job.key);
        self.served_secondary(&job.key);
        self.running.insert(job.key, Running { seq: job.seq, urgent: job.urgent, interactive: job.interactive || secondary, secondary, invalidated: false });
        if job.urgent {
            self.urgent_running += 1;
        }
        Some(job)
    }
}

/// Bound the optional disk copy before allocating it. Queue admission accounts for this
/// owned vector after submission; the viewer's RAM image has its own independent budget.
fn cache_rgba(img: &egui::ColorImage) -> Option<(u32, u32, Vec<u8>)> {
    let width = u32::try_from(img.size[0]).ok()?;
    let height = u32::try_from(img.size[1]).ok()?;
    let len = disk_cache::frame_cache_len(width, height)?;
    if img.pixels.len() != len / 4 {
        return None;
    }
    let mut rgba = Vec::new();
    rgba.try_reserve_exact(len).ok()?;
    for pixel in &img.pixels {
        rgba.extend_from_slice(&pixel.to_array());
    }
    Some((width, height, rgba))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(content: u64, frame: i64, opts: u64) -> FrameKey {
        FrameKey { revision: content, content, comp: 1, frame, scale: 1000, view: 0, opts }
    }

    fn img() -> FrameImage {
        FrameImage::Cpu(Arc::new(egui::ColorImage::new([2, 2], vec![egui::Color32::BLACK; 4])))
    }

    fn insert(f: &Frames, k: FrameKey) {
        f.cache.lock().unwrap().insert(k, img());
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

    fn tiny_source(footage: Arc<dyn FootageSource>) -> (RenderSource, ItemId) {
        use effectcraft_engine::project::{Comp, Footage, build};
        let mut p = Project::default();
        p.settings.gpu_acceleration = false;
        let mut c = Comp::new(2, 2, effectcraft_engine::time::FrameRate::FPS_30, Tick::from_seconds_f64(10.0));
        let item = p.add_item(
            "Original pixels",
            Default::default(),
            None,
            ItemKind::Footage(Footage {
                width: 2,
                height: 2,
                kind: effectcraft_engine::project::FootageKind::Video,
                has_video: true,
                duration: c.duration,
                ..Default::default()
            }),
        );
        let layer = build::layer(&mut p, &c, "Pixels", LayerSource::Footage { item }, (2, 2), None);
        c.layers.push(layer);
        let comp = p.add_item("Tiny", Default::default(), None, ItemKind::Comp(c.into()));
        (RenderSource { project: Arc::new(p), footage, expr: None, layer_cache: Arc::default(), gpu: None, gpu_display: false, disk: None }, comp)
    }

    fn queued(f: &Frames, src: &RenderSource, comp: ItemId, frame: i64, seq: u64) -> FrameKey {
        let key = FrameKey { comp: comp.0, ..key(1, frame, 0) };
        f.queue.lock().unwrap().jobs.push(Job {
            key,
            src: src.clone(),
            comp,
            t: Tick::from_seconds_f64(frame as f64 / 30.0),
            opts: Default::default(),
            urgent: true,
            interactive: false,
            secondary: false,
            seq,
        });
        f.inflight.lock().unwrap().insert(key);
        key
    }

    #[test]
    fn released_guard_cannot_remove_a_later_same_key_dispatch() {
        let f = Frames::default();
        let w = f.worker();
        let key = key(1, 0, 0);
        f.queue.lock().unwrap().running.insert(key, Running { seq: 1, urgent: true, interactive: false, secondary: false, invalidated: false });
        f.queue.lock().unwrap().urgent_running = 1;
        f.inflight.lock().unwrap().insert(key);
        let guard = JobGuard { worker: w.clone(), key, seq: 1, remote_busy: None };
        w.release(key, 1);
        assert_eq!(f.inflight(), 0);
        f.queue.lock().unwrap().running.insert(key, Running { seq: 2, urgent: true, interactive: false, secondary: false, invalidated: false });
        f.queue.lock().unwrap().urgent_running = 1;
        f.inflight.lock().unwrap().insert(key);
        drop(guard);
        assert_eq!(f.inflight(), 1);
        let q = f.queue.lock().unwrap();
        assert_eq!(q.running.get(&key).unwrap().seq, 2);
        assert_eq!(q.urgent_running, 1);
    }

    struct PanicOnce(std::sync::atomic::AtomicBool);

    impl FootageSource for PanicOnce {
        fn frame(&self, _: ItemId, _: &effectcraft_engine::project::Footage, _: Tick) -> Option<Arc<effectcraft_engine::render::Image>> {
            if !self.0.swap(true, std::sync::atomic::Ordering::SeqCst) {
                std::panic::resume_unwind(Box::new("synthetic frame failure"));
            }
            Some(Arc::new(effectcraft_engine::render::Image::new(2, 2)))
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn native_drain_recovers_from_a_panicking_frame_without_another_request() {
        let f = Frames::default();
        let (src, comp) = tiny_source(Arc::new(PanicOnce(Default::default())));
        // The newest urgent frame starts first and fails; remaining work still completes.
        let successful = queued(&f, &src, comp, 0, 1);
        let failed = queued(&f, &src, comp, 1, 2);
        f.worker().drain();
        assert!(!f.is_cached(&failed));
        assert!(f.is_cached(&successful));
        assert_eq!(f.inflight(), 0);
        let q = f.queue.lock().unwrap();
        assert!(q.jobs.is_empty() && q.running.is_empty());
        assert_eq!(q.urgent_running, 0);
    }

    struct LostCallback {
        invoke: bool,
    }

    impl RemoteFrames for LostCallback {
        fn slots(&self) -> usize {
            1
        }
        fn start(&self, _: RemoteJob, done: RemoteDone) {
            if self.invoke {
                done(Err("synthetic worker failure".into()));
            }
        }
    }

    #[test]
    fn lost_and_inline_remote_callbacks_release_their_slots_once() {
        for invoke in [false, true] {
            let mut f = Frames::default();
            f.set_remote(Some(Arc::new(LostCallback { invoke })));
            let (src, comp) = tiny_source(Arc::new(effectcraft_engine::render::NoFootage));
            queued(&f, &src, comp, 0, 1);
            assert_eq!(f.dispatch_remote(), 1);
            assert_eq!(f.remote_busy(), 0);
            assert_eq!(f.inflight(), 0);
            let q = f.queue.lock().unwrap();
            assert!(q.running.is_empty());
            assert_eq!(q.urgent_running, 0);
        }
    }

    #[test]
    fn remote_slot_release_repaints_after_the_completion_repaint_was_consumed() {
        let mut f = Frames::default();
        let ctx = egui::Context::default();
        f.set_context(&ctx);
        let w = f.worker();
        let key = key(1, 0, 0);
        f.queue.lock().unwrap().running.insert(key, Running { seq: 1, urgent: true, interactive: false, secondary: false, invalidated: false });
        f.queue.lock().unwrap().urgent_running = 1;
        f.inflight.lock().unwrap().insert(key);
        *f.remote_busy.lock().unwrap() = 1;
        let guard = JobGuard { worker: w.clone(), key, seq: 1, remote_busy: Some(f.remote_busy.clone()) };
        // The UI consumes completion's first repaint while the remote slot is still busy.
        w.release(key, 1);
        for _ in 0..3 {
            ctx.run_ui(Default::default(), |_| {}).drop_without_applying_deltas();
        }
        assert!(!ctx.has_requested_repaint());
        assert_eq!(f.remote_busy(), 1);
        drop(guard);
        assert_eq!(f.remote_busy(), 0);
        assert!(ctx.has_requested_repaint(), "newly freed remote capacity must wake queued work");
    }

    struct HeldRemote {
        slots: usize,
        pending: Mutex<Vec<(Tick, RemoteDone)>>,
    }

    impl RemoteFrames for HeldRemote {
        fn slots(&self) -> usize {
            self.slots
        }
        fn start(&self, job: RemoteJob, done: RemoteDone) {
            if let Ok(mut pending) = self.pending.lock() {
                pending.push((job.t, done));
            }
        }
    }

    fn held_frames() -> (Frames, Arc<HeldRemote>, RenderSource, ItemId) {
        let remote = Arc::new(HeldRemote { slots: 4, pending: Mutex::default() });
        let mut frames = Frames::default();
        frames.set_remote(Some(remote.clone()));
        let (source, comp) = tiny_source(Arc::new(effectcraft_engine::render::NoFootage));
        (frames, remote, source, comp)
    }

    fn interactive(f: &Frames, src: &RenderSource, comp: ItemId, frame: i64) -> FrameKey {
        let k = FrameKey { comp: comp.0, ..key(1, frame, 0) };
        f.request_interactive(src, k, comp, effectcraft_engine::time::FrameRate::FPS_30.tick_of(frame), Default::default());
        k
    }

    fn secondary(f: &Frames, src: &RenderSource, comp: ItemId, slot: SecondarySlot, frame: i64) -> FrameKey {
        let k = FrameKey { comp: comp.0, ..key(1, frame, 0) };
        f.request_secondary(src, slot, k, comp, effectcraft_engine::time::FrameRate::FPS_30.tick_of(frame), Default::default());
        k
    }

    #[test]
    fn purge_rejects_a_late_same_key_remote_completion() {
        let (f, remote, src, comp) = held_frames();
        let old = interactive(&f, &src, comp, 0);
        assert_eq!(f.dispatch_remote(), 1);
        f.clear();
        let (_, done) = remote.pending.lock().unwrap().remove(0);
        done(Ok(RemoteFrame { width: 2, height: 2, rgba: [80, 120, 160, 255].repeat(4), ms: 0.0 }));
        assert!(!f.is_cached(&old), "late pre-purge pixels repopulated the cache");
        assert_eq!(f.inflight(), 0);
        assert_eq!(f.remote_busy(), 0);
    }

    #[test]
    fn purge_drops_queued_frames_and_rejects_late_remote_pixels_without_releasing_live_slots() {
        let (f, remote, src, comp) = held_frames();
        let old = interactive(&f, &src, comp, 0);
        assert_eq!(f.dispatch_remote(), 1);
        secondary(&f, &src, comp, SecondarySlot::Top, 1);
        assert_eq!(f.inflight(), 2);
        f.clear();
        assert_eq!(f.inflight(), 1, "queued old-source frames should be retired");
        assert_eq!(f.remote_busy(), 1, "a purge cannot reuse a worker's live slot early");
        let (_, done) = remote.pending.lock().unwrap().remove(0);
        done(Ok(RemoteFrame { width: 2, height: 2, rgba: [80, 120, 160, 255].repeat(4), ms: 0.0 }));
        assert!(!f.is_cached(&old), "pre-purge pixels cannot repopulate the cleared cache");
        assert_eq!(f.remote_busy(), 0);
        assert_eq!(f.inflight(), 0);
        assert_eq!(f.dispatch_remote(), 0);
        assert_eq!(interactive(&f, &src, comp, 0), old);
        assert_eq!(f.dispatch_remote(), 1);
        let (_, done) = remote.pending.lock().unwrap().remove(0);
        done(Ok(RemoteFrame { width: 2, height: 2, rgba: [20, 40, 60, 255].repeat(4), ms: 0.0 }));
        let FrameImage::Cpu(image) = f.get(&old).unwrap() else { panic!("expected CPU pixels") };
        assert_eq!(image.pixels[0].to_array(), [20, 40, 60, 255]);
    }

    #[test]
    fn secondary_slots_share_jobs_and_only_cancel_their_own_obsolete_demand() {
        let (f, _remote, src, comp) = held_frames();
        let shared = secondary(&f, &src, comp, SecondarySlot::Top, 0);
        secondary(&f, &src, comp, SecondarySlot::Front, 0);
        assert_eq!(f.inflight(), 1);
        let top = secondary(&f, &src, comp, SecondarySlot::Top, 1);
        assert!(f.inflight.lock().unwrap().contains(&shared));
        let front = secondary(&f, &src, comp, SecondarySlot::Front, 2);
        assert!(!f.inflight.lock().unwrap().contains(&shared));
        let main = FrameKey { revision: 2, comp: comp.0, ..key(2, 3, 0) };
        f.request_interactive(&src, main, comp, Tick::ZERO, Default::default());
        assert_eq!(f.inflight(), 3, "a primary edit must preserve both secondary demands");
        for k in [top, front, main] {
            assert!(f.queue.lock().unwrap().jobs.iter().any(|j| j.key == k));
        }
        secondary(&f, &src, comp, SecondarySlot::Top, 4);
        assert!(f.inflight.lock().unwrap().contains(&main), "secondary updates cannot cancel the primary frame");
    }

    #[test]
    fn dynamic_secondary_slots_are_bounded_without_crowding_out_fixed_panes() {
        let (f, _remote, src, comp) = held_frames();
        for id in 0..MAX_SECONDARY_VIEWERS as u32 {
            secondary(&f, &src, comp, SecondarySlot::Viewer(id), i64::from(id));
        }
        let fixed = [SecondarySlot::Top, SecondarySlot::Front, SecondarySlot::Right, SecondarySlot::LockedSplit];
        for (i, slot) in fixed.into_iter().enumerate() {
            secondary(&f, &src, comp, slot, 64 + i as i64);
        }
        let denied = secondary(&f, &src, comp, SecondarySlot::Viewer(u32::MAX), 1000);
        f.retain_secondary(SecondarySlot::Viewer(u32::MAX), denied);
        {
            let q = f.queue.lock().unwrap();
            assert_eq!((q.desired_secondary.len(), q.secondary_seen.len(), q.secondary_served.len(), q.jobs.len()), (68, 68, 68, 68));
            assert!(!q.jobs.iter().any(|j| j.key == denied));
        }
        // An existing slot can still replace its key while all slots are occupied.
        secondary(&f, &src, comp, SecondarySlot::Viewer(0), 1001);
        assert_eq!(f.inflight(), 68);
        f.begin_secondary_paint();
        for (i, slot) in fixed.into_iter().enumerate() {
            secondary(&f, &src, comp, slot, 64 + i as i64);
        }
        f.end_secondary_paint();
        secondary(&f, &src, comp, SecondarySlot::Viewer(u32::MAX), 1000);
        let q = f.queue.lock().unwrap();
        assert_eq!((q.desired_secondary.len(), q.secondary_seen.len(), q.secondary_served.len(), q.jobs.len()), (5, 5, 5, 5));
        assert!(q.jobs.iter().any(|j| j.key == denied), "a rejected id can retry after retirement frees capacity");
    }

    #[test]
    fn passive_shared_job_promotes_to_primary_and_survives_slot_retirement() {
        let (f, remote, src, comp) = held_frames();
        let shared = secondary(&f, &src, comp, SecondarySlot::Viewer(1), 0);
        secondary(&f, &src, comp, SecondarySlot::Viewer(2), 0);
        assert_eq!(f.inflight(), 1);
        f.request_urgent(&src, shared, comp, Tick::ZERO, Default::default());
        f.begin_secondary_paint();
        f.end_secondary_paint();
        assert_eq!(f.inflight(), 1, "activation transfers ownership before the passive slot retires");
        assert_eq!(f.dispatch_remote(), 1);
        assert!(f.queue.lock().unwrap().running.get(&shared).unwrap().urgent);
        let (_, done) = remote.pending.lock().unwrap().remove(0);
        done(Err("test complete".into()));
        assert_eq!(f.inflight(), 0);
    }

    #[test]
    fn continuously_changing_secondary_panes_take_turns_despite_fixed_paint_order() {
        let (f, remote, src, comp) = held_frames();
        let slots = [SecondarySlot::Top, SecondarySlot::Front, SecondarySlot::Right];
        for round in 0..9 {
            f.begin_secondary_paint();
            for (i, slot) in slots.into_iter().enumerate() {
                secondary(&f, &src, comp, slot, round * 3 + i as i64);
            }
            f.end_secondary_paint();
            assert_eq!(f.dispatch_remote(), 1);
            let (time, done) = remote.pending.lock().unwrap().remove(0);
            let frame = effectcraft_engine::time::FrameRate::FPS_30.frame_at(time);
            done(Err("release this pane".into()));
            assert_eq!(frame, round * 3 + round % 3, "each pane retains its scheduling age while its requested frame changes");
        }
    }

    #[test]
    fn primary_shared_dispatch_serves_all_secondary_owners_and_age_wrap_preserves_order() {
        let (f, remote, src, comp) = held_frames();
        let shared = secondary(&f, &src, comp, SecondarySlot::Top, 0);
        secondary(&f, &src, comp, SecondarySlot::Front, 0);
        f.retain_secondary(SecondarySlot::Right, FrameKey { frame: 1, ..shared });
        f.request_urgent(&src, shared, comp, Tick::ZERO, Default::default());
        assert_eq!(f.dispatch_remote(), 1);
        let (_, done) = remote.pending.lock().unwrap().remove(0);
        done(Err("primary served two panes".into()));
        {
            let mut q = f.queue.lock().unwrap();
            assert_eq!(q.secondary_served[&SecondarySlot::Top], q.secondary_served[&SecondarySlot::Front]);
            assert!(q.secondary_served[&SecondarySlot::Top] > q.secondary_served[&SecondarySlot::Right]);
            q.secondary_clock = u64::MAX;
        }
        secondary(&f, &src, comp, SecondarySlot::Top, 2);
        secondary(&f, &src, comp, SecondarySlot::Front, 3);
        let right = secondary(&f, &src, comp, SecondarySlot::Right, 4);
        assert_eq!(f.dispatch_remote(), 1);
        assert!(f.queue.lock().unwrap().running.contains_key(&right));
        let q = f.queue.lock().unwrap();
        assert!(q.secondary_clock < u64::MAX);
        assert!(q.secondary_served[&SecondarySlot::Right] > q.secondary_served[&SecondarySlot::Top]);
    }

    #[test]
    fn secondary_cap_reserves_primary_capacity_and_does_not_limit_playback() {
        let (f, remote, src, comp) = held_frames();
        let top = secondary(&f, &src, comp, SecondarySlot::Top, 0);
        assert_eq!(f.dispatch_remote(), 1);
        secondary(&f, &src, comp, SecondarySlot::Front, 1);
        for frame in 2..102 {
            secondary(&f, &src, comp, SecondarySlot::Top, frame);
        }
        assert_eq!(f.queue.lock().unwrap().jobs.len(), 2);
        assert_eq!(f.dispatch_remote(), 0, "only one secondary pane renders at once");
        interactive(&f, &src, comp, 102);
        assert_eq!(f.dispatch_remote(), 1, "secondary work reserves room for primary interaction");
        interactive(&f, &src, comp, 103);
        assert_eq!(f.dispatch_remote(), 0, "primary and secondary together share the two-job limit");
        let playback = FrameKey { frame: 104, ..top };
        f.request_urgent(&src, playback, comp, Tick::from_seconds_f64(104.0 / 30.0), Default::default());
        assert_eq!(f.dispatch_remote(), 1, "playback bypasses the interactive limit");
        let (_, done) = remote.pending.lock().unwrap().remove(0);
        done(Err("free secondary capacity".into()));
        assert_eq!(f.dispatch_remote(), 1);
        let q = f.queue.lock().unwrap();
        assert!(q.running.keys().any(|k| k.frame == 1), "the older waiting pane must precede newly scrubbed panes");
        assert!(q.jobs.iter().any(|j| j.key.frame == 101));
    }

    #[test]
    fn secondary_paint_retirement_preserves_primary_prefetch_and_running_leases() {
        let (f, _remote, src, comp) = held_frames();
        f.begin_secondary_paint();
        let top = secondary(&f, &src, comp, SecondarySlot::Top, 0);
        f.dispatch_remote();
        let front = secondary(&f, &src, comp, SecondarySlot::Front, 1);
        let owned = secondary(&f, &src, comp, SecondarySlot::Right, 2);
        f.request(&src, owned, comp, Tick::ZERO, Default::default());
        let main = interactive(&f, &src, comp, 3);
        f.end_secondary_paint();
        f.begin_secondary_paint();
        f.end_secondary_paint();
        let q = f.queue.lock().unwrap();
        assert!(q.desired_secondary.is_empty());
        assert!(q.running.contains_key(&top), "visibility retirement leaves the active lease intact");
        assert!(!q.jobs.iter().any(|j| j.key == front));
        assert!(q.jobs.iter().any(|j| j.key == owned), "prefetch ownership survives pane retirement");
        assert!(q.jobs.iter().any(|j| j.key == main));
        assert_eq!(f.inflight(), 3);
    }

    #[test]
    fn secondary_demand_promotes_prefetch_without_demoting_primary_or_changing_telemetry() {
        let (f, _remote, src, comp) = held_frames();
        let older = FrameKey { comp: comp.0, ..key(1, 0, 0) };
        let wanted = FrameKey { frame: 1, ..older };
        f.request(&src, older, comp, Tick::ZERO, Default::default());
        f.request(&src, wanted, comp, Tick::ZERO, Default::default());
        secondary(&f, &src, comp, SecondarySlot::Top, 1);
        assert!(!f.urgent_pending());
        assert_eq!(*f.last_ms.lock().unwrap(), 0.0);
        {
            let q = f.queue.lock().unwrap();
            assert_eq!(q.jobs[q.best().unwrap()].key, wanted);
            assert!(!q.jobs.iter().find(|j| j.key == wanted).unwrap().secondary);
        }
        let main = interactive(&f, &src, comp, 2);
        secondary(&f, &src, comp, SecondarySlot::Front, 3);
        let q = f.queue.lock().unwrap();
        assert_eq!(q.jobs[q.best().unwrap()].key, main);
        assert_eq!(q.jobs.iter().filter(|j| j.key == wanted).count(), 1);
    }

    #[test]
    fn secondary_cached_and_running_revisits_retire_obsolete_queued_frames() {
        let (f, _remote, src, comp) = held_frames();
        let running = secondary(&f, &src, comp, SecondarySlot::Top, 0);
        f.dispatch_remote();
        secondary(&f, &src, comp, SecondarySlot::Top, 1);
        secondary(&f, &src, comp, SecondarySlot::Top, running.frame);
        assert!(f.queue.lock().unwrap().jobs.is_empty());
        secondary(&f, &src, comp, SecondarySlot::Top, 2);
        let cached = FrameKey { frame: 3, ..running };
        insert(&f, cached);
        secondary(&f, &src, comp, SecondarySlot::Top, cached.frame);
        assert!(f.queue.lock().unwrap().jobs.is_empty());
        assert_eq!(f.inflight(), 1);
    }

    #[test]
    fn disk_pixel_copy_checks_dimensions_and_preserves_premultiplied_bytes() {
        let pixels = [1, 2, 3, 4, 5, 6, 7, 8];
        let valid = egui::ColorImage::from_rgba_premultiplied([2, 1], &pixels);
        assert_eq!(cache_rgba(&valid), Some((2, 1, pixels.to_vec())));
        let mut invalid = valid.clone();
        invalid.size = [2, 2];
        assert!(cache_rgba(&invalid).is_none());
        invalid.size = [1 << 31, 1 << 31];
        assert!(cache_rgba(&invalid).is_none());
        invalid.size = [0, 1];
        assert!(cache_rgba(&invalid).is_none());
    }

    #[test]
    fn persistent_frame_identity_includes_exact_options_without_revision_or_xor_cancellation() {
        let (src, comp) = tiny_source(Arc::new(effectcraft_engine::render::NoFootage));
        let keys = ContentKeys::default();
        let opts = opts_hash(&RenderOpts::default());
        let key = FrameKey { comp: comp.0, opts, ..key(1, 0, 0) };
        let original = frame_disk_key(&keys, &src.project, &key, opts);
        assert_ne!(original, frame_disk_key(&keys, &src.project, &FrameKey { opts: opts.wrapping_add(1), ..key }, opts));
        assert_eq!(original, frame_disk_key(&keys, &src.project, &FrameKey { revision: 2, ..key }, opts));
        let draft = opts_hash(&RenderOpts { draft: true, ..Default::default() });
        assert_ne!(original, frame_disk_key(&keys, &src.project, &FrameKey { opts: draft, ..key }, draft));
    }

    #[test]
    fn interactive_scrubbing_keeps_two_running_and_only_the_latest_pending_frame() {
        let (f, remote, src, comp) = held_frames();
        interactive(&f, &src, comp, 0);
        assert_eq!(f.dispatch_remote(), 1);
        interactive(&f, &src, comp, 1);
        assert_eq!(f.dispatch_remote(), 1);
        for frame in 2..102 {
            interactive(&f, &src, comp, frame);
        }
        assert_eq!(f.inflight(), 3);
        assert_eq!(f.dispatch_remote(), 0, "interactive limit applies even with spare remote slots");
        {
            let q = f.queue.lock().unwrap();
            assert_eq!(q.jobs.len(), 1);
            assert_eq!(q.jobs[0].key.frame, 101);
        }
        let (_, done) = remote.pending.lock().unwrap().remove(0);
        done(Err("release one held frame".into()));
        assert_eq!(f.dispatch_remote(), 1);
        let pending = std::mem::take(&mut *remote.pending.lock().unwrap());
        assert_eq!(pending.iter().map(|(t, _)| effectcraft_engine::time::FrameRate::FPS_30.frame_at(*t)).collect::<Vec<_>>(), vec![1, 101]);
        for (_, done) in pending {
            done(Err("test complete".into()));
        }
        assert_eq!(f.inflight(), 0);
    }

    #[test]
    fn cached_and_running_revisits_cancel_an_obsolete_pending_edit() {
        let (f, _remote, src, comp) = held_frames();
        let running = interactive(&f, &src, comp, 0);
        f.dispatch_remote();
        interactive(&f, &src, comp, 1);
        f.dispatch_remote();
        interactive(&f, &src, comp, 2);
        interactive(&f, &src, comp, running.frame);
        assert!(f.queue.lock().unwrap().jobs.is_empty());
        assert_eq!(f.inflight(), 2);
        interactive(&f, &src, comp, 3);
        let cached = FrameKey { comp: comp.0, ..key(1, 4, 0) };
        insert(&f, cached);
        interactive(&f, &src, comp, cached.frame);
        assert!(f.queue.lock().unwrap().jobs.is_empty());
        assert_eq!(f.inflight(), 2);
    }

    #[test]
    fn prefetch_is_promoted_once_and_playback_keeps_its_capacity() {
        let (f, remote, src, comp) = held_frames();
        let k = FrameKey { comp: comp.0, ..key(1, 0, 0) };
        f.request(&src, k, comp, Tick::ZERO, Default::default());
        interactive(&f, &src, comp, 0);
        assert_eq!(f.inflight(), 1);
        assert!(f.queue.lock().unwrap().jobs[0].interactive);
        assert_eq!(f.dispatch_remote(), 1);
        interactive(&f, &src, comp, 1);
        assert_eq!(f.dispatch_remote(), 1);
        interactive(&f, &src, comp, 2);
        for frame in 3..5 {
            let k = FrameKey { frame, ..k };
            f.request_urgent(&src, k, comp, effectcraft_engine::time::FrameRate::FPS_30.tick_of(frame), Default::default());
        }
        assert_eq!(f.dispatch_remote(), 2, "playback is not throttled by interactive slots");
        assert_eq!(f.remote_busy(), 4);
        assert_eq!(remote.pending.lock().unwrap().len(), 4);
        assert_eq!(f.queue.lock().unwrap().jobs.len(), 1);
    }

    #[test]
    fn playback_can_promote_the_same_frame_that_was_queued_as_an_edit() {
        let (f, _remote, src, comp) = held_frames();
        interactive(&f, &src, comp, 0);
        f.dispatch_remote();
        interactive(&f, &src, comp, 1);
        f.dispatch_remote();
        let k = interactive(&f, &src, comp, 2);
        assert_eq!(f.dispatch_remote(), 0);
        f.request_urgent(&src, k, comp, effectcraft_engine::time::FrameRate::FPS_30.tick_of(2), Default::default());
        assert_eq!(f.dispatch_remote(), 1, "starting playback must reclassify its queued frame");
        assert_eq!(f.remote_busy(), 3);
    }

    struct ReplyRemote(Mutex<Option<RemoteFrame>>);

    impl RemoteFrames for ReplyRemote {
        fn slots(&self) -> usize {
            1
        }
        fn start(&self, _: RemoteJob, done: RemoteDone) {
            match self.0.lock().ok().and_then(|mut reply| reply.take()) {
                Some(reply) => done(Ok(reply)),
                None => done(Err("no synthetic reply".into())),
            }
        }
    }

    #[test]
    fn malformed_remote_pixels_are_rejected_without_panicking_and_can_retry() {
        for (width, height, bytes) in [(2, 2, 15), (2, 2, 17), (0, 2, 0), (2, 0, 0), (u32::MAX, u32::MAX, 0)] {
            let remote = Arc::new(ReplyRemote(Mutex::new(Some(RemoteFrame { width, height, rgba: vec![0; bytes], ms: 0.0 }))));
            let mut f = Frames::default();
            f.set_remote(Some(remote.clone()));
            let (src, comp) = tiny_source(Arc::new(effectcraft_engine::render::NoFootage));
            let k = interactive(&f, &src, comp, 0);
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f.dispatch_remote()));
            assert!(outcome.is_ok(), "malformed {width}x{height} frame with {bytes} bytes panicked");
            assert!(!f.is_cached(&k));
            assert_eq!((f.remote_busy(), f.inflight()), (0, 0));
            assert_eq!(f.queue.lock().unwrap().urgent_running, 0);
            *remote.0.lock().unwrap() = Some(RemoteFrame { width: 2, height: 2, rgba: [255, 0, 0, 255].repeat(4), ms: 0.0 });
            interactive(&f, &src, comp, 0);
            assert_eq!(f.dispatch_remote(), 1);
            assert!(f.is_cached(&k));
            assert_eq!((f.remote_busy(), f.inflight()), (0, 0));
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    struct HeldFootage {
        calls: std::sync::atomic::AtomicUsize,
        entered: std::sync::mpsc::Sender<Tick>,
        release: Mutex<std::sync::mpsc::Receiver<()>>,
    }

    #[cfg(not(target_arch = "wasm32"))]
    impl FootageSource for HeldFootage {
        fn frame(&self, _: ItemId, _: &effectcraft_engine::project::Footage, t: Tick) -> Option<Arc<effectcraft_engine::render::Image>> {
            let call = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.entered.send(t).ok()?;
            if call < 2 {
                self.release.lock().ok()?.recv_timeout(std::time::Duration::from_secs(10)).ok()?;
            }
            Some(Arc::new(effectcraft_engine::render::Image::new(2, 2)))
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn native_playback_promotion_wakes_a_previously_blocked_queue() {
        use std::sync::mpsc;
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let footage = Arc::new(HeldFootage { calls: Default::default(), entered: entered_tx, release: Mutex::new(release_rx) });
        let (mut src, comp) = tiny_source(footage);
        src.layer_cache = Arc::new(LayerCache::new(0));
        let mut f = Frames { pool: Some(rayon::ThreadPoolBuilder::new().num_threads(3).build().unwrap()), ..Default::default() };
        interactive(&f, &src, comp, 0);
        let first = entered_rx.recv_timeout(std::time::Duration::from_secs(5));
        interactive(&f, &src, comp, 1);
        let second = entered_rx.recv_timeout(std::time::Duration::from_secs(5));
        // Reproduce the queue after a capped spawn has already returned, without a race
        // against an OS-scheduled spawn: suppress only that third request's native spawn.
        f.set_remote(Some(Arc::new(HeldRemote { slots: 4, pending: Default::default() })));
        let third = interactive(&f, &src, comp, 2);
        f.set_remote(None);
        f.request_urgent(&src, third, comp, effectcraft_engine::time::FrameRate::FPS_30.tick_of(2), Default::default());
        let promoted = entered_rx.recv_timeout(std::time::Duration::from_secs(5));
        let _ = release_tx.send(());
        let _ = release_tx.send(());
        assert!(first.is_ok() && second.is_ok(), "two interactive jobs must reach the test gates");
        assert_eq!(
            effectcraft_engine::time::FrameRate::FPS_30.frame_at(promoted.unwrap()),
            2,
            "playback promotion must start before held interactive jobs finish"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn native_interactive_workers_deliver_the_final_scrub_without_another_request() {
        use std::sync::mpsc;
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let footage = Arc::new(HeldFootage { calls: Default::default(), entered: entered_tx, release: Mutex::new(release_rx) });
        let (mut src, comp) = tiny_source(footage.clone());
        src.layer_cache = Arc::new(LayerCache::new(0));
        let mut f = Frames::default();
        let ctx = egui::Context::default();
        for _ in 0..3 {
            ctx.run_ui(Default::default(), |_| {}).drop_without_applying_deltas();
        }
        let (repaint_tx, repaint_rx) = mpsc::channel();
        ctx.set_request_repaint_callback(move |_| {
            let _ = repaint_tx.send(());
        });
        f.set_context(&ctx);
        interactive(&f, &src, comp, 0);
        let first = entered_rx.recv_timeout(std::time::Duration::from_secs(10));
        interactive(&f, &src, comp, 1);
        let second = entered_rx.recv_timeout(std::time::Duration::from_secs(10));
        let mut latest = key(1, 0, 0);
        for frame in 2..102 {
            latest = interactive(&f, &src, comp, frame);
        }
        let counts = {
            let q = f.queue.lock().unwrap();
            (q.running.len(), q.jobs.len())
        };
        // Release both gates before asserting, including on a regression failure.
        release_tx.send(()).unwrap();
        release_tx.send(()).unwrap();
        let rate = effectcraft_engine::time::FrameRate::FPS_30;
        assert_eq!(rate.frame_at(first.unwrap()), 0);
        assert_eq!(rate.frame_at(second.unwrap()), 1);
        assert_eq!(counts, (2, 1));
        let final_started = entered_rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
        assert_eq!(rate.frame_at(final_started), 101);
        while !f.is_cached(&latest) || f.inflight() != 0 {
            repaint_rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
            for _ in 0..3 {
                ctx.run_ui(Default::default(), |_| {}).drop_without_applying_deltas();
            }
        }
        assert_eq!(footage.calls.load(std::sync::atomic::Ordering::SeqCst), 3);
        assert!(entered_rx.try_recv().is_err(), "obsolete scrubs must never start");
    }
}
