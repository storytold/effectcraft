//! Bounded observations made outside the failed GUI. Measurements are evidence, not a cause.
use std::io::{self, Write};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant, SystemTime};

use sysinfo::{MemoryRefreshKind, Pid, ProcessRefreshKind, ProcessesToUpdate, System};

pub fn utc() -> String {
    chrono::DateTime::<chrono::Utc>::from(SystemTime::now()).to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

#[derive(Default)]
struct Memory {
    samples: u64,
    resident_peak: u64,
    virtual_peak: u64,
    available_min: Option<u64>,
    total: u64,
    last_at: Option<String>,
    last_uptime_secs: Option<u64>,
}

impl Memory {
    fn sample(&mut self, resident: u64, virtual_bytes: u64, available: u64, total: u64) {
        self.samples = self.samples.saturating_add(1);
        self.resident_peak = self.resident_peak.max(resident);
        self.virtual_peak = self.virtual_peak.max(virtual_bytes);
        // Zero total means the platform could not supply physical-memory information.
        if total != 0 {
            self.available_min = Some(self.available_min.map_or(available, |old| old.min(available)));
            self.total = total;
        }
        self.last_at = Some(utc());
    }
}

pub struct Observation {
    pid: Pid,
    system: System,
    birth: Option<u64>,
    started: Instant,
    started_at: String,
    detected_at: Option<String>,
    last_sample: Instant,
    memory: Memory,
    // Only Windows can retrieve an unrelated process's ExitStatus through a retained handle.
    exit: Receiver<Option<std::process::ExitStatus>>,
    status: Option<std::process::ExitStatus>,
    alive: Option<bool>,
}

fn target_system(pid: Pid) -> System {
    let mut system = System::new();
    system.refresh_processes_specifics(ProcessesToUpdate::Some(&[pid]), false, ProcessRefreshKind::nothing().with_memory());
    system
}

impl Observation {
    /// Acquire the Windows handle before acknowledging reporter readiness, so even an immediate
    /// GUI abort leaves an observable process object. Never enumerate commands or environments.
    pub fn start(pid: u32) -> Self {
        let pid = Pid::from_u32(pid);
        let system = target_system(pid);
        let birth = system.process(pid).map(|p| p.start_time());
        let (send, exit) = mpsc::sync_channel(1);
        #[cfg(windows)]
        {
            let waiting = target_system(pid);
            let _ = std::thread::Builder::new().name("ec-exit-status".into()).spawn(move || {
                let status = waiting.process(pid).and_then(|p| p.wait());
                let _ = send.send(status);
            });
        }
        #[cfg(not(windows))]
        drop(send);
        let now = Instant::now();
        let mut result = Self {
            pid,
            system,
            birth,
            started: now,
            started_at: utc(),
            detected_at: None,
            last_sample: now,
            memory: Memory::default(),
            exit,
            status: None,
            alive: None,
        };
        result.sample();
        result
    }

    fn sample(&mut self) {
        self.system.refresh_processes_specifics(ProcessesToUpdate::Some(&[self.pid]), false, ProcessRefreshKind::nothing().with_memory());
        self.system.refresh_memory_specifics(MemoryRefreshKind::nothing().with_ram());
        let process = self.system.process(self.pid).filter(|p| Some(p.start_time()) == self.birth && p.exists());
        self.alive = self.birth.map(|_| process.is_some());
        if let Some(process) = process {
            self.memory.last_uptime_secs = Some(process.run_time());
            self.memory.sample(process.memory(), process.virtual_memory(), self.system.available_memory(), self.system.total_memory());
        }
        if let Ok(status) = self.exit.try_recv() {
            self.status = status;
        }
        self.last_sample = Instant::now();
    }

    pub fn tick(&mut self) {
        if self.last_sample.elapsed() >= Duration::from_secs(1) {
            self.sample();
        }
    }

    /// Pipe closure can precede the OS signalling process termination. Bound that grace period;
    /// a broken pipe or early reporter drop must never be called a confirmed process exit.
    pub fn finish(&mut self) {
        self.detected_at = Some(utc());
        if self.status.is_none()
            && let Ok(status) = self.exit.recv_timeout(Duration::from_millis(750))
        {
            self.status = status;
        }
        self.sample();
    }

    pub fn confirmed_exit(&self) -> bool {
        self.status.is_some()
    }

    pub fn write(&self, report: &mut impl Write) -> io::Result<()> {
        writeln!(
            report,
            "Detection UTC: {}\nObserver started UTC: {}\nObserved duration: {} ms",
            self.detected_at.as_deref().unwrap_or("not captured"),
            self.started_at,
            self.started.elapsed().as_millis()
        )?;
        writeln!(
            report,
            "Process observation: {}",
            if self.confirmed_exit() {
                "process termination observed"
            } else if self.alive == Some(true) {
                "GUI still present when the control channel ended"
            } else {
                "target absent from refresh or process identity/status unavailable (not proof of termination)"
            }
        )?;
        match &self.status {
            Some(status) => {
                writeln!(report, "OS exit status: {status}")?;
                #[cfg(windows)]
                if let Some(code) = status.code() {
                    write_windows_status(report, code as u32)?;
                }
            }
            None => writeln!(report, "OS exit status: unavailable (not inferred from the control pipe)")?,
        }
        let memory = &self.memory;
        writeln!(report, "\nMemory evidence (1 s sampling; sampled maxima, not OS peaks):\nTarget-present sampling attempts: {}", memory.samples)?;
        if let Some(uptime) = memory.last_uptime_secs {
            writeln!(report, "GUI uptime at last target-present observation: {uptime} s")?;
        }
        #[cfg(windows)]
        let virtual_label = "Peak sampled private committed bytes";
        #[cfg(not(windows))]
        let virtual_label = "Peak sampled virtual bytes";
        if memory.samples != 0 {
            writeln!(
                report,
                "Last target sampling attempt UTC: {}\nPeak sampled resident bytes: {}\n{virtual_label}: {}",
                memory.last_at.as_deref().unwrap_or("unavailable"),
                memory.resident_peak,
                memory.virtual_peak
            )?;
        }
        if let Some(available) = memory.available_min {
            writeln!(report, "Minimum sampled system available RAM bytes: {available}\nSystem total RAM bytes at last sample: {}", memory.total)?;
        }
        writeln!(
            report,
            "Samples can miss short spikes or retain stale/zero readings when an OS query fails; per-field success is not exposed by sysinfo. Windows private commit is not GPU VRAM; on other platforms virtual memory is not physical RAM. System commit limit/usage and GPU memory budgets were not collected. No memory-pressure threshold establishes the cause of this incident.\nThe memory observer does not collect process command lines, environments, documents, native dumps or system event logs. The existing session-log tail below may contain paths or other logged details; review it before sharing."
        )
    }
}

#[cfg(any(windows, test))]
fn write_windows_status(report: &mut impl Write, code: u32) -> io::Result<()> {
    let meaning = match code {
        0xc0000005 => "STATUS_ACCESS_VIOLATION",
        0xc0000017 => "STATUS_NO_MEMORY",
        0xc000012d => "STATUS_COMMITMENT_LIMIT",
        0xc0000374 => "STATUS_HEAP_CORRUPTION",
        0xc0000409 => "STATUS_STACK_BUFFER_OVERRUN / fast fail",
        0x40000015 => "STATUS_FATAL_APP_EXIT",
        _ => "unclassified",
    };
    writeln!(
        report,
        "Windows status bits: 0x{code:08X} ({meaning})\nThese are process exit bits, not a verified exception record. Software can set an exit code explicitly; an access violation does not identify the faulting module or prove a driver crash."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_target_without_exit_status_does_not_confirm_termination() {
        let (send, exit) = mpsc::sync_channel(1);
        drop(send);
        let now = Instant::now();
        let observation = Observation {
            pid: Pid::from_u32(1),
            system: System::new(),
            birth: Some(1),
            started: now,
            started_at: utc(),
            detected_at: Some(utc()),
            last_sample: now,
            memory: Memory::default(),
            exit,
            status: None,
            alive: Some(false),
        };
        assert!(!observation.confirmed_exit());
        let mut report = Vec::new();
        observation.write(&mut report).unwrap();
        let report = String::from_utf8(report).unwrap();
        assert!(report.contains("not proof of termination"));
        assert!(!report.contains("process termination observed"));
    }

    #[test]
    fn memory_high_water_marks_are_bounded_and_not_a_causal_diagnosis() {
        let mut memory = Memory::default();
        memory.sample(40, 80, 30, 100);
        memory.sample(20, 90, 50, 100);
        assert_eq!((memory.samples, memory.resident_peak, memory.virtual_peak, memory.available_min), (2, 40, 90, Some(30)));
        memory.sample(0, 0, 0, 0);
        assert_eq!(memory.available_min, Some(30));
    }

    #[test]
    fn windows_status_bits_preserve_unsigned_codes_without_claiming_a_driver_crash() {
        for (code, name) in
            [(0xc0000005, "STATUS_ACCESS_VIOLATION"), (0xc0000017, "STATUS_NO_MEMORY"), (0xc000012d, "STATUS_COMMITMENT_LIMIT"), (42, "unclassified")]
        {
            let mut text = Vec::new();
            write_windows_status(&mut text, code).unwrap();
            let text = String::from_utf8(text).unwrap();
            assert!(text.contains(name));
            assert!(text.contains(&format!("0x{code:08X}")));
            assert!(text.contains("not a verified exception record"));
        }
    }

    #[test]
    fn utc_timestamps_are_parseable_and_zero_offset() {
        let time = chrono::DateTime::parse_from_rfc3339(&utc()).unwrap();
        assert_eq!(time.offset().local_minus_utc(), 0);
    }
}
