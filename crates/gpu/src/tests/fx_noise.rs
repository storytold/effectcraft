//! GPU noise, blur and time family effects vs the CPU effects (the oracle): direct on a buffer at full and half
//! resolution, as adjustment, and composited at 8 and 32 bpc.

use effectcraft_color::Label;
use effectcraft_keyframe::{Keyframe, Value};
use effectcraft_project::build;
use effectcraft_project::{BitDepth, Comp, ItemKind, LayerSource};
use effectcraft_render::RenderOpts;
use effectcraft_time::{FrameRate, Tick};

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
fn median_kernel_selection_preserves_physical_gpu_path() {
    use crate::fx_noise::MedianKernel;
    for backend in [wgpu::Backend::Vulkan, wgpu::Backend::Metal, wgpu::Backend::Dx12, wgpu::Backend::Gl, wgpu::Backend::BrowserWebGpu] {
        for device_type in
            [wgpu::DeviceType::DiscreteGpu, wgpu::DeviceType::IntegratedGpu, wgpu::DeviceType::VirtualGpu, wgpu::DeviceType::Cpu, wgpu::DeviceType::Other]
        {
            let selected = MedianKernel::for_adapter(backend, device_type);
            let expected =
                if backend == wgpu::Backend::Vulkan && device_type == wgpu::DeviceType::Cpu { MedianKernel::Software2 } else { MedianKernel::Private64 };
            assert_eq!(selected, expected, "{backend:?} {device_type:?}");
        }
    }
    assert_eq!(MedianKernel::Private64.rows_per_workgroup(), 64);
    assert_eq!(MedianKernel::Software2.rows_per_workgroup(), 2);
    assert_eq!(MedianKernel::Software2.workgroup_storage(), 16_384);
}

#[test]
fn medians() {
    effect_case("ec.noise.median", &[("radius", n(2.0))]);
    effect_case("ec.noise.median", &[("radius", n(5.0)), ("operateOnAlpha", on())]);
    effect_case("ec.noise.medianlegacy", &[("radius", n(3.0))]);
    effect_case("ec.noise.medianlegacy", &[("radius", n(2.0)), ("operateOnAlphaChannel", on())]);
    effect_case("ec.noise.dustscratches", &[("radius", n(3.0)), ("threshold", n(10.0))]);
    effect_case("ec.noise.dustscratches", &[("radius", n(2.0)), ("threshold", n(0.0)), ("operateOnAlpha", on())]);
}

#[test]
fn large_radius_medians() {
    // Past the bisection radius: the sliding-histogram kernel (windows larger than the layer).
    effect_case("ec.noise.median", &[("radius", n(9.0))]);
    effect_case("ec.noise.median", &[("radius", n(24.0)), ("operateOnAlpha", on())]);
    effect_case("ec.noise.medianlegacy", &[("radius", n(17.0))]);
    effect_case("ec.noise.medianlegacy", &[("radius", n(40.0)), ("operateOnAlphaChannel", on())]);
    effect_case("ec.noise.dustscratches", &[("radius", n(20.0)), ("threshold", n(10.0))]);
    effect_case("ec.noise.dustscratches", &[("radius", n(60.0)), ("threshold", n(0.0)), ("operateOnAlpha", on())]);
}

#[test]
fn edge_preserving_blurs() {
    effect_case("ec.blur.bilateral", &[]);
    effect_case("ec.blur.bilateral", &[("radius", n(9.0)), ("threshold", n(30.0)), ("colorize", Value::Bool(false))]);
    effect_case("ec.blur.bilateral", &[("radius", n(14.0)), ("threshold", n(5.0))]);
    effect_case("ec.blur.smart", &[]);
    effect_case("ec.blur.smart", &[("radius", n(6.0)), ("threshold", n(15.0)), ("mode", e(1))]);
    effect_case("ec.blur.smart", &[("radius", n(4.0)), ("threshold", n(40.0)), ("mode", e(2))]);
}

#[test]
fn sharpen_and_unsharp_mask() {
    effect_case("ec.blur.sharpen", &[("amount", n(60.0))]);
    effect_case("ec.blur.sharpen", &[("amount", n(250.0))]);
    effect_case("ec.blur.unsharp", &[("amount", n(120.0)), ("radius", n(4.0))]);
    effect_case("ec.blur.unsharp", &[("amount", n(80.0)), ("radius", n(2.5)), ("threshold", n(12.0))]);
}

#[test]
fn channel_and_compound_blur() {
    effect_case("ec.blur.channel", &[("redBlurriness", n(8.0)), ("blueBlurriness", n(3.0))]);
    effect_case("ec.blur.channel", &[("greenBlurriness", n(6.0)), ("alphaBlurriness", n(6.0)), ("repeatEdge", on())]);
    effect_case("ec.blur.channel", &[("redBlurriness", n(5.0)), ("alphaBlurriness", n(10.0)), ("dimensions", e(1))]);
    effect_case("ec.blur.compound", &[]);
    effect_case("ec.blur.compound", &[("maximumBlur", n(12.0)), ("invertBlur", on())]);
}

/// Compound Blur reading another layer (stretched and centred), composited.
#[test]
fn compound_blur_layer() {
    for depth in [BitDepth::Bpc8, BitDepth::Bpc32] {
        for stretch in [true, false] {
            let mut s = Scene::new(depth);
            let bg = s.solid([0.15, 0.1, 0.2], 97, 61);
            s.push(bg);
            let mut map = s.footage(50, 36);
            map.switches.video = false;
            let mid = s.push(map);
            let mut l = s.footage(70, 44);
            s.effect(
                &mut l,
                "ec.blur.compound",
                &[("blurLayer", Value::Layer(Some(mid.0))), ("maximumBlur", n(10.0)), ("stretchMapToFit", Value::Bool(stretch))],
            );
            set(&mut l, "transform/rotation", Value::Scalar(8.0));
            s.push(l);
            let label = format!("compound blur layer {stretch} {depth:?}");
            check(&label, compare_at(&s, opts(), Tick::ZERO), 0.0);
            check(&format!("{label} half"), compare_at(&s, RenderOpts { scale: 0.5, ..opts() }, Tick::ZERO), 0.0);
        }
    }
}

#[test]
fn minimax() {
    effect_case("ec.channel.minimax", &[("radius", n(3.0))]);
    effect_case("ec.channel.minimax", &[("radius", n(2.0)), ("operation", e(0)), ("channel", e(2))]);
    effect_case("ec.channel.minimax", &[("radius", n(4.0)), ("operation", e(2)), ("channel", e(1)), ("direction", e(1))]);
    effect_case("ec.channel.minimax", &[("radius", n(3.0)), ("operation", e(3)), ("channel", e(4)), ("dontShrinkEdges", on()), ("direction", e(2))]);
}

#[test]
fn turbulent_noise() {
    effect_case("ec.noise.turbulent", &[]);
    effect_case(
        "ec.noise.turbulent",
        &[("fractalType", e(3)), ("noiseType", e(3)), ("evolution", n(120.0)), ("evolutionOptions/turbulenceFactor", n(1.5)), ("blendingMode", e(5))],
    );
    effect_case(
        "ec.noise.turbulent",
        &[("fractalType", e(5)), ("noiseType", e(1)), ("complexity", n(3.5)), ("transform/scale", n(40.0)), ("overflow", e(1)), ("blendingMode", e(0))],
    );
    effect_case("ec.noise.turbulent", &[("fractalType", e(7)), ("noiseType", e(0)), ("transform/perspectiveOffset", on()), ("opacity", n(70.0))]);
}

#[test]
fn fractal_noise_other_modes() {
    effect_case("ec.noise.fractal", &[("noiseType", e(3)), ("fractalType", e(4)), ("blendingMode", e(3))]);
    effect_case(
        "ec.noise.fractal",
        &[
            ("fractalType", e(6)),
            ("evolutionOptions/cycleEvolution", on()),
            ("evolutionOptions/cycle", n(2.0)),
            ("evolution", n(500.0)),
            ("subSettings/subRotation", n(25.0)),
            ("subSettings/subOffset", pt(4.0, -3.0)),
        ],
    );
    effect_case("ec.noise.fractal", &[("fractalType", e(8)), ("noiseType", e(1)), ("blendingMode", e(12)), ("contrast", n(160.0)), ("invert", on())]);
}

#[test]
fn noise_alpha_and_hls() {
    effect_case("ec.noise.noisealpha", &[("amount", n(40.0))]);
    effect_case("ec.noise.noisealpha", &[("amount", n(70.0)), ("noise", e(3)), ("originalAlpha", e(2)), ("overflow", e(1)), ("noisePhase", n(100.0))]);
    effect_case(
        "ec.noise.noisealpha",
        &[("amount", n(60.0)), ("noise", e(2)), ("originalAlpha", e(3)), ("overflow", e(2)), ("noiseOptions/cycleNoise", on()), ("randomSeed", n(9.0))],
    );
    effect_case("ec.noise.noisehls", &[("hue", n(30.0)), ("lightness", n(20.0)), ("saturation", n(40.0))]);
    effect_case("ec.noise.noisehls", &[("noise", e(1)), ("lightness", n(50.0)), ("noisePhase", n(200.0))]);
    effect_case("ec.noise.noisehls", &[("noise", e(2)), ("hue", n(60.0)), ("saturation", n(25.0)), ("grainSize", n(3.0))]);
    effect_case("ec.noise.noisehlsauto", &[("noise", e(0)), ("hue", n(20.0)), ("lightness", n(30.0)), ("noiseAnimationSpeed", n(7.0))]);
}

#[test]
fn remove_grain() {
    effect_case("ec.noise.removegrain", &[]);
    effect_case(
        "ec.noise.removegrain",
        &[
            ("noiseReductionSettings/noiseReduction", n(2.5)),
            ("noiseReductionSettings/passes", n(2.0)),
            ("noiseReductionSettings/mode", e(1)),
            ("fineTuning/texture", n(0.3)),
        ],
    );
    effect_case("ec.noise.removegrain", &[("unsharpMask/amount", n(80.0)), ("unsharpMask/radius", n(2.0)), ("unsharpMask/threshold", n(4.0))]);
    effect_case("ec.noise.removegrain", &[("viewingMode", e(0)), ("previewRegion/showBox", on()), ("previewRegion/boxColor", c(1.0, 0.2, 0.1))]);
}

/// Echo and Posterize Time fetch other frames through the effect host: a layer whose rotation
/// is keyframed, at a nonzero time (the direct cases have no host and pass the layer through).
#[test]
fn echo_and_posterize_time() {
    effect_case("ec.time.echo", &[]);
    effect_case("ec.time.posterizetime", &[]);
    for depth in [BitDepth::Bpc8, BitDepth::Bpc32] {
        for (id, vals) in [
            ("ec.time.echo", vec![("numberOfEchoes", n(3.0)), ("decay", n(0.7)), ("echoTime", n(-0.1))]),
            ("ec.time.echo", vec![("numberOfEchoes", n(4.0)), ("echoOperator", e(2)), ("echoTime", n(0.05))]),
            ("ec.time.echo", vec![("numberOfEchoes", n(2.0)), ("echoOperator", e(4)), ("startingIntensity", n(0.8))]),
            ("ec.time.echo", vec![("numberOfEchoes", n(5.0)), ("echoOperator", e(6))]),
            ("ec.time.posterizetime", vec![("frameRate", n(4.0))]),
        ] {
            let mut s = Scene::new(depth);
            let bg = s.solid([0.15, 0.1, 0.2], 97, 61);
            s.push(bg);
            // A precomp whose content moves, so the frames differ.
            let inner = Comp::new(60, 40, FrameRate::FPS_30, Tick::from_seconds_f64(2.0));
            let inner_id = s.p.add_item("Inner", Label::Sandstone, None, ItemKind::Comp(inner.into()));
            let mut a = s.footage(30, 24);
            let pos = a.props.prop_mut("transform/position").unwrap();
            pos.keys = vec![Keyframe::new(Tick::ZERO, crate::tests::v3(10.0, 12.0)), Keyframe::new(Tick::from_seconds_f64(1.0), crate::tests::v3(50.0, 30.0))];
            let rot = a.props.prop_mut("transform/rotation").unwrap();
            rot.keys = vec![Keyframe::new(Tick::ZERO, Value::Scalar(0.0)), Keyframe::new(Tick::from_seconds_f64(1.0), Value::Scalar(90.0))];
            s.p.comp_mut(inner_id).unwrap().layers = vec![a].into();
            let mut l = build::layer(&mut s.p, &s.comp, "Inner", LayerSource::Comp { item: inner_id }, (60, 40), None);
            s.effect(&mut l, id, &vals);
            set(&mut l, "transform/rotation", Value::Scalar(8.0));
            s.push(l);
            let t = Tick::from_seconds_f64(0.4);
            let label = format!("{id} {vals:?} {depth:?}");
            check(&label, compare_at(&s, opts(), t), 0.0);
            check(&format!("{label} half"), compare_at(&s, RenderOpts { scale: 0.5, ..opts() }, t), 0.0);
        }
    }
}

#[test]
fn echo_after_star_burst_preserves_the_stars() {
    for depth in [BitDepth::Bpc8, BitDepth::Bpc32] {
        for operator in 0..7 {
            for posterize in [false, true] {
                let mut s = Scene::new(depth);
                let mut l = s.solid([1.0; 3], 97, 61);
                s.effect(&mut l, "ec.sim.ccstarburst", &[("gridSpacing", n(8.0)), ("size", n(40.0))]);
                s.effect(&mut l, "ec.time.echo", &[("numberOfEchoes", n(2.0)), ("echoTime", n(-0.1)), ("decay", n(0.6)), ("echoOperator", e(operator))]);
                if posterize {
                    s.effect(&mut l, "ec.time.posterizetime", &[("frameRate", n(4.0))]);
                }
                s.push(l);
                let t = Tick::from_seconds_f64(0.6);
                let label = format!("Star Burst + Echo {operator} posterize={posterize} {depth:?}");
                check(&label, compare_at(&s, opts(), t), 0.0);
                check(&format!("{label} half"), compare_at(&s, RenderOpts { scale: 0.5, ..opts() }, t), 0.0);
            }
        }
    }
}

#[test]
fn median_levels_match_cpu_division() {
    let Some(g) = crate::tests::gpu() else { return };
    let mut image = effectcraft_raster::Image::new(512, 1);
    for (i, pixel) in image.data.iter_mut().enumerate() {
        *pixel = [i as f32 / 511.0; 4];
    }
    let input = g.ctx.upload_image(&image).unwrap();
    for radius in [3, 5] {
        // A monotone row's median is its centre, including repeated edges. Exercise both
        // the bisection and sliding-histogram kernels at every quantised level.
        let mut enc = crate::context::Enc::new(&g.ctx);
        let started = std::time::Instant::now();
        let median = crate::fx_noise::median_image(&mut enc, &input, radius, false);
        let actual = enc.download(&median).unwrap_or_else(|| {
            panic!(
                "median download radius {radius}; elapsed={:?}, health={:?}, unavailable={:?}, transfers={:?}",
                started.elapsed(),
                g.ctx.check_health(),
                g.ctx.unavailable_kernels(),
                g.ctx.transfer_stats()
            )
        });
        for (level, (want, got)) in image.data.iter().zip(&actual.data).enumerate() {
            assert_eq!(got, want, "radius {radius}, level {level}");
        }
    }
}

#[test]
fn dust_strict_threshold_boundaries() {
    let Some(g) = crate::tests::gpu() else { return };
    let spec = effectcraft_effects::find("ec.noise.dustscratches").unwrap();
    let size = [7.0, 7.0];
    let mut params =
        effectcraft_effects::Params { values: spec.params.iter().map(|p| (p.id.to_string(), effectcraft_effects::default_value(p, size))).collect() };
    let exact = 63.75_f32;
    let half_colour = [128.0 / 511.0, 128.0 / 511.0, 128.0 / 511.0, 256.0 / 511.0];
    let cases = [
        // The NVIDIA failure: rounding 154 / 511 down changes the keep/replace decision.
        ([126.0 / 511.0, 60.0 / 511.0, 62.0 / 511.0, 154.0 / 511.0], [73.0 / 255.0, 32.0 / 255.0, 32.0 / 255.0, 77.0 / 255.0], 10.0, true),
        // Candidate red = 0.25, original red = 0.5. Equality must keep the original;
        // adjacent representable thresholds exercise both sides of the strict comparison.
        (half_colour, [0.5, 0.25, 0.25, 0.5], exact.next_down(), true),
        (half_colour, [0.5, 0.25, 0.25, 0.5], exact, false),
        (half_colour, [0.5, 0.25, 0.25, 0.5], exact.next_up(), false),
    ];
    for radius in [3.0, 5.0] {
        params.values.insert("radius".into(), n(radius));
        for (median, original, threshold, replaced) in cases {
            params.values.insert("threshold".into(), n(threshold as f64));
            let ctx = || effectcraft_effects::EffectCtx { params: &params, time: 0.0, layer_size: size, seed: 0, adjustment: false, env: Default::default() };
            let mut img = effectcraft_raster::Image::filled(7, 7, median);
            img.set(3, 3, original);
            let buf = effectcraft_effects::Buf { img, offset: [0.0; 2], scale: 1.0 };
            let cpu = (spec.render)(&ctx(), buf.clone());
            assert_eq!(cpu.img.data[24] != original, replaced, "CPU radius {radius}, threshold {threshold}");
            let started = std::time::Instant::now();
            let gpu = effectcraft_render::Accelerator::effects(g, &[effectcraft_render::FxStep { spec, ctx: ctx() }], &buf, None).unwrap_or_else(|| {
                panic!(
                    "dust radius {radius}, threshold {threshold}; elapsed={:?}, health={:?}, unavailable={:?}, transfers={:?}",
                    started.elapsed(),
                    g.ctx.check_health(),
                    g.ctx.unavailable_kernels(),
                    g.ctx.transfer_stats()
                )
            });
            assert_eq!(gpu.img.data[24], cpu.img.data[24], "radius {radius}, threshold {threshold}");
        }
    }
}

#[test]
fn dust_threshold_on_quantized_input() {
    let Some(g) = crate::tests::gpu() else { return };
    let spec = effectcraft_effects::find("ec.noise.dustscratches").unwrap();
    let size = [70.0, 44.0];
    let mut params =
        effectcraft_effects::Params { values: spec.params.iter().map(|p| (p.id.to_string(), effectcraft_effects::default_value(p, size))).collect() };
    params.values.insert("radius".into(), n(3.0));
    params.values.insert("threshold".into(), n(10.0));
    let ctx = || effectcraft_effects::EffectCtx { params: &params, time: 0.25, layer_size: size, seed: 11, adjustment: false, env: Default::default() };
    for seed in 1..32 {
        let mut img = crate::tests::pattern(seed, 70, 44);
        effectcraft_render::color::quantize(&mut img, 255.0);
        let buf = effectcraft_effects::Buf { img, offset: [0.0; 2], scale: 1.0 };
        let cpu = (spec.render)(&ctx(), buf.clone());
        let gpu = effectcraft_render::Accelerator::effects(g, &[effectcraft_render::FxStep { spec, ctx: ctx() }], &buf, None).unwrap();
        for (i, (a, b)) in cpu.img.data.iter().zip(&gpu.img.data).enumerate() {
            let d = a.iter().zip(b).map(|(x, y)| (x - y).abs()).fold(0.0_f32, f32::max);
            assert!(d < 1e-6, "seed {seed} pixel {i} input {:?} CPU {a:?} GPU {b:?}", buf.img.data[i]);
        }
    }
}
