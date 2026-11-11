//! The persistent disk cache (Settings ▸ Media & Disk Cache ▸ Disk Cache): RAM-preview frames
//! and layer cache entries kept on disk between sessions, like After Effects' disk cache.
//!
//! * Entries are files keyed by a 128-bit content hash (`<folder>/v1/<kind>/<2 hex>/<32 hex>.ecc`):
//!   frames by the hash of the composition's content as it is in memory (the comp and what it can
//!   draw, footage items with their settings and paths, and what its expressions can read; no
//!   file is looked at, [`comp_content_key`]) plus frame, scale and view; layer buffers by the
//!   layer cache's content key (which names the footage items a buffer uses with their settings
//!   and paths) and the renderer version ([`layer_disk_key`]). A changed project therefore never
//!   reads stale pixels: its keys simply change, and an edit elsewhere keeps them.
//! * Frame and layer entries keep the source files they were made of (footage, and the font
//!   files text is drawn with), each with the size and modification time it was read with
//!   ([`EntryFiles`], #3184). An entry is used only while those files are unchanged: a handful
//!   of `metadata` calls when it is read, never a scan of the project's files. A frame or
//!   buffer whose files can't all be named is not stored (text in a family that isn't installed
//!   or with fallback glyphs, footage refused by automation read roots, …). File ▸ Reload
//!   Footage deletes the entries made of the reloaded files ([`DiskCache::drop_files`]): a file
//!   replaced by one of the same size and time is read again.
//! * Payloads are LZ4-compressed (`lz4_flex`, MIT): frames as 8-bit premultiplied RGBA (or, for
//!   32 bpc viewer frames, half-float RGBA with the bytes shuffled into planes; the entry
//!   header's format byte tells them apart: 2 and 3, both with their source files first), layer
//!   buffers (format 4, their source files first) as `f32` RGBA with the bytes shuffled into
//!   planes (exact, so cached and fresh renders are bit-identical), Roto Brush / Object Matte segmentations as
//!   `FrameSeg::to_bytes` (keyed by their content-keyed chain key and scale, so they are
//!   reused whenever the same frames, settings and prompt come back, also after a restart). Every file ends in a checksum; damaged files read as misses and
//!   are deleted.
//! * Size limit with least-recently-used eviction, down to 90% of the limit so the eviction scan
//!   runs once per tenth of the limit written, not on every write. The limit shrinks to keep
//!   free space on the cache's disk ([`DiskCache::set_disk_space`]). The in-memory index is
//!   rebuilt from the folder when the cache opens (file modification times order it), so the
//!   cache survives restarts; reads refresh a file's time.
//! * Safe concurrent access: writes go to a unique temporary file that is renamed into place
//!   (atomic on every desktop OS), so readers in this or another process never see a partial
//!   entry; a missing or damaged file is just a miss. Writes run on a background thread with a
//!   bounded queue (a full queue drops the write rather than stalling rendering).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use effectcraft_effects::Buf;
use effectcraft_raster::Image;

use crate::reads::ReadFiles;

mod entry_files;
pub use entry_files::{EntryFile, EntryFiles, EntryStamp};

/// File format version (part of the folder layout).
const VERSION: &str = "v1";
const MAGIC: &[u8; 4] = b"ECDC";
/// Pending layer and matte writes before new ones are dropped.
#[cfg(not(target_arch = "wasm32"))]
const QUEUE_LIMIT: usize = 64;
/// Pending frame bytes before new frames are dropped. Frames have their own lane, written
/// before layer buffers: a burst of large layer writes used to fill the shared queue and drop
/// frames, so a fully cached comp came back with gaps in its blue bar.
#[cfg(not(target_arch = "wasm32"))]
const FRAME_QUEUE_BYTES: usize = 1 << 30;
/// Pending layer and matte bytes before new ones are dropped: a layer job holds its float
/// buffer until the writer compresses it (64 of them at 4K were ~8.5 GB).
#[cfg(not(target_arch = "wasm32"))]
const LAYER_QUEUE_BYTES: usize = 1 << 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Frame,
    Layer,
    /// A Roto Brush / Object Matte segmentation.
    Matte,
}

impl Kind {
    /// The kind's folder name.
    pub fn dir(self) -> &'static str {
        match self {
            Kind::Frame => "frames",
            Kind::Layer => "layers",
            Kind::Matte => "mattes",
        }
    }
    fn tag(self) -> u8 {
        match self {
            Kind::Frame => 1,
            Kind::Layer => 2,
            Kind::Matte => 3,
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

/// A cached 32 bpc viewer frame: premultiplied RGBA half floats (IEEE binary16 bits), four
/// per pixel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameF16 {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u16>,
}

/// The entry header's format byte: segmentations (and 8-bit frames and layer buffers before
/// #3184).
const FORMAT_RGBA8: u8 = 0;
/// Frame entries: their source files ([`EntryFiles`]), then 8-bit or half-float pixels. The
/// formats before them (0: 8-bit, 1: half float, without files) read as misses and are deleted.
const FORMAT_RGBA8_FILES: u8 = 2;
const FORMAT_F16_FILES: u8 = 3;
/// Layer entries: their source files ([`EntryFiles`]), then the buffer. The format before it
/// (0, without files) reads as a miss and is deleted.
const FORMAT_LAYER_FILES: u8 = 4;

/// Counters for tests, the Info panel and `cache.diskStats`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DiskStats {
    pub entries: usize,
    pub bytes: u64,
    pub max_bytes: u64,
    /// The limit in effect: `max_bytes`, or less when the disk is short of free space.
    pub limit_bytes: u64,
    pub hits: u64,
    pub misses: u64,
    pub writes: u64,
    pub evictions: u64,
    pub dropped: u64,
    /// Writes that failed (a full disk, permissions).
    pub failed: u64,
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

    /// When the total exceeds `max` bytes, remove least recently used entries down to 90% of
    /// it (so the sort runs once per tenth of `max` written, not on every write); returns them
    /// (the caller deletes their files).
    pub fn evict(&mut self, max: u64) -> Vec<(Kind, u128)> {
        if self.total <= max {
            return vec![];
        }
        let target = max - max / 10;
        let mut out = vec![];
        for k in self.lru_order() {
            if self.total <= target {
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
    /// A frame's layer-cache misses (disk keys, [`layer_disk_key`]).
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
    /// Kind, key, header format byte, payload.
    Write(Kind, u128, u8, Payload),
    /// Delete the frame and layer entries made of these files ([`DiskCache::drop_files`]).
    Drop(Vec<Arc<str>>),
}

/// What a queued write holds. 8-bit frames and layer buffers are encoded (LZ4) on the writer
/// thread, not on the render thread that queued them: encoding a 1080p frame took ~9 ms of every
/// frame's render, and a full queue no longer throws away encoding work.
enum Payload {
    Encoded(Vec<u8>),
    /// 8-bit premultiplied RGBA, `width * height * 4` bytes, and the files it was made of.
    Frame(u32, u32, Vec<u8>, EntryFiles),
    Layer(Arc<Buf>, EntryFiles),
}

impl Payload {
    /// Bytes it holds (the frame queue's budget).
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    fn len(&self) -> usize {
        match self {
            Payload::Encoded(p) | Payload::Frame(_, _, p, _) => p.len(),
            Payload::Layer(b, _) => b.img.data.len().saturating_mul(std::mem::size_of::<effectcraft_raster::Px>()),
        }
    }

    /// The entry's payload: its source files, then its pixels.
    fn encode(self) -> Vec<u8> {
        match self {
            Payload::Encoded(p) => p,
            Payload::Frame(w, h, rgba, files) => with_files(&files, encode_frame(w, h, &rgba)),
            Payload::Layer(b, files) => with_files(&files, encode_layer(&b)),
        }
    }
}

struct Queue {
    /// Frame writes (taken first) and their payload bytes.
    frames: std::collections::VecDeque<Job>,
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    frame_bytes: usize,
    /// Layer and matte writes, and their payload bytes.
    jobs: std::collections::VecDeque<Job>,
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    job_bytes: usize,
    busy: bool,
    /// Keys of frames queued or being written.
    pending_frames: std::collections::HashSet<u128>,
    /// The drops so far (under the queue's lock: a write is queued either before a drop, which
    /// then sweeps it, or after it, when this refuses it if it was made from before).
    drops: Drops,
}

/// Files dropped from the cache files beyond this are not remembered one by one: everything made
/// before the drop that forgot them is refused instead.
const MAX_DROPPED_PATHS: usize = 1 << 16;

/// The [`DiskCache::drop_files`] calls so far: their number (the drop generation) and the last one
/// of each file. An entry made of a file read before that file's last drop (its render started at
/// an earlier generation) holds the pixels from before and is refused.
#[derive(Default)]
struct Drops {
    generation: u64,
    last: HashMap<Arc<str>, u64>,
    /// Everything made before this generation is refused (the per-file record was forgotten).
    floor: u64,
}

impl Drops {
    /// Notes that the given paths were dropped, bumping the generation and tracking them for later checks.
    fn record(&mut self, paths: &[Arc<str>]) {
        self.generation = self.generation.saturating_add(1);
        if self.last.len().saturating_add(paths.len()) > MAX_DROPPED_PATHS {
            self.last.clear();
            self.floor = self.generation;
            return;
        }
        for p in paths {
            self.last.insert(p.clone(), self.generation);
        }
    }

    /// Whether something made of `paths`, whose reads started at generation `since`, read one of
    /// them before it was dropped.
    fn stale<'a>(&self, mut paths: impl Iterator<Item = &'a Arc<str>>, since: u64) -> bool {
        since < self.floor || paths.any(|p| self.last.get(p).is_some_and(|g| *g > since))
    }
}

impl Queue {
    /// Takes the next queued job, frames first, and releases the bytes it held.
    #[cfg(not(target_arch = "wasm32"))]
    fn pop(&mut self) -> Option<Job> {
        if let Some(j) = self.frames.pop_front() {
            if let Job::Write(_, _, _, p) = &j {
                self.frame_bytes = self.frame_bytes.saturating_sub(p.len());
            }
            return Some(j);
        }
        let j = self.jobs.pop_front()?;
        if let Job::Write(_, _, _, p) = &j {
            self.job_bytes = self.job_bytes.saturating_sub(p.len());
        }
        Some(j)
    }

    /// No room for a `kind` write of `len` bytes. A layer bigger than the whole budget is still
    /// taken when nothing else is queued.
    #[cfg(not(target_arch = "wasm32"))]
    fn full(&self, kind: Kind, len: usize) -> bool {
        match kind {
            Kind::Frame => self.frame_bytes.saturating_add(len) > FRAME_QUEUE_BYTES,
            _ => self.jobs.len() >= QUEUE_LIMIT || (self.job_bytes > 0 && self.job_bytes.saturating_add(len) > LAYER_QUEUE_BYTES),
        }
    }
    fn is_empty(&self) -> bool {
        self.frames.is_empty() && self.jobs.is_empty()
    }
}

pub struct DiskCache {
    dir: PathBuf,
    index: Mutex<DiskIndex>,
    max_bytes: AtomicU64,
    /// The size free disk space allows (`u64::MAX` = unknown), see [`DiskCache::set_disk_space`].
    disk_cap: AtomicU64,
    /// Free space is limiting the cache (warned once per episode).
    space_limited: AtomicBool,
    /// The free space the cache leaves on its disk ([`disk_reserve`]); 0 = unknown.
    reserve: AtomicU64,
    hits: AtomicU64,
    misses: AtomicU64,
    writes: AtomicU64,
    evictions: AtomicU64,
    dropped: AtomicU64,
    failed: AtomicU64,
    /// Source files looked at to check frame and layer entries ([`DiskCache::file_checks`]).
    file_checks: AtomicU64,
    /// Frame and layer entries compressed for a queued write ([`DiskCache::encodes`]).
    encodes: AtomicU64,
    tmp_counter: AtomicU64,
    queue: Arc<(Mutex<Queue>, Condvar)>,
    /// Layer buffers that took less than this to render are not written (ms).
    min_layer_ms: AtomicU64,
    /// Files whose entries are being deleted ([`DiskCache::drop_files`]), each with the number
    /// of drops of it pending: until then an entry made of one is a miss.
    dropping: Mutex<HashMap<Arc<str>, u32>>,
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

/// The least free space the disk cache leaves on its disk.
pub const MIN_DISK_RESERVE: u64 = 2 << 30;
/// The most free space the disk cache leaves on its disk: 5% of a large disk is far more than
/// the system needs to keep working (on a 4 TB disk it held a 20 GB cache at 0 with 190 GB free).
pub const MAX_DISK_RESERVE: u64 = 10 << 30;
/// Below this much room left by free disk space, frame and layer writes are refused before they
/// are encoded ([`DiskCache::accepts_writes`]): a few frames would fill it, and the rest would be
/// compressed only to be refused.
const MIN_WRITE_ROOM: u64 = 64 << 20;

/// The free space the disk cache leaves on a disk of `total` bytes: 5% of it, at least 2 GB and
/// at most 10 GB. It keeps the cache from filling the disk; the cache's size is the user's
/// Maximum Disk Cache Size (After Effects has that maximum and no reserve).
pub fn disk_reserve(total: u64) -> u64 {
    (total / 20).clamp(MIN_DISK_RESERVE, MAX_DISK_RESERVE)
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
    // Without a home folder: a per-user folder in the shared temp folder, made private by `open`.
    let user = std::env::var("USER").or_else(|_| std::env::var("USERNAME")).unwrap_or_default();
    home.map(|h| h.join(".cache/effectcraft/disk-cache")).unwrap_or_else(|| std::env::temp_dir().join(format!("effectcraft-disk-cache-{user}")))
}

/// A cache folder in the shared temp folder must be ours alone: not a link someone placed there,
/// and private (setting the mode fails unless we own it), so another user cannot redirect the
/// cache's writes and clean-up or read its files.
#[cfg(unix)]
fn make_private(folder: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    // Judge the folder as written and as it resolves (a relative path, a link or an alias such as
    // /tmp -> /private/tmp can all land in the shared temp folder).
    // The shared roots: the process's temp folder (`TMPDIR`) and the system's own, which a cache
    // configured by path lands in whatever `TMPDIR` says.
    let roots: Vec<PathBuf> = [std::env::temp_dir(), PathBuf::from("/tmp"), PathBuf::from("/var/tmp")]
        .into_iter()
        .flat_map(|t| {
            let real = t.canonicalize().unwrap_or_else(|_| t.clone());
            [t, real]
        })
        .collect();
    let given = std::path::absolute(folder).unwrap_or_else(|_| folder.to_path_buf());
    let real = folder.canonicalize().unwrap_or_else(|_| given.clone());
    // The shared folder itself is not ours to change or to keep a cache in: refused.
    if [&given, &real].iter().any(|p| roots.contains(p)) {
        return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "the disk cache folder is the shared temp folder"));
    }
    // Only folders inside it are made private.
    if ![&given, &real].iter().any(|p| roots.iter().any(|t| p.starts_with(t) && **p != *t)) {
        return Ok(());
    }
    if std::fs::symlink_metadata(folder)?.file_type().is_symlink() {
        return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "the disk cache folder is a link"));
    }
    std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn make_private(_: &Path) -> std::io::Result<()> {
    Ok(())
}

impl DiskCache {
    /// Open (or create) the cache in `folder`, holding at most `max_bytes`.
    pub fn open(folder: impl AsRef<Path>, max_bytes: u64) -> std::io::Result<Arc<DiskCache>> {
        std::fs::create_dir_all(folder.as_ref())?;
        make_private(folder.as_ref())?;
        let dir = folder.as_ref().join(VERSION);
        std::fs::create_dir_all(&dir)?;
        let dc = Arc::new(DiskCache {
            dir,
            index: Mutex::new(DiskIndex::default()),
            max_bytes: AtomicU64::new(max_bytes),
            disk_cap: AtomicU64::new(u64::MAX),
            space_limited: AtomicBool::new(false),
            reserve: AtomicU64::new(0),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            writes: AtomicU64::new(0),
            evictions: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            failed: AtomicU64::new(0),
            file_checks: AtomicU64::new(0),
            encodes: AtomicU64::new(0),
            tmp_counter: AtomicU64::new(0),
            queue: Arc::new((
                Mutex::new(Queue {
                    frames: Default::default(),
                    frame_bytes: 0,
                    jobs: Default::default(),
                    job_bytes: 0,
                    busy: cfg!(not(target_arch = "wasm32")),
                    pending_frames: Default::default(),
                    drops: Drops::default(),
                }),
                Condvar::new(),
            )),
            min_layer_ms: AtomicU64::new(30),
            dropping: Mutex::new(HashMap::new()),
        });
        dc.rescan();
        // Shrinking a large cache deletes many files: the writer thread does it.
        #[cfg(target_arch = "wasm32")]
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
        for kind in [Kind::Frame, Kind::Layer, Kind::Matte] {
            let Ok(shards) = std::fs::read_dir(self.dir.join(kind.dir())) else { continue };
            for shard in shards.flatten() {
                let Ok(files) = std::fs::read_dir(shard.path()) else { continue };
                for f in files.flatten() {
                    let p = f.path();
                    let name = p.file_name().and_then(std::ffi::OsStr::to_str).unwrap_or("");
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

    /// The limit in effect: the setting, or less when the disk is short of free space.
    pub fn limit_bytes(&self) -> u64 {
        self.max_bytes().min(self.disk_cap.load(Ordering::Relaxed))
    }

    /// The cache's disk: `(total, available)` bytes, `None` = unknown. The cache leaves
    /// [`disk_reserve`] free (5% of the disk, at least 2 GB and at most 10 GB), evicting when it
    /// must.
    pub fn set_disk_space(&self, space: Option<(u64, u64)>) {
        let mut reserve = 0;
        let cap = match space {
            // (a disk reported as 0 bytes, such as an automount map, is unknown)
            Some((total, available)) if total > 0 => {
                reserve = disk_reserve(total);
                let used = self.index.lock().map(|g| g.total()).unwrap_or_default();
                // The cache's own files count as free: below the reserve it gives space back.
                used.saturating_add(available).saturating_sub(reserve)
            }
            _ => u64::MAX,
        };
        self.reserve.store(reserve, Ordering::Relaxed);
        self.disk_cap.store(cap, Ordering::Relaxed);
        let limited = cap < self.max_bytes();
        if self.space_limited.swap(limited, Ordering::Relaxed) != limited && limited {
            log::warn!("disk cache at {}: little free disk space, limiting the cache to {} MB", self.folder().display(), cap >> 20);
        }
        self.evict();
    }

    /// Whether frame and layer writes are worth encoding now: `false` while free disk space
    /// holds the limit at 0 or under 64 MB (a limit the setting asks for is always used). Two
    /// atomic reads, so writes check it before they spend time converting and compressing
    /// pixels that [`DiskCache::write_format`] would refuse anyway. (Whether a given entry fits
    /// is known only once it is compressed: `write_format` decides.)
    pub fn accepts_writes(&self) -> bool {
        let (max, limit) = (self.max_bytes(), self.limit_bytes());
        limit > 0 && (limit >= MIN_WRITE_ROOM || limit == max)
    }

    /// The free space the cache leaves on its disk ([`disk_reserve`]), once the disk's size is
    /// known ([`DiskCache::set_disk_space`]).
    pub fn reserve_bytes(&self) -> Option<u64> {
        Some(self.reserve.load(Ordering::Relaxed)).filter(|r| *r > 0)
    }

    /// Frame and layer entries compressed for writing so far (tests: a write refused up front
    /// costs no encode).
    pub fn encodes(&self) -> u64 {
        self.encodes.load(Ordering::Relaxed)
    }

    /// Count one entry compressed for a queued write.
    fn count_encode(&self) {
        self.encodes.fetch_add(1, Ordering::Relaxed);
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

    /// A frame write is queued or in progress.
    pub fn frame_pending(&self, key: u128) -> bool {
        self.queue.0.lock().is_ok_and(|q| q.pending_frames.contains(&key))
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
            limit_bytes: self.limit_bytes(),
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            writes: self.writes.load(Ordering::Relaxed),
            evictions: self.evictions.load(Ordering::Relaxed),
            dropped: self.dropped.load(Ordering::Relaxed),
            failed: self.failed.load(Ordering::Relaxed),
        }
    }

    /// Delete every entry (Edit ▸ Purge ▸ All Disk Cache / Empty Disk Cache).
    pub fn clear(&self) {
        self.flush();
        if let Ok(mut g) = self.index.lock() {
            g.clear();
        }
        for kind in [Kind::Frame, Kind::Layer, Kind::Matte] {
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
            q.job_bytes = 0;
            q.frames.clear();
            q.pending_frames.clear();
        }
        while !q.is_empty() || q.busy {
            q = match cv.wait(q) {
                Ok(q) => q,
                Err(_) => return,
            };
        }
    }

    fn read(&self, kind: Kind, key: u128) -> Option<Vec<u8>> {
        self.read_entry(kind, key).map(|(_, p)| p)
    }

    /// An entry's payload and its header's format byte.
    fn read_entry(&self, kind: Kind, key: u128) -> Option<(u8, Vec<u8>)> {
        let entry = self.load(kind, key);
        match &entry {
            Some(_) => self.hit(kind, key),
            None => _ = self.misses.fetch_add(1, Ordering::Relaxed),
        }
        entry
    }

    /// An entry's payload and format byte, without counting it as a hit or miss. A damaged file
    /// is deleted; a missing one forgotten.
    fn load(&self, kind: Kind, key: u128) -> Option<(u8, Vec<u8>)> {
        if !self.contains(kind, key) {
            return None;
        }
        let path = self.path(kind, key);
        let data = std::fs::read(&path).ok();
        let ok = data.as_deref().and_then(|d| Some((*d.get(5)?, verify(d, kind)?.to_vec())));
        if ok.is_none() {
            // Gone (another process evicted it) or damaged: forget it.
            if data.is_some() {
                let _ = std::fs::remove_file(&path);
            }
            self.forget(kind, key);
        }
        ok
    }

    /// Count a hit: the entry becomes the most recently used, also for the next session.
    fn hit(&self, kind: Kind, key: u128) {
        self.hits.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut g) = self.index.lock() {
            g.touch(kind, key);
        }
        // Persist the recency for the next session (best effort).
        if let Ok(f) = std::fs::File::options().append(true).open(self.path(kind, key)) {
            let _ = f.set_modified(std::time::SystemTime::now());
        }
    }

    /// An entry of `kind` and `format` whose source files are unchanged, decoded by `decode`,
    /// with those files. An entry whose files changed, of the format before #3184, or damaged,
    /// is a miss and is deleted (it renders again and is stored anew). A frame entry of the
    /// other current frame format (8-bit and half-float frames have keys of their own) is a miss
    /// and stays.
    fn get_checked<T>(
        &self,
        kind: Kind,
        key: u128,
        format: u8,
        virtual_id: Option<VirtualFileId<'_>>,
        decode: impl FnOnce(&[u8]) -> Option<T>,
    ) -> Option<(T, ReadFiles)> {
        let miss = || {
            self.misses.fetch_add(1, Ordering::Relaxed);
            None
        };
        // (before the file is read: the writer may finish a drop's sweep between the read and the
        // check below, and a drop may start meanwhile)
        let drops = self.drops_now();
        let Some((found, payload)) = self.load(kind, key) else { return miss() };
        if found != format {
            let other_frame_format = kind == Kind::Frame && (found == FORMAT_RGBA8_FILES || found == FORMAT_F16_FILES);
            if !other_frame_format {
                self.discard(kind, key);
            }
            return miss();
        }
        let Some((files, body)) = EntryFiles::decode(&payload) else {
            self.discard(kind, key);
            return miss();
        };
        self.file_checks.fetch_add(files.len() as u64, Ordering::Relaxed);
        if self.made_of_dropped(&files, &drops) || !files.unchanged(virtual_id) {
            self.discard(kind, key);
            return miss();
        }
        let Some(value) = decode(body) else {
            self.discard(kind, key);
            return miss();
        };
        self.hit(kind, key);
        Some((value, files.to_read_files()))
    }

    /// Source files looked at so far to check frame and layer entries before using them (one per
    /// file a read entry lists; tests).
    pub fn file_checks(&self) -> u64 {
        self.file_checks.load(Ordering::Relaxed)
    }

    /// A valid file whose payload does not decode: delete it, as a damaged one.
    fn discard(&self, kind: Kind, key: u128) {
        let _ = std::fs::remove_file(self.path(kind, key));
        self.forget(kind, key);
    }

    fn forget(&self, kind: Kind, key: u128) {
        if let Ok(mut g) = self.index.lock() {
            g.remove(kind, key);
        }
    }

    /// Queue a write; `false` when it was dropped (a full queue).
    fn enqueue(&self, kind: Kind, key: u128, payload: Vec<u8>) -> bool {
        self.enqueue_format(kind, key, FORMAT_RGBA8, Payload::Encoded(payload), None)
    }

    /// [`DiskCache::enqueue`] with the entry's header format byte. `made`: the source files of a
    /// frame or layer entry and the drop generation its reads started at
    /// ([`DiskCache::drop_generation`]); refused (`false`) when one of them was dropped since.
    fn enqueue_format(&self, kind: Kind, key: u128, format: u8, payload: Payload, made: Option<(&EntryFiles, u64)>) -> bool {
        let stale = |q: &Queue| made.is_some_and(|(files, since)| q.drops.stale(files.files().iter().map(|f| &f.path), since));
        #[cfg(target_arch = "wasm32")]
        {
            if self.queue.0.lock().is_ok_and(|q| stale(&q)) {
                return false;
            }
            self.write_format(kind, key, format, &payload.encode());
            true
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (m, cv) = &*self.queue;
            let Ok(mut q) = m.lock() else { return false };
            if stale(&q) {
                return false;
            }
            if q.full(kind, payload.len()) {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                return false;
            }
            if kind == Kind::Frame {
                q.pending_frames.insert(key);
                q.frame_bytes = q.frame_bytes.saturating_add(payload.len());
                q.frames.push_back(Job::Write(kind, key, format, payload));
            } else {
                q.job_bytes = q.job_bytes.saturating_add(payload.len());
                q.jobs.push_back(Job::Write(kind, key, format, payload));
            }
            cv.notify_all();
            true
        }
    }

    /// Write an entry now (atomic rename), then evict down to the limit.
    fn write_format(&self, kind: Kind, key: u128, format: u8, payload: &[u8]) {
        if self.contains(kind, key) {
            return;
        }
        let path = self.path(kind, key);
        let Some(parent) = path.parent() else { return };
        if let Err(e) = std::fs::create_dir_all(parent) {
            self.write_failed(&e);
            return;
        }
        let data = seal_format(kind, format, payload);
        // An entry that cannot fit would be evicted straight away.
        if data.len() as u64 > self.limit_bytes() {
            return;
        }
        // Make room first, so the cache never exceeds its limit (or the free-space reserve).
        self.evict_to(self.limit_bytes().saturating_sub(data.len() as u64));
        let n = self.tmp_counter.fetch_add(1, Ordering::Relaxed);
        let tmp = parent.join(format!("{key:032x}.tmp.{}.{n}", std::process::id()));
        if let Err(e) = std::fs::write(&tmp, &data).and_then(|()| std::fs::rename(&tmp, &path)) {
            let _ = std::fs::remove_file(&tmp);
            self.write_failed(&e);
            return;
        }
        self.writes.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut g) = self.index.lock() {
            g.insert(kind, key, data.len() as u64);
        }
    }

    /// Count a failed write; the first one is logged (a full disk shouldn't go unnoticed).
    fn write_failed(&self, e: &std::io::Error) {
        if self.failed.fetch_add(1, Ordering::Relaxed) == 0 {
            log::warn!("disk cache at {}: a write failed: {e}", self.folder().display());
        }
    }

    /// Remove least recently used entries until the cache fits its limit.
    fn evict(&self) {
        self.evict_to(self.limit_bytes());
    }

    /// Remove least recently used entries until the cache fits `max` bytes.
    fn evict_to(&self, max: u64) {
        let victims = match self.index.lock() {
            Ok(mut g) => g.evict(max),
            Err(_) => return,
        };
        for (kind, key) in victims {
            let _ = std::fs::remove_file(self.path(kind, key));
            self.evictions.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Delete the frame and layer entries made of any of `paths` (File ▸ Reload Footage: a file
    /// replaced by one with the same size and modification time must be read again, which the
    /// check on hit can't tell). The index knows keys, not files, so the writer thread reads the
    /// files section at the start of each frame and layer entry (no pixels) and deletes the
    /// entries that list one of `paths`, after the writes queued before (those are swept too);
    /// until it is done, an entry made of one of them is a miss. Entries are deleted for good,
    /// so they don't come back after a restart. Nothing else in the cache is touched.
    ///
    /// A render that was under way meanwhile may still offer an entry made of one of `paths` as
    /// it was before: it is refused, see [`DiskCache::drop_generation`].
    pub fn drop_files(&self, paths: impl IntoIterator<Item = impl Into<Arc<str>>>) {
        let paths: Vec<Arc<str>> = paths.into_iter().map(Into::into).collect();
        if paths.is_empty() {
            return;
        }
        {
            let mut d = self.dropping.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            for p in &paths {
                let n = d.entry(p.clone()).or_default();
                *n = n.saturating_add(1);
            }
        }
        #[cfg_attr(target_arch = "wasm32", allow(unused_variables))]
        let (m, cv) = &*self.queue;
        match m.lock() {
            Ok(mut q) => {
                q.drops.record(&paths);
                #[cfg(not(target_arch = "wasm32"))]
                {
                    // (never refused for a full queue)
                    q.jobs.push_back(Job::Drop(paths));
                    cv.notify_all();
                    return;
                }
            }
            // (no writer thread to take the job any more)
            Err(poisoned) => poisoned.into_inner().drops.record(&paths),
        }
        self.sweep(&paths);
    }

    /// The number of [`DiskCache::drop_files`] calls so far. A render takes it before it starts
    /// reading and hands it to `put_frame` / `put_layer` with what it made: an entry listing a
    /// file dropped after that is refused (the render may have read the file as it was before).
    pub fn drop_generation(&self) -> u64 {
        self.queue.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner).drops.generation
    }

    /// Whether one of `files` was dropped after generation `since` ([`DiskCache::drop_generation`]),
    /// or after the earlier generation the list's own reads started at ([`ReadFiles::since`]: a
    /// buffer made by an earlier render, from memory): what was made of them then is from before
    /// the drop (shown, never stored).
    pub fn dropped_since(&self, files: &ReadFiles, since: u64) -> bool {
        let since = files.since().map_or(since, |s| s.min(since));
        self.queue.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner).drops.stale(files.files().iter().map(|f| &f.path), since)
    }

    /// Whether a [`DiskCache::drop_files`] is pending.
    pub fn is_dropping(&self) -> bool {
        !self.dropping.lock().unwrap_or_else(std::sync::PoisonError::into_inner).is_empty()
    }

    /// The drop generation and the files being dropped now, taken before an entry is read
    /// (see [`DiskCache::made_of_dropped`]).
    fn drops_now(&self) -> (u64, Vec<Arc<str>>) {
        let generation = self.drop_generation();
        let d = self.dropping.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        (generation, d.keys().cloned().collect())
    }

    /// An entry read after [`DiskCache::drops_now`] gave `drops` that is made of a file whose
    /// drop was pending then (its sweep may have ended since, after the entry was read) or that
    /// was dropped since.
    fn made_of_dropped(&self, files: &EntryFiles, drops: &(u64, Vec<Arc<str>>)) -> bool {
        let (since, pending) = drops;
        let paths = || files.files().iter().map(|f| &f.path);
        paths().any(|p| pending.contains(p)) || self.queue.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner).drops.stale(paths(), *since)
    }

    /// The writer's part of [`DiskCache::drop_files`]: delete the entries made of `paths`, then
    /// end their drop.
    fn sweep(&self, paths: &[Arc<str>]) {
        let set: std::collections::HashSet<&str> = paths.iter().map(|p| &**p).collect();
        for kind in [Kind::Frame, Kind::Layer] {
            for key in self.keys(kind) {
                if self.entry_files(kind, key).is_some_and(|f| f.files().iter().any(|f| set.contains(&*f.path))) {
                    self.discard(kind, key);
                }
            }
        }
        let mut d = self.dropping.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        for p in paths {
            if let Some(n) = d.get_mut(p) {
                *n = n.saturating_sub(1);
                if *n == 0 {
                    d.remove(p);
                }
            }
        }
    }

    /// The source files a frame or layer entry lists, read from the start of its file (the
    /// whole file only when the list is longer than that). `None` when it has none to read (an
    /// earlier format, gone, damaged): such an entry is never used anyway.
    fn entry_files(&self, kind: Kind, key: u128) -> Option<EntryFiles> {
        use std::io::Read;
        const HEAD: u64 = 64 << 10;
        let parse = |d: &[u8]| {
            let format = *d.get(5)?;
            let ours = d.get(..4)? == MAGIC && *d.get(4)? == kind.tag();
            if !ours || !matches!(format, FORMAT_RGBA8_FILES | FORMAT_F16_FILES | FORMAT_LAYER_FILES) {
                return None;
            }
            EntryFiles::decode(d.get(8..)?).map(|(files, _)| files)
        };
        let path = self.path(kind, key);
        let mut head = Vec::new();
        std::fs::File::open(&path).ok()?.take(HEAD).read_to_end(&mut head).ok()?;
        parse(&head).or_else(|| if head.len() as u64 == HEAD { parse(&std::fs::read(&path).ok()?) } else { None })
    }

    // ------------------------------------------------------------ typed entries

    /// An 8-bit frame whose source files are unchanged (see [`DiskCache::get_checked`]), with
    /// those files; `None` for a half-float entry of that key, which
    /// [`DiskCache::get_frame_f16`] reads. `virtual_id` names files served from memory.
    pub fn get_frame(&self, key: u128, virtual_id: Option<VirtualFileId<'_>>) -> Option<(Frame8, ReadFiles)> {
        self.get_checked(Kind::Frame, key, FORMAT_RGBA8_FILES, virtual_id, decode_frame)
    }

    /// A half-float frame (32 bpc viewer frames), as [`DiskCache::get_frame`].
    pub fn get_frame_f16(&self, key: u128, virtual_id: Option<VirtualFileId<'_>>) -> Option<(FrameF16, ReadFiles)> {
        self.get_checked(Kind::Frame, key, FORMAT_F16_FILES, virtual_id, decode_frame_f16)
    }

    /// Queue a half-float frame for writing (`width * height * 4` binary16 values) with the
    /// source files it was made of. `true` when it is on disk or queued; `false` when refused
    /// (wrong size, a full queue, no room on the disk, files that can't be checked later: see
    /// [`EntryFiles::of`]; a file dropped after `since`, the [`DiskCache::drop_generation`] the
    /// render started at).
    #[allow(clippy::too_many_arguments)]
    pub fn put_frame_f16(
        &self,
        key: u128,
        width: u32,
        height: u32,
        rgba: &[u16],
        files: &ReadFiles,
        virtual_id: Option<VirtualFileId<'_>>,
        since: u64,
    ) -> bool {
        if rgba.len() as u64 != width as u64 * height as u64 * 4 {
            return false;
        }
        let Some(files) = EntryFiles::of(files, virtual_id) else { return false };
        if self.contains(Kind::Frame, key) {
            return true;
        }
        // (no room on the disk: refused before the pixels are compressed)
        if !self.accepts_writes() {
            return false;
        }
        self.count_encode();
        let payload = with_files(&files, encode_frame_f16(width, height, rgba));
        self.enqueue_format(Kind::Frame, key, FORMAT_F16_FILES, Payload::Encoded(payload), Some((&files, since)))
    }

    /// Write a half-float frame synchronously (tests); a list that can't be stored is not.
    pub fn put_frame_f16_now(&self, key: u128, width: u32, height: u32, rgba: &[u16], files: &ReadFiles) {
        if let Some(files) = EntryFiles::of(files, None) {
            self.write_format(Kind::Frame, key, FORMAT_F16_FILES, &with_files(&files, encode_frame_f16(width, height, rgba)));
        }
    }

    /// Queue a frame for writing (8-bit premultiplied RGBA, `width * height * 4` bytes) with the
    /// source files it was made of. `true` when it is on disk or queued; `false` when refused
    /// (wrong size, a full queue, no room on the disk, files that can't be checked later: see
    /// [`EntryFiles::of`]; a file dropped after `since`, the [`DiskCache::drop_generation`] the
    /// render started at).
    #[allow(clippy::too_many_arguments)]
    pub fn put_frame(&self, key: u128, width: u32, height: u32, rgba: &[u8], files: &ReadFiles, virtual_id: Option<VirtualFileId<'_>>, since: u64) -> bool {
        self.put_frame_owned(key, width, height, rgba.to_vec(), files, virtual_id, since)
    }

    /// [`DiskCache::put_frame`] of a frame the caller hands over (no copy; compressed on the
    /// writer thread).
    #[allow(clippy::too_many_arguments)]
    pub fn put_frame_owned(
        &self,
        key: u128,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
        files: &ReadFiles,
        virtual_id: Option<VirtualFileId<'_>>,
        since: u64,
    ) -> bool {
        if rgba.len() as u64 != width as u64 * height as u64 * 4 {
            return false;
        }
        let Some(files) = EntryFiles::of(files, virtual_id) else { return false };
        if self.contains(Kind::Frame, key) {
            return true;
        }
        // (no room on the disk: refused before the pixels are queued and compressed)
        if !self.accepts_writes() {
            return false;
        }
        self.count_encode();
        let made = files.clone();
        self.enqueue_format(Kind::Frame, key, FORMAT_RGBA8_FILES, Payload::Frame(width, height, rgba, files), Some((&made, since)))
    }

    /// A layer buffer (key: [`layer_disk_key`]) whose source files are unchanged, with those
    /// files, as [`DiskCache::get_checked`]: an entry whose files changed, damaged, or of the
    /// format before #3184 is a miss and is deleted, so the buffer renders again and replaces it.
    /// `virtual_id` names files served from memory.
    pub fn get_layer(&self, key: u128, virtual_id: Option<VirtualFileId<'_>>) -> Option<(Buf, ReadFiles)> {
        self.get_checked(Kind::Layer, key, FORMAT_LAYER_FILES, virtual_id, decode_layer)
    }

    /// Queue a layer buffer for writing with the source files it was made of (see
    /// [`EntryFiles::of`]: a buffer whose files can't all be named must not be stored). Refused
    /// when one of them was dropped after `since`, the [`DiskCache::drop_generation`] its render
    /// started at.
    /// The buffer is shared, and encoded on the writer thread.
    pub fn put_layer(&self, key: u128, buf: &Arc<Buf>, files: &EntryFiles, since: u64) {
        // (no room on the disk, or a buffer no entry could hold: refused before it is queued and
        // compressed)
        if !layer_fits(buf.img.data.len()) || self.contains(Kind::Layer, key) || !self.accepts_writes() {
            return;
        }
        self.count_encode();
        let _ = self.enqueue_format(Kind::Layer, key, FORMAT_LAYER_FILES, Payload::Layer(buf.clone(), files.clone()), Some((files, since)));
    }

    /// Write a layer buffer synchronously (tests); a list that can't be stored is not.
    pub fn put_layer_now(&self, key: u128, buf: &Buf, files: &ReadFiles) {
        if !layer_fits(buf.img.data.len()) {
            return;
        }
        if let Some(files) = EntryFiles::of(files, None) {
            self.write_format(Kind::Layer, key, FORMAT_LAYER_FILES, &with_files(&files, encode_layer(buf)));
        }
    }

    /// A segmentation (chain key, scale) written by [`DiskCache::put_seg`].
    pub fn get_seg(&self, key: u64, scale: f64) -> Option<effectcraft_effects::roto::FrameSeg> {
        let p = self.read(Kind::Matte, seg_key(key, scale))?;
        let seg = decompress_checked(&p, None).and_then(|raw| effectcraft_effects::roto::FrameSeg::from_bytes(&raw));
        if seg.is_none() {
            // A rejected entry would otherwise stay indexed and keep `put_seg` from replacing it.
            self.discard(Kind::Matte, seg_key(key, scale));
        }
        seg
    }

    /// Queue a segmentation for writing.
    pub fn put_seg(&self, key: u64, scale: f64, seg: &effectcraft_effects::roto::FrameSeg) {
        let k = seg_key(key, scale);
        if self.contains(Kind::Matte, k) {
            return;
        }
        let _ = self.enqueue(Kind::Matte, k, lz4_flex::compress_prepend_size(&seg.to_bytes()));
    }

    /// Make the frame at key `from` the frame at key `to` too (an edit that can't change it gave
    /// its comp a new content key): a hard link to the same file, or a copy where the file system
    /// has no links. `false` when `from` isn't cached, `to` already is, or the file system
    /// refused. (Entries don't contain their key, so the same file serves both.)
    pub fn link_frame(&self, from: u128, to: u128) -> bool {
        // (while files are being dropped, `from` may be made of one: it renders again instead)
        if !self.contains(Kind::Frame, from) || self.contains(Kind::Frame, to) || self.is_dropping() {
            return false;
        }
        let (src, dst) = (self.path(Kind::Frame, from), self.path(Kind::Frame, to));
        let Some(parent) = dst.parent() else { return false };
        let n = self.tmp_counter.fetch_add(1, Ordering::Relaxed);
        let tmp = parent.join(format!("{to:032x}.tmp.{}.{n}", std::process::id()));
        let linked = std::fs::create_dir_all(parent)
            .and_then(|()| std::fs::hard_link(&src, &tmp).or_else(|_| std::fs::copy(&src, &tmp).map(|_| ())))
            .and_then(|()| std::fs::metadata(&tmp))
            .and_then(|m| std::fs::rename(&tmp, &dst).map(|()| m.len()));
        let bytes = match linked {
            Ok(bytes) => bytes,
            Err(_) => {
                // (gone meanwhile: another process evicted it)
                let _ = std::fs::remove_file(&tmp);
                return false;
            }
        };
        // (a link shares its file, but counts in full: the cache only ever errs small)
        self.evict_to(self.limit_bytes().saturating_sub(bytes));
        if let Ok(mut g) = self.index.lock() {
            g.insert(Kind::Frame, to, bytes);
        }
        true
    }

    /// Write a frame synchronously (tests); a list that can't be stored is not.
    pub fn put_frame_now(&self, key: u128, width: u32, height: u32, rgba: &[u8], files: &ReadFiles) {
        if let Some(files) = EntryFiles::of(files, None) {
            self.write_format(Kind::Frame, key, FORMAT_RGBA8_FILES, &with_files(&files, encode_frame(width, height, rgba)));
        }
    }
}

/// A frame or layer entry's payload: its source files, then its pixels.
fn with_files(files: &EntryFiles, pixels: Vec<u8>) -> Vec<u8> {
    let mut out = Vec::with_capacity(pixels.len() + 64);
    files.encode(&mut out);
    out.extend_from_slice(&pixels);
    out
}

/// The writer thread: evicts to the limit at open, then drains the job queue until the cache is dropped.
#[cfg(not(target_arch = "wasm32"))]
fn writer(dc: std::sync::Weak<DiskCache>, q: Arc<(Mutex<Queue>, Condvar)>) {
    let (m, cv) = &*q;
    // The eviction at open (a lower limit than the folder holds), off the opening thread;
    // `busy` starts set, so `flush` waits for it.
    if let Some(cache) = dc.upgrade() {
        cache.evict();
    }
    if let Ok(mut g) = m.lock() {
        g.busy = false;
    }
    cv.notify_all();
    loop {
        let job = {
            let Ok(mut g) = m.lock() else { return };
            loop {
                if let Some(j) = g.pop() {
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
        let written = match job {
            Job::Write(kind, key, format, payload) => {
                cache.write_format(kind, key, format, &payload.encode());
                (kind == Kind::Frame).then_some(key)
            }
            Job::Drop(paths) => {
                cache.sweep(&paths);
                None
            }
        };
        drop(cache);
        if let Ok(mut g) = m.lock() {
            g.busy = false;
            if let Some(key) = written {
                g.pending_frames.remove(&key);
            }
        }
        cv.notify_all();
    }
}

/// An entry file's bytes: header (magic, kind, the format byte: [`FORMAT_RGBA8`] for
/// segmentations, [`FORMAT_RGBA8_FILES`] / [`FORMAT_F16_FILES`] for frames,
/// [`FORMAT_LAYER_FILES`] for layer buffers), payload, checksum.
fn seal_format(kind: Kind, format: u8, payload: &[u8]) -> Vec<u8> {
    let mut data = Vec::with_capacity(payload.len() + 16);
    data.extend_from_slice(MAGIC);
    data.push(kind.tag());
    data.extend_from_slice(&[format, 0, 0]);
    data.extend_from_slice(payload);
    let sum = checksum(&data);
    data.extend_from_slice(&sum.to_le_bytes());
    data
}

/// A sealed 8-bit frame entry with the source files it was made of: what the browser's frame
/// workers write to the Origin Private File System. `None` when the frame must not be stored
/// (its files can't be checked later, see [`EntryFiles::of`]; or a wrong size).
pub fn frame_entry(width: u32, height: u32, rgba: &[u8], files: &ReadFiles, virtual_id: Option<VirtualFileId<'_>>) -> Option<Vec<u8>> {
    if rgba.len() as u64 != width as u64 * height as u64 * 4 {
        return None;
    }
    let files = EntryFiles::of(files, virtual_id)?;
    Some(seal_format(Kind::Frame, FORMAT_RGBA8_FILES, &with_files(&files, encode_frame(width, height, rgba))))
}

/// The frame of a sealed entry's bytes, if its source files are unchanged. `None` when damaged,
/// of another format, or made of files that changed since: a miss, to be rendered again.
pub fn read_frame_entry(data: &[u8], virtual_id: Option<VirtualFileId<'_>>) -> Option<Frame8> {
    let payload = verify(data, Kind::Frame)?;
    if data.get(5) != Some(&FORMAT_RGBA8_FILES) {
        return None;
    }
    let (files, pixels) = EntryFiles::decode(payload)?;
    if !files.unchanged(virtual_id) {
        return None;
    }
    decode_frame(pixels)
}

/// A sealed layer-buffer entry with the source files it was made of: what the browser's frame
/// workers write to the Origin Private File System (`layers/<32 hex>.ecc`).
pub fn layer_entry(buf: &Buf, files: &EntryFiles) -> Vec<u8> {
    seal_format(Kind::Layer, FORMAT_LAYER_FILES, &with_files(files, encode_layer(buf)))
}

/// A layer buffer and its source files from a sealed entry's bytes (`None` when damaged or of
/// another format). The files are not looked at here: the store checks them when the buffer is
/// used ([`EntryFiles::unchanged`]).
pub fn read_layer_entry(data: &[u8]) -> Option<(Buf, EntryFiles)> {
    let payload = verify(data, Kind::Layer)?;
    if data.get(5) != Some(&FORMAT_LAYER_FILES) {
        return None;
    }
    let (files, body) = EntryFiles::decode(payload)?;
    Some((decode_layer(body)?, files))
}

/// The disk key of a segmentation: its chain key and scale.
fn seg_key(key: u64, scale: f64) -> u128 {
    let mut h = Hash128::default();
    h.write(b"segmentation-v2");
    h.write_u64(key);
    h.write_u64(scale.to_bits());
    h.finish()
}

impl effectcraft_effects::roto::SegStore for DiskCache {
    fn get_seg(&self, key: u64, scale: f64) -> Option<effectcraft_effects::roto::FrameSeg> {
        DiskCache::get_seg(self, key, scale)
    }
    fn put_seg(&self, key: u64, scale: f64, seg: &effectcraft_effects::roto::FrameSeg) {
        DiskCache::put_seg(self, key, scale, seg);
    }
}

impl crate::cache::LayerStore for DiskCache {
    /// The layer for `key` from the disk cache, or `None` when it is missing or stale.
    fn get_layer(&self, key: u128, virtual_id: Option<VirtualFileId<'_>>) -> Option<(Arc<Buf>, ReadFiles)> {
        DiskCache::get_layer(self, key, virtual_id).map(|(b, f)| (Arc::new(b), f))
    }
    /// Queues a layer write on the disk cache.
    fn put_layer(&self, key: u128, buf: &Arc<Buf>, files: &EntryFiles, since: u64) {
        DiskCache::put_layer(self, key, buf, files, since);
    }
    /// The current drop generation of the disk cache.
    fn drop_generation(&self) -> u64 {
        DiskCache::drop_generation(self)
    }
    fn dropped_since(&self, files: &ReadFiles, since: u64) -> bool {
        DiskCache::dropped_since(self, files, since)
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

/// The most bytes one cache entry decompresses to (a 16384² frame of 8-bit RGBA): the size an
/// entry states is checked against it, and against what the entry's dimensions need, before
/// anything is allocated.
const MAX_ENTRY_BYTES: usize = 1 << 30;

/// An LZ4 payload with its stated size prepended, decompressed only when that size is
/// `expected` (any size up to [`MAX_ENTRY_BYTES`] when `None`).
fn decompress_checked(p: &[u8], expected: Option<usize>) -> Option<Vec<u8>> {
    let size = u32::from_le_bytes(p.get(..4)?.try_into().ok()?) as usize;
    if size > MAX_ENTRY_BYTES || expected.is_some_and(|e| e != size) {
        return None;
    }
    // A block that ends short of the size it states is damaged: callers index by that size.
    lz4_flex::decompress(p.get(4..)?, size).ok().filter(|raw| raw.len() == size)
}

fn decode_frame(p: &[u8]) -> Option<Frame8> {
    if p.len() < 8 {
        return None;
    }
    let w = u32::from_le_bytes(p[0..4].try_into().ok()?);
    let h = u32::from_le_bytes(p[4..8].try_into().ok()?);
    let rgba = decompress_checked(&p[8..], Some((w as usize).checked_mul(h as usize)?.checked_mul(4)?))?;
    (rgba.len() == w as usize * h as usize * 4).then_some(Frame8 { width: w, height: h, rgba })
}

/// Whether a layer of `pixels` pixels (16 bytes each, decompressed) is within what a cache entry
/// may hold: a larger one could never be read back, so it is not compressed or written.
fn layer_fits(pixels: usize) -> bool {
    pixels.checked_mul(16).is_some_and(|b| b <= MAX_ENTRY_BYTES)
}

/// Half floats with their low and high bytes in planes (per channel), then LZ4: the high bytes
/// (sign, exponent) repeat a lot, mixed with the low ones LZ4 finds little.
fn encode_frame_f16(w: u32, h: u32, rgba: &[u16]) -> Vec<u8> {
    use rayon::prelude::*;
    let n = rgba.len() / 4;
    let mut planes = vec![0u8; n * 8];
    // Plane (byte k, channel c) at (k · 4 + c) · n.
    planes.par_chunks_mut(n.max(1)).enumerate().for_each(|(plane, out)| {
        let (k, c) = (plane / 4, plane % 4);
        for (o, px) in out.iter_mut().zip(rgba.as_chunks::<4>().0) {
            *o = px[c].to_le_bytes()[k];
        }
    });
    let mut out = Vec::with_capacity(planes.len() / 3 + 16);
    out.extend_from_slice(&w.to_le_bytes());
    out.extend_from_slice(&h.to_le_bytes());
    out.extend_from_slice(&lz4_flex::compress_prepend_size(&planes));
    out
}

fn decode_frame_f16(p: &[u8]) -> Option<FrameF16> {
    if p.len() < 8 {
        return None;
    }
    let w = u32::from_le_bytes(p[0..4].try_into().ok()?);
    let h = u32::from_le_bytes(p[4..8].try_into().ok()?);
    let n = (w as usize).checked_mul(h as usize)?;
    let planes = decompress_checked(&p[8..], Some(n.checked_mul(8)?))?;
    let mut rgba = vec![0u16; n * 4];
    for (i, px) in rgba.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        for (c, v) in px.iter_mut().enumerate() {
            *v = u16::from_le_bytes([planes[c * n + i], planes[(4 + c) * n + i]]);
        }
    }
    Some(FrameF16 { width: w, height: h, rgba })
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

/// Decodes a stored layer payload into a buffer, or `None` when it is too short or damaged.
fn decode_layer(p: &[u8]) -> Option<Buf> {
    if p.len() < 32 {
        return None;
    }
    let w = u32::from_le_bytes(p[0..4].try_into().ok()?);
    let h = u32::from_le_bytes(p[4..8].try_into().ok()?);
    let f = |i: usize| -> Option<f64> { Some(f64::from_le_bytes(p[i..i + 8].try_into().ok()?)) };
    let offset = [f(8)?, f(16)?];
    let scale = f(24)?;
    let n = (w as usize).checked_mul(h as usize)?;
    let planes = decompress_checked(&p[32..], Some(n.checked_mul(16)?))?;
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
const RENDER_VERSION: &str = "effectcraft-render-7";

/// Feeds the `Debug` form of `v` into the hash, followed by a separator byte.
fn hash_debug(h: &mut Hash128, v: &impl std::fmt::Debug) {
    h.write(format!("{v:?}").as_bytes());
    h.write(&[0xff]);
}

/// Size and content hash of a path that has no native metadata (the browser file table).
pub type VirtualFileId<'a> = &'a (dyn Fn(&str) -> Option<(u64, u64)> + Send + Sync);

/// A [`VirtualFileId`] a cache can keep (the browser file table's identities).
pub type SharedVirtualFileId = Arc<dyn Fn(&str) -> Option<(u64, u64)> + Send + Sync>;

/// Layer entries with their source files (#3184), part of every layer disk key: entries of the
/// earlier format (keyed by a project-wide footage salt instead) are never looked up again;
/// least recently used, they are evicted.
const LAYER_ENTRY_VERSION: &[u8] = b"layer-entry-files-2";

/// Disk key of a layer buffer: the layer cache's content key ([`crate::cache::layer_key`] and
/// the keys derived from it), the layer entry format and the renderer version. The content key
/// comes from memory alone: it names the footage items the buffer uses with their settings and
/// paths, so two projects only share an entry for the same pixels of the same files; whether
/// those files still hold what the buffer was made of is checked when the entry is read
/// ([`EntryFiles`]). No file is looked at here.
pub fn layer_disk_key(key: u64) -> u128 {
    let mut h = Hash128::default();
    h.write(LAYER_ENTRY_VERSION);
    h.write(RENDER_VERSION.as_bytes());
    h.write_u64(key);
    h.finish()
}

/// Content key of a composition's pixels, from the project in memory alone (no file is looked
/// at: the frame entries check their own source files when read, see [`EntryFiles`]): project
/// render settings, the comp and the items it can draw (nested comps, footage items with their
/// settings and paths, solids, Media Replacement footage, their proxies and Use Proxy switches;
/// [`effectcraft_project::Project::pixel_reach`]). With expressions among them, everything those
/// expressions can read counts too: every layer of their comps, markers, the comps and footage
/// they name with literal names (and which items have those names), or the whole project when
/// they compute a name or reach items another way. An edit to an item the comp can't reach keeps
/// its key and its frames on disk (#3184, #1492). Switches that don't change pixels (Audio, Lock,
/// Shy, Hide Shy Layers) don't count, and neither do markers unless an expression can read them.
pub fn comp_content_key(project: &effectcraft_project::Project, comp: effectcraft_project::ItemId) -> u128 {
    comp_content_key_for_renderer(project, comp, RENDER_VERSION)
}

/// The content key of `comp` for one renderer: its settings, the comps and footage it uses, and the renderer name.
fn comp_content_key_for_renderer(project: &effectcraft_project::Project, comp: effectcraft_project::ItemId, renderer: &str) -> u128 {
    use effectcraft_project::ItemKind;
    let mut h = Hash128::default();
    h.write(renderer.as_bytes());
    hash_debug(&mut h, &project.settings);
    let heard = project.heard_layers();
    let reach = project.pixel_reach(comp, &heard);
    // Expressions can read any layer of the comps they reach, hidden or not, and their markers
    // (#1083); without them, layers that can't change the picture (music, hidden layers nothing
    // reads) don't count.
    let expressions = reach.expressions;
    let silent = |id, c: &effectcraft_project::Comp| if expressions { Default::default() } else { project.silent_layers(id, c, &heard) };
    // (the names expressions look items up by: which items have them now)
    if !reach.names.is_empty() {
        hash_debug(&mut h, &reach.names);
    }
    let ids: Vec<effectcraft_project::ItemId> = match reach.items {
        Some(items) => items.into_iter().collect(),
        None => project.items.keys().copied().collect(),
    };
    for id in ids {
        let Some(it) = project.item(id) else { continue };
        h.write_u64(id.0);
        h.write(it.name.as_bytes());
        match &it.kind {
            ItemKind::Comp(c) => {
                let quiet = silent(id, c);
                let heard = |l| heard.contains(id, l);
                if quiet.is_empty() {
                    hash_debug(&mut h, &c.pixel_form_with(expressions, &heard));
                } else {
                    let mut pic = (**c).clone();
                    pic.layers.retain(|l| !quiet.contains(&l.id));
                    hash_debug(&mut h, &pic.pixel_form_with(expressions, &heard));
                }
            }
            k => hash_debug(&mut h, k),
        }
        hash_debug(&mut h, &it.proxy);
    }
    h.write_u64(comp.0);
    h.finish()
}

/// Frame entries with their source files (#3184), part of every frame key: entries of the
/// earlier format (keyed by footage stamps instead) are never looked up again; least recently
/// used, they are evicted.
const FRAME_ENTRY_VERSION: &[u8] = b"frame-entry-files-2";

/// Disk key of one viewer frame.
pub fn frame_key(content: u128, frame: i64, scale_milli: u32, view: u64) -> u128 {
    let mut h = Hash128::default();
    h.write(FRAME_ENTRY_VERSION);
    h.write(&content.to_le_bytes());
    h.write_u64(frame as u64);
    h.write_u64(scale_milli as u64);
    h.write_u64(view);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn a_cache_folder_in_the_temp_folder_is_made_private_and_never_a_link() {
        use std::os::unix::fs::PermissionsExt;
        let d = tmpdir("private");
        let dc = DiskCache::open(&d, 1 << 20).unwrap();
        assert_eq!(std::fs::metadata(&d).unwrap().permissions().mode() & 0o777, 0o700);
        drop(dc);
        let link = tmpdir("link-private-temp");
        std::os::unix::fs::symlink(&d, &link).unwrap();
        assert!(DiskCache::open(&link, 1 << 20).is_err());
        let _ = std::fs::remove_file(&link);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn the_shared_temp_folder_itself_is_refused_as_a_cache_folder() {
        let tmp = std::env::temp_dir();
        assert!(DiskCache::open(&tmp, 1 << 20).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_folder_that_resolves_into_the_temp_folder_is_made_private() {
        use std::os::unix::fs::PermissionsExt;
        // `<cwd>/link` names a folder in the temp folder only through the link.
        let real = tmpdir("alias-real");
        std::fs::create_dir_all(&real).unwrap();
        let link = std::env::current_dir().unwrap().join(format!("effectcraft-alias-link-{}", std::process::id()));
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let dc = DiskCache::open(link.join("cache"), 1 << 20).unwrap();
        let mode = std::fs::metadata(real.join("cache")).map(|m| m.permissions().mode() & 0o777);
        drop(dc);
        let _ = std::fs::remove_file(&link);
        let _ = std::fs::remove_dir_all(&real);
        assert_eq!(mode.unwrap(), 0o700);
    }

    #[test]
    fn layers_too_big_to_read_back_are_not_written() {
        assert!(layer_fits(MAX_ENTRY_BYTES / 16));
        assert!(!layer_fits(MAX_ENTRY_BYTES / 16 + 1));
        assert!(!layer_fits(10_000 * 10_000));
        assert!(!layer_fits(usize::MAX));
    }

    #[test]
    fn an_entry_stating_a_size_its_dimensions_do_not_need_is_not_decompressed() {
        // A 2×2 frame whose payload states 3 GiB: refused before any allocation.
        let mut p = 2u32.to_le_bytes().to_vec();
        p.extend_from_slice(&2u32.to_le_bytes());
        p.extend_from_slice(&(3u32 << 30).to_le_bytes());
        p.extend_from_slice(&[0; 16]);
        assert!(decode_frame(&p).is_none());
        assert!(decompress_checked(&p[8..], None).is_none());
        // A well-formed entry still decodes.
        let ok = encode_frame(2, 2, &[7; 16]);
        assert_eq!(decode_frame(&ok).map(|f| f.rgba), Some(vec![7; 16]));
    }

    #[test]
    fn a_block_shorter_than_the_size_it_states_is_rejected() {
        // The payload states 100 bytes but holds a valid block of 10: `lz4_flex` returns the 10.
        let mut p = 100u32.to_le_bytes().to_vec();
        p.extend_from_slice(&lz4_flex::block::compress(&[5u8; 10]));
        assert!(decompress_checked(&p, None).is_none());
        // A half-float frame whose planes come up short is refused, not indexed past.
        let mut f16 = 2u32.to_le_bytes().to_vec();
        f16.extend_from_slice(&2u32.to_le_bytes());
        f16.extend_from_slice(&32u32.to_le_bytes());
        f16.extend_from_slice(&lz4_flex::block::compress(&[1u8; 8]));
        assert!(decode_frame_f16(&f16).is_none());
    }

    fn tmpdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("effectcraft-disk-cache-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    /// A linked frame reads back as the original under its new key, survives the original's
    /// eviction, and is never linked over an existing entry or from a missing one.
    #[test]
    fn link_frame_serves_a_frame_under_a_new_key() {
        let dc = DiskCache::open(tmpdir("link"), 1 << 30).unwrap();
        let rgba: Vec<u8> = (0..16).collect();
        dc.put_frame_now(1, 2, 2, &rgba, &ReadFiles::default());
        assert!(dc.link_frame(1, 2));
        assert_eq!(dc.get_frame(2, None).map(|f| f.0).map(|f| f.rgba), Some(rgba.clone()));
        assert!(!dc.link_frame(1, 2), "already there");
        assert!(!dc.link_frame(3, 4), "nothing to link");
        assert!(!dc.contains(Kind::Frame, 4));
        dc.put_frame_now(5, 2, 2, &[7; 16], &ReadFiles::default());
        assert!(!dc.link_frame(5, 2), "an existing entry stays");
        assert_eq!(dc.get_frame(2, None).map(|f| f.0).map(|f| f.rgba), Some(rgba.clone()));
        let _ = std::fs::remove_file(dc.path(Kind::Frame, 1));
        dc.forget(Kind::Frame, 1);
        assert_eq!(dc.get_frame(2, None).map(|f| f.0).map(|f| f.rgba), Some(rgba), "the link outlives the original");
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
        let files: ReadFiles = [crate::reads::ReadFile::new("/v.png", crate::reads::FileStamp::Virtual)].into_iter().collect();
        let (one, two) = (|_: &str| Some((4, 1)), |_: &str| Some((4, 2)));
        assert!(frame_entry(6, 3, &rgba, &files, None).is_none(), "a file from memory needs its identity");
        assert!(frame_entry(6, 3, &rgba[1..], &files, Some(&one)).is_none(), "a wrong size");
        let e = frame_entry(6, 3, &rgba, &files, Some(&one)).unwrap();
        assert_eq!(read_frame_entry(&e, Some(&one)), Some(Frame8 { width: 6, height: 3, rgba: rgba.clone() }));
        assert!(read_frame_entry(&e, Some(&two)).is_none(), "the file changed");
        let mut incomplete = files.clone();
        incomplete.mark_incomplete();
        assert!(frame_entry(6, 3, &rgba, &incomplete, Some(&one)).is_none(), "an incomplete list is not stored");
        let mut bad = e.clone();
        bad[20] ^= 1;
        assert!(read_frame_entry(&bad, Some(&one)).is_none());
        // An entry of the format before #3184 (pixels without files) is never read.
        assert!(read_frame_entry(&seal_format(Kind::Frame, FORMAT_RGBA8, &encode_frame(6, 3, &rgba)), Some(&one)).is_none());
        let key = 0x0123_4567_89ab_cdef_0011_2233_4455_6677u128;
        assert_eq!(parse_entry_name(&entry_name(key)), Some(key));
        assert_eq!(parse_entry_name("abc.ecc"), None);
        assert_eq!(parse_entry_name(&format!("{key:032x}.tmp.1")), None);
    }

    /// Half-float frames (32 bpc viewer frames) round-trip bit for bit, over-range values,
    /// negatives and odd sizes included; they and 8-bit frames never read as each other; a
    /// damaged half-float entry is a miss and is deleted; they count toward the size limit.
    #[test]
    fn half_float_frames_round_trip() {
        let dir = tmpdir("f16");
        let dc = DiskCache::open(&dir, 1 << 30).unwrap();
        let (w, h) = (7u32, 5u32);
        // 4.0, 16.0, -0.25, 65504 and a spread of bit patterns.
        let rgba: Vec<u16> = (0..w * h * 4).map(|i| [0x4400, 0x4c00, 0xb400, 0x7bff][i as usize % 4] ^ ((i * 37) as u16 & 0x03ff)).collect();
        dc.put_frame_f16_now(1, w, h, &rgba, &ReadFiles::default());
        assert_eq!(dc.get_frame_f16(1, None).map(|f| f.0), Some(FrameF16 { width: w, height: h, rgba: rgba.clone() }));
        assert!(dc.get_frame(1, None).map(|f| f.0).is_none(), "not an 8-bit frame");
        dc.put_frame_now(2, 2, 2, &[9; 16], &ReadFiles::default());
        assert!(dc.get_frame_f16(2, None).map(|f| f.0).is_none() && dc.contains(Kind::Frame, 2), "an 8-bit entry stays valid");
        assert_eq!(dc.get_frame(2, None).map(|f| f.0).unwrap().rgba, vec![9; 16]);
        assert!(dc.stats().bytes > 0);
        // Reopened, still there (format byte persisted).
        drop(dc);
        let dc = DiskCache::open(&dir, 1 << 30).unwrap();
        assert_eq!(dc.get_frame_f16(1, None).map(|f| f.0).unwrap().rgba, rgba);
        // A sealed entry with a payload of the wrong size: deleted on read.
        let p = dc.path(Kind::Frame, 3);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, seal_format(Kind::Frame, FORMAT_F16_FILES, &with_files(&EntryFiles::default(), encode_frame_f16(w, h + 1, &rgba[..])))).unwrap();
        dc.rescan();
        assert!(dc.contains(Kind::Frame, 3));
        assert!(dc.get_frame_f16(3, None).map(|f| f.0).is_none());
        assert!(!dc.contains(Kind::Frame, 3) && !p.exists());
        // Wrong lengths are not written.
        dc.put_frame_f16(4, w, h, &rgba[1..], &ReadFiles::default(), None, 0);
        dc.flush();
        assert!(!dc.contains(Kind::Frame, 4));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Segmentations round-trip by (chain key, scale), survive a reopen (rescan), are bounded by
    /// the size limit like every entry, and a damaged file is a miss.
    #[test]
    fn segmentations_round_trip_and_reopen() {
        use effectcraft_effects::roto::FrameSeg;
        let dir = tmpdir("seg");
        let dc = DiskCache::open(&dir, 1 << 30).unwrap();
        let seg = FrameSeg { w: 7, h: 5, matte: (0..35).map(|i| (i % 3 == 0) as u8).collect(), refine: vec![0; 35], ..Default::default() };
        assert!(dc.get_seg(42, 1.0).is_none());
        dc.put_seg(42, 1.0, &seg);
        dc.flush();
        assert_eq!(dc.get_seg(42, 1.0), Some(seg.clone()));
        assert!(dc.get_seg(42, 0.5).is_none() && dc.get_seg(43, 1.0).is_none());
        assert_eq!(dc.keys(Kind::Matte).len(), 1);
        drop(dc);
        let dc = DiskCache::open(&dir, 1 << 30).unwrap();
        assert_eq!(dc.get_seg(42, 1.0), Some(seg.clone()));
        // Damaged on disk: a miss, and the entry is forgotten.
        let path = dc.path(Kind::Matte, seg_key(42, 1.0));
        let mut d = std::fs::read(&path).unwrap();
        d[18] ^= 0x55;
        std::fs::write(&path, d).unwrap();
        assert!(dc.get_seg(42, 1.0).is_none());
        assert!(!dc.contains(Kind::Matte, seg_key(42, 1.0)));
        // The size limit evicts segmentations too.
        dc.put_seg(1, 1.0, &seg);
        dc.flush();
        dc.set_max_bytes(1);
        assert!(dc.keys(Kind::Matte).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hit_miss_and_exact_round_trip() {
        let dir = tmpdir("hit");
        let dc = DiskCache::open(&dir, 1 << 30).unwrap();
        assert!(dc.get_layer(42, None).is_none());
        let b = buf(1, 33, 17);
        dc.put_layer(42, &Arc::new(b.clone()), &EntryFiles::default(), 0);
        dc.flush();
        let (back, files) = dc.get_layer(42, None).unwrap();
        assert!(files.is_empty() && files.is_complete());
        assert_eq!(back.img, b.img);
        assert_eq!((back.offset, back.scale), (b.offset, b.scale));
        let rgba: Vec<u8> = (0..20 * 10 * 4).map(|i| (i % 251) as u8).collect();
        assert!(dc.put_frame(7, 20, 10, &rgba, &ReadFiles::default(), None, 0));
        assert!(!dc.put_frame(8, 2, 2, &[0; 3], &ReadFiles::default(), None, 0), "a wrong-size frame is refused");
        // Handed over and compressed on the writer thread: the same pixels come back.
        assert!(dc.put_frame_owned(9, 20, 10, rgba.clone(), &ReadFiles::default(), None, 0));
        assert!(!dc.put_frame_owned(10, 2, 2, vec![0; 3], &ReadFiles::default(), None, 0), "a wrong-size frame is refused");
        dc.flush();
        assert_eq!(dc.get_frame(7, None).map(|f| f.0).unwrap().rgba, rgba);
        assert_eq!(dc.get_frame(9, None).map(|f| f.0).unwrap().rgba, rgba);
        let st = dc.stats();
        assert_eq!((st.entries, st.hits, st.misses, st.writes), (3, 3, 1, 3));
        // Wrong kind is a miss.
        assert!(dc.get_frame(42, None).map(|f| f.0).is_none());
    }

    /// Music doesn't change a comp's disk keys (adding it to the timeline kept no blue frame);
    /// hiding a clip does. Importing music keeps them too.
    #[test]
    fn music_keeps_the_disk_keys() {
        use effectcraft_project::{Comp, Footage, FootageKind, ItemKind, LayerSource, Project, Solid, build};
        use effectcraft_time::{FrameRate, Tick};
        let mut p = Project::default();
        let comp = Comp::new(64, 64, FrameRate::FPS_30, Tick::from_seconds_f64(2.0));
        let cid = p.add_item("Main", Default::default(), None, ItemKind::Comp(comp.clone().into()));
        let sid = p.add_item("Solid", Default::default(), None, ItemKind::Solid(Solid { color: [1.0, 0.0, 0.0], width: 64, height: 64, pixel_aspect: 1.0 }));
        let clip = build::layer(&mut p, &comp, "Clip", LayerSource::Solid { item: sid }, (64, 64), None);
        let clip_id = clip.id;
        p.comp_mut(cid).unwrap().layers.push(clip);
        let key = comp_content_key(&p, cid);
        let song = p.add_item(
            "song.wav",
            Default::default(),
            None,
            ItemKind::Footage(Footage { kind: FootageKind::Audio, has_video: false, has_audio: true, ..Default::default() }),
        );
        assert_eq!(comp_content_key(&p, cid), key, "importing music");
        let music = build::layer(&mut p, &comp, "Music", LayerSource::Footage { item: song }, (64, 64), None);
        p.comp_mut(cid).unwrap().layers.insert(0, music);
        assert_eq!(comp_content_key(&p, cid), key, "music on the timeline");
        p.comp_mut(cid).unwrap().layer_mut(clip_id).unwrap().switches.video = false;
        assert_ne!(comp_content_key(&p, cid), key, "a hidden clip");
    }

    /// An expression that reads a 3D model layer's source (`thisComp.layer("Model").source.name`)
    /// gives a new key when the model item is renamed: the frames on disk hold the old name.
    #[test]
    fn renaming_a_model_an_expression_reads_changes_the_disk_key() {
        use effectcraft_project::{Comp, Expression, Footage, FootageKind, ItemKind, LayerSource, Project, Solid, build};
        use effectcraft_time::{FrameRate, Tick};
        let mut p = Project::default();
        let comp = Comp::new(64, 64, FrameRate::FPS_30, Tick::from_seconds_f64(2.0));
        let cid = p.add_item("Main", Default::default(), None, ItemKind::Comp(comp.clone().into()));
        let model = p.add_item("chair.glb", Default::default(), None, ItemKind::Footage(Footage { kind: FootageKind::Model, ..Default::default() }));
        let sid = p.add_item("Solid", Default::default(), None, ItemKind::Solid(Solid { color: [1.0, 0.0, 0.0], width: 64, height: 64, pixel_aspect: 1.0 }));
        let mut m = build::layer(&mut p, &comp, "Model", LayerSource::Model { item: model }, (64, 64), None);
        m.switches.video = false;
        let mut s = build::layer(&mut p, &comp, "Reader", LayerSource::Solid { item: sid }, (64, 64), None);
        s.props.prop_mut("transform/opacity").unwrap().expr =
            Some(Expression { text: "thisComp.layer(\"Model\").source.name.length * 10".into(), enabled: true });
        p.comp_mut(cid).unwrap().layers.extend([s, m]);
        let key = comp_content_key(&p, cid);
        p.item_mut(model).unwrap().name = "table.glb".into();
        assert_ne!(comp_content_key(&p, cid), key, "the expression reads the model's name");
    }

    /// Layer jobs hold float buffers until they are compressed: the queue holds at most
    /// [`LAYER_QUEUE_BYTES`] of them (64 at 4K held ~8.5 GB), and a single bigger one only alone.
    #[test]
    fn layer_writes_queue_within_a_byte_budget() {
        let mut q = Queue {
            frames: Default::default(),
            frame_bytes: 0,
            jobs: Default::default(),
            job_bytes: 0,
            busy: true,
            pending_frames: Default::default(),
            drops: Drops::default(),
        };
        let layer = LAYER_QUEUE_BYTES / 4 + 1;
        for _ in 0..3 {
            assert!(!q.full(Kind::Layer, layer));
            q.jobs.push_back(Job::Write(Kind::Layer, 0, 0, Payload::Encoded(vec![])));
            q.job_bytes += layer;
        }
        assert!(q.full(Kind::Layer, layer), "a fourth would pass the budget");
        assert!(!q.full(Kind::Frame, 1024), "frames have their own budget");
        let mut empty = Queue { jobs: Default::default(), job_bytes: 0, ..q };
        assert!(!empty.full(Kind::Layer, 2 * LAYER_QUEUE_BYTES), "a single big layer, alone");
        // Taking a job gives its bytes back.
        empty.job_bytes = 3;
        empty.jobs.push_back(Job::Write(Kind::Layer, 0, 0, Payload::Encoded(vec![0; 3])));
        assert!(empty.pop().is_some());
        assert_eq!(empty.job_bytes, 0);
    }

    /// A burst of layer writes never crowds out frames: every frame of a fully cached comp is
    /// on disk afterwards (the blue bar came back with gaps).
    #[test]
    fn frames_are_kept_when_layer_writes_fill_the_queue() {
        let dir = tmpdir("burst");
        let dc = DiskCache::open(&dir, 1 << 30).unwrap();
        for k in 0..300 {
            dc.put_layer(10_000 + k, &Arc::new(buf(1, 64, 64)), &EntryFiles::default(), 0);
        }
        let rgba = vec![7u8; 32 * 32 * 4];
        for k in 0..300 {
            dc.put_frame(k, 32, 32, &rgba, &ReadFiles::default(), None, 0);
        }
        dc.flush();
        assert!((0..300).all(|k| dc.contains(Kind::Frame, k)), "every frame written");
    }

    #[test]
    fn previous_renderer_frames_are_not_reused() {
        use effectcraft_project::{Comp, ItemKind, Project};
        use effectcraft_time::{FrameRate, Tick};

        let mut project = Project::default();
        let comp = Comp::new(2, 2, FrameRate::FPS_30, Tick::from_seconds_f64(1.0));
        let id = project.add_item("Cached composition", effectcraft_color::Label::None, None, ItemKind::Comp(comp.into()));
        let key = frame_key(comp_content_key(&project, id), 0, 1000, 0);
        let previous_key = frame_key(comp_content_key_for_renderer(&project, id, "effectcraft-render-5"), 0, 1000, 0);
        let dir = tmpdir("previous-renderer");
        {
            let cache = DiskCache::open(&dir, 1 << 30).unwrap();
            cache.put_frame_now(previous_key, 2, 2, &[9; 16], &ReadFiles::default());
        }
        let cache = DiskCache::open(&dir, 1 << 30).unwrap();
        assert_eq!(cache.get_frame(previous_key, None).map(|f| f.0).unwrap().rgba, vec![9; 16]);
        assert!(cache.get_frame(key, None).map(|f| f.0).is_none());
        cache.put_frame_now(key, 2, 2, &[17; 16], &ReadFiles::default());
        assert_eq!(cache.get_frame(key, None).map(|f| f.0).unwrap().rgba, vec![17; 16]);
    }

    #[test]
    fn persists_across_restarts_and_clears() {
        let dir = tmpdir("persist");
        {
            let dc = DiskCache::open(&dir, 1 << 30).unwrap();
            dc.put_layer_now(1, &buf(1, 8, 8), &ReadFiles::default());
            dc.put_frame_now(2, 2, 2, &[9; 16], &ReadFiles::default());
        }
        let dc = DiskCache::open(&dir, 1 << 30).unwrap();
        assert_eq!(dc.stats().entries, 2);
        assert!(dc.contains(Kind::Layer, 1) && dc.contains(Kind::Frame, 2));
        assert_eq!(dc.get_frame(2, None).map(|f| f.0).unwrap().rgba, vec![9; 16]);
        dc.clear();
        assert_eq!(dc.stats().entries, 0);
        assert!(dc.get_frame(2, None).map(|f| f.0).is_none());
        let again = DiskCache::open(&dir, 1 << 30).unwrap();
        assert_eq!(again.stats().entries, 0);
    }

    #[test]
    fn lru_eviction_respects_the_limit() {
        let dir = tmpdir("lru");
        let dc = DiskCache::open(&dir, u64::MAX).unwrap();
        for k in 0..6u128 {
            dc.put_layer_now(k, &buf(k as u32, 64, 64), &ReadFiles::default());
        }
        let per = dc.stats().bytes / 6;
        // Touch 0 so it is the most recently used.
        assert!(dc.get_layer(0, None).is_some());
        dc.set_max_bytes(per * 3 + per * 3 / 4);
        let st = dc.stats();
        assert!(st.bytes <= per * 3 + per * 3 / 4, "{st:?}");
        assert_eq!(st.entries, 3);
        assert!(dc.contains(Kind::Layer, 0), "recently used survives");
        assert!(!dc.contains(Kind::Layer, 1) && !dc.contains(Kind::Layer, 2) && !dc.contains(Kind::Layer, 3));
        assert_eq!(st.evictions, 3);
        // Files are really gone.
        let files = walk(&dir);
        assert_eq!(files, 3);
        // New writes keep evicting.
        dc.put_layer_now(99, &buf(9, 64, 64), &ReadFiles::default());
        assert!(dc.stats().bytes <= per * 3 + per * 3 / 4);
    }

    #[test]
    fn eviction_goes_down_to_ninety_percent_in_lru_order() {
        let mut ix = DiskIndex::default();
        for k in 0..10u128 {
            ix.insert(Kind::Layer, k, 100);
        }
        assert!(ix.touch(Kind::Layer, 0));
        // Over 950: down to 855, least recently used first.
        ix.insert(Kind::Layer, 10, 100);
        assert_eq!(ix.evict(950), vec![(Kind::Layer, 1), (Kind::Layer, 2), (Kind::Layer, 3)]);
        assert_eq!(ix.total(), 800);
        // The slack absorbs the next write without another scan.
        ix.insert(Kind::Layer, 11, 100);
        assert!(ix.evict(950).is_empty());
    }

    #[test]
    fn free_disk_space_caps_the_limit() {
        let dir = tmpdir("space");
        let dc = DiskCache::open(&dir, 1 << 30).unwrap();
        for k in 0..4u128 {
            dc.put_layer_now(k, &buf(k as u32, 64, 64), &ReadFiles::default());
        }
        let used = dc.stats().bytes;
        // Unknown space: the setting alone.
        dc.set_disk_space(None);
        assert_eq!(dc.stats().limit_bytes, 1 << 30);
        // A disk reported as 0 bytes is unknown.
        dc.set_disk_space(Some((0, 0)));
        assert_eq!(dc.stats().limit_bytes, 1 << 30);
        // 100 GB disk, 5 GB reserved (5%, between the 2 GB floor and the 10 GB cap), exactly
        // 5 GB free: the cache may not grow.
        dc.set_disk_space(Some((100 << 30, 5 << 30)));
        let st = dc.stats();
        assert_eq!((st.max_bytes, st.limit_bytes), (1 << 30, used));
        dc.put_layer_now(9, &buf(9, 64, 64), &ReadFiles::default());
        let st = dc.stats();
        assert!(st.bytes <= st.limit_bytes && st.bytes <= used - used / 10 && st.evictions > 0, "{st:?}");
        assert!(dc.contains(Kind::Layer, 9), "the new entry is the most recently used");
        // Below the reserve the cache gives its space back, and skips writes that can't fit.
        dc.set_disk_space(Some((100 << 30, 4 << 30)));
        dc.put_layer_now(10, &buf(10, 64, 64), &ReadFiles::default());
        let st = dc.stats();
        assert_eq!((st.limit_bytes, st.entries, st.bytes), (0, 0, 0), "{st:?}");
        // Plenty of space again: back to the setting.
        dc.set_disk_space(Some((100 << 30, 50 << 30)));
        assert_eq!(dc.stats().limit_bytes, 1 << 30);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The reserve is 5% of the disk, at least 2 GB and at most 10 GB: a large disk with plenty
    /// free keeps the user's size. (A 4 TB disk with 190 GB free reserved 200 GB, held a 20 GB
    /// cache at 0, and every write was refused.)
    #[test]
    fn a_large_disk_reserves_at_most_ten_gb() {
        const GB: u64 = 1 << 30;
        assert_eq!(disk_reserve(4096 * GB), 10 * GB);
        assert_eq!(disk_reserve(100 * GB), 5 * GB);
        assert_eq!(disk_reserve(32 * GB), 2 * GB);
        assert_eq!(disk_reserve(1), 2 * GB);
        let dir = tmpdir("space-large");
        let dc = DiskCache::open(&dir, 20 * GB).unwrap();
        dc.set_disk_space(Some((4096 * GB, 190 * GB)));
        let st = dc.stats();
        assert_eq!((st.max_bytes, st.limit_bytes), (20 * GB, 20 * GB), "{st:?}");
        assert!(dc.accepts_writes());
        assert_eq!(dc.reserve_bytes(), Some(10 * GB));
        // Just above the 10 GB reserve: the cache may use what is left above it.
        dc.set_disk_space(Some((4096 * GB, 15 * GB)));
        assert_eq!(dc.stats().limit_bytes, 5 * GB);
        // A small disk still keeps 2 GB free: 3 GB free leaves 1 GB for the cache.
        dc.set_disk_space(Some((32 * GB, 3 * GB)));
        assert_eq!(dc.stats().limit_bytes, GB);
        // Below the floor: nothing.
        dc.set_disk_space(Some((32 * GB, GB)));
        assert_eq!(dc.stats().limit_bytes, 0);
        assert!(!dc.accepts_writes());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A write the free disk space refuses is refused before its pixels are compressed: the
    /// frame workers spent about 8 ms per frame and 14 ms per layer encoding entries that were
    /// then refused (`limit_bytes=0`).
    #[test]
    fn no_encode_while_the_disk_has_no_room() {
        const GB: u64 = 1 << 30;
        let dir = tmpdir("space-no-encode");
        let dc = DiskCache::open(&dir, 20 * GB).unwrap();
        // 4 TB disk, 1 GB free: under the 10 GB reserve, the limit is 0.
        dc.set_disk_space(Some((4096 * GB, GB)));
        assert_eq!(dc.stats().limit_bytes, 0);
        let rgba = vec![7u8; 16 * 16 * 4];
        assert!(!dc.put_frame(1, 16, 16, &rgba, &ReadFiles::default(), None, 0), "refused");
        assert!(!dc.put_frame_f16(2, 16, 16, &[0x3c00u16; 16 * 16 * 4], &ReadFiles::default(), None, 0), "refused");
        dc.put_layer(3, &Arc::new(buf(3, 16, 16)), &EntryFiles::default(), 0);
        dc.flush();
        assert_eq!(dc.encodes(), 0, "nothing compressed");
        assert_eq!(dc.stats().entries, 0);
        // 32 MB of room above the reserve: a few frames would fill it, the rest would be
        // compressed only to be refused. Not worth encoding either.
        dc.set_disk_space(Some((4096 * GB, 10 * GB + (32 << 20))));
        assert_eq!(dc.stats().limit_bytes, 32 << 20);
        assert!(!dc.accepts_writes());
        assert!(!dc.put_frame(1, 16, 16, &rgba, &ReadFiles::default(), None, 0), "refused");
        dc.put_layer(3, &Arc::new(buf(3, 16, 16)), &EntryFiles::default(), 0);
        assert_eq!(dc.encodes(), 0, "nothing compressed");
        // A small limit the setting asks for (not free space) is used.
        dc.set_disk_space(None);
        dc.set_max_bytes(32 << 20);
        assert!(dc.accepts_writes());
        dc.set_max_bytes(20 * GB);
        // Room again: the same writes are compressed and stored.
        dc.set_disk_space(Some((4096 * GB, 500 * GB)));
        assert!(dc.put_frame(1, 16, 16, &rgba, &ReadFiles::default(), None, 0));
        assert!(dc.put_frame_f16(2, 16, 16, &[0x3c00u16; 16 * 16 * 4], &ReadFiles::default(), None, 0));
        dc.put_layer(3, &Arc::new(buf(3, 16, 16)), &EntryFiles::default(), 0);
        dc.flush();
        assert_eq!(dc.encodes(), 3);
        assert_eq!(dc.stats().entries, 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn failed_writes_are_counted() {
        let dir = tmpdir("failed");
        let dc = DiskCache::open(&dir, 1 << 30).unwrap();
        // A file where the layers folder should be: every layer write fails.
        std::fs::write(dir.join(VERSION).join(Kind::Layer.dir()), b"x").unwrap();
        dc.put_layer_now(1, &buf(1, 8, 8), &ReadFiles::default());
        dc.put_layer_now(2, &buf(2, 8, 8), &ReadFiles::default());
        let st = dc.stats();
        assert_eq!((st.failed, st.writes, st.entries), (2, 0, 0));
        let _ = std::fs::remove_dir_all(&dir);
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
        dc.put_frame_now(5, 4, 4, &[1; 64], &ReadFiles::default());
        let p = dc.path(Kind::Frame, 5);
        let mut d = std::fs::read(&p).unwrap();
        let n = d.len();
        d[n / 2] ^= 0xff;
        std::fs::write(&p, d).unwrap();
        assert!(dc.get_frame(5, None).map(|f| f.0).is_none());
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
                        w.put_layer_now(k, &buf(k as u32, 16, 16), &ReadFiles::default());
                        if let Some((x, _)) = w.get_layer(k, None) {
                            assert_eq!(x.img, buf(k as u32, 16, 16).img);
                        }
                    }
                });
            }
        });
        b.rescan();
        for k in 0..5u128 {
            assert_eq!(b.get_layer(k, None).unwrap().0.img, buf(k as u32, 16, 16).img);
        }
    }

    /// Replaced bytes of a path with no native metadata (the browser file table) change neither
    /// the comp key nor a layer's disk key (both from memory): the frame and layer entries made
    /// of them check the hook's size and content hash when read and are misses, deleted (#1253,
    /// #3184).
    #[test]
    fn virtual_file_bytes_are_checked_by_the_entries_not_the_keys() {
        use effectcraft_project::build;
        use effectcraft_project::{Comp, Footage, ItemKind, LayerSource, Project};
        use effectcraft_time::{FrameRate, Tick};

        let mut p = Project::default();
        let f = Footage { path: "/virtual.mov".into(), width: 16, height: 9, duration: Tick::from_seconds_f64(1.0), has_video: true, ..Default::default() };
        let fid = p.add_item("virtual.mov", effectcraft_color::Label::None, None, ItemKind::Footage(f));
        let mut comp = Comp::new(16, 9, FrameRate::FPS_30, Tick::from_seconds_f64(1.0));
        comp.layers = vec![build::layer(&mut p, &comp, "clip", LayerSource::Footage { item: fid }, (16, 9), None)].into();
        let cid = p.add_item("C", effectcraft_color::Label::None, None, ItemKind::Comp(comp.into()));

        let id = |mark: u64| move |path: &str| (path == "/virtual.mov").then_some((8, mark));
        let (one, two) = (id(1), id(2));
        let dc = DiskCache::open(tmpdir("virtual"), 1 << 30).unwrap();
        let k = frame_key(comp_content_key(&p, cid), 0, 1000, 0);
        let files: ReadFiles = [crate::reads::ReadFile::new("/virtual.mov", crate::reads::FileStamp::Virtual)].into_iter().collect();
        assert!(dc.put_frame(k, 1, 1, &[1, 2, 3, 4], &files, Some(&one), 0));
        dc.flush();
        assert!(dc.get_frame(k, Some(&one)).is_some_and(|(_, f)| f == files));
        assert!(dc.get_frame(k, Some(&two)).is_none() && !dc.contains(Kind::Frame, k), "replaced: a miss, deleted");
        // A layer buffer made of it.
        let lk = layer_disk_key(7);
        dc.put_layer(lk, &Arc::new(buf(1, 4, 4)), &EntryFiles::of(&files, Some(&one)).unwrap(), 0);
        dc.flush();
        assert!(dc.get_layer(lk, Some(&one)).is_some_and(|(b, f)| b.img == buf(1, 4, 4).img && f == files && f.is_complete()));
        assert!(dc.get_layer(lk, None).is_none() && !dc.contains(Kind::Layer, lk), "no identity: a miss, deleted");
        dc.put_layer(lk, &Arc::new(buf(1, 4, 4)), &EntryFiles::of(&files, Some(&one)).unwrap(), 0);
        dc.flush();
        assert!(dc.get_layer(lk, Some(&two)).is_none() && !dc.contains(Kind::Layer, lk), "replaced: a miss, deleted");
        // The browser's sealed layer entries carry the same list.
        let e = layer_entry(&buf(2, 3, 2), &EntryFiles::of(&files, Some(&one)).unwrap());
        let (b, list) = read_layer_entry(&e).unwrap();
        assert!(b.img == buf(2, 3, 2).img && list.unchanged(Some(&one)) && !list.unchanged(Some(&two)));
        assert!(read_layer_entry(&seal_format(Kind::Layer, FORMAT_RGBA8, &encode_layer(&buf(2, 3, 2)))).is_none(), "the format before #3184");
        let mut bad = e.clone();
        bad[12] ^= 1;
        assert!(read_layer_entry(&bad).is_none(), "damaged");
    }

    /// Layer disk keys come from the layer cache's key alone (no file, no project-wide salt) and
    /// differ per key.
    #[test]
    fn layer_disk_keys_need_no_files() {
        assert_eq!(layer_disk_key(1), layer_disk_key(1));
        assert_ne!(layer_disk_key(1), layer_disk_key(2));
        // Not the layout of the salted keys before #3184 (`salt << 64 | key`).
        assert_ne!(layer_disk_key(1) as u64, 1);
    }

    /// A layer entry is used only while the files it lists are what the buffer was made of: a
    /// new size, a new time, a missing file that is back, a file replaced by a folder, a damaged
    /// files section and the format before #3184 are misses, and the entry is deleted so the
    /// buffer is rendered and stored again. A buffer whose files can't all be named is not
    /// stored. Only the listed files are looked at.
    #[test]
    fn layer_entries_check_their_files() {
        use crate::reads::{FileStamp, ReadFile};
        let dir = tmpdir("layer-check");
        let src = dir.join("src");
        std::fs::create_dir_all(&src).unwrap();
        let file = |name: &str, bytes: &[u8]| {
            let p = src.join(name);
            std::fs::write(&p, bytes).unwrap();
            p.to_string_lossy().to_string()
        };
        let (a, b) = (file("a.png", b"aa"), file("b.png", b"bb"));
        let gone = src.join("gone.png").to_string_lossy().to_string();
        let read = |paths: &[&str]| -> ReadFiles { paths.iter().map(|p| ReadFile::new(*p, FileStamp::native(p).unwrap_or(FileStamp::Missing))).collect() };
        let dc = DiskCache::open(&dir, 1 << 30).unwrap();
        dc.put_layer_now(1, &buf(1, 4, 4), &read(&[&a]));
        dc.put_layer_now(2, &buf(2, 4, 4), &read(&[&b, &gone]));
        dc.put_layer_now(3, &buf(3, 4, 4), &read(&[&b]));
        let (one, files) = dc.get_layer(1, None).unwrap();
        assert!(one.img == buf(1, 4, 4).img && files == read(&[&a]) && files.is_complete());
        assert_eq!(dc.file_checks(), 1, "only the file of that buffer");
        // a.png gets a new size: entry 1 is a miss and deleted, the others stay.
        std::fs::write(&a, b"aaa").unwrap();
        assert!(dc.get_layer(1, None).is_none() && !dc.contains(Kind::Layer, 1));
        assert!(dc.get_layer(2, None).is_some() && dc.get_layer(3, None).is_some());
        // b.png gets a new time only.
        let t = std::fs::metadata(&b).unwrap().modified().unwrap() + std::time::Duration::from_secs(7);
        std::fs::File::options().append(true).open(&b).unwrap().set_modified(t).unwrap();
        assert!(dc.get_layer(3, None).is_none() && !dc.contains(Kind::Layer, 3));
        assert!(dc.get_layer(2, None).is_none(), "b.png is listed there too");
        // A missing file that is back.
        dc.put_layer_now(4, &buf(4, 4, 4), &read(&[&gone]));
        assert!(dc.get_layer(4, None).is_some(), "missing, still missing");
        file("gone.png", b"g");
        assert!(dc.get_layer(4, None).is_none() && !dc.contains(Kind::Layer, 4), "back");
        // A file replaced by a folder (unreadable).
        let c = file("c.png", b"cc");
        dc.put_layer_now(5, &buf(5, 4, 4), &read(&[&c]));
        std::fs::remove_file(&c).unwrap();
        std::fs::create_dir(&c).unwrap();
        assert!(dc.get_layer(5, None).is_none() && !dc.contains(Kind::Layer, 5));
        // Written again after the miss: a hit with the new identity.
        dc.put_layer_now(1, &buf(9, 4, 4), &read(&[&a]));
        assert_eq!(dc.get_layer(1, None).map(|l| l.0.img), Some(buf(9, 4, 4).img));
        // The format before #3184 (no files): a miss, deleted.
        dc.write_format(Kind::Layer, 6, FORMAT_RGBA8, &encode_layer(&buf(6, 4, 4)));
        assert!(dc.contains(Kind::Layer, 6));
        assert!(dc.get_layer(6, None).is_none() && !dc.contains(Kind::Layer, 6));
        // A valid checksum over a damaged files section, or over a cut buffer: misses, deleted.
        let mut payload = vec![];
        payload.extend_from_slice(&3u32.to_le_bytes());
        payload.extend_from_slice(&encode_layer(&buf(7, 4, 4)));
        dc.write_format(Kind::Layer, 7, FORMAT_LAYER_FILES, &payload);
        assert!(dc.get_layer(7, None).is_none() && !dc.contains(Kind::Layer, 7));
        let mut cut = vec![];
        EntryFiles::default().encode(&mut cut);
        cut.extend_from_slice(&encode_layer(&buf(8, 4, 4))[..40]);
        dc.write_format(Kind::Layer, 8, FORMAT_LAYER_FILES, &cut);
        assert!(dc.get_layer(8, None).is_none() && !dc.contains(Kind::Layer, 8));
        // A damaged file on disk.
        dc.put_layer_now(9, &buf(9, 4, 4), &ReadFiles::default());
        let p = dc.path(Kind::Layer, 9);
        let mut d = std::fs::read(&p).unwrap();
        let n = d.len();
        d[n / 2] ^= 0xff;
        std::fs::write(&p, d).unwrap();
        assert!(dc.get_layer(9, None).is_none() && !dc.contains(Kind::Layer, 9) && !p.exists());
        // Buffers whose files can't all be named are not stored.
        let mut incomplete = read(&[&a]);
        incomplete.mark_incomplete();
        dc.put_layer_now(10, &buf(10, 4, 4), &incomplete);
        assert!(!dc.contains(Kind::Layer, 10));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A frame entry is used only while the files it lists are what it was made of: a new size,
    /// a new time, a missing file that is back, a file replaced by a folder are misses, and the
    /// entry is deleted so the frame is rendered and stored again. Only the listed files are
    /// looked at. Entries of the format before #3184 are misses and deleted.
    #[test]
    fn frame_entries_check_their_files() {
        use crate::reads::{FileStamp, ReadFile};
        let dir = tmpdir("check");
        let src = dir.join("src");
        std::fs::create_dir_all(&src).unwrap();
        let file = |name: &str, bytes: &[u8]| {
            let p = src.join(name);
            std::fs::write(&p, bytes).unwrap();
            p.to_string_lossy().to_string()
        };
        let (a, b) = (file("a.png", b"aa"), file("b.png", b"bb"));
        let gone = src.join("gone.png").to_string_lossy().to_string();
        let read = |paths: &[&str]| -> ReadFiles { paths.iter().map(|p| ReadFile::new(*p, FileStamp::native(p).unwrap_or(FileStamp::Missing))).collect() };
        let dc = DiskCache::open(&dir, 1 << 30).unwrap();
        dc.put_frame_now(1, 1, 1, &[1; 4], &read(&[&a]));
        dc.put_frame_now(2, 1, 1, &[2; 4], &read(&[&b, &gone]));
        dc.put_frame_now(3, 1, 1, &[3; 4], &read(&[&b]));
        assert_eq!(dc.get_frame(1, None).map(|f| f.1), Some(read(&[&a])));
        assert_eq!(dc.file_checks(), 1, "only the file of that frame");
        // a.png gets a new size: frame 1 is a miss and deleted, the others stay.
        std::fs::write(&a, b"aaa").unwrap();
        assert!(dc.get_frame(1, None).is_none() && !dc.contains(Kind::Frame, 1));
        assert!(dc.get_frame(2, None).is_some() && dc.get_frame(3, None).is_some());
        // b.png gets a new time only.
        let t = std::fs::metadata(&b).unwrap().modified().unwrap() + std::time::Duration::from_secs(7);
        std::fs::File::options().append(true).open(&b).unwrap().set_modified(t).unwrap();
        assert!(dc.get_frame(3, None).is_none() && !dc.contains(Kind::Frame, 3));
        assert!(dc.get_frame(2, None).is_none(), "b.png is listed there too");
        // A missing file that is back.
        dc.put_frame_now(4, 1, 1, &[4; 4], &read(&[&gone]));
        assert!(dc.get_frame(4, None).is_some(), "missing, still missing");
        file("gone.png", b"g");
        assert!(dc.get_frame(4, None).is_none() && !dc.contains(Kind::Frame, 4), "back");
        // A file replaced by a folder (unreadable).
        let c = file("c.png", b"cc");
        dc.put_frame_now(5, 1, 1, &[5; 4], &read(&[&c]));
        std::fs::remove_file(&c).unwrap();
        std::fs::create_dir(&c).unwrap();
        assert!(dc.get_frame(5, None).is_none() && !dc.contains(Kind::Frame, 5));
        // Written again after the miss: a hit with the new identity.
        let fresh = file("d.png", b"dd");
        dc.put_frame_now(1, 1, 1, &[9; 4], &read(&[&fresh]));
        assert_eq!(dc.get_frame(1, None).map(|f| f.0.rgba), Some(vec![9; 4]));
        // The format before #3184 (no files): a miss, deleted.
        dc.write_format(Kind::Frame, 6, FORMAT_RGBA8, &encode_frame(1, 1, &[6; 4]));
        assert!(dc.contains(Kind::Frame, 6));
        assert!(dc.get_frame(6, None).is_none() && !dc.contains(Kind::Frame, 6));
        // A valid checksum over a damaged files section: a miss, deleted.
        let mut payload = vec![];
        payload.extend_from_slice(&3u32.to_le_bytes());
        payload.extend_from_slice(&encode_frame(1, 1, &[7; 4]));
        dc.write_format(Kind::Frame, 7, FORMAT_RGBA8_FILES, &payload);
        assert!(dc.get_frame(7, None).is_none() && !dc.contains(Kind::Frame, 7));
        // Frames whose files can't all be named are not stored.
        let mut incomplete = read(&[&fresh]);
        incomplete.mark_incomplete();
        assert!(!dc.put_frame(8, 1, 1, &[8; 4], &incomplete, None, 0));
        dc.flush();
        assert!(!dc.contains(Kind::Frame, 8));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Reload Footage drops the entries made of the reloaded files (#3184): a file replaced by
    /// one with the same size and modification time passes the check on hit, so only the drop
    /// makes the next use a miss. Entries of other files stay; an entry written after the drop
    /// is used; a layer entry and one written while the drop waits in the queue are dropped too;
    /// the drop survives a restart (the files are gone).
    #[test]
    fn dropped_files_take_their_entries_with_them() {
        use crate::reads::{FileStamp, ReadFile};
        let dir = tmpdir("drop");
        let src = dir.join("src");
        std::fs::create_dir_all(&src).unwrap();
        let file = |name: &str, bytes: &[u8]| {
            let p = src.join(name);
            std::fs::write(&p, bytes).unwrap();
            p.to_string_lossy().to_string()
        };
        let (a, b) = (file("a.png", b"aa"), file("b.png", b"bb"));
        let read = |paths: &[&str]| -> ReadFiles { paths.iter().map(|p| ReadFile::new(*p, FileStamp::native(p).unwrap())).collect() };
        let dc = DiskCache::open(&dir, 1 << 30).unwrap();
        dc.put_frame_now(1, 1, 1, &[1; 4], &read(&[&a]));
        dc.put_frame_now(2, 1, 1, &[2; 4], &read(&[&b]));
        dc.put_frame_now(3, 1, 1, &[3; 4], &read(&[&a, &b]));
        dc.put_layer_now(4, &buf(4, 2, 2), &read(&[&a]));
        // a.png gets other bytes of the same size and its old time back: the check can't tell.
        let t = std::fs::metadata(&a).unwrap().modified().unwrap();
        std::fs::write(&a, b"AA").unwrap();
        std::fs::File::options().append(true).open(&a).unwrap().set_modified(t).unwrap();
        assert!(dc.get_frame(1, None).is_some(), "same size and time: still a hit");
        // A frame queued (not written yet) when the drop comes is dropped too.
        assert!(dc.put_frame(5, 1, 1, &[5; 4], &read(&[&a]), None, 0));
        dc.drop_files([a.clone()]);
        dc.flush();
        assert!(!dc.is_dropping());
        for (kind, key) in [(Kind::Frame, 1), (Kind::Frame, 3), (Kind::Layer, 4), (Kind::Frame, 5)] {
            assert!(!dc.contains(kind, key), "{kind:?} {key} made of a.png is gone");
        }
        assert!(dc.get_frame(1, None).is_none() && dc.get_layer(4, None).is_none());
        assert_eq!(dc.get_frame(2, None).map(|f| f.0.rgba), Some(vec![2; 4]), "b.png's frame stays");
        // Written again from the reloaded file: used.
        dc.put_frame_now(1, 1, 1, &[9; 4], &read(&[&a]));
        assert_eq!(dc.get_frame(1, None).map(|f| f.0.rgba), Some(vec![9; 4]));
        // The drop is for good: a new session on the folder finds what is left.
        drop(dc);
        let dc = DiskCache::open(&dir, 1 << 30).unwrap();
        assert_eq!(dc.keys(Kind::Frame).into_iter().collect::<std::collections::BTreeSet<_>>(), [1, 2].into());
        assert!(dc.keys(Kind::Layer).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A render under way when Reload Footage drops its files offers its entries after the drop
    /// (#3184), when the writer has already deleted the old ones: made of the files as they were,
    /// they are refused, frames and layer buffers alike. What the same render made of other files
    /// is stored, and so is what a render that started after the drop makes.
    #[test]
    fn entries_made_before_a_drop_are_refused_after_it() {
        use crate::reads::{FileStamp, ReadFile};
        let dir = tmpdir("drop-in-flight");
        std::fs::create_dir_all(&dir).unwrap();
        let file = |name: &str| {
            let p = dir.join(name);
            std::fs::write(&p, name.as_bytes()).unwrap();
            p.to_string_lossy().to_string()
        };
        let (a, b) = (file("a.png"), file("b.png"));
        let read = |paths: &[&str]| -> ReadFiles { paths.iter().map(|p| ReadFile::new(*p, FileStamp::native(p).unwrap())).collect() };
        let dc = DiskCache::open(dir.join("cache"), 1 << 30).unwrap();
        // The render starts (and reads a.png and b.png), then a.png is reloaded.
        let before = dc.drop_generation();
        dc.drop_files([a.clone()]);
        dc.flush();
        assert!(!dc.is_dropping());
        assert!(dc.dropped_since(&read(&[&a]), before) && !dc.dropped_since(&read(&[&b]), before));
        assert!(!dc.put_frame(1, 1, 1, &[1; 4], &read(&[&a, &b]), None, before), "a frame from before the drop");
        assert!(!dc.put_frame_f16(2, 1, 1, &[1; 4], &read(&[&a]), None, before));
        dc.put_layer(3, &Arc::new(buf(3, 2, 2)), &EntryFiles::of(&read(&[&a]), None).unwrap(), before);
        assert!(dc.put_frame(4, 1, 1, &[4; 4], &read(&[&b]), None, before), "b.png wasn't dropped");
        dc.flush();
        assert!(!dc.contains(Kind::Frame, 1) && !dc.contains(Kind::Frame, 2) && !dc.contains(Kind::Layer, 3));
        assert!(dc.get_frame(1, None).is_none() && dc.get_layer(3, None).is_none());
        assert_eq!(dc.get_frame(4, None).map(|f| f.0.rgba), Some(vec![4; 4]));
        // A render that starts after the drop reads the reloaded file: stored and served.
        let after = dc.drop_generation();
        assert!(after > before && !dc.dropped_since(&read(&[&a]), after));
        assert!(dc.put_frame(5, 1, 1, &[5; 4], &read(&[&a]), None, after));
        dc.put_layer(6, &Arc::new(buf(6, 2, 2)), &EntryFiles::of(&read(&[&a]), None).unwrap(), after);
        dc.flush();
        assert_eq!(dc.get_frame(5, None).map(|f| f.0.rgba), Some(vec![5; 4]));
        assert!(dc.get_layer(6, None).is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The record of dropped files is bounded: past it, everything made before the drop that
    /// forgot the record is refused, and what is made after it is stored.
    #[test]
    fn the_record_of_dropped_files_is_bounded() {
        let mut d = Drops::default();
        let a: Arc<str> = "/a.png".into();
        d.record(std::slice::from_ref(&a));
        assert!(d.stale([&a].into_iter(), 0) && !d.stale([&a].into_iter(), 1));
        let many: Vec<Arc<str>> = (0..=MAX_DROPPED_PATHS).map(|i| format!("/f{i}").into()).collect();
        d.record(&many);
        assert!(d.last.is_empty(), "forgotten, not grown");
        let other: Arc<str> = "/other.png".into();
        assert!(d.stale([&other].into_iter(), 1), "made before the drop that forgot the record");
        assert!(!d.stale([&a].into_iter(), 2), "made after it");
    }

    /// While a drop waits, an entry made of a dropped file is a miss (and deleted), and frames
    /// aren't linked to new keys.
    #[test]
    fn entries_of_a_pending_drop_are_misses() {
        use crate::reads::{FileStamp, ReadFile};
        let dir = tmpdir("drop-pending");
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.png");
        std::fs::write(&a, b"aa").unwrap();
        let a = a.to_string_lossy().to_string();
        let files: ReadFiles = [ReadFile::new(a.as_str(), FileStamp::native(&a).unwrap())].into_iter().collect();
        let dc = DiskCache::open(dir.join("cache"), 1 << 30).unwrap();
        dc.put_frame_now(1, 1, 1, &[1; 4], &files);
        dc.dropping.lock().unwrap().insert(a.as_str().into(), 1);
        assert!(!dc.link_frame(1, 2));
        assert!(dc.get_frame(1, None).is_none() && !dc.contains(Kind::Frame, 1));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An entry read while its file's drop was pending is a miss even when the writer ends the
    /// sweep between the read and the check (it deleted the file, but its bytes were read), and
    /// so is one read before a drop that starts meanwhile. Found under load: a stale hit.
    #[test]
    fn a_sweep_ending_during_a_read_never_makes_a_hit() {
        use crate::reads::{FileStamp, ReadFile};
        let dir = tmpdir("drop-race");
        let dc = DiskCache::open(dir.join("cache"), 1 << 30).unwrap();
        let files = EntryFiles::of(&[ReadFile::new("/a.png", FileStamp::Missing)].into_iter().collect(), None).unwrap();
        // Read while the drop is pending; the sweep ends before the check.
        dc.dropping.lock().unwrap().insert("/a.png".into(), 1);
        let drops = dc.drops_now();
        dc.dropping.lock().unwrap().clear();
        assert!(dc.made_of_dropped(&files, &drops));
        // Read before a drop that starts (and ends) before the check.
        let drops = dc.drops_now();
        dc.drop_files(["/a.png"]);
        dc.flush();
        assert!(dc.made_of_dropped(&files, &drops));
        assert!(!dc.made_of_dropped(&files, &dc.drops_now()), "read after the drop: used");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
