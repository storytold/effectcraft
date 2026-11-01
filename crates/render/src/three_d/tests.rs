use effectcraft_color::Label;
use effectcraft_geom::{Vec3, vec3};
use effectcraft_keyframe::Value;
use effectcraft_project::build;
use effectcraft_project::{Comp, ItemId, ItemKind, Layer, LayerSource, LightKind, Project, Solid};
use effectcraft_time::{FrameRate, Tick};

use super::camera::{Rig, View3D, default_view_cam, orientation_for};
use super::*;
use crate::{NoFootage, RenderOpts, Renderer, render_frame};

const W: u32 = 200;
const H: u32 = 100;

fn setup() -> (Project, ItemId, Comp) {
    setup_sized(W, H)
}

/// [`setup`] with a `w`×`h` comp.
fn setup_sized(w: u32, h: u32) -> (Project, ItemId, Comp) {
    let mut p = Project::default();
    // Exact float maths; 8/16 bpc quantisation has its own tests (tests_color).
    p.settings.bit_depth = effectcraft_project::BitDepth::Bpc32;
    let comp = Comp::new(w, h, FrameRate::FPS_30, Tick::from_seconds_f64(2.0));
    let cid = p.add_item("Comp", Label::Sandstone, None, ItemKind::Comp(comp.clone().into()));
    (p, cid, comp)
}

/// A 3D solid of `w`×`h` centred at `pos`.
fn solid3(p: &mut Project, comp: &Comp, color: [f32; 3], w: u32, h: u32, pos: [f64; 3]) -> Layer {
    let sid = p.add_item("Solid", Label::Red, None, ItemKind::Solid(Solid { color, width: w, height: h, pixel_aspect: 1.0 }));
    let mut l = build::layer(p, comp, "Solid", LayerSource::Solid { item: sid }, (w, h), None);
    l.switches.three_d = true;
    set(&mut l, "transform/position", Value::Vec3(pos));
    l
}

fn set(l: &mut Layer, path: &str, v: Value) {
    l.props.prop_mut(path).unwrap_or_else(|| panic!("no {path}")).value = v;
}

fn push(p: &mut Project, cid: ItemId, l: Layer) {
    // Appended = lowest in the stack.
    p.comp_mut(cid).unwrap().layers.push(l);
}

fn render(p: &Project, cid: ItemId) -> effectcraft_raster::Image {
    render_frame(p, cid, Tick::ZERO, 1.0)
}

fn close(a: f32, b: f32, eps: f32) -> bool {
    (a - b).abs() <= eps
}

fn zoom() -> f64 {
    W as f64 * 50.0 / 36.0
}

// ---------------------------------------------------------------- projection

#[test]
fn z0_plane_matches_2d() {
    let (mut p, cid, comp) = setup();
    let l = solid3(&mut p, &comp, [1.0, 0.0, 0.0], 40, 20, [50.0, 30.0, 0.0]);
    push(&mut p, cid, l);
    let img = render(&p, cid);
    // Covers x 30..70, y 20..40.
    assert_eq!(img.get(31, 21), [1.0, 0.0, 0.0, 1.0]);
    assert_eq!(img.get(68, 38), [1.0, 0.0, 0.0, 1.0]);
    assert_eq!(img.get(29, 30)[3], 0.0);
    assert_eq!(img.get(71, 30)[3], 0.0);
}

#[test]
fn farther_layers_project_smaller() {
    let (mut p, cid, comp) = setup();
    // At z = zoom the plane is twice as far from the eye: half size around the comp centre.
    let l = solid3(&mut p, &comp, [0.0, 1.0, 0.0], 80, 40, [100.0, 50.0, zoom()]);
    push(&mut p, cid, l);
    let img = render(&p, cid);
    // 40×20 centred on (100, 50): x 80..120, y 40..60.
    assert!(img.get(82, 42)[3] > 0.99);
    assert!(img.get(117, 57)[3] > 0.99);
    assert!(img.get(77, 50)[3] < 0.01, "{:?}", img.get(77, 50));
    assert!(img.get(100, 37)[3] < 0.01);
}

#[test]
fn projection_math_matches_camera_state() {
    let cam = default_camera(1920.0, 1080.0);
    let pr = cam.projection(1920.0, 1080.0);
    let p = pr.apply(vec3(100.0, 200.0, 0.0));
    assert!((p.x - 100.0).abs() < 1e-6 && (p.y - 200.0).abs() < 1e-6);
    // A point at depth 2·zoom maps halfway to the centre.
    let q = cam.project(1920.0, 1080.0, vec3(0.0, 0.0, cam.zoom)).unwrap();
    assert!((q.x - 480.0).abs() < 1e-6 && (q.y - 270.0).abs() < 1e-6, "{q:?}");
    assert!(cam.project(1920.0, 1080.0, vec3(960.0, 540.0, -cam.zoom - 10.0)).is_none());
}

#[test]
fn y_rotation_foreshortens_with_correct_perspective() {
    let (mut p, cid, comp) = setup();
    let mut l = solid3(&mut p, &comp, [1.0, 1.0, 1.0], 100, 60, [100.0, 50.0, 0.0]);
    set(&mut l, "transform/rotationY", Value::Scalar(60.0));
    push(&mut p, cid, l);
    let img = render(&p, cid);
    // cos 60° = 0.5: roughly half as wide; the near edge (right, z<0) is taller than the far edge.
    assert!(img.get(100, 50)[3] > 0.99);
    assert!(img.get(140, 50)[3] < 0.01);
    let col_h = |x: u32| (0..H).filter(|&y| img.get(x as i64, y as i64)[3] > 0.5).count();
    assert!(col_h(122) > col_h(78), "near edge {} vs far edge {}", col_h(122), col_h(78));
}

#[test]
fn layer_behind_camera_is_clipped() {
    let (mut p, cid, comp) = setup();
    let l = solid3(&mut p, &comp, [1.0, 0.0, 0.0], 200, 100, [100.0, 50.0, -zoom() - 50.0]);
    push(&mut p, cid, l);
    let img = render(&p, cid);
    assert!(img.data.iter().all(|px| px[3] == 0.0));
}

// ---------------------------------------------------------------- depth ordering

#[test]
fn nearer_layer_wins_regardless_of_stack_order() {
    let (mut p, cid, comp) = setup();
    let far = solid3(&mut p, &comp, [1.0, 0.0, 0.0], 600, 300, [100.0, 50.0, 200.0]);
    let near = solid3(&mut p, &comp, [0.0, 1.0, 0.0], 40, 40, [100.0, 50.0, 0.0]);
    // Far layer is on top of the stack.
    push(&mut p, cid, far);
    push(&mut p, cid, near);
    let img = render(&p, cid);
    assert_eq!(img.get(100, 50), [0.0, 1.0, 0.0, 1.0]);
    assert_eq!(img.get(5, 50)[0], 1.0);
}

#[test]
fn coplanar_layers_keep_stack_order() {
    let (mut p, cid, comp) = setup();
    let top = solid3(&mut p, &comp, [1.0, 0.0, 0.0], 40, 40, [100.0, 50.0, 30.0]);
    let bottom = solid3(&mut p, &comp, [0.0, 0.0, 1.0], 80, 80, [100.0, 50.0, 30.0]);
    push(&mut p, cid, top);
    push(&mut p, cid, bottom);
    let img = render(&p, cid);
    assert_eq!(img.get(100, 50), [1.0, 0.0, 0.0, 1.0]);
}

#[test]
fn intersecting_planes_split_per_pixel() {
    let (mut p, cid, comp) = setup();
    let flat = solid3(&mut p, &comp, [1.0, 0.0, 0.0], 160, 80, [100.0, 50.0, 0.0]);
    let mut tilted = solid3(&mut p, &comp, [0.0, 1.0, 0.0], 160, 80, [100.0, 50.0, 0.0]);
    // Rotated about Y: its right half comes towards the camera, its left half goes behind.
    set(&mut tilted, "transform/rotationY", Value::Scalar(40.0));
    push(&mut p, cid, tilted);
    push(&mut p, cid, flat);
    let img = render(&p, cid);
    assert_eq!(img.get(125, 50), [0.0, 1.0, 0.0, 1.0], "right: tilted plane in front");
    assert_eq!(img.get(75, 50), [1.0, 0.0, 0.0, 1.0], "left: flat plane in front");
}

#[test]
fn two_d_layer_breaks_3d_group() {
    let (mut p, cid, comp) = setup();
    let far_top = solid3(&mut p, &comp, [1.0, 0.0, 0.0], 60, 60, [100.0, 50.0, 300.0]);
    let sid = p.add_item("S2", Label::Red, None, ItemKind::Solid(Solid { color: [0.0, 0.0, 1.0], width: 20, height: 100, pixel_aspect: 1.0 }));
    let mut two_d = build::layer(&mut p, &comp, "2D", LayerSource::Solid { item: sid }, (20, 100), None);
    set(&mut two_d, "transform/position", Value::Vec3([100.0, 50.0, 0.0]));
    let near_bottom = solid3(&mut p, &comp, [0.0, 1.0, 0.0], 200, 100, [100.0, 50.0, 0.0]);
    // Stack: far 3D (top), 2D, near 3D (bottom). The far layer is a separate group above the
    // 2D layer, so it draws over everything even though it is farther away.
    push(&mut p, cid, far_top);
    push(&mut p, cid, two_d);
    push(&mut p, cid, near_bottom);
    let img = render(&p, cid);
    assert_eq!(img.get(100, 50), [1.0, 0.0, 0.0, 1.0]);
    assert_eq!(img.get(5, 5), [0.0, 1.0, 0.0, 1.0]);
}

// ---------------------------------------------------------------- lighting

fn light(p: &mut Project, comp: &Comp, kind: LightKind, pos: [f64; 3]) -> Layer {
    let mut l = build::layer(p, comp, "Light", LayerSource::Light { kind }, (W, H), None);
    if kind != LightKind::Ambient {
        set(&mut l, "transform/position", Value::Vec3(pos));
    }
    l
}

fn no_specular(l: &mut Layer) {
    set(l, "materialOptions/specularIntensity", Value::Scalar(0.0));
}

#[test]
fn head_on_point_light_scales_by_diffuse() {
    let (mut p, cid, comp) = setup();
    let mut s = solid3(&mut p, &comp, [1.0, 0.8, 0.6], 200, 100, [100.0, 50.0, 0.0]);
    no_specular(&mut s);
    let l = light(&mut p, &comp, LightKind::Point, [100.0, 50.0, -500.0]);
    push(&mut p, cid, l);
    push(&mut p, cid, s);
    let img = render(&p, cid);
    let c = img.get(100, 50);
    // Diffuse 50% · N·L = 1 · intensity 100%.
    assert!(close(c[0], 0.5, 1e-3) && close(c[1], 0.4, 1e-3) && close(c[2], 0.3, 1e-3), "{c:?}");
    // Off-axis pixels are lit at a grazing angle → darker.
    assert!(img.get(2, 2)[0] < c[0]);
}

#[test]
fn ambient_light_uses_ambient_coefficient() {
    let (mut p, cid, comp) = setup();
    let mut s = solid3(&mut p, &comp, [0.8, 0.8, 0.8], 200, 100, [100.0, 50.0, 0.0]);
    set(&mut s, "materialOptions/ambient", Value::Scalar(50.0));
    let mut l = light(&mut p, &comp, LightKind::Ambient, [0.0; 3]);
    set(&mut l, "lightOptions/intensity", Value::Scalar(50.0));
    push(&mut p, cid, l);
    push(&mut p, cid, s);
    let c = render(&p, cid).get(10, 10);
    assert!(close(c[0], 0.8 * 0.5 * 0.5, 1e-4), "{c:?}");
}

#[test]
fn lights_off_or_not_accepting_lights_render_unlit() {
    let (mut p, cid, comp) = setup();
    let mut s = solid3(&mut p, &comp, [0.3, 0.6, 0.9], 200, 100, [100.0, 50.0, 0.0]);
    set(&mut s, "materialOptions/acceptsLights", Value::Bool(false));
    let l = light(&mut p, &comp, LightKind::Point, [100.0, 50.0, -500.0]);
    push(&mut p, cid, l);
    push(&mut p, cid, s);
    let c = render(&p, cid).get(100, 50);
    assert!(close(c[0], 0.3, 1e-5) && close(c[2], 0.9, 1e-5), "{c:?}");
}

#[test]
fn light_behind_layer_needs_transmission() {
    let (mut p, cid, comp) = setup();
    let mut s = solid3(&mut p, &comp, [1.0, 1.0, 1.0], 200, 100, [100.0, 50.0, 0.0]);
    no_specular(&mut s);
    let l = light(&mut p, &comp, LightKind::Point, [100.0, 50.0, 500.0]);
    push(&mut p, cid, l);
    push(&mut p, cid, s.clone());
    assert!(render(&p, cid).get(100, 50)[0] < 1e-4);
    // 100% transmission passes the light through the layer.
    let c = p.comp_mut(cid).unwrap();
    set(&mut c.layers[1], "materialOptions/lightTransmission", Value::Scalar(100.0));
    let v = render(&p, cid).get(100, 50)[0];
    assert!(close(v, 0.5, 1e-3), "{v}");
}

#[test]
fn specular_highlight_adds_light() {
    let (mut p, cid, comp) = setup();
    let s = solid3(&mut p, &comp, [0.5, 0.5, 0.5], 200, 100, [100.0, 50.0, 0.0]);
    let l = light(&mut p, &comp, LightKind::Point, [100.0, 50.0, -zoom()]);
    push(&mut p, cid, l);
    push(&mut p, cid, s);
    let c = render(&p, cid).get(100, 50);
    // 0.5·0.5 diffuse + 0.5 specular (N·H = 1) tinted by the layer colour (Metal 100%) = 0.5.
    assert!(close(c[0], 0.25 + 0.25, 2e-3), "{c:?}");
}

#[test]
fn spot_cone_limits_light() {
    let (mut p, cid, comp) = setup();
    let mut s = solid3(&mut p, &comp, [1.0, 1.0, 1.0], 200, 100, [100.0, 50.0, 0.0]);
    no_specular(&mut s);
    let mut l = light(&mut p, &comp, LightKind::Spot, [100.0, 50.0, -200.0]);
    set(&mut l, "transform/poi", Value::Vec3([100.0, 50.0, 0.0]));
    set(&mut l, "lightOptions/coneAngle", Value::Scalar(20.0));
    set(&mut l, "lightOptions/coneFeather", Value::Scalar(0.0));
    push(&mut p, cid, l);
    push(&mut p, cid, s);
    let img = render(&p, cid);
    assert!(img.get(100, 50)[0] > 0.45);
    // tan(10°)·200 ≈ 35 px radius.
    assert!(img.get(160, 50)[0] < 1e-4);
}

#[test]
fn falloff_inverse_square_clamped() {
    let mut ls = LightState::from_layer(&crate::EvalCtx::new(&Project::default(), ItemId(0), &Comp::new(W, H, FrameRate::FPS_30, Tick::ZERO), Tick::ZERO), &{
        let (mut p, _, comp) = setup();
        light(&mut p, &comp, LightKind::Point, [0.0, 0.0, 0.0])
    })
    .unwrap();
    ls.falloff = 2;
    ls.radius = 100.0;
    assert!((ls.attenuation(vec3(50.0, 0.0, 0.0)) - 1.0).abs() < 1e-12);
    assert!((ls.attenuation(vec3(200.0, 0.0, 0.0)) - 0.25).abs() < 1e-12);
    ls.falloff = 1;
    ls.falloff_distance = 100.0;
    assert!((ls.attenuation(vec3(150.0, 0.0, 0.0)) - 0.5).abs() < 1e-12);
    assert_eq!(ls.attenuation(vec3(250.0, 0.0, 0.0)), 0.0);
}

// ---------------------------------------------------------------- shadows

#[test]
fn caster_shadows_receiver() {
    let (mut p, cid, comp) = setup();
    let mut recv = solid3(&mut p, &comp, [1.0, 1.0, 1.0], 200, 100, [100.0, 50.0, 100.0]);
    no_specular(&mut recv);
    let mut caster = solid3(&mut p, &comp, [1.0, 1.0, 1.0], 20, 20, [60.0, 50.0, 0.0]);
    no_specular(&mut caster);
    set(&mut caster, "materialOptions/castsShadows", Value::Enum(1));
    // Parallel light straight along +z: the shadow lands right behind the caster.
    let mut l = light(&mut p, &comp, LightKind::Parallel, [100.0, 50.0, -500.0]);
    set(&mut l, "transform/poi", Value::Vec3([100.0, 50.0, 0.0]));
    set(&mut l, "lightOptions/castsShadows", Value::Bool(true));
    push(&mut p, cid, l);
    push(&mut p, cid, caster);
    push(&mut p, cid, recv);
    let img = render(&p, cid);
    // Lit receiver away from the shadow: 0.5.
    assert!(close(img.get(150, 50)[0], 0.5, 1e-3));
    // Light from the left (direction (300, 0, 500)): the shadow shifts +60 px at the receiver's
    // depth, to world x 110..130 → screen ≈ 107..122.
    let c = p.comp_mut(cid).unwrap();
    set(&mut c.layers[0], "transform/position", Value::Vec3([100.0 - 300.0, 50.0, -500.0]));
    let img = render(&p, cid);
    let lit = img.get(150, 50)[0];
    let expect = 0.5 * (500.0 / (300.0f64 * 300.0 + 500.0 * 500.0).sqrt()) as f32;
    assert!(close(lit, expect, 1e-3), "lit {lit} vs {expect}");
    assert!(img.get(115, 50)[0] < 1e-3, "shadowed {:?}", img.get(115, 50));
    assert!(close(img.get(60, 50)[0], expect, 1e-3), "caster itself is lit");
}

#[test]
fn shadow_darkness_and_accepts_off() {
    let (mut p, cid, comp) = setup();
    let mut recv = solid3(&mut p, &comp, [1.0, 1.0, 1.0], 400, 200, [100.0, 50.0, 100.0]);
    no_specular(&mut recv);
    let mut caster = solid3(&mut p, &comp, [1.0, 1.0, 1.0], 400, 200, [100.0, 50.0, 0.0]);
    set(&mut caster, "materialOptions/castsShadows", Value::Enum(2)); // Only: invisible caster
    let mut l = light(&mut p, &comp, LightKind::Parallel, [100.0, 50.0, -500.0]);
    set(&mut l, "transform/poi", Value::Vec3([100.0, 50.0, 0.0]));
    set(&mut l, "lightOptions/castsShadows", Value::Bool(true));
    set(&mut l, "lightOptions/shadowDarkness", Value::Scalar(50.0));
    push(&mut p, cid, l);
    push(&mut p, cid, caster);
    push(&mut p, cid, recv);
    let v = render(&p, cid).get(100, 50)[0];
    assert!(close(v, 0.25, 1e-3), "half-dark shadow: {v}");
    let c = p.comp_mut(cid).unwrap();
    set(&mut c.layers[2], "materialOptions/acceptsShadows", Value::Enum(0));
    let v = render(&p, cid).get(100, 50)[0];
    assert!(close(v, 0.5, 1e-3), "no shadow accepted: {v}");
}

#[test]
fn shadow_diffusion_is_a_soft_ramp_not_copies() {
    // #2444: a wide Shadow Diffusion stamped 3 × 3 copies of the caster, so the penumbra stepped
    // through three flat levels instead of ramping smoothly from lit to shadowed.
    let (mut p, cid, comp) = setup();
    let mut recv = solid3(&mut p, &comp, [1.0, 1.0, 1.0], 400, 200, [100.0, 50.0, 100.0]);
    no_specular(&mut recv);
    let mut caster = solid3(&mut p, &comp, [1.0, 1.0, 1.0], 100, 200, [40.0, 50.0, 0.0]);
    set(&mut caster, "materialOptions/castsShadows", Value::Enum(2)); // Only: invisible caster
    let mut l = light(&mut p, &comp, LightKind::Parallel, [100.0, 50.0, -500.0]);
    set(&mut l, "transform/poi", Value::Vec3([100.0, 50.0, 0.0]));
    set(&mut l, "lightOptions/castsShadows", Value::Bool(true));
    // 100 px to the caster over the parallel light's 1000: a 15 px penumbra radius.
    set(&mut l, "lightOptions/shadowDiffusion", Value::Scalar(150.0));
    push(&mut p, cid, l);
    push(&mut p, cid, caster);
    push(&mut p, cid, recv);
    let img = render(&p, cid);
    let row: Vec<f32> = (0..i64::from(W)).map(|x| img.get(x, 50)[0]).collect();
    // The row crosses both vertical edges of the caster's shadow: lit, ramp, shadowed, ramp, lit.
    assert!(row.iter().any(|v| *v < 0.01) && close(row[W as usize - 1], 0.5, 1e-3), "shadow and light: {row:?}");
    let ramp = row.iter().filter(|v| **v > 0.02 && **v < 0.48).count();
    assert!(ramp >= 16, "two penumbras several pixels wide: {row:?}");
    let step = row.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
    assert!(step < 0.08, "no jumps between copies of the caster (largest step {step}): {row:?}");
}

// ---------------------------------------------------------------- cameras

fn camera(p: &mut Project, comp: &Comp) -> Layer {
    build::layer(p, comp, "Camera", LayerSource::Camera, (comp.width, comp.height), None)
}

#[test]
fn default_camera_layer_matches_comp_camera() {
    let (mut p, cid, comp) = setup();
    let cam = camera(&mut p, &comp);
    assert_eq!(cam.props.prop("cameraOptions/zoom").unwrap().value.as_f64(), zoom());
    let s = solid3(&mut p, &comp, [1.0, 0.0, 0.0], 40, 20, [50.0, 30.0, 0.0]);
    push(&mut p, cid, s.clone());
    let a = render(&p, cid);
    p.comp_mut(cid).unwrap().layers.insert(0, cam);
    let b = render(&p, cid);
    assert_eq!(a, b);
}

#[test]
fn two_node_camera_looks_at_poi() {
    let (mut p, cid, comp) = setup();
    let mut cam = camera(&mut p, &comp);
    // Look at a layer far to the right: it lands in the middle of the frame.
    set(&mut cam, "transform/poi", Value::Vec3([600.0, 50.0, 0.0]));
    set(&mut cam, "transform/position", Value::Vec3([600.0, 50.0, -zoom()]));
    let s = solid3(&mut p, &comp, [0.0, 0.0, 1.0], 20, 20, [600.0, 50.0, 0.0]);
    push(&mut p, cid, cam);
    push(&mut p, cid, s);
    let img = render(&p, cid);
    assert!(img.get(100, 50)[2] > 0.99);
    assert!(img.get(80, 50)[3] < 0.01);
}

#[test]
fn one_node_camera_uses_orientation() {
    let (mut p, cid, comp) = setup();
    let mut cam = camera(&mut p, &comp);
    cam.auto_orient = effectcraft_project::AutoOrient::Off;
    set(&mut cam, "transform/position", Value::Vec3([100.0, 50.0, 0.0]));
    // Turn to look along +x.
    set(&mut cam, "transform/orientation", Value::Vec3(orientation_for(vec3(1.0, 0.0, 0.0))));
    let mut s = solid3(&mut p, &comp, [1.0, 1.0, 0.0], 40, 40, [400.0, 50.0, 0.0]);
    set(&mut s, "transform/rotationY", Value::Scalar(90.0));
    push(&mut p, cid, cam);
    push(&mut p, cid, s);
    let img = render(&p, cid);
    assert!(img.get(100, 50)[3] > 0.99, "{:?}", img.get(100, 50));
}

#[test]
fn orientation_for_round_trips() {
    for d in [vec3(1.0, 0.0, 0.0), vec3(0.0, 0.0, 1.0), vec3(-0.3, 0.4, 0.8), vec3(0.2, -0.9, -0.1)] {
        let o = orientation_for(d);
        let f = effectcraft_geom::Mat4::orientation(Vec3::from(o)).apply_vec(vec3(0.0, 0.0, 1.0));
        assert!((f - d.normalize()).length() < 1e-9, "{d:?} → {o:?} → {f:?}");
    }
}

#[test]
fn depth_of_field_blurs_out_of_focus_layers() {
    let (mut p, cid, comp) = setup();
    let mut cam = camera(&mut p, &comp);
    set(&mut cam, "cameraOptions/dof", Value::Bool(true));
    set(&mut cam, "cameraOptions/aperture", Value::Scalar(60.0));
    let s = solid3(&mut p, &comp, [1.0, 1.0, 1.0], 60, 60, [100.0, 50.0, 0.0]);
    push(&mut p, cid, cam);
    push(&mut p, cid, s);
    // In focus (focus distance = zoom = distance to z=0): hard edge at x = 70.
    let a = render(&p, cid);
    assert!(a.get(71, 50)[3] > 0.99 && a.get(69, 50)[3] < 0.01);
    let c = p.comp_mut(cid).unwrap();
    set(&mut c.layers[0], "cameraOptions/focusDistance", Value::Scalar(zoom() * 0.5));
    let b = render(&p, cid);
    let e = b.get(70, 50)[3];
    assert!(e > 0.1 && e < 0.9, "soft edge {e}");
    assert!(b.get(66, 50)[3] > 0.01);
}

/// A non-finite Focus Distance doesn't reach the blur: the camera focuses on the property's
/// default plane (`zoom`) instead.
#[test]
fn dof_falls_back_when_focus_distance_is_not_finite() {
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let (mut p, cid, comp) = setup();
        let mut cam = camera(&mut p, &comp);
        set(&mut cam, "cameraOptions/dof", Value::Bool(true));
        set(&mut cam, "cameraOptions/aperture", Value::Scalar(60.0));
        set(&mut cam, "cameraOptions/focusDistance", Value::Scalar(bad));
        let s = solid3(&mut p, &comp, [1.0, 1.0, 1.0], 60, 60, [100.0, 50.0, 0.0]);
        push(&mut p, cid, cam);
        push(&mut p, cid, s);
        let cp = p.comp(cid).unwrap();
        let ctx = crate::EvalCtx::new(&p, cid, cp, Tick::ZERO);
        let cs = crate::three_d::camera::layer_camera(&ctx, &cp.layers[0]);
        let f = cs.dof.unwrap().focus;
        assert!((f - zoom()).abs() < 1e-9, "{bad}: evaluated to {f}, expected the fallback to zoom {}", zoom());
        // The z = 0 plane is in focus: hard edge, finite pixels.
        let img = render(&p, cid);
        assert!(img.get(71, 50)[3] > 0.99 && img.get(69, 50)[3] < 0.01, "{bad}: the edge should be hard");
        assert!(img.data.iter().all(|px| px.iter().all(|c| c.is_finite())), "{bad}: non-finite pixels");
    }
}

/// The Classic 3D blur stays bounded for degenerate inputs: a 0%-scaled 3D layer off the focal
/// plane (e.g. the first frame of a scale-in) or a huge but finite Focus Distance asks for a
/// radius of billions of buffer pixels, whose padded buffer can't be allocated.
#[test]
fn dof_blur_radius_is_bounded_for_degenerate_inputs() {
    let scene = |focus: f64, scale: f64| {
        let (mut p, cid, comp) = setup();
        let mut cam = camera(&mut p, &comp);
        set(&mut cam, "cameraOptions/dof", Value::Bool(true));
        set(&mut cam, "cameraOptions/aperture", Value::Scalar(60.0));
        set(&mut cam, "cameraOptions/focusDistance", Value::Scalar(focus));
        let mut s = solid3(&mut p, &comp, [1.0, 1.0, 1.0], 60, 60, [100.0, 50.0, 0.0]);
        set(&mut s, "transform/scale", Value::Vec3([scale, scale, 100.0]));
        push(&mut p, cid, cam);
        push(&mut p, cid, s);
        render(&p, cid)
    };
    let finite = |img: &effectcraft_raster::Image| img.data.iter().all(|px| px.iter().all(|c| c.is_finite()));
    // A 0%-scaled layer 10 px off the focal plane: an ordinary defocus times 1/sx (sx = 1e-9).
    assert!(finite(&scene(zoom() + 10.0, 0.0)), "zero-scaled: non-finite pixels");
    // Huge but finite Focus Distances (typed in, or a large expression result): the blur is as
    // large as the buffer budget allows, never refused (which would render the layer sharp).
    // 1e307 overflows the circle of confusion itself to infinity: still the largest blur.
    for focus in [1.0e12, 1.0e307] {
        let img = scene(focus, 100.0);
        assert!(finite(&img), "focus {focus:e}: non-finite pixels");
        assert!(img.get(100, 50)[3] < 0.5, "focus {focus:e}: the layer should be spread out, not sharp ({})", img.get(100, 50)[3]);
    }
}

/// The blur radius map of a plane: a 0%-scaled plane is degenerate on screen and isn't blurred;
/// any other radius stays within the buffer budget's cap.
#[test]
fn dof_radii_skip_degenerate_planes_and_respect_the_cap() {
    let radii = |focus: f64, scale: f64| {
        let (mut p, cid, comp) = setup();
        let mut cam = camera(&mut p, &comp);
        set(&mut cam, "cameraOptions/dof", Value::Bool(true));
        set(&mut cam, "cameraOptions/aperture", Value::Scalar(60.0));
        set(&mut cam, "cameraOptions/focusDistance", Value::Scalar(focus));
        let mut s = solid3(&mut p, &comp, [1.0, 1.0, 1.0], 60, 60, [100.0, 50.0, 0.0]);
        set(&mut s, "transform/scale", Value::Vec3([scale, scale, 100.0]));
        push(&mut p, cid, cam);
        push(&mut p, cid, s);
        let cp = p.comp(cid).unwrap();
        let ctx = crate::EvalCtx::new(&p, cid, cp, Tick::ZERO);
        let cs = super::camera::layer_camera(&ctx, &cp.layers[0]);
        let dof = cs.dof.unwrap();
        let cap = super::compose::dof_radius_cap(&dof, 60, 60);
        (super::compose::dof_radii_at(&cs, &dof, ctx.world_matrix(&cp.layers[1]), (60, 60), [0.0; 2], 1.0, cap), cap)
    };
    let (r, _) = radii(zoom() + 10.0, 0.0);
    assert!(r.iter().all(|&r| r == 0.0), "a 0%-scaled plane is not blurred");
    let (r, cap) = radii(1.0e12, 100.0);
    assert!(cap > 1000.0, "a 60×60 buffer may blur by well over 1000 px: {cap}");
    assert!(r.iter().all(|&r| r as f64 <= cap && r as f64 > cap - 1.0), "huge focus: every radius at the cap {cap}");
}

/// Two layers that look the same on screen (400×100 at 100%, 1600×400 at 25%) at the same depth
/// blur by the same amount on screen: the radius bound must not depend on the layer's scale.
#[test]
fn dof_blur_is_independent_of_layer_scale() {
    let ramp = |w: u32, h: u32, scale: f64| {
        let (mut p, cid, comp) = setup_sized(800, 400);
        let zoom = 800.0 * 50.0 / 36.0;
        let mut cam = camera(&mut p, &comp);
        set(&mut cam, "cameraOptions/dof", Value::Bool(true));
        set(&mut cam, "cameraOptions/aperture", Value::Scalar(200.0));
        // coc = aperture·|z − f| / z: a 40 px comp radius on the z = 0 plane (depth = zoom).
        set(&mut cam, "cameraOptions/focusDistance", Value::Scalar(zoom + 2.0 * 40.0 * zoom / 200.0));
        let mut s = solid3(&mut p, &comp, [1.0, 1.0, 1.0], w, h, [400.0, 200.0, 0.0]);
        set(&mut s, "transform/scale", Value::Vec3([scale, scale, 100.0]));
        push(&mut p, cid, cam);
        push(&mut p, cid, s);
        let img = render(&p, cid);
        soft_count(&img, (0..400).map(|x| (x, 200)))
    };
    let (full, quarter) = (ramp(400, 100, 100.0), ramp(1600, 400, 25.0));
    assert!(full > 60, "the out-of-focus edge should be soft: {full}");
    assert!(full.abs_diff(quarter) <= 2, "same on-screen layer, same depth: {full} px at 100% vs {quarter} px at 25%");
}

/// The same frame at Full and Half resolution shows the same blur (in comp pixels). The radius
/// (~100 px) is large so that any bound that depends on the resolution would show.
#[test]
fn dof_blur_matches_across_render_resolutions() {
    let (mut p, cid, comp) = setup_sized(800, 400);
    let zoom = 800.0 * 50.0 / 36.0;
    let mut cam = camera(&mut p, &comp);
    set(&mut cam, "cameraOptions/dof", Value::Bool(true));
    set(&mut cam, "cameraOptions/aperture", Value::Scalar(200.0));
    set(&mut cam, "cameraOptions/focusDistance", Value::Scalar(2.0 * zoom));
    let s = solid3(&mut p, &comp, [1.0, 1.0, 1.0], 400, 100, [400.0, 200.0, 0.0]);
    push(&mut p, cid, cam);
    push(&mut p, cid, s);
    let ramp = |scale: f64| {
        let img = render_frame(&p, cid, Tick::ZERO, scale);
        let (w, h) = (img.width as i64, img.height as i64);
        soft_count(&img, (0..w / 2).map(|x| (x, h / 2))) as f64 / scale
    };
    let (full, half) = (ramp(1.0), ramp(0.5));
    assert!(full > 150.0, "a ~100 px radius must not be clamped: {full} comp px");
    assert!((full - half).abs() <= 4.0, "Full {full} vs Half {half} comp px");
}

/// `Dof::coc` treats a non-finite focus or depth as in focus.
#[test]
fn dof_coc_is_zero_for_non_finite_inputs() {
    let d = |focus: f64| super::camera::Dof { focus, aperture: 50.0, blur_level: 1.0, iris: Default::default(), highlight: Default::default() };
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert_eq!(d(bad).coc(500.0), 0.0, "focus {bad}");
        assert_eq!(d(400.0).coc(bad), 0.0, "depth {bad}");
    }
    assert!(d(400.0).coc(500.0) > 0.0);
}

/// Pixels among `pts` whose alpha is strictly between 0.05 and 0.95.
fn soft_count(img: &effectcraft_raster::Image, pts: impl Iterator<Item = (i64, i64)>) -> usize {
    pts.filter(|&(x, y)| (0.05..0.95).contains(&img.get(x, y)[3])).count()
}

/// Pixels in column `x` whose alpha is strictly between 0.05 and 0.95.
fn soft_px(img: &effectcraft_raster::Image, x: i64) -> usize {
    soft_count(img, (0..img.height as i64).map(|y| (x, y)))
}

#[test]
fn tilted_layer_blurs_progressively_with_depth() {
    let (mut p, cid, comp) = setup();
    let mut cam = camera(&mut p, &comp);
    set(&mut cam, "cameraOptions/dof", Value::Bool(true));
    set(&mut cam, "cameraOptions/aperture", Value::Scalar(80.0));
    set(&mut cam, "cameraOptions/irisShape", Value::Enum(4)); // Hexagon
    let mut s = solid3(&mut p, &comp, [1.0, 1.0, 1.0], 160, 40, [100.0, 50.0, 0.0]);
    set(&mut s, "transform/rotationY", Value::Scalar(60.0));
    push(&mut p, cid, cam);
    push(&mut p, cid, s);
    let img = render(&p, cid);
    // The centre column sits on the focal plane: a hard top edge. The ends are 70 px in front of
    // and behind it: soft.
    let xs: Vec<i64> = (0..W as i64).filter(|&x| img.get(x, 50)[3] > 0.5).collect();
    let (x0, x1) = (xs[0], *xs.last().unwrap());
    let at = |f: f64| x0 + ((x1 - x0) as f64 * f) as i64;
    let (centre, left, right) = (soft_px(&img, 100), soft_px(&img, at(0.1)), soft_px(&img, at(0.9)));
    assert!(centre <= 2, "centre {centre}");
    assert!(left >= 4 && right >= 4, "ends {left} {right}");
}

#[test]
fn iris_shape_and_highlights_shape_the_bokeh() {
    // A small bright square far out of focus: its blur takes the iris' shape.
    let spot = |shape: u32, gain: f64| {
        let (mut p, cid, comp) = setup();
        let mut cam = camera(&mut p, &comp);
        set(&mut cam, "cameraOptions/dof", Value::Bool(true));
        set(&mut cam, "cameraOptions/aperture", Value::Scalar(120.0));
        set(&mut cam, "cameraOptions/focusDistance", Value::Scalar(zoom() * 0.5));
        set(&mut cam, "cameraOptions/irisShape", Value::Enum(shape));
        set(&mut cam, "cameraOptions/highlightGain", Value::Scalar(gain));
        set(&mut cam, "cameraOptions/highlightThreshold", Value::Scalar(200.0));
        let s = solid3(&mut p, &comp, [1.0, 1.0, 1.0], 2, 2, [100.0, 50.0, 0.0]);
        push(&mut p, cid, cam);
        push(&mut p, cid, s);
        render(&p, cid)
    };
    let area = |img: &effectcraft_raster::Image| img.data.iter().filter(|q| q[3] > 1e-4).count();
    let (tri, dec, rect) = (spot(1, 0.0), spot(8, 0.0), spot(0, 0.0));
    assert!(area(&tri) * 3 < area(&dec) * 2, "triangle {} vs decagon {}", area(&tri), area(&dec));
    // Fast Rectangle: a box.
    let (x0, x1) = (0..W as i64).filter(|&x| rect.get(x, 50)[3] > 1e-4).fold((i64::MAX, 0), |(a, b), x| (a.min(x), b.max(x)));
    let y_extent = (0..H as i64).filter(|&y| rect.get(100, y)[3] > 1e-4).count() as i64;
    assert!(((x1 - x0 + 1) - y_extent).abs() <= 2, "square box {} vs {}", x1 - x0 + 1, y_extent);
    // Highlight Gain brightens the blurred highlight.
    let lit = spot(8, 100.0);
    let sum = |img: &effectcraft_raster::Image| img.data.iter().map(|q| q[0]).sum::<f32>();
    assert!(sum(&lit) > sum(&dec) * 1.5, "{} vs {}", sum(&lit), sum(&dec));
}

// ---------------------------------------------------------------- views

#[test]
fn view_ids_round_trip() {
    for v in View3D::ALL {
        assert_eq!(View3D::from_id(v.id()), Some(v));
        assert_eq!(View3D::from_id(v.label()), Some(v));
    }
    assert_eq!(View3D::from_id("Custom View 2"), Some(View3D::Custom2));
}

#[test]
fn front_view_is_orthographic() {
    let (mut p, cid, comp) = setup();
    // In an orthographic view depth doesn't change size.
    let a = solid3(&mut p, &comp, [1.0, 0.0, 0.0], 40, 40, [60.0, 50.0, 0.0]);
    let b = solid3(&mut p, &comp, [0.0, 1.0, 0.0], 40, 40, [140.0, 50.0, 1000.0]);
    push(&mut p, cid, a);
    push(&mut p, cid, b);
    let mut vc = default_view_cam(View3D::Front, W as f64, H as f64);
    vc.zoom = 1.0;
    let opts = RenderOpts { view: Some(vc.state()), ..Default::default() };
    let img = Renderer::new(&p, &NoFootage, opts).comp_frame(cid, Tick::ZERO);
    assert!(img.get(41, 50)[0] > 0.99 && img.get(39, 50)[3] < 0.01);
    assert!(img.get(121, 50)[1] > 0.99 && img.get(159, 50)[1] > 0.99 && img.get(161, 50)[3] < 0.01);
}

#[test]
fn top_view_sees_planes_edge_on() {
    let (mut p, cid, comp) = setup();
    let a = solid3(&mut p, &comp, [1.0, 0.0, 0.0], 100, 100, [100.0, 50.0, 0.0]);
    let mut b = solid3(&mut p, &comp, [0.0, 1.0, 0.0], 100, 100, [100.0, 50.0, 0.0]);
    set(&mut b, "transform/rotationX", Value::Scalar(90.0));
    push(&mut p, cid, a);
    push(&mut p, cid, b);
    let mut vc = default_view_cam(View3D::Top, W as f64, H as f64);
    vc.zoom = 1.0;
    let opts = RenderOpts { view: Some(vc.state()), ..Default::default() };
    let img = Renderer::new(&p, &NoFootage, opts).comp_frame(cid, Tick::ZERO);
    // From the top, the X-rotated plane is a 100×100 square; the upright one is a line.
    assert!(img.get(100, 30)[1] > 0.99 && img.get(100, 70)[1] > 0.99);
    assert!(img.get(100, 30)[0] < 0.01);
}

#[test]
fn rig_orbit_keeps_distance_and_pan_moves_both() {
    let vc = default_view_cam(View3D::Custom1, 1920.0, 1080.0);
    let mut r = Rig::from_view(&vc);
    let d0 = (r.eye - r.poi).length();
    r.orbit(37.0, -15.0);
    assert!(((r.eye - r.poi).length() - d0).abs() < 1e-6);
    let (e, p) = (r.eye, r.poi);
    r.pan(10.0, 0.0);
    assert!(((r.eye - e) - (r.poi - p)).length() < 1e-9);
    r.dolly(100.0, false);
    assert!(((r.eye - r.poi).length() - (d0 - 100.0)).abs() < 1e-6);
    // Pitch is clamped short of the poles.
    r.orbit(0.0, 500.0);
    let off = r.eye - r.poi;
    assert!(off.y.abs() / off.length() < 0.9999);
}
