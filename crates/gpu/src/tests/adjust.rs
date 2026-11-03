//! Adjustment layers on the GPU vs the CPU: GPU-only stacks (resident on the device), mixed
//! stacks (CPU effects in between read back and upload), masks, opacity, colour management.

use effectcraft_keyframe::{ShapePath, Value};
use effectcraft_project::build::{self, Ids};
use effectcraft_project::{BitDepth, ColorSpace, MaskMode, MatteKind, TrackMatte};
use effectcraft_render::{Backend, FxStep, FxTarget, RenderOpts, Renderer};
use effectcraft_time::Tick;

use crate::tests::{Pattern, Scene, c, check, compare_at, gpu, n, opts, set};

fn adjustment_scene(depth: BitDepth, stack: &[(&str, Vec<(&str, Value)>)], masked: bool) -> Scene {
    let mut s = Scene::new(depth);
    let bg = s.footage(97, 61);
    s.push(bg);
    let mut l = s.footage(60, 40);
    set(&mut l, "transform/rotation", Value::Scalar(15.0));
    s.push(l);
    let mut adj = s.solid([1.0, 1.0, 1.0], 80, 50);
    adj.switches.adjustment = true;
    set(&mut adj, "transform/opacity", Value::Scalar(75.0));
    set(&mut adj, "transform/rotation", Value::Scalar(-6.0));
    for (id, vals) in stack {
        s.effect(&mut adj, id, vals);
    }
    if masked {
        let mut next = s.p.next_id;
        let m = build::mask(&mut Ids(&mut next), "Mask 1", ShapePath::ellipse([40.0, 25.0], 60.0, 40.0), MaskMode::Add, [255, 255, 0]);
        s.p.next_id = next;
        adj.props.sub_mut("masks").unwrap().children.push(m.into());
    }
    s.push(adj);
    let top = s.solid([0.2, 0.6, 0.9], 20, 20);
    s.push(top);
    s
}

fn gpu_stack() -> Vec<(&'static str, Vec<(&'static str, Value)>)> {
    vec![
        ("ec.blur.gaussian", vec![("blurriness", n(6.0))]),
        ("ec.color.levels", vec![("inBlack", n(0.1)), ("gamma", n(1.4))]),
        ("ec.color.tint", vec![("black", c(0.1, 0.0, 0.2)), ("amount", n(60.0))]),
    ]
}

#[test]
fn gpu_adjustment_chains_match_the_cpu() {
    for depth in [BitDepth::Bpc8, BitDepth::Bpc16, BitDepth::Bpc32] {
        for masked in [false, true] {
            let s = adjustment_scene(depth, &gpu_stack(), masked);
            check(&format!("gpu adjustment {depth:?} masked {masked}"), compare_at(&s, opts(), Tick::ZERO), 0.0);
            check(&format!("gpu adjustment {depth:?} masked {masked} half"), compare_at(&s, RenderOpts { scale: 0.5, ..opts() }, Tick::ZERO), 0.0);
        }
    }
}

#[test]
fn mixed_adjustment_chains_fall_back_per_effect() {
    let mut stack = gpu_stack();
    stack.insert(1, ("ec.distort.rollingshutterrepair", vec![]));
    stack.push(("ec.stylize.glow", vec![("threshold", n(30.0))]));
    for depth in [BitDepth::Bpc8, BitDepth::Bpc32] {
        let s = adjustment_scene(depth, &stack, true);
        check(&format!("mixed adjustment {depth:?}"), compare_at(&s, opts(), Tick::ZERO), 0.0);
    }
    // Colour-managed: effects run in the working space, compositing in linear light.
    for depth in [BitDepth::Bpc8, BitDepth::Bpc32] {
        let mut s = adjustment_scene(depth, &stack, false);
        s.p.settings.working_space = Some(ColorSpace::Rec709);
        s.p.settings.blend_linear = true;
        check(&format!("colour-managed adjustment {depth:?}"), compare_at(&s, opts(), Tick::ZERO), 0.0);
    }
}

/// A track matte limits where the adjustment shows, as on the CPU (#1201).
#[test]
fn track_mattes_limit_gpu_adjustments_like_the_cpu() {
    for depth in [BitDepth::Bpc8, BitDepth::Bpc32] {
        for kind in [MatteKind::Alpha, MatteKind::AlphaInverted, MatteKind::Luma, MatteKind::LumaInverted] {
            let mut s = adjustment_scene(depth, &gpu_stack(), false);
            let mut matte = s.footage(50, 30);
            matte.switches.video = false;
            set(&mut matte, "transform/rotation", Value::Scalar(10.0));
            let mid = s.push(matte);
            let c = s.p.comp_mut(s.cid).unwrap();
            let adj = c.layers.find_mut(|l| l.switches.adjustment).unwrap();
            adj.track_matte = Some(TrackMatte { layer: mid, kind });
            check(&format!("matted adjustment {kind:?} {depth:?}"), compare_at(&s, opts(), Tick::ZERO), 0.0);
        }
    }
}

/// Counts where each run of the stack goes.
#[derive(Default)]
struct Probe {
    gpu: usize,
    cpu: usize,
}

impl FxTarget for Probe {
    fn gpu(&mut self, steps: &[FxStep]) -> bool {
        self.gpu += steps.len();
        true
    }
    fn cpu(&mut self, _f: &mut dyn FnMut(effectcraft_effects::Buf) -> effectcraft_effects::Buf) {
        self.cpu += 1;
    }
}

#[test]
fn adjustment_stacks_split_into_gpu_runs_and_cpu_steps() {
    let Some(g) = gpu() else { return };
    let mut stack = gpu_stack();
    stack.insert(1, ("ec.generate.fractal", vec![]));
    let s = adjustment_scene(BitDepth::Bpc32, &stack, false);
    let mut r = Renderer::new(&s.p, &Pattern, RenderOpts { backend: Backend::Gpu, ..opts() });
    r.accel = Some(g);
    let ctx = r.eval_ctx(s.cid, Tick::ZERO).unwrap();
    let adj = ctx.comp.layers.iter().find(|l| l.switches.adjustment).unwrap();
    let mut p = Probe::default();
    r.run_effects_on(&ctx, adj, true, &mut p);
    assert_eq!((p.gpu, p.cpu), (3, 1));
}

#[test]
fn composite_reads_the_original_adjustment_input_on_gpu() {
    for depth in [BitDepth::Bpc8, BitDepth::Bpc32] {
        for mode in [0, 1, 2, 4, 15] {
            for rgb in [false, true] {
                let stack = vec![
                    ("ec.channel.invert", vec![]),
                    ("ec.channel.cccomposite", vec![("compositeOriginal", Value::Enum(mode)), ("opacity", n(65.0)), ("rgbOnly", Value::Bool(rgb))]),
                ];
                let s = adjustment_scene(depth, &stack, true);
                check(&format!("original adjustment {depth:?} mode {mode} rgb {rgb}"), compare_at(&s, RenderOpts { scale: 0.5, ..opts() }, Tick::ZERO), 0.0);
            }
        }
    }
}
