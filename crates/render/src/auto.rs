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
    stats: Mutex<HashMap<AutoKey, AutoStats>>,
    // Fixture-only chosen samples, not a production clock or performance measurement.
    #[cfg(test)]
    test_costs: Option<TestCosts>,
}

#[cfg(test)]
#[derive(Clone, Copy)]
struct TestCosts {
    key: AutoKey,
    cpu_ms: f64,
    gpu_ms: f64,
}

impl AutoPick {
    /// Drive real Renderer routing/recording with known samples for one exact key.
    /// Default histories and all non-test builds retain their measured elapsed costs.
    #[cfg(test)]
    pub(crate) fn with_test_costs(key: AutoKey, cpu_ms: f64, gpu_ms: f64) -> Self {
        Self { test_costs: Some(TestCosts { key, cpu_ms, gpu_ms }), ..Default::default() }
    }
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

    /// Render the frame for `key` on the GPU?
    pub fn choose(&self, key: AutoKey) -> bool {
        let Ok(mut m) = self.stats.lock() else { return true };
        if m.len() > Self::MAX_KEYS && !m.contains_key(&key) {
            m.clear();
        }
        let s = m.entry(key).or_default();
        // Warm up both sides, alternating, GPU first.
        let gpu_warm = s.declined || s.gpu_frames >= Self::WARMUP;
        if !gpu_warm && s.gpu_frames <= s.cpu_frames {
            return true;
        }
        if s.cpu_frames < Self::WARMUP {
            return false;
        }
        let gpu_faster = match (s.gpu_ms, s.cpu_ms) {
            (Some(g), Some(c)) => c >= g * Self::MARGIN,
            (g, _) => g.is_some(),
        };
        // Periodically re-measure the slower side.
        if s.since_probe >= Self::REPROBE {
            s.since_probe = 0;
            return !gpu_faster;
        }
        s.since_probe += 1;
        gpu_faster
    }

    /// A frame for `key` took `ms` on the GPU (`gpu`) or the CPU.
    ///
    /// The warm-up keeps the fastest of its counted frames; after that the average follows
    /// improvements quickly and slow-downs gradually, and a single frame counts at most twice
    /// the average (a stall of the machine does not flip the choice for a whole re-probe
    /// period).
    pub fn record(&self, key: AutoKey, gpu: bool, ms: f64) {
        #[cfg(test)]
        let ms = self.test_costs.filter(|costs| costs.key == key).map_or(ms, |costs| if gpu { costs.gpu_ms } else { costs.cpu_ms });
        let Ok(mut m) = self.stats.lock() else { return };
        let s = m.entry(key).or_default();
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
            let s = m.entry(key).or_default();
            s.declined = true;
            s.gpu_ms = None;
            s.since_probe = 0;
        }
    }

    /// The history of `key`.
    pub fn stats(&self, key: AutoKey) -> Option<AutoStats> {
        self.stats.lock().ok()?.get(&key).copied()
    }

    /// Forget all history (settings that change both paths' costs).
    pub fn reset(&self) {
        if let Ok(mut m) = self.stats.lock() {
            m.clear();
        }
    }
}
