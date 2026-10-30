//! Memory back-pressure for frames in flight.
//!
//! wgpu destroys a texture or buffer we drop only once the submissions that used it have
//! completed, and pooled textures released while other frames' encoders are open stay pending
//! until then. With several 4K frames in flight, the frames record work faster than the GPU
//! retires it, and freed-but-not-destroyed memory runs the device out of memory (#879). Once the
//! allocator holds more than the cap and enough of that total is reclaimable (not held by the
//! upload cache, the texture pool, or live images), a frame submits its encoder and waits for its
//! own submission so wgpu frees what was dropped.
//!
//! wgpu has no safe query for the adapter's memory size (`max_buffer_size` is capped at `i32::MAX`
//! on Direct3D 12 and Linux/Mesa Vulkan), so the default cap is conservative per device type;
//! `EFFECTCRAFT_GPU_MEMCAP_MB` overrides it (0 = off). Backends without an allocator report
//! (Metal, WebGPU, GL) are never gated.

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::time::Duration;

/// Check the allocator only every this many gate calls (effect calls and layers): the report
/// lists every allocation, and between checks a frame adds at most a few textures.
const EVERY: u64 = 8;

const GIB: u64 = 1 << 30;
const MIB: u64 = 1 << 20;

/// Overrides the default cap: whole MiB, 0 turns the gate off.
const MEMCAP_ENV: &str = "EFFECTCRAFT_GPU_MEMCAP_MB";

pub(crate) struct MemCap {
    cap: u64,
    hysteresis: u64,
    calls: AtomicU64,
    waits: AtomicU64,
    /// Reclaimable bytes remaining after the last wait: another wait fires only once reclaimable
    /// memory grows by `hysteresis` above this floor (or drops below it first).
    floor: AtomicU64,
    #[cfg(test)]
    timeout_ns: AtomicU64,
}

impl MemCap {
    pub(crate) fn new(info: &wgpu::AdapterInfo) -> MemCap {
        let env = std::env::var(MEMCAP_ENV).ok();
        Self::from_env(env.as_deref(), info.device_type)
    }

    fn from_env(var: Option<&str>, device_type: wgpu::DeviceType) -> MemCap {
        let cap = cap_from_env(var, device_type);
        MemCap {
            cap,
            hysteresis: default_hysteresis(cap),
            calls: AtomicU64::new(0),
            waits: AtomicU64::new(0),
            floor: AtomicU64::new(0),
            #[cfg(test)]
            timeout_ns: AtomicU64::new(crate::readback::TIMEOUT.as_nanos() as u64),
        }
    }

    /// The same cap with fresh counters (a test context sharing another's device).
    #[cfg(test)]
    pub(crate) fn fresh(&self) -> MemCap {
        MemCap {
            cap: self.cap,
            hysteresis: self.hysteresis,
            calls: AtomicU64::new(0),
            waits: AtomicU64::new(0),
            floor: AtomicU64::new(0),
            timeout_ns: AtomicU64::new(self.timeout_ns.load(Relaxed)),
        }
    }

    pub(crate) fn cap(&self) -> u64 {
        self.cap
    }

    /// Whether to sample the allocator on this call.
    pub(crate) fn due(&self) -> bool {
        self.cap != 0 && self.calls.fetch_add(1, Relaxed).is_multiple_of(EVERY)
    }

    /// Whether `allocated` bytes in the allocator with `kept` non-reclaimable bytes (upload
    /// cache, pool free list, live frames) should trigger a submit and wait.
    pub(crate) fn should_wait(&self, allocated: u64, kept: u64) -> bool {
        if self.cap == 0 || allocated <= self.cap {
            self.floor.store(0, Relaxed);
            return false;
        }
        let reclaimable = allocated.saturating_sub(kept);
        let floor = self.floor.load(Relaxed);
        if reclaimable < floor {
            self.floor.store(reclaimable, Relaxed);
            return false;
        }
        reclaimable >= floor.saturating_add(self.hysteresis)
    }

    pub(crate) fn record_wait(&self, allocated_after: u64, kept_after: u64) {
        self.floor.store(allocated_after.saturating_sub(kept_after), Relaxed);
        let n = self.waits.fetch_add(1, Relaxed);
        if n == 0 {
            log::info!("gpu: allocator over the {} MiB memory cap, frames wait for the GPU to catch up", self.cap >> 20);
        }
    }

    pub(crate) fn wait_timeout(&self) -> Duration {
        #[cfg(test)]
        {
            Duration::from_nanos(self.timeout_ns.load(Relaxed))
        }
        #[cfg(not(test))]
        {
            crate::readback::TIMEOUT
        }
    }

    #[cfg(test)]
    pub(crate) fn waits(&self) -> u64 {
        self.waits.load(Relaxed)
    }

    #[cfg(test)]
    pub(crate) fn set_wait_timeout(&self, timeout: Duration) {
        self.timeout_ns.store(timeout.as_nanos() as u64, Relaxed);
    }
}

/// The cap from the `EFFECTCRAFT_GPU_MEMCAP_MB` value: whole MiB (0 turns the gate off), or the
/// default for `device_type` when unset, or (with a warning) when not a number or too large.
fn cap_from_env(var: Option<&str>, device_type: wgpu::DeviceType) -> u64 {
    let Some(v) = var else { return default_cap(device_type) };
    match v.trim().parse::<u64>().ok().and_then(|mb| mb.checked_mul(MIB)) {
        Some(cap) => cap,
        None => {
            let cap = default_cap(device_type);
            log::warn!(target: "effectcraft_gpu", "{MEMCAP_ENV}={v:?} is not a whole number of MiB that fits in 64 bits: using {} MiB", cap / MIB);
            cap
        }
    }
}

/// Discrete cards: 3 GiB, low enough to fire on 4–8 GB cards with headroom for the driver and
/// desktop while leaving ~1 GiB above the default 1.5 GiB upload cache + 256 MB pool (larger
/// cards can raise it with `EFFECTCRAFT_GPU_MEMCAP_MB`). Integrated and software adapters share
/// system memory: 2 GiB.
fn default_cap(t: wgpu::DeviceType) -> u64 {
    match t {
        wgpu::DeviceType::DiscreteGpu => 3 * GIB,
        _ => 2 * GIB,
    }
}

/// Minimum reclaimable bytes before a wait is worth draining an encoder (1/8 of the cap,
/// clamped between 1 MiB and 256 MiB, or 1/4 of caps smaller than 4 MiB).
fn default_hysteresis(cap: u64) -> u64 {
    if cap == 0 { 0 } else { (cap / 8).clamp(MIB.min(cap / 4), 256 * MIB) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_cadence() {
        assert_eq!(default_cap(wgpu::DeviceType::DiscreteGpu), 3 * GIB);
        assert_eq!(default_cap(wgpu::DeviceType::IntegratedGpu), 2 * GIB);
        let m = MemCap::from_env(None, wgpu::DeviceType::IntegratedGpu);
        assert_eq!(m.cap(), 2 * GIB);
        let due = (0..2 * EVERY).filter(|_| m.due()).count();
        assert_eq!(due, 2);
        let off = MemCap::from_env(Some("0"), wgpu::DeviceType::DiscreteGpu);
        assert_eq!(off.cap(), 0);
        assert!((0..EVERY).all(|_| !off.due()));
    }

    #[test]
    fn override_is_whole_mib_or_the_default() {
        let cap = |v| MemCap::from_env(Some(v), wgpu::DeviceType::DiscreteGpu).cap();
        assert_eq!(cap("2048"), 2 * GIB);
        assert_eq!(cap(" 8 "), 8 * MIB);
        assert_eq!(cap("17592186044415"), 17592186044415 * MIB, "the largest value that fits");
        for bad in ["", "lots", "-1", "1.5", "17592186044416", "99999999999999999999"] {
            assert_eq!(cap(bad), 3 * GIB, "{bad:?} falls back to the default");
        }
    }

    #[test]
    fn hysteresis_skips_non_reclaimable_and_unfreed_memory() {
        let m = MemCap::from_env(Some("2048"), wgpu::DeviceType::IntegratedGpu);
        // Upload cache + pool + live frames (2.15 GiB) sit over the 2 GiB cap with only 50 MiB
        // reclaimable: waiting would free almost nothing, so the gate must stay quiet.
        let kept = 2150 * MIB;
        assert!(!m.should_wait(kept + 50 * MIB, kept));
        assert!(!m.should_wait(kept + 200 * MIB, kept));
        // Once 256 MiB of dropped textures pile up, the gate fires.
        assert!(m.should_wait(kept + 256 * MIB, kept));
        // If a wait freed nothing (e.g. held by another thread's open encoder or timed out),
        // the floor prevents firing again until another 256 MiB of reclaimable memory accumulates.
        m.record_wait(kept + 256 * MIB, kept);
        assert!(!m.should_wait(kept + 256 * MIB, kept));
        assert!(!m.should_wait(kept + 400 * MIB, kept));
        assert!(m.should_wait(kept + 512 * MIB, kept));
        // Once the other thread finishes and reclaimable memory drops, the floor drops with it.
        assert!(!m.should_wait(kept + 16 * MIB, kept));
        assert!(m.should_wait(kept + (16 + 256) * MIB, kept));
    }
}
