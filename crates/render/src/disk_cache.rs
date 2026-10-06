//! The persistent disk cache (Settings ▸ Media & Disk Cache ▸ Disk Cache): RAM-preview frames
//! and layer cache entries kept on disk between sessions, like After Effects' disk cache.
//!
//! * Entries are files keyed by a 128-bit content hash (`<folder>/v1/<kind>/<2 hex>/<32 hex>.ecc`):
//!   frames by the hash of the composition's content (the comp and everything it uses, footage
//!   files' size and modification time included) plus frame, scale and view; layer buffers by
//!   the layer cache's content key salted with the project's footage fingerprint. A changed
//!   project therefore never reads stale pixels: its keys simply change.
//! * Payloads are LZ4-compressed (`lz4_flex`, MIT): frames as 8-bit premultiplied RGBA, layer
//!   buffers as `f32` RGBA with the bytes shuffled into planes (exact, so cached and fresh
//!   renders are bit-identical). Every file ends in a checksum; damaged files read as misses and
//!   are deleted.
//! * Size limit with least-recently-used eviction. The in-memory index is rebuilt from the
//!   folder when the cache opens (file modification times order it), so the cache survives
//!   restarts; reads refresh a file's time.
//! * Safe concurrent access: writes go to a unique temporary file that is renamed into place
//!   (atomic on every desktop OS), so readers in this or another process never see a partial
//!   entry; a missing or damaged file is just a miss. Writes run on a background thread with a
//!   bounded queue (a full queue drops the write rather than stalling rendering).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use effectcraft_effects::Buf;
use effectcraft_raster::Image;

/// File format version (part of the folder layout).
const VERSION: &str = "v1";
const MAGIC: &[u8; 4] = b"ECDC";
/// Pending writes before new ones are dropped.
#[cfg(not(target_arch = "wasm32"))]
const QUEUE_LIMIT: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Frame,
    Layer,
}

impl Kind {
    /// The kind's folder name.
    pub fn dir(self) -> &'static str {
        match self {
            Kind::Frame => "frames",
            Kind::Layer => "layers",
        }
    }
    fn tag(self) -> u8 {
        match self {
            Kind::Frame => 1,
            Kind::Layer => 2,
        }
    }
}

/// A cached viewer frame: 8-bit premultiplied RGBA.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame8 {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Counters for tests, the Info panel and `cache.diskStats`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DiskStats {
    pub entries: usize,
    pub bytes: u64,
    pub max_bytes: u64,
    pub hits: u64,
    pub misses: u64,
    pub writes: u64,
    pub evictions: u64,
    pub dropped: u64,
}

struct Meta {
    bytes: u64,
    last_use: u64,
}

/// The cache's index: entry sizes and least-recently-used order under a size limit. Portable
/// (no file system): [`DiskCache`] keeps one over its folder, and the browser keeps one over the
/// Origin Private File System (`apps/effectcraft-web`, where the files are written by workers).
#[derive(Default)]
pub struct DiskIndex {
    map: HashMap<(Kind, u128), Meta>,
    total: u64,
    clock: u64,
}

impl DiskIndex {
    /// Record an entry (`bytes` on disk) as the most recently used; replaces an older record.
    pub fn insert(&mut self, kind: Kind, key: u128, bytes: u64) {
        self.clock += 1;
        let c = self.clock;
        if let Some(old) = self.map.insert((kind, key), Meta { bytes, last_use: c }) {
            self.total -= old.bytes;
        }
        self.total += bytes;
    }

    /// Mark an entry as just used (a read). `false` when it is not indexed.
    pub fn touch(&mut self, kind: Kind, key: u128) -> bool {
        self.clock += 1;
        let c = self.clock;
        match self.map.get_mut(&(kind, key)) {
            Some(m) => {
                m.last_use = c;
                true
            }
            None => false,
        }
    }

    /// Forget an entry; its size, if it was indexed.
    pub fn remove(&mut self, kind: Kind, key: u128) -> Option<u64> {
        let m = self.map.remove(&(kind, key))?;
        self.total -= m.bytes;
        Some(m.bytes)
    }

    pub fn contains(&self, kind: Kind, key: u128) -> bool {
        self.map.contains_key(&(kind, key))
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Bytes of every indexed entry.
    pub fn total(&self) -> u64 {
        self.total
    }

    /// Keys of one kind (unordered).
    pub fn keys(&self, kind: Kind) -> Vec<u128> {
        self.map.keys().filter(|k| k.0 == kind).map(|k| k.1).collect()
    }

    /// Entries from least to most recently used.
    pub fn lru_order(&self) -> Vec<(Kind, u128)> {
        let mut v: Vec<(u64, (Kind, u128))> = self.map.iter().map(|(k, m)| (m.last_use, *k)).collect();
        v.sort_unstable_by_key(|e| e.0);
        v.into_iter().map(|e| e.1).collect()
    }

    /// Remove least recently used entries until the total fits `max` bytes; returns them (the
    /// caller deletes their files).
    pub fn evict(&mut self, max: u64) -> Vec<(Kind, u128)> {
        if self.total <= max {
            return vec![];
        }
        let mut out = vec![];
        for k in self.lru_order() {
            if self.total <= max {
                break;
            }
            self.remove(k.0, k.1);
            out.push(k);
        }
        out
    }

    pub fn clear(&mut self) {
        *self = DiskIndex::default();
    }
}

/// Which layer buffers a browser frame worker should fetch from the disk cache before its next
/// frame (its lookups mid-render are synchronous, the Origin Private File System is not): the
/// layer-cache misses the workers reported for recent frames, most recent first, that are on
/// disk ([`DiskIndex`], kind [`Kind::Layer`]).
#[derive(Default)]
pub struct LayerPrefetch {
    recent: std::collections::VecDeque<u128>,
}

/// Misses [`LayerPrefetch`] remembers.
const PREFETCH_RECENT: usize = 512;

impl LayerPrefetch {
    /// A frame's layer-cache misses (salted keys).
    pub fn note_misses(&mut self, keys: impl IntoIterator<Item = u128>) {
        for k in keys {
            self.recent.retain(|x| *x != k);
            self.recent.push_front(k);
        }
        self.recent.truncate(PREFETCH_RECENT);
    }

    /// Up to `max` keys to fetch before the next frame.
    pub fn plan(&self, index: &DiskIndex, max: usize) -> Vec<u128> {
        self.recent.iter().copied().filter(|k| index.contains(Kind::Layer, *k)).take(max).collect()
    }

    pub fn clear(&mut self) {
        self.recent.clear();
    }
}

#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
enum Job {
    Write(Kind, u128, Vec<u8>),
}

struct Queue {
    jobs: std::collections::VecDeque<Job>,
    busy: bool,
}

pub struct DiskCache {
    dir: PathBuf,
    index: Mutex<DiskIndex>,
    max_bytes: AtomicU64,
    hits: AtomicU64,
    misses: AtomicU64,
    writes: AtomicU64,
    evictions: AtomicU64,
    dropped: AtomicU64,
    tmp_counter: AtomicU64,
    queue: Arc<(Mutex<Queue>, Condvar)>,
    /// Layer buffers that took less than this to render are not written (ms).
    min_layer_ms: AtomicU64,
}

/// FNV-1a 128-bit (content keys).
#[derive(Clone, Copy)]
pub struct Hash128(u128);

impl Default for Hash128 {
    fn default() -> Self {
        Hash128(0x6c62_272e_07bb_0142_62b8_2175_6295_c58d)
    }
}

impl Hash128 {
    pub fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 ^= b as u128;
            self.0 = self.0.wrapping_mul(0x0000_0000_0100_0000_0000_0000_0000_013b);
        }
    }
    pub fn write_u64(&mut self, v: u64) {
        self.write(&v.to_le_bytes());
    }
    pub fn finish(&self) -> u128 {
        self.0
    }
}

fn checksum(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// The platform's default disk cache folder.
pub fn default_folder() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    if cfg!(target_os = "macos")
        && let Some(h) = &home
    {
        return h.join("Library/Caches/EffectCraft/Disk Cache");
    }
    if cfg!(windows)
        && let Some(l) = std::env::var_os("LOCALAPPDATA")
    {
        return PathBuf::from(l).join("EffectCraft").join("Disk Cache");
    }
    if let Some(x) = std::env::var_os("XDG_CACHE_HOME") {
        return PathBuf::from(x).join("effectcraft/disk-cache");
    }
    home.map(|h| h.join(".cache/effectcraft/disk-cache")).unwrap_or_else(|| std::env::temp_dir().join("effectcraft-disk-cache"))
}

impl DiskCache {
    /// Open (or create) the cache in `folder`, holding at most `max_bytes`.
    pub fn open(folder: impl AsRef<Path>, max_bytes: u64) -> std::io::Result<Arc<DiskCache>> {
        let dir = folder.as_ref().join(VERSION);
        std::fs::create_dir_all(&dir)?;
        let dc = Arc::new(DiskCache {
            dir,
            index: Mutex::new(DiskIndex::default()),
            max_bytes: AtomicU64::new(max_bytes),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            writes: AtomicU64::new(0),
            evictions: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            tmp_counter: AtomicU64::new(0),
            queue: Arc::new((Mutex::new(Queue { jobs: Default::default(), busy: false }), Condvar::new())),
            min_layer_ms: AtomicU64::new(30),
        });
        dc.rescan();
        dc.evict();
        #[cfg(not(target_arch = "wasm32"))]
        {
            let weak = Arc::downgrade(&dc);
            let q = dc.queue.clone();
            std::thread::Builder::new().name("ec-disk-cache".into()).spawn(move || writer(weak, q))?;
        }
        Ok(dc)
    }

    pub fn folder(&self) -> &Path {
        self.dir.parent().unwrap_or(&self.dir)
    }

    /// Rebuild the index from the folder (oldest files first).
    pub fn rescan(&self) {
        let mut found: Vec<(std::time::SystemTime, Kind, u128, u64)> = vec![];
        for kind in [Kind::Frame, Kind::Layer] {
            let Ok(shards) = std::fs::read_dir(self.dir.join(kind.dir())) else { continue };
            for shard in shards.flatten() {
                let Ok(files) = std::fs::read_dir(shard.path()) else { continue };
                for f in files.flatten() {
                    let p = f.path();
                    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if name.contains(".tmp") {
                        // A writer that died mid-write; old temporaries are removed.
                        if f.metadata().and_then(|m| m.modified()).is_ok_and(|t| t.elapsed().is_ok_and(|e| e.as_secs() > 600)) {
                            let _ = std::fs::remove_file(&p);
                        }
                        continue;
                    }
                    let Some(hex) = name.strip_suffix(".ecc") else { continue };
                    let Ok(key) = u128::from_str_radix(hex, 16) else { continue };
                    let Ok(md) = f.metadata() else { continue };
                    found.push((md.modified().unwrap_or(std::time::UNIX_EPOCH), kind, key, md.len()));
                }
            }
        }
        found.sort_by_key(|e| e.0);
        let mut idx = DiskIndex::default();
        for (_, kind, key, bytes) in found {
            idx.insert(kind, key, bytes);
        }
        if let Ok(mut g) = self.index.lock() {
            *g = idx;
        }
    }

    fn path(&self, kind: Kind, key: u128) -> PathBuf {
        let hex = format!("{key:032x}");
        self.dir.join(kind.dir()).join(&hex[..2]).join(format!("{hex}.ecc"))
    }

    pub fn max_bytes(&self) -> u64 {
        self.max_bytes.load(Ordering::Relaxed)
    }

    /// Change the size limit (evicts the least recently used entries when shrinking).
    pub fn set_max_bytes(&self, b: u64) {
        self.max_bytes.store(b, Ordering::Relaxed);
        self.evict();
    }

    /// Layer buffers faster to render than this are not worth a disk write.
    pub fn set_min_layer_ms(&self, ms: u64) {
        self.min_layer_ms.store(ms, Ordering::Relaxed);
    }

    pub fn min_layer_ms(&self) -> u64 {
        self.min_layer_ms.load(Ordering::Relaxed)
    }

    pub fn contains(&self, kind: Kind, key: u128) -> bool {
        self.index.lock().is_ok_and(|g| g.contains(kind, key))
    }

    /// Keys of one kind (unordered).
    pub fn keys(&self, kind: Kind) -> Vec<u128> {
        self.index.lock().map(|g| g.keys(kind)).unwrap_or_default()
    }

    pub fn stats(&self) -> DiskStats {
        let (entries, bytes) = self.index.lock().map(|g| (g.len(), g.total())).unwrap_or_default();
        DiskStats {
            entries,
            bytes,
            max_bytes: self.max_bytes(),
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            writes: self.writes.load(Ordering::Relaxed),
            evictions: self.evictions.load(Ordering::Relaxed),
            dropped: self.dropped.load(Ordering::Relaxed),
        }
    }

    /// Delete every entry (Edit ▸ Purge ▸ All Disk Cache / Empty Disk Cache).
    pub fn clear(&self) {
        self.flush();
        if let Ok(mut g) = self.index.lock() {
            g.clear();
        }
        for kind in [Kind::Frame, Kind::Layer] {
            let _ = std::fs::remove_dir_all(self.dir.join(kind.dir()));
        }
    }

    /// Wait until queued writes are on disk.
    pub fn flush(&self) {
        let (m, cv) = &*self.queue;
        let Ok(mut q) = m.lock() else { return };
        #[cfg(target_arch = "wasm32")]
        {
            q.jobs.clear();
        }
        while !q.jobs.is_empty() || q.busy {
            q = match cv.wait(q) {
                Ok(q) => q,
                Err(_) => return,
            };
        }
    }

    fn read(&self, kind: Kind, key: u128) -> Option<Vec<u8>> {
        if !self.contains(kind, key) {
            self.misses.fetch_add(1, Ordering::Relaxed);
            return None;
        }
        let path = self.path(kind, key);
        let data = std::fs::read(&path).ok();
        let ok = data.as_deref().and_then(|d| verify(d, kind));
        match ok {
            Some(payload) => {
                let payload = payload.to_vec();
                self.hits.fetch_add(1, Ordering::Relaxed);
                if let Ok(mut g) = self.index.lock() {
                    g.touch(kind, key);
                }
                // Persist the recency for the next session (best effort).
                if let Ok(f) = std::fs::File::options().append(true).open(&path) {
                    let _ = f.set_modified(std::time::SystemTime::now());
                }
                Some(payload)
            }
            None => {
                self.misses.fetch_add(1, Ordering::Relaxed);
                // Gone (another process evicted it) or damaged: forget it.
                if data.is_some() {
                    let _ = std::fs::remove_file(&path);
                }
                self.forget(kind, key);
                None
            }
        }
    }

    fn forget(&self, kind: Kind, key: u128) {
        if let Ok(mut g) = self.index.lock() {
            g.remove(kind, key);
        }
    }

    fn enqueue(&self, kind: Kind, key: u128, payload: Vec<u8>) {
        #[cfg(target_arch = "wasm32")]
        {
            self.write_now(kind, key, &payload);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (m, cv) = &*self.queue;
            let Ok(mut q) = m.lock() else { return };
            if q.jobs.len() >= QUEUE_LIMIT {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                return;
            }
            q.jobs.push_back(Job::Write(kind, key, payload));
            cv.notify_all();
        }
    }

    /// Write an entry now (atomic rename), then evict down to the limit.
    fn write_now(&self, kind: Kind, key: u128, payload: &[u8]) {
        if self.contains(kind, key) {
            return;
        }
        let path = self.path(kind, key);
        let Some(parent) = path.parent() else { return };
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
        let data = seal(kind, payload);
        let n = self.tmp_counter.fetch_add(1, Ordering::Relaxed);
        let tmp = parent.join(format!("{key:032x}.tmp.{}.{n}", std::process::id()));
        if std::fs::write(&tmp, &data).is_err() {
            let _ = std::fs::remove_file(&tmp);
            return;
        }
        if std::fs::rename(&tmp, &path).is_err() {
            let _ = std::fs::remove_file(&tmp);
            return;
        }
        self.writes.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut g) = self.index.lock() {
            g.insert(kind, key, data.len() as u64);
        }
        self.evict();
    }

    /// Remove least recently used entries until the cache fits its limit.
    fn evict(&self) {
        let max = self.max_bytes();
        let victims = match self.index.lock() {
            Ok(mut g) => g.evict(max),
            Err(_) => return,
        };
        for (kind, key) in victims {
            let _ = std::fs::remove_file(self.path(kind, key));
            self.evictions.fetch_add(1, Ordering::Relaxed);
        }
    }

    // ------------------------------------------------------------ typed entries

    pub fn get_frame(&self, key: u128) -> Option<Frame8> {
        let p = self.read(Kind::Frame, key)?;
        decode_frame(&p)
    }

    /// Queue a frame for writing (8-bit premultiplied RGBA, `width * height * 4` bytes).
    pub fn put_frame(&self, key: u128, width: u32, height: u32, rgba: &[u8]) {
        if self.contains(Kind::Frame, key) || rgba.len() != width as usize * height as usize * 4 {
            return;
        }
        self.enqueue(Kind::Frame, key, encode_frame(width, height, rgba));
    }

    pub fn get_layer(&self, key: u128) -> Option<Buf> {
        let p = self.read(Kind::Layer, key)?;
        decode_layer(&p)
    }

    /// Queue a layer buffer for writing.
    pub fn put_layer(&self, key: u128, buf: &Buf) {
        if self.contains(Kind::Layer, key) {
            return;
        }
        self.enqueue(Kind::Layer, key, encode_layer(buf));
    }

    /// Write a layer buffer synchronously (tests).
    pub fn put_layer_now(&self, key: u128, buf: &Buf) {
        self.write_now(Kind::Layer, key, &encode_layer(buf));
    }

    /// Write a frame synchronously (tests).
    pub fn put_frame_now(&self, key: u128, width: u32, height: u32, rgba: &[u8]) {
        self.write_now(Kind::Frame, key, &encode_frame(width, height, rgba));
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn writer(dc: std::sync::Weak<DiskCache>, q: Arc<(Mutex<Queue>, Condvar)>) {
    let (m, cv) = &*q;
    loop {
        let job = {
            let Ok(mut g) = m.lock() else { return };
            loop {
                if let Some(j) = g.jobs.pop_front() {
                    g.busy = true;
                    break j;
                }
                if dc.strong_count() == 0 {
                    return;
                }
                g = match cv.wait_timeout(g, std::time::Duration::from_millis(500)) {
                    Ok((g, _)) => g,
                    Err(_) => return,
                };
            }
        };
        let Some(cache) = dc.upgrade() else { return };
        match job {
            Job::Write(kind, key, payload) => cache.write_now(kind, key, &payload),
        }
        drop(cache);
        if let Ok(mut g) = m.lock() {
            g.busy = false;
        }
        cv.notify_all();
    }
}

/// An entry file's bytes: header (magic, kind), payload, checksum.
pub fn seal(kind: Kind, payload: &[u8]) -> Vec<u8> {
    let mut data = Vec::with_capacity(payload.len() + 16);
    data.extend_from_slice(MAGIC);
    data.push(kind.tag());
    data.extend_from_slice(&[0, 0, 0]);
    data.extend_from_slice(payload);
    let sum = checksum(&data);
    data.extend_from_slice(&sum.to_le_bytes());
    data
}

/// A sealed frame entry ([`seal`] of the encoded frame): what the browser's frame workers
/// write to the Origin Private File System.
pub fn frame_entry(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    seal(Kind::Frame, &encode_frame(width, height, rgba))
}

/// A frame from a sealed entry's bytes (`None` when damaged).
pub fn read_frame_entry(data: &[u8]) -> Option<Frame8> {
    decode_frame(verify(data, Kind::Frame)?)
}

/// A sealed layer-buffer entry: what the browser's frame workers write to the Origin Private
/// File System (`layers/<32 hex>.ecc`).
pub fn layer_entry(buf: &Buf) -> Vec<u8> {
    seal(Kind::Layer, &encode_layer(buf))
}

/// A layer buffer from a sealed entry's bytes (`None` when damaged).
pub fn read_layer_entry(data: &[u8]) -> Option<Buf> {
    decode_layer(verify(data, Kind::Layer)?)
}

impl crate::cache::LayerStore for DiskCache {
    fn get_layer(&self, key: u128) -> Option<Arc<Buf>> {
        DiskCache::get_layer(self, key).map(Arc::new)
    }
    fn put_layer(&self, key: u128, buf: &Buf) {
        DiskCache::put_layer(self, key, buf);
    }
    fn min_layer_ms(&self) -> u64 {
        DiskCache::min_layer_ms(self)
    }
}

/// An entry's file name in its kind's folder (`<32 hex>.ecc`).
pub fn entry_name(key: u128) -> String {
    format!("{key:032x}.ecc")
}

/// The key of an entry file name (`None` for temporaries and other files).
pub fn parse_entry_name(name: &str) -> Option<u128> {
    let hex = name.strip_suffix(".ecc")?;
    (hex.len() == 32).then(|| u128::from_str_radix(hex, 16).ok()).flatten()
}

/// Check magic, kind and checksum; returns the payload.
fn verify(d: &[u8], kind: Kind) -> Option<&[u8]> {
    if d.len() < 16 || &d[..4] != MAGIC || d[4] != kind.tag() {
        return None;
    }
    let (body, sum) = d.split_at(d.len() - 8);
    let mut s = [0u8; 8];
    s.copy_from_slice(sum);
    (checksum(body) == u64::from_le_bytes(s)).then_some(&body[8..])
}

fn encode_frame(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgba.len() / 2 + 16);
    out.extend_from_slice(&w.to_le_bytes());
    out.extend_from_slice(&h.to_le_bytes());
    out.extend_from_slice(&lz4_flex::compress_prepend_size(rgba));
    out
}

fn decode_frame(p: &[u8]) -> Option<Frame8> {
    if p.len() < 8 {
        return None;
    }
    let w = u32::from_le_bytes(p[0..4].try_into().ok()?);
    let h = u32::from_le_bytes(p[4..8].try_into().ok()?);
    let rgba = lz4_flex::decompress_size_prepended(&p[8..]).ok()?;
    (rgba.len() == w as usize * h as usize * 4).then_some(Frame8 { width: w, height: h, rgba })
}

fn encode_layer(b: &Buf) -> Vec<u8> {
    let (w, h) = (b.img.width, b.img.height);
    let n = b.img.data.len();
    // Byte planes: all first bytes of every float, then all second bytes… (compresses well).
    let mut planes = vec![0u8; n * 16];
    for (i, px) in b.img.data.iter().enumerate() {
        for c in 0..4 {
            let bytes = px[c].to_le_bytes();
            for (k, byte) in bytes.iter().enumerate() {
                planes[(k * 4 + c) * n + i] = *byte;
            }
        }
    }
    let mut out = Vec::with_capacity(planes.len() / 4 + 40);
    out.extend_from_slice(&w.to_le_bytes());
    out.extend_from_slice(&h.to_le_bytes());
    out.extend_from_slice(&b.offset[0].to_le_bytes());
    out.extend_from_slice(&b.offset[1].to_le_bytes());
    out.extend_from_slice(&b.scale.to_le_bytes());
    out.extend_from_slice(&lz4_flex::compress_prepend_size(&planes));
    out
}

fn decode_layer(p: &[u8]) -> Option<Buf> {
    if p.len() < 32 {
        return None;
    }
    let w = u32::from_le_bytes(p[0..4].try_into().ok()?);
    let h = u32::from_le_bytes(p[4..8].try_into().ok()?);
    let f = |i: usize| -> Option<f64> { Some(f64::from_le_bytes(p[i..i + 8].try_into().ok()?)) };
    let offset = [f(8)?, f(16)?];
    let scale = f(24)?;
    let planes = lz4_flex::decompress_size_prepended(&p[32..]).ok()?;
    let n = w as usize * h as usize;
    if planes.len() != n * 16 {
        return None;
    }
    let mut img = Image::new(w, h);
    for (i, px) in img.data.iter_mut().enumerate() {
        for c in 0..4 {
            let mut bytes = [0u8; 4];
            for (k, byte) in bytes.iter_mut().enumerate() {
                *byte = planes[(k * 4 + c) * n + i];
            }
            px[c] = f32::from_le_bytes(bytes);
        }
    }
    Some(Buf { img, offset, scale })
}

// ---------------------------------------------------------------- content keys

/// Bump when rendering changes in a way that makes old disk entries wrong.
const RENDER_VERSION: &str = "effectcraft-render-2";

fn hash_debug(h: &mut Hash128, v: &impl std::fmt::Debug) {
    h.write(format!("{v:?}").as_bytes());
    h.write(&[0xff]);
}

/// A file's size and modification time (so edited footage gets new keys).
fn file_stamp(h: &mut Hash128, path: &str) {
    if path.is_empty() {
        return;
    }
    if let Ok(md) = std::fs::metadata(path) {
        h.write_u64(md.len());
        if let Ok(t) = md.modified()
            && let Ok(d) = t.duration_since(std::time::UNIX_EPOCH)
        {
            h.write_u64(d.as_nanos() as u64);
        }
    }
}

fn footage_stamp(h: &mut Hash128, f: &effectcraft_project::Footage) {
    file_stamp(h, &f.path);
    for p in &f.sequence {
        file_stamp(h, p);
    }
}

/// Salt for layer cache keys on disk: the renderer version and every footage file's identity
/// (layer keys name footage items, not file contents).
pub fn footage_salt(project: &effectcraft_project::Project) -> u64 {
    let mut h = Hash128::default();
    h.write(RENDER_VERSION.as_bytes());
    for (id, it) in &project.items {
        if let effectcraft_project::ItemKind::Footage(f) = &it.kind {
            h.write_u64(id.0);
            hash_debug(&mut h, f);
            footage_stamp(&mut h, f);
        }
    }
    let v = h.finish();
    (v as u64) ^ ((v >> 64) as u64)
}

/// Content key of a composition's pixels: project render settings, the comp and everything it
/// uses (nested comps, footage, solids, their proxies and Use Proxy switches, footage and proxy
/// files' size and time). With expressions in play (which can read any comp) the whole project
/// counts.
pub fn comp_content_key(project: &effectcraft_project::Project, comp: effectcraft_project::ItemId) -> u128 {
    use effectcraft_project::{ItemKind, LayerSource};
    let mut h = Hash128::default();
    h.write(RENDER_VERSION.as_bytes());
    hash_debug(&mut h, &project.settings);
    let mut seen = std::collections::BTreeSet::new();
    let mut stack = vec![comp];
    let mut expressions = false;
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        let Some(it) = project.item(id) else { continue };
        if let ItemKind::Comp(c) = &it.kind {
            for l in &c.layers {
                if let LayerSource::Comp { item } | LayerSource::Footage { item } | LayerSource::Solid { item } = l.source {
                    stack.push(item);
                }
                if !expressions {
                    l.props.walk("", &mut |_, p| expressions |= p.expr.is_some());
                }
            }
        }
    }
    let ids: Vec<effectcraft_project::ItemId> = if expressions { project.items.keys().copied().collect() } else { seen.into_iter().collect() };
    for id in ids {
        let Some(it) = project.item(id) else { continue };
        h.write_u64(id.0);
        h.write(it.name.as_bytes());
        hash_debug(&mut h, &it.kind);
        if let ItemKind::Footage(f) = &it.kind {
            footage_stamp(&mut h, f);
        }
        hash_debug(&mut h, &it.proxy);
        if let Some(px) = &it.proxy {
            footage_stamp(&mut h, &px.footage);
        }
    }
    h.write_u64(comp.0);
    h.finish()
}

/// Disk key of one viewer frame.
pub fn frame_key(content: u128, frame: i64, scale_milli: u32, view: u64) -> u128 {
    let mut h = Hash128::default();
    h.write(&content.to_le_bytes());
    h.write_u64(frame as u64);
    h.write_u64(scale_milli as u64);
    h.write_u64(view);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("effectcraft-disk-cache-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn buf(seed: u32, w: u32, h: u32) -> Buf {
        let mut img = Image::new(w, h);
        for (i, p) in img.data.iter_mut().enumerate() {
            let v = ((i as u32).wrapping_mul(2654435761u32).wrapping_add(seed) % 1000) as f32 / 999.0;
            *p = [v, v * 0.5, 1.0 - v, if i % 7 == 0 { 0.0 } else { 1.0 }];
        }
        Buf { img, offset: [1.5, -2.25], scale: 0.5 }
    }

    #[test]
    fn index_evicts_least_recently_used() {
        let mut ix = DiskIndex::default();
        ix.insert(Kind::Frame, 1, 100);
        ix.insert(Kind::Frame, 2, 100);
        ix.insert(Kind::Layer, 3, 100);
        assert_eq!((ix.len(), ix.total()), (3, 300));
        assert!(ix.touch(Kind::Frame, 1));
        assert!(!ix.touch(Kind::Frame, 3));
        assert_eq!(ix.lru_order(), vec![(Kind::Frame, 2), (Kind::Layer, 3), (Kind::Frame, 1)]);
        assert_eq!(ix.evict(250), vec![(Kind::Frame, 2)]);
        assert!(ix.evict(250).is_empty());
        ix.insert(Kind::Frame, 1, 50);
        assert_eq!(ix.total(), 150);
        assert_eq!(ix.remove(Kind::Layer, 3), Some(100));
        assert_eq!(ix.keys(Kind::Frame), vec![1]);
        ix.clear();
        assert!(ix.is_empty() && ix.total() == 0);
    }

    #[test]
    fn frame_entries_round_trip_and_names_parse() {
        let rgba: Vec<u8> = (0..4 * 6 * 3).map(|i| (i * 7 % 256) as u8).collect();
        let e = frame_entry(6, 3, &rgba);
        assert_eq!(read_frame_entry(&e), Some(Frame8 { width: 6, height: 3, rgba: rgba.clone() }));
        let mut bad = e.clone();
        bad[20] ^= 1;
        assert!(read_frame_entry(&bad).is_none());
        let key = 0x0123_4567_89ab_cdef_0011_2233_4455_6677u128;
        assert_eq!(parse_entry_name(&entry_name(key)), Some(key));
        assert_eq!(parse_entry_name("abc.ecc"), None);
        assert_eq!(parse_entry_name(&format!("{key:032x}.tmp.1")), None);
    }

    #[test]
    fn hit_miss_and_exact_round_trip() {
        let dir = tmpdir("hit");
        let dc = DiskCache::open(&dir, 1 << 30).unwrap();
        assert!(dc.get_layer(42).is_none());
        let b = buf(1, 33, 17);
        dc.put_layer(42, &b);
        dc.flush();
        let back = dc.get_layer(42).unwrap();
        assert_eq!(back.img, b.img);
        assert_eq!((back.offset, back.scale), (b.offset, b.scale));
        let rgba: Vec<u8> = (0..20 * 10 * 4).map(|i| (i % 251) as u8).collect();
        dc.put_frame(7, 20, 10, &rgba);
        dc.flush();
        assert_eq!(dc.get_frame(7).unwrap().rgba, rgba);
        let st = dc.stats();
        assert_eq!((st.entries, st.hits, st.misses, st.writes), (2, 2, 1, 2));
        // Wrong kind is a miss.
        assert!(dc.get_frame(42).is_none());
    }

    #[test]
    fn persists_across_restarts_and_clears() {
        let dir = tmpdir("persist");
        {
            let dc = DiskCache::open(&dir, 1 << 30).unwrap();
            dc.put_layer_now(1, &buf(1, 8, 8));
            dc.put_frame_now(2, 2, 2, &[9; 16]);
        }
        let dc = DiskCache::open(&dir, 1 << 30).unwrap();
        assert_eq!(dc.stats().entries, 2);
        assert!(dc.contains(Kind::Layer, 1) && dc.contains(Kind::Frame, 2));
        assert_eq!(dc.get_frame(2).unwrap().rgba, vec![9; 16]);
        dc.clear();
        assert_eq!(dc.stats().entries, 0);
        assert!(dc.get_frame(2).is_none());
        let again = DiskCache::open(&dir, 1 << 30).unwrap();
        assert_eq!(again.stats().entries, 0);
    }

    #[test]
    fn lru_eviction_respects_the_limit() {
        let dir = tmpdir("lru");
        let dc = DiskCache::open(&dir, u64::MAX).unwrap();
        for k in 0..6u128 {
            dc.put_layer_now(k, &buf(k as u32, 64, 64));
        }
        let per = dc.stats().bytes / 6;
        // Touch 0 so it is the most recently used.
        assert!(dc.get_layer(0).is_some());
        dc.set_max_bytes(per * 3 + per / 2);
        let st = dc.stats();
        assert!(st.bytes <= per * 3 + per / 2, "{st:?}");
        assert_eq!(st.entries, 3);
        assert!(dc.contains(Kind::Layer, 0), "recently used survives");
        assert!(!dc.contains(Kind::Layer, 1) && !dc.contains(Kind::Layer, 2) && !dc.contains(Kind::Layer, 3));
        assert_eq!(st.evictions, 3);
        // Files are really gone.
        let files = walk(&dir);
        assert_eq!(files, 3);
        // New writes keep evicting.
        dc.put_layer_now(99, &buf(9, 64, 64));
        assert!(dc.stats().bytes <= per * 3 + per / 2);
    }

    fn walk(d: &Path) -> usize {
        let mut n = 0;
        if let Ok(rd) = std::fs::read_dir(d) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    n += walk(&p);
                } else if p.extension().is_some_and(|x| x == "ecc") {
                    n += 1;
                }
            }
        }
        n
    }

    #[test]
    fn damaged_files_are_misses() {
        let dir = tmpdir("damaged");
        let dc = DiskCache::open(&dir, 1 << 30).unwrap();
        dc.put_frame_now(5, 4, 4, &[1; 64]);
        let p = dc.path(Kind::Frame, 5);
        let mut d = std::fs::read(&p).unwrap();
        let n = d.len();
        d[n / 2] ^= 0xff;
        std::fs::write(&p, d).unwrap();
        assert!(dc.get_frame(5).is_none());
        assert!(!dc.contains(Kind::Frame, 5));
        assert!(!p.exists());
    }

    #[test]
    fn concurrent_readers_and_writers() {
        let dir = tmpdir("concurrent");
        let a = DiskCache::open(&dir, 1 << 30).unwrap();
        // A second instance on the same folder (another process, in effect).
        let b = DiskCache::open(&dir, 1 << 30).unwrap();
        std::thread::scope(|s| {
            for t in 0..4u32 {
                let (a, b) = (a.clone(), b.clone());
                s.spawn(move || {
                    for i in 0..20u128 {
                        let k = i % 5;
                        let w = if t % 2 == 0 { &a } else { &b };
                        w.put_layer_now(k, &buf(k as u32, 16, 16));
                        if let Some(x) = w.get_layer(k) {
                            assert_eq!(x.img, buf(k as u32, 16, 16).img);
                        }
                    }
                });
            }
        });
        b.rescan();
        for k in 0..5u128 {
            assert_eq!(b.get_layer(k).unwrap().img, buf(k as u32, 16, 16).img);
        }
    }
}
