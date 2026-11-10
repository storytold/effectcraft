//! [`Backend::Auto`](crate::Backend::Auto)'s per-comp choice between the CPU and the GPU
//! compositor, from measured frame times.
//!
//! A light comp (a couple of small layers) composites faster on the CPU, which only touches
//! the layers' bounds, than on the GPU, which works on whole frames and reads the result back;
//! heavy comps (many layers, 3D, GPU effects) are faster on the GPU. Neither a layer count nor
//! a pixel count predicts the crossover across machines, so Auto measures: per comp, output
//! scale and path (readback / viewer display), it keeps an exponentially weighted average of
//! the CPU and GPU frame times and renders on the faster one. Each side is warmed up first
//! (its first frame, with layer cache misses and texture uploads, is not counted; the fastest of
//! the next ones starts the average), and the slower side is re-measured every
//! [`AutoPick::REPROBE`] frames so the choice follows the comp as it changes.
//!
//! Frames render several at a time (a preview's lanes), and a frame is only measured once it
//! is done: so at most one GPU warm-up frame, or re-probe frame on either side, runs at once,
//! and the frames asked for meanwhile go to the other side. And a frame's time counts per
//! frame running on its side at once: twelve CPU frames that each take 200 ms side by side cost
//! about 17 ms a frame, against a GPU frame of 175 ms on its own. Otherwise every lane of a starting preview took a
//! warm-up frame on a GPU that turned out 2–4× slower (an integrated GPU), all at once.

use std::collections::HashMap;
use std::sync::Mutex;

use effectcraft_project::ItemId;

/// Which frames share timing history: one comp at one output scale on one path.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AutoKey {
    pub comp: ItemId,
    /// `RenderOpts::scale` bits.
    pub scale: u64,
    /// The viewer's display path (GPU frames stay on the device) rather than a readback.
    pub display: bool,
}

impl AutoKey {
    pub fn new(comp: ItemId, scale: f64, display: bool) -> AutoKey {
        AutoKey { comp, scale: scale.to_bits(), display }
    }
}

/// Timing history of one [`AutoKey`].
#[derive(Clone, Copy, Debug, Default)]
pub struct AutoStats {
    /// Averaged ms/frame (`None` until a counted sample).
    pub cpu_ms: Option<f64>,
    pub gpu_ms: Option<f64>,
    /// Frames measured on each side (the first is the uncounted warm-up).
    pub cpu_frames: u32,
    pub gpu_frames: u32,
    /// Frames since the side not chosen was last measured.
    pub since_probe: u32,
    /// The GPU declined the last frame it was asked for (the CPU rendered it).
    pub declined: bool,
}

/// Per-comp CPU / GPU timing history for [`Backend::Auto`](crate::Backend::Auto) (held by the
/// accelerator, see [`Accelerator::auto_pick`](crate::Accelerator::auto_pick)).
#[derive(Default)]
pub struct AutoPick {
    stats: Mutex<HashMap<AutoKey, Entry>>,
}

/// One key's history, when its running warm-up or re-probe frame started on each side, and
/// how many of its frames are running on each side (`[CPU, GPU]`).
#[derive(Default)]
struct Entry {
    stats: AutoStats,
    measuring: [Option<web_time::Instant>; 2],
    // Lowered by `record`, `declined` and `abandoned`: every frame asked for ends in one (an
    // [`AutoTicket`] makes sure of it).
    running: [u32; 2],
}

impl Entry {
    /// A warm-up or re-probe frame is running on `gpu`'s side. It counts as done once
    /// [`AutoPick::MEASURING_FOR`] has passed (a frame that failed without reporting back).
    fn measuring(&self, gpu: bool) -> bool {
        self.measuring[usize::from(gpu)].is_some_and(|t| t.elapsed() < AutoPick::MEASURING_FOR)
    }

    /// Render the next warm-up or re-probe frame on `gpu`'s side, unless one is running there:
    /// then the other side's (`!gpu`).
    fn measure(&mut self, gpu: bool) -> bool {
        if self.measuring(gpu) {
            return !gpu;
        }
        self.measuring[usize::from(gpu)] = Some(web_time::Instant::now());
        gpu
    }
}

impl AutoPick {
    /// Frames measured on each side before choosing (the first is not counted).
    pub const WARMUP: u32 = 3;
    /// Weight of a new sample in the running average (slower / faster than the average).
    pub const ALPHA: f64 = 0.3;
    pub const ALPHA_DOWN: f64 = 0.5;
    /// Re-measure the side not chosen after this many frames.
    pub const REPROBE: u32 = 48;
    /// The CPU wins only when clearly faster (ties and noise stay on the GPU).
    pub const MARGIN: f64 = 0.85;
    /// Keys remembered before the history is reset.
    const MAX_KEYS: usize = 1024;
    /// Frames counted as running on one side at most (see `Entry::running`).
    const MAX_RUNNING: u32 = 256;
    /// A warm-up or re-probe frame not reported back by then no longer holds its side.
    const MEASURING_FOR: std::time::Duration = std::time::Duration::from_secs(10);

    /// [`AutoPick::choose`] for a frame that ends where it started: the ticket reports back, and
    /// a ticket dropped without a time (an error, a retry elsewhere, a panic, a cancelled future)
    /// ends the frame with [`AutoPick::abandoned`].
    pub fn ticket(&self, key: AutoKey) -> AutoTicket<'_> {
        AutoTicket { pick: self, key, gpu: self.choose(key), open: true }
    }

    /// Render the frame for `key` on the GPU? Every frame asked for reports back with
    /// [`AutoPick::record`] (or [`AutoPick::declined`]), or ends with [`AutoPick::abandoned`]
    /// ([`AutoPick::ticket`] does that on its own).
    pub fn choose(&self, key: AutoKey) -> bool {
        let Ok(mut m) = self.stats.lock() else { return true };
        if m.len() > Self::MAX_KEYS && !m.contains_key(&key) {
            m.clear();
        }
        let e = m.entry(key).or_default();
        let gpu = Self::pick(e);
        let n = &mut e.running[usize::from(gpu)];
        *n = n.saturating_add(1).min(Self::MAX_RUNNING);
        gpu
    }

    fn pick(e: &mut Entry) -> bool {
        let s = e.stats;
        // Warm up both sides, alternating, CPU first: the first frames asked for are the
        // ones on screen, and the CPU is the side that is never slow to start (no pipelines
        // to build; an integrated GPU took 2 s for the frame at the playhead).
        let gpu_warm = s.declined || s.gpu_frames >= Self::WARMUP;
        if !gpu_warm && s.gpu_frames < s.cpu_frames {
            return e.measure(true);
        }
        // (CPU frames already render side by side, on lanes of their own.)
        if s.cpu_frames < Self::WARMUP {
            return false;
        }
        let gpu_faster = match (s.gpu_ms, s.cpu_ms) {
            (Some(g), Some(c)) => c >= g * Self::MARGIN,
            (g, _) => g.is_some(),
        };
        // Periodically re-measure the slower side (one frame at a time).
        if s.since_probe >= Self::REPROBE && !e.measuring(!gpu_faster) {
            e.stats.since_probe = 0;
            return e.measure(!gpu_faster);
        }
        e.stats.since_probe = s.since_probe.saturating_add(1);
        gpu_faster
    }

    /// A frame for `key` took `ms` on the GPU (`gpu`) or the CPU.
    ///
    /// The warm-up keeps the fastest of its counted frames; after that the average follows
    /// improvements quickly and slow-downs gradually, and a single frame counts at most twice
    /// the average (a stall of the machine does not flip the choice for a whole re-probe
    /// period).
    pub fn record(&self, key: AutoKey, gpu: bool, ms: f64) {
        let Ok(mut m) = self.stats.lock() else { return };
        let e = m.entry(key).or_default();
        e.measuring[usize::from(gpu)] = None;
        // Per frame running on this side alongside it (itself included).
        let n = &mut e.running[usize::from(gpu)];
        let ms = ms / f64::from((*n).max(1));
        *n = n.saturating_sub(1);
        let s = &mut e.stats;
        let (avg, frames) = if gpu { (&mut s.gpu_ms, &mut s.gpu_frames) } else { (&mut s.cpu_ms, &mut s.cpu_frames) };
        *frames = frames.saturating_add(1);
        if gpu {
            s.declined = false;
        }
        if *frames < 2 {
            return;
        }
        *avg = Some(match *avg {
            None => ms,
            Some(a) if *frames <= Self::WARMUP => a.min(ms),
            Some(a) if ms < a => a + (ms - a) * Self::ALPHA_DOWN,
            Some(a) => a + (ms.min(2.0 * a) - a) * Self::ALPHA,
        });
    }

    /// The GPU did not handle a frame for `key` (it rendered on the CPU instead).
    pub fn declined(&self, key: AutoKey) {
        if let Ok(mut m) = self.stats.lock() {
            let e = m.entry(key).or_default();
            e.measuring[1] = None;
            // The frame renders on the CPU instead (and reports back from there).
            e.running[1] = e.running[1].saturating_sub(1);
            e.running[0] = e.running[0].saturating_add(1).min(Self::MAX_RUNNING);
            let s = &mut e.stats;
            s.declined = true;
            s.gpu_ms = None;
            s.since_probe = 0;
        }
    }

    /// A frame for `key` asked of `gpu`'s side ends without a time (it failed, or was replaced
    /// before it finished). Without this its side stays counted as running, and every later
    /// frame there is timed as one of several running side by side: that side looks cheaper.
    // ponytail: clears the side's warm-up/probe mark even when another frame holds it; that
    // only lets the next probe start early.
    pub fn abandoned(&self, key: AutoKey, gpu: bool) {
        if let Ok(mut m) = self.stats.lock()
            && let Some(e) = m.get_mut(&key)
        {
            e.measuring[usize::from(gpu)] = None;
            let n = &mut e.running[usize::from(gpu)];
            *n = n.saturating_sub(1);
        }
    }

    /// The history of `key`.
    pub fn stats(&self, key: AutoKey) -> Option<AutoStats> {
        self.stats.lock().ok()?.get(&key).map(|e| e.stats)
    }

    /// Forget all history (settings that change both paths' costs).
    pub fn reset(&self) {
        if let Ok(mut m) = self.stats.lock() {
            m.clear();
        }
    }
}

/// One frame [`AutoPick`] chose a side for ([`AutoPick::ticket`]): it reports the frame's time
/// once, and ends the frame unmeasured when dropped without one.
#[must_use = "a ticket dropped at once ends its frame unmeasured"]
pub struct AutoTicket<'a> {
    pick: &'a AutoPick,
    key: AutoKey,
    gpu: bool,
    open: bool,
}

impl AutoTicket<'_> {
    /// The frame renders on the GPU.
    pub fn gpu(&self) -> bool {
        self.gpu
    }

    /// The frame took `ms` on its side.
    pub fn record(mut self, ms: f64) {
        self.open = false;
        self.pick.record(self.key, self.gpu, ms);
    }

    /// The GPU did not take the frame: it renders on the CPU, and the ticket reports from there.
    pub fn declined(&mut self) {
        if self.gpu {
            self.pick.declined(self.key);
            self.gpu = false;
        }
    }
}

impl Drop for AutoTicket<'_> {
    fn drop(&mut self) {
        if self.open {
            self.pick.abandoned(self.key, self.gpu);
        }
    }
}
