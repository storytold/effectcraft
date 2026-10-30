use super::*;
use crate::{EffectEnv, run_fx};

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

fn max_diff(a: &Image, b: &Image) -> f32 {
    a.data.iter().zip(&b.data).map(|(p, q)| (0..4).map(|k| (p[k] - q[k]).abs()).fold(0.0, f32::max)).fold(0.0, f32::max)
}

fn sum_diff(a: &Image, b: &Image) -> f32 {
    a.data.iter().zip(&b.data).map(|(p, q)| (0..4).map(|k| (p[k] - q[k]).abs()).sum::<f32>()).sum()
}

#[test]
fn piece_at_rest_is_identity() {
    let src = ramp(16, 16);
    let cam = Cam { c: [8.0, 8.0], d: 64.0 };
    let id = rot_axis([1.0, 0.0, 0.0], 0.0);
    let pc = Piece::new(&[[0.0, 0.0], [16.0, 0.0], [16.0, 16.0], [0.0, 16.0]], [8.0, 8.0], &id, [8.0, 8.0, 0.0], cam).unwrap();
    let mut out = Image::new(16, 16);
    draw_pieces(&mut out, &[pc], &src, None);
    assert!(max_diff(&out, &src) < 1e-4);
}

#[test]
fn ball_action_draws_balls() {
    let v = [("ballSize", num(80.0))];
    let a = run("ec.sim.ccballaction", &v, ramp(32, 32), 0.0);
    assert_eq!(a, run("ec.sim.ccballaction", &v, ramp(32, 32), 0.0));
    assert!(a.data.iter().any(|p| p[3] > 0.9) && a.data.iter().any(|p| p[3] == 0.0), "balls with gaps");
    let none = run("ec.sim.ccballaction", &[("ballSize", num(0.0))], ramp(32, 32), 0.0);
    assert!(none.data.iter().all(|p| p[3] == 0.0));
    let sc = run("ec.sim.ccballaction", &[("ballSize", num(80.0)), ("scatter", num(20.0))], ramp(32, 32), 0.0);
    assert!(sum_diff(&a, &sc) > 1.0);
}

#[test]
fn pixel_polly_breaks_after_start() {
    let v = [("forceCenter", pt(16.0, 16.0)), ("gridSpacing", num(4.0))];
    let before = run("ec.sim.ccpixelpolly", &v, ramp(32, 32), 0.0);
    assert_eq!(before, ramp(32, 32), "intact before the start time");
    let a = run("ec.sim.ccpixelpolly", &v, ramp(32, 32), 0.5);
    assert_eq!(a, run("ec.sim.ccpixelpolly", &v, ramp(32, 32), 0.5));
    assert!(sum_diff(&a, &ramp(32, 32)) > 10.0);
    let tex = run("ec.sim.ccpixelpolly", &[("forceCenter", pt(16.0, 16.0)), ("gridSpacing", num(4.0)), ("object", Value::Enum(3))], ramp(32, 32), 0.5);
    assert!(sum_diff(&a, &tex) > 0.1);
}

#[test]
fn scatterize_identity_and_scatter() {
    assert_eq!(run("ec.sim.ccscatterize", &[], ramp(24, 24), 0.0), ramp(24, 24));
    let a = run("ec.sim.ccscatterize", &[("scatter", num(5.0))], ramp(24, 24), 0.0);
    assert_eq!(a, run("ec.sim.ccscatterize", &[("scatter", num(5.0))], ramp(24, 24), 0.0));
    assert!(sum_diff(&a, &ramp(24, 24)) > 1.0);
    let tw = run("ec.sim.ccscatterize", &[("leftTwist", num(80.0)), ("rightTwist", num(80.0))], ramp(24, 24), 0.0);
    assert!(sum_diff(&tw, &ramp(24, 24)) > 1.0);
}

#[test]
fn card_dance_identity_and_rotation() {
    let src = ramp(30, 20);
    let a = run("ec.sim.carddance", &[], src.clone(), 0.0);
    assert!(max_diff(&a, &src) < 0.02, "flat cards reproduce the layer: {}", max_diff(&a, &src));
    let r = run("ec.sim.carddance", &[("yRotation/yRotSource", Value::Enum(1)), ("yRotation/yRotMultiplier", num(90.0))], src.clone(), 0.0);
    assert_eq!(r, run("ec.sim.carddance", &[("yRotation/yRotSource", Value::Enum(1)), ("yRotation/yRotMultiplier", num(90.0))], src.clone(), 0.0));
    assert!(sum_diff(&r, &src) > 5.0);
    let z = run("ec.sim.carddance", &[("zPosition/zPosOffset", num(-1.0))], src.clone(), 0.0);
    assert!(sum_diff(&z, &src) > 5.0, "cards pulled towards the camera grow");
}

#[test]
fn shatter_at_rest_then_breaks() {
    let v = [("view", Value::Enum(0)), ("force1/force1Position", pt(16.0, 16.0)), ("shape/origin", pt(16.0, 16.0))];
    let src = ramp(32, 32);
    let t0 = run("ec.sim.shatter", &v, src.clone(), 0.0);
    assert!(max_diff(&t0, &src) < 0.03, "{}", max_diff(&t0, &src));
    let t1 = run("ec.sim.shatter", &v, src.clone(), 0.5);
    assert_eq!(t1, run("ec.sim.shatter", &v, src.clone(), 0.5));
    assert!(sum_diff(&t1, &src) > 10.0);
    for pat in 0..SHATTER_PATTERNS.len() as u32 {
        let vv = [("view", Value::Enum(0)), ("shape/pattern", Value::Enum(pat)), ("force1/force1Position", pt(16.0, 16.0)), ("shape/origin", pt(16.0, 16.0))];
        let r = run("ec.sim.shatter", &vv, src.clone(), 0.0);
        assert!(max_diff(&r, &src) < 0.05, "pattern {pat} tiles the layer: {}", max_diff(&r, &src));
    }
    let wire = run("ec.sim.shatter", &[("shape/repetitions", num(3.0))], src.clone(), 0.0);
    assert!(wire.data.iter().any(|p| p[3] > 0.5) && wire.data.iter().any(|p| p[3] == 0.0));
    let layer_only =
        run("ec.sim.shatter", &[("view", Value::Enum(0)), ("render", Value::Enum(1)), ("force1/force1Position", pt(16.0, 16.0))], src.clone(), 0.5);
    assert!(layer_only.get(16, 16)[3] == 0.0, "broken centre removed");
}

#[test]
fn caustics_flat_water() {
    let src = ramp(24, 24);
    let a = run(
        "ec.sim.caustics",
        &[
            ("water/surfaceOpacity", num(0.0)),
            ("lighting/ambientLight", num(0.0)),
            ("material/diffuse", num(1.0)),
            ("lighting/lightHeight", num(1000.0)),
            ("material/specular", num(0.0)),
        ],
        src.clone(),
        0.0,
    );
    assert!(max_diff(&a, &src) < 0.01, "flat water, light overhead = bottom: {}", max_diff(&a, &src));
    let b = run("ec.sim.caustics", &[], src.clone(), 0.0);
    assert_eq!(b, run("ec.sim.caustics", &[], src.clone(), 0.0));
    assert!(sum_diff(&b, &src) > 0.1);
}

#[test]
fn caustics_sky_and_light_types() {
    let src = Image::filled(32, 32, [0.5, 0.5, 0.5, 1.0]);
    let env = EffectEnv { host: Some(&FoamHost), ..Default::default() };
    let run_h = |vals: &[(&str, Value)]| run_fx("ec.sim.caustics", vals, src.clone(), 0.0, env).img;
    // A mirror-like surface shows the (green) sky.
    let mirror = [("water/surfaceOpacity", num(1.0)), ("sky/sky", Value::Layer(Some(2))), ("sky/intensity", num(1.0))];
    let p = run_h(&mirror).get(16, 16);
    assert!(p[1] > p[0] + 0.3 && p[1] > p[2] + 0.3, "{p:?}");
    // Point Source lights pixels by their own direction to the light: the image is no
    // longer uniform; Distant Source lights the flat water evenly.
    let lit = |t: u32| {
        run_h(&[
            ("lighting/lightType", Value::Enum(t)),
            ("lighting/lightPosition", pt(0.0, 0.0)),
            ("lighting/lightHeight", num(0.2)),
            ("water/surfaceOpacity", num(0.0)),
        ])
    };
    let (distant, point) = (lit(0), lit(1));
    assert!((distant.get(2, 2)[0] - distant.get(30, 30)[0]).abs() < 1e-5);
    assert!(point.get(2, 2)[0] > point.get(30, 30)[0] + 0.02, "{:?} {:?}", point.get(2, 2), point.get(30, 30));
    // First Comp Light without a comp light: ambient only.
    let none = run_h(&[("lighting/lightType", Value::Enum(2)), ("water/surfaceOpacity", num(0.0))]);
    assert!(none.get(16, 16)[0] < distant.get(16, 16)[0]);
}

#[test]
fn wave_world_wireframe_controls_ground_and_dry_areas() {
    let base = [("producer1/producer1Position", pt(8.0, 16.0)), ("simulation/gridResolution", num(24.0))];
    let wire = |extra: &[(&str, Value)]| run("ec.sim.waveworld", &[base.as_slice(), extra].concat(), Image::new(32, 32), 0.5);
    assert_ne!(wire(&[]), wire(&[("wireframeControls/horizontalRotation", num(40.0))]));
    assert_ne!(wire(&[]), wire(&[("wireframeControls/verticalScale", num(3.0))]));
    // Ground (left dark, right light): the light side rises above the water and is dry.
    let env = EffectEnv { host: Some(&FoamHost), ..Default::default() };
    let hm = |extra: &[(&str, Value)]| {
        let mut v: Vec<(&str, Value)> = base.to_vec();
        v.extend([("view", Value::Enum(1)), ("ground/ground", Value::Layer(Some(1))), ("ground/steepness", num(0.5)), ("ground/height", num(0.25))]);
        v.extend_from_slice(extra);
        run_fx("ec.sim.waveworld", &v, Image::new(32, 32), 1.0, env).img
    };
    let solid = hm(&[]);
    let clear = hm(&[("heightMapControls/renderDryAreasAs", Value::Enum(1))]);
    assert!(solid.get(30, 16)[3] > 0.99 && clear.get(30, 16)[3] == 0.0);
    assert!(clear.get(4, 16)[3] > 0.99, "wet side stays");
    // Dry ground stays flat: mid grey (brightness 0.5) there.
    assert!((solid.get(30, 16)[0] - 0.5).abs() < 1e-4);
    // Wave Strength slows waves over shallow water: the surface differs.
    assert_ne!(hm(&[]), hm(&[("ground/waveStrength", num(1.0))]));
}

#[test]
fn wave_world_seek_and_waves() {
    let v = vec![("view", Value::Enum(1)), ("producer1/producer1Position", pt(16.0, 16.0)), ("simulation/gridResolution", num(24.0))];
    let _ = run("ec.sim.waveworld", &v, Image::new(32, 32), 0.5);
    let a = run("ec.sim.waveworld", &v, Image::new(32, 32), 1.0);
    let mut v2 = v.clone();
    v2.push(("heightMapControls/transparency", num(0.0)));
    assert_eq!(a, run("ec.sim.waveworld", &v2, Image::new(32, 32), 1.0));
    // Waves spread: some pixels differ from the flat-water grey.
    assert!(a.data.iter().any(|p| (p[0] - 0.5).abs() > 0.05), "waves");
    let flat = run("ec.sim.waveworld", &[("view", Value::Enum(1)), ("producer1/producer1Amplitude", num(0.0))], Image::new(32, 32), 1.0);
    assert!(flat.data.iter().all(|p| (p[0] - 0.5).abs() < 1e-5));
    let wire = run("ec.sim.waveworld", &[("producer1/producer1Position", pt(16.0, 16.0)), ("simulation/gridResolution", num(10.0))], Image::new(32, 32), 0.3);
    assert!(wire.data.iter().any(|p| p[1] > 0.3));
}

#[test]
fn wave_world_steps_match_direct() {
    // Fresh caches: resumed simulation equals simulation from scratch.
    let v = vec![
        ("view", Value::Enum(1)),
        ("producer1/producer1Position", pt(16.0, 16.0)),
        ("simulation/gridResolution", num(20.0)),
        ("simulation/damping", num(0.07)),
    ];
    let a = run("ec.sim.waveworld", &v, Image::new(32, 32), 1.3);
    let mut v2 = v.clone();
    v2.push(("simulation/damping", num(0.07000001)));
    // Different key: computed from scratch at 1.3 directly vs via 0.4 (both fresh keys).
    let _ = run("ec.sim.waveworld", &v2, Image::new(32, 32), 0.4);
    let b = run("ec.sim.waveworld", &v2, Image::new(32, 32), 1.3);
    assert!(max_diff(&a, &b) < 1e-3, "{}", max_diff(&a, &b));
}

#[test]
fn foam_bubbles_drift_and_seek() {
    let v = vec![("producer/producerPoint", pt(16.0, 16.0)), ("view", Value::Enum(2)), ("producer/productionRate", num(2.0))];
    let none = run("ec.sim.foam", &v, Image::new(32, 32), 0.0);
    assert!(none.data.iter().all(|p| p[3] == 0.0));
    let _ = run("ec.sim.foam", &v, Image::new(32, 32), 0.6);
    let a = run("ec.sim.foam", &v, Image::new(32, 32), 1.2);
    let mut v2 = v.clone();
    v2.push(("randomSeed", num(1.0)));
    assert_eq!(a, run("ec.sim.foam", &v2, Image::new(32, 32), 1.2));
    assert!(a.data.iter().any(|p| p[3] > 0.05), "bubbles");
    let draft = run("ec.sim.foam", &[("producer/producerPoint", pt(16.0, 16.0)), ("producer/productionRate", num(2.0))], Image::new(32, 32), 1.0);
    assert!(draft.data.iter().any(|p| p[3] > 0.05));
}

#[test]
fn foam_strength_controls_early_popping() {
    // Strong bubbles all live their full lifespan; weak ones pop at random earlier.
    assert_eq!(foam_pop_age(7, 300.0, 0.0, 1), 300.0);
    let early = (0..200).filter(|&id| foam_pop_age(id, 300.0, 1.0, 1) < 150.0).count();
    assert!(early > 60 && early < 140, "{early}");
    let count = |strength: f64| {
        let v = vec![
            ("producer/producerPoint", pt(16.0, 16.0)),
            ("producer/productionRate", num(3.0)),
            ("bubbles/lifespan", num(40.0)),
            ("bubbles/strength", num(strength)),
            ("physics/windSpeed", num(0.0)),
        ];
        run("ec.sim.foam", &v, Image::new(32, 32), 1.5).data.iter().map(|p| p[3]).sum::<f32>()
    };
    assert!(count(0.0) < count(20.0) * 0.9, "{} vs {}", count(0.0), count(20.0));
}

/// A host with a comp camera (identity, or tilted) and a first comp light.
struct Scene {
    camera: Option<[[f64; 4]; 3]>,
    light: Option<crate::CompLight>,
    layer: Option<Image>,
}
impl crate::EffectHost for Scene {
    fn layer(&self, _: u64, _: bool) -> Option<crate::LayerPixels> {
        let img = self.layer.clone()?;
        let size = [img.width as f64, img.height as f64];
        Some(crate::LayerPixels { buf: Buf { img, offset: [0.0; 2], scale: 1.0 }, size })
    }
    fn audio(&self, _: u64, _: f64, _: usize, _: u32) -> Option<Vec<f32>> {
        None
    }
    fn comp_scene(&self) -> Option<crate::CompScene> {
        Some(crate::CompScene { camera: self.camera, camera_layer: true, light: self.light })
    }
}

fn run_env(id: &str, vals: &[(&str, Value)], img: Image, t: f64, host: &Scene) -> Image {
    run_fx(id, vals, img, t, EffectEnv { host: Some(host), ..Default::default() }).img
}

#[test]
fn card_dance_camera_systems() {
    let src = ramp(40, 30);
    // Camera Position: rotating the camera changes the view; zero rotation is the layer.
    let r = run("ec.sim.carddance", &[("cameraPosition/yRotation", num(30.0))], src.clone(), 0.0);
    assert!(sum_diff(&r, &src) > 10.0);
    let order = run("ec.sim.carddance", &[("cameraPosition/yRotation", num(30.0)), ("cameraPosition/xRotation", num(30.0))], src.clone(), 0.0);
    let order2 = run(
        "ec.sim.carddance",
        &[("cameraPosition/yRotation", num(30.0)), ("cameraPosition/xRotation", num(30.0)), ("cameraPosition/transformOrder", Value::Enum(5))],
        src.clone(),
        0.0,
    );
    assert!(sum_diff(&order, &order2) > 1.0, "Transform Order matters");
    // X, Y Position moves the view: the layer shifts by the offset.
    let moved = run("ec.sim.carddance", &[("cameraPosition/xyPosition", pt(30.0, 15.0))], src.clone(), 0.0);
    assert!((moved.get(25, 10)[0] - src.get(15, 10)[0]).abs() < 0.03, "{:?} {:?}", moved.get(25, 10), src.get(15, 10));
    // Corner Pins: the layer's corners land on the pins.
    let pins = [
        ("cameraSystem", Value::Enum(1)),
        ("cornerPins/upperLeftCorner", pt(10.0, 5.0)),
        ("cornerPins/upperRightCorner", pt(30.0, 5.0)),
        ("cornerPins/lowerLeftCorner", pt(10.0, 25.0)),
        ("cornerPins/lowerRightCorner", pt(30.0, 25.0)),
    ];
    let cp = run("ec.sim.carddance", &pins, src.clone(), 0.0);
    assert_eq!(cp.get(3, 3)[3], 0.0, "outside the pinned quad");
    assert!(cp.get(20, 15)[3] > 0.99);
    assert!((cp.get(20, 15)[0] - src.get(20, 15)[0]).abs() < 0.05, "centre maps to centre");
    // Comp Camera: the identity camera reproduces the layer; a camera that doubles x moves it.
    let ident = Scene { camera: Some([[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 0.002, 1.0]]), light: None, layer: None };
    let cc = run_env("ec.sim.carddance", &[("cameraSystem", Value::Enum(2))], src.clone(), 0.0, &ident);
    assert!(max_diff(&cc, &src) < 0.02, "{}", max_diff(&cc, &src));
    let half = Scene { camera: Some([[0.5, 0.0, 0.0, 0.0], [0.0, 0.5, 0.0, 0.0], [0.0, 0.0, 0.002, 1.0]]), ..ident };
    let cc = run_env("ec.sim.carddance", &[("cameraSystem", Value::Enum(2))], src.clone(), 0.0, &half);
    assert_eq!(cc.get(35, 25)[3], 0.0, "half-size view leaves the far corner empty");
}

#[test]
fn card_dance_lighting_and_material() {
    let src = Image::filled(40, 30, [0.5, 0.5, 0.5, 1.0]);
    let tilt = [("rows", num(1.0)), ("columns", num(1.0)), ("yRotation/yRotSource", Value::Enum(0)), ("yRotation/yRotOffset", num(60.0))];
    let lit = |extra: &[(&str, Value)]| {
        let mut v: Vec<(&str, Value)> = tilt.to_vec();
        v.extend_from_slice(extra);
        run("ec.sim.carddance", &v, src.clone(), 0.0).get(20, 15)
    };
    let base = lit(&[]);
    // Tilted cards facing away from a frontal light get darker than flat ones.
    assert!(base[0] < 0.45, "{base:?}");
    // Light colour tints; intensity brightens; ambient alone lights evenly.
    let red = lit(&[("lighting/lightColor", col(1.0, 0.0, 0.0))]);
    assert!(red[0] > red[1] + 0.05);
    assert!(lit(&[("lighting/lightIntensity", num(2.0))])[0] > base[0]);
    let amb = lit(&[("lighting/ambientLight", num(1.0)), ("material/diffuse", num(0.0))]);
    assert!((amb[0] - 0.5).abs() < 1e-3);
    // A light placed to the side (Point Source) changes the shading of the tilted card.
    let side = lit(&[("lighting/lightType", Value::Enum(1)), ("lighting/lightPosition", pt(200.0, 15.0)), ("lighting/lightDepth", num(0.2))]);
    assert!((side[0] - base[0]).abs() > 0.02, "{side:?} vs {base:?}");
    // Specular adds a highlight on flat, frontally lit cards.
    let flat = run("ec.sim.carddance", &[("material/specular", num(0.5))], src.clone(), 0.0).get(20, 15);
    assert!(flat[0] > 0.9, "{flat:?}");
    // First Comp Light: no light in the comp → ambient only; a light from the host is used.
    let none = Scene { camera: None, light: None, layer: None };
    let a = run_env("ec.sim.carddance", &[("lighting/lightType", Value::Enum(2))], src.clone(), 0.0, &none).get(20, 15);
    assert!((a[0] - 0.5 * (0.25 + 0.75)).abs() < 1e-3, "{a:?}");
    let blue = Scene { light: Some(crate::CompLight { pos: [20.0, 15.0, -100.0], dir: [0.0, 0.0, 1.0], color: [0.0, 0.0, 1.0], kind: 0 }), ..none };
    let a = run_env("ec.sim.carddance", &[("lighting/lightType", Value::Enum(2))], src.clone(), 0.0, &blue).get(20, 15);
    assert!(a[2] > a[0] + 0.2, "{a:?}");
}

#[test]
fn shatter_custom_map_gradient_textures_and_extrusion() {
    let src = ramp(32, 32);
    let base = [("view", Value::Enum(0)), ("force1/force1Position", pt(16.0, 16.0)), ("force1/force1Radius", num(2.0)), ("shape/origin", pt(16.0, 16.0))];
    let with = |extra: &[(&str, Value)], t: f64, host: Option<&Scene>| {
        let mut v: Vec<(&str, Value)> = base.to_vec();
        v.extend_from_slice(extra);
        match host {
            Some(h) => run_env("ec.sim.shatter", &v, src.clone(), t, h),
            None => run("ec.sim.shatter", &v, src.clone(), t),
        }
    };
    // Custom Shatter Map: a map white on the left and black on the right; with White Tiles
    // Fixed the left half stays put while the right half breaks away.
    let mut map = Image::new(32, 32);
    for y in 0..32 {
        for x in 0..32 {
            map.set(x, y, if x < 16 { [1.0; 4] } else { [0.0, 0.0, 0.0, 1.0] });
        }
    }
    let host = Scene { camera: None, light: None, layer: Some(map) };
    let custom = [("shape/pattern", Value::Enum(5)), ("shape/customShatterMap", Value::Layer(Some(1))), ("shape/whiteTilesFixed", Value::Bool(true))];
    let at_rest = with(&custom, 0.0, Some(&host));
    assert!(max_diff(&at_rest, &src) < 0.05, "custom pieces tile the layer: {}", max_diff(&at_rest, &src));
    let broke = with(&custom, 1.0, Some(&host));
    assert!(max_diff_region(&broke, &src, 0..12) < 0.05, "white tiles fixed");
    assert!(sum_diff_region(&broke, &src, 20..32) > 5.0, "black group moved");
    // Gradient: a black gradient layer with threshold 50 % keeps everything together.
    let black = Scene { camera: None, light: None, layer: Some(Image::filled(32, 32, [0.0, 0.0, 0.0, 1.0])) };
    let g = with(&[("gradient/gradientLayer", Value::Layer(Some(1))), ("gradient/shatterThreshold", num(50.0))], 1.0, Some(&black));
    assert!(max_diff(&g, &src) < 0.05);
    let gi = with(
        &[("gradient/gradientLayer", Value::Layer(Some(1))), ("gradient/shatterThreshold", num(50.0)), ("gradient/invertGradient", Value::Bool(true))],
        1.0,
        Some(&black),
    );
    assert!(sum_diff(&gi, &src) > 10.0);
    // Textures: Front Mode Color paints the pieces flat; Opacity fades them.
    let flat = with(&[("textures/frontMode", Value::Enum(0)), ("textures/color", col(0.0, 1.0, 0.0))], 0.0, None);
    let p = flat.get(16, 16);
    assert!(p[1] > 0.95 && p[0] < 0.05, "{p:?}");
    let half = with(&[("textures/opacity", num(0.5)), ("shape/repetitions", num(2.0))], 0.0, None);
    assert!((half.get(8, 4)[3] - 0.5).abs() < 0.02, "{:?}", half.get(8, 4));
    // Extrusion: thicker pieces cover more once they tumble.
    let cover = |d: f64| {
        with(&[("shape/extrusionDepth", num(d)), ("textures/sideMode", Value::Enum(0)), ("textures/color", col(1.0, 1.0, 1.0))], 0.6, None)
            .data
            .iter()
            .map(|p| p[3])
            .sum::<f32>()
    };
    assert!(cover(1.0) > cover(0.0) + 1.0, "{} vs {}", cover(1.0), cover(0.0));
    // Deterministic and seek-consistent.
    assert_eq!(with(&custom, 0.7, Some(&host)), with(&custom, 0.7, Some(&host)));
}

fn max_diff_region(a: &Image, b: &Image, xs: std::ops::Range<u32>) -> f32 {
    let mut m = 0.0f32;
    for y in 0..a.height {
        for x in xs.clone() {
            let (p, q) = (a.get(x as i64, y as i64), b.get(x as i64, y as i64));
            m = (0..4).map(|k| (p[k] - q[k]).abs()).fold(m, f32::max);
        }
    }
    m
}

fn sum_diff_region(a: &Image, b: &Image, xs: std::ops::Range<u32>) -> f32 {
    let mut s = 0.0;
    for y in 0..a.height {
        for x in xs.clone() {
            let (p, q) = (a.get(x as i64, y as i64), b.get(x as i64, y as i64));
            s += (0..4).map(|k| (p[k] - q[k]).abs()).sum::<f32>();
        }
    }
    s
}

/// Layer 1 = a left-dark, right-light ramp; layer 2 = solid green.
struct FoamHost;
impl crate::EffectHost for FoamHost {
    fn layer(&self, id: u64, _: bool) -> Option<crate::LayerPixels> {
        let img = if id == 1 { ramp(32, 32) } else { Image::filled(32, 32, [0.0, 1.0, 0.0, 1.0]) };
        Some(crate::LayerPixels { buf: Buf { img, offset: [0.0; 2], scale: 1.0 }, size: [32.0, 32.0] })
    }
    fn layer_at(&self, id: u64, _: f64, me: bool) -> Option<crate::LayerPixels> {
        self.layer(id, me)
    }
    fn audio(&self, _: u64, _: f64, _: usize, _: u32) -> Option<Vec<f32>> {
        None
    }
}

#[test]
fn foam_flow_map_pop_velocity_wobble_textures_and_reflection() {
    let base = vec![
        ("producer/producerPoint", pt(16.0, 16.0)),
        ("producer/productionRate", num(2.0)),
        ("physics/windSpeed", num(0.0)),
        ("physics/turbulence", num(0.0)),
        ("view", Value::Enum(2)),
    ];
    let with = |extra: &[(&str, Value)], t: f64| {
        let mut v = base.clone();
        v.extend_from_slice(extra);
        run_fx("ec.sim.foam", &v, Image::new(32, 32), t, EffectEnv { host: Some(&FoamHost), ..Default::default() }).img
    };
    let cx = |img: &Image| {
        let (mut s, mut n) = (0.0, 0.0);
        for y in 0..32 {
            for x in 0..32 {
                let a = img.get(x, y)[3];
                s += a * x as f32;
                n += a;
            }
        }
        s / n.max(1e-6)
    };
    // Flow map: bubbles run downhill (towards the dark left side).
    let still = with(&[], 1.0);
    let flowing = with(&[("flowMap/flowMap", Value::Layer(Some(1))), ("flowMap/flowMapSteepness", num(1.0))], 1.0);
    assert!(cx(&flowing) < cx(&still) - 1.0, "{} vs {}", cx(&flowing), cx(&still));
    let hq = with(&[("flowMap/flowMap", Value::Layer(Some(1))), ("flowMap/flowMapSteepness", num(1.0)), ("flowMap/simulationQuality", Value::Enum(2))], 1.0);
    assert_eq!(
        hq,
        with(&[("flowMap/flowMap", Value::Layer(Some(1))), ("flowMap/flowMapSteepness", num(1.0)), ("flowMap/simulationQuality", Value::Enum(2))], 1.0)
    );
    // Pop velocity scatters the survivors when short-lived bubbles pop.
    let short = [("bubbles/lifespan", num(8.0)), ("bubbles/strength", num(0.0))];
    let a = with(&[short.as_slice(), &[("physics/popVelocity", num(0.0))]].concat(), 1.0);
    let b2 = with(&[short.as_slice(), &[("physics/popVelocity", num(20.0))]].concat(), 1.0);
    assert_ne!(a, b2);
    // Wobble changes the drawn bubbles.
    assert_ne!(with(&[("physics/wobbleAmount", num(0.0))], 1.0), with(&[("physics/wobbleAmount", num(3.0))], 1.0));
    // User Defined texture: bubbles show the texture layer (green).
    let tex = with(&[("rendering/bubbleTexture", Value::Enum(5)), ("rendering/bubbleTextureLayer", Value::Layer(Some(2)))], 1.0);
    assert!(tex.data.iter().any(|p| p[1] > 0.5 && p[0] < 0.1));
    // Environment map: reflections tint the bubbles.
    let refl = with(&[("rendering/environmentMap", Value::Layer(Some(2))), ("rendering/reflectionStrength", num(1.0))], 1.0);
    let g = |i: &Image| i.data.iter().map(|p| p[1] - p[0]).sum::<f32>();
    assert!(g(&refl) > g(&still) + 1.0);
    // Zoom Producer Point zooms about the producer instead of the layer centre.
    let off = [("producer/producerPoint", pt(8.0, 8.0)), ("zoom", num(2.0))];
    let z1 = with(&off, 1.0);
    let z2 = with(&[off.as_slice(), &[("producer/zoomProducerPoint", Value::Bool(true))]].concat(), 1.0);
    assert!((cx(&z2) - 8.0).abs() < 2.0 && cx(&z1) < 4.0, "{} vs {}", cx(&z2), cx(&z1));
}
