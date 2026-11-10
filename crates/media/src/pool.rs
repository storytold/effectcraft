//! [`MediaPool`]: decoded footage frames for the compositor, with a memory-budgeted LRU cache.

use std::cell::Cell;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};

use effectcraft_project::{AlphaMode, Footage, FootageKind, ItemId};
use effectcraft_raster::channels3d::AuxNeed;
use effectcraft_raster::{Image, Px};
use effectcraft_render::disk_cache::Frame8;
use effectcraft_render::reads::{FileStamp, ReadFile, ReadFiles, ReadLog};
use effectcraft_render::{FloatCurve, FootageSource};
use effectcraft_time::{FrameRate, TICKS_PER_SECOND, Tick};
use filmcraft_media::{FrameRequest, SharedSource};
use image::metadata::Orientation;

use crate::convert::{AlphaOp, dynamic_to_image, frame_to_image_in, frame_to_image_scaled, frame_to_rgba8, turned};
use crate::downmix::Order;
use crate::gif_anim::{self, Anim};
use crate::reads;
use crate::{MediaError, Result};

/// Default frame-cache budget: 1 GiB of decoded frames.
pub const DEFAULT_BUDGET: usize = 1 << 30;

/// The cache path of a missing image-sequence frame (no file can have this name).
const PLACEHOLDER: &str = "\0missing frame";

/// What a missing image-sequence frame shows: 75 % colour bars (white, yellow, cyan, green,
/// magenta, red, blue), the usual "no picture here" frame.
fn colour_bars(w: u32, h: u32) -> Image {
    const BARS: [[f32; 3]; 7] = [[1.0, 1.0, 1.0], [1.0, 1.0, 0.0], [0.0, 1.0, 1.0], [0.0, 1.0, 0.0], [1.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]];
    let (w, h) = (w.clamp(1, 16_384), h.clamp(1, 16_384));
    let row: Vec<Px> = (0..w)
        .map(|x| {
            let c = BARS.get((x as usize * BARS.len()) / w as usize).copied().unwrap_or([0.0; 3]);
            [c[0] * 0.75, c[1] * 0.75, c[2] * 0.75, 1.0]
        })
        .collect();
    Image { width: w, height: h, data: row.repeat(h as usize) }
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct Key {
    path: Arc<str>,
    frame: i64,
    /// The alpha interpretation (bits 0–1), how float values are encoded (bits 2–3: sRGB, as
    /// stored, a 2.4 power curve) and a placed PDF / Illustrator page's framing (bit 4,
    /// [`Footage::placed_page`]): they change the decoded pixels.
    interp: u8,
    matte: [u32; 3],
    /// A movie frame made at this size for a reduced-resolution render ((0, 0): as decoded).
    size: (u32, u32),
    /// A still at a reduced `size`: the footage's nominal size, which decides whether it is
    /// resampled (see `Inner::sized_still`); also at its own size for a placed PDF /
    /// Illustrator page, which is drawn to fill it ([`Footage::placed_page`]); (0, 0) otherwise.
    nominal: (u32, u32),
    /// An OpenEXR frame of which only these rows are read (a region of interest, #3271; the other
    /// rows are empty); `None`: all of it.
    rows: Option<(i64, i64)>,
    /// Kept as 8-bit levels ([`FootageSource::frame8`]), not float.
    q8: bool,
}

/// A cached frame's pixels: float, or the 8-bit levels an 8 bpc render quantises it to.
#[derive(Clone)]
enum Pixels {
    F32(Arc<Image>),
    Q8(Arc<Frame8>),
}

impl Pixels {
    fn bytes(&self) -> usize {
        match self {
            Pixels::F32(img) => img.data.len() * std::mem::size_of::<Px>(),
            Pixels::Q8(f) => f.rgba.len(),
        }
    }

    /// The float picture (`None` for 8-bit levels).
    fn f32(self) -> Option<Arc<Image>> {
        match self {
            Pixels::F32(img) => Some(img),
            Pixels::Q8(_) => None,
        }
    }
}

struct Entry {
    img: Pixels,
    /// The files the frame was decoded from (see [`crate::reads`]).
    files: Arc<ReadFiles>,
    stamp: u64,
    bytes: usize,
    /// When it was last used, for the memory accountant ([`effectcraft_render::memory::tick`]).
    used: u64,
    /// What decoding it took, in microseconds (a typical cost when not timed).
    cost_us: u64,
}

/// Conformed audio files written by builds before this version are ignored (and written again):
/// version 1 files of sources longer than about 12:36 at 48 kHz were silent from there on (#172);
/// version 2 files were written silent where the source failed to decode, and kept the footage
/// silent from then on (#324).
const CONFORM_VERSION: u32 = 3;

/// Footage longer than this (24 hours) is read from its decoder rather than conformed: its
/// duration comes from the file's header, and a damaged one claiming days of audio had the
/// conform thread allocate all of it at once (#408).
const MAX_CONFORM_SECONDS: i64 = 24 * 3600;

/// The sample rates audio is read at; a header outside them is damaged or hostile (#2141).
const MIN_SAMPLE_RATE: u32 = 1_000;
const MAX_SAMPLE_RATE: u32 = 768_000;

/// The sample frames of `footage`'s audio at `rate` Hz that a conformed file holds, `None` past
/// [`MAX_CONFORM_SECONDS`].
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn conform_frames(footage: &Footage, rate: u32) -> Option<usize> {
    let frames = footage.duration.to_units_floor(i64::from(rate)).max(0);
    if frames > MAX_CONFORM_SECONDS.saturating_mul(i64::from(rate)) {
        return None;
    }
    usize::try_from(frames).ok()
}

/// The source time of sample frame `at` at `rate` Hz. (`at` × ticks per second overflows `i64`
/// past about 12:36 at 48 kHz.)
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn sample_time(at: usize, rate: u32) -> Tick {
    Tick::from_units(i64::try_from(at).unwrap_or(i64::MAX), i64::from(rate))
}

/// At most this many recycled pixel buffers are kept (outside the budget).
const MAX_SPARE: usize = 4;

/// Least-recently-used frames, bounded by bytes.
#[derive(Default)]
struct Lru {
    map: HashMap<Key, Entry>,
    order: BTreeMap<u64, Key>,
    clock: u64,
    bytes: usize,
    /// Pixel buffers of evicted frames nobody else holds, reused for new frames: playback then
    /// writes into memory that is already mapped instead of faulting in fresh pages every frame.
    spare: Vec<Vec<Px>>,
}

impl Lru {
    /// The cached frame and its read files for `k`, marking the entry as most recently used.
    fn get(&mut self, k: &Key) -> Option<(Pixels, Arc<ReadFiles>)> {
        let e = self.map.get_mut(k)?;
        self.order.remove(&e.stamp);
        self.clock += 1;
        e.stamp = self.clock;
        e.used = effectcraft_render::memory::tick();
        self.order.insert(e.stamp, k.clone());
        Some((e.img.clone(), e.files.clone()))
    }
    /// Stores a frame with its read files under `k` and evicts the least recently used entries until the cache fits `budget` bytes.
    fn insert(&mut self, k: Key, img: Pixels, files: Arc<ReadFiles>, budget: usize) {
        self.insert_took(k, img, files, budget, None);
    }
    /// [`Lru::insert`] with what decoding the frame took in microseconds (`None`: not timed, a
    /// typical cost).
    fn insert_took(&mut self, k: Key, img: Pixels, files: Arc<ReadFiles>, budget: usize, took: Option<u64>) {
        let bytes = img.bytes() + 64;
        let cost_us = took.map_or_else(|| effectcraft_render::memory::default_cost(bytes, effectcraft_render::memory::DEFAULT_FRAME_US_PER_MB), |t| t.max(1));
        self.clock += 1;
        let used = effectcraft_render::memory::tick();
        if let Some(old) = self.map.insert(k.clone(), Entry { img, files, stamp: self.clock, bytes, used, cost_us }) {
            self.order.remove(&old.stamp);
            self.bytes -= old.bytes;
        }
        self.order.insert(self.clock, k);
        self.bytes += bytes;
        self.evict(budget);
    }
    fn remove(&mut self, k: &Key) {
        if let Some(e) = self.map.remove(k) {
            self.order.remove(&e.stamp);
            self.bytes -= e.bytes;
        }
    }
    /// Evict least-recently-used frames until within `budget` (the newest frame always stays).
    fn evict(&mut self, budget: usize) {
        while self.bytes > budget && self.map.len() > 1 {
            let Some((_, k)) = self.order.pop_first() else { break };
            if let Some(e) = self.map.remove(&k) {
                self.bytes -= e.bytes;
                // ponytail: float buffers only; add 8-bit spares if their allocation shows up in a profile
                if self.spare.len() < MAX_SPARE
                    && let Pixels::F32(img) = e.img
                    && let Ok(img) = Arc::try_unwrap(img)
                {
                    self.spare.push(img.data);
                }
            }
        }
    }
    /// The least recently used frame it may drop (the newest always stays) and its last use.
    fn oldest(&self) -> Option<(&Key, u64)> {
        if self.map.len() <= 1 {
            return None;
        }
        let (_, k) = self.order.first_key_value()?;
        Some((k, self.map.get(k)?.used))
    }
    /// Drop the least recently used frame it may drop; the bytes freed.
    fn drop_oldest(&mut self) -> usize {
        let Some(k) = self.oldest().map(|(k, _)| k.clone()) else { return 0 };
        let had = self.bytes;
        self.remove(&k);
        had.saturating_sub(self.bytes)
    }
    /// A recycled buffer of exactly `len` pixels, or an empty Vec.
    fn take_spare(&mut self, len: usize) -> Vec<Px> {
        match self.spare.iter().position(|b| b.len() == len) {
            Some(i) => self.spare.swap_remove(i),
            None => Vec::new(),
        }
    }
}

/// Cache counters (see [`MediaPool::stats`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PoolStats {
    /// Requests answered from the frame cache.
    pub hits: u64,
    /// Requests that had to decode (or wait for a read-ahead decode).
    pub misses: u64,
    /// Frames decoded ahead of sequential playback.
    pub prefetched: u64,
    /// Frames currently cached and their total size in bytes.
    pub frames: usize,
    pub bytes: usize,
    /// Movie and audio files opened (each path once, however many callers ask for it at once).
    pub opened: u64,
    /// Pictures decoded: a still's file (once for all the sizes asked for at the same time) or
    /// a movie frame. Requests for a picture being decoded wait for it instead (#3269).
    pub decodes: u64,
    /// Bytes of whole stills kept beside the frame cache for other sizes of the same frame (at
    /// most half the budget; not in `bytes`).
    pub recent_bytes: usize,
}

/// Whole stills decoded for a reduced size kept beside the frame cache ([`CacheState::recent`]):
/// at most this many bytes in all (and half the frame cache's budget)…
const RECENT_BYTES: usize = 512 << 20;
/// …and only while the frame cache has taken fewer than this many frames since.
const RECENT_AGE: u64 = 256;

/// How long a request waits for another thread's decode of the same picture before it decodes
/// a copy itself. A safety net only: decodes run on their own threads ([`decode_threads`]), so
/// no wait depends on the waiting thread or on other requests; only a hung decode takes this long.
#[cfg(not(target_arch = "wasm32"))]
const WAIT_LIMIT: std::time::Duration = std::time::Duration::from_secs(10);

/// How often a request asks again after the decode it waited for was called off for the
/// request that ran it (#329), before it decodes a copy itself.
const CANCELLED_RETRIES: u32 = 3;

#[derive(Default)]
struct CacheState {
    lru: Lru,
    /// Pictures being decoded right now, by what is decoded (a still's whole picture, a movie
    /// frame at its size): requests for them wait for that decode instead of decoding again.
    inflight: HashMap<Key, Arc<Flight>>,
    /// The last whole stills decoded for a reduced size (their keys have no size; see
    /// [`RECENT_BYTES`] and [`RECENT_AGE`]). Another size of the same frame shortly after (the
    /// Project panel's thumbnail of the comp at the viewer's time) is resampled from it instead of
    /// decoded again (#3269), and comes out as if decoded on its own. Not in the frame cache: a
    /// large plate shown reduced is not kept whole there (#1812).
    recent: VecDeque<Recent>,
    /// Counts `forget` calls (an epoch requests and decodes start in; see [`CacheState::stale`]).
    generation: u64,
    /// The generation each path was last forgotten at (at most [`MAX_FORGOTTEN`] paths): a
    /// request or decode of that path that started before it is stale (the file may have changed
    /// meanwhile), one of another path is not.
    forgotten: HashMap<Arc<str>, u64>,
    /// Requests and decodes that started before this generation are stale whatever their path
    /// (set when `forgotten` is cleared to keep it bounded).
    stale_before: u64,
    /// Last frame index requested per movie (sequential-playback detection).
    last: HashMap<Arc<str>, i64>,
    /// Movies whose frames can't be kept as 8-bit levels (more bits, or shown turned).
    wide: HashSet<Arc<str>>,
}

/// A whole still in [`CacheState::recent`].
struct Recent {
    key: Key,
    img: Arc<Image>,
    /// The files it was decoded from (see [`crate::reads`]).
    files: Arc<ReadFiles>,
    bytes: usize,
    /// The frame cache's clock when it was kept.
    stamp: u64,
    /// When it was kept ([`effectcraft_render::memory::tick`]).
    used: u64,
}

/// At most this many forgotten paths are remembered ([`CacheState::forgotten`]); beyond that
/// everything that started before is treated as stale instead.
const MAX_FORGOTTEN: usize = 4096;

impl CacheState {
    /// Whether what was made of `path` by a request or decode that started at `generation` is
    /// stale: the path was forgotten since (#3269 review: forgetting one file left every other
    /// file's decodes uncached and unshared).
    fn stale(&self, path: &str, generation: u64) -> bool {
        generation < self.stale_before || self.forgotten.get(path).is_some_and(|&at| at > generation)
    }

    /// Note that `path` was forgotten.
    fn forgot(&mut self, path: &str) {
        self.generation = self.generation.saturating_add(1);
        if self.forgotten.len() >= MAX_FORGOTTEN {
            self.forgotten.clear();
            self.stale_before = self.generation;
        }
        self.forgotten.insert(path.into(), self.generation);
    }

    /// The whole still `key` decoded recently, with the files it was decoded from, if it is
    /// still kept.
    fn recent(&mut self, key: &Key) -> Option<(Arc<Image>, Arc<ReadFiles>)> {
        let clock = self.lru.clock;
        self.recent.retain(|r| clock.saturating_sub(r.stamp) <= RECENT_AGE);
        self.recent.iter().find(|r| r.key == *key).map(|r| (r.img.clone(), r.files.clone()))
    }

    /// Keep the whole still `img` decoded for a reduced size, dropping the oldest beyond
    /// [`RECENT_BYTES`] (and half of `budget`).
    fn keep_recent(&mut self, key: Key, img: Arc<Image>, files: Arc<ReadFiles>, budget: usize) {
        let bytes = img.data.len().saturating_mul(std::mem::size_of::<Px>());
        self.recent.retain(|r| r.key != key);
        if bytes > Self::recent_cap(budget) {
            return;
        }
        let stamp = self.lru.clock;
        self.recent.push_back(Recent { key, img, files, bytes, stamp, used: effectcraft_render::memory::tick() });
        self.trim_recent(budget);
    }

    /// The most bytes of whole stills [`CacheState::recent`] holds with the frame cache's `budget`.
    fn recent_cap(budget: usize) -> usize {
        RECENT_BYTES.min(budget / 2)
    }

    /// Drop the oldest whole stills until the rest fit [`CacheState::recent_cap`] of `budget`
    /// (also when the budget shrinks).
    fn trim_recent(&mut self, budget: usize) {
        let cap = Self::recent_cap(budget);
        while self.recent_bytes() > cap {
            if self.recent.pop_front().is_none() {
                break;
            }
        }
    }

    /// The bytes the frame cache and the whole stills beside it hold (the memory holder's).
    fn held_bytes(&self) -> usize {
        self.lru.bytes.saturating_add(self.recent_bytes())
    }

    /// The bytes of the whole stills [`CacheState::recent`] holds.
    fn recent_bytes(&self) -> usize {
        self.recent.iter().map(|r| r.bytes).sum()
    }
}

/// What a decode ended with: the picture (or why not) and the files it read (see
/// [`crate::reads`]; also on failure, so a frame that failed names the files to watch).
type Outcome = (Result<Pixels>, Arc<ReadFiles>);

/// A decode in progress ([`CacheState::inflight`]): its outcome once it ends, for the requests
/// waiting on it.
#[derive(Default)]
struct Flight {
    /// [`CacheState::generation`] when it started: a request made after its file was forgotten
    /// (it changed on disk) doesn't join it, and decodes the file anew.
    generation: u64,
    outcome: Mutex<Option<Outcome>>,
    ready: Condvar,
}

thread_local! {
    /// Decodes running on this thread itself right now ([`Decoding`]; only where there are no
    /// decode threads). Such a thread doesn't wait for another decode: a rayon worker waiting on
    /// its decode's parallel work runs other jobs meanwhile, and one asking for a picture being
    /// decoded could wait on a decode further down its own stack.
    static OWNED: Cell<u32> = const { Cell::new(0) };
}

/// Marks a decode running on this thread ([`OWNED`]) until dropped (also when it panics).
struct Decoding;

impl Decoding {
    fn new() -> Self {
        OWNED.with(|n| n.set(n.get().saturating_add(1)));
        Decoding
    }
}

impl Drop for Decoding {
    fn drop(&mut self) {
        OWNED.with(|n| n.set(n.get().saturating_sub(1)));
    }
}

/// The threads footage pictures are decoded on (one per core, `ec-decode-N`), `None` where they
/// can't be started. Requests hand their decode to them and block until it is done (#3269):
/// a frame worker then never runs part of a decode (rayon would let it pick up another frame's
/// job meanwhile, which could wait on that very decode), and a thread decoding never runs a
/// frame job that waits on it. The decode's own parallel work runs on these threads, which
/// never wait for a picture.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn decode_threads() -> Option<&'static rayon::ThreadPool> {
    static THREADS: OnceLock<Option<rayon::ThreadPool>> = OnceLock::new();
    THREADS
        .get_or_init(|| {
            let n = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(4, 32);
            rayon::ThreadPoolBuilder::new()
                .num_threads(n)
                .thread_name(|i| format!("ec-decode-{i}"))
                // (the request waiting for it gets an error; rayon's default aborts)
                .panic_handler(|_| log::error!("media: a footage decode panicked; that frame shows nothing"))
                .build()
                .inspect_err(|e| log::warn!("media: no decode threads ({e}); footage decodes on the threads asking for it"))
                .ok()
        })
        .as_ref()
}

/// What waiting for another thread's decode ended with ([`Inner::wait`]).
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
enum Waited {
    /// Its outcome.
    Got(Outcome),
    /// This request was called off meanwhile (its own cancel flag, #329).
    Cancelled,
    /// This thread may not wait, or waited [`WAIT_LIMIT`]: it decodes a copy itself.
    No,
}

/// At most this many movie/audio files are kept open ([`Sources`]): the least recently used one
/// nobody is using is closed beyond it (it opens again when asked for; a project that cycles
/// through more files than this opens them again each time).
/// Still images and image sequences keep no file open (an OpenEXR file is opened for one decode).
const MAX_OPEN_SOURCES: usize = 256;

/// The opened movie/audio sources by path, with when each was last asked for.
#[derive(Default)]
struct Sources {
    map: HashMap<Arc<str>, (SourceSlot, u64)>,
    clock: u64,
}

impl Sources {
    /// The slot of `path` (new when there is none), as just used; the least recently used slot
    /// beyond [`MAX_OPEN_SOURCES`] is dropped.
    fn slot(&mut self, path: &Arc<str>) -> SourceSlot {
        self.clock = self.clock.wrapping_add(1);
        let clock = self.clock;
        let slot = {
            let e = self.map.entry(path.clone()).or_insert_with(|| (SourceSlot::default(), clock));
            e.1 = clock;
            e.0.clone()
        };
        // (only a slot nobody is using or opening: an open in progress isn't started twice)
        if self.map.len() > MAX_OPEN_SOURCES
            && let Some(oldest) = self.map.iter().filter(|(_, (s, _))| Arc::strong_count(s) == 1).min_by_key(|(_, (_, used))| *used).map(|(p, _)| p.clone())
        {
            self.map.remove(&oldest);
        }
        slot
    }
}

/// A movie/audio source by path ([`Opened`]), opened by the first caller while the others wait
/// for it (`None`: failed to open; not retried until `forget`), with the file's identity as
/// opened.
type SourceSlot = Arc<OnceLock<(Option<Opened>, ReadFile)>>;

/// An opened movie/audio source, the turn its picture is shown with and the order its decoded
/// audio channels arrive in.
#[derive(Clone)]
struct Opened {
    src: SharedSource,
    turn: Orientation,
    order: Order,
}

impl Opened {
    /// `src` with what its container says: `header` is an MP4/MOV's ftyp and moov (or the whole
    /// file), `file` the file for other containers' AAC configuration.
    fn new(src: SharedSource, header: Option<&[u8]>, file: Option<&dyn filmcraft_media::ByteReader>) -> Opened {
        let turn = header.map_or(Orientation::NoTransforms, crate::probe::video_turn);
        Opened { order: channel_order(&src, header, file), src, turn }
    }
}

/// The order `src`'s audio channels decode in; multichannel AAC's by its configuration, where
/// the container's header shows it ([`crate::aac::config`]).
fn channel_order(src: &SharedSource, header: Option<&[u8]>, file: Option<&dyn filmcraft_media::ByteReader>) -> Order {
    match src.info().audio() {
        Some(a) if a.codec.contains("AAC") && a.channels > 2 => crate::aac::config(header, file).map_or(Order::Aac, |asc| crate::downmix::aac_order(&asc)),
        _ => Order::Wav,
    }
}

/// An animated GIF by path, as [`SourceSlot`] (the error message when it failed to load).
type GifSlot = Arc<OnceLock<(std::result::Result<Arc<Anim>, String>, ReadFile)>>;

struct Inner {
    /// Opened movie/audio sources by path, at most [`MAX_OPEN_SOURCES`].
    sources: Mutex<Sources>,
    /// Files opened into `sources` (see [`PoolStats::opened`]).
    opened: AtomicU64,
    /// In-memory files registered with `add_bytes` (web builds, tests).
    files: Mutex<HashMap<String, Arc<[u8]>>>,
    /// Parsed 3D models by path (`None`: failed to load; not retried until `forget`), with the
    /// files read for them (the model's and the resources it names).
    models: Mutex<HashMap<String, (Option<Arc<effectcraft_model::Model>>, Arc<ReadFiles>)>>,
    /// Decoded animated GIFs and WebPs by path, loaded by the first caller while the others wait (an
    /// `Err` is not retried until `forget`).
    gifs: Mutex<HashMap<Arc<str>, GifSlot>>,
    cache: Mutex<CacheState>,
    budget: AtomicU64,
    read_ahead: AtomicBool,
    hits: AtomicU64,
    misses: AtomicU64,
    prefetched: AtomicU64,
    /// See [`PoolStats::decodes`].
    decodes: AtomicU64,
    /// Requests waiting for another thread's decode right now (tests).
    #[cfg(test)]
    waiting: std::sync::atomic::AtomicUsize,
    /// Runs before each decode; `Some` answers in its place (tests).
    #[cfg(test)]
    gate: Mutex<Option<Gate>>,
    /// Settings ▸ Disk ▸ Conformed Audio Folder: decoded audio tracks written once per file and
    /// rate, read back instead of decoding again (`None` = off).
    conform: Mutex<Option<std::path::PathBuf>>,
    /// Conformed files being written right now.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    conforming: Mutex<HashSet<std::path::PathBuf>>,
    /// Conformed files that failed to write (the source didn't decode) this session: their
    /// audio is read from the decoder instead of trying again on every read.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    conform_failed: Mutex<HashSet<std::path::PathBuf>>,
    /// Movies that failed to open or decode (path to a readable reason; see
    /// [`FootageSource::problem`]). The first failure of a file is logged, later ones are not.
    problems: Mutex<HashMap<Arc<str>, String>>,
    /// OpenEXR footage (by path) whose channels effects read (EXtractoR, Cryptomatte, the 3D
    /// Channel effects): its pictures come from the same cached decode (#474).
    aux_footage: Mutex<AuxFootage>,
}

/// [`Inner::gate`].
#[cfg(test)]
type Gate = Arc<dyn Fn() -> Option<Result<Image>> + Send + Sync>;

/// Footage whose channels effects read ([`Inner::aux_footage`]), by [`Footage::path`].
#[derive(Default)]
struct AuxFootage {
    /// What the project says ([`FootageSource::set_aux_footage`]).
    given: HashSet<String>,
    /// Read through [`FootageSource::aux`] but not given (a reader the project doesn't name, or
    /// a source that never gives a set). Cleared with each new set.
    learned: HashSet<String>,
    /// What the effects reading each footage's channels read of them
    /// ([`FootageSource::set_aux_needs`], #3271); a footage read without one reads them all.
    needs: HashMap<String, AuxNeed>,
}

impl AuxFootage {
    fn contains(&self, path: &str) -> bool {
        self.given.contains(path) || self.learned.contains(path)
    }

    /// The channels to read with the picture of footage `path`: what its effects read, all of
    /// them when they didn't say, none (`None`) when no effect reads them.
    fn plan(&self, path: &str) -> Option<AuxNeed> {
        self.contains(path).then(|| self.needs.get(path).cloned().unwrap_or_default())
    }
}

/// Decodes footage for the renderer: stills, image sequences and movies (FilmCraft codecs), plus
/// audio. Thread-safe and cheap to clone (clones share decoders and cache).
///
/// - One decoder per movie: FilmCraft's GOP cache keeps it positioned, so sequential frames
///   decode without re-seeking, and a seek decodes forward from the preceding sync sample.
/// - Converted frames live in an LRU cache bounded by a byte budget ([`DEFAULT_BUDGET`]).
/// - Sequential playback reads one frame ahead on a worker thread (native builds), so decoding
///   and conversion of the next frame overlap with the caller's use of the current one.
/// - Concurrent requests for the same frame decode it once: the first decodes, the others wait
///   for its picture (#3269). A still asked for at several sizes is decoded once and resampled
///   to each.
#[derive(Clone)]
pub struct MediaPool {
    inner: Arc<Inner>,
}

impl Default for MediaPool {
    fn default() -> Self {
        Self::new()
    }
}

/// Where a frame comes from.
#[derive(Clone)]
struct Loc {
    key: Key,
    /// Media time for movies (`None`: an image file, `key.path`).
    media_t: Option<Tick>,
    /// How float values are encoded (also in `key.interp`).
    float: FloatCurve,
}

/// The request decoding a picture ([`CacheState::inflight`]). Dropped, it keeps the picture
/// (in the frame cache, or a reduced still's whole picture in [`CacheState::recent`]; not when
/// the file was forgotten meanwhile), ends the flight and hands its outcome to the requests
/// waiting on it: an error when the decode panicked or returned early, so none waits for a
/// picture that never comes.
struct Owner<'a> {
    inner: &'a Inner,
    /// When the decode started ([`effectcraft_render::memory::tick`]: what it took is the frame's
    /// cost to the memory accountant).
    started: u64,
    key: Key,
    flight: Arc<Flight>,
    /// The picture is the one asked for (else a still's whole picture, resampled for the request).
    as_asked: bool,
    /// [`CacheState::generation`] when the decode started.
    generation: u64,
    outcome: Option<Outcome>,
}

impl Drop for Owner<'_> {
    fn drop(&mut self) {
        // (a decode that stopped early can't name the files it read)
        let outcome = self.outcome.take().unwrap_or_else(|| {
            let mut files = ReadFiles::default();
            files.mark_incomplete();
            (Err(MediaError::Decode(format!("{}: the decode stopped", self.key.path))), Arc::new(files))
        });
        let budget = self.inner.budget();
        let mut c = lock(&self.inner.cache);
        let had = c.held_bytes();
        if let (Ok(img), files) = &outcome
            && !c.stale(&self.key.path, self.generation)
        {
            if self.as_asked {
                let took = effectcraft_render::memory::tick().saturating_sub(self.started);
                let mut key = self.key.clone();
                // (asked as 8-bit levels but not 8-bit: kept as the float frame it is, which the
                // renderer asks for next)
                if key.q8 && matches!(img, Pixels::F32(_)) {
                    c.wide.insert(key.path.clone());
                    key.q8 = false;
                }
                c.lru.insert_took(key, img.clone(), files.clone(), budget, Some(took));
            } else if let Pixels::F32(whole) = img {
                c.keep_recent(self.key.clone(), whole.clone(), files.clone(), budget);
            }
        }
        let grown = c.held_bytes().saturating_sub(had);
        if c.inflight.get(&self.key).is_some_and(|f| Arc::ptr_eq(f, &self.flight)) {
            c.inflight.remove(&self.key);
        }
        drop(c);
        *lock(&self.flight.outcome) = Some(outcome);
        self.flight.ready.notify_all();
        effectcraft_render::memory::Accountant::global().note_growth(grown);
    }
}

impl MediaPool {
    /// A pool with the [`DEFAULT_BUDGET`] frame cache.
    pub fn new() -> Self {
        Self::with_budget(DEFAULT_BUDGET)
    }

    /// A pool whose frame cache holds at most `bytes` of decoded frames, in the common memory
    /// budget ([`effectcraft_render::memory::Accountant`]).
    pub fn with_budget(bytes: usize) -> Self {
        let pool = Self::unregistered(bytes);
        let holder: Arc<dyn effectcraft_render::memory::MemoryHolder> = pool.inner.clone();
        effectcraft_render::memory::Accountant::global().register(&holder);
        pool
    }

    fn unregistered(bytes: usize) -> Self {
        Self {
            inner: Arc::new(Inner {
                sources: Mutex::default(),
                files: Mutex::default(),
                models: Mutex::default(),
                gifs: Mutex::default(),
                cache: Mutex::default(),
                budget: AtomicU64::new(bytes as u64),
                read_ahead: AtomicBool::new(cfg!(not(target_arch = "wasm32"))),
                hits: AtomicU64::new(0),
                misses: AtomicU64::new(0),
                prefetched: AtomicU64::new(0),
                decodes: AtomicU64::new(0),
                #[cfg(test)]
                waiting: std::sync::atomic::AtomicUsize::new(0),
                #[cfg(test)]
                gate: Mutex::default(),
                opened: AtomicU64::new(0),
                conform: Mutex::default(),
                conforming: Mutex::default(),
                conform_failed: Mutex::default(),
                problems: Mutex::default(),
                aux_footage: Mutex::default(),
            }),
        }
    }

    pub fn budget(&self) -> usize {
        self.inner.budget()
    }

    /// Keep a frame of `width` by `height` as the frame of `path` (tests of the footage budget).
    #[cfg(test)]
    pub(crate) fn keep_test_frame(&self, path: &str, width: u32, height: u32) {
        let key = Key { path: path.into(), frame: 0, interp: 0, matte: [0; 3], size: (0, 0), nominal: (0, 0), rows: None, q8: false };
        let budget = self.inner.budget();
        lock(&self.inner.cache).lru.insert(key, Pixels::F32(Arc::new(Image::new(width, height))), Arc::default(), budget);
    }

    /// The paths of the frames [`MediaPool::keep_test_frame`] kept that are still cached.
    #[cfg(test)]
    pub(crate) fn test_frames(&self) -> Vec<String> {
        let c = lock(&self.inner.cache);
        let mut v: Vec<String> = c.lru.map.keys().map(|k| k.path.to_string()).collect();
        v.sort();
        v
    }

    /// The frame cache as the memory accountant sees it (tests).
    #[cfg(test)]
    pub(crate) fn memory_holder(&self) -> Arc<dyn effectcraft_render::memory::MemoryHolder> {
        self.inner.clone()
    }

    /// Change the frame-cache budget (evicts immediately when shrinking).
    pub fn set_budget(&self, bytes: usize) {
        self.inner.budget.store(bytes as u64, Ordering::Relaxed);
        let mut c = lock(&self.inner.cache);
        c.lru.evict(bytes);
        c.trim_recent(bytes);
    }

    /// Enable or disable reading one frame ahead during sequential playback (on by default on
    /// native builds; ignored on wasm32).
    pub fn set_read_ahead(&self, on: bool) {
        self.inner.read_ahead.store(on && cfg!(not(target_arch = "wasm32")), Ordering::Relaxed);
    }

    pub fn stats(&self) -> PoolStats {
        let i = &self.inner;
        let c = lock(&i.cache);
        PoolStats {
            hits: i.hits.load(Ordering::Relaxed),
            misses: i.misses.load(Ordering::Relaxed),
            prefetched: i.prefetched.load(Ordering::Relaxed),
            frames: c.lru.map.len(),
            bytes: c.lru.bytes,
            opened: i.opened.load(Ordering::Relaxed),
            decodes: i.decodes.load(Ordering::Relaxed),
            recent_bytes: c.recent_bytes(),
        }
    }

    /// Drop every cached frame (decoders stay open).
    pub fn clear_frames(&self) {
        let mut c = lock(&self.inner.cache);
        c.lru = Lru::default();
        c.recent.clear();
        c.last.clear();
        drop(c);
        // Decoded GIFs sit outside the frame budget; Purge frees them too.
        lock(&self.inner.gifs).clear();
    }

    /// Drop every cached frame and decoder.
    pub fn clear(&self) {
        self.clear_frames();
        lock(&self.inner.sources).map.clear();
        lock(&self.inner.problems).clear();
    }

    /// Forget a file (after it changed on disk, or to retry a file that failed to open).
    pub fn forget(&self, path: &str) {
        lock(&self.inner.sources).map.remove(path);
        lock(&self.inner.models).remove(path);
        lock(&self.inner.gifs).remove(path);
        lock(&self.inner.problems).remove(path);
        let mut c = lock(&self.inner.cache);
        let keys: Vec<Key> = c.lru.map.keys().filter(|k| &*k.path == path).cloned().collect();
        for k in keys {
            c.lru.remove(&k);
        }
        c.recent.retain(|r| &*r.key.path != path);
        c.forgot(path);
        c.last.remove(path);
        c.wide.remove(path);
        drop(c);
        crate::exr_planes::forget(path);
    }

    /// Provide the contents of `path` from memory (used instead of the file system; web builds).
    pub fn add_bytes(&self, path: &str, bytes: Arc<[u8]>) {
        lock(&self.inner.files).insert(path.to_string(), bytes);
        self.forget(path);
    }

    /// Decode (or fetch from cache) the frame of `footage` at source time `t`.
    pub fn frame_at(&self, footage: &Footage, t: Tick) -> Result<Arc<Image>> {
        Inner::frame_at(&self.inner, footage, t, true, None, FloatCurve::Srgb)
    }

    /// [`FootageSource::frame8`]: `None` (the renderer then asks for the float frame) for
    /// anything but an 8-bit movie frame shown as decoded, and on errors.
    fn frame8_of(&self, footage: &Footage, t: Tick, float: FloatCurve) -> Option<Arc<Frame8>> {
        if footage.missing || footage.kind != FootageKind::Video || gif_anim::is_animated(footage) {
            return None;
        }
        if lock(&self.inner.cache).wide.contains(footage.path.as_str()) {
            return None;
        }
        match Inner::frame_px(&self.inner, footage, t, true, None, float, true, None) {
            Ok(Pixels::Q8(f)) => Some(f),
            _ => None,
        }
    }

    /// [`FootageSource::frame`] / [`FootageSource::frame_at_size`]: `None` (logged) on errors.
    fn frame_sized(&self, _item: ItemId, footage: &Footage, t: Tick, size: Option<(u32, u32)>, float: FloatCurve) -> Option<Arc<Image>> {
        if footage.missing {
            return None;
        }
        match Inner::frame_at(&self.inner, footage, t, true, size, float) {
            Ok(img) => Some(img),
            // nobody wants the frame any more (`cancellable`): not an error
            Err(MediaError::Cancelled) => None,
            // (a movie's failure is logged once per file, when it is recorded)
            Err(e) => {
                if lock(&self.inner.problems).get(footage.path.as_str()).is_none() {
                    log::warn!("media: frame of {} at {:?}: {e}", footage.path, t);
                }
                None
            }
        }
    }

    /// Settings ▸ Disk ▸ Conformed Audio Folder: write each footage file's decoded audio there
    /// once per sample rate (raw interleaved stereo `f32`) and read it back instead of decoding
    /// again. `None` turns it off.
    pub fn set_conform_folder(&self, folder: Option<std::path::PathBuf>) {
        *lock(&self.inner.conform) = folder;
    }

    /// The conformed-audio file of `path` at `rate` (named by a hash of the path, size and
    /// modification time, so an edited file conforms again, and of `CONFORM_VERSION`). Notes
    /// `path` as read with that identity (see [`crate::reads`]): its samples come from it.
    pub fn conformed_path(&self, path: &str, rate: u32) -> Option<std::path::PathBuf> {
        use std::hash::{Hash, Hasher};
        let folder = lock(&self.inner.conform).clone()?;
        let meta = std::fs::metadata(path).ok()?;
        reads::note(ReadFile::new(path, FileStamp::of_metadata(&meta)));
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (CONFORM_VERSION, path, meta.len(), meta.modified().ok()).hash(&mut h);
        Some(folder.join(format!("{:016x}_{rate}.ecaf", h.finish())))
    }

    /// Samples from a conformed file: `Some` when it exists and covers the request.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    fn read_conformed(file: &std::path::Path, s0: usize, frames: usize) -> Option<Vec<f32>> {
        use std::io::{Read, Seek, SeekFrom};
        let mut f = std::fs::File::open(file).ok()?;
        let len = f.metadata().ok()?.len() as usize / 8;
        let mut out = vec![0.0f32; frames * 2];
        if s0 >= len {
            return Some(out);
        }
        let n = frames.min(len - s0);
        f.seek(SeekFrom::Start(s0 as u64 * 8)).ok()?;
        let mut bytes = vec![0u8; n * 8];
        f.read_exact(&mut bytes).ok()?;
        for (o, c) in out.iter_mut().zip(bytes.as_chunks::<4>().0.iter()) {
            *o = f32::from_le_bytes([c[0], c[1], c[2], c[3]]);
        }
        Some(out)
    }

    /// Conform `frames` sample frames of `footage`'s audio at `rate` into `file` on a worker
    /// thread (once).
    #[cfg(not(target_arch = "wasm32"))]
    fn conform(&self, footage: &Footage, file: std::path::PathBuf, frames: usize, rate: u32) {
        if lock(&self.inner.conform_failed).contains(&file) || !lock(&self.inner.conforming).insert(file.clone()) {
            return;
        }
        let (pool, f) = (self.clone(), footage.clone());
        let spawned = std::thread::Builder::new().name("ec-conform-audio".into()).spawn(move || {
            let tmp = file.with_extension("tmp");
            let written = match file.parent().map(std::fs::create_dir_all) {
                Some(Ok(())) => pool.write_conformed(&f, &tmp, frames, rate).and_then(|()| std::fs::rename(&tmp, &file)),
                Some(Err(e)) => Err(e),
                None => Err(std::io::Error::other("no folder")),
            };
            // A failed decode leaves no file: one written silent would keep the footage silent
            // in every later session (#324).
            if let Err(e) = written {
                let _ = std::fs::remove_file(&tmp);
                log::warn!("media: could not write conformed audio {} of {}: {e}", file.display(), f.path);
                lock(&pool.inner.conform_failed).insert(file.clone());
            }
            lock(&pool.inner.conforming).remove(&file);
        });
        if spawned.is_err() {
            lock(&self.inner.conforming).clear();
        }
    }

    /// Write `frames` sample frames of `footage`'s audio at `rate` to `path` (raw interleaved
    /// stereo `f32`) a chunk at a time: the whole track was held in memory before it was written.
    /// Fails when any chunk doesn't decode.
    #[cfg(not(target_arch = "wasm32"))]
    fn write_conformed(&self, footage: &Footage, path: &std::path::Path, frames: usize, rate: u32) -> std::io::Result<()> {
        use std::io::Write;
        let mut out = std::io::BufWriter::new(std::fs::File::create(path)?);
        let mut at = 0usize;
        while at < frames {
            let n = (frames - at).min(1 << 16);
            for v in self.try_decode_audio(footage, sample_time(at, rate), n, rate).map_err(std::io::Error::other)? {
                out.write_all(&v.to_le_bytes())?;
            }
            at += n;
        }
        out.flush()
    }

    /// `frames` stereo sample frames of `footage`'s audio starting at source time `start`, at
    /// `rate` Hz, interleaved (L R L R …). Mono is duplicated to both channels; multichannel
    /// sources are downmixed to stereo ([`crate::downmix`]). Silence where there is no audio (or before the media starts).
    /// With a conformed audio folder, reads come from the conformed file once it is written.
    pub fn audio_samples(&self, footage: &Footage, start: Tick, frames: usize, rate: u32) -> Vec<f32> {
        if !footage.has_audio || footage.missing || rate == 0 || frames == 0 {
            return vec![0.0; frames * 2];
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(conform) = conform_frames(footage, rate)
            && let Some(file) = self.conformed_path(&footage.path, rate)
        {
            let s0 = start.to_units_floor(rate as i64);
            if file.exists() && !lock(&self.inner.conforming).contains(&file) {
                let skip = (-s0).max(0) as usize;
                if skip >= frames {
                    return vec![0.0; frames * 2];
                }
                if let Some(v) = Self::read_conformed(&file, s0.max(0) as usize, frames - skip) {
                    let mut out = vec![0.0; skip * 2];
                    out.extend(v);
                    return out;
                }
            } else {
                self.conform(footage, file, conform, rate);
            }
        }
        self.decode_audio(footage, start, frames, rate)
    }

    /// [`MediaPool::audio_samples`] straight from the decoder. The source is read at its own
    /// rate and resampled here (linear, by absolute sample position, so a range comes out the
    /// same however it is split into reads). FilmCraft's MP4 reader, asked for another rate,
    /// applied the edit list's priming offset twice and ramped the start of every read: AAC
    /// footage not at the mix rate stuttered in previews and exports (#274).
    fn decode_audio(&self, footage: &Footage, start: Tick, frames: usize, rate: u32) -> Vec<f32> {
        self.try_decode_audio(footage, start, frames, rate).unwrap_or_else(|e| {
            log::warn!("media: audio of {}: {e}", footage.path);
            vec![0.0; frames * 2]
        })
    }

    /// [`MediaPool::decode_audio`], failing when the source can't be opened or read.
    fn try_decode_audio(&self, footage: &Footage, start: Tick, frames: usize, rate: u32) -> std::result::Result<Vec<f32>, String> {
        let path: Arc<str> = footage.path.as_str().into();
        let Opened { src, order, .. } = self.inner.opened(&path).ok_or("the file can't be opened")?;
        let s0 = start.to_units_floor(rate as i64);
        let native = src.info().audio().map_or(rate, |a| a.sample_rate);
        // The header's rate scales the samples read: 0xFFFFFFFF Hz made one conform chunk about
        // 47 GB (#2141). Real audio is 1–768 kHz.
        if native != 0 && !(MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&native) {
            return Err(format!("a sample rate of {native} Hz is outside {MIN_SAMPLE_RATE}–{MAX_SAMPLE_RATE} Hz"));
        }
        if native == 0 || rate == 0 || native == rate {
            return read_stereo(&src, &order, s0, frames, rate);
        }
        // Output sample `n` sits at source position n × native / rate.
        let (native_i, rate_i) = (i128::from(native), i128::from(rate));
        let first = (i128::from(s0) * native_i).div_euclid(rate_i);
        let last = ((i128::from(s0) + frames as i128) * native_i).div_euclid(rate_i) + 1;
        let (Ok(first64), Ok(count)) = (i64::try_from(first), usize::try_from(last - first + 1)) else { return Ok(vec![0.0; frames * 2]) };
        let buf = read_stereo(&src, &order, first64, count, native)?;
        let mut out = Vec::with_capacity(frames * 2);
        for k in 0..frames as i128 {
            let pos = (i128::from(s0) + k) * native_i;
            let i = usize::try_from(pos.div_euclid(rate_i) - first).unwrap_or(0) * 2;
            let f = (pos.rem_euclid(rate_i) as f64 / rate as f64) as f32;
            for c in 0..2 {
                let a = buf.get(i + c).copied().unwrap_or(0.0);
                let b = buf.get(i + 2 + c).copied().unwrap_or(a);
                out.push(a + (b - a) * f);
            }
        }
        Ok(out)
    }

    /// A thumbnail of `footage` (its first frame) fitting in `max_side` × `max_side`, aspect kept.
    pub fn thumbnail(&self, footage: &Footage, max_side: u32) -> Result<Image> {
        let img = Inner::frame_at(&self.inner, footage, Tick::ZERO, false, None, FloatCurve::Srgb)?;
        let (w, h) = (img.width.max(1), img.height.max(1));
        let s = (max_side.max(1) as f64 / w.max(h) as f64).min(1.0);
        if s >= 1.0 {
            return Ok((*img).clone());
        }
        let (tw, th) = (((w as f64 * s).round() as u32).max(1), ((h as f64 * s).round() as u32).max(1));
        Ok(effectcraft_raster::resample(&img, tw, th))
    }
}

impl Inner {
    fn budget(&self) -> usize {
        self.budget.load(Ordering::Relaxed) as usize
    }

    /// A 3D model file and its sibling resources (buffers, textures, MTL files), parsed once.
    fn model(&self, path: &str) -> Option<Arc<effectcraft_model::Model>> {
        let found = lock(&self.models).get(path).cloned();
        if let Some((m, files)) = found {
            reads::note_all(&files);
            return m;
        }
        let (loaded, files) = reads::captured(|| {
            self.read(path).map_err(|e| e.to_string()).and_then(|bytes| {
                // Files the model names are read only when local or on the model's own network share.
                let resolve = |uri: &str| -> Option<Vec<u8>> {
                    let p = craft_docpath::resolve_absolute(uri, Some(std::path::Path::new(path))).ok()?;
                    self.read(&p.to_string_lossy()).ok().map(|b| b.to_vec())
                };
                effectcraft_model::load(path, &bytes, &resolve).map_err(|e| e.to_string())
            })
        });
        let m = match loaded {
            Ok(m) => Some(Arc::new(m)),
            Err(e) => {
                log::warn!("media: cannot load model {path}: {e}");
                None
            }
        };
        let (m, files) = lock(&self.models).entry(path.to_string()).or_insert((m, Arc::new(files))).clone();
        reads::note_all(&files);
        m
    }

    /// The bytes of the file at `path`, noted as read (see [`crate::reads`]).
    fn read(&self, path: &str) -> Result<Arc<[u8]>> {
        let (bytes, file) = self.read_file(path);
        reads::note(file);
        bytes
    }

    /// Where the OpenEXR file at `path` is read from: registered bytes, else the file.
    fn exr_source(&self, path: &str) -> crate::exr_read::ExrSource {
        use crate::exr_read::ExrSource;
        if let Some(b) = lock(&self.files).get(path) {
            return ExrSource::Bytes(b.clone());
        }
        let file = std::path::PathBuf::from(path);
        // A network file: copied in the background when the localizer is on, and read from its
        // copy once that is complete and the file unchanged (#3271).
        if crate::localize::is_network(&file) && crate::localize::status().is_some() {
            crate::localize::request(&file);
            // (the identity as read within the last moment: one check per file and frame, #3334)
            if let Ok(stamp) = crate::exr_read::recent_stamp(&file)
                && let Some(copy) = crate::localize::local_copy(&file, &stamp)
            {
                return ExrSource::Localized { original: file, copy, stamp };
            }
        }
        ExrSource::File(file)
    }

    /// The bytes of the file at `path` and its identity as read ([`FileStamp::Missing`] when it
    /// can't be read), not noted.
    fn read_file(&self, path: &str) -> (Result<Arc<[u8]>>, ReadFile) {
        let in_memory = lock(&self.files).get(path).cloned();
        if let Some(b) = in_memory {
            return (Ok(b), ReadFile::new(path, FileStamp::Virtual));
        }
        match crate::read_regular_file_stamped(std::path::Path::new(path)) {
            Ok((bytes, stamp)) => (Ok(bytes.into()), ReadFile::new(path, stamp)),
            Err(e) => (Err(MediaError::Io(format!("{path}: {e}"))), ReadFile::new(path, FileStamp::Missing)),
        }
    }

    /// The opened movie/audio source for `path` ([`Opened`]). The first caller opens it, outside
    /// the map's lock (reading and parsing a large file takes a moment; FLAC, MP3, Ogg and AIFF
    /// files are decoded whole); callers asking for the same file meanwhile wait for that instead
    /// of opening it again: a preview's frame workers, audio feeder, conform and waveform threads
    /// each decoded a long FLAC in full at once (#408).
    /// Each call notes the file as read (see [`crate::reads`]), with its identity as opened.
    fn opened(&self, path: &Arc<str>) -> Option<Opened> {
        let slot = lock(&self.sources).slot(path);
        let (opened, file) = slot.get_or_init(|| self.open(path));
        reads::note(file.clone());
        opened.clone()
    }

    /// Open the movie/audio file at `path` (see [`Inner::opened`]), with its identity as opened.
    fn open(&self, path: &Arc<str>) -> (Option<Opened>, ReadFile) {
        self.opened.fetch_add(1, Ordering::Relaxed);
        // Files on disk are read in place (the index, then samples on demand) rather than
        // whole; in-memory files (`add_bytes`) and wasm32 builds open from bytes.
        let in_memory = lock(&self.files).contains_key(&**path);
        let file = std::path::Path::new(&**path);
        let in_place = if in_memory { None } else { crate::stream::open_source(file) };
        let in_place = in_place.map(|(src, stamp)| {
            let (header, reader) = (crate::stream::movie_header(file), crate::stream::file_reader(file));
            (src.map(|src| Opened::new(src, header.as_deref(), reader.as_deref())), ReadFile::new(path.clone(), stamp))
        });
        let (opened, read) = in_place.unwrap_or_else(|| {
            let (bytes, read) = self.read_file(path);
            let opened = bytes.and_then(|bytes| {
                let name = std::path::Path::new(&**path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                let src = filmcraft_codecs::open_bytes(&name, bytes.clone()).map_err(MediaError::from)?;
                Ok(Opened::new(src, Some(&bytes), Some(&filmcraft_media::reader::MemReader(bytes.clone()))))
            });
            (opened, read)
        });
        match opened {
            Ok(s) => (Some(s), read),
            Err(e) => {
                self.note_problem(path, "", &e);
                (None, read)
            }
        }
    }

    /// Remember why the movie at `path` cannot be shown (`codec` is the footage's, when known)
    /// and log it the first time. I/O errors (missing or unreadable file) are not decoder
    /// problems and are left to the missing-footage handling.
    fn note_problem(&self, path: &Arc<str>, codec: &str, e: &MediaError) {
        // A decode called off for a newer frame (#329) says nothing about the file either.
        if matches!(e, MediaError::Io(_) | MediaError::Cancelled) {
            return;
        }
        let reason = describe_problem(codec, e);
        if lock(&self.problems).insert(path.clone(), reason.clone()).is_none() {
            log::warn!("media: {path}: {reason}");
        }
    }

    /// The decoded frames of the animated GIF or WebP at `path`. The first caller decodes it, outside the
    /// map's lock; callers asking for the same file meanwhile wait for that.
    fn gif(&self, path: &Arc<str>) -> Result<Arc<Anim>> {
        let slot = lock(&self.gifs).entry(path.clone()).or_default().clone();
        let (loaded, file) = slot.get_or_init(|| {
            let (bytes, file) = self.read_file(path);
            let loaded = bytes.and_then(|bytes| Anim::load(&bytes)).map(Arc::new).map_err(|e| {
                log::warn!("media: cannot decode {path}: {e}");
                e.to_string()
            });
            (loaded, file)
        });
        reads::note(file.clone());
        loaded.clone().map_err(|e| MediaError::Decode(format!("{path}: {e}")))
    }

    /// The frame index (in the footage's interpreted rate) shown at source time `t`, after
    /// looping (`loop_count`) and clamping to the footage's frames; `n` = frames per loop.
    fn frame_index(rate: FrameRate, t: Tick, n: i64, loops: u32) -> i64 {
        let n = n.max(1);
        let total = n.saturating_mul(loops.max(1) as i64);
        rate.frame_at(t).clamp(0, total - 1) % n
    }

    /// Number of frames in `d` at `rate` (rounded).
    fn frame_count(rate: FrameRate, d: Tick) -> i64 {
        let den = TICKS_PER_SECOND as i128 * rate.den as i128;
        ((d.0 as i128 * rate.num as i128 + den / 2) / den) as i64
    }

    /// Where the frame of `footage` at `t` comes from (file, frame or media time) and its cache
    /// key, at the footage's own size.
    fn locate(footage: &Footage, t: Tick, float: FloatCurve) -> Loc {
        let alpha = match footage.alpha {
            AlphaMode::Straight => 0,
            AlphaMode::Premultiplied => 1,
            AlphaMode::Ignore => 2,
        };
        let float = AlphaOp::of(footage, float).float_curve();
        let interp = alpha
            | (match float {
                FloatCurve::Srgb => 0,
                FloatCurve::Linear => 1,
                FloatCurve::Gamma24 => 2,
            } << 2)
            | (u8::from(footage.placed_page) << 4);
        let matte = if footage.alpha == AlphaMode::Premultiplied { footage.premul_color.map(f32::to_bits) } else { [0; 3] };
        let key = |path: &str, frame| Key { path: path.into(), frame, interp, matte, size: (0, 0), nominal: (0, 0), rows: None, q8: false };
        match footage.kind {
            FootageKind::Sequence if !footage.sequence.is_empty() => {
                let i = Self::frame_index(footage.frame_rate, t, footage.sequence_frames(), footage.loop_count);
                match footage.sequence_file_held(i) {
                    Some(path) => Loc { key: key(path, 0), media_t: None, float },
                    // No file at or before the frame: colour bars.
                    None => Loc { key: key(PLACEHOLDER, ((footage.width as i64) << 32) | footage.height as i64), media_t: None, float },
                }
            }
            // A layer of a layered still is keyed by its layer index and size mode (smart objects
            // by their embedded file; PDF pages by their number).
            FootageKind::Still if footage.layer.is_some() || footage.page != 0 => {
                let l = footage.layer.as_ref().map_or(0, |l| 1 + l.index as i64 * 2 + l.layer_size as i64 + if l.embedded.is_some() { 1 << 40 } else { 0 })
                    + ((footage.page as i64) << 42);
                Loc { key: key(&footage.path, l), media_t: None, float }
            }
            FootageKind::Still | FootageKind::Sequence | FootageKind::Model | FootageKind::Data => Loc { key: key(&footage.path, 0), media_t: None, float },
            FootageKind::Video | FootageKind::Audio => {
                let rate = footage.frame_rate;
                let n = Self::frame_count(rate, footage.duration);
                let i = Self::frame_index(rate, t, n, footage.loop_count);
                // A quarter into the frame in the file's own timing: FilmCraft rounds a request to
                // the track's nearest timestamp unit (halves up), so the middle of a frame would
                // land on the next one when the timescale is the frame rate; a quarter still
                // tolerates timestamps rounded either way.
                let native = footage.native_rate.unwrap_or(rate);
                let mt = native.tick_of(i) + Tick(native.frame_duration().0 / 4);
                Loc { key: key(&footage.path, i), media_t: Some(mt), float }
            }
        }
    }

    /// The frame of `footage` at `t`, resampled to `size` (see [`FootageSource::frame_at_size`]):
    /// a movie's made at that size directly, a still's resampled once and kept at that size.
    ///
    /// Each picture is decoded once however many threads ask for it at the same time (#3269):
    /// the first decodes it and the others wait for its picture. A still is decoded whole and
    /// resampled to each size asked for, so a second size of the same frame (the Project panel's
    /// thumbnail beside a reduced viewer) comes from the same decode, the frame cache's whole
    /// picture or a recent one ([`CacheState::recent`]), and is the same as decoded on its own.
    fn frame_at(self: &Arc<Self>, footage: &Footage, t: Tick, playback: bool, size: Option<(u32, u32)>, float: FloatCurve) -> Result<Arc<Image>> {
        self.frame_at_rows(footage, t, playback, size, float, None)
    }

    /// Whether a request for some rows of `footage` at `t` reads only those: an OpenEXR frame
    /// (not a movie) whose channels no effect reads.
    fn reads_rows(&self, footage: &Footage, t: Tick) -> bool {
        let loc = Self::locate(footage, t, FloatCurve::Srgb);
        loc.media_t.is_none() && crate::exr_planes::is_exr(&loc.key.path) && lock(&self.aux_footage).plan(&footage.path).is_none()
    }

    /// [`Inner::frame_at`] of only the rows `rows` of an OpenEXR frame whose channels no effect
    /// reads (#3271; others are read whole).
    fn frame_at_rows(
        self: &Arc<Self>,
        footage: &Footage,
        t: Tick,
        playback: bool,
        size: Option<(u32, u32)>,
        float: FloatCurve,
        rows: Option<(i64, i64)>,
    ) -> Result<Arc<Image>> {
        match Self::frame_px(self, footage, t, playback, size, float, false, rows)? {
            Pixels::F32(img) => Ok(img),
            // (a float request's key never holds 8-bit levels)
            Pixels::Q8(_) => Err(MediaError::Decode(format!("{}: 8-bit frame for a float request", footage.path))),
        }
    }

    /// [`Self::frame_at`], kept as 8-bit levels when `q8` is set and the frame has 8 bits (a
    /// float frame under the float key otherwise, and the movie is remembered as `wide`).
    #[allow(clippy::too_many_arguments)]
    fn frame_px(
        self: &Arc<Self>,
        footage: &Footage,
        t: Tick,
        playback: bool,
        size: Option<(u32, u32)>,
        float: FloatCurve,
        q8: bool,
        rows: Option<(i64, i64)>,
    ) -> Result<Pixels> {
        if !footage.has_video {
            return Err(MediaError::Unsupported(format!("{}: no video", footage.path)));
        }
        let mut loc = Self::locate(footage, t, float);
        loc.key.q8 = q8;
        let movie = loc.media_t.is_some();
        loc.key.rows = rows.filter(|_| self.reads_rows(footage, t));
        // A still asked for at its own size is the unsized frame (one decode, one entry).
        if let Some(size) = size.filter(|s| movie || *s != (footage.width, footage.height)) {
            loc.key.size = size;
            if !movie {
                loc.key.nominal = (footage.width, footage.height);
            }
        }
        // A placed PDF / Illustrator page is drawn to fill the footage's size (`layered::decode`,
        // #3099): two items of one file stored at different sizes have different pixels.
        if footage.placed_page && !movie {
            loc.key.nominal = (footage.width, footage.height);
        }
        // Some rows of an OpenEXR frame (a region of interest, #3271): the whole frame when the
        // cache has it, else those rows read for this request alone (not kept: a frame cache
        // entry of a few rows would hold a whole frame's memory).
        if loc.key.rows.is_some() {
            return self.frame_rows_of(&loc, footage).map(Pixels::F32);
        }
        // What is decoded: a movie frame at its size, a still whole (its key has no size; a
        // placed page keeps the footage's size, which it is drawn to fill).
        let whole_nominal = if footage.placed_page { (footage.width, footage.height) } else { (0, 0) };
        let source = Loc {
            key: if movie { loc.key.clone() } else { Key { size: (0, 0), nominal: whole_nominal, ..loc.key.clone() } },
            media_t: loc.media_t,
            float: loc.float,
        };
        let as_asked = source.key == loc.key;
        // The picture asked for, made of the decoded one.
        let shape = |whole: Pixels| match whole {
            Pixels::F32(img) if !movie => Pixels::F32(Self::sized_still(img, &loc, footage)),
            whole => whole,
        };
        let mut c = lock(&self.cache);
        let generation = c.generation;
        // sequential playback of a movie: read the next frame ahead
        if playback && movie {
            self.prefetch_next(&mut c, &loc, footage, t, size, float);
        }
        // (Each exit notes the files the frame came from, see `crate::reads`; a cached frame
        // notes those it was decoded from, without touching them.)
        let mut retries = 0;
        loop {
            if let Some((img, files)) = c.lru.get(&loc.key) {
                self.hits.fetch_add(1, Ordering::Relaxed);
                drop(c);
                reads::note_all(&files);
                return Ok(img);
            }
            // A still decoded already for another size: resampled from its whole picture.
            if !movie {
                let whole = c.lru.get(&source.key).or_else(|| c.recent(&source.key).map(|(img, files)| (Pixels::F32(img), files)));
                if let Some((whole, files)) = whole {
                    drop(c);
                    self.hits.fetch_add(1, Ordering::Relaxed);
                    reads::note_all(&files);
                    return Ok(self.keep(&loc.key, shape(whole), files, generation));
                }
            }
            // (one started before the file was forgotten is left to its own requests)
            let Some(flight) = c.inflight.get(&source.key).filter(|f| !c.stale(&source.key.path, f.generation)).cloned() else { break };
            // Another request (or the read-ahead) is decoding this picture: wait for it.
            drop(c);
            self.misses.fetch_add(1, Ordering::Relaxed);
            match self.wait(&flight) {
                // (kept also when it is the picture decoded: a still's whole picture decoded for
                // another size is not in the frame cache)
                Waited::Got((Ok(whole), files)) => {
                    reads::note_all(&files);
                    return Ok(self.keep(&loc.key, shape(whole), files, generation));
                }
                // The other request was called off (#329), not this one: ask again (and decode
                // it here if nobody else does by now).
                Waited::Got((Err(MediaError::Cancelled), _)) if retries < CANCELLED_RETRIES => retries += 1,
                // Called off again and again, or this thread may not wait (or waited too long):
                // decode a copy.
                Waited::Got((Err(MediaError::Cancelled), _)) | Waited::No => {
                    let (img, files) = self.run_decode(&source, footage);
                    reads::note_all(&files);
                    return Ok(shape(img?));
                }
                Waited::Got((Err(e), files)) => {
                    reads::note_all(&files);
                    return Err(e);
                }
                Waited::Cancelled => return Err(MediaError::Cancelled),
            }
            c = lock(&self.cache);
        }
        self.misses.fetch_add(1, Ordering::Relaxed);
        let flight = Arc::new(Flight { generation, ..Flight::default() });
        c.inflight.insert(source.key.clone(), flight.clone());
        drop(c);
        let mut owner =
            Owner { inner: self, started: effectcraft_render::memory::tick(), key: source.key.clone(), flight, as_asked, generation, outcome: None };
        let (decoded, files) = self.run_decode(&source, footage);
        let files = Arc::new(files);
        reads::note_all(&files);
        owner.outcome = Some((decoded.clone(), files.clone()));
        // (keeps the picture and hands it to the waiting requests)
        drop(owner);
        let whole = decoded?;
        Ok(if as_asked { whole } else { self.keep(&loc.key, shape(whole), files, generation) })
    }

    /// [`Inner::frame_at_rows`] of a request for rows (`loc.key.rows`): the whole frame at that
    /// size or the whole still from the frame cache, else the rows decoded now.
    fn frame_rows_of(self: &Arc<Self>, loc: &Loc, footage: &Footage) -> Result<Arc<Image>> {
        let whole = Key { rows: None, ..loc.key.clone() };
        let still = Key { size: (0, 0), nominal: (0, 0), ..whole.clone() };
        let mut c = lock(&self.cache);
        if let Some((img, files)) = c.lru.get(&whole).and_then(|(p, f)| Some((p.f32()?, f))) {
            drop(c);
            self.hits.fetch_add(1, Ordering::Relaxed);
            reads::note_all(&files);
            return Ok(img);
        }
        if let Some((img, files)) = c.lru.get(&still).and_then(|(p, f)| Some((p.f32()?, f))).or_else(|| c.recent(&still)) {
            drop(c);
            self.hits.fetch_add(1, Ordering::Relaxed);
            reads::note_all(&files);
            return Ok(Self::sized_still(img, loc, footage));
        }
        drop(c);
        self.misses.fetch_add(1, Ordering::Relaxed);
        let source = Loc { key: Key { size: (0, 0), nominal: (0, 0), ..loc.key.clone() }, media_t: None, float: loc.float };
        let (img, files) = self.run_decode(&source, footage);
        reads::note_all(&files);
        match img? {
            Pixels::F32(img) => Ok(Self::sized_still(img, loc, footage)),
            Pixels::Q8(_) => Err(MediaError::Decode(format!("{}: 8-bit frame for a still", loc.key.path))),
        }
    }

    /// Keep `img`, made of `files`, in the frame cache as `key` (unless its file was forgotten
    /// since `generation`, see [`CacheState::stale`]), and return it.
    fn keep(&self, key: &Key, img: Pixels, files: Arc<ReadFiles>, generation: u64) -> Pixels {
        let mut c = lock(&self.cache);
        let had = c.held_bytes();
        if !c.stale(&key.path, generation) {
            c.lru.insert(key.clone(), img.clone(), files, self.budget());
        }
        let grown = c.held_bytes().saturating_sub(had);
        drop(c);
        effectcraft_render::memory::Accountant::global().note_growth(grown);
        img
    }

    /// Wait for the outcome of another request's decode (`flight`).
    ///
    /// Waiting can't deadlock: decodes run on their own threads ([`decode_threads`]), which never
    /// wait for a picture, so a decode doesn't depend on the requests waiting for it. Where there
    /// are no decode threads, a thread running a decode itself ([`OWNED`]) doesn't wait. The time
    /// limit is a safety net for a hung decode.
    #[cfg(not(target_arch = "wasm32"))]
    fn wait(&self, flight: &Flight) -> Waited {
        if OWNED.with(Cell::get) > 0 {
            log::debug!("media: a picture asked for during a decode on this thread is being decoded elsewhere; decoding a copy");
            return Waited::No;
        }
        let until = std::time::Instant::now() + WAIT_LIMIT;
        #[cfg(test)]
        self.waiting.fetch_add(1, Ordering::SeqCst);
        let mut outcome = lock(&flight.outcome);
        let waited = loop {
            if let Some(o) = outcome.as_ref() {
                break Waited::Got(o.clone());
            }
            if filmcraft_media::cancel::cancelled() {
                break Waited::Cancelled;
            }
            let left = until.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                log::warn!("media: a picture took over {} s to decode on another thread; decoding a copy", WAIT_LIMIT.as_secs());
                break Waited::No;
            }
            // (in slices, to notice this request being called off)
            outcome = match flight.ready.wait_timeout(outcome, left.min(std::time::Duration::from_millis(20))) {
                Ok((g, _)) => g,
                Err(p) => p.into_inner().0,
            };
        };
        drop(outcome);
        #[cfg(test)]
        self.waiting.fetch_sub(1, Ordering::SeqCst);
        waited
    }

    /// (A wasm32 build has one thread: a picture in flight there is being decoded further down
    /// this thread's own stack.)
    #[cfg(target_arch = "wasm32")]
    fn wait(&self, _flight: &Flight) -> Waited {
        Waited::No
    }

    /// Sequential playback of a movie (`loc`, at `t`): read the frame after it ahead.
    fn prefetch_next(self: &Arc<Self>, c: &mut CacheState, loc: &Loc, footage: &Footage, t: Tick, size: Option<(u32, u32)>, float: FloatCurve) {
        let prev = c.last.insert(loc.key.path.clone(), loc.key.frame);
        if prev == Some(loc.key.frame - 1) && self.read_ahead.load(Ordering::Relaxed) {
            let next_t = t + footage.frame_rate.frame_duration();
            let mut next = Self::locate(footage, next_t, float);
            next.key.size = loc.key.size;
            next.key.q8 = loc.key.q8;
            if next.key != loc.key && !c.lru.map.contains_key(&next.key) && !c.inflight.contains_key(&next.key) {
                self.prefetch(footage, next_t, size, float, loc.key.q8);
            }
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn prefetch(self: &Arc<Self>, footage: &Footage, t: Tick, size: Option<(u32, u32)>, float: FloatCurve, q8: bool) {
        let (me, f) = (self.clone(), footage.clone());
        self.prefetched.fetch_add(1, Ordering::Relaxed);
        // A plain thread, not a rayon job: it waits for the decode.
        std::thread::spawn(move || {
            let _ = me.frame_px(&f, t, false, size, float, q8, None);
        });
    }

    #[cfg(target_arch = "wasm32")]
    fn prefetch(self: &Arc<Self>, _footage: &Footage, _t: Tick, _size: Option<(u32, u32)>, _float: FloatCurve, _q8: bool) {}

    /// A still (image file or sequence frame) at `loc.key.size` when one is set, made of its
    /// whole decoded picture `img`: resampled as the renderer would ([`FootageSource::frame_at_size`]),
    /// so the pool keeps the reduced frame and a reduced-resolution render neither decodes nor
    /// resamples a large still again for every frame (#1812: 21120-px plates at Quarter). Like the
    /// renderer's `footage_buf`, an image decoded larger than the footage's nominal size (a smart
    /// object's pixels) is resampled too, and one decoded smaller is not (the renderer scales the
    /// layer instead).
    fn sized_still(img: Arc<Image>, loc: &Loc, footage: &Footage) -> Arc<Image> {
        let (w, h) = loc.key.size;
        let smaller = img.width == 0 || footage.width as f64 / img.width as f64 > 1.0 + 1e-9;
        if w == 0 || h == 0 || smaller || (w, h) == (img.width, img.height) {
            return img;
        }
        Arc::new(effectcraft_raster::resample(&img, w, h))
    }

    /// Decode `loc`'s picture ([`Inner::decode_source`]) on the decode threads
    /// ([`decode_threads`]) and block until it is done; on this thread where there are none
    /// (wasm32) or this is one of them. With the files the decode read (see [`crate::reads`]):
    /// collected on the thread that decodes, not noted anywhere (the caller notes them).
    fn run_decode(self: &Arc<Self>, loc: &Loc, footage: &Footage) -> (Result<Pixels>, ReadFiles) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(threads) = decode_threads()
            && threads.current_thread_index().is_none()
        {
            let (tx, rx) = std::sync::mpsc::sync_channel(1);
            let (me, at, f) = (self.clone(), loc.clone(), footage.clone());
            // The request's cancel flag and decoding hints go with it (#329).
            let hints = filmcraft_media::cancel::Context::current();
            threads.spawn(move || {
                #[cfg(not(test))]
                let decoded = reads::captured(|| hints.run(|| me.decode_source(&at, &f)));
                // (the decodes counted on that thread are this thread's)
                #[cfg(test)]
                let decoded = crate::exr_channels::counting(|| reads::captured(|| hints.run(|| me.decode_source(&at, &f))));
                let _ = tx.send(decoded);
            });
            // A plain block: this thread runs nothing else meanwhile.
            return match rx.recv() {
                #[cfg(not(test))]
                Ok(decoded) => decoded,
                #[cfg(test)]
                Ok((decoded, counted)) => {
                    crate::exr_channels::add_decodes(counted);
                    decoded
                }
                // (the decode panicked: what it read is unknown)
                Err(_) => {
                    let mut files = ReadFiles::default();
                    files.mark_incomplete();
                    (Err(MediaError::Decode(format!("{}: the decode stopped", loc.key.path))), files)
                }
            };
        }
        let _decoding = Decoding::new();
        reads::captured(|| self.decode_source(loc, footage))
    }

    /// Decode the picture `loc` names: a still whole, a movie frame at its size.
    fn decode_source(&self, loc: &Loc, footage: &Footage) -> Result<Pixels> {
        self.decodes.fetch_add(1, Ordering::Relaxed);
        #[cfg(test)]
        {
            // (not holding the lock while it runs)
            let gate = lock(&self.gate).clone();
            if let Some(answer) = gate.and_then(|g| g()) {
                return answer.map(|img| Pixels::F32(Arc::new(img)));
            }
        }
        match loc.media_t {
            None => self.decode_still(loc, footage).map(|img| Pixels::F32(Arc::new(img))),
            Some(_) => self.decode_media(loc, footage),
        }
    }

    /// Decodes the still image at `loc` and applies the footage alpha and float handling.
    /// A placeholder path decodes to colour bars.
    fn decode_still(&self, loc: &Loc, footage: &Footage) -> Result<Image> {
        let op = AlphaOp::of(footage, loc.float);
        let path = &loc.key.path;
        if &**path == PLACEHOLDER {
            return Ok(colour_bars(footage.width, footage.height));
        }
        // OpenEXR: the picture's channels from the plane store, read chunk by chunk (#3271), with
        // the channels effects read when they read this footage's (#474).
        if crate::exr_planes::is_exr(path) {
            use crate::exr_planes::Picture;
            let extra = lock(&self.aux_footage).plan(&footage.path);
            let picture = match loc.key.rows {
                Some(rows) if extra.is_none() => crate::exr_planes::picture_rows(path, &self.exr_source(path), op, rows),
                _ => crate::exr_planes::picture(path, &self.exr_source(path), op, extra.as_ref()),
            };
            match picture {
                Picture::Done(Ok(img)) => return Ok(img),
                Picture::Done(Err(why)) => return Err(MediaError::Decode(format!("{path}: {why}"))),
                Picture::Cancelled => return Err(MediaError::Cancelled),
                // (the `image` crate decodes the whole file)
                Picture::Legacy => {}
            }
        }
        let (bytes, file) = self.read_file(path);
        reads::note(file);
        let bytes = bytes?;
        if let Some(img) = crate::dpx::decode(path, &bytes)? {
            return Ok(dynamic_to_image(&img, op));
        }
        if let Some(img) = crate::layered::decode(path, &bytes, footage, op)? {
            return Ok(img);
        }
        // (an OpenEXR compression the decoder lacks came out transparent or as a missing
        // channel error)
        if path.to_ascii_lowercase().ends_with(".exr")
            && let Some(why) = crate::exr_channels::unsupported_compression(&bytes)
        {
            return Err(MediaError::Decode(format!("{path}: {why}")));
        }
        // A hostile OpenEXR header (a data window a billion pixels wide over one tiny chunk) made
        // the decoder allocate gigabytes for a block: refused before any decoder sees it.
        if crate::still_format(path, &bytes) == Some(image::ImageFormat::OpenExr) && !crate::exr_index::safe_to_decode(&bytes) {
            return Err(MediaError::Decode(format!("{path}: an OpenEXR part larger than this reader takes")));
        }
        #[cfg(test)]
        crate::exr_channels::count_decode(false);
        match crate::probe::load_oriented(&bytes, crate::still_format(path, &bytes)) {
            Ok(img) => Ok(dynamic_to_image(&img, op)),
            // A multi-layer OpenEXR file without an unnamed RGB layer: its colour layer.
            Err(e) => match crate::exr_channels::layered_image(&bytes) {
                Some(img) => Ok(dynamic_to_image(&img, op)),
                None => Err(MediaError::Decode(format!("{path}: {e}"))),
            },
        }
    }

    fn decode_media(&self, loc: &Loc, footage: &Footage) -> Result<Pixels> {
        let op = AlphaOp::of(footage, loc.float);
        let path = &loc.key.path;
        let f32 = |img| Pixels::F32(Arc::new(img));
        match loc.media_t {
            None => self.decode_still(loc, footage).map(f32),
            Some(mt) if gif_anim::is_animated(footage) => {
                let anim = self.gif(path)?;
                let frame = anim.frame_at(Tick(mt.0)).ok_or_else(|| MediaError::Decode(format!("{path}: no frames")))?;
                Ok(f32(dynamic_to_image(frame, op)))
            }
            Some(mt) => {
                let Opened { src, turn, .. } = self.opened(path).ok_or_else(|| MediaError::Io(format!("{path}: cannot open")))?;
                let vf = src
                    .video_frame(FrameRequest::full(filmcraft_time::Tick(mt.0)))
                    .map_err(MediaError::from)
                    .inspect_err(|e| self.note_problem(path, &footage.codec, e))?;
                // A movie shown turned a quarter (a phone held upright): the footage's size and
                // the render's are the turned picture's, the decoded frame's are the file's.
                let side = |(w, h): (u32, u32)| if crate::probe::swaps_sides(turn) { (h, w) } else { (w, h) };
                // A reduced-resolution render's frame, made at its size (a frame of the footage's
                // own size only, as the renderer resamples only those).
                let (w, h) = side(loc.key.size);
                if w > 0
                    && (vf.width, vf.height) == side((footage.width, footage.height))
                    && let Some(img) = frame_to_image_scaled(&vf, w, h)
                {
                    return Ok(f32(turned(img, turn)));
                }
                if loc.key.q8
                    && turn == Orientation::NoTransforms
                    && let Some(f) = frame_to_rgba8(&vf, op)
                {
                    return Ok(Pixels::Q8(Arc::new(f)));
                }
                let buf = lock(&self.cache).lru.take_spare(vf.width as usize * vf.height as usize);
                Ok(f32(turned(frame_to_image_in(&vf, op, buf), turn)))
            }
        }
    }
}

/// The decoded frames in the common memory budget (#3271, #3298): each frame is worth what
/// decoding it took, per byte; the whole stills kept to resample other sizes shortly after
/// ([`CacheState::recent`]) are worth nothing.
impl effectcraft_render::memory::MemoryHolder for Inner {
    fn name(&self) -> &'static str {
        "footage frames"
    }
    fn bytes(&self) -> usize {
        lock(&self.cache).held_bytes()
    }
    /// The older of the least recently used frame and the oldest whole still kept beside the
    /// frames (#3271: whichever was used last goes first); a whole still is kept for a moment
    /// only, to resample other sizes, so it is offered as worth nothing.
    fn oldest(&self) -> Option<effectcraft_render::memory::Candidate> {
        use effectcraft_render::memory::Candidate;
        let c = lock(&self.cache);
        let frame =
            c.lru.oldest().and_then(|(k, _)| c.lru.map.get(k)).map(|e| Candidate { used: e.used, bytes: e.bytes, cost_us: e.cost_us, protected: false });
        let still = c.recent.front().map(|r| Candidate { used: r.used, bytes: r.bytes.max(1), cost_us: 0, protected: false });
        match (frame, still) {
            (Some(f), Some(s)) => Some(if s.used < f.used { s } else { f }),
            (f, s) => f.or(s),
        }
    }
    fn drop_oldest(&self) -> usize {
        let mut c = lock(&self.cache);
        let frame = c.lru.oldest().map(|(_, t)| t);
        let still = c.recent.front().map(|r| r.used);
        match (frame, still) {
            (Some(f), Some(s)) if s < f => c.recent.pop_front().map_or(0, |r| r.bytes.max(1)),
            (None, Some(_)) => c.recent.pop_front().map_or(0, |r| r.bytes.max(1)),
            _ => c.lru.drop_oldest(),
        }
    }
}

impl MediaPool {
    /// The OpenEXR file holding the channels of `footage` at `t` (a still or a sequence frame).
    fn aux_path(&self, footage: &Footage, t: Tick) -> Option<Arc<str>> {
        if footage.missing || !footage.has_video {
            return None;
        }
        let loc = Inner::locate(footage, t, FloatCurve::Srgb);
        if loc.media_t.is_some() || !crate::exr_planes::is_exr(&loc.key.path) {
            return None;
        }
        Some(loc.key.path)
    }
}

impl FootageSource for MediaPool {
    fn set_conform_folder(&self, folder: Option<std::path::PathBuf>) {
        MediaPool::set_conform_folder(self, folder);
    }
    fn model(&self, _item: ItemId, footage: &Footage) -> Option<Arc<effectcraft_model::Model>> {
        if footage.missing || footage.kind != FootageKind::Model {
            return None;
        }
        self.inner.model(&footage.path)
    }
    /// The most the footage frames, and the decoded OpenEXR planes, hold by themselves (#3298:
    /// the whole common budget on the desktop, the browser's caps there; the memory accountant
    /// keeps them within the budget with the other caches).
    fn set_cache_budget(&self, bytes: usize) {
        self.set_budget(bytes);
        crate::exr_planes::set_budget(bytes);
    }
    fn purge(&self) {
        self.clear_frames();
    }
    fn forget(&self, path: &str) {
        MediaPool::forget(self, path);
    }
    fn cache_budget(&self) -> Option<usize> {
        Some(self.budget())
    }
    /// A movie's decoder gives up on a seek (before it, and between samples while it decodes
    /// forward from the keyframe) once `cancel` is set: a scrub's superseded frames no longer
    /// hold the decoder the frame under the pointer waits for (#329).
    fn cancellable(&self, cancel: &Arc<AtomicBool>, f: &mut dyn FnMut()) {
        filmcraft_media::cancel::with_cancel(cancel, f)
    }
    /// Every read of a file notes it (see [`crate::reads`]): a frame's image file or sequence
    /// frame, a movie, a model and the files it names, an EXR's channels, a movie's audio. A
    /// frame (or other entry) served from memory notes the files it was made of, as they were
    /// when read, without reading them again. A missing sequence frame (colour bars) reads none.
    fn recording(&self, reads: &ReadLog, f: &mut dyn FnMut()) {
        reads::recording(reads, f)
    }
    fn frame(&self, item: ItemId, footage: &Footage, t: Tick) -> Option<Arc<Image>> {
        self.frame_sized(item, footage, t, None, FloatCurve::Srgb)
    }
    fn frame_at_size(&self, item: ItemId, footage: &Footage, t: Tick, width: u32, height: u32) -> Option<Arc<Image>> {
        self.frame_sized(item, footage, t, Some((width, height)), FloatCurve::Srgb)
    }
    fn frame_with(&self, item: ItemId, footage: &Footage, t: Tick, size: Option<(u32, u32)>, float: FloatCurve) -> Option<Arc<Image>> {
        self.frame_sized(item, footage, t, size, float)
    }
    fn frame_rows(&self, _item: ItemId, footage: &Footage, t: Tick, size: Option<(u32, u32)>, float: FloatCurve, rows: (i64, i64)) -> Option<Arc<Image>> {
        if footage.missing {
            return None;
        }
        Inner::frame_at_rows(&self.inner, footage, t, true, size, float, Some(rows)).ok()
    }
    /// Only an OpenEXR frame whose channels no effect reads is read by rows (others whole, see
    /// [`Inner::frame_at_rows`]).
    fn reads_rows(&self, footage: &Footage, t: Tick) -> bool {
        !footage.missing && self.inner.reads_rows(footage, t)
    }
    fn frame8(&self, _item: ItemId, footage: &Footage, t: Tick, float: FloatCurve) -> Option<Arc<Frame8>> {
        self.frame8_of(footage, t, float)
    }

    fn problem(&self, footage: &Footage) -> Option<String> {
        if footage.missing || footage.kind != FootageKind::Video {
            return None;
        }
        lock(&self.inner.problems).get(footage.path.as_str()).cloned()
    }

    fn vector_frame(&self, _item: ItemId, footage: &Footage, scale: f64) -> Option<Arc<Image>> {
        if footage.missing || !effectcraft_render::is_vector_footage(footage) {
            return None;
        }
        let bytes = self.inner.read(&footage.path).ok()?;
        crate::layered::rasterize_vector(&footage.path, &bytes, footage, scale).map(Arc::new)
    }

    fn set_aux_footage(&self, paths: &[String]) {
        let given: HashSet<String> = paths.iter().cloned().collect();
        let mut wanted = lock(&self.inner.aux_footage);
        if given != wanted.given || !wanted.learned.is_empty() {
            crate::exr_planes::release_held();
        }
        wanted.given = given;
        wanted.learned.clear();
    }

    fn set_localizer(&self, config: Option<effectcraft_render::LocalizeConfig>) {
        crate::localize::configure(config);
    }

    fn localize_status(&self) -> Option<effectcraft_render::LocalizeReport> {
        crate::localize::status()
    }

    fn set_disk_cache(&self, cache: Option<Arc<effectcraft_render::disk_cache::DiskCache>>) {
        crate::exr_planes::set_disk_cache(cache.as_ref());
    }

    fn set_aux_needs(&self, needs: &[(String, AuxNeed)]) {
        let mut wanted = lock(&self.inner.aux_footage);
        let needs: HashMap<String, AuxNeed> = needs.iter().cloned().collect();
        if needs != wanted.needs {
            crate::exr_planes::release_held();
        }
        wanted.needs = needs;
    }

    /// Every channel, the picture's too (no render asks for that: renders ask for what their
    /// effects read, [`FootageSource::aux_for`]).
    fn aux(&self, _item: ItemId, footage: &Footage, t: Tick) -> Option<Arc<effectcraft_raster::AuxChannels>> {
        let path = self.aux_path(footage, t)?;
        crate::exr_planes::every_channel(&path, &self.inner.exr_source(&path))
    }

    fn aux_names(&self, _item: ItemId, footage: &Footage, t: Tick) -> Option<Arc<effectcraft_raster::AuxChannels>> {
        let path = self.aux_path(footage, t)?;
        crate::exr_planes::names(&path, &self.inner.exr_source(&path))
    }

    fn aux_for(&self, _item: ItemId, footage: &Footage, t: Tick, need: &AuxNeed) -> Option<Arc<effectcraft_raster::AuxChannels>> {
        let path = self.aux_path(footage, t)?;
        {
            let mut wanted = lock(&self.inner.aux_footage);
            if !wanted.contains(&footage.path) {
                // (a bound: names of footage items, cleared otherwise only by a new set)
                if wanted.learned.len() >= 4096 {
                    wanted.learned.clear();
                }
                wanted.learned.insert(footage.path.clone());
            }
        }
        crate::exr_planes::aux(&path, &self.inner.exr_source(&path), need)
    }

    fn audio(&self, _item: ItemId, footage: &Footage, t: Tick, frames: usize, rate: u32) -> Option<Vec<f32>> {
        if !footage.has_audio || footage.missing {
            return None;
        }
        Some(self.audio_samples(footage, t, frames, rate))
    }
}

/// `frames` stereo sample frames (interleaved) of `src` from sample `s0` at `rate` Hz, which
/// FilmCraft serves as decoded when it is the source's own rate. Mono is duplicated to both
/// channels, multichannel sources (channels in `order`) are downmixed ([`crate::downmix`]); silence before the start. An error where decoding fails.
fn read_stereo(src: &SharedSource, order: &Order, s0: i64, frames: usize, rate: u32) -> std::result::Result<Vec<f32>, String> {
    let mut out = vec![0.0; frames * 2];
    let skip = usize::try_from(s0.saturating_neg()).unwrap_or(0);
    if skip >= frames {
        return Ok(out);
    }
    let buf = src.audio(s0.max(0), frames - skip, rate).map_err(|e| e.to_string())?;
    let Some(dst) = skip.checked_mul(2).and_then(|i| out.get_mut(i..)) else { return Ok(out) };
    crate::downmix::downmix_into(&buf.channels, order, dst);
    Ok(out)
}

/// The one-line reason a movie shows no picture: its `codec` (the probe's, e.g. "H.264 (High
/// 4:2:2)") and what the decoder said, without the error wrappers it passed through.
fn describe_problem(codec: &str, e: &MediaError) -> String {
    let mut detail = match e {
        MediaError::Io(s) | MediaError::Unsupported(s) | MediaError::Decode(s) => s.as_str(),
        MediaError::Cancelled => "the decode was called off",
    };
    let mut unsupported = matches!(e, MediaError::Unsupported(_));
    while let Some(rest) = ["decode error: ", "unsupported media: ", "unsupported: ", "I/O error: "].iter().find_map(|p| detail.strip_prefix(p)) {
        unsupported |= detail.starts_with("unsupported");
        detail = rest;
    }
    let what = if codec.is_empty() { "This video" } else { codec };
    if unsupported { format!("{what} isn't supported by the built-in decoder: {detail}") } else { format!("{what} can't be decoded: {detail}") }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Conforming a long file's audio reads every chunk at its own time: the sample index times
    /// ticks per second overflowed `i64` past 12:36 at 48 kHz, and later chunks came out silent
    /// (#172).
    #[test]
    fn conform_chunks_keep_their_times_on_long_files() {
        for rate in [24_000u32, 44_100, 48_000, 96_000] {
            let mut last = Tick(-1);
            // Every chunk start of a 2-hour file.
            for at in (0..rate as usize * 7200).step_by(1 << 16) {
                let t = sample_time(at, rate);
                assert!(t > last, "{rate} Hz, frame {at}: {t:?} after {last:?}");
                assert!((t.seconds() - at as f64 / rate as f64).abs() < 1e-6, "{rate} Hz, frame {at}");
                last = t;
            }
        }
        assert!((sample_time(36_372_480, 48_000).seconds() - 757.76).abs() < 1e-9);
    }

    /// #408: footage whose header claims a month of audio isn't conformed: the conform thread
    /// allocated the whole conformed track up front (a terabyte here), and the failed allocation
    /// aborted the app. Its audio is read from the decoder instead (silence: this file doesn't
    /// decode).
    #[test]
    fn a_header_claiming_a_month_of_audio_is_not_conformed() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-conform-month");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("month.flac");
        std::fs::write(&path, b"fLaC, not really").unwrap();
        let footage = Footage {
            path: path.to_string_lossy().into(),
            kind: FootageKind::Audio,
            duration: Tick::from_seconds_f64(30.0 * 86_400.0),
            has_audio: true,
            ..Default::default()
        };
        assert_eq!(conform_frames(&footage, 48_000), None);
        assert_eq!(conform_frames(&Footage { duration: Tick::from_seconds_f64(60.0), ..footage.clone() }, 48_000), Some(2_880_000));
        let pool = MediaPool::new();
        let conformed = dir.join("conformed");
        pool.set_conform_folder(Some(conformed.clone()));
        let s = pool.audio_samples(&footage, Tick::from_seconds_f64(3600.0), 1024, 48_000);
        assert_eq!(s.len(), 2048);
        assert!(s.iter().all(|v| *v == 0.0));
        assert!(lock(&pool.inner.conforming).is_empty(), "no conform thread started");
        assert!(!conformed.exists(), "nothing was written");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #324: audio that fails to decode isn't conformed. A conformed file of silence was written
    /// instead, and as it is found by the file's path, size and date the footage stayed silent
    /// in every later session (a renamed copy played).
    #[test]
    fn audio_that_fails_to_decode_is_not_conformed() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-conform-fail");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("song.mp3");
        std::fs::write(&path, b"ID3, not really").unwrap();
        let footage = Footage {
            path: path.to_string_lossy().into(),
            kind: FootageKind::Audio,
            duration: Tick::from_seconds_f64(2.0),
            has_audio: true,
            ..Default::default()
        };
        let pool = MediaPool::new();
        let conformed = dir.join("conformed");
        pool.set_conform_folder(Some(conformed.clone()));
        let file = pool.conformed_path(&footage.path, 48_000).unwrap();
        assert!(pool.audio_samples(&footage, Tick::ZERO, 1024, 48_000).iter().all(|v| *v == 0.0));
        for _ in 0..500 {
            if lock(&pool.inner.conforming).is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(lock(&pool.inner.conforming).is_empty(), "the conform thread finished");
        assert!(!file.exists() && !file.with_extension("tmp").exists(), "no conformed file was left");
        assert!(lock(&pool.inner.conform_failed).contains(&file));
        // Later reads go to the decoder without trying again.
        pool.audio_samples(&footage, Tick::ZERO, 1024, 48_000);
        assert!(lock(&pool.inner.conforming).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // Regression test for #382 (#274): resampling footage audio divided by the requested mix
    // rate, so a rate of 0 panicked (division by zero); the read must come back instead.
    #[test]
    fn decoding_audio_at_a_zero_mix_rate_does_not_divide_by_zero() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-zero-rate");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // One second of a 440 Hz tone, 44.1 kHz mono 16-bit PCM: not the 48 kHz mix rate.
        let native = 44_100u32;
        let pcm: Vec<u8> =
            (0..native).flat_map(|n| (((n as f32 * 440.0 * std::f32::consts::TAU / native as f32).sin() * 8000.0) as i16).to_le_bytes()).collect();
        let mut wav = b"RIFF".to_vec();
        wav.extend((36 + pcm.len() as u32).to_le_bytes());
        wav.extend(b"WAVEfmt ");
        wav.extend(16u32.to_le_bytes());
        wav.extend([1u16, 1].iter().flat_map(|v| v.to_le_bytes()));
        wav.extend(native.to_le_bytes());
        wav.extend((native * 2).to_le_bytes());
        wav.extend([2u16, 16].iter().flat_map(|v| v.to_le_bytes()));
        wav.extend(b"data");
        wav.extend((pcm.len() as u32).to_le_bytes());
        wav.extend(&pcm);
        let path = dir.join("tone.wav");
        std::fs::write(&path, &wav).unwrap();
        let footage = Footage {
            path: path.to_string_lossy().into(),
            kind: FootageKind::Audio,
            duration: Tick::from_seconds_f64(1.0),
            has_audio: true,
            ..Default::default()
        };
        let pool = MediaPool::new();
        // The file really is read, and resampled, at a normal rate...
        let tone = pool.decode_audio(&footage, Tick::from_seconds_f64(0.1), 1024, 48_000);
        assert!(tone.iter().any(|v| v.abs() > 0.05), "the tone is audible at 48 kHz");
        // ...and a mix rate of 0 (which `audio_samples` itself turns away) returns a read of the
        // requested length instead of dividing by it.
        let degenerate = pool.try_decode_audio(&footage, Tick::from_seconds_f64(0.1), 64, 0).unwrap();
        assert_eq!(degenerate.len(), 128);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn frame_index_loops_and_clamps() {
        let r = FrameRate::FPS_30;
        assert_eq!(Inner::frame_index(r, r.tick_of(-3), 10, 1), 0);
        assert_eq!(Inner::frame_index(r, r.tick_of(4), 10, 1), 4);
        assert_eq!(Inner::frame_index(r, r.tick_of(25), 10, 1), 9);
        assert_eq!(Inner::frame_index(r, r.tick_of(25), 10, 3), 5);
        assert_eq!(Inner::frame_index(r, r.tick_of(45), 10, 3), 9);
        assert_eq!(Inner::frame_count(r, Tick(2 * TICKS_PER_SECOND)), 60);
    }

    #[test]
    fn lru_respects_budget_and_recycles() {
        let mut c = Lru::default();
        let img = || Pixels::F32(Arc::new(Image::new(16, 16))); // 4 KiB + overhead
        let k = |i| Key { path: "a".into(), frame: i, interp: 0, matte: [0; 3], size: (0, 0), nominal: (0, 0), rows: None, q8: false };
        let budget = 3 * (4096 + 64);
        for i in 0..10 {
            c.insert(k(i), img(), Arc::default(), budget);
        }
        assert_eq!(c.map.len(), 3);
        assert!(c.get(&k(9)).is_some() && c.get(&k(6)).is_none());
        // touching 7 makes 8 the oldest
        c.get(&k(7));
        c.insert(k(10), img(), Arc::default(), budget);
        assert!(c.get(&k(7)).is_some() && c.get(&k(8)).is_none());
        assert_eq!(c.spare.len(), MAX_SPARE);
        assert_eq!(c.take_spare(256).len(), 256);
        assert!(c.take_spare(17).is_empty());
    }

    /// A frame kept as 8-bit levels is its own entry beside the float one, counted at 4 bytes
    /// a pixel.
    #[test]
    fn lru_keeps_8_bit_frames_apart_at_a_quarter_of_the_bytes() {
        let mut c = Lru::default();
        let k = |q8| Key { path: "a".into(), frame: 0, interp: 0, matte: [0; 3], size: (0, 0), nominal: (0, 0), rows: None, q8 };
        c.insert(k(false), Pixels::F32(Arc::new(Image::new(16, 16))), Arc::default(), usize::MAX);
        c.insert(k(true), Pixels::Q8(Arc::new(Frame8 { width: 16, height: 16, rgba: vec![0; 16 * 16 * 4] })), Arc::default(), usize::MAX);
        assert!(matches!(c.get(&k(false)), Some((Pixels::F32(_), _))) && matches!(c.get(&k(true)), Some((Pixels::Q8(_), _))));
        assert_eq!(c.bytes, (16 * 16 * 16 + 64) + (16 * 16 * 4 + 64));
    }

    /// #482: an OpenEXR file with HTJ2K compression (which the decoder lacks) says so on import
    /// and when its frame is read, instead of "no non-deep rgb channels" or a transparent frame.
    #[test]
    fn htj2k_exr_is_reported_as_unsupported() {
        use exr::prelude::*;
        let channels =
            AnyChannels::sort(vec![AnyChannel::new("R", FlatSamples::F32(vec![0.5; 4])), AnyChannel::new("G", FlatSamples::F32(vec![0.5; 4]))].into());
        let image = exr::image::Image::from_layer(Layer::new((2, 2), LayerAttributes::default(), Encoding::UNCOMPRESSED, channels));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image.write().to_buffered(&mut bytes).unwrap();
        let mut bytes = bytes.into_inner();
        assert_eq!(crate::exr_channels::unsupported_compression(&bytes), None);
        // The header's compression attribute (name, type, size 1, value) says HTJ2K32 (11).
        let tag = b"compression\0compression\0\x01\0\0\0";
        let at = bytes.windows(tag.len()).position(|w| w == tag).unwrap() + tag.len();
        bytes[at] = 11;
        let b: Arc<[u8]> = bytes.into();
        let e = crate::probe_bytes("/shot.exr", b.clone()).unwrap_err().to_string();
        assert!(e.contains("/shot.exr: OpenEXR HTJ2K compression isn't supported yet"), "{e}");
        let pool = MediaPool::new();
        pool.add_bytes("/shot.exr", b);
        let f = Footage { path: "/shot.exr".into(), kind: FootageKind::Still, width: 2, height: 2, has_video: true, ..Default::default() };
        let e = pool.frame_at(&f, Tick::ZERO).unwrap_err().to_string();
        assert!(e.contains("HTJ2K compression isn't supported yet"), "{e}");
    }

    /// #411: Interpret Footage ▸ Preserve RGB reads a float OpenEXR's values as the file stores
    /// them (no automatic linear → sRGB encode), so an OCIO effect is the only conversion; the
    /// default interpretation still encodes. Both interpretations of one file decode separately.
    #[test]
    fn preserve_rgb_reads_float_exr_values_unconverted() {
        use exr::prelude::*;
        let (w, h) = (2usize, 2usize);
        let ch = |n: &str, v: f32| AnyChannel::new(n, FlatSamples::F32(vec![v; w * h]));
        let channels = AnyChannels::sort(vec![ch("R", 0.18), ch("G", 0.5), ch("B", 2.0), ch("A", 1.0)].into());
        let image = exr::image::Image::from_layer(Layer::new((w, h), LayerAttributes::default(), Encoding::FAST_LOSSLESS, channels));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image.write().to_buffered(&mut bytes).unwrap();
        let b: Arc<[u8]> = bytes.into_inner().into();
        let pool = MediaPool::new();
        pool.add_bytes("/render.exr", b.clone());
        let f = crate::probe_bytes("/render.exr", b).unwrap();
        let encoded = pool.frame_at(&f, Tick::ZERO).unwrap().get(0, 0);
        assert!((encoded[0] - 0.4613).abs() < 1e-3 && (encoded[1] - 0.7354).abs() < 1e-3 && encoded[2] > 1.3, "sRGB-encoded: {encoded:?}");
        let raw = pool.frame_at(&Footage { preserve_rgb: true, ..f.clone() }, Tick::ZERO).unwrap().get(0, 0);
        assert_eq!(raw, [0.18, 0.5, 2.0, 1.0]);
        assert_eq!(pool.frame_at(&f, Tick::ZERO).unwrap().get(0, 0), encoded, "the default interpretation is cached apart");
    }

    /// #750: float footage is handed over as the renderer asks: sRGB-encoded, with Working
    /// Gamma 2.4's pure power curve, or as stored (Interpret As Linear Light encodes it then);
    /// Preserve RGB reads it as stored whatever is asked. Each decodes and caches apart.
    #[test]
    fn float_exr_is_encoded_with_the_requested_curve() {
        use exr::prelude::*;
        let (w, h) = (2usize, 2usize);
        let ch = |n: &str, v: f32| AnyChannel::new(n, FlatSamples::F32(vec![v; w * h]));
        let channels = AnyChannels::sort(vec![ch("R", 0.051), ch("G", 0.18), ch("B", 2.0), ch("A", 1.0)].into());
        let image = exr::image::Image::from_layer(Layer::new((w, h), LayerAttributes::default(), Encoding::FAST_LOSSLESS, channels));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image.write().to_buffered(&mut bytes).unwrap();
        let b: Arc<[u8]> = bytes.into_inner().into();
        let pool = MediaPool::new();
        pool.add_bytes("/linear.exr", b.clone());
        let f = crate::probe_bytes("/linear.exr", b).unwrap();
        let px = |f: &Footage, float| pool.frame_with(ItemId(1), f, Tick::ZERO, None, float).unwrap().get(0, 0);
        let near = |o: [f32; 4], want: [f32; 3]| (0..3).all(|i| (o[i] - want[i]).abs() < 1e-3);
        let srgb = px(&f, FloatCurve::Srgb);
        assert!(near(srgb, [0.2503, 0.4614, 1.3532]), "sRGB: {srgb:?}");
        assert_eq!(pool.frame(ItemId(1), &f, Tick::ZERO).unwrap().get(0, 0), srgb, "frame() is sRGB");
        let g24 = px(&f, FloatCurve::Gamma24);
        assert!(near(g24, [0.2898, 0.4898, 2.0f32.powf(1.0 / 2.4)]), "2.4: {g24:?}");
        assert_eq!(px(&f, FloatCurve::Linear), [0.051, 0.18, 2.0, 1.0]);
        let preserve = Footage { preserve_rgb: true, ..f.clone() };
        assert_eq!(px(&preserve, FloatCurve::Gamma24), [0.051, 0.18, 2.0, 1.0]);
        // Each curve is cached apart: asking again returns the same pixels.
        assert_eq!(px(&f, FloatCurve::Srgb), srgb);
        assert_eq!(px(&f, FloatCurve::Gamma24), g24);
    }

    /// A model's material library is read beside it; one named by a path to another computer is
    /// looked up under its own file name beside the model, never fetched from that machine (#1274).
    #[test]
    fn model_files_on_other_computers_are_not_read() {
        let dir = std::env::temp_dir().join(format!("ec-model-files-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("red.mtl"), "newmtl red\nKd 1 0 0\n").unwrap();
        let pool = MediaPool::new();
        let red = |name: &str, mtl: &str| {
            let path = dir.join(name);
            std::fs::write(&path, format!("mtllib {mtl}\nv 0 0 0\nv 1 0 0\nv 0 1 0\nusemtl red\nf 1 2 3\n")).unwrap();
            let model = pool.inner.model(&path.to_string_lossy()).unwrap();
            model.materials.iter().any(|m| m.base_color[..3] == [1.0, 0.0, 0.0])
        };
        assert!(red("beside.obj", "red.mtl"));
        // 192.0.2.1 is a test-net address nothing here can reach, so these passes prove the
        // material library came from beside the model, not from another computer.
        assert!(red("unc.obj", r"\\192.0.2.1\share\red.mtl"));
        assert!(red("url.obj", "file://192.0.2.1/s/red.mtl"));
        // Linux reads `//dir/red.mtl` as the local file.
        #[cfg(unix)]
        assert!(red("slashes.obj", &format!("/{}", dir.join("red.mtl").display())));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A 16 × 8 OpenEXR still with a different value in every pixel, registered with `pool` at
    /// `path`.
    fn plate(pool: &MediaPool, path: &str) -> Footage {
        use exr::prelude::*;
        let (w, h) = (16usize, 8usize);
        let ch = |n: &str, k: f32| AnyChannel::new(n, FlatSamples::F32((0..w * h).map(|i| (i as f32 * k).sin().abs()).collect()));
        let channels = AnyChannels::sort(vec![ch("R", 0.37), ch("G", 0.11), ch("B", 0.73), AnyChannel::new("A", FlatSamples::F32(vec![1.0; w * h]))].into());
        let image = exr::image::Image::from_layer(Layer::new((w, h), LayerAttributes::default(), Encoding::FAST_LOSSLESS, channels));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image.write().to_buffered(&mut bytes).unwrap();
        let b: Arc<[u8]> = bytes.into_inner().into();
        pool.add_bytes(path, b.clone());
        crate::probe_bytes(path, b).unwrap()
    }

    /// Hold `pool`'s decodes until `n` requests wait for one (at most 10 s), then answer with
    /// `then` (`None`: decode the file).
    fn gate(pool: &MediaPool, n: usize, then: impl Fn() -> Option<Result<Image>> + Send + Sync + 'static) {
        let inner = Arc::downgrade(&pool.inner);
        *lock(&pool.inner.gate) = Some(Arc::new(move || {
            let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while inner.upgrade().is_some_and(|i| i.waiting.load(Ordering::SeqCst) < n) && std::time::Instant::now() < until {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            then()
        }));
    }

    type Job<T> = Box<dyn FnOnce() -> T + Send>;

    /// Run `jobs` at once, each on a rayon worker of its own (as the viewer's frame workers ask
    /// for frames), and collect what they return; a job that panics returns nothing, and one
    /// that hangs fails the test (20 s) instead of hanging it.
    fn on_workers<T: Send + 'static>(jobs: Vec<Job<T>>) -> Vec<T> {
        let n = jobs.len();
        let workers = rayon::ThreadPoolBuilder::new().num_threads(n).panic_handler(|_| {}).build().unwrap();
        let start = Arc::new(std::sync::Barrier::new(n));
        let (tx, rx) = std::sync::mpsc::channel();
        for job in jobs {
            let (tx, start) = (tx.clone(), start.clone());
            workers.spawn(move || {
                // (each worker holds one job until all have started)
                start.wait();
                let _ = tx.send(job());
            });
        }
        drop(tx);
        (0..n).map_while(|_| rx.recv_timeout(std::time::Duration::from_secs(20)).ok()).collect()
    }

    /// Requests for the frame of `f` on `pool`, one at each of `sizes`, as [`on_workers`] jobs.
    fn requests(pool: &MediaPool, f: &Footage, sizes: &[Option<(u32, u32)>]) -> Vec<Job<(Option<(u32, u32)>, Result<Arc<Image>>)>> {
        sizes
            .iter()
            .map(|&size| {
                let (pool, f) = (pool.clone(), f.clone());
                Box::new(move || (size, Inner::frame_at(&pool.inner, &f, Tick::ZERO, true, size, FloatCurve::Srgb))) as Job<_>
            })
            .collect()
    }

    /// #3269: requests for a frame being decoded wait for that decode and take its picture, also
    /// on rayon workers (the viewer's frame workers), which each decoded a private copy before.
    #[test]
    fn concurrent_requests_for_a_frame_decode_it_once() {
        let pool = MediaPool::new();
        let f = plate(&pool, "/plate.exr");
        const N: usize = 8;
        gate(&pool, N - 1, || None);
        let got = on_workers(requests(&pool, &f, &[None; N]));
        assert_eq!(got.len(), N, "every request came back");
        let first = got[0].1.as_ref().unwrap().clone();
        for (_, img) in &got {
            assert!(Arc::ptr_eq(img.as_ref().unwrap(), &first), "the same picture");
        }
        assert_eq!(pool.stats().decodes, 1, "one decode for {N} requests");
        assert!(lock(&pool.inner.cache).inflight.is_empty(), "the flight ended");
    }

    /// #3269: a still asked for at several sizes at once (a reduced viewer, the Project panel's
    /// thumbnail of its comp) is decoded once, and each size is what decoding it alone gives.
    #[test]
    fn sizes_of_a_still_asked_for_at_once_share_one_decode() {
        let whole = {
            let alone = MediaPool::new();
            let f = plate(&alone, "/plate.exr");
            alone.frame_at(&f, Tick::ZERO).unwrap()
        };
        let pool = MediaPool::new();
        let f = plate(&pool, "/plate.exr");
        let sizes = [Some((8, 4)), Some((4, 2)), None];
        gate(&pool, sizes.len() - 1, || None);
        let got = on_workers(requests(&pool, &f, &sizes));
        assert_eq!(got.len(), sizes.len(), "every request came back");
        assert_eq!(pool.stats().decodes, 1, "one decode for all sizes");
        for (size, img) in got {
            let img = img.unwrap();
            match size {
                None => assert_eq!(img.data, whole.data),
                Some((w, h)) => {
                    assert_eq!((img.width, img.height), (w, h));
                    assert_eq!(img.data, effectcraft_raster::resample(&whole, w, h).data, "{w}×{h} as decoded alone");
                }
            }
        }
    }

    /// #3269: a second size of a still shown reduced (the thumbnail after the viewer) is
    /// resampled from the first request's decode, not decoded again, and the whole picture is
    /// not kept in the frame cache (#1812).
    #[test]
    fn a_second_size_of_a_still_is_resampled_from_the_first_decode() {
        let whole = {
            let alone = MediaPool::new();
            let f = plate(&alone, "/plate.exr");
            alone.frame_at(&f, Tick::ZERO).unwrap()
        };
        let pool = MediaPool::new();
        let f = plate(&pool, "/plate.exr");
        let at = |size| Inner::frame_at(&pool.inner, &f, Tick::ZERO, true, Some(size), FloatCurve::Srgb).unwrap();
        let viewer = at((8, 4));
        let thumb = at((4, 2));
        assert_eq!(pool.stats().decodes, 1, "the thumbnail did not decode the file again");
        assert_eq!(viewer.data, effectcraft_raster::resample(&whole, 8, 4).data);
        assert_eq!(thumb.data, effectcraft_raster::resample(&whole, 4, 2).data, "as decoded alone");
        assert!(lock(&pool.inner.cache).lru.map.keys().all(|k| k.size != (0, 0)), "the whole picture is not in the frame cache");
        // Forgetting the file forgets its whole picture too.
        pool.forget("/plate.exr");
        at((2, 1));
        assert_eq!(pool.stats().decodes, 2);
    }

    /// #3269: when the decode others wait for fails, or panics, every waiting request gets an
    /// error and none hangs; the next request decodes again.
    #[test]
    fn a_failed_decode_fails_every_waiting_request() {
        for panics in [false, true] {
            let pool = MediaPool::new();
            let f = plate(&pool, "/broken.exr");
            const N: usize = 6;
            gate(&pool, N - 1, move || if panics { panic!("a decoder bug") } else { Some(Err(MediaError::Decode("/broken.exr: a bad block".into()))) });
            let got = on_workers(requests(&pool, &f, &[None; N]));
            let errors: Vec<String> = got.iter().map(|(_, r)| r.as_ref().err().map(ToString::to_string).unwrap_or_default()).collect();
            assert_eq!(got.len(), N, "panics: {panics}, {errors:?}");
            let why = if panics { "the decode stopped" } else { "a bad block" };
            assert!(errors.iter().all(|e| e.contains(why)), "panics: {panics}, {errors:?}");
            assert_eq!(pool.stats().decodes, 1, "the waiting requests did not decode");
            assert!(lock(&pool.inner.cache).inflight.is_empty(), "the flight ended");
            *lock(&pool.inner.gate) = None;
            assert!(pool.frame_at(&f, Tick::ZERO).is_ok(), "decodes again");
        }
    }

    /// #3269: a decode runs on the decode threads, not on the frame worker asking for it: a frame
    /// worker that ran part of it (rayon lets a worker waiting in a parallel loop take another
    /// job) could take a request waiting for that very decode, which then never finished. Here
    /// the decode works in parallel while a request for the same picture comes to the frame
    /// workers: none of its work runs there, and both requests are answered at once.
    #[test]
    fn a_decode_runs_off_the_frame_workers() {
        use rayon::prelude::*;
        let pool = MediaPool::new();
        let f = plate(&pool, "/plate.exr");
        let workers = Arc::new(rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap());
        let on_frame_workers = Arc::new(AtomicU64::new(0));
        let (tx, rx) = std::sync::mpsc::channel();
        {
            let (workers, seen, asker, f, tx) = (workers.clone(), on_frame_workers.clone(), pool.clone(), f.clone(), tx.clone());
            let inner = Arc::downgrade(&pool.inner);
            *lock(&pool.inner.gate) = Some(Arc::new(move || {
                let (asker, f, tx) = (asker.clone(), f.clone(), tx.clone());
                workers.spawn(move || {
                    let _ = tx.send(asker.frame_at(&f, Tick::ZERO));
                });
                (0..64).into_par_iter().for_each(|_| {
                    if workers.current_thread_index().is_some() {
                        seen.fetch_add(1, Ordering::SeqCst);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(1));
                });
                let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
                while inner.upgrade().is_some_and(|i| i.waiting.load(Ordering::SeqCst) < 1) && std::time::Instant::now() < until {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                None
            }));
        }
        let started = std::time::Instant::now();
        {
            let (pool, f, tx) = (pool.clone(), f.clone(), tx.clone());
            workers.spawn(move || {
                let _ = tx.send(pool.frame_at(&f, Tick::ZERO));
            });
        }
        drop(tx);
        let got: Vec<_> = (0..2).map_while(|_| rx.recv_timeout(std::time::Duration::from_secs(20)).ok()).collect();
        // (the gate holds a pool: let it go)
        *lock(&pool.inner.gate) = None;
        assert_eq!(got.len(), 2, "both requests were answered");
        assert!(got.iter().all(Result::is_ok));
        assert!(started.elapsed() < WAIT_LIMIT, "nobody waited out the time limit: {:?}", started.elapsed());
        assert_eq!(on_frame_workers.load(Ordering::SeqCst), 0, "no part of the decode ran on a frame worker");
        assert_eq!(pool.stats().decodes, 1);
    }

    /// Where a thread decodes a picture itself (no decode threads), a request that comes to it
    /// meanwhile (a rayon worker runs other jobs while its decode waits on its parallel work)
    /// doesn't wait for a picture being decoded: that could be its own decode further down the
    /// stack. It decodes a copy instead.
    #[test]
    fn a_thread_decoding_a_picture_does_not_wait_for_one() {
        let pool = MediaPool::new();
        let f = plate(&pool, "/plate.exr");
        let flight = current_flight(&pool);
        lock(&pool.inner.cache).inflight.insert(Key { size: (0, 0), nominal: (0, 0), ..Inner::locate(&f, Tick::ZERO, FloatCurve::Srgb).key }, flight);
        let decoding = Decoding::new();
        let started = std::time::Instant::now();
        assert!(pool.frame_at(&f, Tick::ZERO).is_ok());
        assert!(started.elapsed() < WAIT_LIMIT, "did not wait for the flight");
        drop(decoding);
        assert_eq!(OWNED.with(Cell::get), 0);
    }

    /// #329: a request waiting for another's decode gives up once it is called off itself.
    #[test]
    fn a_waiting_request_called_off_stops_waiting() {
        let pool = MediaPool::new();
        let f = plate(&pool, "/plate.exr");
        let flight = current_flight(&pool);
        lock(&pool.inner.cache).inflight.insert(Key { size: (0, 0), nominal: (0, 0), ..Inner::locate(&f, Tick::ZERO, FloatCurve::Srgb).key }, flight);
        let cancel = Arc::new(AtomicBool::new(false));
        let started = std::time::Instant::now();
        std::thread::scope(|s| {
            s.spawn(|| {
                std::thread::sleep(std::time::Duration::from_millis(50));
                cancel.store(true, Ordering::Relaxed);
            });
            let got = filmcraft_media::cancel::with_cancel(&cancel, || pool.frame_at(&f, Tick::ZERO));
            assert!(matches!(got, Err(MediaError::Cancelled)), "{:?}", got.err());
        });
        assert!(started.elapsed() < WAIT_LIMIT);
    }

    /// #3269: a whole still kept for a second size is kept within its own cap, and not after
    /// the frame cache has moved on.
    #[test]
    fn recent_whole_stills_are_capped_and_expire() {
        let key = |i| Key { path: "/s.exr".into(), frame: i, interp: 0, matte: [0; 3], size: (0, 0), nominal: (0, 0), rows: None, q8: false };
        let mut c = CacheState::default();
        // 64 KiB each (64 × 64 pixels of 16 bytes), a budget of 512 KiB: room for 4.
        for i in 0..6 {
            c.keep_recent(key(i), Arc::new(Image::new(64, 64)), Arc::default(), 512 << 10);
        }
        assert!(c.recent(&key(1)).is_none() && c.recent(&key(2)).is_some() && c.recent(&key(5)).is_some());
        // Larger than the cap: not kept.
        c.keep_recent(key(9), Arc::new(Image::new(512, 512)), Arc::default(), 512 << 10);
        assert!(c.recent(&key(9)).is_none());
        c.lru.clock += RECENT_AGE + 1;
        assert!(c.recent(&key(5)).is_none(), "expired");
    }

    /// #3270 review: lowering the budget drops whole stills beyond the new cap at once (they
    /// were kept until the next one was), and the stats count them.
    #[test]
    fn a_lower_budget_trims_recent_whole_stills() {
        let pool = MediaPool::new();
        let f = plate(&pool, "/plate.exr");
        Inner::frame_at(&pool.inner, &f, Tick::ZERO, true, Some((8, 4)), FloatCurve::Srgb).unwrap();
        let whole = 16 * 8 * std::mem::size_of::<Px>();
        assert_eq!(pool.stats().recent_bytes, whole);
        pool.set_budget(whole);
        assert_eq!(pool.stats().recent_bytes, 0, "half the new budget holds no whole still");
        assert!(lock(&pool.inner.cache).recent.is_empty());
    }

    /// A flight of `pool`'s current generation (one requests join).
    fn current_flight(pool: &MediaPool) -> Arc<Flight> {
        Arc::new(Flight { generation: lock(&pool.inner.cache).generation, ..Flight::default() })
    }

    /// #3270 review: a request made after a file was forgotten (it changed on disk) while its
    /// decode was in flight doesn't join that decode: it got the old pixels, and kept them as
    /// the new file's.
    #[test]
    fn a_request_after_forget_does_not_join_the_old_decode() {
        let pool = MediaPool::new();
        let f = plate(&pool, "/plate.exr");
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        {
            let (calls, inner) = (calls.clone(), Arc::downgrade(&pool.inner));
            // The first decode (of the old file) waits for a request to join it (2 s at most)
            // and answers with a black picture; later ones decode the file.
            *lock(&pool.inner.gate) = Some(Arc::new(move || {
                if calls.fetch_add(1, Ordering::SeqCst) > 0 {
                    return None;
                }
                let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
                while inner.upgrade().is_some_and(|i| i.waiting.load(Ordering::SeqCst) < 1) && std::time::Instant::now() < until {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                Some(Ok(Image::new(16, 8)))
            }));
        }
        std::thread::scope(|s| {
            let old = s.spawn(|| pool.frame_at(&f, Tick::ZERO));
            while calls.load(Ordering::SeqCst) == 0 {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            pool.forget("/plate.exr");
            let new = pool.frame_at(&f, Tick::ZERO).unwrap();
            assert!(new.data.iter().any(|p| p[0] > 0.0), "the file's pixels, not the old decode's");
            assert!(old.join().unwrap().is_ok());
        });
        *lock(&pool.inner.gate) = None;
        let again = pool.frame_at(&f, Tick::ZERO).unwrap();
        assert!(again.data.iter().any(|p| p[0] > 0.0), "the old decode's pixels were not kept");
    }

    /// #3270 review: forgetting one file (a file watcher's change, `add_bytes`) leaves another
    /// file's decode in flight joinable and cached: a global count made every other decode
    /// stale, so requests decoded those files again and nothing was kept.
    #[test]
    fn forgetting_one_file_keeps_another_files_decode() {
        let pool = MediaPool::new();
        let f = plate(&pool, "/a.exr");
        let other = plate(&pool, "/b.exr");
        let (pool_a, f_a) = (pool.clone(), f.clone());
        let started = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let started_a = started.clone();
        // The decode waits (2 s at most) for a request to join it, then decodes the file.
        *lock(&pool.inner.gate) = {
            let inner = Arc::downgrade(&pool.inner);
            Some(Arc::new(move || {
                started_a.store(true, Ordering::SeqCst);
                let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
                while inner.upgrade().is_some_and(|i| i.waiting.load(Ordering::SeqCst) < 1) && std::time::Instant::now() < until {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                None
            }))
        };
        std::thread::scope(|s| {
            let first = s.spawn(move || pool_a.frame_at(&f_a, Tick::ZERO));
            while !started.load(Ordering::SeqCst) {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            pool.forget(&other.path);
            let joined = pool.frame_at(&f, Tick::ZERO).unwrap();
            let first = first.join().unwrap().unwrap();
            assert!(Arc::ptr_eq(&joined, &first), "the request joined the decode in flight");
        });
        assert_eq!(pool.stats().decodes, 1, "one decode");
        pool.frame_at(&f, Tick::ZERO).unwrap();
        assert_eq!(pool.stats().decodes, 1, "and its picture was kept");
        *lock(&pool.inner.gate) = None;
    }

    /// Forgotten paths are remembered per path and boundedly: past [`MAX_FORGOTTEN`], everything
    /// that started before counts as stale.
    #[test]
    fn forgotten_paths_stay_bounded() {
        let mut c = CacheState::default();
        let at = c.generation;
        c.forgot("/a.exr");
        assert!(c.stale("/a.exr", at) && !c.stale("/b.exr", at) && !c.stale("/a.exr", c.generation));
        for i in 0..MAX_FORGOTTEN + 10 {
            c.forgot(&format!("/{i}.exr"));
        }
        assert!(c.forgotten.len() <= MAX_FORGOTTEN);
        assert!(c.stale("/b.exr", at), "older than the remembered paths: stale");
        assert!(!c.stale("/b.exr", c.generation));
    }

    /// #3301: open movie/audio files are capped: beyond [`MAX_OPEN_SOURCES`] the least recently
    /// asked for is closed (a project of thousands of clips kept every one open).
    #[test]
    fn open_sources_are_capped_least_recently_used_first() {
        let mut s = Sources::default();
        let path = |i: usize| -> Arc<str> { format!("/clips/{i}.mov").into() };
        let first = s.slot(&path(0));
        for i in 1..MAX_OPEN_SOURCES + 40 {
            s.slot(&path(i));
            // (the first clip is asked for again and again: it stays)
            s.slot(&path(0));
        }
        assert_eq!(s.map.len(), MAX_OPEN_SOURCES);
        assert!(s.map.contains_key(&path(0)), "used lately: kept");
        assert!(!s.map.contains_key(&path(1)), "the least recently used went");
        assert!(Arc::ptr_eq(&first, &s.slot(&path(0))), "a kept slot is the same slot");
    }
}
