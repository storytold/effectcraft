//! Masks against the synthetic mask fixtures of `effectcraft-aep` (on the red 1920x1080 solid `L`
//! of composition `C`, 25 fps, unless noted). Expected values are those the fixtures were made with
//! (`MANIFEST.md` of the corpus; After Effects read the paths back as listed there).

mod common;

use common::{comp, load};
use effectcraft_aep_import::{LossCategory, analyze};
use effectcraft_keyframe::{Ease, Interp, ShapePath, Value};
use effectcraft_project::{GroupKind, Layer, MaskMode, PropGroup};
use effectcraft_time::{TICKS_PER_SECOND, Tick};

/// Every mask fixture in the repository.
const MASK_FIXTURES: [&str; 17] = [
    "090-mask-rect.aep",
    "091-mask-rect-props.aep",
    "093-mask-path-2keys.aep",
    "208-key-mask-feather-2key-linear.aep",
    "232-mask-ellipse.aep",
    "233-mask-open-path.aep",
    "234-mask-mode-intersect.aep",
    "235-mask-mode-lighten.aep",
    "236-mask-mode-darken.aep",
    "237-mask-mode-difference.aep",
    "306-key-maskfeather-bezier-ease.aep",
    "309-solid-100x100-mask-rect.aep",
    "328-mask-two-ellipse-rect.aep",
    "329-mask-ellipse-mode-none.aep",
    "900-mask-added-no-path.aep",
    "901-mask-no-path-expansion-m100.aep",
    "000-base.aep",
];

/// The rectangle 800x400 at the composition centre (090), clockwise from the top left.
const RECT: [[f64; 2]; 4] = [[560.0, 340.0], [1360.0, 340.0], [1360.0, 740.0], [560.0, 740.0]];

fn layer_l(name: &str) -> Layer {
    let (p, r) = load(name);
    let (_, c) = comp(&p, &r, "C");
    c.layers.iter().find(|l| l.name == "L").unwrap_or_else(|| panic!("{name}: no layer L")).clone()
}

fn masks(l: &Layer) -> Vec<PropGroup> {
    l.masks().expect("masks group").groups().cloned().collect()
}

fn kind(g: &PropGroup) -> (MaskMode, bool) {
    match g.kind {
        GroupKind::Mask { mode, inverted, .. } => (mode, inverted),
        _ => panic!("{} is not a mask", g.name),
    }
}

fn path(g: &PropGroup) -> ShapePath {
    match &g.get("path").expect("path").value {
        Value::Path(p) => p.clone(),
        other => panic!("{other:?}"),
    }
}

fn near(a: [f64; 2], b: [f64; 2], tol: f64) {
    assert!((a[0] - b[0]).abs() < tol && (a[1] - b[1]).abs() < tol, "{a:?} != {b:?}");
}

/// Vertices within float32 noise, no tangents.
fn assert_polygon(p: &ShapePath, expected: &[[f64; 2]]) {
    assert_eq!(p.len(), expected.len());
    for (v, e) in p.vertices.iter().zip(expected) {
        near(*v, *e, 1e-3);
    }
    for t in p.in_tangents.iter().chain(&p.out_tangents) {
        near(*t, [0.0, 0.0], 1e-3);
    }
}

fn scalar(g: &PropGroup, id: &str) -> f64 {
    match g.get(id).expect(id).value {
        Value::Scalar(v) => v,
        ref other => panic!("{other:?}"),
    }
}

fn vec2(v: &Value) -> [f64; 2] {
    match v {
        Value::Vec2(v) => *v,
        other => panic!("{other:?}"),
    }
}

fn secs(s: f64) -> Tick {
    Tick((s * TICKS_PER_SECOND as f64).round() as i64)
}

#[test]
fn every_mask_fixture_imports_without_losses() {
    for name in MASK_FIXTURES {
        let (p, r) = load(name);
        assert!(r.report.is_lossless(), "{name}: {:?}", r.report.losses);
        let (_, c) = comp(&p, &r, "C");
        let n: usize = c.layers.iter().map(|l| l.masks().map_or(0, |m| m.groups().count())).sum();
        assert_eq!(r.report.imported.masks, n, "{name}");
        let expected = match name {
            "000-base.aep" => 0,
            "328-mask-two-ellipse-rect.aep" => 2,
            _ => 1,
        };
        assert_eq!(n, expected, "{name}");
    }
}

#[test]
fn rectangle_with_defaults_090() {
    let l = layer_l("090-mask-rect.aep");
    let m = masks(&l);
    assert_eq!(m.len(), 1);
    assert_eq!(m[0].name, "Mask 1");
    assert_eq!(kind(&m[0]), (MaskMode::Add, false));
    let p = path(&m[0]);
    assert!(p.closed);
    assert_polygon(&p, &RECT);
    assert_eq!(vec2(&m[0].get("feather").unwrap().value), [0.0, 0.0]);
    assert_eq!(scalar(&m[0], "opacity"), 100.0);
    assert_eq!(scalar(&m[0], "expansion"), 0.0);
    assert!(m[0].get("path").unwrap().keys.is_empty());
}

/// A mask whose path was never set stores no Mask Path entry (900); After Effects reads it back as
/// the source rectangle `[0,0] [0,1080] [1920,1080] [1920,0]` and renders it so (901: expansion
/// -100 leaves a 100 px border).
#[test]
fn a_mask_without_a_stored_path_is_the_source_rectangle_900_901() {
    for (name, expansion) in [("900-mask-added-no-path.aep", 0.0), ("901-mask-no-path-expansion-m100.aep", -100.0)] {
        let m = masks(&layer_l(name));
        assert_eq!(m.len(), 1, "{name}");
        assert_eq!(kind(&m[0]), (MaskMode::Add, false), "{name}");
        let p = path(&m[0]);
        assert!(p.closed, "{name}");
        assert_eq!(p.vertices, [[0.0, 0.0], [0.0, 1080.0], [1920.0, 1080.0], [1920.0, 0.0]], "{name}");
        assert!(p.in_tangents.iter().chain(&p.out_tangents).all(|t| *t == [0.0, 0.0]), "{name}");
        assert_eq!(scalar(&m[0], "expansion"), expansion, "{name}");
        assert_eq!(scalar(&m[0], "opacity"), 100.0, "{name}");
    }
}

#[test]
fn feather_opacity_expansion_mode_and_invert_091() {
    let m = masks(&layer_l("091-mask-rect-props.aep"));
    assert_eq!(kind(&m[0]), (MaskMode::Subtract, true));
    assert_eq!(vec2(&m[0].get("feather").unwrap().value), [20.0, 20.0]);
    assert_eq!(scalar(&m[0], "opacity"), 50.0);
    assert_eq!(scalar(&m[0], "expansion"), 5.0);
    assert_polygon(&path(&m[0]), &RECT);
}

#[test]
fn animated_path_093() {
    let m = masks(&layer_l("093-mask-path-2keys.aep"));
    let prop = m[0].get("path").unwrap();
    assert_eq!(prop.keys.len(), 2);
    assert_eq!((prop.keys[0].time, prop.keys[1].time), (Tick::ZERO, secs(1.0)));
    for k in &prop.keys {
        assert_eq!((k.in_interp, k.out_interp), (Interp::Linear, Interp::Linear));
    }
    let shape = |i: usize| match &prop.keys[i].value {
        Value::Path(p) => p.clone(),
        other => panic!("{other:?}"),
    };
    assert_polygon(&shape(0), &[[760.0, 440.0], [1160.0, 440.0], [1160.0, 640.0], [760.0, 640.0]]);
    assert_polygon(&shape(1), &RECT);
    // The static value is the first key's.
    assert_eq!(prop.value, prop.keys[0].value);
}

#[test]
fn feather_keys_linear_208() {
    let m = masks(&layer_l("208-key-mask-feather-2key-linear.aep"));
    assert_eq!(kind(&m[0]), (MaskMode::Add, false));
    let f = m[0].get("feather").unwrap();
    assert_eq!(f.keys.len(), 2);
    assert_eq!((f.keys[0].time, f.keys[1].time), (Tick::ZERO, secs(1.0)));
    assert_eq!(vec2(&f.keys[0].value), [0.0, 0.0]);
    assert_eq!(vec2(&f.keys[1].value), [40.0, 20.0]);
    for k in &f.keys {
        assert_eq!((k.in_interp, k.out_interp), (Interp::Linear, Interp::Linear));
    }
    assert_polygon(&path(&m[0]), &RECT);
}

#[test]
fn feather_keys_bezier_ease_306() {
    let m = masks(&layer_l("306-key-maskfeather-bezier-ease.aep"));
    let f = m[0].get("feather").unwrap();
    assert_eq!(f.keys.len(), 2);
    assert_eq!((f.keys[0].time, f.keys[1].time), (Tick::ZERO, secs(2.0)));
    assert_eq!(vec2(&f.keys[1].value), [50.0, 30.0]);
    let (k1, k2) = (&f.keys[0], &f.keys[1]);
    assert_eq!((k1.out_interp, k2.in_interp), (Interp::Bezier, Interp::Bezier));
    // Speed in pixels per second, influence a fraction, one ease per dimension. The first key's
    // incoming and the last key's outgoing ease mean nothing and are dropped.
    let check = |eases: &[Ease], speed: f64, influence: f64| {
        assert_eq!(eases.len(), 2);
        for e in eases {
            assert!((e.speed - speed).abs() < 1e-9 && (e.influence - influence).abs() < 1e-9, "{e:?}");
        }
    };
    check(&k1.out_ease, 15.0, 0.66);
    check(&k2.in_ease, 8.0, 0.66);
    assert!(k1.in_ease.is_empty() && k2.out_ease.is_empty());
}

#[test]
fn ellipse_tangents_232() {
    let m = masks(&layer_l("232-mask-ellipse.aep"));
    let p = path(&m[0]);
    assert!(p.closed);
    // After Effects read the path back as vertices [960,340],[1360,540],[960,740],[560,540] and
    // tangents of 0.5523 times the radius: the same as an EffectCraft ellipse.
    let e = ShapePath::ellipse([960.0, 540.0], 800.0, 400.0);
    assert_eq!(p.len(), 4);
    for i in 0..4 {
        near(p.vertices[i], e.vertices[i], 1e-3);
        near(p.in_tangents[i], e.in_tangents[i], 1e-2);
        near(p.out_tangents[i], e.out_tangents[i], 1e-2);
    }
    near(p.in_tangents[0], [-220.913879394531, 0.0], 1e-3);
    near(p.out_tangents[1], [0.0, 110.456970214844], 1e-3);
}

#[test]
fn open_path_233() {
    let p = path(&masks(&layer_l("233-mask-open-path.aep"))[0]);
    assert!(!p.closed);
    assert_polygon(&p, &[[500.0, 300.0], [960.0, 700.0], [1400.0, 300.0]]);
}

#[test]
fn every_mode_234_to_237_and_329() {
    for (name, want) in [
        ("090-mask-rect.aep", MaskMode::Add),
        ("234-mask-mode-intersect.aep", MaskMode::Intersect),
        ("235-mask-mode-lighten.aep", MaskMode::Lighten),
        ("236-mask-mode-darken.aep", MaskMode::Darken),
        ("237-mask-mode-difference.aep", MaskMode::Difference),
        ("329-mask-ellipse-mode-none.aep", MaskMode::None),
    ] {
        let m = masks(&layer_l(name));
        assert_eq!(kind(&m[0]), (want, false), "{name}");
    }
}

#[test]
fn vertices_are_in_layer_pixels_309() {
    // A 60x40 rectangle centred on a 100x100 solid: the stored box is normalised to the layer size,
    // so the vertices are layer pixels, not composition pixels.
    let (p, r) = load("309-solid-100x100-mask-rect.aep");
    let (_, c) = comp(&p, &r, "C");
    let l = c.layers.iter().find(|l| !masks(l).is_empty()).expect("a layer with a mask");
    assert_polygon(&path(&masks(l)[0]), &[[20.0, 30.0], [80.0, 30.0], [80.0, 70.0], [20.0, 70.0]]);
}

#[test]
fn two_masks_in_stack_order_328() {
    let m = masks(&layer_l("328-mask-two-ellipse-rect.aep"));
    assert_eq!(m.len(), 2);
    assert_eq!((m[0].name.as_str(), m[1].name.as_str()), ("Mask 1", "Mask 2"));
    assert_eq!((kind(&m[0]), kind(&m[1])), ((MaskMode::Add, false), (MaskMode::Add, false)));
    assert_eq!(path(&m[0]).len(), 4);
    near(path(&m[0]).vertices[0], [960.0, 340.0], 1e-3);
    assert_polygon(&path(&m[1]), &[[500.0, 350.0], [900.0, 350.0], [900.0, 650.0], [500.0, 650.0]]);
    // Distinct ids and the default colour cycle.
    assert_ne!(m[0].uid, m[1].uid);
    let color = |g: &PropGroup| match g.kind {
        GroupKind::Mask { color, .. } => color,
        _ => [0; 3],
    };
    assert_ne!(color(&m[0]), color(&m[1]));
}

#[test]
fn the_dry_run_counts_masks_and_lists_the_mask_limitations() {
    let report = analyze(&common::fixture_bytes("091-mask-rect-props.aep"), &|_| false).unwrap();
    assert_eq!(report.count(LossCategory::Masks), 0);
    assert_eq!(report.imported.masks, 1);
    assert!(report.limitations.iter().any(|l| l.contains("feather falloff")));
}
