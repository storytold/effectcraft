//! The plane store (#3271): decoded OpenEXR channel planes in RAM, one entry per (file, level),
//! shared by every consumer of a frame: the picture of a footage layer and the channels its
//! effects read (EXtractoR, Cryptomatte, the 3D Channel effects). It replaces the pool's own
//! OpenEXR decode and the separate channel cache of #474.
//!
//! - **Single flight.** One decode per entry at a time; requests for it wait instead of
//!   decoding again. A request arriving before the decode has started reading adds its
//!   channels to it; one arriving later waits, and if that decode doesn't cover it, the next
//!   decode reads only the channels still missing.
//! - **Covered requests are served from the store**: an entry holds one set of planes (the
//!   union of what was decoded, [`Batch`]), and any request for a subset gets it as it is.
//! - **Never on the wrong thread.** The decode runs on the footage decode threads
//!   ([`crate::pool::decode_threads`]); a request on another thread hands it over and blocks
//!   plainly. A thread running a decode never waits for another (rayon lets a thread waiting in
//!   a parallel loop run other jobs, which could wait on that very decode): it decodes a copy.
//! - **Nothing made before a Reload is kept or served** ([`forget`]): a decode that started
//!   before the file was forgotten keeps nothing, and the requests waiting for it ask again.
//! - Bounded by bytes ([`CACHE_BUDGET`]), least recently used out first; entries in use (planes
//!   an effect holds, a decode in progress, the last pictures' planes kept for their effects,
//!   [`HELD_FILES`]) are never dropped, and the newest stays even when it alone is over.

use std::collections::{BTreeSet, VecDeque};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};

use effectcraft_raster::channels3d::AuxNeed;
use effectcraft_raster::{AuxChannels, Image};
use effectcraft_render::reads::ReadFile;

use crate::convert::AlphaOp;
use crate::exr_channels::{Rgb, planes_picture};
use crate::exr_index::{self, ExrIndex, IndexError};
use crate::exr_channels::CACHE_BUDGET;
use crate::exr_read::{ExrSource, ReadError, ReadRequest, read_planes};

/// At most this many entries are kept (unreadable files hold no bytes).
const CACHE_FILES: usize = 64;

/// At most this many pictures' planes are kept for their effects (the renderer asks for a
/// frame's picture, then its channels): about as many frames as are rendered at once.
const HELD_FILES: usize = 4;

/// How long a request waits for another's decode before it decodes a copy (a safety net for a
/// decode that hangs).
#[cfg(not(target_arch = "wasm32"))]
const WAIT_LIMIT: std::time::Duration = std::time::Duration::from_secs(10);

/// How often a request asks again after a decode it waited for was called off or forgotten.
const RETRIES: u32 = 3;

/// Decoded planes of a file: the union of the channels read so far.
pub(crate) struct Batch {
    /// The index they were read with (channel ids are its).
    pub(crate) index: Arc<ExrIndex>,
    /// The channels held, in file order (parallel to `aux.channels`).
    pub(crate) ids: Vec<usize>,
    /// Every channel of the file.
    pub(crate) all: bool,
    pub(crate) aux: Arc<AuxChannels>,
    /// The file as read.
    pub(crate) file: ReadFile,
}

impl Batch {
    /// Bytes held (the planes; names and manifests are small).
    fn bytes(&self) -> usize {
        self.aux.channels.iter().map(|(n, p)| n.len() + std::mem::size_of_val(p.as_slice())).sum()
    }

    /// The plane of channel `id`.
    pub(crate) fn plane(&self, id: usize) -> Option<&[f32]> {
        let at = self.ids.binary_search(&id).ok()?;
        self.aux.channels.get(at).map(|(_, p)| p.as_slice())
    }

    /// It holds what `want` asks of the file `index` describes.
    fn covers(&self, index: &Arc<ExrIndex>, want: &Want) -> bool {
        Arc::ptr_eq(&self.index, index) && (self.all || (!want.all && want.ids.iter().all(|id| self.ids.binary_search(id).is_ok())))
    }
}

/// The channels a request asks for.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Want {
    /// Every channel.
    pub(crate) all: bool,
    pub(crate) ids: BTreeSet<usize>,
}

impl Want {
    fn add(&mut self, other: &Want) {
        self.all |= other.all;
        self.ids.extend(other.ids.iter().copied());
    }
}

/// Who asks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Use {
    /// The picture of a footage item whose effects read its channels next: kept for them.
    Picture,
    /// Anything else (the channels themselves, a picture alone).
    Other,
}

/// Why a request got no planes.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Refused {
    /// Not readable (remembered until the file is forgotten).
    Unreadable,
    /// More planes than one file may allocate (the reason; remembered).
    TooLarge(String),
    /// The request was called off.
    Cancelled,
    /// The file changed since it was indexed: index it again and ask again.
    Changed,
    /// The file was forgotten while it decoded: ask again.
    Forgotten,
}

/// A decode in progress: what it reads (until it starts reading) and its outcome.
struct Flight {
    state: Mutex<FlightState>,
    ready: Condvar,
}

struct FlightState {
    index: Arc<ExrIndex>,
    want: Want,
    /// Reading started: requests no longer add to `want`.
    sealed: bool,
    outcome: Option<Result<Arc<Batch>, Refused>>,
}

impl Flight {
    fn new(index: Arc<ExrIndex>, want: Want) -> Arc<Flight> {
        Arc::new(Flight { state: Mutex::new(FlightState { index, want, sealed: false, outcome: None }), ready: Condvar::new() })
    }

    fn state(&self) -> MutexGuard<'_, FlightState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn end(&self, outcome: Result<Arc<Batch>, Refused>) {
        self.state().outcome = Some(outcome);
        self.ready.notify_all();
    }
}

struct Entry {
    path: String,
    level: u32,
    batch: Option<Arc<Batch>>,
    flight: Option<Arc<Flight>>,
    failed: Option<Refused>,
    bytes: usize,
    /// When it was last asked for or finished decoding (a counter).
    used: u64,
}

impl Entry {
    /// Dropping it would free nothing or lose work in progress: a decode runs, or planes are held.
    fn in_use(&self) -> bool {
        self.flight.is_some() || self.batch.as_ref().is_some_and(|b| Arc::strong_count(b) > 1 || Arc::strong_count(&b.aux) > 1)
    }
}

struct State {
    entries: Vec<Entry>,
    clock: u64,
    /// Pictures' planes kept for their effects, oldest first (at most [`HELD_FILES`]).
    held: VecDeque<(String, Arc<Batch>)>,
}

/// The request decoding an entry: it stores the outcome and ends the flight, also when the decode
/// panics (then with an error), so no request waits for planes that never come and the entry
/// doesn't stay "decoding".
struct Owning<'a> {
    store: &'a PlaneStore,
    path: &'a str,
    level: u32,
    flight: &'a Arc<Flight>,
    ended: bool,
}

impl Owning<'_> {
    fn end(&mut self, outcome: Result<Arc<Batch>, Refused>) -> Result<Arc<Batch>, Refused> {
        self.ended = true;
        let outcome = self.store.finish(self.path, self.level, self.flight, outcome);
        self.flight.end(outcome.clone());
        outcome
    }
}

impl Drop for Owning<'_> {
    fn drop(&mut self) {
        if !self.ended {
            let _ = self.end(Err(Refused::Unreadable));
        }
    }
}

/// Decoded planes by (file, level), bounded by bytes.
pub(crate) struct PlaneStore {
    state: Mutex<State>,
    budget: usize,
}

thread_local! {
    /// Store decodes running on this thread: such a thread doesn't wait for another decode.
    static DECODING: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Marks a store decode on this thread ([`DECODING`]) until dropped (also when it panics).
struct Decoding;

impl Decoding {
    fn new() -> Decoding {
        DECODING.with(|n| n.set(n.get().saturating_add(1)));
        Decoding
    }
}

impl Drop for Decoding {
    fn drop(&mut self) {
        DECODING.with(|n| n.set(n.get().saturating_sub(1)));
    }
}

/// What waiting for another request's decode ended with.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
enum Waited {
    Got(Result<Arc<Batch>, Refused>),
    /// This request was called off meanwhile.
    Cancelled,
    /// This thread may not wait, or waited too long: decode a copy.
    No,
}

/// One decode the store ran (tests and measurements): the channels read and what it cost.
#[cfg(test)]
#[derive(Clone, Debug)]
pub(crate) struct DecodeRecord {
    pub(crate) channels: Vec<String>,
    pub(crate) stats: crate::exr_read::ReadStats,
}

#[cfg(test)]
static DECODES: Mutex<Vec<(String, DecodeRecord)>> = Mutex::new(Vec::new());

/// The decodes of `path` so far (tests; paths are unique per test).
#[cfg(test)]
pub(crate) fn decodes_of(path: &str) -> Vec<DecodeRecord> {
    DECODES.lock().unwrap_or_else(PoisonError::into_inner).iter().filter(|(p, _)| p == path).map(|(_, d)| d.clone()).collect()
}

/// Runs before a decode reads its chunks, with the file's path (tests).
#[cfg(test)]
type Hook = Arc<dyn Fn(&str) + Send + Sync>;
#[cfg(test)]
static BEFORE_READ: Mutex<Option<Hook>> = Mutex::new(None);

#[cfg(test)]
pub(crate) fn set_before_read(hook: Option<Hook>) {
    *BEFORE_READ.lock().unwrap_or_else(PoisonError::into_inner) = hook;
}

impl PlaneStore {
    pub(crate) const fn new(budget: usize) -> PlaneStore {
        PlaneStore { state: Mutex::new(State { entries: Vec::new(), clock: 0, held: VecDeque::new() }), budget }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The planes of `want` of the file at `path` (`index`, read from `source`): from the store,
    /// from a decode in progress, or decoded now (only what the store doesn't hold yet). The
    /// file it was read from is noted ([`crate::reads`]).
    pub(crate) fn get(&self, path: &str, source: &ExrSource, index: &Arc<ExrIndex>, want: &Want, by: Use) -> Result<Arc<Batch>, Refused> {
        let level = 0;
        let mut retries = 0;
        loop {
            let mut st = self.lock();
            st.clock += 1;
            let clock = st.clock;
            let at = match st.entries.iter().position(|e| e.path == path && e.level == level) {
                Some(i) => i,
                None => {
                    st.entries.push(Entry { path: path.to_string(), level, batch: None, flight: None, failed: None, bytes: 0, used: clock });
                    st.entries.len() - 1
                }
            };
            let Some(e) = st.entries.get_mut(at) else { return Err(Refused::Unreadable) };
            e.used = clock;
            if let Some(f) = &e.failed {
                return Err(f.clone());
            }
            if let Some(b) = e.batch.clone().filter(|b| b.covers(index, want)) {
                Self::hold(&mut st, path, &b, by);
                drop(st);
                crate::reads::note(b.file.clone());
                return Ok(b);
            }
            if let Some(flight) = e.flight.clone() {
                {
                    let mut fs = flight.state();
                    if !fs.sealed && Arc::ptr_eq(&fs.index, index) {
                        fs.want.add(want);
                    }
                }
                drop(st);
                match self.wait(&flight) {
                    Waited::Got(Ok(b)) if b.covers(index, want) => {
                        crate::reads::note(b.file.clone());
                        return Ok(b);
                    }
                    // Not all it asks for: ask again (the next decode reads what is missing).
                    Waited::Got(Ok(_)) => {}
                    Waited::Got(Err(Refused::Cancelled | Refused::Forgotten)) if retries < RETRIES => retries += 1,
                    Waited::Got(Err(e)) => return Err(e),
                    Waited::Cancelled => return Err(Refused::Cancelled),
                    // A copy, kept nowhere.
                    Waited::No => return run(path, source, &Flight::new(index.clone(), want.clone()), None),
                }
                continue;
            }
            // This request decodes: what it asks for, minus what the store holds.
            let base = e.batch.clone().filter(|b| Arc::ptr_eq(&b.index, index));
            let flight = Flight::new(index.clone(), want.clone());
            e.flight = Some(flight.clone());
            drop(st);
            #[cfg(test)]
            crate::exr_channels::count_decode(true);
            let mut owning = Owning { store: self, path, level, flight: &flight, ended: false };
            let outcome = run(path, source, &flight, base);
            let outcome = owning.end(outcome);
            match outcome {
                Ok(b) if b.covers(index, want) => {
                    let mut st = self.lock();
                    Self::hold(&mut st, path, &b, by);
                    drop(st);
                    crate::reads::note(b.file.clone());
                    return Ok(b);
                }
                Ok(_) | Err(Refused::Forgotten) if retries < RETRIES => retries += 1,
                Ok(_) | Err(Refused::Forgotten) => return Err(Refused::Cancelled),
                Err(e) => return Err(e),
            }
        }
    }

    /// Keep `b` for the effects of the picture asking (`Use::Picture`), or stop keeping the
    /// file's planes once its channels are asked for.
    fn hold(st: &mut State, path: &str, b: &Arc<Batch>, by: Use) {
        match by {
            Use::Picture => {
                if !st.held.iter().any(|(p, _)| p == path) {
                    st.held.push_back((path.to_string(), b.clone()));
                    if st.held.len() > HELD_FILES {
                        st.held.pop_front();
                    }
                }
            }
            Use::Other => st.held.retain(|(p, _)| p != path),
        }
    }

    /// Store the outcome of `flight`'s decode in its entry, unless the file was forgotten
    /// meanwhile (then nothing made of it is kept or handed out); drop what doesn't fit.
    fn finish(&self, path: &str, level: u32, flight: &Arc<Flight>, outcome: Result<Arc<Batch>, Refused>) -> Result<Arc<Batch>, Refused> {
        let mut st = self.lock();
        st.clock += 1;
        let clock = st.clock;
        let Some(e) = st.entries.iter_mut().find(|e| e.path == path && e.level == level && e.flight.as_ref().is_some_and(|f| Arc::ptr_eq(f, flight))) else {
            return match outcome {
                Err(Refused::Cancelled) => Err(Refused::Cancelled),
                _ => Err(Refused::Forgotten),
            };
        };
        e.flight = None;
        e.used = clock;
        match &outcome {
            Ok(b) => {
                e.bytes = b.bytes();
                e.batch = Some(b.clone());
            }
            Err(Refused::Unreadable) => e.failed = Some(Refused::Unreadable),
            Err(Refused::TooLarge(why)) => e.failed = Some(Refused::TooLarge(why.clone())),
            // (the file changed: what is held is of the old one)
            Err(Refused::Changed) => {
                e.batch = None;
                e.bytes = 0;
            }
            Err(Refused::Cancelled | Refused::Forgotten) => {}
        }
        Self::evict(&mut st, self.budget, path, level);
        outcome
    }

    /// Drop least recently used entries not in use until the rest fit the budget, keeping the
    /// newest (`path` at `level`, just decoded) and the pictures' planes held for their effects.
    fn evict(st: &mut State, budget: usize, path: &str, level: u32) {
        loop {
            if st.entries.iter().map(|e| e.bytes).fold(0usize, usize::saturating_add) <= budget && st.entries.len() <= CACHE_FILES {
                break;
            }
            let held = &st.held;
            let oldest = st
                .entries
                .iter()
                .enumerate()
                .filter(|(_, e)| !(e.path == path && e.level == level) && !e.in_use())
                .filter(|(_, e)| !e.batch.as_ref().is_some_and(|b| held.iter().any(|(_, h)| Arc::ptr_eq(h, b))))
                .min_by_key(|(_, e)| e.used)
                .map(|(i, _)| i);
            let Some(i) = oldest else { break };
            st.entries.remove(i);
        }
    }

    /// Wait for the outcome of another request's decode (`flight`). A thread running a store
    /// decode doesn't wait (see the module notes); the time limit is a safety net.
    #[cfg(not(target_arch = "wasm32"))]
    fn wait(&self, flight: &Flight) -> Waited {
        if DECODING.with(std::cell::Cell::get) > 0 {
            return Waited::No;
        }
        let until = std::time::Instant::now() + WAIT_LIMIT;
        let mut fs = flight.state();
        loop {
            if let Some(o) = fs.outcome.as_ref() {
                return Waited::Got(o.clone());
            }
            if filmcraft_media::cancel::cancelled() {
                return Waited::Cancelled;
            }
            let left = until.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                log::warn!("media: OpenEXR planes took over {} s to decode on another thread; decoding a copy", WAIT_LIMIT.as_secs());
                return Waited::No;
            }
            fs = match flight.ready.wait_timeout(fs, left.min(std::time::Duration::from_millis(20))) {
                Ok((g, _)) => g,
                Err(p) => p.into_inner().0,
            };
        }
    }

    /// (A wasm32 build has one thread: a decode in flight is further down this thread's stack.)
    #[cfg(target_arch = "wasm32")]
    fn wait(&self, _flight: &Flight) -> Waited {
        Waited::No
    }

    /// Drop the planes of `path` (the file changed on disk); a decode of it in progress keeps
    /// nothing.
    pub(crate) fn forget(&self, path: &str) {
        let mut st = self.lock();
        st.entries.retain(|e| e.path != path);
        st.held.retain(|(p, _)| p != path);
    }

    /// Stop keeping the last pictures' planes for their effects.
    pub(crate) fn release_held(&self) {
        self.lock().held.clear();
    }

    #[cfg(test)]
    pub(crate) fn cached(&self) -> Vec<String> {
        self.lock().entries.iter().filter(|e| e.batch.is_some() || e.failed.is_some()).map(|e| e.path.clone()).collect()
    }

    #[cfg(test)]
    pub(crate) fn is_held(&self, path: &str) -> bool {
        self.lock().held.iter().any(|(p, _)| p == path)
    }
}

/// Run `flight`'s decode on the footage decode threads (here when this is one of them, or
/// there are none) and block until it is done.
fn run(path: &str, source: &ExrSource, flight: &Arc<Flight>, base: Option<Arc<Batch>>) -> Result<Arc<Batch>, Refused> {
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(threads) = crate::pool::decode_threads()
        && threads.current_thread_index().is_none()
    {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let (path, source, flight) = (path.to_string(), source.clone(), flight.clone());
        // The request's cancel flag and decoding hints go with it.
        let hints = filmcraft_media::cancel::Context::current();
        threads.spawn(move || {
            let _ = tx.send(hints.run(|| decode(&path, &source, &flight, base)));
        });
        // A plain block: this thread runs nothing else meanwhile.
        return rx.recv().unwrap_or(Err(Refused::Unreadable));
    }
    decode(path, source, flight, base)
}

/// Decode what `flight` asks for and `base` doesn't hold, and make the union of both.
fn decode(path: &str, source: &ExrSource, flight: &Arc<Flight>, base: Option<Arc<Batch>>) -> Result<Arc<Batch>, Refused> {
    let _decoding = Decoding::new();
    let mut opened = source.open().map_err(|e| {
        log::warn!("media: {path}: {e}");
        Refused::Unreadable
    })?;
    let index = flight.state().index.clone();
    if opened.stamp != index.stamp {
        // The file changed since it was indexed.
        let fresh = exr_index::build(&mut *opened.reader, opened.len, opened.stamp).map_err(|_| Refused::Unreadable)?;
        exr_index::keep(path, Arc::new(fresh));
        return Err(Refused::Changed);
    }
    #[cfg(test)]
    {
        let hook = BEFORE_READ.lock().unwrap_or_else(PoisonError::into_inner).clone();
        if let Some(h) = hook {
            h(path);
        }
    }
    // From here on the request is fixed.
    let want = {
        let mut fs = flight.state();
        fs.sealed = true;
        fs.want.clone()
    };
    let wanted: Vec<usize> = if want.all { (0..index.channels.len()).collect() } else { want.ids.iter().copied().filter(|i| *i < index.channels.len()).collect() };
    let base = base.filter(|b| Arc::ptr_eq(&b.index, &index));
    let missing: Vec<usize> = wanted.iter().copied().filter(|id| !base.as_ref().is_some_and(|b| b.ids.binary_search(id).is_ok())).collect();
    let (planes, stats) = read_planes(&mut *opened.reader, &index, &ReadRequest::full(missing.clone()));
    #[cfg(test)]
    DECODES.lock().unwrap_or_else(PoisonError::into_inner).push((
        path.to_string(),
        DecodeRecord { channels: missing.iter().filter_map(|i| index.channels.get(*i)).map(|c| c.name.clone()).collect(), stats: stats.clone() },
    ));
    #[cfg(not(test))]
    let _ = &stats;
    let planes = planes.map_err(|e| match e {
        ReadError::Cancelled => Refused::Cancelled,
        ReadError::TooLarge(why) => {
            log::warn!("media: {path}: {why}");
            Refused::TooLarge(why)
        }
        ReadError::Unreadable(why) => {
            log::warn!("media: {path}: {why}");
            Refused::Unreadable
        }
    })?;
    // The union, in file order: the planes held and the ones just read.
    let mut new: Vec<(usize, Vec<f32>)> = missing.into_iter().zip(planes).collect();
    if let Some(b) = &base {
        for (at, id) in b.ids.iter().enumerate() {
            if let Some((_, p)) = b.aux.channels.get(at) {
                new.push((*id, p.clone()));
            }
        }
    }
    new.sort_by_key(|(id, _)| *id);
    new.dedup_by_key(|(id, _)| *id);
    let all = new.len() == index.channels.len();
    let (w, h) = index.size();
    let mut aux = AuxChannels::new(w as u32, h as u32, 1.0);
    aux.manifests = index.names.manifests.clone();
    aux.aliases = index.names.aliases.clone();
    if !all {
        aux.listed = index.channels.iter().map(|c| c.name.clone()).collect();
    }
    let ids: Vec<usize> = new.iter().map(|(id, _)| *id).collect();
    aux.channels = new.into_iter().filter_map(|(id, p)| Some((index.channels.get(id)?.name.clone(), p))).collect();
    Ok(Arc::new(Batch { index, ids, all, aux: Arc::new(aux), file: ReadFile::new(path, opened.stamp) }))
}

fn store() -> &'static PlaneStore {
    static STORE: PlaneStore = PlaneStore::new(CACHE_BUDGET);
    &STORE
}

/// Is `path` an OpenEXR file by name (the store reads those; other files, and files named so
/// that aren't OpenEXR, go the `image` crate's way).
pub(crate) fn is_exr(path: &str) -> bool {
    path.to_ascii_lowercase().ends_with(".exr")
}

/// The index of `path`, and the store's verdict on an unreadable one.
fn index_of(path: &str, source: &ExrSource) -> Result<Arc<ExrIndex>, IndexError> {
    exr_index::get(path, source)
}

/// The ids `need` reads of the file `index` describes (`None`: all).
fn need_ids(index: &ExrIndex, need: &AuxNeed) -> Option<BTreeSet<usize>> {
    let names = need.resolve(&index.names)?;
    Some(index.ids_named(names.iter().map(String::as_str)).into_iter().collect())
}

/// The outcome of [`picture`].
pub(crate) enum Picture {
    Done(Result<Image, String>),
    Cancelled,
    /// Not for the planes (an `image` crate picture, or not an OpenEXR this reader takes):
    /// decode the whole file with the `image` crate.
    Legacy,
}

/// The picture of the OpenEXR at `path` made of its planes: the channels the `image` crate's
/// decoder would show, or a multi-layer file's colour layer (see
/// [`crate::exr_channels::layered_image`]). With `extra` (an effect reads the footage's
/// channels) those channels are read in the same pass and kept for the effects.
pub(crate) fn picture(path: &str, source: &ExrSource, op: AlphaOp, extra: Option<&AuxNeed>) -> Picture {
    for _ in 0..2 {
        let index = match index_of(path, source) {
            Ok(ix) => ix,
            Err(IndexError::Unsupported(why)) => return Picture::Done(Err(why.to_string())),
            Err(_) => return Picture::Legacy,
        };
        let Some(ids) = index.picture_ids() else { return Picture::Legacy };
        let mut want = Want { all: false, ids: ids.into_iter().collect() };
        if let Some(need) = extra {
            match need_ids(&index, need) {
                Some(more) => want.ids.extend(more),
                None => want.all = true,
            }
        }
        let by = if extra.is_some() { Use::Picture } else { Use::Other };
        let batch = match store().get(path, source, &index, &want, by) {
            Ok(b) => b,
            Err(Refused::Changed) => continue,
            Err(Refused::Cancelled | Refused::Forgotten) => return Picture::Cancelled,
            Err(Refused::TooLarge(why)) => return Picture::Done(Err(why)),
            Err(Refused::Unreadable) => return Picture::Legacy,
        };
        let aux = &batch.aux;
        let img = match index.rgb {
            Rgb::Planes([r, g, b], a) => {
                let a = match a {
                    Some(a) => batch.plane(a).map(Some),
                    None => Some(None),
                };
                match (batch.plane(r), batch.plane(g), batch.plane(b), a) {
                    (Some(r), Some(g), Some(b), Some(a)) => planes_picture(aux, [r, g, b], a, op),
                    _ => None,
                }
            }
            Rgb::None => index.picture.as_ref().and_then(|(_, rgb, a)| {
                let a = match a {
                    Some(n) => Some(aux.get(n)?),
                    None => None,
                };
                planes_picture(aux, [aux.get(&rgb[0])?, aux.get(&rgb[1])?, aux.get(&rgb[2])?], a, op)
            }),
            Rgb::Unsupported => None,
        };
        return match img {
            Some(img) => Picture::Done(Ok(img)),
            None => Picture::Legacy,
        };
    }
    Picture::Legacy
}

/// The channels `need` reads of the OpenEXR at `path` (all for [`AuxNeed::All`]), from the
/// store. `None` when it isn't a readable OpenEXR (or the request was called off).
pub(crate) fn aux(path: &str, source: &ExrSource, need: &AuxNeed) -> Option<Arc<AuxChannels>> {
    for _ in 0..2 {
        let index = index_of(path, source).ok()?;
        let want = match need_ids(&index, need) {
            Some(ids) => Want { all: false, ids },
            None => Want { all: true, ids: BTreeSet::new() },
        };
        match store().get(path, source, &index, &want, Use::Other) {
            Ok(b) => return Some(b.aux.clone()),
            Err(Refused::Changed) => continue,
            Err(_) => return None,
        }
    }
    None
}

/// The channel names (planes empty), manifests and aliases of the OpenEXR at `path`, from its
/// headers alone. Never handed to an effect.
pub(crate) fn names(path: &str, source: &ExrSource) -> Option<Arc<AuxChannels>> {
    index_of(path, source).ok().map(|ix| ix.names.clone())
}

/// Drop what is kept of `path`: its index and planes.
pub(crate) fn forget(path: &str) {
    exr_index::forget(path);
    store().forget(path);
}

/// Stop keeping the last pictures' planes for their effects (which footage effects read changed).
pub(crate) fn release_held() {
    store().release_held();
}

/// The store all footage shares (tests).
#[cfg(test)]
pub(crate) fn shared() -> &'static PlaneStore {
    store()
}

#[cfg(test)]
mod tests;
