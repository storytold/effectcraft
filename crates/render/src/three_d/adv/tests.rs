use effectcraft_keyframe::Value;
use effectcraft_project::{Comp, ItemId, ItemKind, LayerSource, LightKind, PrimitiveKind, Project, Renderer as R3, build};
use effectcraft_time::{FrameRate, Tick};

use super::scene::{Material, Scene, TexInfo, Vertex, dominant_direction, irradiance_map};
use super::shade::{self, PI};
use super::*;
use crate::{NoFootage, RenderOpts, Renderer};

// ---------------------------------------------------------------- shading

#[test]
fn brdf_matches_cook_torrance_formula() {
    // Reference: f = kd·c/π + D·G·F/(4·NV·NL), returned × NL × π.
    let n = [0.0, 0.0, 1.0];
    let v = shade::norm([0.3, 0.1, 1.0]);
    let l = shade::norm([-0.4, 0.2, 1.0]);
    let (base, metal, rough) = ([0.8, 0.4, 0.2], 0.3f32, 0.45f32);
    let got = shade::brdf(shade::BrdfSurface { base, metallic: metal, roughness: rough, specular: 1.0, diffuse: 1.0 }, n, v, l);
    let h = shade::norm([v[0] + l[0], v[1] + l[1], v[2] + l[2]]);
    let (nv, nl, nh, vh) = (shade::dot(n, v), shade::dot(n, l), shade::dot(n, h), shade::dot(v, h));
    let a2 = (rough * rough).powi(2);
    let d = a2 / (PI * ((nh * nh) * (a2 - 1.0) + 1.0).powi(2));
    let k = (rough + 1.0).powi(2) / 8.0;
    let g = nv / (nv * (1.0 - k) + k) * nl / (nl * (1.0 - k) + k);
    for c in 0..3 {
        let f0 = 0.04 + (base[c] - 0.04) * metal;
        let f = f0 + (1.0 - f0) * (1.0 - vh).powi(5);
        let expect = ((1.0 - f) * (1.0 - metal) * base[c] / PI + d * g * f / (4.0 * nv * nl)) * nl * PI;
        assert!((got[c] - expect).abs() < 1e-5, "channel {c}: {} vs {expect}", got[c]);
    }
    // No light from behind the surface.
    assert_eq!(shade::brdf(shade::BrdfSurface { base, metallic: metal, roughness: rough, specular: 1.0, diffuse: 1.0 }, n, v, [0.0, 0.0, -1.0]), [0.0; 3]);
    // A white rough dielectric lit head-on reflects about its base colour (π folded in).
    let w = shade::brdf(shade::BrdfSurface { base: [1.0; 3], metallic: 0.0, roughness: 1.0, specular: 1.0, diffuse: 1.0 }, n, n, n);
    assert!(w[0] > 0.95 && w[0] < 1.05, "{w:?}");
}

#[test]
fn ggx_distribution_is_normalised() {
    // ∫ D(h)·(n·h) dω = 1 over the hemisphere.
    for &rough in &[0.2f32, 0.5, 0.9] {
        let a = rough * rough;
        let n = 4000;
        let mut sum = 0.0f64;
        for i in 0..n {
            let th = (i as f64 + 0.5) / n as f64 * std::f64::consts::FRAC_PI_2;
            let c = th.cos() as f32;
            sum += shade::d_ggx(c, a) as f64 * c as f64 * th.sin() * std::f64::consts::FRAC_PI_2 / n as f64 * 2.0 * std::f64::consts::PI;
        }
        assert!((sum - 1.0).abs() < 0.02, "roughness {rough}: {sum}");
    }
}

#[test]
fn constant_environment_has_constant_irradiance() {
    let src = vec![[0.25, 0.5, 0.75, 1.0]; 64 * 32];
    let irr = irradiance_map(64, 32, &src, 32, 16);
    for p in &irr {
        for c in 0..3 {
            assert!((p[c] - src[0][c]).abs() < 1e-4, "{p:?}");
        }
    }
    // And a sky brighter than the ground lights upward-facing normals more.
    let sky: Vec<[f32; 4]> = (0..32).flat_map(|y| (0..64).map(move |_| if y < 16 { [1.0, 1.0, 1.0, 1.0] } else { [0.0, 0.0, 0.0, 1.0] })).collect();
    let irr = irradiance_map(64, 32, &sky, 32, 16);
    assert!(irr[16][0] > 0.9 && irr[15 * 32 + 16][0] < 0.1, "{:?} {:?}", irr[16], irr[15 * 32 + 16]);
}

/// A NaN or infinite texel poisons the sums (every NaN comparison is false, so the thresholds
/// alone would pass it through as a NaN direction); an infinite one even where sin θ = 0 turns
/// into NaN. Either way the sum is garbage, not a direction: no sun and no key, so no shadow
/// (#2436).
#[test]
fn a_non_finite_environment_has_no_sun_direction() {
    let nan = vec![[f32::NAN; 4]; 8 * 4];
    assert_eq!(dominant_direction(8, 4, &nan, [1.0; 3], 0.0), ([0.0; 3], 0.0));
    let inf = vec![[f32::INFINITY, 0.0, 0.0, 1.0]; 8 * 4];
    assert_eq!(dominant_direction(8, 4, &inf, [1.0; 3], 0.0), ([0.0; 3], 0.0));
}

/// The key fraction: a uniform image is directionless (key 0), a lone bright spot on black is
/// almost fully directional, and a broad gradient sits in between — the soft studio default
/// shadows visibly but keeps most of its light as fill (#2436).
#[test]
fn environment_key_strength_tracks_directionality() {
    let uniform = vec![[0.5f32, 0.5, 0.5, 1.0]; 8 * 4];
    assert_eq!(dominant_direction(8, 4, &uniform, [1.0; 3], 0.0).1, 0.0);
    let mut spot = vec![[0.0f32; 4]; 64 * 32];
    spot[16 * 64 + 32] = [1.0, 1.0, 1.0, 1.0];
    let (_, key) = dominant_direction(64, 32, &spot, [1.0; 3], 0.0);
    assert!(key > 0.95, "a lone spot is almost fully directional: {key}");
    let (w, h, d) = scene::default_environment();
    let (_, key) = dominant_direction(w, h, &d, [1.0; 3], 0.0);
    assert!((0.35..0.55).contains(&key), "the default studio is a broad key with fill: {key}");
}

/// #2436 review: a non-uniform image can be directionless too — two equal bright spots at
/// opposite azimuths pull the radiance-weighted mean in opposite directions and cancel,
/// leaving no dominant region to shadow from (under 1 % directional): no sun, no key, so no
/// shadow, just like a uniform image. Tilt the balance and a direction re-emerges, pointing
/// at the brighter spot.
#[test]
fn opposing_bright_regions_cancel_to_no_shadow() {
    let (w, h) = (64u32, 32u32);
    let mut opposed = vec![[0.0f32; 4]; (w * h) as usize];
    // Rows 15 and 16 mirror about the equator (v = 0.484375 / 0.515625), and columns 16 and 48
    // sit half a width apart (u = 0.2578125 / 0.7578125): the two texel-centre directions are
    // exactly opposite — equal solid-angle weight, opposite pull.
    let (a, b) = ((15 * w + 16) as usize, (16 * w + 48) as usize);
    opposed[a] = [1.0, 1.0, 1.0, 1.0];
    opposed[b] = [1.0, 1.0, 1.0, 1.0];
    assert_eq!(dominant_direction(w, h, &opposed, [1.0; 3], 0.0), ([0.0; 3], 0.0));
    // One side twice as bright: the mean points at the brighter spot with its share of the
    // weight (key = 1/3 — two parts against one).
    let mut tilted = opposed.clone();
    tilted[a] = [2.0, 2.0, 2.0, 1.0];
    let (sun, key) = dominant_direction(w, h, &tilted, [1.0; 3], 0.0);
    assert!(sun != [0.0; 3] && key > 0.3, "the brighter side wins: {sun:?} key {key}");
    let want = shade::equirect_dir(16.5 / 64.0, 15.5 / 32.0, 0.0);
    let dot = sun[0] * want[0] + sun[1] * want[1] + sun[2] * want[2];
    assert!(dot > 0.999, "the sun points at the brighter spot: {sun:?} vs {want:?}");
}

/// #2436 review: the sun follows the tinted radiance, not the raw texels — equally bright red
/// and blue regions on exactly opposite texels cancel to a white light (no shadow), but the
/// red light's tint removes the blue side's pull, so the mean points straight at the red one
/// (a blue light, at the blue one): the shadow lands where the emitted light comes from.
#[test]
fn a_coloured_light_shadows_from_the_region_it_tints() {
    let (w, h) = (64u32, 32u32);
    let mut opposed = vec![[0.0f32; 4]; (w * h) as usize];
    // Rows 15 and 16 mirror about the equator, columns 16 and 48 half a width apart: the two
    // texel-centre directions are exactly opposite — equal solid-angle weight, opposite pull.
    let (a, b) = ((15 * w + 16) as usize, (16 * w + 48) as usize);
    opposed[a] = [1.0, 0.0, 0.0, 1.0];
    opposed[b] = [0.0, 0.0, 1.0, 1.0];
    // To a white light the channel sums are equal: they cancel, no shadow.
    assert_eq!(dominant_direction(w, h, &opposed, [1.0; 3], 0.0), ([0.0; 3], 0.0));
    // A red light's tint zeroes the blue texel's weight: the red one becomes a lone spot.
    // (Any positive scale of the tint — the normalised [3, 0, 0] included — points the same
    // way; only the channel ratios matter.)
    let (sun, key) = dominant_direction(w, h, &opposed, [1.0, 0.0, 0.0], 0.0);
    let want = shade::equirect_dir(16.5 / 64.0, 15.5 / 32.0, 0.0);
    let dot = sun[0] * want[0] + sun[1] * want[1] + sun[2] * want[2];
    assert!(dot > 0.999 && key > 0.95, "the red side wins: {sun:?} key {key}");
    // A blue light points at the blue one instead.
    let (sun, _) = dominant_direction(w, h, &opposed, [0.0, 0.0, 1.0], 0.0);
    let want = shade::equirect_dir(48.5 / 64.0, 16.5 / 32.0, 0.0);
    let dot = sun[0] * want[0] + sun[1] * want[1] + sun[2] * want[2];
    assert!(dot > 0.999, "the blue side wins: {sun:?}");
}

#[test]
fn equirect_round_trip_and_env_brdf() {
    for (u, v) in [(0.1f32, 0.2f32), (0.5, 0.5), (0.9, 0.7)] {
        let d = shade::equirect_dir(u, v, 0.3);
        let (u2, v2) = shade::equirect_uv(d, 0.3);
        assert!((u - u2).abs() < 1e-4 && (v - v2).abs() < 1e-4);
    }
    // Up (−Y in AE space) is the top row.
    assert!(shade::equirect_uv([0.0, -1.0, 0.0], 0.0).1 < 1e-3);
    let (a, b) = shade::env_brdf(1.0, 0.0);
    assert!(a + b > 0.9 && a + b <= 1.05);
    let (a, b) = shade::env_brdf(0.1, 1.0);
    assert!(a >= 0.0 && b > -0.01 && a + b < 1.0);
}

// ---------------------------------------------------------------- rasteriser

/// A scene of axis-aligned quads (z = camera depth) seen by an orthographic identity camera
/// mapping world x/y straight to raster pixels.
fn quad_scene(w: u32, h: u32, quads: &[([f32; 4], f32, [f32; 4])]) -> Scene {
    let mut s = Scene { width: w, height: h, ssaa: 1, linear_io: true, ..Scene::default() };
    // clip: x → 2x/w − 1, y → 1 − 2y/h, z: depth 0..1000 → 1..0 (nearer = larger).
    s.clip = [[2.0 / w as f32, 0.0, 0.0, -1.0], [0.0, -2.0 / h as f32, 0.0, 1.0], [0.0, 0.0, -0.001, 1.0], [0.0, 0.0, 0.0, 1.0]];
    s.view = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]];
    s.ortho = true;
    s.cam_fwd = [0.0, 0.0, 1.0];
    for (rect, z, color) in quads {
        let m = s.materials.len() as u32;
        s.materials.push(Material { base: *color, unlit: true, double_sided: true, alpha_mode: if color[3] < 1.0 { 2 } else { 0 }, ..Material::default() });
        let b = s.vertices.len() as u32;
        for (x, y) in [(rect[0], rect[1]), (rect[2], rect[1]), (rect[2], rect[3]), (rect[0], rect[3])] {
            s.vertices.push(Vertex { pos: [x, y, *z], normal: [0.0, 0.0, -1.0], uv: [0.0; 2], tangent: [1.0, 0.0, 0.0, 1.0], material: m });
        }
        s.indices.extend([b, b + 2, b + 1, b, b + 3, b + 2]);
    }
    // Opaque first, transparent (in given order) after.
    let tris: Vec<[u32; 3]> = s.indices.chunks(3).map(|t| [t[0], t[1], t[2]]).collect();
    let (o, t): (Vec<_>, Vec<_>) = tris.into_iter().partition(|t| !s.materials[s.vertices[t[0] as usize].material as usize].transparent());
    s.opaque_count = 3 * o.len() as u32;
    s.indices = o.into_iter().chain(t).flatten().collect();
    s
}

#[test]
fn depth_buffer_keeps_the_nearest_surface() {
    let red = [1.0, 0.0, 0.0, 1.0];
    let blue = [0.0, 0.0, 1.0, 1.0];
    // Blue is listed first but is farther away.
    let s = quad_scene(40, 30, &[([0.0, 0.0, 30.0, 20.0], 500.0, blue), ([10.0, 10.0, 40.0, 30.0], 100.0, red)]);
    let t = raster::render(&s);
    let px = |x: usize, y: usize| t.color[y * 40 + x];
    assert_eq!(px(5, 5), blue);
    assert_eq!(px(15, 15), red, "overlap: the nearer quad wins");
    assert_eq!(px(35, 25), red);
    assert_eq!(px(35, 5), [0.0; 4]);
    assert_eq!(t.depth[15 * 40 + 15], 100.0);
    assert!(t.depth[5 * 40 + 35].is_infinite());
}

#[test]
fn coverage_is_exact_for_pixel_aligned_quads() {
    // Shared diagonal edges are filled once (top-left rule): exact pixel counts.
    let s = quad_scene(32, 32, &[([4.0, 4.0, 20.0, 12.0], 10.0, [1.0, 1.0, 1.0, 0.5])]);
    let t = raster::render(&s);
    let covered = t.color.iter().filter(|c| c[3] > 0.0).count();
    assert_eq!(covered, 16 * 8);
    // Blended once (no double coverage along the diagonal).
    assert!(t.color.iter().all(|c| c[3] == 0.0 || (c[3] - 0.5).abs() < 1e-6));
    // Two transparent layers composite in order.
    let s = quad_scene(8, 8, &[([0.0, 0.0, 8.0, 8.0], 100.0, [1.0, 0.0, 0.0, 0.5]), ([0.0, 0.0, 8.0, 8.0], 50.0, [0.0, 0.0, 1.0, 0.5])]);
    let c = raster::render(&s).color[0];
    assert!((c[0] - 0.25).abs() < 1e-6 && (c[2] - 0.5).abs() < 1e-6 && (c[3] - 0.75).abs() < 1e-6, "{c:?}");
}

#[test]
fn textures_sample_bilinear_with_wrap_modes() {
    let t = TexInfo { offset: 0, width: 2, height: 1, wrap_u: 0, wrap_v: 1 };
    let texels = vec![[0.0, 0.0, 0.0, 1.0], [1.0, 1.0, 1.0, 1.0]];
    assert_eq!(shade::sample(&t, &texels, 0.25, 0.5)[0], 0.0);
    assert_eq!(shade::sample(&t, &texels, 0.5, 0.5)[0], 0.5);
    // Repeat wraps the right edge back to texel 0.
    assert!((shade::sample(&t, &texels, 1.0, 0.5)[0] - 0.5).abs() < 1e-6);
    let c = TexInfo { wrap_u: 1, ..t };
    assert_eq!(shade::sample(&c, &texels, 1.0, 0.5)[0], 1.0);
}

// ---------------------------------------------------------------- full renderer

fn project(w: u32, h: u32) -> (Project, ItemId) {
    let mut p = Project::default();
    let mut c = Comp::new(w, h, FrameRate::FPS_30, Tick::from_seconds_f64(1.0));
    c.renderer = R3::Advanced3D;
    let id = p.add_item("C", Default::default(), None, ItemKind::Comp(c.into()));
    (p, id)
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

fn render(p: &Project, cid: ItemId) -> crate::Image {
    Renderer::new(p, &NoFootage, RenderOpts::default()).comp_frame(cid, Tick::ZERO)
}

#[test]
fn lit_cube_renders_with_depth_and_shading() {
    let (mut p, cid) = project(160, 120);
    add(&mut p, cid, LayerSource::Primitive { kind: PrimitiveKind::Cube }, |l| {
        set(l, "geometryOptions/width", Value::Scalar(60.0));
        set(l, "geometryOptions/height", Value::Scalar(60.0));
        set(l, "geometryOptions/depth", Value::Scalar(60.0));
        set(l, "transform/rotationY", Value::Scalar(35.0));
        set(l, "transform/rotationX", Value::Scalar(-25.0));
        set(l, "materialOptions/baseColor", Value::Color([1.0, 1.0, 1.0, 1.0]));
    });
    let img = render(&p, cid);
    let centre = img.data[60 * 160 + 80];
    assert!(centre[3] > 0.99, "cube covers the centre: {centre:?}");
    assert_eq!(img.data[2 * 160 + 2][3], 0.0, "corner is empty");
    // No lights: unlit (base colour) — add a light and faces get different shades.
    add(&mut p, cid, LayerSource::Light { kind: LightKind::Parallel }, |_| {});
    let lit = render(&p, cid);
    let mut shades: Vec<i32> = lit.data.iter().filter(|q| q[3] > 0.99).map(|q| (q[0] * 20.0) as i32).collect();
    shades.sort();
    shades.dedup();
    assert!(shades.len() >= 3, "faces are shaded differently: {shades:?}");
    // Classic 3D draws no meshes.
    p.comp_mut(cid).unwrap().renderer = R3::Classic3D;
    assert!(render(&p, cid).data.iter().all(|q| q[3] == 0.0));
}

#[test]
fn shadows_darken_the_receiver() {
    let (mut p, cid) = project(200, 150);
    // A floor plane facing the camera, a sphere in front of it, a spot light behind the camera.
    add(&mut p, cid, LayerSource::Primitive { kind: PrimitiveKind::Plane }, |l| {
        set(l, "geometryOptions/width", Value::Scalar(400.0));
        set(l, "geometryOptions/height", Value::Scalar(400.0));
        set(l, "transform/position", Value::Vec3([100.0, 75.0, 200.0]));
        set(l, "materialOptions/baseColor", Value::Color([1.0, 1.0, 1.0, 1.0]));
    });
    add(&mut p, cid, LayerSource::Primitive { kind: PrimitiveKind::Sphere }, |l| {
        set(l, "geometryOptions/radius", Value::Scalar(30.0));
        set(l, "transform/position", Value::Vec3([100.0, 75.0, 50.0]));
        set(l, "materialOptions/castsShadows", Value::Enum(2));
    });
    add(&mut p, cid, LayerSource::Light { kind: LightKind::Point }, |l| {
        set(l, "transform/position", Value::Vec3([100.0, 75.0, -300.0]));
        set(l, "lightOptions/castsShadows", Value::Bool(true));
    });
    let img = render(&p, cid);
    // The sphere casts only (invisible); its shadow lands on the plane behind it.
    let mid = img.data[75 * 200 + 100];
    let side = img.data[75 * 200 + 20];
    assert!(mid[3] > 0.99 && side[3] > 0.99);
    assert!(mid[0] < side[0] * 0.5, "shadowed centre {mid:?} vs lit side {side:?}");
    // Without shadow casting the centre is lit.
    let l = &mut p.comp_mut(cid).unwrap().layers[0];
    set(l, "lightOptions/castsShadows", Value::Bool(false));
    let img = render(&p, cid);
    assert!(img.data[75 * 200 + 100][0] > side[0] * 0.8);
}

#[test]
fn accepts_shadows_only_draws_just_the_shadow() {
    // #1691: a shadow catcher (card or plane primitive, "Accepts Shadows: Only") in front of a
    // red backdrop card, an invisible sphere casting onto it, a point light behind the camera.
    let (mut p, cid) = project(200, 150);
    let red = p.add_item(
        "R",
        Default::default(),
        None,
        ItemKind::Solid(effectcraft_project::Solid { color: [1.0, 0.0, 0.0], width: 400, height: 400, pixel_aspect: 1.0 }),
    );
    add(&mut p, cid, LayerSource::Solid { item: red }, |l| {
        l.switches.three_d = true;
        set(l, "transform/position", Value::Vec3([100.0, 75.0, 300.0]));
        set(l, "materialOptions/acceptsShadows", Value::Enum(0));
    });
    let white = p.add_item(
        "W",
        Default::default(),
        None,
        ItemKind::Solid(effectcraft_project::Solid { color: [1.0, 1.0, 1.0], width: 400, height: 400, pixel_aspect: 1.0 }),
    );
    add(&mut p, cid, LayerSource::Solid { item: white }, |l| {
        l.switches.three_d = true;
        set(l, "transform/position", Value::Vec3([100.0, 75.0, 200.0]));
        set(l, "materialOptions/acceptsShadows", Value::Enum(2));
    });
    add(&mut p, cid, LayerSource::Primitive { kind: PrimitiveKind::Sphere }, |l| {
        set(l, "geometryOptions/radius", Value::Scalar(30.0));
        set(l, "transform/position", Value::Vec3([100.0, 75.0, 50.0]));
        set(l, "materialOptions/castsShadows", Value::Enum(2));
    });
    add(&mut p, cid, LayerSource::Light { kind: LightKind::Point }, |l| {
        set(l, "transform/position", Value::Vec3([100.0, 75.0, -300.0]));
        set(l, "lightOptions/castsShadows", Value::Bool(true));
    });
    let check = |img: &crate::Image, what: &str| {
        let (mid, side) = (img.data[75 * 200 + 100], img.data[75 * 200 + 60]);
        // Unshadowed: the catcher is see-through, the lit red card behind it shows.
        assert!(side[0] > 0.5 && side[1] < side[0] * 0.25 && side[3] > 0.99, "{what}: side {side:?}");
        // Shadowed: black over the red card.
        assert!(mid[0] < 0.02 && mid[3] > 0.99, "{what}: shadow {mid:?} vs side {side:?}");
    };
    check(&render(&p, cid), "card");
    // Nothing behind the catcher: only the shadow is opaque.
    let solo = |p: &Project| {
        let mut q = p.clone();
        q.comp_mut(cid).unwrap().layers.pop();
        let img = render(&q, cid);
        let (mid, side) = (img.data[75 * 200 + 100], img.data[75 * 200 + 60]);
        assert!(side[3] < 1e-3, "unshadowed catcher is transparent: {side:?}");
        assert!(mid[3] > 0.5 && mid[0] < 1e-3, "shadow is black: {mid:?}");
    };
    solo(&p);
    // The same with a plane primitive as the catcher.
    let layers = &mut p.comp_mut(cid).unwrap().layers;
    layers.remove(2);
    add(&mut p, cid, LayerSource::Primitive { kind: PrimitiveKind::Plane }, |l| {
        set(l, "geometryOptions/width", Value::Scalar(400.0));
        set(l, "geometryOptions/height", Value::Scalar(400.0));
        set(l, "transform/position", Value::Vec3([100.0, 75.0, 200.0]));
        set(l, "materialOptions/acceptsShadows", Value::Enum(2));
    });
    check(&render(&p, cid), "plane");
    solo(&p);
}

#[test]
fn cards_and_environment_light() {
    let (mut p, cid) = project(120, 90);
    let sid = p.add_item(
        "S",
        Default::default(),
        None,
        ItemKind::Solid(effectcraft_project::Solid { color: [0.5, 0.5, 0.5], width: 60, height: 40, pixel_aspect: 1.0 }),
    );
    add(&mut p, cid, LayerSource::Solid { item: sid }, |l| l.switches.three_d = true);
    let unlit = render(&p, cid);
    let c = unlit.data[20 * 120 + 30];
    assert!((c[0] - 0.5).abs() < 0.01 && c[3] > 0.99, "unlit card keeps its colour: {c:?}");
    assert_eq!(unlit.data[80 * 120 + 110][3], 0.0);
    // An Environment light sourcing a white environment layer brightens the card.
    let env = p.add_item(
        "E",
        Default::default(),
        None,
        ItemKind::Solid(effectcraft_project::Solid { color: [1.0, 1.0, 1.0], width: 64, height: 32, pixel_aspect: 1.0 }),
    );
    add(&mut p, cid, LayerSource::Solid { item: env }, |l| {
        l.environment = true;
        l.switches.three_d = true;
    });
    add(&mut p, cid, LayerSource::Light { kind: LightKind::Environment }, |_| {});
    let lit = render(&p, cid);
    let c2 = lit.data[20 * 120 + 30];
    assert!(c2[0] > 0.3, "{c2:?}");
    // The environment layer itself is not drawn.
    assert_eq!(lit.data[80 * 120 + 110][3], 0.0);
}

/// An Environment light with no Source and no Environment Layer lights the scene with a flat
/// default environment of its colour × intensity — After Effects "uses a default environment
/// map" for an Environment light — not nothing at all (#2436).
#[test]
fn environment_light_without_a_source_lights_the_scene() {
    let (mut p, cid) = project(120, 90);
    let sid = p.add_item(
        "S",
        Default::default(),
        None,
        ItemKind::Solid(effectcraft_project::Solid { color: [0.5, 0.5, 0.5], width: 60, height: 40, pixel_aspect: 1.0 }),
    );
    add(&mut p, cid, LayerSource::Solid { item: sid }, |l| l.switches.three_d = true);
    let unlit = render(&p, cid);
    let c = unlit.data[20 * 120 + 30];
    assert!((c[0] - 0.5).abs() < 0.01 && c[3] > 0.99, "unlit card keeps its colour: {c:?}");
    // No Source, no Environment Layer: the light still lights the card (a flat white
    // environment at the light's colour × intensity; a card's Material Options shade it).
    add(&mut p, cid, LayerSource::Light { kind: LightKind::Environment }, |_| {});
    let lit = render(&p, cid);
    let c2 = lit.data[20 * 120 + 30];
    assert!((c2[0] - c[0]).abs() > 0.05 && c2[3] > 0.99, "the light has an effect: {c2:?} vs {c:?}");
    // The light's parameters drive it: half intensity darkens towards zero intensity's black.
    set(p.comp_mut(cid).unwrap().layers.first_mut().unwrap(), "lightOptions/intensity", Value::Scalar(50.0));
    let half = render(&p, cid);
    let c3 = half.data[20 * 120 + 30];
    assert!(c3[0] < c2[0] - 0.02, "half intensity lights less: {c3:?} vs {c2:?}");
    set(p.comp_mut(cid).unwrap().layers.first_mut().unwrap(), "lightOptions/intensity", Value::Scalar(0.0));
    let dark = render(&p, cid);
    let c4 = dark.data[20 * 120 + 30];
    assert!(c4[0] < 0.02 && c4[3] > 0.99, "zero intensity lights nothing: {c4:?}");
    // And its colour tints the light red.
    set(p.comp_mut(cid).unwrap().layers.first_mut().unwrap(), "lightOptions/intensity", Value::Scalar(100.0));
    set(p.comp_mut(cid).unwrap().layers.first_mut().unwrap(), "lightOptions/color", Value::Color([1.0, 0.0, 0.0, 1.0]));
    let red = render(&p, cid);
    let c5 = red.data[20 * 120 + 30];
    assert!(c5[0] > 0.15 && c5[1] < 0.05 && c5[2] < 0.05, "red environment: {c5:?}");
}

/// #2436 review: a selected or auto-picked environment source that fails to render (missing
/// footage, a deleted layer) must not silently swap in the default studio environment — the
/// light contributes nothing, exactly like a layer whose source failed renders nothing. Only a
/// light with no Source at all (and no Environment Layer in the comp) uses the default.
#[test]
fn a_failed_environment_source_lights_nothing() {
    let (mut p, cid) = project(120, 90);
    let sid = p.add_item(
        "S",
        Default::default(),
        None,
        ItemKind::Solid(effectcraft_project::Solid { color: [1.0, 1.0, 1.0], width: 60, height: 40, pixel_aspect: 1.0 }),
    );
    add(&mut p, cid, LayerSource::Solid { item: sid }, |l| l.switches.three_d = true);
    let unlit = render(&p, cid);
    // The Environment Layer's footage is unreadable (missing; NoFootage serves no frames).
    let fid = p.add_item(
        "F",
        Default::default(),
        None,
        ItemKind::Footage(effectcraft_project::Footage {
            path: "missing.exr".into(),
            kind: effectcraft_project::FootageKind::Still,
            width: 64,
            height: 32,
            pixel_aspect: 1.0,
            duration: Tick::from_seconds_f64(1.0),
            has_video: true,
            missing: true,
            ..Default::default()
        }),
    );
    add(&mut p, cid, LayerSource::Footage { item: fid }, |l| {
        l.switches.three_d = true;
        l.environment = true;
    });
    let env_layer = p.comp(cid).unwrap().layers.iter().find(|l| l.environment).unwrap().id;
    add(&mut p, cid, LayerSource::Light { kind: LightKind::Environment }, |l| {
        set(l, "lightOptions/castsShadows", Value::Bool(true));
    });
    fn light(p: &mut Project, cid: ItemId) -> &mut effectcraft_project::Layer {
        p.comp_mut(cid).unwrap().layers.iter_mut().find(|l| l.is_light()).unwrap()
    }
    // Auto-picked Environment Layer, unreadable: the light is off — no studio default, no
    // default-environment shadow either.
    let failed = render(&p, cid);
    assert_eq!(failed.data, unlit.data, "an unreadable Environment Layer lights nothing");
    // The light's Source set to that unreadable layer: same.
    set(light(&mut p, cid), "lightOptions/source", Value::Layer(Some(env_layer.0)));
    let failed = render(&p, cid);
    assert_eq!(failed.data, unlit.data, "an unreadable selected Source lights nothing");
    // The Source pointing at a deleted layer: same.
    set(light(&mut p, cid), "lightOptions/source", Value::Layer(Some(999_999)));
    let failed = render(&p, cid);
    assert_eq!(failed.data, unlit.data, "a deleted selected Source lights nothing");
    // Contrast: with no Source and no Environment Layer, the default studio still lights it.
    let comp = p.comp_mut(cid).unwrap();
    comp.layers.retain(|l| l.is_light() || !l.environment);
    set(comp.layers.iter_mut().find(|l| l.is_light()).unwrap(), "lightOptions/source", Value::Layer(None));
    let lit = render(&p, cid);
    let (c, c2) = (unlit.data[20 * 120 + 30], lit.data[20 * 120 + 30]);
    assert!(c2[3] > 0.99 && (c2[0] - c[0]).abs() > 0.05, "the unconfigured light still lights: {c2:?} vs {c:?}");
}

/// #2436: an Environment light's Cast Shadows works like After Effects' — its image's dominant
/// (brightest) region shadows the scene. A bright disc above the image's centre, turned to
/// shine from up and behind the camera (rotation 180°), shadows a sphere onto the floor below
/// it; Cast Shadows off lights it evenly.
#[test]
fn environment_lights_cast_shadows() {
    let (mut p, cid) = project(200, 150);
    // The floor facing the camera, a sphere in front of it casting shadows only.
    add(&mut p, cid, LayerSource::Primitive { kind: PrimitiveKind::Plane }, |l| {
        set(l, "geometryOptions/width", Value::Scalar(400.0));
        set(l, "geometryOptions/height", Value::Scalar(400.0));
        set(l, "transform/position", Value::Vec3([100.0, 75.0, 110.0]));
        set(l, "materialOptions/baseColor", Value::Color([1.0, 1.0, 1.0, 1.0]));
    });
    add(&mut p, cid, LayerSource::Primitive { kind: PrimitiveKind::Sphere }, |l| {
        set(l, "geometryOptions/radius", Value::Scalar(30.0));
        set(l, "transform/position", Value::Vec3([100.0, 75.0, 50.0]));
        set(l, "materialOptions/castsShadows", Value::Enum(2));
    });
    // The Environment Layer: a tight bright key above the image's centre, on black — a
    // full-comp rect keeps the layer buffer full-size, the radial gradient is the key.
    add(&mut p, cid, LayerSource::Shape, |l| {
        l.switches.three_d = true;
        let mut next = 10_000u64;
        let ids = &mut build::Ids(&mut next);
        let rect = build::shape_rect(ids, [200.0, 150.0], [0.0, 0.0], 0.0);
        let key = build::shape_gradient_fill(
            ids,
            true,
            [0.0, -30.0],
            [0.0, 5.0],
            effectcraft_keyframe::Gradient { colors: vec![(0.0, [1.0, 1.0, 1.0, 1.0]), (1.0, [0.0, 0.0, 0.0, 1.0])], opacities: vec![(0.0, 1.0), (1.0, 1.0)] },
        );
        let g = build::shape_group(ids, "Key", vec![rect, key]);
        l.props.sub_mut("contents").unwrap().children.push(g.into());
        l.environment = true;
    });
    // Turned 180°, the key shines from up and behind the camera onto the sphere and floor.
    add(&mut p, cid, LayerSource::Light { kind: LightKind::Environment }, |l| {
        set(l, "lightOptions/rotation", Value::Scalar(180.0));
        set(l, "lightOptions/castsShadows", Value::Bool(true));
    });
    // The shadow lands below the sphere on the floor: its core is the darkest floor pixel
    // below the sphere; the lit floor is the sample to the left. The frame is sRGB-encoded,
    // so the ratios below are gamma-compressed (a linear halving reads as ~0.73).
    let luma = |img: &crate::Image, x: usize, y: usize| (img.data[y * 200 + x][0] + img.data[y * 200 + x][1] + img.data[y * 200 + x][2]) / 3.0;
    let img = render(&p, cid);
    let side = luma(&img, 25, 75);
    let (mut core, mut cx, mut cy) = (f32::INFINITY, 0usize, 0usize);
    for y in 100..150 {
        for x in 60..140 {
            if luma(&img, x, y) < core {
                core = luma(&img, x, y);
                cx = x;
                cy = y;
            }
        }
    }
    assert!(side > 0.1, "the floor is lit: {side}");
    assert!(core < side * 0.65, "the key shadows the sphere onto the floor: {core} vs {side} at ({cx}, {cy})");
    // Cast Shadows off: the floor is lit everywhere, core included.
    let comp = p.comp_mut(cid).unwrap();
    set(&mut comp.layers[0], "lightOptions/castsShadows", Value::Bool(false));
    let img = render(&p, cid);
    let mid = luma(&img, cx, cy);
    assert!(mid > side * 0.9, "no shadow without Cast Shadows: {mid} vs {side}");
}

/// #2436: a uniform environment has no dominant region, so even with Cast Shadows on it casts
/// nothing — the render is the same as with the option off.
#[test]
fn a_uniform_environment_casts_no_shadow() {
    let (mut p, cid) = project(120, 90);
    let card = p.add_item(
        "S",
        Default::default(),
        None,
        ItemKind::Solid(effectcraft_project::Solid { color: [1.0, 1.0, 1.0], width: 60, height: 40, pixel_aspect: 1.0 }),
    );
    add(&mut p, cid, LayerSource::Solid { item: card }, |l| l.switches.three_d = true);
    let env = p.add_item(
        "E",
        Default::default(),
        None,
        ItemKind::Solid(effectcraft_project::Solid { color: [1.0, 1.0, 1.0], width: 64, height: 32, pixel_aspect: 1.0 }),
    );
    add(&mut p, cid, LayerSource::Solid { item: env }, |l| {
        l.environment = true;
        l.switches.three_d = true;
    });
    add(&mut p, cid, LayerSource::Light { kind: LightKind::Environment }, |l| {
        set(l, "lightOptions/castsShadows", Value::Bool(true));
    });
    let casting = render(&p, cid);
    let comp = p.comp_mut(cid).unwrap();
    set(&mut comp.layers[0], "lightOptions/castsShadows", Value::Bool(false));
    let off = render(&p, cid);
    assert!(casting.data[20 * 120 + 30][0] > 0.3, "the card is lit");
    assert_eq!(casting.data, off.data, "a directionless image casts no shadow");
}

/// #2436 review: a chromatic environment under a coloured light — equally bright red and blue
/// regions on exactly opposite sides of the image cancel to a white light (it casts no
/// shadow), but the red Environment light's tint removes the blue region's pull, so the red
/// one becomes the key and casts the shadow the user asked for, from the red side.
#[test]
fn a_red_light_shadows_from_the_red_region_of_a_chromatic_environment() {
    let (mut p, cid) = project(200, 150);
    // The floor facing the camera, a sphere in front of it casting shadows only.
    add(&mut p, cid, LayerSource::Primitive { kind: PrimitiveKind::Plane }, |l| {
        set(l, "geometryOptions/width", Value::Scalar(400.0));
        set(l, "geometryOptions/height", Value::Scalar(400.0));
        set(l, "transform/position", Value::Vec3([100.0, 75.0, 110.0]));
        set(l, "materialOptions/baseColor", Value::Color([1.0, 1.0, 1.0, 1.0]));
    });
    add(&mut p, cid, LayerSource::Primitive { kind: PrimitiveKind::Sphere }, |l| {
        set(l, "geometryOptions/radius", Value::Scalar(30.0));
        set(l, "transform/position", Value::Vec3([100.0, 75.0, 50.0]));
        set(l, "materialOptions/castsShadows", Value::Enum(2));
    });
    // The Environment Layer: a black background with a red square and an equally bright blue
    // one at exactly antipodal image positions (the blue is the point mirror of the red through
    // the antipodal map x -> x + w/2, y -> h - y: centres (14.5, 44.5) and (114.5, 105.5) in a
    // 200x150 image, 20x20 each) — equal solid-angle weight, opposite pull. The red sits up,
    // left and toward the camera; the blue down, right and away from it.
    add(&mut p, cid, LayerSource::Shape, |l| {
        l.environment = true;
        let mut next = 10_000u64;
        let ids = &mut build::Ids(&mut next);
        // Contents stack like the AE timeline — the first group is on top — so the background
        // goes in last, under the red and blue squares (and its full-comp rect keeps the
        // layer buffer full-size: a shape layer's buffer otherwise crops to its content).
        let red_rect = build::shape_rect(ids, [20.0, 20.0], [-85.5, -30.5], 0.0);
        let red_fill = build::shape_fill(ids, [1.0, 0.0, 0.0, 1.0]);
        let red = build::shape_group(ids, "Red", vec![red_rect, red_fill]);
        let blue_rect = build::shape_rect(ids, [20.0, 20.0], [14.5, 30.5], 0.0);
        let blue_fill = build::shape_fill(ids, [0.0, 0.0, 1.0, 1.0]);
        let blue = build::shape_group(ids, "Blue", vec![blue_rect, blue_fill]);
        let bg_rect = build::shape_rect(ids, [200.0, 150.0], [0.0, 0.0], 0.0);
        let bg_fill = build::shape_fill(ids, [0.0, 0.0, 0.0, 1.0]);
        let bg = build::shape_group(ids, "BG", vec![bg_rect, bg_fill]);
        for g in [red, blue, bg] {
            l.props.sub_mut("contents").unwrap().children.push(g.into());
        }
    });
    // A white light over the chromatic image: the regions cancel — Cast Shadows changes
    // nothing, exactly like a directionless image.
    add(&mut p, cid, LayerSource::Light { kind: LightKind::Environment }, |l| {
        set(l, "lightOptions/castsShadows", Value::Bool(true));
    });
    let white_casting = render(&p, cid);
    let comp = p.comp_mut(cid).unwrap();
    set(comp.layers.iter_mut().find(|l| l.is_light()).unwrap(), "lightOptions/castsShadows", Value::Bool(false));
    let white_off = render(&p, cid);
    assert_eq!(white_casting.data, white_off.data, "a white light sees no dominant region in the opposed red and blue");
    // The red light: the tint removes the blue region, the red one shadows the sphere onto the
    // floor below and right of it (the shadow falls opposite the red region).
    let comp = p.comp_mut(cid).unwrap();
    let light = comp.layers.iter_mut().find(|l| l.is_light()).unwrap();
    set(light, "lightOptions/color", Value::Color([1.0, 0.0, 0.0, 1.0]));
    set(light, "lightOptions/castsShadows", Value::Bool(true));
    let img = render(&p, cid);
    let chan = |img: &crate::Image, x: usize, y: usize| img.data[y * 200 + x][0];
    let side = chan(&img, 25, 75);
    let (mut core, mut cx) = (f32::INFINITY, 0usize);
    for y in 100..150 {
        for x in 60..170 {
            if chan(&img, x, y) < core {
                core = chan(&img, x, y);
                cx = x;
            }
        }
    }
    assert!(side > 0.1, "the floor is lit red: {side}");
    assert!(core < side * 0.65, "the red region shadows the sphere onto the floor: {core} vs {side} at x {cx}");
    assert!(cx > 100, "the shadow is opposite the red region (right of the sphere): {cx}");
}

/// #2436: the default environment (no Source, no Environment Layer) is a dim studio — a mean
/// of 0.3, not the 1.0 that would act like a 100 % white Ambient light and wash the scene out
/// (After Effects' default does not) — and directional: its dominant region points up and
/// toward the initial camera, so a shadow-casting Environment light casts soft, partial
/// shadows out of the box, like After Effects' default.
#[test]
fn default_environment_keeps_the_level_and_points_up() {
    let (w, h, d) = scene::default_environment();
    assert_eq!((w, h), (8, 4));
    let (mut mean, mut weight) = (0.0f32, 0.0f32);
    for (i, p) in d.iter().enumerate() {
        let v = ((i / w as usize) as f32 + 0.5) / h as f32;
        let sa = (v * PI).sin();
        mean += (p[0] + p[1] + p[2]) / 3.0 * sa;
        weight += sa;
    }
    assert!((mean / weight - 0.3).abs() < 0.01, "dim studio mean: {}", mean / weight);
    let (sun, key) = dominant_direction(w, h, &d, [1.0; 3], 0.0);
    assert!(sun[1] < -0.5, "the key is up: {sun:?}");
    assert!(sun[2] < -0.3, "the key is toward the camera: {sun:?}");
    assert!(sun[0].abs() < 0.1, "symmetric left-right: {sun:?}");
    assert!((0.35..0.55).contains(&key), "a broad key with fill: {key}");
    // No texel clips past white at the default intensity (the fog of a mean-1 environment).
    assert!(d.iter().all(|p| p[0] <= 1.0 && p[0] > 0.0), "0 < radiance <= 1");
}

/// #2436: with no Source and no Environment Layer, the Environment light's default environment
/// casts soft, partial shadows like After Effects' default environment map — a sphere shadows
/// the plane behind it, darkening part of the light while the fill keeps the floor visible
/// (an area-light shadow, not the pitch black a point light casts); Cast Shadows off lights
/// the floor evenly.
#[test]
fn the_default_environment_casts_shadows() {
    let (mut p, cid) = project(200, 150);
    // The floor facing the camera, a sphere in front of it casting shadows only.
    add(&mut p, cid, LayerSource::Primitive { kind: PrimitiveKind::Plane }, |l| {
        set(l, "geometryOptions/width", Value::Scalar(400.0));
        set(l, "geometryOptions/height", Value::Scalar(400.0));
        set(l, "transform/position", Value::Vec3([100.0, 75.0, 120.0]));
        set(l, "materialOptions/baseColor", Value::Color([1.0, 1.0, 1.0, 1.0]));
    });
    add(&mut p, cid, LayerSource::Primitive { kind: PrimitiveKind::Sphere }, |l| {
        set(l, "geometryOptions/radius", Value::Scalar(30.0));
        set(l, "transform/position", Value::Vec3([100.0, 45.0, 50.0]));
        set(l, "materialOptions/castsShadows", Value::Enum(2));
    });
    add(&mut p, cid, LayerSource::Light { kind: LightKind::Environment }, |l| {
        set(l, "lightOptions/castsShadows", Value::Bool(true));
    });
    // The default key (up and toward the camera) drops the shadow below the sphere: the core
    // is the darkest floor pixel below it, the lit floor the sample to the left.
    let luma = |img: &crate::Image, x: usize, y: usize| (img.data[y * 200 + x][0] + img.data[y * 200 + x][1] + img.data[y * 200 + x][2]) / 3.0;
    let img = render(&p, cid);
    let side = luma(&img, 30, 100);
    let (mut core, mut cx, mut cy) = (f32::INFINITY, 0usize, 0usize);
    for y in 110..150 {
        for x in 80..120 {
            if luma(&img, x, y) < core {
                core = luma(&img, x, y);
                cx = x;
                cy = y;
            }
        }
    }
    assert!(side > 0.1, "the floor is lit: {side}");
    // Partial: the key lobe is occluded (a clear darkening) but the fill keeps shining —
    // not the pitch black a point light's shadow would cast at the same darkness. The frame
    // is sRGB-encoded, so the ratios are gamma-compressed (a linear halving reads as ~0.73).
    assert!(core < side * 0.8, "the default environment shadows the sphere onto the floor: {core} vs {side} at ({cx}, {cy})");
    assert!(core > side * 0.45, "an area-light shadow, partial like After Effects': {core} vs {side}");
    // Cast Shadows off: the floor is lit everywhere, core included.
    let comp = p.comp_mut(cid).unwrap();
    set(&mut comp.layers[0], "lightOptions/castsShadows", Value::Bool(false));
    let img = render(&p, cid);
    let mid = luma(&img, cx, cy);
    assert!(mid > side * 0.9, "no shadow without Cast Shadows: {mid} vs {side}");
}

#[test]
fn depth_of_field_blurs_out_of_focus_surfaces() {
    let mut img = Image::new(64, 64);
    for y in 0..64 {
        for x in 0..64 {
            let v = if (x / 4 + y / 4) % 2 == 0 { 1.0 } else { 0.0 };
            img.data[y * 64 + x] = [v, v, v, 1.0];
        }
    }
    let var = |im: &Image| {
        let m: f32 = im.data.iter().map(|p| p[0]).sum::<f32>() / im.data.len() as f32;
        im.data.iter().map(|p| (p[0] - m).powi(2)).sum::<f32>() / im.data.len() as f32
    };
    let dof = crate::three_d::camera::Dof { focus: 1000.0, aperture: 40.0, blur_level: 1.0, iris: Default::default(), highlight: Default::default() };
    let sharp = depth_of_field(&img, &vec![1000.0; 64 * 64], &dof, 1.0);
    assert!((var(&sharp) - var(&img)).abs() < 1e-6, "in focus: unchanged");
    let blurred = depth_of_field(&img, &vec![3000.0; 64 * 64], &dof, 1.0);
    assert!(var(&blurred) < var(&img) * 0.3, "out of focus: blurred");
}

#[test]
fn extruded_text_renders_as_a_mesh() {
    let (mut p, cid) = project(200, 100);
    add(&mut p, cid, LayerSource::Text, |l| {
        l.switches.three_d = true;
        let doc = effectcraft_keyframe::TextDoc { text: "IO".into(), size: 60.0, fill: [1.0, 1.0, 1.0, 1.0], apply_fill: true, ..Default::default() };
        set(l, "text/sourceText", Value::Text(Box::new(doc)));
        let mut next = 10_000u64;
        l.props.children.push(build::extrusion_geometry_options(&mut build::Ids(&mut next)).into());
        set(l, "geometryOptions/extrusionDepth", Value::Scalar(30.0));
        set(l, "geometryOptions/bevelStyle", Value::Enum(1));
        set(l, "transform/rotationY", Value::Scalar(40.0));
    });
    let ctx = crate::EvalCtx {
        project: &p,
        comp_id: cid,
        comp: p.comp(cid).unwrap(),
        time: Tick::ZERO,
        expr: None,
        footage: None,
        property_overrides: &[],
        media_overrides: &[],
    };
    let l = &ctx.comp.layers[0];
    let params = scene::extrusion_params(&ctx, l).unwrap();
    let meshes = scene::extruded_meshes(&ctx, l, &params);
    assert!(!meshes.is_empty());
    assert!(meshes.iter().all(|(m, _, _)| m.positions.iter().any(|q| (q[2] - 30.0).abs() < 1e-3)));
    let img = render(&p, cid);
    assert!(img.data.iter().filter(|q| q[3] > 0.5).count() > 200);
}

#[test]
fn shape_surface_items_colour_the_extruded_surfaces() {
    let (mut p, cid) = project(200, 200);
    add(&mut p, cid, LayerSource::Shape, |l| {
        l.switches.three_d = true;
        let mut next = 10_000u64;
        let ids = &mut build::Ids(&mut next);
        let surface = |ids: &mut build::Ids, m: &str, c: [f64; 4]| {
            let mut g = build::shape_surface(ids, m).unwrap();
            g.children.push(build::shape_surface_prop(ids, "color").unwrap().into());
            if let Some(effectcraft_project::Node::Prop(pr)) = g.children.last_mut() {
                pr.value = Value::Color(c);
            }
            g
        };
        // A white square whose group colours its sides green; the layer's Contents colour every
        // front blue and every side red, but the group's own Side wins.
        let rect = build::shape_rect(ids, [80.0, 80.0], [0.0, 0.0], 0.0);
        let fill = build::shape_fill(ids, [1.0, 1.0, 1.0, 1.0]);
        let side = surface(ids, "side", [0.0, 1.0, 0.0, 1.0]);
        let group = build::shape_group(ids, "Group 1", vec![rect, fill, side]);
        let front = surface(ids, "front", [0.0, 0.0, 1.0, 1.0]);
        let outer_side = surface(ids, "side", [1.0, 0.0, 0.0, 1.0]);
        let contents = l.props.sub_mut("contents").unwrap();
        contents.children.extend([group.into(), front.into(), outer_side.into()]);
        l.props.children.push(build::extrusion_geometry_options(ids).into());
        set(l, "geometryOptions/extrusionDepth", Value::Scalar(40.0));
        set(l, "transform/position", Value::Vec3([100.0, 100.0, 0.0]));
    });
    let ctx = crate::EvalCtx {
        project: &p,
        comp_id: cid,
        comp: p.comp(cid).unwrap(),
        time: Tick::ZERO,
        expr: None,
        footage: None,
        property_overrides: &[],
        media_overrides: &[],
    };
    let l = &ctx.comp.layers[0];
    let params = scene::extrusion_params(&ctx, l).unwrap();
    let meshes = scene::extruded_meshes(&ctx, l, &params);
    // No bevel: Front, Side and Back (the Back keeps the fill's white).
    let colours: Vec<[f32; 4]> = meshes.iter().map(|m| m.1).collect();
    assert_eq!(colours, [[0.0, 0.0, 1.0, 1.0], [0.0, 1.0, 0.0, 1.0], [1.0, 1.0, 1.0, 1.0]]);
    assert!(meshes[0].0.normals.iter().all(|n| n[2] < -0.99), "front cap");
    assert!(meshes[1].0.normals.iter().all(|n| n[2].abs() < 1e-6), "walls");
    // Rendered (unlit: base colours) at an angle: the blue face and the green side both show.
    p.comp_mut(cid).unwrap().layers[0].props.prop_mut("transform/rotationY").unwrap().value = Value::Scalar(50.0);
    let img = render(&p, cid);
    let blue = img.data.iter().filter(|q| q[3] > 0.99 && q[2] > 0.9 && q[0] < 0.1 && q[1] < 0.1).count();
    let green = img.data.iter().filter(|q| q[3] > 0.99 && q[1] > 0.9 && q[0] < 0.1 && q[2] < 0.1).count();
    assert!(blue > 200 && green > 50, "blue {blue}, green {green}");
}

#[test]
fn text_animators_colour_the_extruded_surfaces() {
    let (mut p, cid) = project(200, 100);
    add(&mut p, cid, LayerSource::Text, |l| {
        l.switches.three_d = true;
        let doc = effectcraft_keyframe::TextDoc { text: "IO".into(), size: 60.0, fill: [1.0, 1.0, 1.0, 1.0], apply_fill: true, ..Default::default() };
        set(l, "text/sourceText", Value::Text(Box::new(doc)));
        let mut next = 10_000u64;
        let ids = &mut build::Ids(&mut next);
        let props = ["sideColor", "bevelColor"].iter().flat_map(|k| build::text_anim_props(ids, k, false)).collect();
        let anim = build::text_animator(ids, "Animator 1", props);
        l.props.group_mut("text/animators").unwrap().children.push(anim.into());
        set(l, "text/animators/#1/properties/sideColor", Value::Color([0.0, 1.0, 0.0, 1.0]));
        // Half the selection: the bevel colour blends half way over the fill.
        set(l, "text/animators/#1/selectors/#1/advanced/amount", Value::Scalar(50.0));
        l.props.children.push(build::extrusion_geometry_options(ids).into());
        set(l, "geometryOptions/extrusionDepth", Value::Scalar(30.0));
        set(l, "geometryOptions/bevelStyle", Value::Enum(1));
    });
    let ctx = crate::EvalCtx {
        project: &p,
        comp_id: cid,
        comp: p.comp(cid).unwrap(),
        time: Tick::ZERO,
        expr: None,
        footage: None,
        property_overrides: &[],
        media_overrides: &[],
    };
    let l = &ctx.comp.layers[0];
    let params = scene::extrusion_params(&ctx, l).unwrap();
    let meshes = scene::extruded_meshes(&ctx, l, &params);
    // Two characters × (Front, Bevel, Side, Back).
    assert_eq!(meshes.len(), 8);
    for m in meshes.chunks(4) {
        assert_eq!(m[0].1, [1.0, 1.0, 1.0, 1.0], "front keeps the fill");
        assert_eq!(m[1].1, [1.0, 0.5, 0.5, 1.0], "bevel: red half way over white");
        assert_eq!(m[2].1, [0.5, 1.0, 0.5, 1.0], "side: green half way over white");
        assert_eq!(m[3].1, [1.0, 1.0, 1.0, 1.0], "back keeps the fill");
    }
}

// ---------------------------------------------------------------- motion blur, blend modes, mattes

fn solid(p: &mut Project, color: [f32; 3], w: u32, h: u32) -> ItemId {
    p.add_item("S", Default::default(), None, ItemKind::Solid(effectcraft_project::Solid { color, width: w, height: h, pixel_aspect: 1.0 }))
}

/// Pixels of row `y` whose alpha is partial (motion-blur smear or anti-aliasing).
fn partial(img: &crate::Image, y: u32) -> usize {
    (0..img.width).filter(|x| (0.05..0.95).contains(&img.data[(y * img.width + x) as usize][3])).count()
}

#[test]
fn motion_blur_smears_moving_meshes() {
    let (mut p, cid) = project(160, 120);
    add(&mut p, cid, LayerSource::Primitive { kind: PrimitiveKind::Cube }, |l| {
        set(l, "geometryOptions/width", Value::Scalar(30.0));
        set(l, "geometryOptions/height", Value::Scalar(30.0));
        set(l, "geometryOptions/depth", Value::Scalar(30.0));
        // 60 px per frame to the right.
        let pr = l.props.prop_mut("transform/position").unwrap();
        pr.keys = vec![
            effectcraft_keyframe::Keyframe::new(Tick::ZERO, Value::Vec3([50.0, 60.0, 0.0])),
            effectcraft_keyframe::Keyframe::new(Tick::from_seconds_f64(1.0), Value::Vec3([50.0 + 30.0 * 60.0, 60.0, 0.0])),
        ];
    });
    let sharp = render(&p, cid);
    let hard = partial(&sharp, 60);
    assert!(hard <= 4, "no blur without the switch: {hard}");
    p.comp_mut(cid).unwrap().layers[0].switches.motion_blur = true;
    let blurred = render(&p, cid);
    let smear = partial(&blurred, 60);
    // A 180° shutter smears over half a frame of motion (≈ 30 px).
    assert!(smear > 20, "smeared: {smear}");
    // Coverage is conserved: the summed alpha along the row stays about the cube's width.
    let sum = |im: &crate::Image| (0..160).map(|x| im.data[60 * 160 + x][3]).sum::<f32>();
    assert!((sum(&blurred) - sum(&sharp)).abs() < 4.0, "{} vs {}", sum(&blurred), sum(&sharp));
    // The comp's motion blur switch and the render option turn it off.
    p.comp_mut(cid).unwrap().enable_motion_blur = false;
    assert!(partial(&render(&p, cid), 60) <= 4);
    p.comp_mut(cid).unwrap().enable_motion_blur = true;
    let r = Renderer::new(&p, &NoFootage, RenderOpts { motion_blur: false, ..Default::default() });
    assert!(partial(&r.comp_frame(cid, Tick::ZERO), 60) <= 4);
    // Shutter angle 0 has no exposure time: no smear.
    p.comp_mut(cid).unwrap().shutter_angle = 0.0;
    assert!(partial(&render(&p, cid), 60) <= 4);
}

#[test]
fn blend_modes_and_occlusion_in_advanced_3d() {
    let (mut p, cid) = project(120, 90);
    let red = solid(&mut p, [1.0, 0.0, 0.0], 120, 90);
    let gray = solid(&mut p, [0.5, 0.5, 0.5], 60, 40);
    let blue = solid(&mut p, [0.0, 0.0, 1.0], 20, 20);
    // Back: a red card; middle: a gray Multiply card; front: a small blue card on the left.
    add(&mut p, cid, LayerSource::Solid { item: red }, |l| {
        l.switches.three_d = true;
    });
    add(&mut p, cid, LayerSource::Solid { item: gray }, |l| {
        l.switches.three_d = true;
        l.blend_mode = effectcraft_color::BlendMode::Multiply;
        set(l, "transform/position", Value::Vec3([60.0, 45.0, -20.0]));
        set(l, "transform/anchor", Value::Vec3([30.0, 20.0, 0.0]));
    });
    add(&mut p, cid, LayerSource::Solid { item: blue }, |l| {
        l.switches.three_d = true;
        set(l, "transform/position", Value::Vec3([45.0, 45.0, -40.0]));
        set(l, "transform/anchor", Value::Vec3([10.0, 10.0, 0.0]));
    });
    let img = render(&p, cid);
    let at = |x: usize, y: usize| img.data[y * 120 + x];
    // Multiply over red: half red, no gray.
    let c = at(70, 45);
    assert!((c[0] - 0.5).abs() < 0.02 && c[1] < 0.02 && c[2] < 0.02 && c[3] > 0.99, "multiplied {c:?}");
    // Outside the gray card: plain red.
    let r = at(115, 5);
    assert!(r[0] > 0.98 && r[1] < 0.02, "{r:?}");
    // The nearer blue card hides the Multiply card behind it.
    let b = at(40, 45);
    assert!(b[2] > 0.98 && b[0] < 0.02, "occluded {b:?}");
    // Normal mode for comparison: gray covers the red.
    p.comp_mut(cid).unwrap().layers[1].blend_mode = effectcraft_color::BlendMode::Normal;
    let n = render(&p, cid).data[45 * 120 + 70];
    assert!((n[0] - 0.5).abs() < 0.02 && (n[1] - 0.5).abs() < 0.02, "{n:?}");
}

#[test]
fn track_mattes_in_advanced_3d() {
    let (mut p, cid) = project(120, 90);
    let white = solid(&mut p, [1.0, 1.0, 1.0], 120, 90);
    let small = solid(&mut p, [1.0, 1.0, 1.0], 40, 30);
    // Layer order (top first): the matte, then the matted layer.
    add(&mut p, cid, LayerSource::Solid { item: white }, |l| l.switches.three_d = true);
    add(&mut p, cid, LayerSource::Solid { item: small }, |l| {
        l.switches.three_d = true;
        l.switches.video = false;
        // A 3D matte is seen through the camera.
        set(l, "transform/position", Value::Vec3([30.0, 30.0, 0.0]));
        set(l, "transform/anchor", Value::Vec3([20.0, 15.0, 0.0]));
    });
    let matte = p.comp(cid).unwrap().layers[0].id;
    p.comp_mut(cid).unwrap().layers[1].track_matte = Some(effectcraft_project::TrackMatte { layer: matte, kind: effectcraft_project::MatteKind::Alpha });
    let img = render(&p, cid);
    let inside = img.data[30 * 120 + 30];
    let outside = img.data[70 * 120 + 100];
    assert!(inside[3] > 0.99 && inside[0] > 0.99, "{inside:?}");
    assert!(outside[3] < 0.01, "{outside:?}");
    // An inverted alpha matte keeps the outside instead.
    p.comp_mut(cid).unwrap().layers[1].track_matte =
        Some(effectcraft_project::TrackMatte { layer: matte, kind: effectcraft_project::MatteKind::AlphaInverted });
    let img = render(&p, cid);
    assert!(img.data[30 * 120 + 30][3] < 0.01 && img.data[70 * 120 + 100][3] > 0.99);
}

#[test]
fn collapsed_precomps_join_the_advanced_3d_scene() {
    // A cube in a nested (Classic 3D) comp, collapsed into an Advanced 3D comp: the cube is
    // real geometry lit by the parent's light, not a flat card of the nested comp (which, being
    // Classic 3D, draws no meshes at all).
    let (mut p, cid) = project(160, 120);
    let mut nc = Comp::new(160, 120, FrameRate::FPS_30, Tick::from_seconds_f64(1.0));
    nc.renderer = R3::Classic3D;
    let nid = p.add_item("N", Default::default(), None, ItemKind::Comp(nc.into()));
    add(&mut p, nid, LayerSource::Primitive { kind: PrimitiveKind::Cube }, |l| {
        set(l, "geometryOptions/width", Value::Scalar(60.0));
        set(l, "geometryOptions/height", Value::Scalar(60.0));
        set(l, "geometryOptions/depth", Value::Scalar(60.0));
        set(l, "transform/rotationY", Value::Scalar(35.0));
        set(l, "transform/rotationX", Value::Scalar(-25.0));
        set(l, "materialOptions/baseColor", Value::Color([1.0, 1.0, 1.0, 1.0]));
    });
    add(&mut p, cid, LayerSource::Light { kind: LightKind::Parallel }, |_| {});
    add(&mut p, cid, LayerSource::Comp { item: nid }, |l| {
        l.switches.three_d = true;
        l.switches.collapse = true;
    });
    let shades = |img: &crate::Image| {
        let mut s: Vec<i32> = img.data.iter().filter(|q| q[3] > 0.99).map(|q| (q[0] * 20.0) as i32).collect();
        s.sort();
        s.dedup();
        s
    };
    let img = render(&p, cid);
    assert!(img.data[60 * 160 + 80][3] > 0.99, "the nested cube covers the centre");
    assert!(shades(&img).len() >= 3, "faces lit by the parent's light: {:?}", shades(&img));
    // The scene holds the cube's triangles (a card would be two).
    let r = Renderer::new(&p, &NoFootage, RenderOpts::default());
    let comp = p.comp(cid).unwrap();
    let ctx = crate::EvalCtx::new(&p, cid, comp, Tick::ZERO);
    let run: Vec<&effectcraft_project::Layer> = comp.layers.iter().filter(|l| l.is_3d() && !l.is_light()).collect();
    let s = scene_of(&r, &ctx, &run, (160, 120));
    assert!(s.indices.len() / 3 >= 12, "{} triangles", s.indices.len() / 3);
    // A 2D collapsed precomp draws its 3D layers through the parent's Advanced 3D renderer too.
    p.comp_mut(cid).unwrap().layers[0].switches.three_d = false;
    let flat = render(&p, cid);
    assert!(flat.data[60 * 160 + 80][3] > 0.99);
    assert!(shades(&flat).len() >= 3, "{:?}", shades(&flat));
    // Collapse off: the nested comp renders on its own (Classic 3D: no meshes).
    p.comp_mut(cid).unwrap().layers[0].switches.collapse = false;
    p.comp_mut(cid).unwrap().layers[0].switches.three_d = true;
    assert!(render(&p, cid).data.iter().all(|q| q[3] == 0.0));
}

#[test]
fn extruded_strokes_render_as_bevelled_meshes() {
    let (mut p, cid) = project(200, 120);
    add(&mut p, cid, LayerSource::Text, |l| {
        l.switches.three_d = true;
        let doc = effectcraft_keyframe::TextDoc {
            text: "I".into(),
            size: 80.0,
            fill: [1.0, 1.0, 1.0, 1.0],
            apply_fill: true,
            stroke: [1.0, 0.0, 0.0, 1.0],
            apply_stroke: true,
            stroke_width: 8.0,
            stroke_over_fill: false,
            ..Default::default()
        };
        set(l, "text/sourceText", Value::Text(Box::new(doc)));
        let mut next = 10_000u64;
        l.props.children.push(build::extrusion_geometry_options(&mut build::Ids(&mut next)).into());
        set(l, "geometryOptions/extrusionDepth", Value::Scalar(20.0));
        set(l, "geometryOptions/bevelStyle", Value::Enum(1));
    });
    let ctx = crate::EvalCtx {
        project: &p,
        comp_id: cid,
        comp: p.comp(cid).unwrap(),
        time: Tick::ZERO,
        expr: None,
        footage: None,
        property_overrides: &[],
        media_overrides: &[],
    };
    let l = &ctx.comp.layers[0];
    let params = scene::extrusion_params(&ctx, l).unwrap();
    let meshes = scene::extruded_meshes(&ctx, l, &params);
    assert_eq!(meshes.len(), 2, "fill and stroke");
    let width = |m: &effectcraft_model::Primitive| {
        let (a, b) = m.positions.iter().fold((f32::INFINITY, f32::NEG_INFINITY), |(a, b), q| (a.min(q[0]), b.max(q[0])));
        b - a
    };
    let (fill, stroke) = (&meshes[0], &meshes[1]);
    assert_eq!(fill.1, [1.0, 1.0, 1.0, 1.0]);
    assert_eq!(stroke.1, [1.0, 0.0, 0.0, 1.0]);
    // The stroke reaches half its width beyond the fill on each side, and is extruded too.
    assert!((width(&stroke.0) - width(&fill.0) - 8.0).abs() < 1.0, "{} vs {}", width(&stroke.0), width(&fill.0));
    assert!(stroke.0.positions.iter().any(|q| q[2] > 19.0));
    // Fill Over Stroke: the fill's front cap sits in front of the stroke's.
    let front = |m: &effectcraft_model::Primitive| m.positions.iter().fold(f32::INFINITY, |a, q| a.min(q[2]));
    assert!(front(&fill.0) < front(&stroke.0));
    // Rendered (unlit: base colours): a red rim around the white face.
    let img = render(&p, cid);
    let red = img.data.iter().filter(|q| q[3] > 0.99 && q[0] > 0.9 && q[1] < 0.1).count();
    let white = img.data.iter().filter(|q| q[3] > 0.99 && q[1] > 0.9).count();
    assert!(red > 50 && white > 50, "red {red}, white {white}");
}

#[test]
fn stroke_regions_are_rings() {
    // A closed path's stroke covers a ring (the inside stays open), so an extruded stroke is a
    // hollow frame around its fill.
    let sq = effectcraft_path::rect([100.0, 100.0], [0.0, 0.0], 0.0);
    let st = effectcraft_path::StrokeStyle { width: 10.0, ..Default::default() };
    let r = crate::shapes::stroke_region(std::slice::from_ref(&sq), &st).unwrap();
    let a = effectcraft_path::boolean::filled_area(&r, effectcraft_path::FillRule::NonZero);
    assert!((a - (110.0 * 110.0 - 90.0 * 90.0)).abs() < 20.0, "{a}");
    let outlines = effectcraft_model::extrude::group_contours(scene::contours(&r, 0.25));
    assert_eq!(outlines.len(), 1);
    assert_eq!(outlines[0].holes.len(), 1);
}

// ---------------------------------------------------------------- shadow quality

use super::raster::{MAX_SHADOW_TEXELS, shadow_map_size, shadow_point_face_size, shadow_sizes};
use super::scene::ShadowMap;

/// Geometric normal of a receiver squarely facing a `flat_map` light (which looks along +z).
const FACING: [f32; 3] = [0.0, 0.0, -1.0];

/// Identity-view orthographic map: map coords are world x, y × `focal` offset by
/// `size/2` (`focal` texels per world unit).
fn flat_map(size: u32, focal: f32, radius: f32) -> ShadowMap {
    ShadowMap { view: [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]], focal, ortho: true, size, offset: 0, bias: 1.5, radius }
}

/// Wall: texel columns ≥ a quarter of the map at depth 10, the rest unoccluded (1000).
/// Its edge sits at world x = −size / (4·focal).
fn wall_map(size: u32, focal: f32, radius: f32) -> (ShadowMap, Vec<f32>) {
    let texels: Vec<f32> = (0..size * size).map(|i| if i % size >= size / 4 { 10.0 } else { 1000.0 }).collect();
    (flat_map(size, focal, radius), texels)
}

/// Luma of a render of `cid` with its top layer (a light) set to cast shadows or not,
/// at the given Shadow Diffusion.
fn render_light_luma(p: &mut Project, cid: ItemId, casting: bool, diffusion: f64) -> Vec<f32> {
    let comp = p.comp_mut(cid).unwrap();
    set(&mut comp.layers[0], "lightOptions/castsShadows", Value::Bool(casting));
    set(&mut comp.layers[0], "lightOptions/shadowDiffusion", Value::Scalar(diffusion));
    render(p, cid).data.iter().map(|q| (q[0] + q[1] + q[2]) / 3.0).collect()
}

#[test]
fn shadow_map_size_tracks_raster_resolution() {
    assert_eq!(shadow_map_size(640, 360, false), 1024);
    assert_eq!(shadow_map_size(1920, 1080, false), 2048);
    assert_eq!(shadow_map_size(3840, 2160, false), 4096);
    // Clamped to the pow2 range: small or absurd rasters stay inside 512..4096.
    assert_eq!(shadow_map_size(64, 64, false), 512);
    assert_eq!(shadow_map_size(0, 0, false), 512);
    assert_eq!(shadow_map_size(u32::MAX, u32::MAX, false), 4096);
    assert_eq!(shadow_map_size(7, 100000, false), 4096);
    // The fast-preview (draft) budget stays at 512 regardless, so interactivity is
    // untouched by the quality path's new sizing.
    assert_eq!(shadow_map_size(3840, 2160, true), 512);
    assert_eq!(shadow_map_size(0, 0, true), 512);
}

#[test]
fn shadow_point_faces_are_capped() {
    // Cube faces take the full map size up to 2048 (one 2048 cube ≈ 25 M f32 texels).
    assert_eq!(shadow_point_face_size(4096, false), 2048);
    assert_eq!(shadow_point_face_size(2048, false), 2048);
    assert_eq!(shadow_point_face_size(1024, false), 1024);
    assert_eq!(shadow_point_face_size(512, false), 512);
    // Draft keeps the historical half-size face.
    assert_eq!(shadow_point_face_size(512, true), 256);
}

fn budget_texels(sizes: &[Option<u32>], kinds: &[u32]) -> u64 {
    sizes.iter().zip(kinds).map(|(s, &k)| s.map(|z| (z as u64) * (z as u64) * if k == 2 { 6 } else { 1 }).unwrap_or(0)).sum()
}

#[test]
fn shadow_sizes_stay_within_the_scene_budget() {
    // Within budget a light gets its full size: one 4K point cube keeps 2048 faces.
    assert_eq!(shadow_sizes(4096, 2160, false, &[2]), vec![Some(2048)]);
    // Several cubes overflow: deterministically halved (largest first, earliest light
    // on ties) until inside: three or four settle at 1024, five push two to 512.
    assert_eq!(shadow_sizes(4096, 2160, false, &[2; 3]), vec![Some(1024); 3]);
    assert_eq!(shadow_sizes(4096, 2160, false, &[2; 4]), vec![Some(1024); 4]);
    let sizes = shadow_sizes(4096, 2160, false, &[2; 5]);
    assert_eq!(sizes, vec![Some(512), Some(512), Some(1024), Some(1024), Some(1024)]);
    assert!(budget_texels(&sizes, &[2; 5]) <= MAX_SHADOW_TEXELS);
    // A hostile light count can never break the cap; sizes only shrink or drop to None.
    let many = vec![2u32; 1000];
    let sizes = shadow_sizes(3840, 2160, false, &many);
    assert_eq!(sizes.len(), 1000);
    assert!(budget_texels(&sizes, &many) <= MAX_SHADOW_TEXELS);
    assert!(sizes.iter().flatten().all(|&z| (128..=2048).contains(&z)));
    // Deterministic: same inputs, same plan.
    assert_eq!(sizes, shadow_sizes(3840, 2160, false, &many));
    // Non-point lights and a modest scene keep full quality sizes.
    assert_eq!(shadow_sizes(1920, 1080, false, &[1; 2]), vec![Some(2048), Some(2048)]);
    // A light that casts nothing costs nothing: it stays None and leaves its
    // full-size budget to the lights that do cast.
    assert_eq!(shadow_sizes(640, 360, false, &[0, 1]), vec![None, Some(1024)]);
    assert!(shadow_sizes(640, 360, false, &[0; 500]).iter().all(Option::is_none));
}

#[test]
fn diffusion_does_not_lift_a_deep_shadow() {
    // Shadow Diffusion may widen the penumbra but must not scale the depth bias: a
    // point deep inside a shadow stays fully shadowed at any diffusion.
    let (m, texels) = wall_map(64, 1.0, 1.0);
    let deep = [0.0f32, 0.0, 13.5]; // u = 32: ±16 texels of margin around the edge at 16; z − bias 12 > 10
    let hard = shade::shadow_factor(&m, &texels, deep, FACING, 1.0);
    assert!(hard < 0.01, "deep shadow at radius 1: {hard}");
    let wide = shade::shadow_factor(&flat_map(64, 1.0, 25.0), &texels, deep, FACING, 1.0);
    assert!(wide < 0.01, "deep shadow at radius 25 (bias must not grow with radius): {wide}");
}

#[test]
fn pcf_varies_within_a_texel() {
    // Nearest-texel PCF snaps the edge to texel boundaries: two receivers 1/20 texel
    // apart get bit-identical factors, which is what stair-steps straight edges.
    // The filtering must produce coverage that varies within a single texel.
    let (m, texels) = wall_map(64, 1.0, 2.0);
    let mut pairs = 0;
    let mut varying = 0;
    let mut u = -17.0f32;
    while u < -15.3 {
        let a = shade::shadow_factor(&m, &texels, [u, 0.0, 13.5], FACING, 1.0);
        let b = shade::shadow_factor(&m, &texels, [u + 0.05, 0.0, 13.5], FACING, 1.0);
        pairs += 1;
        if (a - b).abs() > 1e-6 {
            varying += 1;
        }
        assert!((0.0..=1.0).contains(&a) && (0.0..=1.0).contains(&b), "{a} {b}");
        u += 0.05;
    }
    assert!(pairs >= 30);
    assert_eq!(varying, pairs, "coverage varies inside every texel along the edge");
}

#[test]
fn wide_diffusion_keeps_shadow_depth_and_contact() {
    // Scene: camera-facing floor, sphere casting, point light behind the camera and
    // off to the upper right so most of the shadow lands beside the sphere, not
    // behind it. Raising Shadow Diffusion must neither wash the shadow out (a
    // diffusion-scaled bias leaks light into it) nor move where it touches.
    let (mut p, cid) = project(200, 150);
    add(&mut p, cid, LayerSource::Primitive { kind: PrimitiveKind::Plane }, |l| {
        set(l, "geometryOptions/width", Value::Scalar(400.0));
        set(l, "geometryOptions/height", Value::Scalar(400.0));
        set(l, "transform/position", Value::Vec3([100.0, 75.0, 200.0]));
        set(l, "materialOptions/baseColor", Value::Color([1.0, 1.0, 1.0, 1.0]));
    });
    add(&mut p, cid, LayerSource::Primitive { kind: PrimitiveKind::Sphere }, |l| {
        set(l, "geometryOptions/radius", Value::Scalar(30.0));
        set(l, "transform/position", Value::Vec3([100.0, 75.0, 50.0]));
        set(l, "materialOptions/castsShadows", Value::Enum(1));
    });
    add(&mut p, cid, LayerSource::Light { kind: LightKind::Point }, |l| {
        set(l, "transform/position", Value::Vec3([180.0, 20.0, -300.0]));
        set(l, "lightOptions/castsShadows", Value::Bool(true));
    });
    let noshadow = render_light_luma(&mut p, cid, false, 0.0);
    let d0 = render_light_luma(&mut p, cid, true, 0.0);
    let d40 = render_light_luma(&mut p, cid, true, 40.0);
    // Per-column darkening vs the shadowless render: depth (max darkening) and the
    // deepest dark row (contact edge). Diffusion may blur the edge but must keep the
    // shadow dark (no bias leak) and keep it where it touches (no lift).
    let dep = |img: &[f32], x: usize| -> f32 { (20..140).map(|y| noshadow[y * 200 + x] - img[y * 200 + x]).fold(0.0f32, f32::max) };
    let cols: Vec<usize> = (30..170).filter(|&x| dep(&d0, x) > 0.1).collect();
    assert!(cols.len() > 30, "shadow covers many columns: {}", cols.len());
    // Core depth: the 90th-percentile darkening over the d0 shadow's pixels. A blur
    // only softens the rim and leaves the core dark; a bias leak lightens all of it.
    let shadow_px: Vec<usize> = (20..140).flat_map(|y| (30..170).map(move |x| y * 200 + x)).filter(|&i| noshadow[i] - d0[i] > 0.1).collect();
    let core = |img: &[f32]| -> f32 {
        let mut v: Vec<f32> = shadow_px.iter().map(|&i| noshadow[i] - img[i]).collect();
        v.sort_by(f32::total_cmp);
        v[v.len() * 9 / 10]
    };
    let (core0, core40) = (core(&d0), core(&d40));
    // Width of the shadow across its centre row: a diffusion-scaled bias shrinks the
    // shadow (light leaks under its edges), while a correct filter keeps the extent
    // and only softens the boundary (threshold crossings move at most with the
    // penumbra, not with the bias).
    let width = |img: &[f32]| -> f32 {
        (30..130)
            .map(|y| {
                let dark: Vec<bool> = (0..200).map(|x| noshadow[y * 200 + x] - img[y * 200 + x] > 0.06).collect();
                match (dark.iter().position(|&d| d), dark.iter().rposition(|&d| d)) {
                    (Some(l), Some(r)) if r > l => (r - l) as f32,
                    _ => 0.0,
                }
            })
            .fold(0.0f32, f32::max)
    };
    let (w0, w40) = (width(&d0), width(&d40));
    let wratio = w40 / w0;
    assert!(w0 > 20.0, "shadow is wide enough to judge: {w0}");
    assert!(core40 >= 0.9 * core0, "diffusion 40 keeps the shadow core dark: {core40:.4} vs {core0:.4}");
    assert!(wratio >= 0.8, "diffusion 40 does not shrink the shadow: width {w40} vs {w0}");
}

#[test]
fn penumbra_dithers_along_a_straight_edge() {
    // Receivers at the same distance from a straight edge, one texel apart along it.
    // A fixed tap grid gives each the same coverage, so the penumbra repeats texel by
    // texel (stair-steps and bands); the jitter, keyed below texel size, must vary it.
    let (m, texels) = wall_map(64, 1.0, 6.0);
    let distinct = |vs: &[f32]| -> usize {
        let mut bits: Vec<u32> = vs.iter().map(|v| v.to_bits()).collect();
        bits.sort_unstable();
        bits.dedup();
        bits.len()
    };
    let along: Vec<f32> = (0..32).map(|k| shade::shadow_factor(&m, &texels, [-16.0, -16.0 + k as f32, 13.5], FACING, 1.0)).collect();
    assert!(distinct(&along) >= 16, "coverage along the edge takes {} distinct values", distinct(&along));
    // And finer than a texel: where one texel spans several output pixels (a grazing
    // floor), dither keyed per texel would show as texel-sized blocks of grain.
    let within: Vec<f32> = (0..8).map(|k| shade::shadow_factor(&m, &texels, [-16.0, 0.0625 + k as f32 * 0.125, 13.5], FACING, 1.0)).collect();
    assert!(distinct(&within) >= 4, "coverage inside one texel takes {} distinct values", distinct(&within));
}

#[test]
fn penumbra_jitter_is_continuous_across_its_lattice() {
    // Receiver pairs 1/5000 texel either side of a line of the 1/8-texel jitter lattice,
    // across u and across v, inside the penumbra. The GPU places a receiver within a few
    // ulps of the CPU, so coverage that jumps at a lattice line differs between the two
    // backends there; both sides of each line must agree within one 8-bit level.
    let (m, texels) = wall_map(64, 1.0, 6.0);
    let f = |p: [f32; 2]| shade::shadow_factor(&m, &texels, [p[0], p[1], 13.5], FACING, 1.0);
    let mut worst = 0.0f32;
    for k in 0..16 {
        let line = k as f32 * 0.125;
        for (a, b) in [([-16.0 + line - 2e-4, 0.3], [-16.0 + line + 2e-4, 0.3]), ([-16.3, line - 2e-4], [-16.3, line + 2e-4])] {
            let (fa, fb) = (f(a), f(b));
            assert!(fa > 0.05 && fa < 0.95, "{a:?} lies in the penumbra: {fa}");
            worst = worst.max((fa - fb).abs());
        }
    }
    assert!(worst < 1.0 / 255.0, "coverage jumps by {worst} across a jitter lattice line");
}

#[test]
fn penumbra_width_is_independent_of_map_resolution() {
    // One wall (edge at world x = −16) in a 64-texel map at 1 texel per world unit and
    // a 256-texel map at 4. Shadow Diffusion sets the penumbra in output pixels (world
    // units here), so a finer map must not narrow it.
    let width = |size: u32, focal: f32| -> f32 {
        let (m, texels) = wall_map(size, focal, 8.0);
        let xs: Vec<f32> = (0..400).map(|k| -30.0 + k as f32 * 0.07).collect();
        let f: Vec<f32> = xs.iter().map(|&x| shade::shadow_factor(&m, &texels, [x, 0.0, 13.5], FACING, 1.0)).collect();
        let first_below = |t: f32| f.iter().position(|&v| v < t).map_or(f32::NAN, |i| xs[i]);
        first_below(0.1) - first_below(0.9)
    };
    let (w64, w256) = (width(64, 1.0), width(256, 4.0));
    assert!(w64 > 4.0 && w64 < 12.0, "penumbra spans about the requested 8 units: {w64}");
    assert!((w256 / w64 - 1.0).abs() < 0.25, "same penumbra at any map resolution: {w64} vs {w256}");
}

#[test]
fn tilted_receivers_do_not_shadow_themselves() {
    // A receiver plane z = 100 + 4x (76° from the light, n·l ≈ 0.24), each texel holding
    // its depth half a texel off-centre the way a rasteriser can. Wide taps must compare
    // against the plane rather than the receiver point, and the slope-scaled bias must
    // absorb the half-texel error, at every diffusion.
    let (size, g) = (128u32, 4.0f32);
    let c = size as f32 * 0.5;
    let texels: Vec<f32> = (0..size * size).map(|i| 100.0 + g * ((i % size) as f32 - c)).collect();
    let len = (1.0 + g * g).sqrt();
    let (ng, n_l) = ([g / len, 0.0, -1.0 / len], 1.0 / len);
    for radius in [1.0f32, 6.0, 25.0] {
        let m = flat_map(size, 1.0, radius);
        for k in 0..40 {
            let x = -20.0 + k as f32 * 0.97;
            let f = shade::shadow_factor(&m, &texels, [x, 3.3, 100.0 + g * x], ng, n_l);
            assert!(f > 0.99, "radius {radius}, x {x}: lit tilted receiver shadowed itself ({f})");
        }
    }
}

#[test]
fn lit_casters_do_not_shadow_themselves() {
    // A sphere and a turned cube that cast shadows, lit by each light kind: the lit
    // surface must render as bright as with shadows off at any Shadow Diffusion (a
    // wide kernel on a surface tilted to the light must not see its own depth).
    // 640×360: faces then span enough texels that near-grazing ones exercise the
    // receiver plane's slope limit.
    for (prim, rot_y) in [(PrimitiveKind::Sphere, 0.0), (PrimitiveKind::Cube, 35.0)] {
        for kind in [LightKind::Point, LightKind::Spot, LightKind::Parallel] {
            let (mut p, cid) = project(640, 360);
            add(&mut p, cid, LayerSource::Primitive { kind: prim }, |l| {
                set(l, "transform/position", Value::Vec3([320.0, 180.0, 100.0]));
                if prim == PrimitiveKind::Sphere {
                    set(l, "geometryOptions/radius", Value::Scalar(120.0));
                } else {
                    for k in ["width", "height", "depth"] {
                        set(l, &format!("geometryOptions/{k}"), Value::Scalar(216.0));
                    }
                    set(l, "transform/rotationX", Value::Scalar(-25.0));
                }
                set(l, "transform/rotationY", Value::Scalar(rot_y));
                set(l, "materialOptions/baseColor", Value::Color([1.0, 1.0, 1.0, 1.0]));
                set(l, "materialOptions/castsShadows", Value::Enum(1));
            });
            add(&mut p, cid, LayerSource::Light { kind }, |l| {
                set(l, "transform/position", Value::Vec3([-280.0, -108.0, -600.0]));
                if let Some(poi) = l.props.prop_mut("transform/pointOfInterest") {
                    poi.value = Value::Vec3([320.0, 180.0, 100.0]);
                }
            });
            let noshadow = render_light_luma(&mut p, cid, false, 0.0);
            let lit: Vec<usize> = (0..noshadow.len()).filter(|&i| noshadow[i] > 0.15).collect();
            assert!(lit.len() > 10_000, "{prim:?} {kind:?}: lit area {}", lit.len());
            for diffusion in [0.0, 20.0, 48.0] {
                let img = render_light_luma(&mut p, cid, true, diffusion);
                let dark = lit.iter().filter(|&&i| img[i] < 0.9 * noshadow[i]).count() as f32 / lit.len() as f32;
                assert!(dark <= 0.02, "{prim:?} {kind:?} diffusion {diffusion}: {:.1} % of the lit surface self-shadowed", dark * 100.0);
            }
        }
    }
}

#[test]
fn shadow_sizes_match_the_halve_largest_rule_and_scale_to_many_lights() {
    // The specification `shadow_sizes` implements, step by step: halve the largest map
    // above its floor (earliest light on ties) until the scene fits, then drop maps
    // from the last light.
    let spec = |w: u32, h: u32, draft: bool, kinds: &[u32]| -> Vec<Option<u32>> {
        let base = shadow_map_size(w, h, draft);
        let mut sizes: Vec<u32> = kinds
            .iter()
            .map(|&k| {
                if k == 0 {
                    0
                } else if k == 2 {
                    shadow_point_face_size(base, draft)
                } else {
                    base
                }
            })
            .collect();
        let total = |s: &[u32]| -> u64 { s.iter().zip(kinds).map(|(&z, &k)| (z as u64) * (z as u64) * if k == 2 { 6 } else { 1 }).sum() };
        while total(&sizes) > MAX_SHADOW_TEXELS {
            let pick = (0..sizes.len()).filter(|&i| sizes[i] > if kinds[i] == 2 { 128 } else { 512 }).max_by_key(|&i| (sizes[i], std::cmp::Reverse(i)));
            match pick {
                Some(i) => sizes[i] /= 2,
                None => match sizes.iter().rposition(|&z| z > 0) {
                    Some(i) => sizes[i] = 0,
                    None => break,
                },
            }
        }
        sizes.into_iter().map(|z| (z > 0).then_some(z)).collect()
    };
    let mut seed = 0x2545_F491u32;
    for case in 0..300 {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let n = (seed >> 8) as usize % 40;
        let kinds: Vec<u32> = (0..n)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (seed >> 16) % 3
            })
            .collect();
        let (w, h, draft) = ([640, 1920, 3840, 8000][case % 4], [360, 1080, 2160, 100][case % 4], case % 7 == 0);
        assert_eq!(shadow_sizes(w, h, draft, &kinds), spec(w, h, draft, &kinds), "case {case}: {kinds:?}");
    }
    // A hostile light count stays fast (n log n) and inside the budget.
    let many = vec![2u32; 100_000];
    let sizes = shadow_sizes(3840, 2160, false, &many);
    assert!(budget_texels(&sizes, &many) <= MAX_SHADOW_TEXELS);
}

#[test]
fn wide_penumbrae_are_smooth() {
    // Coverage at a fixed distance across a wide penumbra, sampled along a straight
    // edge, should be one smooth value plus fine dither. A 5×5 grid stretched over a
    // 20-texel reach leaves gaps the jitter turns into coarse grain (σ ≈ 0.04, ten
    // 8-bit levels); the grid grows with the reach to keep it below 0.02.
    let (m, texels) = wall_map(256, 1.0, 20.0);
    for off in [-5.0f32, 0.0, 5.0] {
        let vs: Vec<f32> = (0..200).map(|k| shade::shadow_factor(&m, &texels, [-64.0 + off, -100.0 + k as f32, 13.5], FACING, 1.0)).collect();
        let mean = vs.iter().sum::<f32>() / vs.len() as f32;
        let sd = (vs.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / vs.len() as f32).sqrt();
        assert!(sd < 0.02, "offset {off}: penumbra grain σ = {sd:.4}");
    }
}

#[test]
fn hard_edges_ramp_over_two_texels() {
    // At Shadow Diffusion 0 the edge still ramps across about two texels (10–90 %):
    // where one texel spans several output pixels (a floor grazing a point light) a
    // one-texel ramp leaves the binary compare's texel steps visible as a sawtooth.
    let (m, texels) = wall_map(64, 1.0, 1.0);
    for v0 in [0.0f32, 0.3, 7.6] {
        let xs: Vec<f32> = (0..400).map(|k| -20.0 + k as f32 * 0.02).collect();
        let f: Vec<f32> = xs.iter().map(|&x| shade::shadow_factor(&m, &texels, [x, v0, 13.5], FACING, 1.0)).collect();
        let first_below = |t: f32| f.iter().position(|&v| v < t).map_or(f32::NAN, |i| xs[i]);
        let w = first_below(0.1) - first_below(0.9);
        assert!((1.7..3.0).contains(&w), "v {v0}: hard edge ramps over {w} texels");
    }
}

// ---------------------------------------------------------------- the After Effects look (#2436)

/// #2436: an Environment light with every option at its default renders like After Effects':
/// a dim studio that leaves the lit floor a mid gray instead of washing it out to white (the
/// fog of a mean-1 default), a partial shadow (the key lobe occluded, the fill still shining —
/// not the pitch black a point light casts), and a soft edge in both quality and draft renders
/// (the map's texel grid never shows through as stair-steps).
#[test]
fn environment_shadows_match_the_after_effects_look() {
    let (mut p, cid) = project(800, 600);
    add(&mut p, cid, LayerSource::Primitive { kind: PrimitiveKind::Plane }, |l| {
        set(l, "geometryOptions/width", Value::Scalar(1200.0));
        set(l, "geometryOptions/height", Value::Scalar(900.0));
        set(l, "transform/position", Value::Vec3([400.0, 300.0, 300.0]));
        set(l, "materialOptions/baseColor", Value::Color([1.0, 1.0, 1.0, 1.0]));
    });
    add(&mut p, cid, LayerSource::Primitive { kind: PrimitiveKind::Sphere }, |l| {
        set(l, "geometryOptions/radius", Value::Scalar(80.0));
        set(l, "transform/position", Value::Vec3([400.0, 300.0, 120.0]));
        set(l, "materialOptions/castsShadows", Value::Enum(2));
    });
    add(&mut p, cid, LayerSource::Light { kind: LightKind::Environment }, |l| {
        set(l, "lightOptions/castsShadows", Value::Bool(true));
    });
    let luma = |img: &crate::Image, x: usize, y: usize| (img.data[y * 800 + x][0] + img.data[y * 800 + x][1] + img.data[y * 800 + x][2]) / 3.0;
    for (name, opts) in [("quality", RenderOpts::default()), ("draft", RenderOpts { draft: true, ..Default::default() })] {
        let img = Renderer::new(&p, &NoFootage, opts).comp_frame(cid, Tick::ZERO);
        // The shadow core: the darkest pixel of the lower half (the key drops the shadow
        // below the sphere); the lit floor is the mean of the band to its left.
        let (mut core, mut cx, mut cy) = (f32::INFINITY, 0usize, 0usize);
        for y in 420..580 {
            for x in 300..500 {
                if luma(&img, x, y) < core {
                    core = luma(&img, x, y);
                    cx = x;
                    cy = y;
                }
            }
        }
        let lit: f32 = (60..260).map(|x| luma(&img, x, cy)).sum::<f32>() / 200.0;
        assert!(lit > 0.15 && lit < 0.85, "{name}: the lit floor is a mid gray, no white fog: {lit}");
        // The frame is sRGB-encoded, so the ratio is gamma-compressed (a linear halving
        // reads as ~0.73): partial means clearly darker than lit, far from black.
        assert!(core > lit * 0.45 && core < lit * 0.85, "{name}: a partial area-light shadow: core {core} vs lit {lit}");
        // The edge's 10-90 % ramp from the lit plateau (left) down into the shadow.
        let first_below = |t: f32| (260..cx).position(|x| luma(&img, x, cy) < core + (lit - core) * t).map(|i| 260 + i);
        let w = (first_below(0.1).unwrap() as f32) - (first_below(0.9).unwrap() as f32);
        assert!(w >= 3.5, "{name}: the edge ramps over {w} px (After Effects' env shadows are soft)");
    }
}
