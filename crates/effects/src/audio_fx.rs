//! Audio effects (Effect > Audio): Backwards, Bass & Treble, Compressor, Delay, Distortion,
//! Flange & Chorus, Gate, High-Low Pass, Modulator, Parametric EQ, Reverb, Stereo Mixer and Tone.
//!
//! They are ordinary effect instances on a layer (so their parameters animate and show in
//! Effect Controls), pass video through unchanged, and are applied to the layer's audio by the
//! composition mixdown (`effectcraft_render::audio`), which preview playback and export share.
//!
//! The mixdown renders in short blocks. To keep the result independent of block boundaries
//! every effect is stateless across calls: the mixer feeds [`preroll`] seconds of the layer's
//! audio before the block, runs the chain from a cleared state and keeps the tail.
//!
//! DSP is textbook: second-order sections use the public RBJ "Audio EQ Cookbook" formulas;
//! Reverb is a Schroeder/Moorer network (parallel low-pass feedback combs into series
//! all-passes, as in the published Freeverb description); chorus/flange/vibrato are modulated
//! fractional delay lines. Compressor and Gate are feed-forward dynamics processors on a
//! stereo-linked peak envelope (one-pole attack/release smoothing, the textbook quadratic soft-knee
//! gain computer, an optional program-dependent "auto" release and an output ceiling); Gate mutes
//! below its threshold with attack / hold / release; Distortion is a static waveshaper (soft and
//! hard clip, two saturation curves, tube, fuzz) with pre-gain, bit reduction, sample-and-hold
//! downsampling, dry/wet mix and output volume.

use std::f64::consts::PI;

use effectcraft_keyframe::Value;
use effectcraft_project::ParamUi;

use crate::{Buf, EffectCtx, EffectSpec, Params, num, p, popup, slider};

fn spec(id: &'static str, name: &'static str, params: Vec<crate::ParamSpec>) -> EffectSpec {
    EffectSpec { id, name, category: "Audio", params, render: passthrough, gpu: false, float: true }
}

fn passthrough(_: &EffectCtx, b: Buf) -> Buf {
    b
}

fn pct(default: f64) -> (Value, ParamUi) {
    (num(default), slider(0.0, 100.0, 0.0, 100.0, 2))
}

fn pp(id: &'static str, name: &'static str, v: (Value, ParamUi)) -> crate::ParamSpec {
    p(id, name, v.0, v.1)
}

/// Distortion's Type options.
pub const DISTORTION_TYPES: [&str; 6] = ["Soft Clip", "Hard Clip", "Saturation 1", "Saturation 2", "Tube", "Fuzz"];

pub fn specs() -> Vec<EffectSpec> {
    const BANDS: [[&str; 8]; 3] = [
        ["band1Enable", "Band 1 Enabled", "band1Frequency", "Band 1 Frequency", "band1Bandwidth", "Band 1 Bandwidth", "band1BoostCut", "Band 1 Boost/Cut"],
        ["band2Enable", "Band 2 Enabled", "band2Frequency", "Band 2 Frequency", "band2Bandwidth", "Band 2 Bandwidth", "band2BoostCut", "Band 2 Boost/Cut"],
        ["band3Enable", "Band 3 Enabled", "band3Frequency", "Band 3 Frequency", "band3Bandwidth", "Band 3 Bandwidth", "band3BoostCut", "Band 3 Boost/Cut"],
    ];
    let mut eq = vec![];
    for (b, (f, on)) in BANDS.iter().zip([(200.0, true), (1000.0, false), (5000.0, false)]) {
        eq.push(p(b[0], b[1], Value::Bool(on), ParamUi::Checkbox));
        eq.push(p(b[2], b[3], num(f), slider(20.0, 20000.0, 20.0, 20000.0, 1)));
        eq.push(p(b[4], b[5], num(30.0), slider(1.0, 100.0, 1.0, 100.0, 1)));
        eq.push(p(b[6], b[7], num(0.0), slider(-24.0, 24.0, -24.0, 24.0, 2)));
    }
    vec![
        spec("ec.audio.backwards", "Backwards", vec![p("swapChannels", "Swap Channels", Value::Bool(false), ParamUi::Checkbox)]),
        spec(
            "ec.audio.basstreble",
            "Bass & Treble",
            vec![
                p("bass", "Bass", num(0.0), slider(-100.0, 100.0, -100.0, 100.0, 2)),
                p("treble", "Treble", num(0.0), slider(-100.0, 100.0, -100.0, 100.0, 2)),
            ],
        ),
        spec(
            "ec.audio.compressor",
            "Compressor",
            vec![
                p("threshold", "Threshold (dB)", num(-16.0), slider(-60.0, 0.0, -60.0, 0.0, 2)),
                p("ratio", "Ratio (x:1)", num(3.0), slider(0.4, 30.0, 0.4, 30.0, 2)),
                p("knee", "Knee (dB)", num(15.0), slider(0.0, 30.0, 0.0, 30.0, 2)),
                p("attack", "Attack (ms)", num(6.0), slider(0.0, 400.0, 0.0, 400.0, 2)),
                p("release", "Release (ms)", num(440.0), slider(1.0, 4000.0, 1.0, 4000.0, 2)),
                p("autoRelease", "Auto Release", Value::Bool(true), ParamUi::Checkbox),
                p("makeupGain", "Makeup Gain (dB)", num(0.0), slider(-30.0, 30.0, -30.0, 30.0, 2)),
                p("outputLimit", "Output Limit (dB)", num(0.0), slider(-30.0, 0.0, -30.0, 0.0, 2)),
            ],
        ),
        spec(
            "ec.audio.delay",
            "Delay",
            vec![
                p("delayTime", "Delay Time (milliseconds)", num(500.0), slider(1.0, 10000.0, 1.0, 2000.0, 2)),
                pp("delayAmount", "Delay Amount", pct(50.0)),
                pp("feedback", "Feedback", pct(50.0)),
                pp("dryOut", "Dry Out", pct(75.0)),
                pp("wetOut", "Wet Out", pct(75.0)),
            ],
        ),
        spec(
            "ec.audio.distortion",
            "Distortion",
            vec![
                p("distortionType", "Type", Value::Enum(0), popup(&DISTORTION_TYPES)),
                pp("drive", "Drive", pct(25.0)),
                p("gain", "Gain", num(25.0), slider(0.0, 300.0, 0.0, 300.0, 2)),
                pp("mix", "Mix", pct(100.0)),
                p("volume", "Volume", num(11.0), slider(0.0, 100.0, 0.0, 100.0, 2)),
                p("bitDepth", "Resolution (bit)", num(24.0), slider(1.0, 24.0, 1.0, 24.0, 0)),
                p("downsample", "Downsample (x)", num(1.0), slider(1.0, 64.0, 1.0, 32.0, 0)),
            ],
        ),
        spec(
            "ec.audio.flangechorus",
            "Flange & Chorus",
            vec![
                p("voiceSeparationTime", "Voice Separation Time (ms)", num(3.0), slider(0.0, 100.0, 0.0, 50.0, 2)),
                p("voices", "Voices", num(1.0), slider(1.0, 8.0, 1.0, 8.0, 0)),
                p("modulationRate", "Modulation Rate", num(0.4), slider(0.0, 10.0, 0.0, 5.0, 2)),
                pp("modulationDepth", "Modulation Depth", pct(50.0)),
                p("voicePhaseChange", "Voice Phase Change", num(0.0), ParamUi::Angle),
                p("invertPhase", "Invert Phase", Value::Bool(false), ParamUi::Checkbox),
                p("stereoVoices", "Stereo Voices", Value::Bool(false), ParamUi::Checkbox),
                pp("dryOut", "Dry Out", pct(50.0)),
                pp("wetOut", "Wet Out", pct(50.0)),
            ],
        ),
        spec(
            "ec.audio.gate",
            "Gate",
            vec![
                p("threshold", "Threshold (dB)", num(-60.0), slider(-60.0, 0.0, -60.0, 0.0, 2)),
                p("attack", "Attack (ms)", num(10.0), slider(0.0, 1000.0, 0.0, 1000.0, 2)),
                p("hold", "Hold (ms)", num(40.0), slider(0.0, 2500.0, 0.0, 2500.0, 2)),
                p("release", "Release (ms)", num(220.0), slider(5.0, 4000.0, 5.0, 4000.0, 2)),
            ],
        ),
        spec(
            "ec.audio.highlowpass",
            "High-Low Pass",
            vec![
                p("filterOptions", "Filter Options", Value::Enum(0), popup(&["High Pass", "Low Pass"])),
                p("cutoffFrequency", "Cutoff Frequency", num(2000.0), slider(20.0, 20000.0, 20.0, 20000.0, 1)),
                pp("dryOut", "Dry Out", pct(0.0)),
                pp("wetOut", "Wet Out", pct(100.0)),
            ],
        ),
        spec(
            "ec.audio.modulator",
            "Modulator",
            vec![
                p("modulationType", "Modulation Type", Value::Enum(0), popup(&["Sine", "Triangle"])),
                p("modulationRate", "Modulation Rate", num(10.0), slider(0.0, 100.0, 0.0, 20.0, 2)),
                pp("modulationDepth", "Modulation Depth", pct(5.0)),
                pp("amplitudeModulation", "Amplitude Modulation", pct(0.0)),
            ],
        ),
        spec("ec.audio.parametriceq", "Parametric EQ", eq),
        spec(
            "ec.audio.reverb",
            "Reverb",
            vec![
                p("reverbTime", "Reverb Time (milliseconds)", num(100.0), slider(1.0, 5000.0, 1.0, 1000.0, 2)),
                pp("diffusion", "Diffusion", pct(75.0)),
                pp("decay", "Decay", pct(25.0)),
                pp("brightness", "Brightness", pct(10.0)),
                pp("dryOut", "Dry Out", pct(90.0)),
                pp("wetOut", "Wet Out", pct(10.0)),
            ],
        ),
        spec(
            "ec.audio.stereomixer",
            "Stereo Mixer",
            vec![
                p("leftLevel", "Left Level", num(100.0), slider(0.0, 400.0, 0.0, 200.0, 2)),
                p("rightLevel", "Right Level", num(100.0), slider(0.0, 400.0, 0.0, 200.0, 2)),
                p("leftPan", "Left Pan", num(-100.0), slider(-100.0, 100.0, -100.0, 100.0, 2)),
                p("rightPan", "Right Pan", num(100.0), slider(-100.0, 100.0, -100.0, 100.0, 2)),
                p("invertPhase", "Invert Phase", Value::Bool(false), ParamUi::Checkbox),
            ],
        ),
        spec(
            "ec.audio.tone",
            "Tone",
            vec![
                p("waveformOptions", "Waveform Options", Value::Enum(0), popup(&["Sine", "Triangle", "Saw", "Square", "White Noise"])),
                p("frequency1", "Frequency 1", num(440.0), slider(0.0, 22000.0, 0.0, 2000.0, 2)),
                p("frequency2", "Frequency 2", num(493.88), slider(0.0, 22000.0, 0.0, 2000.0, 2)),
                p("frequency3", "Frequency 3", num(587.33), slider(0.0, 22000.0, 0.0, 2000.0, 2)),
                p("frequency4", "Frequency 4", num(659.26), slider(0.0, 22000.0, 0.0, 2000.0, 2)),
                p("frequency5", "Frequency 5", num(783.99), slider(0.0, 22000.0, 0.0, 2000.0, 2)),
                pp("level", "Level", pct(20.0)),
            ],
        ),
    ]
}

/// Is `id` an audio effect (applied by the mixdown rather than the compositor)?
pub fn is_audio_effect(id: &str) -> bool {
    id.starts_with("ec.audio.")
}

/// Does the effect produce sound on its own (so a layer without audio becomes audible)?
pub fn generates_audio(id: &str) -> bool {
    id == "ec.audio.tone"
}

/// Does the effect play the layer's audio backwards (a time mapping the mixer applies)?
pub fn reverses(id: &str) -> bool {
    id == "ec.audio.backwards"
}

/// Seconds of input history the effect needs before a block to reach the state it would have
/// had when processing continuously.
pub fn preroll(id: &str, params: &Params) -> f64 {
    match id {
        "ec.audio.basstreble" | "ec.audio.highlowpass" | "ec.audio.parametriceq" => 0.05,
        "ec.audio.flangechorus" | "ec.audio.modulator" => 0.06,
        "ec.audio.distortion" => 0.01,
        // The envelope settles within a few attack/release time constants.
        "ec.audio.compressor" | "ec.audio.gate" => {
            let auto = id == "ec.audio.compressor" && params.b("autoRelease");
            let rel = if auto { AUTO_RELEASE_SLOW_MS } else { params.f("release").max(1.0) } / 1000.0;
            let att = params.f("attack").max(0.0) / 1000.0;
            let hold = if id == "ec.audio.gate" { params.f("hold").max(0.0) / 1000.0 } else { 0.0 };
            (9.0 * rel + 9.0 * att + hold + 0.01).min(10.0)
        }
        "ec.audio.delay" => {
            let d = params.f("delayTime").max(1.0) / 1000.0;
            let fb = (params.f("feedback") / 100.0).clamp(0.0, 0.99);
            let taps = if fb > 1e-3 { (1e-3f64.ln() / fb.ln()).ceil() + 1.0 } else { 1.0 };
            (d * taps).min(10.0)
        }
        "ec.audio.reverb" => {
            let (fb, _, _, pre) = reverb_params(params);
            // T60 of the longest comb (~37 ms) with loop gain fb, plus the pre-delay.
            let t60 = if fb > 1e-3 { -3.0 * 0.0367 / fb.log10() } else { 0.05 };
            (t60 + pre).min(4.0)
        }
        _ => 0.0,
    }
}

/// Run audio effect `id` over interleaved stereo `buf` (L R L R …) at `rate` Hz, from a cleared
/// state. `start` is the layer time (seconds) of the first sample (oscillator phase).
pub fn process(id: &str, params: &Params, buf: &mut [f32], rate: u32, start: f64) {
    if rate == 0 || buf.len() < 2 {
        return;
    }
    let sr = rate as f64;
    match id {
        "ec.audio.backwards" if params.b("swapChannels") => {
            buf.as_chunks_mut::<2>().0.iter_mut().for_each(|f| f.swap(0, 1));
        }
        "ec.audio.basstreble" => {
            let bass = params.f("bass") / 100.0 * 12.0;
            let treble = params.f("treble") / 100.0 * 12.0;
            if bass != 0.0 {
                run_biquad(buf, Biquad::low_shelf(sr, 200.0, bass));
            }
            if treble != 0.0 {
                run_biquad(buf, Biquad::high_shelf(sr, 4000.0, treble));
            }
        }
        "ec.audio.highlowpass" => {
            let fc = params.f("cutoffFrequency").clamp(1.0, sr * 0.49);
            let q = std::f64::consts::FRAC_1_SQRT_2;
            let bq = if params.e("filterOptions") == 1 { Biquad::low_pass(sr, fc, q) } else { Biquad::high_pass(sr, fc, q) };
            let dry = (params.f("dryOut") / 100.0) as f32;
            let wet = (params.f("wetOut") / 100.0) as f32;
            let orig = buf.to_vec();
            run_biquad(buf, bq);
            mix_dry_wet(buf, &orig, dry, wet);
        }
        "ec.audio.parametriceq" => {
            for n in 1..=3 {
                if !params.b(&format!("band{n}Enable")) {
                    continue;
                }
                let g = params.f(&format!("band{n}BoostCut"));
                if g == 0.0 {
                    continue;
                }
                let f = params.f(&format!("band{n}Frequency")).clamp(1.0, sr * 0.49);
                let bw = (params.f(&format!("band{n}Bandwidth")) / 100.0 * 3.0).max(0.05);
                run_biquad(buf, Biquad::peaking(sr, f, bw, g));
            }
        }
        "ec.audio.delay" => delay(params, buf, sr),
        "ec.audio.compressor" => compressor(params, buf, sr),
        "ec.audio.distortion" => distortion(params, buf, sr, start),
        "ec.audio.gate" => gate(params, buf, sr),
        "ec.audio.flangechorus" => flange_chorus(params, buf, sr, start),
        "ec.audio.modulator" => modulator(params, buf, sr, start),
        "ec.audio.reverb" => reverb(params, buf, sr),
        "ec.audio.stereomixer" => stereo_mixer(params, buf),
        "ec.audio.tone" => tone(params, buf, sr, start),
        _ => {}
    }
}

fn mix_dry_wet(buf: &mut [f32], dry_src: &[f32], dry: f32, wet: f32) {
    for (o, d) in buf.iter_mut().zip(dry_src) {
        *o = *o * wet + *d * dry;
    }
}

// ---------------------------------------------------------------- biquads (RBJ cookbook)

/// Normalised second-order section coefficients (a0 = 1).
#[derive(Clone, Copy, Debug)]
pub struct Biquad {
    pub b: [f64; 3],
    pub a: [f64; 2],
}

impl Biquad {
    fn norm(b0: f64, b1: f64, b2: f64, a0: f64, a1: f64, a2: f64) -> Biquad {
        Biquad { b: [b0 / a0, b1 / a0, b2 / a0], a: [a1 / a0, a2 / a0] }
    }
    pub fn low_pass(sr: f64, f0: f64, q: f64) -> Biquad {
        let w = 2.0 * PI * f0 / sr;
        let (s, c) = w.sin_cos();
        let al = s / (2.0 * q);
        Self::norm((1.0 - c) / 2.0, 1.0 - c, (1.0 - c) / 2.0, 1.0 + al, -2.0 * c, 1.0 - al)
    }
    pub fn high_pass(sr: f64, f0: f64, q: f64) -> Biquad {
        let w = 2.0 * PI * f0 / sr;
        let (s, c) = w.sin_cos();
        let al = s / (2.0 * q);
        Self::norm((1.0 + c) / 2.0, -(1.0 + c), (1.0 + c) / 2.0, 1.0 + al, -2.0 * c, 1.0 - al)
    }
    /// Peaking EQ with bandwidth `bw` in octaves and gain in dB.
    pub fn peaking(sr: f64, f0: f64, bw: f64, db: f64) -> Biquad {
        let a = 10f64.powf(db / 40.0);
        let w = 2.0 * PI * f0 / sr;
        let (s, c) = w.sin_cos();
        let al = s * (2f64.ln() / 2.0 * bw * w / s).sinh();
        Self::norm(1.0 + al * a, -2.0 * c, 1.0 - al * a, 1.0 + al / a, -2.0 * c, 1.0 - al / a)
    }
    /// Shelf with slope S = 1.
    pub fn low_shelf(sr: f64, f0: f64, db: f64) -> Biquad {
        let a = 10f64.powf(db / 40.0);
        let w = 2.0 * PI * f0 / sr;
        let (s, c) = w.sin_cos();
        let al = s / 2.0 * std::f64::consts::SQRT_2;
        let sq = 2.0 * a.sqrt() * al;
        Self::norm(
            a * ((a + 1.0) - (a - 1.0) * c + sq),
            2.0 * a * ((a - 1.0) - (a + 1.0) * c),
            a * ((a + 1.0) - (a - 1.0) * c - sq),
            (a + 1.0) + (a - 1.0) * c + sq,
            -2.0 * ((a - 1.0) + (a + 1.0) * c),
            (a + 1.0) + (a - 1.0) * c - sq,
        )
    }
    pub fn high_shelf(sr: f64, f0: f64, db: f64) -> Biquad {
        let a = 10f64.powf(db / 40.0);
        let w = 2.0 * PI * f0 / sr;
        let (s, c) = w.sin_cos();
        let al = s / 2.0 * std::f64::consts::SQRT_2;
        let sq = 2.0 * a.sqrt() * al;
        Self::norm(
            a * ((a + 1.0) + (a - 1.0) * c + sq),
            -2.0 * a * ((a - 1.0) + (a + 1.0) * c),
            a * ((a + 1.0) + (a - 1.0) * c - sq),
            (a + 1.0) - (a - 1.0) * c + sq,
            2.0 * ((a - 1.0) - (a + 1.0) * c),
            (a + 1.0) - (a - 1.0) * c - sq,
        )
    }
}

/// Direct form I over both channels of an interleaved buffer.
fn run_biquad(buf: &mut [f32], q: Biquad) {
    for ch in 0..2 {
        let (mut x1, mut x2, mut y1, mut y2) = (0.0f64, 0.0, 0.0, 0.0);
        for i in (ch..buf.len()).step_by(2) {
            let x = buf[i] as f64;
            let y = q.b[0] * x + q.b[1] * x1 + q.b[2] * x2 - q.a[0] * y1 - q.a[1] * y2;
            x2 = x1;
            x1 = x;
            y2 = y1;
            y1 = y;
            buf[i] = y as f32;
        }
    }
}

// ---------------------------------------------------------------- delay lines

/// Linear-interpolated read `d` samples (fractional) behind position `i` of channel `ch`.
fn read_frac(src: &[f32], i: usize, ch: usize, d: f64) -> f32 {
    let pos = i as f64 - d;
    if pos < 0.0 {
        return 0.0;
    }
    let p0 = pos.floor() as usize;
    let f = (pos - p0 as f64) as f32;
    let a = src[p0 * 2 + ch];
    let b = if p0 < i { src[(p0 + 1) * 2 + ch] } else { a };
    a + (b - a) * f
}

fn delay(params: &Params, buf: &mut [f32], sr: f64) {
    let d = ((params.f("delayTime").max(0.0) / 1000.0 * sr).round() as usize).max(1);
    let amount = (params.f("delayAmount") / 100.0) as f32;
    let fb = (params.f("feedback") / 100.0).clamp(0.0, 0.99) as f32;
    let dry = (params.f("dryOut") / 100.0) as f32;
    let wet = (params.f("wetOut") / 100.0) as f32;
    let n = buf.len() / 2;
    // Echo line: e[i] = x[i-d] + fb * e[i-d]; out = dry*x + wet*amount*e.
    let mut e = vec![0.0f32; buf.len()];
    for i in d..n {
        for ch in 0..2 {
            e[i * 2 + ch] = buf[(i - d) * 2 + ch] + fb * e[(i - d) * 2 + ch];
        }
    }
    for (o, v) in buf.iter_mut().zip(&e) {
        *o = *o * dry + v * wet * amount;
    }
}

#[inline]
fn db_to_lin(db: f64) -> f64 {
    10f64.powf(db / 20.0)
}

#[inline]
fn lin_to_db(v: f64) -> f64 {
    20.0 * v.max(1e-12).log10()
}

/// One-pole smoothing coefficient for a time constant of `ms` milliseconds.
fn smoothing(ms: f64, sr: f64) -> f64 {
    if ms <= 0.0 { 0.0 } else { (-1.0 / (ms / 1000.0 * sr)).exp() }
}

/// Static compressor curve: gain change (dB, ≤ 0) for an input level `x` dB (quadratic soft
/// knee of width `knee` around the threshold).
pub fn compressor_gain_db(x: f64, threshold: f64, ratio: f64, knee: f64) -> f64 {
    // Ratios below 1:1 expand above the threshold.
    let r = ratio.max(0.1);
    let over = x - threshold;
    let y = if knee > 0.0 && 2.0 * over.abs() <= knee {
        x + (1.0 / r - 1.0) * (over + knee / 2.0).powi(2) / (2.0 * knee)
    } else if over > 0.0 {
        threshold + over / r
    } else {
        x
    };
    y - x
}

/// Auto Release: a program-dependent release from two envelopes. A fast one (the Attack time, a
/// short release) follows transients; a slow one (slow attack and release) only builds up under
/// sustained compression. The deeper of the two applies, so brief peaks recover quickly and long
/// loud passages slowly.
const AUTO_RELEASE_FAST_MS: f64 = 60.0;
const AUTO_RELEASE_SLOW_ATTACK_MS: f64 = 150.0;
const AUTO_RELEASE_SLOW_MS: f64 = 900.0;

fn compressor(params: &Params, buf: &mut [f32], sr: f64) {
    let th = params.f("threshold");
    let ratio = params.f("ratio").max(0.1);
    let knee = params.f("knee").max(0.0);
    let att = smoothing(params.f("attack"), sr);
    let auto = params.b("autoRelease");
    let rel = smoothing(if auto { AUTO_RELEASE_FAST_MS } else { params.f("release").max(0.001) }, sr);
    let (att_s, rel_s) = (smoothing(AUTO_RELEASE_SLOW_ATTACK_MS, sr), smoothing(AUTO_RELEASE_SLOW_MS, sr));
    let makeup = params.f("makeupGain");
    // Output Limit: a final ceiling (brick-wall clip) so the result never exceeds it.
    let limit = db_to_lin(params.f("outputLimit").min(0.0)) as f32;
    // Smoothed gain change in dB: attack while the reduction grows, release as it shrinks.
    let (mut g, mut gs) = (0.0f64, 0.0f64);
    for f in buf.as_chunks_mut::<2>().0.iter_mut() {
        let peak = f[0].abs().max(f[1].abs()) as f64;
        let target = compressor_gain_db(lin_to_db(peak), th, ratio, knee);
        let k = if target < g { att } else { rel };
        g = target + (g - target) * k;
        let mut eff = g;
        if auto {
            let k = if target < gs { att_s } else { rel_s };
            gs = target + (gs - target) * k;
            eff = g.min(gs);
        }
        let lin = db_to_lin(eff + makeup) as f32;
        for v in f.iter_mut() {
            *v = (*v * lin).clamp(-limit, limit);
        }
    }
}

fn gate(params: &Params, buf: &mut [f32], sr: f64) {
    let th = db_to_lin(params.f("threshold"));
    let att = smoothing(params.f("attack"), sr);
    let rel = smoothing(params.f("release").max(0.001), sr);
    let hold = (params.f("hold").max(0.0) / 1000.0 * sr) as usize;
    // Openness 0..1 (closed..open) is the gain: a closed gate mutes.
    let mut g = 0.0f64;
    let mut held = 0usize;
    for f in buf.as_chunks_mut::<2>().0.iter_mut() {
        let peak = f[0].abs().max(f[1].abs()) as f64;
        let open = if peak >= th {
            held = hold;
            true
        } else if held > 0 {
            held -= 1;
            true
        } else {
            false
        };
        let target = if open { 1.0 } else { 0.0 };
        let k = if target > g { att } else { rel };
        g = target + (g - target) * k;
        let lin = g as f32;
        f[0] *= lin;
        f[1] *= lin;
    }
}

/// The Distortion effect's waveshaper for one sample: `kind` indexes [`DISTORTION_TYPES`],
/// `drive` (0..1) sets how hard the curve bends. The result stays within ±1.
pub fn distort_sample(kind: u32, x: f32, drive: f32) -> f32 {
    let v = x * (1.0 + drive.clamp(0.0, 1.0) * 24.0);
    match kind {
        // Hard clip: linear up to a flat ceiling.
        1 => v,
        // Saturation 1: rational curve, gentler knee than tanh.
        2 => v / (1.0 + v.abs()),
        // Saturation 2: cubic soft saturator (odd harmonics), flat beyond ±1.5.
        3 => {
            let c = v.clamp(-1.5, 1.5);
            c - c * c * c * 4.0 / 27.0
        }
        // Tube: asymmetric — the positive half saturates harder (even harmonics).
        4 => {
            if v >= 0.0 {
                v.tanh()
            } else {
                (0.6 * v).tanh()
            }
        }
        // Fuzz: near-square exponential clipper.
        5 => v.signum() * (1.0 - (-3.0 * v.abs()).exp()),
        // Soft clip.
        _ => v.tanh(),
    }
    .clamp(-1.0, 1.0)
}

fn distortion(params: &Params, buf: &mut [f32], sr: f64, start: f64) {
    let kind = params.e("distortionType");
    let drive = (params.f("drive") / 100.0) as f32;
    // Gain: pre-distortion amplification (25 = unity); Volume: output level (11 = unity).
    let pre = (params.f("gain").max(0.0) / 25.0) as f32;
    let mix = (params.f("mix") / 100.0).clamp(0.0, 1.0) as f32;
    let vol = (params.f("volume").max(0.0) / 11.0) as f32;
    let bits = params.f("bitDepth").round().clamp(1.0, 24.0);
    let q = (bits < 24.0).then(|| 2f32.powf(bits as f32 - 1.0));
    let down = params.f("downsample").round().clamp(1.0, 64.0) as i64;
    // Sample-and-hold on absolute sample positions, so blocks agree with one long run.
    let first = (start * sr).round() as i64;
    let mut held = [0.0f32; 2];
    for (i, f) in buf.as_chunks_mut::<2>().0.iter_mut().enumerate() {
        let take = i == 0 || (first + i as i64).rem_euclid(down) == 0;
        for ch in 0..2 {
            let x = f[ch];
            let mut y = distort_sample(kind, x * pre, drive);
            if let Some(q) = q {
                y = (y * q).round() / q;
            }
            if take {
                held[ch] = y;
            }
            f[ch] = (x * (1.0 - mix) + held[ch] * mix) * vol;
        }
    }
}

fn lfo(kind: u32, phase: f64) -> f64 {
    let ph = phase.rem_euclid(1.0);
    match kind {
        1 => 1.0 - 4.0 * (ph - 0.5).abs(), // triangle, −1..1
        _ => (2.0 * PI * ph).sin(),
    }
}

fn flange_chorus(params: &Params, buf: &mut [f32], sr: f64, start: f64) {
    let sep = params.f("voiceSeparationTime").max(0.0) / 1000.0 * sr;
    let voices = params.f("voices").round().clamp(1.0, 8.0) as usize;
    let rate = params.f("modulationRate").max(0.0);
    let depth = (params.f("modulationDepth") / 100.0).clamp(0.0, 1.0);
    let vphase = params.f("voicePhaseChange") / 360.0;
    let inv = if params.b("invertPhase") { -1.0 } else { 1.0 };
    let stereo = params.b("stereoVoices");
    let dry = (params.f("dryOut") / 100.0) as f32;
    let wet = (params.f("wetOut") / 100.0) as f32;
    let src = buf.to_vec();
    let n = buf.len() / 2;
    for i in 0..n {
        let t = start + i as f64 / sr;
        let mut acc = [0.0f32; 2];
        let mut cnt = [0usize; 2];
        for v in 0..voices {
            let base = sep * (v + 1) as f64;
            let d = (base * (1.0 + depth * lfo(0, rate * t + vphase * v as f64))).max(0.0);
            for ch in 0..2 {
                if stereo && voices > 1 && v % 2 != ch {
                    continue;
                }
                acc[ch] += read_frac(&src, i, ch, d);
                cnt[ch] += 1;
            }
        }
        for ch in 0..2 {
            let w = if cnt[ch] > 0 { acc[ch] / cnt[ch] as f32 } else { 0.0 };
            buf[i * 2 + ch] = src[i * 2 + ch] * dry + inv * w * wet;
        }
    }
}

fn modulator(params: &Params, buf: &mut [f32], sr: f64, start: f64) {
    let kind = params.e("modulationType");
    let rate = params.f("modulationRate").max(0.0);
    // Depth: vibrato as a modulated delay of up to 5 ms; amplitude: tremolo.
    let depth = (params.f("modulationDepth") / 100.0).clamp(0.0, 1.0) * 0.005 * sr;
    let am = (params.f("amplitudeModulation") / 100.0).clamp(0.0, 1.0);
    let src = buf.to_vec();
    let n = buf.len() / 2;
    for i in 0..n {
        let t = start + i as f64 / sr;
        let l = lfo(kind, rate * t);
        let d = depth * (1.0 + l) * 0.5;
        let g = (1.0 - am * (1.0 - l) * 0.5) as f32;
        for ch in 0..2 {
            let x = if depth > 0.0 { read_frac(&src, i, ch, d) } else { src[i * 2 + ch] };
            buf[i * 2 + ch] = x * g;
        }
    }
}

/// (comb feedback, damping, all-pass gain, pre-delay seconds)
fn reverb_params(params: &Params) -> (f64, f64, f64, f64) {
    let decay = (params.f("decay") / 100.0).clamp(0.0, 1.0);
    let fb = 0.7 + decay * 0.28;
    let damp = (1.0 - params.f("brightness") / 100.0).clamp(0.0, 1.0) * 0.6;
    let ap = 0.3 + 0.4 * (params.f("diffusion") / 100.0).clamp(0.0, 1.0);
    let pre = params.f("reverbTime").max(0.0) / 1000.0;
    (fb, damp, ap, pre)
}

fn reverb(params: &Params, buf: &mut [f32], sr: f64) {
    const COMBS: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
    const ALLPASS: [usize; 4] = [556, 441, 341, 225];
    const SPREAD: usize = 23;
    let (fb, damp, apg, pre) = reverb_params(params);
    let dry = (params.f("dryOut") / 100.0) as f32;
    let wet = (params.f("wetOut") / 100.0) as f32;
    let k = sr / 44100.0;
    let pre_n = (pre * sr).round() as usize;
    let n = buf.len() / 2;
    let src = buf.to_vec();
    for ch in 0..2 {
        let spread = if ch == 1 { SPREAD } else { 0 };
        let mut combs: Vec<(Vec<f64>, usize, f64)> = COMBS.iter().map(|&l| (vec![0.0; (((l + spread) as f64 * k) as usize).max(1)], 0, 0.0)).collect();
        let mut aps: Vec<(Vec<f64>, usize)> = ALLPASS.iter().map(|&l| (vec![0.0; (((l + spread) as f64 * k) as usize).max(1)], 0)).collect();
        for i in 0..n {
            let x = if i >= pre_n { (src[(i - pre_n) * 2] as f64 + src[(i - pre_n) * 2 + 1] as f64) * 0.5 * 0.03 } else { 0.0 };
            let mut out = 0.0;
            for (line, pos, filt) in combs.iter_mut() {
                let y = line[*pos];
                *filt = y * (1.0 - damp) + *filt * damp;
                line[*pos] = x + *filt * fb;
                *pos = (*pos + 1) % line.len();
                out += y;
            }
            for (line, pos) in aps.iter_mut() {
                let b = line[*pos];
                let y = -out + b;
                line[*pos] = out + b * apg;
                *pos = (*pos + 1) % line.len();
                out = y;
            }
            buf[i * 2 + ch] = src[i * 2 + ch] * dry + out as f32 * wet * 3.0;
        }
    }
}

fn stereo_mixer(params: &Params, buf: &mut [f32]) {
    let ll = (params.f("leftLevel") / 100.0) as f32;
    let rl = (params.f("rightLevel") / 100.0) as f32;
    // Linear pan law: −100 = all left, 0 = both at full, +100 = all right.
    let pan = |p: f64| -> (f32, f32) {
        let p = (p / 100.0).clamp(-1.0, 1.0);
        ((1.0 - p).min(1.0) as f32, (1.0 + p).min(1.0) as f32)
    };
    let (l_to_l, l_to_r) = pan(params.f("leftPan"));
    let (r_to_l, r_to_r) = pan(params.f("rightPan"));
    let inv = if params.b("invertPhase") { -1.0 } else { 1.0 };
    for f in buf.as_chunks_mut::<2>().0.iter_mut() {
        let (l, r) = (f[0] * ll, f[1] * rl);
        f[0] = (l * l_to_l + r * r_to_l) * inv;
        f[1] = (l * l_to_r + r * r_to_r) * inv;
    }
}

fn tone(params: &Params, buf: &mut [f32], sr: f64, start: f64) {
    let kind = params.e("waveformOptions");
    let level = (params.f("level") / 100.0) as f32;
    let freqs: Vec<f64> = (1..=5).map(|i| params.f(&format!("frequency{i}"))).filter(|f| *f > 0.0).collect();
    let n = buf.len() / 2;
    for i in 0..n {
        let t = start + i as f64 / sr;
        let mut v = 0.0f64;
        for f in &freqs {
            let ph = (f * t).rem_euclid(1.0);
            v += match kind {
                1 => lfo(1, ph - 0.25),
                2 => 2.0 * ph - 1.0,
                3 => {
                    if ph < 0.5 {
                        1.0
                    } else {
                        -1.0
                    }
                }
                // White noise: a hash of the absolute sample position (every frequency alike).
                4 => {
                    let h = ((t * sr).round() as i64 as u64 ^ f.to_bits()).wrapping_mul(0x9e37_79b9_7f4a_7c15);
                    let h = (h ^ (h >> 29)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
                    ((h ^ (h >> 32)) >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
                }
                _ => (2.0 * PI * ph).sin(),
            };
        }
        let v = v as f32 * level;
        buf[i * 2] += v;
        buf[i * 2 + 1] += v;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults(id: &str) -> Params {
        let s = crate::find(id).unwrap();
        Params { values: s.params.iter().map(|p| (p.id.to_string(), p.default.clone())).collect() }
    }

    fn sine(freq: f64, sr: f64, n: usize) -> Vec<f32> {
        (0..n)
            .flat_map(|i| {
                let v = (2.0 * PI * freq * i as f64 / sr).sin() as f32;
                [v, v]
            })
            .collect()
    }

    fn rms(b: &[f32], from: usize) -> f32 {
        let s: f32 = b[from * 2..].iter().map(|v| v * v).sum();
        (s / (b.len() - from * 2) as f32).sqrt()
    }

    #[test]
    fn registered_as_audio() {
        for id in [
            "ec.audio.backwards",
            "ec.audio.basstreble",
            "ec.audio.compressor",
            "ec.audio.delay",
            "ec.audio.distortion",
            "ec.audio.gate",
            "ec.audio.flangechorus",
            "ec.audio.highlowpass",
            "ec.audio.modulator",
            "ec.audio.parametriceq",
            "ec.audio.reverb",
            "ec.audio.stereomixer",
            "ec.audio.tone",
        ] {
            let s = crate::find(id).unwrap_or_else(|| panic!("{id}"));
            assert_eq!(s.category, "Audio");
            assert!(is_audio_effect(id));
        }
    }

    #[test]
    fn high_low_pass_attenuates() {
        let sr = 48000.0;
        let mut p = defaults("ec.audio.highlowpass");
        p.values.insert("filterOptions".into(), Value::Enum(1));
        p.values.insert("cutoffFrequency".into(), num(500.0));
        let mut hi = sine(8000.0, sr, 4800);
        process("ec.audio.highlowpass", &p, &mut hi, 48000, 0.0);
        let mut lo = sine(100.0, sr, 4800);
        process("ec.audio.highlowpass", &p, &mut lo, 48000, 0.0);
        assert!(rms(&hi, 1000) < 0.01, "{}", rms(&hi, 1000));
        assert!(rms(&lo, 1000) > 0.6, "{}", rms(&lo, 1000));
        // High pass does the opposite.
        p.values.insert("filterOptions".into(), Value::Enum(0));
        let mut lo = sine(50.0, sr, 9600);
        process("ec.audio.highlowpass", &p, &mut lo, 48000, 0.0);
        assert!(rms(&lo, 2000) < 0.02);
    }

    #[test]
    fn delay_offsets_impulse() {
        let mut p = defaults("ec.audio.delay");
        p.values.insert("delayTime".into(), num(10.0));
        p.values.insert("feedback".into(), num(0.0));
        let mut b = vec![0.0f32; 2000];
        b[0] = 1.0;
        b[1] = 1.0;
        process("ec.audio.delay", &p, &mut b, 10000, 0.0);
        // dry 75 %, then the echo at 100 samples: wet 75 % × amount 50 %.
        assert!((b[0] - 0.75).abs() < 1e-6);
        assert!((b[200] - 0.375).abs() < 1e-6 && (b[201] - 0.375).abs() < 1e-6);
        assert!(b[2..200].iter().all(|v| *v == 0.0));
        assert!(preroll("ec.audio.delay", &p) >= 0.01);
    }

    #[test]
    fn stereo_mixer_pans() {
        let mut p = defaults("ec.audio.stereomixer");
        let mut b = vec![1.0, 0.0, 1.0, 0.0];
        process("ec.audio.stereomixer", &p, &mut b, 48000, 0.0);
        assert_eq!(b, vec![1.0, 0.0, 1.0, 0.0], "defaults are identity");
        p.values.insert("leftPan".into(), num(100.0));
        let mut b = vec![1.0, 0.0];
        process("ec.audio.stereomixer", &p, &mut b, 48000, 0.0);
        assert_eq!(b, vec![0.0, 1.0]);
        p.values.insert("leftPan".into(), num(0.0));
        p.values.insert("leftLevel".into(), num(50.0));
        let mut b = vec![1.0, 0.0];
        process("ec.audio.stereomixer", &p, &mut b, 48000, 0.0);
        assert_eq!(b, vec![0.5, 0.5]);
    }

    #[test]
    fn tone_generates_frequency() {
        let mut p = defaults("ec.audio.tone");
        p.values.insert("frequency1".into(), num(1000.0));
        for i in 2..=5 {
            p.values.insert(format!("frequency{i}"), num(0.0));
        }
        let sr = 48000;
        let mut b = vec![0.0f32; 48000 * 2];
        process("ec.audio.tone", &p, &mut b, sr, 0.0);
        let left: Vec<f32> = b.iter().step_by(2).copied().collect();
        let crossings = left.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
        assert!((crossings as i64 - 1000).abs() <= 1, "{crossings}");
        let peak = left.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!((peak - 0.2).abs() < 1e-3, "{peak}");
    }

    #[test]
    fn bass_treble_eq_reverb_modulator_flange() {
        let sr = 48000.0;
        // Bass boost raises 60 Hz, treble cut lowers 10 kHz.
        let mut p = defaults("ec.audio.basstreble");
        p.values.insert("bass".into(), num(100.0));
        p.values.insert("treble".into(), num(-100.0));
        let mut lo = sine(60.0, sr, 9600);
        process("ec.audio.basstreble", &p, &mut lo, 48000, 0.0);
        assert!(rms(&lo, 4000) > 0.707 * 3.0, "{}", rms(&lo, 4000));
        let mut hi = sine(10000.0, sr, 9600);
        process("ec.audio.basstreble", &p, &mut hi, 48000, 0.0);
        assert!(rms(&hi, 4000) < 0.707 * 0.4, "{}", rms(&hi, 4000));
        // Parametric EQ boost at its centre.
        let mut p = defaults("ec.audio.parametriceq");
        p.values.insert("band1BoostCut".into(), num(12.0));
        p.values.insert("band1Frequency".into(), num(1000.0));
        let mut s = sine(1000.0, sr, 9600);
        process("ec.audio.parametriceq", &p, &mut s, 48000, 0.0);
        assert!((rms(&s, 4000) * std::f32::consts::SQRT_2 - 3.98).abs() < 0.2, "{}", rms(&s, 4000) * std::f32::consts::SQRT_2);
        // Reverb leaves a tail after an impulse.
        let p = defaults("ec.audio.reverb");
        let mut b = vec![0.0f32; 48000];
        b[0] = 1.0;
        b[1] = 1.0;
        process("ec.audio.reverb", &p, &mut b, 48000, 0.0);
        assert!(b[2 * 12000..].iter().any(|v| v.abs() > 1e-5));
        assert!(b.iter().all(|v| v.is_finite() && v.abs() < 2.0));
        // Modulator with amplitude modulation dips the level; flange keeps it bounded.
        let mut p = defaults("ec.audio.modulator");
        p.values.insert("amplitudeModulation".into(), num(100.0));
        let mut b = vec![1.0f32; 9600];
        process("ec.audio.modulator", &p, &mut b, 48000, 0.0);
        assert!(b.iter().any(|v| *v < 0.05) && b.iter().any(|v| *v > 0.95));
        let p = defaults("ec.audio.flangechorus");
        let mut b = sine(440.0, sr, 4800);
        process("ec.audio.flangechorus", &p, &mut b, 48000, 0.0);
        assert!(b.iter().all(|v| v.abs() <= 1.0 + 1e-4));
    }

    #[test]
    fn backwards_swaps_channels() {
        let mut p = defaults("ec.audio.backwards");
        p.values.insert("swapChannels".into(), Value::Bool(true));
        let mut b = vec![1.0, 2.0];
        process("ec.audio.backwards", &p, &mut b, 48000, 0.0);
        assert_eq!(b, vec![2.0, 1.0]);
        assert!(reverses("ec.audio.backwards"));
    }

    #[test]
    fn compressor_reduces_gain_above_threshold() {
        // Static curve: 10 dB over a −20 dB threshold at 4:1 comes out 7.5 dB lower.
        assert!((compressor_gain_db(-10.0, -20.0, 4.0, 0.0) + 7.5).abs() < 1e-9);
        assert_eq!(compressor_gain_db(-30.0, -20.0, 4.0, 0.0), 0.0);
        // The soft knee meets the hard curve at its edges.
        let k = 6.0;
        assert!(compressor_gain_db(-23.0, -20.0, 4.0, k).abs() < 1e-9);
        assert!((compressor_gain_db(-17.0, -20.0, 4.0, k) - compressor_gain_db(-17.0, -20.0, 4.0, 0.0)).abs() < 1e-9);
        let sr = 48000.0;
        let mut p = defaults("ec.audio.compressor");
        for (k, v) in [("attack", 1.0), ("threshold", -20.0), ("ratio", 4.0), ("knee", 0.0), ("release", 100.0)] {
            p.values.insert(k.into(), num(v));
        }
        p.values.insert("autoRelease".into(), Value::Bool(false));
        let mut loud = sine(1000.0, sr, 9600);
        process("ec.audio.compressor", &p, &mut loud, 48000, 0.0);
        // A full-scale sine (peak 0 dB) is pulled down by ~15 dB once the envelope settles.
        let peak = loud[18000..].iter().fold(0.0f32, |m, v| m.max(v.abs()));
        let want = db_to_lin(compressor_gain_db(0.0, -20.0, 4.0, 0.0)) as f32;
        assert!((peak - want).abs() < 0.05, "{peak} vs {want}");
        // Quiet input passes untouched.
        let mut quiet: Vec<f32> = sine(1000.0, sr, 4800).iter().map(|v| v * 0.01).collect();
        let orig = quiet.clone();
        process("ec.audio.compressor", &p, &mut quiet, 48000, 0.0);
        assert!(quiet.iter().zip(&orig).all(|(a, b)| (a - b).abs() < 1e-6));
    }

    #[test]
    fn gate_mutes_below_threshold() {
        let sr = 48000.0;
        let mut p = defaults("ec.audio.gate");
        p.values.insert("threshold".into(), num(-40.0));
        // A −60 dB signal under the −40 dB threshold is muted.
        let mut quiet: Vec<f32> = sine(500.0, sr, 9600).iter().map(|v| v * 0.001).collect();
        process("ec.audio.gate", &p, &mut quiet, 48000, 0.0);
        assert!(rms(&quiet, 4800) < 1e-6, "{}", rms(&quiet, 4800));
        // A loud signal opens the gate.
        let mut loud: Vec<f32> = sine(500.0, sr, 9600).iter().map(|v| v * 0.5).collect();
        let r0 = rms(&loud, 4800);
        process("ec.audio.gate", &p, &mut loud, 48000, 0.0);
        assert!((rms(&loud, 4800) - r0).abs() < 0.01);
        // Deterministic.
        let mut a = sine(300.0, sr, 2000);
        let mut b = a.clone();
        process("ec.audio.gate", &p, &mut a, 48000, 0.0);
        process("ec.audio.gate", &p, &mut b, 48000, 0.0);
        assert_eq!(a, b);
    }

    #[test]
    fn distortion_shapes_and_bounds() {
        assert!((distort_sample(0, 0.5, 0.0) - 0.5f32.tanh()).abs() < 1e-6);
        assert_eq!(distort_sample(1, 0.8, 1.0), 1.0);
        for k in 0..DISTORTION_TYPES.len() as u32 {
            for x in [-2.0f32, -0.3, 0.0, 0.2, 1.7] {
                let y = distort_sample(k, x, 0.5);
                assert!(y.abs() <= 1.0 && y.signum() * x.signum() >= 0.0, "{k} {x} {y}");
            }
        }
        // Tube is asymmetric.
        assert!(distort_sample(4, 0.1, 0.5) > -distort_sample(4, -0.1, 0.5));
        let p = defaults("ec.audio.distortion");
        let mut b = sine(200.0, 48000.0, 4800);
        process("ec.audio.distortion", &p, &mut b, 48000, 0.0);
        assert!(b.iter().all(|v| v.abs() <= 1.0));
        assert!(rms(&b, 100) > 0.1);
        assert!(preroll("ec.audio.compressor", &defaults("ec.audio.compressor")) > 0.4);
    }

    #[test]
    fn distortion_mix_resolution_and_downsample() {
        let sr = 48000.0;
        let src: Vec<f32> = sine(300.0, sr, 480).iter().map(|v| v * 0.5).collect();
        // Mix 0 with unity volume passes the input through.
        let mut p = defaults("ec.audio.distortion");
        p.values.insert("mix".into(), num(0.0));
        let mut b = src.clone();
        process("ec.audio.distortion", &p, &mut b, 48000, 0.0);
        assert!(b.iter().zip(&src).all(|(a, s)| (a - s).abs() < 1e-6));
        // 2-bit resolution leaves at most 5 distinct levels (−1, −½, 0, ½, 1).
        let mut p = defaults("ec.audio.distortion");
        p.values.insert("bitDepth".into(), num(2.0));
        let mut b = src.clone();
        process("ec.audio.distortion", &p, &mut b, 48000, 0.0);
        let mut levels: Vec<i32> = b.iter().map(|v| (v * 1000.0).round() as i32).collect();
        levels.sort();
        levels.dedup();
        assert!(levels.len() <= 5, "{levels:?}");
        // Downsample ×4 holds each value for 4 frames.
        let mut p = defaults("ec.audio.distortion");
        p.values.insert("downsample".into(), num(4.0));
        let mut b = src.clone();
        process("ec.audio.distortion", &p, &mut b, 48000, 0.0);
        assert_eq!(b[0], b[6]);
        assert_ne!(b[6], b[8]);
    }

    #[test]
    fn compressor_output_limit_and_auto_release() {
        let sr = 48000.0;
        let mut p = defaults("ec.audio.compressor");
        p.values.insert("outputLimit".into(), num(-12.0));
        p.values.insert("makeupGain".into(), num(20.0));
        let mut b = sine(1000.0, sr, 4800);
        process("ec.audio.compressor", &p, &mut b, 48000, 0.0);
        let ceiling = db_to_lin(-12.0) as f32;
        assert!(b.iter().all(|v| v.abs() <= ceiling + 1e-6));
        // After a short burst, auto release recovers faster than a long manual release.
        let burst = |auto: bool| {
            let mut p = defaults("ec.audio.compressor");
            p.values.insert("autoRelease".into(), Value::Bool(auto));
            p.values.insert("release".into(), num(4000.0));
            let mut b: Vec<f32> = sine(1000.0, sr, 9600).iter().enumerate().map(|(i, v)| if i < 960 { *v } else { v * 0.2 }).collect();
            process("ec.audio.compressor", &p, &mut b, 48000, 0.0);
            rms(&b, 4800)
        };
        assert!(burst(true) > burst(false) * 1.2, "{} {}", burst(true), burst(false));
    }
}
