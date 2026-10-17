//! Memory back-pressure for frames in flight.
//!
//! wgpu destroys a texture or buffer we drop only once the submissions that used it have
//! completed, and pooled textures released while other frames' encoders are open stay pending
//! until then. With several 4K frames in flight, the frames record work faster than the GPU
//! retires it, and freed-but-not-destroyed memory runs the device out of memory (#879). Once the
//! allocator holds more than the cap, a frame submits its encoder and waits for the queue, which
//! lets wgpu free what was dropped.
//!
//! wgpu has no safe query for the adapter's memory size, so the default cap is conservative per
//! device type; `EFFECTCRAFT_GPU_MEMCAP_MB` overrides it (0 = off). Backends without an
//! allocator report (Metal, WebGPU, GL) are never gated.

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

/// Check the allocator only every this many gate calls (effect calls and layers): the report
/// lists every allocation, and between checks a frame adds at most a few textures.
const EVERY: u64 = 8;

const GIB: u64 = 1 << 30;

pub(crate) struct MemCap {
    cap: u64,
    calls: AtomicU64,
    waits: AtomicU64,
}

impl MemCap {
    pub(crate) fn new(info: &wgpu::AdapterInfo) -> MemCap {
        let env = std::env::var("EFFECTCRAFT_GPU_MEMCAP_MB").ok().and_then(|v| v.trim().parse::<u64>().ok());
        MemCap { cap: env.map_or_else(|| default_cap(info.device_type), |mb| mb << 20), calls: AtomicU64::new(0), waits: AtomicU64::new(0) }
    }

    /// The cap to check against now, or `None` (off, or not this call).
    pub(crate) fn due(&self) -> Option<u64> {
        (self.cap != 0 && self.calls.fetch_add(1, Relaxed).is_multiple_of(EVERY)).then_some(self.cap)
    }

    pub(crate) fn waited(&self) {
        let n = self.waits.fetch_add(1, Relaxed);
        if n == 0 {
            log::info!("gpu: allocator over the {} MiB memory cap, frames wait for the GPU to catch up", self.cap >> 20);
        }
    }
}

/// Discrete cards: 8 GiB, under the 10–24 GiB of current cards with headroom for the
/// driver and other apps (smaller cards: set `EFFECTCRAFT_GPU_MEMCAP_MB`). Integrated and
/// software adapters share system memory: 2 GiB.
fn default_cap(t: wgpu::DeviceType) -> u64 {
    match t {
        wgpu::DeviceType::DiscreteGpu => 8 * GIB,
        _ => 2 * GIB,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_cadence() {
        assert_eq!(default_cap(wgpu::DeviceType::DiscreteGpu), 8 * GIB);
        assert_eq!(default_cap(wgpu::DeviceType::IntegratedGpu), 2 * GIB);
        let m = MemCap { cap: GIB, calls: AtomicU64::new(0), waits: AtomicU64::new(0) };
        let due = (0..2 * EVERY).filter(|_| m.due().is_some()).count();
        assert_eq!(due, 2);
        let off = MemCap { cap: 0, calls: AtomicU64::new(0), waits: AtomicU64::new(0) };
        assert!((0..EVERY).all(|_| off.due().is_none()));
    }
}
