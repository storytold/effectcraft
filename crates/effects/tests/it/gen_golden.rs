//! Golden hashes of the CPU effects that draw through a shared per-frame plan (M13.28).
//!
//! The GPU compositor rasterises these effects from geometry the CPU computes (bolts, strokes,
//! glyphs, audio marks, wave lists, flood-fill regions, sampled colours); the CPU effects were
//! refactored to build the same plans. The hashes guard against unintended output changes;
//! intended behavior changes require a focused regression test before re-pinning. Each effect
//! renders on a generated layer with masks, another layer and an audio track, at default
//! and non-default parameters and two layer times, in an 8 bpc pipeline (input and output
//! clamped and quantised) and a 32 bpc one, at full and at half resolution. Each frame's pixels
//! are hashed (FNV-1a over the `f32` bits) and compared with the pinned hashes below.
//!
//! The hashes are exact `f32` bit patterns, so they depend on the platform's libm; they are
//! pinned for aarch64 macOS (where the gates run). Elsewhere the test only checks that every
//! case renders deterministically.
//!
//! `GEN_GOLDEN_PRINT=1` prints the table (to re-pin after an intended change).

use effectcraft_effects::{Buf, EffectCtx, EffectEnv, EffectHost, LayerPixels, MaskShape, Params, apply, default_value, find};
use effectcraft_keyframe::Value;
use effectcraft_raster::Image;

const W: u32 = 64;
const H: u32 = 48;
/// Layer id served by [`Host`]: a radial luminance map (also the audio layer).
const MAP: u64 = 8;

/// The effect's own layer: a colour ramp with a clear hole and a half-transparent band.
fn layer(hdr: bool) -> Image {
    let mut img = Image::new(W, H);
    for y in 0..H {
        for x in 0..W {
            let (u, v) = (x as f32 / W as f32, y as f32 / H as f32);
            let (dx, dy) = (x as f32 - 40.0, y as f32 - 20.0);
            let a = if dx * dx + dy * dy < 64.0 {
                0.0
            } else if (30..36).contains(&y) {
                0.45
            } else {
                1.0
            };
            let gain = if hdr && x < 20 { 1.6 } else { 1.0 };
            let c = [u * gain, v * gain, (0.3 + 0.4 * (u * 9.0).sin().abs()) * gain];
            img.set(x, y, [c[0] * a, c[1] * a, c[2] * a, a]);
        }
    }
    img
}

fn map() -> Image {
    let mut img = Image::new(W, H);
    for y in 0..H {
        for x in 0..W {
            let d = ((x as f32 - 30.0).powi(2) + (y as f32 - 22.0).powi(2)).sqrt() / 30.0;
            let l = (1.0 - d).clamp(0.0, 1.0);
            img.set(x, y, [l, l * 0.8, l * 0.5, 1.0]);
        }
    }
    img
}

struct Host;
impl EffectHost for Host {
    fn layer(&self, _: u64, _: bool) -> Option<LayerPixels> {
        Some(LayerPixels { buf: Buf { img: map(), offset: [0.0; 2], scale: 1.0 }, size: [W as f64, H as f64] })
    }
    fn audio(&self, _: u64, start: f64, frames: usize, rate: u32) -> Option<Vec<f32>> {
        // Two tones plus a chirp, stereo interleaved.
        Some(
            (0..frames)
                .flat_map(|i| {
                    let t = start + i as f64 / rate as f64;
                    let l = 0.5 * (t * 440.0 * std::f64::consts::TAU).sin() + 0.2 * (t * 1300.0 * std::f64::consts::TAU).sin();
                    let r = 0.4 * (t * (200.0 + 300.0 * t) * std::f64::consts::TAU).sin();
                    [l as f32, r as f32]
                })
                .collect(),
        )
    }
}

/// The layer's masks (layer pixels): a closed pentagon, a closed quad and an open curve.
fn masks() -> Vec<MaskShape> {
    let pent = (0..5)
        .map(|i| {
            let a = i as f64 * std::f64::consts::TAU / 5.0 - 1.3;
            [30.0 + a.cos() * 17.0, 24.0 + a.sin() * 15.0]
        })
        .collect();
    vec![
        MaskShape { name: "Mask 1".into(), points: pent, closed: true, inverted: false },
        MaskShape { name: "Mask 2".into(), points: vec![[6.3, 5.1], [58.2, 7.7], [55.4, 43.6], [9.1, 40.2]], closed: true, inverted: false },
        MaskShape { name: "Mask 3".into(), points: vec![[4.2, 30.3], [18.7, 12.4], [33.1, 28.9], [47.6, 9.8], [60.2, 25.5]], closed: false, inverted: false },
    ]
}

fn n(v: f64) -> Value {
    Value::Scalar(v)
}
fn e(v: u32) -> Value {
    Value::Enum(v)
}
fn b(v: bool) -> Value {
    Value::Bool(v)
}
fn pt(x: f64, y: f64) -> Value {
    Value::Vec2([x, y])
}
fn c(r: f64, g: f64, bl: f64) -> Value {
    Value::Color([r, g, bl, 1.0])
}

type Set = Vec<(&'static str, Value)>;

/// (effect id, parameter sets) — the first set of each is the defaults.
fn cases() -> Vec<(&'static str, Vec<Set>)> {
    vec![
        (
            "ec.obsolete.lightning",
            vec![
                vec![],
                vec![("branching", n(0.9)), ("rebranching", n(0.6)), ("detailLevel", n(4.0)), ("blendingMode", e(1)), ("pullForce", n(40.0))],
                vec![("blendingMode", e(2)), ("rerunAtEachFrame", b(true)), ("fixedEndpoint", b(true)), ("coreWidth", n(0.8))],
            ],
        ),
        (
            "ec.generate.advancedlightning",
            vec![
                vec![],
                vec![("lightningType", e(4)), ("forking", n(60.0)), ("decayMainCore", b(true)), ("expertSettings/coreDrain", n(40.0))],
                vec![("alphaObstacle", n(6.0)), ("compositeOnOriginal", b(false)), ("expertSettings/fractalType", e(2)), ("glowSettings/glowRadius", n(10.0))],
                vec![("lightningType", e(7)), ("alphaObstacle", n(-4.0)), ("expertSettings/forkVariation", n(50.0))],
            ],
        ),
        (
            "ec.generate.radiowaves",
            vec![
                vec![],
                vec![
                    ("polygon/sides", n(5.0)),
                    ("polygon/star", b(true)),
                    ("waveMotion/spin", n(30.0)),
                    ("waveStroke/profile", e(4)),
                    ("waveMotion/velocity", n(20.0)),
                ],
                vec![("waveType", e(1)), ("imageContour/sourceLayer", Value::Layer(Some(MAP))), ("waveStroke/fadeOutTime", n(1.0))],
                vec![
                    ("waveType", e(2)),
                    ("waveMask/mask", n(1.0)),
                    ("waveMotion/reflection", b(true)),
                    ("waveMotion/velocity", n(40.0)),
                    ("waveMotion/expansion", n(15.0)),
                ],
            ],
        ),
        (
            "ec.generate.vegas",
            vec![
                vec![],
                vec![("stroke", e(1)), ("path", n(3.0)), ("blendMode", e(2)), ("segments", n(12.0)), ("randomPhase", b(true))],
                vec![("inputLayer", Value::Layer(Some(MAP))), ("blendMode", e(3)), ("shorterContoursHave", e(1)), ("threshold", n(30.0))],
            ],
        ),
        (
            "ec.generate.stroke",
            vec![
                vec![],
                vec![("allMasks", b(true)), ("spacing", n(80.0)), ("paintStyle", e(1)), ("end", n(70.0)), ("brushSize", n(6.0))],
                vec![("path", n(3.0)), ("paintStyle", e(2)), ("start", n(20.0)), ("brushHardness", n(10.0))],
            ],
        ),
        (
            "ec.generate.scribble",
            vec![
                vec![],
                vec![("scribble", e(1)), ("fillType", e(2)), ("edgeOptions/endCap", e(2)), ("edgeOptions/join", e(1)), ("composite", e(0))],
                vec![("mask", n(2.0)), ("fillType", e(1)), ("edgeOptions/join", e(2)), ("wiggleType", e(2)), ("end", n(60.0)), ("composite", e(2))],
            ],
        ),
        ("ec.generate.writeon", vec![vec![], vec![("brushPosition", pt(20.0, 30.0)), ("brushSize", n(14.0)), ("paintStyle", e(1))]]),
        (
            "ec.generate.paintbucket",
            vec![
                vec![],
                vec![("fillPoint", pt(10.0, 10.0)), ("fillSelector", e(1)), ("tolerance", n(60.0)), ("stroke", e(4)), ("strokeWidth", n(4.0))],
                vec![("fillPoint", pt(40.0, 20.0)), ("fillSelector", e(2)), ("stroke", e(1)), ("color", c(0.2, 0.9, 0.4))],
                vec![("stroke", e(2)), ("spreadRadius", n(2.5)), ("tolerance", n(120.0)), ("viewThreshold", b(false))],
            ],
        ),
        (
            "ec.generate.eyedropperfill",
            vec![
                vec![],
                vec![("samplePoint", pt(12.0, 30.0)), ("sampleRadius", n(6.0)), ("averagePixelColors", e(3)), ("blendWithOriginal", n(30.0))],
                vec![("sampleRadius", n(9.0)), ("averagePixelColors", e(1)), ("maintainOriginalAlpha", b(true))],
            ],
        ),
        (
            "ec.generate.audiospectrum",
            vec![
                vec![("audioLayer", Value::Layer(Some(MAP)))],
                vec![("audioLayer", Value::Layer(Some(MAP))), ("displayOptions", e(1)), ("sideOptions", e(2)), ("compositeOnOriginal", b(true))],
                vec![("audioLayer", Value::Layer(Some(MAP))), ("displayOptions", e(2)), ("path", n(3.0)), ("usePolarPath", b(false))],
            ],
        ),
        (
            "ec.generate.audiowaveform",
            vec![
                vec![("audioLayer", Value::Layer(Some(MAP)))],
                vec![("audioLayer", Value::Layer(Some(MAP))), ("displayOptions", e(0)), ("waveformOptions", e(2)), ("usePolarPath", b(true))],
                vec![("audioLayer", Value::Layer(Some(MAP))), ("displayOptions", e(2)), ("path", n(1.0)), ("compositeOnOriginal", b(true))],
            ],
        ),
        (
            "ec.obsolete.basictext",
            vec![
                vec![],
                vec![("text", Value::Str("Hi\nthere".into())), ("fillAndStroke/displayOptions", e(2)), ("alignment", e(0)), ("size", n(14.0))],
                vec![("fillAndStroke/displayOptions", e(3)), ("fillAndStroke/strokeWidth", n(5.0)), ("compositeOnOriginal", b(true))],
            ],
        ),
        (
            "ec.obsolete.pathtext",
            vec![
                vec![],
                vec![("pathOptions/shapeType", e(1)), ("advanced/visibleCharacters", n(2.5)), ("advanced/fadeTime", n(50.0)), ("character/size", n(14.0))],
                vec![
                    ("pathOptions/customPath", n(3.0)),
                    ("fillAndStroke/displayOptions", e(2)),
                    ("advanced/jitterSettings/rotationJitterMax", n(20.0)),
                    ("compositeOnOriginal", b(true)),
                ],
            ],
        ),
        (
            "ec.key.innerouter",
            vec![
                vec![("foreground", e(1))],
                vec![("foreground", e(1)), ("background", e(2)), ("edgeFeather", n(3.0)), ("edgeThin", n(1.5))],
                vec![("foreground", e(1)), ("singleMaskHighlightRadius", n(4.0)), ("invertExtraction", b(true)), ("blendWithOriginal", n(20.0))],
            ],
        ),
        (
            "ec.color.colorlink",
            vec![
                vec![],
                vec![("sampleSource", e(2)), ("clip", n(10.0)), ("blendingMode", e(3))],
                vec![("sourceLayer", Value::Layer(Some(MAP))), ("sampleSource", e(1))],
                vec![("sampleSource", e(9)), ("opacity", n(70.0))],
            ],
        ),
    ]
}

/// Layer times.
const TIMES: [f64; 2] = [0.4, 1.3];

fn fnv(img: &Image) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in [img.width, img.height].iter().flat_map(|v| v.to_le_bytes()) {
        h = (h ^ b as u64).wrapping_mul(0x100_0000_01b3);
    }
    for p in &img.data {
        for c in p {
            for b in c.to_bits().to_le_bytes() {
                h = (h ^ b as u64).wrapping_mul(0x100_0000_01b3);
            }
        }
    }
    h
}

fn render(id: &str, set: &Set, time: f64, bpc8: bool, half: bool) -> Image {
    let spec = find(id).unwrap_or_else(|| panic!("no effect {id}"));
    let size = [W as f64, H as f64];
    let mut params = Params { values: spec.params.iter().map(|p| (p.id.to_string(), default_value(p, size))).collect() };
    for (k, v) in set {
        assert!(params.values.contains_key(*k), "{id}: unknown param {k}");
        params.values.insert(k.to_string(), v.clone());
    }
    let mut img = layer(!bpc8);
    if bpc8 {
        img.clamp01();
        img.quantize(255.0);
    }
    let mut buf = Buf { img, offset: [0.0; 2], scale: 1.0 };
    if half {
        // A half-resolution preview buffer padded by 3 px.
        let (w2, h2) = (W / 2, H / 2);
        let mut small = Image::new(w2, h2);
        for y in 0..h2 {
            for x in 0..w2 {
                small.set(x, y, buf.img.sample_bilinear(x as f64 * 2.0 + 1.0, y as f64 * 2.0 + 1.0));
            }
        }
        buf = Buf { img: small, offset: [0.0; 2], scale: 0.5 };
        buf.pad(3);
    }
    let host = Host;
    let masks = masks();
    let env = EffectEnv { host: Some(&host), masks: &masks, frame_rate: 30.0, comp_time: time, ..Default::default() };
    let ctx = EffectCtx { params: &params, time, layer_size: size, seed: 1, adjustment: false, env };
    let mut out = apply(spec, &ctx, buf).img;
    if bpc8 {
        out.clamp01();
        out.quantize(255.0);
    }
    out
}

/// Every case's name and hash, in a fixed order.
fn hashes() -> Vec<(String, u64)> {
    let mut v = Vec::new();
    for (id, sets) in cases() {
        for (si, set) in sets.iter().enumerate() {
            for (bpc8, half) in [(true, false), (false, false), (false, true)] {
                for t in TIMES {
                    let img = render(id, set, t, bpc8, half);
                    let name = format!("{id}/{si}/{}{}/t{t}", if bpc8 { "8bpc" } else { "32bpc" }, if half { "/half" } else { "" });
                    v.push((name, fnv(&img)));
                }
            }
        }
    }
    v
}

/// Pin the intended linear spectrum behavior independently of the pixel hash table.
#[test]
fn audio_spectrum_uses_linear_frequency_and_amplitude() {
    struct Tone {
        frequency: f64,
        amplitude: f64,
    }
    impl EffectHost for Tone {
        fn layer(&self, _: u64, _: bool) -> Option<LayerPixels> {
            None
        }
        fn audio(&self, _: u64, start: f64, frames: usize, rate: u32) -> Option<Vec<f32>> {
            Some(
                (0..frames)
                    .flat_map(|i| {
                        let t = start + i as f64 / rate as f64;
                        let sample = (self.amplitude * (t * self.frequency * std::f64::consts::TAU).sin()) as f32;
                        [sample, sample]
                    })
                    .collect(),
            )
        }
    }
    let id = "ec.generate.audiospectrum";
    let spec = find(id).unwrap();
    let size = [320.0, 100.0];
    let mut params = Params { values: spec.params.iter().map(|p| (p.id.to_string(), default_value(p, size))).collect() };
    for (key, value) in [
        ("audioLayer", Value::Layer(Some(MAP))),
        ("startPoint", pt(0.0, 50.0)),
        ("endPoint", pt(320.0, 50.0)),
        ("startFrequency", n(0.0)),
        ("endFrequency", n(1500.0)),
        ("frequencyBands", n(33.0)),
        ("maximumHeight", n(40.0)),
        ("audioDuration", n(256.0)),
    ] {
        params.values.insert(key.to_string(), value);
    }
    let buf = Buf { img: Image::new(320, 100), offset: [0.0; 2], scale: 1.0 };
    for (frequency, expected_band) in [(375.0, 8), (750.0, 16), (1125.0, 24)] {
        let mut peaks = Vec::new();
        for amplitude in [0.0, 0.125, 0.25] {
            let host = Tone { frequency, amplitude };
            let env = EffectEnv { host: Some(&host), comp_time: 1.0, ..Default::default() };
            let ctx = EffectCtx { params: &params, time: 1.0, layer_size: size, seed: 1, adjustment: false, env };
            let plan = effectcraft_effects::marks_plan(id, &ctx, &buf).unwrap();
            assert_eq!(plan.segs.len(), 33);
            let heights: Vec<f64> = plan.segs.iter().map(|seg| (seg.b[1] - seg.a[1]).abs()).collect();
            assert!(heights.iter().all(|height| height.is_finite()));
            if amplitude == 0.0 {
                assert!(heights.iter().all(|height| *height == 0.0), "silence has no bar height");
                continue;
            }
            let (band, height) = heights.iter().enumerate().max_by(|(_, a), (_, b)| a.total_cmp(b)).unwrap();
            assert_eq!(band, expected_band, "{frequency} Hz uses its linear frequency position");
            // A windowed pure tone retains its amplitude, allowing for the finite Hann window.
            assert!((height - amplitude * 40.0).abs() < 0.01, "{frequency} Hz at {amplitude}: height {height}");
            peaks.push(*height);
        }
        assert!((peaks[1] - 2.0 * peaks[0]).abs() < 1e-6, "doubling the signal doubles the height: {peaks:?}");
    }
}

#[test]
fn plan_effects_golden_hashes() {
    let got = hashes();
    if std::env::var_os("GEN_GOLDEN_PRINT").is_some() {
        for (name, h) in &got {
            println!("    (\"{name}\", 0x{h:016x}),");
        }
    }
    assert_eq!(got, hashes(), "the effects render deterministically");
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        let bad: Vec<String> = got
            .iter()
            .zip(GOLDEN)
            .filter(|((n, h), (gn, gh))| n != gn || h != gh)
            .map(|((n, h), (_, gh))| format!("{n}: 0x{h:016x} (pinned 0x{gh:016x})"))
            .collect();
        assert_eq!(got.len(), GOLDEN.len(), "case count");
        assert!(bad.is_empty(), "CPU output changed:\n{}", bad.join("\n"));
    }
}

/// Pinned hashes (aarch64 macOS).
const GOLDEN: &[(&str, u64)] = &[
    ("ec.obsolete.lightning/0/8bpc/t0.4", 0xd829bb3a79ec2d61),
    ("ec.obsolete.lightning/0/8bpc/t1.3", 0xb2221364c0defe9b),
    ("ec.obsolete.lightning/0/32bpc/t0.4", 0x828cc4e53d579af7),
    ("ec.obsolete.lightning/0/32bpc/t1.3", 0x15770238cde9e766),
    ("ec.obsolete.lightning/0/32bpc/half/t0.4", 0xf64f2b87fa9a09d3),
    ("ec.obsolete.lightning/0/32bpc/half/t1.3", 0x625a00bd52fde5aa),
    ("ec.obsolete.lightning/1/8bpc/t0.4", 0xa7c8cb94beebca84),
    ("ec.obsolete.lightning/1/8bpc/t1.3", 0x6e583b1d1f455099),
    ("ec.obsolete.lightning/1/32bpc/t0.4", 0x2ffd973cc7de98f7),
    ("ec.obsolete.lightning/1/32bpc/t1.3", 0x8f35e6018cac57d7),
    ("ec.obsolete.lightning/1/32bpc/half/t0.4", 0x96c14e911783f8d6),
    ("ec.obsolete.lightning/1/32bpc/half/t1.3", 0x620ee605f56c2068),
    ("ec.obsolete.lightning/2/8bpc/t0.4", 0x119e5cedac21cd9b),
    ("ec.obsolete.lightning/2/8bpc/t1.3", 0x04b775fba837ddce),
    ("ec.obsolete.lightning/2/32bpc/t0.4", 0xfee767091adac4ce),
    ("ec.obsolete.lightning/2/32bpc/t1.3", 0x2ae26f9670c93751),
    ("ec.obsolete.lightning/2/32bpc/half/t0.4", 0x99111f77bf26704f),
    ("ec.obsolete.lightning/2/32bpc/half/t1.3", 0x8eb781efc14183c3),
    ("ec.generate.advancedlightning/0/8bpc/t0.4", 0x5a53bf1e0236225d),
    ("ec.generate.advancedlightning/0/8bpc/t1.3", 0x5a53bf1e0236225d),
    ("ec.generate.advancedlightning/0/32bpc/t0.4", 0x6817bf5b3583e0a0),
    ("ec.generate.advancedlightning/0/32bpc/t1.3", 0x6817bf5b3583e0a0),
    ("ec.generate.advancedlightning/0/32bpc/half/t0.4", 0x5dcc055f6cca9922),
    ("ec.generate.advancedlightning/0/32bpc/half/t1.3", 0x5dcc055f6cca9922),
    ("ec.generate.advancedlightning/1/8bpc/t0.4", 0xf07f11457fa683d0),
    ("ec.generate.advancedlightning/1/8bpc/t1.3", 0xf07f11457fa683d0),
    ("ec.generate.advancedlightning/1/32bpc/t0.4", 0x1b21531df9187431),
    ("ec.generate.advancedlightning/1/32bpc/t1.3", 0x1b21531df9187431),
    ("ec.generate.advancedlightning/1/32bpc/half/t0.4", 0xf1032a0a9345edf1),
    ("ec.generate.advancedlightning/1/32bpc/half/t1.3", 0xf1032a0a9345edf1),
    ("ec.generate.advancedlightning/2/8bpc/t0.4", 0x62b5d179f05c6a4c),
    ("ec.generate.advancedlightning/2/8bpc/t1.3", 0x62b5d179f05c6a4c),
    ("ec.generate.advancedlightning/2/32bpc/t0.4", 0x192256fe81f73975),
    ("ec.generate.advancedlightning/2/32bpc/t1.3", 0x192256fe81f73975),
    ("ec.generate.advancedlightning/2/32bpc/half/t0.4", 0x953dd51008c3ce31),
    ("ec.generate.advancedlightning/2/32bpc/half/t1.3", 0x953dd51008c3ce31),
    ("ec.generate.advancedlightning/3/8bpc/t0.4", 0x57b6ad61bd3d89e0),
    ("ec.generate.advancedlightning/3/8bpc/t1.3", 0x57b6ad61bd3d89e0),
    ("ec.generate.advancedlightning/3/32bpc/t0.4", 0x7e7c9d20e94c8b6c),
    ("ec.generate.advancedlightning/3/32bpc/t1.3", 0x7e7c9d20e94c8b6c),
    ("ec.generate.advancedlightning/3/32bpc/half/t0.4", 0xa37940f5a490a0fe),
    ("ec.generate.advancedlightning/3/32bpc/half/t1.3", 0xa37940f5a490a0fe),
    ("ec.generate.radiowaves/0/8bpc/t0.4", 0xbc3643b8af12c44b),
    ("ec.generate.radiowaves/0/8bpc/t1.3", 0x703e948dc562e132),
    ("ec.generate.radiowaves/0/32bpc/t0.4", 0x2b0db208ee7517c1),
    ("ec.generate.radiowaves/0/32bpc/t1.3", 0x7b32484a1fec7db3),
    ("ec.generate.radiowaves/0/32bpc/half/t0.4", 0x166c2672959953cb),
    ("ec.generate.radiowaves/0/32bpc/half/t1.3", 0x5820255022c11f22),
    ("ec.generate.radiowaves/1/8bpc/t0.4", 0x7b74eaa4d0501c4e),
    ("ec.generate.radiowaves/1/8bpc/t1.3", 0xcab53207ecf01165),
    ("ec.generate.radiowaves/1/32bpc/t0.4", 0x433e2457d21bfe74),
    ("ec.generate.radiowaves/1/32bpc/t1.3", 0x2b2c3b2284170a7d),
    ("ec.generate.radiowaves/1/32bpc/half/t0.4", 0x9752bb53cb40c823),
    ("ec.generate.radiowaves/1/32bpc/half/t1.3", 0x8641d37e339914fe),
    ("ec.generate.radiowaves/2/8bpc/t0.4", 0x8ad88caae775ea83),
    ("ec.generate.radiowaves/2/8bpc/t1.3", 0x1f32a9a4a45ff7b3),
    ("ec.generate.radiowaves/2/32bpc/t0.4", 0xe2b8332b96a1c76e),
    ("ec.generate.radiowaves/2/32bpc/t1.3", 0x60f6e6b2a4f5e466),
    ("ec.generate.radiowaves/2/32bpc/half/t0.4", 0x60e6c05f8f8fe967),
    ("ec.generate.radiowaves/2/32bpc/half/t1.3", 0x6fcd7f86aedfbf1d),
    ("ec.generate.radiowaves/3/8bpc/t0.4", 0xa52d7f06548c7d3c),
    ("ec.generate.radiowaves/3/8bpc/t1.3", 0x7a3a0bed3bdd53cf),
    ("ec.generate.radiowaves/3/32bpc/t0.4", 0xbcab47ad7445c601),
    ("ec.generate.radiowaves/3/32bpc/t1.3", 0x940a18e2bd95f75e),
    ("ec.generate.radiowaves/3/32bpc/half/t0.4", 0xb485746038429450),
    ("ec.generate.radiowaves/3/32bpc/half/t1.3", 0xcf62c191e2aa8cc4),
    ("ec.generate.vegas/0/8bpc/t0.4", 0x5e58d44254e9101b),
    ("ec.generate.vegas/0/8bpc/t1.3", 0x5e58d44254e9101b),
    ("ec.generate.vegas/0/32bpc/t0.4", 0xe18712686e402759),
    ("ec.generate.vegas/0/32bpc/t1.3", 0xe18712686e402759),
    ("ec.generate.vegas/0/32bpc/half/t0.4", 0xb037c97234bbd936),
    ("ec.generate.vegas/0/32bpc/half/t1.3", 0xb037c97234bbd936),
    ("ec.generate.vegas/1/8bpc/t0.4", 0x68566eb5dc411cd3),
    ("ec.generate.vegas/1/8bpc/t1.3", 0x68566eb5dc411cd3),
    ("ec.generate.vegas/1/32bpc/t0.4", 0xaacea0d8fbb176ae),
    ("ec.generate.vegas/1/32bpc/t1.3", 0xaacea0d8fbb176ae),
    ("ec.generate.vegas/1/32bpc/half/t0.4", 0xe2f8f5f4a12628d6),
    ("ec.generate.vegas/1/32bpc/half/t1.3", 0xe2f8f5f4a12628d6),
    ("ec.generate.vegas/2/8bpc/t0.4", 0x002c48a91aea2dcf),
    ("ec.generate.vegas/2/8bpc/t1.3", 0x002c48a91aea2dcf),
    ("ec.generate.vegas/2/32bpc/t0.4", 0x4f550bbbf8b8316b),
    ("ec.generate.vegas/2/32bpc/t1.3", 0x4f550bbbf8b8316b),
    ("ec.generate.vegas/2/32bpc/half/t0.4", 0x30036df3e56a01ed),
    ("ec.generate.vegas/2/32bpc/half/t1.3", 0x30036df3e56a01ed),
    ("ec.generate.stroke/0/8bpc/t0.4", 0xa580fbfb806d655c),
    ("ec.generate.stroke/0/8bpc/t1.3", 0xa580fbfb806d655c),
    ("ec.generate.stroke/0/32bpc/t0.4", 0xbec2fe1ec01e1d3c),
    ("ec.generate.stroke/0/32bpc/t1.3", 0xbec2fe1ec01e1d3c),
    ("ec.generate.stroke/0/32bpc/half/t0.4", 0x7c6ae5abc7d0f6e4),
    ("ec.generate.stroke/0/32bpc/half/t1.3", 0x7c6ae5abc7d0f6e4),
    ("ec.generate.stroke/1/8bpc/t0.4", 0x4c9ff27861fe68dd),
    ("ec.generate.stroke/1/8bpc/t1.3", 0x4c9ff27861fe68dd),
    ("ec.generate.stroke/1/32bpc/t0.4", 0xfd1f31366b8d5c1d),
    ("ec.generate.stroke/1/32bpc/t1.3", 0xfd1f31366b8d5c1d),
    ("ec.generate.stroke/1/32bpc/half/t0.4", 0x4dd9dbb22169eae5),
    ("ec.generate.stroke/1/32bpc/half/t1.3", 0x4dd9dbb22169eae5),
    ("ec.generate.stroke/2/8bpc/t0.4", 0x4caead423d6d5ecf),
    ("ec.generate.stroke/2/8bpc/t1.3", 0x4caead423d6d5ecf),
    ("ec.generate.stroke/2/32bpc/t0.4", 0x9ad6539da7e9f26f),
    ("ec.generate.stroke/2/32bpc/t1.3", 0x9ad6539da7e9f26f),
    ("ec.generate.stroke/2/32bpc/half/t0.4", 0xe6d239affe36da4e),
    ("ec.generate.stroke/2/32bpc/half/t1.3", 0xe6d239affe36da4e),
    ("ec.generate.scribble/0/8bpc/t0.4", 0x2e4cbde3ccd30e11),
    ("ec.generate.scribble/0/8bpc/t1.3", 0xe021ad3960135205),
    ("ec.generate.scribble/0/32bpc/t0.4", 0xce10491af6674d85),
    ("ec.generate.scribble/0/32bpc/t1.3", 0x6e9278119b86a775),
    ("ec.generate.scribble/0/32bpc/half/t0.4", 0x968e88723205ae41),
    ("ec.generate.scribble/0/32bpc/half/t1.3", 0x2f69d6431024b969),
    ("ec.generate.scribble/1/8bpc/t0.4", 0x21e7b74d0c7426a1),
    ("ec.generate.scribble/1/8bpc/t1.3", 0x38fe4c9a597fbe00),
    ("ec.generate.scribble/1/32bpc/t0.4", 0xbd6add569300ad1a),
    ("ec.generate.scribble/1/32bpc/t1.3", 0xadfa4f7102cadd9a),
    ("ec.generate.scribble/1/32bpc/half/t0.4", 0x2924a0669270a714),
    ("ec.generate.scribble/1/32bpc/half/t1.3", 0x33784c624845f913),
    ("ec.generate.scribble/2/8bpc/t0.4", 0x83906d3aacbab171),
    ("ec.generate.scribble/2/8bpc/t1.3", 0xe41b5809995bdf9e),
    ("ec.generate.scribble/2/32bpc/t0.4", 0x8c5321d1274edb78),
    ("ec.generate.scribble/2/32bpc/t1.3", 0x905bdccaf9b98a5c),
    ("ec.generate.scribble/2/32bpc/half/t0.4", 0x9d07d455e6a51d41),
    ("ec.generate.scribble/2/32bpc/half/t1.3", 0xc8cddc8a88c8024b),
    ("ec.generate.writeon/0/8bpc/t0.4", 0x38a58bc3538a16e4),
    ("ec.generate.writeon/0/8bpc/t1.3", 0x38a58bc3538a16e4),
    ("ec.generate.writeon/0/32bpc/t0.4", 0x4d555ffd999b1065),
    ("ec.generate.writeon/0/32bpc/t1.3", 0x4d555ffd999b1065),
    ("ec.generate.writeon/0/32bpc/half/t0.4", 0x8ab7075374b11f07),
    ("ec.generate.writeon/0/32bpc/half/t1.3", 0x8ab7075374b11f07),
    ("ec.generate.writeon/1/8bpc/t0.4", 0x21eaeca83e433c55),
    ("ec.generate.writeon/1/8bpc/t1.3", 0x21eaeca83e433c55),
    ("ec.generate.writeon/1/32bpc/t0.4", 0x15d5c8948781d675),
    ("ec.generate.writeon/1/32bpc/t1.3", 0x15d5c8948781d675),
    ("ec.generate.writeon/1/32bpc/half/t0.4", 0x7b849d248824de9d),
    ("ec.generate.writeon/1/32bpc/half/t1.3", 0x7b849d248824de9d),
    ("ec.generate.paintbucket/0/8bpc/t0.4", 0x41390203c012ade6),
    ("ec.generate.paintbucket/0/8bpc/t1.3", 0x41390203c012ade6),
    ("ec.generate.paintbucket/0/32bpc/t0.4", 0x2cc586c148219d9d),
    ("ec.generate.paintbucket/0/32bpc/t1.3", 0x2cc586c148219d9d),
    ("ec.generate.paintbucket/0/32bpc/half/t0.4", 0x0e8eedcff76d46c6),
    ("ec.generate.paintbucket/0/32bpc/half/t1.3", 0x0e8eedcff76d46c6),
    ("ec.generate.paintbucket/1/8bpc/t0.4", 0x7577e2e528aa3ab9),
    ("ec.generate.paintbucket/1/8bpc/t1.3", 0x7577e2e528aa3ab9),
    ("ec.generate.paintbucket/1/32bpc/t0.4", 0x62b69ad0e2b4dfd8),
    ("ec.generate.paintbucket/1/32bpc/t1.3", 0x62b69ad0e2b4dfd8),
    ("ec.generate.paintbucket/1/32bpc/half/t0.4", 0xf5ea3b0a7ac224ed),
    ("ec.generate.paintbucket/1/32bpc/half/t1.3", 0xf5ea3b0a7ac224ed),
    ("ec.generate.paintbucket/2/8bpc/t0.4", 0x530d0a248dc22c72),
    ("ec.generate.paintbucket/2/8bpc/t1.3", 0x530d0a248dc22c72),
    ("ec.generate.paintbucket/2/32bpc/t0.4", 0xe1b655f8815e1357),
    ("ec.generate.paintbucket/2/32bpc/t1.3", 0xe1b655f8815e1357),
    ("ec.generate.paintbucket/2/32bpc/half/t0.4", 0xb95b3b205255cd7e),
    ("ec.generate.paintbucket/2/32bpc/half/t1.3", 0xb95b3b205255cd7e),
    ("ec.generate.paintbucket/3/8bpc/t0.4", 0xa7485b7277e03e66),
    ("ec.generate.paintbucket/3/8bpc/t1.3", 0xa7485b7277e03e66),
    ("ec.generate.paintbucket/3/32bpc/t0.4", 0xc8e06c6eed30448b),
    ("ec.generate.paintbucket/3/32bpc/t1.3", 0xc8e06c6eed30448b),
    ("ec.generate.paintbucket/3/32bpc/half/t0.4", 0xfe0d6223c876db05),
    ("ec.generate.paintbucket/3/32bpc/half/t1.3", 0xfe0d6223c876db05),
    ("ec.generate.eyedropperfill/0/8bpc/t0.4", 0x86248fa83ab166d5),
    ("ec.generate.eyedropperfill/0/8bpc/t1.3", 0x86248fa83ab166d5),
    ("ec.generate.eyedropperfill/0/32bpc/t0.4", 0x5e9ef63fca8086d5),
    ("ec.generate.eyedropperfill/0/32bpc/t1.3", 0x5e9ef63fca8086d5),
    ("ec.generate.eyedropperfill/0/32bpc/half/t0.4", 0x1b6649c0688d829d),
    ("ec.generate.eyedropperfill/0/32bpc/half/t1.3", 0x1b6649c0688d829d),
    ("ec.generate.eyedropperfill/1/8bpc/t0.4", 0x8fd6bb78816570cc),
    ("ec.generate.eyedropperfill/1/8bpc/t1.3", 0x8fd6bb78816570cc),
    ("ec.generate.eyedropperfill/1/32bpc/t0.4", 0x30d8489019c071fe),
    ("ec.generate.eyedropperfill/1/32bpc/t1.3", 0x30d8489019c071fe),
    ("ec.generate.eyedropperfill/1/32bpc/half/t0.4", 0x13a90c31c89568ea),
    ("ec.generate.eyedropperfill/1/32bpc/half/t1.3", 0x13a90c31c89568ea),
    ("ec.generate.eyedropperfill/2/8bpc/t0.4", 0xbfe145e68fb67e0f),
    ("ec.generate.eyedropperfill/2/8bpc/t1.3", 0xbfe145e68fb67e0f),
    ("ec.generate.eyedropperfill/2/32bpc/t0.4", 0x500e27795935722a),
    ("ec.generate.eyedropperfill/2/32bpc/t1.3", 0x500e27795935722a),
    ("ec.generate.eyedropperfill/2/32bpc/half/t0.4", 0x480fd6a293c6597c),
    ("ec.generate.eyedropperfill/2/32bpc/half/t1.3", 0x480fd6a293c6597c),
    // Linear frequency bands and amplitude from 581326ec; all other effects retain their hashes.
    ("ec.generate.audiospectrum/0/8bpc/t0.4", 0x07be49f76890493c),
    ("ec.generate.audiospectrum/0/8bpc/t1.3", 0xc0109e19c3958517),
    ("ec.generate.audiospectrum/0/32bpc/t0.4", 0x24fe2e48cc432c1f),
    ("ec.generate.audiospectrum/0/32bpc/t1.3", 0x9bdbf835e7d42f78),
    ("ec.generate.audiospectrum/0/32bpc/half/t0.4", 0x445e16a81e313a40),
    ("ec.generate.audiospectrum/0/32bpc/half/t1.3", 0x88849fde30e6b048),
    ("ec.generate.audiospectrum/1/8bpc/t0.4", 0xf95a769e610688cc),
    ("ec.generate.audiospectrum/1/8bpc/t1.3", 0x7df3a63a59a10c38),
    ("ec.generate.audiospectrum/1/32bpc/t0.4", 0x53f3e442e240599b),
    ("ec.generate.audiospectrum/1/32bpc/t1.3", 0x213066492a32f6a8),
    ("ec.generate.audiospectrum/1/32bpc/half/t0.4", 0x38ceb79d523c7402),
    ("ec.generate.audiospectrum/1/32bpc/half/t1.3", 0x7d63c37d260eb112),
    ("ec.generate.audiospectrum/2/8bpc/t0.4", 0x73c15c41ad557cec),
    ("ec.generate.audiospectrum/2/8bpc/t1.3", 0x0bb38170a3fbb6fd),
    ("ec.generate.audiospectrum/2/32bpc/t0.4", 0x2ac65e1ca97db691),
    ("ec.generate.audiospectrum/2/32bpc/t1.3", 0xa8be6d0571415ab0),
    ("ec.generate.audiospectrum/2/32bpc/half/t0.4", 0x318b82f955d58300),
    ("ec.generate.audiospectrum/2/32bpc/half/t1.3", 0x4775b23a8cc7c6c6),
    ("ec.generate.audiowaveform/0/8bpc/t0.4", 0x68a7733a023bc9f1),
    ("ec.generate.audiowaveform/0/8bpc/t1.3", 0x57e13a1aaa458b14),
    ("ec.generate.audiowaveform/0/32bpc/t0.4", 0xb78e946d794b5ade),
    ("ec.generate.audiowaveform/0/32bpc/t1.3", 0x2acdbc2a2558c847),
    ("ec.generate.audiowaveform/0/32bpc/half/t0.4", 0x99da8b92d697e171),
    ("ec.generate.audiowaveform/0/32bpc/half/t1.3", 0xdfe01c4d3368ea92),
    ("ec.generate.audiowaveform/1/8bpc/t0.4", 0x5f463552fd80eda6),
    ("ec.generate.audiowaveform/1/8bpc/t1.3", 0x4e724e55141cb0e2),
    ("ec.generate.audiowaveform/1/32bpc/t0.4", 0xcba797b9142cc9bc),
    ("ec.generate.audiowaveform/1/32bpc/t1.3", 0x1efc192f83ff114f),
    ("ec.generate.audiowaveform/1/32bpc/half/t0.4", 0x5f81670776ddb569),
    ("ec.generate.audiowaveform/1/32bpc/half/t1.3", 0xf56e5d8f8445125f),
    ("ec.generate.audiowaveform/2/8bpc/t0.4", 0xb9fb312b8553ee47),
    ("ec.generate.audiowaveform/2/8bpc/t1.3", 0x08640b88a8496eb1),
    ("ec.generate.audiowaveform/2/32bpc/t0.4", 0x12a7502643511c69),
    ("ec.generate.audiowaveform/2/32bpc/t1.3", 0xc5f3b7bc698f6328),
    ("ec.generate.audiowaveform/2/32bpc/half/t0.4", 0x2409e86ae0f5e841),
    ("ec.generate.audiowaveform/2/32bpc/half/t1.3", 0x5518084e651af2c6),
    ("ec.obsolete.basictext/0/8bpc/t0.4", 0x97a4c8907a44ab03),
    ("ec.obsolete.basictext/0/8bpc/t1.3", 0x97a4c8907a44ab03),
    ("ec.obsolete.basictext/0/32bpc/t0.4", 0x79bfd2ba4e76c65e),
    ("ec.obsolete.basictext/0/32bpc/t1.3", 0x79bfd2ba4e76c65e),
    ("ec.obsolete.basictext/0/32bpc/half/t0.4", 0xd1c0bf17ed973ec3),
    ("ec.obsolete.basictext/0/32bpc/half/t1.3", 0xd1c0bf17ed973ec3),
    ("ec.obsolete.basictext/1/8bpc/t0.4", 0xa3457f3a34ff4655),
    ("ec.obsolete.basictext/1/8bpc/t1.3", 0xa3457f3a34ff4655),
    ("ec.obsolete.basictext/1/32bpc/t0.4", 0x2583d7ce25436fe8),
    ("ec.obsolete.basictext/1/32bpc/t1.3", 0x2583d7ce25436fe8),
    ("ec.obsolete.basictext/1/32bpc/half/t0.4", 0x2c08ade06e4e7bbc),
    ("ec.obsolete.basictext/1/32bpc/half/t1.3", 0x2c08ade06e4e7bbc),
    ("ec.obsolete.basictext/2/8bpc/t0.4", 0xfce091451c0ac93a),
    ("ec.obsolete.basictext/2/8bpc/t1.3", 0xfce091451c0ac93a),
    ("ec.obsolete.basictext/2/32bpc/t0.4", 0x86cd4111d46f6bf8),
    ("ec.obsolete.basictext/2/32bpc/t1.3", 0x86cd4111d46f6bf8),
    ("ec.obsolete.basictext/2/32bpc/half/t0.4", 0xe8c740683bcf309f),
    ("ec.obsolete.basictext/2/32bpc/half/t1.3", 0xe8c740683bcf309f),
    ("ec.obsolete.pathtext/0/8bpc/t0.4", 0x70ca391f2e77dd8b),
    ("ec.obsolete.pathtext/0/8bpc/t1.3", 0x70ca391f2e77dd8b),
    ("ec.obsolete.pathtext/0/32bpc/t0.4", 0x98faf5dd968c859b),
    ("ec.obsolete.pathtext/0/32bpc/t1.3", 0x98faf5dd968c859b),
    ("ec.obsolete.pathtext/0/32bpc/half/t0.4", 0xf903dfeeb23c8394),
    ("ec.obsolete.pathtext/0/32bpc/half/t1.3", 0xf903dfeeb23c8394),
    ("ec.obsolete.pathtext/1/8bpc/t0.4", 0xeac5ee9e29df401c),
    ("ec.obsolete.pathtext/1/8bpc/t1.3", 0xeac5ee9e29df401c),
    ("ec.obsolete.pathtext/1/32bpc/t0.4", 0x2228529a378510e0),
    ("ec.obsolete.pathtext/1/32bpc/t1.3", 0x2228529a378510e0),
    ("ec.obsolete.pathtext/1/32bpc/half/t0.4", 0x15f6edfc65f2baec),
    ("ec.obsolete.pathtext/1/32bpc/half/t1.3", 0x15f6edfc65f2baec),
    ("ec.obsolete.pathtext/2/8bpc/t0.4", 0x3ae68427950162d9),
    ("ec.obsolete.pathtext/2/8bpc/t1.3", 0x3879c008d5655bcc),
    ("ec.obsolete.pathtext/2/32bpc/t0.4", 0xdc9616ff88ce1362),
    ("ec.obsolete.pathtext/2/32bpc/t1.3", 0x0062c20d2a4c9d99),
    ("ec.obsolete.pathtext/2/32bpc/half/t0.4", 0x373d3ede825faeed),
    ("ec.obsolete.pathtext/2/32bpc/half/t1.3", 0xc2680bc43740918a),
    ("ec.key.innerouter/0/8bpc/t0.4", 0x4342d220a1bee59b),
    ("ec.key.innerouter/0/8bpc/t1.3", 0x4342d220a1bee59b),
    ("ec.key.innerouter/0/32bpc/t0.4", 0x7251dbed6a1fafe6),
    ("ec.key.innerouter/0/32bpc/t1.3", 0x7251dbed6a1fafe6),
    ("ec.key.innerouter/0/32bpc/half/t0.4", 0x003941eb2cd18339),
    ("ec.key.innerouter/0/32bpc/half/t1.3", 0x003941eb2cd18339),
    ("ec.key.innerouter/1/8bpc/t0.4", 0xd7bfea3d9550db56),
    ("ec.key.innerouter/1/8bpc/t1.3", 0xd7bfea3d9550db56),
    ("ec.key.innerouter/1/32bpc/t0.4", 0x62f7705e43cc6a71),
    ("ec.key.innerouter/1/32bpc/t1.3", 0x62f7705e43cc6a71),
    ("ec.key.innerouter/1/32bpc/half/t0.4", 0x22f15f770f22f072),
    ("ec.key.innerouter/1/32bpc/half/t1.3", 0x22f15f770f22f072),
    ("ec.key.innerouter/2/8bpc/t0.4", 0x7bdece44f841bb11),
    ("ec.key.innerouter/2/8bpc/t1.3", 0x7bdece44f841bb11),
    ("ec.key.innerouter/2/32bpc/t0.4", 0x5130a1fc09c42120),
    ("ec.key.innerouter/2/32bpc/t1.3", 0x5130a1fc09c42120),
    ("ec.key.innerouter/2/32bpc/half/t0.4", 0x46c0880853618fc8),
    ("ec.key.innerouter/2/32bpc/half/t1.3", 0x46c0880853618fc8),
    ("ec.color.colorlink/0/8bpc/t0.4", 0x539d1da6b2ed744a),
    ("ec.color.colorlink/0/8bpc/t1.3", 0x539d1da6b2ed744a),
    ("ec.color.colorlink/0/32bpc/t0.4", 0x67ed14da67ce1a97),
    ("ec.color.colorlink/0/32bpc/t1.3", 0x67ed14da67ce1a97),
    ("ec.color.colorlink/0/32bpc/half/t0.4", 0x30d29c51622dcc97),
    ("ec.color.colorlink/0/32bpc/half/t1.3", 0x30d29c51622dcc97),
    ("ec.color.colorlink/1/8bpc/t0.4", 0x89ec922f2c8941ed),
    ("ec.color.colorlink/1/8bpc/t1.3", 0x89ec922f2c8941ed),
    ("ec.color.colorlink/1/32bpc/t0.4", 0x684dda31f96ab6fb),
    ("ec.color.colorlink/1/32bpc/t1.3", 0x684dda31f96ab6fb),
    ("ec.color.colorlink/1/32bpc/half/t0.4", 0x93507fbaf4779a8d),
    ("ec.color.colorlink/1/32bpc/half/t1.3", 0x93507fbaf4779a8d),
    ("ec.color.colorlink/2/8bpc/t0.4", 0x8dffbc4fb58aa28d),
    ("ec.color.colorlink/2/8bpc/t1.3", 0x8dffbc4fb58aa28d),
    ("ec.color.colorlink/2/32bpc/t0.4", 0xfcce2f6652e44967),
    ("ec.color.colorlink/2/32bpc/t1.3", 0xfcce2f6652e44967),
    ("ec.color.colorlink/2/32bpc/half/t0.4", 0xfff90f774f305d69),
    ("ec.color.colorlink/2/32bpc/half/t1.3", 0xfff90f774f305d69),
    ("ec.color.colorlink/3/8bpc/t0.4", 0xbbf4d5c846df587a),
    ("ec.color.colorlink/3/8bpc/t1.3", 0xbbf4d5c846df587a),
    ("ec.color.colorlink/3/32bpc/t0.4", 0xa0e34caa39618d1e),
    ("ec.color.colorlink/3/32bpc/t1.3", 0xa0e34caa39618d1e),
    ("ec.color.colorlink/3/32bpc/half/t0.4", 0xd97c6eb3a216573c),
    ("ec.color.colorlink/3/32bpc/half/t1.3", 0xd97c6eb3a216573c),
];
