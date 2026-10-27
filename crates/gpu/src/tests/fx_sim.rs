//! GPU simulation render passes vs the CPU effects (the oracle): the frame's plan (sprites,
//! pieces, blobs, drops, height grid) is shared, the raster / shading pass runs on each side.
//! Direct on a buffer (full and half resolution) and composited at 8 and 32 bpc, at a time
//! where the simulations have something to show.

use effectcraft_keyframe::Value;
use effectcraft_project::BitDepth;
use effectcraft_render::RenderOpts;
use effectcraft_time::Tick;

use crate::tests::{Scene, c, check, compare_at, diff, gpu, n, opts, pattern, set};

pub(crate) fn e(v: u32) -> Value {
    Value::Enum(v)
}

pub(crate) fn on() -> Value {
    Value::Bool(true)
}

pub(crate) fn off() -> Value {
    Value::Bool(false)
}

/// The effect alone on a buffer at layer time `t` (`allow` = fraction of pixels over 1e-3).
pub(crate) fn direct(id: &str, vals: &[(&str, Value)], t: f64, allow: f64) {
    let Some(g) = gpu() else { return };
    let spec = effectcraft_effects::find(id).unwrap();
    let size = [70.0, 44.0];
    let mut params =
        effectcraft_effects::Params { values: spec.params.iter().map(|ps| (ps.id.to_string(), effectcraft_effects::default_value(ps, size))).collect() };
    for (k, v) in vals {
        params.values.insert(k.to_string(), v.clone());
    }
    for scale in [1.0, 0.5] {
        let img = pattern(7, (size[0] * scale) as u32, (size[1] * scale) as u32);
        let buf = effectcraft_effects::Buf { img, offset: [0.0, 0.0], scale };
        let ctx = || effectcraft_effects::EffectCtx { params: &params, time: t, layer_size: size, seed: 11, adjustment: false, env: Default::default() };
        let cpu = (spec.render)(&ctx(), buf.clone());
        let out = effectcraft_render::Accelerator::effects(g, &[effectcraft_render::FxStep { spec, ctx: ctx() }], &buf, None).expect("the GPU runs the effect");
        assert_eq!((out.offset, out.scale), (cpu.offset, cpu.scale), "{id}: geometry");
        let d = diff(&cpu.img, &out.img, 1e-3);
        assert!(
            d.over as f64 <= allow * d.total as f64,
            "{id} {vals:?} scale {scale}: {} of {} pixels over 1e-3 (max {}); worst {:?}",
            d.over,
            d.total,
            d.max,
            d.worst
        );
    }
}

/// A footage layer with the effect over a background and a second footage layer (the target of
/// `Value::Layer(Some(0))` parameters), composited at 8 and 32 bpc at comp time `t`.
pub(crate) fn composited(id: &str, vals: &[(&str, Value)], t: f64, allow: f64) {
    for depth in [BitDepth::Bpc8, BitDepth::Bpc32] {
        let mut s = Scene::new(depth);
        let bg = s.solid([0.15, 0.1, 0.2], 97, 61);
        s.push(bg);
        let other = s.footage(60, 50);
        let other_id = s.push(other);
        let mut l = s.footage(70, 44);
        let vals: Vec<(&str, Value)> =
            vals.iter().map(|(k, v)| (*k, if *v == Value::Layer(Some(0)) { Value::Layer(Some(other_id.0)) } else { v.clone() })).collect();
        s.effect(&mut l, id, &vals);
        set(&mut l, "transform/rotation", Value::Scalar(8.0));
        s.push(l);
        let at = Tick::from_seconds_f64(t);
        check(&format!("{id} {vals:?} {depth:?}"), compare_at(&s, opts(), at), allow);
        check(&format!("{id} {vals:?} {depth:?} half"), compare_at(&s, RenderOpts { scale: 0.5, ..opts() }, at), allow);
    }
}

pub(crate) fn case(id: &str, vals: &[(&str, Value)], t: f64, allow: f64) {
    direct(id, vals, t, allow);
    composited(id, vals, t, allow);
}

#[test]
fn weather_and_star_burst() {
    case("ec.sim.ccrainfall", &[("drops", n(800.0)), ("size", n(3.0))], 0.5, 0.0);
    case("ec.sim.ccrainfall", &[("drops", n(500.0)), ("extras/appearance", e(1)), ("transferMode", e(0)), ("wind", n(800.0))], 0.7, 0.0);
    case("ec.sim.ccrainfall", &[("drops", n(300.0)), ("compositeWithOriginal", off()), ("size", n(6.0))], 0.3, 0.0);
    case("ec.sim.ccsnowfall", &[("flakes", n(600.0)), ("size", n(6.0))], 0.5, 0.0);
    case("ec.sim.ccsnowfall", &[("flakes", n(400.0)), ("transferMode", e(1)), ("wiggle/wiggleAmount", n(8.0))], 1.2, 0.0);
    case("ec.sim.ccstarburst", &[("gridSpacing", n(6.0))], 0.4, 0.0);
    case("ec.sim.ccstarburst", &[("gridSpacing", n(4.0)), ("blendWithOriginal", n(40.0)), ("size", n(160.0))], 0.9, 0.0);
}

#[test]
fn bubbles_drizzle_mercury() {
    case("ec.sim.ccbubbles", &[("bubbleAmount", n(60.0)), ("bubbleSize", n(3.0))], 0.5, 0.0);
    for shading in 1..5 {
        case(
            "ec.sim.ccbubbles",
            &[("bubbleAmount", n(40.0)), ("bubbleSize", n(4.0)), ("shadingType", e(shading)), ("reflectionType", e(shading % 2))],
            0.8,
            0.0,
        );
    }
    case("ec.sim.ccdrizzle", &[("dripRate", n(12.0)), ("spreading", n(300.0))], 0.6, 0.0);
    case("ec.sim.ccdrizzle", &[("dripRate", n(20.0)), ("shading/diffuse", n(40.0)), ("shading/ambient", n(-20.0)), ("light/lightHeight", n(60.0))], 1.1, 0.0);
    case("ec.sim.ccmrmercury", &[("birthRate", n(4.0)), ("blobBirthSize", n(1.5)), ("blobDeathSize", n(1.0))], 1.0, 0.0);
    case("ec.sim.ccmrmercury", &[("birthRate", n(3.0)), ("blobInfluence", n(60.0)), ("shading/metal", n(80.0)), ("gravity", n(2.0))], 1.5, 0.0);
}

#[test]
fn hair_wave_world_foam() {
    case("ec.sim.cchair", &[("density", n(300.0)), ("length", n(10.0))], 0.0, 0.0);
    case("ec.sim.cchair", &[("density", n(200.0)), ("length", n(14.0)), ("hairColor/colorInheritance", n(30.0)), ("thickness", n(1.5))], 0.0, 0.0);
    case("ec.sim.waveworld", &[], 0.5, 0.0);
    case("ec.sim.waveworld", &[("view", e(1))], 0.8, 0.0);
    composited("ec.sim.waveworld", &[("view", e(1)), ("ground/ground", Value::Layer(Some(0))), ("heightMapControls/renderDryAreasAs", e(1))], 0.6, 0.0);
    case("ec.sim.foam", &[], 1.0, 0.0);
    case("ec.sim.foam", &[("view", e(2))], 1.5, 0.0);
    case("ec.sim.foam", &[("view", e(2)), ("rendering/bubbleTexture", e(4)), ("rendering/blendMode", e(2))], 1.2, 0.0);
}

#[test]
fn pieces() {
    case("ec.sim.carddance", &[("rows", n(5.0)), ("columns", n(7.0)), ("zRotation/zRotMultiplier", n(20.0))], 0.0, 0.0);
    composited("ec.sim.carddance", &[("rows", n(4.0)), ("backLayer", Value::Layer(Some(0))), ("yRotation/yRotMultiplier", n(150.0))], 0.0, 0.0);
    case("ec.sim.shatter", &[("view", e(0))], 1.0, 0.0);
    case("ec.sim.shatter", &[("view", e(0)), ("shape/pattern", e(2)), ("physics/gravity", n(4.0))], 1.6, 0.0);
    case("ec.transition.cardwipe", &[("completion", n(40.0))], 0.0, 0.0);
    composited("ec.transition.cardwipe", &[("completion", n(60.0)), ("backLayer", Value::Layer(Some(0))), ("flipAxis", e(1))], 0.0, 0.0);
}

#[test]
fn caustics() {
    case("ec.sim.caustics", &[], 0.0, 0.0);
    composited(
        "ec.sim.caustics",
        &[("water/waterSurface", Value::Layer(Some(0))), ("water/waveHeight", n(0.3)), ("water/smoothing", n(4.0)), ("lighting/lightType", e(1))],
        0.0,
        0.0,
    );
    composited(
        "ec.sim.caustics",
        &[
            ("water/waterSurface", Value::Layer(Some(0))),
            ("sky/sky", Value::Layer(Some(0))),
            ("water/surfaceOpacity", n(0.6)),
            ("bottom/repeatMode", e(2)),
            ("bottom/scaling", n(1.4)),
            ("bottom/blur", n(3.0)),
            ("sky/repeatMode", e(1)),
        ],
        0.0,
        0.0,
    );
    let _ = (c(0.0, 0.0, 0.0), on());
}

#[test]
fn shatter_wireframes_and_foam_extras() {
    // Shatter's wireframe views (Wireframe Front View is the default) draw lines as sprites.
    for view in 1..5 {
        case("ec.sim.shatter", &[("view", e(view))], 1.0, 0.0);
    }
    case("ec.sim.shatter", &[("view", e(4)), ("force2/force2Radius", n(0.3)), ("shape/pattern", e(3))], 1.5, 0.0);
    // Foam's User Defined texture, Environment Map and flow-map preview.
    composited("ec.sim.foam", &[("view", e(2)), ("rendering/bubbleTexture", e(5)), ("rendering/bubbleTextureLayer", Value::Layer(Some(0)))], 1.0, 0.0);
    composited(
        "ec.sim.foam",
        &[("view", e(2)), ("rendering/bubbleTexture", e(5)), ("rendering/bubbleTextureLayer", Value::Layer(Some(0))), ("rendering/bubbleOrientation", e(2))],
        1.4,
        0.0,
    );
    composited("ec.sim.foam", &[("view", e(2)), ("rendering/environmentMap", Value::Layer(Some(0))), ("rendering/reflectionStrength", n(0.5))], 1.2, 0.0);
    composited(
        "ec.sim.foam",
        &[
            ("view", e(2)),
            ("rendering/environmentMap", Value::Layer(Some(0))),
            ("rendering/reflectionStrength", n(0.8)),
            ("rendering/reflectionConvergence", n(1.0)),
        ],
        1.2,
        0.0,
    );
    composited("ec.sim.foam", &[("view", e(1)), ("flowMap/flowMap", Value::Layer(Some(0))), ("flowMap/flowMapSteepness", n(0.5))], 1.0, 0.0);
}
