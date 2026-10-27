use super::*;
use crate::{EffectEnv, EffectHost, LayerPixels, MaskShape, Params};

/// Run with point defaults scaled to the layer size.
pub(crate) fn run_env(id: &str, set: &[(&str, Value)], img: Image, time: f64, env: EffectEnv) -> Buf {
    let s = crate::find(id).unwrap();
    let ls = [img.width as f64, img.height as f64];
    let mut params = Params { values: s.params.iter().map(|p| (p.id.to_string(), p.default.clone())).collect() };
    for p in &s.params {
        if let (ParamUi::Point, Value::Vec2(v)) = (&p.ui, &p.default) {
            params.values.insert(p.id.to_string(), Value::Vec2([v[0] * ls[0], v[1] * ls[1]]));
        }
    }
    for (k, v) in set {
        assert!(params.values.contains_key(*k), "{id}: unknown {k}");
        params.values.insert(k.to_string(), v.clone());
    }
    let ctx = EffectCtx { params: &params, time, layer_size: ls, seed: 3, adjustment: false, env };
    crate::apply(s, &ctx, Buf { img, offset: [0.0, 0.0], scale: 1.0 })
}

fn run(id: &str, set: &[(&str, Value)], img: Image) -> Buf {
    run_env(id, set, img, 0.5, EffectEnv::default())
}

fn square_mask() -> Vec<MaskShape> {
    vec![MaskShape { name: "Mask 1".into(), points: vec![[16.0, 16.0], [48.0, 16.0], [48.0, 48.0], [16.0, 48.0]], closed: true, inverted: false }]
}

fn alpha_sum(img: &Image, x0: usize, y0: usize, x1: usize, y1: usize) -> f32 {
    let mut s = 0.0;
    for y in y0..y1 {
        for x in x0..x1 {
            s += img.data[y * img.width as usize + x][3];
        }
    }
    s
}

#[test]
fn audio_spectrum_linear_frequency_and_linear_amplitude() {
    let host = Tone; // Tone produces 440 Hz
    let env = EffectEnv { host: Some(&host), comp_time: 1.0, ..Default::default() };
    let img = Image::new(200, 100);
    let out = run_env(
        "ec.generate.audiospectrum",
        &[
            ("audioLayer", Value::Layer(Some(1))),
            ("startPoint", Value::Vec2([0.0, 50.0])),
            ("endPoint", Value::Vec2([200.0, 50.0])),
            ("startFrequency", num(20.0)),
            ("endFrequency", num(2000.0)),
            ("frequencyBands", num(100.0)),
            ("maximumHeight", num(40.0)),
            ("thickness", num(2.0)),
            ("softness", num(0.0)),
        ],
        img,
        1.0,
        env,
    );
    // Verify 440 Hz tone produces a peak above baseline, while high frequencies have zero bar height
    let peak_zone = alpha_sum(&out.img, 36, 10, 48, 48);
    let high_zone = alpha_sum(&out.img, 120, 10, 150, 48);
    let silent_zone = alpha_sum(&out.img, 170, 10, 195, 48);
    assert!(peak_zone > 5.0, "peak zone has energy: {peak_zone}");
    assert!(peak_zone > high_zone * 2.0, "linear peak is distinctly taller than far frequencies: {peak_zone} vs {high_zone}");
    assert_eq!(silent_zone, 0.0, "silent end has zero height, not boosted by dB scale");
}

#[test]
fn fractal_deterministic_and_has_inside_set() {
    let img = Image::new(64, 48);
    let a = run("ec.generate.fractal", &[], img.clone());
    let b = run("ec.generate.fractal", &[], img.clone());
    assert_eq!(a.img.data, b.img.data);
    // The default view contains the main cardioid (black, opaque) and escaped (coloured) points.
    assert!(a.img.data.iter().any(|p| p[3] > 0.99 && p[0] + p[1] + p[2] < 0.01));
    assert!(a.img.data.iter().any(|p| p[0] + p[1] + p[2] > 0.3));
    let t = run("ec.generate.fractal", &[("fractalColor/transparency", Value::Bool(true))], img);
    assert!(t.img.data.iter().any(|p| p[3] == 0.0));
}

#[test]
fn fractal_palettes_and_set_choices() {
    let img = Image::new(64, 48);
    // Solid Color: only the inside of the set is drawn; Transparency flips it.
    let solid = run("ec.generate.fractal", &[("fractalColor/palette", Value::Enum(3))], img.clone());
    assert!(solid.img.data.iter().any(|p| p[3] == 0.0) && solid.img.data.iter().any(|p| p[3] > 0.99 && p[0] > 0.9));
    let flipped = run("ec.generate.fractal", &[("fractalColor/palette", Value::Enum(3)), ("fractalColor/transparency", Value::Bool(true))], img.clone());
    for (a, b) in solid.img.data.iter().zip(&flipped.img.data) {
        assert!((a[3] > 0.5) != (b[3] > 0.5) || (a[3] - 0.5).abs() < 0.49);
    }
    // Black And White: escaped pixels are pure black or white (away from supersampled edges).
    let bw = run("ec.generate.fractal", &[("fractalColor/palette", Value::Enum(2)), ("highQualitySettings/samplingFactor", num(1.0))], img.clone());
    assert!(bw.img.data.iter().all(|p| p[0] == p[1] && p[1] == p[2] && (p[0] == 0.0 || p[0] == 1.0)));
    // Mandelbrot Over Julia depends on the Julia centre.
    let o1 = run("ec.generate.fractal", &[("setChoice", Value::Enum(2)), ("julia/juliaX", num(0.3))], img.clone());
    let o2 = run("ec.generate.fractal", &[("setChoice", Value::Enum(2)), ("julia/juliaX", num(-0.2))], img.clone());
    assert_ne!(o1.img.data, o2.img.data);
    let plain = run("ec.generate.fractal", &[("julia/juliaX", num(0.3))], img);
    assert_ne!(o1.img.data, plain.img.data);
}

#[test]
fn scribble_fills_mask_and_animates() {
    let masks = square_mask();
    let env = EffectEnv { masks: &masks, ..Default::default() };
    let img = Image::filled(64, 64, [0.0, 0.0, 1.0, 1.0]);
    let a = run_env("ec.generate.scribble", &[], img.clone(), 0.5, env);
    let b = run_env("ec.generate.scribble", &[], img.clone(), 0.5, env);
    assert_eq!(a.img.data, b.img.data);
    let inside = alpha_sum(&a.img, 18, 18, 46, 46);
    let outside = alpha_sum(&a.img, 0, 0, 10, 64);
    assert!(inside > 100.0 && outside < 1.0, "{inside} {outside}");
    let c = run_env("ec.generate.scribble", &[], img.clone(), 1.5, env);
    assert_ne!(a.img.data, c.img.data, "jumpy wiggle changes over time");
    let s = run_env("ec.generate.scribble", &[("wiggleType", Value::Enum(0))], img.clone(), 0.5, env);
    let s2 = run_env("ec.generate.scribble", &[("wiggleType", Value::Enum(0))], img.clone(), 3.5, env);
    assert_eq!(s.img.data, s2.img.data, "static wiggle is constant");
    let half = run_env("ec.generate.scribble", &[("end", num(50.0))], img, 0.5, env);
    assert!(alpha_sum(&half.img, 0, 0, 64, 64) < alpha_sum(&a.img, 0, 0, 64, 64));
}

#[test]
fn scribble_edge_types() {
    let masks = square_mask();
    let env = EffectEnv { masks: &masks, ..Default::default() };
    let img = Image::new(64, 64);
    let o = run_env("ec.generate.scribble", &[("fillType", Value::Enum(3)), ("edgeOptions/edgeWidth", num(6.0))], img, 0.5, env);
    // Outside edge: paint near the border outside, nothing in the centre.
    assert!(alpha_sum(&o.img, 26, 26, 38, 38) < 0.5);
    assert!(alpha_sum(&o.img, 8, 8, 16, 56) > 5.0);
}

#[test]
fn stroke_draws_along_mask() {
    let masks = square_mask();
    let env = EffectEnv { masks: &masks, ..Default::default() };
    let img = Image::new(64, 64);
    let a = run_env("ec.generate.stroke", &[("brushSize", num(4.0)), ("paintStyle", Value::Enum(1))], img.clone(), 0.0, env);
    assert!(a.img.get(32, 16)[3] > 0.9, "on the top edge");
    assert!(a.img.get(32, 32)[3] < 0.01, "centre empty");
    let b = run_env("ec.generate.stroke", &[("brushSize", num(4.0)), ("end", num(25.0))], img.clone(), 0.0, env);
    assert!(b.img.get(32, 16)[3] > 0.9 && b.img.get(48, 32)[3] < 0.01, "first quarter only");
    let d = run_env("ec.generate.stroke", &[("brushSize", num(4.0)), ("spacing", num(300.0))], img, 0.0, env);
    assert!(alpha_sum(&d.img, 0, 0, 64, 64) < alpha_sum(&a.img, 0, 0, 64, 64), "spaced dabs");
    // No masks: unchanged.
    let src = Image::filled(8, 8, [0.2, 0.3, 0.4, 1.0]);
    assert_eq!(run("ec.generate.stroke", &[], src.clone()).img.data, src.data);
}

#[test]
fn vegas_traces_contours() {
    let mut img = Image::new(64, 64);
    for y in 16..48 {
        for x in 16..48 {
            img.set(x, y, [1.0, 1.0, 1.0, 1.0]);
        }
    }
    let a =
        run("ec.generate.vegas", &[("blendMode", Value::Enum(0)), ("length", num(1.0)), ("midpointOpacity", num(1.0)), ("endOpacity", num(1.0))], img.clone());
    let b =
        run("ec.generate.vegas", &[("blendMode", Value::Enum(0)), ("length", num(1.0)), ("midpointOpacity", num(1.0)), ("endOpacity", num(1.0))], img.clone());
    assert_eq!(a.img.data, b.img.data);
    assert!(alpha_sum(&a.img, 14, 14, 50, 50) > 50.0, "edges drawn");
    assert!(alpha_sum(&a.img, 26, 26, 38, 38) < 0.5, "interior untouched");
    let r = run("ec.generate.vegas", &[("blendMode", Value::Enum(0)), ("rotation", num(90.0))], img);
    assert_ne!(r.img.data, a.img.data);
}

#[test]
fn contours_of_square_are_closed() {
    let mut pl = Plane::new(20, 20);
    for y in 5..15 {
        for x in 5..15 {
            pl.data[y * 20 + x] = 1.0;
        }
    }
    let c = contours(&pl, 0.5);
    assert_eq!(c.len(), 1);
    assert!(c[0].1);
    let l = poly_length(&c[0].0, true);
    assert!((l - 40.0).abs() < 4.0, "{l}");
}

#[test]
fn write_on_and_glue_gun_draw_at_brush() {
    let img = Image::new(32, 32);
    let a = run("ec.generate.writeon", &[], img.clone());
    assert!(a.img.get(16, 16)[3] > 0.9 && a.img.get(2, 2)[3] == 0.0);
    assert_eq!(a.img.data, run("ec.generate.writeon", &[], img.clone()).img.data);
    let src = Image::filled(32, 32, [0.5, 0.2, 0.1, 1.0]);
    let g = run("ec.generate.ccgluegun", &[], src.clone());
    assert_eq!(g.img.data, run("ec.generate.ccgluegun", &[], src.clone()).img.data);
    assert_ne!(g.img.get(16, 16), src.get(16, 16));
    assert_eq!(g.img.get(0, 0), src.get(0, 0));
}

/// #536: an animated Brush Position draws a stroke that stays (Stroke Length 0), expires
/// after Stroke Length, and lays its dabs Brush Spacing seconds apart.
#[test]
fn write_on_accumulates_a_stroke_along_the_animated_brush() {
    // Brush Position from [8, 16] at 0 s to [56, 16] at 1 s; red, hard, 6 px.
    struct Brush(Vec<(&'static str, Value)>);
    impl Brush {
        fn at(&self, t: f64) -> Vec<(&'static str, Value)> {
            let mut v = self.0.clone();
            v.push(("brushPosition", Value::Vec2([8.0 + 48.0 * t.clamp(0.0, 1.0), 16.0])));
            v
        }
    }
    impl EffectHost for Brush {
        fn layer(&self, _: u64, _: bool) -> Option<LayerPixels> {
            None
        }
        fn audio(&self, _: u64, _: f64, _: usize, _: u32) -> Option<Vec<f32>> {
            None
        }
        fn params_at(&self, t: f64) -> Option<Params> {
            let s = crate::find("ec.generate.writeon").unwrap();
            let mut p = Params { values: s.params.iter().map(|p| (p.id.to_string(), p.default.clone())).collect() };
            for (k, v) in self.at(t) {
                p.values.insert(k.to_string(), v);
            }
            Some(p)
        }
    }
    let render = |extra: &[(&'static str, Value)], t: f64| {
        let mut set = vec![("color", col(1.0, 0.0, 0.0)), ("brushSize", num(6.0)), ("brushHardness", num(100.0)), ("brushSpacing", num(0.05))];
        set.extend(extra.iter().cloned());
        let host = Brush(set);
        let env = EffectEnv { host: Some(&host), ..Default::default() };
        let at = host.at(t);
        run_env("ec.generate.writeon", &at, Image::new(64, 32), t, env).img
    };
    let on = |img: &Image, x: i64| img.get(x, 16)[3] > 0.9;
    let half = render(&[], 0.5);
    assert!(on(&half, 8) && on(&half, 20) && on(&half, 32) && !on(&half, 44) && !on(&half, 56));
    let end = render(&[], 1.0);
    assert!([8, 20, 32, 44, 56].iter().all(|x| on(&end, *x)));
    // Stroke Length 0.25 s: only the last quarter second's trail.
    let trail = render(&[("strokeLength", num(0.25))], 1.0);
    assert!(!on(&trail, 8) && !on(&trail, 32) && on(&trail, 48) && on(&trail, 56));
    // Brush Spacing 0.5 s: three separate marks.
    let spaced = render(&[("brushSpacing", num(0.5))], 1.0);
    assert!(on(&spaced, 8) && on(&spaced, 32) && on(&spaced, 56) && !on(&spaced, 20) && !on(&spaced, 44));
    // Paint Time Properties ▸ Opacity: each dab keeps the opacity it was laid with.
    struct Fade;
    impl EffectHost for Fade {
        fn layer(&self, _: u64, _: bool) -> Option<LayerPixels> {
            None
        }
        fn audio(&self, _: u64, _: f64, _: usize, _: u32) -> Option<Vec<f32>> {
            None
        }
        fn params_at(&self, t: f64) -> Option<Params> {
            let mut p = Brush(vec![("brushHardness", num(100.0)), ("brushSpacing", num(0.05)), ("paintTimeProperties", Value::Enum(1))]).params_at(t)?;
            p.values.insert("brushOpacity".into(), num(if t < 0.5 { 100.0 } else { 20.0 }));
            Some(p)
        }
    }
    let p1 = Fade.params_at(1.0).unwrap();
    let set: Vec<(&str, Value)> = p1.values.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
    let faded = run_env("ec.generate.writeon", &set, Image::new(64, 32), 1.0, EffectEnv { host: Some(&Fade), ..Default::default() }).img;
    assert!(faded.get(8, 16)[3] > 0.9 && (faded.get(56, 16)[3] - 0.2).abs() < 0.05, "{:?} {:?}", faded.get(8, 16), faded.get(56, 16));
}

#[test]
fn paint_bucket_fills_enclosed_region_only() {
    let mut img = Image::filled(32, 32, [1.0, 1.0, 1.0, 1.0]);
    for i in 8..24 {
        for &(x, y) in &[(i, 8), (i, 23), (8, i), (23, i)] {
            img.set(x, y, [0.0, 0.0, 0.0, 1.0]);
        }
    }
    let out = run("ec.generate.paintbucket", &[], img.clone());
    let inside = out.img.get(16, 16);
    assert!(inside[0] > 0.99 && inside[1] < 0.01, "{inside:?}");
    assert_eq!(out.img.get(2, 2), [1.0, 1.0, 1.0, 1.0], "outside untouched");
    assert_eq!(out.img.get(8, 16), [0.0, 0.0, 0.0, 1.0], "border untouched");
    assert_eq!(out.img.data, run("ec.generate.paintbucket", &[], img).img.data);
}

#[test]
fn eyedropper_fill_uses_sample() {
    let mut img = Image::filled(16, 16, [0.0, 0.0, 1.0, 1.0]);
    img.set(8, 8, [1.0, 0.0, 0.0, 1.0]);
    let out = run("ec.generate.eyedropperfill", &[], img.clone());
    assert!(out.img.data.iter().all(|p| *p == [1.0, 0.0, 0.0, 1.0]));
    let blended = run("ec.generate.eyedropperfill", &[("blendWithOriginal", num(100.0))], img.clone());
    assert_eq!(blended.img.data, img.data);
}

#[test]
fn threads_weave_is_deterministic_and_gappy() {
    let src = Image::filled(64, 64, [0.4, 0.6, 0.2, 1.0]);
    let a = run("ec.generate.ccthreads", &[("width", num(16.0)), ("height", num(16.0)), ("coverage", num(50.0))], src.clone());
    assert_eq!(a.img.data, run("ec.generate.ccthreads", &[("width", num(16.0)), ("height", num(16.0)), ("coverage", num(50.0))], src).img.data);
    assert!(a.img.data.iter().any(|p| p[3] == 0.0), "gaps between threads");
    assert!(a.img.data.iter().any(|p| p[3] > 0.99), "threads");
}

struct Tone;
impl EffectHost for Tone {
    fn layer(&self, _: u64, _: bool) -> Option<LayerPixels> {
        None
    }
    fn audio(&self, _: u64, start: f64, frames: usize, rate: u32) -> Option<Vec<f32>> {
        Some(
            (0..frames)
                .flat_map(|i| {
                    let t = start + i as f64 / rate as f64;
                    let v = (2.0 * PI * 440.0 * t).sin() as f32 * 0.8;
                    [v, v]
                })
                .collect(),
        )
    }
}

#[test]
fn audio_effects_without_audio_pass_through() {
    let src = Image::filled(16, 16, [0.3, 0.3, 0.3, 1.0]);
    for id in ["ec.generate.audiospectrum", "ec.generate.audiowaveform"] {
        assert_eq!(run(id, &[], src.clone()).img.data, src.data, "{id}");
    }
}

#[test]
fn audio_effects_draw_with_host() {
    let host = Tone;
    let env = EffectEnv { host: Some(&host), comp_time: 1.0, ..Default::default() };
    let img = Image::new(96, 64);
    for id in ["ec.generate.audiospectrum", "ec.generate.audiowaveform"] {
        let a = run_env(id, &[("audioLayer", Value::Layer(Some(1)))], img.clone(), 1.0, env);
        let b = run_env(id, &[("audioLayer", Value::Layer(Some(1)))], img.clone(), 1.0, env);
        assert_eq!(a.img.data, b.img.data, "{id}");
        assert!(alpha_sum(&a.img, 0, 0, 96, 64) > 10.0, "{id} drew nothing");
    }
}

#[test]
fn fft_finds_tone() {
    let n = 256;
    let mut re: Vec<f64> = (0..n).map(|i| (2.0 * PI * 8.0 * i as f64 / n as f64).cos()).collect();
    let mut im = vec![0.0; n];
    fft(&mut re, &mut im);
    let mags: Vec<f64> = (0..n / 2).map(|k| (re[k] * re[k] + im[k] * im[k]).sqrt()).collect();
    let peak = mags.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).unwrap().0;
    assert_eq!(peak, 8);
}

#[test]
fn stroke_outline_caps_and_joins() {
    let line = vec![vec![[10.0, 10.0], [30.0, 10.0]]];
    let cov = |cap: u32| raster_convex(40, 20, &stroke_outline(&line, 3.0, cap, 0, 4.0));
    // Round caps reach 3 px past the ends (on the axis), Butt stops there, Projecting is square.
    let (round, butt, proj) = (cov(0), cov(1), cov(2));
    assert!(round.get(8, 10) > 0.9 && butt.get(8, 10) < 0.1 && proj.get(8, 10) > 0.9);
    assert!(proj.get(7, 7) > 0.9 && round.get(7, 7) < 0.5, "square corner");
    // A right-angle turn: Miter fills the outer corner, Bevel cuts it.
    let corner = vec![vec![[5.0, 30.0], [30.0, 30.0], [30.0, 5.0]]];
    let j = |join: u32| raster_convex(40, 40, &stroke_outline(&corner, 3.0, 1, join, 4.0));
    let (miter, bevel) = (j(1), j(2));
    assert!(miter.get(32, 32) > 0.9 && bevel.get(32, 32) < 0.1, "{} {}", miter.get(32, 32), bevel.get(32, 32));
    // A tight miter limit falls back to a bevel.
    let limited = raster_convex(40, 40, &stroke_outline(&corner, 3.0, 1, 1, 1.0));
    assert!(limited.get(32, 32) < 0.1);
}

#[test]
fn scribble_caps_and_start_end_apply_to() {
    let masks = square_mask();
    let env = EffectEnv { masks: &masks, ..Default::default() };
    let img = Image::new(64, 64);
    let base = [("wiggleType", Value::Enum(0)), ("strokeWidth", num(3.0))];
    let round = run_env("ec.generate.scribble", &base, img.clone(), 0.0, env);
    let butt = run_env(
        "ec.generate.scribble",
        &[base.as_slice(), &[("edgeOptions/endCap", Value::Enum(1)), ("edgeOptions/join", Value::Enum(2))]].concat(),
        img.clone(),
        0.0,
        env,
    );
    assert_ne!(round.img.data, butt.img.data);
    // Each Stroke: every stroke is trimmed on its own, so the half-way image has ink all over.
    let two = vec![
        crate::MaskShape { name: String::new(), points: vec![[4.0, 4.0], [28.0, 4.0], [28.0, 60.0], [4.0, 60.0]], closed: true, inverted: false },
        crate::MaskShape { name: String::new(), points: vec![[36.0, 4.0], [60.0, 4.0], [60.0, 60.0], [36.0, 60.0]], closed: true, inverted: false },
    ];
    let env2 = EffectEnv { masks: &two, ..Default::default() };
    let seq = [("scribble", Value::Enum(1)), ("end", num(50.0))];
    let all = run_env("ec.generate.scribble", &[base.as_slice(), &seq].concat(), img.clone(), 0.0, env2);
    let each = run_env("ec.generate.scribble", &[base.as_slice(), &seq, &[("startEndApplyTo", Value::Enum(1))]].concat(), img, 0.0, env2);
    assert_ne!(all.img.data, each.img.data);
}
