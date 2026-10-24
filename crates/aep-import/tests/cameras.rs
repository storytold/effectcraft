//! Cameras, lights and the 3D rotations (251-256, 341-345, 485-488, 535-537).

mod common;

use common::{comp, load, try_import};
use effectcraft_keyframe::Value;
use effectcraft_project::{AutoOrient, Layer, LayerSource, LightKind, Project};

/// Imports without losses and returns layer `name` of comp C.
fn layer(file: &str, name: &str) -> Layer {
    let mut p = Project::default();
    let r = try_import(&mut p, file, false).unwrap_or_else(|e| panic!("{file}: lossless import refused: {e}"));
    let (_, c) = comp(&p, &r, "C");
    c.layers.iter().find(|l| l.name == name).unwrap_or_else(|| panic!("{file}: no layer {name}")).clone()
}

fn v(l: &Layer, path: &str) -> Value {
    l.props.prop(path).unwrap_or_else(|| panic!("no {path}")).value.clone()
}

fn close(a: &Value, b: &[f64]) {
    let c = a.components();
    assert_eq!(c.len(), b.len(), "{a:?}");
    for (x, y) in c.iter().zip(b) {
        assert!((x - y).abs() < 1e-3, "{a:?} != {b:?}");
    }
}

#[test]
fn rotations_and_orientation_of_a_3d_layer() {
    let l = layer("486-layer-3d-rotx-30.aep", "L");
    assert!(l.switches.three_d);
    close(&v(&l, "transform/rotationX"), &[30.0]);
    close(&v(&l, "transform/rotationY"), &[0.0]);
    close(&v(&layer("487-layer-3d-roty-45.aep", "L"), "transform/rotationY"), &[45.0]);
    close(&v(&layer("488-layer-3d-orientation-10-20-30.aep", "L"), "transform/orientation"), &[10.0, 20.0, 30.0]);
    close(&v(&layer("485-layer-3d-on.aep", "L"), "transform/orientation"), &[0.0, 0.0, 0.0]);
}

#[test]
fn cameras() {
    // 256: default two-node camera: point of interest at the centre, the auto Position as After
    // Effects reports it, Zoom = Focus Distance = 2666.67, Aperture 25.309.
    let c = layer("256-camera-two-node-default.aep", "Camera 1");
    assert!(matches!(c.source, LayerSource::Camera));
    assert_eq!(c.auto_orient, AutoOrient::TowardsPointOfInterest);
    close(&v(&c, "transform/poi"), &[960.0, 540.0, 0.0]);
    close(&v(&c, "transform/position"), &[0.0, 0.0, -2666.6667]);
    close(&v(&c, "cameraOptions/zoom"), &[2666.6667]);
    close(&v(&c, "cameraOptions/focusDistance"), &[2666.6667]);
    close(&v(&c, "cameraOptions/aperture"), &[25.3093]);
    assert_eq!(v(&c, "cameraOptions/dof"), Value::Bool(false));
    // 251: one-node camera.
    assert_eq!(layer("251-camera-one-node.aep", "Camera 1").auto_orient, AutoOrient::Off);
    // 253: position set by the user.
    close(&v(&layer("253-camera-position-100-200-m1000.aep", "Camera 1"), "transform/position"), &[100.0, 200.0, -1000.0]);
    // 341, 342: zoom, depth of field.
    close(&v(&layer("341-camera-zoom-2000.aep", "Camera 1"), "cameraOptions/zoom"), &[2000.0]);
    let c = layer("342-camera-dof-800-30.aep", "Camera 1");
    assert_eq!(v(&c, "cameraOptions/dof"), Value::Bool(true));
    close(&v(&c, "cameraOptions/focusDistance"), &[800.0]);
    close(&v(&c, "cameraOptions/aperture"), &[30.0]);
    // 535-537: a camera above the 3D solid of 486.
    let c = layer("535-camera-position-on-3d-rotx.aep", "K");
    close(&v(&c, "transform/position"), &[100.0, 200.0, -1000.0]);
    assert_eq!(c.auto_orient, AutoOrient::TowardsPointOfInterest);
    assert_eq!(layer("536-camera-one-node-position-on-3d-rotx.aep", "K").auto_orient, AutoOrient::Off);
}

#[test]
fn spot_and_point_lights() {
    let l = layer("254-light-spot-intensity-50.aep", "Light 1");
    assert!(matches!(l.source, LayerSource::Light { kind: LightKind::Spot }));
    close(&v(&l, "lightOptions/intensity"), &[50.0]);
    close(&v(&l, "transform/poi"), &[960.0, 540.0, 0.0]);
    close(&v(&l, "transform/position"), &[0.0, 0.0, -1333.3333]);
    assert_eq!(l.auto_orient, AutoOrient::TowardsPointOfInterest);
    close(&v(&layer("343-light-spot-color-red.aep", "Light 1"), "lightOptions/color"), &[1.0, 0.0, 0.0, 1.0]);
    close(&v(&layer("344-light-spot-coneangle-60.aep", "Light 1"), "lightOptions/coneAngle"), &[60.0]);
    close(&v(&layer("345-light-spot-conefeather-30.aep", "Light 1"), "lightOptions/coneFeather"), &[30.0]);
    // 252: a point light stores its cone as default-state records, which hold nothing.
    let l = layer("252-light-point.aep", "Light 1");
    assert!(matches!(l.source, LayerSource::Light { kind: LightKind::Point }));
    close(&v(&l, "lightOptions/intensity"), &[100.0]);
    assert_eq!(l.auto_orient, AutoOrient::Off);
}

#[test]
fn cameras_count_in_the_report() {
    let (_, r) = load("537-camera-default-on-3d-rotx.aep");
    assert_eq!((r.report.imported.cameras, r.report.imported.lights, r.report.total_losses()), (1, 0, 0));
}
