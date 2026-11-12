//! GPU CC light effects (CC Light Rays, CC Light Burst 2.5, CC Light Sweep, CC Light Wipe) vs
//! the CPU effects (the oracle): direct on a buffer at full and half resolution, as
//! adjustment, and composited at 8 and 32 bpc.

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
fn light_rays() {
    effect_case("ec.generate.cclightrays", &[("center", pt(30.0, 20.0))]);
    effect_case(
        "ec.generate.cclightrays",
        &[("center", pt(40.0, 18.0)), ("intensity", n(250.0)), ("radius", n(15.0)), ("shape", e(1)), ("transferMode", e(1))],
    );
    effect_case(
        "ec.generate.cclightrays",
        &[("intensity", n(80.0)), ("colorFromSource", Value::Bool(false)), ("color", c(1.0, 0.6, 0.2)), ("transferMode", e(2)), ("warpSoftness", n(120.0))],
    );
    effect_case("ec.generate.cclightrays", &[("intensity", n(150.0)), ("transferMode", e(3)), ("radius", n(25.0))]);
}

#[test]
fn light_burst() {
    effect_case("ec.generate.cclightburst", &[("center", pt(30.0, 20.0)), ("burst", e(1))]);
    effect_case("ec.generate.cclightburst", &[("rayLength", n(80.0)), ("burst", e(2)), ("intensity", n(150.0))]);
    effect_case("ec.generate.cclightburst", &[("rayLength", n(30.0)), ("setColor", on()), ("color", c(0.3, 0.7, 1.0)), ("burst", e(1))]);
    effect_case("ec.generate.cclightburst", &[("rayLength", n(40.0))]);
}

#[test]
fn light_sweep() {
    effect_case("ec.generate.cclightsweep", &[("center", pt(30.0, 20.0))]);
    effect_case("ec.generate.cclightsweep", &[("shape", e(0)), ("width", n(30.0)), ("edgeThickness", n(4.0)), ("lightReceptionMode", e(1))]);
    effect_case("ec.generate.cclightsweep", &[("shape", e(2)), ("direction", n(60.0)), ("lightReceptionMode", e(2)), ("lightColor", c(1.0, 0.8, 0.4))]);
    effect_case("ec.generate.cclightsweep", &[("edgeThickness", n(0.0)), ("sweepIntensity", n(120.0))]);
}

#[test]
fn light_wipe() {
    effect_case("ec.transition.cclightwipe", &[("completion", n(40.0)), ("center", pt(30.0, 20.0))]);
    effect_case("ec.transition.cclightwipe", &[("completion", n(25.0)), ("shape", e(0)), ("direction", n(30.0)), ("colorFromSource", on())]);
    effect_case("ec.transition.cclightwipe", &[("completion", n(60.0)), ("shape", e(2)), ("reverse", on()), ("color", c(0.2, 0.9, 0.5))]);
}
