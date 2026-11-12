//! Layer render cache.
//!
//! The processed pixels of a layer (source → masks → effects, in layer space) depend only on the
//! layer's non-transform properties, its source item, selected-layer dependencies and the render
//! options. Its own transform is normally applied later; referenced layers' transforms can
//! affect their sampled pixels. [`LayerCache`] keeps those buffers keyed
//! by a content hash of exactly those inputs, *evaluated at the frame time*, so:
//!
//! * a static layer (or one whose only animation is in Transform) renders once and is reused on
//!   every frame while scrubbing or playing;
//! * editing one layer re-renders only that layer — every other layer's cached pixels stay valid
//!   because their keys do not change;
//! * any change to a property value, keyframe, expression result, effect, mask, switch or source
//!   item changes the key, so stale pixels are never returned.
//!
//! Layers whose pixels depend on time in ways the property values do not capture (time-based
//! effects such as Noise or Wave Warp, Wiggle Paths, footage, precomps) either fold the layer
//! time into the key or are not cached.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};

use effectcraft_effects::Buf;
use effectcraft_keyframe::Value;
use effectcraft_project::{FootageKind, ItemKind, Layer, LayerSource, Node, PropGroup};

use crate::disk_cache::{EntryFiles, SharedVirtualFileId, VirtualFileId, layer_disk_key};
use crate::eval::EvalCtx;
use crate::reads::ReadFiles;

mod dependencies;

/// Thread-safe, memory-budgeted cache of processed layer buffers, optionally backed by the
/// persistent [`DiskCache`](crate::disk_cache::DiskCache): a memory miss reads the disk, and
/// buffers that were slow to render are written there with the source files they were made of
/// (keys: [`crate::disk_cache::layer_disk_key`]; an entry is used only while its files are
/// unchanged, #3184).
pub struct LayerCache {
    inner: Mutex<Inner>,
    disk: Mutex<Option<Arc<crate::disk_cache::DiskCache>>>,
    /// Another persistent store when there is no disk cache (the browser's, [`PrefetchStore`]).
    store: Mutex<Option<Arc<dyn LayerStore>>>,
    /// Size and content hash of files served from memory (the browser file table), to store
    /// and check the source files of persistent entries.
    virtual_id: Mutex<Option<SharedVirtualFileId>>,
    /// While set, inserts are dropped (see [`LayerCache::set_gate`]).
    gate: Mutex<Option<Arc<std::sync::atomic::AtomicBool>>>,
}

struct Entry {
    buf: Arc<Buf>,
    /// The source files the buffer was made of (see [`crate::reads`]).
    files: Arc<ReadFiles>,
    bytes: usize,
    last_use: u64,
}

struct Inner {
    map: HashMap<u64, Entry>,
    bytes: usize,
    budget: usize,
    clock: u64,
    hits: u64,
    misses: u64,
    /// When each recent miss happened, to time the render that follows it (the drop generation
    /// a render's reads started at travels with its files instead, [`ReadFiles::since`]: two
    /// renders of a key may overlap).
    missed_at: HashMap<u64, web_time::Instant>,
}

/// Hit/miss counters and memory use (for perf readouts and tests).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CacheStats {
    pub entries: usize,
    pub bytes: usize,
    pub hits: u64,
    pub misses: u64,
}

impl Default for LayerCache {
    fn default() -> Self {
        LayerCache::new(1 << 30)
    }
}

impl LayerCache {
    /// A cache holding at most `budget` bytes of pixels.
    pub fn new(budget: usize) -> LayerCache {
        LayerCache {
            inner: Mutex::new(Inner { map: HashMap::new(), bytes: 0, budget, clock: 0, hits: 0, misses: 0, missed_at: HashMap::new() }),
            disk: Mutex::new(None),
            store: Mutex::new(None),
            virtual_id: Mutex::new(None),
            gate: Mutex::new(None),
        }
    }

    /// Drop inserts while `gate` is set. A render whose GPU readbacks are still in flight (the
    /// browser's deferred readbacks, see [`crate::Accelerator::pass_missed`]) computes some
    /// buffers from placeholders; the accelerator raises the gate from its first missing
    /// readback to the end of the pass, so none of them is kept (in memory or on disk).
    pub fn set_gate(&self, gate: Option<Arc<std::sync::atomic::AtomicBool>>) {
        if let Ok(mut g) = self.gate.lock() {
            *g = gate;
        }
    }

    fn gated(&self) -> bool {
        self.gate.lock().ok().and_then(|g| g.as_ref().map(|g| g.load(std::sync::atomic::Ordering::Relaxed))).unwrap_or(false)
    }

    /// Attach (or detach) the disk cache. It is used at once: its entries check their own
    /// source files when read, so nothing has to be looked at first.
    pub fn set_disk(&self, disk: Option<Arc<crate::disk_cache::DiskCache>>) {
        if let Ok(mut d) = self.disk.lock() {
            *d = disk;
        }
    }

    /// The attached disk cache, if any.
    pub fn disk(&self) -> Option<Arc<crate::disk_cache::DiskCache>> {
        self.disk.lock().ok()?.clone()
    }

    /// Attach (or detach) a [`LayerStore`] used when no disk cache is attached (the browser's
    /// Origin Private File System through a [`PrefetchStore`]).
    pub fn set_store(&self, store: Option<Arc<dyn LayerStore>>) {
        if let Ok(mut s) = self.store.lock() {
            *s = store;
        }
    }

    /// Size and content hash of files served from memory (the browser file table): what a
    /// persistent entry keeps for them and checks them against.
    pub fn set_virtual_id(&self, id: Option<SharedVirtualFileId>) {
        if let Ok(mut v) = self.virtual_id.lock() {
            *v = id;
        }
    }

    /// The attached virtual-file identity lookup, if any.
    fn virtual_id(&self) -> Option<SharedVirtualFileId> {
        self.virtual_id.lock().ok()?.clone()
    }

    /// The persistent store behind the memory: the disk cache, else the other store.
    fn persistent(&self) -> Option<Arc<dyn LayerStore>> {
        if let Some(dc) = self.disk.lock().ok()?.as_ref() {
            return Some(dc.clone() as Arc<dyn LayerStore>);
        }
        self.store.lock().ok()?.clone()
    }

    /// The buffer kept under `key` and the source files it was made of (see [`crate::reads`]).
    /// A buffer from the disk cache or another store comes with the files its entry lists, after
    /// they were found unchanged. A buffer made of a file dropped since its reads began (File ▸
    /// Reload Footage) is never served: it is forgotten, and the lookup is a miss.
    pub fn get(&self, key: u64) -> Option<(Arc<Buf>, Arc<ReadFiles>)> {
        match self.kept(key, true) {
            Some(found) => Some(found),
            None => {
                let mut g = self.inner.lock().ok()?;
                g.misses += 1;
                drop(g);
                let store = self.persistent();
                // (before the entry is read: what it is made of may be dropped meanwhile)
                let since = store.as_ref().map_or(0, |s| s.drop_generation());
                if let Ok(mut g) = self.inner.lock() {
                    if g.missed_at.len() > 4096 {
                        g.missed_at.clear();
                    }
                    g.missed_at.insert(key, web_time::Instant::now());
                }
                // Disk cache: a hit comes back into memory, read at generation `since`.
                let store = store?;
                let id = self.virtual_id();
                let (buf, mut files) = store.get_layer(layer_disk_key(key), id.as_deref())?;
                files.note_since(since);
                let files = Arc::new(files);
                Some(self.insert_mem(key, buf, files))
            }
        }
    }

    /// Keep `buf` under `key`, and return the buffer the cache holds for it: the one kept first
    /// when another render of the same layer got there before (#1812: preview workers starting
    /// together each rendered every layer; sharing the first buffer, the GPU compositor uploads
    /// and packs it once instead of once per worker, which ran it out of memory). Renders don't
    /// wait for each other: on the frame workers' shared thread pool a wait can end up under
    /// the very work it waits for. `files`: the source files `buf` was made of (see
    /// [`crate::reads`]); a buffer whose files can't all be named stays in memory. The files
    /// returned are the ones the returned buffer was made of.
    ///
    /// The drop generation `buf`'s reads started at comes with `files` ([`ReadFiles::since`],
    /// from [`LayerCache::drop_generation`] when the render began): a buffer made of a file
    /// dropped since (File ▸ Reload Footage while it rendered) is kept neither on disk nor in
    /// memory. A list that doesn't say counts as made before every drop. A buffer made of values
    /// its key doesn't capture ([`ReadFiles::is_unkeyed`]) stays in memory.
    pub fn insert(&self, key: u64, buf: Arc<Buf>, files: Arc<ReadFiles>) -> (Arc<Buf>, Arc<ReadFiles>) {
        if self.gated() {
            return (buf, files);
        }
        if let Some(kept) = self.kept(key, false) {
            return kept;
        }
        let store = self.persistent();
        let since = files.since().unwrap_or(0);
        // (before the miss's time is taken: it may time another render of the key)
        if store.as_ref().is_some_and(|s| s.dropped_since(&files, since)) {
            return (buf, files);
        }
        let started = self.inner.lock().ok().and_then(|mut g| g.missed_at.remove(&key));
        if let Some(store) = store {
            let id = self.virtual_id();
            // (an incomplete list, or a file from memory without its identity: not stored)
            if let Some(t) = started
                && !files.is_unkeyed()
                && t.elapsed().as_secs_f64() * 1e3 >= store.min_layer_ms() as f64
                && let Some(entry) = EntryFiles::of(&files, id.as_deref())
            {
                store.put_layer(layer_disk_key(key), &buf, &entry, since);
            }
        }
        self.insert_mem(key, buf, files)
    }

    /// The buffer kept in memory under `key` with its files, if it is not made of a file dropped
    /// since its reads began (one that is, is forgotten). `hit`: count it as a hit and as just
    /// used. (A render that started before a drop may publish its buffer after the drop's
    /// purge, Reload Footage clearing the cache while the buffer was being written: the check
    /// here, not the order of the two, keeps it from being served.)
    /// Whether a buffer is kept in memory under `key` (a peek: no hit, no miss, no disk read).
    pub(crate) fn holds(&self, key: u64) -> bool {
        self.kept(key, false).is_some()
    }

    fn kept(&self, key: u64, hit: bool) -> Option<(Arc<Buf>, Arc<ReadFiles>)> {
        let found = {
            let mut g = self.inner.lock().ok()?;
            g.clock += 1;
            let now = g.clock;
            let e = g.map.get_mut(&key)?;
            if hit {
                e.last_use = now;
            }
            (e.buf.clone(), e.files.clone())
        };
        if self.made_before_a_drop(&found.1) {
            if let Ok(mut g) = self.inner.lock()
                && g.map.get(&key).is_some_and(|e| Arc::ptr_eq(&e.files, &found.1))
                && let Some(e) = g.map.remove(&key)
            {
                g.bytes = g.bytes.saturating_sub(e.bytes);
            }
            return None;
        }
        if hit && let Ok(mut g) = self.inner.lock() {
            g.hits += 1;
        }
        Some(found)
    }

    /// Whether a buffer made of `files` holds a file as it was before a drop that came after its
    /// reads began ([`ReadFiles::since`]; a list that doesn't say counts as made before every
    /// drop).
    fn made_before_a_drop(&self, files: &ReadFiles) -> bool {
        self.persistent().is_some_and(|s| s.dropped_since(files, files.since().unwrap_or(0)))
    }

    /// The persistent store's drop generation now ([`LayerStore::drop_generation`]), taken by a
    /// render before it starts reading (`None` without a store).
    pub fn drop_generation(&self) -> Option<u64> {
        self.persistent().map(|s| s.drop_generation())
    }

    /// See [`LayerCache::insert`]: the buffer kept under `key` and its files (`buf` and `files`,
    /// unless one was kept first or `buf` is too large to keep).
    fn insert_mem(&self, key: u64, buf: Arc<Buf>, files: Arc<ReadFiles>) -> (Arc<Buf>, Arc<ReadFiles>) {
        let bytes = buf.img.data.len() * std::mem::size_of::<effectcraft_raster::Px>();
        let Ok(mut g) = self.inner.lock() else { return (buf, files) };
        if bytes > g.budget / 4 {
            return (buf, files);
        }
        g.clock += 1;
        let now = g.clock;
        if let Some(e) = g.map.get_mut(&key) {
            // (one made before a drop gives way: see [`LayerCache::kept`])
            if !self.made_before_a_drop(&e.files) {
                e.last_use = now;
                return (e.buf.clone(), e.files.clone());
            }
            if let Some(old) = g.map.remove(&key) {
                g.bytes = g.bytes.saturating_sub(old.bytes);
            }
        }
        // (checked again just before it is published: a drop may have come since `insert`
        // checked; one that comes later is caught when the buffer is looked up)
        if self.made_before_a_drop(&files) {
            return (buf, files);
        }
        g.map.insert(key, Entry { buf: buf.clone(), files: files.clone(), bytes, last_use: now });
        g.bytes += bytes;
        if g.bytes > g.budget {
            // Evict least-recently used entries down to 3/4 of the budget.
            let target = g.budget / 4 * 3;
            let mut by_age: Vec<(u64, u64)> = g.map.iter().map(|(k, e)| (e.last_use, *k)).collect();
            by_age.sort_unstable();
            for (_, k) in by_age {
                if g.bytes <= target {
                    break;
                }
                if let Some(e) = g.map.remove(&k) {
                    g.bytes -= e.bytes;
                }
            }
        }
        (buf, files)
    }

    /// The memory budget in bytes.
    pub fn budget(&self) -> usize {
        self.inner.lock().map(|g| g.budget).unwrap_or(0)
    }

    /// Change the memory budget (evicts least-recently used entries when shrinking).
    pub fn set_budget(&self, budget: usize) {
        let Ok(mut g) = self.inner.lock() else { return };
        g.budget = budget;
        if g.bytes > budget {
            let mut by_age: Vec<(u64, u64)> = g.map.iter().map(|(k, e)| (e.last_use, *k)).collect();
            by_age.sort_unstable();
            for (_, k) in by_age {
                if g.bytes <= budget {
                    break;
                }
                if let Some(e) = g.map.remove(&k) {
                    g.bytes -= e.bytes;
                }
            }
        }
    }

    pub fn clear(&self) {
        if let Ok(mut g) = self.inner.lock() {
            g.map.clear();
            g.bytes = 0;
        }
    }

    pub fn stats(&self) -> CacheStats {
        self.inner.lock().map(|g| CacheStats { entries: g.map.len(), bytes: g.bytes, hits: g.hits, misses: g.misses }).unwrap_or_default()
    }
}

/// Persistent storage of layer buffers behind a [`LayerCache`] (keys:
/// [`crate::disk_cache::layer_disk_key`]). Every entry keeps the source files its buffer was made
/// of and is used only while they are unchanged (#3184).
pub trait LayerStore: Send + Sync {
    /// A stored buffer whose source files are unchanged, with those files (synchronous:
    /// mid-render). An entry whose files changed, or damaged, is a miss and is dropped, so the
    /// buffer rendered again replaces it. `virtual_id` names files served from memory.
    fn get_layer(&self, key: u128, virtual_id: Option<VirtualFileId<'_>>) -> Option<(Arc<Buf>, ReadFiles)>;
    /// Keep a buffer that was slow to render, with the source files it was made of (shared, so
    /// a store can encode it off the render thread). `since`: the
    /// [`LayerStore::drop_generation`] when its render started (a buffer made of a file dropped
    /// after that is refused).
    fn put_layer(&self, key: u128, buf: &Arc<Buf>, files: &EntryFiles, since: u64);
    /// How many times files were dropped from the store (File ▸ Reload Footage,
    /// [`crate::disk_cache::DiskCache::drop_files`]); `0` for a store that never drops.
    fn drop_generation(&self) -> u64 {
        0
    }
    /// Whether one of `files` was dropped after generation `since`: what was made of them is
    /// from before the drop. `false` for a store that never drops.
    fn dropped_since(&self, _files: &ReadFiles, _since: u64) -> bool {
        false
    }
    /// Buffers faster to render than this (ms) are not stored.
    fn min_layer_ms(&self) -> u64 {
        0
    }
}

/// Layers buffers rendered slower than this go to the browser's disk cache (ms).
pub const PREFETCH_MIN_LAYER_MS: u64 = 20;

/// A [`LayerStore`] over storage that can't be read in the middle of a render (the browser's
/// Origin Private File System, read asynchronously). Lookups are served from buffers fetched
/// before the frame ([`PrefetchStore::provide`]), once the files their entries list are found
/// unchanged; the keys a render looked up and didn't find are recorded
/// ([`PrefetchStore::take_misses`]) so the transport can fetch them before the next frame;
/// buffers to store queue up as sealed entries ([`PrefetchStore::take_writes`]) for the
/// transport to write after the frame. Fetched buffers are kept least recently used first, up
/// to a byte budget.
pub struct PrefetchStore {
    st: Mutex<Prefetched>,
    budget: usize,
    min_ms: u64,
}

/// A fetched entry: the buffer, the source files it was made of, its last use.
struct Fetched {
    buf: Arc<Buf>,
    files: Arc<EntryFiles>,
    used: u64,
}

/// The memory a buffer's pixels take, in bytes.
fn buf_bytes(b: &Buf) -> usize {
    b.img.data.len().saturating_mul(std::mem::size_of::<effectcraft_raster::Px>())
}

#[derive(Default)]
struct Prefetched {
    ready: HashMap<u128, Fetched>,
    bytes: usize,
    clock: u64,
    misses: Vec<u128>,
    hits: Vec<u128>,
    writes: Vec<(u128, Vec<u8>)>,
    /// Keys stored or queued (not written twice).
    known: std::collections::HashSet<u128>,
}

impl Prefetched {
    /// Records a missed key for prefetching, up to a fixed bound.
    fn note_miss(&mut self, key: u128) {
        if !self.misses.contains(&key) && self.misses.len() < 1024 {
            self.misses.push(key);
        }
    }

    /// Drops the prefetched entry for `key` and frees its bytes.
    fn remove(&mut self, key: u128) {
        if let Some(f) = self.ready.remove(&key) {
            self.bytes = self.bytes.saturating_sub(buf_bytes(&f.buf));
        }
    }
}

impl Default for PrefetchStore {
    fn default() -> Self {
        PrefetchStore::new(256 << 20)
    }
}

impl PrefetchStore {
    pub fn new(budget: usize) -> PrefetchStore {
        PrefetchStore { st: Mutex::new(Prefetched::default()), budget, min_ms: PREFETCH_MIN_LAYER_MS }
    }

    /// Store buffers that took at least `ms` to render (default [`PREFETCH_MIN_LAYER_MS`]).
    pub fn with_min_ms(mut self, ms: u64) -> PrefetchStore {
        self.min_ms = ms;
        self
    }

    /// Whether `key`'s buffer is here.
    pub fn has(&self, key: u128) -> bool {
        self.st.lock().is_ok_and(|s| s.ready.contains_key(&key))
    }

    /// A fetched entry (sealed bytes, [`crate::disk_cache::layer_entry`]) for `key`; `false`
    /// when damaged or of another format. Its files are checked when the buffer is used.
    pub fn provide(&self, key: u128, entry: &[u8]) -> bool {
        let Some((buf, files)) = crate::disk_cache::read_layer_entry(entry) else { return false };
        let buf = Arc::new(buf);
        let bytes = buf_bytes(&buf);
        let Ok(mut s) = self.st.lock() else { return false };
        s.clock += 1;
        let used = s.clock;
        s.remove(key);
        s.ready.insert(key, Fetched { buf, files: Arc::new(files), used });
        s.bytes = s.bytes.saturating_add(bytes);
        s.known.insert(key);
        while s.bytes > self.budget {
            let Some(oldest) = s.ready.iter().min_by_key(|(_, f)| f.used).map(|(k, _)| *k) else { break };
            s.remove(oldest);
        }
        true
    }

    /// Keys looked up and not found since the last call (each once).
    pub fn take_misses(&self) -> Vec<u128> {
        self.st.lock().map(|mut s| std::mem::take(&mut s.misses)).unwrap_or_default()
    }

    /// Keys served since the last call (each once).
    pub fn take_hits(&self) -> Vec<u128> {
        self.st.lock().map(|mut s| std::mem::take(&mut s.hits)).unwrap_or_default()
    }

    /// Entries to write: (key, sealed bytes).
    pub fn take_writes(&self) -> Vec<(u128, Vec<u8>)> {
        self.st.lock().map(|mut s| std::mem::take(&mut s.writes)).unwrap_or_default()
    }

    /// Forget everything (Empty Disk Cache, Purge).
    pub fn clear(&self) {
        if let Ok(mut s) = self.st.lock() {
            *s = Prefetched::default();
        }
    }
}

impl LayerStore for PrefetchStore {
    /// The prefetched layer for `key` with its read files, or `None` when it is not ready.
    fn get_layer(&self, key: u128, virtual_id: Option<VirtualFileId<'_>>) -> Option<(Arc<Buf>, ReadFiles)> {
        let found = {
            let mut s = self.st.lock().ok()?;
            s.clock += 1;
            let now = s.clock;
            match s.ready.get_mut(&key) {
                Some(f) => {
                    f.used = now;
                    Some((f.buf.clone(), f.files.clone()))
                }
                None => {
                    s.note_miss(key);
                    None
                }
            }
        };
        let (buf, files) = found?;
        // (looked at without the lock: one call of the hook, or one `metadata`, per file)
        let unchanged = files.unchanged(virtual_id);
        let mut s = self.st.lock().ok()?;
        if !unchanged {
            // Made of files that changed since: forgotten, so the buffer rendered again is
            // written over its entry.
            s.remove(key);
            s.known.remove(&key);
            s.note_miss(key);
            return None;
        }
        if !s.hits.contains(&key) {
            s.hits.push(key);
        }
        Some((buf, files.to_read_files()))
    }

    /// Queues the layer for writing to the persistent store the first time `key` is seen.
    fn put_layer(&self, key: u128, buf: &Arc<Buf>, files: &EntryFiles, _since: u64) {
        let Ok(mut s) = self.st.lock() else { return };
        if s.known.insert(key) {
            s.writes.push((key, crate::disk_cache::layer_entry(buf, files)));
        }
    }

    fn min_layer_ms(&self) -> u64 {
        self.min_ms
    }
}

/// FNV-1a style 64-bit hasher fed by `Debug` output (exact float formatting) and raw values.
struct KeyHasher(u64);

impl Hasher for KeyHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 ^= b as u64;
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }
}

impl std::fmt::Write for KeyHasher {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        Hasher::write(self, s.as_bytes());
        Ok(())
    }
}

fn hash_debug(h: &mut KeyHasher, v: &impl std::fmt::Debug) {
    use std::fmt::Write;
    let _ = write!(h, "{v:?}");
    Hasher::write_u8(h, 0xff);
}

/// Time-based layer content: does any part of the source/effects read the clock directly
/// (rather than through property values)?
/// `effects: false` ignores the effects (a key of the source and masks only).
fn time_dependent(layer: &Layer, effects: bool) -> bool {
    if effects
        && layer.switches.effects
        && let Some(fx) = layer.effects()
    {
        for g in fx.groups() {
            if let effectcraft_project::GroupKind::Effect { effect } = &g.kind
                && g.enabled
                && effectcraft_effects::is_time_dependent(effect)
            {
                return true;
            }
            // Clone strokes reading another layer or another time.
            if g.enabled && effectcraft_effects::paint::is_paint(g) && effectcraft_effects::paint::cache_key(g, 0.0).1 {
                return true;
            }
        }
    }
    if matches!(layer.source, LayerSource::Shape)
        && let Some(c) = layer.props.sub("contents")
    {
        return group_has(c, "wiggle") || group_has(c, "wiggler");
    }
    // Wiggly selectors move with time; Expression selectors evaluate per character (their
    // property value alone doesn't capture the result).
    if matches!(layer.source, LayerSource::Text)
        && let Some(t) = layer.props.sub("text")
    {
        return group_has(t, "wigglySelector") || group_has(t, "expressionSelector");
    }
    false
}

/// A text layer with an Expression Selector: its amount is evaluated per character, and the
/// layer's key holds only the first character's (see [`crate::reads::ReadFiles::is_unkeyed`]).
pub(crate) fn per_character_expressions(layer: &Layer) -> bool {
    matches!(layer.source, LayerSource::Text) && layer.props.sub("text").is_some_and(|t| group_has(t, "expressionSelector"))
}

fn group_has(g: &PropGroup, match_id: &str) -> bool {
    g.children.iter().any(|c| match c {
        Node::Group(sub) => sub.match_id == match_id || group_has(sub, match_id),
        Node::Prop(_) => false,
    })
}

/// Feed every property of `g` that can change over time (keyframes or expression) as its value
/// at the context time. The static structure is hashed separately.
/// Mix the collapsed-instance overrides that target `id` into a cache key. They replace values
/// without editing the layer, so a key without them would reuse another instance's buffer.
fn hash_overrides(h: &mut KeyHasher, ctx: &EvalCtx, id: effectcraft_project::LayerId) {
    for o in ctx.property_overrides.iter().filter(|o| o.layer == id) {
        o.prop.hash(h);
        o.font.hash(h);
        hash_debug(h, &o.value);
    }
    for m in ctx.media_overrides.iter().filter(|m| m.layer == id) {
        m.item.hash(h);
    }
}

fn hash_values(h: &mut KeyHasher, ctx: &EvalCtx, layer: &Layer, g: &PropGroup) {
    for c in &g.children {
        match c {
            Node::Prop(p) => {
                if !p.keys.is_empty() || p.has_expression() {
                    p.uid.hash(h);
                    hash_debug(h, &ctx.value(layer, p));
                }
            }
            Node::Group(sub) => hash_values(h, ctx, layer, sub),
        }
    }
}

/// Cache key for the processed (source → masks → effects) buffer of `layer` at the context
/// time, or `None` when the layer must not be cached (movie footage, precomps, cameras, adjustment
/// layers…).
pub fn layer_key(ctx: &EvalCtx, layer: &Layer, scale: f64, draft: bool, blur: bool) -> Option<u64> {
    // Time effects (and CC Particle World's Birth Rate history) see property values at other
    // times. Keyframes are part of the hashed structure (and the layer time is folded in), but
    // expressions may read other layers at those times, which the key cannot see: such layers
    // are not cached.
    if history_unkeyable(layer, None) {
        return None;
    }
    key_with(ctx, layer, scale, draft, blur, false)
}

/// Cache key for a layer's *input* at the context time: source → masks → its first `effects`
/// effects (see `Renderer::layer_input`). Unlike [`layer_key`], footage layers are cached here
/// (keyed by item and source time), since Time effects read many neighbouring frames.
pub fn input_key(ctx: &EvalCtx, layer: &Layer, scale: f64, draft: bool, blur: bool, effects: usize) -> Option<u64> {
    if layer.switches.adjustment || history_unkeyable(layer, Some(effects)) {
        return None;
    }
    let base = key_any(ctx, layer, scale, draft, blur, true, EffectScope::First(effects))?;
    let mut h = KeyHasher(base ^ 0x5bd1_e995_7a3c_11d3);
    effects.hash(&mut h);
    Some(h.finish())
}

/// Does the layer run (among its first `effects` effects, all when `None`) an effect that reads
/// the layer at other times (Echo, Timewarp…) or its own parameters' history (CC Particle
/// World's Birth Rate, #1373)?
fn reads_other_times(layer: &Layer, effects: Option<usize>) -> bool {
    layer.switches.effects
        && layer.effects().is_some_and(|fx| {
            fx.groups().take(effects.unwrap_or(usize::MAX)).any(|g| {
                g.enabled
                    && matches!(&g.kind, effectcraft_project::GroupKind::Effect { effect }
                        if effect.starts_with("ec.time.")
                            || effectcraft_effects::reads_param_history(effect)
                            || (!effect.starts_with("ec.") && effectcraft_effects::plugin::reads_other_times(effect)))
            })
        })
}

/// The layer reads values at other times (see [`reads_other_times`]) and has expressions
/// (outside Transform), whose results at those times the key can't see.
fn history_unkeyable(layer: &Layer, effects: Option<usize>) -> bool {
    reads_other_times(layer, effects) && layer.props.children.iter().any(|c| matches!(c, Node::Group(g) if g.match_id != "transform" && has_expression(g)))
}

/// A still image (no proxy): its layer is cached like a text or shape layer, so the GPU
/// compositor uploads it once, not every frame (#1812: four 21120 px plates re-uploaded by every
/// preview worker ran the GPU out of memory). Movies and sequences stay in the media pool only.
fn still_footage(ctx: &EvalCtx, item: effectcraft_project::ItemId) -> bool {
    ctx.project.item(item).is_some_and(|it| it.proxy.is_none() && matches!(&it.kind, ItemKind::Footage(f) if f.kind == FootageKind::Still))
}

/// Does the layer have an animated mask that motion blur may draw at other times (Layer ▸ Mask ▸
/// Motion Blur On, or Same As Layer with the layer blurred)?
fn mask_motion_blur(ctx: &EvalCtx, layer: &Layer, blur: bool) -> bool {
    use effectcraft_project::{GroupKind, MaskMotionBlur};
    fn animated(g: &PropGroup) -> bool {
        g.children.iter().any(|c| match c {
            Node::Prop(p) => !p.keys.is_empty() || p.has_expression(),
            Node::Group(sub) => animated(sub),
        })
    }
    ctx.comp.enable_motion_blur
        && layer.masks().is_some_and(|m| {
            m.groups().any(|g| match g.kind {
                GroupKind::Mask { motion_blur, .. } => {
                    (motion_blur == MaskMotionBlur::On || (motion_blur == MaskMotionBlur::SameAsLayer && blur)) && animated(g)
                }
                _ => false,
            })
        })
}

fn has_expression(g: &PropGroup) -> bool {
    g.children.iter().any(|c| match c {
        Node::Prop(p) => p.has_expression(),
        Node::Group(sub) => has_expression(sub),
    })
}

/// How far the effect stack reaches into a cache key. [`EffectScope::Footprint`] keys an
/// adjustment layer's footprint — its source through its masks, without any effects — so the
/// stack must not reach the key at all (#687).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum EffectScope {
    /// The whole stack (the processed layer).
    All,
    /// Only the first `n` effects (a layer's input, see [`input_key`]).
    First(usize),
    /// None of it (an adjustment layer's footprint).
    Footprint,
}

/// Cache key for an adjustment layer's footprint (its source through its masks, see
/// `Renderer::adjustment_footprint`): the GPU compositor then uploads it once, not every frame.
pub fn footprint_key(ctx: &EvalCtx, layer: &Layer, scale: f64, draft: bool, blur: bool) -> Option<u64> {
    // The footprint is the source through the masks, without the effects: a time-dependent
    // effect (Noise on a finishing adjustment layer) must not re-key it every frame (on the GPU,
    // a full-frame re-upload per frame). Shutter, paint, the comp scene and effect dependencies
    // stay out of this key too; a mask still does.
    Some(derive(key_any(ctx, layer, scale, draft, blur, false, EffectScope::Footprint)?, 0xf007_9417))
}

fn key_with(ctx: &EvalCtx, layer: &Layer, scale: f64, draft: bool, blur: bool, footage: bool) -> Option<u64> {
    if layer.switches.adjustment {
        return None;
    }
    key_any(ctx, layer, scale, draft, blur, footage, EffectScope::All)
}

/// `blur`: the layer is motion blurred in this render (`Renderer::mb_on`). `effects`: how far
/// the effect stack reaches into the key (see [`EffectScope`]).
fn key_any(ctx: &EvalCtx, layer: &Layer, scale: f64, draft: bool, blur: bool, footage: bool, effects: EffectScope) -> Option<u64> {
    let mut h = KeyHasher(0xcbf2_9ce4_8422_2325);
    ctx.comp.frame_rate.as_f64().to_bits().hash(&mut h);
    ctx.comp.enable_frame_blending.hash(&mut h);
    dependencies::hash(&mut h, ctx, layer, effects)?;
    match &layer.source {
        LayerSource::Solid { item } => {
            let it = ctx.project.item(*item)?;
            let ItemKind::Solid(s) = &it.kind else { return None };
            hash_debug(&mut h, s);
        }
        LayerSource::Text | LayerSource::Shape => {}
        LayerSource::Footage { item } if footage || still_footage(ctx, ctx.media_item(layer, *item)) => {
            let item = ctx.media_item(layer, *item);
            let it = ctx.project.item(item)?;
            let ItemKind::Footage(f) = &it.kind else { return None };
            hash_debug(&mut h, f);
            // A proxy (and its Use Proxy switch) changes the pixels.
            hash_debug(&mut h, &it.proxy);
            item.hash(&mut h);
            // A still is the same at every time (unless a proxy, which may be a movie, stands in).
            if f.kind != FootageKind::Still || it.proxy.is_some() {
                ctx.source_time(layer).0.hash(&mut h);
            }
            // Frame blending mixes neighbouring source frames.
            ctx.comp.enable_frame_blending.hash(&mut h);
        }
        _ => return None,
    }
    // Bit depth and colour management change the pixels.
    crate::color::Pipe::of(&ctx.project.settings).key().hash(&mut h);
    ctx.comp_id.hash(&mut h);
    ctx.comp.width.hash(&mut h);
    ctx.comp.height.hash(&mut h);
    scale.to_bits().hash(&mut h);
    draft.hash(&mut h);
    layer.id.hash(&mut h);
    // Collapsed-instance overrides replace values without editing the nested layer, so mix them
    // into the key: a Font override, effect parameter or any other source-affecting override
    // must not reuse another instance's processed buffer.
    hash_overrides(&mut h, ctx, layer.id);
    hash_debug(&mut h, &layer.source);
    // (Audio, Lock and Shy don't change pixels.)
    hash_debug(&mut h, &layer.switches.pixels());
    // Mask motion blur and effects that read the shutter depend on whether the layer is motion
    // blurred in this render and on the comp's shutter. A footprint has no effects, so only its
    // masks put the shutter in the key.
    if layer.masks().is_some_and(|m| !m.children.is_empty()) || (effects != EffectScope::Footprint && layer.effects().is_some_and(|fx| !fx.children.is_empty()))
    {
        blur.hash(&mut h);
        ctx.comp.enable_motion_blur.hash(&mut h);
        ctx.comp.shutter_angle.to_bits().hash(&mut h);
        ctx.comp.shutter_phase.to_bits().hash(&mut h);
        ctx.comp.motion_blur_samples.hash(&mut h);
    }
    for c in &layer.props.children {
        // Transform is applied later; Layer Styles are keyed separately (see [`styles_key`]).
        if let Node::Group(g) = c
            && (g.match_id == "transform" || g.match_id == effectcraft_project::styles::GROUP)
        {
            continue;
        }
        // A footprint is its source through its masks: the effect stack is not in it at all.
        // A layer's input runs only its first effects: the later ones (a Puppet whose pins are
        // dragged, #212) don't change it, so it stays cached.
        if let Node::Group(g) = c
            && g.match_id == "effects"
        {
            if effects == EffectScope::Footprint {
                continue;
            }
            if let EffectScope::First(n) = effects {
                let mut first = g.clone();
                let mut seen = 0;
                first.children.retain(|e| match e {
                    Node::Prop(_) => true,
                    Node::Group(_) => {
                        seen += 1;
                        seen <= n
                    }
                });
                hash_debug(&mut h, &first);
                hash_values(&mut h, ctx, layer, &first);
                continue;
            }
        }
        // Static structure: values, keyframes, expressions, enabled flags, effect ids, modes.
        hash_debug(&mut h, c);
        if let Node::Group(g) = c {
            hash_values(&mut h, ctx, layer, g);
        }
    }
    // Paint strokes appear and disappear with their Duration spans (not property values).
    // A footprint does not include them.
    if effects != EffectScope::Footprint
        && layer.switches.effects
        && let Some(fx) = layer.effects()
    {
        let lt = layer.layer_time(ctx.time).seconds();
        for g in fx.groups().filter(|g| g.enabled && effectcraft_effects::paint::is_paint(g)) {
            effectcraft_effects::paint::cache_key(g, lt).0.hash(&mut h);
        }
    }
    // Mask motion blur draws animated masks at the shutter's other times too: their values at
    // this time don't tell two frames apart (a hold before a move).
    if mask_motion_blur(ctx, layer, blur) {
        layer.layer_time(ctx.time).0.hash(&mut h);
    }
    if time_dependent(layer, effects != EffectScope::Footprint) {
        layer.layer_time(ctx.time).0.hash(&mut h);
        // Effects also see the layer's time mapping.
        layer.start_time.0.hash(&mut h);
        layer.stretch.to_bits().hash(&mut h);
    }
    // Effects that see the comp camera or lights (#397) change when they or the layer move.
    if effects != EffectScope::Footprint && reads_comp_scene(layer) {
        hash_debug(&mut h, &crate::comp_scene(ctx, layer));
    }
    Some(h.finish())
}

/// Does the layer read another layer through an enabled effect's layer parameter (Gradient Wipe,
/// Set Matte…)? Those layers are drawn by the reading layer's renderer, so the switches it
/// passes down belong in the key (`Renderer::nested_look`).
pub(crate) fn reads_layers(ctx: &EvalCtx, layer: &Layer) -> bool {
    /// Whether any enabled layer-valued property of `layer` points at another layer.
    fn reads(ctx: &EvalCtx, layer: &Layer, g: &PropGroup, depth: usize) -> bool {
        depth < 32
            && g.children.iter().any(|n| match n {
                Node::Prop(p) => matches!(p.value, Value::Layer(_)) && ctx.value(layer, p).as_layer().is_some_and(|id| id != layer.id.0),
                Node::Group(sub) => sub.enabled && reads(ctx, layer, sub, depth + 1),
            })
    }
    layer.switches.effects && layer.effects().is_some_and(|fx| fx.groups().any(|g| g.enabled && reads(ctx, layer, g, 0)))
}

/// Does the layer run an effect that reads the comp camera or lights (Card Dance, CC Particle
/// World…)?
fn reads_comp_scene(layer: &Layer) -> bool {
    layer.switches.effects
        && layer.effects().is_some_and(|fx| {
            fx.groups().any(|g| {
                g.enabled
                    && matches!(&g.kind, effectcraft_project::GroupKind::Effect { effect } if effectcraft_effects::READS_COMP_SCENE.contains(&effect.as_str()))
            })
        })
}

/// Cache key for the styled layer (content key + the Layer Styles group: structure, eye
/// switches, values, keyframes, expressions and their values at the context time — Global Light
/// included, as every layer mirrors it in its Blending Options).
pub fn styles_key(ctx: &EvalCtx, layer: &Layer, content_key: u64) -> u64 {
    let mut h = KeyHasher(content_key ^ 0x9e37_79b9_7f4a_7c15);
    if let Some(g) = layer.layer_styles() {
        hash_debug(&mut h, g);
        hash_values(&mut h, ctx, layer, g);
    }
    h.finish()
}

/// A sub-key of `key` (styled layer passes, flattened buffer).
pub fn derive(key: u64, i: u64) -> u64 {
    let mut h = KeyHasher(key);
    i.hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod shared_insert_tests {
    use std::sync::Arc;

    use super::LayerCache;
    use effectcraft_effects::Buf;
    use effectcraft_raster::Image;

    /// Two renders of one layer kept under one key: both use the buffer kept first.
    #[test]
    fn the_buffer_kept_first_is_shared() {
        let cache = LayerCache::default();
        let buf = || Arc::new(Buf { img: Image::new(4, 4), offset: [0.0; 2], scale: 1.0 });
        let (a, b) = (buf(), buf());
        assert!(Arc::ptr_eq(&cache.insert(3, a.clone(), Default::default()).0, &a));
        assert!(Arc::ptr_eq(&cache.insert(3, b, Default::default()).0, &a), "the later render gets the first buffer");
        assert!(Arc::ptr_eq(&cache.get(3).unwrap().0, &a));
    }

    /// The browser's store (#3184): a fetched entry is used only while the files it lists are
    /// unchanged (here from memory, through the hook), and comes with them; one made of files
    /// that changed is dropped and counts as a miss, so the buffer rendered again is queued over
    /// it. A buffer whose files can't all be named is not queued at all.
    #[test]
    fn the_prefetch_store_checks_the_files_of_its_entries() {
        use super::{LayerStore, PrefetchStore};
        use crate::disk_cache::{EntryFiles, layer_disk_key, layer_entry};
        use crate::reads::{FileStamp, ReadFile, ReadFiles};
        let buf = || Buf { img: Image::new(2, 2), offset: [0.0; 2], scale: 1.0 };
        let files: ReadFiles = [ReadFile::new("/v.png", FileStamp::Virtual)].into_iter().collect();
        let (one, two) = (|_: &str| Some((3, 1)), |_: &str| Some((3, 2)));
        let entry = layer_entry(&buf(), &EntryFiles::of(&files, Some(&one)).unwrap());
        let store = PrefetchStore::new(1 << 20).with_min_ms(0);
        assert!(store.provide(5, &entry));
        assert!(!store.provide(6, b"damaged"));
        let (_, got) = store.get_layer(5, Some(&one)).unwrap();
        assert_eq!(got, files);
        assert_eq!(store.take_hits(), [5]);
        // The file changed: a miss, forgotten, and the new buffer is queued over it.
        assert!(store.get_layer(5, Some(&two)).is_none());
        assert!(!store.has(5));
        assert_eq!(store.take_misses(), [5]);
        store.put_layer(5, &Arc::new(buf()), &EntryFiles::of(&files, Some(&two)).unwrap(), 0);
        assert_eq!(store.take_writes().len(), 1, "written over the changed entry");
        // Through the layer cache: a hit reports its files; incomplete lists are not queued.
        let cache = LayerCache::default();
        let store = Arc::new(PrefetchStore::new(1 << 20).with_min_ms(0));
        cache.set_store(Some(store.clone()));
        cache.set_virtual_id(Some(Arc::new(one)));
        assert!(store.provide(layer_disk_key(9), &entry));
        let (_, from_store) = cache.get(9).unwrap();
        assert!(*from_store == files && from_store.is_complete());
        let mut incomplete = files.clone();
        incomplete.mark_incomplete();
        assert!(cache.get(10).is_none());
        cache.insert(10, Arc::new(buf()), Arc::new(incomplete));
        assert!(cache.get(11).is_none());
        cache.insert(11, Arc::new(buf()), Arc::new(files.clone()));
        let written: Vec<u128> = store.take_writes().into_iter().map(|w| w.0).collect();
        assert_eq!(written, [layer_disk_key(11)], "only the buffer whose files are all named");
    }

    /// A layer buffer whose render started before Reload Footage dropped its file (#3184) is
    /// not written to the disk cache when it is kept afterwards; one whose render started after
    /// the drop is.
    #[test]
    fn a_buffer_rendered_across_a_drop_is_not_written() {
        use crate::disk_cache::{DiskCache, Kind, layer_disk_key};
        use crate::reads::{FileStamp, ReadFile, ReadLog};
        let dir = tempfile::tempdir().unwrap();
        let dc = DiskCache::open(dir.path(), 1 << 30).unwrap();
        dc.set_min_layer_ms(0);
        let cache = LayerCache::default();
        cache.set_disk(Some(dc.clone()));
        let buf = || Arc::new(Buf { img: Image::new(2, 2), offset: [0.0; 2], scale: 1.0 });
        // A render's files, read after it took the drop generation (`Renderer::recorded`).
        let render = || {
            let log = ReadLog::starting_at(cache.drop_generation().unwrap());
            move || {
                log.note(ReadFile::new("/a.png", FileStamp::Native { len: 3, modified: Some(1) }));
                Arc::new(log.into_files())
            }
        };
        // The miss starts the render; the file is reloaded before the buffer is kept.
        assert!(cache.get(1).is_none());
        let a = render();
        dc.drop_files(["/a.png"]);
        dc.flush();
        // Another render of the same key starts after the drop, while the first one still runs
        // (its miss must not lend the first render its generation).
        assert!(cache.get(1).is_none());
        let b = render();
        let (stale, _) = cache.insert(1, buf(), a());
        dc.flush();
        assert!(!dc.contains(Kind::Layer, layer_disk_key(1)), "made of the file as it was before the drop");
        // Not kept in memory either: the second render keeps its own buffer, and writes it.
        let fresh = buf();
        let (kept, _) = cache.insert(1, fresh.clone(), b());
        assert!(Arc::ptr_eq(&kept, &fresh) && !Arc::ptr_eq(&kept, &stale));
        // A render of another key after the drop.
        assert!(cache.get(2).is_none());
        cache.insert(2, buf(), render()());
        dc.flush();
        assert!(dc.contains(Kind::Layer, layer_disk_key(1)), "the render after the drop");
        assert!(dc.contains(Kind::Layer, layer_disk_key(2)));
        // A list that doesn't say when its reads began counts as made before the drop.
        assert!(cache.get(3).is_none());
        cache.insert(3, buf(), Arc::new([ReadFile::new("/a.png", FileStamp::Native { len: 3, modified: Some(1) })].into_iter().collect()));
        dc.flush();
        assert!(!dc.contains(Kind::Layer, layer_disk_key(3)));
        drop((cache, dc));
    }

    /// A disk cache whose layer writes stop at a barrier until the test lets them go on.
    struct PausedWrites {
        dc: Arc<crate::disk_cache::DiskCache>,
        arrived: std::sync::Barrier,
        go_on: std::sync::Barrier,
    }

    impl super::LayerStore for PausedWrites {
        fn get_layer(&self, key: u128, virtual_id: Option<crate::disk_cache::VirtualFileId<'_>>) -> Option<(Arc<Buf>, crate::reads::ReadFiles)> {
            super::LayerStore::get_layer(&*self.dc, key, virtual_id)
        }
        fn put_layer(&self, key: u128, buf: &Arc<Buf>, files: &crate::disk_cache::EntryFiles, since: u64) {
            self.arrived.wait();
            self.go_on.wait();
            super::LayerStore::put_layer(&*self.dc, key, buf, files, since);
        }
        fn drop_generation(&self) -> u64 {
            self.dc.drop_generation()
        }
        fn dropped_since(&self, files: &crate::reads::ReadFiles, since: u64) -> bool {
            self.dc.dropped_since(files, since)
        }
    }

    /// Reload Footage while a render from before it is writing its buffer: the cache is cleared
    /// and the file dropped after the buffer passed its check, before it is kept. The buffer
    /// then reaches memory after the purge, but it is never served, and a render after the drop
    /// keeps its own buffer (#3191 review).
    #[test]
    fn a_buffer_kept_after_the_purge_of_a_reload_is_not_served() {
        use crate::reads::{FileStamp, ReadFile, ReadLog};
        let dir = tempfile::tempdir().unwrap();
        let dc = crate::disk_cache::DiskCache::open(dir.path(), 1 << 30).unwrap();
        let store = Arc::new(PausedWrites { dc: dc.clone(), arrived: std::sync::Barrier::new(2), go_on: std::sync::Barrier::new(2) });
        let cache = Arc::new(LayerCache::default());
        cache.set_store(Some(store.clone()));
        let buf = || Arc::new(Buf { img: Image::new(2, 2), offset: [0.0; 2], scale: 1.0 });
        let files = |cache: &LayerCache| {
            let log = ReadLog::starting_at(cache.drop_generation().unwrap());
            log.note(ReadFile::new("/a.png", FileStamp::Native { len: 3, modified: Some(1) }));
            Arc::new(log.into_files())
        };
        // The old render: a miss, its reads, then its insert, which stops in the disk write.
        assert!(cache.get(1).is_none());
        let old_files = files(&cache);
        let old = buf();
        let writer = {
            let (cache, old) = (cache.clone(), old.clone());
            std::thread::spawn(move || cache.insert(1, old, old_files))
        };
        store.arrived.wait();
        // Reload Footage meanwhile: the layer cache is purged and the file dropped.
        cache.clear();
        dc.drop_files(["/a.png"]);
        dc.flush();
        store.go_on.wait();
        let _ = writer.join().unwrap();
        dc.flush();
        assert!(!dc.contains(crate::disk_cache::Kind::Layer, crate::disk_cache::layer_disk_key(1)), "not on disk");
        assert!(cache.get(1).is_none(), "the old pixels are not served from memory");
        // A render after the drop keeps its own buffer.
        let fresh = buf();
        let new_files = files(&cache);
        let mut kept = None;
        std::thread::scope(|s| {
            s.spawn(|| {
                store.arrived.wait();
                store.go_on.wait();
            });
            kept = Some(cache.insert(1, fresh.clone(), new_files).0);
        });
        let kept = kept.unwrap();
        assert!(Arc::ptr_eq(&kept, &fresh) && !Arc::ptr_eq(&kept, &old));
        assert!(Arc::ptr_eq(&cache.get(1).unwrap().0, &fresh));
    }

    /// A render that gets another render's buffer gets that buffer's files too (#3184): when a
    /// file changed between the two reads, its frame lists the file with both identities, so the
    /// list is incomplete and the frame is not kept on disk with the new identity and the old
    /// pixels.
    #[test]
    fn the_buffer_kept_first_brings_its_files() {
        use crate::reads::{FileStamp, ReadFile, ReadFiles, ReadLog};
        use crate::{NoFootage, RenderOpts, Renderer};
        let cache = LayerCache::default();
        let buf = || Arc::new(Buf { img: Image::new(4, 4), offset: [0.0; 2], scale: 1.0 });
        let files = |len| Arc::new([ReadFile::new("/a.png", FileStamp::Native { len, modified: Some(1) })].into_iter().collect::<ReadFiles>());
        let (first, second) = (files(1), files(2));
        let a = buf();
        cache.insert(3, a.clone(), first.clone());
        let project = effectcraft_project::Project::default();
        let log = ReadLog::new();
        let mut r = Renderer::new(&project, &NoFootage, RenderOpts::default());
        r.cache = Some(&cache);
        r.reads = Some(&log);
        // (what `Renderer::recorded` noted for the second render)
        log.note_all(&second);
        let (kept, kept_files) = r.keep(Some(3), buf(), &second);
        assert!(Arc::ptr_eq(&kept, &a));
        assert_eq!(kept_files, first);
        assert!(!log.files().is_complete());
        // The same files: nothing changes.
        let same = ReadLog::new();
        r.reads = Some(&same);
        same.note_all(&first);
        let _ = r.keep(Some(3), buf(), &files(1));
        assert!(same.files().is_complete());
    }
}
