//! Audio preview output through cpal (the device and channel mapping chosen in Settings ▸ Audio;
//! the default output device when none is set or it is gone).
//!
//! cpal streams are not `Send` on every platform, so each preview runs its stream on a small
//! dedicated thread that owns it until stopped. The callback pulls interleaved stereo from the
//! frontend's [`AudioFeed`] and maps it onto the device's channels (mono: L+R average; extra
//! channels: silence), converting to the device's sample format.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SizedSample};
use effectcraft_ui_egui::audio::{AudioDevice, AudioFeed, AudioOutput};

/// Device callback period: about 10 ms, clamped so Pulse/PipeWire/ALSA does not open a
/// multi-hundred-millisecond default buffer that plays as silence at the start of preview (#554).
fn preferred_buffer_frames(rate: u32) -> u32 {
    (rate / 100).clamp(256, 2048)
}

/// Gate for stream-error logging: admits at most one error per `window_ms` and counts the ones
/// it suppressed, so a backend whose poll loop keeps failing (alsa's POLLERR) cannot flood the
/// log from a single stream.
struct ErrorGate {
    window_ms: u64,
    last: Option<u64>,
    suppressed: u64,
}

impl ErrorGate {
    /// A fresh gate: nothing reported yet, nothing suppressed, `window_ms` between reports.
    fn new(window_ms: u64) -> Self {
        ErrorGate { window_ms, last: None, suppressed: 0 }
    }

    /// Admit an error at monotonic `now_ms`: `Some(n)` logs it after `n` suppressed ones;
    /// `None` suppresses it.
    fn admit(&mut self, now_ms: u64) -> Option<u64> {
        let due = self.last.is_none_or(|last| now_ms.saturating_sub(last) >= self.window_ms);
        if due {
            let suppressed = self.suppressed;
            self.last = Some(now_ms);
            self.suppressed = 0;
            Some(suppressed)
        } else {
            self.suppressed += 1;
            None
        }
    }
}

/// The stream's error callback: report the failure through the gate (see [`ErrorGate`]).
fn on_stream_error(gate: &mut ErrorGate, origin: &Instant, e: impl std::fmt::Display) {
    if let Some(suppressed) = gate.admit(origin.elapsed().as_millis() as u64) {
        if suppressed > 0 {
            log::warn!("audio output: {e} (and {suppressed} more since the last report)");
        } else {
            log::warn!("audio output: {e}");
        }
    }
}

/// Wait until the feeder has mixed a block, or until preview is stopped. Starting the stream
/// before that fills the device with silence, so the first real samples are heard late (#554).
fn wait_to_play(feed: &AudioFeed, stop: &mpsc::Receiver<()>) -> bool {
    loop {
        if feed.queued_frames() > 0 {
            return true;
        }
        match stop.recv_timeout(Duration::from_millis(2)) {
            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => return false,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}

pub struct CpalOut {
    out: AudioOutput,
    rate: u32,
    latency: Arc<AtomicU64>,
    stop: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

/// Names of the output devices (Settings ▸ Audio).
pub fn devices() -> Vec<String> {
    let host = cpal::default_host();
    #[allow(deprecated)]
    host.output_devices().map(|d| d.filter_map(|d| d.name().ok()).collect()).unwrap_or_default()
}

/// The named output device, or the default one.
fn device(name: &str) -> Option<cpal::Device> {
    let host = cpal::default_host();
    if !name.is_empty() {
        #[allow(deprecated)]
        let found = host.output_devices().ok().and_then(|mut d| d.find(|d| d.name().ok().as_deref() == Some(name)));
        if found.is_some() {
            return found;
        }
    }
    host.default_output_device()
}

/// Open the output device (`None` without one).
pub fn open(out: &AudioOutput) -> Option<Box<dyn AudioDevice>> {
    let dev = device(&out.device)?;
    let cfg = dev.default_output_config().ok()?;
    // Settings ▸ Audio ▸ Preview Sample Rate: open the device at that rate when it supports it
    // (otherwise the preview is mixed at that rate and resampled to the device's).
    let supports = |r: u32| {
        dev.supported_output_configs()
            .map(|mut c| c.any(|c| c.channels() == cfg.channels() && c.min_sample_rate().0 <= r && r <= c.max_sample_rate().0))
            .unwrap_or(false)
    };
    let rate = if out.rate > 0 && supports(out.rate) { out.rate } else { cfg.sample_rate().0 };
    Some(Box::new(CpalOut { out: out.clone(), rate, latency: Arc::new(AtomicU64::new(0)), stop: None, thread: None }))
}

fn run<T: SizedSample + FromSample<f32>>(
    dev: &cpal::Device,
    cfg: &cpal::StreamConfig,
    feed: Arc<AudioFeed>,
    latency: Arc<AtomicU64>,
    map: (usize, usize),
) -> Result<cpal::Stream, String> {
    let ch = cfg.channels as usize;
    // Output mapping: device channels (0-based) for left and right; out of range → 1 and 2.
    let (left, right) = if map.0 < ch && map.1 < ch { map } else { (0, 1) };
    let rate = cfg.sample_rate.0 as f64;
    let mut stereo: Vec<f32> = Vec::new();
    let mut gate = ErrorGate::new(1000);
    let gate_origin = Instant::now();
    dev.build_output_stream(
        cfg,
        move |out: &mut [T], info: &cpal::OutputCallbackInfo| {
            let frames = out.len() / ch.max(1);
            stereo.resize(frames * 2, 0.0);
            feed.pull(&mut stereo);
            for (f, s) in out.chunks_mut(ch.max(1)).zip(stereo.as_chunks::<2>().0.iter()) {
                for (c, o) in f.iter_mut().enumerate() {
                    // Mono devices, or left and right mapped to the same channel, get the mix.
                    let v = if ch == 1 || (c == left && c == right) {
                        (s[0] + s[1]) * 0.5
                    } else if c == left {
                        s[0]
                    } else if c == right {
                        s[1]
                    } else {
                        0.0
                    };
                    *o = T::from_sample(v.clamp(-1.0, 1.0));
                }
            }
            // Frames between "handed over" and "audible": device latency plus this buffer.
            let ts = info.timestamp();
            let dev_lat = ts.playback.duration_since(&ts.callback).map(|d| d.as_secs_f64()).unwrap_or(0.0);
            latency.store((dev_lat * rate) as u64 + frames as u64, Ordering::Relaxed);
        },
        move |e| on_stream_error(&mut gate, &gate_origin, e),
        None,
    )
    .map_err(|e| e.to_string())
}

impl AudioDevice for CpalOut {
    fn sample_rate(&self) -> u32 {
        self.rate
    }

    fn start(&mut self, feed: Arc<AudioFeed>) -> Result<(), String> {
        self.stop();
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
        let latency = self.latency.clone();
        let rate = self.rate;
        let name = self.out.device.clone();
        let map = ((self.out.left.max(1) - 1) as usize, (self.out.right.max(1) - 1) as usize);
        let thread = std::thread::Builder::new()
            .name("ec-audio-out".into())
            .spawn(move || {
                let Some(dev) = device(&name) else {
                    let _ = ready_tx.send(Err("no audio output device".into()));
                    return;
                };
                let sup = match dev.default_output_config() {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = ready_tx.send(Err(e.to_string()));
                        return;
                    }
                };
                let mut cfg: cpal::StreamConfig = sup.config();
                cfg.sample_rate = cpal::SampleRate(rate);
                // A short period only where the default one is long (ALSA/PulseAudio on Linux, #554),
                // and only within what the device reports it takes.
                if cfg!(target_os = "linux") {
                    let want = preferred_buffer_frames(rate);
                    cfg.buffer_size = match sup.buffer_size() {
                        cpal::SupportedBufferSize::Range { min, max } if *min <= *max => cpal::BufferSize::Fixed(want.clamp(*min, *max)),
                        _ => cpal::BufferSize::Default,
                    };
                }
                let build = |cfg: &cpal::StreamConfig| match sup.sample_format() {
                    cpal::SampleFormat::F32 => run::<f32>(&dev, cfg, feed.clone(), latency.clone(), map),
                    cpal::SampleFormat::I16 => run::<i16>(&dev, cfg, feed.clone(), latency.clone(), map),
                    cpal::SampleFormat::U16 => run::<u16>(&dev, cfg, feed.clone(), latency.clone(), map),
                    cpal::SampleFormat::I32 => run::<i32>(&dev, cfg, feed.clone(), latency.clone(), map),
                    f => Err(format!("unsupported sample format {f:?}")),
                };
                let stream = match build(&cfg) {
                    Ok(s) => s,
                    Err(_) => {
                        // Some hosts reject a fixed period; a large default still plays, just later.
                        cfg.buffer_size = cpal::BufferSize::Default;
                        match build(&cfg) {
                            Ok(s) => s,
                            Err(e) => {
                                let _ = ready_tx.send(Err(e));
                                return;
                            }
                        }
                    }
                };
                let _ = ready_tx.send(Ok(()));
                if !wait_to_play(&feed, &stop_rx) {
                    return;
                }
                if let Err(e) = stream.play() {
                    log::warn!("audio output: {e}");
                    return;
                }
                let _ = stop_rx.recv();
                drop(stream);
            })
            .map_err(|e| e.to_string())?;
        self.stop = Some(stop_tx);
        self.thread = Some(thread);
        ready_rx.recv().map_err(|e| e.to_string())?
    }

    fn stop(&mut self) {
        if let Some(tx) = self.stop.take() {
            let _ = tx.send(());
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }

    fn latency_frames(&self) -> u64 {
        self.latency.load(Ordering::Relaxed)
    }
}

impl Drop for CpalOut {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preferred_buffer_is_about_ten_milliseconds() {
        assert_eq!(preferred_buffer_frames(48_000), 480);
        assert_eq!(preferred_buffer_frames(8_000), 256);
        assert_eq!(preferred_buffer_frames(192_000), 1920);
    }

    #[test]
    fn wait_to_play_returns_once_the_feeder_has_samples() {
        let feed = Arc::new(AudioFeed::default());
        let (_tx, rx) = mpsc::channel();
        let f = feed.clone();
        let h = std::thread::spawn(move || wait_to_play(&f, &rx));
        assert_eq!(feed.queued_frames(), 0);
        std::thread::sleep(Duration::from_millis(20));
        feed.push(&[0.1, 0.2]);
        assert!(h.join().unwrap());
        assert_eq!(feed.queued_frames(), 1);
    }

    #[test]
    fn wait_to_play_stops_without_samples() {
        let feed = AudioFeed::default();
        let (tx, rx) = mpsc::channel();
        let h = std::thread::spawn(move || wait_to_play(&feed, &rx));
        tx.send(()).unwrap();
        assert!(!h.join().unwrap());
    }

    /// #3317: a failing backend's poll loop calls the error callback far faster than any log
    /// should grow: the gate admits one per window and counts the suppressed ones, so the next
    /// report says how many were swallowed.
    #[test]
    fn the_error_gate_admits_one_per_window_and_counts_the_rest() {
        let mut gate = ErrorGate::new(1000);
        assert_eq!(gate.admit(0), Some(0), "the first error is always reported");
        assert_eq!(gate.admit(1), None);
        assert_eq!(gate.admit(999), None);
        assert_eq!(gate.admit(999), None, "suppression counts, not throttles");
        assert_eq!(gate.admit(1000), Some(3), "the next report carries the suppressed count");
        assert_eq!(gate.admit(1500), None);
        assert_eq!(gate.admit(2500), Some(1), "the window is measured from the last report");
        // A clock that stalls (a suspended device) does not make old errors flood back.
        assert_eq!(gate.admit(2500), None);
    }
}
