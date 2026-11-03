//! GPU vs CPU for the blur, transition and generate family (`fx_generate`).

use effectcraft_keyframe::Value;
use effectcraft_project::BitDepth;
use effectcraft_render::RenderOpts;
use effectcraft_time::Tick;

use crate::tests::{Scene, c, check, compare_at, effect_case, n, opts, set};

fn e(v: u32) -> Value {
    Value::Enum(v)
}

fn b(v: bool) -> Value {
    Value::Bool(v)
}

fn pt(x: f64, y: f64) -> Value {
    Value::Vec2([x, y])
}

#[test]
fn radial_blur() {
    effect_case("ec.blur.radial", &[("amount", n(12.0))]);
    effect_case("ec.blur.radial", &[("amount", n(30.0)), ("type", e(1)), ("center", pt(20.3, 14.7))]);
    effect_case("ec.blur.radial", &[("amount", n(-45.0)), ("center", pt(50.0, 30.0))]);
}

#[test]
fn cc_radial_fast_blur() {
    effect_case("ec.blur.ccradialfast", &[("amount", n(40.0))]);
    effect_case("ec.blur.ccradialfast", &[("amount", n(70.0)), ("zoom", e(1)), ("center", pt(22.4, 16.1))]);
    effect_case("ec.blur.ccradialfast", &[("amount", n(25.0)), ("zoom", e(2))]);
    // Center outside the layer: the buffer grows so the streaks pass its edge (#2095).
    effect_case("ec.blur.ccradialfast", &[("amount", n(80.0)), ("center", pt(-30.0, 20.0))]);
}

#[test]
fn camera_lens_blur() {
    effect_case("ec.blur.cameralens", &[("blurRadius", n(5.0))]);
    effect_case(
        "ec.blur.cameralens",
        &[
            ("blurRadius", n(7.5)),
            ("irisProperties/irisShape", e(2)),
            ("irisProperties/irisRotation", n(20.0)),
            ("irisProperties/irisAspectRatio", n(1.6)),
            ("repeatEdge", b(true)),
        ],
    );
    effect_case(
        "ec.blur.cameralens",
        &[
            ("blurRadius", n(4.0)),
            ("irisProperties/irisShape", e(8)),
            ("highlight/specularBrightness", n(60.0)),
            ("highlight/specularThreshold", n(150.0)),
            ("highlight/highlightSaturation", n(40.0)),
            ("useLinear", b(true)),
        ],
    );
    // Large iris: per-row prefix sums.
    effect_case("ec.blur.cameralens", &[("blurRadius", n(14.0)), ("irisProperties/irisRoundness", n(60.0))]);
    effect_case("ec.blur.cameralens", &[("blurRadius", n(12.0)), ("repeatEdge", b(true)), ("useLinear", b(true))]);
}

#[test]
fn wipes() {
    effect_case("ec.transition.linearwipe", &[("completion", n(40.0)), ("angle", n(30.0)), ("feather", n(12.0))]);
    effect_case("ec.transition.linearwipe", &[("completion", n(55.0)), ("angle", n(-120.0))]);
    effect_case("ec.transition.linearwipe", &[("completion", n(20.0)), ("angle", n(90.0)), ("feather", n(3.0))]);
    effect_case("ec.transition.venetian", &[("completion", n(35.0)), ("direction", n(30.0)), ("width", n(11.0)), ("feather", n(2.0))]);
    effect_case("ec.transition.venetian", &[("completion", n(60.0)), ("width", n(17.0))]);
    effect_case("ec.transition.venetian", &[("completion", n(25.0)), ("direction", n(-70.0)), ("width", n(9.0)), ("feather", n(0.5))]);
    effect_case("ec.transition.radialwipe", &[("completion", n(30.0)), ("startAngle", n(25.0)), ("feather", n(10.0))]);
    effect_case("ec.transition.radialwipe", &[("completion", n(45.0)), ("wipe", e(1)), ("center", pt(30.2, 19.6))]);
    effect_case("ec.transition.radialwipe", &[("completion", n(60.0)), ("wipe", e(2)), ("startAngle", n(-40.0)), ("feather", n(3.0))]);
    effect_case("ec.transition.radialwipe", &[("completion", n(0.0)), ("feather", n(20.0))]);
    effect_case("ec.transition.gradientwipe", &[("completion", n(40.0))]);
    effect_case("ec.transition.gradientwipe", &[("completion", n(55.0)), ("softness", n(30.0)), ("invert", b(true))]);
    effect_case("ec.transition.gradientwipe", &[("completion", n(100.0))]);
}

/// Gradient Wipe driven by another layer (fetched through the effect host, placed on the CPU).
#[test]
fn gradient_wipe_with_a_gradient_layer() {
    for depth in [BitDepth::Bpc8, BitDepth::Bpc32] {
        for placement in 0..3 {
            let mut s = Scene::new(depth);
            let bg = s.solid([0.15, 0.1, 0.2], 97, 61);
            s.push(bg);
            let grad = s.footage(40, 30);
            let gid = s.push(grad);
            let mut l = s.footage(70, 44);
            s.effect(
                &mut l,
                "ec.transition.gradientwipe",
                &[("completion", n(45.0)), ("softness", n(20.0)), ("gradientLayer", Value::Layer(Some(gid.0))), ("gradientPlacement", e(placement))],
            );
            set(&mut l, "transform/rotation", Value::Scalar(8.0));
            s.push(l);
            let label = format!("gradient wipe layer {placement} {depth:?}");
            check(&label, compare_at(&s, opts(), Tick::ZERO), 0.0);
            check(&format!("{label} half"), compare_at(&s, RenderOpts { scale: 0.5, ..opts() }, Tick::ZERO), 0.0);
        }
    }
}

#[test]
fn generators() {
    effect_case("ec.generate.cellpattern", &[("size", n(14.0))]);
    effect_case("ec.generate.cellpattern", &[("cellPattern", e(2)), ("size", n(11.0)), ("evolution", n(130.0)), ("overflow", e(1)), ("contrast", n(150.0))]);
    effect_case(
        "ec.generate.cellpattern",
        &[
            ("cellPattern", e(11)),
            ("size", n(9.0)),
            ("tilingOptions/enableTiling", b(true)),
            ("tilingOptions/cellsHorizontal", n(4.0)),
            ("tilingOptions/cellsVertical", n(3.0)),
            ("overflow", e(2)),
            ("contrast", n(250.0)),
            ("invert", b(true)),
            ("evolutionOptions/randomSeed", n(17.0)),
        ],
    );
    effect_case("ec.generate.cellpattern", &[("cellPattern", e(5)), ("size", n(16.0)), ("disperse", n(0.6)), ("offset", pt(3.3, -7.1))]);
    effect_case("ec.generate.checkerboard", &[("width", n(9.0))]);
    effect_case(
        "ec.generate.checkerboard",
        &[
            ("sizeFrom", e(2)),
            ("width", n(13.0)),
            ("height", n(7.0)),
            ("feather/featherWidth", n(4.0)),
            ("anchor", pt(5.3, 2.2)),
            ("blendingMode", e(5)),
            ("opacity", n(70.0)),
        ],
    );
    effect_case("ec.generate.checkerboard", &[("sizeFrom", e(0)), ("corner", pt(16.0, 11.0)), ("color", c(0.9, 0.3, 0.1)), ("blendingMode", e(1))]);
    effect_case("ec.generate.grid", &[("width", n(12.0)), ("border", n(3.0))]);
    effect_case(
        "ec.generate.grid",
        &[
            ("sizeFrom", e(2)),
            ("width", n(15.0)),
            ("height", n(9.0)),
            ("border", n(2.5)),
            ("feather/featherWidth", n(3.0)),
            ("feather/featherHeight", n(2.0)),
            ("invertGrid", b(true)),
            ("blendingMode", e(4)),
        ],
    );
    effect_case(
        "ec.generate.grid",
        &[("sizeFrom", e(0)), ("corner", pt(11.0, 8.0)), ("anchor", pt(2.5, 3.5)), ("blendingMode", e(14)), ("color", c(0.2, 0.8, 0.4))],
    );
    effect_case("ec.generate.fourcolor", &[]);
    effect_case("ec.generate.fourcolor", &[("blend", n(40.0)), ("jitter", n(50.0)), ("opacity", n(80.0)), ("blendingMode", e(3))]);
    effect_case("ec.generate.fourcolor", &[("blend", n(600.0)), ("blendingMode", e(16)), ("positionsColors/point2", pt(30.0, 20.0))]);
}

#[test]
fn noise_and_grain() {
    effect_case("ec.noise.noise", &[("amount", n(30.0))]);
    effect_case("ec.noise.noise", &[("amount", n(50.0)), ("color", b(false))]);
    effect_case("ec.noise.noise", &[("amount", n(40.0)), ("clip", b(false))]);
    effect_case("ec.noise.addgrain", &[("tweaking/intensity", n(2.0))]);
    effect_case(
        "ec.noise.addgrain",
        &[
            ("tweaking/intensity", n(3.0)),
            ("tweaking/size", n(2.5)),
            ("tweaking/softness", n(1.5)),
            ("tweaking/aspectRatio", n(1.4)),
            ("tweaking/channelSize/redSize", n(0.6)),
            ("color/saturation", n(0.5)),
            ("color/tintAmount", n(0.4)),
            ("color/tintColor", c(1.0, 0.6, 0.2)),
            ("application/blendingMode", e(4)),
        ],
    );
    effect_case(
        "ec.noise.addgrain",
        &[
            ("tweaking/intensity", n(1.5)),
            ("color/monochromatic", b(true)),
            ("application/blendingMode", e(3)),
            ("application/shadows", n(0.3)),
            ("application/highlights", n(1.8)),
            ("application/midpoint", n(0.35)),
            ("animation/randomSeed", n(42.0)),
            ("animation/animationSpeed", n(2.0)),
        ],
    );
    effect_case(
        "ec.noise.addgrain",
        &[("tweaking/intensity", n(2.0)), ("application/blendingMode", e(1)), ("tweaking/channelIntensities/blueIntensity", n(2.5))],
    );
}
