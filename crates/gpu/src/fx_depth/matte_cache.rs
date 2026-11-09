//! Finished Cryptomatte planes (#3334), kept so a frame shown again neither reads the rank
//! channels again nor makes the matte again: per footage frame (the decoded channels the plane
//! was made of), Cryptomatte layer, selection, output and render scale.
//!
//! The frame is named by its channels' identity: the footage source hands out the same
//! [`AuxChannels`] while it keeps that frame's channels, and new ones once the frame changes or
//! the file is forgotten (Reload Footage), so a stale plane can't be found. The cache holds them
//! only weakly (it never keeps decoded channels alive); a plane whose channels are gone is the
//! first to go.
//!
//! The planes are in the common memory budget ([`effectcraft_render::memory`]), each worth what
//! making it took, and bounded by themselves as well (a render without a budget keeps no more
//! than [`MAX_ENTRIES`] planes and [`MAX_BYTES`]).

use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, Weak};

use effectcraft_effects::{CryptoOutput, CryptoPlane};
use effectcraft_raster::AuxChannels;
use effectcraft_render::memory::{self, Accountant, Candidate, MemoryHolder};

/// At most this many planes…
const MAX_ENTRIES: usize = 32;
/// …and bytes, whatever the common budget.
const MAX_BYTES: usize = 512 << 20;

#[derive(Clone, Debug, PartialEq)]
struct Key {
    layer: String,
    sel: Vec<u32>,
    output: CryptoOutput,
    /// The render scale the plane is box-filtered to (its bits), `None` at source resolution.
    scale: Option<u64>,
}

struct Entry {
    /// The channels it was made of (held weakly: the allocation stays while this lives, so its
    /// address names these channels alone).
    aux: Weak<AuxChannels>,
    key: Key,
    plane: Arc<CryptoPlane>,
    /// Last use ([`memory::tick`]) and what making it took (µs).
    used: u64,
    cost_us: u64,
}

impl Entry {
    fn of(&self, a: &Arc<AuxChannels>) -> bool {
        std::ptr::eq(self.aux.as_ptr(), Arc::as_ptr(a))
    }
    fn gone(&self) -> bool {
        self.aux.strong_count() == 0
    }
}

struct Cache {
    entries: Vec<Entry>,
    /// Planes made so far (tests).
    made: u64,
}

static CACHE: Mutex<Cache> = Mutex::new(Cache { entries: Vec::new(), made: 0 });

fn lock() -> MutexGuard<'static, Cache> {
    CACHE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// In the common memory budget, registered once.
fn register() {
    static HOLDER: OnceLock<Arc<dyn MemoryHolder>> = OnceLock::new();
    HOLDER.get_or_init(|| {
        let h: Arc<dyn MemoryHolder> = Arc::new(Holder);
        Accountant::global().register(&h);
        h
    });
}

/// Cryptomatte layer `layer`'s finished plane for selection `sel` (hashes) at `render_scale`
/// ([`effectcraft_effects::crypto_plane`]), kept or made now.
pub(crate) fn plane(a: &Arc<AuxChannels>, layer: &str, sel: &[u32], output: CryptoOutput, render_scale: f64) -> Arc<CryptoPlane> {
    register();
    let key = Key { layer: layer.to_string(), sel: sel.to_vec(), output, scale: effectcraft_effects::crypto_box_scale(render_scale).map(f64::to_bits) };
    {
        let mut c = lock();
        let now = memory::tick();
        if let Some(e) = c.entries.iter_mut().find(|e| e.of(a) && e.key == key) {
            e.used = now;
            return e.plane.clone();
        }
    }
    let started = web_time::Instant::now();
    let made = Arc::new(effectcraft_effects::crypto_plane(a, layer, sel, output, render_scale));
    let cost_us = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX).max(1);
    let bytes = made.bytes();
    {
        let mut c = lock();
        c.made += 1;
        // (planes of channels that are gone can't be asked for again; another thread may have
        // made this one meanwhile)
        c.entries.retain(|e| !e.gone() && !(e.of(a) && e.key == key));
        c.entries.push(Entry { aux: Arc::downgrade(a), key, plane: made.clone(), used: memory::tick(), cost_us });
        loop {
            let total: usize = c.entries.iter().map(|e| e.plane.bytes()).fold(0, usize::saturating_add);
            if c.entries.len() <= 1 || (c.entries.len() <= MAX_ENTRIES && total <= MAX_BYTES) {
                break;
            }
            let Some(i) = oldest(&c) else { break };
            c.entries.swap_remove(i);
        }
    }
    Accountant::global().note_growth(bytes);
    made
}

/// The entry to drop next: one whose channels are gone, else the least recently used of those
/// no render holds now (dropping one in use would free nothing yet).
fn oldest(c: &Cache) -> Option<usize> {
    c.entries
        .iter()
        .position(Entry::gone)
        .or_else(|| c.entries.iter().enumerate().filter(|(_, e)| Arc::strong_count(&e.plane) == 1).min_by_key(|(_, e)| e.used).map(|(i, _)| i))
}

/// The planes in the common memory budget.
struct Holder;

impl MemoryHolder for Holder {
    fn name(&self) -> &'static str {
        "Cryptomatte mattes"
    }
    fn bytes(&self) -> usize {
        lock().entries.iter().map(|e| e.plane.bytes()).fold(0, usize::saturating_add)
    }
    fn oldest(&self) -> Option<Candidate> {
        let c = lock();
        let e = c.entries.get(oldest(&c)?)?;
        // (a plane of channels that are gone is worth nothing)
        Some(if e.gone() {
            Candidate { used: 0, bytes: e.plane.bytes(), cost_us: 0 }
        } else {
            Candidate { used: e.used, bytes: e.plane.bytes(), cost_us: e.cost_us }
        })
    }
    fn drop_oldest(&self) -> usize {
        let mut c = lock();
        let Some(i) = oldest(&c) else { return 0 };
        c.entries.swap_remove(i).plane.bytes()
    }
}

/// Planes made so far (tests).
#[cfg(test)]
pub(crate) fn made() -> u64 {
    lock().made
}

/// Planes kept now (tests).
#[cfg(test)]
pub(crate) fn kept() -> usize {
    lock().entries.len()
}

/// The planes in the common memory budget (tests).
#[cfg(test)]
pub(crate) fn holder() -> Arc<dyn MemoryHolder> {
    Arc::new(Holder)
}
