//! Audio measurements for `audio.analyze` (MCP `analyze_audio`), fed block by block with the
//! comp's stereo mixdown (the samples preview playback and export produce): sample peak, RMS,
//! integrated loudness (ITU-R BS.1770-4: K-weighting, 400 ms blocks with 75 % overlap, absolute
//! gate at −70 LKFS and relative gate 10 LU below), maximum momentary (400 ms) and short-term
//! (3 s) loudness, a true-peak estimate (4× oversampling), clipped samples, silent ranges and an
//! optional min/max waveform.
//!
//! Implemented from the published Recommendation (its 48 kHz filter coefficients); the mix is
//! always analysed at [`RATE`].

/// Analysis sample rate (Hz). The K-weighting coefficients below are the 48 kHz ones of
/// ITU-R BS.1770-4, Tables 1 and 2.
pub const RATE: u32 = 48_000;

#[derive(Clone, Copy, Debug, Default)]
struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
    z: [f64; 2],
}

impl Biquad {
    const fn new(b: [f64; 3], a: [f64; 2]) -> Biquad {
        Biquad { b, a, z: [0.0; 2] }
    }
    /// Transposed direct form II.
    fn run(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}

/// BS.1770 stage 1 (the head's acoustic effect: a high shelf) and stage 2 (RLB high-pass).
fn k_filter() -> [Biquad; 2] {
    [
        Biquad::new([1.535_124_859_586_97, -2.691_696_189_406_38, 1.198_392_810_852_85], [-1.690_659_293_182_41, 0.732_480_774_215_85]),
        Biquad::new([1.0, -2.0, 1.0], [-1.990_047_454_833_98, 0.990_072_250_366_21]),
    ]
}

/// Taps per phase of the true-peak interpolator.
const TP_TAPS: usize = 12;

/// Phases 1–3 of a 4× windowed-sinc interpolator (Blackman window, ±6 samples), each
/// normalised to unity DC gain.
fn tp_coeffs() -> [[f64; TP_TAPS]; 3] {
    let mut c = [[0.0; TP_TAPS]; 3];
    for (pi, row) in c.iter_mut().enumerate() {
        let frac = (pi + 1) as f64 / 4.0;
        for (k, v) in row.iter_mut().enumerate() {
            // Sample x[i − k] sits `u` samples from the interpolated point (i − 6 + frac).
            let u = k as f64 - 6.0 + frac;
            let sinc = if u.abs() < 1e-12 { 1.0 } else { (std::f64::consts::PI * u).sin() / (std::f64::consts::PI * u) };
            let w =
                if u.abs() >= 6.0 { 0.0 } else { 0.42 + 0.5 * (std::f64::consts::PI * u / 6.0).cos() + 0.08 * (2.0 * std::f64::consts::PI * u / 6.0).cos() };
            *v = sinc * w;
        }
        let sum: f64 = row.iter().sum();
        if sum.abs() > 1e-9 {
            row.iter_mut().for_each(|v| *v /= sum);
        }
    }
    c
}

/// Settings of an analysis.
#[derive(Clone, Copy, Debug)]
pub struct Settings {
    /// Total stereo frames that will be fed (for the waveform's columns).
    pub frames: u64,
    /// A 10 ms window is silent when both channels stay below this level (dBFS).
    pub silence_db: f64,
    /// Shortest silence reported (seconds).
    pub min_silence: f64,
    /// Waveform columns (0 = none).
    pub waveform: usize,
    /// Estimate the true peak (4× oversampling).
    pub true_peak: bool,
}

/// One channel's running statistics.
#[derive(Clone, Debug, Default)]
struct Channel {
    peak: f64,
    sumsq: f64,
    clipped: u64,
    k: [Biquad; 2],
    k_acc: f64,
    hist: [f64; TP_TAPS],
    hist_pos: usize,
    true_peak: f64,
}

/// The analyser: feed interleaved stereo blocks with [`Analyzer::push`], then [`Analyzer::finish`].
#[derive(Clone, Debug)]
pub struct Analyzer {
    set: Settings,
    ch: [Channel; 2],
    tp: [[f64; TP_TAPS]; 3],
    frames: u64,
    /// K-weighted power (summed over channels) of each complete 100 ms sub-block.
    subs: Vec<f64>,
    sub_n: usize,
    win_peak: f64,
    win_n: usize,
    win_start: u64,
    silent_from: Option<u64>,
    silences: Vec<(u64, u64)>,
    columns: Vec<[f32; 4]>,
}

/// Samples per 100 ms gating sub-block and per 10 ms silence window.
const SUB: usize = RATE as usize / 10;
const WIN: usize = RATE as usize / 100;

impl Analyzer {
    pub fn new(set: Settings) -> Analyzer {
        let k = k_filter();
        let ch = Channel { k, ..Default::default() };
        Analyzer {
            set,
            ch: [ch.clone(), ch],
            tp: tp_coeffs(),
            frames: 0,
            subs: vec![],
            sub_n: 0,
            win_peak: 0.0,
            win_n: 0,
            win_start: 0,
            silent_from: None,
            silences: vec![],
            columns: vec![[f32::INFINITY, f32::NEG_INFINITY, f32::INFINITY, f32::NEG_INFINITY]; set.waveform],
        }
    }

    /// Feed interleaved stereo samples (L R L R …).
    pub fn push(&mut self, block: &[f32]) {
        let silence_amp = 10f64.powf(self.set.silence_db / 20.0);
        for f in block.as_chunks::<2>().0 {
            let mut frame_peak = 0.0f64;
            for (c, &v) in self.ch.iter_mut().zip(f) {
                let x = v as f64;
                let a = x.abs();
                frame_peak = frame_peak.max(a);
                c.peak = c.peak.max(a);
                c.sumsq += x * x;
                if a >= 1.0 {
                    c.clipped += 1;
                }
                let [k0, k1] = &mut c.k;
                let y = k1.run(k0.run(x));
                c.k_acc += y * y;
                if self.set.true_peak {
                    c.hist_pos = (c.hist_pos + 1) % TP_TAPS;
                    if let Some(h) = c.hist.get_mut(c.hist_pos) {
                        *h = x;
                    }
                    c.true_peak = c.true_peak.max(a);
                    for row in &self.tp {
                        // x[i − k] is at ring position hist_pos − k.
                        let s: f64 = row.iter().enumerate().map(|(k, w)| w * c.hist.get((c.hist_pos + TP_TAPS - k) % TP_TAPS).copied().unwrap_or(0.0)).sum();
                        c.true_peak = c.true_peak.max(s.abs());
                    }
                }
            }
            // Waveform column of this frame.
            if !self.columns.is_empty() && self.set.frames > 0 {
                let col = (self.frames as u128 * self.columns.len() as u128 / self.set.frames as u128) as usize;
                if let Some(c) = self.columns.get_mut(col) {
                    c[0] = c[0].min(f[0]);
                    c[1] = c[1].max(f[0]);
                    c[2] = c[2].min(f[1]);
                    c[3] = c[3].max(f[1]);
                }
            }
            self.frames += 1;
            // 100 ms gating sub-blocks.
            self.sub_n += 1;
            if self.sub_n == SUB {
                let p: f64 = self.ch.iter_mut().map(|c| std::mem::take(&mut c.k_acc) / SUB as f64).sum();
                self.subs.push(p);
                self.sub_n = 0;
            }
            // 10 ms silence windows.
            self.win_peak = self.win_peak.max(frame_peak);
            self.win_n += 1;
            if self.win_n == WIN {
                self.close_window(silence_amp);
            }
        }
    }

    fn close_window(&mut self, silence_amp: f64) {
        if self.win_n == 0 {
            return;
        }
        if self.win_peak < silence_amp {
            self.silent_from.get_or_insert(self.win_start);
        } else if let Some(from) = self.silent_from.take() {
            self.silences.push((from, self.win_start));
        }
        self.win_start = self.frames;
        self.win_peak = 0.0;
        self.win_n = 0;
    }

    /// The measurements.
    pub fn finish(mut self) -> Report {
        let silence_amp = 10f64.powf(self.set.silence_db / 20.0);
        self.close_window(silence_amp);
        if let Some(from) = self.silent_from.take() {
            self.silences.push((from, self.frames));
        }
        let min = (self.set.min_silence.max(0.0) * RATE as f64).round() as u64;
        let silences = self.silences.iter().filter(|(a, b)| b - a >= min.max(1)).map(|&(a, b)| (secs(a), secs(b))).collect();
        let n = self.frames.max(1) as f64;
        let channels = self.ch.iter().map(|c| ChannelStats { peak: c.peak, rms: (c.sumsq / n).sqrt(), clipped: c.clipped, true_peak: c.true_peak }).collect();
        let rms = ((self.ch.iter().map(|c| c.sumsq).sum::<f64>()) / (2.0 * n)).sqrt();
        let block = |len: usize| -> Vec<f64> { self.subs.windows(len).map(|w| w.iter().sum::<f64>() / len as f64).collect() };
        let momentary = block(4);
        let short = block(30);
        let loud = |p: f64| -0.691 + 10.0 * p.log10();
        let max_loud = |v: &[f64]| v.iter().copied().filter(|p| *p > 0.0).fold(None, |m: Option<f64>, p| Some(m.map_or(p, |m| m.max(p)))).map(loud);
        Report {
            frames: self.frames,
            peak: self.ch.iter().map(|c| c.peak).fold(0.0, f64::max),
            true_peak: self.set.true_peak.then(|| self.ch.iter().map(|c| c.true_peak).fold(0.0, f64::max)),
            rms,
            integrated: integrated(&momentary),
            momentary_max: max_loud(&momentary),
            short_term_max: max_loud(&short),
            channels,
            silences,
            columns: self.columns.iter().map(|c| if c[0] > c[1] { [0.0; 4] } else { *c }).collect(),
        }
    }
}

fn secs(frames: u64) -> f64 {
    frames as f64 / RATE as f64
}

/// Gated integrated loudness (LKFS / LUFS) of 400 ms block powers; `None` when no block passes
/// the gates (silence, or less than 400 ms of audio).
pub fn integrated(blocks: &[f64]) -> Option<f64> {
    let loud = |p: f64| -0.691 + 10.0 * p.log10();
    let abs: Vec<f64> = blocks.iter().copied().filter(|&p| p > 0.0 && loud(p) > -70.0).collect();
    if abs.is_empty() {
        return None;
    }
    let rel = loud(abs.iter().sum::<f64>() / abs.len() as f64) - 10.0;
    let gated: Vec<f64> = abs.into_iter().filter(|&p| loud(p) > rel).collect();
    if gated.is_empty() {
        return None;
    }
    Some(loud(gated.iter().sum::<f64>() / gated.len() as f64))
}

#[derive(Clone, Debug)]
pub struct ChannelStats {
    pub peak: f64,
    pub rms: f64,
    pub clipped: u64,
    pub true_peak: f64,
}

/// What an analysis measured (linear amplitudes; [`db`] converts).
#[derive(Clone, Debug)]
pub struct Report {
    pub frames: u64,
    pub peak: f64,
    pub true_peak: Option<f64>,
    pub rms: f64,
    /// Integrated loudness, LUFS.
    pub integrated: Option<f64>,
    pub momentary_max: Option<f64>,
    pub short_term_max: Option<f64>,
    pub channels: Vec<ChannelStats>,
    /// Silent ranges in seconds from the start of the analysed span.
    pub silences: Vec<(f64, f64)>,
    /// Waveform columns `[min L, max L, min R, max R]`.
    pub columns: Vec<[f32; 4]>,
}

/// Amplitude → dBFS (`None` for silence).
pub fn db(a: f64) -> Option<f64> {
    (a > 0.0 && a.is_finite()).then(|| 20.0 * a.log10())
}

/// A waveform picture of `columns` (one per pixel column) as opaque RGBA8 `width`×`height`:
/// the left channel in the top half, the right in the bottom, with a centre line each and red
/// where a column reaches full scale.
pub fn waveform_rgba(columns: &[[f32; 4]], height: u32) -> (u32, u32, Vec<u8>) {
    let w = columns.len().max(1) as u32;
    let h = height.clamp(16, 2048);
    let mut px = vec![0u8; w as usize * h as usize * 4];
    let half = h / 2;
    let put = |px: &mut Vec<u8>, x: u32, y: u32, c: [u8; 3]| {
        let i = (y as usize * w as usize + x as usize) * 4;
        if let Some(p) = px.get_mut(i..i + 4) {
            p.copy_from_slice(&[c[0], c[1], c[2], 255]);
        }
    };
    for y in 0..h {
        for x in 0..w {
            put(&mut px, x, y, [24, 24, 28]);
        }
    }
    for (x, c) in columns.iter().enumerate() {
        let x = x as u32;
        for lane in 0..2u32 {
            let (lo, hi) = (c[lane as usize * 2] as f64, c[lane as usize * 2 + 1] as f64);
            let top = lane * half;
            let lane_h = if lane == 0 { half } else { h - half };
            let mid = top as f64 + lane_h as f64 / 2.0;
            let amp = (lane_h as f64 / 2.0 - 1.0).max(1.0);
            let y = |v: f64| (mid - v.clamp(-1.0, 1.0) * amp).round().clamp(top as f64, (top + lane_h - 1) as f64) as u32;
            put(&mut px, x, mid as u32, [70, 70, 80]);
            let clip = lo <= -1.0 || hi >= 1.0;
            let colour = if clip { [235, 70, 60] } else { [90, 200, 140] };
            for yy in y(hi)..=y(lo) {
                put(&mut px, x, yy, colour);
            }
        }
    }
    (w, h, px)
}
