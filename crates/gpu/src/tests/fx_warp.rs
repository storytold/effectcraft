//! GPU warp family effects (Warp, Bezier Warp, CC Bend It, CC Page Turn, Smear, Reshape, Color
//! Emboss, Cartoon) vs the CPU effects (the oracle): direct on a buffer at full and half
//! resolution, as adjustment, and composited at 8 and 32 bpc.

use std::io::Write;

use effectcraft_effects::MaskShape;
use effectcraft_keyframe::{ShapePath, Value};
use effectcraft_project::build::{self, Ids};
use effectcraft_project::{BitDepth, MaskMode};
use effectcraft_render::RenderOpts;
use effectcraft_time::Tick;

use crate::tests::{Scene, c, check, compare_at, diff, effect_case, gpu, n, on_llvmpipe, opts, pattern};

fn e(v: u32) -> Value {
    Value::Enum(v)
}

fn pt(x: f64, y: f64) -> Value {
    Value::Vec2([x, y])
}

#[test]
fn warp_styles() {
    for style in 0..15 {
        effect_case("ec.distort.warp", &[("warpStyle", e(style)), ("bend", n(45.0))]);
    }
    effect_case("ec.distort.warp", &[("warpStyle", e(8)), ("warpAxis", e(1)), ("bend", n(-60.0)), ("horizontalDistortion", n(20.0))]);
    effect_case("ec.distort.warp", &[("warpStyle", e(4)), ("bend", n(80.0)), ("verticalDistortion", n(-35.0))]);
    effect_case("ec.distort.warp", &[("warpStyle", e(11)), ("bend", n(-50.0))]);
    effect_case("ec.distort.warp", &[("warpStyle", e(14)), ("bend", n(-35.0))]);
    // Strong Fisheye / Twist bends and either with a distortion: the CPU's f64 inverse map.
    for (style, bend) in [(11, 90.0), (11, -100.0), (14, 75.0), (14, -100.0)] {
        effect_case("ec.distort.warp", &[("warpStyle", e(style)), ("bend", n(bend))]);
    }
    effect_case("ec.distort.warp", &[("warpStyle", e(14)), ("verticalDistortion", n(-35.0))]);
    effect_case("ec.distort.warp", &[("warpStyle", e(11)), ("warpAxis", e(1)), ("bend", n(60.0)), ("horizontalDistortion", n(30.0))]);
    effect_case("ec.distort.warp", &[("warpStyle", e(3)), ("bend", n(0.0)), ("horizontalDistortion", n(-40.0)), ("verticalDistortion", n(25.0))]);
}

#[test]
fn bezier_warp() {
    effect_case("ec.distort.bezierwarp", &[]);
    effect_case(
        "ec.distort.bezierwarp",
        &[("topLeftVertex", pt(8.0, -4.0)), ("topLeftTangent", pt(30.0, 12.0)), ("rightBottomTangent", pt(80.0, 30.0)), ("leftBottomVertex", pt(-6.0, 50.0))],
    );
    effect_case("ec.distort.bezierwarp", &[("topRightTangent", pt(40.0, -20.0)), ("bottomLeftTangent", pt(20.0, 60.0)), ("quality", n(3.0))]);
}

#[test]
fn bend_it_and_page_turn() {
    effect_case("ec.distort.ccbendit", &[("bend", n(45.0))]);
    effect_case("ec.distort.ccbendit", &[("bend", n(-120.0)), ("start", pt(10.0, 30.0)), ("end", pt(55.0, 8.0)), ("renderPrestart", e(1))]);
    // A slight bend: a large arc radius.
    effect_case("ec.distort.ccbendit", &[("bend", n(0.05)), ("start", pt(5.0, 22.0)), ("end", pt(60.0, 22.0))]);
    effect_case("ec.distort.ccpageturn", &[]);
    effect_case("ec.distort.ccpageturn", &[("foldRadius", n(4.0)), ("foldDirection", n(30.0)), ("foldPosition", pt(40.0, 20.0))]);
    effect_case(
        "ec.distort.ccpageturn",
        &[("render", e(2)), ("backPageOpacity", n(40.0)), ("paperColor", c(0.9, 0.85, 0.7)), ("lightDirection", n(120.0)), ("foldPosition", pt(30.0, 25.0))],
    );
    effect_case("ec.distort.ccpageturn", &[("render", e(1)), ("foldPosition", pt(50.0, 10.0))]);
}

#[test]
fn color_emboss_and_cartoon() {
    effect_case("ec.stylize.coloremboss", &[]);
    effect_case("ec.stylize.coloremboss", &[("direction", n(30.0)), ("relief", n(3.5)), ("contrast", n(250.0)), ("blend", n(30.0))]);
    effect_case("ec.stylize.cartoon", &[]);
    effect_case("ec.stylize.cartoon", &[("render", e(0)), ("detailRadius", n(4.0)), ("fill/shadingSteps", n(5.0)), ("fill/shadingSmoothness", n(0.0))]);
    effect_case("ec.stylize.cartoon", &[("render", e(1)), ("edge/edgeWidth", n(0.6)), ("advanced/edgeBlackLevel", n(40.0)), ("advanced/edgeContrast", n(0.8))]);
    effect_case("ec.stylize.cartoon", &[("edge/edgeWidth", n(3.3)), ("advanced/edgeEnhancement", n(60.0)), ("edge/edgeSoftness", n(20.0))]);
    effect_case("ec.stylize.cartoon", &[("advanced/edgeEnhancement", n(-50.0)), ("detailRadius", n(0.0))]);
}

/// Mask outlines in layer pixels (70 × 44 layers): a source blob, a destination blob and a
/// boundary around both.
fn outlines() -> Vec<MaskShape> {
    let ellipse = |cx: f64, cy: f64, rx: f64, ry: f64| -> Vec<[f64; 2]> {
        (0..40).map(|i| i as f64 / 40.0 * std::f64::consts::TAU).map(|a| [cx + rx * a.cos(), cy + ry * a.sin()]).collect()
    };
    let shape = |name: &str, points| MaskShape { name: name.into(), points, closed: true, inverted: false };
    vec![shape("Mask 1", ellipse(25.0, 20.0, 9.0, 7.0)), shape("Mask 2", ellipse(42.0, 24.0, 12.0, 6.0)), shape("Mask 3", ellipse(34.0, 22.0, 30.0, 19.0))]
}

/// The effect alone on a buffer with masks (`effect_direct` with the layer's masks).
fn direct_with_masks(id: &str, vals: &[(&str, Value)]) {
    let Some(g) = gpu() else { return };
    let spec = effectcraft_effects::find(id).unwrap();
    let size = [70.0, 44.0];
    let masks = outlines();
    let mut params =
        effectcraft_effects::Params { values: spec.params.iter().map(|ps| (ps.id.to_string(), effectcraft_effects::default_value(ps, size))).collect() };
    for (k, v) in vals {
        params.values.insert(k.to_string(), v.clone());
    }
    for adjustment in [false, true] {
        for scale in [1.0, 0.5] {
            let img = pattern(7, (size[0] * scale) as u32, (size[1] * scale) as u32);
            let buf = effectcraft_effects::Buf { img, offset: [0.0, 0.0], scale };
            let ctx = || effectcraft_effects::EffectCtx {
                params: &params,
                time: 0.25,
                layer_size: size,
                seed: 11,
                adjustment,
                env: effectcraft_effects::EffectEnv { masks: &masks, ..Default::default() },
            };
            let cpu = (spec.render)(&ctx(), buf.clone());
            let out =
                effectcraft_render::Accelerator::effects(g, &[effectcraft_render::FxStep { spec, ctx: ctx() }], &buf, None).expect("the GPU runs the effect");
            assert_eq!((out.offset, out.scale), (cpu.offset, cpu.scale), "{id}: geometry");
            assert!(cpu.img.data != buf.img.data, "{id} {vals:?}: the case changes the picture");
            let d = diff(&cpu.img, &out.img, 1.0 / 255.0);
            // f32 point-in-polygon tests may flip a pixel exactly on an outline.
            assert!(
                d.over * 1000 <= d.total,
                "{id} {vals:?} adj {adjustment} scale {scale}: {} of {} pixels over 1/255 (max {}); worst {:?}",
                d.over,
                d.total,
                d.max,
                d.worst
            );
        }
    }
}

/// Composited at 8 and 32 bpc, with the masks on the layer (mode None: they only feed the effect).
fn composite_with_masks(id: &str, vals: &[(&str, Value)]) {
    for depth in [BitDepth::Bpc8, BitDepth::Bpc32] {
        let mut s = Scene::new(depth);
        let bg = s.solid([0.15, 0.1, 0.2], 97, 61);
        s.push(bg);
        let mut l = s.footage(70, 44);
        let mut next = s.p.next_id;
        for m in outlines() {
            let path = ShapePath {
                vertices: m.points.clone(),
                in_tangents: vec![[0.0; 2]; m.points.len()],
                out_tangents: vec![[0.0; 2]; m.points.len()],
                closed: true,
                ..Default::default()
            };
            let g = build::mask(&mut Ids(&mut next), &m.name, path, MaskMode::None, [255, 255, 0]);
            l.props.sub_mut("masks").unwrap().children.push(g.into());
        }
        s.p.next_id = next;
        s.effect(&mut l, id, vals);
        s.push(l);
        check(&format!("{id} {vals:?} {depth:?}"), compare_at(&s, opts(), Tick::ZERO), 0.003);
        check(&format!("{id} {vals:?} {depth:?} half"), compare_at(&s, RenderOpts { scale: 0.5, ..opts() }, Tick::ZERO), 0.003);
    }
}

#[test]
fn smear_and_reshape() {
    let smear: &[&[(&str, Value)]] = &[
        &[("sourceMask", e(1)), ("boundaryMask", e(3)), ("maskOffset", pt(10.0, 3.0))],
        &[
            ("sourceMask", e(1)),
            ("boundaryMask", e(3)),
            ("maskOffset", pt(-4.0, 6.0)),
            ("maskRotation", n(30.0)),
            ("maskScale", n(130.0)),
            ("elasticity", e(6)),
            ("percent", n(70.0)),
        ],
    ];
    for vals in smear {
        direct_with_masks("ec.distort.smear", vals);
        composite_with_masks("ec.distort.smear", vals);
    }
    let reshape: &[&[(&str, Value)]] = &[
        &[("sourceMask", e(1)), ("destinationMask", e(2))],
        &[("sourceMask", e(1)), ("destinationMask", e(2)), ("boundaryMask", e(3)), ("interpolation", e(2)), ("elasticity", e(0))],
        &[
            ("sourceMask", e(1)),
            ("destinationMask", e(2)),
            ("boundaryMask", e(3)),
            ("correspondencePoints", Value::Str("0,0.25 0.5,0.8".into())),
            ("percent", n(60.0)),
        ],
    ];
    for vals in reshape {
        if on_llvmpipe() && vals.iter().any(|(k, _)| *k == "boundaryMask") {
            // Straight to stderr: libtest captures `eprintln!` of passing tests, and the skip
            // should show in the log.
            let _ = writeln!(std::io::stderr(), "tests_fx_warp::smear_and_reshape: skipping ec.distort.reshape {vals:?} on llvmpipe (crashes lavapipe, #2238)");
            continue;
        }
        direct_with_masks("ec.distort.reshape", vals);
        composite_with_masks("ec.distort.reshape", vals);
    }
}
