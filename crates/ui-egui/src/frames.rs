//! Background frame rendering and the RAM preview cache.
//!
//! Frames render on a dedicated thread pool from immutable `Arc<Project>` snapshots, so the UI
//! never waits for the compositor (a playing preview's frames and idle caching on one thread
//! each, so the frames started first are done first). Results land in a memory-budgeted cache keyed by the comp's
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
//! written to disk under a content key (the comp's content hash from the project in memory,
//! frame, scale, view and render options) with the source files it read, and a job first looks
//! there: frames survive restarts and project reopenings. An entry is used only while the files
//! it lists are unchanged, checked by the frame worker when it reads the entry (a frame whose
//! files can't all be named is shown but not stored). The timeline shows disk-only frames as a
//! blue cache bar (present entries; their files are checked only when a frame is used).
//!
//! With Mercury GPU Acceleration (a [`Gpu`] on egui-wgpu's device and the project's renderer set
//! to the GPU) frames stay on the GPU: the compositor writes an RGBA8 texture that the viewer
//! registers with egui-wgpu and draws directly; pixels are read back only when something needs
//! them (Info panel, eyedroppers, histograms).
//!
//! 32 bpc projects keep half-float frames on the desktop ([`F16Frame`], rendered without the HDR
//! step): twice the bytes of an 8-bit frame, so the RAM preview holds half as many. The disk
//! cache keeps them as half-float entries.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};

use effectcraft_engine::project::{Comp, Heard, ItemId, ItemKind, Node, Project, PropGroup};
use effectcraft_engine::render::disk_cache::{self, DiskCache};
use effectcraft_engine::render::reads::{ReadFiles, ReadLog};
use effectcraft_engine::render::{AutoTicket, CancellableFootage, ExprHost, FootageSource, LayerCache, RenderOpts, Renderer};
use effectcraft_engine::time::Tick;
use effectcraft_gpu::{DisplayFrame, FrameError, Gpu};
use rayon::prelude::*;

/// Comp identities (and the project snapshots behind them) the RAM preview keeps frames of.
const MAX_KEEPERS: usize = 64;

/// GPU frames kept for RAM preview (bytes of video memory) at first. wgpu can't tell how much
/// video memory there is: allocation pressure halves the budget (to [`MIN_GPU_BUDGET`])
/// and the frame renders on the CPU (#106). One burst is counted once; successful GPU
/// renders gradually restore the budget after pressure subsides.
const GPU_BUDGET: usize = 1 << 30;
const MIN_GPU_BUDGET: usize = 64 << 20;
const GPU_PRESSURE_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(5);
const GPU_RECOVERY_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);
/// GPU frames that render at once, however many frame threads there are. Each one holds its own
/// working textures, so 16 at once ran an RTX 3090 out of video memory (#3028).
const MAX_GPU_RENDERS: usize = 2;

/// Counts the GPU renders in flight; the rest of the frame threads wait their turn.
struct GpuRenderSlots {
    #[cfg(test)]
    entered: std::sync::atomic::AtomicUsize,
    busy: Mutex<usize>,
    freed: Condvar,
}

/// One GPU render slot, given back on drop.
struct GpuRenderSlot<'a>(&'a GpuRenderSlots);

static GPU_RENDER_SLOTS: GpuRenderSlots = GpuRenderSlots::new();

impl GpuRenderSlots {
    const fn new() -> Self {
        Self {
            #[cfg(test)]
            entered: std::sync::atomic::AtomicUsize::new(0),
            busy: Mutex::new(0),
            freed: Condvar::new(),
        }
    }

    /// The UI-thread display path must hand a frame to the remote worker instead of waiting
    /// behind preview lanes. Never block the window to enforce the native memory cap.
    fn try_acquire(&self, max: usize) -> Option<GpuRenderSlot<'_>> {
        let mut busy = match self.busy.try_lock() {
            Ok(busy) => busy,
            Err(std::sync::TryLockError::Poisoned(error)) => error.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => return None,
        };
        if !cfg!(target_arch = "wasm32") && *busy >= max {
            return None;
        }
        *busy += 1;
        #[cfg(test)]
        self.entered.fetch_add(1, Ordering::SeqCst);
        Some(GpuRenderSlot(self))
    }

    /// A Rayon helper may be executing a nested frame while an admitted render waits for
    /// its work. Blocking that helper on a permit would deadlock the render (#3028).
    fn for_render(&self, max: usize) -> Option<GpuRenderSlot<'_>> {
        // Only a lane's drain thread waits: it renders one frame at a time, protected from
        // nested drains, and a lane runs one drain at a time (it is idle again, and can be
        // started anew, only once its drain found no job, so no frame of the lane is still
        // running). The lane's other threads ([`LANE_WIDTH`]) only help with that frame's
        // parallel work and are Rayon helpers here: they never wait. So no thread waits while
        // an admitted frame waits for it, and waiting keeps the GPU preview's throughput.
        #[cfg(not(target_arch = "wasm32"))]
        if DRAINING_LANE.get() {
            return Some(self.acquire(max));
        }
        if rayon::current_thread_index().is_some() { self.try_acquire(max) } else { Some(self.acquire(max)) }
    }

    /// Wait until fewer than `max` renders are in flight, then take a slot. The browser has no
    /// threads to wait on: it never blocks.
    fn acquire(&self, max: usize) -> GpuRenderSlot<'_> {
        let mut busy = self.busy.lock().unwrap_or_else(PoisonError::into_inner);
        while !cfg!(target_arch = "wasm32") && *busy >= max {
            busy = self.freed.wait(busy).unwrap_or_else(PoisonError::into_inner);
        }
        *busy += 1;
        #[cfg(test)]
        self.entered.fetch_add(1, Ordering::SeqCst);
        GpuRenderSlot(self)
    }
}

impl Drop for GpuRenderSlot<'_> {
    fn drop(&mut self) {
        let mut busy = self.0.busy.lock().unwrap_or_else(PoisonError::into_inner);
        *busy = busy.saturating_sub(1);
        self.0.freed.notify_one();
    }
}

/// The window's frame gaps that adapt [`Queue::gpu_limit`] while a preview plays: a longer one
/// is a stall, shorter ones for a while are smooth.
const UI_STALL: std::time::Duration = std::time::Duration::from_millis(100);
const UI_SMOOTH: std::time::Duration = std::time::Duration::from_millis(50);
/// How long the window must stay smooth before one more lane composites on the GPU, and before it
/// tries again a limit that stalled.
const GPU_LANE_STEP: std::time::Duration = std::time::Duration::from_millis(500);
const GPU_LANE_RETRY: std::time::Duration = std::time::Duration::from_secs(10);

mod f16;
pub use f16::{F16_PIXEL_BYTES, F16Frame};

/// A rendered viewer frame: 8-bit CPU pixels, half-float CPU pixels (32 bpc) or a GPU texture.
#[derive(Clone)]
pub enum FrameImage {
    Cpu(Arc<egui::ColorImage>),
    CpuF16(Arc<F16Frame>),
    Gpu(Arc<DisplayFrame>),
}

impl FrameImage {
    pub fn size(&self) -> [usize; 2] {
        match self {
            FrameImage::Cpu(c) => c.size,
            FrameImage::CpuF16(f) => f.size(),
            FrameImage::Gpu(f) => [f.width as usize, f.height as usize],
        }
    }
    /// Bytes the frame holds in the RAM preview (GPU frames: without their mips).
    pub fn bytes(&self) -> usize {
        let [w, h] = self.size();
        w * h * if matches!(self, FrameImage::CpuF16(_)) { F16_PIXEL_BYTES } else { 4 }
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

/// Where a playing preview is and what it plays next (see [`Frames::set_playhead`]): the RAM
/// preview renders the frames it reaches first before later ones, and keeps them over frames it
/// reaches later or not at all (#595).
#[derive(Clone, Copy, Debug)]
pub struct Playhead {
    /// The frame series that plays ([`FrameKey::same_series`]; its `frame` is ignored).
    pub series: FrameKey,
    /// The frame on screen.
    pub cur: i64,
    /// The preview's range, skip step and looping (as in its `PreviewPlan`).
    pub start: i64,
    pub end: i64,
    pub step: i64,
    pub looping: bool,
}

impl Playhead {
    /// How many frames the preview shows before it reaches `k` (0: on screen); `None` for
    /// frames it doesn't reach (other series, outside the range, behind it without looping).
    pub fn ahead(&self, k: &FrameKey) -> Option<i64> {
        if !k.same_series(&self.series) || k.frame < self.start || k.frame > self.end {
            return None;
        }
        let step = self.step.max(1);
        if k.frame >= self.cur {
            return Some((k.frame - self.cur) / step);
        }
        // Looping: through the end of the range and around from its start.
        self.looping.then(|| (self.end.saturating_sub(self.cur) / step).saturating_add(1).saturating_add((k.frame - self.start) / step))
    }
}

/// A frame in the RAM preview and the source files it was made of (`None`: unknown, a frame
/// rendered elsewhere or kept on the GPU), so it can be written to the disk cache later
/// ([`Frames::persist_missing`]).
struct Cached {
    img: FrameImage,
    /// (the browser's frames are stored by its frame workers, not from here)
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    files: Option<Arc<ReadFiles>>,
    /// The disk cache its render started with and that cache's drop generation then: the frame
    /// is written to that cache only, and only while none of its files was dropped since
    /// (`None`: rendered without a disk cache, or elsewhere; never written from here).
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    since: Option<DiskSince>,
}

/// A disk cache and its drop generation ([`DiskCache::drop_generation`]) when a render started.
#[derive(Clone)]
struct DiskSince(std::sync::Weak<DiskCache>, u64);

impl DiskSince {
    /// A marker for the drop generation of `dc` at the time of a render.
    fn of(dc: &Arc<DiskCache>, generation: u64) -> DiskSince {
        DiskSince(Arc::downgrade(dc), generation)
    }

    /// The generation, when `dc` is the cache it was taken from (the weak reference keeps that
    /// cache's address from being reused by another).
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    fn for_cache(&self, dc: &Arc<DiskCache>) -> Option<u64> {
        std::ptr::eq(self.0.as_ptr(), Arc::as_ptr(dc)).then_some(self.1)
    }
}

struct Cache {
    map: HashMap<FrameKey, Cached>,
    order: VecDeque<FrameKey>,
    bytes: usize,
    /// Bytes of GPU frames (also counted in `bytes`).
    gpu_bytes: usize,
    budget: usize,
    /// Bytes of GPU frames to keep at most ([`GPU_BUDGET`]).
    gpu_budget: usize,
    gpu_pressure: Option<web_time::Instant>,
    gpu_adjusted: Option<web_time::Instant>,
    /// The playing preview, if any: evictions spare the frames it shows next.
    playhead: Option<Playhead>,
    /// Scale of the frame on screen (the newest urgent request): frames of other scales go first.
    scale: Option<u32>,
}

impl Cache {
    /// A frame without its source files (tests).
    #[cfg(test)]
    fn insert(&mut self, k: FrameKey, img: FrameImage) {
        self.insert_with(k, img, None, None);
    }
    /// [`Cache::insert`] with the source files the frame was made of and the disk cache's drop
    /// generation when its render started.
    fn insert_with(&mut self, k: FrameKey, img: FrameImage, files: Option<Arc<ReadFiles>>, since: Option<DiskSince>) {
        let (sz, gpu) = (img.bytes(), img.is_gpu());
        if let Some(old) = self.map.insert(k, Cached { img, files, since }) {
            self.forget(&old.img);
        } else {
            self.order.push_back(k);
        }
        self.bytes += sz;
        if gpu {
            self.gpu_bytes += sz;
        }
        self.evict_to_budget();
        self.evict_gpu();
    }
    /// The frame to evict next: the least recently shown, but while a preview plays, the least
    /// recently shown of the frames it doesn't reach, then the one it reaches last. Plain LRU
    /// evicted the frames just ahead of the playhead first (rendered a while ago, not shown yet)
    /// to keep the ones just shown, so a preview longer than the cache played into a gap that
    /// prefetch kept refilling while it evicted the next frames (#595). Frames of another scale
    /// than the one on screen go first (a zoom or resolution change).
    fn victim(&self) -> Option<usize> {
        if let Some(s) = self.scale
            && let Some(i) = self.order.iter().position(|k| k.scale != s)
        {
            return Some(i);
        }
        let Some(p) = &self.playhead else { return (!self.order.is_empty()).then_some(0) };
        let mut farthest: Option<(i64, usize)> = None;
        for (i, k) in self.order.iter().enumerate() {
            match p.ahead(k) {
                None => return Some(i),
                Some(d) if farthest.is_none_or(|(f, _)| d > f) => farthest = Some((d, i)),
                Some(_) => {}
            }
        }
        farthest.map(|(_, i)| i)
    }
    /// Video memory: drop the oldest GPU frames past their own budget, frames of another scale
    /// than the one on screen first (a zoom or resolution change).
    fn evict_gpu(&mut self) {
        while self.gpu_bytes > self.gpu_budget {
            let is_gpu = |k: &FrameKey| self.map.get(k).is_some_and(|c| c.img.is_gpu());
            let other_scale = self.order.iter().position(|k| Some(k.scale) != self.scale && is_gpu(k));
            let Some(i) = other_scale.or_else(|| self.order.iter().position(is_gpu)) else { break };
            if let Some(old) = self.order.remove(i) {
                self.remove(&old);
            }
        }
    }
    /// Several workers failing in one burst must not halve the shared budget repeatedly.
    fn gpu_out_of_memory(&mut self) {
        let now = web_time::Instant::now();
        if self.gpu_pressure.is_none_or(|last| now.duration_since(last) >= GPU_PRESSURE_COOLDOWN) {
            self.gpu_budget = (self.gpu_budget / 2).max(MIN_GPU_BUDGET);
            self.gpu_adjusted = Some(now);
        }
        self.gpu_pressure = Some(now);
        self.evict_gpu();
    }
    fn gpu_succeeded(&mut self) {
        let now = web_time::Instant::now();
        if self.gpu_pressure.is_some_and(|last| now.duration_since(last) >= GPU_RECOVERY_INTERVAL)
            && self.gpu_adjusted.is_some_and(|last| now.duration_since(last) >= GPU_RECOVERY_INTERVAL)
        {
            self.gpu_budget = self.gpu_budget.saturating_add(MIN_GPU_BUDGET).min(GPU_BUDGET);
            self.gpu_adjusted = Some(now);
        }
    }
    fn gpu_ready(&self) -> bool {
        self.gpu_pressure.is_none_or(|last| last.elapsed() >= GPU_PRESSURE_COOLDOWN)
    }
    fn forget(&mut self, img: &FrameImage) {
        self.bytes = self.bytes.saturating_sub(img.bytes());
        if img.is_gpu() {
            self.gpu_bytes = self.gpu_bytes.saturating_sub(img.bytes());
        }
    }
    /// Removes the entry for `k` and releases its image.
    fn remove(&mut self, k: &FrameKey) {
        if let Some(i) = self.map.remove(k) {
            self.forget(&i.img);
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
    /// Move the frames of identity `from` that `keep` accepts to identity `to` (in place in the
    /// eviction order), unless `to` already has them.
    fn carry(&mut self, from: u64, to: u64, keep: impl Fn(i64) -> bool) {
        for k in self.order.iter_mut().filter(|k| k.content == from && keep(k.frame)) {
            let moved = FrameKey { content: to, ..*k };
            if !self.map.contains_key(&moved)
                && let Some(img) = self.map.remove(k)
            {
                self.map.insert(moved, img);
                *k = moved;
            }
        }
    }
    fn evict_to_budget(&mut self) {
        while self.bytes > self.budget {
            let Some(old) = self.victim().and_then(|i| self.order.remove(i)) else { break };
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

/// Disk-cache content keys ([`disk_cache::comp_content_key`]) of (project revision, comp). They
/// come from the project in memory alone: the files a frame read are checked by its entry
/// when it is read (#3184).
type ContentKeys = Arc<Mutex<HashMap<(u64, u64), u128>>>;

#[cfg(not(target_arch = "wasm32"))]
#[derive(Default)]
struct ContentKeyJobs {
    running: bool,
    pending: VecDeque<(Arc<Project>, u64, u64)>,
}

/// The content key of `comp` at `revision`, memoised per revision.
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
    comp_content_with(project, comp, project, |_, c| c, true)
}

/// Frame ranges (inclusive) of `comp` in `new` that may draw differently than in `old`
/// ([`Project::dirty_spans`]), a frame wider either side for rounding.
fn dirty_frames(old: &Project, new: &Project, comp: ItemId, heard: &Heard, outside: bool) -> Option<Vec<(i64, i64)>> {
    let fr = new.comp(comp)?.frame_rate;
    Some(new.dirty_spans(old, comp, heard, outside)?.into_iter().map(|(s, e)| (fr.frame_at(s).saturating_sub(1), fr.frame_at(e).saturating_add(1))).collect())
}

/// [`comp_content`] with each comp's version selected for both its sources and its address.
/// Sources removed by undo are read from `base`. Only the comps and items it draws count unless
/// `everything` and an expression (which can read any item) is among them.
fn comp_content_with<'a>(
    project: &'a Project,
    comp: ItemId,
    base: &'a Project,
    version: impl Fn(ItemId, &'a Arc<Comp>) -> &'a Arc<Comp>,
    everything: bool,
) -> u64 {
    use std::hash::{Hash, Hasher};
    fn has_expression(g: &PropGroup) -> bool {
        g.children.iter().any(|c| match c {
            Node::Prop(p) => p.expr.is_some(),
            Node::Group(sub) => has_expression(sub),
        })
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    format!("{:?}", project.settings).hash(&mut h);
    let item = |id| project.item(id).or_else(|| base.item(id));
    let mut seen = BTreeSet::new();
    let mut stack = vec![comp];
    let mut expressions = false;
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        let Some(ItemKind::Comp(c)) = item(id).map(|i| &i.kind) else { continue };
        let c = version(id, c);
        for l in &c.layers {
            // (a 3D model's item too)
            if let Some(item) = l.source.item() {
                stack.push(item);
            }
            expressions = expressions || (everything && has_expression(&l.props));
        }
    }
    let ids: Vec<ItemId> = if expressions { project.items.keys().copied().collect() } else { seen.into_iter().collect() };
    for id in ids {
        let Some(it) = item(id) else { continue };
        id.0.hash(&mut h);
        if expressions {
            // (`comp("Name")`, `footage("Name")`)
            it.name.hash(&mut h);
        }
        match &it.kind {
            ItemKind::Comp(c) => (Arc::as_ptr(version(id, c)) as usize).hash(&mut h),
            k => format!("{k:?}").hash(&mut h),
        }
        format!("{:?}", it.proxy).hash(&mut h);
    }
    comp.0.hash(&mut h);
    h.finish()
}

/// Hash of render options (part of [`FrameKey`] and of the disk key). The scale counts as the
/// key's thousandths ([`FrameKey::scale`]): a series rebuilt from the key (the cache bars) has
/// the same options as the frames rendered at 1/3 = 0.333… (#595).
pub fn opts_hash(o: &RenderOpts) -> u64 {
    let o = RenderOpts { scale: (o.scale * 1000.0).round() / 1000.0, ..*o };
    let mut h = disk_cache::Hash128::default();
    h.write(format!("{o:?}").as_bytes());
    h.finish() as u64
}

/// Disk key of a viewer frame (render options included: region of interest, draft…).
fn frame_disk_key(keys: &ContentKeys, project: &Project, key: &FrameKey, opts_hash: u64) -> u128 {
    let content = content_key(keys, project, key.revision, key.comp);
    disk_cache::frame_key(content, key.frame, key.scale, key.view ^ opts_hash)
}

/// Queue `img` for the disk cache under `dk` with the source files it was made of. `false` when
/// refused: a GPU frame (no CPU pixels), files that can't be checked when the entry is read
/// (an incomplete list, #3184: the frame is shown, not stored), a full queue, a file dropped
/// (File ▸ Reload Footage) after `since`, the drop generation its render started at; no room on
/// the disk (checked first: the conversion and compression would be wasted).
fn put_disk_frame(dc: &DiskCache, dk: u128, img: &FrameImage, files: &ReadFiles, virtual_id: Option<disk_cache::VirtualFileId<'_>>, since: u64) -> bool {
    if !files.is_complete() || !dc.accepts_writes() {
        return false;
    }
    match img {
        // (one copy; the disk cache's writer thread compresses it)
        FrameImage::Cpu(img) => dc.put_frame_owned(dk, img.size[0] as u32, img.size[1] as u32, img.as_raw().to_vec(), files, virtual_id, since),
        FrameImage::CpuF16(f) => dc.put_frame_f16(dk, f.width, f.height, f.bits(), files, virtual_id, since),
        FrameImage::Gpu(_) => false,
    }
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
    #[cfg(not(target_arch = "wasm32"))]
    /// One thread each, for a playing preview's frames and idle caching (see [`Queue::idle_lanes`]). Empty if
    /// the OS refused the threads: those frames then render on `pool` too.
    lanes: Vec<rayon::ThreadPool>,
    ctx: Option<egui::Context>,
    /// Render time of the last viewer (urgent) frame in ms, for the Info panel / perf readout.
    pub last_ms: Arc<Mutex<f64>>,
    content_keys: ContentKeys,
    /// Size and content hash for paths with no native metadata (the browser's file table): to
    /// store and check the files of disk entries.
    virtual_id: Option<Arc<dyn Fn(&str) -> Option<(u64, u64)> + Send + Sync>>,
    #[cfg(not(target_arch = "wasm32"))]
    content_key_jobs: Arc<Mutex<ContentKeyJobs>>,
    gpu_retired: Arc<AtomicBool>,
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
    /// Disk keys [`Frames::persist_missing`] is converting (the disk cache tracks them once
    /// queued, so an evicted or refused frame is written again).
    #[cfg(not(target_arch = "wasm32"))]
    persisted: Arc<Mutex<HashSet<u128>>>,
    /// The disk limit and how often each disk key was written under it: a frame is rewritten
    /// once after an eviction, not again (a disk cache smaller than the RAM preview, or a frame
    /// that can't fit, would otherwise be rewritten on every idle pass). A new limit resets it.
    #[cfg(not(target_arch = "wasm32"))]
    written: Arc<Mutex<(u64, HashMap<u128, u8>)>>,
    /// Edits whose kept frames are still to be found on disk under the new content key
    /// ([`Frames::carry_disk`]), oldest first.
    #[cfg(not(target_arch = "wasm32"))]
    disk_carries: Arc<Mutex<VecDeque<DiskCarry>>>,
    /// A carry job is scheduled or running: at most one at a time, draining the edits in order.
    #[cfg(not(target_arch = "wasm32"))]
    carry_scheduled: Arc<AtomicBool>,
    /// The disk series (scale, view and options hash) each comp's frames are cached in.
    #[cfg(not(target_arch = "wasm32"))]
    disk_series: Arc<Mutex<HashMap<u64, Vec<(u32, u64)>>>>,
}

/// An edit's kept frames, to find on disk under the comp's new content key: those of `comp` in
/// `from` outside `dirty` (inclusive frame ranges) also draw `to`'s.
#[cfg(not(target_arch = "wasm32"))]
struct DiskCarry {
    comp: u64,
    from: Arc<Project>,
    to: Arc<Project>,
    dirty: Vec<(i64, i64)>,
}

/// Disk series remembered per comp (see [`Frames::disk_series`]).
#[cfg(not(target_arch = "wasm32"))]
const MAX_DISK_SERIES: usize = 8;

/// Threads per lane (see [`Queue::lanes`]). A frame's passes are memory-bound and stop scaling
/// at about 4 threads, so a core per lane (12 frames at once, each on one thread) and 4 cores
/// per lane cache frames at the same rate (within 3 %, widths 1–4 measured on 12 cores); but the
/// 12 single-threaded frames start and finish together, so a preview that can't keep up shows
/// them in bursts, and its second frame waits for the whole batch (0.3–1.3 s). Three lanes of
/// four threads finish frames 3× as often and the second frame shows in 0.13–0.45 s.
/// Never fewer than two lanes: one would finish frames in a single file.
const LANE_WIDTH: usize = 4;

impl Default for Frames {
    fn default() -> Self {
        Self::with_lane_width(LANE_WIDTH)
    }
}

impl Frames {
    /// A frame cache whose lanes are `width` threads each (see [`LANE_WIDTH`]); `Default` uses
    /// [`LANE_WIDTH`], benchmarks compare widths.
    pub fn with_lane_width(width: usize) -> Self {
        #[cfg(target_arch = "wasm32")]
        let _ = width;
        #[cfg(not(target_arch = "wasm32"))]
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(2, 16);
        #[cfg(not(target_arch = "wasm32"))]
        let width = width.clamp(1, (threads / 2).max(1));
        #[cfg(not(target_arch = "wasm32"))]
        let lanes: Vec<rayon::ThreadPool> = (0..(threads / width).max(1))
            .map_while(|i| {
                rayon::ThreadPoolBuilder::new()
                    .num_threads(width)
                    .thread_name(move |_| format!("ec-preview-{i}"))
                    .panic_handler(|_| log::error!("a frame render panicked; that frame was skipped"))
                    .build()
                    .ok()
            })
            .collect();
        #[cfg(not(target_arch = "wasm32"))]
        let queue = Queue { idle_lanes: (0..lanes.len()).collect(), lanes: lanes.len(), gpu_limit: 1, ..Default::default() };
        #[cfg(target_arch = "wasm32")]
        let queue = Queue::default();
        Frames {
            cache: Arc::new(Mutex::new(Cache { map: HashMap::new(), order: VecDeque::new(), bytes: 0, gpu_bytes: 0, budget: 3 << 30, gpu_budget: GPU_BUDGET, gpu_pressure: None, gpu_adjusted: None, playhead: None, scale: None })),
            inflight: Arc::new(Mutex::new(HashSet::new())),
            queue: Arc::new(Mutex::new(queue)),
            #[cfg(not(target_arch = "wasm32"))]
            // A panicking job would abort the whole process (rayon's default for `spawn`): log it
            // and lose only that frame (the panic hook has logged the message).
            pool: rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .thread_name(|i| format!("ec-frame-{i}"))
                .panic_handler(|_| log::error!("a frame render panicked; that frame was skipped"))
                .build()
                .ok(),
            #[cfg(not(target_arch = "wasm32"))]
            lanes,
            ctx: None,
            last_ms: Arc::new(Mutex::new(0.0)),
            content_keys: Arc::default(),
            virtual_id: None,
            #[cfg(not(target_arch = "wasm32"))]
            content_key_jobs: Arc::default(),
            gpu_retired: Arc::default(),
            identities: Mutex::default(),
            bases: Mutex::default(),
            keepers: Mutex::default(),
            remote: None,
            remote_busy: Arc::default(),
            #[cfg(not(target_arch = "wasm32"))]
            persisted: Arc::default(),
            #[cfg(not(target_arch = "wasm32"))]
            written: Arc::default(),
            #[cfg(not(target_arch = "wasm32"))]
            disk_carries: Arc::default(),
            #[cfg(not(target_arch = "wasm32"))]
            carry_scheduled: Arc::default(),
            #[cfg(not(target_arch = "wasm32"))]
            disk_series: Arc::default(),
        }
    }

    /// How many frames render at once on the lanes (a playing preview's prefetch, Cache Frames
    /// When Idle); in the browser, the frame workers.
    pub fn lanes(&self) -> usize {
        #[cfg(target_arch = "wasm32")]
        return self.remote_slots().max(1);
        #[cfg(not(target_arch = "wasm32"))]
        self.queue.lock().map_or(1, |q| q.lanes.max(1))
    }
}

/// Whether viewer frames of a project with `settings` are half floats ([`F16Frame`], rendered
/// with `RenderOpts::skip_hdr_step`): 32 bpc on the desktop. The browser's frames (its frame
/// worker's RGBA8, the GPU compositor's display textures) stay 8-bit for now.
pub fn f16_frames(settings: &effectcraft_engine::project::ProjectSettings) -> bool {
    cfg!(not(target_arch = "wasm32")) && settings.bit_depth.levels().is_none()
}

/// Bytes per pixel of the viewer frames of a project with `settings` (see [`f16_frames`]).
pub fn frame_pixel_bytes(settings: &effectcraft_engine::project::ProjectSettings) -> usize {
    if f16_frames(settings) { F16_PIXEL_BYTES } else { 4 }
}

/// A rendered frame as a cached viewer frame: half floats for `skip_hdr_step` renders (see
/// [`f16_frames`]), else 8-bit.
pub fn cpu_frame(img: &effectcraft_engine::render::Image, project: &Project, opts: &RenderOpts) -> FrameImage {
    if opts.skip_hdr_step {
        let encoding = effectcraft_engine::viewer::FrameEncoding::of(&project.settings, true);
        FrameImage::CpuF16(Arc::new(F16Frame::from_image(img, encoding)))
    } else {
        FrameImage::Cpu(Arc::new(to_color_image(img)))
    }
}

/// Premultiplied f32 → egui premultiplied Color32 (alpha kept, so the viewer can show the
/// transparency grid or the comp background colour underneath).
pub fn to_color_image(img: &effectcraft_engine::render::Image) -> egui::ColorImage {
    let px: Vec<egui::Color32> = img.data.par_iter().map(|p| display_pixel(*p)).collect();
    egui::ColorImage::new([img.width as usize, img.height as usize], px)
}

pub(crate) fn display_pixel(pixel: [f32; 4]) -> egui::Color32 {
    let alpha = if pixel[3].is_nan() { 0.0 } else { pixel[3].clamp(0.0, 1.0) };
    let component = |value: f32| (value.clamp(0.0, alpha) * 255.0 + 0.5) as u8;
    egui::Color32::from_rgba_premultiplied(component(pixel[0]), component(pixel[1]), component(pixel[2]), (alpha * 255.0 + 0.5) as u8)
}

/// The whole factor an image of `size` is averaged down by for a texture: at least `min_k`, and
/// the smallest that fits its sides in `max_side` (frames of comps wider than the GPU's texture
/// limit, #201).
pub fn fit_factor(size: [usize; 2], max_side: usize, min_k: usize) -> usize {
    size[0].max(size[1]).div_ceil(max_side.max(1)).max(min_k).max(1)
}

/// The size `size` is shown at in a texture whose sides can't exceed `max_side`.
pub fn fitted_size(size: [usize; 2], max_side: usize) -> [usize; 2] {
    let k = fit_factor(size, max_side, 1);
    [size[0].div_ceil(k), size[1].div_ceil(k)]
}

/// The texture coordinates of the image within a texture averaged down by `k`: its last texels
/// cover fewer pixels than the factor, so drawn over the image's area with these the texels sit
/// exactly where their pixels are.
pub fn fitted_uv(size: [usize; 2], k: usize) -> egui::Rect {
    let k = k.max(1);
    let f = |n: usize| (n as f64 / (n.div_ceil(k).max(1) * k) as f64) as f32;
    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(f(size[0]), f(size[1])))
}

/// `img` as a texture the renderer accepts: unchanged when it fits in `max_side`, else averaged
/// down by a whole factor (premultiplied pixels average correctly). The viewer draws it over the
/// same area, so only detail beyond the GPU's texture limit is lost, instead of the upload
/// failing (#201).
pub fn fit_texture(img: egui::ColorImage, max_side: usize) -> egui::ColorImage {
    fit_texture_by(img, max_side, 1)
}

/// [`fit_texture`] averaging down by at least `min_k` (see [`fit_factor`]).
pub fn fit_texture_by(img: egui::ColorImage, max_side: usize, min_k: usize) -> egui::ColorImage {
    let k = fit_factor(img.size, max_side, min_k);
    if k == 1 {
        return img;
    }
    average_down(&img, k)
}

/// A shared frame as texture data (see [`fit_texture_by`]): the frame itself when it needs no
/// averaging, without copying its pixels. The viewer puts every frame a preview shows on screen;
/// copying a 4K frame first held the UI thread for about a millisecond each time (#595).
pub fn fit_shared_texture_by(img: &Arc<egui::ColorImage>, max_side: usize, min_k: usize) -> egui::ImageData {
    let k = fit_factor(img.size, max_side, min_k);
    if k == 1 {
        return egui::ImageData::Color(img.clone());
    }
    average_down(img, k).into()
}

/// `img` averaged down by `k` (rounded up at the edges).
fn average_down(img: &egui::ColorImage, k: usize) -> egui::ColorImage {
    let [w, h] = img.size;
    let (fw, fh) = (w.div_ceil(k), h.div_ceil(k));
    let px: Vec<egui::Color32> = (0..fw * fh)
        .into_par_iter()
        .map(|i| {
            let (x0, y0) = ((i % fw) * k, (i / fw) * k);
            let (mut sum, mut n) = ([0u64; 4], 0u64);
            for y in y0..(y0 + k).min(h) {
                for c in img.pixels.get(y * w + x0..y * w + (x0 + k).min(w)).unwrap_or_default() {
                    for (s, v) in sum.iter_mut().zip(c.to_array()) {
                        *s += v as u64;
                    }
                    n += 1;
                }
            }
            let n = n.max(1);
            let [r, g, b, a] = sum.map(|s| ((s + n / 2) / n) as u8);
            egui::Color32::from_rgba_premultiplied(r, g, b, a)
        })
        .collect();
    egui::ColorImage::new([fw, fh], px)
}

/// The largest texture side the renderer accepts.
pub fn max_texture_side(ctx: &egui::Context) -> usize {
    ctx.input(|i| i.max_texture_side)
}

/// `ctx.load_texture` for frame-sized images: fitted to the renderer's texture limit first.
pub fn load_fitted(ctx: &egui::Context, name: impl Into<String>, img: egui::ColorImage, opts: egui::TextureOptions) -> egui::TextureHandle {
    ctx.load_texture(name, fit_texture(img, max_texture_side(ctx)), opts)
}

impl Frames {
    /// Retire only GPU preview state; documents, CPU frames and running job leases survive.
    /// Publication checks the same latch while holding the cache lock, so a late GPU frame
    /// cannot restore an invalid texture after this removal.
    pub fn retire_gpu(&self) {
        self.gpu_retired.store(true, Ordering::Release);
        if let Ok(mut cache) = self.cache.lock() {
            let keys: Vec<_> = cache.map.iter().filter_map(|(key, c)| c.img.is_gpu().then_some(*key)).collect();
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
        let img = c.map.get(k).map(|c| c.img.clone())?;
        c.touch(k);
        Some(img)
    }

    /// Prefer the exact requested frame. While an edit renders, advance to the newest completed
    /// revision of the same comp, time, scale, view and render options instead of freezing on the
    /// image from before the drag. Input may arrive before the viewer draws on every UI frame,
    /// so even a fast worker cannot always finish the *latest* revision in time.
    ///
    /// Never move backwards when workers finish out of order, and never substitute another time
    /// or view while scrubbing or switching comps. The exact content lookup above also means an
    /// undo can immediately reuse an old cache entry, regardless of its original revision.
    pub fn get_for_viewer(&self, wanted: &FrameKey, shown: Option<&FrameKey>) -> Option<(FrameKey, FrameImage)> {
        if let Some(img) = self.get(wanted) {
            return Some((*wanted, img));
        }
        let shown = shown?;
        if shown.comp != wanted.comp || shown.frame != wanted.frame || shown.view != wanted.view || shown.revision >= wanted.revision {
            return None;
        }
        let mut c = self.cache.lock().ok()?;
        let newest = c
            .map
            .keys()
            .filter(|k| {
                k.revision > shown.revision
                    && k.revision < wanted.revision
                    && k.comp == wanted.comp
                    && k.frame == wanted.frame
                    && k.scale == wanted.scale
                    && k.view == wanted.view
                    && k.opts == wanted.opts
            })
            .max_by_key(|k| k.revision)
            .copied()?;
        let img = c.map.get(&newest).map(|c| c.img.clone())?;
        c.touch(&newest);
        Some((newest, img))
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
        // Markers are pixels only to the expressions that read them (#1083).
        let markers = project.reads_markers();
        // Whose audio is drawn, before or after the edit (a waveform added or removed counts).
        let heard = bases.get(&comp.0).map_or_else(|| project.heard_layers(), |(_, base)| project.heard_layers().union(&base.heard_layers()));
        if let Some((base_id, base)) = bases.get(&comp.0)
            && *base_id != plain
            && comp_content_with(
                project,
                comp,
                project,
                |id, c| match base.item(id).map(|i| &i.kind) {
                    Some(ItemKind::Comp(b)) if Arc::ptr_eq(b, c) || b.same_pixels_with(c, markers, &|l| heard.contains(id, l)) => b,
                    _ => c,
                },
                true,
            ) == *base_id
        {
            return (*base_id, Some(base.clone()));
        }
        // Only comps changed (not the footage, solids or settings they draw): its frames outside
        // the spans the edit reaches (and a frame either side for rounding) keep drawing the same
        // pixels, so a nudge, cut or trim, here or in a precomp, recaches only the frames it
        // touched (#1641). Expressions can read anything at any time, so with them a change
        // elsewhere in the project (another comp, an item's name) reaches the frames of the
        // layers that have one.
        let base_comp = |id: ItemId| bases.get(&comp.0).and_then(|(_, base)| base.item(id)).map(|i| &i.kind);
        let base_tree = |id, c| match base_comp(id) {
            Some(ItemKind::Comp(b)) if id == comp || Arc::ptr_eq(b, c) || b.same_pixels_with(c, markers, &|l| heard.contains(id, l)) => b,
            _ => c,
        };
        let base_comps = |id, c| match base_comp(id) {
            Some(ItemKind::Comp(b)) => b,
            _ => c,
        };
        if let Some((base_id, base)) = bases.get(&comp.0)
            && comp_content_with(project, comp, base, base_comps, false) == comp_content_with(base, comp, base, |_, c| c, false)
            && let Some(dirty) = dirty_frames(base, project, comp, &heard, comp_content_with(project, comp, base, base_tree, true) != *base_id)
            && let Ok(mut cache) = self.cache.lock()
        {
            cache.carry(*base_id, plain, |f| !dirty.iter().any(|(lo, hi)| (*lo..=*hi).contains(&f)));
            #[cfg(not(target_arch = "wasm32"))]
            if let Ok(mut q) = self.disk_carries.lock() {
                if q.len() >= MAX_KEEPERS {
                    q.pop_front();
                }
                q.push_back(DiskCarry { comp: comp.0, from: base.clone(), to: project.clone(), dirty });
            }
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
    /// `opts` are the viewer's render options (part of the disk key). Native queries return
    /// no blue frames until the content key is ready; hashing never blocks timeline painting.
    pub fn disk_frames(&self, src: &RenderSource, base: &FrameKey, range: std::ops::Range<i64>, opts: &RenderOpts) -> Vec<i64> {
        let start = range.start.clamp(0, 100_000);
        let end = range.end.clamp(0, 100_000);
        if start >= end {
            return vec![];
        }
        let remote = self.remote.as_ref().filter(|r| r.disk());
        let contains = |k: u128| match (&src.disk, remote) {
            (Some(dc), _) => dc.contains(disk_cache::Kind::Frame, k),
            (None, Some(r)) => r.disk_contains(k),
            (None, None) => false,
        };
        if src.disk.is_none() && remote.is_none() {
            return vec![];
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.note_disk_series(base.comp, base.scale, base.view ^ opts_hash(opts));
            self.carry_disk(src);
        }
        #[cfg(not(target_arch = "wasm32"))]
        let content = {
            let known = self.content_keys.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(&(base.revision, base.comp)).copied();
            let Some(content) = known else {
                self.request_content_key(src.project.clone(), base.revision, base.comp);
                return vec![];
            };
            content
        };
        #[cfg(target_arch = "wasm32")]
        let content = content_key(&self.content_keys, &src.project, base.revision, base.comp);
        let ram: HashSet<i64> = self.cached_frames(base).into_iter().collect();
        let oh = opts_hash(opts);
        (start..end).filter(|f| !ram.contains(f)).filter(|f| contains(disk_cache::frame_key(content, *f, base.scale, base.view ^ oh))).collect()
    }

    /// Write the RAM frames of `base`'s series that the disk cache lacks: frames an edit kept
    /// (written under the comp's previous content key) and writes the disk cache dropped. They
    /// are converted on the frame pool, each queued once. `true` while there may be more to
    /// write (a content key being computed, frames being converted): call again soon, since a
    /// refused write is only retried by a later call. Disk keys hash `opts` the way the worker
    /// writes them (`base.opts` also carries the Roto Brush model stamp). Only frames that know
    /// every source file they were made of are written (an entry is checked against them).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn persist_missing(&self, src: &RenderSource, base: &FrameKey, opts: &RenderOpts) -> bool {
        let Some(dc) = src.disk.clone() else { return false };
        let oh = opts_hash(opts);
        self.note_disk_series(base.comp, base.scale, base.view ^ oh);
        self.carry_disk(src);
        // Free disk space leaves the cache (next to) no room: nothing would be stored, so nothing
        // is converted (a refusal doesn't count as a try, so every idle pass would convert them
        // all again). A later idle pass writes them once there is room (`prefs_live::memory_watch`
        // keeps frames coming meanwhile).
        if !dc.accepts_writes() {
            return false;
        }
        let known = self.content_keys.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(&(base.revision, base.comp)).copied();
        let Some(content) = known else {
            self.request_content_key(src.project.clone(), base.revision, base.comp);
            return true;
        };
        let (missing, converting): (Vec<(u128, FrameImage, Arc<ReadFiles>, u64)>, bool) = {
            let (Ok(c), Ok(mut queued), Ok(mut written)) = (self.cache.lock(), self.persisted.lock(), self.written.lock()) else { return false };
            if queued.len() > 100_000 {
                queued.clear();
            }
            let limit = dc.limit_bytes();
            if written.0 != limit || written.1.len() > 100_000 {
                *written = (limit, HashMap::new());
            }
            let missing = c
                .map
                .iter()
                .filter(|(k, _)| k.same_series(base))
                .filter_map(|(k, c)| {
                    // (GPU frames have no CPU pixels to write; a frame that can't name its files
                    // can't be checked when read)
                    let files = c.files.as_ref().filter(|f| f.is_complete() && !c.img.is_gpu())?;
                    // (only with the generation its render started at, for this cache: a frame
                    // made of a file Reload Footage dropped since holds the pixels from before)
                    let since = c.since.as_ref().and_then(|s| s.for_cache(&dc)).filter(|s| !dc.dropped_since(files, *s))?;
                    let dk = disk_cache::frame_key(content, k.frame, k.scale, k.view ^ oh);
                    let tries = written.1.get(&dk).copied().unwrap_or(0);
                    (tries < 2 && !dc.contains(disk_cache::Kind::Frame, dk) && !dc.frame_pending(dk) && queued.insert(dk))
                        .then(|| (dk, c.img.clone(), files.clone(), since))
                })
                .collect();
            (missing, !queued.is_empty())
        };
        if missing.is_empty() {
            return converting;
        }
        let (persisted, written, virtual_id) = (self.persisted.clone(), self.written.clone(), self.virtual_id.clone());
        let work = move || {
            for (dk, img, files, since) in missing {
                // (accepted: the disk cache tracks it until written, and it counts as a try;
                // refused by a full queue: retried, without counting)
                let accepted = put_disk_frame(&dc, dk, &img, &files, virtual_id.as_deref(), since);
                if accepted && let Ok(mut w) = written.lock() {
                    let n = w.1.entry(dk).or_default();
                    *n = n.saturating_add(1);
                }
                if let Ok(mut q) = persisted.lock() {
                    q.remove(&dk);
                }
            }
        };
        match &self.pool {
            Some(pool) => pool.spawn(work),
            None => rayon::spawn(work),
        }
        true
    }

    /// Remember that `comp`'s frames are cached on disk at `scale` with view and options hash
    /// `view` (the most recent [`MAX_DISK_SERIES`]).
    #[cfg(not(target_arch = "wasm32"))]
    fn note_disk_series(&self, comp: u64, scale: u32, view: u64) {
        if let Ok(mut m) = self.disk_series.lock() {
            let v = m.entry(comp).or_default();
            if v.first() != Some(&(scale, view)) {
                v.retain(|s| *s != (scale, view));
                v.insert(0, (scale, view));
                v.truncate(MAX_DISK_SERIES);
            }
        }
    }

    /// Find the frames edits kept ([`Frames::identity`]) on disk under their comps' new content
    /// keys: link each kept frame of every disk series of the comp to its new key, on the frame
    /// pool, oldest edit first.
    #[cfg(not(target_arch = "wasm32"))]
    fn carry_disk(&self, src: &RenderSource) {
        let Some(dc) = src.disk.clone() else { return };
        let pending = |q: &Mutex<VecDeque<DiskCarry>>| q.lock().is_ok_and(|q| !q.is_empty());
        // (called on every paint: one job drains every carry queued meanwhile)
        if !pending(&self.disk_carries) || self.carry_scheduled.swap(true, Ordering::AcqRel) {
            return;
        }
        let (carries, scheduled, series, ctx) = (self.disk_carries.clone(), self.carry_scheduled.clone(), self.disk_series.clone(), self.ctx.clone());
        let work = move || {
            let mut linked = false;
            loop {
                let todo: Vec<DiskCarry> = carries.lock().map(|mut q| q.drain(..).collect()).unwrap_or_default();
                if todo.is_empty() {
                    scheduled.store(false, Ordering::Release);
                    // (a carry queued after the drain and before the store: keep it, unless a new job took it)
                    if pending(&carries) && !scheduled.swap(true, Ordering::AcqRel) {
                        continue;
                    }
                    break;
                }
                for c in todo {
                    // (a panicking carry is skipped: the others still carry, and the flag is
                    // still cleared below)
                    let carry = || {
                        let Some(series) = series.lock().ok().and_then(|m| m.get(&c.comp).cloned()) else { return false };
                        let comp = ItemId(c.comp);
                        let n = c.to.comp(comp).map_or(0, |k| k.frame_rate.frame_at(k.duration)).clamp(0, 100_000);
                        let key = |p: &Project| disk_cache::comp_content_key(p, comp);
                        let (a, b) = (key(&c.from), key(&c.to));
                        if a == b {
                            return false;
                        }
                        let mut linked = false;
                        for f in (0..n).filter(|f| !c.dirty.iter().any(|(lo, hi)| (*lo..=*hi).contains(f))) {
                            for (scale, view) in &series {
                                linked |= dc.link_frame(disk_cache::frame_key(a, f, *scale, *view), disk_cache::frame_key(b, f, *scale, *view));
                            }
                        }
                        linked
                    };
                    linked |= std::panic::catch_unwind(std::panic::AssertUnwindSafe(carry)).unwrap_or(false);
                }
            }
            if linked && let Some(ctx) = &ctx {
                ctx.request_repaint();
            }
        };
        match &self.pool {
            Some(pool) => pool.spawn(work),
            None => rayon::spawn(work),
        }
    }

    /// Queues a background hash of `comp` at `revision`, coalescing rapid edits so only a bounded number of snapshots are kept.
    #[cfg(not(target_arch = "wasm32"))]
    fn request_content_key(&self, project: Arc<Project>, revision: u64, comp: u64) {
        let mut jobs = self.content_key_jobs.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        // Coalesce rapid edits. One worker hashes at most one revision at a time; retained
        // snapshots are bounded even with multiple timelines open.
        jobs.pending.retain(|(_, _, c)| *c != comp);
        if jobs.pending.len() >= 8 {
            jobs.pending.pop_front();
        }
        jobs.pending.push_back((project, revision, comp));
        if jobs.running {
            return;
        }
        jobs.running = true;
        drop(jobs);
        let (jobs, keys, ctx) = (self.content_key_jobs.clone(), self.content_keys.clone(), self.ctx.clone());
        let work = move || loop {
            let next = {
                let mut jobs = jobs.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                let next = jobs.pending.pop_back();
                if next.is_none() {
                    jobs.running = false;
                }
                next
            };
            let Some((project, revision, comp)) = next else { return };
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| content_key(&keys, &project, revision, comp))).is_err() {
                log::error!("could not compute a preview disk-cache key; the cache indicator will retry");
            }
            if let Some(ctx) = &ctx {
                ctx.request_repaint();
            }
        };
        match &self.pool {
            Some(pool) => pool.spawn(work),
            None => rayon::spawn(work),
        }
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

    /// Drops every cached frame and purges the remote server, if there is one.
    pub fn clear(&self) {
        if let Some(r) = &self.remote {
            r.purge();
        }
        if let Ok(mut c) = self.cache.lock() {
            c.map.clear();
            c.order.clear();
            c.bytes = 0;
            c.gpu_bytes = 0;
            c.gpu_budget = GPU_BUDGET;
            c.gpu_pressure = None;
            c.gpu_adjusted = None;
        }
        // (the disk keys of a revision don't depend on files: a file changed on disk is caught
        // when its frames' entries are read; they are forgotten with the frames all the same)
        if let Ok(mut k) = self.content_keys.lock() {
            k.clear();
        }
    }

    /// The playing preview's position (`None` when no preview plays): prefetch renders the
    /// frames it reaches first before the others, evictions spare the frames it shows next, and
    /// queued prefetch frames of its series it no longer reaches are dropped (#595).
    pub fn set_playhead(&self, playhead: Option<Playhead>) {
        if let Ok(mut c) = self.cache.lock() {
            c.playhead = playhead;
        }
        let Ok(mut q) = self.queue.lock() else { return };
        // A preview starting or stopping moves the queued prefetch frames between the lanes and
        // the pool (see [`Queue::idle_lanes`]).
        #[cfg(not(target_arch = "wasm32"))]
        let moved = q.playhead.is_some() != playhead.is_some();
        q.playhead = playhead;
        // (called once per frame of the window while a preview plays)
        let more_gpu = q.ui_frame(playhead.map(|_| web_time::Instant::now()));
        if let Some(p) = playhead
            && let Ok(mut inf) = self.inflight.lock()
        {
            q.jobs.retain(|j| {
                let gone = !j.urgent && j.key.same_series(&p.series) && p.ahead(&j.key).is_none();
                if gone {
                    inf.remove(&j.key);
                }
                !gone
            });
        }
        // A lane more may composite: one that stopped at the limit starts again.
        #[cfg(not(target_arch = "wasm32"))]
        if more_gpu && !moved {
            self.start_lanes(&mut q, 1);
        }
        #[cfg(target_arch = "wasm32")]
        let _ = more_gpu;
        #[cfg(not(target_arch = "wasm32"))]
        if moved {
            self.start_lanes(&mut q, usize::MAX);
            let pool_jobs = q.jobs.iter().filter(|j| !q.on_lane(j)).count().min(self.parallelism());
            drop(q);
            for _ in 0..pool_jobs {
                self.start_pool();
            }
        }
    }

    /// Start up to `n` idle lanes on the queued jobs (see [`Queue::idle_lanes`]).
    #[cfg(not(target_arch = "wasm32"))]
    fn start_lanes(&self, q: &mut Queue, n: usize) {
        if self.remote.is_some() {
            return;
        }
        let jobs = q.jobs.iter().filter(|j| q.on_lane(j)).count();
        for _ in 0..n.min(jobs) {
            let Some(lane) = q.idle_lanes.pop() else { break };
            let Some(pool) = self.lanes.get(lane) else { break };
            let w = self.worker();
            pool.spawn(move || w.drain(Some(lane)));
        }
    }

    /// Have a pool thread render the queued jobs that are the pool's.
    #[cfg(not(target_arch = "wasm32"))]
    fn start_pool(&self) {
        if self.remote.is_some() {
            return;
        }
        let w = self.worker();
        let job = move || w.drain(None);
        match &self.pool {
            Some(pool) => pool.spawn(job),
            None => rayon::spawn(job),
        }
    }

    /// Queue a prefetch frame (no-op if cached or already queued/rendering).
    pub fn request(&self, src: &RenderSource, key: FrameKey, comp: ItemId, t: Tick, opts: RenderOpts) {
        self.request_with(src, key, comp, t, opts, Want::Prefetch);
    }

    /// Queue a frame nobody waits for (Cache Frames When Idle): as [`Frames::request`], but it
    /// renders on a lane, one thread, and leaves the pool to the viewer's frame (see
    /// [`Queue::idle_lanes`]).
    pub fn request_background(&self, src: &RenderSource, key: FrameKey, comp: ItemId, t: Tick, opts: RenderOpts) {
        self.request_with(src, key, comp, t, opts, Want::Background);
    }

    /// Queue the frame the viewer is showing: it jumps ahead of every prefetch job.
    pub fn request_urgent(&self, src: &RenderSource, key: FrameKey, comp: ItemId, t: Tick, opts: RenderOpts) {
        self.request_with(src, key, comp, t, opts, Want::Viewer);
    }

    /// An urgent (viewer) frame is queued or rendering.
    pub fn urgent_pending(&self) -> bool {
        self.queue.lock().map(|q| q.urgent_running > 0 || q.jobs.iter().any(|j| j.urgent)).unwrap_or(false)
    }

    fn request_with(&self, src: &RenderSource, key: FrameKey, comp: ItemId, t: Tick, opts: RenderOpts, want: Want) {
        let urgent = want == Want::Viewer;
        // The frame on screen sets the scale evictions keep: a prefetch or a passive viewer at
        // another zoom must not evict the active viewer's frames.
        let cached = self
            .cache
            .lock()
            .map(|mut c| {
                if urgent {
                    c.scale = Some(key.scale);
                }
                c.map.contains_key(&key)
            })
            .unwrap_or(false);
        if cached {
            return;
        }
        let Ok(mut q) = self.queue.lock() else { return };
        let Ok(mut inf) = self.inflight.lock() else { return };
        // Jobs for an older project revision can never be shown: drop them.
        let before = q.jobs.len();
        q.jobs.retain(|j| j.key.revision >= key.revision);
        if q.jobs.len() < before {
            inf.retain(|k| k.revision >= key.revision || q.running.contains(k));
        }
        if urgent {
            // Only the newest viewer frame is urgent. The comp's earlier viewer frames that
            // haven't started are dropped (scrubbing: rendering every frame the pointer passed
            // would hold the workers the frame under it needs); frames already rendering finish
            // into the cache. During edits the viewer advances through compatible completed
            // revisions; scrubbing holds its last frame until the requested time is in (#284).
            // Other comps' viewer frames become prefetch.
            q.jobs.retain(|j| {
                let superseded = j.urgent && j.key != key && j.key.comp == key.comp;
                if superseded {
                    inf.remove(&j.key);
                }
                !superseded
            });
            for j in q.jobs.iter_mut() {
                j.urgent = j.key == key;
            }
            // The comp's viewer frames of other times that are rendering are called off too: a
            // movie seeking for each of them held the decoder the frame under the pointer
            // waited for, so it came seconds after the pointer stopped (#329). Another revision
            // of the same frame (a live edit) finishes, as the viewer shows it meanwhile.
            for (k, cancel) in &q.urgent_cancels {
                if k.comp == key.comp && k.frame != key.frame {
                    cancel.store(true, Ordering::Relaxed);
                }
            }
        }
        if !inf.insert(key) {
            drop(inf);
            drop(q);
            // Queued for a lane and now the viewer's frame: it is the pool's.
            #[cfg(not(target_arch = "wasm32"))]
            if urgent {
                self.start_pool();
            }
            return;
        }
        q.seq += 1;
        let seq = q.seq;
        let job = Job { key, src: src.clone(), comp, t, opts, urgent, background: want == Want::Background, seq, cancel: Arc::default() };
        // A lane's frame: a busy lane takes the next job when it is done, an idle one starts now.
        #[cfg(not(target_arch = "wasm32"))]
        let on_lane = q.on_lane(&job);
        q.jobs.push(job);
        #[cfg(not(target_arch = "wasm32"))]
        if on_lane {
            self.start_lanes(&mut q, 1);
        }
        drop(inf);
        drop(q);
        // One spawn per job; each spawn renders whichever queued jobs matter most right now.
        #[cfg(not(target_arch = "wasm32"))]
        if !on_lane {
            self.start_pool();
        }
        #[cfg(target_arch = "wasm32")]
        if let Some(ctx) = &self.ctx {
            // rendered by `pump` after this UI frame
            ctx.request_repaint();
        }
    }

    /// A render worker sharing this cache's queue, cache and in-flight set.
    fn worker(&self) -> Worker {
        Worker {
            queue: self.queue.clone(),
            cache: self.cache.clone(),
            inflight: self.inflight.clone(),
            ctx: self.ctx.clone(),
            last: self.last_ms.clone(),
            content_keys: self.content_keys.clone(),
            virtual_id: self.virtual_id.clone(),
            gpu_retired: self.gpu_retired.clone(),
        }
    }

    /// Browser file table: `id` gives the size and content hash of a path served from memory,
    /// stored with the disk entries of the frames that read it and checked when they are read.
    pub fn set_virtual_id(&mut self, id: Arc<dyn Fn(&str) -> Option<(u64, u64)> + Send + Sync>) {
        self.virtual_id = Some(id);
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
                let Some(i) = q.best() else { break };
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
                // (A frame kept on the GPU is not written to the disk cache: its files are not
                // needed.)
                if let Some((img, _files)) = w.render_gpu(&job) {
                    w.finish(&job, img, None, None, t0);
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
            remote.start(
                rj,
                Box::new(move |r: Result<RemoteFrame, String>| {
                    if let Ok(mut b) = busy.lock() {
                        *b = b.saturating_sub(1);
                    }
                    match r {
                        Ok(f) => {
                            let img = egui::ColorImage::from_rgba_premultiplied([f.width as usize, f.height as usize], &f.rgba);
                            fw.finish_key(key, urgent, FrameImage::Cpu(Arc::new(img)), None, None, t0);
                        }
                        Err(e) => {
                            // Not rendered (a stale revision, a lost worker): the viewer asks again.
                            log::debug!("remote frame: {e}");
                            fw.release(key, urgent);
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
        while w.run_next(None) {
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
    virtual_id: Option<Arc<dyn Fn(&str) -> Option<(u64, u64)> + Send + Sync>>,
    gpu_retired: Arc<AtomicBool>,
}

#[cfg(not(target_arch = "wasm32"))]
thread_local! {
    /// This pool thread is inside [`Worker::drain`] (rendering a frame).
    static DRAINING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Only a lane's drain thread may wait for GPU admission (see [`GpuRenderSlots::for_render`]).
    static DRAINING_LANE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Clears [`DRAINING`] when the drain ends, even by a panic.
#[cfg(not(target_arch = "wasm32"))]
struct Draining;

#[cfg(not(target_arch = "wasm32"))]
impl Draining {
    fn enter(lane: Option<usize>) -> Option<Self> {
        if DRAINING.replace(true) {
            return None;
        }
        DRAINING_LANE.set(lane.is_some());
        Some(Self)
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for Draining {
    fn drop(&mut self) {
        DRAINING_LANE.set(false);
        DRAINING.set(false);
    }
}

/// A lane's hold on one of the [`Queue::gpu_limit`] GPU slots (see [`Queue::gpu_lanes`]), given back when dropped,
/// even by a panic.
struct GpuSlot<'a>(&'a Mutex<Queue>);

impl Drop for GpuSlot<'_> {
    fn drop(&mut self) {
        let mut q = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        q.gpu_lanes = q.gpu_lanes.saturating_sub(1);
    }
}

impl Worker {
    /// Render queued jobs, most important first, until the queue is empty.
    ///
    /// A pool thread waiting inside a render (on a parallel loop's join) runs other pool work
    /// meanwhile, including the spawns of newer requests: each would start a whole frame render
    /// on top of the one waiting, with all of its buffers. Dragging a layer asks for a new frame
    /// every UI frame, so renders piled up on every thread until memory ran out (#647). A spawn
    /// that lands inside a render returns at once instead; the render below it takes the queued
    /// jobs when it is done, so each thread renders one frame at a time.
    ///
    /// `lane`: the lane this runs on (see [`Queue::idle_lanes`]), `None` on the pool.
    #[cfg(not(target_arch = "wasm32"))]
    fn drain(&self, lane: Option<usize>) {
        let Some(_reset) = Draining::enter(lane) else { return };
        while self.run_next(lane) {}
    }

    /// Render the queued job that matters most right now, on lane `lane` (`None`: on
    /// the pool, or the UI thread). `false` when the queue had no job for it; the lane is then
    /// idle again.
    fn run_next(&self, lane: Option<usize>) -> bool {
        let Worker { queue, content_keys, .. } = self;
        // (asked before the queue's lock: it takes the cache's)
        let gpu = lane.is_some() && self.gpu_ready();
        let (job, mut slot) = {
            let Ok(mut q) = queue.lock() else { return false };
            let Some(i) = q.next(lane.is_some(), gpu) else {
                q.idle_lanes.extend(lane);
                return false;
            };
            let job = q.jobs.swap_remove(i);
            let slot = (gpu && q.gpu_job(&job)).then(|| {
                q.gpu_lanes += 1;
                GpuSlot(queue)
            });
            q.running.insert(job.key);
            if job.urgent {
                q.urgent_running += 1;
                q.urgent_cancels.push((job.key, job.cancel.clone()));
            }
            (job, slot)
        };
        let t0 = web_time::Instant::now();
        // (the drop generation before the frame reads anything: File ▸ Reload Footage meanwhile
        // makes what it read from a reloaded file stale)
        let disk =
            job.src.disk.as_ref().map(|dc| (dc.clone(), frame_disk_key(content_keys, &job.src.project, &job.key, opts_hash(&job.opts)), dc.drop_generation()));
        let from_disk = disk.as_ref().and_then(|(dc, k, _)| self.disk_frame(dc, *k, &job));
        let disk_hit = from_disk.is_some();
        if disk_hit {
            // Nothing for the GPU to do: another lane may composite meanwhile.
            slot = None;
        }
        // `files`: the source files the frame was made of (see `render::reads`).
        let (ci, files) = match from_disk {
            Some(hit) => hit,
            None => match self.render_guarded(&job).filter(|_| !job.cancel.load(Ordering::Relaxed)) {
                Some(rendered) => rendered,
                // Called off (frames it read may be missing) or failed: not cached, and not
                // "in progress" forever: the viewer asks for it again.
                None => {
                    self.release(job.key, job.urgent);
                    return true;
                }
            },
        };
        let mut files = files;
        if let Some((dc, _, since)) = &disk
            && dc.dropped_since(&files, *since)
        {
            // Made of a file as it was before File ▸ Reload Footage dropped it: shown, but its
            // list can't vouch for it, so neither this worker nor `persist_missing` stores it.
            files.mark_incomplete();
        }
        let files = Arc::new(files);
        // Publish the shared frame and wake the viewer before optional RGBA conversion
        // and compression. Persistence still runs on this worker, using the same snapshot.
        let since = disk.as_ref().map(|(dc, _, since)| DiskSince::of(dc, *since));
        self.finish(&job, ci.clone(), Some(files.clone()), since, t0);
        // The GPU's part is done; storing it on disk is the CPU's.
        drop(slot);
        if !disk_hit
            && let Some((dc, k, since)) = &disk
            && put_disk_frame(dc, *k, &ci, &files, self.virtual_id.as_deref(), *since)
            && let Some(ctx) = &self.ctx
        {
            ctx.request_repaint();
        }
        true
    }

    /// The frame of `job` from the disk cache (key `k`), if its entry is there and the source
    /// files it lists are unchanged (checked here, on the frame worker: a few `metadata` calls
    /// for that frame's files). An entry whose files changed is deleted: the frame renders again
    /// and its new entry replaces it.
    fn disk_frame(&self, dc: &DiskCache, k: u128, job: &Job) -> Option<(FrameImage, ReadFiles)> {
        let virtual_id = self.virtual_id.as_deref();
        // Half-float frames (32 bpc) have entries of their own (their key has the options).
        if job.opts.skip_hdr_step {
            let (f, files) = dc.get_frame_f16(k, virtual_id)?;
            let encoding = effectcraft_engine::viewer::FrameEncoding::of(&job.src.project.settings, true);
            Some((FrameImage::CpuF16(Arc::new(F16Frame::from_bits(f.width, f.height, f.rgba, encoding))), files))
        } else {
            let (f, files) = dc.get_frame(k, virtual_id)?;
            Some((FrameImage::Cpu(Arc::new(egui::ColorImage::from_rgba_premultiplied([f.width as usize, f.height as usize], &f.rgba))), files))
        }
    }

    /// Store a finished frame (with the source files it was made of, when known) and release its
    /// job.
    fn finish(&self, job: &Job, img: FrameImage, files: Option<Arc<ReadFiles>>, since: Option<DiskSince>, t0: web_time::Instant) {
        self.finish_key(job.key, job.urgent, img, files, since, t0);
    }

    /// Stores a finished frame in the cache with its read files, records the render time for urgent jobs, then releases the job.
    fn finish_key(&self, key: FrameKey, urgent: bool, img: FrameImage, files: Option<Arc<ReadFiles>>, since: Option<DiskSince>, t0: web_time::Instant) {
        if urgent && let Ok(mut l) = self.last.lock() {
            *l = t0.elapsed().as_secs_f64() * 1000.0;
        }
        if let Ok(mut c) = self.cache.lock()
            && (!img.is_gpu() || !self.gpu_retired.load(Ordering::Acquire))
        {
            c.insert_with(key, img, files, since);
        }
        self.release(key, urgent);
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
                if let Some(i) = q.urgent_cancels.iter().position(|(k, _)| *k == key) {
                    q.urgent_cancels.swap_remove(i);
                }
            }
        }
        if let Some(ctx) = &self.ctx {
            ctx.request_repaint();
        }
    }

    /// [`Worker::render_job`], again on the CPU if it panicked (the panic hook has logged it);
    /// `None` if that panicked too. With the source files the frame was made of.
    fn render_guarded(&self, job: &Job) -> Option<(FrameImage, ReadFiles)> {
        let attempt = |gpu: bool| {
            let files = ReadLog::new();
            let img = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.render_job(job, gpu, &files))).ok()?;
            Some((img, files.into_files()))
        };
        let img = attempt(true).or_else(|| {
            log::error!("a frame render panicked (frame {})", job.key.frame);
            job.src.gpu.as_ref().and_then(|_| attempt(false))
        });
        // A preview frame is done: the GPU pool ages the sizes it did not ask for.
        if let Some(g) = &job.src.gpu {
            effectcraft_engine::render::Accelerator::frame_end(g);
        }
        img
    }

    /// A failed scoped attempt retries on CPU. Retired contexts cannot recover by reducing
    /// their budget; only a healthy device's allocation failure uses the upstream backoff.
    fn gpu_attempt_failed(&self, gpu: &Gpu, e: &FrameError) {
        if gpu.context().check_health().is_err() {
            log::warn!("the GPU context is retired ({e}); the frame renders on the CPU");
            return;
        }
        // Not memory pressure: keeping fewer GPU frames wouldn't help (#1319).
        if let FrameError::Validation(_) = e {
            log::warn!("the GPU rejected the frame ({e}); it renders on the CPU, the next frame tries the GPU again");
            return;
        }
        log::warn!("the GPU ran out of memory ({e}); the frame renders on the CPU and fewer GPU frames are kept");
        if let Ok(mut c) = self.cache.lock() {
            c.gpu_out_of_memory();
        }
    }

    /// Tells the cache that a GPU render succeeded.
    fn gpu_succeeded(&self) {
        if let Ok(mut c) = self.cache.lock() {
            c.gpu_succeeded();
        }
    }

    /// Whether GPU rendering is available and has not been retired.
    fn gpu_ready(&self) -> bool {
        !self.gpu_retired.load(Ordering::Acquire) && self.cache.lock().is_ok_and(|c| c.gpu_ready())
    }

    /// The frame on the GPU, kept there for the viewer, if the compositor can do it alone, with
    /// the source files it was made of.
    fn render_gpu(&self, job: &Job) -> Option<(FrameImage, ReadFiles)> {
        if !self.gpu_ready() {
            return None;
        }
        let g = job.src.gpu.as_ref()?;
        let footage = job.footage();
        let files = ReadLog::new();
        let mut r = Renderer::new(&job.src.project, &footage, job.opts);
        r.expr = job.src.expr.as_deref();
        r.cache = Some(&job.src.layer_cache);
        r.reads = Some(&files);
        r.accel = Some(g as &dyn effectcraft_engine::render::Accelerator);
        r.active_accel()?;
        let f = {
            let _slot = GPU_RENDER_SLOTS.try_acquire(MAX_GPU_RENDERS)?;
            g.within_memory(|| g.render_display(&r, job.comp, job.t))
        };
        effectcraft_engine::render::Accelerator::frame_end(g);
        match f {
            Ok(f) => {
                if f.is_some() {
                    self.gpu_succeeded();
                }
                f.map(|f| (FrameImage::Gpu(Arc::new(f)), files.into_files()))
            }
            Err(e) => {
                self.gpu_attempt_failed(g, &e);
                None
            }
        }
    }

    /// Render a frame; `gpu: false` keeps it off the GPU compositor. The source files it reads
    /// are noted in `files` (a GPU attempt that falls back to the CPU notes both renders').
    fn render_job(&self, job: &Job, gpu: bool, files: &ReadLog) -> FrameImage {
        let footage = job.footage();
        let mut r = Renderer::new(&job.src.project, &footage, job.opts);
        r.expr = job.src.expr.as_deref();
        r.reads = Some(files);
        let gpu_allowed = gpu && self.gpu_ready();
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
            // Every exit that doesn't record ends the frame unmeasured: the ticket is dropped
            // before a retry (`take`), or on return.
            let mut ticket = if cfg!(target_arch = "wasm32") { None } else { r.frame_auto(a, job.comp, true).map(|(p, key)| p.ticket(key)) };
            if ticket.as_ref().is_none_or(AutoTicket::gpu) {
                let t0 = web_time::Instant::now();
                match gpu_attempt(g, || g.render_display(&r, job.comp, job.t)) {
                    Ok(Some(f)) => {
                        self.gpu_succeeded();
                        if let Some(t) = ticket.take() {
                            // Time the GPU's work, not just its recording.
                            g.wait();
                            if g.context().check_health().is_err() {
                                drop(t);
                                return self.render_job(job, false, files);
                            }
                            t.record(t0.elapsed().as_secs_f64() * 1000.0);
                        }
                        return FrameImage::Gpu(Arc::new(f));
                    }
                    Ok(None) => {
                        if let Some(t) = &mut ticket {
                            t.declined();
                        }
                    }
                    Err(e) => {
                        drop(ticket.take());
                        self.gpu_attempt_rejected(g, &e);
                        return self.render_job(job, false, files);
                    }
                }
            }
            let t0 = web_time::Instant::now();
            // CPU compositing in Auto can still run GPU layer effects. Keep those inside the
            // same admission and error scope as a GPU-composited frame (#3028).
            let img = match gpu_attempt(g, || r.comp_frame_cpu(job.comp, job.t)) {
                Ok(img) => cpu_frame(&img, &job.src.project, &job.opts),
                Err(e) => {
                    drop(ticket.take());
                    self.gpu_attempt_rejected(g, &e);
                    return self.render_job(job, false, files);
                }
            };
            if let Some(t) = ticket {
                t.record(t0.elapsed().as_secs_f64() * 1000.0);
            }
            return img;
        }
        if let Some(g) = gpu {
            // The GPU composites and the frame is read back.
            return match r.comp_frame_scoped(job.comp, job.t, |a| gpu_attempt(g, || a.comp_frame(&r, job.comp, job.t))) {
                Ok(img) => {
                    self.gpu_succeeded();
                    cpu_frame(&img, &job.src.project, &job.opts)
                }
                Err(e) => {
                    self.gpu_attempt_rejected(g, &e);
                    self.render_job(job, false, files)
                }
            };
        }
        cpu_frame(&r.comp_frame(job.comp, job.t), &job.src.project, &job.opts)
    }
    /// Busy shared-pool workers retry on the CPU without treating contention as a GPU fault.
    fn gpu_attempt_rejected(&self, gpu: &Gpu, error: &GpuAttemptError) {
        if let GpuAttemptError::Failed(error) = error {
            self.gpu_attempt_failed(gpu, error);
        }
    }
}

enum GpuAttemptError {
    Busy,
    Failed(FrameError),
}

/// Bound all preview GPU attempts, including effects in an Auto CPU composite. The permit
/// ends before the caller retries on the CPU, so failed frames cannot block GPU admissions.
fn gpu_attempt<T>(gpu: &Gpu, render: impl FnOnce() -> T) -> Result<T, GpuAttemptError> {
    let _slot = GPU_RENDER_SLOTS.for_render(MAX_GPU_RENDERS).ok_or(GpuAttemptError::Busy)?;
    gpu.within_memory(render).map_err(GpuAttemptError::Failed)
}

/// Who asked for a frame (see [`Frames::request_with`]).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Want {
    Viewer,
    Prefetch,
    Background,
}

struct Job {
    key: FrameKey,
    src: RenderSource,
    comp: ItemId,
    t: Tick,
    opts: RenderOpts,
    urgent: bool,
    /// Nobody waits for it (see [`Frames::request_background`]).
    background: bool,
    seq: u64,
    /// Set when the viewer no longer waits for this frame (see [`Frames::request_urgent`]).
    cancel: Arc<AtomicBool>,
}

impl Job {
    /// The footage this job's render reads, called off with the job.
    fn footage(&self) -> CancellableFootage<'_> {
        CancellableFootage { inner: self.src.footage.as_ref(), cancel: self.cancel.clone() }
    }
}

#[derive(Default)]
struct Queue {
    jobs: Vec<Job>,
    running: HashSet<FrameKey>,
    urgent_running: usize,
    /// The running viewer (urgent) frames and how to call them off.
    urgent_cancels: Vec<(FrameKey, Arc<AtomicBool>)>,
    seq: u64,
    /// The playing preview, if any: its prefetch renders nearest first.
    playhead: Option<Playhead>,
    /// How many lanes there are, and the ones not rendering. A lane is one thread that renders
    /// one frame at a time. The frames of a playing preview and the frames nobody waits for
    /// (Cache Frames When Idle) render on the lanes; the pool keeps the viewer's frame and the
    /// few frames prefetched around it while paused. On the pool a frame's parallel work is
    /// shared: a thread that took a piece of one frame and waited inside it started rendering
    /// the queued frames on top, and went back to its piece only once the queue was empty. The
    /// frames a preview started first, the ones right ahead of the playhead, were done last:
    /// the preview stood still on a gap while the frames further ahead turned green. Idle
    /// caching left the same gaps, and held up the viewer's frame when it shared a piece.
    lanes: usize,
    idle_lanes: Vec<usize>,
    /// Lanes compositing a frame on the GPU, at most [`Queue::gpu_limit`]. The GPU is the
    /// viewer's device: each lane records, submits and reads back a whole frame on it, and the
    /// window can't draw until that work is done. With a lane per core on a GPU that can't keep
    /// up, the window froze for hundreds of milliseconds at a time while a preview played, and
    /// the preview jumped ahead when it came back (#3345). The other lanes go on with the frames
    /// that don't need the GPU (the disk cache's, or all of them while the GPU is out of memory
    /// or retired); the pool and the viewer's frame are not counted.
    gpu_lanes: usize,
    /// How many lanes may composite on the GPU at a time (at least 1, see [`Queue::ui_frame`]).
    gpu_limit: usize,
    /// The limit that last stalled the window: tried again only after [`GPU_LANE_RETRY`].
    gpu_stalled_at: usize,
    /// While a preview plays: the window's last frame, and since when it has been smooth.
    ui_last: Option<web_time::Instant>,
    ui_smooth_since: Option<web_time::Instant>,
    /// Tests: as if there were a GPU compositor (no adapter is there).
    #[cfg(test)]
    fake_gpu: bool,
}

impl Queue {
    /// Whether `j` renders on a lane (else on the pool).
    fn on_lane(&self, j: &Job) -> bool {
        self.lanes > 0 && !j.urgent && (j.background || self.playhead.is_some())
    }

    /// The job a lane (`lane`) or the pool renders next: the [`Queue::best`] of its own. `gpu`:
    /// the GPU composites (a lane's frames), so a lane takes no GPU frame while
    /// [`Queue::gpu_limit`] lanes are compositing.
    fn next(&self, lane: bool, gpu: bool) -> Option<usize> {
        let gpu_full = gpu && self.gpu_lanes >= self.gpu_limit.max(1);
        self.best_of(|j| self.on_lane(j) == lane && !(gpu_full && self.gpu_job(j)))
    }

    /// The window drew a frame at `now` (`None`: no preview plays). A window that stalls with
    /// lanes compositing gets fewer of them: the GPU can't keep up with them and the window. One
    /// that stays smooth while every allowed lane composites gets one more, up to all of them: a
    /// GPU that keeps up renders the preview on every lane. A limit that stalled is tried again
    /// only after a longer smooth spell, so the window doesn't stall over and over. `true` when
    /// one more lane may composite.
    fn ui_frame(&mut self, now: Option<web_time::Instant>) -> bool {
        let Some(now) = now else {
            self.ui_last = None;
            return false;
        };
        let limit = self.gpu_limit.max(1);
        let gap = self.ui_last.map(|last| now.saturating_duration_since(last));
        self.ui_last = Some(now);
        let Some(gap) = gap else {
            self.ui_smooth_since = Some(now);
            return false;
        };
        if gap > UI_STALL && self.gpu_lanes > 0 {
            self.gpu_stalled_at = limit;
            self.gpu_limit = (limit / 2).max(1);
            self.ui_smooth_since = Some(now);
        } else if gap > UI_SMOOTH {
            self.ui_smooth_since = Some(now);
        } else {
            let since = *self.ui_smooth_since.get_or_insert(now);
            let wait = if limit + 1 >= self.gpu_stalled_at && self.gpu_stalled_at > 0 { GPU_LANE_RETRY } else { GPU_LANE_STEP };
            if self.gpu_lanes >= limit && limit < self.lanes && now.saturating_duration_since(since) >= wait {
                self.gpu_limit = limit + 1;
                self.ui_smooth_since = Some(now);
                return true;
            }
        }
        false
    }

    /// Whether `j` is composited on the GPU when the GPU is ready: there is a GPU compositor and
    /// the frame's renderer uses it (as `Renderer::active_accel` decides: Software Only keeps
    /// every frame on the CPU).
    fn gpu_job(&self, j: &Job) -> bool {
        let adapter = j.src.gpu.is_some();
        #[cfg(test)]
        let adapter = adapter || self.fake_gpu;
        adapter
            && match j.opts.backend {
                effectcraft_engine::render::Backend::Cpu => false,
                effectcraft_engine::render::Backend::Gpu => true,
                effectcraft_engine::render::Backend::Auto => j.src.project.settings.gpu_acceleration,
            }
    }

    /// The job that matters most: urgent first (newest), then, while a preview plays, the
    /// prefetch frame it reaches first, then prefetch in request order. In request order alone,
    /// frames asked for again after an eviction (the ones right ahead of the playhead) waited
    /// behind every frame further ahead (#595).
    fn best(&self) -> Option<usize> {
        self.best_of(|_| true)
    }

    /// [`Queue::best`] among the jobs `of` accepts.
    fn best_of(&self, of: impl Fn(&Job) -> bool) -> Option<usize> {
        let p = self.playhead.as_ref();
        self.jobs
            .iter()
            .enumerate()
            .filter(|(_, j)| of(j))
            .max_by_key(|(_, j)| {
                let near = if j.urgent { None } else { p.and_then(|p| p.ahead(&j.key)) };
                (j.urgent, near.is_some(), near.map_or(0, |d| -d), if j.urgent { j.seq as i64 } else { -(j.seq as i64) })
            })
            .map(|(i, _)| i)
    }
}

/// The render scale a viewer frame's key stands for: it keeps thousandths, but the viewer
/// renders every n-th pixel, and 0.333 is not 1/3 (2 pixels off by row 1000 at Third). The
/// comp's own factor decides when two factors share the thousandths (from 1/34).
pub(crate) fn render_scale(thousandths: u32, factor: Option<u8>) -> f64 {
    let n = factor.map(f64::from).filter(|n| (1000.0 / n).round() == f64::from(thousandths));
    1.0 / n.unwrap_or_else(|| (1000.0 / f64::from(thousandths.max(1))).round()).max(1.0)
}

#[cfg(test)]
pub(crate) mod tests;
