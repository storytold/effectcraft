//! GPU transition and perspective effects (Block Dissolve, the CC transitions, Radial Shadow,
//! CC Bender, CC Blobbylize, CC Cylinder, CC Sphere, CC Spotlight, CC Environment, 3D Glasses)
//! vs the CPU effects (the oracle): direct on a buffer at full and half resolution, as
//! adjustment, and composited at 8 and 32 bpc; effects reading other layers also with a layer
//! chosen.

use effectcraft_keyframe::Value;
use effectcraft_project::BitDepth;
use effectcraft_render::RenderOpts;
use effectcraft_time::Tick;

use crate::tests::{Scene, c, check, compare_at, effect_case, n, opts, set};

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
fn block_dissolve() {
    effect_case("ec.transition.blockdissolve", &[("completion", n(40.0))]);
    effect_case("ec.transition.blockdissolve", &[("completion", n(55.0)), ("blockWidth", n(7.0)), ("blockHeight", n(5.5)), ("feather", n(3.0))]);
    effect_case("ec.transition.blockdissolve", &[("completion", n(30.0)), ("blockWidth", n(4.3)), ("blockHeight", n(9.0)), ("softEdges", Value::Bool(false))]);
}

#[test]
fn grid_and_scale_wipes() {
    effect_case("ec.transition.ccgridwipe", &[("completion", n(40.0)), ("center", pt(33.0, 21.0))]);
    effect_case("ec.transition.ccgridwipe", &[("completion", n(60.0)), ("rotation", n(20.0)), ("border", n(3.0)), ("tiles", n(7.0)), ("shape", e(0))]);
    effect_case("ec.transition.ccgridwipe", &[("completion", n(35.0)), ("shape", e(2)), ("reverse", on()), ("tiles", n(5.0))]);
    effect_case("ec.transition.ccradialscalewipe", &[("completion", n(40.0)), ("center", pt(30.0, 20.0))]);
    effect_case("ec.transition.ccradialscalewipe", &[("completion", n(30.0)), ("center", pt(40.0, 25.0)), ("reverse", on())]);
    effect_case("ec.transition.ccscalewipe", &[("stretch", n(2.0)), ("center", pt(30.0, 20.0))]);
    effect_case("ec.transition.ccscalewipe", &[("stretch", n(0.7)), ("direction", n(200.0))]);
}

#[test]
fn gradient_driven_wipes() {
    effect_case("ec.transition.ccglasswipe", &[("completion", n(45.0))]);
    effect_case("ec.transition.ccglasswipe", &[("completion", n(30.0)), ("softness", n(40.0)), ("displacementAmount", n(25.0))]);
    effect_case("ec.transition.ccimagewipe", &[("completion", n(50.0))]);
    effect_case(
        "ec.transition.ccimagewipe",
        &[("completion", n(40.0)), ("property", e(5)), ("blur", n(4.0)), ("inverseGradient", on()), ("borderSoftness", n(20.0))],
    );
    effect_case("ec.transition.ccimagewipe", &[("completion", n(60.0)), ("property", e(6)), ("autoSoftness", Value::Bool(false))]);
    effect_case("ec.transition.ccwarpomatic", &[("completion", n(40.0))]);
    effect_case("ec.transition.ccwarpomatic", &[("completion", n(55.0)), ("reactor", e(1)), ("warpDirection", e(1))]);
    effect_case("ec.transition.ccwarpomatic", &[("completion", n(35.0)), ("reactor", e(2)), ("warpDirection", e(2)), ("blendSpan", n(30.0))]);
}

#[test]
fn geometric_transitions() {
    effect_case("ec.transition.ccjaws", &[("completion", n(30.0)), ("center", pt(33.0, 21.0))]);
    for shape in 1..4 {
        effect_case("ec.transition.ccjaws", &[("completion", n(20.0)), ("shape", e(shape)), ("direction", n(25.0)), ("width", n(13.0))]);
    }
    effect_case("ec.transition.cclinesweep", &[("completion", n(40.0))]);
    effect_case(
        "ec.transition.cclinesweep",
        &[("completion", n(50.0)), ("direction", n(35.0)), ("slant", n(-12.0)), ("thickness", n(7.0)), ("flipDirection", on())],
    );
    effect_case("ec.transition.cctwister", &[("completion", n(30.0)), ("center", pt(33.0, 21.0))]);
    effect_case("ec.transition.cctwister", &[("completion", n(60.0)), ("axis", n(10.0)), ("shading", Value::Bool(false))]);
}

#[test]
fn perspective() {
    effect_case("ec.perspective.radialshadow", &[("lightSource", pt(20.0, 10.0)), ("projectionDistance", n(15.0))]);
    effect_case(
        "ec.perspective.radialshadow",
        &[
            ("lightSource", pt(50.0, 15.0)),
            ("softness", n(6.0)),
            ("render", e(1)),
            ("colorInfluence", n(60.0)),
            ("resizeLayer", on()),
            ("color", c(0.3, 0.0, 0.5)),
        ],
    );
    effect_case("ec.perspective.radialshadow", &[("shadowOnly", on()), ("opacity", n(80.0)), ("resizeLayer", on())]);
    effect_case("ec.perspective.cccylinder", &[("position", pt(35.0, 22.0)), ("rotation", n(30.0))]);
    effect_case("ec.perspective.cccylinder", &[("radius", n(60.0)), ("render", e(1)), ("lightDirection", n(70.0)), ("roughness", n(0.2))]);
    effect_case("ec.perspective.cccylinder", &[("render", e(2)), ("lightHeight", n(20.0)), ("specular", n(60.0))]);
    // All three axes: independently and combined, against the CPU reference.
    effect_case("ec.perspective.cccylinder", &[("rotationX", n(35.0)), ("render", e(1))]);
    effect_case("ec.perspective.cccylinder", &[("rotationY", n(65.0)), ("rotation", n(15.0))]);
    effect_case("ec.perspective.cccylinder", &[("rotationZ", n(-25.0)), ("render", e(2))]);
    effect_case("ec.perspective.cccylinder", &[("rotationX", n(30.0)), ("rotationY", n(20.0)), ("rotationZ", n(15.0))]);
    effect_case("ec.perspective.ccsphere", &[("offset", pt(35.0, 22.0)), ("radius", n(18.0)), ("rotationX", n(20.0)), ("rotationY", n(35.0))]);
    effect_case("ec.perspective.ccsphere", &[("radius", n(20.0)), ("rotationZ", n(50.0)), ("render", e(2))]);
    effect_case("ec.perspective.ccspotlight", &[("from", pt(15.0, 8.0)), ("to", pt(35.0, 22.0))]);
    effect_case(
        "ec.perspective.ccspotlight",
        &[("from", pt(50.0, 5.0)), ("to", pt(30.0, 30.0)), ("coneAngle", n(35.0)), ("edgeSoftness", n(60.0)), ("render", e(1))],
    );
    effect_case("ec.perspective.ccspotlight", &[("to", pt(35.0, 22.0)), ("edgeSoftness", n(0.0)), ("color", c(1.0, 0.7, 0.3)), ("intensity", n(150.0))]);
    for mapping in 0..3 {
        effect_case("ec.perspective.ccenvironment", &[("mapping", e(mapping)), ("height", n(40.0))]);
    }
    effect_case("ec.perspective.ccenvironment", &[("filterEnvironment", Value::Bool(false))]);
    for view in 0..9 {
        effect_case("ec.perspective.3dglasses", &[("view3d", e(view)), ("sceneConvergence", n(4.0)), ("verticalAlignment", n(1.5))]);
    }
    effect_case("ec.perspective.3dglasses", &[("units", e(1)), ("sceneConvergence", n(3.0)), ("leftRightSwap", on()), ("balance", n(12.0))]);
}

#[test]
fn distort() {
    effect_case("ec.distort.ccbender", &[("amount", n(40.0)), ("top", pt(35.0, 0.0)), ("base", pt(35.0, 44.0))]);
    for style in 1..4 {
        effect_case(
            "ec.distort.ccbender",
            &[("amount", n(-30.0)), ("style", e(style)), ("top", pt(20.0, 5.0)), ("base", pt(40.0, 40.0)), ("adjustToDistance", on())],
        );
    }
    effect_case("ec.distort.ccblobbylize", &[]);
    effect_case(
        "ec.distort.ccblobbylize",
        &[("property", e(4)), ("softness", n(3.0)), ("cutAway", n(30.0)), ("lightType", e(1)), ("lightPosition", pt(20.0, 10.0))],
    );
    effect_case("ec.distort.ccblobbylize", &[("property", e(5)), ("metal", n(30.0)), ("roughness", n(0.2))]);
}

/// Effects reading another layer (gradient, reveal, backside, environment, stereo views).
#[test]
fn with_other_layers() {
    for depth in [BitDepth::Bpc8, BitDepth::Bpc32] {
        let cases: Vec<(&str, Vec<(&str, Value)>)> = vec![
            ("ec.transition.ccglasswipe", vec![("completion", n(40.0)), ("gradientLayer", Value::Layer(None)), ("layerToReveal", Value::Layer(None))]),
            ("ec.transition.ccimagewipe", vec![("completion", n(45.0)), ("layer", Value::Layer(None))]),
            ("ec.transition.ccwarpomatic", vec![("completion", n(40.0)), ("reactorLayer", Value::Layer(None)), ("layerToReveal", Value::Layer(None))]),
            ("ec.transition.cctwister", vec![("completion", n(70.0)), ("backside", Value::Layer(None))]),
            ("ec.perspective.ccenvironment", vec![("environment", Value::Layer(None))]),
            ("ec.perspective.3dglasses", vec![("view3d", e(5)), ("leftView", Value::Layer(None)), ("sceneConvergence", n(3.0))]),
            ("ec.distort.ccblobbylize", vec![("blobLayer", Value::Layer(None))]),
        ];
        for (id, vals) in cases {
            let mut s = Scene::new(depth);
            let bg = s.solid([0.15, 0.1, 0.2], 97, 61);
            s.push(bg);
            let mut other = s.footage(50, 36);
            other.switches.video = false;
            let oid = s.push(other);
            let vals: Vec<(&str, Value)> = vals.into_iter().map(|(k, v)| (k, if v == Value::Layer(None) { Value::Layer(Some(oid.0)) } else { v })).collect();
            let mut l = s.footage(70, 44);
            s.effect(&mut l, id, &vals);
            set(&mut l, "transform/rotation", Value::Scalar(8.0));
            s.push(l);
            let label = format!("{id} with a layer {depth:?}");
            check(&label, compare_at(&s, opts(), Tick::ZERO), 0.0);
            check(&format!("{label} half"), compare_at(&s, RenderOpts { scale: 0.5, ..opts() }, Tick::ZERO), 0.0);
        }
    }
}
