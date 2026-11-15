//! Memory back-pressure for frames in flight.
//!
//! wgpu destroys a texture or buffer we drop only once the submissions that used it have
//! completed, and pooled textures released while other frames' encoders are open stay pending
//! until then. With several 4K frames in flight, the frames record work faster than the GPU
//! retires it, and freed-but-not-destroyed memory runs the device out of memory (#879). Once the
//! allocator holds more than the cap and enough of that total is reclaimable (not held by the
//! upload cache, the texture pool, or live images), a frame submits its encoder and waits for its
//! own submission so wgpu frees what was dropped. Frames are not held back for memory that cannot
//! be reclaimed; a warning is logged once.
//!
//! wgpu has no safe query for the adapter's memory size (`max_buffer_size` is capped at `i32::MAX`
//! on Direct3D 12 and Linux/Mesa Vulkan), so the default cap is conservative per device type,
//! and lower on a discrete card whose memory the OS reports without `unsafe` (Linux's amdgpu
//! driver, in sysfs, and the display class key of the registry on Windows: a 2–4 GB card ran
//! out of memory and took the system down, #1487);
//! `EFFECTCRAFT_GPU_MEMCAP_MB` overrides it (0 = off). Backends without an allocator report
//! (Metal, WebGPU, GL) are never gated.
//!
//! The upload cache (textures of layers already uploaded) has a budget of its own, three eighths
//! of the card's memory when that is known and 1.5 GiB otherwise (`EFFECTCRAFT_GPU_UPLOAD_MB`
//! overrides it): a comp whose layers are larger than the budget re-uploads all of them every
//! frame (#3731). The cache counts against the cap, so the default cap grows with the budget
//! (never above three quarters of the card) and frames in flight are not starved by it.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::time::Duration;

/// Check the allocator only every this many gate calls (effect calls and layers): the report
/// lists every allocation, and between checks a frame adds at most a few textures.
const EVERY: u64 = 8;

const GIB: u64 = 1 << 30;
const MIB: u64 = 1 << 20;

/// Overrides the default cap: whole MiB, 0 turns the gate off.
const MEMCAP_ENV: &str = "EFFECTCRAFT_GPU_MEMCAP_MB";

/// Overrides the default upload cache budget: whole MiB, 0 = the default.
const UPLOAD_ENV: &str = "EFFECTCRAFT_GPU_UPLOAD_MB";

/// The upload cache budget when the card's memory is not known (the default cap leaves room for it).
const UPLOAD_DEFAULT: u64 = GIB + GIB / 2;

pub(crate) struct MemCap {
    cap: u64,
    /// Bytes of uploaded layer textures to keep (see [`upload_budget`]).
    upload: usize,
    hysteresis: u64,
    calls: AtomicU64,
    waits: AtomicU64,
    /// Reclaimable bytes remaining after the last wait: another wait fires only once reclaimable
    /// memory grows by `hysteresis` above this floor (or drops below it first).
    floor: AtomicU64,
    /// Set after the first over-cap sample where non-reclaimable memory alone exceeds the cap.
    warned: AtomicBool,
    /// The most the allocator held in any sample (what frames in flight are sized by, #3335).
    peak: AtomicU64,
    #[cfg(test)]
    timeout_ns: AtomicU64,
}

impl MemCap {
    pub(crate) fn new(info: &wgpu::AdapterInfo) -> MemCap {
        let env = std::env::var(MEMCAP_ENV).ok();
        let upload = std::env::var(UPLOAD_ENV).ok();
        Self::from_envs(env.as_deref(), upload.as_deref(), info.device_type, vram(info))
    }

    #[cfg(test)]
    fn from_env(var: Option<&str>, device_type: wgpu::DeviceType, vram: Option<u64>) -> MemCap {
        Self::from_envs(var, None, device_type, vram)
    }

    fn from_envs(cap_var: Option<&str>, upload_var: Option<&str>, device_type: wgpu::DeviceType, vram: Option<u64>) -> MemCap {
        let mut upload = upload_budget(upload_var, vram);
        // An explicit cap is the user's: it does not grow with the cache, so a default budget
        // is lowered to half of it (never below the former fixed 1.5 GiB); an explicit one stays.
        let cap = match cap_var {
            Some(_) => {
                let cap = cap_from_env(cap_var, device_type, vram);
                if cap > 0 && explicit_upload(upload_var).is_none() {
                    upload = upload.min((cap / 2).max(UPLOAD_DEFAULT));
                }
                cap
            }
            None => default_cap_for(device_type, vram, upload),
        };
        MemCap {
            cap,
            upload: usize::try_from(upload).unwrap_or(usize::MAX),
            hysteresis: default_hysteresis(cap),
            calls: AtomicU64::new(0),
            waits: AtomicU64::new(0),
            floor: AtomicU64::new(0),
            warned: AtomicBool::new(false),
            peak: AtomicU64::new(0),
            #[cfg(test)]
            timeout_ns: AtomicU64::new(crate::readback::TIMEOUT.as_nanos() as u64),
        }
    }

    /// The same cap with fresh counters (a test context sharing another's device).
    #[cfg(test)]
    pub(crate) fn fresh(&self) -> MemCap {
        MemCap {
            cap: self.cap,
            upload: self.upload,
            hysteresis: self.hysteresis,
            calls: AtomicU64::new(0),
            waits: AtomicU64::new(0),
            floor: AtomicU64::new(0),
            warned: AtomicBool::new(false),
            peak: AtomicU64::new(0),
            timeout_ns: AtomicU64::new(self.timeout_ns.load(Relaxed)),
        }
    }

    pub(crate) fn cap(&self) -> u64 {
        self.cap
    }

    /// Bytes of uploaded layer textures the context keeps.
    pub(crate) fn upload_budget(&self) -> usize {
        self.upload
    }

    /// Remember an allocator sample: the peak only grows.
    pub(crate) fn note(&self, allocated: u64) {
        self.peak.fetch_max(allocated, Relaxed);
    }

    /// The most the allocator held in any sample so far.
    pub(crate) fn peak(&self) -> u64 {
        self.peak.load(Relaxed)
    }

    /// Whether frames have had to wait for memory (the device ran over the cap).
    pub(crate) fn pressed(&self) -> bool {
        self.waits.load(Relaxed) > 0
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
        if kept > self.cap && !self.warned.swap(true, Relaxed) {
            log::warn!(
                target: "effectcraft_gpu",
                "gpu: {} MiB held by live frames, the upload cache and the texture pool is over the {} MiB memory cap: waiting cannot free it, so frames are not held back for it ({MEMCAP_ENV} sets the cap)",
                kept >> 20,
                self.cap >> 20,
            );
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
    pub(crate) fn warned(&self) -> bool {
        self.warned.load(Relaxed)
    }

    #[cfg(test)]
    pub(crate) fn set_wait_timeout(&self, timeout: Duration) {
        self.timeout_ns.store(timeout.as_nanos() as u64, Relaxed);
    }
}

/// The cap from the `EFFECTCRAFT_GPU_MEMCAP_MB` value: whole MiB (0 turns the gate off), or the
/// default for `device_type` when unset, or (with a warning) when not a number or too large.
fn cap_from_env(var: Option<&str>, device_type: wgpu::DeviceType, vram: Option<u64>) -> u64 {
    let Some(v) = var else { return default_cap(device_type, vram) };
    match v.trim().parse::<u64>().ok().and_then(|mb| mb.checked_mul(MIB)) {
        Some(cap) => cap,
        None => {
            let cap = default_cap(device_type, vram);
            log::warn!(target: "effectcraft_gpu", "{MEMCAP_ENV}={v:?} is not a whole number of MiB that fits in 64 bits: using {} MiB", cap / MIB);
            cap
        }
    }
}

/// The upload cache budget from the `EFFECTCRAFT_GPU_UPLOAD_MB` value: whole MiB, or (0, unset, or
/// with a warning not a number) three eighths of the card's memory when known (`vram`: 4 GB gives
/// the former fixed 1.5 GiB, a 32 GB card 12 GiB) and 1.5 GiB otherwise.
fn upload_budget(var: Option<&str>, vram: Option<u64>) -> u64 {
    let default = vram.map_or(UPLOAD_DEFAULT, |v| v / 8 * 3);
    if let Some(v) = var
        && explicit_upload(var).is_none()
        && v.trim() != "0"
    {
        log::warn!(target: "effectcraft_gpu", "{UPLOAD_ENV}={v:?} is not a whole number of MiB that fits in 64 bits: using {} MiB", default / MIB);
    }
    explicit_upload(var).unwrap_or(default)
}

/// The budget the user set: whole MiB above 0.
fn explicit_upload(var: Option<&str>) -> Option<u64> {
    var?.trim().parse::<u64>().ok().and_then(|mb| mb.checked_mul(MIB)).filter(|&b| b > 0)
}

/// The default cap plus what the upload cache holds beyond the 1.5 GiB the default cap already
/// leaves room for, never above three quarters of the card's memory when that is known.
fn default_cap_for(t: wgpu::DeviceType, vram: Option<u64>, upload: u64) -> u64 {
    let base = default_cap(t, vram);
    let cap = base.saturating_add(upload.saturating_sub(UPLOAD_DEFAULT));
    match vram {
        Some(v) if t == wgpu::DeviceType::DiscreteGpu => cap.min((v / 4).saturating_mul(3)).max(base),
        _ => cap,
    }
}

/// Discrete cards: 3 GiB, low enough to fire on 4–8 GB cards with headroom for the driver and
/// desktop while leaving ~1 GiB above the default 1.5 GiB upload cache + 256 MB pool (a bigger
/// upload cache raises it, see [`default_cap_for`]; `EFFECTCRAFT_GPU_MEMCAP_MB` sets it), and at most three quarters of the card's
/// memory when it is known (`vram`), so a 2 GB card gets 1.5 GiB (#1487). Integrated and software
/// adapters share system memory: 2 GiB, whatever their carve-out.
fn default_cap(t: wgpu::DeviceType, vram: Option<u64>) -> u64 {
    match t {
        wgpu::DeviceType::DiscreteGpu => vram.map_or(3 * GIB, |v| (v / 4).saturating_mul(3).min(3 * GIB)),
        _ => 2 * GIB,
    }
}

/// The adapter's dedicated memory in bytes, where the OS reports it without `unsafe`: Linux's
/// amdgpu driver in sysfs, the display class key of the registry on Windows.
fn vram(info: &wgpu::AdapterInfo) -> Option<u64> {
    if info.device_type != wgpu::DeviceType::DiscreteGpu {
        return None;
    }
    #[cfg(target_os = "linux")]
    let (bytes, source) = (vram_in_sysfs(Path::new("/sys/class/drm"), info.vendor, info.device, &info.device_pci_bus_id), "sysfs");
    #[cfg(windows)]
    let (bytes, source) = (vram_in_registry(info.vendor, info.device), "registry");
    #[cfg(not(any(target_os = "linux", windows)))]
    let (bytes, source) = (None::<u64>, "");
    if let Some(b) = bytes {
        log::info!(target: "effectcraft_gpu", "gpu {}: {} MiB of video memory ({source})", info.name, b >> 20);
    }
    bytes
}

/// `HardwareInformation.qwMemorySize` of the display adapters under the display class key whose
/// `MatchingDeviceId` names the PCI vendor and device (drivers write it as a 64-bit value, so a
/// 32 GB card reads right). Variants of one model share the ids: the smallest wins.
#[cfg(windows)]
fn vram_in_registry(vendor: u32, device: u32) -> Option<u64> {
    use windows_registry::LOCAL_MACHINE;
    let class = LOCAL_MACHINE.open(r"SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}").ok()?;
    let entries: Vec<(String, u64)> = class
        .keys()
        .ok()?
        .filter_map(|name| {
            let key = class.open(&name).ok()?;
            Some((key.get_string("MatchingDeviceId").ok()?, key.get_u64("HardwareInformation.qwMemorySize").ok()?))
        })
        .collect();
    vram_from_entries(&entries, vendor, device)
}

/// The smallest nonzero memory among `(MatchingDeviceId, bytes)` entries of the PCI device
/// `vendor`:`device` (ids look like `pci\ven_10de&dev_2b85&subsys_...`).
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
fn vram_from_entries(entries: &[(String, u64)], vendor: u32, device: u32) -> Option<u64> {
    let want = format!("ven_{vendor:04x}&dev_{device:04x}");
    entries.iter().filter(|(id, bytes)| *bytes > 0 && id.to_ascii_lowercase().contains(&want)).map(|&(_, b)| b).min()
}

/// `mem_info_vram_total` (amdgpu) of the DRM card under `drm` whose PCI vendor and device ids are
/// `vendor` and `device`. Connector entries (`card1-DP-1`) are skipped. Variants of one model share
/// those ids (the RX 550 ships with 2 and 4 GB), so the card at PCI address `bus` (`uevent`'s
/// `PCI_SLOT_NAME`, wgpu's `device_pci_bus_id`) wins; without one, the smallest matching card.
#[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
fn vram_in_sysfs(drm: &Path, vendor: u32, device: u32, bus: &str) -> Option<u64> {
    let id = |path: &Path| std::fs::read_to_string(path).ok().and_then(|s| u32::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok());
    let slot = |dev: &Path| {
        std::fs::read_to_string(dev.join("uevent")).ok()?.lines().find_map(|l| l.strip_prefix("PCI_SLOT_NAME=").map(|s| s.trim().to_ascii_lowercase()))
    };
    let mut cards: Vec<_> = std::fs::read_dir(drm)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("card") && !n.contains('-')))
        .collect();
    cards.sort();
    let found: Vec<(Option<String>, u64)> = cards
        .iter()
        .filter_map(|card| {
            let dev = card.join("device");
            if id(&dev.join("vendor")) != Some(vendor) || id(&dev.join("device")) != Some(device) {
                return None;
            }
            let bytes = std::fs::read_to_string(dev.join("mem_info_vram_total")).ok()?.trim().parse::<u64>().ok().filter(|&b| b > 0)?;
            Some((slot(&dev), bytes))
        })
        .collect();
    let bus = bus.trim().to_ascii_lowercase();
    found.iter().find(|(s, _)| !bus.is_empty() && s.as_deref() == Some(bus.as_str())).map(|&(_, b)| b).or_else(|| found.iter().map(|&(_, b)| b).min())
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
        assert_eq!(default_cap(wgpu::DeviceType::DiscreteGpu, None), 3 * GIB);
        assert_eq!(default_cap(wgpu::DeviceType::IntegratedGpu, None), 2 * GIB);
        let m = MemCap::from_env(None, wgpu::DeviceType::IntegratedGpu, None);
        assert_eq!(m.cap(), 2 * GIB);
        let due = (0..2 * EVERY).filter(|_| m.due()).count();
        assert_eq!(due, 2);
        let off = MemCap::from_env(Some("0"), wgpu::DeviceType::DiscreteGpu, None);
        assert_eq!(off.cap(), 0);
        assert!((0..EVERY).all(|_| !off.due()));
    }

    #[test]
    fn override_is_whole_mib_or_the_default() {
        let cap = |v| MemCap::from_env(Some(v), wgpu::DeviceType::DiscreteGpu, None).cap();
        assert_eq!(cap("2048"), 2 * GIB);
        assert_eq!(cap(" 8 "), 8 * MIB);
        assert_eq!(cap("17592186044415"), 17592186044415 * MIB, "the largest value that fits");
        for bad in ["", "lots", "-1", "1.5", "17592186044416", "99999999999999999999"] {
            assert_eq!(cap(bad), 3 * GIB, "{bad:?} falls back to the default");
        }
    }

    #[test]
    fn hysteresis_skips_non_reclaimable_and_unfreed_memory() {
        let m = MemCap::from_env(Some("2048"), wgpu::DeviceType::IntegratedGpu, None);
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

    #[test]
    fn kept_memory_over_the_cap_warns_once_and_never_waits() {
        let m = MemCap::from_env(Some("8"), wgpu::DeviceType::IntegratedGpu, None);
        // Cap 8 MiB: kept alone over the cap with reclaimable under the hysteresis → no wait, warn once.
        assert!(!m.should_wait(20 * MIB, 20 * MIB));
        assert!(m.warned());
        assert!(!m.should_wait(20 * MIB, 20 * MIB));
        assert!(!m.should_wait(20 * MIB, 19 * MIB + MIB / 2));
        assert!(m.should_wait(20 * MIB, 4 * MIB), "16 MiB reclaimable is over the hysteresis");
    }

    /// #1487: a 2–4 GB card gets three quarters of its memory instead of the 3 GiB default; a
    /// larger card keeps the default, and an integrated GPU's carve-out doesn't lower its cap.
    #[test]
    fn known_video_memory_lowers_the_discrete_cap() {
        let d = wgpu::DeviceType::DiscreteGpu;
        assert_eq!(default_cap(d, Some(2 * GIB)), 3 * GIB / 2);
        assert_eq!(default_cap(d, Some(4 * GIB)), 3 * GIB);
        assert_eq!(default_cap(d, Some(16 * GIB)), 3 * GIB, "never above the default");
        assert_eq!(default_cap(wgpu::DeviceType::IntegratedGpu, Some(GIB / 2)), 2 * GIB);
        assert_eq!(cap_from_env(None, d, Some(2 * GIB)), 3 * GIB / 2);
        assert_eq!(cap_from_env(Some("lots"), d, Some(2 * GIB)), 3 * GIB / 2, "a bad override falls back to it");
        assert_eq!(cap_from_env(Some("4096"), d, Some(2 * GIB)), 4 * GIB, "the override still wins");
    }

    #[test]
    fn video_memory_is_matched_by_pci_ids_in_registry_entries() {
        let e = |id: &str, gb: u64| (id.to_string(), gb * GIB);
        let entries = [e(r"PCI\VEN_10DE&DEV_2B85&SUBSYS_1", 32), e(r"PCI\VEN_8086&DEV_46A6&SUBSYS_2", 0), e(r"PCI\VEN_10DE&DEV_2204", 24)];
        assert_eq!(vram_from_entries(&entries, 0x10de, 0x2b85), Some(32 * GIB));
        assert_eq!(vram_from_entries(&entries, 0x8086, 0x46a6), None, "a zero size is not known");
        assert_eq!(vram_from_entries(&entries, 0x1002, 0x73bf), None);
        let twins = [e(r"PCI\VEN_1002&DEV_699F", 4), e(r"PCI\VEN_1002&DEV_699F&SUBSYS_9", 2)];
        assert_eq!(vram_from_entries(&twins, 0x1002, 0x699f), Some(2 * GIB), "the smaller variant");
    }

    /// #3731: the upload budget follows the card (3/8 of its memory), the environment overrides
    /// it, and the default cap leaves room for it.
    #[test]
    fn upload_budget_follows_the_card_and_the_cap_leaves_room() {
        let d = wgpu::DeviceType::DiscreteGpu;
        assert_eq!(upload_budget(None, None), 3 * GIB / 2, "unknown memory: the former fixed budget");
        assert_eq!(upload_budget(None, Some(4 * GIB)), 3 * GIB / 2);
        assert_eq!(upload_budget(None, Some(32 * GIB)), 12 * GIB);
        assert_eq!(upload_budget(None, Some(2 * GIB)), 768 * MIB);
        assert_eq!(upload_budget(Some("24000"), Some(2 * GIB)), 24000 * MIB, "the override wins");
        assert_eq!(upload_budget(Some("0"), Some(32 * GIB)), 12 * GIB, "0 is the default");
        for bad in ["", "lots", "-1", "99999999999999999999"] {
            assert_eq!(upload_budget(Some(bad), None), 3 * GIB / 2, "{bad:?}");
        }
        // The cap grows with the budget, never above three quarters of a known card.
        assert_eq!(default_cap_for(d, None, 3 * GIB / 2), 3 * GIB);
        assert_eq!(default_cap_for(d, Some(32 * GIB), 12 * GIB), 3 * GIB + 21 * GIB / 2);
        assert_eq!(default_cap_for(d, Some(32 * GIB), 40 * GIB), 24 * GIB);
        assert_eq!(default_cap_for(d, Some(2 * GIB), 768 * MIB), 3 * GIB / 2);
        assert_eq!(default_cap_for(wgpu::DeviceType::IntegratedGpu, None, 3 * GIB / 2), 2 * GIB);
        // A card of any size: the default budget and cap leave a quarter of the memory free.
        for gb in [4, 6, 8, 12, 16, 24, 32] {
            let v = gb * GIB;
            assert!(default_cap_for(d, Some(v), upload_budget(None, Some(v))) <= v / 4 * 3, "{gb} GB");
        }
        // An explicit cap is not changed by the budget; the context takes both.
        let m = MemCap::from_envs(Some("4096"), Some("9000"), d, Some(32 * GIB));
        assert_eq!((m.cap(), m.upload_budget()), (4 * GIB, 9000 * MIB as usize));
        // A default budget is lowered to half an explicit cap (not below 1.5 GiB), an explicit one
        // is kept, and a cap of 0 (gate off) changes nothing.
        let m = MemCap::from_envs(Some("8192"), None, d, Some(32 * GIB));
        assert_eq!((m.cap(), m.upload_budget()), (8 * GIB, 4 * GIB as usize));
        let m = MemCap::from_envs(Some("4096"), None, d, Some(32 * GIB));
        assert_eq!((m.cap(), m.upload_budget()), (4 * GIB, 2 * GIB as usize));
        let m = MemCap::from_envs(Some("8"), None, d, Some(32 * GIB));
        assert_eq!((m.cap(), m.upload_budget()), (8 * MIB, 3 * GIB as usize / 2));
        let m = MemCap::from_envs(Some("0"), None, d, Some(32 * GIB));
        assert_eq!((m.cap(), m.upload_budget()), (0, 12 * GIB as usize));
        let m = MemCap::from_envs(None, Some("9000"), d, Some(32 * GIB));
        assert_eq!((m.cap(), m.upload_budget()), (3 * GIB + 9000 * MIB - 3 * GIB / 2, 9000 * MIB as usize));
        assert_eq!(MemCap::from_env(None, d, None).upload_budget(), 3 * GIB as usize / 2);
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
        assert_eq!(vram_in_sysfs(&dir, 0x1002, 0x699f, ""), Some(4 * GIB));
        assert_eq!(vram_in_sysfs(&dir, 0x8086, 0x9bc5, ""), None, "no mem_info_vram_total");
        assert_eq!(vram_in_sysfs(&dir, 0x1002, 0x73bf, ""), None, "no such card");
        assert_eq!(vram_in_sysfs(&dir.join("missing"), 0x1002, 0x699f, ""), None);
        // A 2 GB RX 550 next to the 4 GB one: same ids, told apart by PCI address.
        card("card2", "0x1002\n", "0x699f\n", Some("2147483648\n"));
        std::fs::write(dir.join("card1/device/uevent"), "DRIVER=amdgpu\nPCI_SLOT_NAME=0000:01:00.0\n").unwrap();
        std::fs::write(dir.join("card2/device/uevent"), "DRIVER=amdgpu\nPCI_SLOT_NAME=0000:03:00.0\n").unwrap();
        assert_eq!(vram_in_sysfs(&dir, 0x1002, 0x699f, "0000:01:00.0"), Some(4 * GIB));
        assert_eq!(vram_in_sysfs(&dir, 0x1002, 0x699f, "0000:03:00.0"), Some(2 * GIB));
        assert_eq!(vram_in_sysfs(&dir, 0x1002, 0x699f, ""), Some(2 * GIB), "unknown address: the smaller card");
        assert_eq!(vram_in_sysfs(&dir, 0x1002, 0x699f, "0000:09:00.0"), Some(2 * GIB), "no such address: the smaller card");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
