//! Keys on time-stretched layers (cases 641-643): the key times count source time in the layer's
//! time scale (the composition's x stretch at 100 % or more, the composition's below).

mod common;

use common::*;
use effectcraft_aep::{PropNode, parse};
use effectcraft_aep_export::LossCategory;
use effectcraft_keyframe::{Interp, Value};
use effectcraft_project::{LayerSource, Project};
use effectcraft_time::FrameRate;

fn fixture_bytes(name: &str) -> Vec<u8> {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../aep/tests/fixtures").join(name);
    std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// (stretch numerator, denominator, start units, Position time scale, key times) of the first
/// layer of the first composition.
fn position_keys(bytes: &[u8]) -> (i32, u32, i32, u32, Vec<i32>) {
    let file = parse(bytes).expect("parse");
    let view = effectcraft_aep::Project::new(&file).expect("project");
    let comp = view.all_items().into_iter().find_map(|i| i.comp()).expect("comp");
    let layer = comp.layers().next().expect("layer");
    let h = layer.header().expect("header");
    let Some(PropNode::Property(pos)) = layer.properties().and_then(|g| g.find(&["ADBE Transform Group", "ADBE Position"])) else { panic!("position") };
    let times = pos.keys().expect("keys").keyframes().filter_map(|k| k.time_units()).collect();
    (h.stretch, h.stretch_den, h.start.num as i32, pos.time_scale().expect("time scale"), times)
}

fn reimport(bytes: &[u8]) -> Project {
    let mut p = Project::default();
    let none = |_: &str| false;
    effectcraft_aep_import::import(
        &mut p,
        bytes,
        &effectcraft_aep_import::ImportOptions { allow_losses: true, ..effectcraft_aep_import::ImportOptions::new(&none) },
    )
    .expect("import");
    p
}

#[test]
fn keys_on_stretched_layers_are_written_as_after_effects_stores_them() {
    for name in ["641-stretch-200-keys-position-1s-2s.aep", "642-stretch-50-keys-position-1s-2s.aep", "643-stretch-200-start-1-keys-position-2s-3s.aep"] {
        let bytes = fixture_bytes(name);
        let p1 = reimport(&bytes);
        let r = export_all(&p1);
        assert!(r.report.is_lossless(), "{name}: {:?}", r.report.losses);
        assert_eq!(position_keys(&r.bytes), position_keys(&bytes), "{name}");
        // A second import gives the same layer and keys.
        let p2 = reimport(&r.bytes);
        let layer = |p: &Project| p.comps().next().map(|(_, c)| c.layers[0].clone()).expect("layer");
        let (a, b) = (layer(&p1), layer(&p2));
        assert_eq!((a.stretch, a.start_time, a.in_point, a.out_point), (b.stretch, b.start_time, b.in_point, b.out_point), "{name}");
        let keys =
            |l: &effectcraft_project::Layer| l.props.prop("transform/position").map(|p| p.keys.iter().map(|k| (k.time, k.value.clone())).collect::<Vec<_>>());
        assert_eq!(keys(&a), keys(&b), "{name}");
    }
    // What the cases hold (MANIFEST): 200 % keys at comp 1 s / 2 s = units 25 600 / 51 200 at
    // time scale 51 200; 50 % at time scale 25 600 = 51 200 / 102 400.
    assert_eq!(position_keys(&fixture_bytes("641-stretch-200-keys-position-1s-2s.aep")), (2, 1, 0, 51200, vec![25600, 51200]));
    assert_eq!(position_keys(&fixture_bytes("642-stretch-50-keys-position-1s-2s.aep")), (1, 2, 0, 25600, vec![51200, 102400]));
}

#[test]
fn a_comp_time_survives_any_stretch() {
    // Keys built in code on a 150 % layer starting at 0.4 s: the key at layer time 2 s is at
    // comp 0.4 + 3 s, in source units 2 x 38 400.
    let mut p = Project::default();
    let c = add_comp(&mut p, "C", 640, 360, FrameRate::new(25, 1), 10.0, None);
    let s = add_solid(&mut p, "S", 100, 100, [1.0, 0.0, 0.0], None);
    add_layer(&mut p, c, "S", LayerSource::Solid { item: s }, (100, 100));
    with_layer(&mut p, c, 0, |l| {
        l.stretch = 150.0;
        l.start_time = secs(0.4);
        let pos = l.props.prop_mut("transform/position").expect("pos");
        pos.keys = vec![key(1.0, Value::Vec3([0.0, 0.0, 0.0])), key(2.0, Value::Vec3([100.0, 0.0, 0.0]))];
    });
    let r = export_all(&p);
    assert!(r.report.is_lossless(), "{:?}", r.report.losses);
    let (n, d, _, ts, times) = position_keys(&r.bytes);
    assert_eq!((n, d, ts, times), (3, 2, 38400, vec![38400, 76800]));
    // Bezier ease on a stretched layer is written but reported (its speed unit is not sampled).
    with_layer(&mut p, c, 0, |l| {
        let pos = l.props.prop_mut("transform/position").expect("pos");
        pos.keys[0].out_interp = Interp::Bezier;
    });
    assert_eq!(export_all(&p).report.count(LossCategory::ApproximatedKeyframes), 1);
    assert_eq!(export_all(&p).report.count(LossCategory::TimeStretch), 0);
}

/// (stretch numerator, denominator, start, in, out as stored; Opacity time scale and key times).
fn reversed_header(bytes: &[u8]) -> (i32, u32, i64, i64, i64, u32, Vec<i32>) {
    let file = parse(bytes).expect("parse");
    let view = effectcraft_aep::Project::new(&file).expect("project");
    let comp = view.all_items().into_iter().find_map(|i| i.comp()).expect("comp");
    let layer = comp.layers().next().expect("layer");
    let h = layer.header().expect("header");
    let (ts, times) = match layer.properties().and_then(|g| g.find(&["ADBE Transform Group", "ADBE Opacity"])) {
        Some(PropNode::Property(p)) if p.is_animated() => {
            (p.time_scale().unwrap_or(0), p.keys().expect("keys").keyframes().filter_map(|k| k.time_units()).collect())
        }
        _ => (0, Vec::new()),
    };
    (h.stretch, h.stretch_den, h.start.num, h.in_relative.num, h.out_relative.num, ts, times)
}

#[test]
fn a_reversed_layer_is_written_as_after_effects_stores_it() {
    // 981-983: a negative stretch numerator, the start at the later end, in / out relative in
    // source time, keys in the composition's time scale.
    for name in ["981-layer-stretch-minus100-start5.aep", "982-layer-stretch-minus100-opacity-keys.aep", "983-layer-stretch-minus50-opacity-keys.aep"] {
        let bytes = fixture_bytes(name);
        let p1 = reimport(&bytes);
        let l = p1.comps().next().map(|(_, c)| c.layers[0].clone()).expect("layer");
        assert!(l.stretch < 0.0 && l.in_point < l.out_point, "{name}: {} {:?} {:?}", l.stretch, l.in_point, l.out_point);
        let r = export_all(&p1);
        assert!(r.report.is_lossless(), "{name}: {:?}", r.report.losses);
        assert_eq!(reversed_header(&r.bytes), reversed_header(&bytes), "{name}");
        let p2 = reimport(&r.bytes);
        let m = p2.comps().next().map(|(_, c)| c.layers[0].clone()).expect("layer");
        assert_eq!((l.stretch, l.start_time, l.in_point, l.out_point), (m.stretch, m.start_time, m.in_point, m.out_point), "{name}");
    }
}
