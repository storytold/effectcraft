//! The plane store (#3271): decoded OpenEXR channel planes in RAM, one entry per file,
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
//! - Kept by the memory accountant (#3298): least recently used offered first, each entry worth
//!   what decoding it took. Also bounded by bytes by itself ([`CACHE_BUDGET`] until the caches'
//!   budget is set, then the footage frames' cap, [`set_budget`]: the whole budget on the
//!   desktop, at most [`crate::exr_channels::WEB_CACHE_CAP`] in the browser); there the newest
//!   entry stays even when it alone is over. Entries in use (planes an effect holds, a decode in
//!   progress, the last pictures' planes kept for their effects, [`HELD_FILES`]) are never
//!   dropped, by either.

use std::collections::{BTreeSet, VecDeque};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};

use rayon::prelude::*;

use effectcraft_raster::channels3d::AuxNeed;
use effectcraft_raster::{AuxChannels, Image};
use effectcraft_render::reads::ReadFile;

use crate::convert::AlphaOp;
use crate::exr_channels::CACHE_BUDGET;
use crate::exr_channels::{Rgb, planes_picture};
use crate::exr_index::{self, ExrIndex, IndexError};
use crate::exr_read::{ExrSource, Opened, ReadError, ReadRequest, read_planes};

/// At most this many entries are kept (unreadable files hold no bytes).
const CACHE_FILES: usize = 64;

/// At most this many pictures' planes are kept for their effects (the renderer asks for a
/// frame's picture, then its channels): about as many frames as are rendered at once.
const HELD_FILES: usize = 4;

/// How long a request waits for another's decode before it decodes a copy (a safety net for a
/// decode that hangs).
#[cfg(not(target_arch = "wasm32"))]
const WAIT_LIMIT: std::time::Duration = std::time::Duration::from_secs(10);

/// How often a request asks again after a decode it waited for was called off or forgotten,
/// and how many rounds it waits at most, before it decodes a copy itself.
const RETRIES: u32 = 3;
const ROUNDS: u32 = 8;

/// At most this many failed requests are remembered per entry.
const FAILED_KEPT: usize = 8;

/// What a round of [`PlaneStore::get`] ended with.
enum Step {
    Done(Result<Arc<Batch>, Refused>),
    /// Ask again (`retry`: the decode waited for was called off, stopped or forgotten).
    Again {
        retry: bool,
    },
}

/// A request for `want` of the file `index` describes asks for at least what `failed` asked for
/// (`all` is every channel but the picture's: it doesn't cover a picture channel asked for by id).
fn asks_at_least(index: &ExrIndex, want: &Want, failed: &Want) -> bool {
    if failed.all && !want.all {
        return false;
    }
    let aux = if want.all { index.aux_ids() } else { Vec::new() };
    failed.ids.iter().all(|id| want.ids.contains(id) || aux.contains(id))
}

/// Decode `want` for this request alone (kept nowhere), noting the file it read.
fn copy(path: &str, source: &ExrSource, index: &Arc<ExrIndex>, want: &Want) -> Result<Arc<Batch>, Refused> {
    let b = run(path, source, &Flight::new(index.clone(), want.clone()), None, None);
    if let Ok(b) = &b {
        crate::reads::note(b.file.clone());
    }
    b
}

/// Decoded planes of a file: the union of the channels read so far.
pub(crate) struct Batch {
    /// The index they were read with (channel ids are its).
    pub(crate) index: Arc<ExrIndex>,
    /// The channels held, in file order (parallel to `aux.channels`).
    pub(crate) ids: Vec<usize>,
    /// Every channel of the file but the picture's ([`ExrIndex::aux_ids`]; the picture's may be
    /// there too).
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
        Arc::ptr_eq(&self.index, index) && (self.all || !want.all) && want.ids.iter().all(|id| self.ids.binary_search(id).is_ok())
    }
}

/// The channels a request asks for.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Want {
    /// Every channel but the picture's ([`ExrIndex::aux_ids`]): the picture's planes are asked
    /// for by name (`ids`).
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
    /// The decode stopped (it panicked, or the file couldn't be opened just then): not
    /// remembered, the next request decodes again.
    Stopped,
}

/// A decode in progress: what it reads (until it starts reading) and its outcome.
struct Flight {
    state: Mutex<FlightState>,
    ready: Condvar,
    /// [`QUEUED`], [`RUNNING`] (the decode began where it was asked for) or [`CLAIMED`] (a
    /// decode thread took it over before it began, [`Flight::claim`]: the outcome comes through
    /// `handed`).
    phase: std::sync::atomic::AtomicU8,
    handed: Mutex<Option<Result<Arc<Batch>, Refused>>>,
    handed_ready: Condvar,
    /// What the store held when the decode was asked for (the decode adds to it).
    base: Option<Arc<Batch>>,
    /// The disk cache's drop generation before the file was opened for this request (#3191: a
    /// plane entry made of a file reloaded since is refused).
    since: u64,
    /// The cancel flag and decoding hints of the request that asked for the decode: a decode
    /// thread that claims it decodes under them (its own being called off doesn't call off the
    /// asker's decode).
    hints: filmcraft_media::cancel::Context,
    /// When the decode was asked for ([`effectcraft_render::memory::tick`]): what it took is the
    /// planes' cost to the memory accountant.
    asked: u64,
}

/// [`Flight::phase`] values.
const QUEUED: u8 = 0;
const RUNNING: u8 = 1;
const CLAIMED: u8 = 2;

/// The disk cache's drop generation now (0 without one): taken before a file is opened.
fn drop_generation() -> u64 {
    disk_cache().map_or(0, |d| d.drop_generation())
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
        Flight::with_base(index, want, None, drop_generation())
    }

    fn with_base(index: Arc<ExrIndex>, want: Want, base: Option<Arc<Batch>>, since: u64) -> Arc<Flight> {
        Arc::new(Flight {
            state: Mutex::new(FlightState { index, want, sealed: false, outcome: None }),
            ready: Condvar::new(),
            phase: std::sync::atomic::AtomicU8::new(QUEUED),
            handed: Mutex::new(None),
            handed_ready: Condvar::new(),
            base,
            since,
            hints: filmcraft_media::cancel::Context::current(),
            asked: effectcraft_render::memory::tick(),
        })
    }

    /// Move the flight from [`QUEUED`] to `to`; `false` when it left that phase already.
    fn take(&self, to: u8) -> bool {
        use std::sync::atomic::Ordering;
        self.phase.compare_exchange(QUEUED, to, Ordering::SeqCst, Ordering::SeqCst).is_ok()
    }

    /// Take over the decode while it is still queued (a decode thread asking for the planes
    /// it would wait for: waiting there could starve the queue it waits on). `false` once it
    /// began.
    fn claim(&self) -> bool {
        self.take(CLAIMED)
    }

    fn claimed(&self) -> bool {
        self.phase.load(std::sync::atomic::Ordering::SeqCst) == CLAIMED
    }

    /// Hand the claimed decode's outcome to the request that queued it.
    fn hand(&self, outcome: Result<Arc<Batch>, Refused>) {
        *self.handed.lock().unwrap_or_else(PoisonError::into_inner) = Some(outcome);
        self.handed_ready.notify_all();
    }

    /// The outcome a claiming decode thread hands over (a plain block: that decode is running
    /// and waits for nothing).
    fn take_handed(&self) -> Result<Arc<Batch>, Refused> {
        let mut h = self.handed.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if let Some(o) = h.as_ref() {
                return o.clone();
            }
            h = match self.handed_ready.wait(h) {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
        }
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
    batch: Option<Arc<Batch>>,
    flight: Option<Arc<Flight>>,
    /// Requests that failed for good (unreadable, too large), the index they were made with, and
    /// why: a request of that index (the file as it was) asking for at
    /// least as much fails the same way until the file is forgotten.
    failed: Vec<(Arc<ExrIndex>, Want, Refused)>,
    bytes: usize,
    /// When it was last asked for or finished decoding ([`effectcraft_render::memory::tick`]).
    used: u64,
    /// What decoding its planes took, in microseconds (0: not decoded yet).
    cost_us: u64,
}

impl Entry {
    /// Dropping it would free nothing or lose work in progress: a decode runs, or planes are held.
    fn in_use(&self) -> bool {
        self.flight.is_some() || self.batch.as_ref().is_some_and(|b| Arc::strong_count(b) > 1 || Arc::strong_count(&b.aux) > 1)
    }
}

struct State {
    entries: Vec<Entry>,
    /// Pictures' planes kept for their effects, oldest first (at most [`HELD_FILES`]).
    held: VecDeque<(String, Arc<Batch>)>,
}

/// The request decoding an entry: it stores the outcome and ends the flight, also when the decode
/// panics (then with an error), so no request waits for planes that never come and the entry
/// doesn't stay "decoding".
struct Owning<'a> {
    store: &'a PlaneStore,
    path: &'a str,
    flight: &'a Arc<Flight>,
    ended: bool,
}

impl Owning<'_> {
    fn end(&mut self, outcome: Result<Arc<Batch>, Refused>) -> Result<Arc<Batch>, Refused> {
        self.ended = true;
        let (outcome, grown) = self.store.finish(self.path, self.flight, outcome);
        self.flight.end(outcome.clone());
        effectcraft_render::memory::Accountant::global().note_growth(grown);
        outcome
    }
}

impl Drop for Owning<'_> {
    fn drop(&mut self) {
        if !self.ended {
            let _ = self.end(Err(Refused::Stopped));
        }
    }
}

/// Decoded planes by file, bounded by bytes.
pub(crate) struct PlaneStore {
    state: Mutex<State>,
    /// Bytes it keeps at most ([`set_budget`]).
    budget: std::sync::atomic::AtomicUsize,
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
    /// Read from the disk cache, not from the file.
    pub(crate) from_disk: bool,
}

#[cfg(test)]
static DECODES: Mutex<Vec<(String, DecodeRecord)>> = Mutex::new(Vec::new());

/// Paths whose queued decode a decode thread took over (tests), and a signal for each.
#[cfg(test)]
pub(crate) static CLAIMS: Mutex<Vec<String>> = Mutex::new(Vec::new());
#[cfg(test)]
pub(crate) static CLAIM_SIGNAL: Condvar = Condvar::new();

/// The decodes of `path` so far (tests; paths are unique per test).
#[cfg(test)]
pub(crate) fn decodes_of(path: &str) -> Vec<DecodeRecord> {
    DECODES.lock().unwrap_or_else(PoisonError::into_inner).iter().filter(|(p, _)| p == path).map(|(_, d)| d.clone()).collect()
}

/// Run before a decode of a path is queued on the decode threads (`QUEUING`), when the
/// queued job begins (`STARTING`) and after it sent its outcome (`SENT`), keyed by path (tests:
/// holding a decode queued, or its decode thread busy after it).
#[cfg(test)]
pub(crate) static QUEUING: Mutex<Vec<(String, Hook)>> = Mutex::new(Vec::new());
#[cfg(test)]
pub(crate) static STARTING: Mutex<Vec<(String, Hook)>> = Mutex::new(Vec::new());
#[cfg(test)]
pub(crate) static SENT: Mutex<Vec<(String, Hook)>> = Mutex::new(Vec::new());

#[cfg(test)]
fn run_hook(hooks: &Mutex<Vec<(String, Hook)>>, path: &str) {
    let hook = hooks.lock().unwrap_or_else(PoisonError::into_inner).iter().find(|(p, _)| p == path).map(|(_, h)| h.clone());
    if let Some(h) = hook {
        h(path);
    }
}

/// Runs before a decode reads its chunks, with the file's path (tests).
#[cfg(test)]
type Hook = Arc<dyn Fn(&str) + Send + Sync>;
#[cfg(test)]
static BEFORE_READ: Mutex<Vec<(String, Hook)>> = Mutex::new(Vec::new());

/// Run `hook` before each decode of `path` reads its chunks (`None`: no longer). Keyed by path,
/// so tests running at the same time don't see each other's.
#[cfg(test)]
pub(crate) fn set_before_read(path: &str, hook: Option<Hook>) {
    let mut hooks = BEFORE_READ.lock().unwrap_or_else(PoisonError::into_inner);
    hooks.retain(|(p, _)| p != path);
    if let Some(h) = hook {
        hooks.push((path.to_string(), h));
    }
}

/// Planes of `path` may take at most this many bytes in a read (tests: a small cap in place of
/// [`crate::exr_read::MAX_DECODE_BYTES`]).
#[cfg(test)]
static CAPS: Mutex<Vec<(String, usize)>> = Mutex::new(Vec::new());

#[cfg(test)]
pub(crate) fn set_cap(path: &str, bytes: usize) {
    CAPS.lock().unwrap_or_else(PoisonError::into_inner).push((path.to_string(), bytes));
}

/// The most bytes of planes one read of `path` may allocate.
fn cap_of(path: &str) -> usize {
    #[cfg(test)]
    if let Some((_, c)) = CAPS.lock().unwrap_or_else(PoisonError::into_inner).iter().find(|(p, _)| p == path) {
        return *c;
    }
    let _ = path;
    crate::exr_read::MAX_DECODE_BYTES
}

impl PlaneStore {
    pub(crate) const fn new(budget: usize) -> PlaneStore {
        PlaneStore { state: Mutex::new(State { entries: Vec::new(), held: VecDeque::new() }), budget: std::sync::atomic::AtomicUsize::new(budget) }
    }

    fn budget(&self) -> usize {
        self.budget.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Keep at most `bytes` from now on, dropping the oldest entries not in use at once.
    pub(crate) fn set_budget(&self, bytes: usize) {
        self.budget.store(bytes, std::sync::atomic::Ordering::Relaxed);
        Self::drop_oldest(&mut self.lock(), bytes, CACHE_FILES, None);
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The planes of `want` of the file at `path` (`index`, read from `source`): from the store,
    /// from a decode in progress, or decoded now (only what the store doesn't hold yet). The
    /// file it was read from is noted ([`crate::reads`]). A request that keeps finding the
    /// decodes it waits for called off decodes a copy itself.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn get(&self, path: &str, source: &ExrSource, index: &Arc<ExrIndex>, want: &Want, by: Use) -> Result<Arc<Batch>, Refused> {
        self.get_opened(path, source, index, want, by, None, drop_generation())
    }

    /// [`PlaneStore::get`] with the file already open (`opened`, as indexing it opened it): a
    /// decode reads from it instead of opening the file again, so a new frame opens its file
    /// once (#3271). Dropped unused when the store serves the request. `since`: the disk cache's
    /// drop generation before the file was opened ([`drop_generation`]).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn get_opened(
        &self,
        path: &str,
        source: &ExrSource,
        index: &Arc<ExrIndex>,
        want: &Want,
        by: Use,
        mut opened: Option<Opened>,
        since: u64,
    ) -> Result<Arc<Batch>, Refused> {
        let mut retries = 0;
        for _ in 0..ROUNDS {
            match self.step(path, source, index, want, by, &mut opened, since) {
                Step::Done(r) => return r,
                Step::Again { retry: true } if retries >= RETRIES => break,
                Step::Again { retry } => retries += u32::from(retry),
            }
        }
        copy(path, source, index, want)
    }

    /// One round of [`PlaneStore::get`].
    #[allow(clippy::too_many_arguments)]
    fn step(&self, path: &str, source: &ExrSource, index: &Arc<ExrIndex>, want: &Want, by: Use, opened: &mut Option<Opened>, since: u64) -> Step {
        let mut st = self.lock();
        // (shared with the footage frames: one least-recently-used order across both)
        let clock = effectcraft_render::memory::tick();
        let at = match st.entries.iter().position(|e| e.path == path) {
            Some(i) => i,
            None => {
                st.entries.push(Entry { path: path.to_string(), batch: None, flight: None, failed: vec![], bytes: 0, used: clock, cost_us: 0 });
                st.entries.len() - 1
            }
        };
        let Some(e) = st.entries.get_mut(at) else { return Step::Done(Err(Refused::Unreadable)) };
        e.used = clock;
        // What the store holds first: a failure of a larger request doesn't hide it.
        if let Some(b) = e.batch.clone().filter(|b| b.covers(index, want)) {
            Self::hold(&mut st, path, &b, by);
            drop(st);
            crate::reads::note(b.file.clone());
            return Step::Done(Ok(b));
        }
        // (a failure of the file as it was before: the file changed since, it is read again)
        if let Some((_, _, why)) = e.failed.iter().find(|(of, failed, _)| Arc::ptr_eq(of, index) && asks_at_least(index, want, failed)) {
            return Step::Done(Err(why.clone()));
        }
        if let Some(flight) = e.flight.clone() {
            {
                let mut fs = flight.state();
                if !fs.sealed && Arc::ptr_eq(&fs.index, index) {
                    fs.want.add(want);
                }
            }
            drop(st);
            // A decode thread doesn't wait (see `wait`), but takes over a decode still queued:
            // the planes are decoded once, here, and handed to the request that queued them.
            if may_not_wait() && flight.claim() {
                return Step::Done(decode_claimed(path, source, &flight, index, want));
            }
            return match self.wait(&flight) {
                Waited::Got(Ok(b)) if b.covers(index, want) => {
                    crate::reads::note(b.file.clone());
                    Step::Done(Ok(b))
                }
                // Not all it asks for: ask again (the next decode reads what is missing).
                Waited::Got(Ok(_)) => Step::Again { retry: false },
                Waited::Got(Err(Refused::Cancelled | Refused::Forgotten | Refused::Stopped)) => Step::Again { retry: true },
                Waited::Got(Err(e)) => Step::Done(Err(e)),
                Waited::Cancelled => Step::Done(Err(Refused::Cancelled)),
                // A copy, kept nowhere.
                Waited::No => Step::Done(copy(path, source, index, want)),
            };
        }
        // This request decodes: what it asks for, minus what the store holds.
        let base = e.batch.clone().filter(|b| Arc::ptr_eq(&b.index, index));
        let flight = Flight::with_base(index.clone(), want.clone(), base.clone(), since);
        e.flight = Some(flight.clone());
        drop(st);
        #[cfg(test)]
        crate::exr_channels::count_decode(true);
        let mut owning = Owning { store: self, path, flight: &flight, ended: false };
        let outcome = run(path, source, &flight, base, opened.take());
        match owning.end(outcome) {
            Ok(b) if b.covers(index, want) => {
                let mut st = self.lock();
                Self::hold(&mut st, path, &b, by);
                drop(st);
                crate::reads::note(b.file.clone());
                Step::Done(Ok(b))
            }
            Ok(_) | Err(Refused::Forgotten | Refused::Stopped) => Step::Again { retry: true },
            Err(e) => Step::Done(Err(e)),
        }
    }

    /// Keep only the channels `keep` of the planes `b` of `path` (a picture converted: the pool
    /// keeps the picture, its planes would be a second copy of it); nothing when none is left.
    /// The set is trimmed in place when nobody else holds it, else the kept planes are copied.
    pub(crate) fn keep_only(&self, path: &str, b: Arc<Batch>, keep: &BTreeSet<usize>) {
        if b.ids.iter().all(|id| keep.contains(id)) {
            return;
        }
        let mut st = self.lock();
        let Some(at) = st.entries.iter().position(|e| e.path == path && e.batch.as_ref().is_some_and(|k| Arc::ptr_eq(k, &b))) else { return };
        let held = st.held.iter().any(|(p, h)| p == path && Arc::ptr_eq(h, &b));
        // (taken out of the entry and the held list: then this is likely the only holder)
        if let Some(e) = st.entries.get_mut(at) {
            e.batch = None;
            e.bytes = 0;
        }
        st.held.retain(|(p, h)| !(p == path && Arc::ptr_eq(h, &b)));
        let slim = match Arc::try_unwrap(b) {
            Ok(mut owned) => {
                let mut aux = Arc::try_unwrap(owned.aux).unwrap_or_else(|shared| AuxChannels::clone(&shared));
                let channels = std::mem::take(&mut aux.channels);
                aux.channels = owned.ids.iter().zip(channels).filter(|(id, _)| keep.contains(id)).map(|(_, c)| c).collect();
                owned.ids.retain(|id| keep.contains(id));
                aux.listed = owned.index.channels.iter().map(|c| c.name.clone()).collect();
                owned.aux = Arc::new(aux);
                owned
            }
            Err(shared) => {
                let mut aux = AuxChannels::new(shared.aux.width, shared.aux.height, shared.aux.scale);
                aux.manifests = shared.aux.manifests.clone();
                aux.aliases = shared.aux.aliases.clone();
                aux.listed = shared.index.channels.iter().map(|c| c.name.clone()).collect();
                aux.channels = shared.ids.iter().zip(&shared.aux.channels).filter(|(id, _)| keep.contains(id)).map(|(_, c)| c.clone()).collect();
                let ids = shared.ids.iter().copied().filter(|id| keep.contains(id)).collect();
                Batch { index: shared.index.clone(), ids, all: shared.all, aux: Arc::new(aux), file: shared.file.clone() }
            }
        };
        if slim.ids.is_empty() {
            return;
        }
        let slim = Arc::new(slim);
        if let Some(e) = st.entries.get_mut(at) {
            e.bytes = slim.bytes();
            e.batch = Some(slim.clone());
        }
        if held {
            st.held.push_back((path.to_string(), slim));
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
    /// Also the bytes the store grew by (for the memory accountant).
    fn finish(&self, path: &str, flight: &Arc<Flight>, outcome: Result<Arc<Batch>, Refused>) -> (Result<Arc<Batch>, Refused>, usize) {
        let mut st = self.lock();
        // (shared with the footage frames: one least-recently-used order across both)
        let clock = effectcraft_render::memory::tick();
        let Some(e) = st.entries.iter_mut().find(|e| e.path == path && e.flight.as_ref().is_some_and(|f| Arc::ptr_eq(f, flight))) else {
            let outcome = match outcome {
                Err(e @ (Refused::Cancelled | Refused::Stopped)) => Err(e),
                _ => Err(Refused::Forgotten),
            };
            return (outcome, 0);
        };
        e.flight = None;
        e.used = clock;
        let had = e.bytes;
        match &outcome {
            Ok(b) => {
                e.bytes = b.bytes();
                e.batch = Some(b.clone());
                let took = clock.saturating_sub(flight.asked).max(1);
                // A decode adding channels to what was held: both decodes made the planes; one
                // that read them all anew replaces what was held, and its cost.
                e.cost_us = if flight.base.is_some() { e.cost_us.saturating_add(took) } else { took };
            }
            Err(why @ (Refused::Unreadable | Refused::TooLarge(_))) => {
                let (index, want) = {
                    let fs = flight.state();
                    (fs.index.clone(), fs.want.clone())
                };
                // (failures of another version of the file no longer apply)
                e.failed.retain(|(of, _, _)| Arc::ptr_eq(of, &index));
                if e.failed.len() >= FAILED_KEPT {
                    e.failed.remove(0);
                }
                e.failed.push((index, want, why.clone()));
            }
            // (the file changed: what is held is of the old one)
            Err(Refused::Changed) => {
                e.batch = None;
                e.bytes = 0;
                e.cost_us = 0;
            }
            Err(Refused::Cancelled | Refused::Forgotten | Refused::Stopped) => {}
        }
        let grown = st.entries.iter().find(|e| e.path == path).map_or(0, |e| e.bytes.saturating_sub(had));
        Self::drop_oldest(&mut st, self.budget(), CACHE_FILES, Some(path));
        (outcome, grown)
    }

    /// Drop least recently used entries until the rest take at most `bytes` in at most `files`
    /// entries, never one in use, the pictures' planes held for their effects, or `newest` (just
    /// decoded); entries that hold no bytes (a failure remembered) only when there are too many.
    fn drop_oldest(st: &mut State, bytes: usize, files: usize, newest: Option<&str>) {
        loop {
            if st.entries.iter().map(|e| e.bytes).fold(0usize, usize::saturating_add) <= bytes && st.entries.len() <= files {
                break;
            }
            // (an entry holding no bytes only goes for the count)
            let over_count = st.entries.len() > files;
            let Some(i) = Self::oldest_droppable(st, newest, over_count) else { break };
            st.entries.remove(i);
        }
    }

    /// The least recently used entry that may be dropped: not `newest`, not in use, not a
    /// picture's planes held for its effects, and holding bytes (unless `empty_too`).
    fn oldest_droppable(st: &State, newest: Option<&str>, empty_too: bool) -> Option<usize> {
        let held = &st.held;
        st.entries
            .iter()
            .enumerate()
            .filter(|(_, e)| Some(e.path.as_str()) != newest && !e.in_use() && (e.bytes > 0 || empty_too))
            .filter(|(_, e)| !e.batch.as_ref().is_some_and(|b| held.iter().any(|(_, h)| Arc::ptr_eq(h, b))))
            .min_by_key(|(_, e)| e.used)
            .map(|(i, _)| i)
    }

    /// The entry the memory accountant may drop next ([`PlaneStore::oldest_droppable`]), with
    /// what decoding it took (a typical cost when not timed).
    fn oldest_candidate(&self) -> Option<effectcraft_render::memory::Candidate> {
        let st = self.lock();
        let e = st.entries.get(Self::oldest_droppable(&st, None, false)?)?;
        let cost_us =
            if e.cost_us > 0 { e.cost_us } else { effectcraft_render::memory::default_cost(e.bytes, effectcraft_render::memory::DEFAULT_PLANE_US_PER_MB) };
        Some(effectcraft_render::memory::Candidate { used: e.used, bytes: e.bytes, cost_us })
    }

    /// Drop [`PlaneStore::oldest_candidate`]; the bytes freed.
    fn drop_candidate(&self) -> usize {
        let mut st = self.lock();
        let Some(i) = Self::oldest_droppable(&st, None, false) else { return 0 };
        if i < st.entries.len() { st.entries.remove(i).bytes } else { 0 }
    }

    /// Wait for the outcome of another request's decode (`flight`). A thread running a store
    /// decode doesn't wait (see the module notes); the time limit is a safety net.
    #[cfg(not(target_arch = "wasm32"))]
    fn wait(&self, flight: &Flight) -> Waited {
        if may_not_wait() {
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

    /// The channels kept of `path` and their bytes (tests).
    #[cfg(test)]
    pub(crate) fn kept(&self, path: &str) -> Option<(Vec<String>, usize)> {
        let st = self.lock();
        let e = st.entries.iter().find(|e| e.path == path)?;
        let b = e.batch.as_ref()?;
        Some((b.aux.channels.iter().map(|(n, _)| n.clone()).collect(), e.bytes))
    }

    #[cfg(test)]
    pub(crate) fn cached(&self) -> Vec<String> {
        self.lock().entries.iter().filter(|e| e.batch.is_some() || !e.failed.is_empty()).map(|e| e.path.clone()).collect()
    }

    #[cfg(test)]
    pub(crate) fn is_held(&self, path: &str) -> bool {
        self.lock().held.iter().any(|(p, _)| p == path)
    }
}

/// This thread may not wait for planes: a decode thread (#3270: a decode doesn't depend on the
/// requests waiting for it; an owner on a frame worker queues its decode on these threads, and
/// a decode thread waiting for it could starve that queue), or one running a store decode.
fn may_not_wait() -> bool {
    let on_decode_thread = {
        #[cfg(not(target_arch = "wasm32"))]
        {
            crate::pool::decode_threads().is_some_and(|t| t.current_thread_index().is_some())
        }
        #[cfg(target_arch = "wasm32")]
        {
            false
        }
    };
    on_decode_thread || DECODING.with(std::cell::Cell::get) > 0
}

/// Hands a claimed decode's outcome over when dropped, an error when the decode stopped
/// (panicked): the request that queued it never waits for an outcome that doesn't come.
struct Handing<'a> {
    flight: &'a Flight,
    outcome: Option<Result<Arc<Batch>, Refused>>,
}

impl Drop for Handing<'_> {
    fn drop(&mut self) {
        self.flight.hand(self.outcome.take().unwrap_or(Err(Refused::Stopped)));
    }
}

/// Decode a flight claimed by this thread (see [`Flight::claim`]) here, hand the outcome to the
/// request that queued it, and answer this request with it (or a copy when it doesn't cover it).
fn decode_claimed(path: &str, source: &ExrSource, flight: &Arc<Flight>, index: &Arc<ExrIndex>, want: &Want) -> Result<Arc<Batch>, Refused> {
    #[cfg(test)]
    {
        CLAIMS.lock().unwrap_or_else(PoisonError::into_inner).push(path.to_string());
        CLAIM_SIGNAL.notify_all();
    }
    let mut handing = Handing { flight, outcome: None };
    let outcome = flight.hints.clone().run(|| decode(path, source, flight, flight.base.clone(), None));
    handing.outcome = Some(outcome.clone());
    drop(handing);
    match outcome {
        Ok(b) if b.covers(index, want) => {
            crate::reads::note(b.file.clone());
            Ok(b)
        }
        _ => copy(path, source, index, want),
    }
}

/// Run `flight`'s decode on the footage decode threads (here when this is one of them, or
/// there are none) and block until it is done.
fn run(path: &str, source: &ExrSource, flight: &Arc<Flight>, base: Option<Arc<Batch>>, opened: Option<Opened>) -> Result<Arc<Batch>, Refused> {
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(threads) = crate::pool::decode_threads()
        && threads.current_thread_index().is_none()
    {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let (path, source, queued) = (path.to_string(), source.clone(), flight.clone());
        // The request's cancel flag and decoding hints go with it.
        let hints = filmcraft_media::cancel::Context::current();
        #[cfg(test)]
        run_hook(&QUEUING, &path);
        threads.spawn(move || {
            #[cfg(test)]
            run_hook(&STARTING, &path);
            // (claimed meanwhile by a decode thread: it decodes, and hands the outcome over)
            if queued.take(RUNNING) {
                let outcome = hints.run(|| decode(&path, &source, &queued, base, opened));
                // (let go of the flight before the asker goes on: the flight keeps the outcome
                // for its waiters, and a flight still held here would keep the planes looking in
                // use, so the store would drop another file's instead)
                drop(queued);
                let _ = tx.send(outcome);
                #[cfg(test)]
                run_hook(&SENT, &path);
            }
        });
        // A plain block: this thread runs nothing else meanwhile.
        return match rx.recv() {
            Ok(outcome) => outcome,
            Err(_) if flight.claimed() => flight.take_handed(),
            // (a decode that panicked sent nothing)
            Err(_) => Err(Refused::Stopped),
        };
    }
    // (claimed by a decode thread before this one began: it hands the outcome over)
    if !flight.take(RUNNING) {
        return flight.take_handed();
    }
    decode(path, source, flight, base, opened)
}

/// Decode what `flight` asks for and `base` doesn't hold, and make the union of both.
fn decode(path: &str, source: &ExrSource, flight: &Arc<Flight>, base: Option<Arc<Batch>>, opened: Option<Opened>) -> Result<Arc<Batch>, Refused> {
    let _decoding = Decoding::new();
    // The file as indexing opened it, else opened now (a share that doesn't answer just now
    // isn't remembered as unreadable).
    let opened = match opened {
        Some(o) => Ok(o),
        None => source.open(),
    };
    let mut opened = opened.map_err(|e| {
        log::warn!("media: {path}: {e}");
        Refused::Stopped
    })?;
    let index = flight.state().index.clone();
    if opened.stamp != index.stamp {
        // The file changed since it was indexed (the next index check sees its new identity).
        source.opened_as(opened.stamp);
        let fresh = exr_index::build(&mut *opened.reader, opened.len, opened.stamp).map_err(|_| Refused::Unreadable)?;
        exr_index::keep(path, Arc::new(fresh));
        return Err(Refused::Changed);
    }
    #[cfg(test)]
    {
        let hook = BEFORE_READ.lock().unwrap_or_else(PoisonError::into_inner).iter().find(|(p, _)| p == path).map(|(_, h)| h.clone());
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
    let wanted: Vec<usize> = if want.all {
        index.aux_ids().into_iter().chain(want.ids.iter().copied()).collect::<BTreeSet<usize>>().into_iter().collect()
    } else {
        want.ids.iter().copied().collect()
    };
    let wanted: Vec<usize> = wanted.into_iter().filter(|i| *i < index.channels.len()).collect();
    let base = base.filter(|b| Arc::ptr_eq(&b.index, &index));
    let missing: Vec<usize> = wanted.iter().copied().filter(|id| !base.as_ref().is_some_and(|b| b.ids.binary_search(id).is_ok())).collect();
    let names: Vec<String> = missing.iter().filter_map(|i| index.channels.get(*i)).map(|c| c.name.clone()).collect();
    let (w, h) = index.size();
    // The disk tier first (#3271): what an earlier session or a decode RAM let go of kept, one
    // entry per channel (any request is assembled from them, whatever was asked before).
    // (a file naming two channels alike can't tell their entries apart: not kept)
    let unique = index.channels.iter().map(|c| c.name.as_str()).collect::<std::collections::HashSet<_>>().len() == index.channels.len();
    let disk = disk_cache_for(path).filter(|_| !missing.is_empty() && unique);
    let since = flight.since;
    let key = |name: &str| effectcraft_render::disk_cache::plane_key(path, &opened.stamp, name);
    // (read and decompressed side by side on this pool's threads)
    let mut kept: Vec<Option<Vec<f32>>> = match &disk {
        Some(d) => names
            .par_iter()
            .map(|n| {
                let (name, plane) = d.get_plane(key(n), w as u32, h as u32)?;
                (name == *n && plane.len() == w.saturating_mul(h)).then_some(plane)
            })
            .collect(),
        None => vec![None; names.len()],
    };
    let to_read: Vec<usize> = missing.iter().zip(&kept).filter(|(_, k)| k.is_none()).map(|(id, _)| *id).collect();
    let from_disk = to_read.is_empty();
    let (read, stats) = if to_read.is_empty() {
        (Ok(vec![]), Default::default())
    } else {
        let request = ReadRequest { max_bytes: cap_of(path), ..ReadRequest::full(to_read.clone()) };
        read_planes(&mut *opened.reader, &index, &request)
    };
    #[cfg(test)]
    DECODES.lock().unwrap_or_else(PoisonError::into_inner).push((path.to_string(), DecodeRecord { channels: names.clone(), stats: stats.clone(), from_disk }));
    #[cfg(not(test))]
    let _ = (&stats, from_disk);
    let read: Vec<Vec<f32>> = read.map_err(|e| match e {
        ReadError::Cancelled => Refused::Cancelled,
        // (a read that failed just now: the next request reads again)
        ReadError::Io(why) => {
            log::warn!("media: {path}: {why}");
            Refused::Stopped
        }
        ReadError::TooLarge(why) => {
            log::warn!("media: {path}: {why}");
            Refused::TooLarge(why)
        }
        ReadError::Unreadable(why) => {
            log::warn!("media: {path}: {why}");
            Refused::Unreadable
        }
    })?;
    let mut read = read.into_iter();
    let planes: Vec<Vec<f32>> = kept.iter_mut().map(|k| k.take().or_else(|| read.next()).unwrap_or_default()).collect();
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
    let all = index.aux_ids().iter().all(|id| new.binary_search_by_key(id, |(i, _)| *i).is_ok());
    let mut aux = AuxChannels::new(w as u32, h as u32, 1.0);
    aux.manifests = index.names.manifests.clone();
    aux.aliases = index.names.aliases.clone();
    if new.len() != index.channels.len() {
        aux.listed = index.channels.iter().map(|c| c.name.clone()).collect();
    }
    let ids: Vec<usize> = new.iter().map(|(id, _)| *id).collect();
    aux.channels = new.into_iter().filter_map(|(id, p)| Some((index.channels.get(id)?.name.clone(), p))).collect();
    let file = ReadFile::new(path, opened.stamp);
    let batch = Arc::new(Batch { index, ids, all, aux: Arc::new(aux), file });
    // Kept on disk for the next time, each channel read from the file in an entry of its own,
    // encoded on the disk cache's writer thread (a file from memory, which can't be checked
    // later, isn't).
    if let Some(d) = &disk {
        let mut files = effectcraft_render::reads::ReadFiles::default();
        files.add(batch.file.clone());
        if let Some(entry) = effectcraft_render::disk_cache::EntryFiles::of(&files, None) {
            // (copies, each owned by its write: a queued write never keeps the store's set)
            let planes: Vec<(u128, String, Vec<f32>)> = to_read
                .par_iter()
                .filter_map(|id| {
                    let (name, plane) = batch.aux.channels.get(batch.ids.binary_search(id).ok()?)?;
                    Some((key(name), name.clone(), plane.clone()))
                })
                .collect();
            d.put_planes(w as u32, h as u32, planes, &entry, since);
        }
    }
    Ok(batch)
}

/// The disk cache decoded planes are kept in ([`set_disk_cache`]).
static DISK: Mutex<Option<std::sync::Weak<effectcraft_render::disk_cache::DiskCache>>> = Mutex::new(None);

/// Keep decoded planes in `cache` too (`None`: no disk tier), #3271. Held weakly: switching the
/// disk cache off drops it.
pub(crate) fn set_disk_cache(cache: Option<&Arc<effectcraft_render::disk_cache::DiskCache>>) {
    *DISK.lock().unwrap_or_else(PoisonError::into_inner) = cache.map(Arc::downgrade);
}

fn disk_cache() -> Option<Arc<effectcraft_render::disk_cache::DiskCache>> {
    DISK.lock().unwrap_or_else(PoisonError::into_inner).as_ref().and_then(std::sync::Weak::upgrade)
}

/// The disk cache planes of `path` are kept in (tests: only for the paths of the test that set
/// it, [`use_disk_for`], so tests running beside it don't share it).
fn disk_cache_for(path: &str) -> Option<Arc<effectcraft_render::disk_cache::DiskCache>> {
    #[cfg(test)]
    if !DISK_PATHS.lock().unwrap_or_else(PoisonError::into_inner).iter().any(|p| path.starts_with(p.as_str())) {
        return None;
    }
    let _ = path;
    disk_cache()
}

#[cfg(test)]
static DISK_PATHS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Keep the planes of paths under `prefix` in the disk cache (tests).
#[cfg(test)]
pub(crate) fn use_disk_for(prefix: &str) {
    DISK_PATHS.lock().unwrap_or_else(PoisonError::into_inner).push(prefix.to_string());
}

/// The store all footage shares.
static STORE: PlaneStore = PlaneStore::new(CACHE_BUDGET);

fn store() -> &'static PlaneStore {
    // In the common memory budget (#3271), registered once.
    static HOLDER: std::sync::OnceLock<Arc<dyn effectcraft_render::memory::MemoryHolder>> = std::sync::OnceLock::new();
    HOLDER.get_or_init(|| {
        let h: Arc<dyn effectcraft_render::memory::MemoryHolder> = Arc::new(SharedStore);
        effectcraft_render::memory::Accountant::global().register(&h);
        h
    });
    &STORE
}

/// The shared plane store in the common memory budget (#3271, #3298): decoded planes the effects
/// read, each file's worth what decoding it took, per byte.
struct SharedStore;

impl effectcraft_render::memory::MemoryHolder for SharedStore {
    fn name(&self) -> &'static str {
        "footage planes"
    }
    fn bytes(&self) -> usize {
        STORE.lock().entries.iter().map(|e| e.bytes).fold(0, usize::saturating_add)
    }
    fn oldest(&self) -> Option<effectcraft_render::memory::Candidate> {
        STORE.oldest_candidate()
    }
    fn drop_oldest(&self) -> usize {
        STORE.drop_candidate()
    }
}

/// The most the shared plane store keeps by itself, the footage frames' cap (#3298: the whole
/// common budget on the desktop; in the browser at most [`crate::exr_channels::WEB_CACHE_CAP`]).
/// The memory accountant keeps the planes within the common budget with the other caches.
pub(crate) fn set_budget(bytes: usize) {
    let bytes = if cfg!(target_arch = "wasm32") { bytes.min(crate::exr_channels::WEB_CACHE_CAP) } else { bytes };
    store().set_budget(bytes);
}

/// The bytes the shared plane store keeps at most.
#[cfg(test)]
pub(crate) fn budget() -> usize {
    STORE.budget()
}

/// Is `path` an OpenEXR file by name (the store reads those; other files, and files named so
/// that aren't OpenEXR, go the `image` crate's way).
pub(crate) fn is_exr(path: &str) -> bool {
    path.to_ascii_lowercase().ends_with(".exr")
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
    let mut extra = extra;
    for _ in 0..3 {
        // (before the file is opened: see `Flight::since`)
        let since = drop_generation();
        let (index, opened) = match exr_index::get_opened(path, source) {
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
        let batch = match store().get_opened(path, source, &index, &want, by, opened, since) {
            Ok(b) => b,
            Err(Refused::Changed) => continue,
            // The channels with it are too many: the picture alone (the effects ask for theirs).
            Err(Refused::TooLarge(_)) if extra.is_some() => {
                extra = None;
                continue;
            }
            Err(Refused::TooLarge(why)) => return Picture::Done(Err(why)),
            Err(Refused::Cancelled | Refused::Forgotten) => return Picture::Cancelled,
            // (the whole file's decode says why)
            Err(Refused::Stopped | Refused::Unreadable) => return Picture::Legacy,
        };
        let img = picture_of(&index, &batch, op);
        // The picture is kept by the pool once converted, not twice: its planes go, but for those
        // an effect names (EXtractoR reading "R").
        if img.is_some() {
            let named = extra.and_then(|n| need_ids(&index, n)).unwrap_or_default();
            let picture: BTreeSet<usize> = index.picture_ids().unwrap_or_default().into_iter().filter(|id| !named.contains(id)).collect();
            let keep: BTreeSet<usize> = batch.ids.iter().copied().filter(|id| !picture.contains(id)).collect();
            store().keep_only(path, batch, &keep);
        }
        return match img {
            Some(img) => Picture::Done(Ok(img)),
            None => Picture::Legacy,
        };
    }
    Picture::Legacy
}

/// The picture of the file `index` describes, made of the planes of `batch`.
fn picture_of(index: &ExrIndex, batch: &Batch, op: AlphaOp) -> Option<Image> {
    let aux = &batch.aux;
    match index.rgb {
        Rgb::Planes([r, g, b], a) => {
            let a = match a {
                Some(a) => Some(batch.plane(a)?),
                None => None,
            };
            planes_picture(aux, [batch.plane(r)?, batch.plane(g)?, batch.plane(b)?], a, op)
        }
        Rgb::None => {
            let (_, rgb, a) = index.picture.as_ref()?;
            let a = match a {
                Some(n) => Some(aux.get(n)?),
                None => None,
            };
            planes_picture(aux, [aux.get(&rgb[0])?, aux.get(&rgb[1])?, aux.get(&rgb[2])?], a, op)
        }
        Rgb::Unsupported => None,
    }
}

/// The picture of the OpenEXR at `path` (as [`picture`]) of only the display rows `rows`: those
/// rows are read (with 1-line chunks exactly those lines, else the chunks holding them) and
/// converted, the others stay empty (#3271: a region of interest). Read for this request alone,
/// not kept in the store or on disk (the pool keeps the picture under a key naming the rows).
pub(crate) fn picture_rows(path: &str, source: &ExrSource, op: AlphaOp, rows: (i64, i64)) -> Picture {
    let (index, opened) = match exr_index::get_opened(path, source) {
        Ok(ix) => ix,
        Err(IndexError::Unsupported(why)) => return Picture::Done(Err(why.to_string())),
        Err(_) => return Picture::Legacy,
    };
    let Some(mut ids) = index.picture_ids() else { return Picture::Legacy };
    // (in file order: the planes are looked up by id)
    ids.sort_unstable();
    ids.dedup();
    let opened = match opened {
        Some(o) => Ok(o),
        None => source.open(),
    };
    let Ok(mut opened) = opened else { return Picture::Legacy };
    // (a file changed since it was indexed: the whole picture, as `picture` reads it)
    if opened.stamp != index.stamp {
        return picture(path, source, op, None);
    }
    let request = ReadRequest { rows: Some(rows.0..rows.1), max_bytes: cap_of(path), ..ReadRequest::full(ids.clone()) };
    let (planes, stats) = read_planes(&mut *opened.reader, &index, &request);
    let names: Vec<String> = ids.iter().filter_map(|i| index.channels.get(*i)).map(|c| c.name.clone()).collect();
    #[cfg(test)]
    DECODES.lock().unwrap_or_else(PoisonError::into_inner).push((path.to_string(), DecodeRecord { channels: names.clone(), stats, from_disk: false }));
    #[cfg(not(test))]
    let _ = stats;
    let planes = match planes {
        Ok(p) => p,
        Err(ReadError::Cancelled) => return Picture::Cancelled,
        Err(ReadError::TooLarge(why)) => return Picture::Done(Err(why)),
        Err(_) => return Picture::Legacy,
    };
    let (w, h) = index.size();
    let mut aux = AuxChannels::new(w as u32, h as u32, 1.0);
    aux.channels = names.into_iter().zip(planes).collect();
    let file = ReadFile::new(path, opened.stamp);
    crate::reads::note(file.clone());
    let batch = Batch { index: index.clone(), ids, all: false, aux: Arc::new(aux), file };
    match picture_of(&index, &batch, op) {
        Some(img) => Picture::Done(Ok(img)),
        None => Picture::Legacy,
    }
}

/// The channels `need` reads of the OpenEXR at `path` (all for [`AuxNeed::All`]), from the
/// store. `None` when it isn't a readable OpenEXR (or the request was called off).
pub(crate) fn aux(path: &str, source: &ExrSource, need: &AuxNeed) -> Option<Arc<AuxChannels>> {
    aux_of(path, source, need, false)
}

/// Every channel of the OpenEXR at `path`, the picture's too ([`FootageSource::aux`]: no render
/// asks for that; a render asks for what its effects read, [`aux`]).
///
/// [`FootageSource::aux`]: effectcraft_render::FootageSource::aux
pub(crate) fn every_channel(path: &str, source: &ExrSource) -> Option<Arc<AuxChannels>> {
    aux_of(path, source, &AuxNeed::All, true)
}

fn aux_of(path: &str, source: &ExrSource, need: &AuxNeed, with_picture: bool) -> Option<Arc<AuxChannels>> {
    for _ in 0..2 {
        let since = drop_generation();
        let (index, opened) = exr_index::get_opened(path, source).ok()?;
        let mut want = match need_ids(&index, need) {
            Some(ids) => Want { all: false, ids },
            None => Want { all: true, ids: BTreeSet::new() },
        };
        if with_picture {
            want.ids.extend(index.picture_ids().unwrap_or_default());
        }
        match store().get_opened(path, source, &index, &want, Use::Other, opened, since) {
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
    exr_index::get(path, source).ok().map(|ix| ix.names.clone())
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
