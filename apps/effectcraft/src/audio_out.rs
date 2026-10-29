//! Audio preview output through cpal (the device and channel mapping chosen in Settings ▸ Audio;
//! the default output device when none is set or it is gone).
//!
//! The output opens once and stays open for the whole session. Every preview start used to open
//! the device again — enumerating the devices and initializing the stream, which takes a good
//! fraction of a second on Windows' WASAPI — so a preview's sound began some frames after the
//! press that started it (#947, #1498). One thread now owns the stream for the process
//! ([`ensure`] builds it; the app warms it at startup, see [`prewarm`]): a playback binds its
//! [`AudioFeed`] to the callback and unbinds it again when it stops, and a stream that errors
//! (the device went away) is rebuilt on the next use. With nothing bound the callback pulls
//! silence, like a DAW whose output stays open between plays.
//!
//! The channel mapping is as before: interleaved stereo from the feed onto the device's channels
//! (mono: L+R average; extra channels: silence), converted to the device's sample format.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::PoisonError;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread::JoinHandle;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SizedSample};
use effectcraft_ui_egui::audio::{AudioDevice, AudioFeed, AudioOutput};

/// Device callback period: about 10 ms, clamped so Pulse/PipeWire/ALSA does not open a
/// multi-hundred-millisecond default buffer that plays as silence at the start of preview (#554).
fn preferred_buffer_frames(rate: u32) -> u32 {
    (rate / 100).clamp(256, 2048)
}

/// A command for the running output thread.
enum Cmd {
    /// Bind `feed` as the one the callback pulls from, under `session` (until that session is
    /// cleared, or another play replaces it).
    Play { session: u64, feed: Arc<AudioFeed> },
    /// Unbind the feed when `session` is still the one playing; a newer play keeps its own.
    Clear { session: u64 },
    /// The stream errored (the device went away), or the settings changed: stop serving. The
    /// next use rebuilds the output.
    Died,
}

/// What the running stream shares with its owners: the feed its callback pulls from (swapped per
/// playback), the output latency it measures (device buffering, for A/V sync), and whether the
/// stream is still alive.
#[derive(Default)]
struct Shared {
    slot: Mutex<Option<Arc<AudioFeed>>>,
    latency: AtomicU64,
    /// The session whose feed the callback serves (0: none). [`attach`] waits for it, so a `Play`
    /// queued behind a dead stream's `Died` is caught instead of leaving a stalled preview.
    bound: AtomicU64,
    /// The stream reported an error: the output is rebuilt rather than reused.
    dead: AtomicBool,
}

impl Shared {
    /// The bound feed (a panic in a thread holding the lock doesn't stop the music).
    fn feed(&self) -> Option<Arc<AudioFeed>> {
        self.slot.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// One command for the running stream; `current` is the session whose feed is bound.
    /// Returns whether the stream goes on serving.
    fn serve(&self, current: &mut u64, cmd: Cmd) -> bool {
        let mut slot = self.slot.lock().unwrap_or_else(PoisonError::into_inner);
        match cmd {
            Cmd::Play { session, feed } => {
                *current = session;
                *slot = Some(feed);
                self.bound.store(session, Ordering::Release);
                true
            }
            Cmd::Clear { session } => {
                if session == *current {
                    *slot = None;
                    self.bound.store(0, Ordering::Release);
                }
                true
            }
            Cmd::Died => false,
        }
    }
}

/// The process's audio output: the thread owning its stream, built for one [`AudioOutput`]
/// (device, channels, rate). It outlives every playback; the next one attaches to it.
struct Output {
    /// The settings the stream was built for.
    key: AudioOutput,
    /// The rate the device was opened at.
    rate: u32,
    shared: Arc<Shared>,
    tx: mpsc::Sender<Cmd>,
    thread: JoinHandle<()>,
}

/// The one output the process keeps open, built under this lock (so only one is ever built).
static OUTPUT: Mutex<Option<Output>> = Mutex::new(None);

/// Sessions of playbacks, so one playback's stop never unbinds a newer playback's feed (a scrub
/// closing after a preview started, say).
static SESSION: AtomicU64 = AtomicU64::new(0);

/// Lock the process output (a panic while holding it doesn't take the audio down).
fn output() -> std::sync::MutexGuard<'static, Option<Output>> {
    OUTPUT.lock().unwrap_or_else(PoisonError::into_inner)
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

/// The rate to open the device at: Settings ▸ Audio ▸ Preview Sample Rate when the device
/// supports it, else the device's own (the preview is then mixed at that rate instead).
fn resolve_rate(out: &AudioOutput) -> Option<u32> {
    let dev = device(&out.device)?;
    let cfg = dev.default_output_config().ok()?;
    let supports = |r: u32| {
        dev.supported_output_configs()
            .map(|mut c| c.any(|c| c.channels() == cfg.channels() && c.min_sample_rate().0 <= r && r <= c.max_sample_rate().0))
            .unwrap_or(false)
    };
    Some(if out.rate > 0 && supports(out.rate) { out.rate } else { cfg.sample_rate().0 })
}

/// The running output when it matches `out` (nothing is touched when it doesn't): the rate it
/// runs at, the shared state its callback pulls from, and where its commands go.
fn running(out: &AudioOutput) -> Option<(u32, Arc<Shared>, mpsc::Sender<Cmd>)> {
    output()
        .as_ref()
        .filter(|o| o.key == *out && !o.thread.is_finished() && !o.shared.dead.load(Ordering::Acquire))
        .map(|o| (o.rate, o.shared.clone(), o.tx.clone()))
}

/// The output for `out`, building it first when none matches (the first use, after a change in
/// Settings ▸ Audio, or when the device went away). `rate` is the rate the device is opened at
/// (see [`resolve_rate`]); building it here, under [`OUTPUT`]'s lock, is what a first press of
/// Space would otherwise wait for.
fn ensure(out: &AudioOutput, rate: u32) -> Result<(Arc<Shared>, mpsc::Sender<Cmd>), String> {
    let mut guard = output();
    if guard.as_ref().is_some_and(|o| o.key != *out || o.thread.is_finished() || o.shared.dead.load(Ordering::Acquire)) {
        // End the old stream: its thread stops with this command (or already has, when the device
        // went away), whether or not a stale playback still holds a sender.
        if let Some(old) = guard.take() {
            let _ = old.tx.send(Cmd::Died);
        }
    }
    if guard.is_none() {
        *guard = Some(spawn_output(out, rate)?);
    }
    let Some(o) = guard.as_ref() else {
        return Err("the audio output could not be opened".into());
    };
    Ok((o.shared.clone(), o.tx.clone()))
}

/// Open the device, run its stream until it is told to stop or errors, and report over `ready`
/// whether the stream was built. This is the only thread that touches the device.
fn spawn_output(out: &AudioOutput, rate: u32) -> Result<Output, String> {
    let (tx, rx) = mpsc::channel::<Cmd>();
    let shared = Arc::new(Shared::default());
    // The Output hands back its own handle: the thread's closure takes this one.
    let shared_out = shared.clone();
    let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
    let name = out.device.clone();
    let map = ((out.left.max(1) - 1) as usize, (out.right.max(1) - 1) as usize);
    let died = tx.clone();
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
            // Settings ▸ Audio ▸ Preview Sample Rate: the device is opened at the rate resolved
            // in `open` (the preview's mix is resampled to it when mixed at another rate).
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
                cpal::SampleFormat::F32 => run::<f32>(&dev, cfg, shared.clone(), map, died.clone()),
                cpal::SampleFormat::I16 => run::<i16>(&dev, cfg, shared.clone(), map, died.clone()),
                cpal::SampleFormat::U16 => run::<u16>(&dev, cfg, shared.clone(), map, died.clone()),
                cpal::SampleFormat::I32 => run::<i32>(&dev, cfg, shared.clone(), map, died.clone()),
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
            // The stream runs from here for the rest of the process: with nothing bound its
            // callback pulls silence, so a playback's first samples are heard as soon as they are
            // mixed (the device's buffer stays small for exactly that; #554). Reported ready only
            // once it plays: a device that fails to start is an error here, not a later surprise.
            if let Err(e) = stream.play() {
                let _ = ready_tx.send(Err(e.to_string()));
                return;
            }
            let _ = ready_tx.send(Ok(()));
            let mut current = 0u64;
            while let Ok(cmd) = rx.recv() {
                if !shared.serve(&mut current, cmd) {
                    break;
                }
            }
            drop(stream);
        })
        .map_err(|e| e.to_string())?;
    ready_rx.recv().map_err(|e| e.to_string())??;
    Ok(Output { key: out.clone(), rate, shared: shared_out, tx, thread })
}

/// Build `dev`'s stream: the callback pulls the bound feed (silence when none is bound), maps
/// it onto the device's channels and records the output latency; an error reports the stream
/// dead over `died`.
fn run<T: SizedSample + FromSample<f32>>(
    dev: &cpal::Device,
    cfg: &cpal::StreamConfig,
    shared: Arc<Shared>,
    map: (usize, usize),
    died: mpsc::Sender<Cmd>,
) -> Result<cpal::Stream, String> {
    let ch = cfg.channels as usize;
    // Output mapping: device channels (0-based) for left and right; out of range → 1 and 2.
    let (left, right) = if map.0 < ch && map.1 < ch { map } else { (0, 1) };
    let rate = cfg.sample_rate.0 as f64;
    let mut stereo: Vec<f32> = Vec::new();
    let alive = shared.clone();
    dev.build_output_stream(
        cfg,
        move |out: &mut [T], info: &cpal::OutputCallbackInfo| {
            let frames = out.len() / ch.max(1);
            stereo.resize(frames * 2, 0.0);
            match shared.feed() {
                Some(f) => f.pull(&mut stereo),
                None => stereo.iter_mut().for_each(|s| *s = 0.0),
            }
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
            shared.latency.store((dev_lat * rate) as u64 + frames as u64, Ordering::Relaxed);
        },
        move |e| {
            // ALSA and PulseAudio report recoverable polling and xrun errors here, and the worker
            // goes on: only a device that is gone ends the stream. A stall a platform reports as
            // something else is caught by `AudioPlayback::stalled`, which hands the preview back to
            // the frame clock, and the next playback rebuilds an output whose `Play` never lands.
            if matches!(e, cpal::StreamError::DeviceNotAvailable) {
                // Before the command, so an output that died is never attached again.
                alive.dead.store(true, Ordering::Release);
                let _ = died.send(Cmd::Died);
            }
            log::warn!("audio output: {e}");
        },
        None,
    )
    .map_err(|e| e.to_string())
}

pub struct CpalOut {
    out: AudioOutput,
    rate: u32,
    /// The process output this playback runs on, once started.
    running: Option<(Arc<Shared>, mpsc::Sender<Cmd>)>,
    /// The session of this playback within it (see [`Cmd::Clear`]).
    session: u64,
}

/// Open the output device (`None` without one). When the process output is already running for
/// these settings, nothing is touched: the device opens once per session (see the module doc),
/// so a preview's sound starts with the press that started it.
pub fn open(out: &AudioOutput) -> Option<Box<dyn AudioDevice>> {
    // The running output already knows the rate; otherwise resolve it (this is the enumeration a
    // first press, or one after a settings change, waits for).
    let rate = match running(out) {
        Some((rate, ..)) => Some(rate),
        None => resolve_rate(out),
    };
    Some(Box::new(CpalOut { out: out.clone(), rate: rate?, running: None, session: 0 }))
}

/// Open the output ahead of the first preview (the app does this at startup): the devices are
/// enumerated and the stream built now, so the first press doesn't wait for it. Fails without a
/// device; the first preview then opens it as before.
pub fn prewarm(out: &AudioOutput) -> Result<(), String> {
    let rate = resolve_rate(out).ok_or("no audio output device")?;
    // Warm up only when nothing runs yet: an output that already exists (a preview started first,
    // perhaps with newer settings) is left alone, and checking and building under one lock means
    // a preview cannot slip in between and have its stream replaced by this one.
    warm_up(out, rate)
}

/// Build the process output for `out` when none exists, under [`OUTPUT`]'s lock.
fn warm_up(out: &AudioOutput, rate: u32) -> Result<(), String> {
    let mut guard = output();
    if guard.is_some() {
        return Ok(());
    }
    let built = spawn_output(out, rate)?;
    *guard = Some(built);
    Ok(())
}

/// Bind `feed` to the process's output for `out` (building the output first when none matches),
/// under a new session whose stop unbinds it.
fn attach(out: &AudioOutput, rate: u32, feed: Arc<AudioFeed>) -> Result<((Arc<Shared>, mpsc::Sender<Cmd>), u64), String> {
    // Twice at most: an output that died between the check and the send is rebuilt by the second
    // `ensure` (its thread has finished by then).
    for _ in 0..2 {
        let (shared, tx) = ensure(out, rate)?;
        let session = SESSION.fetch_add(1, Ordering::Relaxed) + 1;
        if tx.send(Cmd::Play { session, feed: feed.clone() }).is_ok() && bound_within(&shared, session) {
            return Ok(((shared, tx), session));
        }
        // The stream is going away: its `Died` was queued before this `Play`, or it was already
        // dead when `ensure` looked. Stop it for good and build a new one, rather than leaving
        // the preview with a clock that never advances.
        shared.dead.store(true, Ordering::Release);
        let _ = tx.send(Cmd::Died);
        await_finished(out);
    }
    Err("the audio output went away".into())
}

/// Wait (bounded) until the callback serves `session`: the audio thread wakes on the command, so
/// this is microseconds when the stream is alive.
fn bound_within(shared: &Shared, session: u64) -> bool {
    for _ in 0..100 {
        if shared.bound.load(Ordering::Acquire) == session {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    false
}

/// Wait (bounded) until the dead output's thread has finished, so the next [`ensure`] builds a new
/// stream instead of racing the old one for the device.
fn await_finished(out: &AudioOutput) {
    for _ in 0..400 {
        let finished = {
            let guard = output();
            guard.as_ref().is_none_or(|o| o.key != *out || o.thread.is_finished())
        };
        if finished {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

impl AudioDevice for CpalOut {
    /// The rate the process output was opened at.
    fn sample_rate(&self) -> u32 {
        self.rate
    }

    /// Bind `feed` to the process output (building it first when none matches).
    fn start(&mut self, feed: Arc<AudioFeed>) -> Result<(), String> {
        self.stop();
        let ((shared, tx), session) = attach(&self.out, self.rate, feed)?;
        self.running = Some((shared, tx));
        self.session = session;
        Ok(())
    }

    /// Unbind this playback's feed; the stream stays open and pulls silence.
    fn stop(&mut self) {
        if let Some((_, tx)) = self.running.take() {
            let _ = tx.send(Cmd::Clear { session: self.session });
        }
    }

    /// The output latency the callback measured (0 until it has run).
    fn latency_frames(&self) -> u64 {
        self.running.as_ref().map(|(s, _)| s.latency.load(Ordering::Relaxed)).unwrap_or(0)
    }
}

impl Drop for CpalOut {
    /// Unbind the feed; the process output itself outlives every playback.
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Linux period stays about 10 ms across rates.
    #[test]
    fn preferred_buffer_is_about_ten_milliseconds() {
        assert_eq!(preferred_buffer_frames(48_000), 480);
        assert_eq!(preferred_buffer_frames(8_000), 256);
        assert_eq!(preferred_buffer_frames(192_000), 1920);
    }

    /// A playback binds its feed, and only its own session unbinds it.
    #[test]
    fn a_play_binds_its_feed_until_its_session_clears() {
        let s = Shared::default();
        let mut current = 0;
        let feed = Arc::new(AudioFeed::default());
        feed.push(&[0.1, 0.2]);
        assert!(s.serve(&mut current, Cmd::Play { session: 7, feed: feed.clone() }));
        assert_eq!(s.bound.load(Ordering::Acquire), 7, "the callback reports the session it serves");
        let bound = s.feed().expect("the feed is bound");
        let mut out = [0.0f32; 2];
        bound.pull(&mut out);
        assert_eq!(out, [0.1, 0.2]);

        // A newer play replaces it, and the old session's clear leaves the newer one playing.
        assert!(s.serve(&mut current, Cmd::Play { session: 8, feed: Arc::new(AudioFeed::default()) }));
        assert!(s.serve(&mut current, Cmd::Clear { session: 7 }));
        assert!(s.feed().is_some(), "a stale clear must not unbind the newer play");

        // Its own session clears it, and a dead stream stops serving.
        assert!(s.serve(&mut current, Cmd::Clear { session: 8 }));
        assert!(s.feed().is_none());
        assert_eq!(s.bound.load(Ordering::Acquire), 0, "nothing is served after its own clear");
        assert!(!s.serve(&mut current, Cmd::Died));
    }
}
