//! Odd projects never panic and always produce a container that parses and writes back.

mod common;

use common::*;
use effectcraft_aep::{parse, write};
use effectcraft_aep_export::{ExportOptions, LossCategory, export};
use effectcraft_color::Label;
use effectcraft_keyframe::{Ease, Interp, Value};
use effectcraft_project::{ItemKind, LayerSource, Project};
use effectcraft_time::{FrameRate, Tick};

fn check(p: &Project) -> effectcraft_aep_export::ExportResult {
    let r = export(p, &ExportOptions { allow_losses: true }).expect("export");
    let f = parse(&r.bytes).expect("parse");
    assert_eq!(write(&f).expect("write"), r.bytes);
    r
}

#[test]
fn empty_project_and_empty_names() {
    let p = Project::default();
    let r = check(&p);
    assert!(r.report.is_lossless());
    let mut p = Project::default();
    let c = add_comp(&mut p, "", 100, 100, FrameRate::new(25, 1), 1.0, None);
    let s = add_solid(&mut p, "", 10, 10, [0.0; 3], None);
    add_layer(&mut p, c, "", LayerSource::Solid { item: s }, (10, 10));
    let f = p.add_item("", Label::None, None, ItemKind::Folder);
    add_comp(&mut p, "x", 10, 10, FrameRate::new(25, 1), 1.0, Some(f));
    check(&p);
}

#[test]
fn huge_zero_and_negative_sizes_and_durations() {
    let mut p = Project::default();
    for (w, h) in [(0, 0), (1, 1), (30_000, 30_000), (u32::MAX, u32::MAX), (65_536, 3)] {
        let c = add_comp(&mut p, "c", w, h, FrameRate::new(25, 1), 1.0, None);
        let s = add_solid(&mut p, "s", w, h, [2.0, -1.0, f32::NAN], None);
        add_layer(&mut p, c, "l", LayerSource::Solid { item: s }, (w, h));
    }
    for dur in [0.0, -5.0, 1e9, 1e-9] {
        add_comp(&mut p, "d", 100, 100, FrameRate::new(25, 1), dur, None);
    }
    let r = check(&p);
    assert!(r.report.count(LossCategory::CompositionSettings) >= 1);
}

#[test]
fn strange_frame_rates() {
    let mut p = Project::default();
    for (n, d) in [(0, 1), (1, 0), (-25, 1), (7, 3), (i64::MAX, 1), (1, i64::MAX), (25, 1), (1000, 1), (24_000, 1001)] {
        let rate = FrameRate { num: n, den: d };
        let mut c = effectcraft_project::Comp::new(100, 100, FrameRate::new(25, 1), Tick(254_016_000_000));
        c.frame_rate = rate;
        p.add_item("r", Label::None, None, ItemKind::Comp(std::sync::Arc::new(c)));
    }
    check(&p);
}

#[test]
fn non_finite_and_huge_values_are_written_as_defaults() {
    let (mut p, c, _) = simple();
    with_layer(&mut p, c, 0, |l| {
        let tr = l.transform_mut().expect("tr");
        tr.get_mut("position").expect("p").value = Value::Vec3([f64::NAN, f64::INFINITY, 1e300]);
        tr.get_mut("opacity").expect("o").value = Value::Scalar(f64::NAN);
        let sc = tr.get_mut("scale").expect("s");
        let mut k0 = key(0.0, Value::Vec3([f64::NAN, 1e30, 0.0]));
        k0.out_interp = Interp::Bezier;
        k0.out_ease = vec![Ease { speed: f64::NAN, influence: f64::INFINITY }];
        let mut k1 = key(1.0, Value::Vec3([1.0, 2.0, 3.0]));
        k1.in_interp = Interp::Bezier;
        k1.in_ease = vec![Ease { speed: 1e300, influence: -5.0 }];
        sc.keys = vec![k0, k1];
        let rot = tr.get_mut("rotation").expect("r");
        rot.keys = vec![key(0.0, Value::Scalar(1.0)), key(0.0, Value::Scalar(2.0)), key(1e12, Value::Scalar(3.0)), key(-1e12, Value::Scalar(4.0))];
    });
    let r = check(&p);
    assert!(r.report.count(LossCategory::InvalidValues) >= 3);
}

#[test]
fn thousands_of_layers_and_keys() {
    let mut p = Project::default();
    let c = add_comp(&mut p, "Many", 640, 360, FrameRate::new(25, 1), 20.0, None);
    let s = add_solid(&mut p, "S", 64, 64, [0.5; 3], None);
    let mut comp = p.comp(c).cloned().expect("comp");
    for i in 0..2000 {
        let mut l = effectcraft_project::build::layer(&mut p, &comp, &format!("L{i}"), LayerSource::Solid { item: s }, (64, 64), None);
        l.name = format!("L{i}");
        let pos = l.transform_mut().expect("tr").get_mut("position").expect("p");
        pos.keys = (0..10).map(|k| key(f64::from(k), Value::Vec3([f64::from(i), f64::from(k), 0.0]))).collect();
        comp.layers.push(l);
    }
    if let Some(i) = p.item_mut(c) {
        i.kind = ItemKind::Comp(std::sync::Arc::new(comp));
    }
    let r = check(&p);
    assert_eq!(r.report.exported.layers, 2000);
    assert_eq!(r.report.exported.keyframes, 20_000);
}

#[test]
fn parent_loops_missing_sources_and_folder_loops() {
    let (mut p, c, _) = simple();
    add_layer(&mut p, c, "B", LayerSource::Solid { item: effectcraft_project::ItemId(9999) }, (10, 10));
    add_layer(&mut p, c, "A", LayerSource::Null, (10, 10));
    let (a, b) = {
        let cc = p.comp(c).expect("c");
        (cc.layers[0].id, cc.layers[1].id)
    };
    // A layer that is its own parent, and the unsupported layer B as the parent of a third.
    with_layer(&mut p, c, 0, |l| l.parent = Some(a));
    with_layer(&mut p, c, 2, |l| l.parent = Some(b));
    // A loop of two exported layers.
    add_layer(&mut p, c, "P", LayerSource::Null, (10, 10));
    add_layer(&mut p, c, "Q", LayerSource::Null, (10, 10));
    let (pid, qid) = {
        let cc = p.comp(c).expect("c");
        (cc.layers[1].id, cc.layers[0].id)
    };
    with_layer(&mut p, c, 1, |l| l.parent = Some(qid));
    with_layer(&mut p, c, 0, |l| l.parent = Some(pid));
    // Two folders that are each other's parent, and an item that names a missing parent.
    let f1 = p.add_item("F1", Label::None, None, ItemKind::Folder);
    let f2 = p.add_item("F2", Label::None, Some(f1), ItemKind::Folder);
    if let Some(i) = p.item_mut(f1) {
        i.parent = Some(f2);
    }
    let s = add_solid(&mut p, "InLoop", 10, 10, [0.0; 3], Some(f1));
    add_layer(&mut p, c, "UsesLoopSolid", LayerSource::Solid { item: s }, (10, 10));
    let r = check(&p);
    assert!(r.report.count(LossCategory::ParentLinks) >= 3, "{}", r.report.summary());
    assert!(r.report.count(LossCategory::FootageItems) >= 3);
    assert!(r.report.count(LossCategory::UnsupportedLayers) >= 2);
}

#[test]
fn very_long_and_odd_names() {
    let mut p = Project::default();
    let long = "x".repeat(10_000);
    let weird = "a\0b\u{1f600}\u{202e}\"'<>&\n\r\t".to_string();
    let c = add_comp(&mut p, &long, 100, 100, FrameRate::new(25, 1), 1.0, None);
    let s = add_solid(&mut p, &long, 10, 10, [0.0; 3], None);
    add_layer(&mut p, c, &long, LayerSource::Solid { item: s }, (10, 10));
    add_layer(&mut p, c, &weird, LayerSource::Null, (10, 10));
    let s2 = add_solid(&mut p, &weird, 10, 10, [0.0; 3], None);
    add_layer(&mut p, c, "\u{3042}".repeat(100).as_str(), LayerSource::Solid { item: s2 }, (10, 10));
    let r = check(&p);
    assert!(r.report.count(LossCategory::Names) >= 1);
}

#[test]
fn a_wide_folder_tree() {
    let mut p = Project::default();
    let mut parent = None;
    for i in 0..100 {
        parent = Some(p.add_item(&format!("F{i}"), Label::None, parent, ItemKind::Folder));
    }
    for i in 0..300 {
        add_solid(&mut p, &format!("s{i}"), 8, 8, [0.1; 3], parent);
    }
    let r = check(&p);
    assert!(r.report.count(LossCategory::FootageItems) > 0, "folders deeper than the limit are reported");
}
