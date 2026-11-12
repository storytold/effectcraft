//! GPU keying, matte and channel family effects vs the CPU effects (the oracle): direct on a
//! buffer at full and half resolution, as adjustment, and composited at 8 and 32 bpc.

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

fn off() -> Value {
    Value::Bool(false)
}

/// [`effect_case`] for hard thresholds (8-bit quantisation, bitwise operators): the composited
/// renders may flip a few pixels sitting on the threshold after the layer transform's float
/// rounding, so up to 0.1 % of them may differ by any amount.
fn discontinuous_case(id: &str, vals: &[(&str, Value)]) {
    crate::tests::effect_direct(id, vals, false);
    crate::tests::effect_direct(id, vals, true);
    for depth in [BitDepth::Bpc8, BitDepth::Bpc32] {
        let mut s = Scene::new(depth);
        let bg = s.solid([0.15, 0.1, 0.2], 97, 61);
        s.push(bg);
        let mut l = s.footage(70, 44);
        s.effect(&mut l, id, vals);
        set(&mut l, "transform/rotation", Value::Scalar(8.0));
        s.push(l);
        check(&format!("{id} {vals:?} {depth:?}"), compare_at(&s, opts(), Tick::ZERO), 0.001);
        check(&format!("{id} {vals:?} {depth:?} half"), compare_at(&s, RenderOpts { scale: 0.5, ..opts() }, Tick::ZERO), 0.001);
    }
}

#[test]
fn colour_luma_range_extract_keys() {
    effect_case("ec.key.colorkey", &[("keyColor", c(0.7, 0.6, 0.5)), ("colorTolerance", n(60.0))]);
    effect_case("ec.key.colorkey", &[("keyColor", c(0.5, 0.6, 0.5)), ("colorTolerance", n(70.0)), ("edgeThin", n(1.5)), ("edgeFeather", n(4.0))]);
    effect_case("ec.key.colorkey", &[("keyColor", c(0.5, 0.6, 0.5)), ("colorTolerance", n(70.0)), ("edgeThin", n(-2.0))]);
    effect_case("ec.key.luma", &[("threshold", n(120.0)), ("tolerance", n(40.0))]);
    effect_case("ec.key.luma", &[("keyType", e(0)), ("threshold", n(140.0)), ("tolerance", n(30.0)), ("edgeFeather", n(3.0))]);
    effect_case("ec.key.luma", &[("keyType", e(2)), ("threshold", n(128.0)), ("tolerance", n(25.0)), ("edgeThin", n(1.0))]);
    effect_case("ec.key.luma", &[("keyType", e(3)), ("threshold", n(100.0)), ("tolerance", n(50.0))]);
    effect_case("ec.key.colorrange", &[("fuzziness", n(20.0))]);
    effect_case("ec.key.colorrange", &[("colorSpace", e(1)), ("minL", n(60.0)), ("maxA", n(150.0)), ("minB", n(90.0)), ("fuzziness", n(15.0))]);
    effect_case("ec.key.colorrange", &[("colorSpace", e(2)), ("minL", n(80.0)), ("maxA", n(180.0)), ("minB", n(40.0)), ("fuzziness", n(30.0))]);
    effect_case("ec.key.extract", &[("blackPoint", n(60.0)), ("whitePoint", n(200.0)), ("blackSoftness", n(30.0)), ("whiteSoftness", n(20.0))]);
    discontinuous_case("ec.key.extract", &[("channel", e(1)), ("blackPoint", n(90.0)), ("invert", on())]);
    effect_case("ec.key.extract", &[("channel", e(4)), ("whitePoint", n(200.0)), ("whiteSoftness", n(40.0))]);
}

#[test]
fn difference_colour_difference_and_screen_keys() {
    effect_case("ec.key.differencematte", &[]);
    effect_case("ec.key.differencematte", &[("view", e(2)), ("matchingTolerance", n(0.0)), ("blurBeforeDifference", n(3.0))]);
    effect_case("ec.key.colordifference", &[("keyColor", c(0.2, 0.3, 0.8))]);
    effect_case(
        "ec.key.colordifference",
        &[
            ("keyColor", c(0.3, 0.8, 0.2)),
            ("colorMatchingAccuracy", e(1)),
            ("partialAInBlack", n(20.0)),
            ("partialAGamma", n(1.4)),
            ("partialBOutWhite", n(200.0)),
            ("blackLevel", n(10.0)),
            ("gamma", n(0.8)),
        ],
    );
    for view in [1, 2, 3, 4, 5, 6, 8] {
        effect_case("ec.key.colordifference", &[("keyColor", c(0.2, 0.3, 0.8)), ("view", e(view)), ("partialBInWhite", n(180.0))]);
    }
    effect_case("ec.key.screen", &[("screenColor", c(0.4, 0.75, 0.3))]);
    effect_case(
        "ec.key.screen",
        &[
            ("screenColor", c(0.3, 0.4, 0.8)),
            ("screenGain", n(130.0)),
            ("screenBalance", n(30.0)),
            ("despill", n(60.0)),
            ("clipBlack", n(10.0)),
            ("clipWhite", n(85.0)),
            ("screenPreblur", n(2.0)),
            ("screenShrinkGrow", n(-1.5)),
            ("screenSoftness", n(3.0)),
        ],
    );
    effect_case("ec.key.screen", &[("screenColor", c(0.8, 0.4, 0.3)), ("screenShrinkGrow", n(2.0)), ("edgeColor", e(1))]);
    for view in [1, 2] {
        effect_case("ec.key.screen", &[("screenColor", c(0.4, 0.75, 0.3)), ("view", e(view))]);
    }
}

#[test]
fn spill_unmult_and_key_cleaner() {
    effect_case("ec.key.spill", &[("colorToSuppress", c(0.2, 0.9, 0.1))]);
    effect_case("ec.key.spill", &[("colorToSuppress", c(0.9, 0.8, 0.1)), ("colorAccuracy", e(1)), ("suppression", n(150.0))]);
    effect_case("ec.key.advancedspill", &[]);
    effect_case("ec.key.advancedspill", &[("suppression", n(80.0)), ("ultraSettings/spillRange", n(20.0)), ("ultraSettings/lumaCorrection", n(50.0))]);
    effect_case(
        "ec.key.advancedspill",
        &[
            ("method", e(1)),
            ("ultraSettings/keyColor", c(0.9, 0.2, 0.3)),
            ("ultraSettings/tolerance", n(40.0)),
            ("ultraSettings/desaturate", n(50.0)),
            ("ultraSettings/spillColorCorrection", n(30.0)),
        ],
    );
    effect_case("ec.key.unmult", &[]);
    effect_case("ec.key.unmult", &[("backgroundColor", e(1)), ("whiteLevel", n(90.0)), ("softness", n(40.0)), ("clipHdrResults", off())]);
    effect_case("ec.key.unmult", &[("blackLevel", n(30.0)), ("softness", n(0.0)), ("removeColorMatting", off())]);
    effect_case("ec.key.keycleaner", &[]);
    effect_case("ec.key.keycleaner", &[("additionalEdgeRadius", n(6.0)), ("reduceChatter", on()), ("alphaContrast", n(250.0)), ("strength", n(60.0))]);
}

#[test]
fn chokers() {
    effect_case("ec.matte.simplechoker", &[("chokeMatte", n(1.5))]);
    effect_case("ec.matte.simplechoker", &[("chokeMatte", n(-2.5))]);
    effect_case("ec.matte.simplechoker", &[("chokeMatte", n(-1.0)), ("view", e(1))]);
    effect_case("ec.matte.mattechoker", &[]);
    effect_case(
        "ec.matte.mattechoker",
        &[
            ("geometricSoftness1", n(2.0)),
            ("choke1", n(-40.0)),
            ("geometricSoftness2", n(3.0)),
            ("choke2", n(30.0)),
            ("grayLevelSoftness2", n(40.0)),
            ("iterations", n(2.0)),
        ],
    );
}

#[test]
fn refine_mattes() {
    effect_case("ec.matte.refinesoft", &[]);
    effect_case("ec.matte.refinesoft", &[("radius", n(4.0)), ("smooth", n(40.0)), ("feather", n(30.0)), ("contrast", n(20.0)), ("shiftEdge", n(-25.0))]);
    effect_case("ec.matte.refinesoft", &[("calculateEdgeDetails", off()), ("smooth", n(50.0)), ("shiftEdge", n(30.0))]);
    effect_case("ec.matte.refinesoft", &[("radius", n(3.0)), ("viewEdgeRegion", on())]);
    effect_case("ec.matte.refinesoft", &[("decontamination/viewDecontaminationMap", on()), ("decontamination/extendWhereSmoothed", off())]);
    effect_case("ec.matte.refinesoft", &[("decontamination/increaseDecontaminationRadius", n(4.0)), ("decontamination/decontaminationAmount", n(60.0))]);
    effect_case("ec.matte.refinehard", &[]);
    effect_case("ec.matte.refinehard", &[("choke", n(20.0)), ("feather", n(20.0)), ("decontaminate", off())]);
}

#[test]
fn channel_effects() {
    effect_case("ec.channel.setchannels", &[("setRedTo", e(2)), ("setGreenTo", e(4)), ("setBlueTo", e(5)), ("setAlphaTo", e(6))]);
    effect_case("ec.channel.shiftchannels", &[("takeRedFrom", e(3)), ("takeGreenFrom", e(7)), ("takeAlphaFrom", e(4))]);
    // #1797: the constant sources at the end of the menu, where the GPU's order could drift from
    // the CPU's (Full On, Full Off, Half On).
    effect_case("ec.channel.shiftchannels", &[("takeRedFrom", e(8)), ("takeGreenFrom", e(9)), ("takeBlueFrom", e(10))]);
    effect_case("ec.channel.removecolormatting", &[("backgroundColor", c(0.3, 0.5, 0.7))]);
    effect_case("ec.channel.removecolormatting", &[("backgroundColor", c(0.8, 0.2, 0.1)), ("clipHdr", off())]);
    for op in [0, 2, 3, 4, 8, 10, 12] {
        discontinuous_case("ec.channel.arithmetic", &[("operator", e(op)), ("redValue", n(120.0)), ("greenValue", n(60.0)), ("blueValue", n(200.0))]);
    }
    effect_case("ec.channel.arithmetic", &[("operator", e(3)), ("redValue", n(200.0)), ("clip", off())]);
    for mode in [0, 2, 4, 9, 11] {
        effect_case("ec.channel.solidcomposite", &[("blendingMode", e(mode)), ("color", c(0.8, 0.3, 0.2)), ("opacity", n(70.0)), ("sourceOpacity", n(80.0))]);
    }
    effect_case("ec.channel.setmatte", &[("takeMatteFrom", e(4))]);
    effect_case("ec.channel.setmatte", &[("takeMatteFrom", e(3)), ("invertMatte", on())]);
    effect_case("ec.channel.setmatte", &[("takeMatteFrom", e(1)), ("compositeMatteWithOriginal", off()), ("premultiplyMatteLayer", off())]);
    for (from, to) in [(0, 0), (1, 0), (2, 0), (3, 0), (8, 4), (9, 5), (10, 6), (11, 3), (12, 8), (13, 7), (7, 1)] {
        effect_case("ec.channel.combiner", &[("from", e(from)), ("to", e(to))]);
    }
    effect_case("ec.channel.combiner", &[("from", e(4)), ("to", e(2)), ("invert", on()), ("solidAlpha", on())]);
    effect_case("ec.channel.blend", &[("mode", e(1)), ("blendWithOriginal", n(30.0))]);
    effect_case("ec.channel.calculations", &[("inputChannel", e(1))]);
    effect_case("ec.channel.calculations", &[("inputChannel", e(0)), ("invertInput", on())]);
    effect_case("ec.channel.compoundarithmetic", &[("operator", e(3)), ("operateOnChannels", e(1))]);
}

/// The channel and keying effects that read another layer, composited.
#[test]
fn layer_inputs() {
    let cases: Vec<(&str, Vec<(&str, Value)>)> = vec![
        ("ec.key.differencematte", vec![("matchingTolerance", n(10.0)), ("matchingSoftness", n(20.0))]),
        ("ec.key.differencematte", vec![("ifLayerSizesDiffer", e(1)), ("blurBeforeDifference", n(2.0)), ("view", e(2))]),
        ("ec.channel.setmatte", vec![("takeMatteFrom", e(4)), ("stretchMatteToFit", off())]),
        ("ec.channel.setmatte", vec![("takeMatteFrom", e(3))]),
        ("ec.channel.setchannels", vec![("setGreenTo", e(0)), ("setAlphaTo", e(4))]),
        ("ec.channel.combiner", vec![("useSecondLayer", on()), ("from", e(11)), ("to", e(7))]),
        ("ec.channel.blend", vec![("mode", e(0)), ("blendWithOriginal", n(40.0))]),
        ("ec.channel.blend", vec![("mode", e(2))]),
        ("ec.channel.blend", vec![("mode", e(3)), ("ifLayerSizesDiffer", e(1))]),
        ("ec.channel.blend", vec![("mode", e(4))]),
        ("ec.channel.calculations", vec![("secondLayerChannel", e(2)), ("blendingMode", e(2)), ("secondLayerOpacity", n(70.0))]),
        ("ec.channel.calculations", vec![("inputChannel", e(4)), ("blendingMode", e(10)), ("preserveTransparency", on()), ("invertSecondLayer", on())]),
        ("ec.channel.compoundarithmetic", vec![("operator", e(1)), ("overflowBehavior", e(2))]),
        ("ec.channel.compoundarithmetic", vec![("operator", e(4)), ("operateOnChannels", e(1)), ("blendWithOriginal", n(25.0))]),
        ("ec.channel.compoundarithmetic", vec![("operator", e(13)), ("operateOnChannels", e(2))]),
    ];
    let layer_param = |id: &str| match id {
        "ec.key.differencematte" => vec!["differenceLayer"],
        "ec.channel.setmatte" => vec!["takeMatteFromLayer"],
        "ec.channel.setchannels" => vec!["sourceLayer2", "sourceLayer4"],
        "ec.channel.combiner" => vec!["sourceLayer"],
        "ec.channel.blend" => vec!["blendWithLayer"],
        "ec.channel.calculations" => vec!["secondLayer"],
        _ => vec!["secondSource"],
    };
    for depth in [BitDepth::Bpc8, BitDepth::Bpc32] {
        for (id, vals) in &cases {
            let mut s = Scene::new(depth);
            let bg = s.solid([0.15, 0.1, 0.2], 97, 61);
            s.push(bg);
            let mut other = s.footage(50, 36);
            other.switches.video = false;
            let oid = s.push(other);
            let mut l = s.footage(70, 44);
            let mut vals = vals.clone();
            for p in layer_param(id) {
                vals.push((p, Value::Layer(Some(oid.0))));
            }
            s.effect(&mut l, id, &vals);
            set(&mut l, "transform/rotation", Value::Scalar(8.0));
            s.push(l);
            let label = format!("{id} {vals:?} {depth:?}");
            check(&label, compare_at(&s, opts(), Tick::ZERO), 0.0);
            check(&format!("{label} half"), compare_at(&s, RenderOpts { scale: 0.5, ..opts() }, Tick::ZERO), 0.0);
        }
    }
}

#[test]
fn arithmetic_preserves_positive_subepsilon_alpha() {
    let Some(g) = crate::tests::gpu() else { return };
    let spec = effectcraft_effects::find("ec.channel.arithmetic").unwrap();
    let size = [1.0, 1.0];
    let mut params =
        effectcraft_effects::Params { values: spec.params.iter().map(|p| (p.id.to_string(), effectcraft_effects::default_value(p, size))).collect() };
    params.values.insert("operator".into(), Value::Enum(3));
    for key in ["redValue", "greenValue", "blueValue"] {
        params.values.insert(key.into(), n(0.0));
    }
    params.values.insert("clip".into(), Value::Bool(false));
    let ctx = || effectcraft_effects::EffectCtx { params: &params, time: 0.0, layer_size: size, seed: 0, adjustment: false, env: Default::default() };
    let img = effectcraft_raster::Image::filled(1, 1, [1.0, 0.0, 0.0, 5e-7]);
    let buf = effectcraft_effects::Buf { img, offset: [0.0; 2], scale: 1.0 };
    let cpu = (spec.render)(&ctx(), buf.clone());
    let out = effectcraft_render::Accelerator::effects(g, &[effectcraft_render::FxStep { spec, ctx: ctx() }], &buf, None).unwrap();
    assert_eq!(cpu.img.get(0, 0), [1.0, 0.0, 0.0, 5e-7]);
    assert_eq!(out.img.get(0, 0), cpu.img.get(0, 0));
}
