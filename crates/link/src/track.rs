//! The change tracker: which links changed, which epoch each is at, and which results to render.
//!
//! [`Tracker`] holds no clock and no session. The caller says what happened and when.
//!
//! - [`Tracker::changed`] adds 1 to the epoch of every bound link and starts a quiet period.
//! - [`Tracker::due`] names the links whose result file is stale once the quiet period of
//!   [`SETTLE`] has passed with no change. It names a link once per change.
//! - A link added with [`Tracker::sync`] starts at the epoch the binding stored.
//!
//! Contract section 9 rule 2: an epoch never decreases.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

/// Quiet time after the last change before the result file is rendered.
pub const SETTLE: Duration = Duration::from_millis(500);

#[derive(Debug, Default)]
pub struct Tracker {
    epochs: BTreeMap<String, u64>,
    /// Links whose result file is older than their epoch.
    stale: BTreeSet<String>,
    last_change: Option<Instant>,
    settle: Option<Duration>,
}

/// A link and the epoch it is at.
pub type Stamp = (String, u64);

impl Tracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// A tracker with another quiet period (tests).
    pub fn with_settle(settle: Duration) -> Self {
        Self { settle: Some(settle), ..Self::default() }
    }

    fn settle(&self) -> Duration {
        self.settle.unwrap_or(SETTLE)
    }

    /// Make the tracked links the bound ones. `bound` is `(link_id, epoch the binding stores)`.
    /// A new link starts at the stored epoch, or at its own epoch when that is higher. A link that
    /// is gone is dropped. Returns true when a link was added.
    pub fn sync<'a>(&mut self, bound: impl IntoIterator<Item = (&'a str, u64)>) -> bool {
        let mut added = false;
        let mut keep = BTreeSet::new();
        for (link_id, stored) in bound {
            keep.insert(link_id.to_string());
            match self.epochs.get_mut(link_id) {
                Some(epoch) => *epoch = (*epoch).max(stored),
                None => {
                    self.epochs.insert(link_id.to_string(), stored);
                    added = true;
                }
            }
        }
        self.epochs.retain(|link_id, _| keep.contains(link_id));
        self.stale.retain(|link_id| keep.contains(link_id));
        added
    }

    /// The comp changed at `now`. Returns every link with its new epoch.
    pub fn changed(&mut self, now: Instant) -> Vec<Stamp> {
        self.last_change = Some(now);
        let mut out = Vec::with_capacity(self.epochs.len());
        for (link_id, epoch) in &mut self.epochs {
            *epoch += 1;
            self.stale.insert(link_id.clone());
            out.push((link_id.clone(), *epoch));
        }
        out
    }

    /// Links to render now: stale, and quiet for [`SETTLE`]. Each is returned once per change.
    pub fn due(&mut self, now: Instant) -> Vec<Stamp> {
        let Some(last) = self.last_change else { return Vec::new() };
        if self.stale.is_empty() || now.saturating_duration_since(last) < self.settle() {
            return Vec::new();
        }
        let links = std::mem::take(&mut self.stale);
        links.into_iter().filter_map(|link_id| self.epochs.get(&link_id).map(|epoch| (link_id, *epoch))).collect()
    }

    /// Links to render now, without the quiet period. The window is closing, so there is no time
    /// to wait for the user to stop. Each is returned once.
    pub fn due_now(&mut self) -> Vec<Stamp> {
        let links = std::mem::take(&mut self.stale);
        links.into_iter().filter_map(|link_id| self.epochs.get(&link_id).map(|epoch| (link_id, *epoch))).collect()
    }

    /// The result file of `link_id` is missing, so it needs a render whatever the epoch says. An
    /// epoch is spent on it, because the file will carry a newer one than any the peer has seen.
    /// A link that is already stale keeps the epoch its edits gave it. Returns the new stamp when
    /// the epoch moved.
    pub fn needs_render(&mut self, link_id: &str) -> Option<Stamp> {
        let epoch = self.epochs.get_mut(link_id)?;
        if self.stale.insert(link_id.to_string()) {
            *epoch += 1;
            return Some((link_id.to_string(), *epoch));
        }
        None
    }

    /// True while a link waits for a render.
    pub fn has_stale(&self) -> bool {
        !self.stale.is_empty()
    }

    /// Forget every waiting render (the user gave up on the close).
    pub fn clear_stale(&mut self) {
        self.stale.clear();
    }

    /// When [`Tracker::due`] will have something, if a change is waiting.
    pub fn wake_at(&self) -> Option<Instant> {
        if self.stale.is_empty() { None } else { self.last_change.map(|last| last + self.settle()) }
    }

    /// A render of `link_id` ended. When it did not produce the file (`rendered` is false: it
    /// failed or was cancelled) the link is stale again and the next quiet period renders it.
    pub fn render_ended(&mut self, link_id: &str, rendered: bool) {
        if !rendered && self.epochs.contains_key(link_id) {
            self.stale.insert(link_id.to_string());
        }
    }

    pub fn epoch(&self, link_id: &str) -> Option<u64> {
        self.epochs.get(link_id).copied()
    }

    /// Every tracked link with its epoch.
    pub fn epochs(&self) -> impl Iterator<Item = (&str, u64)> {
        self.epochs.iter().map(|(link_id, epoch)| (link_id.as_str(), *epoch))
    }

    pub fn is_empty(&self) -> bool {
        self.epochs.is_empty()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    const A: &str = "11111111-1111-4111-8111-111111111111";
    const B: &str = "22222222-2222-4222-8222-222222222222";

    fn at(ms: u64, base: Instant) -> Instant {
        base + Duration::from_millis(ms)
    }

    #[test]
    fn a_change_bumps_every_bound_link_by_one() {
        let base = Instant::now();
        let mut t = Tracker::new();
        assert!(t.sync([(A, 3), (B, 0)]));
        let mut stamps = t.changed(base);
        stamps.sort();
        assert_eq!(stamps, vec![(A.to_string(), 4), (B.to_string(), 1)]);
        assert_eq!(t.epoch(A), Some(4));
    }

    #[test]
    fn sync_keeps_a_higher_epoch_and_drops_unbound_links() {
        let base = Instant::now();
        let mut t = Tracker::new();
        t.sync([(A, 0)]);
        t.changed(base);
        t.changed(base);
        assert!(!t.sync([(A, 1)]), "A is not new");
        assert_eq!(t.epoch(A), Some(2), "a stored epoch below the live one does not lower it");
        assert!(!t.sync([(A, 9)]));
        assert_eq!(t.epoch(A), Some(9), "a stored epoch above the live one raises it");
        t.sync([(B, 0)]);
        assert_eq!(t.epoch(A), None);
        assert!(t.due(at(10_000, base)).is_empty(), "A is gone, so it is not rendered");
    }

    #[test]
    fn the_result_is_due_only_after_a_quiet_half_second() {
        let base = Instant::now();
        let mut t = Tracker::new();
        t.sync([(A, 0)]);
        t.changed(base);
        assert!(t.due(at(499, base)).is_empty());
        assert_eq!(t.wake_at(), Some(at(500, base)));
        t.changed(at(400, base));
        assert!(t.due(at(899, base)).is_empty(), "a new change restarts the quiet period");
        assert_eq!(t.due(at(900, base)), vec![(A.to_string(), 2)]);
        assert!(t.due(at(5_000, base)).is_empty(), "once per change");
        assert_eq!(t.wake_at(), None);
    }

    #[test]
    fn nothing_is_due_without_a_change() {
        let mut t = Tracker::new();
        t.sync([(A, 5)]);
        assert!(t.due(Instant::now() + Duration::from_secs(60)).is_empty());
    }

    #[test]
    fn a_failed_render_is_due_again() {
        let base = Instant::now();
        let mut t = Tracker::with_settle(Duration::from_millis(10));
        t.sync([(A, 0)]);
        t.changed(base);
        assert_eq!(t.due(at(10, base)).len(), 1);
        t.render_ended(A, false);
        assert_eq!(t.due(at(20, base)).len(), 1);
        t.render_ended(A, true);
        assert!(t.due(at(30, base)).is_empty());
    }
}
