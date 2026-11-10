//! Fixed-vocabulary observations retained by the crash helper independently of the log tail.
//! Driver messages and environment text can contain private data; never forward either here.

use std::ffi::OsStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use crate::crash_report::Shutdown;

pub fn record(reporter: &Shutdown, event: &'static str) {
    log::info!("diagnostic observation: {event}");
    if let Err(error) = reporter.record(event) {
        log::warn!("could not retain diagnostic observation: {error}");
    }
}

/// Mirror only wgpu's documented names/aliases, without calling its parser: that parser
/// logs unknown tokens verbatim. Unknown, non-Unicode and oversized values remain private.
pub fn backend_override_events(value: &OsStr) -> Vec<&'static str> {
    let Some(value) = value.to_str().filter(|value| value.len() <= 4096) else {
        return vec!["WGPU_BACKEND: non-Unicode or oversized value omitted"];
    };
    let mut selected = [false; 6];
    let mut unknown = false;
    for token in value.split(',').map(str::trim) {
        let aliases: &[&[&str]] = &[&["vulkan", "vk"], &["dx12", "d3d12"], &["metal", "mtl"], &["opengl", "gles", "gl"], &["webgpu"], &["noop"]];
        if let Some(index) = aliases.iter().position(|names| names.iter().any(|name| token.eq_ignore_ascii_case(name))) {
            if let Some(flag) = selected.get_mut(index) {
                *flag = true;
            }
        } else {
            unknown = true;
        }
    }
    let events = [
        "WGPU_BACKEND requests: vulkan",
        "WGPU_BACKEND requests: dx12",
        "WGPU_BACKEND requests: metal",
        "WGPU_BACKEND requests: gl",
        "WGPU_BACKEND requests: webgpu",
        "WGPU_BACKEND requests: noop",
    ];
    let mut result: Vec<_> = selected.into_iter().zip(events).filter_map(|(present, event)| present.then_some(event)).collect();
    if unknown {
        result.push("WGPU_BACKEND: unrecognized or empty tokens omitted");
    }
    result
}

/// At most eight events per device, even when a driver floods its callbacks. A wgpu OOM
/// observation is not proof of system RAM exhaustion; device loss is not a driver-crash verdict.
#[derive(Clone)]
pub struct GpuObservations {
    seen: Arc<AtomicU8>,
    reporter: Shutdown,
}

impl GpuObservations {
    pub fn new(reporter: &Shutdown) -> Self {
        Self { seen: Arc::new(AtomicU8::new(0)), reporter: reporter.clone() }
    }

    pub fn uncaptured(&self, error: &eframe::wgpu::Error) {
        let (bit, event) = match error {
            eframe::wgpu::Error::OutOfMemory { .. } => (1, "GPU observation: wgpu out-of-memory error (not proof of system RAM exhaustion)"),
            eframe::wgpu::Error::Validation { .. } => (2, "GPU observation: wgpu validation error"),
            eframe::wgpu::Error::Internal { .. } => (4, "GPU observation: wgpu internal error"),
        };
        self.once(bit, event);
    }

    pub fn scoped(&self, error: effectcraft_ui_egui::gpu_failure::ScopedError) {
        use effectcraft_ui_egui::gpu_failure::ScopedError;
        let (bit, event) = match error {
            ScopedError::OutOfMemory => (32, "GPU scoped observation: out-of-memory (not proof of system RAM exhaustion)"),
            ScopedError::Internal => (64, "GPU scoped observation: internal error (not confirmed out-of-memory)"),
            ScopedError::Validation => (128, "GPU scoped observation: validation error"),
        };
        self.once(bit, event);
    }

    pub fn device_lost(&self, reason: eframe::wgpu::DeviceLostReason) {
        match reason {
            eframe::wgpu::DeviceLostReason::Unknown => self.once(8, "GPU observation: device lost, reason unknown (not proof of a driver crash)"),
            eframe::wgpu::DeviceLostReason::Destroyed => self.once(16, "GPU observation: device-lost callback, explicit device destruction"),
        };
    }

    fn once(&self, bit: u8, event: &'static str) -> bool {
        if self.seen.fetch_or(bit, Ordering::Relaxed) & bit != 0 {
            return false;
        }
        record(&self.reporter, event);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn override_names_are_normalized_deduplicated_and_private() {
        let events = backend_override_events(OsStr::new(" Vk, VULKAN, d3d12, MTL, gles, webgpu, noop, private-token"));
        assert_eq!(events.len(), 7);
        assert_eq!(events.first(), Some(&"WGPU_BACKEND requests: vulkan"));
        assert!(!events.join(" ").contains("private-token"));
        assert!(events.iter().all(|event| event.len() <= 100));
        assert_eq!(backend_override_events(OsStr::new("")).len(), 1);
        assert_eq!(backend_override_events(OsStr::new(&"x".repeat(4097))), ["WGPU_BACKEND: non-Unicode or oversized value omitted"]);
    }

    #[test]
    fn gpu_categories_are_latched_independently() {
        let observations = GpuObservations::new(&Shutdown::default());
        observations.uncaptured(&eframe::wgpu::Error::OutOfMemory { source: Box::new(std::io::Error::other("private driver detail")) });
        observations.device_lost(eframe::wgpu::DeviceLostReason::Unknown);
        observations.device_lost(eframe::wgpu::DeviceLostReason::Unknown);
        assert_eq!(observations.seen.load(Ordering::Relaxed), 9);
        observations.device_lost(eframe::wgpu::DeviceLostReason::Destroyed);
        observations.uncaptured(&eframe::wgpu::Error::Validation {
            source: Box::new(std::io::Error::other("private driver detail")),
            description: "private validation detail".into(),
        });
        observations.uncaptured(&eframe::wgpu::Error::Internal {
            source: Box::new(std::io::Error::other("private driver detail")),
            description: "private internal detail".into(),
        });
        assert_eq!(observations.seen.load(Ordering::Relaxed), 31);
        use effectcraft_ui_egui::gpu_failure::ScopedError;
        observations.scoped(ScopedError::Internal);
        assert_eq!(observations.seen.load(Ordering::Relaxed), 95, "internal scope must not set scoped OOM bit");
        observations.scoped(ScopedError::OutOfMemory);
        observations.scoped(ScopedError::Validation);
        observations.scoped(ScopedError::OutOfMemory);
        assert_eq!(observations.seen.load(Ordering::Relaxed), 255);
        assert!(!observations.once(1, "GPU observation: duplicate"));
        assert!(!observations.once(8, "GPU observation: duplicate"));
    }

    #[test]
    #[cfg(unix)]
    fn non_unicode_override_is_not_copied() {
        use std::os::unix::ffi::OsStrExt;
        assert_eq!(backend_override_events(OsStr::from_bytes(b"private\xff")), ["WGPU_BACKEND: non-Unicode or oversized value omitted"]);
    }
}
