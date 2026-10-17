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

/// A deterministic expression host keeps this compositor test independent of the JS engine.
struct PositionExpression(fn(f64) -> [f64; 3]);

#[test]
fn orient_expression_closed_excursion_survives_a_long_hold() {
    let host = PositionExpression(|t| {
        let t = t - 60.0;
        let y = if t < 0.0 {
            0.0
        } else if t < 0.1 {
            1000.0 * t
        } else if t < 0.2 {
            200.0 - 1000.0 * t
        } else {
            0.0
        };
        [300.0, 200.0 + y, 0.0]
    });
    for three_d in [false, true] {
        let (mut p, cid, mut comp) = setup();
        comp.duration = Tick::from_seconds_f64(1800.0);
        let mut l = solid(&mut p, &comp, three_d, [300.0, 200.0, 0.0]);
        l.auto_orient = AutoOrient::AlongPath;
        l.props.prop_mut("transform/position").unwrap().expr = Some(effectcraft_project::Expression { text: "closed excursion".into(), enabled: true });
        comp.layers.push(l);
        *p.comp_mut(cid).unwrap() = comp;
        let c = p.comp(cid).unwrap();
        // The first request is a cold, late seek. Reverse order and subframe seeks agree.
        for secs in [900.0, 60.2333333333, 1799.0, 900.017, 60.2] {
            let ctx = EvalCtx { expr: Some(&host), ..EvalCtx::new(&p, cid, c, Tick::from_seconds_f64(secs)) };
            let x = ctx.world_matrix(&c.layers[0]).apply_vec(vec3(1.0, 0.0, 0.0));
            assert!((x - vec3(0.0, -1.0, 0.0)).length() < 1e-6, "3d={three_d} t={secs}: {x:?}");
        }
    }
}

impl crate::ExprHost for PositionExpression {
    fn eval(&self, ctx: &EvalCtx, _layer: &Layer, _prop: &effectcraft_project::Property, _value: &Value) -> Result<Value, String> {
        Ok(Value::Vec3((self.0)(ctx.time.seconds())))
    }
}

/// #1650: Position driven only by an expression has no keys to recover the stopped heading from.
#[test]
fn orient_expression_keeps_heading_after_stop_and_before_start() {
    let host = PositionExpression(|t| [150.0 - 100.0 * (t - 0.5).clamp(0.0, 0.5), 50.0, 0.0]);
    for three_d in [false, true] {
        let (mut p, cid, comp) = setup();
        let mut l = solid(&mut p, &comp, three_d, [150.0, 50.0, 0.0]);
        l.auto_orient = AutoOrient::AlongPath;
        l.props.prop_mut("transform/position").unwrap().expr =
            Some(effectcraft_project::Expression { text: "expression-driven position".into(), enabled: true });
        p.comp_mut(cid).unwrap().layers.push(l);
        let c = p.comp(cid).unwrap();
        // Seek to the held frame first: orientation must not depend on playback history.
        for secs in [1.9, 1.0 + 1.0 / 30.0, 1.0, 0.75, 0.0, 0.49] {
            let ctx = EvalCtx { expr: Some(&host), ..EvalCtx::new(&p, cid, c, Tick::from_seconds_f64(secs)) };
            let x_axis = ctx.world_matrix(&c.layers[0]).apply_vec(vec3(1.0, 0.0, 0.0));
            assert!((x_axis - vec3(-1.0, 0.0, 0.0)).length() < 1e-6, "3d={three_d} t={secs}: {x_axis:?}");
        }
    }
}

#[test]
fn orient_constant_expression_does_not_invent_a_heading() {
    let host = PositionExpression(|_| [50.0, 50.0, 0.0]);
    let (mut p, cid, comp) = setup();
    let mut l = solid(&mut p, &comp, false, [50.0, 50.0, 0.0]);
    l.auto_orient = AutoOrient::AlongPath;
    l.props.prop_mut("transform/position").unwrap().expr = Some(effectcraft_project::Expression { text: "[50, 50, 0]".into(), enabled: true });
    p.comp_mut(cid).unwrap().layers.push(l);
    let c = p.comp(cid).unwrap();
    let ctx = EvalCtx { expr: Some(&host), ..EvalCtx::new(&p, cid, c, Tick::from_seconds_f64(1.5)) };
    let x_axis = ctx.world_matrix(&c.layers[0]).apply_vec(vec3(1.0, 0.0, 0.0));
    assert!((x_axis - vec3(1.0, 0.0, 0.0)).length() < 1e-6, "{x_axis:?}");
}

#[test]
fn orient_expression_hold_uses_the_previous_leg_not_the_next_one() {
    // Travel left, pause, then travel down. During the pause the heading must remain left.
    let host = PositionExpression(|t| [150.0 - 100.0 * t.clamp(0.0, 0.5), 50.0 + 100.0 * (t - 1.0).clamp(0.0, 0.5), 0.0]);
    for three_d in [false, true] {
        let (mut p, cid, comp) = setup();
        let mut l = solid(&mut p, &comp, three_d, [150.0, 50.0, 0.0]);
        l.auto_orient = AutoOrient::AlongPath;
        let prop = l.props.prop_mut("transform/position").unwrap();
        prop.expr = Some(effectcraft_project::Expression { text: "expression overrides these keys".into(), enabled: true });
        // Unrelated underlying keys must not dictate the expression's heading.
        prop.keys = vec![Keyframe::new(Tick::ZERO, Value::Vec3([0.0; 3])), Keyframe::new(Tick::from_seconds_f64(0.7), Value::Vec3([100.0, 0.0, 0.0]))];
        p.comp_mut(cid).unwrap().layers.push(l);
        let c = p.comp(cid).unwrap();
        for (secs, expected) in [(1.9, vec3(0.0, 1.0, 0.0)), (0.8, vec3(-1.0, 0.0, 0.0)), (1.25, vec3(0.0, 1.0, 0.0))] {
            let ctx = EvalCtx { expr: Some(&host), ..EvalCtx::new(&p, cid, c, Tick::from_seconds_f64(secs)) };
            let x_axis = ctx.world_matrix(&c.layers[0]).apply_vec(vec3(1.0, 0.0, 0.0));
            assert!((x_axis - expected).length() < 1e-6, "3d={three_d} t={secs}: {x_axis:?}");
        }
    }
}

#[test]
fn orient_expression_curved_path_keeps_its_terminal_tangent() {
    let host = PositionExpression(|t| {
        let angle = std::f64::consts::PI * ((t - 0.2) / 0.6).clamp(0.0, 1.0);
        [100.0 + 50.0 * angle.cos(), 50.0 + 50.0 * angle.sin(), 0.0]
    });
    for three_d in [false, true] {
        let (mut p, cid, comp) = setup();
        let mut l = solid(&mut p, &comp, three_d, [150.0, 50.0, 0.0]);
        l.auto_orient = AutoOrient::AlongPath;
        l.props.prop_mut("transform/position").unwrap().expr = Some(effectcraft_project::Expression { text: "curved expression path".into(), enabled: true });
        p.comp_mut(cid).unwrap().layers.push(l);
        let c = p.comp(cid).unwrap();
        let heading = |t| {
            let ctx = EvalCtx { expr: Some(&host), ..EvalCtx::new(&p, cid, c, Tick::from_seconds_f64(t)) };
            ctx.world_matrix(&c.layers[0]).apply_vec(vec3(1.0, 0.0, 0.0))
        };
        let terminal = heading(0.8);
        for secs in [1.9, 1.901] {
            let held = heading(secs);
            assert!((held - terminal).length() < 1e-5, "3d={three_d} t={secs}: {held:?} vs {terminal:?}");
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
