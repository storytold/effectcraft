//! Host information for Settings ▸ Memory & CPU (RAM reserved for other applications, reducing
//! caches when the system is low on memory) and Help ▸ System Compatibility Report.
//!
//! Pure Rust and dependency-free: the values come from the operating system's own reporting
//! (`/proc/meminfo` on Linux, `sysctl`/`vm_stat` on macOS, `wmic`/PowerShell on Windows). Every
//! query may fail (sandboxes, the web): callers treat `None` as "unknown" and keep their
//! configured budgets. The periodic reading behind the cache budgets runs on a background
//! thread ([`MemoryWatch`]): on Windows it starts PowerShell, which takes about a second.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use serde::Serialize;

/// Physical memory in bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct SysMemory {
    pub total: u64,
    /// Memory the system can hand out without swapping (free + reclaimable caches).
    pub available: u64,
}

impl SysMemory {
    /// The system is low on memory: less than 10% (or less than 1 GB) is available.
    pub fn is_low(&self) -> bool {
        self.total > 0 && (self.available < self.total / 10 || self.available < 1 << 30)
    }
}

/// The system's memory, read in the background: [`MemoryWatch::poll`] starts a reading (unless
/// one is running) and [`MemoryWatch::latest`] returns the last one without waiting.
#[derive(Clone, Default)]
pub struct MemoryWatch {
    latest: Arc<Mutex<Option<SysMemory>>>,
    busy: Arc<AtomicBool>,
}

impl MemoryWatch {
    /// Start a new reading in the background (no threads on the web: read inline, it is
    /// unknown there anyway).
    pub fn poll(&self) {
        if self.busy.swap(true, Ordering::AcqRel) {
            return;
        }
        let (latest, busy) = (self.latest.clone(), self.busy.clone());
        let read = move || {
            if let Some(m) = memory() {
                *latest.lock().unwrap_or_else(PoisonError::into_inner) = Some(m);
            }
            busy.store(false, Ordering::Release);
        };
        #[cfg(not(target_arch = "wasm32"))]
        if std::thread::Builder::new().name("ec-memory".into()).spawn(read).is_err() {
            self.busy.store(false, Ordering::Release);
        }
        #[cfg(target_arch = "wasm32")]
        read();
    }

    /// The last reading (`None` until one finished).
    pub fn latest(&self) -> Option<SysMemory> {
        *self.latest.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[allow(dead_code)]
fn run(cmd: &str, args: &[&str]) -> Option<String> {
    let mut c = std::process::Command::new(cmd);
    c.args(args);
    // A console program started from the windowed app would flash a console window.
    #[cfg(target_os = "windows")]
    std::os::windows::process::CommandExt::creation_flags(&mut c, 0x0800_0000); // CREATE_NO_WINDOW
    let out = c.output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).to_string())
}

/// Parse `/proc/meminfo` (kB values).
pub fn parse_meminfo(text: &str) -> Option<SysMemory> {
    let field = |name: &str| -> Option<u64> {
        let line = text.lines().find(|l| l.starts_with(name))?;
        let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
        Some(kb * 1024)
    };
    let total = field("MemTotal:")?;
    let available = field("MemAvailable:").or_else(|| Some(field("MemFree:")? + field("Cached:").unwrap_or(0)))?;
    Some(SysMemory { total, available })
}

/// Parse macOS `vm_stat` output: free + inactive + speculative + purgeable pages.
pub fn parse_vm_stat(text: &str, total: u64) -> Option<SysMemory> {
    let page: u64 = text.split("page size of ").nth(1)?.split_whitespace().next()?.parse().ok()?;
    let pages = |name: &str| -> u64 {
        text.lines()
            .find(|l| l.starts_with(name))
            .and_then(|l| l.rsplit(':').next())
            .and_then(|v| v.trim().trim_end_matches('.').parse::<u64>().ok())
            .unwrap_or(0)
    };
    let free = pages("Pages free") + pages("Pages inactive") + pages("Pages speculative") + pages("Pages purgeable");
    Some(SysMemory { total, available: (free * page).min(total) })
}

/// Parse `sysctl -n hw.physmem hw.pagesize vm.stats.vm.v_free_count vm.stats.vm.v_inactive_count`
/// (FreeBSD: one number per line, in that order).
pub fn parse_freebsd_sysctl(text: &str) -> Option<SysMemory> {
    let mut it = text.split_whitespace().map(|v| v.parse::<u64>().ok());
    let total = it.next()??;
    let page = it.next()??;
    let free = it.next()??.saturating_add(it.next()??);
    Some(SysMemory { total, available: free.saturating_mul(page).min(total) })
}

/// Total and available physical memory, when the system reports them.
pub fn memory() -> Option<SysMemory> {
    #[cfg(target_os = "linux")]
    {
        return parse_meminfo(&std::fs::read_to_string("/proc/meminfo").ok()?);
    }
    #[cfg(target_os = "macos")]
    {
        let total: u64 = run("sysctl", &["-n", "hw.memsize"])?.trim().parse().ok()?;
        return parse_vm_stat(&run("vm_stat", &[])?, total).or(Some(SysMemory { total, available: total }));
    }
    #[cfg(target_os = "freebsd")]
    {
        return parse_freebsd_sysctl(&run("sysctl", &["-n", "hw.physmem", "hw.pagesize", "vm.stats.vm.v_free_count", "vm.stats.vm.v_inactive_count"])?);
    }
    #[cfg(target_os = "windows")]
    {
        let text = run(
            "powershell",
            &["-NoProfile", "-Command", "$o=Get-CimInstance Win32_OperatingSystem; \"$($o.TotalVisibleMemorySize) $($o.FreePhysicalMemory)\""],
        )?;
        let mut it = text.split_whitespace().filter_map(|v| v.parse::<u64>().ok());
        return Some(SysMemory { total: it.next()? * 1024, available: it.next()? * 1024 });
    }
    #[allow(unreachable_code)]
    None
}

/// CPU model name.
pub fn cpu_name() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        let t = std::fs::read_to_string("/proc/cpuinfo").ok()?;
        return t.lines().find(|l| l.starts_with("model name")).and_then(|l| l.split(':').nth(1)).map(|s| s.trim().to_string());
    }
    #[cfg(target_os = "macos")]
    {
        return run("sysctl", &["-n", "machdep.cpu.brand_string"]).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    }
    #[cfg(target_os = "freebsd")]
    {
        return run("sysctl", &["-n", "hw.model"]).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    }
    #[cfg(target_os = "windows")]
    {
        return std::env::var("PROCESSOR_IDENTIFIER").ok();
    }
    #[allow(unreachable_code)]
    None
}

/// Operating system name and version.
pub fn os_version() -> String {
    #[cfg(target_os = "macos")]
    if let Some(v) = run("sw_vers", &["-productVersion"]) {
        return format!("macOS {}", v.trim());
    }
    #[cfg(target_os = "linux")]
    if let Ok(t) = std::fs::read_to_string("/etc/os-release")
        && let Some(n) = t.lines().find_map(|l| l.strip_prefix("PRETTY_NAME="))
    {
        return n.trim_matches('"').to_string();
    }
    #[cfg(target_os = "freebsd")]
    if let Some(v) = run("freebsd-version", &[]) {
        return format!("FreeBSD {}", v.trim());
    }
    #[cfg(target_os = "windows")]
    if let Some(v) = run("cmd", &["/C", "ver"]) {
        return v.trim().to_string();
    }
    format!("{} ({})", std::env::consts::OS, std::env::consts::ARCH)
}

/// Logical CPU cores.
pub fn cpu_cores() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1)
}

/// The graphics lines of Settings ▸ GPU Information and Help ▸ System Report: whether the GPU
/// compositor runs (and on which adapter), else why compositing is on the CPU, and the adapter
/// the window itself got. Without this a card the compositor couldn't use looked undetected (#596).
pub fn gpu_summary(s: &crate::Session) -> String {
    let mut text = match s.accel.as_ref().map(|a| a.name()) {
        Some(name) => format!("Compositing: GPU, {name}.\n"),
        None => {
            let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
            let why = s.accel_note.as_deref().unwrap_or("no reason was recorded");
            format!("Compositing: CPU, {threads} threads (pure Rust, rayon).\nGPU compositing is off: {why}\n")
        }
    };
    match &s.window_adapter {
        Some(adapter) => text.push_str(&format!("Window graphics: {adapter}\n")),
        None => text.push_str("Window graphics: not known yet (the window has not drawn its first frame).\n"),
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freebsd_sysctl_memory_parses() {
        let m = parse_freebsd_sysctl("17179869184\n4096\n1000000\n500000\n").expect("parses");
        assert_eq!(m.total, 17_179_869_184);
        assert_eq!(m.available, 1_500_000 * 4096);
        assert!(parse_freebsd_sysctl("17179869184\n4096\n").is_none());
        assert!(parse_freebsd_sysctl("garbage").is_none());
        // Available never exceeds total, even with nonsense counts.
        let m = parse_freebsd_sysctl("1000\n4096\n999999999\n999999999").expect("parses");
        assert_eq!(m.available, 1000);
    }

    #[test]
    fn parses_proc_meminfo_and_vm_stat() {
        let m = parse_meminfo("MemTotal:       16000000 kB\nMemFree:  1000 kB\nMemAvailable:    8000000 kB\n").unwrap();
        assert_eq!(m, SysMemory { total: 16_000_000 * 1024, available: 8_000_000 * 1024 });
        assert!(!m.is_low());
        let vm = "Mach Virtual Memory Statistics: (page size of 16384 bytes)\nPages free:                               1000.\nPages active:  5.\nPages inactive:                           3000.\nPages speculative:                         100.\n";
        let m = parse_vm_stat(vm, 8 << 30).unwrap();
        assert_eq!(m.available, 4100 * 16384);
        assert!(m.is_low());
    }
}
