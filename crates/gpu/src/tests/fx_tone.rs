//! GPU colour-correction family effects vs the CPU effects (the oracle): direct on a buffer at
//! full and half resolution, as adjustment, and composited at 8 and 32 bpc.

use effectcraft_color::Label;
use effectcraft_keyframe::{Keyframe, Value};
use effectcraft_project::{BitDepth, Comp, ItemKind, LayerSource, build};
use effectcraft_render::RenderOpts;
use effectcraft_time::{FrameRate, Tick};

use crate::tests::{Scene, c, check, compare_at, effect_case, effect_direct, effect_direct_on, n, opts, set};

fn e(v: u32) -> Value {
    Value::Enum(v)
}

fn on() -> Value {
    Value::Bool(true)
}

fn off() -> Value {
    Value::Bool(false)
}

#[test]
fn levels_and_gamma() {
    effect_case(
        "ec.color.levelsic",
        &[("rgb/rgbInBlack", n(0.1)), ("rgb/rgbGamma", n(1.3)), ("red/redOutWhite", n(0.8)), ("blue/blueInWhite", n(0.7)), ("alpha/alphaGamma", n(0.7))],
    );
    effect_case("ec.color.levelsic", &[("green/greenInBlack", n(0.2)), ("green/greenInWhite", n(0.6)), ("clipToOutputWhite", e(0))]);
    effect_case("ec.color.levelsic", &[("alpha/alphaOutBlack", n(0.3))]);
    // #3487-#3491: the ends of the valid ranges, the curve bounded where gamma 0 would overflow.
    effect_case(
        "ec.color.levelsic",
        &[
            ("rgb/rgbInWhite", n(0.2)),
            ("rgb/rgbGamma", n(0.0)),
            ("red/redOutBlack", n(-10000.0)),
            ("blue/blueOutWhite", n(10000.0)),
            ("clipToOutputWhite", e(0)),
        ],
    );
    effect_case("ec.color.levels", &[("redInBlack", n(0.1)), ("greenGamma", n(1.5)), ("blueOutWhite", n(0.7)), ("inWhite", n(0.9))]);
    effect_case("ec.color.levels", &[("alphaInWhite", n(0.6)), ("redInWhite", n(0.5)), ("redClipToOutputWhite", e(0))]);
    effect_case("ec.color.gammapedestalgain", &[("redGamma", n(1.4)), ("greenPedestal", n(0.1)), ("blueGain", n(0.8))]);
    effect_case("ec.color.gammapedestalgain", &[("blackStretch", n(2.0)), ("redGain", n(1.2))]);
}

fn levels_controls(prefix: &str, values: [f64; 5]) -> Vec<(String, Value)> {
    let ids = if prefix.is_empty() {
        ["inBlack", "inWhite", "gamma", "outBlack", "outWhite", "clipToOutputBlack", "clipToOutputWhite"].map(String::from)
    } else {
        ["InBlack", "InWhite", "Gamma", "OutBlack", "OutWhite", "ClipToOutputBlack", "ClipToOutputWhite"].map(|s| format!("{prefix}{s}"))
    };
    ids.into_iter().zip(values.map(n).into_iter().chain([e(0), e(0)])).collect()
}

#[test]
fn levels_stays_finite_and_matches_the_cpu_with_clipping_off() {
    let empty_input = [0.5, 0.5, 0.1, 0.0, 1.0];
    let empty_input_and_output = [0.5, 0.5, 0.1, 0.5, 0.5];
    let cases = [
        levels_controls("", empty_input_and_output),
        levels_controls("", empty_input),
        levels_controls("red", empty_input),
        levels_controls("alpha", empty_input),
        [levels_controls("", empty_input), levels_controls("red", empty_input)].concat(),
        [levels_controls("", [0.0, 0.0, 0.1, 0.0, 1.0]), levels_controls("red", [0.0, 1.0, 10.0, -1.0, -0.999])].concat(),
    ];
    for owned in &cases {
        let vals: Vec<(&str, Value)> = owned.iter().map(|(id, v)| (id.as_str(), v.clone())).collect();
        for adjustment in [false, true] {
            for out in effect_direct("ec.color.levels", &vals, adjustment) {
                assert!(out.data.iter().flatten().all(|v| v.is_finite()), "{vals:?} adj {adjustment}");
            }
        }
    }
}

#[test]
fn levels_matches_the_cpu_at_the_edge_of_f32() {
    let edge = [f32::from_bits(0x7f3f_ffff), f32::MAX, 0.0071313, -f32::MAX, 0.25];
    let source = |w: u32, h: u32| {
        let mut img = effectcraft_raster::Image::new(w, h);
        for (i, px) in img.data.iter_mut().enumerate() {
            *px = [edge[i % 5], edge[(i + 1) % 5], edge[(i + 2) % 5], 1.0];
        }
        img
    };
    let settings = [[0.0, 0.75, 10.0, 0.0, 1.0], [0.0, 0.0, 0.1, 0.0, 0.001], [0.0, 0.5, 10.0, 0.0, 1.0], [0.0, 0.5, 1.0, 0.0, 1.0]];
    for prefix in ["", "red"] {
        for values in settings {
            let unclipped = levels_controls(prefix, values);
            let clip_white =
                [unclipped.clone(), vec![(if prefix.is_empty() { "clipToOutputWhite".into() } else { format!("{prefix}ClipToOutputWhite") }, e(1))]].concat();
            for owned in [unclipped, clip_white] {
                let vals: Vec<(&str, Value)> = owned.iter().map(|(id, v)| (id.as_str(), v.clone())).collect();
                for out in effect_direct_on("ec.color.levels", &vals, false, source) {
                    assert!(out.data.iter().flatten().all(|v| v.is_finite()), "{vals:?}");
                }
            }
        }
    }
}

fn filled(px: [f32; 4]) -> impl Fn(u32, u32) -> effectcraft_raster::Image + Copy {
    move |w, h| effectcraft_raster::Image::filled(w, h, px)
}

#[test]
fn levels_keeps_finite_results_near_f32_max_distinct() {
    let owned = levels_controls("", [0.0, 1.0, 1.0, 1.0, 2.0]);
    let vals: Vec<(&str, Value)> = owned.iter().map(|(id, v)| (id.as_str(), v.clone())).collect();
    for out in effect_direct_on("ec.color.levels", &vals, false, filled([3.4006e38, 3.4022e38, f32::MAX, 1.0])) {
        let [r, g, b, _] = out.data[0];
        assert!(r < g && g < f32::MAX && b.is_finite() && g <= b, "{:?}", out.data[0]);
    }
}

#[test]
fn levels_channel_stage_reads_a_saturated_master_back_finite() {
    let owned = [levels_controls("", [0.5, 0.5, 0.1, 0.0, 1.0]), levels_controls("red", [0.0, 1.0, 10.0, 0.0, 1.0])].concat();
    let vals: Vec<(&str, Value)> = owned.iter().map(|(id, v)| (id.as_str(), v.clone())).collect();
    for alpha in [0.75, 0.3, 1.0] {
        for out in effect_direct_on("ec.color.levels", &vals, false, filled([0.45 * alpha, 0.55 * alpha, 0.0, alpha])) {
            assert!(out.data.iter().flatten().all(|v| v.is_finite()), "alpha {alpha}: {:?}", out.data[0]);
        }
    }
}

#[test]
fn hue_saturation_ranges() {
    effect_case("ec.color.huesaturation", &[("redsHue", n(40.0)), ("bluesSaturation", n(-60.0)), ("greensLightness", n(30.0))]);
    effect_case(
        "ec.color.huesaturation",
        &[
            ("hue", n(20.0)),
            ("saturation", n(15.0)),
            ("yellowsSaturation", n(50.0)),
            ("yellowsRangeStart", n(30.0)),
            ("yellowsStartFalloff", n(8.0)),
            ("cyansLightness", n(-40.0)),
        ],
    );
}

#[test]
fn colour_replacement() {
    effect_case("ec.color.photofilter", &[]);
    effect_case(
        "ec.color.photofilter",
        &[("filter", e(effectcraft_effects::PHOTO_FILTER_CUSTOM)), ("color", c(0.2, 0.7, 0.4)), ("density", n(60.0)), ("preserveLuminosity", off())],
    );
    effect_case("ec.color.changetocolor", &[("from", c(0.8, 0.4, 0.3)), ("to", c(0.2, 0.5, 0.9)), ("toleranceGroup/hue", n(15.0))]);
    effect_case(
        "ec.color.changetocolor",
        &[("from", c(0.6, 0.6, 0.3)), ("to", c(0.9, 0.2, 0.6)), ("change", e(3)), ("changeBy", e(1)), ("toleranceGroup/hue", n(25.0)), ("softness", n(30.0))],
    );
    effect_case("ec.color.changetocolor", &[("from", c(0.6, 0.6, 0.3)), ("change", e(2)), ("toleranceGroup/hue", n(25.0)), ("viewCorrectionMatte", on())]);
    effect_case("ec.color.leavecolor", &[("amount", n(80.0)), ("color", c(0.7, 0.5, 0.4)), ("tolerance", n(10.0)), ("edgeSoftness", n(20.0))]);
    effect_case(
        "ec.color.leavecolor",
        &[("amount", n(100.0)), ("color", c(0.8, 0.4, 0.3)), ("matchColors", e(1)), ("tolerance", n(5.0)), ("edgeSoftness", n(30.0))],
    );
    effect_case("ec.color.changecolor", &[("hueTransform", n(90.0)), ("colorToChange", c(0.7, 0.5, 0.4)), ("matchingSoftness", n(20.0))]);
    // #3480: near the top of After Effects' range (four and three quarter turns).
    effect_case("ec.color.changecolor", &[("hueTransform", n(1710.0)), ("colorToChange", c(0.7, 0.5, 0.4)), ("matchingSoftness", n(20.0))]);
    effect_case(
        "ec.color.changecolor",
        &[
            ("lightnessTransform", n(-30.0)),
            ("saturationTransform", n(40.0)),
            ("matchColors", e(0)),
            ("matchingTolerance", n(20.0)),
            ("matchingSoftness", n(15.0)),
        ],
    );
    effect_case("ec.color.changecolor", &[("view", e(1)), ("matchColors", e(2)), ("matchingSoftness", n(25.0)), ("invertColorCorrectionMask", on())]);
}

#[test]
fn broadcast_balance_limiter() {
    effect_case("ec.color.broadcast", &[("maxSignal", n(95.0))]);
    effect_case("ec.color.broadcast", &[("locale", e(1)), ("howToMakeSafe", e(1)), ("maxSignal", n(90.0))]);
    effect_case("ec.color.colorbalancehls", &[("hue", n(40.0)), ("lightness", n(-20.0)), ("saturation", n(30.0))]);
    effect_case("ec.color.colorbalancehls", &[("lightness", n(25.0)), ("saturation", n(-50.0))]);
    effect_case("ec.color.videolimiter", &[("clipLevel", e(0))]);
    effect_case("ec.color.videolimiter", &[("clipLevel", e(1)), ("clipMethod", e(1)), ("compressionBeforeClipping", e(4))]);
    effect_case("ec.color.videolimiter", &[("clipLevel", e(0)), ("clipMethod", e(0)), ("gamutWarning", on())]);
}

#[test]
fn cc_colour_and_arbitrary_map() {
    effect_case("ec.color.cctoner", &[]);
    effect_case("ec.color.cctoner", &[("tones", e(2)), ("blend", n(30.0))]);
    effect_case("ec.color.cctoner", &[("tones", e(0)), ("shadows", c(0.1, 0.0, 0.3)), ("highlights", c(1.0, 0.9, 0.6))]);
    effect_case("ec.color.cccoloroffset", &[("redPhase", n(90.0)), ("bluePhase", n(200.0))]);
    effect_case("ec.color.cccoloroffset", &[("greenPhase", n(120.0)), ("overflow", e(1))]);
    effect_case("ec.color.cckernel", &[("k1", n(-1.0)), ("k5", n(5.0)), ("k9", n(-1.0)), ("scale", n(0.5)), ("offset", n(0.05))]);
    effect_case("ec.color.cckernel", &[("k2", n(1.0)), ("k5", n(0.0)), ("k8", n(-1.0)), ("absolute", on())]);
    let map: String = (0..256).map(|i| format!("{}", ((i as f64 / 255.0).powf(0.6) * 255.0).round())).collect::<Vec<_>>().join(",");
    effect_case("ec.color.psarbitrarymap", &[("map", Value::Str(map.clone()))]);
    effect_case("ec.color.psarbitrarymap", &[("map", Value::Str(map)), ("phase", n(40.0)), ("applyPhaseMapToAlpha", on())]);
}

#[test]
fn automatic_corrections() {
    effect_case("ec.color.autolevels", &[]);
    effect_case("ec.color.autolevels", &[("blackClip", n(2.0)), ("whiteClip", n(3.0)), ("blend", n(20.0))]);
    effect_case("ec.color.autocontrast", &[("blackClip", n(1.0))]);
    effect_case("ec.color.autocolor", &[]);
    effect_case("ec.color.autocolor", &[("snapNeutralMidtones", on()), ("whiteClip", n(2.0))]);
    effect_case("ec.color.equalize", &[]);
    effect_case("ec.color.equalize", &[("style", e(1)), ("amount", n(70.0))]);
    effect_case("ec.color.equalize", &[("style", e(2))]);
}

#[test]
fn shadow_highlight_neutralizer_invert() {
    effect_case("ec.color.shadowhighlight", &[]);
    effect_case(
        "ec.color.shadowhighlight",
        &[
            ("autoAmounts", off()),
            ("shadowAmount", n(70.0)),
            ("highlightAmount", n(40.0)),
            ("moreOptions/shadowRadius", n(8.0)),
            ("moreOptions/highlightRadius", n(15.0)),
            ("moreOptions/midtoneContrast", n(30.0)),
            ("moreOptions/blackClip", n(2.0)),
            ("blend", n(20.0)),
        ],
    );
    effect_case("ec.color.shadowhighlight", &[("moreOptions/blackClip", n(0.0)), ("moreOptions/whiteClip", n(0.0)), ("moreOptions/colorCorrection", n(-40.0))]);
    effect_case(
        "ec.color.cccolorneutralizer",
        &[("shadowsBalance", c(0.1, 0.0, 0.05)), ("midtonesUnbalance", c(0.6, 0.5, 0.4)), ("pinning", n(40.0)), ("contrast", n(20.0)), ("darks", n(-30.0))],
    );
    effect_case("ec.color.cccolorneutralizer", &[("highlightsBalance", c(0.9, 1.0, 0.8)), ("brights", n(40.0)), ("blendWOriginal", n(25.0))]);
    for ch in 4..=11 {
        effect_case("ec.channel.invert", &[("channel", e(ch)), ("blend", n(20.0))]);
    }
}

/// Color Stabilizer reads its reference frame through the effect host: a precomp whose content
/// moves, at a nonzero time (the direct cases have no host and pass the layer through).
#[test]
fn color_stabilizer() {
    effect_case("ec.color.colorstabilizer", &[]);
    for depth in [BitDepth::Bpc8, BitDepth::Bpc32] {
        for mode in [0, 1, 2] {
            let mut s = Scene::new(depth);
            let bg = s.solid([0.15, 0.1, 0.2], 97, 61);
            s.push(bg);
            let inner = Comp::new(60, 40, FrameRate::FPS_30, Tick::from_seconds_f64(2.0));
            let inner_id = s.p.add_item("Inner", Label::Sandstone, None, ItemKind::Comp(inner.into()));
            let mut a = s.footage(50, 34);
            let pos = a.props.prop_mut("transform/position").unwrap();
            pos.keys = vec![Keyframe::new(Tick::ZERO, crate::tests::v3(22.0, 18.0)), Keyframe::new(Tick::from_seconds_f64(1.0), crate::tests::v3(38.0, 24.0))];
            let op = a.props.prop_mut("transform/opacity").unwrap();
            op.keys = vec![Keyframe::new(Tick::ZERO, Value::Scalar(100.0)), Keyframe::new(Tick::from_seconds_f64(1.0), Value::Scalar(40.0))];
            s.p.comp_mut(inner_id).unwrap().layers = vec![a].into();
            let mut l = build::layer(&mut s.p, &s.comp, "Inner", LayerSource::Comp { item: inner_id }, (60, 40), None);
            let vals = [
                ("stabilize", e(mode)),
                ("blackPoint", Value::Vec2([18.0, 20.0])),
                ("midPoint", Value::Vec2([30.0, 18.0])),
                ("whitePoint", Value::Vec2([42.0, 22.0])),
                ("sampleSize", n(4.0)),
            ];
            s.effect(&mut l, "ec.color.colorstabilizer", &vals);
            set(&mut l, "transform/rotation", Value::Scalar(8.0));
            s.push(l);
            let t = Tick::from_seconds_f64(0.5);
            let label = format!("color stabilizer {mode} {depth:?}");
            check(&label, compare_at(&s, opts(), t), 0.0);
            check(&format!("{label} half"), compare_at(&s, RenderOpts { scale: 0.5, ..opts() }, t), 0.0);
        }
    }
}

#[test]
fn gradient_ramp_matches_the_cpu() {
    effect_case("ec.generate.gradientramp", &[]);
}

/// #3487-#3491: at the ends of the valid ranges the GPU's Levels (Individual Controls) output is
/// finite (the CPU / GPU comparison counts infinity on both sides as equal).
#[test]
fn levels_ic_is_finite_at_the_ends_of_its_ranges() {
    let Some(g) = crate::tests::gpu() else { return };
    let spec = effectcraft_effects::find("ec.color.levelsic").unwrap();
    let size = [16.0, 8.0];
    let mut params =
        effectcraft_effects::Params { values: spec.params.iter().map(|ps| (ps.id.to_string(), effectcraft_effects::default_value(ps, size))).collect() };
    for (k, v) in [
        ("rgb/rgbInWhite", n(0.2)),
        ("rgb/rgbGamma", n(0.0)),
        ("red/redOutBlack", n(-10000.0)),
        ("blue/blueOutWhite", n(10000.0)),
        ("green/greenOutWhite", n(0.0)),
        ("clipToOutputWhite", e(0)),
    ] {
        params.values.insert(k.to_string(), v);
    }
    let mut img = crate::tests::pattern(7, 16, 8);
    for p in &mut img.data {
        *p = p.map(|v| v * 3.0);
    }
    let buf = effectcraft_effects::Buf { img, offset: [0.0, 0.0], scale: 1.0 };
    let ctx = effectcraft_effects::EffectCtx { params: &params, time: 0.0, layer_size: size, seed: 1, adjustment: false, env: Default::default() };
    let out = effectcraft_render::Accelerator::effects(g, &[effectcraft_render::FxStep { spec, ctx }], &buf, None)
        .expect("the GPU runs Levels (Individual Controls)");
    assert!(out.img.data.iter().flatten().all(|v| v.is_finite()), "{:?}", out.img.data.iter().find(|p| p.iter().any(|v| !v.is_finite())));
}
