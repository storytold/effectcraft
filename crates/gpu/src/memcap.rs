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
//! device type, and lower on a discrete card whose memory the OS reports without `unsafe`
//! (Linux's amdgpu driver, in sysfs: a 2–4 GB card ran out under the 8 GiB default and took
//! the system down, #1487); `EFFECTCRAFT_GPU_MEMCAP_MB` overrides it (0 = off). Backends
//! without an allocator report (Metal, WebGPU, GL) are never gated.

use std::path::Path;
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
        let cap = env.map_or_else(|| default_cap(info.device_type, vram(info)), |mb| mb << 20);
        MemCap { cap, calls: AtomicU64::new(0), waits: AtomicU64::new(0) }
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
/// driver and other apps, and at most three quarters of the card's memory when it is known
/// (smaller cards elsewhere: set `EFFECTCRAFT_GPU_MEMCAP_MB`). Integrated and software
/// adapters share system memory: 2 GiB.
fn default_cap(t: wgpu::DeviceType, vram: Option<u64>) -> u64 {
    match t {
        wgpu::DeviceType::DiscreteGpu => vram.map_or(8 * GIB, |v| (v / 4).saturating_mul(3).min(8 * GIB)),
        _ => 2 * GIB,
    }
}

/// The adapter's dedicated memory in bytes, where the OS reports it without `unsafe`.
fn vram(info: &wgpu::AdapterInfo) -> Option<u64> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let bytes = vram_in_sysfs(Path::new("/sys/class/drm"), info.vendor, info.device);
    if let Some(b) = bytes {
        log::info!("gpu {}: {} MiB of video memory (sysfs)", info.name, b >> 20);
    }
    bytes
}

/// `mem_info_vram_total` (amdgpu) of the DRM card under `drm` whose PCI vendor and device ids
/// are `vendor` and `device`.
fn vram_in_sysfs(drm: &Path, vendor: u32, device: u32) -> Option<u64> {
    let id = |path: &Path| std::fs::read_to_string(path).ok().and_then(|s| u32::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok());
    let mut cards: Vec<_> = std::fs::read_dir(drm)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("card") && !n.contains('-')))
        .collect();
    cards.sort();
    cards.iter().find_map(|card| {
        let dev = card.join("device");
        if id(&dev.join("vendor")) != Some(vendor) || id(&dev.join("device")) != Some(device) {
            return None;
        }
        std::fs::read_to_string(dev.join("mem_info_vram_total")).ok()?.trim().parse::<u64>().ok().filter(|&b| b > 0)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_cadence() {
        assert_eq!(default_cap(wgpu::DeviceType::DiscreteGpu, None), 8 * GIB);
        assert_eq!(default_cap(wgpu::DeviceType::IntegratedGpu, None), 2 * GIB);
        let m = MemCap { cap: GIB, calls: AtomicU64::new(0), waits: AtomicU64::new(0) };
        let due = (0..2 * EVERY).filter(|_| m.due().is_some()).count();
        assert_eq!(due, 2);
        let off = MemCap { cap: 0, calls: AtomicU64::new(0), waits: AtomicU64::new(0) };
        assert!((0..EVERY).all(|_| off.due().is_none()));
    }

    /// #1487: a 4 GB RX 550 gets a 3 GiB cap, not the 8 GiB that let it run out; a 16 GB card
    /// keeps 8 GiB, and an integrated GPU's carve-out doesn't lower its shared-memory cap.
    #[test]
    fn known_video_memory_lowers_the_discrete_cap() {
        assert_eq!(default_cap(wgpu::DeviceType::DiscreteGpu, Some(4 * GIB)), 3 * GIB);
        assert_eq!(default_cap(wgpu::DeviceType::DiscreteGpu, Some(2 * GIB)), 3 * GIB / 2);
        assert_eq!(default_cap(wgpu::DeviceType::DiscreteGpu, Some(16 * GIB)), 8 * GIB);
        assert_eq!(default_cap(wgpu::DeviceType::IntegratedGpu, Some(GIB / 2)), 2 * GIB);
    }

    #[test]
    fn video_memory_is_read_from_the_matching_drm_card() {
        let dir = std::env::temp_dir().join(format!("effectcraft-memcap-{}", std::process::id()));
        let card = |name: &str, vendor: &str, device: &str, vram: Option<&str>| {
            let dev = dir.join(name).join("device");
            std::fs::create_dir_all(&dev).unwrap();
            std::fs::write(dev.join("vendor"), vendor).unwrap();
            std::fs::write(dev.join("device"), device).unwrap();
            if let Some(v) = vram {
                std::fs::write(dev.join("mem_info_vram_total"), v).unwrap();
            }
        };
        // An integrated Intel GPU (no amdgpu file), the RX 550 and a connector entry.
        card("card0", "0x8086\n", "0x9bc5\n", None);
        card("card1", "0x1002\n", "0x699f\n", Some("4294967296\n"));
        std::fs::create_dir_all(dir.join("card1-DP-1")).unwrap();
        assert_eq!(vram_in_sysfs(&dir, 0x1002, 0x699f), Some(4 * GIB));
        assert_eq!(vram_in_sysfs(&dir, 0x8086, 0x9bc5), None, "no mem_info_vram_total");
        assert_eq!(vram_in_sysfs(&dir, 0x1002, 0x73bf), None, "no such card");
        assert_eq!(vram_in_sysfs(&dir.join("missing"), 0x1002, 0x699f), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
