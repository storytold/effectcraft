//! Audio preview output through cpal (the device and channel mapping chosen in Settings ▸ Audio;
//! the default output device when none is set or it is gone).
//!
//! cpal streams are not `Send` on every platform, so each preview runs its stream on a small
//! dedicated thread that owns it until stopped. The callback pulls interleaved stereo from the
//! frontend's [`AudioFeed`] and maps it onto the device's channels (mono: L+R average; extra
//! channels: silence), converting to the device's sample format.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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

/// EffectCraft's usable A/V sync window, in seconds. A bound the app sets for itself, not a
/// claim about hardware: holding the preview picture still behind the sound for longer than
/// this is past the point where a sync is worth applying, so past it there is nothing a sync
/// could do — at any stream age, including once the stream's own age and the tablet's uptime
/// look alike. It is what rejects an uptime-sized reading once age alone cannot tell it apart;
/// [implausible_device_latency] names why.
const MAX_SYNC_WINDOW_SECS: f64 = 2.0;

/// How far past its own age a stream's reading can legitimately be. The stream is opened after
/// the mixer has already filled its buffer, which the device plays ahead of the first callback,
/// and a wireless output adds codec delay on top; together that is the slack a real delay can
/// carry beyond the stream's lifetime. Anything further ahead cannot be this stream's delay,
/// whatever its value looks like.
const MIXER_SLACK_SECS: f64 = 0.5;

/// Why a reported device delay cannot be used, or `None` when it can. Two bounds, each catching
/// what the other cannot. The stream bound rejects a reading ahead of what this stream could
/// have produced — its own age, plus the buffer already handed over, plus [MIXER_SLACK_SECS] —
/// because cpal's AAudio backend (Android) builds `playback` from `Clockid::Monotonic`
/// (absolute, since boot) while `callback` counts from stream creation, so their difference
/// carries the time between boot and this stream: a stream opened seconds after boot reports
/// roughly that many seconds of latency, which is why a value that merely looks
/// hardware-plausible is not enough (#2845). The window bound rejects anything past
/// EffectCraft's own sync horizon even once the stream is old enough for its age to look
/// compatible with the reading. A delay inside both bounds keeps being used.
fn implausible_device_latency(dev_lat: f64, elapsed: f64, buffer_frames: usize, rate: f64) -> Option<&'static str> {
    if !dev_lat.is_finite() {
        return Some("not a finite duration");
    }
    if dev_lat < 0.0 {
        return Some("negative, the playback timestamp sitting before the callback");
    }
    // rate comes from the stream config; bound the division rather than trust it to be nonzero.
    let stream_bound = elapsed + (buffer_frames as f64 / rate.max(1.0)) + MIXER_SLACK_SECS;
    if dev_lat > stream_bound {
        return Some("ahead of a stream that only just started — AAudio reports playback since boot, callback since stream creation");
    }
    if dev_lat > MAX_SYNC_WINDOW_SECS {
        return Some("past the sync window EffectCraft can apply, so the picture would sit frozen behind the sound");
    }
    None
}

/// Frames between "handed over" and "audible": the device's output latency plus one callback's
/// buffer. An uptime-sized latency holds the preview's audio clock behind the frames the device
/// consumed — the picture freezes on the first frame while the sound plays on — so a reading
/// [implausible_device_latency] rejects is dropped and the buffer alone still counts.
fn sync_latency_frames(dev_lat: f64, elapsed: f64, rate: f64, buffer_frames: usize) -> u64 {
    static WARNED: AtomicBool = AtomicBool::new(false);
    if let Some(reason) = implausible_device_latency(dev_lat, elapsed, buffer_frames, rate) {
        if !WARNED.swap(true, Ordering::Relaxed) {
            log::warn!("audio output: device latency {dev_lat}s is {reason}; syncing A/V to this buffer alone");
        }
        return buffer_frames as u64;
    }
    (dev_lat * rate) as u64 + buffer_frames as u64
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
    // How long this stream has been alive, which tells an uptime-sized timestamp pair apart from
    // a real output delay (see [implausible_device_latency]).
    let started = Instant::now();
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
            latency.store(sync_latency_frames(dev_lat, started.elapsed().as_secs_f64(), rate, frames), Ordering::Relaxed);
        },
        |e| log::warn!("audio output: {e}"),
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

    /// Real device latencies (ALSA/Pulse/PipeWire report a few to a few hundred milliseconds)
    /// are used as they are, plus the callback's own buffer.
    #[test]
    fn sync_latency_keeps_a_plausible_device_latency() {
        assert_eq!(sync_latency_frames(0.02, 0.5, 48_000.0, 480), 960 + 480);
        assert_eq!(sync_latency_frames(0.0, 0.5, 48_000.0, 480), 480);
        assert_eq!(sync_latency_frames(0.3, 1.0, 44_100.0, 441), 13_230 + 441);
    }

    /// A delay over one second keeps being used when the stream has been alive long enough to
    /// have had it — a Pulse/PipeWire period can be a second or more — as long as it stays
    /// inside EffectCraft's sync horizon.
    #[test]
    fn sync_latency_keeps_a_long_device_latency_on_an_old_stream() {
        assert_eq!(sync_latency_frames(1.5, 2.5, 48_000.0, 480), 72_000 + 480);
        assert_eq!(sync_latency_frames(2.0, 3_600.0, 48_000.0, 480), 96_000 + 480);
    }

    /// A multi-second reading is dropped when the stream could not have produced it yet: a
    /// stream opened about five seconds after boot reports about five seconds of latency
    /// because AAudio subtracts since-boot playback from since-creation callback (#2845), and
    /// holding the picture there is exactly the freeze this guards against. Passing the stream
    /// its own buffer and sample rate is what makes the bound the stream's, not the hardware's.
    #[test]
    fn sync_latency_drops_a_reading_a_young_stream_cannot_have() {
        assert_eq!(sync_latency_frames(5.0, 0.1, 48_000.0, 480), 480);
        assert_eq!(sync_latency_frames(2.5, 1.0, 48_000.0, 480), 480);
        // A reading inside the stream's own bound is still used: the buffer-only fallback is
        // for the impossible readings, not for a plausible young-stream delay.
        assert_eq!(sync_latency_frames(0.3, 0.1, 48_000.0, 480), 14_400 + 480);
    }

    /// cpal's AAudio backend (Android) subtracts two different clocks — `playback` is
    /// CLOCK_MONOTONIC since boot, `callback` since the stream was created — so the difference
    /// is the tablet's uptime. That must not become the A/V-sync latency: pinned there, the
    /// preview's clock never leaves its first frame (picture frozen while the sound plays). The
    /// stream bound rejects it while the stream is young; EffectCraft's own sync horizon rejects
    /// it once age and uptime look alike, which is also what drops a multi-second reading from a
    /// stream opened after boot that never had one.
    #[test]
    fn sync_latency_drops_an_uptime_sized_device_latency() {
        // A tablet up ~5 days at 48 kHz would otherwise ask for ~2e10 frames of latency.
        // Young stream: the uptime-sized reading is ahead of the stream's own age.
        assert_eq!(sync_latency_frames(432_000.0, 0.05, 48_000.0, 480), 480);
        assert_eq!(sync_latency_frames(10.5, 0.05, 48_000.0, 480), 480);
        // Old stream: uptime and stream age now look alike, so the sync window catches it.
        assert_eq!(sync_latency_frames(432_000.0, 432_000.0, 48_000.0, 480), 480);
        // An uptime-sized reading below the old 10 s ceiling: once boot-to-stream and stream
        // age look alike, age cannot tell it apart, so the sync horizon catches it instead.
        assert_eq!(sync_latency_frames(5.0, 6.0, 48_000.0, 480), 480);
        assert_eq!(sync_latency_frames(f64::INFINITY, 1.0, 48_000.0, 480), 480);
        assert_eq!(sync_latency_frames(f64::NAN, 1.0, 48_000.0, 480), 480);
        assert_eq!(sync_latency_frames(-0.5, 1.0, 48_000.0, 480), 480);
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
}
