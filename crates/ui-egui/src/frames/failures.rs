//! Bounded preview failure history; unknown transport failures are never permanent.
use super::{FrameKey, RemoteFrame};
use std::time::Duration;

const MAX_FAILURES: usize = 128;
const MAX_MESSAGE_BYTES: usize = 512;
const INITIAL_BACKOFF_MS: u64 = 250;
const MAX_BACKOFF_MS: u64 = 8_000;
/// Preview conversion only: allows an 8K RGBA8 frame (~127 MiB), not export admission.
const MAX_REMOTE_PREVIEW_BYTES: usize = 256 << 20;

pub(super) fn remote_image(frame: &RemoteFrame, cache_budget: usize, max_side: Option<usize>) -> Result<egui::ColorImage, PreviewFailure> {
    let reject = || PreviewFailure::new(FailureKind::Retryable, "Frame worker pixels exceed preview storage or have invalid dimensions");
    let w = usize::try_from(frame.width).map_err(|_| reject())?;
    let h = usize::try_from(frame.height).map_err(|_| reject())?;
    if let Some(limit) = max_side {
        presentation_size_limit([w, h], limit)?;
    }
    // A headless worker has no device limit; every eventual presentation revalidates it.
    let count = w.checked_mul(h).ok_or_else(reject)?;
    let bytes = count.checked_mul(4).ok_or_else(reject)?;
    if count == 0 || bytes != frame.rgba.len() || bytes > MAX_REMOTE_PREVIEW_BYTES.min(cache_budget) {
        return Err(reject());
    }
    let mut pixels = Vec::new();
    pixels.try_reserve_exact(count).map_err(|_| PreviewFailure::new(FailureKind::Retryable, "Could not reserve preview pixel storage"))?;
    for &[r, g, b, a] in frame.rgba.as_chunks::<4>().0 {
        pixels.push(egui::Color32::from_rgba_premultiplied(r, g, b, a));
    }
    Ok(egui::ColorImage::new([w, h], pixels))
}

/// Check the live egui presentation limit, never a guessed hardware limit.
/// The byte ceiling applies only to preview textures, not export or CPU sampling.
pub fn presentation_size_limit(size: [usize; 2], max_side: usize) -> Result<(), PreviewFailure> {
    let valid = size.iter().all(|side| *side > 0 && *side <= max_side)
        && size[0].checked_mul(size[1]).and_then(|n| n.checked_mul(4)).is_some_and(|bytes| bytes <= MAX_REMOTE_PREVIEW_BYTES);
    if valid {
        Ok(())
    } else {
        Err(PreviewFailure::new(
            FailureKind::Retryable,
            "Frame exceeds the current preview texture limit; reduce preview resolution or retry after device recovery",
        ))
    }
}
pub fn presentation_size(ctx: &egui::Context, size: [usize; 2]) -> Result<(), PreviewFailure> {
    presentation_size_limit(size, ctx.input(|i| i.max_texture_side))
}
pub fn presentation_image(ctx: &egui::Context, image: &egui::ColorImage) -> Result<(), PreviewFailure> {
    presentation_size(ctx, image.size)?;
    if image.size[0].checked_mul(image.size[1]) != Some(image.pixels.len()) {
        return Err(PreviewFailure::new(FailureKind::Retryable, "Frame pixel count does not match its preview dimensions"));
    }
    Ok(())
}
/// Pane-local callers have no main-frame key. Preserve a bounded actionable status.
pub fn presentation_check(ctx: &egui::Context, size: [usize; 2], status: &mut String) -> bool {
    match presentation_size(ctx, size) {
        Ok(()) => true,
        Err(error) => {
            *status = error.message().to_owned();
            false
        }
    }
}

/// Only an explicitly typed content/admission error can wait for an edit or manual retry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureKind {
    Content,
    Retryable,
}

#[derive(Clone, Debug)]
pub struct PreviewFailure {
    pub kind: FailureKind,
    message: String,
}
impl PreviewFailure {
    pub fn new(kind: FailureKind, message: &str) -> Self {
        let mut end = message.len().min(MAX_MESSAGE_BYTES);
        while !message.is_char_boundary(end) {
            end = end.saturating_sub(1);
        }
        let mut bounded = String::new();
        if bounded.try_reserve_exact(end).is_ok()
            && let Some(text) = message.get(..end)
        {
            bounded.push_str(text);
        }
        Self { kind, message: bounded }
    }
    pub fn message(&self) -> &str {
        if self.message.is_empty() { "Preview rendering failed" } else { &self.message }
    }
}

struct Entry {
    key: FrameKey,
    failure: PreviewFailure,
    attempts: u32,
    retry_at: Duration,
}
#[derive(Default)]
pub(super) struct Failures {
    entries: Vec<Entry>,
    /// Also acts as a generation: late callbacks before retry/recovery cannot re-block it.
    pub epoch: u64,
    allocation_retry: Option<Duration>,
}
impl Failures {
    pub fn record(&mut self, key: FrameKey, epoch: u64, failure: PreviewFailure, now: Duration) {
        if epoch != self.epoch {
            return;
        }
        let attempts = self.entries.iter().find(|e| e.key == key).map_or(1, |e| e.attempts.saturating_add(1).min(6));
        self.entries.retain(|e| e.key != key);
        if self.entries.len() >= MAX_FAILURES {
            self.entries.remove(0);
        }
        if self.entries.try_reserve(1).is_err() {
            self.allocation_retry = Some(now.saturating_add(Duration::from_millis(MAX_BACKOFF_MS)));
            return;
        }
        let millis = INITIAL_BACKOFF_MS.saturating_mul(1 << attempts.saturating_sub(1)).min(MAX_BACKOFF_MS);
        self.entries.push(Entry { key, failure, attempts, retry_at: now.saturating_add(Duration::from_millis(millis)) });
    }
    pub fn delay(&self, key: &FrameKey, now: Duration) -> Option<Duration> {
        if let Some(until) = self.allocation_retry.filter(|until| *until > now) {
            return Some(until.saturating_sub(now));
        }
        let entry = self.entries.iter().find(|e| e.key == *key)?;
        match entry.failure.kind {
            FailureKind::Content => Some(Duration::MAX),
            FailureKind::Retryable if entry.retry_at > now => Some(entry.retry_at.saturating_sub(now)),
            FailureKind::Retryable => None,
        }
    }
    pub fn get(&self, key: &FrameKey) -> Option<PreviewFailure> {
        self.entries
            .iter()
            .find(|e| e.key == *key)
            .map(|e| e.failure.clone())
            .or_else(|| self.allocation_retry.map(|_| PreviewFailure::new(FailureKind::Retryable, "Could not retain preview failure history")))
    }
    pub fn succeeded(&mut self, key: &FrameKey) {
        self.entries.retain(|e| e.key != *key);
        self.allocation_retry = None;
    }
    pub fn recover(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        self.entries.clear();
        self.allocation_retry = None;
    }
}

/// A previous frame may be retained across a content/time edit, but never across comp,
/// camera or render-option changes (including ROI). Scale changes may reuse its pixels.
pub fn same_view(requested: &FrameKey, shown: &FrameKey) -> bool {
    requested.comp == shown.comp && requested.view == shown.view && requested.opts == shown.opts
}

#[cfg(test)]
mod tests {
    use super::*;
    fn key(frame: i64) -> FrameKey {
        FrameKey { revision: 1, content: 1, comp: 1, frame, scale: 1000, view: 0, opts: 0 }
    }
    #[test]
    fn unknown_failures_back_off_then_recover_and_typed_content_waits_for_retry() {
        let mut f = Failures::default();
        let k = key(0);
        for attempt in 0..16 {
            let now = Duration::from_secs(attempt * 10);
            f.record(k, 0, PreviewFailure::new(FailureKind::Retryable, "temporary"), now);
            let delay = f.delay(&k, now).unwrap();
            assert!(delay >= Duration::from_millis(250) && delay <= Duration::from_secs(8));
            assert!(f.delay(&k, now + delay).is_none());
        }
        f.record(k, 0, PreviewFailure::new(FailureKind::Content, "bad radius"), Duration::ZERO);
        assert_eq!(f.delay(&k, Duration::from_secs(100_000)), Some(Duration::MAX));
        assert!(f.delay(&key(1), Duration::ZERO).is_none());
        f.recover();
        f.record(k, 0, PreviewFailure::new(FailureKind::Content, "late failure"), Duration::ZERO);
        assert!(f.get(&k).is_none(), "old callback cannot poison retry/recovery");
    }
    #[test]
    fn failure_history_and_unicode_messages_have_fixed_bounds() {
        let mut f = Failures::default();
        for i in 0..1024 {
            f.record(key(i), 0, PreviewFailure::new(FailureKind::Retryable, &"é".repeat(400)), Duration::ZERO);
        }
        assert_eq!(f.entries.len(), MAX_FAILURES);
        assert!(f.get(&key(0)).is_none());
        assert_eq!(f.get(&key(1023)).unwrap().message().len(), 512);
        f.succeeded(&key(1023));
        assert!(f.get(&key(1023)).is_none());
    }
}
