//! Auto-orient (along path / towards camera) and camera parenting.

use effectcraft_color::Label;
use effectcraft_geom::vec3;
use effectcraft_keyframe::{Keyframe, Value};
use effectcraft_project::build;
use effectcraft_project::{AutoOrient, Comp, ItemId, ItemKind, Layer, LayerSource, Project, Solid};
use effectcraft_time::{FrameRate, Tick};

use super::active_camera;
use crate::EvalCtx;

fn setup() -> (Project, ItemId, Comp) {
    let mut p = Project::default();
    let comp = Comp::new(200, 100, FrameRate::FPS_30, Tick::from_seconds_f64(2.0));
    let cid = p.add_item("Comp", Label::Sandstone, None, ItemKind::Comp(comp.clone().into()));
    (p, cid, comp)
}

fn solid(p: &mut Project, comp: &Comp, three_d: bool, pos: [f64; 3]) -> Layer {
    let sid = p.add_item("S", Label::Red, None, ItemKind::Solid(Solid { color: [1.0; 3], width: 10, height: 10, pixel_aspect: 1.0 }));
    let mut l = build::layer(p, comp, "S", LayerSource::Solid { item: sid }, (10, 10), None);
    l.switches.three_d = three_d;
    l.props.prop_mut("transform/position").unwrap().value = Value::Vec3(pos);
    l
}

#[test]
fn orient_towards_camera_faces_the_eye() {
    let (mut p, cid, comp) = setup();
    let mut l = solid(&mut p, &comp, true, [600.0, -200.0, 300.0]);
    l.auto_orient = AutoOrient::TowardsCamera;
    p.comp_mut(cid).unwrap().layers.push(l);
    let c = p.comp(cid).unwrap();
    let ctx = EvalCtx::new(&p, cid, c, Tick::ZERO);
    let w = ctx.world_matrix(&c.layers[0]);
    let n = w.apply_vec(vec3(0.0, 0.0, 1.0)).normalize();
    let eye = active_camera(&ctx).eye;
    let to_layer = (w.apply(vec3(5.0, 5.0, 0.0)) - eye).normalize();
    assert!((n - to_layer).length() < 1e-6, "{n:?} vs {to_layer:?}");
}

#[test]
fn orient_along_path_2d_follows_motion() {
    let (mut p, cid, comp) = setup();
    let mut l = solid(&mut p, &comp, false, [50.0, 10.0, 0.0]);
    l.auto_orient = AutoOrient::AlongPath;
    l.props.prop_mut("transform/position").unwrap().keys =
        vec![Keyframe::new(Tick::ZERO, Value::Vec3([50.0, 10.0, 0.0])), Keyframe::new(Tick::from_seconds_f64(1.0), Value::Vec3([50.0, 90.0, 0.0]))];
    p.comp_mut(cid).unwrap().layers.push(l);
    let c = p.comp(cid).unwrap();
    let ctx = EvalCtx::new(&p, cid, c, Tick::from_seconds_f64(0.5));
    let x_axis = ctx.world_matrix(&c.layers[0]).apply_vec(vec3(1.0, 0.0, 0.0));
    // Moving straight down (+y): the layer's x axis points down the screen.
    assert!(x_axis.x.abs() < 1e-6 && (x_axis.y - 1.0).abs() < 1e-6, "{x_axis:?}");
}

/// #1081: a layer travelling left kept its heading until the last keyframe, then (no motion left
/// to measure) snapped back to 0 degrees and looked flipped. It must keep its last direction of
/// travel at the final key and after it, in 2D and 3D.
#[test]
fn orient_along_path_keeps_the_last_heading_at_and_after_the_end() {
    for three_d in [false, true] {
        let (mut p, cid, comp) = setup();
        let mut l = solid(&mut p, &comp, three_d, [150.0, 50.0, 0.0]);
        l.auto_orient = AutoOrient::AlongPath;
        let end = Tick::from_seconds_f64(1.0);
        l.props.prop_mut("transform/position").unwrap().keys =
            vec![Keyframe::new(Tick::ZERO, Value::Vec3([150.0, 50.0, 0.0])), Keyframe::new(end, Value::Vec3([50.0, 50.0, 0.0]))];
        p.comp_mut(cid).unwrap().layers.push(l);
        let c = p.comp(cid).unwrap();
        for secs in [0.5, 0.999, 1.0, 1.2, 1.9] {
            let ctx = EvalCtx::new(&p, cid, c, Tick::from_seconds_f64(secs));
            let x_axis = ctx.world_matrix(&c.layers[0]).apply_vec(vec3(1.0, 0.0, 0.0));
            // Moving left (-x): the layer's x axis keeps pointing left.
            assert!((x_axis.x + 1.0).abs() < 1e-6 && x_axis.y.abs() < 1e-6 && x_axis.z.abs() < 1e-6, "3d={three_d} t={secs}: {x_axis:?}");
        }
    }
}

#[test]
fn layers_parented_to_a_camera_follow_its_rotation() {
    let (mut p, cid, comp) = setup();
    let mut cam = build::layer(&mut p, &comp, "Camera", LayerSource::Camera, (200, 100), None);
    cam.auto_orient = AutoOrient::Off;
    cam.props.prop_mut("transform/position").unwrap().value = Value::Vec3([0.0, 0.0, 0.0]);
    cam.props.prop_mut("transform/orientation").unwrap().value = Value::Vec3([0.0, 90.0, 0.0]);
    let mut child = solid(&mut p, &comp, true, [0.0, 0.0, 100.0]);
    child.parent = Some(cam.id);
    let c = p.comp_mut(cid).unwrap();
    c.layers.push(cam);
    c.layers.push(child);
    let c = p.comp(cid).unwrap();
    let ctx = EvalCtx::new(&p, cid, c, Tick::ZERO);
    // 100 px in front of a camera turned to +x is at x = 100 (anchor at the solid's centre).
    let at = ctx.world_matrix(&c.layers[1]).apply(vec3(5.0, 5.0, 0.0));
    assert!((at - vec3(100.0, 0.0, 0.0)).length() < 1e-6, "{at:?}");
}

#[test]
fn camera_parented_to_towards_camera_layer_does_not_recurse() {
    let (mut p, cid, comp) = setup();
    let mut l = solid(&mut p, &comp, true, [100.0, 50.0, 0.0]);
    l.auto_orient = AutoOrient::TowardsCamera;
    let mut cam = build::layer(&mut p, &comp, "Camera", LayerSource::Camera, (200, 100), None);
    cam.parent = Some(l.id);
    let c = p.comp_mut(cid).unwrap();
    c.layers.push(cam);
    c.layers.push(l);
    let _ = crate::render_frame(&p, cid, Tick::ZERO, 0.5);
}

/// The layer cache holds layer pixels only; camera, lights, DOF and the view override act at
/// compositing time, so cached renders must match uncached ones after any of them change.
#[test]
fn layer_cache_is_independent_of_camera_lights_and_view() {
    let (mut p, cid, comp) = setup();
    let mut cam = build::layer(&mut p, &comp, "Camera", LayerSource::Camera, (200, 100), None);
    cam.props.prop_mut("cameraOptions/dof").unwrap().value = Value::Bool(true);
    cam.props.prop_mut("cameraOptions/aperture").unwrap().value = Value::Scalar(40.0);
    let light = build::layer(&mut p, &comp, "Light", LayerSource::Light { kind: effectcraft_project::LightKind::Point }, (200, 100), None);
    let a = solid(&mut p, &comp, true, [80.0, 50.0, 0.0]);
    let b = solid(&mut p, &comp, true, [120.0, 50.0, 150.0]);
    let c = p.comp_mut(cid).unwrap();
    c.layers.extend([cam, light, a, b]);
    let cache = crate::LayerCache::new(64 << 20);
    let render = |p: &Project, cached: bool, view: Option<super::CameraState>| {
        let opts = crate::RenderOpts { view, motion_blur: false, ..Default::default() };
        let mut r = crate::Renderer::new(p, &crate::NoFootage, opts);
        if cached {
            r.cache = Some(&cache);
        }
        r.comp_frame(cid, Tick::ZERO)
    };
    let check = |p: &Project, view: Option<super::CameraState>| {
        let warm = render(p, true, view);
        assert_eq!(render(p, true, view), warm);
        assert_eq!(render(p, false, view), warm, "cached render differs from uncached");
        warm
    };
    let first = check(&p, None);
    // Move the camera, refocus, change the light: cached pixels must follow.
    let c = p.comp_mut(cid).unwrap();
    c.layers[0].props.prop_mut("transform/position").unwrap().value = Value::Vec3([40.0, 20.0, -250.0]);
    c.layers[0].props.prop_mut("cameraOptions/focusDistance").unwrap().value = Value::Scalar(400.0);
    c.layers[1].props.prop_mut("lightOptions/intensity").unwrap().value = Value::Scalar(40.0);
    let second = check(&p, None);
    assert_ne!(first, second, "camera/light changes re-render");
    let top = super::default_view_cam(super::View3D::Top, 200.0, 100.0).state();
    let third = check(&p, Some(top));
    assert_ne!(third, second);
    assert!(cache.stats().hits > 0, "the cache was actually used");
}

/// A 1920 × 1080 comp with a full-frame 3D solid whose transform `set` changes (and an optional
/// one-node camera at the default camera's place), as the After Effects reference cases.
fn ae_frame(set: impl Fn(&mut Layer), camera: Option<([f64; 3], [f64; 3])>) -> (Project, ItemId) {
    let mut p = Project::default();
    let comp = Comp::new(1920, 1080, FrameRate::FPS_25, Tick::from_seconds_f64(5.0));
    let cid = p.add_item("C", Label::Sandstone, None, ItemKind::Comp(comp.clone().into()));
    let sid = p.add_item("L", Label::Red, None, ItemKind::Solid(Solid { color: [1.0, 0.0, 0.0], width: 1920, height: 1080, pixel_aspect: 1.0 }));
    let mut l = build::layer(&mut p, &comp, "L", LayerSource::Solid { item: sid }, (1920, 1080), None);
    l.switches.three_d = true;
    set(&mut l);
    let mut layers = vec![];
    if let Some((orientation, rotation)) = camera {
        let mut cam = build::layer(&mut p, &comp, "K", LayerSource::Camera, (1920, 1080), None);
        cam.auto_orient = AutoOrient::Off;
        cam.props.prop_mut("transform/position").unwrap().value = Value::Vec3([960.0, 540.0, -1920.0 * 50.0 / 36.0]);
        cam.props.prop_mut("transform/orientation").unwrap().value = Value::Vec3(orientation);
        for (m, v) in [("transform/rotationX", rotation[0]), ("transform/rotationY", rotation[1]), ("transform/rotation", rotation[2])] {
            cam.props.prop_mut(m).unwrap().value = Value::Scalar(v);
        }
        cam.props.prop_mut("cameraOptions/zoom").unwrap().value = Value::Scalar(1920.0 * 50.0 / 36.0);
        layers.push(cam);
    }
    layers.push(l);
    p.comp_mut(cid).unwrap().layers = layers;
    (p, cid)
}

/// Whether comp pixel (x, y) shows the solid, from its layer → comp mapping.
fn covers(p: &Project, cid: ItemId, secs: f64, x: u32, y: u32) -> bool {
    let c = p.comp(cid).unwrap();
    let ctx = EvalCtx::new(p, cid, c, Tick::from_seconds_f64(secs));
    let l = c.layers.last().unwrap();
    let inv = ctx.layer_to_comp(l).0.inverse().unwrap();
    let u = inv.apply(effectcraft_geom::vec2(x as f64 + 0.5, y as f64 + 0.5));
    (0.0..=1920.0).contains(&u.x) && (0.0..=1080.0).contains(&u.y)
}

fn check_points(p: &Project, cid: ItemId, secs: f64, case: &str, pts: &[(u32, u32, bool)]) {
    for &(x, y, red) in pts {
        assert_eq!(covers(p, cid, secs, x, y), red, "case {case}: pixel ({x}, {y}) should be {}", if red { "red" } else { "black" });
    }
}

fn set3(l: &mut Layer, m: &str, v: [f64; 3]) {
    l.props.prop_mut(m).unwrap().value = Value::Vec3(v);
}

fn set1(l: &mut Layer, m: &str, v: f64) {
    l.props.prop_mut(m).unwrap().value = Value::Scalar(v);
}

/// Orientation and X/Y/Z Rotation compose as `Rx · Ry · Rz` (Z turns the layer first), and
/// Orientation comes before the axis rotations, in layers and cameras alike. The points are
/// sampled from After Effects 2026 renders of the synthetic cases (each at least 3 px from an
/// edge; red = the solid covers the pixel), a third of them where the old Z · Y · X order differed.
#[test]
fn rotation_order_matches_after_effects() {
    // 488: Orientation [10, 20, 30].
    let (p, cid) = ae_frame(|l| set3(l, "transform/orientation", [10.0, 20.0, 30.0]), None);
    check_points(
        &p,
        cid,
        1.0,
        "488",
        &[
            (328, 687, false),
            (416, 764, false),
            (209, 237, true),
            (548, 861, false),
            (1899, 425, false),
            (87, 462, true),
            (162, 581, false),
            (662, 948, false),
            (682, 849, true),
            (618, 413, true),
            (1380, 181, true),
            (605, 378, true),
            (1830, 518, true),
            (559, 197, true),
            (1760, 843, true),
            (550, 823, true),
            (572, 952, false),
            (272, 963, false),
            (268, 761, false),
            (628, 1073, false),
            (218, 721, false),
            (256, 807, false),
            (225, 881, false),
            (167, 637, false),
        ],
    );
    // 800: X / Y / Z Rotation 10 / 20 / 30.
    let (p, cid) = ae_frame(
        |l| {
            set1(l, "transform/rotationX", 10.0);
            set1(l, "transform/rotationY", 20.0);
            set1(l, "transform/rotation", 30.0);
        },
        None,
    );
    check_points(
        &p,
        cid,
        1.0,
        "800",
        &[
            (104, 450, true),
            (755, 1002, false),
            (1407, 149, false),
            (271, 66, true),
            (288, 54, true),
            (113, 393, true),
            (204, 174, true),
            (1790, 360, false),
            (1414, 324, true),
            (682, 343, true),
            (1147, 153, true),
            (345, 446, true),
            (1516, 542, true),
            (666, 25, true),
            (1327, 1065, true),
            (778, 267, true),
            (1588, 5, false),
            (1706, 147, false),
            (357, 978, false),
            (97, 689, false),
            (1483, 79, false),
            (433, 848, false),
            (255, 802, false),
            (636, 1057, false),
        ],
    );
    // 801: Orientation [0, 30, 0] with X Rotation 40 (Orientation first).
    let (p, cid) = ae_frame(
        |l| {
            set3(l, "transform/orientation", [0.0, 30.0, 0.0]);
            set1(l, "transform/rotationX", 40.0);
        },
        None,
    );
    check_points(
        &p,
        cid,
        1.0,
        "801",
        &[
            (1356, 35, true),
            (1762, 19, true),
            (190, 227, true),
            (108, 182, true),
            (502, 832, true),
            (1539, 203, true),
            (722, 812, true),
            (1872, 76, true),
            (1166, 165, true),
            (1640, 380, true),
            (629, 619, true),
            (1796, 574, true),
            (1767, 311, true),
            (1432, 421, true),
            (1819, 425, true),
            (283, 252, true),
            (563, 1062, false),
            (428, 963, false),
            (77, 417, false),
            (47, 833, false),
            (995, 933, false),
            (159, 839, false),
            (827, 947, false),
            (564, 908, false),
        ],
    );
    // 804: a one-node camera with Orientation [4, 6, 30]; 805: the same as X / Y / Z Rotation.
    for (case, cam) in [("804", ([4.0, 6.0, 30.0], [0.0; 3])), ("805", ([0.0; 3], [4.0, 6.0, 30.0]))] {
        let (p, cid) = ae_frame(|_| {}, Some(cam));
        check_points(
            &p,
            cid,
            1.0,
            case,
            &[
                (204, 410, false),
                (1077, 38, false),
                (60, 492, false),
                (1732, 752, true),
                (265, 475, false),
                (1743, 821, true),
                (1417, 1043, true),
                (1559, 944, true),
                (1280, 310, true),
                (702, 984, true),
                (1072, 893, true),
                (370, 1042, true),
                (1353, 570, true),
                (1178, 136, true),
                (1201, 289, true),
                (1241, 605, true),
                (75, 337, false),
                (71, 138, false),
                (352, 24, false),
                (1764, 109, false),
                (186, 338, false),
                (1835, 668, false),
                (211, 269, false),
                (157, 143, false),
            ],
        );
    }
}

/// Keyed Orientation turns along the shortest arc between the keys (case 802: [0, 0, 0] at 0 s
/// to [60, 80, 40] at 2 s, linear; points from the After Effects render at 1 s, a third of them
/// where blending each angle on its own differed).
#[test]
fn keyed_orientation_renders_as_after_effects() {
    let (p, cid) = ae_frame(
        |l| {
            l.props.prop_mut("transform/orientation").unwrap().keys =
                vec![Keyframe::new(Tick::ZERO, Value::Vec3([0.0; 3])), Keyframe::new(Tick::from_seconds_f64(2.0), Value::Vec3([60.0, 80.0, 40.0]))];
        },
        None,
    );
    check_points(
        &p,
        cid,
        1.0,
        "802",
        &[
            (1880, 472, false),
            (1598, 191, false),
            (248, 285, true),
            (1157, 51, false),
            (1628, 129, false),
            (357, 37, true),
            (1865, 891, true),
            (1915, 946, true),
            (1728, 1021, true),
            (1178, 665, true),
            (1303, 631, true),
            (1339, 954, true),
            (910, 891, true),
            (1257, 1002, true),
            (1349, 890, true),
            (1388, 551, true),
            (356, 500, false),
            (490, 1058, false),
            (524, 829, false),
            (73, 29, false),
            (393, 769, false),
            (1867, 124, false),
            (42, 733, false),
            (648, 1007, false),
        ],
    );
    // The value After Effects reads back half-way.
    let c = p.comp(cid).unwrap();
    let v = c.layers[0].props.prop("transform/orientation").unwrap().value_at(Tick::from_seconds_f64(1.0)).as_vec3();
    let want = [30.6186, 35.7617, 28.4317];
    assert!((0..3).all(|i| (v[i] - want[i]).abs() < 1e-3), "{v:?}");
}
