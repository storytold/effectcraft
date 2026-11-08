//! One memory accountant for the big caches (#3271): the layer cache, the media pool's frames,
//! the decoded footage planes and the preview frames each keep their own budget, and register
//! here as [`MemoryHolder`]s. When all of them together hold more than the one budget
//! ([`Accountant::set_budget`], the sum of Settings ▸ Memory & CPU's budgets), the accountant
//! frees from the lowest priority first, in large steps: down to [`TRIM_TO`] of the budget, so
//! a cache doesn't fill up and trim again with every insert.
//!
//! Holders can share a budget of their own ([`MemoryHolder::group`],
//! [`Accountant::set_group_budget`]): the footage frames and the decoded footage planes
//! ([`FOOTAGE`]) together take at most the Footage Frame Cache. Within a group the least recently
//! used item goes first, whichever holder keeps it ([`MemoryHolder::oldest_use`], stamped with
//! [`tick`]): a frame can push out plane sets and plane sets can push out frames; neither has a
//! share of its own.
//!
//! A holder that had to make again what it let go of shortly before (its "refills",
//! [`MemoryHolder::refills`]) is thrashing: its priority goes up by one until it settles, so the
//! data that is needed again is kept longest.
//!
//! Holders are held weakly: a cache that goes away is forgotten. The accountant never calls a
//! holder while it holds its own lock, and holders call [`Accountant::balance`] after releasing
//! theirs.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};

/// The share of the budget the accountant frees down to once it is over (hysteresis).
pub const TRIM_TO: f64 = 0.9;

/// The group of the footage frames and the decoded footage planes: together within the
/// Footage Frame Cache ([`Accountant::set_group_budget`]).
pub const FOOTAGE: &str = "footage";

/// A use stamp the holders of a group share: later uses get larger stamps (a counter, not a
/// clock: two uses never tie).
pub fn tick() -> u64 {
    static TICK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    TICK.fetch_add(1, Ordering::Relaxed)
}

/// How long a holder that refilled stays protected.
const THRASH_FOR: std::time::Duration = std::time::Duration::from_secs(10);

/// Inserts balance at most this often ([`Accountant::note_growth`]).
const BALANCE_EVERY: std::time::Duration = std::time::Duration::from_millis(100);

/// A cache the accountant keeps within the common budget.
pub trait MemoryHolder: Send + Sync {
    /// A name for logs and stats.
    fn name(&self) -> &'static str;
    /// The bytes it holds now.
    fn bytes(&self) -> usize;
    /// How valuable its data is: lower is freed first (a copy of what another cache holds,
    /// then what is cheap to make again, then what is expensive).
    fn priority(&self) -> u8;
    /// Free its least valuable data until it holds at most `bytes` (data in use may stay).
    fn trim_to(&self, bytes: usize);
    /// How many times so far it made again what it had let go of shortly before (0 when it
    /// doesn't tell).
    fn refills(&self) -> u64 {
        0
    }
    /// The budget it shares with other holders ([`Accountant::set_group_budget`]), if any.
    fn group(&self) -> Option<&'static str> {
        None
    }
    /// The [`tick`] its least recently used item it may drop was last used at (`None`: nothing
    /// to drop, or it doesn't tell; then its group trims it by priority).
    fn oldest_use(&self) -> Option<u64> {
        None
    }
    /// Drop its least recently used item it may drop; the bytes freed (0: nothing dropped).
    fn drop_oldest(&self) -> usize {
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
    /// The budgets holders share ([`MemoryHolder::group`]).
    groups: Mutex<Vec<(&'static str, usize)>>,
    holders: Mutex<Vec<Registered>>,
    /// Bytes freed by the accountant so far (stats, tests).
    freed: AtomicUsize,
    /// One balance at a time (another one asked for meanwhile is skipped), and when the last
    /// one ran.
    balancing: Mutex<Option<web_time::Instant>>,
}

impl Default for Accountant {
    fn default() -> Self {
        Accountant::new(usize::MAX)
    }
}

impl Accountant {
    /// An accountant with `budget` bytes (`usize::MAX`: no limit).
    pub const fn new(budget: usize) -> Accountant {
        Accountant {
            budget: AtomicUsize::new(budget),
            groups: Mutex::new(Vec::new()),
            holders: Mutex::new(Vec::new()),
            freed: AtomicUsize::new(0),
            balancing: Mutex::new(None),
        }
    }

    /// The process's accountant.
    pub fn global() -> &'static Accountant {
        static GLOBAL: Accountant = Accountant::new(usize::MAX);
        &GLOBAL
    }

    /// Set the common budget and free down to it at once.
    pub fn set_budget(&self, bytes: usize) {
        self.budget.store(bytes, Ordering::Relaxed);
        self.balance();
    }

    pub fn budget(&self) -> usize {
        self.budget.load(Ordering::Relaxed)
    }

    /// Set the budget the holders of `group` share and free down to it at once.
    pub fn set_group_budget(&self, group: &'static str, bytes: usize) {
        {
            let mut g = self.groups.lock().unwrap_or_else(PoisonError::into_inner);
            match g.iter_mut().find(|(n, _)| *n == group) {
                Some(e) => e.1 = bytes,
                None => g.push((group, bytes)),
            }
        }
        self.balance();
    }

    /// The budget the holders of `group` share (`None`: none set).
    pub fn group_budget(&self, group: &str) -> Option<usize> {
        self.groups.lock().unwrap_or_else(PoisonError::into_inner).iter().find(|(n, _)| *n == group).map(|(_, b)| *b)
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

    /// A holder grew (an insert): balance, at most every [`BALANCE_EVERY`] (the holders' locks
    /// aren't taken on every insert).
    pub fn note_growth(&self) {
        if self.budget() == usize::MAX && self.groups.lock().unwrap_or_else(PoisonError::into_inner).is_empty() {
            return;
        }
        let recent = self.balancing.try_lock().map(|g| g.is_some_and(|t| t.elapsed() < BALANCE_EVERY)).unwrap_or(true);
        if !recent {
            self.balance();
        }
    }

    /// The holders alive now, with their effective priority (raised while thrashing).
    fn alive(&self) -> Vec<(Arc<dyn MemoryHolder>, u8)> {
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
                let thrashing = r.thrash_until.is_some_and(|t| t > now);
                let p = holder.priority().saturating_add(u8::from(thrashing));
                Some((holder, p))
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

    /// When the holders of a group are over its budget, or all holders over the common one,
    /// free down to [`TRIM_TO`] of it, lowest priority first (a holder is trimmed as far as
    /// needed before the next one is touched): the groups first, then all holders.
    pub fn balance(&self) {
        let budget = self.budget();
        let groups = self.groups.lock().unwrap_or_else(PoisonError::into_inner).clone();
        if budget == usize::MAX && groups.is_empty() {
            return;
        }
        // (one at a time: a balance running meanwhile does the work)
        let Ok(mut last) = self.balancing.try_lock() else { return };
        *last = Some(web_time::Instant::now());
        let mut holders = self.alive();
        holders.sort_by_key(|(_, p)| *p);
        for (group, bytes) in groups {
            let members: Vec<&Arc<dyn MemoryHolder>> = holders.iter().filter(|(h, _)| h.group() == Some(group)).map(|(h, _)| h).collect();
            self.trim_oldest(&members, bytes);
            self.trim(&members, bytes);
        }
        if budget != usize::MAX {
            let all: Vec<&Arc<dyn MemoryHolder>> = holders.iter().map(|(h, _)| h).collect();
            self.trim(&all, budget);
        }
    }

    /// Free from `holders` least recently used first, whichever holder keeps it, down to
    /// [`TRIM_TO`] of `budget` when they hold more than it (holders that tell their oldest use).
    fn trim_oldest(&self, holders: &[&Arc<dyn MemoryHolder>], budget: usize) {
        let mut total: usize = holders.iter().map(|h| h.bytes()).fold(0, usize::saturating_add);
        if total <= budget {
            return;
        }
        let target = (budget as f64 * TRIM_TO) as usize;
        let mut left: Vec<&Arc<dyn MemoryHolder>> = holders.to_vec();
        while total > target {
            let Some((i, _)) = left.iter().enumerate().filter_map(|(i, h)| Some((i, h.oldest_use()?))).min_by_key(|(_, t)| *t) else { break };
            let Some(h) = left.get(i) else { break };
            let freed = h.drop_oldest();
            if freed == 0 {
                left.remove(i);
                continue;
            }
            total = total.saturating_sub(freed);
            self.freed.fetch_add(freed, Ordering::Relaxed);
        }
    }

    /// Free from `holders` (lowest priority first) down to [`TRIM_TO`] of `budget` when they
    /// hold more than it.
    fn trim(&self, holders: &[&Arc<dyn MemoryHolder>], budget: usize) {
        let mut total: usize = holders.iter().map(|h| h.bytes()).fold(0, usize::saturating_add);
        if total <= budget {
            return;
        }
        let target = (budget as f64 * TRIM_TO) as usize;
        for h in holders {
            if total <= target {
                break;
            }
            let had = h.bytes();
            let over = total - target;
            h.trim_to(had.saturating_sub(over));
            let now = h.bytes();
            let freed = had.saturating_sub(now);
            total = total.saturating_sub(freed);
            self.freed.fetch_add(freed, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A holder of `bytes` in blocks of `step`, trimmed whole blocks at a time.
    struct Fake {
        name: &'static str,
        bytes: Mutex<usize>,
        step: usize,
        priority: u8,
        refills: Mutex<u64>,
        group: Option<&'static str>,
    }

    impl Fake {
        fn new(name: &'static str, bytes: usize, priority: u8) -> Arc<Fake> {
            Arc::new(Fake { name, bytes: Mutex::new(bytes), step: 10, priority, refills: Mutex::new(0), group: None })
        }
        fn in_group(name: &'static str, bytes: usize, priority: u8, group: &'static str) -> Arc<Fake> {
            Arc::new(Fake { name, bytes: Mutex::new(bytes), step: 10, priority, refills: Mutex::new(0), group: Some(group) })
        }
        fn held(&self) -> usize {
            *self.bytes.lock().unwrap()
        }
        fn fill(&self, more: usize) {
            *self.bytes.lock().unwrap() += more;
        }
    }

    impl MemoryHolder for Fake {
        fn name(&self) -> &'static str {
            self.name
        }
        fn bytes(&self) -> usize {
            self.held()
        }
        fn priority(&self) -> u8 {
            self.priority
        }
        fn trim_to(&self, bytes: usize) {
            let mut b = self.bytes.lock().unwrap();
            while *b > bytes {
                *b = b.saturating_sub(self.step);
            }
        }
        fn refills(&self) -> u64 {
            *self.refills.lock().unwrap()
        }
        fn group(&self) -> Option<&'static str> {
            self.group
        }
    }

    /// #3271: holders of a group share its budget and make room for each other (lowest
    /// priority first) before the common budget is looked at; a holder outside the group is
    /// left alone while the common budget holds.
    #[test]
    fn a_group_shares_its_budget() {
        let acc = Accountant::new(usize::MAX);
        let (frames, planes, layers) = (Fake::in_group("frames", 400, 0, FOOTAGE), Fake::in_group("planes", 500, 1, FOOTAGE), Fake::new("layers", 800, 2));
        let held: Vec<Arc<dyn MemoryHolder>> = vec![holder(&frames), holder(&planes), holder(&layers)];
        for h in &held {
            acc.register(h);
        }
        acc.set_group_budget(FOOTAGE, 1000);
        assert_eq!((frames.held(), planes.held(), layers.held()), (400, 500, 800), "within the group's budget");
        planes.fill(300);
        acc.balance();
        // 1200 in the group, down to 900: the frames make room for the planes.
        assert_eq!((frames.held(), planes.held(), layers.held()), (100, 800, 800));
        // A larger footage budget: the planes alone may hold more than the old fixed 1 GB.
        acc.set_group_budget(FOOTAGE, 10 << 30);
        planes.fill(2 << 30);
        acc.balance();
        assert!(planes.held() > 1 << 30, "{}", planes.held());
        assert_eq!(acc.group_budget(FOOTAGE), Some(10 << 30));
    }

    fn holder(f: &Arc<Fake>) -> Arc<dyn MemoryHolder> {
        f.clone()
    }

    /// Over the budget, the lowest priority is freed first, down to 90% of the budget; under it,
    /// nothing is touched.
    #[test]
    fn lowest_priority_goes_first_down_to_the_trim_line() {
        let acc = Accountant::new(1000);
        let (planes, frames, layers) = (Fake::new("planes", 300, 0), Fake::new("frames", 400, 1), Fake::new("layers", 300, 2));
        let held: Vec<Arc<dyn MemoryHolder>> = vec![holder(&layers), holder(&planes), holder(&frames)];
        for h in &held {
            acc.register(h);
        }
        acc.balance();
        assert_eq!((planes.held(), frames.held(), layers.held()), (300, 400, 300), "at the budget: nothing freed");
        frames.fill(200);
        acc.balance();
        // 1200 held, down to 900: the planes go first (300), then frames give the rest.
        assert_eq!((planes.held(), frames.held(), layers.held()), (0, 600, 300));
        assert_eq!(acc.total(), 900);
        assert_eq!(acc.freed(), 300);
        // A little over again: freed in one step down to 900, not by the few bytes over.
        layers.fill(110);
        acc.balance();
        assert_eq!(acc.total(), 900);
    }

    /// A holder that makes again what it let go of is kept longer: its priority goes up while it
    /// thrashes.
    #[test]
    fn a_thrashing_holder_is_protected() {
        let acc = Accountant::new(1000);
        let (cheap, other) = (Fake::new("cheap", 600, 0), Fake::new("other", 600, 0));
        let held: Vec<Arc<dyn MemoryHolder>> = vec![holder(&cheap), holder(&other)];
        for h in &held {
            acc.register(h);
        }
        *cheap.refills.lock().unwrap() += 3;
        acc.balance();
        assert_eq!(cheap.held(), 600, "thrashing: kept");
        assert_eq!(other.held(), 300);
    }

    /// Holders that went away are forgotten; no budget means nothing is freed.
    #[test]
    fn holders_are_held_weakly() {
        let acc = Accountant::new(10);
        {
            let gone = Fake::new("gone", 500, 0);
            acc.register(&holder(&gone));
        }
        assert_eq!(acc.total(), 0);
        let free = Accountant::default();
        let kept = Fake::new("kept", 500, 0);
        free.register(&holder(&kept));
        free.balance();
        assert_eq!(kept.held(), 500);
    }

    /// The layer cache as a holder: trimmed least recently used first, and a buffer asked for
    /// again after the accountant let it go counts as a refill, which protects the cache.
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
        // Thrashing: a holder of the same priority is freed before it.
        let other = Fake::new("other", 2 * one, 2);
        acc.register(&holder(&other));
        acc.balance();
        assert!(other.held() < 2 * one, "the other holder gave way");
    }
}
