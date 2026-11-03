//! Real-time audio preview.
//!
//! The frontend owns no audio API: the native app supplies an [`AudioDevice`] (cpal) through
//! [`crate::Hooks::audio_device`], the web app one on Web Audio. During preview playback an
//! [`AudioPlayback`] runs a feeder thread (wasm32: [`AudioPlayback::pump`] every UI frame) that
//! mixes the composition ahead of the device with the same mixdown export uses
//! (`effectcraft_render::audio::mix_comp`: Audio switch, solo, Audio Levels, audio effects) and
//! pushes it into an [`AudioFeed`]; the device callback pulls from the feed.
//!
//! **A/V sync:** while audio plays, the audio clock drives playback. The clock is the number of
//! sample frames the device has actually consumed (underruns do not advance it) minus the
//! device's output latency, mapped through the work-area loop ([`clock_sample`]); the viewer
//! shows the comp frame at that sample ([`frame_of_sample`]), dropping video frames rather
//! than drifting.
//!
//! The feed also tracks per-channel peaks for the Audio panel's VU meters ([`Meter`]).
//!
//! **Scrubbing:** Ctrl/Cmd-dragging the current-time indicator plays a short snippet of the mix
//! at each new frame ([`AudioScrub`], `playback.scrubAudio`), as in After Effects.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use effectcraft_engine::project::ItemId;
use effectcraft_engine::time::{FrameRate, Tick};

use crate::frames::RenderSource;

/// An audio output (implemented by the native app with cpal).
pub trait AudioDevice: Send {
    /// Device sample rate in Hz.
    fn sample_rate(&self) -> u32;
    /// Start pulling interleaved stereo frames from `feed` (see [`AudioFeed::pull`]).
    fn start(&mut self, feed: Arc<AudioFeed>) -> Result<(), String>;
    fn stop(&mut self);
    /// Output latency in sample frames (device buffering), for A/V sync.
    fn latency_frames(&self) -> u64 {
        0
    }
    /// Called every UI frame while playing: devices without an audio thread of their own (Web
    /// Audio) move queued samples to the output here.
    fn pump(&mut self) {}
}

/// Opens the default output device (`None` when there is none).
pub type AudioDeviceFactory = Box<dyn Fn(&AudioOutput) -> Option<Box<dyn AudioDevice>>>;

/// Which output to open (Settings ▸ Audio): device name (empty = the system default) and the
/// 1-based device channels for left and right.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioOutput {
    pub device: String,
    pub left: u32,
    pub right: u32,
    /// Preview Sample Rate: devices that support it are opened at this rate.
    pub rate: u32,
}

impl AudioOutput {
    pub fn from_prefs(p: &effectcraft_engine::prefs::Prefs) -> AudioOutput {
        AudioOutput { device: p.audio.output_device.clone(), left: p.audio.output_left, right: p.audio.output_right, rate: p.audio.preview_sample_rate }
    }
}

/// Lock-protected sample queue between the feeder thread and the device callback.
#[derive(Default)]
pub struct AudioFeed {
    queue: Mutex<VecDeque<f32>>,
    /// Sample frames handed to the device (real audio only: underruns do not count).
    consumed: AtomicU64,
    /// Peak |sample| per channel since the last [`AudioFeed::take_peaks`] (f32 bits).
    peak: [AtomicU32; 2],
    /// The feeder reached the end of a non-looping span.
    finished: AtomicBool,
}

impl AudioFeed {
    /// Fill `out` (interleaved stereo) from the queue; silence where the queue runs dry.
    pub fn pull(&self, out: &mut [f32]) {
        let mut real = 0usize;
        let mut pk = [0.0f32; 2];
        if let Ok(mut q) = self.queue.lock() {
            let n = q.len().min(out.len()) & !1;
            for (i, (o, s)) in out.iter_mut().zip(q.drain(..n)).enumerate() {
                *o = s;
                pk[i & 1] = pk[i & 1].max(s.abs());
            }
            real = n;
        }
        out[real..].iter_mut().for_each(|o| *o = 0.0);
        self.consumed.fetch_add((real / 2) as u64, Ordering::Relaxed);
        for (c, v) in pk.into_iter().enumerate() {
            // Rust 1.99 renamed `fetch_update` to `try_update`; keep the old name while the
            // workspace supports Rust 1.95 (`rust-version`).
            #[allow(deprecated)]
            let _ = self.peak[c].fetch_update(Ordering::Relaxed, Ordering::Relaxed, |old| (v > f32::from_bits(old)).then_some(v.to_bits()));
        }
    }

    /// Drop what is still queued (scrubbing replaces the snippet still playing).
    pub fn clear(&self) {
        if let Ok(mut q) = self.queue.lock() {
            q.clear();
        }
    }

    pub fn push(&self, samples: &[f32]) {
        if let Ok(mut q) = self.queue.lock() {
            q.extend(samples.iter().copied());
        }
    }

    /// Sample frames waiting in the queue.
    pub fn queued_frames(&self) -> usize {
        self.queue.lock().map(|q| q.len() / 2).unwrap_or(0)
    }

    pub fn consumed(&self) -> u64 {
        self.consumed.load(Ordering::Relaxed)
    }

    /// Peak |sample| per channel since the last call (resets them).
    pub fn take_peaks(&self) -> [f32; 2] {
        [f32::from_bits(self.peak[0].swap(0, Ordering::Relaxed)), f32::from_bits(self.peak[1].swap(0, Ordering::Relaxed))]
    }

    pub fn finished(&self) -> bool {
        self.finished.load(Ordering::Relaxed) && self.queued_frames() == 0
    }
}

/// What a preview plays, in comp sample indices at the device rate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlaySpan {
    pub start: i64,
    /// Work area [wa, wb).
    pub wa: i64,
    pub wb: i64,
    pub looping: bool,
}

impl PlaySpan {
    /// Span for comp times: start, work area [wa, wb).
    pub fn new(start: Tick, wa: Tick, wb: Tick, looping: bool, rate: u32) -> PlaySpan {
        let r = rate as i64;
        PlaySpan { start: start.to_units_floor(r), wa: wa.to_units_floor(r), wb: wb.to_units_floor(r).max(wa.to_units_floor(r) + 1), looping }
    }
}

/// The comp sample index audible now: `consumed` frames played by the device minus its
/// `latency`, from the span start, wrapped into the work area when looping (clamped at its end
/// otherwise).
pub fn clock_sample(span: &PlaySpan, consumed: u64, latency: u64) -> i64 {
    let s = span.start + consumed.saturating_sub(latency) as i64;
    if s < span.wb {
        return s;
    }
    if span.looping { span.wa + (s - span.wb).rem_euclid((span.wb - span.wa).max(1)) } else { span.wb - 1 }
}

/// Comp time of sample index `s` at `rate` Hz.
pub fn sample_time(s: i64, rate: u32) -> Tick {
    Tick::from_units(s, rate.max(1) as i64)
}

/// The comp frame showing sample `s` (the frame whose interval contains it).
pub fn frame_of_sample(s: i64, rate: u32, fps: FrameRate) -> i64 {
    fps.frame_at(sample_time(s, rate))
}

/// Block size the feeder mixes at a time.
const BLOCK: usize = 1024;

/// A running audio preview: device + feeder thread.
pub struct AudioPlayback {
    pub feed: Arc<AudioFeed>,
    device: Box<dyn AudioDevice>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    /// wasm32 (no threads): what the feeder thread would own, run by [`AudioPlayback::pump`].
    feeder: Option<(RenderSource, ItemId, i64, u32)>,
    pub rate: u32,
    pub span: PlaySpan,
}

/// Audio kept queued ahead of the device: ~200 ms (a feeder thread tops it up continuously);
/// wasm32 tops it up once per UI frame, so it keeps ~350 ms to ride out slow frames.
fn queue_target(rate: u32) -> usize {
    if cfg!(target_arch = "wasm32") { rate as usize * 7 / 20 } else { rate as usize / 5 }
}

impl AudioPlayback {
    /// Start mixing `comp` over `span` into `device`.
    /// `mix_rate`: Settings ▸ Audio ▸ Preview Sample Rate, the rate the comp's audio is mixed
    /// at (resampled to the device's rate when they differ; 0 = the device's rate).
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        mut device: Box<dyn AudioDevice>,
        src: RenderSource,
        comp: ItemId,
        start: Tick,
        wa: Tick,
        wb: Tick,
        looping: bool,
        mix_rate: u32,
    ) -> Result<AudioPlayback, String> {
        let rate = device.sample_rate().max(8000);
        let mix = if mix_rate == 0 { rate } else { mix_rate.clamp(8000, 192_000) };
        let span = PlaySpan::new(start, wa, wb, looping, rate);
        let feed = Arc::new(AudioFeed::default());
        let stop = Arc::new(AtomicBool::new(false));
        if cfg!(target_arch = "wasm32") {
            // The device takes samples only in `pump`, which fills the queue first.
            let mut p = AudioPlayback { feed: feed.clone(), device, stop, thread: None, feeder: Some((src, comp, span.start, mix)), rate, span };
            p.device.start(feed)?;
            p.pump();
            return Ok(p);
        }
        // All mixing happens on the feeder thread, the first block too: reading footage for the
        // first time opens it (a FLAC or MP3 file decodes whole), which froze the UI for as long
        // as that took (#408). The device plays silence until the first block arrives, and the
        // clock doesn't count it.
        let thread = {
            let (feed, stop) = (feed.clone(), stop.clone());
            std::thread::Builder::new()
                .name("ec-audio-feed".into())
                .spawn(move || {
                    let mut cursor = span.start;
                    let target = queue_target(rate);
                    while !stop.load(Ordering::Relaxed) {
                        if feed.queued_frames() < target && !feed.finished.load(Ordering::Relaxed) {
                            cursor = feed_block(&src, comp, &feed, &span, cursor, rate, mix);
                        } else {
                            std::thread::sleep(std::time::Duration::from_millis(4));
                        }
                    }
                })
                .map_err(|e| e.to_string())?
        };
        device.start(feed.clone())?;
        Ok(AudioPlayback { feed, device, stop, thread: Some(thread), feeder: None, rate, span })
    }

    /// Every UI frame while playing: on wasm32 mix ahead into the feed (what the feeder thread
    /// does elsewhere), then let the device take what it needs.
    pub fn pump(&mut self) {
        if let Some((src, comp, cursor, mix)) = &mut self.feeder {
            let target = queue_target(self.rate);
            while self.feed.queued_frames() < target && !self.feed.finished.load(Ordering::Relaxed) {
                *cursor = feed_block(src, *comp, &self.feed, &self.span, *cursor, self.rate, *mix);
            }
        }
        self.device.pump();
    }

    /// The comp sample index audible now.
    pub fn clock(&self) -> i64 {
        clock_sample(&self.span, self.feed.consumed(), self.device.latency_frames())
    }

    pub fn finished(&self) -> bool {
        self.feed.finished()
    }

    pub fn stop(&mut self) {
        self.device.stop();
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for AudioPlayback {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Mix one block at `cursor` into the feed; returns the next cursor (wrapping or finishing at
/// the work-area end).
fn feed_block(src: &RenderSource, comp: ItemId, feed: &AudioFeed, span: &PlaySpan, cursor: i64, rate: u32, mix: u32) -> i64 {
    let mut cursor = cursor;
    if cursor >= span.wb {
        if !span.looping {
            feed.finished.store(true, Ordering::Relaxed);
            return cursor;
        }
        cursor = span.wa;
    }
    let n = ((span.wb - cursor) as usize).min(BLOCK);
    feed.push(&mix_block(src, comp, sample_time(cursor, rate), n, rate, mix));
    cursor + n as i64
}

/// `n` stereo frames of `comp`'s mix from comp time `t` at the device `rate`, mixed at `mix` Hz
/// (resampled when they differ).
fn mix_block(src: &RenderSource, comp: ItemId, t: Tick, n: usize, rate: u32, mix: u32) -> Vec<f32> {
    let render =
        |frames: usize, at: u32| effectcraft_engine::render::audio::mix_comp(&src.project, src.footage.as_ref(), src.expr.as_deref(), comp, t, frames, at);
    if mix == rate {
        return render(n, rate);
    }
    let m = (n as u64 * mix as u64).div_ceil(rate as u64) as usize + 1;
    resample_stereo(&render(m, mix), mix, rate, n)
}

/// Audio scrubbing: an open output that plays a short, faded snippet of the comp's mix each time
/// the current time moves to another frame (each snippet replaces what is still queued, so the
/// sound follows the pointer without lagging behind).
pub struct AudioScrub {
    pub feed: Arc<AudioFeed>,
    device: Box<dyn AudioDevice>,
    pub rate: u32,
    /// The comp and frame last played (holding still repeats nothing).
    last: Option<(ItemId, i64)>,
    /// When it last played (UI seconds): an idle scrub closes the device.
    pub last_used: f64,
}

impl AudioScrub {
    pub fn open(mut device: Box<dyn AudioDevice>, now: f64) -> Result<AudioScrub, String> {
        let rate = device.sample_rate().max(8000);
        let feed = Arc::new(AudioFeed::default());
        device.start(feed.clone())?;
        Ok(AudioScrub { feed, device, rate, last: None, last_used: now })
    }

    /// Play the frame of `comp` at `t` (one frame long, 30–100 ms) unless it was the last one
    /// played. Returns whether a snippet was queued.
    pub fn play(&mut self, src: &RenderSource, comp: ItemId, t: Tick, mix_rate: u32, now: f64) -> bool {
        self.last_used = now;
        let Some(c) = src.project.comp(comp) else { return false };
        let frame = c.frame_rate.frame_at(t);
        if self.last == Some((comp, frame)) {
            return false;
        }
        self.last = Some((comp, frame));
        let rate = self.rate;
        let mix = if mix_rate == 0 { rate } else { mix_rate.clamp(8000, 192_000) };
        let n = (c.frame_duration().seconds().clamp(0.03, 0.1) * rate as f64) as usize;
        let mut buf = mix_block(src, comp, c.frame_rate.tick_of(frame), n, rate, mix);
        // 3 ms fades: no clicks where snippets meet.
        let fade = (rate as usize * 3 / 1000).clamp(1, n / 2 + 1);
        let frames = buf.len() / 2;
        for i in 0..frames {
            let g = (i.min(frames - 1 - i) as f32 / fade as f32).min(1.0);
            buf[2 * i] *= g;
            buf[2 * i + 1] *= g;
        }
        self.feed.clear();
        self.feed.push(&buf);
        true
    }

    /// Every UI frame while open (Web Audio moves queued samples to the output here).
    pub fn pump(&mut self) {
        self.device.pump();
    }
}

impl Drop for AudioScrub {
    fn drop(&mut self) {
        self.device.stop();
    }
}

/// Linear resampling of interleaved stereo from `from` Hz to `n` frames at `to` Hz.
pub fn resample_stereo(src: &[f32], from: u32, to: u32, n: usize) -> Vec<f32> {
    let frames = src.len() / 2;
    let mut out = Vec::with_capacity(n * 2);
    if frames == 0 {
        out.resize(n * 2, 0.0);
        return out;
    }
    let step = from as f64 / to as f64;
    for i in 0..n {
        let x = i as f64 * step;
        let a = (x.floor() as usize).min(frames - 1);
        let b = (a + 1).min(frames - 1);
        let f = (x - a as f64) as f32;
        for c in 0..2 {
            out.push(src[a * 2 + c] * (1.0 - f) + src[b * 2 + c] * f);
        }
    }
    out
}

/// VU meter state for the Audio panel: smoothed level with a falling decay and peak hold, in
/// dBFS per channel.
#[derive(Clone, Copy, Debug)]
pub struct Meter {
    pub level_db: [f32; 2],
    pub peak_db: [f32; 2],
    peak_at: [f64; 2],
    /// Clip indicators (latched until clicked / playback restarts).
    pub clipped: [bool; 2],
    last: f64,
}

impl Default for Meter {
    fn default() -> Self {
        Meter { level_db: [METER_FLOOR; 2], peak_db: [METER_FLOOR; 2], peak_at: [0.0; 2], clipped: [false; 2], last: 0.0 }
    }
}

/// Bottom of the meter scale (dB).
pub const METER_FLOOR: f32 = -48.0;
/// How long the peak marker holds (s) and how fast levels fall (dB/s).
const PEAK_HOLD: f64 = 1.5;
const FALL_DB_PER_S: f32 = 30.0;

pub fn to_db(v: f32) -> f32 {
    if v <= 1e-6 { METER_FLOOR } else { (20.0 * v.log10()).max(METER_FLOOR) }
}

impl Meter {
    /// Feed the peaks measured since the last update at wall time `now` (seconds).
    pub fn update(&mut self, peaks: [f32; 2], now: f64) {
        let dt = if self.last > 0.0 { (now - self.last).clamp(0.0, 0.5) as f32 } else { 0.0 };
        self.last = now;
        for c in 0..2 {
            let db = to_db(peaks[c]);
            self.level_db[c] = db.max(self.level_db[c] - FALL_DB_PER_S * dt);
            if db >= self.peak_db[c] || now - self.peak_at[c] > PEAK_HOLD {
                self.peak_db[c] = db.max(self.level_db[c]);
                self.peak_at[c] = now;
            }
            if peaks[c] >= 1.0 {
                self.clipped[c] = true;
            }
        }
    }

    /// Anything to animate (levels still falling)?
    pub fn active(&self) -> bool {
        self.level_db.iter().chain(self.peak_db.iter()).any(|d| *d > METER_FLOOR)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_follows_consumed_frames_and_loops() {
        let span = PlaySpan { start: 4800, wa: 0, wb: 48000, looping: true };
        assert_eq!(clock_sample(&span, 0, 0), 4800);
        // Latency holds the clock back until the device has played through its buffer.
        assert_eq!(clock_sample(&span, 512, 1024), 4800);
        assert_eq!(clock_sample(&span, 2048, 1024), 4800 + 1024);
        // Wrap at the work-area end.
        assert_eq!(clock_sample(&span, 48000 - 4800 + 100, 0), 100);
        let once = PlaySpan { looping: false, ..span };
        assert_eq!(clock_sample(&once, 100_000, 0), 47999);
    }

    #[test]
    fn frame_mapping_matches_comp_frames() {
        let fps = FrameRate::new(30, 1);
        // 48 kHz at 30 fps: 1600 samples per frame.
        assert_eq!(frame_of_sample(0, 48000, fps), 0);
        assert_eq!(frame_of_sample(1599, 48000, fps), 0);
        assert_eq!(frame_of_sample(1600, 48000, fps), 1);
        assert_eq!(frame_of_sample(48000 * 2, 48000, fps), 60);
        let span = PlaySpan::new(Tick::from_seconds_f64(1.0), Tick::ZERO, Tick::from_seconds_f64(2.0), true, 48000);
        assert_eq!((span.start, span.wa, span.wb), (48000, 0, 96000));
        // 29.97: frame boundaries are fractional in samples.
        let ntsc = FrameRate::new(30000, 1001);
        assert_eq!(frame_of_sample(1601, 48000, ntsc), 0);
        assert_eq!(frame_of_sample(1602, 48000, ntsc), 1);
    }

    #[test]
    fn feed_counts_real_frames_and_peaks() {
        let f = AudioFeed::default();
        f.push(&[0.5, -0.25, 0.1, 0.9]);
        let mut out = [1.0f32; 8];
        f.pull(&mut out);
        assert_eq!(out, [0.5, -0.25, 0.1, 0.9, 0.0, 0.0, 0.0, 0.0]);
        assert_eq!(f.consumed(), 2);
        assert_eq!(f.take_peaks(), [0.5, 0.9]);
        assert_eq!(f.take_peaks(), [0.0, 0.0]);
    }

    /// #408: starting a preview with sound returns at once even when reading the footage takes
    /// long (the first read of a long FLAC decodes all of it): the first block was mixed on the
    /// calling (UI) thread. The sound arrives from the feeder thread, and the clock waits for it.
    #[test]
    fn starting_audio_does_not_wait_for_slow_footage() {
        use effectcraft_engine::color::Label;
        use effectcraft_engine::project::{Comp, Footage, FootageKind, ItemKind, LayerSource, Project, build};
        use effectcraft_engine::render::FootageSource;
        use effectcraft_engine::render::cache::LayerCache;
        use std::time::{Duration, Instant};

        /// Footage audio that takes `delay` to read the first time (then a constant 0.5).
        struct Slow {
            delay: Duration,
            opened: Mutex<bool>,
        }
        impl FootageSource for Slow {
            fn frame(&self, _: ItemId, _: &Footage, _: Tick) -> Option<Arc<effectcraft_engine::raster::Image>> {
                None
            }
            fn audio(&self, _: ItemId, _: &Footage, _: Tick, frames: usize, _: u32) -> Option<Vec<f32>> {
                let mut opened = self.opened.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                if !*opened {
                    std::thread::sleep(self.delay);
                    *opened = true;
                }
                Some(vec![0.5; frames * 2])
            }
        }
        /// An output that only takes the feed.
        struct Device;
        impl AudioDevice for Device {
            fn sample_rate(&self) -> u32 {
                48000
            }
            fn start(&mut self, _: Arc<AudioFeed>) -> Result<(), String> {
                Ok(())
            }
            fn stop(&mut self) {}
        }

        let mut p = Project::default();
        let footage =
            Footage { path: "long.flac".into(), kind: FootageKind::Audio, duration: Tick::from_seconds_f64(600.0), has_audio: true, ..Default::default() };
        let item = p.add_item("long.flac", Label::SeaFoam, None, ItemKind::Footage(footage));
        let mut comp = Comp::new(64, 36, FrameRate::new(30, 1), Tick::from_seconds_f64(10.0));
        let layer = build::layer(&mut p, &comp, "long.flac", LayerSource::Footage { item }, (0, 0), None);
        comp.layers = vec![layer].into();
        let cid = p.add_item("Comp", Label::Sandstone, None, ItemKind::Comp(comp.into()));
        let src = RenderSource {
            project: Arc::new(p),
            footage: Arc::new(Slow { delay: Duration::from_secs(2), opened: Mutex::new(false) }),
            expr: None,
            layer_cache: Arc::new(LayerCache::default()),
            gpu: None,
            gpu_display: false,
            disk: None,
        };
        let t = Instant::now();
        let mut playback =
            AudioPlayback::start(Box::new(Device), src, cid, Tick::ZERO, Tick::ZERO, Tick::from_seconds_f64(10.0), true, 0).expect("the preview starts");
        assert!(t.elapsed() < Duration::from_secs(1), "start waited {:?} for the footage", t.elapsed());
        assert_eq!(playback.clock(), 0, "the clock waits for the sound");
        // The feeder thread delivers it once the footage is open.
        while playback.feed.queued_frames() == 0 {
            assert!(t.elapsed() < Duration::from_secs(30), "the feeder never delivered");
            std::thread::sleep(Duration::from_millis(10));
        }
        let mut out = [0.0f32; 8];
        playback.feed.pull(&mut out);
        assert_eq!(out, [0.5; 8]);
        playback.stop();
    }

    #[test]
    fn meter_decays_and_holds_peaks() {
        let mut m = Meter::default();
        m.update([1.0, 0.5], 1.0);
        assert!((m.level_db[0] - 0.0).abs() < 1e-4 && (m.level_db[1] + 6.0206).abs() < 1e-3);
        assert!(m.clipped[0] && !m.clipped[1]);
        m.update([0.0, 0.0], 1.1);
        assert!((m.level_db[0] + 3.0).abs() < 1e-3, "{}", m.level_db[0]);
        assert_eq!(m.peak_db[0], 0.0, "peak holds");
        // After the hold time the peak marker follows the falling level.
        m.update([0.0, 0.0], 3.0);
        assert!(m.peak_db[0] < -10.0, "{}", m.peak_db[0]);
    }
}
