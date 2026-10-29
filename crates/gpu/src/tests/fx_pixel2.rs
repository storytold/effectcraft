//! GPU pixel ports of part C (CC Cross / Radial / Vector Blur, Reduce Interlace Flicker, CC
//! Composite, Color Link, Cineon Converter, HDR Compander, HDR Highlight Compression, Grow
//! Bounds, CC Overbrights, CC Block Load, CC Burn Film, CC Glass, CC HexTile, CC Mr. Smoothie,
//! CC Plastic, Inner/Outer Key, CC Simple Wire Removal, Basic 3D) vs the CPU effects (the
//! oracle): direct on a buffer at full and half resolution, as adjustment, and composited at 8
//! and 32 bpc. Also the helpers for effects that read masks, other layers and audio.

use effectcraft_effects::{Buf, EffectEnv, EffectHost, LayerPixels, MaskShape};
use effectcraft_keyframe::{ShapePath, Value};
use effectcraft_project::build::{self, Ids};
use effectcraft_project::{BitDepth, MaskMode};
use effectcraft_render::RenderOpts;
use effectcraft_time::Tick;

use crate::tests::{Scene, allow_llvmpipe_boundary_flips, c, check, compare_at, diff, effect_case, gpu, n, opts, pattern, set};

pub(crate) fn e(v: u32) -> Value {
    Value::Enum(v)
}

pub(crate) fn on() -> Value {
    Value::Bool(true)
}

pub(crate) fn off() -> Value {
    Value::Bool(false)
}

pub(crate) fn pt(x: f64, y: f64) -> Value {
    Value::Vec2([x, y])
}

/// Size of the test layer (layer pixels).
const SIZE: [f64; 2] = [70.0, 44.0];

/// The test layer's masks (layer pixels): a closed pentagon, a closed quad and an open curve.
pub(crate) fn mask_points() -> Vec<(Vec<[f64; 2]>, bool)> {
    let pent = (0..5)
        .map(|i| {
            let a = i as f64 * std::f64::consts::TAU / 5.0 - 1.3;
            [33.3 + a.cos() * 17.1, 22.4 + a.sin() * 15.2]
        })
        .collect();
    vec![
        (pent, true),
        (vec![[6.3, 5.1], [62.2, 7.7], [59.4, 39.6], [9.1, 37.2]], true),
        (vec![[4.2, 30.3], [18.7, 12.4], [33.1, 28.9], [47.6, 9.8], [64.2, 25.5]], false),
    ]
}

/// Another layer (`id` 9) and an audio track for layer parameters.
pub(crate) struct Host;
impl EffectHost for Host {
    fn layer(&self, id: u64, _: bool) -> Option<LayerPixels> {
        Some(LayerPixels { buf: Buf { img: pattern(id as u32 + 3, 70, 44), offset: [0.0; 2], scale: 1.0 }, size: SIZE })
    }
    fn audio(&self, _: u64, start: f64, frames: usize, rate: u32) -> Option<Vec<f32>> {
        Some(
            (0..frames)
                .flat_map(|i| {
                    let t = start + i as f64 / rate as f64;
                    let l = 0.5 * (t * 440.0 * std::f64::consts::TAU).sin() + 0.2 * (t * 1300.0 * std::f64::consts::TAU).sin();
                    let r = 0.4 * (t * (200.0 + 300.0 * t) * std::f64::consts::TAU).sin();
                    [l as f32, r as f32]
                })
                .collect(),
        )
    }
}

/// The effect alone on a buffer with the test masks and [`Host`]: GPU vs CPU (32-bit float)
/// at full and half resolution (the half buffer padded and offset), direct and as adjustment.
/// `allow` = fraction of pixels allowed over 1e-3.
pub(crate) fn direct_env(id: &str, vals: &[(&str, Value)], allow: f64) {
    let Some(g) = gpu() else { return };
    let spec = effectcraft_effects::find(id).unwrap();
    let mut params =
        effectcraft_effects::Params { values: spec.params.iter().map(|ps| (ps.id.to_string(), effectcraft_effects::default_value(ps, SIZE))).collect() };
    for (k, v) in vals {
        assert!(params.values.contains_key(*k), "{id}: no {k}");
        params.values.insert(k.to_string(), v.clone());
    }
    let masks: Vec<MaskShape> = mask_points()
        .into_iter()
        .enumerate()
        .map(|(i, (points, closed))| MaskShape { name: format!("Mask {}", i + 1), points, closed, inverted: false })
        .collect();
    let host = Host;
    for adjustment in [false, true] {
        for scale in [1.0, 0.5] {
            let img = pattern(7, (SIZE[0] * scale) as u32, (SIZE[1] * scale) as u32);
            let mut buf = Buf { img, offset: [0.0, 0.0], scale };
            if scale < 1.0 {
                buf.pad(3);
            }
            let env = EffectEnv { masks: &masks, host: Some(&host), comp_time: 0.25, frame_rate: 30.0, ..Default::default() };
            let ctx = || effectcraft_effects::EffectCtx { params: &params, time: 0.25, layer_size: SIZE, seed: 11, adjustment, env };
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

/// [`direct_env`], then a footage layer with the test masks (mode None) and the effect,
/// composited over a background at 8 and 32 bpc, full and half resolution.
pub(crate) fn env_case(id: &str, vals: &[(&str, Value)]) {
    env_case_allow(id, vals, 0.0);
}

/// [`env_case`] allowing a fraction of pixels over the tolerance (documented exceptions).
pub(crate) fn env_case_allow(id: &str, vals: &[(&str, Value)], allow: f64) {
    env_case_at(id, vals, allow, Tick::ZERO);
}

/// [`env_case_allow`] with the composited frames at comp time `t`.
pub(crate) fn env_case_at(id: &str, vals: &[(&str, Value)], allow: f64, t: Tick) {
    direct_env(id, vals, allow);
    for depth in [BitDepth::Bpc8, BitDepth::Bpc32] {
        let mut s = Scene::new(depth);
        let bg = s.solid([0.15, 0.1, 0.2], 97, 61);
        s.push(bg);
        let mut l = s.footage(70, 44);
        let mut next = s.p.next_id;
        for (i, (pts, closed)) in mask_points().into_iter().enumerate() {
            let m = build::mask(&mut Ids(&mut next), &format!("Mask {}", i + 1), ShapePath::polygon(&pts, closed), MaskMode::None, [255, 255, 0]);
            l.props.sub_mut("masks").unwrap().children.push(m.into());
        }
        s.p.next_id = next;
        s.effect(&mut l, id, vals);
        set(&mut l, "transform/rotation", Value::Scalar(8.0));
        s.push(l);
        check(&format!("{id} {vals:?} {depth:?}"), compare_at(&s, opts(), t), allow);
        check(&format!("{id} {vals:?} {depth:?} half"), compare_at(&s, RenderOpts { scale: 0.5, ..opts() }, t), allow);
    }
}

#[test]
fn blurs() {
    effect_case("ec.blur.cccross", &[]);
    effect_case("ec.blur.cccross", &[("radiusX", n(4.0)), ("radiusY", n(0.0)), ("transferMode", e(1))]);
    effect_case("ec.blur.cccross", &[("radiusX", n(3.0)), ("radiusY", n(7.0)), ("transferMode", e(2))]);
    effect_case("ec.blur.cccross", &[("radiusX", n(6.0)), ("radiusY", n(2.0)), ("transferMode", e(3))]);
    effect_case("ec.blur.ccradial", &[]);
    effect_case("ec.blur.ccradial", &[("type", e(0)), ("amount", n(-40.0)), ("center", pt(20.0, 30.0))]);
    effect_case("ec.blur.ccradial", &[("type", e(2)), ("amount", n(60.0)), ("quality", n(20.0))]);
    effect_case("ec.blur.ccradial", &[("type", e(3)), ("amount", n(30.0))]);
    effect_case("ec.blur.ccradial", &[("type", e(4)), ("amount", n(-50.0)), ("quality", n(80.0))]);
    effect_case("ec.blur.ccvector", &[]);
    effect_case("ec.blur.ccvector", &[("type", e(1)), ("amount", n(6.0)), ("angleOffset", n(30.0))]);
    effect_case("ec.blur.ccvector", &[("type", e(2)), ("mapSoftness", n(0.0))]);
    effect_case("ec.blur.ccvector", &[("type", e(3)), ("amount", n(5.0))]);
    effect_case("ec.blur.ccvector", &[("type", e(4)), ("amount", n(12.0)), ("mapSoftness", n(40.0))]);
    effect_case("ec.blur.reduceflicker", &[("softness", n(3.0))]);
    effect_case("ec.blur.reduceflicker", &[("softness", n(0.5))]);
}

#[test]
fn composites_and_colour() {
    for m in [0, 1, 2, 3, 5, 8, 11, 14, 15] {
        effect_case("ec.channel.cccomposite", &[("compositeOriginal", e(m)), ("opacity", n(70.0))]);
    }
    effect_case("ec.channel.cccomposite", &[("compositeOriginal", e(4)), ("rgbOnly", on())]);
    effect_case("ec.color.colorlink", &[]);
    effect_case("ec.color.colorlink", &[("sampleSource", e(2)), ("clip", n(10.0)), ("blendingMode", e(3)), ("opacity", n(60.0))]);
    effect_case("ec.color.colorlink", &[("sampleSource", e(1)), ("blendingMode", e(11)), ("stencilOriginalAlpha", off())]);
    effect_case("ec.color.colorlink", &[("sampleSource", e(9)), ("clip", n(20.0))]);
    effect_case("ec.color.colorlink", &[("sampleSource", e(6)), ("opacity", n(50.0)), ("stencilOriginalAlpha", off())]);
    env_case("ec.color.colorlink", &[("sourceLayer", Value::Layer(Some(9))), ("sampleSource", e(4)), ("clip", n(5.0))]);
    effect_case("ec.utility.cineon", &[]);
    effect_case("ec.utility.cineon", &[("conversionType", e(0)), ("gamma", n(2.2))]);
    effect_case("ec.utility.cineon", &[("conversionType", e(2)), ("highlightRolloff", n(60.0)), ("tenBitWhitePoint", n(800.0))]);
    effect_case("ec.utility.hdrcompander", &[("gain", n(4.0)), ("gamma", n(2.0))]);
    effect_case("ec.utility.hdrcompander", &[("mode", e(1)), ("gain", n(3.0)), ("gamma", n(1.5))]);
    effect_case("ec.utility.hdrcompression", &[]);
    effect_case("ec.utility.hdrcompression", &[("amount", n(40.0))]);
    effect_case("ec.utility.growbounds", &[("pixels", n(9.0))]);
    effect_case("ec.utility.ccoverbrights", &[]);
    for ch in 1..=5 {
        effect_case("ec.utility.ccoverbrights", &[("channel", e(ch)), ("highlightColor", c(0.2, 0.9, 0.4))]);
    }
}

#[test]
fn stylize() {
    let _flips = allow_llvmpipe_boundary_flips();
    for (done, flags) in
        [(30.0, [true, true, true, false]), (55.0, [false, true, false, true]), (70.0, [true, false, false, false]), (10.0, [false, false, true, true])]
    {
        effect_case(
            "ec.stylize.ccblockload",
            &[
                ("completion", n(done)),
                ("scanlines", Value::Bool(flags[0])),
                ("smoothing", Value::Bool(flags[1])),
                ("startCleared", Value::Bool(flags[2])),
                ("bilinear", Value::Bool(flags[3])),
            ],
        );
    }
    effect_case("ec.stylize.ccburnfilm", &[("burn", n(30.0))]);
    effect_case("ec.stylize.ccburnfilm", &[("burn", n(60.0)), ("center", pt(20.0, 15.0)), ("randomSeed", n(4.0))]);
    effect_case("ec.stylize.ccglass", &[]);
    effect_case("ec.stylize.ccglass", &[("lightType", e(1)), ("displacement", n(-60.0)), ("property", e(4)), ("softness", n(4.0))]);
    effect_case("ec.stylize.ccglass", &[("height", n(0.0)), ("lightColor", c(1.0, 0.8, 0.5)), ("metal", n(60.0))]);
    effect_case("ec.stylize.ccplastic", &[]);
    effect_case("ec.stylize.ccplastic", &[("lightType", e(1)), ("cutMin", n(20.0)), ("cutMax", n(70.0)), ("property", e(5))]);
    env_case("ec.stylize.ccplastic", &[("bumpLayer", Value::Layer(Some(9))), ("property", e(0)), ("roughness", n(0.2))]);
    effect_case("ec.stylize.cchextile", &[("radius", n(9.3))]);
    effect_case("ec.stylize.cchextile", &[("radius", n(7.7)), ("render", e(1)), ("rotate", n(17.0)), ("smearing", n(30.0))]);
    effect_case("ec.stylize.cchextile", &[("radius", n(12.1)), ("render", e(2)), ("center", pt(31.3, 19.7))]);
    effect_case("ec.stylize.ccmrsmoothie", &[]);
    effect_case("ec.stylize.ccmrsmoothie", &[("property", e(6)), ("smoothness", n(5.0)), ("colorLoop", e(2)), ("phase", n(40.0))]);
    env_case("ec.stylize.ccmrsmoothie", &[("flowLayer", Value::Layer(Some(9))), ("property", e(1))]);
}

#[test]
fn keys_and_3d() {
    env_case("ec.key.innerouter", &[]);
    env_case("ec.key.innerouter", &[("foreground", e(1))]);
    direct_env("ec.key.innerouter", &[("foreground", e(1)), ("background", e(2))], 0.0);
    env_case("ec.key.innerouter", &[("foreground", e(1)), ("background", e(2)), ("edgeFeather", n(3.0)), ("edgeThin", n(1.5))]);
    env_case("ec.key.innerouter", &[("foreground", e(1)), ("singleMaskHighlightRadius", n(4.0)), ("invertExtraction", on()), ("blendWithOriginal", n(20.0))]);
    env_case(
        "ec.key.innerouter",
        &[
            ("foreground", e(2)),
            ("edgeThin", n(-2.3)),
            ("edgeThreshold", n(30.0)),
            ("cleanupForeground/cleanupForeground1/path", e(3)),
            ("cleanupBackground/cleanupBackground1/path", e(1)),
        ],
    );
    effect_case("ec.key.ccsimplewireremoval", &[]);
    effect_case("ec.key.ccsimplewireremoval", &[("removalStyle", e(0)), ("thickness", n(8.0)), ("slope", n(50.0))]);
    effect_case("ec.key.ccsimplewireremoval", &[("removalStyle", e(1)), ("mirrorBlend", n(60.0)), ("pointB", pt(60.0, 10.0))]);
    effect_case("ec.key.ccsimplewireremoval", &[("removalStyle", e(1)), ("frameOffset", n(-2.0)), ("thickness", n(6.0))]);
    effect_case("ec.key.ccsimplewireremoval", &[("removalStyle", e(3)), ("mirrorBlend", n(30.0)), ("slope", n(20.0))]);
    effect_case("ec.obsolete.basic3d", &[("swivel", n(25.0))]);
    effect_case("ec.obsolete.basic3d", &[("tilt", n(-30.0)), ("swivel", n(10.0)), ("specularHighlight", on())]);
    effect_case("ec.obsolete.basic3d", &[("distanceToImage", n(20.0)), ("preview/drawPreviewWireframe", on()), ("tilt", n(15.0))]);
}
