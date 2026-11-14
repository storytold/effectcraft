//! Channel effects, batch 2: layer-to-layer combinations (Blend, Calculations, Compound
//! Arithmetic) and Source Composite. Second layers come from layer parameters through the effect
//! host; with no layer chosen the effect uses the layer itself, as After Effects does.

use effectcraft_color::{BlendMode, blend_pixel, hsl_to_rgb, luminance, rgb_to_hsl};
use effectcraft_keyframe::Value;
use effectcraft_project::ParamUi;
use rayon::prelude::*;

use crate::util::{layer_or_self, lerp4, premul, unpremul};
use crate::{Buf, EffectCtx, EffectSpec, num, p, popup, slider};

fn spec(id: &'static str, name: &'static str, params: Vec<crate::ParamSpec>, render: crate::RenderFn) -> EffectSpec {
    EffectSpec { id, name, category: "Channel", params, render, gpu: false, float: true }
}

fn pct() -> ParamUi {
    slider(0.0, 100.0, 0.0, 100.0, 1)
}

fn layer_param(id: &'static str, name: &'static str) -> crate::ParamSpec {
    p(id, name, Value::Layer(None), ParamUi::Layer)
}

const SIZE_DIFF: [&str; 2] = ["Center", "Stretch to Fit"];

// ---------------------------------------------------------------- Blend

fn blend(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let mode = ctx.params.e("mode");
    let orig = (ctx.params.f("blendWithOriginal") as f32 / 100.0).clamp(0.0, 1.0);
    if orig >= 1.0 {
        return b;
    }
    let other = layer_or_self(ctx, &b, "blendWithLayer", true, ctx.params.e("ifLayerSizesDiffer") == 1);
    b.img.data.par_iter_mut().zip(other.data.par_iter()).for_each(|(px, &o)| {
        let src = *px;
        let blended = match mode {
            // Crossfade: the other layer replaces this one.
            0 => o,
            // Color Only / Tint Only: hue (and saturation) from the other layer, lightness kept.
            1 | 2 => {
                let (c, a) = unpremul(src);
                let (oc, oa) = unpremul(o);
                if a <= 0.0 || oa <= 0.0 {
                    src
                } else {
                    let (_, s, l) = rgb_to_hsl(c[0], c[1], c[2]);
                    let (oh, os, _) = rgb_to_hsl(oc[0], oc[1], oc[2]);
                    let (h2, s2) = if mode == 1 {
                        (oh, os)
                    } else if s > 1e-4 {
                        (oh, s)
                    } else {
                        (0.0, 0.0)
                    };
                    let (r, g, bb) = hsl_to_rgb(h2, s2, l);
                    let t = oa;
                    lerp4(src, premul([r, g, bb], a), t)
                }
            }
            3 => [src[0].min(o[0]), src[1].min(o[1]), src[2].min(o[2]), src[3]],
            _ => [src[0].max(o[0]), src[1].max(o[1]), src[2].max(o[2]), src[3].max(o[3])],
        };
        *px = lerp4(blended, src, orig);
    });
    b
}

// ---------------------------------------------------------------- Calculations

/// Blending modes offered by Calculations / Source Composite (menu order).
const CALC_MODES: [BlendMode; 22] = [
    BlendMode::Normal,
    BlendMode::Darken,
    BlendMode::Multiply,
    BlendMode::ColorBurn,
    BlendMode::LinearBurn,
    BlendMode::Add,
    BlendMode::Lighten,
    BlendMode::Screen,
    BlendMode::ColorDodge,
    BlendMode::LinearDodge,
    BlendMode::Overlay,
    BlendMode::SoftLight,
    BlendMode::HardLight,
    BlendMode::LinearLight,
    BlendMode::VividLight,
    BlendMode::PinLight,
    BlendMode::HardMix,
    BlendMode::Difference,
    BlendMode::Exclusion,
    BlendMode::Hue,
    BlendMode::Saturation,
    BlendMode::Luminosity,
];

fn calc_mode_names() -> Vec<&'static str> {
    CALC_MODES.iter().map(|m| m.label()).collect()
}

const CHANNELS: [&str; 6] = ["RGBA", "Gray", "Red", "Green", "Blue", "Alpha"];

/// Extract channel `ch` (index into [`CHANNELS`]) of a premultiplied pixel as a premultiplied
/// pixel: RGBA unchanged, single channels as opaque grey (Alpha as grey too).
#[inline]
fn extract(px: [f32; 4], ch: u32, invert: bool) -> [f32; 4] {
    let (c, a) = unpremul(px);
    let g = match ch {
        0 => {
            if invert {
                return premul([1.0 - c[0], 1.0 - c[1], 1.0 - c[2]], a);
            }
            return px;
        }
        1 => luminance(c[0], c[1], c[2]) * a,
        2 => c[0] * a,
        3 => c[1] * a,
        4 => c[2] * a,
        _ => a,
    };
    let g = if invert { 1.0 - g } else { g };
    [g, g, g, 1.0]
}

fn calculations(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let in_ch = ctx.params.e("inputChannel");
    let in_inv = ctx.params.b("invertInput");
    let second = ctx.params.get("secondLayer").and_then(Value::as_layer).is_some();
    let sec_ch = ctx.params.e("secondLayerChannel");
    let sec_op = (ctx.params.f("secondLayerOpacity") as f32 / 100.0).clamp(0.0, 1.0);
    let sec_inv = ctx.params.b("invertSecondLayer");
    let mode = CALC_MODES[(ctx.params.e("blendingMode") as usize).min(CALC_MODES.len() - 1)];
    let keep_alpha = ctx.params.b("preserveTransparency");
    let other = if second { Some(layer_or_self(ctx, &b, "secondLayer", true, ctx.params.b("stretchSecondLayerToFit"))) } else { None };
    b.img.data.par_iter_mut().enumerate().for_each(|(i, px)| {
        let orig_a = px[3];
        let mut o = extract(*px, in_ch, in_inv);
        if let Some(other) = &other {
            let s = extract(other.data[i], sec_ch, sec_inv).map(|v| v * sec_op);
            o = blend_pixel(mode, o, s, 0.5);
        }
        if keep_alpha && o[3] > 1e-6 {
            let (c, _) = unpremul(o);
            o = premul(c, orig_a);
        }
        o[3] = o[3].clamp(0.0, 1.0);
        *px = o;
    });
    b
}

// ---------------------------------------------------------------- Source Composite

const CC_COMPOSITE_MODES: [&str; 16] = [
    "In Front",
    "In Back",
    "Add",
    "Multiply",
    "Screen",
    "Overlay",
    "Soft Light",
    "Hard Light",
    "Darken",
    "Lighten",
    "Difference",
    "Hue",
    "Saturation",
    "Color",
    "Luminosity",
    "Stencil Alpha",
];

/// Composites the original stack input over/under/with the result of preceding effects.
fn cc_composite(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let op = (ctx.params.f("opacity") as f32 / 100.0).clamp(0.0, 1.0);
    let m = ctx.params.e("compositeOriginal");
    let rgb_only = ctx.params.b("rgbOnly");
    let mode = match m {
        2 => BlendMode::Add,
        3 => BlendMode::Multiply,
        4 => BlendMode::Screen,
        5 => BlendMode::Overlay,
        6 => BlendMode::SoftLight,
        7 => BlendMode::HardLight,
        8 => BlendMode::Darken,
        9 => BlendMode::Lighten,
        10 => BlendMode::Difference,
        11 => BlendMode::Hue,
        12 => BlendMode::Saturation,
        13 => BlendMode::Color,
        14 => BlendMode::Luminosity,
        15 => BlendMode::StencilAlpha,
        _ => BlendMode::Normal,
    };
    let original = ctx.env.host.and_then(|h| h.original()).map(|o| crate::util::fit_buffer(o, b.img.width, b.img.height, b.offset, b.scale));
    b.img.data.par_iter_mut().enumerate().for_each(|(i, px)| {
        let result = *px;
        let orig = original.as_ref().and_then(|o| o.data.get(i)).copied().unwrap_or(result).map(|v| v * op);
        let mut o = if m == 1 { blend_pixel(BlendMode::Normal, orig, result, 0.5) } else { blend_pixel(mode, result, orig, 0.5) };
        o[3] = o[3].clamp(0.0, 1.0);
        if rgb_only {
            let (c, _) = unpremul(o);
            o = premul(c, result[3]);
        }
        *px = o;
    });
    b
}

// ---------------------------------------------------------------- Compound Arithmetic

const OPERATORS: [&str; 15] =
    ["Copy", "Add", "Subtract", "Multiply", "Difference", "And", "Or", "Xor", "Lighten", "Darken", "Minimum", "Maximum", "Screen", "Overlay", "Hard Light"];

/// One channel operation on straight values in 0..1 (may overflow).
#[inline]
fn arith(op: u32, a: f32, b: f32) -> f32 {
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
    match op {
        0 => b,
        1 => a + b,
        2 => a - b,
        3 => a * b,
        4 => (a - b).abs(),
        5 => (q(a) & q(b)) as f32 / 255.0,
        6 => (q(a) | q(b)) as f32 / 255.0,
        7 => (q(a) ^ q(b)) as f32 / 255.0,
        8 | 11 => a.max(b),
        9 | 10 => a.min(b),
        12 => a + b - a * b,
        13 => {
            if a < 0.5 {
                2.0 * a * b
            } else {
                1.0 - 2.0 * (1.0 - a) * (1.0 - b)
            }
        }
        _ => {
            if b < 0.5 {
                2.0 * a * b
            } else {
                1.0 - 2.0 * (1.0 - a) * (1.0 - b)
            }
        }
    }
}

/// Overflow handling: 0 Clip, 1 Wrap, 2 Scale (map the operator's full range to 0..1).
#[inline]
fn overflow(op: u32, v: f32, how: u32) -> f32 {
    match how {
        0 => v.clamp(0.0, 1.0),
        1 => v.rem_euclid(1.0001).min(1.0),
        _ => match op {
            1 => v * 0.5,
            2 => (v + 1.0) * 0.5,
            _ => v.clamp(0.0, 1.0),
        },
    }
}

fn compound_arithmetic(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let op = ctx.params.e("operator");
    let chans = ctx.params.e("operateOnChannels");
    let how = ctx.params.e("overflowBehavior");
    let orig = (ctx.params.f("blendWithOriginal") as f32 / 100.0).clamp(0.0, 1.0);
    if orig >= 1.0 {
        return b;
    }
    let other = layer_or_self(ctx, &b, "secondSource", true, ctx.params.b("stretchSecondSourceToFit"));
    b.img.data.par_iter_mut().zip(other.data.par_iter()).for_each(|(px, &o)| {
        let src = *px;
        let (c, a) = unpremul(src);
        let (oc, oa) = unpremul(o);
        let mut rc = c;
        let mut ra = a;
        if chans != 2 {
            for k in 0..3 {
                rc[k] = overflow(op, arith(op, c[k], oc[k]), how);
            }
        }
        if chans != 0 {
            ra = overflow(op, arith(op, a, oa), how);
        }
        let res = premul(rc, ra);
        *px = lerp4(res, src, orig);
    });
    b
}

pub fn specs() -> Vec<EffectSpec> {
    let calc_modes = calc_mode_names();
    vec![
        spec(
            "ec.channel.blend",
            "Blend",
            vec![
                layer_param("blendWithLayer", "Blend With Layer"),
                p("mode", "Mode", Value::Enum(0), popup(&["Crossfade", "Color Only", "Tint Only", "Darken Only", "Lighten Only"])),
                p("blendWithOriginal", "Blend With Original", num(0.0), pct()),
                p("ifLayerSizesDiffer", "If Layer Sizes Differ", Value::Enum(0), popup(&SIZE_DIFF)),
            ],
            blend,
        ),
        spec(
            "ec.channel.calculations",
            "Calculations",
            vec![
                p("inputChannel", "Input Channel", Value::Enum(0), popup(&CHANNELS)),
                p("invertInput", "Invert Input", Value::Bool(false), ParamUi::Checkbox),
                layer_param("secondLayer", "Second Layer"),
                p("secondLayerChannel", "Second Layer Channel", Value::Enum(0), popup(&CHANNELS)),
                p("secondLayerOpacity", "Second Layer Opacity", num(100.0), pct()),
                p("invertSecondLayer", "Invert Second Layer", Value::Bool(false), ParamUi::Checkbox),
                p("stretchSecondLayerToFit", "Stretch Second Layer to Fit", Value::Bool(true), ParamUi::Checkbox),
                p("blendingMode", "Blending Mode", Value::Enum(0), ParamUi::Popup { options: calc_modes.iter().map(|s| s.to_string()).collect() }),
                p("preserveTransparency", "Preserve Transparency", Value::Bool(false), ParamUi::Checkbox),
            ],
            calculations,
        ),
        spec(
            "ec.channel.cccomposite",
            "Source Composite",
            vec![
                p("opacity", "Opacity", num(100.0), pct()),
                p("compositeOriginal", "Composite Original", Value::Enum(0), popup(&CC_COMPOSITE_MODES)),
                p("rgbOnly", "RGB Only", Value::Bool(false), ParamUi::Checkbox),
            ],
            cc_composite,
        ),
        spec(
            "ec.channel.compoundarithmetic",
            "Compound Arithmetic",
            vec![
                layer_param("secondSource", "Second Source Layer"),
                p("operator", "Operator", Value::Enum(0), popup(&OPERATORS)),
                p("operateOnChannels", "Operate On Channels", Value::Enum(0), popup(&["RGB", "ARGB", "Alpha"])),
                p("overflowBehavior", "Overflow Behavior", Value::Enum(0), popup(&["Clip", "Wrap", "Scale"])),
                p("stretchSecondSourceToFit", "Stretch Second Source to Fit", Value::Bool(true), ParamUi::Checkbox),
                p("blendWithOriginal", "Blend With Original", num(0.0), pct()),
            ],
            compound_arithmetic,
        ),
    ]
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::{EffectEnv, EffectHost, LayerPixels, run_fx};
    use effectcraft_raster::Image;

    /// A host that knows one other layer (id 7).
    pub(crate) struct FakeHost(pub Image);
    impl EffectHost for FakeHost {
        fn layer(&self, id: u64, _: bool) -> Option<LayerPixels> {
            (id == 7).then(|| LayerPixels { buf: Buf { img: self.0.clone(), offset: [0.0; 2], scale: 1.0 }, size: [self.0.width as f64, self.0.height as f64] })
        }
        fn audio(&self, _: u64, _: f64, _: usize, _: u32) -> Option<Vec<f32>> {
            None
        }
    }

    pub(crate) fn sample() -> Image {
        let mut img = Image::new(8, 6);
        for y in 0..6 {
            for x in 0..8 {
                let a = if x < 6 { 1.0 } else { 0.5 };
                img.set(x, y, [x as f32 / 8.0 * a, y as f32 / 6.0 * a, 0.4 * a, a]);
            }
        }
        img
    }

    fn close(a: &Image, b: &Image, tol: f32) -> bool {
        a.data.iter().zip(b.data.iter()).all(|(p, q)| p.iter().zip(q.iter()).all(|(x, y)| (x - y).abs() <= tol))
    }

    fn with_host<R>(other: Image, f: impl FnOnce(EffectEnv) -> R) -> R {
        let host = FakeHost(other);
        f(EffectEnv { host: Some(&host), ..Default::default() })
    }

    #[test]
    fn blend_self_crossfade_is_identity_and_layer_crossfades() {
        let img = sample();
        let out = run_fx("ec.channel.blend", &[], img.clone(), 0.0, EffectEnv::default());
        assert!(close(&out.img, &img, 1e-6));
        let red = Image::filled(8, 6, [1.0, 0.0, 0.0, 1.0]);
        let out = with_host(red, |env| {
            run_fx("ec.channel.blend", &[("blendWithLayer", Value::Layer(Some(7))), ("blendWithOriginal", num(50.0))], img.clone(), 0.0, env)
        });
        let p = out.img.get(0, 0);
        assert!((p[0] - 0.5).abs() < 1e-5 && (p[3] - 1.0).abs() < 1e-5);
        let again = with_host(Image::filled(8, 6, [1.0, 0.0, 0.0, 1.0]), |env| {
            run_fx("ec.channel.blend", &[("blendWithLayer", Value::Layer(Some(7))), ("blendWithOriginal", num(50.0))], img.clone(), 0.0, env)
        });
        assert_eq!(out.img.data, again.img.data);
    }

    #[test]
    fn calculations_defaults_identity_and_gray() {
        let img = sample();
        let out = run_fx("ec.channel.calculations", &[], img.clone(), 0.0, EffectEnv::default());
        assert!(close(&out.img, &img, 1e-6));
        let g = run_fx("ec.channel.calculations", &[("inputChannel", Value::Enum(2))], img.clone(), 0.0, EffectEnv::default());
        let p = g.img.get(4, 2);
        assert!((p[0] - 0.5).abs() < 1e-5 && p[0] == p[1] && p[1] == p[2]);
        let m = with_host(Image::filled(8, 6, [0.5, 0.5, 0.5, 1.0]), |env| {
            run_fx("ec.channel.calculations", &[("secondLayer", Value::Layer(Some(7))), ("blendingMode", Value::Enum(2))], img.clone(), 0.0, env)
        });
        let q = m.img.get(4, 0);
        assert!((q[0] - 0.25).abs() < 1e-4, "{q:?}");
    }

    #[test]
    fn cc_composite_in_front_is_identity_on_opaque() {
        let img = Image::filled(4, 4, [0.3, 0.6, 0.2, 1.0]);
        let out = run_fx("ec.channel.cccomposite", &[], img.clone(), 0.0, EffectEnv::default());
        assert!(close(&out.img, &img, 1e-6));
        let add = run_fx("ec.channel.cccomposite", &[("compositeOriginal", Value::Enum(2))], img.clone(), 0.0, EffectEnv::default());
        assert!((add.img.get(0, 0)[0] - 0.6).abs() < 1e-5);
    }

    #[test]
    fn compound_arithmetic_copy_self_identity_and_add() {
        let img = sample();
        let out = run_fx("ec.channel.compoundarithmetic", &[], img.clone(), 0.0, EffectEnv::default());
        assert!(close(&out.img, &img, 1e-5));
        let other = Image::filled(8, 6, [0.25, 0.25, 0.25, 1.0]);
        let add = with_host(other, |env| {
            run_fx("ec.channel.compoundarithmetic", &[("secondSource", Value::Layer(Some(7))), ("operator", Value::Enum(1))], img.clone(), 0.0, env)
        });
        let p = add.img.get(4, 3);
        assert!((p[0] - (0.5 + 0.25)).abs() < 1e-5 && (p[1] - 0.75).abs() < 1e-5, "{p:?}");
        let sc = with_host(Image::filled(8, 6, [1.0, 1.0, 1.0, 1.0]), |env| {
            run_fx(
                "ec.channel.compoundarithmetic",
                &[("secondSource", Value::Layer(Some(7))), ("operator", Value::Enum(1)), ("overflowBehavior", Value::Enum(2))],
                img.clone(),
                0.0,
                env,
            )
        });
        assert!((sc.img.get(4, 3)[0] - 0.75).abs() < 1e-5);
    }
}

#[cfg(test)]
mod original_tests {
    use super::*;
    use crate::{EffectEnv, EffectHost, Image, LayerPixels};
    struct Original(Buf);
    impl EffectHost for Original {
        fn original(&self) -> Option<&Buf> {
            Some(&self.0)
        }
        fn layer(&self, _: u64, _: bool) -> Option<LayerPixels> {
            None
        }
        fn audio(&self, _: u64, _: f64, _: usize, _: u32) -> Option<Vec<f32>> {
            None
        }
    }
    #[test]
    fn composite_preserves_alpha_opacity_and_padded_alignment() {
        let spec = crate::find("ec.channel.cccomposite").unwrap();
        let mut params = crate::Params { values: spec.params.iter().map(|p| (p.id.to_string(), p.default.clone())).collect() };
        let host = Original(Buf { img: Image::filled(1, 1, [0.5, 0.0, 0.0, 0.5]), offset: [0.0; 2], scale: 1.0 });
        for (mode, opacity, rgb, want) in [
            (0, 100.0, false, [0.5, 0.0, 0.125, 0.625]),
            (1, 100.0, false, [0.375, 0.0, 0.25, 0.625]),
            (0, 50.0, false, [0.25, 0.0, 0.1875, 0.4375]),
            (0, 100.0, true, [0.2, 0.0, 0.05, 0.25]),
        ] {
            params.values.insert("compositeOriginal".into(), Value::Enum(mode));
            params.values.insert("opacity".into(), num(opacity));
            params.values.insert("rgbOnly".into(), Value::Bool(rgb));
            let ctx = EffectCtx {
                params: &params,
                time: 0.0,
                layer_size: [1.0; 2],
                seed: 0,
                adjustment: false,
                env: EffectEnv { host: Some(&host), ..Default::default() },
            };
            let mut input = Buf { img: Image::filled(1, 1, [0.0, 0.0, 0.25, 0.25]), offset: [0.0; 2], scale: 1.0 };
            input.pad(1);
            let out = cc_composite(&ctx, input);
            for (a, b) in out.img.get(1, 1).iter().zip(want) {
                assert!((a - b).abs() < 1e-6, "{:?} vs {want:?}", out.img.get(1, 1));
            }
            assert_eq!(out.img.get(0, 0), [0.0; 4]);
            assert_eq!(out.offset, [1.0; 2]);
        }
    }
}
