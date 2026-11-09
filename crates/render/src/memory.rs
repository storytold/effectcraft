//! One memory accountant for the big caches (#3271, #3298): the layer cache, the media pool's
//! frames, the decoded footage planes, the preview frames and the disk cache's write queue
//! register here as [`MemoryHolder`]s and share one budget ([`Accountant::set_budget`], from
//! Settings ▸ Memory & CPU ▸ RAM Reserved for Other Applications). No holder has a share of its
//! own: when they hold more than the budget together, the accountant frees the least valuable
//! item first, whichever holder keeps it, in large steps: down to [`TRIM_TO`] of the budget, so
//! a cache doesn't fill up and trim again with every insert.
//!
//! An item's value is what it would cost to make again per byte ([`Candidate::cost_us`], the
//! measured render or decode time where known, else a typical one: [`DEFAULT_LAYER_US_PER_MB`]
//! and its neighbours), decayed slowly by the time since it was last used (by the logarithm of
//! the seconds unused, so a costly frame unused for minutes still outlives a cheap one):
//! [`value`]. A copy of data another cache holds (a queued disk write) costs nothing to make
//! again. Each holder offers the item it would drop next (its least recently used one,
//! [`MemoryHolder::oldest`]); the accountant drops the least valuable of those, asks that holder
//! for its next one, and repeats.
//!
//! A holder that had to make again what it let go of shortly before (its "refills",
//! [`MemoryHolder::refills`]) is thrashing: its items are worth [`THRASH_BOOST`] times more
//! until it settles, so the data that is needed again is kept longest.
//!
//! Holders are held weakly: a cache that goes away is forgotten. The accountant never calls a
//! holder while it holds its own lock, and holders call [`Accountant::note_growth`] after
//! releasing theirs.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, TryLockError, Weak};

/// The share of the budget the accountant frees down to once it is over (hysteresis).
pub const TRIM_TO: f64 = 0.9;

/// While a holder thrashes, its items are worth this many times more ([`value`]).
pub const THRASH_BOOST: f64 = 4.0;

/// What it typically costs to make again a megabyte of each holder's data when its own time was
/// not measured (microseconds per MB): a processed layer buffer…
pub const DEFAULT_LAYER_US_PER_MB: u64 = 4_000;
/// …a RAM preview frame (the whole comp rendered)…
pub const DEFAULT_PREVIEW_US_PER_MB: u64 = 8_000;
/// …a decoded footage frame…
pub const DEFAULT_FRAME_US_PER_MB: u64 = 2_000;
/// …and decoded footage planes (OpenEXR channels).
pub const DEFAULT_PLANE_US_PER_MB: u64 = 2_000;

/// The typical cost (µs) of making again `bytes` bytes at `us_per_mb`.
pub fn default_cost(bytes: usize, us_per_mb: u64) -> u64 {
    let mb = u64::try_from(bytes).unwrap_or(u64::MAX) >> 20;
    mb.max(1).saturating_mul(us_per_mb)
}

/// The use stamp holders give their items: microseconds since the process's first stamp, and
/// later uses get larger stamps (two uses never tie: holders may key their order by it).
pub fn tick() -> u64 {
    static START: std::sync::OnceLock<web_time::Instant> = std::sync::OnceLock::new();
    static LAST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let start = START.get_or_init(web_time::Instant::now);
    let now = u64::try_from(start.elapsed().as_micros()).unwrap_or(u64::MAX);
    let mut last = LAST.load(Ordering::Relaxed);
    loop {
        let next = now.max(last.saturating_add(1));
        match LAST.compare_exchange_weak(last, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return next,
            Err(seen) => last = seen,
        }
    }
}

/// Inserts balance at most this often ([`Accountant::note_growth`])…
const BALANCE_EVERY: std::time::Duration = std::time::Duration::from_millis(100);

/// …unless the holders grew by more than this part of the budget since the last balance
/// (several large decodes landing within one interval could otherwise overshoot by gigabytes).
const GROWTH_PART: usize = 16;

/// How long a holder that refilled stays protected.
const THRASH_FOR: std::time::Duration = std::time::Duration::from_secs(10);

/// The item a holder would drop next ([`MemoryHolder::oldest`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Candidate {
    /// When it was last used ([`tick`]).
    pub used: u64,
    /// The bytes dropping it frees.
    pub bytes: usize,
    /// What making it again costs, in microseconds (0: a copy of data another cache holds).
    pub cost_us: u64,
    /// About to be used: a frame a playing RAM preview shows next. Kept over the budget by up to
    /// [`preview_overage`] while other items can go instead, never past the hard limit.
    pub protected: bool,
}

impl Candidate {
    /// An item that is not [`Candidate::protected`].
    pub fn new(used: u64, bytes: usize, cost_us: u64) -> Candidate {
        Candidate { used, bytes, cost_us, protected: false }
    }
}

/// How far protected items (frames a playing preview shows next) may keep the caches over a
/// budget of `budget` bytes while other items can go instead: a tenth of it, at most 1 GB.
pub fn preview_overage(budget: usize) -> usize {
    (budget / 10).min(1 << 30)
}

/// How often the process's memory is read at most ([`Accountant::set_process_memory`]), in
/// [`tick`]'s units (microseconds).
const PROCESS_READ_EVERY: u64 = 250_000;

/// The process's memory as the accountant watches it ([`Accountant::set_process_memory`]).
struct ProcessWatch {
    /// Reads the memory the process uses now, in bytes (`None`: unknown).
    read: Option<fn() -> Option<u64>>,
    /// Above this the caches are trimmed by what is over, even when they are within their
    /// budget (memory nobody accounts for: GPU staging, effect buffers, the allocator's
    /// fragmentation)…
    purge_at: u64,
    /// …and above this also the protected items go ([`Candidate::protected`]).
    hard_at: u64,
    /// When it was last read ([`tick`]'s units).
    read_at: Option<u64>,
    /// What the caches freed for it since it was last under its purge limit: freed memory often
    /// stays in the reading (the allocator keeps it), so it counts as freed; the caches give way
    /// again only for memory that grew beyond that, and are never emptied for one reading.
    credited: u64,
}

/// What an item is worth keeping at `now` ([`tick`]): its cost to make again per byte, divided
/// by `ln(seconds unused + 1) + 1` (a slow decay: about a fifth after a minute unused, a
/// seventh after ten), [`THRASH_BOOST`] times more while its holder thrashes.
pub fn value(c: &Candidate, now: u64, thrashing: bool) -> f64 {
    let per_byte = c.cost_us as f64 / c.bytes.max(1) as f64;
    let age_s = now.saturating_sub(c.used) as f64 / 1e6;
    let v = per_byte / ((age_s + 1.0).ln() + 1.0);
    if thrashing { v * THRASH_BOOST } else { v }
}

/// A cache the accountant keeps within the common budget.
pub trait MemoryHolder: Send + Sync {
    /// A name for logs and stats.
    fn name(&self) -> &'static str;
    /// The bytes it holds now.
    fn bytes(&self) -> usize;
    /// The item it would drop next, its least recently used one it may drop (`None`: nothing it
    /// may drop, such as data in use).
    fn oldest(&self) -> Option<Candidate>;
    /// Drop that item; the bytes freed (0: nothing dropped).
    fn drop_oldest(&self) -> usize;
    /// How many times so far it made again what it had let go of shortly before (0 when it
    /// doesn't tell).
    fn refills(&self) -> u64 {
        0
    }
}

struct Registered {
    holder: Weak<dyn MemoryHolder>,
    /// Its refills at the last balance, and until when it counts as thrashing (it refilled).
    refills: u64,
    thrash_until: Option<web_time::Instant>,
}

/// The common budget of the big caches (see the module notes).
pub struct Accountant {
    budget: AtomicUsize,
    holders: Mutex<Vec<Registered>>,
    /// Bytes freed by the accountant so far (stats, tests).
    freed: AtomicUsize,
    /// Bytes the holders said they grew by since the last balance ([`Accountant::note_growth`]).
    grown: AtomicUsize,
    /// One balance at a time (another one asked for meanwhile is run by the one running, see
    /// `requested`), and when the last one ran.
    balancing: Mutex<Option<web_time::Instant>>,
    /// A balance was asked for since the running one read the budget: it runs again (a budget
    /// lowered meanwhile is not lost, #3301 review).
    requested: AtomicBool,
    /// The clock items' ages are measured with ([`tick`]; tests give their own).
    clock: fn() -> u64,
    /// The process's memory, read and checked by each balance.
    process: Mutex<ProcessWatch>,
}

impl Default for Accountant {
    fn default() -> Self {
        Accountant::new(usize::MAX)
    }
}

impl Accountant {
    /// An accountant with `budget` bytes (`usize::MAX`: no limit).
    pub const fn new(budget: usize) -> Accountant {
        Accountant::with_clock(budget, tick)
    }

    /// An accountant with `budget` bytes measuring ages with `clock` (in [`tick`]'s units).
    pub const fn with_clock(budget: usize, clock: fn() -> u64) -> Accountant {
        Accountant {
            budget: AtomicUsize::new(budget),
            holders: Mutex::new(Vec::new()),
            freed: AtomicUsize::new(0),
            grown: AtomicUsize::new(0),
            balancing: Mutex::new(None),
            requested: AtomicBool::new(false),
            clock,
            process: Mutex::new(ProcessWatch { read: None, purge_at: u64::MAX, hard_at: u64::MAX, read_at: None, credited: 0 }),
        }
    }

    /// Watch the process's memory with `read` (the bytes it uses now): a balance (at most every
    /// 250 ms) trims the caches by what it is over `purge_at`, even while they are within their
    /// budget, and above `hard_at` also the frames a playing preview shows next.
    pub fn set_process_memory(&self, read: fn() -> Option<u64>, purge_at: u64, hard_at: u64) {
        {
            let mut p = self.process.lock().unwrap_or_else(PoisonError::into_inner);
            p.read = Some(read);
            p.purge_at = purge_at;
            p.hard_at = hard_at.max(purge_at);
        }
        self.balance();
    }

    /// What a balance at `now` must free beyond the budget for the process's memory, and whether
    /// it is past the hard limit (a reading at most every [`PROCESS_READ_EVERY`]). What the caches
    /// already freed for it counts as freed ([`ProcessWatch::credited`]).
    fn process_excess(&self, now: u64) -> (u64, bool) {
        let mut p = self.process.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(read) = p.read else { return (0, false) };
        if p.read_at.is_some_and(|at| now.saturating_sub(at) < PROCESS_READ_EVERY) {
            return (0, false);
        }
        p.read_at = Some(now);
        let Some(used) = read() else { return (0, false) };
        if used <= p.purge_at {
            p.credited = 0;
            return (0, false);
        }
        let used = used.saturating_sub(p.credited);
        // (down to the trim line under the purge limit, as for the budget)
        let line = (p.purge_at as f64 * TRIM_TO) as u64;
        (used.saturating_sub(line), used > p.hard_at)
    }

    /// The caches freed `bytes` for the process's memory.
    fn credit(&self, bytes: usize) {
        let mut p = self.process.lock().unwrap_or_else(PoisonError::into_inner);
        p.credited = p.credited.saturating_add(u64::try_from(bytes).unwrap_or(u64::MAX));
    }

    /// The process's accountant.
    pub fn global() -> &'static Accountant {
        static GLOBAL: Accountant = Accountant::new(usize::MAX);
        &GLOBAL
    }

    /// Set the common budget and free down to it: right away when no balance is running; when
    /// one is (on another thread, or this one further up the stack), that balance runs again
    /// with the new budget once its pass ends, so this may return before the caches are within
    /// it ([`Accountant::balance`]).
    pub fn set_budget(&self, bytes: usize) {
        self.budget.store(bytes, Ordering::Relaxed);
        self.balance();
    }

    pub fn budget(&self) -> usize {
        self.budget.load(Ordering::Relaxed)
    }

    /// Keep `holder` within the common budget (held weakly).
    pub fn register(&self, holder: &Arc<dyn MemoryHolder>) {
        let mut h = self.holders.lock().unwrap_or_else(PoisonError::into_inner);
        h.retain(|r| r.holder.strong_count() > 0);
        let weak = Arc::downgrade(holder);
        // (registered again with each new budget: once is enough)
        if h.iter().any(|r| r.holder.ptr_eq(&weak)) {
            return;
        }
        h.push(Registered { holder: weak, refills: holder.refills(), thrash_until: None });
    }

    /// A holder grew by `bytes` (an insert): balance, at most every [`BALANCE_EVERY`] (the
    /// holders' locks aren't taken on every insert), or at once when the holders grew by more
    /// than a [`GROWTH_PART`]th of the budget since the last balance.
    pub fn note_growth(&self, bytes: usize) {
        let budget = self.budget();
        if budget == usize::MAX {
            return;
        }
        let grown = self.grown.fetch_add(bytes, Ordering::Relaxed).saturating_add(bytes);
        let burst = grown > budget / GROWTH_PART;
        let ran_lately = |last: &Option<web_time::Instant>| last.is_some_and(|t| t.elapsed() < BALANCE_EVERY);
        // (a balance running now counts as recent; a poisoned lock still holds the time)
        let recent = match self.balancing.try_lock() {
            Ok(g) => ran_lately(&g),
            Err(TryLockError::Poisoned(p)) => ran_lately(&p.into_inner()),
            Err(TryLockError::WouldBlock) => true,
        };
        // (a balance asked for while this read held the lock found it busy: serve it here)
        if burst || !recent || self.requested.load(Ordering::SeqCst) {
            self.balance();
        }
    }

    /// The holders alive now, and whether each is thrashing.
    fn alive(&self) -> Vec<(Arc<dyn MemoryHolder>, bool)> {
        let mut h = self.holders.lock().unwrap_or_else(PoisonError::into_inner);
        h.retain(|r| r.holder.strong_count() > 0);
        h.iter_mut()
            .filter_map(|r| {
                let holder = r.holder.upgrade()?;
                let refills = holder.refills();
                let now = web_time::Instant::now();
                if refills > r.refills {
                    r.thrash_until = Some(now + THRASH_FOR);
                }
                r.refills = refills;
                Some((holder, r.thrash_until.is_some_and(|t| t > now)))
            })
            .collect()
    }

    /// The bytes all holders hold now.
    pub fn total(&self) -> usize {
        let h = self.holders.lock().unwrap_or_else(PoisonError::into_inner);
        let alive: Vec<Arc<dyn MemoryHolder>> = h.iter().filter_map(|r| r.holder.upgrade()).collect();
        drop(h);
        alive.iter().map(|h| h.bytes()).fold(0, usize::saturating_add)
    }

    /// Bytes freed by the accountant so far.
    pub fn freed(&self) -> usize {
        self.freed.load(Ordering::Relaxed)
    }

    /// When the holders together are over the budget, free down to [`TRIM_TO`] of it, the least
    /// valuable item first whichever holder keeps it ([`value`]; equal values: the least
    /// recently used first, then the holder registered first).
    ///
    /// One balance runs at a time. One asked for while another runs (a lowered budget, more
    /// growth) is run by that one when it is done, with the budget then in force (or by
    /// [`Accountant::note_growth`], when that held the lock to read the time); a poisoned lock
    /// (a holder panicked in a balance) doesn't stop balancing.
    pub fn balance(&self) {
        self.requested.store(true, Ordering::SeqCst);
        loop {
            let mut last: MutexGuard<'_, Option<web_time::Instant>> = match self.balancing.try_lock() {
                Ok(g) => g,
                Err(TryLockError::Poisoned(p)) => p.into_inner(),
                // (the running balance sees `requested` and runs again)
                Err(TryLockError::WouldBlock) => return,
            };
            while self.requested.swap(false, Ordering::SeqCst) {
                *last = Some(web_time::Instant::now());
                self.balance_once();
            }
            drop(last);
            // (asked for between the last pass and the unlock: run it, unless another took over)
            if !self.requested.load(Ordering::SeqCst) {
                return;
            }
        }
    }

    /// One pass of [`Accountant::balance`] with the budget in force now.
    ///
    /// Over the budget the caches are freed down to [`TRIM_TO`] of it; with the process's memory
    /// over its purge limit ([`Accountant::set_process_memory`]) also by what that is over.
    /// Protected items ([`Candidate::protected`]) go last, and only while the caches are more than
    /// [`preview_overage`] over the target (or the process is past its hard limit).
    fn balance_once(&self) {
        let budget = self.budget();
        if budget == usize::MAX {
            return;
        }
        self.grown.store(0, Ordering::Relaxed);
        let now = (self.clock)();
        let (excess, hard) = self.process_excess(now);
        let holders = self.alive();
        let mut total: usize = holders.iter().map(|(h, _)| h.bytes()).fold(0, usize::saturating_add);
        let excess = usize::try_from(excess).unwrap_or(usize::MAX);
        if total <= budget && excess == 0 {
            return;
        }
        let held = total;
        let trimmed = if total > budget { (budget as f64 * TRIM_TO) as usize } else { total };
        // (the caches can free at most what they hold)
        let target = trimmed.min(total.saturating_sub(excess.min(total)));
        let overage = if hard { 0 } else { preview_overage(budget) };
        // Each holder's next item (`None`: it has nothing more it may drop).
        let mut next: Vec<Option<Candidate>> = holders.iter().map(|(h, _)| h.oldest()).collect();
        while total > target {
            // (protected items only once nothing else can go, or past the overage)
            let any_unprotected = next.iter().flatten().any(|c| !c.protected);
            let protected_ok = !any_unprotected && total > target.saturating_add(overage);
            let pick = next
                .iter()
                .enumerate()
                .filter_map(|(i, c)| {
                    let c = (*c)?;
                    if c.protected && !protected_ok {
                        return None;
                    }
                    let thrashing = holders.get(i).is_some_and(|(_, t)| *t);
                    Some((i, value(&c, now, thrashing), c.used))
                })
                .min_by(|a, b| a.1.total_cmp(&b.1).then(a.2.cmp(&b.2)).then(a.0.cmp(&b.0)));
            let Some((i, _, _)) = pick else { break };
            let Some((h, _)) = holders.get(i) else { break };
            let freed = h.drop_oldest();
            if let Some(slot) = next.get_mut(i) {
                *slot = if freed == 0 { None } else { h.oldest() };
            }
            total = total.saturating_sub(freed);
            self.freed.fetch_add(freed, Ordering::Relaxed);
        }
        if excess > 0 {
            // (what went beyond the budget's own trim was freed for the process's memory)
            let for_budget = held.saturating_sub(trimmed.max(target));
            self.credit(held.saturating_sub(total).saturating_sub(for_budget).min(excess));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A holder of items `(used, bytes, cost_us)`, least recently used first.
    struct Fake {
        name: &'static str,
        items: Mutex<std::collections::VecDeque<Candidate>>,
        refills: Mutex<u64>,
    }

    impl Fake {
        fn new(name: &'static str, items: &[(u64, usize, u64)]) -> Arc<Fake> {
            let items = items.iter().map(|&(used, bytes, cost_us)| Candidate::new(used, bytes, cost_us)).collect();
            Arc::new(Fake { name, items: Mutex::new(items), refills: Mutex::new(0) })
        }
        fn held(&self) -> usize {
            self.items.lock().unwrap().iter().map(|c| c.bytes).sum()
        }
        fn count(&self) -> usize {
            self.items.lock().unwrap().len()
        }
        fn push(&self, used: u64, bytes: usize, cost_us: u64) {
            self.items.lock().unwrap().push_back(Candidate::new(used, bytes, cost_us));
        }
    }

    impl MemoryHolder for Fake {
        fn name(&self) -> &'static str {
            self.name
        }
        fn bytes(&self) -> usize {
            self.held()
        }
        fn oldest(&self) -> Option<Candidate> {
            self.items.lock().unwrap().front().copied()
        }
        fn drop_oldest(&self) -> usize {
            self.items.lock().unwrap().pop_front().map_or(0, |c| c.bytes)
        }
        fn refills(&self) -> u64 {
            *self.refills.lock().unwrap()
        }
    }

    fn holder(f: &Arc<Fake>) -> Arc<dyn MemoryHolder> {
        f.clone()
    }

    /// The tests' clock: everything is measured at 100 s.
    fn at_100s() -> u64 {
        100_000_000
    }

    fn accountant(budget: usize, held: &[&Arc<Fake>]) -> (Accountant, Vec<Arc<dyn MemoryHolder>>) {
        let acc = Accountant::with_clock(budget, at_100s);
        let held: Vec<Arc<dyn MemoryHolder>> = held.iter().map(|f| holder(f)).collect();
        for h in &held {
            acc.register(h);
        }
        (acc, held)
    }

    const S: u64 = 1_000_000;

    /// #3298: an expensive preview frame outlives a cheap footage plane of the same age and size.
    #[test]
    fn an_expensive_item_outlives_a_cheap_one_of_the_same_age() {
        let preview = Fake::new("preview frames", &[(90 * S, 100, 50_000)]);
        let planes = Fake::new("footage planes", &[(90 * S, 100, 5_000)]);
        let (acc, _held) = accountant(150, &[&preview, &planes]);
        acc.balance();
        assert_eq!((preview.count(), planes.count()), (1, 0));
    }

    /// #3298: a recent, costly plane outlives an older, cheap preview frame (by kind alone the
    /// preview frames were freed last, whatever they cost and however long unused).
    #[test]
    fn a_recent_costly_plane_outlives_an_old_cheap_preview_frame() {
        let preview = Fake::new("preview frames", &[(0, 100, 5_000)]);
        let planes = Fake::new("footage planes", &[(99 * S, 100, 50_000)]);
        let (acc, _held) = accountant(150, &[&planes, &preview]);
        acc.balance();
        assert_eq!((preview.count(), planes.count()), (0, 1));
    }

    /// Value decays with time unused: of two items that cost the same, the older goes first, and
    /// one unused for long enough goes before a costlier one used just now.
    #[test]
    fn value_falls_with_time_unused() {
        let c = |used, cost_us| Candidate::new(used, 100, cost_us);
        assert!(value(&c(0, 1000), at_100s(), false) < value(&c(99 * S, 1000), at_100s(), false));
        let fresh = value(&c(at_100s(), 1000), at_100s(), false);
        assert!((fresh - 10.0).abs() < 1e-9, "{fresh}");
        let a_second = value(&c(at_100s() - S, 1000), at_100s(), false);
        assert!((a_second - 10.0 / (2f64.ln() + 1.0)).abs() < 1e-9, "{a_second}");
        // #3301: the decay is slow: unused for ten minutes, an item keeps more than a tenth of its
        // value (it kept under a twentieth when it halved every 30 s).
        let ten_minutes = value(&c(0, 1000), 600 * S, false);
        assert!(ten_minutes > fresh / 10.0, "{ten_minutes}");
        assert!(value(&c(0, 2000), at_100s(), false) < value(&c(at_100s(), 1000), at_100s(), false));
        assert_eq!(value(&c(at_100s(), 0), at_100s(), false), 0.0, "a copy is worth nothing");
        assert_eq!(value(&c(0, 1000), at_100s(), true), THRASH_BOOST * value(&c(0, 1000), at_100s(), false));
    }

    /// Over the budget, items go least valuable first across holders, down to 90 % of the
    /// budget in one step; under it nothing is touched, and a copy goes first.
    #[test]
    fn least_valuable_first_down_to_the_trim_line() {
        let queue = Fake::new("disk write queue", &[(99 * S, 300, 0)]);
        let frames = Fake::new("footage frames", &[(10 * S, 100, 2_000), (20 * S, 100, 2_000), (30 * S, 100, 2_000), (40 * S, 100, 2_000)]);
        let layers = Fake::new("layer cache", &[(10 * S, 100, 40_000), (20 * S, 100, 40_000), (30 * S, 100, 40_000)]);
        let (acc, _held) = accountant(1000, &[&layers, &frames, &queue]);
        acc.balance();
        assert_eq!((queue.held(), frames.held(), layers.held()), (300, 400, 300), "at the budget: nothing freed");
        frames.push(50 * S, 200, 4_000);
        acc.balance();
        // 1200 held, down to 900: the queued copy goes first (300), nothing more is needed.
        assert_eq!((queue.held(), frames.held(), layers.held()), (0, 600, 300));
        assert_eq!(acc.total(), 900);
        assert_eq!(acc.freed(), 300);
        // A little over again: freed in one step to 900 or below, the cheap frames first.
        layers.push(60 * S, 300, 120_000);
        acc.balance();
        assert!(acc.total() <= 900, "{}", acc.total());
        assert_eq!(layers.held(), 600, "the costly layers stay");
    }

    /// A holder that makes again what it let go of is kept longer: its items are worth more
    /// while it thrashes.
    #[test]
    fn a_thrashing_holder_is_protected() {
        let cheap = Fake::new("cheap", &[(50 * S, 300, 1_000), (50 * S, 300, 1_000)]);
        let other = Fake::new("other", &[(50 * S, 300, 2_000), (50 * S, 300, 2_000)]);
        let (acc, _held) = accountant(1000, &[&cheap, &other]);
        *cheap.refills.lock().unwrap() += 3;
        acc.balance();
        assert_eq!(cheap.held(), 600, "thrashing: kept");
        assert_eq!(other.held(), 300);
    }

    /// One holder alone may fill the whole budget: no holder has a share of its own (#3298: a
    /// workload of footage only was held to the Footage Frame Cache).
    #[test]
    fn one_holder_may_use_the_whole_budget() {
        let frames = Fake::new("footage frames", &[]);
        let layers = Fake::new("layer cache", &[]);
        let (acc, _held) = accountant(1000, &[&frames, &layers]);
        for i in 0..10 {
            frames.push(i * S, 100, 2_000);
        }
        acc.balance();
        assert_eq!(frames.held(), 1000);
    }

    /// Growth within the balance interval still balances once it passes a part of the budget:
    /// a burst of large inserts can't run far past the budget before the next interval.
    #[test]
    fn a_burst_of_growth_balances_at_once() {
        let frames = Fake::new("footage frames", &[]);
        let (acc, _held) = accountant(1600, &[&frames]);
        acc.balance();
        // (within the interval of the balance just run)
        for i in 0..10 {
            frames.push(i * S, 200, 2_000);
            acc.note_growth(200);
        }
        assert!(frames.held() <= 1600 + 200, "{}", frames.held());
    }

    /// Holders that went away are forgotten; no budget means nothing is freed.
    #[test]
    fn holders_are_held_weakly() {
        let acc = Accountant::new(10);
        {
            let gone = Fake::new("gone", &[(0, 500, 1)]);
            acc.register(&holder(&gone));
        }
        assert_eq!(acc.total(), 0);
        let free = Accountant::default();
        let kept = Fake::new("kept", &[(0, 500, 1)]);
        free.register(&holder(&kept));
        free.balance();
        assert_eq!(kept.held(), 500);
    }

    /// The layer cache as a holder: its least recently used buffer goes first, and a buffer
    /// asked for again after the accountant let it go counts as a refill.
    #[test]
    fn the_layer_cache_is_trimmed_and_tells_refills() {
        use effectcraft_effects::Buf;
        use effectcraft_raster::Image;
        let cache = Arc::new(crate::cache::LayerCache::new(1 << 30));
        let one = 8 * 8 * std::mem::size_of::<effectcraft_raster::Px>();
        for k in 0..4u64 {
            cache.insert(k, Arc::new(Buf { img: Image::new(8, 8), offset: [0.0; 2], scale: 1.0 }), Default::default());
        }
        let acc = Accountant::new(2 * one);
        let layers: Arc<dyn MemoryHolder> = cache.clone();
        acc.register(&layers);
        acc.balance();
        assert!(layers.bytes() <= (2.0 * one as f64 * TRIM_TO) as usize, "{}", layers.bytes());
        assert!(cache.get(0).is_none(), "the oldest went first");
        assert_eq!(layers.refills(), 1, "asked for again after it was let go of");
        let next = layers.oldest().unwrap();
        assert!(next.cost_us > 0 && next.bytes == one, "{next:?}");
    }

    /// Halving a full budget (Reduce Cache Size When System Is Low on Memory) frees thousands of
    /// items in one balance, on the thread that inserted last: each step must be cheap (the
    /// layer cache keeps its buffers ordered by use instead of looking for the oldest).
    #[test]
    fn halving_a_full_budget_is_quick() {
        use effectcraft_effects::Buf;
        use effectcraft_raster::Image;
        let cache = Arc::new(crate::cache::LayerCache::new(1 << 40));
        for k in 0..5000u64 {
            cache.insert(k, Arc::new(Buf { img: Image::new(8, 8), offset: [0.0; 2], scale: 1.0 }), Default::default());
        }
        let frames = Fake::new("footage frames", &[]);
        for i in 0..5000 {
            frames.push(i, 1024, 2_000);
        }
        let acc = Accountant::new(usize::MAX);
        let held: Vec<Arc<dyn MemoryHolder>> = vec![cache.clone(), holder(&frames)];
        for h in &held {
            acc.register(h);
        }
        let full = acc.total();
        let t = web_time::Instant::now();
        acc.set_budget(full / 2);
        let took = t.elapsed();
        eprintln!("halving {full} bytes of 10000 items took {took:?}");
        assert!(acc.total() <= (full as f64 / 2.0 * TRIM_TO) as usize + 1024);
        assert!(took < std::time::Duration::from_secs(2), "{took:?}");
    }

    /// A holder that lowers the accountant's budget the first time it is asked to drop an item
    /// (another thread setting a lower budget while a balance runs).
    struct Lowering {
        inner: Arc<Fake>,
        acc: std::sync::OnceLock<Arc<Accountant>>,
        to: usize,
        done: AtomicBool,
    }

    impl MemoryHolder for Lowering {
        fn name(&self) -> &'static str {
            "lowering"
        }
        fn bytes(&self) -> usize {
            self.inner.held()
        }
        fn oldest(&self) -> Option<Candidate> {
            self.inner.oldest()
        }
        fn drop_oldest(&self) -> usize {
            if !self.done.swap(true, Ordering::SeqCst)
                && let Some(acc) = self.acc.get()
            {
                acc.set_budget(self.to);
            }
            self.inner.drop_oldest()
        }
    }

    /// #3301 review: a budget lowered while a balance runs is kept: the running balance runs
    /// again with it (it was skipped, and the caches stayed over the new budget).
    #[test]
    fn a_budget_lowered_during_a_balance_is_kept() {
        let items: Vec<(u64, usize, u64)> = (0..20).map(|i| (i * S, 100, 1_000)).collect();
        let lowering = Arc::new(Lowering { inner: Fake::new("frames", &items), acc: std::sync::OnceLock::new(), to: 500, done: AtomicBool::new(false) });
        let acc = Arc::new(Accountant::with_clock(1900, at_100s));
        let _ = lowering.acc.set(acc.clone());
        let held: Arc<dyn MemoryHolder> = lowering.clone();
        acc.register(&held);
        acc.balance();
        assert_eq!(acc.budget(), 500);
        assert!(acc.total() <= 450, "{}", acc.total());
    }

    /// #3301 review: a holder that panicked in a balance poisons its lock; balancing goes on
    /// (it stopped for good), and growth still balances.
    #[test]
    fn a_poisoned_balance_lock_does_not_stop_balancing() {
        let frames = Fake::new("frames", &[]);
        let (acc, _held) = accountant(1000, &[&frames]);
        let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut g = acc.balancing.lock().unwrap();
            *g = None;
            std::panic::resume_unwind(Box::new("a holder panicked"));
        }));
        assert!(poisoned.is_err() && acc.balancing.is_poisoned());
        for i in 0..12 {
            frames.push(i * S, 100, 1_000);
        }
        // (a small growth: no burst; the last balance is long ago)
        acc.note_growth(1);
        assert!(frames.held() <= 900, "growth balanced: {}", frames.held());
        for i in 12..20 {
            frames.push(i * S, 100, 1_000);
        }
        acc.balance();
        assert!(frames.held() <= 900, "{}", frames.held());
    }

    /// #3301 review: a balance asked for while `note_growth` held the lock to read the time
    /// (it found the lock busy and left the work to the holder) is served by `note_growth`.
    #[test]
    fn a_balance_asked_for_during_a_growth_check_is_served() {
        let frames = Fake::new("frames", &[]);
        let (acc, _held) = accountant(1000, &[&frames]);
        acc.balance();
        for i in 0..12 {
            frames.push(i * S, 100, 1_000);
        }
        // Just balanced (recent), a small growth (no burst), and a balance left pending.
        *acc.balancing.lock().unwrap() = Some(web_time::Instant::now());
        acc.requested.store(true, Ordering::SeqCst);
        acc.note_growth(1);
        assert!(frames.held() <= 900, "{}", frames.held());
    }

    static PROCESS_NOW: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    static PROCESS_CLOCK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(100_000_000);
    fn process_now() -> Option<u64> {
        Some(PROCESS_NOW.load(Ordering::SeqCst))
    }
    fn process_clock() -> u64 {
        PROCESS_CLOCK.load(Ordering::SeqCst)
    }
    /// (the process-memory tests share the statics above: one at a time)
    static PROCESS_SERIAL: Mutex<()> = Mutex::new(());

    /// #3301: memory nobody accounts for (GPU staging, effect buffers, fragmentation): with the
    /// caches within their budget but the process over its purge limit, the caches give way by
    /// what it is over, least valuable first; a reading that stays high (freed memory not shown
    /// yet) doesn't empty them; over the hard limit the protected items go too.
    #[test]
    fn the_process_memory_over_its_limit_trims_the_caches() {
        let _serial = PROCESS_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        let frames = Fake::new("frames", &[]);
        for i in 0..10 {
            frames.push(i * S, 100, 1_000);
        }
        let acc = Accountant::with_clock(10_000, process_clock);
        let held: Arc<dyn MemoryHolder> = frames.clone();
        acc.register(&held);
        // Under its purge limit (2000): nothing goes.
        PROCESS_NOW.store(1_500, Ordering::SeqCst);
        acc.set_process_memory(process_now, 2_000, 3_000);
        assert_eq!(frames.held(), 1000, "within the budget and the limit");
        // 2500 measured, down to the trim line under the limit (1800): 700 go.
        PROCESS_CLOCK.fetch_add(PROCESS_READ_EVERY, Ordering::SeqCst);
        PROCESS_NOW.store(2_500, Ordering::SeqCst);
        acc.balance();
        assert_eq!(frames.held(), 300, "the caches gave way for the process");
        // The reading stays high (the memory not handed back yet), for a minute: what was freed
        // counts, the caches are not drained.
        for _ in 0..240 {
            PROCESS_CLOCK.fetch_add(PROCESS_READ_EVERY, Ordering::SeqCst);
            acc.balance();
        }
        assert_eq!(frames.held(), 300, "the same reading doesn't drain the caches");
        // Read at most every 250 ms: a higher reading within that waits.
        PROCESS_NOW.store(2_600, Ordering::SeqCst);
        acc.balance();
        assert_eq!(frames.held(), 300);
        // 100 more than last time: the caches give way by that.
        PROCESS_CLOCK.fetch_add(PROCESS_READ_EVERY, Ordering::SeqCst);
        acc.balance();
        assert_eq!(frames.held(), 200, "grown by 100: 100 more go");
        // Under the limit again, then over: counted afresh.
        PROCESS_CLOCK.fetch_add(PROCESS_READ_EVERY, Ordering::SeqCst);
        PROCESS_NOW.store(1_000, Ordering::SeqCst);
        acc.balance();
        PROCESS_CLOCK.fetch_add(PROCESS_READ_EVERY, Ordering::SeqCst);
        PROCESS_NOW.store(2_050, Ordering::SeqCst);
        acc.balance();
        // (250 over the trim line, with nothing counted as freed: the 200 held go)
        assert_eq!(frames.held(), 0, "counted afresh");
    }

    /// #3301: frames a playing preview shows next (protected) keep the caches over the budget by
    /// at most a tenth of it while other items can go instead, and go once nothing else can or
    /// past that overage; past the process's hard limit no overage is allowed.
    #[test]
    fn protected_items_go_last_and_only_a_bounded_way_over() {
        let _serial = PROCESS_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        let preview = Fake::new("preview", &[]);
        for i in 0..4 {
            preview.items.lock().unwrap().push_back(Candidate { used: i * S, bytes: 100, cost_us: 1, protected: true });
        }
        let footage = Fake::new("footage", &[(99 * S, 300, 1_000_000)]);
        let acc = Accountant::with_clock(usize::MAX, process_clock);
        let held: Vec<Arc<dyn MemoryHolder>> = vec![holder(&preview), holder(&footage)];
        for h in &held {
            acc.register(h);
        }
        // 700 held, budget 500 (target 450, overage 50): the footage goes though worth more.
        acc.set_budget(500);
        assert_eq!((preview.held(), footage.held()), (400, 0));
        // Budget 300 (target 270, overage 30): nothing else left, the preview frames go down to
        // the overage over the target.
        acc.set_budget(300);
        assert_eq!(preview.held(), 300, "over the target by at most the overage");
        // Past the process's hard limit: no overage.
        PROCESS_CLOCK.fetch_add(PROCESS_READ_EVERY, Ordering::SeqCst);
        PROCESS_NOW.store(10_000, Ordering::SeqCst);
        acc.set_process_memory(process_now, 1_000, 2_000);
        assert!(preview.held() <= 200, "{}", preview.held());
    }
}
