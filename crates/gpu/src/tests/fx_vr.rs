//! GPU Immersive Video family vs the CPU effects (the oracle): direct on a buffer (full and half
//! resolution, as adjustment) and composited at 8 and 32 bpc, in mono and both stereo layouts.
//!
//! Exception: re-projections that cross a cube-map face edge or a fisheye rim may pick the
//! neighbouring face / side for directions within f32 rounding of the edge (the CPU decides in
//! f64), so up to 0.5 % of the pixels of those cases may differ.

use effectcraft_keyframe::Value;
use effectcraft_project::BitDepth;
use effectcraft_render::RenderOpts;
use effectcraft_time::Tick;

use crate::tests::{Scene, c, check, compare_at, diff, gpu, n, opts, pattern, set};

fn e(v: u32) -> Value {
    Value::Enum(v)
}

fn on() -> Value {
    Value::Bool(true)
}

/// The effect alone on a buffer (`allow` = fraction of pixels allowed over 1e-3).
fn direct(id: &str, vals: &[(&str, Value)], allow: f64) {
    let Some(g) = gpu() else { return };
    let spec = effectcraft_effects::find(id).unwrap();
    let size = [96.0, 48.0];
    let mut params =
        effectcraft_effects::Params { values: spec.params.iter().map(|ps| (ps.id.to_string(), effectcraft_effects::default_value(ps, size))).collect() };
    for (k, v) in vals {
        params.values.insert(k.to_string(), v.clone());
    }
    for adjustment in [false, true] {
        for scale in [1.0, 0.5] {
            let img = pattern(7, (size[0] * scale) as u32, (size[1] * scale) as u32);
            let buf = effectcraft_effects::Buf { img, offset: [0.0, 0.0], scale };
            let ctx = || effectcraft_effects::EffectCtx { params: &params, time: 0.4, layer_size: size, seed: 11, adjustment, env: Default::default() };
            let cpu = (spec.render)(&ctx(), buf.clone());
            let out =
                effectcraft_render::Accelerator::effects(g, &[effectcraft_render::FxStep { spec, ctx: ctx() }], &buf, None).expect("the GPU runs the effect");
            assert_eq!((out.offset, out.scale), (cpu.offset, cpu.scale), "{id}: geometry");
            let d = diff(&cpu.img, &out.img, 1e-3);
            assert!(
                d.over as f64 <= allow * d.total as f64,
                "{id} {vals:?} adj {adjustment} scale {scale}: {} of {} pixels over 1e-3 (max {}); worst {:?}",
                d.over,
                d.total,
                d.max,
                d.worst
            );
        }
    }
}

fn case_allow(id: &str, vals: &[(&str, Value)], allow: f64) {
    direct(id, vals, allow);
    for depth in [BitDepth::Bpc8, BitDepth::Bpc32] {
        let mut s = Scene::new(depth);
        let bg = s.solid([0.15, 0.1, 0.2], 97, 61);
        s.push(bg);
        let mut l = s.footage(96, 48);
        s.effect(&mut l, id, vals);
        set(&mut l, "transform/rotation", Value::Scalar(8.0));
        s.push(l);
        check(&format!("{id} {vals:?} {depth:?}"), compare_at(&s, opts(), Tick::ZERO), allow);
        check(&format!("{id} {vals:?} {depth:?} half"), compare_at(&s, RenderOpts { scale: 0.5, ..opts() }, Tick::ZERO), allow);
    }
}

fn case(id: &str, vals: &[(&str, Value)]) {
    case_allow(id, vals, 0.0);
}

/// `vals` in mono and in both stereo layouts.
fn layouts(id: &str, vals: &[(&str, Value)]) {
    for l in 0..3 {
        case(id, &[vals, &[("frameLayout", e(l))]].concat());
    }
}

#[test]
fn blur_sharpen_glow() {
    layouts("ec.vr.blur", &[("blurriness", n(12.0))]);
    case("ec.vr.blur", &[("blurriness", n(60.0))]);
    layouts("ec.vr.sharpen", &[("sharpenAmount", n(60.0))]);
    layouts("ec.vr.glow", &[("luminanceThreshold", n(40.0)), ("glowRadius", n(10.0))]);
    case("ec.vr.glow", &[("useTintColor", on()), ("glowSaturation", n(150.0)), ("glowBrightness", n(250.0))]);
}

#[test]
fn denoise() {
    layouts("ec.vr.denoise", &[("noiseLevel", n(40.0))]);
    case("ec.vr.denoise", &[("noiseLevel", n(60.0)), ("detail", n(30.0))]);
    case("ec.vr.denoise", &[("noiseType", e(1)), ("noiseLevel", n(50.0))]);
    case("ec.vr.denoise", &[("noiseType", e(1)), ("noiseLevel", n(80.0)), ("frameLayout", e(1))]);
}

#[test]
fn rotate_and_planes() {
    layouts("ec.vr.rotatesphere", &[("tilt", n(20.0)), ("pan", n(35.0)), ("roll", n(-10.0))]);
    case("ec.vr.rotatesphere", &[("tilt", n(-40.0)), ("pan", n(170.0)), ("invertRotation", on())]);
    layouts("ec.vr.spheretoplane", &[("horizontalFov", n(100.0)), ("tilt", n(10.0)), ("pan", n(-30.0))]);
    case("ec.vr.planetosphere", &[("scale", n(80.0)), ("pan", n(20.0)), ("tilt", n(-15.0))]);
    case("ec.vr.planetosphere", &[("scale", n(120.0)), ("feather", n(30.0)), ("roll", n(25.0))]);
}

#[test]
fn converter() {
    for from in 0..9 {
        for to in [0u32, 1, 7] {
            if from == to {
                continue;
            }
            case_allow(
                "ec.vr.converter",
                &[("sourceProjection", e(from)), ("targetProjection", e(to)), ("sourceHorizontalFov", n(200.0)), ("pan", n(15.0))],
                0.005,
            );
        }
    }
    for to in [2u32, 3, 4, 5, 6, 8] {
        case_allow("ec.vr.converter", &[("targetProjection", e(to)), ("targetHorizontalFov", n(120.0)), ("tilt", n(12.0))], 0.005);
    }
    case_allow("ec.vr.converter", &[("targetProjection", e(1)), ("frameLayout", e(1))], 0.005);
    case_allow("ec.vr.converter", &[("targetProjection", e(6)), ("frameLayout", e(2)), ("roll", n(30.0))], 0.005);
}

#[test]
fn generated() {
    layouts("ec.vr.chromaticaberrations", &[("aberrationRed", n(40.0)), ("aberrationBlue", n(-30.0))]);
    case("ec.vr.chromaticaberrations", &[("centerTilt", n(30.0)), ("centerPan", n(-60.0)), ("falloff", n(10.0)), ("falloffInvert", on())]);
    layouts("ec.vr.colorgradients", &[]);
    case("ec.vr.colorgradients", &[("blend", n(5.0)), ("opacity", n(60.0)), ("blendingMode", e(5)), ("enablePoint5", on())]);
    for mode in 1..5 {
        case("ec.vr.colorgradients", &[("blendingMode", e(mode)), ("color2", c(0.2, 0.9, 0.4))]);
    }
    layouts("ec.vr.fractalnoise", &[]);
    for kind in 1..4 {
        case("ec.vr.fractalnoise", &[("fractalType", e(kind)), ("complexity", n(4.5)), ("evolution", n(70.0)), ("transform/pan", n(30.0))]);
    }
    case("ec.vr.fractalnoise", &[("blendingMode", e(4)), ("opacity", n(70.0)), ("invert", on()), ("transform/scale", n(250.0))]);
    layouts("ec.vr.digitalglitch", &[]);
    case(
        "ec.vr.digitalglitch",
        &[
            ("distortionEvolution", n(100.0)),
            ("geometric/verticalDisplacement", n(80.0)),
            ("target/radius", n(60.0)),
            ("target/feather", n(30.0)),
            ("target/pan", n(40.0)),
            ("scanlineSpacing", n(3.0)),
        ],
    );
}
