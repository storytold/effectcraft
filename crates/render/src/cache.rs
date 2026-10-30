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
use effectcraft_project::{FootageKind, ItemKind, Layer, LayerSource, Node, PropGroup};

use crate::eval::EvalCtx;

mod dependencies;

/// Thread-safe, memory-budgeted cache of processed layer buffers, optionally backed by the
/// persistent [`DiskCache`](crate::disk_cache::DiskCache): a memory miss reads the disk, and
/// buffers that were slow to render are written there (keys salted per project, see
/// [`LayerCache::set_disk`]).
pub struct LayerCache {
    inner: Mutex<Inner>,
    disk: Mutex<Option<(Arc<crate::disk_cache::DiskCache>, u64)>>,
    /// Another persistent store when there is no disk cache (the browser's, [`PrefetchStore`]).
    store: Mutex<Option<(Arc<dyn LayerStore>, u64)>>,
    /// While set, inserts are dropped (see [`LayerCache::set_gate`]).
    gate: Mutex<Option<Arc<std::sync::atomic::AtomicBool>>>,
}

struct Entry {
    buf: Arc<Buf>,
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
    /// When each recent miss happened (to time the render that follows it).
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

    /// Attach (or detach) the disk cache. `salt` separates projects and footage versions (see
    /// `footage_salt`); keys on disk are `(salt, key)`.
    pub fn set_disk(&self, disk: Option<(Arc<crate::disk_cache::DiskCache>, u64)>) {
        if let Ok(mut d) = self.disk.lock() {
            *d = disk;
        }
    }

    /// Change the disk salt (after footage changed) keeping the attached disk cache.
    pub fn set_disk_salt(&self, salt: u64) {
        if let Ok(mut d) = self.disk.lock()
            && let Some((_, s)) = d.as_mut()
        {
            *s = salt;
        }
    }

    pub fn disk(&self) -> Option<Arc<crate::disk_cache::DiskCache>> {
        self.disk.lock().ok()?.as_ref().map(|d| d.0.clone())
    }

    /// Attach (or detach) a [`LayerStore`] used when no disk cache is attached (the browser's
    /// Origin Private File System through a [`PrefetchStore`]); `salt` as in
    /// [`LayerCache::set_disk`].
    pub fn set_store(&self, store: Option<(Arc<dyn LayerStore>, u64)>) {
        if let Ok(mut s) = self.store.lock() {
            *s = store;
        }
    }

    /// Change the store's salt (a new project or footage), keeping the store.
    pub fn set_store_salt(&self, salt: u64) {
        if let Ok(mut d) = self.store.lock()
            && let Some((_, s)) = d.as_mut()
        {
            *s = salt;
        }
    }

    fn disk_key(&self, key: u64) -> Option<(Arc<dyn LayerStore>, u128)> {
        let salted = |salt: u64| ((salt as u128) << 64) | key as u128;
        if let Some((dc, salt)) = self.disk.lock().ok()?.as_ref() {
            return Some((dc.clone() as Arc<dyn LayerStore>, salted(*salt)));
        }
        let s = self.store.lock().ok()?;
        let (st, salt) = s.as_ref()?;
        Some((st.clone(), salted(*salt)))
    }

    pub fn get(&self, key: u64) -> Option<Arc<Buf>> {
        let mut g = self.inner.lock().ok()?;
        g.clock += 1;
        let now = g.clock;
        match g.map.get_mut(&key) {
            Some(e) => {
                e.last_use = now;
                let b = e.buf.clone();
                g.hits += 1;
                Some(b)
            }
            None => {
                g.misses += 1;
                if g.missed_at.len() > 4096 {
                    g.missed_at.clear();
                }
                g.missed_at.insert(key, web_time::Instant::now());
                drop(g);
                // Disk cache: a hit comes back into memory.
                let (dc, dk) = self.disk_key(key)?;
                let buf = dc.get_layer(dk)?;
                Some(self.insert_mem(key, buf))
            }
        }
    }

    /// Keep `buf` under `key`, and return the buffer the cache holds for it: the one kept first
    /// when another render of the same layer got there before (#1812: preview workers starting
    /// together each rendered every layer; sharing the first buffer, the GPU compositor uploads
    /// and packs it once instead of once per worker, which ran it out of memory). Renders don't
    /// wait for each other: on the frame workers' shared thread pool a wait can end up under
    /// the very work it waits for.
    pub fn insert(&self, key: u64, buf: Arc<Buf>) -> Arc<Buf> {
        if self.gated() {
            return buf;
        }
        if let Some(kept) = self.inner.lock().ok().and_then(|g| g.map.get(&key).map(|e| e.buf.clone())) {
            return kept;
        }
        let started = self.inner.lock().ok().and_then(|mut g| g.missed_at.remove(&key));
        if let Some((dc, dk)) = self.disk_key(key) {
            let slow = started.is_some_and(|t| t.elapsed().as_secs_f64() * 1e3 >= dc.min_layer_ms() as f64);
            if slow {
                dc.put_layer(dk, &buf);
            }
        }
        self.insert_mem(key, buf)
    }

    /// See [`LayerCache::insert`]: the buffer kept under `key` (`buf`, unless one was kept first
    /// or `buf` is too large to keep).
    fn insert_mem(&self, key: u64, buf: Arc<Buf>) -> Arc<Buf> {
        let bytes = buf.img.data.len() * std::mem::size_of::<effectcraft_raster::Px>();
        let Ok(mut g) = self.inner.lock() else { return buf };
        if bytes > g.budget / 4 {
            return buf;
        }
        g.clock += 1;
        let now = g.clock;
        if let Some(e) = g.map.get_mut(&key) {
            e.last_use = now;
            return e.buf.clone();
        }
        g.map.insert(key, Entry { buf: buf.clone(), bytes, last_use: now });
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
        buf
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

/// Persistent storage of layer buffers behind a [`LayerCache`] (keys salted per project).
pub trait LayerStore: Send + Sync {
    /// A stored buffer (synchronous: mid-render).
    fn get_layer(&self, key: u128) -> Option<Arc<Buf>>;
    /// Keep a buffer that was slow to render.
    fn put_layer(&self, key: u128, buf: &Buf);
    /// Buffers faster to render than this (ms) are not stored.
    fn min_layer_ms(&self) -> u64 {
        0
    }
}

/// Layers buffers rendered slower than this go to the browser's disk cache (ms).
pub const PREFETCH_MIN_LAYER_MS: u64 = 20;

/// A [`LayerStore`] over storage that can't be read in the middle of a render (the browser's
/// Origin Private File System, read asynchronously). Lookups are served from buffers fetched
/// before the frame ([`PrefetchStore::provide`]); the keys a render looked up and didn't find
/// are recorded ([`PrefetchStore::take_misses`]) so the transport can fetch them before the next
/// frame; buffers to store queue up as sealed entries ([`PrefetchStore::take_writes`]) for the
/// transport to write after the frame. Fetched buffers are kept least recently used first, up
/// to a byte budget.
pub struct PrefetchStore {
    st: Mutex<Prefetched>,
    budget: usize,
    min_ms: u64,
}

#[derive(Default)]
struct Prefetched {
    ready: HashMap<u128, (Arc<Buf>, u64)>,
    bytes: usize,
    clock: u64,
    misses: Vec<u128>,
    hits: Vec<u128>,
    writes: Vec<(u128, Vec<u8>)>,
    /// Keys stored or queued (not written twice).
    known: std::collections::HashSet<u128>,
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
    /// when damaged.
    pub fn provide(&self, key: u128, entry: &[u8]) -> bool {
        let Some(buf) = crate::disk_cache::read_layer_entry(entry) else { return false };
        let bytes = buf.img.data.len() * std::mem::size_of::<effectcraft_raster::Px>();
        let Ok(mut s) = self.st.lock() else { return false };
        s.clock += 1;
        let now = s.clock;
        if let Some((old, _)) = s.ready.insert(key, (Arc::new(buf), now)) {
            s.bytes -= old.img.data.len() * std::mem::size_of::<effectcraft_raster::Px>();
        }
        s.bytes += bytes;
        s.known.insert(key);
        while s.bytes > self.budget {
            let Some((&k, _)) = s.ready.iter().min_by_key(|(_, (_, t))| *t) else { break };
            if let Some((b, _)) = s.ready.remove(&k) {
                s.bytes -= b.img.data.len() * std::mem::size_of::<effectcraft_raster::Px>();
            }
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
    fn get_layer(&self, key: u128) -> Option<Arc<Buf>> {
        let mut s = self.st.lock().ok()?;
        s.clock += 1;
        let now = s.clock;
        match s.ready.get_mut(&key) {
            Some((b, t)) => {
                *t = now;
                let b = b.clone();
                if !s.hits.contains(&key) {
                    s.hits.push(key);
                }
                Some(b)
            }
            None => {
                if !s.misses.contains(&key) && s.misses.len() < 1024 {
                    s.misses.push(key);
                }
                None
            }
        }
    }

    fn put_layer(&self, key: u128, buf: &Buf) {
        let Ok(mut s) = self.st.lock() else { return };
        if s.known.insert(key) {
            s.writes.push((key, crate::disk_cache::layer_entry(buf)));
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
    let base = key_any(ctx, layer, scale, draft, blur, true, Some(effects))?;
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
                        if effect.starts_with("ec.time.") || effectcraft_effects::reads_param_history(effect))
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

/// Cache key for an adjustment layer's footprint (its source through its masks, see
/// `Renderer::adjustment_footprint`): the GPU compositor then uploads it once, not every frame.
pub fn footprint_key(ctx: &EvalCtx, layer: &Layer, scale: f64, draft: bool, blur: bool) -> Option<u64> {
    // The footprint is the source through the masks, without the effects: a time-dependent
    // effect (Noise on a finishing adjustment layer) must not re-key it every frame (on the GPU,
    // a full-frame re-upload per frame).
    Some(derive(key_any(ctx, layer, scale, draft, blur, false, Some(0))?, 0xf007_9417))
}

fn key_with(ctx: &EvalCtx, layer: &Layer, scale: f64, draft: bool, blur: bool, footage: bool) -> Option<u64> {
    if layer.switches.adjustment {
        return None;
    }
    key_any(ctx, layer, scale, draft, blur, footage, None)
}

/// `blur`: the layer is motion blurred in this render (`Renderer::mb_on`). `effects`: key only
/// the first that many effects (a layer's input, see [`input_key`]).
fn key_any(ctx: &EvalCtx, layer: &Layer, scale: f64, draft: bool, blur: bool, footage: bool, effects: Option<usize>) -> Option<u64> {
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
    // blurred in this render and on the comp's shutter.
    if layer.masks().is_some_and(|m| !m.children.is_empty()) || layer.effects().is_some_and(|fx| !fx.children.is_empty()) {
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
        // A layer's input runs only its first effects: the later ones (a Puppet whose pins are
        // dragged, #212) don't change it, so it stays cached.
        if let (Some(n), Node::Group(g)) = (effects, c)
            && g.match_id == "effects"
        {
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
        // Static structure: values, keyframes, expressions, enabled flags, effect ids, modes.
        hash_debug(&mut h, c);
        if let Node::Group(g) = c {
            hash_values(&mut h, ctx, layer, g);
        }
    }
    // Paint strokes appear and disappear with their Duration spans (not property values).
    if layer.switches.effects
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
    if time_dependent(layer, effects != Some(0)) {
        layer.layer_time(ctx.time).0.hash(&mut h);
        // Effects also see the layer's time mapping.
        layer.start_time.0.hash(&mut h);
        layer.stretch.to_bits().hash(&mut h);
    }
    // Effects that see the comp camera or lights (#397) change when they or the layer move.
    if reads_comp_scene(layer) {
        hash_debug(&mut h, &crate::comp_scene(ctx, layer));
    }
    Some(h.finish())
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
        assert!(Arc::ptr_eq(&cache.insert(3, a.clone()), &a));
        assert!(Arc::ptr_eq(&cache.insert(3, b), &a), "the later render gets the first buffer");
        assert!(Arc::ptr_eq(&cache.get(3).unwrap(), &a));
    }
}
