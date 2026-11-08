//! One memory accountant for the big caches (#3271): the layer cache, the media pool's frames,
//! the decoded footage planes and the preview frames each keep their own budget, and register
//! here as [`MemoryHolder`]s. When all of them together hold more than the one budget
//! ([`Accountant::set_budget`], the sum of Settings ▸ Memory & CPU's budgets), the accountant
//! frees from the lowest priority first, in large steps: down to [`TRIM_TO`] of the budget, so
//! a cache doesn't fill up and trim again with every insert.
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
}

struct Registered {
    holder: Weak<dyn MemoryHolder>,
    /// Its refills at the last balance, and whether it was thrashing then.
    refills: u64,
    thrashing: bool,
}

/// The common budget of the big caches (see the module notes).
pub struct Accountant {
    budget: AtomicUsize,
    holders: Mutex<Vec<Registered>>,
    /// Bytes freed by the accountant so far (stats, tests).
    freed: AtomicUsize,
}

impl Default for Accountant {
    fn default() -> Self {
        Accountant::new(usize::MAX)
    }
}

impl Accountant {
    /// An accountant with `budget` bytes (`usize::MAX`: no limit).
    pub const fn new(budget: usize) -> Accountant {
        Accountant { budget: AtomicUsize::new(budget), holders: Mutex::new(Vec::new()), freed: AtomicUsize::new(0) }
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

    /// Keep `holder` within the common budget (held weakly).
    pub fn register(&self, holder: &Arc<dyn MemoryHolder>) {
        let mut h = self.holders.lock().unwrap_or_else(PoisonError::into_inner);
        h.retain(|r| r.holder.strong_count() > 0);
        let weak = Arc::downgrade(holder);
        // (registered again with each new budget: once is enough)
        if h.iter().any(|r| r.holder.ptr_eq(&weak)) {
            return;
        }
        h.push(Registered { holder: weak, refills: holder.refills(), thrashing: false });
    }

    /// The holders alive now, with their effective priority (raised while thrashing).
    fn alive(&self) -> Vec<(Arc<dyn MemoryHolder>, u8)> {
        let mut h = self.holders.lock().unwrap_or_else(PoisonError::into_inner);
        h.retain(|r| r.holder.strong_count() > 0);
        h.iter_mut()
            .filter_map(|r| {
                let holder = r.holder.upgrade()?;
                let refills = holder.refills();
                r.thrashing = refills > r.refills;
                r.refills = refills;
                let p = holder.priority().saturating_add(u8::from(r.thrashing));
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

    /// When the holders together are over the budget, free down to [`TRIM_TO`] of it, lowest
    /// priority first (a holder is trimmed as far as needed before the next one is touched).
    pub fn balance(&self) {
        let budget = self.budget();
        if budget == usize::MAX {
            return;
        }
        let mut holders = self.alive();
        let mut total: usize = holders.iter().map(|(h, _)| h.bytes()).fold(0, usize::saturating_add);
        if total <= budget {
            return;
        }
        let target = (budget as f64 * TRIM_TO) as usize;
        holders.sort_by_key(|(_, p)| *p);
        for (h, _) in holders {
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
    }

    impl Fake {
        fn new(name: &'static str, bytes: usize, priority: u8) -> Arc<Fake> {
            Arc::new(Fake { name, bytes: Mutex::new(bytes), step: 10, priority, refills: Mutex::new(0) })
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
