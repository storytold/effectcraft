//! Classic 3D on the GPU vs the CPU compositor: intersecting planes, blend modes, lights,
//! ray-cast shadows, depth of field, motion blur, track mattes, collapsed precomps and
//! adjustment layers splitting a run. Same tolerances as `tests.rs`; pixels on the exact
//! intersection line of two planes may sort differently (f32 vs f64 depths), so intersecting
//! scenes allow 0.5 % of pixels over.

use effectcraft_color::{BlendMode, Label};
use effectcraft_keyframe::{Keyframe, Value};
use effectcraft_project::build;
use effectcraft_project::{BitDepth, Comp, ItemKind, Layer, LayerSource, LightKind, MatteKind, TrackMatte};
use effectcraft_render::{Backend, RenderOpts, Renderer};
use effectcraft_time::{FrameRate, Tick};

use crate::tests::{Pattern, Scene, check, compare_at, gpu, n, opts, set};

const W: u32 = 160;
const H: u32 = 100;

fn scene(depth: BitDepth) -> Scene {
    let mut s = Scene::new(depth);
    let comp = Comp::new(W, H, FrameRate::FPS_30, Tick::from_seconds_f64(2.0));
    *s.p.comp_mut(s.cid).unwrap() = comp.clone();
    s.comp = comp;
    s
}

fn three_d(mut l: Layer, pos: [f64; 3], rot_y: f64) -> Layer {
    l.switches.three_d = true;
    set(&mut l, "transform/position", Value::Vec3(pos));
    set(&mut l, "transform/rotationY", Value::Scalar(rot_y));
    l
}

fn camera(s: &mut Scene, pos: [f64; 3], dof: Option<(f64, f64)>) {
    let mut cam = build::layer(&mut s.p, &s.comp, "Camera", LayerSource::Camera, (W, H), None);
    set(&mut cam, "cameraOptions/zoom", Value::Scalar(W as f64 * 50.0 / 36.0));
    set(&mut cam, "transform/position", Value::Vec3(pos));
    set(&mut cam, "transform/poi", Value::Vec3([W as f64 / 2.0, H as f64 / 2.0, 0.0]));
    if let Some((focus, aperture)) = dof {
        set(&mut cam, "cameraOptions/dof", Value::Bool(true));
        set(&mut cam, "cameraOptions/focusDistance", Value::Scalar(focus));
        set(&mut cam, "cameraOptions/aperture", Value::Scalar(aperture));
    }
    s.push(cam);
}

fn light(s: &mut Scene, kind: LightKind, pos: [f64; 3], vals: &[(&str, Value)]) {
    let mut l = build::layer(&mut s.p, &s.comp, "Light", LayerSource::Light { kind }, (W, H), None);
    if kind != LightKind::Ambient {
        set(&mut l, "transform/position", Value::Vec3(pos));
    }
    if l.props.prop_mut("transform/poi").is_some() {
        set(&mut l, "transform/poi", Value::Vec3([W as f64 / 2.0, H as f64 / 2.0, 0.0]));
    }
    for (k, v) in vals {
        set(&mut l, &format!("lightOptions/{k}"), v.clone());
    }
    s.push(l);
}

/// Background, a far plane, and two planes crossing each other (one in `mode`).
fn crossing(depth: BitDepth, mode: BlendMode) -> Scene {
    let mut s = scene(depth);
    let bg = s.solid([0.1, 0.12, 0.2], W, H);
    s.push(bg);
    let far = three_d(s.footage(150, 90), [80.0, 50.0, 120.0], 0.0);
    s.push(far);
    let a = three_d(s.footage(90, 70), [80.0, 50.0, 0.0], 35.0);
    s.push(a);
    let mut b = three_d(s.solid([0.9, 0.5, 0.2], 90, 70), [80.0, 52.0, 0.0], -35.0);
    b.blend_mode = mode;
    set(&mut b, "transform/opacity", Value::Scalar(80.0));
    s.push(b);
    s
}

#[test]
fn intersecting_planes_and_blend_modes() {
    for depth in [BitDepth::Bpc8, BitDepth::Bpc32] {
        for mode in [BlendMode::Normal, BlendMode::Screen, BlendMode::Multiply, BlendMode::Add, BlendMode::Dissolve] {
            let mut s = crossing(depth, mode);
            check(&format!("crossing {mode:?} {depth:?} (default camera)"), compare_at(&s, opts(), Tick::ZERO), 0.005);
            camera(&mut s, [30.0, 20.0, -260.0], None);
            check(&format!("crossing {mode:?} {depth:?} (camera)"), compare_at(&s, opts(), Tick::ZERO), 0.005);
        }
    }
}

#[test]
fn lights_specular_transmission_and_shadows() {
    for depth in [BitDepth::Bpc8, BitDepth::Bpc32] {
        let mut s = crossing(depth, BlendMode::Normal);
        // Every layer casts shadows; the crossing pair transmits light.
        for l in &mut s.p.comp_mut(s.cid).unwrap().layers {
            if l.switches.three_d {
                set(l, "materialOptions/castsShadows", Value::Enum(1));
                set(l, "materialOptions/specularIntensity", Value::Scalar(60.0));
                set(l, "materialOptions/specularShininess", Value::Scalar(30.0));
                set(l, "materialOptions/lightTransmission", Value::Scalar(40.0));
            }
        }
        camera(&mut s, [-20.0, -10.0, -250.0], None);
        light(&mut s, LightKind::Ambient, [0.0, 0.0, 0.0], &[("intensity", n(25.0))]);
        light(
            &mut s,
            LightKind::Spot,
            [20.0, -60.0, -150.0],
            &[("castsShadows", Value::Bool(true)), ("shadowDarkness", n(80.0)), ("coneAngle", n(70.0)), ("coneFeather", n(40.0)), ("intensity", n(120.0))],
        );
        light(
            &mut s,
            LightKind::Point,
            [150.0, 20.0, -80.0],
            &[
                ("castsShadows", Value::Bool(true)),
                ("shadowDiffusion", n(6.0)),
                ("falloff", Value::Enum(1)),
                ("radius", n(120.0)),
                ("falloffDistance", n(200.0)),
            ],
        );
        light(&mut s, LightKind::Parallel, [-100.0, -100.0, -100.0], &[("castsShadows", Value::Bool(true)), ("color", Value::Color([0.5, 0.7, 1.0, 1.0]))]);
        check(&format!("lights + shadows {depth:?}"), compare_at(&s, opts(), Tick::ZERO), 0.005);
        // Accepts Shadows: Only, Accepts Lights off, Casts Shadows: Only.
        for l in &mut s.p.comp_mut(s.cid).unwrap().layers {
            match l.name.as_str() {
                "Pattern" => set(l, "materialOptions/acceptsShadows", Value::Enum(2)),
                "Solid" if l.switches.three_d => set(l, "materialOptions/acceptsLights", Value::Bool(false)),
                _ => {}
            }
        }
        check(&format!("shadow-only materials {depth:?}"), compare_at(&s, opts(), Tick::ZERO), 0.005);
    }
}

#[test]
fn depth_of_field_and_motion_blur() {
    for depth in [BitDepth::Bpc8, BitDepth::Bpc32] {
        let mut s = crossing(depth, BlendMode::Normal);
        camera(&mut s, [80.0, 50.0, -220.0], Some((220.0, 30.0)));
        check(&format!("dof {depth:?}"), compare_at(&s, opts(), Tick::ZERO), 0.005);
        // Animated plane with motion blur (sub-sampled geometry, equal weights).
        let mut s = scene(depth);
        let mut l = three_d(s.footage(60, 40), [40.0, 50.0, 0.0], 20.0);
        l.switches.motion_blur = true;
        l.props.prop_mut("transform/position").unwrap().keys =
            vec![Keyframe::new(Tick::ZERO, Value::Vec3([40.0, 50.0, 0.0])), Keyframe::new(Tick::from_seconds_f64(1.0), Value::Vec3([140.0, 40.0, -60.0]))];
        s.push(l);
        s.p.comp_mut(s.cid).unwrap().enable_motion_blur = true;
        light(&mut s, LightKind::Point, [80.0, 0.0, -100.0], &[]);
        check(&format!("3d motion blur {depth:?}"), compare_at(&s, opts(), Tick::from_seconds_f64(0.5)), 0.005);
    }
}

#[test]
fn mattes_preserve_transparency_precomps_and_adjustment_splits() {
    for depth in [BitDepth::Bpc8, BitDepth::Bpc32] {
        let mut s = crossing(depth, BlendMode::Normal);
        // A 2D luma matte for the top plane, preserve transparency on the far plane.
        let m = s.solid([0.7, 0.7, 0.7], 70, 50);
        let mid = s.push(m);
        let layers = &mut s.p.comp_mut(s.cid).unwrap().layers;
        layers[1].track_matte = Some(TrackMatte { layer: mid, kind: MatteKind::Luma });
        if let Some(far) = layers.iter_mut().find(|l| l.switches.three_d && l.name == "Pattern") {
            far.preserve_transparency = true;
        }
        // Move the matte below the bottom so the 3D run stays one run.
        let ml = layers.remove(0);
        layers.push(ml);
        layers.last_mut().unwrap().switches.video = false;
        check(&format!("matte + preserve {depth:?}"), compare_at(&s, opts(), Tick::ZERO), 0.005);

        // An adjustment layer in the middle of the run splits it.
        let mut s = crossing(depth, BlendMode::Normal);
        let mut adj = s.solid([1.0, 1.0, 1.0], W, H);
        adj.switches.adjustment = true;
        adj.switches.three_d = true;
        s.effect(&mut adj, "ec.channel.invert", &[]);
        s.p.comp_mut(s.cid).unwrap().layers.insert(1, adj);
        check(&format!("adjustment split {depth:?}"), compare_at(&s, opts(), Tick::ZERO), 0.005);

        // A collapsed 3D precomp: its layers join the parent's 3D space.
        let mut s = crossing(depth, BlendMode::Normal);
        let inner = Comp::new(80, 60, FrameRate::FPS_30, Tick::from_seconds_f64(2.0));
        let inner_id = s.p.add_item("Inner", Label::Sandstone, None, ItemKind::Comp(inner.clone().into()));
        let mut a = s.footage(50, 40);
        a.switches.three_d = true;
        set(&mut a, "transform/rotationX", Value::Scalar(40.0));
        let b = s.solid([0.2, 0.8, 0.4], 30, 20);
        s.p.comp_mut(inner_id).unwrap().layers = vec![b, a];
        let mut pre = build::layer(&mut s.p, &s.comp, "Inner", LayerSource::Comp { item: inner_id }, (80, 60), None);
        pre.switches.collapse = true;
        pre.switches.three_d = true;
        set(&mut pre, "transform/position", Value::Vec3([70.0, 45.0, -10.0]));
        s.push(pre);
        check(&format!("collapsed 3d precomp {depth:?}"), compare_at(&s, opts(), Tick::ZERO), 0.005);
    }
}

#[test]
fn the_gpu_draws_classic_3d_runs_itself() {
    let Some(g) = gpu() else { return };
    let s = crossing(BitDepth::Bpc32, BlendMode::Normal);
    let r = Renderer::new(&s.p, &Pattern, RenderOpts { backend: Backend::Gpu, ..opts() });
    let ctx = r.eval_ctx(s.cid, Tick::ZERO).unwrap();
    let vis = r.visible_layers(&ctx);
    let run: Vec<_> = vis.iter().copied().filter(|l| l.is_3d()).collect();
    let prep = r.prepare_3d_run(&ctx, &run, (W, H)).expect("a classic run is prepared for the GPU");
    assert_eq!(prep.planes.iter().filter(|p| p.draw).count(), 3);
    let mut e = crate::context::Enc::new(g.context());
    let canvas = e.image(W, H);
    let out = crate::classic3d::draw_run(&mut e, &prep, &canvas).expect("the kernel draws the run");
    let img = e.download(&out).unwrap();
    let mut cpu = effectcraft_raster::Image::new(W, H);
    r.draw_3d_run(&ctx, &run, &mut cpu);
    let covered = cpu.data.iter().filter(|p| p[3] > 0.5).count();
    assert!(covered > (W * H / 3) as usize, "the planes cover the frame ({covered} px)");
    let d = crate::tests::diff(&cpu, &img, 1e-3);
    assert!(d.over * 200 <= d.total, "{d:?}");
}

/// Classic 3D bokeh depth of field runs on the GPU: iris shapes, highlights, fringe, Fast
/// Rectangle and a tilted plane (progressive blur) match the CPU.
#[test]
fn bokeh_depth_of_field_runs_on_the_gpu() {
    let cases: [(&str, &[(&str, Value)]); 4] = [
        ("fast rectangle", &[]),
        ("hexagon + highlights", &[("irisShape", Value::Enum(4)), ("irisRotation", n(15.0)), ("highlightGain", n(40.0)), ("highlightThreshold", n(150.0))]),
        ("round + fringe", &[("irisShape", Value::Enum(6)), ("irisRoundness", n(60.0)), ("irisDiffractionFringe", n(80.0))]),
        ("wide triangle", &[("irisShape", Value::Enum(1)), ("irisAspectRatio", n(2.0))]),
    ];
    for depth in [BitDepth::Bpc8, BitDepth::Bpc32] {
        for (label, vals) in cases {
            // The crossing planes are tilted (±35°): every plane blurs progressively.
            let mut s = crossing(depth, BlendMode::Normal);
            camera(&mut s, [80.0, 50.0, -220.0], Some((180.0, 40.0)));
            for l in &mut s.p.comp_mut(s.cid).unwrap().layers {
                if l.name == "Camera" {
                    for (k, v) in vals {
                        set(l, &format!("cameraOptions/{k}"), v.clone());
                    }
                }
            }
            check(&format!("bokeh {label} {depth:?}"), compare_at(&s, opts(), Tick::ZERO), 0.005);
            // The blur is the GPU's: the prepared planes carry it unapplied.
            if depth == BitDepth::Bpc32
                && let Some(g) = gpu()
            {
                let r = Renderer::new(&s.p, &Pattern, RenderOpts { backend: Backend::Gpu, ..opts() });
                let ctx = r.eval_ctx(s.cid, Tick::ZERO).unwrap();
                let run: Vec<_> = r.visible_layers(&ctx).into_iter().filter(|l| l.is_3d()).collect();
                let prep = r.prepare_3d_run(&ctx, &run, (W, H)).unwrap();
                assert!(prep.planes.iter().filter(|p| p.draw).all(|p| p.dof.is_some()), "{label}: depth of field deferred to the GPU");
                let mut e = crate::context::Enc::new(g.context());
                let canvas = e.image(W, H);
                assert!(crate::classic3d::draw_run(&mut e, &prep, &canvas).is_some(), "{label}: drawn on the GPU");
            }
        }
    }
}

/// A 2D layer whose track matte is a 3D layer: the matte is drawn through the camera on the
/// GPU walk as on the CPU.
#[test]
fn two_d_layer_with_a_three_d_track_matte() {
    for depth in [BitDepth::Bpc8, BitDepth::Bpc32] {
        for kind in [MatteKind::Alpha, MatteKind::Luma] {
            let mut s = scene(depth);
            let bg = s.solid([0.1, 0.12, 0.2], W, H);
            s.push(bg);
            let fill = s.footage(W, H);
            let fid = s.push(fill);
            // The matte: a solid turned in 3D, seen through a camera.
            let m = three_d(s.solid([0.9, 0.9, 0.9], 70, 50), [80.0, 50.0, 30.0], 40.0);
            let mid = s.push(m);
            camera(&mut s, [40.0, 30.0, -200.0], None);
            let layers = &mut s.p.comp_mut(s.cid).unwrap().layers;
            let fi = layers.iter().position(|l| l.id == fid).unwrap();
            layers[fi].track_matte = Some(TrackMatte { layer: mid, kind });
            if let Some(ml) = layers.iter_mut().find(|l| l.id == mid) {
                ml.switches.video = false;
            }
            check(&format!("3d matte {kind:?} {depth:?}"), compare_at(&s, opts(), Tick::ZERO), 0.005);
            // The same with the Advanced 3D renderer (its CPU path draws 3D mattes through the
            // camera with composite_iso_with).
            s.p.comp_mut(s.cid).unwrap().renderer = effectcraft_project::Renderer::Advanced3D;
            check(&format!("3d matte {kind:?} {depth:?} (Advanced 3D)"), compare_at(&s, opts(), Tick::ZERO), 0.005);
        }
    }
}
