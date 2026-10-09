//! [`MediaPool`]: decoded footage frames for the compositor, with a memory-budgeted LRU cache.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use effectcraft_project::{AlphaMode, Footage, FootageKind, ItemId};
use effectcraft_raster::{Image, Px};
use effectcraft_render::FootageSource;
use effectcraft_time::{FrameRate, TICKS_PER_SECOND, Tick};
use filmcraft_media::{FrameRequest, SharedSource};

use crate::convert::{AlphaOp, dynamic_to_image, frame_to_image_in};
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
    alpha: u8,
    matte: [u32; 3],
}

struct Entry {
    img: Arc<Image>,
    stamp: u64,
    bytes: usize,
}

/// Conformed audio files written by builds before this version are ignored (and written again):
/// version 1 files of sources longer than about 12:36 at 48 kHz were silent from there on (#172).
const CONFORM_VERSION: u32 = 2;

/// The source time of sample frame `at` at `rate` Hz. (`at` × ticks per second overflows `i64`
/// past about 12:36 at 48 kHz.)
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
    fn get(&mut self, k: &Key) -> Option<Arc<Image>> {
        let e = self.map.get_mut(k)?;
        self.order.remove(&e.stamp);
        self.clock += 1;
        e.stamp = self.clock;
        self.order.insert(e.stamp, k.clone());
        Some(e.img.clone())
    }
    fn insert(&mut self, k: Key, img: Arc<Image>, budget: usize) {
        let bytes = img.data.len() * std::mem::size_of::<Px>() + 64;
        self.clock += 1;
        if let Some(old) = self.map.insert(k.clone(), Entry { img, stamp: self.clock, bytes }) {
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
                if self.spare.len() < MAX_SPARE
                    && let Ok(img) = Arc::try_unwrap(e.img)
                {
                    self.spare.push(img.data);
                }
            }
        }
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
}

#[derive(Default)]
struct CacheState {
    lru: Lru,
    /// Frames being decoded right now (requests for them wait instead of decoding again).
    inflight: HashSet<Key>,
    /// Last frame index requested per movie (sequential-playback detection).
    last: HashMap<Arc<str>, i64>,
}

struct Inner {
    /// Opened movie/audio sources by path (`None`: failed to open; not retried until `forget`).
    sources: Mutex<HashMap<Arc<str>, Option<SharedSource>>>,
    /// In-memory files registered with `add_bytes` (web builds, tests).
    files: Mutex<HashMap<String, Arc<[u8]>>>,
    /// Parsed 3D models by path (`None`: failed to load; not retried until `forget`).
    models: Mutex<HashMap<String, Option<Arc<effectcraft_model::Model>>>>,
    cache: Mutex<CacheState>,
    done: Condvar,
    budget: AtomicU64,
    read_ahead: AtomicBool,
    hits: AtomicU64,
    misses: AtomicU64,
    prefetched: AtomicU64,
    /// Settings ▸ Disk ▸ Conformed Audio Folder: decoded audio tracks written once per file and
    /// rate, read back instead of decoding again (`None` = off).
    conform: Mutex<Option<std::path::PathBuf>>,
    /// Conformed files being written right now.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    conforming: Mutex<HashSet<std::path::PathBuf>>,
}

/// Decodes footage for the renderer: stills, image sequences and movies (FilmCraft codecs), plus
/// audio. Thread-safe and cheap to clone (clones share decoders and cache).
///
/// - One decoder per movie: FilmCraft's GOP cache keeps it positioned, so sequential frames
///   decode without re-seeking, and a seek decodes forward from the preceding sync sample.
/// - Converted frames live in an LRU cache bounded by a byte budget ([`DEFAULT_BUDGET`]).
/// - Sequential playback reads one frame ahead on a worker thread (native builds), so decoding
///   and conversion of the next frame overlap with the caller's use of the current one.
/// - Concurrent requests for the same frame decode it once.
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
struct Loc {
    key: Key,
    /// Media time for movies (`None`: an image file, `key.path`).
    media_t: Option<Tick>,
}

/// Removes an in-flight key and wakes waiters, also when decoding panics.
struct Inflight<'a> {
    inner: &'a Inner,
    key: Key,
}

impl Drop for Inflight<'_> {
    fn drop(&mut self) {
        lock(&self.inner.cache).inflight.remove(&self.key);
        self.inner.done.notify_all();
    }
}

impl MediaPool {
    /// A pool with the [`DEFAULT_BUDGET`] frame cache.
    pub fn new() -> Self {
        Self::with_budget(DEFAULT_BUDGET)
    }

    /// A pool whose frame cache holds at most `bytes` of decoded frames.
    pub fn with_budget(bytes: usize) -> Self {
        Self {
            inner: Arc::new(Inner {
                sources: Mutex::default(),
                files: Mutex::default(),
                models: Mutex::default(),
                cache: Mutex::default(),
                done: Condvar::new(),
                budget: AtomicU64::new(bytes as u64),
                read_ahead: AtomicBool::new(cfg!(not(target_arch = "wasm32"))),
                hits: AtomicU64::new(0),
                misses: AtomicU64::new(0),
                prefetched: AtomicU64::new(0),
                conform: Mutex::default(),
                conforming: Mutex::default(),
            }),
        }
    }

    pub fn budget(&self) -> usize {
        self.inner.budget()
    }

    /// Change the frame-cache budget (evicts immediately when shrinking).
    pub fn set_budget(&self, bytes: usize) {
        self.inner.budget.store(bytes as u64, Ordering::Relaxed);
        lock(&self.inner.cache).lru.evict(bytes);
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
        }
    }

    /// Drop every cached frame (decoders stay open).
    pub fn clear_frames(&self) {
        let mut c = lock(&self.inner.cache);
        c.lru = Lru::default();
        c.last.clear();
    }

    /// Drop every cached frame and decoder.
    pub fn clear(&self) {
        self.clear_frames();
        lock(&self.inner.sources).clear();
    }

    /// Forget a file (after it changed on disk, or to retry a file that failed to open).
    pub fn forget(&self, path: &str) {
        lock(&self.inner.sources).remove(path);
        lock(&self.inner.models).remove(path);
        let mut c = lock(&self.inner.cache);
        let keys: Vec<Key> = c.lru.map.keys().filter(|k| &*k.path == path).cloned().collect();
        for k in keys {
            c.lru.remove(&k);
        }
        c.last.remove(path);
    }

    /// Provide the contents of `path` from memory (used instead of the file system; web builds).
    pub fn add_bytes(&self, path: &str, bytes: Arc<[u8]>) {
        lock(&self.inner.files).insert(path.to_string(), bytes);
        self.forget(path);
    }

    /// Decode (or fetch from cache) the frame of `footage` at source time `t`.
    pub fn frame_at(&self, footage: &Footage, t: Tick) -> Result<Arc<Image>> {
        Inner::frame_at(&self.inner, footage, t, true)
    }

    /// Settings ▸ Disk ▸ Conformed Audio Folder: write each footage file's decoded audio there
    /// once per sample rate (raw interleaved stereo `f32`) and read it back instead of decoding
    /// again. `None` turns it off.
    pub fn set_conform_folder(&self, folder: Option<std::path::PathBuf>) {
        *lock(&self.inner.conform) = folder;
    }

    /// The conformed-audio file of `path` at `rate` (named by a hash of the path, size and
    /// modification time, so an edited file conforms again, and of [`CONFORM_VERSION`]).
    pub fn conformed_path(&self, path: &str, rate: u32) -> Option<std::path::PathBuf> {
        use std::hash::{Hash, Hasher};
        let folder = lock(&self.inner.conform).clone()?;
        let meta = std::fs::metadata(path).ok()?;
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

    /// Conform `footage`'s audio at `rate` into `file` on a worker thread (once).
    #[cfg(not(target_arch = "wasm32"))]
    fn conform(&self, footage: &Footage, file: std::path::PathBuf, rate: u32) {
        if !lock(&self.inner.conforming).insert(file.clone()) {
            return;
        }
        let (pool, f) = (self.clone(), footage.clone());
        let spawned = std::thread::Builder::new().name("ec-conform-audio".into()).spawn(move || {
            let frames = f.duration.to_units_floor(rate as i64).max(0) as usize;
            let mut bytes = Vec::with_capacity(frames * 8);
            let mut at = 0usize;
            while at < frames {
                let n = (frames - at).min(1 << 16);
                let t = sample_time(at, rate);
                for v in pool.decode_audio(&f, t, n, rate) {
                    bytes.extend_from_slice(&v.to_le_bytes());
                }
                at += n;
            }
            let ok = file.parent().is_some_and(|d| std::fs::create_dir_all(d).is_ok()) && {
                let tmp = file.with_extension("tmp");
                std::fs::write(&tmp, &bytes).is_ok() && std::fs::rename(&tmp, &file).is_ok()
            };
            if !ok {
                log::warn!("media: could not write conformed audio {}", file.display());
            }
            lock(&pool.inner.conforming).remove(&file);
        });
        if spawned.is_err() {
            lock(&self.inner.conforming).clear();
        }
    }

    /// `frames` stereo sample frames of `footage`'s audio starting at source time `start`, at
    /// `rate` Hz, interleaved (L R L R …). Mono is duplicated to both channels; channels beyond
    /// the first two are dropped. Silence where there is no audio (or before the media starts).
    /// With a conformed audio folder, reads come from the conformed file once it is written.
    pub fn audio_samples(&self, footage: &Footage, start: Tick, frames: usize, rate: u32) -> Vec<f32> {
        if !footage.has_audio || footage.missing || rate == 0 || frames == 0 {
            return vec![0.0; frames * 2];
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(file) = self.conformed_path(&footage.path, rate) {
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
                self.conform(footage, file, rate);
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
        let path: Arc<str> = footage.path.as_str().into();
        let Some(src) = self.inner.source(&path) else { return vec![0.0; frames * 2] };
        let s0 = start.to_units_floor(rate as i64);
        let native = src.info().audio.as_ref().map_or(rate, |a| a.sample_rate);
        if native == 0 || rate == 0 || native == rate {
            return read_stereo(&src, &path, s0, frames, rate);
        }
        // Output sample `n` sits at source position n × native / rate.
        let (native_i, rate_i) = (i128::from(native), i128::from(rate));
        let first = (i128::from(s0) * native_i).div_euclid(rate_i);
        let last = ((i128::from(s0) + frames as i128) * native_i).div_euclid(rate_i) + 1;
        let (Ok(first64), Ok(count)) = (i64::try_from(first), usize::try_from(last - first + 1)) else { return vec![0.0; frames * 2] };
        let buf = read_stereo(&src, &path, first64, count, native);
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
        out
    }

    /// A thumbnail of `footage` (its first frame) fitting in `max_side` × `max_side`, aspect kept.
    pub fn thumbnail(&self, footage: &Footage, max_side: u32) -> Result<Image> {
        let img = Inner::frame_at(&self.inner, footage, Tick::ZERO, false)?;
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
        if let Some(m) = lock(&self.models).get(path) {
            return m.clone();
        }
        let loaded = self.read(path).map_err(|e| e.to_string()).and_then(|bytes| {
            let dir = std::path::Path::new(path).parent().map(|d| d.to_path_buf()).unwrap_or_default();
            let resolve = |uri: &str| -> Option<Vec<u8>> {
                let p = dir.join(uri);
                self.read(&p.to_string_lossy()).ok().map(|b| b.to_vec())
            };
            effectcraft_model::load(path, &bytes, &resolve).map_err(|e| e.to_string())
        });
        let m = match loaded {
            Ok(m) => Some(Arc::new(m)),
            Err(e) => {
                log::warn!("media: cannot load model {path}: {e}");
                None
            }
        };
        lock(&self.models).entry(path.to_string()).or_insert(m).clone()
    }

    fn read(&self, path: &str) -> Result<Arc<[u8]>> {
        if let Some(b) = lock(&self.files).get(path) {
            return Ok(b.clone());
        }
        std::fs::read(path).map(Into::into).map_err(|e| MediaError::Io(format!("{path}: {e}")))
    }

    /// The opened movie/audio source for `path`.
    fn source(&self, path: &Arc<str>) -> Option<SharedSource> {
        if let Some(s) = lock(&self.sources).get(path) {
            return s.clone();
        }
        // open outside the lock (reading and parsing a large file takes a moment)
        let opened = self.read(path).and_then(|bytes| {
            let name = std::path::Path::new(&**path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            filmcraft_codecs::open_bytes(&name, bytes).map_err(MediaError::from)
        });
        let opened = match opened {
            Ok(s) => Some(s),
            Err(e) => {
                log::warn!("media: cannot open {path}: {e}");
                None
            }
        };
        lock(&self.sources).entry(path.clone()).or_insert(opened).clone()
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

    fn locate(footage: &Footage, t: Tick) -> Loc {
        let alpha = match footage.alpha {
            AlphaMode::Straight => 0,
            AlphaMode::Premultiplied => 1,
            AlphaMode::Ignore => 2,
        };
        let matte = if footage.alpha == AlphaMode::Premultiplied { footage.premul_color.map(f32::to_bits) } else { [0; 3] };
        let key = |path: &str, frame| Key { path: path.into(), frame, alpha, matte };
        match footage.kind {
            FootageKind::Sequence if !footage.sequence.is_empty() => {
                let i = Self::frame_index(footage.frame_rate, t, footage.sequence_frames(), footage.loop_count);
                match footage.sequence_file(i) {
                    Some(path) => Loc { key: key(path, 0), media_t: None },
                    // A gap in the numbering (Missing Frames ▸ Show Placeholder).
                    None => Loc { key: key(PLACEHOLDER, ((footage.width as i64) << 32) | footage.height as i64), media_t: None },
                }
            }
            // A layer of a layered still is keyed by its layer index and size mode (smart objects
            // by their embedded file; PDF pages by their number).
            FootageKind::Still if footage.layer.is_some() || footage.page != 0 => {
                let l = footage.layer.as_ref().map_or(0, |l| 1 + l.index as i64 * 2 + l.layer_size as i64 + if l.embedded.is_some() { 1 << 40 } else { 0 })
                    + ((footage.page as i64) << 42);
                Loc { key: key(&footage.path, l), media_t: None }
            }
            FootageKind::Still | FootageKind::Sequence | FootageKind::Model | FootageKind::Data => Loc { key: key(&footage.path, 0), media_t: None },
            FootageKind::Video | FootageKind::Audio => {
                let rate = footage.frame_rate;
                let n = Self::frame_count(rate, footage.duration);
                let i = Self::frame_index(rate, t, n, footage.loop_count);
                // Middle of the frame in the file's own timing (robust to timestamp rounding).
                let native = footage.native_rate.unwrap_or(rate);
                let mt = native.tick_of(i) + Tick(native.frame_duration().0 / 2);
                Loc { key: key(&footage.path, i), media_t: Some(mt) }
            }
        }
    }

    fn frame_at(self: &Arc<Self>, footage: &Footage, t: Tick, playback: bool) -> Result<Arc<Image>> {
        if !footage.has_video {
            return Err(MediaError::Unsupported(format!("{}: no video", footage.path)));
        }
        let loc = Self::locate(footage, t);
        let movie = loc.media_t.is_some();
        let mut c = lock(&self.cache);
        // sequential playback of a movie: read the next frame ahead
        if playback && movie {
            let prev = c.last.insert(loc.key.path.clone(), loc.key.frame);
            if prev == Some(loc.key.frame - 1) && self.read_ahead.load(Ordering::Relaxed) {
                let next_t = t + footage.frame_rate.frame_duration();
                let next = Self::locate(footage, next_t);
                if next.key != loc.key && !c.lru.map.contains_key(&next.key) && !c.inflight.contains(&next.key) {
                    self.prefetch(footage, next_t);
                }
            }
        }
        loop {
            if let Some(img) = c.lru.get(&loc.key) {
                self.hits.fetch_add(1, Ordering::Relaxed);
                return Ok(img);
            }
            if !c.inflight.contains(&loc.key) {
                break;
            }
            // Another thread (or the read-ahead) is decoding this frame. Never block a rayon
            // worker on that: while the decoder waits on its own parallel join, the worker can
            // steal this very job, and waiting here would then wait on a decode further down
            // its own stack (deadlock). Decode a private copy instead.
            if rayon::current_thread_index().is_some() {
                self.misses.fetch_add(1, Ordering::Relaxed);
                drop(c);
                return Ok(Arc::new(self.decode(&loc, footage)?));
            }
            c = self.done.wait(c).unwrap_or_else(|e| e.into_inner());
        }
        self.misses.fetch_add(1, Ordering::Relaxed);
        c.inflight.insert(loc.key.clone());
        drop(c);
        let _guard = Inflight { inner: self, key: loc.key.clone() };
        let img = Arc::new(self.decode(&loc, footage)?);
        lock(&self.cache).lru.insert(loc.key, img.clone(), self.budget());
        Ok(img)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn prefetch(self: &Arc<Self>, footage: &Footage, t: Tick) {
        let (me, f) = (self.clone(), footage.clone());
        self.prefetched.fetch_add(1, Ordering::Relaxed);
        // A plain thread, not a rayon job: the decode below parallelises with rayon itself.
        std::thread::spawn(move || {
            let _ = me.frame_at(&f, t, false);
        });
    }

    #[cfg(target_arch = "wasm32")]
    fn prefetch(self: &Arc<Self>, _footage: &Footage, _t: Tick) {}

    fn decode(&self, loc: &Loc, footage: &Footage) -> Result<Image> {
        let op = AlphaOp::new(footage.alpha, footage.premul_color);
        let path = &loc.key.path;
        match loc.media_t {
            None if &**path == PLACEHOLDER => Ok(colour_bars(footage.width, footage.height)),
            None => {
                let bytes = self.read(path)?;
                if let Some(img) = crate::layered::decode(path, &bytes, footage, op)? {
                    return Ok(img);
                }
                let img = match image::load_from_memory(&bytes) {
                    Ok(img) => img,
                    // A multi-layer OpenEXR file without an unnamed RGB layer: its colour layer.
                    Err(e) => crate::exr_channels::layered_image(&bytes).ok_or_else(|| MediaError::Decode(format!("{path}: {e}")))?,
                };
                Ok(dynamic_to_image(&img, op))
            }
            Some(mt) => {
                let src = self.source(path).ok_or_else(|| MediaError::Io(format!("{path}: cannot open")))?;
                let vf = src.video_frame(FrameRequest::full(filmcraft_time::Tick(mt.0))).map_err(MediaError::from)?;
                let buf = lock(&self.cache).lru.take_spare(vf.width as usize * vf.height as usize);
                Ok(frame_to_image_in(&vf, op, buf))
            }
        }
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
    fn set_cache_budget(&self, bytes: usize) {
        self.set_budget(bytes);
    }
    fn purge(&self) {
        self.clear_frames();
    }
    fn cache_budget(&self) -> Option<usize> {
        Some(self.budget())
    }
    fn frame(&self, _item: ItemId, footage: &Footage, t: Tick) -> Option<Arc<Image>> {
        if footage.missing {
            return None;
        }
        match self.frame_at(footage, t) {
            Ok(img) => Some(img),
            Err(e) => {
                log::warn!("media: frame of {} at {:?}: {e}", footage.path, t);
                None
            }
        }
    }

    fn vector_frame(&self, _item: ItemId, footage: &Footage, scale: f64) -> Option<Arc<Image>> {
        if footage.missing || !effectcraft_render::is_vector_footage(footage) {
            return None;
        }
        let bytes = self.inner.read(&footage.path).ok()?;
        crate::layered::rasterize_vector(&footage.path, &bytes, footage.layer.as_ref(), footage.page, scale).map(Arc::new)
    }

    fn aux(&self, _item: ItemId, footage: &Footage, t: Tick) -> Option<Arc<effectcraft_raster::AuxChannels>> {
        if footage.missing || !footage.has_video {
            return None;
        }
        let loc = Inner::locate(footage, t);
        if loc.media_t.is_some() || !loc.key.path.to_ascii_lowercase().ends_with(".exr") {
            return None;
        }
        let path = loc.key.path.clone();
        crate::exr_channels::cached(&path, || self.inner.read(&path).ok())
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
/// channels; silence before the start and where decoding fails.
fn read_stereo(src: &SharedSource, path: &str, s0: i64, frames: usize, rate: u32) -> Vec<f32> {
    let mut out = vec![0.0; frames * 2];
    let skip = usize::try_from(s0.saturating_neg()).unwrap_or(0);
    if skip >= frames {
        return out;
    }
    let buf = match src.audio(s0.max(0), frames - skip, rate) {
        Ok(b) => b,
        Err(e) => {
            log::warn!("media: audio of {path}: {e}");
            return out;
        }
    };
    let (Some(l), Some(r)) = (buf.channels.first(), buf.channels.get(1).or(buf.channels.first())) else { return out };
    let Some(dst) = skip.checked_mul(2).and_then(|i| out.get_mut(i..)) else { return out };
    for ([o0, o1], (a, b)) in dst.as_chunks_mut::<2>().0.iter_mut().zip(l.iter().zip(r)) {
        *o0 = *a;
        *o1 = *b;
    }
    out
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
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
        let img = || Arc::new(Image::new(16, 16)); // 4 KiB + overhead
        let k = |i| Key { path: "a".into(), frame: i, alpha: 0, matte: [0; 3] };
        let budget = 3 * (4096 + 64);
        for i in 0..10 {
            c.insert(k(i), img(), budget);
        }
        assert_eq!(c.map.len(), 3);
        assert!(c.get(&k(9)).is_some() && c.get(&k(6)).is_none());
        // touching 7 makes 8 the oldest
        c.get(&k(7));
        c.insert(k(10), img(), budget);
        assert!(c.get(&k(7)).is_some() && c.get(&k(8)).is_none());
        assert_eq!(c.spare.len(), MAX_SPARE);
        assert_eq!(c.take_spare(256).len(), 256);
        assert!(c.take_spare(17).is_empty());
    }
}
