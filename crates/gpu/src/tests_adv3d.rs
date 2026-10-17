//! Advanced 3D: the GPU rasteriser against the CPU reference on the same scenes.
//!
//! Tolerances: the GPU's colour target is half float (relative error ≤ 1e-3) and its
//! rasteriser and interpolators differ from the CPU's in the last bits, so pixels on triangle
//! edges may be covered by the other neighbour. Interior pixels agree within 2/255; at most
//! 1.5 % of pixels (silhouettes) may differ more. Skipped without an adapter.

use effectcraft_keyframe::Value;
use effectcraft_project::build;
use effectcraft_project::{Comp, ItemId, ItemKind, LayerSource, LightKind, PrimitiveKind, Project, Renderer as R3, Solid};
use effectcraft_render::three_d::adv::{self, Scene};
use effectcraft_render::{Accelerator, EvalCtx, NoFootage, RenderOpts, Renderer};
use effectcraft_time::{FrameRate, Tick};

use crate::Gpu;

/// A device of the test's own (the test holds [`crate::tests::hold_gpu_lock`]).
fn gpu() -> Option<Gpu> {
    crate::tests::hold_gpu_lock();
    match Gpu::try_headless() {
        Ok(g) => Some(g),
        Err(e) => {
            eprintln!("effectcraft-gpu adv3d tests: no GPU ({e}), skipping");
            crate::tests::require_gpu("effectcraft-gpu adv3d tests", Err(&e))
        }
    }
}

fn add(p: &mut Project, cid: ItemId, src: LayerSource, edit: impl FnOnce(&mut effectcraft_project::Layer)) {
    let comp = p.comp(cid).unwrap().clone();
    let mut l = build::layer(p, &comp, "L", src, (comp.width, comp.height), None);
    edit(&mut l);
    p.comp_mut(cid).unwrap().layers.insert(0, l);
}

fn set(l: &mut effectcraft_project::Layer, path: &str, v: Value) {
    l.props.prop_mut(path).unwrap_or_else(|| panic!("{path}")).value = v;
}

/// Primitives with metal/dielectric materials, a shadow-casting spot light and a point light,
/// an environment light, a semi-transparent textured card.
pub(crate) fn scene_project() -> (Project, ItemId) {
    let mut p = Project::default();
    let mut c = Comp::new(240, 160, FrameRate::FPS_30, Tick::from_seconds_f64(1.0));
    c.renderer = R3::Advanced3D;
    let cid = p.add_item("C", Default::default(), None, ItemKind::Comp(c.into()));
    add(&mut p, cid, LayerSource::Primitive { kind: PrimitiveKind::Plane }, |l| {
        set(l, "geometryOptions/width", Value::Scalar(900.0));
        set(l, "geometryOptions/height", Value::Scalar(900.0));
        set(l, "transform/position", Value::Vec3([120.0, 130.0, 0.0]));
        set(l, "transform/rotationX", Value::Scalar(90.0));
    });
    add(&mut p, cid, LayerSource::Primitive { kind: PrimitiveKind::Sphere }, |l| {
        set(l, "geometryOptions/radius", Value::Scalar(35.0));
        set(l, "transform/position", Value::Vec3([80.0, 95.0, 0.0]));
        set(l, "materialOptions/baseColor", Value::Color([0.9, 0.2, 0.2, 1.0]));
        set(l, "materialOptions/roughness", Value::Scalar(35.0));
        set(l, "materialOptions/castsShadows", Value::Enum(1));
    });
    add(&mut p, cid, LayerSource::Primitive { kind: PrimitiveKind::Torus }, |l| {
        set(l, "geometryOptions/radius", Value::Scalar(35.0));
        set(l, "geometryOptions/tubeRadius", Value::Scalar(12.0));
        set(l, "transform/position", Value::Vec3([165.0, 90.0, 0.0]));
        set(l, "transform/rotationX", Value::Scalar(60.0));
        set(l, "materialOptions/metallic", Value::Scalar(100.0));
        set(l, "materialOptions/roughness", Value::Scalar(20.0));
        set(l, "materialOptions/castsShadows", Value::Enum(1));
    });
    let sid = p.add_item("S", Default::default(), None, ItemKind::Solid(Solid { color: [0.2, 0.6, 0.9], width: 60, height: 40, pixel_aspect: 1.0 }));
    add(&mut p, cid, LayerSource::Solid { item: sid }, |l| {
        l.switches.three_d = true;
        set(l, "transform/opacity", Value::Scalar(60.0));
        set(l, "transform/position", Value::Vec3([120.0, 80.0, -60.0]));
        set(l, "transform/rotationY", Value::Scalar(30.0));
    });
    let env = p.add_item("E", Default::default(), None, ItemKind::Solid(Solid { color: [0.6, 0.7, 1.0], width: 32, height: 16, pixel_aspect: 1.0 }));
    add(&mut p, cid, LayerSource::Solid { item: env }, |l| {
        l.environment = true;
        l.switches.three_d = true;
    });
    add(&mut p, cid, LayerSource::Light { kind: LightKind::Environment }, |l| set(l, "lightOptions/intensity", Value::Scalar(50.0)));
    add(&mut p, cid, LayerSource::Light { kind: LightKind::Spot }, |l| {
        set(l, "transform/position", Value::Vec3([40.0, -120.0, -200.0]));
        set(l, "transform/poi", Value::Vec3([120.0, 100.0, 0.0]));
        set(l, "lightOptions/castsShadows", Value::Bool(true));
        set(l, "lightOptions/coneAngle", Value::Scalar(110.0));
        set(l, "lightOptions/shadowDiffusion", Value::Scalar(4.0));
    });
    add(&mut p, cid, LayerSource::Light { kind: LightKind::Point }, |l| {
        set(l, "transform/position", Value::Vec3([200.0, 20.0, -100.0]));
        set(l, "lightOptions/intensity", Value::Scalar(40.0));
        set(l, "lightOptions/falloff", Value::Enum(1));
    });
    (p, cid)
}

pub(crate) fn scene(p: &Project, cid: ItemId) -> Scene {
    let r = Renderer::new(p, &NoFootage, RenderOpts::default());
    let ctx = EvalCtx { project: p, comp_id: cid, comp: p.comp(cid).unwrap(), time: Tick::ZERO, expr: None, footage: None };
    let run: Vec<&effectcraft_project::Layer> = ctx.comp.layers.iter().rev().filter(|l| l.is_3d() && l.has_video()).collect();
    adv::scene_of(&r, &ctx, &run, (ctx.comp.width, ctx.comp.height))
}

fn compare(s: &Scene, g: &Gpu) {
    let cpu = adv::raster::render(s);
    let gt = g.raster_3d(s).expect("gpu raster");
    assert_eq!((gt.width, gt.height), (cpu.width, cpu.height));
    let n = cpu.color.len();
    let mut bad = 0;
    let mut sum = 0.0f64;
    for (a, b) in cpu.color.iter().zip(&gt.color) {
        let d = (0..4).map(|k| (a[k] - b[k]).abs() / a[k].abs().max(1.0)).fold(0.0f32, f32::max);
        sum += d as f64;
        if d > 2.0 / 255.0 {
            bad += 1;
        }
    }
    let frac = bad as f64 / n as f64;
    eprintln!("adv3d gpu vs cpu: {bad}/{n} pixels beyond 2/255 ({:.3} %), mean {:.5}", frac * 100.0, sum / n as f64);
    assert!(frac < 0.015, "{:.3} % of pixels differ", frac * 100.0);
    assert!(sum / (n as f64) < 0.002);
    // Depth agrees where both see a surface.
    let mut dz = 0;
    for (a, b) in cpu.depth.iter().zip(&gt.depth) {
        if a.is_finite() != b.is_finite() || (a.is_finite() && (a - b).abs() > 0.01 * a.abs().max(1.0)) {
            dz += 1;
        }
    }
    assert!((dz as f64) < 0.015 * n as f64, "{dz} depth mismatches");
}

#[test]
fn gpu_matches_cpu_lit_scene() {
    let Some(g) = gpu() else { return };
    let (p, cid) = scene_project();
    let s = scene(&p, cid);
    assert!(!s.shadows.is_empty() && s.env.is_some() && s.opaque_count < s.indices.len() as u32);
    compare(&s, &g);
}

#[test]
fn gpu_matches_cpu_unlit_and_full_frames() {
    let Some(g) = gpu() else { return };
    let (mut p, cid) = scene_project();
    // Unlit: remove the lights.
    p.comp_mut(cid).unwrap().layers.retain(|l| !l.is_light());
    compare(&scene(&p, cid), &g);
    // Whole frames through the renderer with the GPU attached.
    let (p, cid) = scene_project();
    let cpu = Renderer::new(&p, &NoFootage, RenderOpts::default()).comp_frame(cid, Tick::ZERO);
    let mut r = Renderer::new(&p, &NoFootage, RenderOpts { backend: effectcraft_render::Backend::Gpu, ..Default::default() });
    r.accel = Some(&g);
    let gimg = r.comp_frame(cid, Tick::ZERO);
    let diff = cpu.data.iter().zip(&gimg.data).filter(|(a, b)| (0..4).any(|k| (a[k] - b[k]).abs() > 3.0 / 255.0)).count();
    assert!((diff as f64) < 0.02 * cpu.data.len() as f64, "{diff} pixels differ");
}

#[test]
fn gpu_matches_cpu_shadow_catcher() {
    // #1691: the floor plane as a shadow catcher ("Accepts Shadows: Only").
    let Some(g) = gpu() else { return };
    let (mut p, cid) = scene_project();
    let floor = p.comp_mut(cid).unwrap().layers.last_mut().unwrap();
    set(floor, "materialOptions/acceptsShadows", Value::Enum(2));
    let s = scene(&p, cid);
    assert!(s.materials.iter().any(|m| m.shadows_only) && !s.shadows.is_empty());
    compare(&s, &g);
}

#[test]
fn gpu_raster_perf_smoke() {
    let Some(g) = gpu() else { return };
    let (p, cid) = scene_project();
    let s = scene(&p, cid);
    let t0 = std::time::Instant::now();
    let _ = adv::raster::render(&s);
    let cpu = t0.elapsed();
    // First use compiles the pipelines.
    let _ = g.raster_3d(&s);
    let t0 = std::time::Instant::now();
    let _ = g.raster_3d(&s);
    let gpu = t0.elapsed();
    eprintln!("adv3d 480x320 (2x SSAA of 240x160), {} triangles: cpu {:?}, gpu {:?}", s.indices.len() / 3, cpu, gpu);
}

// ---------------------------------------------------------------- whole runs (M13.11)
//
// `Accelerator::render_3d` (raster, resolve, motion-blur average, iris depth of field, encode
// on the GPU) against `adv::render_prepared` on the CPU. Tolerance: ≤ 1/255 on ≥ 99 % of the
// pixels. Against the CPU rasteriser the remaining pixels are silhouette pixels the two
// rasterisers cover differently (and the blur spreads); against the CPU post-processing of
// the *same* GPU raster the agreement is ≥ 99.9 % (f32 order of operations only).

/// The lit scene seen through a depth-of-field camera (hexagonal iris, diffraction fringe,
/// highlight boost) with the sphere moving under motion blur.
pub(crate) fn dof_mb_project() -> (Project, ItemId) {
    let (mut p, cid) = scene_project();
    add(&mut p, cid, LayerSource::Camera, |l| {
        let zoom = match l.props.prop_mut("cameraOptions/zoom").unwrap().value {
            Value::Scalar(z) => z,
            _ => 300.0,
        };
        set(l, "cameraOptions/dof", Value::Bool(true));
        set(l, "cameraOptions/focusDistance", Value::Scalar(zoom - 60.0));
        set(l, "cameraOptions/aperture", Value::Scalar(160.0));
        set(l, "cameraOptions/irisShape", Value::Enum(4));
        set(l, "cameraOptions/irisRotation", Value::Scalar(15.0));
        set(l, "cameraOptions/irisDiffractionFringe", Value::Scalar(50.0));
        set(l, "cameraOptions/highlightGain", Value::Scalar(30.0));
        set(l, "cameraOptions/highlightThreshold", Value::Scalar(150.0));
    });
    let c = p.comp_mut(cid).unwrap();
    c.enable_motion_blur = true;
    c.motion_blur_samples = 6;
    // The sphere moves 40 px per frame.
    let sphere = c.layers.iter_mut().find(|l| matches!(l.source, LayerSource::Primitive { kind: PrimitiveKind::Sphere })).unwrap();
    sphere.switches.motion_blur = true;
    sphere.props.prop_mut("transform/position").unwrap().keys = vec![
        effectcraft_keyframe::Keyframe::new(Tick::ZERO, Value::Vec3([80.0, 95.0, 0.0])),
        effectcraft_keyframe::Keyframe::new(Tick::from_seconds_f64(1.0), Value::Vec3([80.0 + 30.0 * 40.0, 95.0, 0.0])),
    ];
    (p, cid)
}

/// Share of pixels (0–1) differing by more than 1/255 on any channel, and the largest
/// difference.
pub(crate) fn off_share(a: &[[f32; 4]], b: &[[f32; 4]]) -> (f64, f32) {
    let mut off = 0;
    let mut max = 0.0f32;
    for (p, q) in a.iter().zip(b) {
        // NaN counts as a difference (f32::max would drop it).
        let d = (0..4).map(|k| (p[k] - q[k]).abs()).fold(0.0f32, |a, b| if b.is_nan() || a.is_nan() { f32::NAN } else { a.max(b) });
        max = max.max(d);
        if d.is_nan() || d > 1.0 / 255.0 + 1e-6 {
            off += 1;
        }
    }
    (off as f64 / a.len().max(1) as f64, max)
}

pub(crate) fn run_of(p: &Project, cid: ItemId) -> (EvalCtx<'_>, Vec<&effectcraft_project::Layer>) {
    let ctx = EvalCtx { project: p, comp_id: cid, comp: p.comp(cid).unwrap(), time: Tick::ZERO, expr: None, footage: None };
    let run: Vec<&effectcraft_project::Layer> = ctx.comp.layers.iter().rev().filter(|l| l.is_3d() && l.has_video()).collect();
    (ctx, run)
}

fn compare_run(p: &Project, cid: ItemId, g: &Gpu, what: &str) {
    let (ctx, run) = run_of(p, cid);
    let out = (ctx.comp.width, ctx.comp.height);
    // CPU reference: CPU raster and post.
    let cpu_r = Renderer::new(p, &NoFootage, RenderOpts::default());
    let prep = adv::Prepared::new(&cpu_r, &ctx, &run, out);
    let (cimg, cdepth) = adv::render_prepared(&prep).expect("cpu drew");
    // The GPU: everything on the device.
    let (gimg, gdepth) = g.render_3d(&prep).expect("gpu handled").expect("gpu drew");
    let (share, max) = off_share(&cimg.data, &gimg.data);
    eprintln!("{what}: gpu vs cpu {:.3} % of pixels beyond 1/255 (max {max:.4})", share * 100.0);
    assert!(share <= 0.01, "{what}: {:.3} % of pixels differ", share * 100.0);
    let dz = cdepth.iter().zip(&gdepth).filter(|(a, b)| a.is_finite() != b.is_finite() || (a.is_finite() && (*a - *b).abs() > 0.01 * a.abs().max(1.0))).count();
    assert!((dz as f64) < 0.015 * cdepth.len() as f64, "{what}: {dz} depth mismatches");
    // CPU post-processing of the GPU raster: the post kernels alone.
    let mut gr = Renderer::new(p, &NoFootage, RenderOpts { backend: effectcraft_render::Backend::Gpu, ..Default::default() });
    gr.accel = Some(g);
    let prep = adv::Prepared::new(&gr, &ctx, &run, out);
    let (himg, _) = adv::render_prepared(&prep).expect("cpu post drew");
    let (share, max) = off_share(&himg.data, &gimg.data);
    eprintln!("{what}: gpu post vs cpu post on the gpu raster: {:.4} % beyond 1/255 (max {max:.4})", share * 100.0);
    assert!(share <= 0.001, "{what}: post kernels differ on {:.3} % of pixels", share * 100.0);
}

#[test]
fn gpu_runs_match_cpu_with_dof_and_motion_blur() {
    let Some(g) = gpu() else { return };
    let (p, cid) = dof_mb_project();
    let (ctx, run) = run_of(&p, cid);
    let r = Renderer::new(&p, &NoFootage, RenderOpts::default());
    let prep = adv::Prepared::new(&r, &ctx, &run, (240, 160));
    assert_eq!(prep.samples, 6);
    let dof = prep.dof.expect("depth of field");
    assert_eq!(dof.iris.sides, 6);
    compare_run(&p, cid, &g, "dof + motion blur");
    // The depth of field really blurs (against the same frame in focus).
    let (blurred, _) = g.render_3d(&prep).unwrap().unwrap();
    let mut sharp_prep = adv::Prepared::new(&r, &ctx, &run, (240, 160));
    sharp_prep.dof = None;
    let (sharp, _) = g.render_3d(&sharp_prep).unwrap().unwrap();
    let (changed, _) = off_share(&blurred.data, &sharp.data);
    assert!(changed > 0.2, "out-of-focus pixels: {:.1} %", changed * 100.0);
    // Fast Rectangle iris, no highlights; and without depth of field.
    let (mut p, cid) = dof_mb_project();
    for l in &mut p.comp_mut(cid).unwrap().layers {
        if let Some(pr) = l.props.prop_mut("cameraOptions/irisShape") {
            pr.value = Value::Enum(0);
        }
        if let Some(pr) = l.props.prop_mut("cameraOptions/highlightGain") {
            pr.value = Value::Scalar(0.0);
        }
    }
    compare_run(&p, cid, &g, "fast rectangle");
    for l in &mut p.comp_mut(cid).unwrap().layers {
        if let Some(pr) = l.props.prop_mut("cameraOptions/dof") {
            pr.value = Value::Bool(false);
        }
    }
    compare_run(&p, cid, &g, "motion blur only");
}

#[test]
fn gpu_runs_match_cpu_for_extrusions_and_collapsed_precomps() {
    let Some(g) = gpu() else { return };
    let (mut p, cid) = scene_project();
    // Extruded text with a bevelled stroke.
    add(&mut p, cid, LayerSource::Text, |l| {
        l.switches.three_d = true;
        let doc = effectcraft_keyframe::TextDoc {
            text: "Ab".into(),
            size: 50.0,
            fill: [0.9, 0.8, 0.2, 1.0],
            apply_fill: true,
            stroke: [0.1, 0.2, 0.9, 1.0],
            apply_stroke: true,
            stroke_width: 4.0,
            ..Default::default()
        };
        set(l, "text/sourceText", Value::Text(Box::new(doc)));
        let mut next = 20_000u64;
        l.props.children.push(build::extrusion_geometry_options(&mut build::Ids(&mut next)).into());
        set(l, "geometryOptions/extrusionDepth", Value::Scalar(15.0));
        set(l, "geometryOptions/bevelStyle", Value::Enum(1));
        set(l, "transform/position", Value::Vec3([120.0, 50.0, -30.0]));
        set(l, "transform/rotationY", Value::Scalar(25.0));
    });
    // A collapsed precomp holding a cube.
    let mut nc = Comp::new(240, 160, FrameRate::FPS_30, Tick::from_seconds_f64(1.0));
    nc.renderer = R3::Classic3D;
    let nid = p.add_item("N", Default::default(), None, ItemKind::Comp(nc.into()));
    add(&mut p, nid, LayerSource::Primitive { kind: PrimitiveKind::Cube }, |l| {
        for k in ["width", "height", "depth"] {
            set(l, &format!("geometryOptions/{k}"), Value::Scalar(30.0));
        }
        set(l, "transform/position", Value::Vec3([200.0, 120.0, -20.0]));
        set(l, "transform/rotationY", Value::Scalar(35.0));
        set(l, "transform/rotationX", Value::Scalar(-25.0));
    });
    add(&mut p, cid, LayerSource::Comp { item: nid }, |l| {
        l.switches.three_d = true;
        l.switches.collapse = true;
    });
    let (ctx, run) = run_of(&p, cid);
    let s = adv::scene_of(&Renderer::new(&p, &NoFootage, RenderOpts::default()), &ctx, &run, (240, 160));
    assert!(s.indices.len() / 3 > 500, "meshes: {} triangles", s.indices.len() / 3);
    compare_run(&p, cid, &g, "extrusions + collapsed precomp");
}

#[test]
fn gpu_compositor_draws_advanced_3d_runs_on_the_device() {
    let Some(g) = gpu() else { return };
    // A 2D background below the run and a 2D layer above it.
    let (mut p, cid) = dof_mb_project();
    let bg = p.add_item("B", Default::default(), None, ItemKind::Solid(Solid { color: [0.1, 0.1, 0.15], width: 240, height: 160, pixel_aspect: 1.0 }));
    let comp = p.comp(cid).unwrap().clone();
    let l = build::layer(&mut p, &comp, "B", LayerSource::Solid { item: bg }, (240, 160), None);
    p.comp_mut(cid).unwrap().layers.push(l);
    let top = p.add_item("T", Default::default(), None, ItemKind::Solid(Solid { color: [1.0, 0.5, 0.0], width: 40, height: 30, pixel_aspect: 1.0 }));
    add(&mut p, cid, LayerSource::Solid { item: top }, |l| set(l, "transform/opacity", Value::Scalar(50.0)));
    let (ctx, run) = run_of(&p, cid);
    assert!(Renderer::new(&p, &NoFootage, RenderOpts::default()).prepare_adv_run(&ctx, &run, (240, 160)).is_some());
    let cpu = Renderer::new(&p, &NoFootage, RenderOpts::default()).comp_frame(cid, Tick::ZERO);
    let mut r = Renderer::new(&p, &NoFootage, RenderOpts { backend: effectcraft_render::Backend::Gpu, ..Default::default() });
    r.accel = Some(&g);
    let gimg = g.render(&r, cid, Tick::ZERO).expect("gpu walk");
    let (share, max) = off_share(&cpu.data, &gimg.data);
    eprintln!("walk: {:.3} % beyond 1/255 (max {max:.4})", share * 100.0);
    assert!(share <= 0.01, "{:.3} % of pixels differ", share * 100.0);
    // A blend mode in the run takes the 2D compositing path (`SplitRun`, also on the GPU).
    p.comp_mut(cid).unwrap().layers.iter_mut().find(|l| l.switches.three_d && !l.is_light() && l.source.is_av()).unwrap().blend_mode =
        effectcraft_color::BlendMode::Screen;
    let (ctx, run) = run_of(&p, cid);
    assert!(Renderer::new(&p, &NoFootage, RenderOpts::default()).prepare_adv_run(&ctx, &run, (240, 160)).is_none());
    let cpu = Renderer::new(&p, &NoFootage, RenderOpts::default()).comp_frame(cid, Tick::ZERO);
    let mut r = Renderer::new(&p, &NoFootage, RenderOpts { backend: effectcraft_render::Backend::Gpu, ..Default::default() });
    r.accel = Some(&g);
    let gimg = r.comp_frame(cid, Tick::ZERO);
    let (share, _) = off_share(&cpu.data, &gimg.data);
    assert!(share <= 0.01, "blend-mode run: {:.3} %", share * 100.0);
}

#[test]
fn gpu_wireframes_match_cpu_exactly() {
    let Some(g) = gpu() else { return };
    let mut p = Project::default();
    let c = Comp::new(200, 120, FrameRate::FPS_30, Tick::from_seconds_f64(1.0));
    let cid = p.add_item("C", Default::default(), None, ItemKind::Comp(c.into()));
    let sid = p.add_item("S", Default::default(), None, ItemKind::Solid(Solid { color: [0.2, 0.4, 0.6], width: 200, height: 120, pixel_aspect: 1.0 }));
    add(&mut p, cid, LayerSource::Solid { item: sid }, |_| {});
    let s2 = p.add_item("W", Default::default(), None, ItemKind::Solid(Solid { color: [1.0, 0.0, 0.0], width: 70, height: 40, pixel_aspect: 1.0 }));
    add(&mut p, cid, LayerSource::Solid { item: s2 }, |l| {
        l.switches.quality = effectcraft_project::Quality::Wireframe;
        set(l, "transform/rotation", Value::Scalar(23.0));
        set(l, "transform/scale", Value::Vec3([130.0, 90.0, 100.0]));
    });
    let cpu = Renderer::new(&p, &NoFootage, RenderOpts::default()).comp_frame(cid, Tick::ZERO);
    let mut r = Renderer::new(&p, &NoFootage, RenderOpts { backend: effectcraft_render::Backend::Gpu, ..Default::default() });
    r.accel = Some(&g);
    let gimg = g.render(&r, cid, Tick::ZERO).expect("gpu walk");
    let white = cpu.data.iter().filter(|q| **q == [1.0; 4]).count();
    assert!(white > 100, "outline drawn: {white}");
    let (share, max) = off_share(&cpu.data, &gimg.data);
    assert!(share == 0.0, "{share} differ (max {max})");
}

#[test]
fn gpu_dof_with_capped_radii() {
    // Radii at the 48 px cap over most of the frame (8 blur levels).
    let Some(g) = gpu() else { return };
    let (mut p, cid) = dof_mb_project();
    for l in &mut p.comp_mut(cid).unwrap().layers {
        if let Some(pr) = l.props.prop_mut("cameraOptions/aperture") {
            pr.value = Value::Scalar(1500.0);
        }
        l.switches.motion_blur = false;
    }
    compare_run(&p, cid, &g, "capped radii");
}

#[test]
fn gpu_environment_light_on_flat_extrusions_has_no_nan() {
    // Bevelled text faces pointing straight up or down: atan2(0, 0) in the environment lookup
    // (NaN on some GPUs) must give the CPU's 0.
    let Some(g) = gpu() else { return };
    let mut p = Project::default();
    let mut c = Comp::new(480, 270, FrameRate::FPS_30, Tick::from_seconds_f64(1.0));
    c.renderer = R3::Advanced3D;
    let cid = p.add_item("C", Default::default(), None, ItemKind::Comp(c.into()));
    add(&mut p, cid, LayerSource::Text, |l| {
        l.switches.three_d = true;
        let doc =
            effectcraft_keyframe::TextDoc { text: "EffectCraft".into(), size: 45.0, fill: [0.95, 0.75, 0.2, 1.0], apply_fill: true, ..Default::default() };
        set(l, "text/sourceText", Value::Text(Box::new(doc)));
        let mut next = 900_000u64;
        l.props.children.push(build::extrusion_geometry_options(&mut build::Ids(&mut next)).into());
        set(l, "geometryOptions/extrusionDepth", Value::Scalar(10.0));
        set(l, "geometryOptions/bevelStyle", Value::Enum(1));
        set(l, "transform/position", Value::Vec3([240.0, 65.0, 50.0]));
        set(l, "transform/rotationY", Value::Scalar(-15.0));
    });
    let env = p.add_item("E", Default::default(), None, ItemKind::Solid(Solid { color: [0.6, 0.7, 1.0], width: 32, height: 16, pixel_aspect: 1.0 }));
    add(&mut p, cid, LayerSource::Solid { item: env }, |l| {
        l.environment = true;
        l.switches.three_d = true;
    });
    add(&mut p, cid, LayerSource::Light { kind: LightKind::Environment }, |_| {});
    let s = scene(&p, cid);
    let t = g.raster_3d(&s).unwrap();
    let nan = t.color.iter().filter(|c| c.iter().any(|x| !x.is_finite())).count();
    assert_eq!(nan, 0, "non-finite pixels");
    compare(&s, &g);
}

/// Advanced 3D runs whose layers take the 2D compositing path (blend modes, a 2D and a 3D track
/// matte, Preserve Transparency) and environment backgrounds composite on the GPU: against the
/// CPU compositor on the same (GPU) rasters they agree to float rounding; against the CPU
/// rasteriser within the raster tolerance.
#[test]
fn gpu_compositor_draws_split_advanced_3d_runs_and_skies() {
    let Some(g) = gpu() else { return };
    for case in 0..10 {
        let (mut p, cid) = scene_project();
        if case >= 4 {
            p.settings.bit_depth = effectcraft_project::BitDepth::Bpc32;
        }
        let classic = case >= 8;
        let case = if classic { 3 } else { case % 4 };
        if classic {
            // Classic 3D: the sky on the GPU too, then the run's planes.
            p.comp_mut(cid).unwrap().renderer = R3::Classic3D;
        }
        let bg = p.add_item("B", Default::default(), None, ItemKind::Solid(Solid { color: [0.1, 0.1, 0.15], width: 240, height: 160, pixel_aspect: 1.0 }));
        let comp = p.comp(cid).unwrap().clone();
        let l = build::layer(&mut p, &comp, "B", LayerSource::Solid { item: bg }, (240, 160), None);
        p.comp_mut(cid).unwrap().layers.push(l);
        // A varying environment image (so the sky's lookup shows; a CPU effect, so the frame's
        // only readback is its own).
        let mut next = p.next_id;
        let spec = effectcraft_effects::find("ec.generate.fractal").unwrap();
        let ramp = effectcraft_effects::instantiate(spec, &mut build::Ids(&mut next), spec.name, [32.0, 16.0]);
        p.next_id = next;
        if case == 3 {
            // A 2D track matte layer above the run.
            let sid = p.add_item("M", Default::default(), None, ItemKind::Solid(Solid { color: [1.0, 1.0, 1.0], width: 120, height: 90, pixel_aspect: 1.0 }));
            add(&mut p, cid, LayerSource::Solid { item: sid }, |m| {
                m.switches.video = false;
                set(m, "transform/rotation", Value::Scalar(20.0));
            });
        }
        let c = p.comp_mut(cid).unwrap();
        let env = c.layers.iter_mut().find(|l| l.environment).unwrap();
        env.props.sub_mut("effects").unwrap().children.push(ramp.into());
        let ids: Vec<_> = c.layers.iter().filter(|l| l.switches.three_d && !l.is_light() && !l.environment && l.source.is_av()).map(|l| l.id).collect();
        let top = c.layers[0].id;
        fn by_id(c: &mut Comp, id: effectcraft_project::LayerId) -> &mut effectcraft_project::Layer {
            c.layers.iter_mut().find(|l| l.id == id).unwrap()
        }
        match case {
            0 => by_id(c, ids[1]).blend_mode = effectcraft_color::BlendMode::Screen,
            1 => {
                // The card is a 3D track matte (luma) for the sphere; the torus multiplies.
                let card = ids[0];
                by_id(c, ids[1]).blend_mode = effectcraft_color::BlendMode::Multiply;
                by_id(c, ids[2]).track_matte = Some(effectcraft_project::TrackMatte { layer: card, kind: effectcraft_project::MatteKind::Luma });
                by_id(c, ids[0]).switches.video = false;
            }
            2 => {
                // Preserve Transparency and an Overlay layer.
                by_id(c, ids[0]).preserve_transparency = true;
                by_id(c, ids[1]).blend_mode = effectcraft_color::BlendMode::Overlay;
            }
            _ => {
                // An environment background behind the run, and the 2D track matte.
                c.layers.iter_mut().find(|l| l.environment).unwrap().environment_background = true;
                by_id(c, ids[0]).track_matte = Some(effectcraft_project::TrackMatte { layer: top, kind: effectcraft_project::MatteKind::Alpha });
            }
        }
        let (ctx, run) = run_of(&p, cid);
        let r0 = Renderer::new(&p, &NoFootage, RenderOpts::default());
        if case < 3 {
            assert!(r0.split_adv_run(&ctx, &run, (240, 160)).is_some(), "case {case}: split");
        }
        let cpu = r0.comp_frame(cid, Tick::ZERO);
        let mut r = Renderer::new(&p, &NoFootage, RenderOpts { backend: effectcraft_render::Backend::Gpu, ..Default::default() });
        r.accel = Some(&g);
        // The CPU compositor on the GPU's rasters.
        let hybrid = r.comp_frame_cpu(cid, Tick::ZERO);
        let before = g.context().transfer_stats().readbacks;
        let gimg = g.render(&r, cid, Tick::ZERO).expect("gpu walk");
        // On the device end to end: the frame's own readback only (no CPU fallback).
        assert_eq!(g.context().transfer_stats().readbacks - before, 1, "case {case}: readbacks");
        let (share, max) = off_share(&hybrid.data, &gimg.data);
        eprintln!("split case {case}: gpu walk vs cpu compositing {:.4} % beyond 1/255 (max {max:.4})", share * 100.0);
        assert!(share <= 0.001, "case {case}: {:.3} % of pixels differ (max {max})", share * 100.0);
        let (share, max) = off_share(&cpu.data, &gimg.data);
        eprintln!("split case {case}: gpu vs cpu {:.3} % beyond 1/255 (max {max:.4})", share * 100.0);
        assert!(share <= 0.015, "case {case}: {:.3} % of pixels differ from the CPU", share * 100.0);
    }
}

/// An adapter whose render targets don't qualify (`R32Float` was not renderable on GL's llvmpipe;
/// the pipelines failed validation, which release builds only log): Advanced 3D renders on the
/// CPU, the GPU compositor's frame matches the CPU's, and no render pipeline is built (a
/// validation error panics under test).
#[test]
fn adapters_that_cannot_rasterise_render_advanced_3d_on_the_cpu() {
    crate::tests::hold_gpu_lock();
    let Some(mut ctx) = crate::context::GpuContext::headless() else { return };
    ctx.adv3d_raster = false;
    let g = Gpu::from_context(ctx);
    let (p, cid) = scene_project();
    let s = scene(&p, cid);
    assert!(g.raster_3d(&s).is_none(), "no GPU raster");
    let cpu = Renderer::new(&p, &NoFootage, RenderOpts::default()).comp_frame(cid, Tick::ZERO);
    let mut r = Renderer::new(&p, &NoFootage, RenderOpts { backend: effectcraft_render::Backend::Gpu, ..Default::default() });
    r.accel = Some(&g);
    let gimg = r.comp_frame(cid, Tick::ZERO);
    let worst = cpu.data.iter().zip(&gimg.data).flat_map(|(a, b)| (0..4).map(move |k| (a[k] - b[k]).abs())).fold(0.0f32, f32::max);
    assert!(worst <= 2.0 / 255.0, "GPU compositor with CPU Advanced 3D vs CPU: worst {worst}");
}
