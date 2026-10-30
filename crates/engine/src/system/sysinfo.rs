//! Host information for Settings ▸ Memory & CPU (RAM reserved for other applications, reducing
//! caches when the system is low on memory), the disk cache's free-space reserve and Help ▸
//! System Compatibility Report.
//!
//! Pure Rust and dependency-free: the values come from the operating system's own reporting
//! (`/proc/meminfo` on Linux, `sysctl`/`vm_stat` on macOS, `df` on Unix, `wmic`/PowerShell on
//! Windows). Every query may fail (sandboxes, the web): callers treat `None` as "unknown" and
//! keep their configured budgets. The periodic readings behind the cache budgets run on a
//! background thread ([`Watch`]): on Windows they start PowerShell, which takes about a second.

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

/// A system value read in the background: [`Watch::poll_with`] starts a reading (unless one is
/// running) and [`Watch::latest`] returns the last one without waiting.
#[derive(Clone)]
pub struct Watch<T> {
    latest: Arc<Mutex<Option<T>>>,
    busy: Arc<AtomicBool>,
}

impl<T> Default for Watch<T> {
    fn default() -> Self {
        Watch { latest: Arc::new(Mutex::new(None)), busy: Arc::new(AtomicBool::new(false)) }
    }
}

/// The system's memory ([`memory`]).
pub type MemoryWatch = Watch<SysMemory>;
/// The disk cache's free-space reading ([`disk_space`]): the reader applies it to its cache
/// itself, so only the busy guard is used.
pub type DiskWatch = Watch<()>;

impl<T: Copy + Send + 'static> Watch<T> {
    /// Start a new reading in the background on a thread called `name` (no threads on the web:
    /// read inline, it is unknown there anyway).
    pub fn poll_with(&self, name: &str, read: impl FnOnce() -> Option<T> + Send + 'static) {
        if self.busy.swap(true, Ordering::AcqRel) {
            return;
        }
        let (latest, busy) = (self.latest.clone(), self.busy.clone());
        let read = move || {
            // A failed reading clears the last one: a stale value is worse than none.
            *latest.lock().unwrap_or_else(PoisonError::into_inner) = read();
            busy.store(false, Ordering::Release);
        };
        #[cfg(not(target_arch = "wasm32"))]
        if std::thread::Builder::new().name(name.into()).spawn(read).is_err() {
            self.busy.store(false, Ordering::Release);
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = name;
            read();
        }
    }

    /// The last reading (`None` until one finished or when the last one failed).
    pub fn latest(&self) -> Option<T> {
        *self.latest.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl MemoryWatch {
    /// Start a new reading of the system's memory in the background.
    pub fn poll(&self) {
        self.poll_with("ec-memory", memory);
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[allow(dead_code)]
fn run(cmd: &str, args: &[&str]) -> Option<String> {
    run_env(cmd, args, &[])
}

#[cfg(not(target_arch = "wasm32"))]
#[allow(dead_code)]
fn run_env(cmd: &str, args: &[&str], env: &[(&str, &std::ffi::OsStr)]) -> Option<String> {
    let mut c = std::process::Command::new(cmd);
    c.args(args).envs(env.iter().copied());
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

/// Parse `df -Pk` output: the disk's total and available bytes.
pub fn parse_df(text: &str) -> Option<(u64, u64)> {
    let line = text.lines().skip(1).find(|l| !l.trim().is_empty())?;
    let f: Vec<&str> = line.split_whitespace().collect();
    // A full disk into its root-reserved blocks reports negative numbers (Linux): none free.
    let num = |v: &str| match v.strip_prefix('-') {
        Some(d) => d.parse::<u64>().ok().map(|_| 0),
        None => v.parse::<u64>().ok(),
    };
    // Size, used, available and capacity follow the device name (which may contain spaces).
    let i = (0..f.len().saturating_sub(3))
        .find(|&i| f.get(i + 3).is_some_and(|c| c.ends_with('%')) && f.get(i..i + 3).is_some_and(|n| n.iter().all(|v| num(v).is_some())))?;
    let kb = |j: usize| f.get(j).and_then(|v| num(v)).map(|v| v.saturating_mul(1024));
    Some((kb(i)?, kb(i + 2)?))
}

/// Total and available bytes of the disk holding `path`, when the system reports them.
pub fn disk_space(path: &std::path::Path) -> Option<(u64, u64)> {
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd"))]
    {
        return parse_df(&run("df", &["-Pk", &path.to_string_lossy()])?);
    }
    #[cfg(target_os = "windows")]
    {
        // The path goes through the environment: no quoting.
        let text = run_env(
            "powershell",
            &["-NoProfile", "-Command", "$d=[IO.DriveInfo]::new($env:EC_DISK_PATH); \"$($d.TotalSize) $($d.AvailableFreeSpace)\""],
            &[("EC_DISK_PATH", path.as_os_str())],
        )?;
        let mut it = text.split_whitespace().filter_map(|v| v.parse::<u64>().ok());
        return Some((it.next()?, it.next()?));
    }
    #[allow(unreachable_code)]
    {
        let _ = path;
        None
    }
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
    fn a_failed_reading_clears_the_last_one() {
        let w = Watch::<u64>::default();
        let wait = || {
            for _ in 0..500 {
                if !w.busy.load(Ordering::Acquire) {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            panic!("the reading never finished");
        };
        w.poll_with("ec-test", || Some(7));
        wait();
        assert_eq!(w.latest(), Some(7));
        w.poll_with("ec-test", || None);
        wait();
        assert_eq!(w.latest(), None);
    }

    #[test]
    fn parses_df() {
        let linux = "Filesystem     1024-blocks      Used Available Capacity Mounted on\n/dev/nvme0n1p2   488281250 123456789 364824461      26% /\n";
        assert_eq!(parse_df(linux), Some((488_281_250 * 1024, 364_824_461 * 1024)));
        // macOS: a device name with spaces and a mount point with spaces.
        let mac = "Filesystem 1024-blocks Used Available Capacity  Mounted on\nmap auto_home 0 0 0 100% /System/Volumes/Data/home\n";
        assert_eq!(parse_df(mac), Some((0, 0)));
        let spaced = "Filesystem 1024-blocks Used Available Capacity Mounted on\n//nas/My Share 1000 400 600 40% /Volumes/My Share\n";
        assert_eq!(parse_df(spaced), Some((1000 * 1024, 600 * 1024)));
        // A full disk into its reserved blocks: nothing available.
        let full = "Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/sda2 1000 1010 -10 101% /\n";
        assert_eq!(parse_df(full), Some((1000 * 1024, 0)));
        assert_eq!(parse_df("Filesystem 1024-blocks\n"), None);
        assert_eq!(parse_df(""), None);
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
