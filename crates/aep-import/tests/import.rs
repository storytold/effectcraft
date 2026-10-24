//! The importer against the synthetic fixtures of `effectcraft-aep` (one change per file on top of
//! composition `C`: 1920x1080 at 25 fps, 5 s, with one red solid layer `L`).

mod common;

use common::{comp, load, try_import};
use effectcraft_aep_import::{AepImportError, ImportReport, LossCategory, analyze};
use effectcraft_color::{BlendMode, Label};
use effectcraft_keyframe::{Ease, Interp, Value};
use effectcraft_project::{Comp, FootageKind, ItemKind, Layer, LayerSource, Project, Quality, Renderer};
use effectcraft_time::{FrameRate, TICKS_PER_SECOND, Tick};

const SEC: i64 = TICKS_PER_SECOND;

fn secs(s: f64) -> Tick {
    Tick((s * SEC as f64).round() as i64)
}

fn layer<'c>(c: &'c Comp, name: &str) -> &'c Layer {
    c.layers.iter().find(|l| l.name == name).unwrap_or_else(|| panic!("no layer {name}"))
}

fn only_layer(project: &Project, r: &effectcraft_aep_import::ImportResult) -> Layer {
    let (_, c) = comp(project, r, "C");
    layer(c, "L").clone()
}

fn value(l: &Layer, path: &str) -> Value {
    l.props.prop(path).unwrap_or_else(|| panic!("no property {path}")).value.clone()
}

fn vec3(l: &Layer, path: &str) -> [f64; 3] {
    match value(l, path) {
        Value::Vec3(v) => v,
        other => panic!("{path}: {other:?}"),
    }
}

fn scalar(l: &Layer, path: &str) -> f64 {
    match value(l, path) {
        Value::Scalar(v) => v,
        other => panic!("{path}: {other:?}"),
    }
}

fn close(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-9, "{a} != {b}");
}

fn close3(a: [f64; 3], b: [f64; 3]) {
    for (x, y) in a.iter().zip(b) {
        close(*x, y);
    }
}

// ---------------------------------------------------------------- items and composition

#[test]
fn base_project_items_comp_and_layer() {
    let (p, r) = load("000-base.aep");
    assert!(r.report.is_lossless(), "{:?}", r.report.losses);
    // The wrapper folder, then Solids (folder), the solid L and the composition C inside it.
    let root = p.item(r.folder).unwrap();
    assert_eq!((root.name.as_str(), root.parent), ("After Effects Project", None));
    assert_eq!(r.items.len(), 4);
    let solids = p.children(Some(r.folder)).into_iter().find(|i| i.name == "Solids").unwrap();
    assert!(solids.is_folder());
    let kids = p.children(Some(solids.id));
    assert_eq!(kids.len(), 1);
    match &kids[0].kind {
        ItemKind::Solid(s) => assert_eq!((kids[0].name.as_str(), s.color, s.width, s.height), ("L", [1.0, 0.0, 0.0], 1920, 1080)),
        other => panic!("{other:?}"),
    }
    let im = &r.report.imported;
    assert_eq!((im.folders, im.compositions, im.solids, im.layers, im.solid_layers, im.keyframes), (1, 1, 1, 1, 1, 0));

    let (id, c) = comp(&p, &r, "C");
    assert_eq!(p.item(id).unwrap().parent, Some(r.folder));
    assert_eq!((c.width, c.height), (1920, 1080));
    assert_eq!(c.frame_rate, FrameRate::new(25, 1));
    assert_eq!(c.duration, Tick(5 * SEC));
    assert_eq!(c.work_area, (Tick::ZERO, Tick(5 * SEC)));
    assert_eq!(c.background, [0.0, 0.0, 0.0]);
    assert_eq!((c.pixel_aspect, c.shutter_angle, c.enable_motion_blur, c.renderer), (1.0, 180.0, false, Renderer::Classic3D));
    assert_eq!(c.layers.len(), 1);

    let l = &c.layers[0];
    assert_eq!(l.name, "L");
    assert!(matches!(l.source, LayerSource::Solid { .. }));
    assert_eq!((l.in_point, l.out_point, l.start_time, l.stretch), (Tick::ZERO, Tick(5 * SEC), Tick::ZERO, 100.0));
    assert_eq!((l.switches.video, l.switches.audio, l.switches.effects), (true, true, true));
    assert_eq!((l.switches.solo, l.switches.locked, l.switches.shy, l.switches.three_d, l.switches.adjustment), (false, false, false, false, false));
    assert_eq!((l.blend_mode, l.parent, l.label), (BlendMode::Normal, None, Label::Red));
    // Absent properties are the defaults: centred position and anchor, 100 %.
    close3(vec3(l, "transform/position"), [960.0, 540.0, 0.0]);
    close3(vec3(l, "transform/anchor"), [960.0, 540.0, 0.0]);
    close3(vec3(l, "transform/scale"), [100.0, 100.0, 100.0]);
    close(scalar(l, "transform/rotation"), 0.0);
    close(scalar(l, "transform/opacity"), 100.0);
}

#[test]
fn empty_project_imports_just_the_folder() {
    let (p, r) = load("001-empty.aep");
    assert!(r.comps.is_empty());
    assert!(p.item(r.folder).unwrap().is_folder());
    assert!(r.report.is_lossless());
}

#[test]
fn imports_into_an_existing_project_without_touching_it() {
    let mut p = Project::default();
    let first = try_import(&mut p, "000-base.aep", false).unwrap();
    let before = p.items.len();
    let second = try_import(&mut p, "000-base.aep", false).unwrap();
    assert_eq!(p.items.len(), before * 2);
    assert_ne!(first.folder, second.folder);
    // Layer, item and property ids stay unique across both imports.
    let ids: Vec<_> = [&first, &second].iter().flat_map(|r| r.comps.iter().filter_map(|c| p.comp(*c)).flat_map(|c| c.layers.iter().map(|l| l.id))).collect();
    assert_eq!(ids.len(), 2);
    assert_ne!(ids[0], ids[1]);
}

#[test]
fn composition_settings() {
    let cases: [(&str, &dyn Fn(&Comp)); 10] = [
        ("030-comp-size-1280x720.aep", &|c| {
            assert_eq!((c.width, c.height), (1280, 720));
        }),
        ("031-comp-fps30.aep", &|c| assert_eq!(c.frame_rate, FrameRate::new(30, 1))),
        ("032-comp-duration10.aep", &|c| {
            assert_eq!(c.duration, Tick(10 * SEC));
            assert_eq!(c.work_area, (Tick::ZERO, Tick(10 * SEC)));
        }),
        ("033-comp-bg-green.aep", &|c| assert_ne!(c.background, [0.0, 0.0, 0.0])),
        ("034-comp-shutter90.aep", &|c| {
            assert_eq!(c.shutter_angle, 90.0);
            // The stored phase (0 in this file), not an assumed centring.
            assert_eq!(c.shutter_phase, 0.0);
        }),
        ("035-comp-workarea-1-2.aep", &|c| assert_eq!(c.work_area, (Tick(SEC), Tick(3 * SEC)))),
        ("037-comp-pixelaspect2.aep", &|c| assert_eq!(c.pixel_aspect, 2.0)),
        ("038-comp-motionblur.aep", &|c| assert!(c.enable_motion_blur)),
        ("039-comp-renderer-advanced3d.aep", &|c| assert_eq!(c.renderer, Renderer::Advanced3D)),
        ("231-comp-bg-grey-0.5.aep", &|c| assert_eq!(c.background, [128.0 / 255.0; 3])),
    ];
    for (name, check) in cases {
        let (p, r) = load(name);
        let (_, c) = comp(&p, &r, "C");
        check(c);
    }
    // Green: 0, 1, 0 up to the 8-bit storage.
    let (p, r) = load("033-comp-bg-green.aep");
    let (_, c) = comp(&p, &r, "C");
    assert_eq!(c.background[0], 0.0);
    assert!(c.background[1] > 0.4, "{:?}", c.background);
}

#[test]
fn resized_comp_keeps_the_stored_position() {
    // The comp was resized without moving the layer: After Effects then stores the old centre,
    // while a layer still at the default sits at the new centre.
    let (p, r) = load("030-comp-size-1280x720.aep");
    let l = only_layer(&p, &r);
    close3(vec3(&l, "transform/position"), [960.0, 540.0, 0.0]);
}

#[test]
fn ntsc_frame_rates_are_exact() {
    let (p, r) = load("229-comp-fps-29.97.aep");
    let (_, c) = comp(&p, &r, "C");
    assert_eq!(c.frame_rate, FrameRate::FPS_29_97);
    // 120000 units of 1/23976 s: 150 frames, 5.005 s, and a whole number of frames.
    assert_eq!(c.duration, FrameRate::FPS_29_97.tick_of(150));
    assert_eq!(c.frame_rate.frame_at(c.duration), 150);
    assert_eq!(c.work_area, (Tick::ZERO, c.duration));

    let (p, r) = load("230-comp-fps-23.976.aep");
    let (_, c) = comp(&p, &r, "C");
    assert_eq!(c.frame_rate, FrameRate::FPS_23_976);
    assert_eq!(c.duration, FrameRate::FPS_23_976.tick_of(120));
    assert_eq!(c.frame_rate.frame_at(c.duration), 120);
}

// ---------------------------------------------------------------- layers

#[test]
fn layer_switches_timing_and_blend() {
    let sw = |name: &str| {
        let (p, r) = load(name);
        comp(&p, &r, "C").1.layers[0].clone()
    };
    assert!(sw("012-layer-shy.aep").switches.shy);
    assert!(sw("013-layer-locked.aep").switches.locked);
    assert!(sw("014-layer-solo.aep").switches.solo);
    assert!(sw("015-layer-3d.aep").switches.three_d);
    assert!(sw("017-layer-motionblur.aep").switches.motion_blur);
    assert!(sw("024-layer-collapse.aep").switches.collapse);
    assert!(sw("025-layer-adjustment.aep").switches.adjustment);
    assert!(sw("026-layer-guide.aep").switches.guide);
    assert_eq!(sw("023-layer-quality-draft.aep").switches.quality, Quality::Draft);
    assert!(!sw("222-layer-video-off.aep").switches.video);
    assert!(!sw("223-layer-audio-off.aep").switches.audio);
    assert!(!sw("224-layer-effects-off.aep").switches.effects);
    assert_eq!(sw("010-layer-name.aep").name, "Renamed");

    for (name, mode) in [
        ("016-layer-blend-multiply.aep", BlendMode::Multiply),
        ("225-layer-blend-screen.aep", BlendMode::Screen),
        ("226-layer-blend-add.aep", BlendMode::Add),
        ("227-layer-blend-overlay.aep", BlendMode::Overlay),
        ("228-layer-blend-difference.aep", BlendMode::Difference),
    ] {
        assert_eq!(sw(name).blend_mode, mode, "{name}");
    }

    // Stretch 200 %: the out point is the start plus the relative out point times the stretch.
    let l = sw("018-layer-stretch200.aep");
    assert_eq!((l.stretch, l.in_point, l.out_point), (200.0, Tick::ZERO, Tick(10 * SEC)));
    // In point 1 s, out 6 s.
    let l = sw("019-layer-inpoint1.aep");
    assert_eq!((l.in_point, l.out_point, l.start_time), (Tick(SEC), Tick(6 * SEC), Tick::ZERO));
    // Start time 0.5 s moves the whole span.
    let l = sw("021-layer-starttime0.5.aep");
    assert_eq!((l.start_time, l.in_point, l.out_point), (Tick(SEC / 2), Tick(SEC / 2), secs(5.5)));
}

#[test]
fn null_layer_and_parent() {
    let (p, r) = load("022-layer-parent-null.aep");
    assert!(r.report.is_lossless(), "{:?}", r.report.losses);
    let (_, c) = comp(&p, &r, "C");
    assert_eq!(c.layers.len(), 2);
    let null = layer(c, "Null");
    let child = layer(c, "L");
    assert!(matches!(null.source, LayerSource::Null));
    assert_eq!(child.parent, Some(null.id));
    assert_eq!(null.parent, None);
    close3(vec3(child, "transform/position"), [0.0, 0.0, 0.0]);
    // The null's placeholder solid is not an item: only L's solid exists.
    let solids = p.items.values().filter(|i| matches!(i.kind, ItemKind::Solid(_))).count();
    assert_eq!(solids, 1);
    let im = &r.report.imported;
    assert_eq!((im.layers, im.null_layers, im.solid_layers), (2, 1, 1));
}

#[test]
fn adjustment_layer_is_counted() {
    let (_, r) = load("025-layer-adjustment.aep");
    let im = &r.report.imported;
    assert_eq!((im.adjustment_layers, im.solid_layers), (1, 0));
}

#[test]
fn solid_colour_and_size_and_anchor_scaling() {
    let (p, r) = load("259-solid-blue-0-0-1.aep");
    assert!(p.items.values().any(|i| matches!(&i.kind, ItemKind::Solid(s) if s.color == [0.0, 0.0, 1.0])));
    // A 400x300 solid with Anchor Point 100, 50 stores 0.25, 1/6: back to pixels.
    let (p, r2) = load("212-anchor-400x300-solid-100-50.aep");
    let (_, c) = comp(&p, &r2, "C");
    let s400 = layer(c, "S400");
    close3(vec3(s400, "transform/anchor"), [100.0, 50.0, 0.0]);
    // The 100x100 solid anchor 10, 20.
    let (p, r3) = load("211-anchor-100x100-solid-10-20.aep");
    let (_, c) = comp(&p, &r3, "C");
    let l = c.layers.iter().find(|l| vec3(l, "transform/anchor")[0] < 50.0).unwrap();
    close3(vec3(l, "transform/anchor"), [10.0, 20.0, 0.0]);
    let _ = r;
}

#[test]
fn precomposition_layer_links_the_comp() {
    let (p, r) = load("124-precomp-Pre.aep");
    assert!(r.report.is_lossless(), "{:?}", r.report.losses);
    let (pre_id, pre) = comp(&p, &r, "Pre");
    assert_eq!(pre.layers.len(), 1);
    let (_, c) = comp(&p, &r, "C");
    assert_eq!(c.layers.len(), 1);
    assert_eq!(c.layers[0].name, "Pre");
    assert_eq!(c.layers[0].source, LayerSource::Comp { item: pre_id });
    assert_eq!(r.report.imported.precomp_layers, 1);
    assert_eq!(c.layers[0].label, Label::Sandstone);
}

#[test]
fn footage_is_a_placeholder_unless_the_file_exists() {
    let (p, r) = load("126-footage-png-layer.aep");
    assert!(r.report.is_lossless());
    assert_eq!(r.report.missing_footage, ["C:\\fixtures\\dirdirdirdirdirdirdi\\test8x8.png"]);
    let item = p.items.values().find(|i| i.name == "test8x8.png").unwrap();
    match &item.kind {
        ItemKind::Footage(f) => {
            assert_eq!(
                (f.path.as_str(), f.width, f.height, f.missing, f.kind),
                ("C:\\fixtures\\dirdirdirdirdirdirdi\\test8x8.png", 8, 8, true, FootageKind::Still)
            )
        }
        other => panic!("{other:?}"),
    }
    let (_, c) = comp(&p, &r, "C");
    let l = layer(c, "png");
    assert_eq!(l.source, LayerSource::Footage { item: item.id });
    assert_eq!(r.report.imported.footage_layers, 1);

    // With the file present the item is linked, not missing, and the report lists nothing missing.
    let mut project = Project::default();
    let bytes = common::fixture_bytes("126-footage-png-layer.aep");
    let exists = |path: &str| path.ends_with("test8x8.png");
    let opts = effectcraft_aep_import::ImportOptions::new(&exists);
    let r = effectcraft_aep_import::import(&mut project, &bytes, &opts).unwrap();
    assert!(r.report.missing_footage.is_empty());
    let item = project.items.values().find(|i| i.name == "test8x8.png").unwrap();
    assert!(matches!(&item.kind, ItemKind::Footage(f) if !f.missing));
}

// ---------------------------------------------------------------- transform values

#[test]
fn static_transform_values_have_the_right_units() {
    let l = |n: &str| {
        let (p, r) = load(n);
        only_layer(&p, &r)
    };
    let a = l("040-static-position-100-200.aep");
    close3(vec3(&a, "transform/position"), [100.0, 200.0, 0.0]);
    assert!(a.props.prop("transform/position").unwrap().keys.is_empty());
    close3(vec3(&l("041-static-scale-50.aep"), "transform/scale"), [50.0, 50.0, 100.0]);
    close(scalar(&l("042-static-rotation-45.aep"), "transform/rotation"), 45.0);
    close(scalar(&l("043-static-opacity-25.aep"), "transform/opacity"), 25.0);
    // 10, 20 on a 1920x1080 solid is stored as 10/1920, 20/1080.
    close3(vec3(&l("044-static-anchor-10-20.aep"), "transform/anchor"), [10.0, 20.0, 0.0]);
}

#[test]
fn position_keys_with_time_value_and_interpolation() {
    let (p, r) = load("055-key-position-3key-inserted-t2.5.aep");
    let l = only_layer(&p, &r);
    let pos = l.props.prop("transform/position").unwrap();
    assert_eq!(pos.keys.len(), 3);
    assert_eq!(r.report.imported.keyframes, 3);
    let times: Vec<Tick> = pos.keys.iter().map(|k| k.time).collect();
    assert_eq!(times, [Tick::ZERO, Tick(SEC), secs(2.5)]);
    let values: Vec<[f64; 3]> = pos.keys.iter().map(|k| if let Value::Vec3(v) = k.value { v } else { [f64::NAN; 3] }).collect();
    assert_eq!(values, [[0.0, 0.0, 0.0], [100.0, 0.0, 0.0], [200.0, 100.0, 0.0]]);
    assert!(pos.keys.iter().all(|k| k.in_interp == Interp::Linear && k.out_interp == Interp::Linear));
    // Automatic spatial tangents.
    assert!(pos.keys.iter().all(|k| k.spatial_auto));
    // Values at the key times are exact.
    for (k, want) in pos.keys.iter().zip(&values) {
        assert_eq!(pos.value_at(k.time), Value::Vec3(*want));
    }
    // The static value follows the first key.
    close3(vec3(&l, "transform/position"), [0.0, 0.0, 0.0]);
}

#[test]
fn spatial_tangents_and_hold_keys() {
    let (p, r) = load("054-key-position-2key-spatial-bezier.aep");
    let l = only_layer(&p, &r);
    let pos = l.props.prop("transform/position").unwrap();
    assert_eq!(pos.keys.len(), 2);
    assert!(!pos.keys[0].spatial_auto && !pos.keys[1].spatial_auto);
    assert_eq!(pos.keys[0].spatial_out, [30.0, 60.0, 0.0]);
    assert_eq!(pos.keys[1].spatial_in, [-30.0, -60.0, 0.0]);

    let (p, r) = load("053-key-position-2key-hold-key1.aep");
    let pos = only_layer(&p, &r).props.prop("transform/position").unwrap().clone();
    assert_eq!(pos.keys.len(), 2);
    assert_eq!((pos.keys[0].out_interp, pos.keys[1].in_interp), (Interp::Hold, Interp::Linear));
    // Held until the second key.
    let mid = Tick((pos.keys[0].time.0 + pos.keys[1].time.0) / 2);
    assert_eq!(pos.value_at(mid), pos.keys[0].value);

    let (p, r) = load("202-key-out-hold-in-linear.aep");
    let pos = only_layer(&p, &r).props.prop("transform/position").unwrap().clone();
    assert_eq!((pos.keys[0].out_interp, pos.keys[1].in_interp), (Interp::Hold, Interp::Linear));
    let (p, r) = load("203-key-in-hold-out-linear.aep");
    let pos = only_layer(&p, &r).props.prop("transform/position").unwrap().clone();
    assert_eq!((pos.keys[0].in_interp, pos.keys[0].out_interp), (Interp::Hold, Interp::Linear));
}

#[test]
fn ease_speed_and_influence() {
    // Position: pixels per second, kept as stored; the first key's in and the last key's out are
    // meaningless and dropped.
    let (p, r) = load("204-key-ease-asymmetric.aep");
    let pos = only_layer(&p, &r).props.prop("transform/position").unwrap().clone();
    assert_eq!((pos.keys[0].in_interp, pos.keys[0].out_interp), (Interp::Bezier, Interp::Bezier));
    assert_eq!(pos.keys[0].out_ease, [Ease { speed: 50.0, influence: 0.8 }]);
    assert_eq!(pos.keys[1].in_ease, [Ease { speed: 7.0, influence: 0.6 }]);
    assert!(pos.keys[0].in_ease.is_empty() && pos.keys[1].out_ease.is_empty());

    // Opacity: stored per second as a fraction, here in percent (x 100).
    let (p, r) = load("205-key-opacity-ease-asymmetric.aep");
    let op = only_layer(&p, &r).props.prop("transform/opacity").unwrap().clone();
    assert_eq!(op.keys.len(), 2);
    assert_eq!((op.keys[0].time, op.keys[1].time), (Tick::ZERO, Tick(2 * SEC)));
    assert_eq!((op.keys[0].value.clone(), op.keys[1].value.clone()), (Value::Scalar(0.0), Value::Scalar(100.0)));
    let e0 = op.keys[0].out_ease[0];
    let e1 = op.keys[1].in_ease[0];
    close(e0.speed, 20.0);
    close(e0.influence, 0.8);
    close(e1.speed, 30.0);
    close(e1.influence, 0.6);
    assert_eq!(op.value_at(Tick::ZERO), Value::Scalar(0.0));
    assert_eq!(op.value_at(Tick(2 * SEC)), Value::Scalar(100.0));

    let (p, r) = load("058-key-opacity-2key.aep");
    let op = only_layer(&p, &r).props.prop("transform/opacity").unwrap().clone();
    assert_eq!(op.keys.len(), 2);
}

#[test]
fn scale_keys_are_percent() {
    let (p, r) = load("206-key-scale-2key-linear.aep");
    let s = only_layer(&p, &r).props.prop("transform/scale").unwrap().clone();
    assert_eq!(s.keys.len(), 2);
    assert_eq!(s.keys[0].value, Value::Vec3([100.0, 100.0, 100.0]));
    assert_eq!(s.keys[1].value, Value::Vec3([200.0, 150.0, 100.0]));
    assert_eq!(s.keys[1].time, Tick(SEC));
    assert!(s.keys.iter().all(|k| k.in_interp == Interp::Linear && k.out_interp == Interp::Linear));
}

#[test]
fn roving_and_labelled_keys() {
    let (p, r) = load("056-key-position-3key-roving-key2.aep");
    let pos = only_layer(&p, &r).props.prop("transform/position").unwrap().clone();
    assert!(pos.keys[1].roving && !pos.keys[0].roving);
    // Roving moved the key to 1.0355078125 s.
    assert_eq!(pos.keys[1].time, secs(1.0355078125));
    let (p, r) = load("057-key-position-3key-label9-key2.aep");
    let pos = only_layer(&p, &r).props.prop("transform/position").unwrap().clone();
    assert_eq!(pos.keys[1].label, 9);
    assert_eq!(pos.keys[0].label, 0);
}

#[test]
fn expressions_keep_source_and_enable_switch() {
    let (p, r) = load("060-expr-opacity-wiggle.aep");
    let l = only_layer(&p, &r);
    let e = l.props.prop("transform/opacity").unwrap().expr.clone().unwrap();
    assert_eq!((e.text.as_str(), e.enabled), ("wiggle(1,10)", true));
    assert_eq!(r.report.imported.expressions, 1);
    close(scalar(&l, "transform/opacity"), 100.0);

    let (p, r) = load("213-expr-opacity-wiggle-disabled.aep");
    let e = only_layer(&p, &r).props.prop("transform/opacity").unwrap().expr.clone().unwrap();
    assert_eq!((e.text.as_str(), e.enabled), ("wiggle(1,10)", false));
}

// ---------------------------------------------------------------- the report

fn loss(r: &ImportReport, c: LossCategory) -> usize {
    r.count(c)
}

#[test]
fn unsupported_features_are_reported_per_category() {
    let expect: [(&str, LossCategory, usize); 1] = [("077-fx-dropdown-default.aep", LossCategory::Effects, 1)];
    for (name, category, n) in expect {
        let (p, r) = load(name);
        assert_eq!(loss(&r.report, category), n, "{name}: {:?}", r.report.losses);
        assert_eq!(r.report.total_losses(), n, "{name}: {:?}", r.report.losses);
        // The solid layer L still arrived.
        let (_, c) = comp(&p, &r, "C");
        assert!(c.layers.iter().any(|l| l.name == "L"), "{name}");
    }
    // Effect detail names the effect.
    let (_, r) = load("077-fx-dropdown-default.aep");
    assert!(r.report.losses[0].details[0].contains("Dropdown"), "{:?}", r.report.losses);
}

#[test]
fn losses_are_refused_unless_allowed_and_a_refusal_changes_nothing() {
    let mut p = Project::default();
    let before = p.clone();
    let err = try_import(&mut p, "077-fx-dropdown-default.aep", false).unwrap_err();
    let AepImportError::Refused(report) = &err else { panic!("{err:?}") };
    assert_eq!(report.count(LossCategory::Effects), 1);
    assert!(err.to_string().contains("effects: 1"), "{err}");
    assert_eq!(p, before);
    // Allowed: imported, the report is the same.
    let r = try_import(&mut p, "077-fx-dropdown-default.aep", true).unwrap();
    assert_eq!(&r.report, report.as_ref());
    assert!(p.items.len() > before.items.len());
    // A lossless file needs no permission.
    let mut p = Project::default();
    assert!(try_import(&mut p, "000-base.aep", false).is_ok());
}

#[test]
fn the_report_round_trips_through_json() {
    let (_, r) = load("077-fx-dropdown-default.aep");
    let json = serde_json::to_string(&r.report).unwrap();
    let back: ImportReport = serde_json::from_str(&json).unwrap();
    assert_eq!(back, r.report);
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["losses"][0]["category"], "effects");
    assert_eq!(v["imported"]["layers"], 1);
    assert!(v["limitations"].as_array().unwrap().len() >= 2);
}

#[test]
fn analyze_is_a_dry_run() {
    let bytes = common::fixture_bytes("077-fx-dropdown-default.aep");
    let report = analyze(&bytes, &|_| false).unwrap();
    assert_eq!(report.count(LossCategory::Effects), 1);
    assert_eq!(report.imported.layers, 1);
}

#[test]
fn files_that_are_not_projects_are_errors() {
    let mut p = Project::default();
    for bytes in [&b""[..], b"RIFX", b"hello world, not an aep", b"RIFX\0\0\0\x04Egg!"] {
        let none = |_: &str| false;
        let opts = effectcraft_aep_import::ImportOptions::new(&none);
        assert!(effectcraft_aep_import::import(&mut p, bytes, &opts).is_err());
    }
    assert!(p.items.is_empty());
}

#[test]
fn time_stretch_is_the_exact_ratio() {
    // 316: 50 %, the stored out point 5 s becomes 2.5 s.
    let (p, r) = load("316-layer-stretch50.aep");
    let l = only_layer(&p, &r);
    assert_eq!((l.stretch, l.start_time, l.in_point, l.out_point), (50.0, Tick::ZERO, Tick::ZERO, secs(2.5)));
    // 317: 150 %, ends at 7.5 s.
    let (p, r) = load("317-layer-stretch150.aep");
    let l = only_layer(&p, &r);
    assert_eq!((l.stretch, l.in_point, l.out_point), (150.0, Tick::ZERO, secs(7.5)));
}

#[test]
fn fractional_in_point_is_exact() {
    // 318: in point 8532/25600 s, out 136532/25600 s.
    let (p, r) = load("318-layer-inpoint-0.3333.aep");
    let l = only_layer(&p, &r);
    assert_eq!(l.stretch, 100.0);
    assert_eq!(l.in_point, Tick(8532 * (SEC / 25600)));
    assert_eq!(l.out_point, Tick(136532 * (SEC / 25600)));
    assert_eq!(l.start_time, Tick::ZERO);
}

#[test]
fn composition_pixel_aspect_shutter_phase_and_display_start() {
    let (p, r) = load("325-comp-pixelaspect-0.9091.aep");
    close(comp(&p, &r, "C").1.pixel_aspect, 10.0 / 11.0);
    let (p, r) = load("326-comp-shutterphase-m90.aep");
    let c = comp(&p, &r, "C").1;
    assert_eq!((c.shutter_angle, c.shutter_phase), (180.0, -90.0));
    let (p, r) = load("327-comp-displaystartframe-100.aep");
    let c = comp(&p, &r, "C").1;
    assert_eq!((c.display_start, c.shutter_phase), (Tick(4 * SEC), 0.0));
    let (p, r) = load("000-base.aep");
    assert_eq!(comp(&p, &r, "C").1.display_start, Tick::ZERO);
}

#[test]
fn ntsc_fixtures_import_as_exact_rates() {
    let (p, r) = load("229-comp-fps-29.97.aep");
    assert_eq!(comp(&p, &r, "C").1.frame_rate, FrameRate::FPS_29_97);
    let (p, r) = load("230-comp-fps-23.976.aep");
    assert_eq!(comp(&p, &r, "C").1.frame_rate, FrameRate::FPS_23_976);
}

#[test]
fn scale_bezier_ease_is_imported_per_dimension() {
    // 305: key 1 in 0 / 33 %, out 0.2 / 66 %; key 2 in 0.1 / 66 %, out 0 / 33 % in every
    // dimension. Speeds are fractions per second, here percent.
    let (p, r) = load("305-key-scale-bezier-ease.aep");
    let s = only_layer(&p, &r).props.prop("transform/scale").unwrap().clone();
    assert_eq!(s.keys.len(), 2);
    for k in &s.keys {
        assert_eq!((k.in_interp, k.out_interp), (Interp::Bezier, Interp::Bezier));
    }
    assert_eq!(s.keys[0].value, Value::Vec3([100.0, 100.0, 100.0]));
    assert_eq!(s.keys[1].value, Value::Vec3([200.0, 150.0, 100.0]));
    assert_eq!(s.keys[1].time, Tick(2 * SEC));
    assert!(s.keys[0].in_ease.is_empty() && s.keys[1].out_ease.is_empty());
    assert_eq!(s.keys[0].out_ease.len(), 3);
    for e in &s.keys[0].out_ease {
        close(e.speed, 20.0);
        close(e.influence, 0.66);
    }
    assert_eq!(s.keys[1].in_ease.len(), 3);
    for e in &s.keys[1].in_ease {
        close(e.speed, 10.0);
        close(e.influence, 0.66);
    }
    assert_eq!(loss(&r.report, LossCategory::ApproximatedKeyframes), 0);
}

#[test]
fn an_image_sequence_becomes_a_sequence_of_its_files() {
    // 603: shot_0101.exr .. shot_0103.exr in a folder (64x48, 30 fps, 3 frames).
    let (p, r) = load("603-footage-exrseq-0101-as-layer.aep");
    let (name, f) = p.items.values().find_map(|i| if let ItemKind::Footage(f) = &i.kind { Some((i.name.clone(), f.clone())) } else { None }).expect("footage");
    // No name is stored: the item shows the sequence's name with the frame range, as in AE.
    assert_eq!(name, "shot_[0101-0103].exr");
    assert_eq!(f.kind, FootageKind::Sequence);
    let names: Vec<&str> = f.sequence.iter().map(|s| s.rsplit(['\\', '/']).next().unwrap_or_default()).collect();
    assert_eq!(names, vec!["shot_0101.exr", "shot_0102.exr", "shot_0103.exr"]);
    assert!(f.path.ends_with("exrseq\\shot_0101.exr"), "{}", f.path);
    assert_eq!(f.frame_rate, FrameRate::new(30, 1));
    assert_eq!(f.duration, FrameRate::new(30, 1).tick_of(3));
    assert_eq!((f.width, f.height), (64, 48));
    // The missing-footage list names the first file (no file exists in the test).
    assert!(r.report.missing_footage.iter().any(|m| m.ends_with("shot_0101.exr")), "{:?}", r.report.missing_footage);
}

#[test]
fn project_settings_come_with_the_result_not_into_the_project() {
    use effectcraft_project::{BitDepth, ProjectSettings, TimeDisplayStyle};
    let def = ProjectSettings::default();
    // The base: 32 bpc, sRGB, gamma 2.2, timecode, frames from 0.
    let (p, r) = load("000-base.aep");
    assert_eq!(p.settings, def, "import leaves the project's settings alone");
    assert_eq!((r.settings.bit_depth, r.settings.working_space), (BitDepth::Bpc32, Some(effectcraft_color::ColorSpace::Srgb)));
    assert_eq!((r.settings.working_gamma, r.settings.linearize, r.settings.blend_linear), (2.2, false, false));
    assert!(r.report.is_lossless());
    let s = |name: &str| load(name).1.settings;
    assert_eq!(s("621-settings-bpc-8.aep").bit_depth, BitDepth::Bpc8);
    assert_eq!(s("622-settings-bpc-16.aep").bit_depth, BitDepth::Bpc16);
    assert_eq!(s("623-settings-workingspace-none.aep").working_space, None);
    // After Effects renders in the working space, so a Rec.709 project is also output in Rec.709
    // (902); an sRGB one keeps the default output (sRGB).
    let rec709 = s("902-settings-rec709-16bpc-steps-png-grey-solid.aep");
    assert_eq!((rec709.working_space, rec709.output_space), (Some(effectcraft_color::ColorSpace::Rec709), Some(effectcraft_color::ColorSpace::Rec709)));
    assert_eq!(rec709.bit_depth, BitDepth::Bpc16);
    assert_eq!(s("000-base.aep").output_space, None);
    assert_eq!(s("625-settings-gamma-2.4.aep").working_gamma, 2.4);
    let both = s("632-settings-linearize-and-blend-linear.aep");
    assert!(both.linearize && both.blend_linear);
    assert_eq!(s("629-settings-time-display-frames.aep").time_display, TimeDisplayStyle::Frames);
    assert_eq!(s("633-settings-feet-frames-35.aep").time_display, TimeDisplayStyle::Feet35);
    assert_eq!(s("634-settings-feet-frames-16.aep").time_display, TimeDisplayStyle::Feet16);
    assert_eq!(s("630-settings-frame-start-1.aep").frame_start, 1);
    // Settings EffectCraft does not keep (scene-referred compensation) change nothing.
    assert_eq!(s("628-settings-scene-compensation-off.aep"), s("000-base.aep"));
}

#[test]
fn a_working_space_without_a_counterpart_is_reported() {
    // The base with the profile name in the working-space JSON replaced (same length).
    let bytes = common::fixture_bytes("000-base.aep");
    let from = b"\"colorProfileName\":\"sRGB IEC61966-2.1\"";
    let to = b"\"colorProfileName\":\"Some Other Space!\"";
    let at = bytes.windows(from.len()).position(|w| w == from).expect("profile name");
    let mut edited = bytes.clone();
    edited[at..at + from.len()].copy_from_slice(to);
    let mut p = Project::default();
    let none = |_: &str| false;
    let r = effectcraft_aep_import::import(
        &mut p,
        &edited,
        &effectcraft_aep_import::ImportOptions { allow_losses: true, ..effectcraft_aep_import::ImportOptions::new(&none) },
    )
    .expect("import");
    assert_eq!(r.report.count(LossCategory::ProjectSettings), 1, "{:?}", r.report.losses);
    assert_eq!(r.settings.working_space, effectcraft_project::ProjectSettings::default().working_space);
    // Refused without allowLosses.
    let mut q = Project::default();
    assert!(effectcraft_aep_import::import(&mut q, &edited, &effectcraft_aep_import::ImportOptions::new(&none)).is_err());
}

#[test]
fn a_conformed_clip_plays_its_frames_at_the_conformed_rate() {
    // 645: the 25-frame, 25 fps mp4 conformed to 30 fps (AE: 0.8333 s).
    let (p, _) = load("645-footage-mp4-conform-30.aep");
    let f = p.items.values().find_map(|i| if let ItemKind::Footage(f) = &i.kind { Some(f.clone()) } else { None }).expect("clip");
    assert_eq!(f.kind, FootageKind::Video);
    assert_eq!((f.frame_rate, f.native_rate), (FrameRate::new(30, 1), Some(FrameRate::new(25, 1))));
    assert_eq!(f.duration, FrameRate::new(30, 1).tick_of(25));
}

#[test]
fn stills_of_four_more_types_are_footage() {
    for (name, ext) in [
        ("650-footage-tga-still-as-layer.aep", "tga"),
        ("651-footage-psd-flat-as-layer.aep", "psd"),
        ("652-footage-ai-as-layer.aep", "ai"),
        ("653-footage-eps-as-layer.aep", "eps"),
    ] {
        let (p, r) = load(name);
        assert!(r.report.is_lossless(), "{name}: {:?}", r.report.losses);
        let f = p.items.values().find_map(|i| if let ItemKind::Footage(f) = &i.kind { Some(f.clone()) } else { None }).expect(name);
        assert_eq!((f.kind, f.width, f.height), (FootageKind::Still, 64, 48), "{name}");
        assert!(f.path.ends_with(&format!("still.{ext}")), "{name}: {}", f.path);
    }
}
