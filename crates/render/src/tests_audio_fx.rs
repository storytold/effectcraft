//! Audio effects through the mixdown (the path preview playback and export share).

use std::sync::Arc;

use effectcraft_keyframe::Value;
use effectcraft_project::build::{self, Ids};
use effectcraft_project::{Comp, Footage, FootageKind, ItemId, ItemKind, LayerSource, Project, Solid};
use effectcraft_time::{FrameRate, Tick};

use crate::audio::{comp_has_audio, footage_peaks, mix_comp, peak_summary};
use crate::{FootageSource, Image};

const SR: u32 = 8000;

/// Synthetic footage audio as a function of source time (seconds) → (L, R).
struct Signal(fn(f64) -> (f32, f32));
impl FootageSource for Signal {
    fn frame(&self, _: ItemId, _: &Footage, _: Tick) -> Option<Arc<Image>> {
        None
    }
    fn audio(&self, _: ItemId, _: &Footage, start: Tick, frames: usize, rate: u32) -> Option<Vec<f32>> {
        let t0 = start.seconds();
        Some(
            (0..frames)
                .flat_map(|i| {
                    let t = t0 + i as f64 / rate as f64;
                    let (l, r) = if (0.0..2.0).contains(&t) { (self.0)(t) } else { (0.0, 0.0) };
                    [l, r]
                })
                .collect(),
        )
    }
}

fn audio_footage() -> Footage {
    Footage {
        path: "x.wav".into(),
        kind: FootageKind::Audio,
        width: 0,
        height: 0,
        pixel_aspect: 1.0,
        frame_rate: FrameRate::new(25, 1),
        native_rate: None,
        duration: Tick::from_seconds_f64(2.0),
        has_video: false,
        has_audio: true,
        alpha: Default::default(),
        premul_color: [0.0; 3],
        loop_count: 1,
        codec: String::new(),
        missing: false,
        sequence: vec![],
        color_profile: None,
        ..Default::default()
    }
}

/// A 2 s comp with one audio layer (in 0, out 1 s) carrying effect `id` with `vals`.
fn comp_with(id: Option<&str>, vals: &[(&str, Value)], solid: bool) -> (Project, ItemId) {
    let mut p = Project::default();
    let comp = Comp::new(64, 64, FrameRate::new(25, 1), Tick::from_seconds_f64(2.0));
    let src = if solid {
        let sid = p.add_item("S", effectcraft_color::Label::Red, None, ItemKind::Solid(Solid { color: [1.0; 3], width: 64, height: 64, pixel_aspect: 1.0 }));
        LayerSource::Solid { item: sid }
    } else {
        LayerSource::Footage { item: p.add_item("x.wav", effectcraft_color::Label::SeaFoam, None, ItemKind::Footage(audio_footage())) }
    };
    let mut l = build::layer(&mut p, &comp, "a", src, (64, 64), None);
    l.out_point = Tick::from_seconds_f64(1.0);
    if let Some(id) = id {
        let spec = effectcraft_effects::find(id).unwrap();
        let mut next = p.next_id;
        let mut g = effectcraft_effects::instantiate(spec, &mut Ids(&mut next), spec.name, [64.0, 64.0]);
        p.next_id = next;
        for (k, v) in vals {
            g.prop_mut(k).unwrap_or_else(|| panic!("{k}")).value = v.clone();
        }
        l.props.sub_mut("effects").unwrap().children.push(g.into());
    }
    let mut comp = comp;
    comp.layers = vec![l];
    let cid = p.add_item("C", effectcraft_color::Label::Sandstone, None, ItemKind::Comp(comp.into()));
    (p, cid)
}

fn mix(p: &Project, src: &dyn FootageSource, cid: ItemId, from: f64, frames: usize) -> Vec<f32> {
    mix_comp(p, src, None, cid, Tick::from_seconds_f64(from), frames, SR).unwrap()
}

fn ramp(t: f64) -> (f32, f32) {
    (t as f32, -(t as f32))
}

/// Keys map layer seconds to source seconds; the synthetic source has distinct L/R samples.
fn remap(p: &mut Project, cid: ItemId, points: &[(f64, f64)]) {
    let mut next = p.next_id;
    let mut prop = Ids(&mut next).prop("timeRemap", "Time Remap", Value::Scalar(0.0));
    prop.keys = points.iter().map(|&(t, v)| effectcraft_keyframe::Keyframe::new(Tick::from_seconds_f64(t), Value::Scalar(v))).collect();
    p.next_id = next;
    p.comp_mut(cid).unwrap().layers[0].props.children.push(prop.into());
}

#[test]
fn audio_time_remap_follows_offset_and_speed_changes_across_blocks() {
    let (mut p, cid) = comp_with(None, &[], false);
    remap(&mut p, cid, &[(0.0, 0.5), (0.25, 0.75), (0.75, 1.0), (1.0, 1.25)]);
    let whole = mix(&p, &Signal(ramp), cid, 0.0, SR as usize);
    for i in 0..SR as usize {
        let t = i as f64 / SR as f64;
        let expected = if t < 0.25 {
            0.5 + t
        } else if t < 0.75 {
            0.75 + (t - 0.25) * 0.5
        } else {
            1.0 + t - 0.75
        };
        assert!((whole[i * 2] as f64 - expected).abs() < 1e-5, "sample {i}: {} != {expected}", whole[i * 2]);
        assert!((whole[i * 2 + 1] as f64 + expected).abs() < 1e-5);
    }
    let mut blocked = Vec::new();
    for first in (0..SR as usize).step_by(137) {
        blocked.extend(mix(&p, &Signal(ramp), cid, first as f64 / SR as f64, 137.min(SR as usize - first)));
    }
    assert_eq!(whole, blocked, "preview and export block sizes must not change the audio");
}

#[test]
fn audio_time_remap_handles_reverse_source_motion_and_layer_bounds() {
    let (mut p, cid) = comp_with(None, &[], false);
    remap(&mut p, cid, &[(0.0, 1.5), (1.0, 0.5)]);
    let samples = mix(&p, &Signal(ramp), cid, 0.5, SR as usize);
    for i in 0..SR as usize {
        let expected = if i < SR as usize / 2 { 1.0 - i as f64 / SR as f64 } else { 0.0 };
        assert!((samples[i * 2] as f64 - expected).abs() < 1e-5);
    }
}

#[test]
fn audio_time_remap_evaluates_keys_in_stretched_layer_time() {
    let (mut p, cid) = comp_with(None, &[], false);
    remap(&mut p, cid, &[(0.0, 0.5), (1.0, 1.5)]);
    let l = &mut p.comp_mut(cid).unwrap().layers[0];
    l.start_time = Tick::from_seconds_f64(0.25);
    l.in_point = l.start_time;
    l.out_point = Tick::from_seconds_f64(2.0);
    l.stretch = 200.0;
    let samples = mix(&p, &Signal(ramp), cid, 0.75, 200);
    for i in 0..200 {
        let expected = 0.75 + i as f64 / SR as f64 * 0.5;
        assert!((samples[i * 2] as f64 - expected).abs() < 1e-5);
    }
}

#[test]
fn audio_time_remap_of_precomp_uses_nested_source_audio() {
    let (mut p, inner) = comp_with(None, &[], false);
    let mut outer = Comp::new(64, 64, FrameRate::new(25, 1), Tick::from_seconds_f64(1.0));
    outer.layers.push(build::layer(&mut p, &outer, "nested", LayerSource::Comp { item: inner }, (64, 64), None));
    let cid = p.add_item("Outer", effectcraft_color::Label::Sandstone, None, ItemKind::Comp(outer.into()));
    remap(&mut p, cid, &[(0.0, 0.25), (1.0, 0.75)]);
    let samples = mix(&p, &Signal(ramp), cid, 0.0, SR as usize);
    for i in 0..SR as usize {
        let expected = 0.25 + i as f64 / SR as f64 * 0.5;
        assert!((samples[i * 2] as f64 - expected).abs() < 1e-5);
    }
}

#[test]
fn audio_time_remap_hold_repeats_the_selected_sample() {
    let (mut p, cid) = comp_with(None, &[], false);
    remap(&mut p, cid, &[(0.0, 0.75), (0.5, 1.25)]);
    p.comp_mut(cid).unwrap().layers[0].props.prop_mut("timeRemap").unwrap().keys[0].out_interp = effectcraft_keyframe::Interp::Hold;
    // A held picture selects a fixed audio sample (DC), not an advancing excerpt.
    let samples = mix(&p, &Signal(ramp), cid, 0.0, SR as usize);
    for (i, pair) in samples.as_chunks::<2>().0.iter().enumerate() {
        let expected = if i < SR as usize / 2 { 0.75 } else { 1.25 };
        assert_eq!(*pair, [expected, -expected], "sample {i}");
    }
}

#[test]
fn audio_time_remap_large_jump_keeps_source_requests_bounded() {
    use std::sync::Mutex;
    struct Requests(Mutex<Vec<usize>>);
    impl FootageSource for Requests {
        fn frame(&self, _: ItemId, _: &Footage, _: Tick) -> Option<Arc<Image>> {
            None
        }
        fn audio(&self, _: ItemId, _: &Footage, _: Tick, frames: usize, _: u32) -> Option<Vec<f32>> {
            self.0.lock().unwrap().push(frames);
            // Assert before allocating: a distant key must not request the intervening audio.
            assert!(frames <= 4097, "unbounded source request: {frames}");
            Some(vec![0.25; frames * 2])
        }
    }
    let (mut p, cid) = comp_with(None, &[], false);
    remap(&mut p, cid, &[(0.0, 0.0), (0.5, 1_000_000.0)]);
    p.comp_mut(cid).unwrap().layers[0].props.prop_mut("timeRemap").unwrap().keys[0].out_interp = effectcraft_keyframe::Interp::Hold;
    let src = Requests(Mutex::new(Vec::new()));
    assert!(mix(&p, &src, cid, 0.0, SR as usize).iter().all(|v| *v == 0.25));
    assert_eq!(src.0.lock().unwrap().len(), 2, "one fetch per held source page");
}

#[test]
fn audio_time_remap_source_bounds_and_pages_at_44100_hz() {
    let (mut p, cid) = comp_with(None, &[], false);
    remap(&mut p, cid, &[(0.0, -0.25), (1.0, 2.75)]);
    let rate = 44_100;
    let src = Signal(|_| (0.5, -0.5));
    let whole = mix_comp(&p, &src, None, cid, Tick::ZERO, rate as usize, rate).unwrap();
    let mut blocked = Vec::new();
    for first in (0..rate as usize).step_by(137) {
        blocked.extend(mix_comp(&p, &src, None, cid, Tick::from_units(first as i64, rate as i64), 137.min(rate as usize - first), rate).unwrap());
    }
    assert_eq!(whole, blocked, "source page and caller boundaries must not affect samples");
    // Stay away from the one-sample interpolation transition at each source edge.
    for (i, pair) in whole.as_chunks::<2>().0.iter().enumerate() {
        let source = -0.25 + 3.0 * i as f64 / rate as f64;
        if source < -0.001 || source > 2.001 {
            assert_eq!(*pair, [0.0, 0.0], "sample {i}");
        } else if (0.001..1.999).contains(&source) {
            assert_eq!(*pair, [0.5, -0.5], "sample {i}");
        }
    }
}

#[test]
fn audio_time_remap_expression_and_error_fallback() {
    struct Expr(bool);
    impl crate::ExprHost for Expr {
        fn eval(&self, ctx: &crate::EvalCtx, _: &effectcraft_project::Layer, _: &effectcraft_project::Property, _: &Value) -> Result<Value, String> {
            if self.0 { Ok(Value::Scalar(0.5 + ctx.time.seconds() * 0.5)) } else { Err("synthetic expression error".into()) }
        }
    }
    let (mut p, cid) = comp_with(None, &[], false);
    remap(&mut p, cid, &[(0.0, 1.25)]);
    p.comp_mut(cid).unwrap().layers[0].props.prop_mut("timeRemap").unwrap().expr =
        Some(effectcraft_project::Expression { text: "time / 2 + 0.5".into(), enabled: true });
    for enabled in [true, false] {
        let samples = mix_comp(&p, &Signal(ramp), Some(&Expr(enabled)), cid, Tick::ZERO, SR as usize, SR).unwrap();
        for (i, pair) in samples.as_chunks::<2>().0.iter().enumerate() {
            let expected = if enabled { 0.5 + i as f64 / SR as f64 * 0.5 } else { 1.25 };
            assert!((pair[0] as f64 - expected).abs() < 1e-5);
            assert!((pair[1] as f64 + expected).abs() < 1e-5);
        }
    }
}

fn rms(m: &[f32]) -> f32 {
    (m.iter().map(|v| v * v).sum::<f32>() / m.len() as f32).sqrt()
}

#[test]
fn backwards_reverses_the_layer_span() {
    let (p, cid) = comp_with(Some("ec.audio.backwards"), &[], false);
    let m = mix(&p, &Signal(ramp), cid, 0.0, SR as usize);
    // Comp time t plays source time ≈ 1 − t.
    for i in [0usize, 2000, 4000, 7999] {
        let t = i as f64 / SR as f64;
        assert!((m[i * 2] as f64 - (1.0 - t)).abs() < 2.0 / SR as f64, "i={i}: {}", m[i * 2]);
    }
    let (p, cid) = comp_with(None, &[], false);
    let m = mix(&p, &Signal(ramp), cid, 0.0, SR as usize);
    assert!((m[4000 * 2] - 0.5).abs() < 1e-3);
}

#[test]
fn high_low_pass_attenuates() {
    fn hi(t: f64) -> (f32, f32) {
        let v = (2.0 * std::f64::consts::PI * 3000.0 * t).sin() as f32;
        (v, v)
    }
    let vals = [("filterOptions", Value::Enum(1)), ("cutoffFrequency", Value::Scalar(200.0))];
    let (p, cid) = comp_with(Some("ec.audio.highlowpass"), &vals, false);
    let r = rms(&mix(&p, &Signal(hi), cid, 0.5, 800));
    assert!(r < 0.02, "{r}");
    let (p, cid) = comp_with(None, &[], false);
    let r = rms(&mix(&p, &Signal(hi), cid, 0.5, 800));
    assert!(r > 0.6, "{r}");
}

#[test]
fn delay_offsets_across_blocks() {
    fn click(t: f64) -> (f32, f32) {
        if (t * SR as f64).round() as i64 == 2000 { (1.0, 1.0) } else { (0.0, 0.0) }
    }
    let vals = [("delayTime", Value::Scalar(100.0)), ("feedback", Value::Scalar(0.0))];
    let (p, cid) = comp_with(Some("ec.audio.delay"), &vals, false);
    // The block starts after the click (0.25 s): the echo at 0.35 s still comes from pre-roll.
    let m = mix(&p, &Signal(click), cid, 0.3, 800);
    let i = 2800 - 2400;
    assert!((m[i * 2] - 0.375).abs() < 1e-5, "{}", m[i * 2]);
    assert_eq!(m.iter().filter(|v| v.abs() > 1e-6).count(), 2);
}

#[test]
fn stereo_mixer_pans() {
    fn left(_: f64) -> (f32, f32) {
        (0.5, 0.0)
    }
    let (p, cid) = comp_with(Some("ec.audio.stereomixer"), &[("leftPan", Value::Scalar(100.0))], false);
    let m = mix(&p, &Signal(left), cid, 0.2, 10);
    assert!(m.chunks(2).all(|f| f[0].abs() < 1e-6 && (f[1] - 0.5).abs() < 1e-6), "{:?}", &m[..4]);
}

#[test]
fn tone_makes_a_solid_audible_at_its_frequency() {
    let mut vals: Vec<(&str, Value)> = vec![("frequency1", Value::Scalar(500.0))];
    for k in ["frequency2", "frequency3", "frequency4", "frequency5"] {
        vals.push((k, Value::Scalar(0.0)));
    }
    let (p, cid) = comp_with(Some("ec.audio.tone"), &vals, true);
    assert!(comp_has_audio(&p, cid));
    let m = mix(&p, &Signal(ramp), cid, 0.0, SR as usize);
    let left: Vec<f32> = m.iter().step_by(2).copied().collect();
    let ups = left.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
    assert!((ups as i64 - 500).abs() <= 1, "{ups}");
    // The out point (1 s) silences it.
    let after = mix(&p, &Signal(ramp), cid, 1.0, 100);
    assert!(after.iter().all(|v| *v == 0.0));
    let (p, cid) = comp_with(None, &[], true);
    assert!(!comp_has_audio(&p, cid));
}

/// Mixing in video-frame blocks gives the same samples as one long block (pre-roll makes
/// effects stateless across calls), so preview and export agree.
#[test]
fn blocks_match_one_long_mix() {
    fn noise(t: f64) -> (f32, f32) {
        let x = ((t * 12345.678).sin() * 43758.5453).fract() as f32;
        (x, -x)
    }
    for id in [
        "ec.audio.reverb",
        "ec.audio.basstreble",
        "ec.audio.flangechorus",
        "ec.audio.modulator",
        "ec.audio.delay",
        "ec.audio.parametriceq",
        "ec.audio.compressor",
        "ec.audio.gate",
        "ec.audio.distortion",
    ] {
        let vals: Vec<(&str, Value)> = match id {
            "ec.audio.basstreble" => vec![("bass", Value::Scalar(50.0))],
            "ec.audio.parametriceq" => vec![("band1BoostCut", Value::Scalar(9.0))],
            _ => vec![],
        };
        let (p, cid) = comp_with(Some(id), &vals, false);
        let long = mix(&p, &Signal(noise), cid, 0.2, 1600);
        let mut blocks = vec![];
        for k in 0..5 {
            blocks.extend(mix(&p, &Signal(noise), cid, 0.2 + k as f64 * 0.04, 320));
        }
        let worst = long.iter().zip(&blocks).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        assert!(worst < 2e-3, "{id}: {worst}");
    }
}

#[test]
fn peak_summary_bins() {
    let s: Vec<f32> = (0..10).flat_map(|i| [i as f32 * 0.1, -(i as f32) * 0.1]).collect();
    let p = peak_summary(&s, 4);
    assert_eq!(p.len(), 3);
    assert!(p[0][0].abs() < 1e-6 && (p[0][1] - 0.3).abs() < 1e-6);
    assert!((p[1][2] + 0.7).abs() < 1e-6 && (p[1][3] + 0.4).abs() < 1e-6);
    let f = audio_footage();
    let peaks = footage_peaks(&Signal(ramp), ItemId(1), &f, Tick::from_seconds_f64(2.0), 100);
    assert_eq!(peaks.len(), 200);
    assert!((peaks[150][1] - 1.51).abs() < 0.01, "{:?}", peaks[150]);
}

/// Audio Spectrum reads its Audio Layer's samples at the frame time, so the layer cache keys it
/// by time: a cached frame after the sound starts matches an uncached render (#209).
#[test]
fn audio_spectrum_follows_time_through_the_layer_cache() {
    fn late_tone(t: f64) -> (f32, f32) {
        let v = if t < 0.5 { 0.0 } else { (2.0 * std::f64::consts::PI * 1000.0 * t).sin() as f32 };
        (v, v)
    }
    let mut p = Project::default();
    let comp = Comp::new(64, 64, FrameRate::new(25, 1), Tick::from_seconds_f64(2.0));
    let audio = p.add_item("x.wav", effectcraft_color::Label::SeaFoam, None, ItemKind::Footage(audio_footage()));
    let a = build::layer(&mut p, &comp, "a", LayerSource::Footage { item: audio }, (64, 64), None);
    let sid = p.add_item("S", effectcraft_color::Label::Red, None, ItemKind::Solid(Solid { color: [0.0; 3], width: 64, height: 64, pixel_aspect: 1.0 }));
    let mut s = build::layer(&mut p, &comp, "s", LayerSource::Solid { item: sid }, (64, 64), None);
    let spec = effectcraft_effects::find("ec.generate.audiospectrum").unwrap();
    let mut next = p.next_id;
    let mut g = effectcraft_effects::instantiate(spec, &mut Ids(&mut next), spec.name, [64.0, 64.0]);
    p.next_id = next;
    g.prop_mut("audioLayer").unwrap().value = Value::Layer(Some(a.id.0));
    s.props.sub_mut("effects").unwrap().children.push(g.into());
    let mut comp = comp;
    comp.layers = vec![s, a];
    let cid = p.add_item("C", effectcraft_color::Label::Sandstone, None, ItemKind::Comp(comp.into()));

    let src = Signal(late_tone);
    let render = |t: f64, cache: Option<&crate::LayerCache>| {
        let mut r = crate::Renderer::new(&p, &src, crate::RenderOpts::default());
        r.cache = cache;
        r.comp_frame(cid, Tick::from_seconds_f64(t))
    };
    let lit = |img: &Image| img.data.iter().filter(|px| px[0] + px[1] + px[2] > 0.1).count();
    let cache = crate::LayerCache::default();
    let silent = render(0.0, Some(&cache));
    let cached = render(0.8, Some(&cache));
    let fresh = render(0.8, None);
    assert!(lit(&fresh) > lit(&silent), "the tone draws more than the baseline: {} vs {}", lit(&fresh), lit(&silent));
    assert_eq!(cached.data, fresh.data, "cached {} px, uncached {} px", lit(&cached), lit(&fresh));
}

/// A normal export block can span more than the allocation cap at timelapse speeds.
/// Check every interpolated sample, including source-page edges and nested compositions.
#[test]
fn strongly_stretched_audio_reads_bounded_chunks() {
    struct Indexed(std::sync::atomic::AtomicUsize);
    impl FootageSource for Indexed {
        fn frame(&self, _: ItemId, _: &Footage, _: Tick) -> Option<Arc<Image>> {
            None
        }
        fn audio(&self, _: ItemId, _: &Footage, start: Tick, frames: usize, rate: u32) -> Option<Vec<f32>> {
            self.0.fetch_max(frames, std::sync::atomic::Ordering::Relaxed);
            let first = start.to_units_floor(i64::from(rate));
            Some(
                (0..frames)
                    .flat_map(|i| {
                        let v = ((first + i as i64) % 257) as f32 / 257.0;
                        [v, -v]
                    })
                    .collect(),
            )
        }
    }
    for rate in [44100, 48000] {
        for speed in [65.0, 65.5355, 128.5] {
            for nested in [false, true] {
                let (mut p, inner) = comp_with(None, &[], false);
                p.comp_mut(inner).unwrap().layers[0].out_point = Tick::from_seconds_f64(120.0);
                let cid = if nested {
                    let mut outer = Comp::new(64, 64, FrameRate::new(25, 1), Tick::from_seconds_f64(1.0));
                    let layer = build::layer(&mut p, &outer, "nested", LayerSource::Comp { item: inner }, (64, 64), None);
                    outer.layers.push(layer);
                    p.add_item("outer", effectcraft_color::Label::Sandstone, None, ItemKind::Comp(outer.into()))
                } else {
                    inner
                };
                p.comp_mut(cid).unwrap().layers[0].stretch = 100.0 / speed;
                let source = Indexed(std::sync::atomic::AtomicUsize::new(0));
                let start = Tick::from_seconds_f64(0.1);
                let first = p.comp(cid).unwrap().layers[0].layer_time(start).to_units_floor(i64::from(rate)) as usize;
                let samples = mix_comp(&p, &source, None, cid, start, 16384, rate).unwrap();
                for i in 0..16384 {
                    let x = i as f64 * speed;
                    let j = x as usize;
                    let a = ((first + j) % 257) as f32 / 257.0;
                    let b = ((first + j + 1) % 257) as f32 / 257.0;
                    let expected = a + (b - a) * (x - j as f64) as f32;
                    assert!((samples[i * 2] - expected).abs() < 1e-6, "rate={rate} speed={speed} nested={nested} sample={i}");
                    assert_eq!(samples[i * 2], -samples[i * 2 + 1]);
                }
                assert!(source.0.load(std::sync::atomic::Ordering::Relaxed) <= 1_048_576);
            }
        }
    }
}

/// Oversized stretched reads retain cumulative work limits and observe cancellation between pages.
#[test]
fn strongly_stretched_audio_retains_shared_limits() {
    use crate::audio::{AudioError, MixControl, mix_comp_with_control};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    struct Stops {
        cancel: Arc<AtomicBool>,
        reads: AtomicUsize,
        stop: bool,
    }
    impl FootageSource for Stops {
        fn frame(&self, _: ItemId, _: &Footage, _: Tick) -> Option<Arc<Image>> {
            None
        }
        fn audio(&self, _: ItemId, _: &Footage, _: Tick, frames: usize, _: u32) -> Option<Vec<f32>> {
            self.reads.fetch_add(1, Ordering::Relaxed);
            if self.stop {
                self.cancel.store(true, Ordering::Relaxed);
            }
            Some(vec![0.25; frames * 2])
        }
    }
    let (mut p, cid) = comp_with(None, &[], false);
    p.comp_mut(cid).unwrap().layers[0].stretch = 1.0;
    for stop in [false, true] {
        let cancel = Arc::new(AtomicBool::new(false));
        let source = Stops { cancel: cancel.clone(), reads: AtomicUsize::new(0), stop };
        let control = MixControl { cancel: Some(cancel), max_work: if stop { None } else { Some(1_638_400 + 32_768) }, ..Default::default() };
        assert_eq!(
            mix_comp_with_control(&p, &source, None, cid, Tick::ZERO, 16384, 48000, &control),
            Err(if stop { AudioError::Cancelled } else { AudioError::WorkLimit })
        );
        assert_eq!(source.reads.load(Ordering::Relaxed), if stop { 1 } else { 24 }, "limits must stop source work early");
    }
}
