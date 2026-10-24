//! GPU shape and bevel effects (Circle, Ellipse, Iris Wipe, Bevel Alpha, Bevel Edges, Gaussian
//! Blur (Legacy)) vs the CPU effects (the oracle): direct on a buffer at full and half
//! resolution, as adjustment, and composited at 8 and 32 bpc.

use effectcraft_keyframe::Value;

use crate::tests::{c, effect_case, n};

fn e(v: u32) -> Value {
    Value::Enum(v)
}

fn on() -> Value {
    Value::Bool(true)
}

fn pt(x: f64, y: f64) -> Value {
    Value::Vec2([x, y])
}

#[test]
fn circle_and_ellipse() {
    effect_case("ec.generate.circle", &[("center", pt(30.0, 20.0)), ("radius", n(15.0))]);
    effect_case("ec.generate.circle", &[("radius", n(18.0)), ("edge", e(2)), ("thickness", n(5.0)), ("feather/feather", n(3.0)), ("blendingMode", e(3))]);
    effect_case(
        "ec.generate.circle",
        &[("radius", n(20.0)), ("edge", e(4)), ("thickness", n(30.0)), ("feather/featherInner", n(10.0)), ("invert", on()), ("opacity", n(60.0))],
    );
    effect_case("ec.generate.circle", &[("radius", n(12.0)), ("edge", e(1)), ("edgeRadius", n(25.0)), ("color", c(0.2, 0.8, 0.4))]);
    effect_case("ec.generate.ellipse", &[("center", pt(35.0, 22.0)), ("width", n(50.0)), ("height", n(30.0)), ("thickness", n(6.0))]);
    effect_case("ec.generate.ellipse", &[("width", n(40.0)), ("height", n(60.0)), ("softness", n(40.0)), ("compositeOnOriginal", on())]);
}

#[test]
fn iris_wipe() {
    effect_case("ec.transition.iriswipe", &[("outerRadius", n(20.0)), ("center", pt(35.0, 22.0))]);
    effect_case(
        "ec.transition.iriswipe",
        &[("outerRadius", n(25.0)), ("points", n(5.0)), ("useInnerRadius", on()), ("innerRadius", n(10.0)), ("rotation", n(20.0)), ("feather", n(4.0))],
    );
}

#[test]
fn bevels_and_legacy_gaussian() {
    effect_case("ec.perspective.bevelalpha", &[("edgeThickness", n(4.0)), ("lightIntensity", n(0.6))]);
    effect_case("ec.perspective.bevelalpha", &[("edgeThickness", n(2.0)), ("lightAngle", n(200.0)), ("lightColor", c(1.0, 0.8, 0.5))]);
    effect_case("ec.perspective.beveledges", &[("edgeThickness", n(0.1)), ("lightIntensity", n(0.5))]);
    effect_case("ec.perspective.beveledges", &[("edgeThickness", n(0.3)), ("lightAngle", n(130.0))]);
    // Sigma 3 px and up at half scale too: smaller ones render on the CPU (#1939).
    effect_case("ec.obsolete.gaussianlegacy", &[("blurriness", n(14.0))]);
    effect_case("ec.obsolete.gaussianlegacy", &[("blurriness", n(12.0)), ("blurDimensions", e(2))]);
}
