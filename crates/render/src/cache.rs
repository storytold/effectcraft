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

/// `--exp-input-cache`: new caches admit only results rendered again ([`set_input_cache`]).
static INPUT_CACHE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Make caches created from now on reuse-only (the CLI's `--exp-input-cache` or
/// `EFFECTCRAFT_EXP_INPUT_CACHE=1`, read by the CLI). Off by default.
pub fn set_input_cache(on: bool) {
    INPUT_CACHE.store(on, std::sync::atomic::Ordering::Relaxed);
}

fn input_cache() -> bool {
    INPUT_CACHE.load(std::sync::atomic::Ordering::Relaxed)
}
use effectcraft_project::{ItemKind, Layer, LayerSource, Node, PropGroup};

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
    /// `--exp-input-cache` crop: how to give the full buffer back.
    full: Option<Crop>,
    /// The full-extent buffer of a cropped entry, kept while the entry is hot (hit within a
    /// cycle) so hits share one `Arc` and one GPU upload instead of a copy and an upload per
    /// frame. Its bytes count in `bytes`; dropped when the entry idles past a cycle.
    expanded: Option<Arc<Buf>>,
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
    /// `--exp-input-cache`: keys asked for (missed) before. Only these are admitted, so a key
    /// seen once (an animated layer: a new key every frame) never evicts reused entries.
    seen: HashMap<u64, u64>,
    /// Keys evicted (to count misses that re-render something the cache had: thrash).
    evicted: std::collections::HashSet<u64>,
    counts: Counts,
    /// `--exp-input-cache` (set at creation; [`LayerCache::set_reuse_only`]).
    reuse_only: bool,
}

/// Diagnostics (`EFFECTCRAFT_CACHE_STATS=1` prints them when the cache is dropped).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub inserts: u64,
    pub rejected: u64,
    pub evictions: u64,
    pub evicted_bytes: u64,
    /// Misses of keys the cache held before and evicted.
    pub re_misses: u64,
    pub peak_bytes: usize,
    /// `--exp-input-cache`: inserts stored cropped.
    pub cropped: u64,
}

/// Where a cropped entry came from: the full buffer's size, the crop origin within it and the
/// full buffer's own offset (restored bit for bit, not recomputed from the crop's).
#[derive(Clone, Copy, Debug)]
struct Crop {
    w: u32,
    h: u32,
    x0: i64,
    y0: i64,
    offset: [f64; 2],
}

/// `--exp-input-cache` keeps a layer's pixels cropped to the non-zero ones (plus a margin of
/// zeros, so sampling at the edge reads the same): full-frame layers with a small subject take
/// a fraction of the memory. Same pixels at the same place (the offset moves with the crop).
fn trimmed(buf: Arc<Buf>) -> (Arc<Buf>, Option<Crop>) {
    let img = &buf.img;
    let (w, h) = (img.width as usize, img.height as usize);
    if w == 0 || h == 0 || img.data.len() != w * h {
        return (buf, None);
    }
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
    for (y, row) in img.data.chunks(w).enumerate() {
        if let Some(f) = row.iter().position(|p| *p != [0.0; 4]) {
            let l = row.iter().rposition(|p| *p != [0.0; 4]).unwrap_or(f);
            (x0, x1, y0, y1) = (x0.min(f), x1.max(l + 1), y0.min(y), y + 1);
        }
    }
    if x1 == 0 {
        return (buf, None);
    }
    const M: usize = 2;
    let (x0, y0, x1, y1) = (x0.saturating_sub(M), y0.saturating_sub(M), (x1 + M).min(w), (y1 + M).min(h));
    // Only crop when it saves most of the frame: a marginal crop costs a scan now and a full-extent copy on every hit.
    if (x1 - x0) * (y1 - y0) * 4 > w * h {
        return (buf, None);
    }
    let mut out = (*buf).clone();
    out.img = img.crop(x0 as i64, y0 as i64, (x1 - x0) as u32, (y1 - y0) as u32);
    out.offset = [buf.offset[0] - x0 as f64, buf.offset[1] - y0 as f64];
    let crop = Crop { w: w as u32, h: h as u32, x0: x0 as i64, y0: y0 as i64, offset: buf.offset };
    (Arc::new(out), Some(crop))
}

/// A cropped entry handed back at its full extent and origin: compositing a smaller buffer at a
/// shifted offset rounds sample positions differently (1-2 levels in 8 bpc), so a hit must be
/// the very buffer that was rendered.
fn untrimmed(buf: Arc<Buf>, full: Option<Crop>) -> Arc<Buf> {
    let Some(c) = full else { return buf };
    let mut out = (*buf).clone();
    out.img = buf.img.crop(-c.x0, -c.y0, c.w, c.h);
    // The original offset, not `offset - x0 + x0`: that can round a fractional offset (or turn
    // -0.0 into 0.0) and move the layer's samples.
    out.offset = c.offset;
    Arc::new(out)
}

impl Drop for LayerCache {
    fn drop(&mut self) {
        if std::env::var_os("EFFECTCRAFT_CACHE_STATS").is_none() {
            return;
        }
        if let Ok(g) = self.inner.lock() {
            let total = (g.hits + g.misses).max(1);
            eprintln!(
                "layer cache: budget {} MB, hits {} misses {} (hit rate {:.1}%), {:?}, entries {} bytes {} MB, input-cache admission {}",
                g.budget >> 20,
                g.hits,
                g.misses,
                g.hits as f64 * 100.0 / total as f64,
                g.counts,
                g.map.len(),
                g.bytes >> 20,
                g.reuse_only
            );
        }
    }
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
            inner: Mutex::new(Inner {
                map: HashMap::new(),
                bytes: 0,
                budget,
                clock: 0,
                hits: 0,
                misses: 0,
                missed_at: HashMap::new(),
                seen: HashMap::new(),
                evicted: Default::default(),
                counts: Counts::default(),
                reuse_only: input_cache(),
            }),
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
                let (b, full, live) = (e.buf.clone(), e.full, e.expanded.clone());
                g.hits += 1;
                if now % 1024 == 0 {
                    // Drop expansions of entries idle for longer than a cycle.
                    let window = 2 * g.map.len() as u64 + 16;
                    Self::drop_expansions(&mut g, |idle| idle > window, key, None);
                }
                if full.is_none() {
                    return Some(b);
                }
                if let Some(x) = live {
                    return Some(x);
                }
                drop(g);
                let x = untrimmed(b, full);
                let xb = x.img.data.len() * std::mem::size_of::<effectcraft_raster::Px>();
                if let Ok(mut g) = self.inner.lock() {
                    // Keep the copy only within the budget: first drop other entries' idle
                    // copies, then go without (this hit copies again next time).
                    if g.bytes + xb > g.budget {
                        Self::drop_expansions(&mut g, |_| true, key, Some(xb));
                    }
                    if g.bytes + xb <= g.budget
                        && let Some(e) = g.map.get_mut(&key)
                        && e.expanded.is_none()
                    {
                        e.expanded = Some(x.clone());
                        e.bytes += xb;
                        g.bytes += xb;
                    }
                }
                Some(x)
            }
            None => {
                g.misses += 1;
                if g.missed_at.len() > 4096 {
                    g.missed_at.clear();
                }
                g.missed_at.insert(key, web_time::Instant::now());
                if g.evicted.remove(&key) {
                    g.counts.re_misses += 1;
                }
                drop(g);
                // Disk cache: a hit comes back into memory.
                let (dc, dk) = self.disk_key(key)?;
                let buf = dc.get_layer(dk)?;
                self.insert_mem(key, buf.clone());
                Some(buf)
            }
        }
    }

    pub fn insert(&self, key: u64, buf: Arc<Buf>) {
        if self.gated() {
            return;
        }
        let started = self.inner.lock().ok().and_then(|mut g| g.missed_at.remove(&key));
        if let Some((dc, dk)) = self.disk_key(key) {
            let slow = started.is_some_and(|t| t.elapsed().as_secs_f64() * 1e3 >= dc.min_layer_ms() as f64);
            if slow {
                dc.put_layer(dk, &buf);
            }
        }
        self.insert_mem(key, buf);
    }

    /// Drop the full-extent copies of entries (other than `keep`) whose idle time passes
    /// `idle_ok`, least recently used first: with `room = Some(n)` until `n` more bytes fit the
    /// budget, with `None` all of them.
    fn drop_expansions(g: &mut Inner, idle_ok: impl Fn(u64) -> bool, keep: u64, room: Option<usize>) {
        let now = g.clock;
        let mut v: Vec<(u64, u64)> =
            g.map.iter().filter(|(k, e)| **k != keep && e.expanded.is_some() && idle_ok(now - e.last_use)).map(|(k, e)| (e.last_use, *k)).collect();
        v.sort_unstable();
        for (_, k) in v {
            if room.is_some_and(|n| g.bytes + n <= g.budget) {
                break;
            }
            if let Some(e) = g.map.get_mut(&k)
                && let Some(x) = e.expanded.take()
            {
                let xb = x.img.data.len() * std::mem::size_of::<effectcraft_raster::Px>();
                e.bytes -= xb;
                g.bytes -= xb;
            }
        }
    }

    fn insert_mem(&self, key: u64, buf: Arc<Buf>) {
        let raw = buf.img.data.len() * std::mem::size_of::<effectcraft_raster::Px>();
        // `--exp-input-cache` only changes what happens under memory pressure: while the result
        // fits the budget as it is, it is stored whole and at once, as by the plain cache (no
        // second render to be admitted, no crop scan, no full-extent copy on every hit).
        let pressed = self.inner.lock().map(|g| g.reuse_only && g.bytes + raw > g.budget).unwrap_or(false);
        let (buf, full) = if pressed { trimmed(buf) } else { (buf, None) };
        let cropped = full.is_some();
        let bytes = buf.img.data.len() * std::mem::size_of::<effectcraft_raster::Px>();
        let Ok(mut g) = self.inner.lock() else { return };
        if bytes > g.budget / 4 {
            return;
        }
        g.counts.inserts += 1;
        g.counts.cropped += cropped as u64;
        // `--exp-input-cache` over budget: admit a key the second time it is rendered (seen =
        // reused). A one-off result (a new key every frame) then never pushes reused ones out.
        if pressed && g.reuse_only && !g.map.contains_key(&key) {
            let now = g.clock;
            if g.seen.insert(key, now).is_none() {
                if g.seen.len() > 1 << 16 {
                    let cut = now.saturating_sub(1 << 15);
                    g.seen.retain(|_, t| *t >= cut);
                }
                g.counts.rejected += 1;
                return;
            }
        }
        g.clock += 1;
        let now = g.clock;
        if let Some(old) = g.map.insert(key, Entry { buf, bytes, last_use: now, full, expanded: None }) {
            g.bytes -= old.bytes;
        }
        g.bytes += bytes;
        if g.bytes > g.budget && g.reuse_only {
            Self::drop_expansions(&mut g, |_| true, key, Some(0));
        }
        if g.bytes > g.budget && g.reuse_only {
            // `--exp-input-cache`: a frame's reused entries come round once per frame (a cycle).
            // LRU over a cycle bigger than the budget evicts each entry just before it is needed
            // (Singularity at 1 GB). Evict only entries idle for longer than a cycle; otherwise
            // keep what is held and drop the newcomer.
            let window = 2 * g.map.len() as u64 + 16;
            while g.bytes > g.budget {
                let Some((age, k)) = g.map.iter().filter(|(k, _)| **k != key).map(|(k, e)| (e.last_use, *k)).min() else { break };
                let victim = if now - age > window { k } else { key };
                if let Some(e) = g.map.remove(&victim) {
                    g.bytes -= e.bytes;
                    if victim == key {
                        g.counts.rejected += 1;
                        break;
                    }
                    g.counts.evictions += 1;
                    g.counts.evicted_bytes += e.bytes as u64;
                    if g.evicted.len() < 1 << 16 {
                        g.evicted.insert(victim);
                    }
                }
            }
        } else if g.bytes > g.budget {
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
                    g.counts.evictions += 1;
                    g.counts.evicted_bytes += e.bytes as u64;
                    if g.evicted.len() < 1 << 16 {
                        g.evicted.insert(k);
                    }
                }
            }
        }
        g.counts.peak_bytes = g.counts.peak_bytes.max(g.bytes);
    }

    /// Admit only keys rendered a second time (`--exp-input-cache`).
    pub fn set_reuse_only(&self, on: bool) {
        if let Ok(mut g) = self.inner.lock() {
            g.reuse_only = on;
        }
    }

    /// Diagnostic counters.
    pub fn counts(&self) -> Counts {
        self.inner.lock().map(|g| g.counts).unwrap_or_default()
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
        return group_has(c, "wiggle");
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
/// time, or `None` when the layer must not be cached (footage, precomps, cameras, adjustment
/// layers…).
pub fn layer_key(ctx: &EvalCtx, layer: &Layer, scale: f64, draft: bool, blur: bool) -> Option<u64> {
    // Time effects see property values at other times. Keyframes are part of the hashed
    // structure (and the layer time is folded in), but expressions may read other layers at
    // those times, which the key cannot see: such layers are not cached.
    if reads_other_times(layer) && layer.props.children.iter().any(|c| matches!(c, Node::Group(g) if g.match_id != "transform" && has_expression(g))) {
        return None;
    }
    key_with(ctx, layer, scale, draft, blur, false)
}

/// Cache key for a layer's *input* at the context time: source → masks → its first `effects`
/// effects (see `Renderer::layer_input`). Unlike [`layer_key`], footage layers are cached here
/// (keyed by item and source time), since Time effects read many neighbouring frames.
pub fn input_key(ctx: &EvalCtx, layer: &Layer, scale: f64, draft: bool, blur: bool, effects: usize) -> Option<u64> {
    if layer.switches.adjustment {
        return None;
    }
    let base = key_any(ctx, layer, scale, draft, blur, true, Some(effects))?;
    let mut h = KeyHasher(base ^ 0x5bd1_e995_7a3c_11d3);
    effects.hash(&mut h);
    Some(h.finish())
}

/// Does the layer run an effect that reads the layer at other times (Echo, Timewarp…)?
fn reads_other_times(layer: &Layer) -> bool {
    layer.switches.effects
        && layer.effects().is_some_and(|fx| {
            fx.groups().any(|g| g.enabled && matches!(&g.kind, effectcraft_project::GroupKind::Effect { effect } if effect.starts_with("ec.time.")))
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
#[allow(clippy::too_many_arguments)]
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
        LayerSource::Footage { item } if footage => {
            let it = ctx.project.item(*item)?;
            let ItemKind::Footage(f) = &it.kind else { return None };
            hash_debug(&mut h, f);
            // A proxy (and its Use Proxy switch) changes the pixels.
            hash_debug(&mut h, &it.proxy);
            item.hash(&mut h);
            ctx.source_time(layer).0.hash(&mut h);
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
mod reuse_tests {
    use super::*;

    fn buf(px: usize) -> Arc<Buf> {
        Arc::new(Buf { img: effectcraft_raster::Image::new(px as u32, 1), offset: [0.0, 0.0], scale: 1.0 })
    }

    /// `--exp-input-cache`: within the budget a result is kept at once (as the plain cache);
    /// over it, a key seen for the first time is not admitted.
    #[test]
    fn reuse_only_admits_at_once_within_the_budget_and_on_reuse_over_it() {
        let c = LayerCache::new(1 << 20);
        c.set_reuse_only(true);
        c.insert(1, buf(16));
        assert!(c.get(1).is_some(), "within the budget: kept at once");
        let tight = LayerCache::new(4 * 16 * 16);
        tight.set_reuse_only(true);
        for k in 1..=4 {
            tight.insert(k, buf(16));
        }
        tight.insert(5, buf(16));
        assert!(tight.get(5).is_none(), "over the budget: a first render is not kept");
        assert_eq!(tight.counts().rejected, 1);
    }

    /// The Singularity pattern: reused (static) entries plus new one-off keys every frame
    /// (animated layers) over budget. Plain LRU evicts the reused ones every frame (they come
    /// back as misses); reuse-only keeps them.
    #[test]
    fn cropped_entries_come_back_bit_for_bit_with_fractional_offsets() {
        let full = |ox: f64, oy: f64, seed: f32| {
            let mut img = effectcraft_raster::Image::new(64, 48);
            for y in 20..30 {
                for x in 30..41 {
                    img.data[y * 64 + x] = [seed * x as f32 / 7.0, 0.1 + y as f32 / 97.0, 0.3, 0.75];
                }
            }
            Arc::new(Buf { img, offset: [ox, oy], scale: 1.0 })
        };
        let cases = [(-3.25, 7.1), (0.1, -0.0), (-0.0, 0.3), (1e-9 - 40.0, 12345.678)];
        let bytes = 64 * 48 * std::mem::size_of::<effectcraft_raster::Px>();
        // Room for four full buffers (an entry may take a quarter of the budget): the rest
        // arrives under pressure and is cropped.
        let budget = bytes * 4 + bytes / 2;
        let c = LayerCache::new(budget);
        c.set_reuse_only(true);
        for k in 0..4u64 {
            c.insert(100 + k, full(0.0, 0.0, 1.0));
        }
        for (k, &(ox, oy)) in cases.iter().enumerate() {
            let src = full(ox, oy, 1.0 + k as f32);
            c.insert(k as u64, src.clone());
            c.insert(k as u64, src.clone());
            for _ in 0..2 {
                let got = c.get(k as u64).expect("admitted on reuse");
                assert_eq!((got.img.width, got.img.height), (64, 48));
                assert!(got.img.data == src.img.data, "case {k}: pixels");
                assert_eq!(got.offset.map(f64::to_bits), src.offset.map(f64::to_bits), "case {k}: offset");
            }
        }
        assert!(c.counts().cropped >= cases.len() as u64, "{:?}", c.counts());
        assert!(c.stats().bytes <= budget);
    }

    #[test]
    fn one_off_keys_do_not_evict_reused_entries() {
        let px = 1024;
        let budget = 6 * px * 16;
        let run = |reuse: bool| {
            let c = LayerCache::new(budget);
            c.set_reuse_only(reuse);
            for frame in 0..20u64 {
                for k in (1..=4u64).chain((0..3).map(|j| 1000 + frame * 10 + j)) {
                    if c.get(k).is_none() {
                        c.insert(k, buf(px));
                    }
                }
            }
            (c.stats().hits, c.counts().re_misses)
        };
        let (lru_hits, lru_thrash) = run(false);
        let (reuse_hits, reuse_thrash) = run(true);
        assert_eq!(reuse_hits, 4 * 19, "statics hit from the second frame on");
        assert_eq!(reuse_thrash, 0);
        assert!(lru_hits < reuse_hits && lru_thrash > 0, "LRU {lru_hits} hits, {lru_thrash} re-misses");
    }
}
