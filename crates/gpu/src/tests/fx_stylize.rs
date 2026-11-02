//! GPU stylize and distort family effects vs the CPU effects (the oracle): direct on a buffer at full and half
//! resolution, as adjustment, and composited at 8 and 32 bpc.

use effectcraft_keyframe::{Keyframe, Value};
use effectcraft_project::BitDepth;
use effectcraft_render::RenderOpts;
use effectcraft_time::Tick;

use crate::tests::{Scene, allow_llvmpipe_boundary_flips, c, check, compare_at, effect_case, n, opts, set};

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
fn posterize_thresholds_strobe_vignette() {
    let _flips = allow_llvmpipe_boundary_flips();
    effect_case("ec.stylize.posterize", &[("level", n(4.0))]);
    effect_case("ec.stylize.posterize", &[("level", n(9.0))]);
    effect_case("ec.stylize.threshold", &[("level", n(100.0))]);
    effect_case("ec.stylize.threshold", &[("level", n(181.0))]);
    effect_case("ec.stylize.ccthreshold", &[]);
    effect_case("ec.stylize.ccthreshold", &[("channel", e(1)), ("invert", on()), ("threshold", n(90.0))]);
    effect_case("ec.stylize.ccthreshold", &[("channel", e(2)), ("blend", n(30.0))]);
    effect_case("ec.stylize.ccthreshold", &[("channel", e(3)), ("threshold", n(60.0))]);
    effect_case(
        "ec.stylize.ccthresholdrgb",
        &[("redThreshold", n(70.0)), ("greenThreshold", n(150.0)), ("blueThreshold", n(110.0)), ("invertGreen", on()), ("blend", n(25.0))],
    );
    effect_case(
        "ec.stylize.strobe",
        &[("strobeDuration", n(0.5)), ("strobeOperator", e(1)), ("strobeColor", c(0.2, 0.5, 0.9)), ("blendWithOriginal", n(30.0))],
    );
    effect_case("ec.stylize.strobe", &[("strobeDuration", n(0.5)), ("strobeOperator", e(4)), ("strobeColor", c(0.6, 0.1, 0.3))]);
    effect_case("ec.stylize.strobe", &[("strobeDuration", n(0.5)), ("strobe", e(1)), ("blendWithOriginal", n(40.0))]);
    effect_case("ec.stylize.strobe", &[("strobeDuration", n(0.1)), ("strobePeriod", n(0.2)), ("randomStrobeProbability", n(50.0))]);
    effect_case("ec.stylize.ccvignette", &[("amount", n(80.0))]);
    effect_case("ec.stylize.ccvignette", &[("amount", n(-60.0)), ("pinHighlights", n(50.0)), ("angleOfView", n(120.0)), ("center", pt(20.0, 15.0))]);
}

#[test]
fn scatter_brush_strokes() {
    effect_case("ec.stylize.scatter", &[("amount", n(5.0))]);
    effect_case("ec.stylize.scatter", &[("amount", n(3.0)), ("grain", e(1))]);
    effect_case("ec.stylize.scatter", &[("amount", n(7.0)), ("grain", e(2)), ("randomizeEveryFrame", on())]);
    effect_case("ec.stylize.brushstrokes", &[]);
    effect_case(
        "ec.stylize.brushstrokes",
        &[("strokeAngle", n(40.0)), ("brushSize", n(3.0)), ("strokeLength", n(10.0)), ("paintSurface", e(2)), ("blendWithOriginal", n(20.0))],
    );
    effect_case("ec.stylize.brushstrokes", &[("paintSurface", e(1)), ("strokeDensity", n(0.6)), ("strokeRandomness", n(2.0)), ("brushSize", n(1.3))]);
    effect_case("ec.stylize.brushstrokes", &[("paintSurface", e(3)), ("strokeLength", n(25.0))]);
}

#[test]
fn roughen_edges() {
    effect_case("ec.stylize.roughenedges", &[]);
    effect_case("ec.stylize.roughenedges", &[("edgeType", e(1)), ("border", n(4.0)), ("edgeColor", c(0.1, 0.6, 0.9))]);
    effect_case(
        "ec.stylize.roughenedges",
        &[
            ("edgeType", e(5)),
            ("evolutionOptions/cycleEvolution", on()),
            ("evolutionOptions/cycle", n(2.0)),
            ("evolution", n(500.0)),
            ("stretch", n(2.0)),
            ("complexity", n(3.0)),
        ],
    );
    effect_case("ec.stylize.roughenedges", &[("edgeType", e(7)), ("border", n(0.0)), ("scale", n(60.0)), ("evolutionOptions/randomSeed", n(9.0))]);
}

#[test]
fn tiles_kaleida_repetile() {
    effect_case("ec.stylize.motiontile", &[("tileWidth", n(50.0)), ("tileHeight", n(40.0))]);
    effect_case(
        "ec.stylize.motiontile",
        &[("outputWidth", n(150.0)), ("outputHeight", n(130.0)), ("mirrorEdges", on()), ("phase", n(70.0)), ("tileWidth", n(45.0))],
    );
    effect_case(
        "ec.stylize.motiontile",
        &[("horizontalPhaseShift", on()), ("phase", n(33.0)), ("tileCenter", pt(30.0, 17.0)), ("tileHeight", n(55.0)), ("outputWidth", n(80.0))],
    );
    // Output under 100 % on an adjustment crops the comp below (#1727).
    effect_case("ec.stylize.motiontile", &[("outputWidth", n(60.0)), ("outputHeight", n(300.0)), ("mirrorEdges", on())]);
    effect_case("ec.stylize.cckaleida", &[]);
    effect_case("ec.stylize.cckaleida", &[("mirroring", e(2)), ("rotation", n(20.0)), ("size", n(150.0))]);
    effect_case("ec.stylize.ccrepetile", &[("expandRight", n(10.0)), ("expandUp", n(4.0))]);
    effect_case("ec.stylize.ccrepetile", &[("expandLeft", n(12.0)), ("expandDown", n(9.0)), ("tiling", e(1))]);
    effect_case("ec.stylize.ccrepetile", &[("expandLeft", n(30.0)), ("expandRight", n(30.0)), ("expandUp", n(20.0)), ("tiling", e(4))]);
    effect_case("ec.distort.cctiler", &[]);
    effect_case("ec.distort.cctiler", &[("scale", n(70.0)), ("blendWithOriginal", n(30.0)), ("center", pt(30.0, 18.0))]);
    effect_case("ec.distort.ccgriddler", &[("rotation", n(20.0))]);
    effect_case("ec.distort.ccgriddler", &[("cutTiles", on()), ("horizontalScale", n(120.0)), ("verticalScale", n(80.0)), ("tileSize", n(17.0))]);
}

#[test]
fn mirror_offset_polar_spherize_corner_pin() {
    effect_case("ec.distort.mirror", &[("angle", n(30.0))]);
    effect_case("ec.distort.mirror", &[("angle", n(200.0)), ("center", pt(27.0, 19.0))]);
    effect_case("ec.distort.offset", &[("shift", pt(10.0, 5.0))]);
    effect_case("ec.distort.offset", &[("shift", pt(50.3, 20.7)), ("blend", n(40.0))]);
    effect_case("ec.distort.polar", &[("interpolation", n(100.0))]);
    effect_case("ec.distort.polar", &[("interpolation", n(60.0)), ("conversion", e(1))]);
    effect_case("ec.distort.spherize", &[("radius", n(20.0))]);
    effect_case("ec.distort.spherize", &[("radius", n(35.0)), ("center", pt(30.0, 25.0))]);
    effect_case("ec.distort.cornerpin", &[("ul", pt(5.0, 3.0)), ("ur", pt(60.0, 8.0)), ("lr", pt(66.0, 40.0)), ("ll", pt(2.0, 44.0))]);
    effect_case("ec.distort.cornerpin", &[("ul", pt(-8.0, 6.0)), ("ur", pt(75.0, -4.0)), ("lr", pt(50.0, 30.0)), ("ll", pt(12.0, 50.0))]);
}

#[test]
fn optics_magnify() {
    effect_case("ec.distort.opticscompensation", &[("fieldOfView", n(60.0))]);
    effect_case("ec.distort.opticscompensation", &[("fieldOfView", n(90.0)), ("reverseLensDistortion", on()), ("fovOrientation", e(2))]);
    effect_case(
        "ec.distort.opticscompensation",
        &[("fieldOfView", n(110.0)), ("reverseLensDistortion", on()), ("resize", e(1)), ("viewCenter", pt(30.0, 20.0))],
    );
    effect_case("ec.distort.opticscompensation", &[("fieldOfView", n(120.0)), ("optimalPixels", on()), ("fovOrientation", e(1))]);
    effect_case("ec.distort.magnify", &[("size", n(15.0))]);
    effect_case(
        "ec.distort.magnify",
        &[("shape", e(1)), ("feather", n(6.0)), ("size", n(18.0)), ("scaling", e(1)), ("blendingMode", e(3)), ("magnification", n(270.0))],
    );
    effect_case("ec.distort.magnify", &[("scaling", e(2)), ("blendingMode", e(0)), ("size", n(20.0)), ("opacity", n(70.0))]);
    effect_case("ec.distort.magnify", &[("size", n(25.0)), ("center", pt(60.0, 10.0)), ("resizeLayer", on()), ("blendingMode", e(5))]);
}

#[test]
fn cc_slant_smear_split() {
    effect_case("ec.distort.ccslant", &[("slant", n(30.0))]);
    effect_case("ec.distort.ccslant", &[("slant", n(-20.0)), ("stretching", on()), ("height", n(120.0))]);
    effect_case("ec.distort.ccsmear", &[]);
    effect_case("ec.distort.ccsmear", &[("reach", n(-40.0)), ("radius", n(20.0)), ("to", pt(40.0, 10.0))]);
    effect_case("ec.distort.ccsplit", &[]);
    effect_case("ec.distort.ccsplit2", &[("split1", n(30.0)), ("split2", n(80.0)), ("pointB", pt(60.0, 30.0))]);
}

#[test]
fn twirl_legacy_ripple_pulse_power_pin() {
    effect_case("ec.distort.twirllegacy", &[("angle", n(90.0))]);
    effect_case("ec.distort.twirllegacy", &[("angle", n(-250.0)), ("radius", n(80.0)), ("center", pt(30.0, 18.0))]);
    effect_case("ec.distort.ccripplepulse", &[("pulseLevel", n(80.0)), ("timeSpan", n(0.5))]);
    effect_case("ec.distort.ccripplepulse", &[("pulseLevel", n(-60.0)), ("amplitude", n(150.0)), ("renderBump", on()), ("center", pt(25.0, 20.0))]);
    effect_case("ec.distort.ccpowerpin", &[("topLeft", pt(5.0, 3.0)), ("bottomRight", pt(62.0, 38.0))]);
    effect_case("ec.distort.ccpowerpin", &[("topRight", pt(80.0, -6.0)), ("bottomLeft", pt(-4.0, 50.0)), ("expandLeft", n(20.0)), ("expandBottom", n(-10.0))]);
    // Perspective below 100 %: blended toward the bilinear inverse.
    effect_case("ec.distort.ccpowerpin", &[("topRight", pt(80.0, -6.0)), ("bottomLeft", pt(-4.0, 50.0)), ("perspective", n(0.0))]);
    effect_case("ec.distort.ccpowerpin", &[("topLeft", pt(12.0, 6.0)), ("bottomRight", pt(75.0, 30.0)), ("perspective", n(40.0)), ("expandTop", n(15.0))]);
}

#[test]
fn flo_motion() {
    effect_case("ec.distort.ccflomotion", &[]);
    effect_case("ec.distort.ccflomotion", &[("antialiasing", e(2)), ("tileEdges", on()), ("amount1", n(-150.0)), ("falloff", n(0.7))]);
    effect_case("ec.distort.ccflomotion", &[("antialiasing", e(0)), ("amount2", n(0.0)), ("knot1", pt(20.0, 30.0))]);
}

#[test]
fn liquify_render() {
    let mesh = "warp 20 50 0 0 0 10,10 20,15 30,20\ntwirlClockwise 24 80 0 0 0 40,25 42,27";
    effect_case("ec.distort.liquify", &[("distortionMesh", Value::Str(mesh.into()))]);
    effect_case(
        "ec.distort.liquify",
        &[("distortionMesh", Value::Str(mesh.into())), ("distortionPercentage", n(60.0)), ("distortionMeshOffset", pt(3.0, -2.0))],
    );
}

#[test]
fn glow_operations_and_arbitrary_map() {
    effect_case("ec.stylize.glow", &[("glowOperation", e(2)), ("threshold", n(30.0)), ("radius", n(8.0))]);
    effect_case("ec.stylize.glow", &[("glowOperation", e(3)), ("colors", e(1)), ("intensity", n(2.0))]);
    effect_case("ec.stylize.glow", &[("colors", e(2)), ("arbitraryMap", Value::Str("0,0 0.5,0.8 1,1|0,0.2 1,0.6|".into())), ("threshold", n(20.0))]);
    effect_case("ec.stylize.glow", &[("colors", e(2)), ("arbitraryMap", Value::Str("0,1 1,0||0,0 1,1".into())), ("operation", e(1)), ("glowOperation", e(4))]);
}

/// Texturize with a texture layer (Tile / Center / Stretch), composited.
#[test]
fn texturize_layer() {
    for depth in [BitDepth::Bpc8, BitDepth::Bpc32] {
        for placement in 0..3 {
            let mut s = Scene::new(depth);
            let bg = s.solid([0.15, 0.1, 0.2], 97, 61);
            s.push(bg);
            let mut tex = s.footage(50, 36);
            tex.switches.video = false;
            let tid = s.push(tex);
            let mut l = s.footage(70, 44);
            s.effect(
                &mut l,
                "ec.stylize.texturize",
                &[("textureLayer", Value::Layer(Some(tid.0))), ("texturePlacement", e(placement)), ("textureContrast", n(1.5)), ("lightDirection", n(60.0))],
            );
            set(&mut l, "transform/rotation", Value::Scalar(8.0));
            s.push(l);
            let label = format!("texturize {placement} {depth:?}");
            check(&label, compare_at(&s, opts(), Tick::ZERO), 0.0);
            check(&format!("{label} half"), compare_at(&s, RenderOpts { scale: 0.5, ..opts() }, Tick::ZERO), 0.0);
        }
    }
}

/// Transform's own shutter angle: the effect's rotation and position animate.
#[test]
fn transform_motion_blur() {
    for depth in [BitDepth::Bpc8, BitDepth::Bpc32] {
        let mut s = Scene::new(depth);
        let bg = s.solid([0.15, 0.1, 0.2], 97, 61);
        s.push(bg);
        let mut l = s.footage(70, 44);
        s.effect(&mut l, "ec.distort.transform", &[("useCompositionShutterAngle", Value::Bool(false)), ("shutterAngle", n(180.0))]);
        let rot = l.props.prop_mut("effects/#1/rotation").expect("transform rotation");
        rot.keys = vec![Keyframe::new(Tick::ZERO, Value::Scalar(0.0)), Keyframe::new(Tick::from_seconds_f64(1.0), Value::Scalar(120.0))];
        let pos = l.props.prop_mut("effects/#1/position").expect("transform position");
        pos.keys = vec![Keyframe::new(Tick::ZERO, pt(35.0, 22.0)), Keyframe::new(Tick::from_seconds_f64(1.0), pt(95.0, 40.0))];
        s.push(l);
        let label = format!("transform motion blur {depth:?}");
        check(&label, compare_at(&s, opts(), Tick::from_seconds_f64(0.3)), 0.0);
        check(&format!("{label} half"), compare_at(&s, RenderOpts { scale: 0.5, ..opts() }, Tick::from_seconds_f64(0.3)), 0.0);
    }
}

/// Motion Tile's Output size on an adjustment layer with a Transform after it that scales the
/// tiles into view, and cropping (Output under 100 %): the stack's grown or cropped frame goes
/// back onto the canvas as on the CPU (#1727).
#[test]
fn adjustment_motion_tile_output_then_transform() {
    for (out_w, out_h, scale) in [(300.0, 300.0, 40.0), (250.0, 120.0, 70.0), (60.0, 80.0, 100.0)] {
        for depth in [BitDepth::Bpc8, BitDepth::Bpc32] {
            let mut s = Scene::new(depth);
            let bg = s.footage(97, 61);
            s.push(bg);
            let mut adj = s.solid([1.0, 1.0, 1.0], 97, 61);
            adj.switches.adjustment = true;
            s.effect(&mut adj, "ec.stylize.motiontile", &[("outputWidth", n(out_w)), ("outputHeight", n(out_h)), ("mirrorEdges", on())]);
            s.effect(&mut adj, "ec.distort.transform", &[("scaleHeight", n(scale)), ("scaleWidth", n(scale))]);
            s.push(adj);
            let label = format!("adjustment Motion Tile {out_w}×{out_h} % then Scale {scale} % {depth:?}");
            check(&label, compare_at(&s, opts(), Tick::ZERO), 0.0);
            check(&format!("{label} half"), compare_at(&s, RenderOpts { scale: 0.5, ..opts() }, Tick::ZERO), 0.0);
        }
    }
}

/// The same on a full-frame footage layer (#1991).
#[test]
fn layer_motion_tile_output_then_transform() {
    for (out_w, out_h, scale) in [(300.0, 300.0, 40.0), (250.0, 120.0, 70.0)] {
        for depth in [BitDepth::Bpc8, BitDepth::Bpc32] {
            let mut s = Scene::new(depth);
            let mut l = s.footage(97, 61);
            s.effect(&mut l, "ec.stylize.motiontile", &[("outputWidth", n(out_w)), ("outputHeight", n(out_h)), ("mirrorEdges", on())]);
            s.effect(&mut l, "ec.distort.transform", &[("scaleHeight", n(scale)), ("scaleWidth", n(scale))]);
            s.push(l);
            let label = format!("layer Motion Tile {out_w}×{out_h} % then Scale {scale} % {depth:?}");
            check(&label, compare_at(&s, opts(), Tick::ZERO), 0.0);
            check(&format!("{label} half"), compare_at(&s, RenderOpts { scale: 0.5, ..opts() }, Tick::ZERO), 0.0);
        }
    }
}
