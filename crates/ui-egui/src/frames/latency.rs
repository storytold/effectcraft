//! Bounded local timestamps for viewer demand, independent of render-job priority.
use super::FrameKey;
use std::collections::{HashMap, VecDeque};
use web_time::Instant;

const MAX_TIMINGS: usize = 256;

/// Request-to-texture-install timing, not physical presentation or input latency.
#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewerSample {
    pub content: u64,
    pub comp: u64,
    pub frame: i64,
    pub scale: u32,
    pub view: u64,
    pub opts: u64,
    pub cached: bool,
    pub queue_ms: Option<f64>,
    /// Includes disk lookup/conversion and, remotely, scheduling and message transfer.
    pub preparation_ms: Option<f64>,
    pub installation_ms: Option<f64>,
    pub total_ms: f64,
}

#[derive(Clone, Copy, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewerTiming {
    pub pending: bool,
    pub last: Option<ViewerSample>,
}

struct Demand {
    key: FrameKey,
    requested: Instant,
    cached: bool,
    installed: bool,
}

#[derive(Default)]
struct JobTiming {
    started: Option<Instant>,
    ready: Option<Instant>,
}

#[derive(Default)]
pub(super) struct Latency {
    demand: Option<Demand>,
    jobs: HashMap<FrameKey, JobTiming>,
    order: VecDeque<FrameKey>,
    last: Option<ViewerSample>,
}

impl Latency {
    pub fn request(&mut self, key: FrameKey, cached: bool, at: Instant) {
        if self.demand.as_ref().is_some_and(|d| d.key == key) {
            return;
        }
        self.demand = Some(Demand { key, requested: at, cached, installed: false });
    }

    fn job(&mut self, key: FrameKey) -> &mut JobTiming {
        if !self.jobs.contains_key(&key) {
            self.order.push_back(key);
            while self.order.len() > MAX_TIMINGS {
                if let Some(old) = self.order.pop_front() {
                    self.jobs.remove(&old);
                }
            }
        }
        self.jobs.entry(key).or_default()
    }

    pub fn started(&mut self, key: FrameKey, at: Instant) {
        // A RAM hit may have been evicted between the demand and installation.
        if let Some(d) = &mut self.demand
            && d.key == key
            && !d.installed
        {
            d.cached = false;
        }
        *self.job(key) = JobTiming { started: Some(at), ready: None };
    }

    pub fn ready(&mut self, key: FrameKey, at: Instant) {
        self.job(key).ready = Some(at);
    }

    pub fn installed(&mut self, key: FrameKey, at: Instant) {
        let Some(d) = &mut self.demand else { return };
        if d.key != key || d.installed {
            return;
        }
        let ms = |end: Instant, start: Instant| end.saturating_duration_since(start).as_secs_f64() * 1000.0;
        let job = self.jobs.get(&key);
        let ready = if d.cached { Some(d.requested) } else { job.and_then(|j| j.ready).map(|t| t.max(d.requested)) };
        let started = if d.cached { Some(d.requested) } else { job.and_then(|j| j.started).map(|t| t.max(d.requested)) };
        self.last = Some(ViewerSample {
            content: key.content,
            comp: key.comp,
            frame: key.frame,
            scale: key.scale,
            view: key.view,
            opts: key.opts,
            cached: d.cached,
            queue_ms: started.map(|t| ms(t, d.requested)),
            preparation_ms: started.zip(ready).map(|(s, r)| ms(r, s)),
            installation_ms: ready.map(|r| ms(at, r)),
            total_ms: ms(at, d.requested),
        });
        d.installed = true;
    }

    pub fn snapshot(&self) -> ViewerTiming {
        ViewerTiming { pending: self.demand.as_ref().is_some_and(|d| !d.installed), last: self.last }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn key(content: u64) -> FrameKey {
        FrameKey { revision: 1, content, comp: 1, frame: 0, scale: 1000, view: 0, opts: 0 }
    }

    #[test]
    fn phases_duplicates_and_revision_only_changes() {
        let mut l = Latency::default();
        let t = Instant::now();
        let at = |ms| t + Duration::from_millis(ms);
        let k = key(1);
        l.request(k, false, at(0));
        l.request(FrameKey { revision: 2, ..k }, false, at(5));
        l.started(k, at(10));
        l.ready(k, at(30));
        assert!(l.snapshot().pending);
        assert!(l.snapshot().last.is_none());
        l.installed(k, at(45));
        l.installed(k, at(70));
        let s = l.snapshot().last.unwrap();
        assert_eq!((s.queue_ms, s.preparation_ms, s.installation_ms, s.total_ms), (Some(10.0), Some(20.0), Some(15.0), 45.0));
        assert!(!l.snapshot().pending);
    }

    #[test]
    fn prefetch_ram_hit_and_revisit_exclude_earlier_work() {
        let mut l = Latency::default();
        let t = Instant::now();
        let at = |ms| t + Duration::from_millis(ms);
        let k = key(1);
        l.started(k, at(0));
        l.request(k, false, at(15));
        l.ready(k, at(30));
        l.installed(k, at(40));
        let s = l.snapshot().last.unwrap();
        assert_eq!((s.queue_ms, s.preparation_ms, s.installation_ms, s.total_ms), (Some(0.0), Some(15.0), Some(10.0), 25.0));
        l.request(key(2), false, at(50));
        l.request(k, true, at(60));
        l.installed(k, at(65));
        let s = l.snapshot().last.unwrap();
        assert!(s.cached);
        assert_eq!((s.queue_ms, s.preparation_ms, s.installation_ms, s.total_ms), (Some(0.0), Some(0.0), Some(5.0), 5.0));
    }

    #[test]
    fn obsolete_frames_and_failed_installations_never_finish_latest_demand() {
        let mut l = Latency::default();
        let t = Instant::now();
        l.request(key(1), false, t);
        l.request(key(2), false, t);
        l.ready(key(1), t);
        l.installed(key(1), t);
        l.ready(key(2), t);
        // No successful texture installation has been acknowledged.
        assert!(l.snapshot().pending);
        assert!(l.snapshot().last.is_none());
        l.installed(key(2), t);
        l.installed(key(1), t);
        assert_eq!(l.snapshot().last.unwrap().content, 2);
    }

    #[test]
    fn evicted_ram_hit_keeps_demand_start_but_accounts_for_rerender() {
        let mut l = Latency::default();
        let t = Instant::now();
        let at = |ms| t + Duration::from_millis(ms);
        let k = FrameKey { scale: 500, view: 7, opts: 9, ..key(1) };
        l.request(k, true, at(0));
        l.started(k, at(10));
        l.ready(k, at(30));
        l.installed(k, at(40));
        let s = l.snapshot().last.unwrap();
        assert!(!s.cached);
        assert_eq!((s.scale, s.view, s.opts), (500, 7, 9));
        assert_eq!((s.queue_ms, s.preparation_ms, s.installation_ms, s.total_ms), (Some(10.0), Some(20.0), Some(10.0), 40.0));
    }

    #[test]
    fn missing_job_metadata_keeps_total_and_reports_unknown_phases() {
        let mut l = Latency::default();
        let t = Instant::now();
        l.request(key(1), false, t);
        l.installed(key(1), t + Duration::from_millis(12));
        let s = l.snapshot().last.unwrap();
        assert_eq!(s.total_ms, 12.0);
        assert_eq!((s.queue_ms, s.preparation_ms, s.installation_ms), (None, None, None));
    }

    #[test]
    fn bounded_metadata_and_purge_allow_fresh_same_key_demand() {
        let mut l = Latency::default();
        let t = Instant::now();
        for i in 0..1000 {
            l.started(key(i), t);
            l.ready(key(i), t);
        }
        assert_eq!(l.jobs.len(), MAX_TIMINGS);
        assert_eq!(l.order.len(), MAX_TIMINGS);
        l.request(key(999), true, t);
        l.installed(key(999), t);
        l = Latency::default();
        l.request(key(999), false, t);
        assert!(l.snapshot().pending);
        assert!(l.snapshot().last.is_none());
    }
}
