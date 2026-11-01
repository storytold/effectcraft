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

/// #2024: bubbles near the layer's edges show its edge pixels, not the pale colour that stands
/// in for transparent ones.
#[test]
fn bubbles_near_edges_show_the_layer() {
    let blue = Image::filled(40, 40, [0.0, 0.0, 1.0, 1.0]);
    let pale = [0, 1].map(|reflection| {
        let vals = [("bubbleAmount", num(200.0)), ("bubbleSize", num(3.0)), ("reflectionType", Value::Enum(reflection))];
        let a = run("ec.sim.ccbubbles", &vals, blue.clone(), 0.3);
        assert!(a.data.iter().any(|p| p[3] > 0.5), "bubbles drawn");
        // Blue plus the white highlight keeps blue − red = alpha; the pale colour leaves 0.2 · alpha.
        a.data.iter().filter(|p| (p[2] - p[0] - p[3]).abs() > 1e-4).count()
    });
    assert_eq!(pale, [0, 0], "pale bubble pixels (liquid, metal)");
}

/// CC Bubbles on a 40 × 40 layer `img` at `scale`, in a buffer padded by 6 transparent pixels
/// (a half-resolution preview's layout).
fn padded_bubbles(img: Image, scale: f64, reflection: u32) -> Image {
    let size = 40.0;
    let s = crate::find("ec.sim.ccbubbles").unwrap();
    let mut params = crate::Params { values: s.params.iter().map(|p| (p.id.to_string(), crate::default_value(p, [size; 2]))).collect() };
    for (k, v) in [("bubbleAmount", num(200.0)), ("bubbleSize", num(3.0)), ("reflectionType", Value::Enum(reflection))] {
        params.values.insert(k.into(), v);
    }
    let ctx = EffectCtx { params: &params, time: 0.3, layer_size: [size; 2], seed: 1, adjustment: false, env: EffectEnv::default() };
    let mut b = Buf { img, offset: [0.0; 2], scale };
    b.pad(6);
    crate::apply(s, &ctx, b).img
}

/// #2024: the same when the buffer has transparent padding around the layer: samples past the
/// layer's edges repeat its edge pixels, not the padding, and a clear hole inside the layer is
/// still refracted as the pale colour.
#[test]
fn bubbles_near_padded_edges_show_the_layer() {
    let blue = |side| Image::filled(side, side, [0.0, 0.0, 1.0, 1.0]);
    let pale = |img: &Image| img.data.iter().filter(|p| (p[2] - p[0] - p[3]).abs() > 1e-4).count();
    for (scale, side) in [(1.0, 40), (0.5, 20)] {
        let counts = [0, 1].map(|reflection| {
            let a = padded_bubbles(blue(side), scale, reflection);
            assert!(a.data.iter().any(|p| p[3] > 0.5), "bubbles drawn");
            pale(&a)
        });
        assert_eq!(counts, [0, 0], "pale bubble pixels at scale {scale} (liquid, metal)");
    }
    let mut holed = blue(40);
    for y in 12..28 {
        for x in 12..28 {
            holed.set(x, y, [0.0; 4]);
        }
    }
    assert!(pale(&padded_bubbles(holed, 1.0, 0)) > 0, "the hole refracts as the pale colour");
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

/// A host whose every layer is `img`.
struct TexHost(Image);
impl crate::EffectHost for TexHost {
    fn layer(&self, _: u64, _: bool) -> Option<crate::LayerPixels> {
        let s = [self.0.width as f64, self.0.height as f64];
        Some(crate::LayerPixels { buf: Buf { img: self.0.clone(), offset: [0.0; 2], scale: 1.0 }, size: s })
    }
    fn audio(&self, _: u64, _: f64, _: usize, _: u32) -> Option<Vec<f32>> {
        None
    }
}

/// A texture whose left half is green and right half blue.
fn two_tone_tex() -> Image {
    let mut tex = Image::new(8, 8);
    for y in 0..8 {
        for x in 0..8 {
            tex.set(x, y, if x < 4 { [0.0, 1.0, 0.0, 1.0] } else { [0.0, 0.0, 1.0, 1.0] });
        }
    }
    tex
}

/// CC Particle World drawing red Textured Squares with Texture Layer `layer`.
fn pw_textured_vals(layer: Option<u64>) -> Vec<(&'static str, Value)> {
    vec![
        ("birthRate", num(1.0)),
        ("extras/randomSeed", num(4.0)),
        ("particle/particleType", Value::Enum(8)),
        ("particle/texture/textureLayer", Value::Layer(layer)),
        ("particle/birthSize", num(1.0)),
        ("particle/deathSize", num(1.0)),
        ("particle/birthColor", col(1.0, 0.0, 0.0)),
        ("particle/deathColor", col(1.0, 0.0, 0.0)),
    ]
}

/// #1465: Textured Square draws the Texture Layer on each particle, not the particle colour.
#[test]
fn particle_world_textured_square_draws_the_texture_layer() {
    let host = TexHost(two_tone_tex());
    let vals = pw_textured_vals;
    let env = EffectEnv { host: Some(&host), ..Default::default() };
    let out = run_fx("ec.sim.ccparticleworld", &vals(Some(1)), Image::new(48, 48), 1.0, env).img;
    let seen: Vec<_> = out.data.iter().filter(|p| p[3] > 0.05).collect();
    assert!(!seen.is_empty(), "particles drawn");
    assert!(seen.iter().all(|p| p[0] < 0.02 * p[3]), "no particle colour: {:?}", seen.iter().find(|p| p[0] >= 0.02 * p[3]));
    // Both halves of the texture show: it is mapped across the square, not sampled once.
    assert!(seen.iter().any(|p| p[1] > 2.0 * p[2]) && seen.iter().any(|p| p[2] > 2.0 * p[1]), "both texture halves");
    // Without a texture layer the squares keep the particle colour.
    let plain = run_fx("ec.sim.ccparticleworld", &vals(None), Image::new(48, 48), 1.0, env).img;
    assert!(plain.data.iter().any(|p| p[3] > 0.05 && p[0] > 0.9 * p[3]));
}

/// #1465 with #1195's motion trails: with the shutter open the averaged samples still show
/// the Texture Layer, not the particle colour.
#[test]
fn particle_world_textured_square_motion_trails_draw_the_texture_layer() {
    let host = TexHost(two_tone_tex());
    let env = EffectEnv { host: Some(&host), frame_rate: 10.0, shutter: Some((360.0, -180.0, 8)), ..Default::default() };
    let blurred = run_fx("ec.sim.ccparticleworld", &pw_textured_vals(Some(1)), Image::new(48, 48), 1.0, env).img;
    let sharp = run_fx("ec.sim.ccparticleworld", &pw_textured_vals(Some(1)), Image::new(48, 48), 1.0, EffectEnv { shutter: None, ..env }).img;
    assert_ne!(sharp.data, blurred.data, "the shutter smears the particles");
    let seen: Vec<_> = blurred.data.iter().filter(|p| p[3] > 0.02).collect();
    assert!(!seen.is_empty(), "particles drawn");
    assert!(seen.iter().all(|p| p[0] < 0.02 * p[3]), "no particle colour: {:?}", seen.iter().find(|p| p[0] >= 0.02 * p[3]));
    assert!(seen.iter().any(|p| p[1] > 2.0 * p[2]) && seen.iter().any(|p| p[2] > 2.0 * p[1]), "both texture halves");
}

/// A host whose every layer is `0` as given (a buffer that may span only its content).
struct PixelsHost(crate::LayerPixels);
impl crate::EffectHost for PixelsHost {
    fn layer(&self, _: u64, _: bool) -> Option<crate::LayerPixels> {
        Some(self.0.clone())
    }
    fn audio(&self, _: u64, _: f64, _: usize, _: u32) -> Option<Vec<f32>> {
        None
    }
}

/// #1465: a Texture Layer whose buffer spans only its content (text, shapes) is mapped as
/// the whole layer, transparent around the content, not with its edge pixels stretched.
#[test]
fn particle_world_texture_layer_with_a_content_sized_buffer() {
    // An 8×8 layer with a green 4×4 square in the middle: once as the full layer, once as
    // just the square's pixels.
    let mut full = Image::new(8, 8);
    for y in 2..6 {
        for x in 2..6 {
            full.set(x, y, [0.0, 1.0, 0.0, 1.0]);
        }
    }
    let content = Image::filled(4, 4, [0.0, 1.0, 0.0, 1.0]);
    let render = |buf: Buf| {
        let host = PixelsHost(crate::LayerPixels { buf, size: [8.0, 8.0] });
        let env = EffectEnv { host: Some(&host), ..Default::default() };
        run_fx("ec.sim.ccparticleworld", &pw_textured_vals(Some(1)), Image::new(48, 48), 1.0, env).img
    };
    let want = render(Buf { img: full, offset: [0.0; 2], scale: 1.0 });
    let got = render(Buf { img: content, offset: [-2.0, -2.0], scale: 1.0 });
    assert!(want.data.iter().any(|p| p[3] > 0.05), "particles drawn");
    assert_eq!(want.data, got.data, "the content-sized buffer draws as the full layer");
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

// ---------------------------------------------------------- Birth Rate over time (#1373)

/// A host that evaluates effect `index`'s Birth Rate at other layer times as `rate(t)`.
struct Rate {
    index: usize,
    rate: fn(f64) -> f64,
}
impl crate::EffectHost for Rate {
    fn layer(&self, _: u64, _: bool) -> Option<crate::LayerPixels> {
        None
    }
    fn audio(&self, _: u64, _: f64, _: usize, _: u32) -> Option<Vec<f32>> {
        None
    }
    fn param_at(&self, effect_index: usize, id: &str, t: f64) -> Option<Value> {
        (effect_index == self.index && id == "birthRate").then(|| num((self.rate)(t)))
    }
}

/// Birth Rate 3 until 0.5 s, then 0 (the issue's one-shot burst).
fn burst(t: f64) -> f64 {
    if t < 0.5 { 3.0 } else { 0.0 }
}

/// The same burst, cut at 0.25 s (an earlier key moved).
fn short_burst(t: f64) -> f64 {
    if t < 0.25 { 3.0 } else { 0.0 }
}

/// Birth Rate falling linearly from 3 at 0 s to 0 at 0.5 s.
fn falling(t: f64) -> f64 {
    (3.0 * (1.0 - t / 0.5)).max(0.0)
}

/// CC Particle World's parameters: random seed `seed`, Birth Rate `now` (its value at the
/// rendered time), Longevity 1 s.
fn pw_params(seed: f64, now: f64) -> crate::Params {
    let spec = crate::find("ec.sim.ccparticleworld").unwrap();
    let mut p = crate::Params { values: spec.params.iter().map(|ps| (ps.id.to_string(), crate::default_value(ps, [100.0, 100.0]))).collect() };
    for (k, v) in [("birthRate", num(now)), ("extras/randomSeed", num(seed)), ("longevity", num(1.0))] {
        p.values.insert(k.to_string(), v);
    }
    p
}

fn pw_ctx<'a>(p: &'a crate::Params, t: f64, host: Option<&'a Rate>, index: usize) -> EffectCtx<'a> {
    let env = EffectEnv { host: host.map(|h| h as &dyn crate::EffectHost), effect_index: index, ..Default::default() };
    EffectCtx { params: p, time: t, layer_size: [100.0, 100.0], seed: 1, adjustment: false, env }
}

/// The live particles at layer time `t` with Birth Rate `now` there and `host`'s history.
fn pw_at(seed: f64, now: f64, t: f64, host: Option<&Rate>) -> Vec<crate::psim::SimParticle> {
    let p = pw_params(seed, now);
    particle_state("ec.sim.ccparticleworld", &pw_ctx(&p, t, host, host.map_or(0, |h| h.index))).unwrap()
}

/// The same, simulated from layer time 0 with a cache of its own (the reference).
fn pw_fresh(seed: f64, now: f64, t: f64, host: &Rate) -> Vec<crate::psim::SimParticle> {
    let p = pw_params(seed, now);
    let ctx = pw_ctx(&p, t, Some(host), host.index);
    let key = params_key_except(&ctx, &Buf { img: Image::new(0, 0), offset: [0.0; 2], scale: 1.0 }, 1, &[PW_EFFECT_CAMERA, "birthRate"]);
    let st = simulate_rates(&SimCache::new(1), key, &pw_phys(&ctx), &rate_history(&ctx, "birthRate", PW_RATE));
    st.parts.iter().map(|q| crate::psim::SimParticle { p: q.p, v: q.v, age: q.age, life: q.life, rnd: q.rnd, id: q.id }).collect()
}

/// Particles born in the first `t` seconds of `rate`.
fn born_by(rate: fn(f64) -> f64, t: f64) -> usize {
    crate::psim::births_at(steps_at(t), 40_000, |i| rate(i as f64 / SPS) * PW_RATE).unwrap().len()
}

/// #1373: Birth Rate only decides the births of its own time. After a burst stops, the
/// particles born in it live on (moving and ageing) until their Longevity runs out, rather
/// than vanishing because the rate is now 0.
#[test]
fn a_burst_lives_on_after_birth_rate_drops_to_zero() {
    let host = Rate { index: 2, rate: burst };
    // During the burst: as a constant Birth Rate of 3.
    assert_eq!(pw_at(101.0, 3.0, 0.4, Some(&host)), pw_at(101.0, 3.0, 0.4, None));
    // After it: the burst's particles, none newer.
    let late = pw_at(101.0, 0.0, 0.9, Some(&host));
    let n = born_by(burst, 0.5);
    assert!(late.len() > n / 2 && late.iter().all(|q| (q.id as usize) < n), "{} live of {n} born", late.len());
    assert!(pw_at(101.0, 0.0, 0.9, None).is_empty(), "a constant Birth Rate of 0 never bore any");
    // They keep moving.
    let early = pw_at(101.0, 0.0, 0.7, Some(&host));
    let q0 = early.iter().find(|q| q.id == late[0].id).unwrap();
    assert!(q0.p != late[0].p && late[0].age > q0.age + 0.19, "{q0:?} -> {:?}", late[0]);
    // Longevity 1 s (± 20 %): all gone by 1.8 s.
    assert!(pw_at(101.0, 0.0, 1.8, Some(&host)).is_empty());
    // The effect asks for its own index: another effect's history isn't its own.
    let p = pw_params(101.0, 0.0);
    assert!(particle_state("ec.sim.ccparticleworld", &pw_ctx(&p, 0.9, Some(&host), 0)).unwrap().is_empty());
}

/// A Birth Rate ramping down isn't simulated as if its current value had always applied.
#[test]
fn a_ramped_birth_rate_bears_what_each_moment_asked_for() {
    let host = Rate { index: 0, rate: falling };
    let ramped = pw_at(102.0, falling(0.25), 0.25, Some(&host));
    let constant = pw_at(102.0, falling(0.25), 0.25, None);
    assert_eq!(ramped.len(), born_by(falling, 0.25), "every particle born so far is alive");
    assert_eq!(constant.len(), born_by(|_| 1.5, 0.25));
    assert!(ramped.len() > constant.len() + 10, "{} vs {}", ramped.len(), constant.len());
}

/// Seeking backward, forward and to the same time again gives the state simulated from 0.
#[test]
fn seeking_through_a_birth_rate_history_matches_simulating_from_zero() {
    let host = Rate { index: 0, rate: burst };
    for t in [1.4, 0.6, 0.9, 0.3, 0.9, 1.1] {
        let now = burst(t);
        assert_eq!(pw_at(103.0, now, t, Some(&host)), pw_fresh(103.0, now, t, &host), "at {t}");
    }
}

/// The simulation cache key leaves Birth Rate out (so an animated rate keeps its checkpoints),
/// but a checkpoint is only resumed with the history that reached it: after an earlier key
/// moves, the same current rate (0) gives the new history's particles.
#[test]
fn an_earlier_birth_rate_change_is_not_served_from_stale_checkpoints() {
    let (long, short) = (Rate { index: 0, rate: burst }, Rate { index: 0, rate: short_burst });
    let a = pw_at(104.0, 0.0, 0.9, Some(&long));
    let b = pw_at(104.0, 0.0, 0.9, Some(&short));
    assert_eq!(b, pw_fresh(104.0, 0.0, 0.9, &short));
    assert!(b.len() < a.len() && !b.is_empty(), "{} vs {}", b.len(), a.len());
    assert_eq!(pw_at(104.0, 0.0, 1.0, Some(&long)), pw_fresh(104.0, 0.0, 1.0, &long));
}

/// A constant Birth Rate gives exactly the particles it did before the history was sampled.
#[test]
fn a_constant_birth_rate_is_unchanged() {
    let host = Rate { index: 0, rate: |_| 2.0 };
    let p = pw_params(105.0, 2.0);
    let ctx = pw_ctx(&p, 1.3, None, 0);
    let phys = pw_phys(&ctx);
    let before = simulate(&SimCache::new(1), sim_key(&ctx, 1), 1.3, &phys);
    let now = pw_at(105.0, 2.0, 1.3, Some(&host));
    assert_eq!(now.len(), before.parts.len());
    assert!(now.iter().zip(&before.parts).all(|(a, b)| a.id == b.id && a.p == b.p && a.v == b.v && a.age == b.age));
    assert_eq!(crate::psim::births_at(78, 40_000, |_| phys.rate), crate::psim::births(phys.rate, 78, 40_000));
}

/// The GPU's birth schedule for a rate history is the CPU's: the ids born are the ids the
/// CPU simulation numbers.
#[test]
fn the_birth_schedule_follows_the_rate_history() {
    let host = Rate { index: 0, rate: falling };
    let p = pw_params(106.0, 0.0);
    let ctx = pw_ctx(&p, 0.7, Some(&host), 0);
    let rates = rate_history(&ctx, "birthRate", PW_RATE);
    let st = simulate_rates(&SimCache::new(1), 9, &pw_phys(&ctx), &rates);
    let births = crate::psim::births_at(rates.len() as u64, 40_000, |i| rates[i as usize]).unwrap();
    assert_eq!(st.next_id as usize, births.len());
    assert!(births.iter().all(|&b| f64::from(b) / SPS < 0.5), "nothing born once the ramp reaches 0");
}

// ------------------------- CC Particle Systems II and CC Mr. Mercury's Birth Rate over time

/// A stepped particle effect: its id, physics and simulation.
struct System {
    id: &'static str,
    phys: fn(&EffectCtx) -> Phys,
    stepped: Stepped<'static>,
}

const SYSTEMS: [System; 2] =
    [System { id: "ec.sim.ccparticlesystems2", phys: ps2_phys, stepped: PS2 }, System { id: "ec.sim.ccmrmercury", phys: mercury_phys, stepped: MERCURY }];

/// `id`'s parameters with Birth Rate `now` and Longevity 1 s.
fn sys_params(id: &str, now: f64) -> crate::Params {
    let spec = crate::find(id).unwrap();
    let mut p = crate::Params { values: spec.params.iter().map(|ps| (ps.id.to_string(), crate::default_value(ps, [100.0, 100.0]))).collect() };
    for (k, v) in [("birthRate", num(now)), ("longevity", num(1.0))] {
        assert!(p.values.contains_key(k), "{id}: no {k}");
        p.values.insert(k.to_string(), v);
    }
    p
}

/// The context at layer time `t` (`seed` keeps each test's simulations apart).
fn sys_ctx<'a>(p: &'a crate::Params, t: f64, host: Option<&'a Rate>, seed: u32) -> EffectCtx<'a> {
    let env = EffectEnv { host: host.map(|h| h as &dyn crate::EffectHost), effect_index: host.map_or(0, |h| h.index), ..Default::default() };
    EffectCtx { params: p, time: t, layer_size: [100.0, 100.0], seed, adjustment: false, env }
}

/// The live particles (id, position, age) of `sys` at `t` through its simulation.
fn sys_at(sys: &System, seed: u32, now: f64, t: f64, host: Option<&Rate>) -> Vec<(u32, [f32; 3], f32)> {
    let p = sys_params(sys.id, now);
    let ctx = sys_ctx(&p, t, host, seed);
    sys.stepped.state(&ctx, &(sys.phys)(&ctx)).parts.iter().map(|q| (q.id, q.p, q.age)).collect()
}

/// The same, simulated from layer time 0 with a cache of its own (the reference).
fn sys_fresh(sys: &System, seed: u32, now: f64, t: f64, host: &Rate) -> Vec<(u32, [f32; 3], f32)> {
    let p = sys_params(sys.id, now);
    let ctx = sys_ctx(&p, t, Some(host), seed);
    let mut skip = sys.stepped.skip.to_vec();
    skip.push("birthRate");
    let key = params_key_except(&ctx, &Buf { img: Image::new(0, 0), offset: [0.0; 2], scale: 1.0 }, sys.stepped.salt, &skip);
    let st = simulate_rates(&SimCache::new(1), key, &(sys.phys)(&ctx), &rate_history(&ctx, "birthRate", sys.stepped.k));
    st.parts.iter().map(|q| (q.id, q.p, q.age)).collect()
}

/// What `sys` draws at `t`: CC Particle Systems II's particles, CC Mr. Mercury's blobs.
fn sys_drawn(sys: &System, seed: u32, now: f64, t: f64, host: Option<&Rate>) -> usize {
    let p = sys_params(sys.id, now);
    let ctx = sys_ctx(&p, t, host, seed);
    match sys.id {
        "ec.sim.ccmrmercury" => mercury_blobs(&ctx, &Buf { img: Image::new(100, 100), offset: [0.0; 2], scale: 1.0 }).len(),
        id => particle_state(id, &ctx).unwrap().len(),
    }
}

/// #1373 for CC Particle Systems II and CC Mr. Mercury: a burst lives on after Birth Rate
/// drops to 0 (what they draw too), moving and ageing, and is gone after its Longevity.
#[test]
fn particle_systems_ii_and_mr_mercury_keep_a_burst_after_birth_rate_drops() {
    let host = Rate { index: 1, rate: burst };
    for sys in &SYSTEMS {
        let born = crate::psim::births_at(steps_at(0.5), 40_000, |i| burst(i as f64 / SPS) * sys.stepped.k).unwrap().len();
        assert_eq!(sys_at(sys, 201, 3.0, 0.4, Some(&host)), sys_at(sys, 201, 3.0, 0.4, None), "{}: during the burst", sys.id);
        let late = sys_at(sys, 201, 0.0, 0.9, Some(&host));
        assert!(late.len() > born / 2 && late.iter().all(|q| (q.0 as usize) < born), "{}: {} live of {born}", sys.id, late.len());
        assert!(sys_drawn(sys, 201, 0.0, 0.9, Some(&host)) > 0, "{}: drawn", sys.id);
        assert_eq!(sys_drawn(sys, 201, 0.0, 0.9, None), 0, "{}: a constant 0 bore none", sys.id);
        let early = sys_at(sys, 201, 0.0, 0.7, Some(&host));
        let q0 = early.iter().find(|q| q.0 == late[0].0).unwrap();
        assert!(q0.1 != late[0].1 && late[0].2 > q0.2 + 0.19, "{}: {q0:?} -> {:?}", sys.id, late[0]);
        assert!(sys_at(sys, 201, 0.0, 1.8, Some(&host)).is_empty(), "{}: gone after Longevity", sys.id);
    }
}

/// Seeking matches a simulation from 0, and moving an earlier key (same current rate, same
/// cache key) isn't served from the old history's checkpoints.
#[test]
fn particle_systems_ii_and_mr_mercury_seek_and_drop_stale_checkpoints() {
    let (long, short) = (Rate { index: 0, rate: burst }, Rate { index: 0, rate: short_burst });
    for sys in &SYSTEMS {
        for t in [1.4, 0.6, 0.9, 0.3, 0.9, 1.1] {
            let now = burst(t);
            assert_eq!(sys_at(sys, 202, now, t, Some(&long)), sys_fresh(sys, 202, now, t, &long), "{} at {t}", sys.id);
        }
        let b = sys_at(sys, 202, 0.0, 0.9, Some(&short));
        assert_eq!(b, sys_fresh(sys, 202, 0.0, 0.9, &short), "{}: the moved key", sys.id);
        assert!(!b.is_empty() && b.len() < sys_at(sys, 202, 0.0, 0.9, Some(&long)).len(), "{}", sys.id);
    }
}

/// A constant Birth Rate gives exactly the particles the constant-rate simulation did.
#[test]
fn particle_systems_ii_and_mr_mercury_are_unchanged_at_a_constant_rate() {
    let host = Rate { index: 0, rate: |_| 2.0 };
    for sys in &SYSTEMS {
        let p = sys_params(sys.id, 2.0);
        let ctx = sys_ctx(&p, 1.3, None, 203);
        let before = simulate(&SimCache::new(1), 1, 1.3, &(sys.phys)(&ctx));
        let before: Vec<_> = before.parts.iter().map(|q| (q.id, q.p, q.age)).collect();
        assert!(!before.is_empty());
        assert_eq!(sys_at(sys, 203, 2.0, 1.3, Some(&host)), before, "{}: keyed to a constant", sys.id);
        assert_eq!(sys_at(sys, 203, 2.0, 1.3, None), before, "{}: no host", sys.id);
    }
}
