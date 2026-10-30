//! Bounded assembly of the objects a tracker reports separately for each frame.

use std::collections::BTreeMap;
use std::mem::size_of;

use effectcraft_segment::TrackedFrame;

/// Separate from the model's memory banks: probability maps are full-resolution and can otherwise
/// grow with the entire clip when objects start on different frames.
pub(super) const MAX_PENDING_BYTES: usize = 256 << 20;

pub(super) struct PendingMasks {
    frames: BTreeMap<i64, Vec<Option<TrackedFrame>>>,
    objects: usize,
    bytes: usize,
    limit: usize,
}

impl PendingMasks {
    pub(super) fn new(objects: usize, limit: usize) -> Self {
        Self { frames: BTreeMap::new(), objects, bytes: 0, limit }
    }

    /// Probability allocation plus a conservative allowance for the map node and object slots.
    fn frame_bytes(&self) -> usize {
        self.objects.saturating_mul(size_of::<Option<TrackedFrame>>()).saturating_add(256)
    }

    fn mask_bytes(mask: &TrackedFrame) -> usize {
        mask.prob.capacity().saturating_mul(size_of::<f32>())
    }

    /// Complete frames leave immediately; incomplete frames may never exceed the budget. A rejected
    /// insertion leaves the buffer unchanged, including when it replaces an object's previous result.
    pub(super) fn insert(&mut self, object: usize, frame: i64, mask: TrackedFrame) -> Result<Option<Vec<Option<TrackedFrame>>>, String> {
        if object >= self.objects {
            return Err("tracker reported an unknown object".into());
        }
        if self.objects == 1 {
            return Ok(Some(vec![Some(mask)]));
        }
        let existing = self.frames.get(&frame);
        let old_bytes = existing.and_then(|p| p.get(object)).and_then(Option::as_ref).map_or(0, Self::mask_bytes);
        let added = Self::mask_bytes(&mask).saturating_add(if existing.is_none() { self.frame_bytes() } else { 0 });
        let bytes = self.bytes.saturating_sub(old_bytes).checked_add(added);
        let Some(bytes) = bytes.filter(|n| *n <= self.limit) else {
            return Err(format!("pending object masks exceed their {} MiB memory budget", self.limit >> 20));
        };
        let parts = self.frames.entry(frame).or_insert_with(|| vec![None; self.objects]);
        if let Some(slot) = parts.get_mut(object) {
            *slot = Some(mask);
        }
        self.bytes = bytes;
        if parts.iter().all(Option::is_some) {
            let parts = self.frames.remove(&frame).unwrap_or_default();
            let used = parts.iter().flatten().fold(self.frame_bytes(), |n, m| n.saturating_add(Self::mask_bytes(m)));
            self.bytes = self.bytes.saturating_sub(used);
            return Ok(Some(parts));
        }
        Ok(None)
    }

    pub(super) fn into_incomplete(self) -> BTreeMap<i64, Vec<Option<TrackedFrame>>> {
        self.frames
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mask(value: f32) -> TrackedFrame {
        TrackedFrame { prob: vec![value; 16], w: 4, h: 4, confidence: value, occluded: false }
    }

    #[test]
    fn different_start_passes_stop_before_buffering_the_whole_clip() {
        let mut pending = PendingMasks::new(2, 2048);
        let mut accepted = 0;
        // SAM 2 can report all of object 0 before starting object 1. A long clip must stop
        // at the memory limit even though no frame has received all its objects yet.
        for frame in 0..10_000 {
            match pending.insert(0, frame, mask(0.9)) {
                Ok(None) => accepted += 1,
                Ok(Some(_)) => panic!("no frame is complete yet"),
                Err(e) => {
                    assert!(e.contains("memory budget"), "{e}");
                    break;
                }
            }
        }
        assert!(accepted > 0 && accepted < 10_000);
        assert_eq!(pending.frames.len(), accepted);
        assert!(pending.bytes <= pending.limit);
        let before = pending.bytes;
        assert!(pending.insert(0, 10_001, mask(0.8)).is_err());
        assert_eq!(pending.bytes, before, "rejection must not retain the new map");
    }

    #[test]
    fn interleaved_objects_release_the_budget_and_keep_their_masks() {
        let mut pending = PendingMasks::new(2, 2048);
        for frame in 0..10_000 {
            let first = (frame % 2) as usize;
            assert!(pending.insert(first, frame, mask(first as f32)).unwrap().is_none());
            let parts = pending.insert(1 - first, frame, mask((1 - first) as f32)).unwrap().unwrap();
            assert_eq!(parts[0].as_ref().unwrap().prob, vec![0.0; 16]);
            assert_eq!(parts[1].as_ref().unwrap().prob, vec![1.0; 16]);
            assert!(pending.frames.is_empty());
            assert_eq!(pending.bytes, 0);
        }
    }

    #[test]
    fn replacements_charge_capacity_and_partial_frames_are_preserved() {
        let mut pending = PendingMasks::new(2, 2048);
        pending.insert(0, 7, mask(0.1)).unwrap();
        let before = pending.bytes;
        pending.insert(0, 7, mask(0.2)).unwrap();
        assert_eq!(pending.bytes, before, "a duplicate replaces rather than accumulates");
        let mut oversized = mask(0.3);
        oversized.prob.reserve(2048);
        assert!(pending.insert(0, 7, oversized).is_err(), "capacity, not length, owns the memory");
        assert_eq!(pending.bytes, before);
        let frames = pending.into_incomplete();
        assert_eq!(frames[&7][0].as_ref().unwrap().prob, vec![0.2; 16]);
        assert!(frames[&7][1].is_none());
    }

    #[test]
    fn single_objects_need_no_pending_budget_and_unknown_objects_are_rejected() {
        let mut pending = PendingMasks::new(1, 0);
        assert!(pending.insert(0, 0, mask(0.9)).unwrap().is_some());
        assert_eq!(pending.bytes, 0);
        assert!(pending.insert(1, 0, mask(0.9)).is_err());
        assert!(pending.frames.is_empty());
    }
}
