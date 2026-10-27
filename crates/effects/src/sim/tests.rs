use super::*;
use crate::{EffectEnv, run_fx};

fn grey(w: u32, hh: u32) -> Image {
    Image::filled(w, hh, [0.2, 0.2, 0.2, 1.0])
}

fn ramp(w: u32, hh: u32) -> Image {
    let mut img = Image::new(w, hh);
    for y in 0..hh {
        for x in 0..w {
            img.set(x, y, [x as f32 / w as f32, y as f32 / hh as f32, 0.4, 1.0]);
        }
    }
    img
}

fn run(id: &str, vals: &[(&str, Value)], img: Image, t: f64) -> Image {
    run_fx(id, vals, img, t, EffectEnv::default()).img
}

fn diff(a: &Image, b: &Image) -> f32 {
    a.data.iter().zip(&b.data).map(|(p, q)| (0..4).map(|k| (p[k] - q[k]).abs()).sum::<f32>()).sum()
}

/// Centre of mass (luminance-weighted difference from `base`), `[-1, -1]` when equal.
fn mass(img: &Image, base: &Image) -> [f32; 2] {
    let (mut m, mut sx, mut sy) = (0.0, 0.0, 0.0);
    for y in 0..img.height {
        for x in 0..img.width {
            let i = img.idx(x, y);
            let d: f32 = (0..3).map(|k| (img.data[i][k] - base.data[i][k]).abs()).sum();
            m += d;
            sx += d * x as f32;
            sy += d * y as f32;
        }
    }
    if m > 0.0 { [sx / m, sy / m] } else { [-1.0; 2] }
}

/// [`mass`] in y.
fn mass_y(img: &Image, base: &Image) -> f32 {
    mass(img, base)[1]
}

#[test]
fn rasteriser_over_and_add() {
    let s = [Sprite::new(5.0, 5.0, 2.0, [1.0, 0.0, 0.0, 1.0], Shape::Disc)];
    let img = splat(10, 40, &s, Acc::Over);
    assert!(img.get(5, 5)[0] > 0.99 && img.get(9, 30)[3] == 0.0);
    let two = [s[0], s[0]];
    let add = splat(10, 40, &two, Acc::Add);
    assert!(add.get(5, 5)[0] > 1.9);
    // Items spanning band boundaries are drawn in every band.
    let tall = [Sprite::new(5.0, 16.0, 6.0, [0.0, 1.0, 0.0, 1.0], Shape::Square)];
    let t = splat(10, 40, &tall, Acc::Over);
    assert!(t.get(5, 13)[1] > 0.9 && t.get(5, 18)[1] > 0.9);
}

#[test]
fn rainfall_deterministic_and_falls() {
    let vals = [("drops", num(200.0)), ("opacity", num(100.0)), ("extras/appearance", Value::Enum(1)), ("color", col(1.0, 1.0, 1.0))];
    let a = run("ec.sim.ccrainfall", &vals, grey(40, 40), 0.5);
    let b = run("ec.sim.ccrainfall", &vals, grey(40, 40), 0.5);
    assert_eq!(a, b);
    assert!(diff(&a, &grey(40, 40)) > 1.0, "drops visible");
    let none = run("ec.sim.ccrainfall", &[("drops", num(0.0))], grey(40, 40), 0.5);
    assert_eq!(none, grey(40, 40));
    let later = run("ec.sim.ccrainfall", &vals, grey(40, 40), 0.52);
    assert!(diff(&a, &later) > 0.1, "rain moves");
}

#[test]
fn snowfall_deterministic_and_moves() {
    let vals = [("flakes", num(300.0))];
    let a = run("ec.sim.ccsnowfall", &vals, grey(40, 40), 1.0);
    assert_eq!(a, run("ec.sim.ccsnowfall", &vals, grey(40, 40), 1.0));
    assert!(diff(&a, &grey(40, 40)) > 1.0);
    assert!(diff(&a, &run("ec.sim.ccsnowfall", &vals, grey(40, 40), 1.2)) > 0.1);
    let off = run("ec.sim.ccsnowfall", &[("flakes", num(300.0)), ("compositeWithOriginal", Value::Bool(false))], grey(40, 40), 1.0);
    assert!(off.data.iter().any(|p| p[3] == 0.0), "without original the background is transparent");
}

#[test]
fn bubbles_rise() {
    let vals = [("bubbleAmount", num(30.0)), ("bubbleSize", num(3.0))];
    let a = run("ec.sim.ccbubbles", &vals, ramp(40, 40), 0.3);
    assert_eq!(a, run("ec.sim.ccbubbles", &vals, ramp(40, 40), 0.3));
    assert!(a.data.iter().any(|p| p[3] > 0.5));
    let none = run("ec.sim.ccbubbles", &[("bubbleAmount", num(0.0))], ramp(40, 40), 0.3);
    assert!(none.data.iter().all(|p| p[3] == 0.0), "bubbles replace the layer");
    // A single slow bubble moves up over a short time.
    let one = [("bubbleAmount", num(1.0)), ("bubbleSize", num(4.0)), ("wobbleAmplitude", num(0.0)), ("bubbleSpeed", num(0.2))];
    let blank = Image::new(40, 40);
    let y0 = mass_y(&run("ec.sim.ccbubbles", &one, ramp(40, 40), 0.0), &blank);
    let y1 = mass_y(&run("ec.sim.ccbubbles", &one, ramp(40, 40), 0.1), &blank);
    assert!(y0 >= 0.0 && y1 >= 0.0 && (y1 < y0 || y0 < 8.0), "{y0} -> {y1}");
}

#[test]
fn drizzle_ripples() {
    let a = run("ec.sim.ccdrizzle", &[], ramp(48, 48), 0.6);
    assert_eq!(a, run("ec.sim.ccdrizzle", &[], ramp(48, 48), 0.6));
    assert!(diff(&a, &ramp(48, 48)) > 0.1);
    let none = run("ec.sim.ccdrizzle", &[("dripRate", num(0.0))], ramp(48, 48), 0.6);
    assert_eq!(none, ramp(48, 48));
}

#[test]
fn star_burst_moves_stars() {
    let a = run("ec.sim.ccstarburst", &[("gridSpacing", num(4.0))], ramp(40, 40), 0.5);
    assert_eq!(a, run("ec.sim.ccstarburst", &[("gridSpacing", num(4.0))], ramp(40, 40), 0.5));
    assert!(a.data.iter().any(|p| p[3] > 0.0));
    let b = run("ec.sim.ccstarburst", &[("gridSpacing", num(4.0))], ramp(40, 40), 0.9);
    assert!(diff(&a, &b) > 0.5);
    let full = run("ec.sim.ccstarburst", &[("blendWithOriginal", num(100.0))], ramp(40, 40), 0.5);
    assert!(diff(&full, &ramp(40, 40)) < 1e-3);
}

fn pw_vals(seed: f64) -> Vec<(&'static str, Value)> {
    vec![
        ("birthRate", num(1.0)),
        ("extras/randomSeed", num(seed)),
        ("particle/particleType", Value::Enum(2)),
        ("particle/birthSize", num(0.3)),
        ("particle/deathSize", num(0.3)),
    ]
}

/// #1195: with the comp's shutter open, the particles smear across it: the average of the
/// sampled sprite fields differs from the sharp frame, touches more pixels and keeps the
/// alpha energy.
#[test]
fn particle_world_motion_blur_trails() {
    let v = pw_vals(21.0);
    let blank = Image::new(40, 40);
    let sharp = run("ec.sim.ccparticleworld", &v, blank.clone(), 2.0);
    let blurred = crate::run_fx(
        "ec.sim.ccparticleworld",
        &v,
        blank.clone(),
        2.0,
        EffectEnv { frame_rate: 10.0, shutter: Some((360.0, -180.0, 8)), ..Default::default() },
    )
    .img;
    assert!(sharp.data.iter().any(|p| p[3] > 0.0), "sharp frame has particles");
    assert_ne!(sharp.data, blurred.data, "the shutter smears the particles");
    let lit = |img: &Image| img.data.iter().filter(|p| p[3] > 0.005).count();
    let sum = |img: &Image| img.data.iter().map(|p| p[3]).sum::<f32>();
    assert!(lit(&blurred) > lit(&sharp), "trails light more pixels: {} vs {}", lit(&blurred), lit(&sharp));
    let (sa, ba) = (sum(&sharp), sum(&blurred));
    assert!((ba - sa).abs() < sa * 0.6 + 1.0, "alpha energy roughly kept: {ba} vs {sa}");
}

#[test]
fn particle_world_seek_consistent() {
    let v = pw_vals(11.0);
    let direct = run("ec.sim.ccparticleworld", &v, Image::new(40, 40), 2.0);
    let v2 = pw_vals(12.0);
    let _ = run("ec.sim.ccparticleworld", &v2, Image::new(40, 40), 1.0);
    let stepped = run("ec.sim.ccparticleworld", &v2, Image::new(40, 40), 2.0);
    let direct2 = run("ec.sim.ccparticleworld", &v2, Image::new(40, 40), 2.0);
    assert_eq!(stepped, direct2);
    assert!(direct.data.iter().any(|p| p[3] > 0.0), "particles alive");
    // Before birth nothing is drawn.
    let t0 = run("ec.sim.ccparticleworld", &v, Image::new(40, 40), 0.0);
    assert!(t0.data.iter().all(|p| p[3] == 0.0));
}

#[test]
fn engine_resume_equals_from_scratch() {
    for anim in 0..PW_ANIMS.len() as u32 {
        let phys = Phys {
            rate: 90.0,
            life: 1.5,
            producer: [0.0; 3],
            radius: [0.05; 3],
            anim,
            speed: 0.5,
            gravity: [0.0, 0.5, 0.0],
            resistance: 0.2,
            axis: [0.0, -1.0, 0.0],
            extra: 1.0,
            extra_angle: 60.0,
            seed: 9,
            max: 10_000,
            flat: anim % 2 == 0,
        };
        let a: SimCache<PState> = SimCache::new(2);
        let b: SimCache<PState> = SimCache::new(2);
        let direct = simulate(&a, 1, 2.0, &phys);
        let _ = simulate(&b, 1, 1.0, &phys);
        let _ = simulate(&b, 1, 0.37, &phys);
        let resumed = simulate(&b, 1, 2.0, &phys);
        assert_eq!(direct.parts.len(), resumed.parts.len(), "anim {anim}");
        assert!(direct.parts.iter().zip(&resumed.parts).all(|(p, q)| p.p == q.p && p.v == q.v && p.id == q.id), "anim {anim}");
        assert!(!direct.parts.is_empty());
    }
}

#[test]
fn particle_world_gravity_pulls_down() {
    let base = |g: f64| {
        vec![
            ("birthRate", num(2.0)),
            ("physics/velocity", num(0.2)),
            ("physics/gravity", num(g)),
            ("particle/particleType", Value::Enum(2)),
            ("particle/birthSize", num(0.2)),
            ("particle/deathSize", num(0.2)),
            ("longevity", num(3.0)),
        ]
    };
    let blank = Image::new(48, 48);
    let up = mass_y(&run("ec.sim.ccparticleworld", &base(0.0), blank.clone(), 1.0), &blank);
    let down = mass_y(&run("ec.sim.ccparticleworld", &base(2.0), blank.clone(), 1.0), &blank);
    assert!(down > up + 2.0, "{up} {down}");
}

/// A comp whose camera is `camera` ([`crate::CompScene::camera`]); `layer`: a camera layer
/// (else the comp's default view).
struct Comp {
    camera: [[f64; 4]; 3],
    layer: bool,
}
impl crate::EffectHost for Comp {
    fn layer(&self, _: u64, _: bool) -> Option<crate::LayerPixels> {
        None
    }
    fn audio(&self, _: u64, _: f64, _: usize, _: u32) -> Option<Vec<f32>> {
        None
    }
    fn comp_scene(&self) -> Option<crate::CompScene> {
        Some(crate::CompScene { camera: Some(self.camera), camera_layer: self.layer, light: None })
    }
}

/// A camera `d` px in front of layer point `c` looking at it, moved right by `dx` px and
/// zoomed by `zoom` (1 = identity on the layer plane): `c + zoom·d·(p − c − dx) / (z + d)`.
fn cam(c: [f64; 2], d: f64, dx: f64, zoom: f64) -> [[f64; 4]; 3] {
    let k = zoom * d;
    [[k, 0.0, c[0], d * c[0] - k * (c[0] + dx)], [0.0, k, c[1], d * c[1] - k * c[1]], [0.0, 0.0, 1.0, d]]
}

fn pw_in(comp: &Comp, vals: &[(&str, Value)]) -> Image {
    run_fx("ec.sim.ccparticleworld", vals, Image::new(48, 48), 1.0, EffectEnv { host: Some(comp), ..Default::default() }).img
}

fn finite(img: &Image) -> bool {
    img.data.iter().all(|p| p.iter().all(|c| c.is_finite()))
}

/// #397: with a camera layer in the comp, the particles are seen through it.
#[test]
fn particle_world_follows_the_comp_camera() {
    let v = pw_vals(3.0);
    let blank = Image::new(48, 48);
    let here = Comp { camera: cam([24.0, 24.0], 96.0, 0.0, 1.0), layer: true };
    let base = pw_in(&here, &v);
    assert!(base.data.iter().any(|p| p[3] > 0.0), "particles alive");
    // Moving the camera right moves the particles left.
    let moved = pw_in(&Comp { camera: cam([24.0, 24.0], 96.0, 8.0, 1.0), ..here }, &v);
    let (a, b) = (mass(&base, &blank), mass(&moved, &blank));
    assert!(b[0] < a[0] - 4.0 && (b[1] - a[1]).abs() < 2.0, "{a:?} -> {b:?}");
    // Zooming in spreads them out.
    let zoomed = pw_in(&Comp { camera: cam([24.0, 24.0], 96.0, 0.0, 2.0), ..here }, &v);
    let cover = |i: &Image| i.data.iter().filter(|p| p[3] > 0.01).count();
    assert!(cover(&zoomed) > cover(&base), "{} {}", cover(&zoomed), cover(&base));
    // The Effect Camera is ignored then.
    let mut turned = v.clone();
    turned.push(("extras/effectCamera/rotationY", num(60.0)));
    assert_eq!(pw_in(&here, &turned), base);
    // The comp's default view (no camera layer) leaves it to the Effect Camera.
    let default_view = Comp { layer: false, ..here };
    assert_eq!(pw_in(&default_view, &v), run("ec.sim.ccparticleworld", &v, blank.clone(), 1.0));
    // A camera facing away, or a degenerate one, draws nothing and never NaN.
    for camera in [[[96.0, 0.0, 24.0, 0.0], [0.0, 96.0, 24.0, 0.0], [0.0, 0.0, -1.0, -96.0]], [[0.0; 4]; 3], [[f64::NAN; 4]; 3]] {
        assert_eq!(pw_in(&Comp { camera, layer: true }, &v), blank);
    }
    // A camera among the particles draws the ones in front of it.
    let inside = pw_in(&Comp { camera: cam([24.0, 24.0], 1.0, 0.0, 1.0), ..here }, &v);
    assert!(finite(&inside));
}

/// Extras ▸ Effect Camera: the defaults are the fixed camera Particle World always had.
#[test]
fn particle_world_effect_camera() {
    let v = pw_vals(5.0);
    let base = run("ec.sim.ccparticleworld", &v, Image::new(48, 48), 1.0);
    let with = |extra: &[(&'static str, Value)]| {
        let mut w = v.clone();
        w.extend_from_slice(extra);
        run("ec.sim.ccparticleworld", &w, Image::new(48, 48), 1.0)
    };
    // Missing parameters (projects saved before) read as the defaults.
    let spec = crate::find("ec.sim.ccparticleworld").unwrap();
    let mut params = crate::Params { values: spec.params.iter().map(|p| (p.id.to_string(), p.default.clone())).collect() };
    params.values.retain(|k, _| !k.starts_with(PW_EFFECT_CAMERA));
    for (k, val) in &v {
        params.values.insert(k.to_string(), val.clone());
    }
    let ctx = EffectCtx { params: &params, time: 1.0, layer_size: [48.0, 48.0], seed: 1, adjustment: false, env: EffectEnv::default() };
    assert_eq!(crate::apply(spec, &ctx, Buf { img: Image::new(48, 48), offset: [0.0; 2], scale: 1.0 }).img, base);
    assert_eq!(with(&[("extras/effectCamera/fov", num(f64::NAN))]), base);
    // Turning or pulling back the camera changes the view.
    let turned = with(&[("extras/effectCamera/rotationY", num(60.0))]);
    assert!(diff(&turned, &base) > 1.0);
    let cover = |i: &Image| i.data.iter().filter(|p| p[3] > 0.01).count();
    let far = with(&[("extras/effectCamera/distance", num(4.0))]);
    assert!(cover(&far) < cover(&base), "{} {}", cover(&far), cover(&base));
    assert!(diff(&with(&[("extras/effectCamera/fov", num(120.0))]), &base) > 0.1);
    // Extremes never panic or produce NaN.
    for extra in [
        ("extras/effectCamera/distance", num(0.0)),
        ("extras/effectCamera/distance", num(-5.0)),
        ("extras/effectCamera/fov", num(0.0)),
        ("extras/effectCamera/fov", num(180.0)),
        ("extras/effectCamera/rotationX", num(1e12)),
    ] {
        assert!(finite(&with(std::slice::from_ref(&extra))), "{extra:?}");
    }
    // Moving the camera doesn't restart the simulation.
    let key =
        |pr: &crate::Params| sim_key(&EffectCtx { params: pr, time: 1.0, layer_size: [48.0, 48.0], seed: 1, adjustment: false, env: EffectEnv::default() }, 1);
    let mut moved = params.clone();
    moved.values.insert("extras/effectCamera/rotationZ".into(), num(30.0));
    assert_eq!(key(&params), key(&moved));
    moved.values.insert("extras/randomSeed".into(), num(9.0));
    assert_ne!(key(&params), key(&moved));
}

#[test]
fn particle_systems2_seek_and_direction() {
    let v = vec![
        ("producer/position", pt(24.0, 24.0)),
        ("birthRate", num(3.0)),
        ("physics/gravity", num(0.0)),
        ("physics/velocity", num(1.0)),
        ("physics/animation", Value::Enum(1)),
    ];
    let _ = run("ec.sim.ccparticlesystems2", &v, Image::new(48, 48), 0.5);
    let a = run("ec.sim.ccparticlesystems2", &v, Image::new(48, 48), 1.0);
    let mut v2 = v.clone();
    v2.push(("randomSeed", num(0.0)));
    assert_eq!(a, run("ec.sim.ccparticlesystems2", &v2, Image::new(48, 48), 1.0));
    // Direction 0° (up): particles above the producer.
    let blank = Image::new(48, 48);
    let y = mass_y(&a, &blank);
    assert!((0.0..24.0).contains(&y), "{y}");
    // The particles replace the layer: with none born it is empty.
    let none = run("ec.sim.ccparticlesystems2", &[("birthRate", num(0.0))], grey(20, 20), 1.0);
    assert_eq!(none, Image::new(20, 20));
}

/// CC Particle World and CC Particle Systems II replace their layer with the particles, in
/// every Transfer Mode (which only sets how particles meet each other). SceneYard frames from
/// After Effects 2026 show no trace of the source solid: "Task Preparation UI" (black solids
/// under fireworks hid a white page), "Burnout matchstick" (a red rectangle around the
/// flame), "Documentary Man Falling" (a black block over the light).
#[test]
fn cc_particles_replace_their_layer() {
    let red = || {
        let mut img = Image::new(48, 48);
        img.data.iter_mut().for_each(|p| *p = [0.7, 0.1, 0.1, 1.0]);
        img
    };
    for id in ["ec.sim.ccparticlesystems2", "ec.sim.ccparticleworld"] {
        for mode in 0..4 {
            let v = vec![("birthRate", num(3.0)), ("particle/transferMode", Value::Enum(mode))];
            let _ = run(id, &v, red(), 0.5);
            let out = run(id, &v, red(), 1.0);
            // No source left: the corners, far from the producer, are clear.
            for (x, y) in [(0, 0), (47, 0), (0, 47), (47, 47)] {
                assert!(out.get(x, y)[3] < 1e-6, "{id} mode {mode} ({x},{y}): {:?}", out.get(x, y));
            }
            assert!(out.data.iter().any(|p| p[3] > 0.05), "{id} mode {mode}: particles drawn");
        }
    }
}

#[test]
fn mercury_blobs_and_seek() {
    let v = vec![("producer", pt(24.0, 16.0)), ("radiusX", num(5.0)), ("radiusY", num(5.0)), ("birthRate", num(3.0))];
    let _ = run("ec.sim.ccmrmercury", &v, ramp(48, 48), 0.7);
    let a = run("ec.sim.ccmrmercury", &v, ramp(48, 48), 1.4);
    let mut v2 = v.clone();
    v2.push(("shading/metal", num(100.0)));
    let b = run("ec.sim.ccmrmercury", &v2, ramp(48, 48), 1.4);
    assert_eq!(a, b);
    assert!(a.data.iter().any(|p| p[3] > 0.5), "blobs visible");
    let none = run("ec.sim.ccmrmercury", &[("birthRate", num(0.0))], ramp(48, 48), 1.4);
    assert!(none.data.iter().all(|p| p[3] == 0.0));
}

#[test]
fn half_resolution_scales_sprites() {
    // Downsampled buffers render the same layout at half size.
    let s = crate::find("ec.sim.ccsnowfall").unwrap();
    let mut params = crate::Params { values: s.params.iter().map(|p| (p.id.to_string(), p.default.clone())).collect() };
    params.values.insert("flakes".into(), num(50.0));
    params.values.insert("offset".into(), pt(20.0, 20.0));
    let ctx = EffectCtx { params: &params, time: 0.5, layer_size: [40.0, 40.0], seed: 1, adjustment: false, env: EffectEnv::default() };
    let full = crate::apply(s, &ctx, Buf { img: grey(40, 40), offset: [0.0; 2], scale: 1.0 });
    let half = crate::apply(s, &ctx, Buf { img: grey(20, 20), offset: [0.0; 2], scale: 0.5 });
    let sum = |i: &Image| i.data.iter().map(|p| p[0]).sum::<f32>() / i.data.len() as f32;
    assert!((sum(&full.img) - sum(&half.img)).abs() < 0.05, "{} {}", sum(&full.img), sum(&half.img));
}
