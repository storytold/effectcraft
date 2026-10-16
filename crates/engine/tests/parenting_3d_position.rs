//! 3D parenting preserves Position and static poses in uniform parent spaces.
use effectcraft_engine::{
    Session,
    geom::{Mat4, Vec3, vec3},
    project::{BitDepth, LayerId},
    raster::Image,
    render::{Backend, EvalCtx, NoFootage, RenderOpts, Renderer},
    time::Tick,
};
use serde_json::{Value, json};
use std::sync::Arc;

fn command(s: &mut Session, id: &str, p: Value) -> Value {
    s.execute(id, p).unwrap()
}
fn set(s: &mut Session, layer: u64, path: &str, value: Value) {
    command(s, "prop.set", json!({"layer":layer,"path":path,"value":value}));
}
fn solid(s: &mut Session, name: &str) -> u64 {
    command(s, "layer.newSolid", json!({"name":name,"width":12,"height":10,"color":[0.8,0.3,0.1]}))["layer"].as_u64().unwrap()
}
fn fixture(three: bool, rotated: bool) -> (Session, u64, u64) {
    let mut s = Session::default();
    command(&mut s, "comp.new", json!({"name":"Synthetic 3D parent","width":64,"height":48,"duration":2,"frameRate":24}));
    Arc::make_mut(&mut s.project).settings.bit_depth = BitDepth::Bpc32;
    let parent = solid(&mut s, "Parent");
    let child = solid(&mut s, "Child");
    command(&mut s, "layer.setSwitch", json!({"layers":[parent],"switch":"video","value":false}));
    if three {
        command(&mut s, "layer.setSwitch", json!({"layers":[parent,child],"switch":"threeD","value":true}));
    }
    set(&mut s, parent, "transform/position", json!([20.0, 14.0, if three { 8.0 } else { 0.0 }]));
    set(&mut s, child, "transform/position", json!([30.0, 25.0, if three { 3.0 } else { 0.0 }]));
    if rotated {
        set(&mut s, parent, "transform/orientation", json!([15.0, -18.0, 23.0]));
        set(&mut s, parent, "transform/rotationX", json!(7.0));
        set(&mut s, parent, "transform/rotationY", json!(-11.0));
        set(&mut s, parent, "transform/rotation", json!(9.0));
        set(&mut s, parent, "transform/scale", json!([120.0, 120.0, 120.0]));
        set(&mut s, child, "transform/orientation", json!([-8.0, 12.0, -13.0]));
        set(&mut s, child, "transform/rotationX", json!(4.0));
        set(&mut s, child, "transform/rotationY", json!(6.0));
        set(&mut s, child, "transform/rotation", json!(-5.0));
    }
    (s, parent, child)
}
fn world(s: &Session, child: u64, t: Tick) -> Mat4 {
    let cid = s.active_comp_id().unwrap();
    let c = s.project.comp(cid).unwrap();
    EvalCtx::new(&s.project, cid, c, t).world_matrix(c.layer(LayerId(child)).unwrap())
}
fn points(m: Mat4) -> [Vec3; 5] {
    [vec3(6.0, 5.0, 0.0), vec3(0.0, 0.0, 0.0), vec3(12.0, 0.0, 0.0), vec3(0.0, 10.0, 0.0), vec3(12.0, 10.0, 0.0)].map(|p| m.apply(p))
}
fn displacement(a: Mat4, b: Mat4) -> f64 {
    points(a).into_iter().zip(points(b)).map(|(a, b)| (a - b).length()).fold(0.0, f64::max)
}
fn cpu(s: &Session, t: Tick) -> Image {
    Renderer::new(&s.project, &NoFootage, RenderOpts { backend: Backend::Cpu, motion_blur: false, ..Default::default() })
        .comp_frame_cpu(s.active_comp_id().unwrap(), t)
}
fn finite_visible(i: &Image) {
    assert_eq!([i.width, i.height], [64, 48]);
    assert!(i.data.iter().flatten().all(|v| v.is_finite()));
    assert!(i.data.iter().any(|p| p[3] > 0.9 && p[0] > 0.2), "synthetic child must be visibly rendered");
}
fn diff(a: &Image, b: &Image) -> f32 {
    assert_eq!([a.width, a.height], [b.width, b.height]);
    a.data.iter().flatten().zip(b.data.iter().flatten()).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max)
}
fn parent(s: &mut Session, child: u64, to: Option<u64>, compensate: bool) {
    command(s, "layer.setParent", json!({"layers":[child],"parent":to,"compensate":compensate}));
}
#[test]
fn parenting_2d_compensation_positive_control() {
    let (mut s, p, c) = fixture(false, false);
    let before = world(&s, c, Tick::ZERO);
    let image = cpu(&s, Tick::ZERO);
    finite_visible(&image);
    let old = s.project.clone();
    let undo = s.history.undo.len();
    parent(&mut s, c, Some(p), true);
    assert!(displacement(before, world(&s, c, Tick::ZERO)) < 1e-9);
    assert!(diff(&image, &cpu(&s, Tick::ZERO)) < 1e-6);
    assert_eq!(s.history.undo.len(), undo + 1);
    let attached = s.project.clone();
    assert!(s.undo());
    assert!(Arc::ptr_eq(&old, &s.project));
    assert!(s.redo());
    assert!(Arc::ptr_eq(&attached, &s.project));
    parent(&mut s, c, None, true);
    assert!(displacement(before, world(&s, c, Tick::ZERO)) < 1e-9);
}
#[test]
fn parenting_3d_explicit_uncompensated_positive_control() {
    let (mut s, p, c) = fixture(true, true);
    let old = world(&s, c, Tick::ZERO);
    let pm = world(&s, p, Tick::ZERO);
    let old_transform = s.active_comp().unwrap().layer(LayerId(c)).unwrap().transform().unwrap().clone();
    parent(&mut s, c, Some(p), false);
    let after = world(&s, c, Tick::ZERO);
    assert!(displacement(pm * old, after) < 1e-9);
    assert!(displacement(old, after) > 1.0, "control must visibly follow its parent");
    assert_eq!(&old_transform, s.active_comp().unwrap().layer(LayerId(c)).unwrap().transform().unwrap());
    assert!(cpu(&s, Tick::ZERO).data.iter().flatten().all(|v| v.is_finite()));
}
#[test]
fn parenting_3d_default_parent_preserves_position_and_cpu_pixels() {
    let (mut s, p, c) = fixture(true, false);
    let before = world(&s, c, Tick::ZERO);
    let image = cpu(&s, Tick::ZERO);
    finite_visible(&image);
    // Omit compensate to exercise the same public default as timeline pick-whip.
    command(&mut s, "layer.setParent", json!({"layers":[c],"parent":p}));
    let out = cpu(&s, Tick::ZERO);
    assert!(out.data.iter().flatten().all(|v| v.is_finite()));
    let motion = displacement(before, world(&s, c, Tick::ZERO));
    let pixels = diff(&image, &out);
    eprintln!("DEFAULT_PARENT motion={motion} pixels={pixels}");
    assert!(motion < 1e-9 && pixels < 1e-6, "3D default parenting must preserve world position and rendered pixels");
}
#[test]
fn parenting_3d_rotated_uniform_parent_preserves_pose() {
    let (mut s, p, c) = fixture(true, true);
    let before = world(&s, c, Tick::ZERO);
    let image = cpu(&s, Tick::ZERO);
    finite_visible(&image);
    parent(&mut s, c, Some(p), true);
    let motion = displacement(before, world(&s, c, Tick::ZERO));
    let out = cpu(&s, Tick::ZERO);
    let pixels = diff(&image, &out);
    eprintln!("ROTATED_PARENT motion={motion} pixels={pixels}");
    assert!(motion < 1e-8 && pixels < 2e-5, "3D rotated uniform parent must preserve child pose");
}
#[test]
fn parenting_3d_unparent_preserves_position() {
    let (mut s, p, c) = fixture(true, false);
    parent(&mut s, c, Some(p), false);
    let before = world(&s, c, Tick::ZERO);
    let image = cpu(&s, Tick::ZERO);
    finite_visible(&image);
    parent(&mut s, c, None, true);
    let motion = displacement(before, world(&s, c, Tick::ZERO));
    let pixels = diff(&image, &cpu(&s, Tick::ZERO));
    eprintln!("UNPARENT motion={motion} pixels={pixels}");
    assert!(motion < 1e-9 && pixels < 1e-6, "3D unparenting must preserve world position");
}
#[test]
fn parenting_3d_animated_position_preserves_authored_keys() {
    let (mut s, p, c) = fixture(true, false);
    for (time, value) in [(0.0, json!([27.0, 22.0, 2.0])), (1.0, json!([35.0, 29.0, 5.0]))] {
        command(&mut s, "prop.addKey", json!({"layer":c,"path":"transform/position","time":time,"value":value}));
    }
    let times = [0.0, 0.25, 0.75, 1.0].map(Tick::from_seconds_f64);
    let before = times.map(|t| world(&s, c, t));
    let undo = s.history.undo.len();
    let old = s.project.clone();
    parent(&mut s, c, Some(p), true);
    let motion = times.into_iter().zip(before).map(|(t, b)| displacement(b, world(&s, c, t))).fold(0.0, f64::max);
    eprintln!("ANIMATED_PARENT motion={motion}");
    assert!(motion < 1e-8, "3D parenting must re-express ordinary Position keys");
    assert_eq!(s.history.undo.len(), undo + 1);
    let attached = s.project.clone();
    assert!(s.undo());
    assert!(Arc::ptr_eq(&old, &s.project));
    assert!(s.redo());
    assert!(Arc::ptr_eq(&attached, &s.project));
}

fn anchor_motion(a: Mat4, b: Mat4) -> f64 {
    (a.apply(vec3(6.0, 5.0, 0.0)) - b.apply(vec3(6.0, 5.0, 0.0))).length()
}
#[test]
fn parenting_3d_rotated_parent_preserves_anchor() {
    let (mut s, p, c) = fixture(true, true);
    let before = world(&s, c, Tick::ZERO);
    let image = cpu(&s, Tick::ZERO);
    finite_visible(&image);
    parent(&mut s, c, Some(p), true);
    let after = world(&s, c, Tick::ZERO);
    let anchor = anchor_motion(before, after);
    eprintln!("ROTATED_ANCHOR anchor={anchor} corners={} pixels={}", displacement(before, after), diff(&image, &cpu(&s, Tick::ZERO)));
    assert!(anchor < 1e-8, "3D rotated parenting must retain the world-space anchor");
    parent(&mut s, c, None, true);
    assert!(anchor_motion(before, world(&s, c, Tick::ZERO)) < 1e-8, "3D rotated unparent must retain the world-space anchor");
}
#[test]
fn parenting_3d_separated_position_preserves_each_axis_and_undo() {
    let (mut s, p, c) = fixture(true, false);
    command(&mut s, "prop.separateDimensions", json!({"layer":c,"value":true}));
    for (name, keys) in [
        ("positionX", vec![(0.0, 27.0), (1.0, 35.0)]),
        ("positionY", vec![(0.0, 22.0), (0.5, 27.0), (1.0, 29.0)]),
        ("positionZ", vec![(0.0, 2.0), (0.25, 3.0), (1.0, 5.0)]),
    ] {
        for (time, value) in keys {
            command(&mut s, "prop.addKey", json!({"layer":c,"path":format!("transform/{name}"),"time":time,"value":value}));
        }
    }
    let times = [0.0, 0.25, 0.5, 0.75, 1.0].map(Tick::from_seconds_f64);
    let before = times.map(|t| world(&s, c, t));
    let old = s.project.clone();
    let undo = s.history.undo.len();
    parent(&mut s, c, Some(p), true);
    let error = times.into_iter().zip(before).map(|(t, b)| displacement(b, world(&s, c, t))).fold(0.0, f64::max);
    eprintln!("SEPARATED_PARENT motion={error}");
    assert!(error < 1e-8, "3D parenting must preserve separated Position animation");
    for name in ["positionX", "positionY", "positionZ"] {
        let a = &old.comp(s.active_comp_id().unwrap()).unwrap().layer(LayerId(c)).unwrap().props.prop(&format!("transform/{name}")).unwrap().keys;
        let b = &s.active_comp().unwrap().layer(LayerId(c)).unwrap().props.prop(&format!("transform/{name}")).unwrap().keys;
        assert_eq!(a.iter().map(|k| k.time).collect::<Vec<_>>(), b.iter().map(|k| k.time).collect::<Vec<_>>(), "parent compensation does not retime axes");
    }
    assert_eq!(s.history.undo.len(), undo + 1);
    let attached = s.project.clone();
    assert!(s.undo());
    assert!(Arc::ptr_eq(&old, &s.project));
    assert!(s.redo());
    assert!(Arc::ptr_eq(&attached, &s.project));
    parent(&mut s, c, None, true);
    for (t, b) in times.into_iter().zip(before) {
        assert!(displacement(b, world(&s, c, t)) < 1e-8);
    }
}
#[test]
fn parenting_3d_position_spatial_tangents_follow_relative_parent_space() {
    let (mut s, p, c) = fixture(true, true);
    for (time, value) in [(0.0, json!([27.0, 22.0, 2.0])), (1.0, json!([35.0, 29.0, 5.0]))] {
        command(&mut s, "prop.addKey", json!({"layer":c,"path":"transform/position","time":time,"value":value}));
    }
    // Generated manual spatial-curve data; temporal interpolation remains linear.
    let cid = s.active_comp_id().unwrap();
    let pr = Arc::make_mut(&mut s.project).comp_mut(cid).unwrap().layer_mut(LayerId(c)).unwrap().props.prop_mut("transform/position").unwrap();
    for key in &mut pr.keys {
        key.spatial_auto = false;
    }
    pr.keys[0].spatial_out = [3.0, 5.0, -1.0];
    pr.keys[1].spatial_in = [-4.0, 2.0, 1.0];
    let tangents = pr.keys.iter().map(|k| (k.spatial_in, k.spatial_out)).collect::<Vec<_>>();
    let relative = world(&s, p, Tick::ZERO).inverse().unwrap();
    let times = [0.0, 0.25, 0.5, 0.75, 1.0].map(Tick::from_seconds_f64);
    let before = times.map(|t| world(&s, c, t));
    parent(&mut s, c, Some(p), true);
    let error = times.into_iter().zip(before).map(|(t, b)| anchor_motion(b, world(&s, c, t))).fold(0.0, f64::max);
    let keys = &s.active_comp().unwrap().layer(LayerId(c)).unwrap().props.prop("transform/position").unwrap().keys;
    let tangent_error = keys
        .iter()
        .zip(tangents)
        .flat_map(|(key, (tin, tout))| [(key.spatial_in, tin), (key.spatial_out, tout)])
        .map(|(after, before)| (Vec3::from(after) - relative.apply_vec(Vec3::from(before))).length())
        .fold(0.0, f64::max);
    eprintln!("SPATIAL_PARENT anchor={error} tangent={tangent_error}");
    assert!(error < 1e-8 && tangent_error < 1e-8, "3D Position spatial tangents must remain relative vectors");
}
#[test]
fn parenting_3d_parent_source_pixel_aspect_is_not_inherited() {
    let (mut s, _, c) = fixture(true, false);
    let p = command(&mut s, "layer.newSolid", json!({"name":"Wide pixels parent","width":12,"height":10,"color":[0.1,0.2,0.3],"pixelAspect":2.0}))["layer"]
        .as_u64()
        .unwrap();
    command(&mut s, "layer.setSwitch", json!({"layers":[p],"switch":"threeD","value":true}));
    command(&mut s, "layer.setSwitch", json!({"layers":[p],"switch":"video","value":false}));
    set(&mut s, p, "transform/position", json!([20.0, 14.0, 8.0]));
    let before = world(&s, c, Tick::ZERO);
    let image = cpu(&s, Tick::ZERO);
    finite_visible(&image);
    parent(&mut s, c, Some(p), true);
    let error = displacement(before, world(&s, c, Tick::ZERO));
    let pixels = diff(&image, &cpu(&s, Tick::ZERO));
    eprintln!("PAR_PARENT motion={error} pixels={pixels}");
    assert!(error < 1e-8 && pixels < 1e-6, "3D children must not inherit parent source pixel aspect");
}

fn rotation_properties(s: &Session, c: u64) -> Vec<effectcraft_engine::project::Property> {
    ["rotationX", "rotationY", "rotation"]
        .map(|name| s.active_comp().unwrap().layer(LayerId(c)).unwrap().props.prop(&format!("transform/{name}")).unwrap().clone())
        .into()
}
fn axis_animation(s: &mut Session, c: u64) {
    for (name, values) in [("rotationX", [-21.0, 35.0]), ("rotationY", [9.0, -30.0]), ("rotation", [12.0, 28.0])] {
        for (time, value) in [(0.0, values[0]), (1.0, values[1])] {
            command(s, "prop.addKey", json!({"layer":c,"path":format!("transform/{name}"),"time":time,"value":value}));
        }
    }
}
#[test]
fn parenting_static_orientation_preserves_nonuniform_child_scale_and_axis_rotation_keys() {
    let (mut s, p, c) = fixture(true, true);
    set(&mut s, c, "transform/scale", json!([83.0, 127.0, 61.0]));
    axis_animation(&mut s, c);
    let rotations = rotation_properties(&s, c);
    let old = s.project.clone();
    let undo = s.history.undo.len();
    let times = [0.0, 0.25, 0.5, 0.75, 1.0].map(Tick::from_seconds_f64);
    let before = times.map(|t| world(&s, c, t));
    let images = times.map(|t| cpu(&s, t));
    for i in &images {
        finite_visible(i);
    }
    parent(&mut s, c, Some(p), true);
    let error = times.into_iter().zip(before).map(|(t, b)| displacement(b, world(&s, c, t))).fold(0.0, f64::max);
    let pixels = times.into_iter().zip(&images).map(|(t, i)| diff(i, &cpu(&s, t))).fold(0.0, f32::max);
    eprintln!("STATIC_AXIS_POSE corners={error} pixels={pixels}");
    assert!(error < 1e-8 && pixels < 2e-5, "static Orientation must preserve pose with nonuniform child Scale and authored axis Rotation animation");
    assert_eq!(rotation_properties(&s, c), rotations, "axis Rotation properties are byte-for-byte authored");
    assert_eq!(s.history.undo.len(), undo + 1);
    let attached = s.project.clone();
    assert!(s.undo());
    assert!(Arc::ptr_eq(&s.project, &old));
    assert!(s.redo());
    assert!(Arc::ptr_eq(&s.project, &attached));
    parent(&mut s, c, None, true);
    for (t, b) in times.into_iter().zip(before) {
        assert!(displacement(b, world(&s, c, t)) < 1e-8);
    }
    assert_eq!(rotation_properties(&s, c), rotations);
}
#[test]
fn parenting_uniform_parent_preserves_animated_eased_scale() {
    use effectcraft_engine::keyframe::{Ease, Interp};
    let (mut s, p, c) = fixture(true, true);
    for (time, value) in [(0.0, json!([80.0, 120.0, 95.0])), (1.0, json!([135.0, 75.0, 150.0]))] {
        command(&mut s, "prop.addKey", json!({"layer":c,"path":"transform/scale","time":time,"value":value}));
    }
    let cid = s.active_comp_id().unwrap();
    let scale = Arc::make_mut(&mut s.project).comp_mut(cid).unwrap().layer_mut(LayerId(c)).unwrap().props.prop_mut("transform/scale").unwrap();
    for key in &mut scale.keys {
        key.in_interp = Interp::Bezier;
        key.out_interp = Interp::Bezier;
        key.in_ease = vec![Ease { speed: 17.0, influence: 0.3 }, Ease { speed: -13.0, influence: 0.6 }, Ease { speed: 7.0, influence: 0.4 }];
        key.out_ease = vec![Ease { speed: 11.0, influence: 0.4 }, Ease { speed: -19.0, influence: 0.2 }, Ease { speed: 13.0, influence: 0.5 }];
    }
    let old_scale = scale.clone();
    let rotations = rotation_properties(&s, c);
    let times = [0.0, 0.25, 0.5, 0.75, 1.0].map(Tick::from_seconds_f64);
    let before = times.map(|t| world(&s, c, t));
    let images = times.map(|t| cpu(&s, t));
    for image in &images {
        finite_visible(image);
    }
    parent(&mut s, c, Some(p), true);
    let error = times.into_iter().zip(before).map(|(t, b)| displacement(b, world(&s, c, t))).fold(0.0, f64::max);
    let pixels = times.into_iter().zip(&images).map(|(t, i)| diff(i, &cpu(&s, t))).fold(0.0, f32::max);
    eprintln!("EASED_SCALE_POSE corners={error} pixels={pixels}");
    assert!(error < 1e-8 && pixels < 2e-5, "uniform parent compensation must preserve animated eased Scale poses");
    assert_eq!(rotation_properties(&s, c), rotations);
    let scale = s.active_comp().unwrap().layer(LayerId(c)).unwrap().props.prop("transform/scale").unwrap();
    assert_eq!(scale.keys.iter().map(|k| k.time).collect::<Vec<_>>(), old_scale.keys.iter().map(|k| k.time).collect::<Vec<_>>());
    for (old, new) in old_scale.keys.iter().zip(&scale.keys) {
        for (old, new) in old.in_ease.iter().chain(&old.out_ease).zip(new.in_ease.iter().chain(&new.out_ease)) {
            assert!((new.speed - old.speed / 1.2).abs() < 1e-9);
            assert_eq!(new.influence, old.influence);
        }
    }
}
#[test]
fn parenting_static_orientation_handles_signed_uniform_scale_and_gimbal_order() {
    let mut max_corners = 0.0f64;
    let mut max_pixels = 0.0f32;
    for factor in [120.0, -120.0] {
        for orientation in [[23.0, 90.0, -37.0], [-41.0, -90.0, 16.0]] {
            let (mut s, p, c) = fixture(true, true);
            set(&mut s, p, "transform/orientation", json!([0.0, 0.0, 0.0]));
            set(&mut s, p, "transform/rotationX", json!(0.0));
            set(&mut s, p, "transform/rotationY", json!(0.0));
            set(&mut s, p, "transform/rotation", json!(42.0));
            set(&mut s, p, "transform/scale", json!([factor, factor, factor]));
            set(&mut s, c, "transform/orientation", json!(orientation));
            set(&mut s, c, "transform/scale", json!([200.0, 160.0, 150.0]));
            let before = world(&s, c, Tick::ZERO);
            let image = cpu(&s, Tick::ZERO);
            assert!(image.data.iter().flatten().all(|v| v.is_finite()));
            assert!(image.data.iter().any(|p| p[3] > 0.05), "gimbal control must render visible pixels");
            let rotations = rotation_properties(&s, c);
            parent(&mut s, c, Some(p), true);
            let error = displacement(before, world(&s, c, Tick::ZERO));
            let pixels = diff(&image, &cpu(&s, Tick::ZERO));
            max_corners = max_corners.max(error);
            max_pixels = max_pixels.max(pixels);
            assert_eq!(rotation_properties(&s, c), rotations);
        }
    }
    eprintln!("SIGNED_GIMBAL_POSE corners={max_corners} pixels={max_pixels}");
    assert!(max_corners < 1e-8 && max_pixels < 2e-5, "static uniform pose must use the existing Euler order including signed scale and gimbal angles");
}

/// A static Orientation just off gimbal lock (Y within 1e-6 degrees of ±90) keeps its world pose
/// under a uniform parent. Recovering Euler angles from the matrix must not lose precision there.
#[test]
fn parenting_static_orientation_is_exact_just_off_gimbal_lock() {
    let mut max_corners = 0.0f64;
    for factor in [120.0, -120.0] {
        for y in [90.0 - 1e-6, -90.0 + 1e-6] {
            let (mut s, p, c) = fixture(true, true);
            set(&mut s, p, "transform/orientation", json!([0.0, 0.0, 0.0]));
            set(&mut s, p, "transform/rotationX", json!(0.0));
            set(&mut s, p, "transform/rotationY", json!(0.0));
            set(&mut s, p, "transform/rotation", json!(42.0));
            set(&mut s, p, "transform/scale", json!([factor, factor, factor]));
            set(&mut s, c, "transform/orientation", json!([23.0, y, -37.0]));
            set(&mut s, c, "transform/scale", json!([200.0, 160.0, 150.0]));
            let before = world(&s, c, Tick::ZERO);
            parent(&mut s, c, Some(p), true);
            max_corners = max_corners.max(displacement(before, world(&s, c, Tick::ZERO)));
        }
    }
    assert!(max_corners < 1e-9, "a static Orientation near gimbal lock must keep its world pose: corners {max_corners}");
}

#[test]
fn parenting_animated_orientation_and_nonuniform_parent_keep_authored_pose_boundary() {
    for animated in [true, false] {
        let (mut s, p, c) = fixture(true, true);
        if animated {
            for (time, value) in [(0.0, json!([-8.0, 12.0, -13.0])), (1.0, json!([5.0, 18.0, 14.0]))] {
                command(&mut s, "prop.addKey", json!({"layer":c,"path":"transform/orientation","time":time,"value":value}));
            }
        } else {
            set(&mut s, p, "transform/scale", json!([120.0, 100.0, 80.0]));
        }
        let layer = s.active_comp().unwrap().layer(LayerId(c)).unwrap();
        let orientation = layer.props.prop("transform/orientation").unwrap().clone();
        let scale = layer.props.prop("transform/scale").unwrap().clone();
        let before = world(&s, c, Tick::ZERO);
        parent(&mut s, c, Some(p), true);
        let layer = s.active_comp().unwrap().layer(LayerId(c)).unwrap();
        assert_eq!(layer.props.prop("transform/orientation").unwrap(), &orientation, "unsupported animated Euler/shear receives no blind Orientation rewrite");
        assert_eq!(layer.props.prop("transform/scale").unwrap(), &scale, "unsupported animated Euler/shear receives no blind Scale rewrite");
        assert!(anchor_motion(before, world(&s, c, Tick::ZERO)) < 1e-8, "boundary retains already-proved Position behavior");
        assert!(cpu(&s, Tick::ZERO).data.iter().flatten().all(|v| v.is_finite()));
    }
}

/// Position's temporal speed is measured along its spatial path, in parent-space units.
#[test]
fn parenting_uniform_parent_preserves_eased_position_at_quarter_times_and_undo() {
    use effectcraft_engine::keyframe::{Ease, Interp};
    let mut maximum_error = 0.0f64;
    for factor in [100.0, 200.0] {
        let (mut s, p, c) = fixture(true, false);
        set(&mut s, p, "transform/scale", json!([factor, factor, factor]));
        for (time, value) in [(0.0, json!([27.0, 22.0, 2.0])), (1.0, json!([35.0, 29.0, 5.0]))] {
            command(&mut s, "prop.addKey", json!({"layer":c,"path":"transform/position","time":time,"value":value}));
        }
        let cid = s.active_comp_id().unwrap();
        let position = Arc::make_mut(&mut s.project).comp_mut(cid).unwrap().layer_mut(LayerId(c)).unwrap().props.prop_mut("transform/position").unwrap();
        for key in &mut position.keys {
            key.in_interp = Interp::Bezier;
            key.out_interp = Interp::Bezier;
            key.in_ease = vec![Ease { speed: 10.0, influence: 1.0 / 3.0 }];
            key.out_ease = vec![Ease { speed: 10.0, influence: 1.0 / 3.0 }];
            key.auto_bezier = false;
            key.spatial_auto = false;
            key.spatial_in = [0.0; 3];
            key.spatial_out = [0.0; 3];
        }
        let times = [0.0, 0.25, 0.75, 1.0].map(Tick::from_seconds_f64);
        let before = times.map(|t| world(&s, c, t));
        assert!(anchor_motion(before[0], before[1]) > 1.0, "the eased Position fixture must actually move at quarter time");
        let old = s.project.clone();
        let undo = s.history.undo.len();
        // Omit compensate to exercise the public default without depending on the schema-only fix.
        s.execute_checked("layer.setParent", json!({"layers":[c],"parent":p})).unwrap();
        let after = times.map(|t| world(&s, c, t));
        assert!(after.iter().all(|m| m.0.iter().flatten().all(|v| v.is_finite())));
        for i in [0, 3] {
            assert!(anchor_motion(before[i], after[i]) < 1e-8, "parent compensation must preserve the authored Position endpoints");
        }
        for i in [1, 2] {
            let error = anchor_motion(before[i], after[i]);
            maximum_error = maximum_error.max(error);
            eprintln!("EASED_POSITION_PARENT factor={factor} time={} anchor_error={error}", times[i].seconds());
        }
        assert_eq!(s.history.undo.len(), undo + 1, "parent compensation is one undo action");
        let attached = s.project.clone();
        assert!(s.undo());
        assert!(Arc::ptr_eq(&old, &s.project), "Undo restores the exact authored project");
        for (t, expected) in times.into_iter().zip(before) {
            assert!(anchor_motion(expected, world(&s, c, t)) < 1e-8, "Undo restores all sampled world anchors");
        }
        assert!(s.redo());
        assert!(Arc::ptr_eq(&attached, &s.project), "Redo restores the exact compensated project");
        for (t, expected) in times.into_iter().zip(after) {
            assert!(anchor_motion(expected, world(&s, c, t)) < 1e-8, "Redo retains the same compensated samples");
        }
    }
    assert!(maximum_error < 1e-8, "3D uniform parent compensation must preserve eased Position at quarter and three-quarter times");
}

/// Parenting a two-node camera to a default null keeps its world frame (eye, forward, down):
/// Position and Point of Interest are re-expressed in the null's space (#1010, #1020).
#[test]
fn parenting_camera_to_default_null_preserves_framing() {
    use effectcraft_engine::render::three_d::camera::layer_frame;
    let mut s = Session::default();
    command(&mut s, "comp.new", json!({"name":"Camera","width":640,"height":360,"duration":2,"frameRate":24}));
    let cam = command(&mut s, "layer.newCamera", json!({"name":"Camera 1"}))["layer"].as_u64().unwrap();
    let null = command(&mut s, "layer.newNull", json!({"name":"Null 1"}))["layer"].as_u64().unwrap();
    let frame = |s: &Session| {
        let cid = s.active_comp_id().unwrap();
        let c = s.project.comp(cid).unwrap();
        let ctx = EvalCtx::new(&s.project, cid, c, Tick::ZERO);
        layer_frame(&ctx, c.layer(LayerId(cam)).unwrap())
    };
    let authored = |s: &Session, path: &str| {
        let cid = s.active_comp_id().unwrap();
        s.project.comp(cid).unwrap().layer(LayerId(cam)).unwrap().props.prop(path).unwrap().value.as_vec3()
    };
    let (eye0, fwd0, down0) = frame(&s);
    let (pos0, poi0) = (authored(&s, "transform/position"), authored(&s, "transform/poi"));
    let frame_error = |(a0, b0, c0): (Vec3, Vec3, Vec3), (a1, b1, c1): (Vec3, Vec3, Vec3)| {
        [(a0 - a1).length(), (b0 - b1).length(), (c0 - c1).length()].into_iter().fold(0.0, f64::max)
    };
    let undo = s.history.undo.len();
    parent(&mut s, cam, Some(null), true);
    assert!(frame_error(frame(&s), (eye0, fwd0, down0)) < 1e-9, "the camera frame must not move");
    // Compensation rewrote the authored values, not just the parent pointer.
    assert!((authored(&s, "transform/position")[0] - pos0[0]).abs() > 1.0, "position is re-expressed in the null's space");
    assert!((authored(&s, "transform/poi")[0] - poi0[0]).abs() > 1.0, "point of interest follows it");
    assert_eq!(s.history.undo.len(), undo + 1, "parent compensation is one undo action");
    // Unparenting restores the authored transform and the same frame.
    parent(&mut s, cam, None, true);
    assert!(frame_error(frame(&s), (eye0, fwd0, down0)) < 1e-9);
    assert_eq!(authored(&s, "transform/position"), pos0, "unparenting restores position");
    assert_eq!(authored(&s, "transform/poi"), poi0, "unparenting restores point of interest");
}
