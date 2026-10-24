//! Precomposition layers: a composition shown as a layer of another composition. The exported
//! bytes are parsed back with `effectcraft-aep` (source id, `idpc` / `idpi`, the Time Remapping
//! maximum, the anchor unit) and imported again.

mod common;

use common::*;
use effectcraft_aep::{PropNode, parse};
use effectcraft_aep_export::LossCategory;
use effectcraft_aep_import::{ImportOptions, import};
use effectcraft_keyframe::Value;
use effectcraft_project::{ItemId, ItemKind, LayerSource, Project};
use effectcraft_time::FrameRate;

fn reimport(bytes: &[u8]) -> Project {
    let mut p = Project::default();
    let none = |_: &str| false;
    let opts = ImportOptions { allow_losses: true, ..ImportOptions::new(&none) };
    import(&mut p, bytes, &opts).expect("import");
    p
}

/// `Pre` (640 x 360, 30 fps, 3 s, one solid) shown in `C` with an anchor and a position.
fn nested() -> (Project, ItemId, ItemId) {
    let mut p = Project::default();
    let pre = add_comp(&mut p, "Pre", 640, 360, FrameRate::new(30, 1), 3.0, None);
    let s = add_solid(&mut p, "S", 640, 360, [0.0, 1.0, 0.0], None);
    add_layer(&mut p, pre, "S", LayerSource::Solid { item: s }, (640, 360));
    let c = add_comp(&mut p, "C", 1920, 1080, FrameRate::new(25, 1), 5.0, None);
    add_layer(&mut p, c, "Pre", LayerSource::Comp { item: pre }, (640, 360));
    with_layer(&mut p, c, 0, |l| {
        let tr = l.props.sub_mut("transform").expect("transform");
        tr.get_mut("anchor").expect("anchor").value = Value::Vec3([160.0, 90.0, 0.0]);
        tr.get_mut("position").expect("position").value = Value::Vec3([300.0, 200.0, 0.0]);
    });
    (p, c, pre)
}

#[test]
fn a_precomposition_layer_points_at_its_composition() {
    let (p, _, _) = nested();
    let r = export_all(&p);
    assert!(r.report.is_lossless(), "{:?}", r.report.losses);
    assert_eq!(r.report.exported.precomp_layers, 1);
    let file = parse(&r.bytes).expect("parse");
    let view = effectcraft_aep::Project::new(&file).expect("project");
    let items = view.all_items();
    let by_name = |n: &str| items.iter().find(|i| i.display_name().as_deref() == Some(n)).copied().expect(n);
    let (c, pre) = (by_name("C"), by_name("Pre"));
    let pre_id = pre.id().expect("id");
    // C records the composition it uses (idpi, little-endian), idpc byte 0 is 1.
    assert_eq!(c.used_item_ids(), vec![pre_id]);
    let idpc = c.chunk().child(b"idpc").and_then(|x| x.bytes()).expect("idpc");
    assert_eq!(idpc.first(), Some(&1));
    assert_eq!(pre.used_item_ids(), Vec::<u32>::new());
    assert_eq!(pre.chunk().child(b"idpc").and_then(|x| x.bytes()).and_then(|b| b.first().copied()), Some(0));
    let layer = c.comp().expect("comp").layers().next().expect("layer");
    let h = layer.header().expect("header");
    assert_eq!(h.source_id, Some(pre_id));
    assert_eq!(view.layer_source_size(&layer), Some((640, 360)));
    assert_eq!(layer.name().as_deref(), Some("Pre"));
    // The anchor is a fraction of the precomposition's size.
    let tr = layer.properties().and_then(|g| g.get("ADBE Transform Group"));
    let Some(PropNode::Group(tr)) = tr else { panic!("transform") };
    let Some(PropNode::Property(a)) = tr.get("ADBE Anchor Point") else { panic!("anchor") };
    let v = a.static_value().expect("value");
    assert!((v[0] - 0.25).abs() < 1e-12 && (v[1] - 0.25).abs() < 1e-12, "{v:?}");
    // The Time Remapping maximum is the precomposition's duration (3 s; 0 on a solid).
    let tdum_max = |l: &effectcraft_aep::Layer<'_, '_>| -> f64 {
        let Some(PropNode::Property(tr)) = l.properties().and_then(|g| g.get("ADBE Time Remapping")) else { panic!("time remap") };
        let b = tr.chunk().child(b"tduM").and_then(|x| x.bytes()).expect("tduM");
        f64::from_be_bytes(b.get(..8).and_then(|s| s.try_into().ok()).expect("8 bytes"))
    };
    assert_eq!(tdum_max(&layer), 3.0);
    let inner = pre.comp().expect("comp").layers().next().expect("layer");
    assert_eq!(tdum_max(&inner), 0.0);
}

#[test]
fn a_precomposition_survives_import() {
    let (p, _, _) = nested();
    let q = reimport(&export_all(&p).bytes);
    let comp_named = |n: &str| q.items.values().find(|i| i.name == n && matches!(i.kind, ItemKind::Comp(_))).expect(n);
    let (c, pre) = (comp_named("C"), comp_named("Pre"));
    let ItemKind::Comp(cc) = &c.kind else { unreachable!() };
    let ItemKind::Comp(pc) = &pre.kind else { unreachable!() };
    assert_eq!((pc.width, pc.height, pc.frame_rate), (640, 360, FrameRate::new(30, 1)));
    let l = cc.layers.first().expect("layer");
    assert_eq!(l.source, LayerSource::Comp { item: pre.id });
    let tr = l.transform().expect("transform");
    assert_eq!(tr.get("anchor").expect("anchor").value.components()[..2], [160.0, 90.0]);
    assert_eq!(tr.get("position").expect("position").value.components()[..2], [300.0, 200.0]);
}

#[test]
fn one_composition_used_twice_is_recorded_once_and_chains_nest() {
    let mut p = Project::default();
    let a = add_comp(&mut p, "A", 100, 100, FrameRate::new(25, 1), 1.0, None);
    let b = add_comp(&mut p, "B", 200, 100, FrameRate::new(25, 1), 2.0, None);
    add_layer(&mut p, b, "A1", LayerSource::Comp { item: a }, (100, 100));
    add_layer(&mut p, b, "A2", LayerSource::Comp { item: a }, (100, 100));
    let c = add_comp(&mut p, "C", 400, 300, FrameRate::new(25, 1), 3.0, None);
    add_layer(&mut p, c, "B", LayerSource::Comp { item: b }, (200, 100));
    add_layer(&mut p, c, "A", LayerSource::Comp { item: a }, (100, 100));
    let r = export_all(&p);
    assert!(r.report.is_lossless(), "{:?}", r.report.losses);
    assert_eq!(r.report.exported.precomp_layers, 4);
    let file = parse(&r.bytes).expect("parse");
    let view = effectcraft_aep::Project::new(&file).expect("project");
    let items = view.all_items();
    let id = |n: &str| items.iter().find(|i| i.display_name().as_deref() == Some(n)).and_then(|i| i.id()).expect(n);
    let used = |n: &str| items.iter().find(|i| i.display_name().as_deref() == Some(n)).expect(n).used_item_ids();
    assert_eq!(used("B"), vec![id("A")]);
    // Ascending ids (601; the order carries no meaning), and idpc counts them.
    let mut ab = vec![id("A"), id("B")];
    ab.sort_unstable();
    assert_eq!(used("C"), ab);
    let idpc = |n: &str| {
        items.iter().find(|i| i.display_name().as_deref() == Some(n)).and_then(|i| i.chunk().child(b"idpc")).and_then(|c| c.bytes()).map(|b| b[..4].to_vec())
    };
    assert_eq!(idpc("C"), Some(vec![2, 0, 0, 0]));
    assert_eq!(idpc("B"), Some(vec![1, 0, 0, 0]));
    assert!(used("A").is_empty());
    let q = reimport(&r.bytes);
    let n = q.items.values().filter_map(|i| if let ItemKind::Comp(c) = &i.kind { Some(c.layers.len()) } else { None }).sum::<usize>();
    assert_eq!(n, 4);
}

#[test]
fn a_loop_of_precompositions_is_reported_not_written() {
    let mut p = Project::default();
    let a = add_comp(&mut p, "A", 100, 100, FrameRate::new(25, 1), 1.0, None);
    let b = add_comp(&mut p, "B", 100, 100, FrameRate::new(25, 1), 1.0, None);
    add_layer(&mut p, a, "self", LayerSource::Comp { item: a }, (100, 100));
    add_layer(&mut p, a, "B", LayerSource::Comp { item: b }, (100, 100));
    add_layer(&mut p, b, "A", LayerSource::Comp { item: a }, (100, 100));
    // A missing composition.
    add_layer(&mut p, b, "gone", LayerSource::Comp { item: ItemId(999_999) }, (100, 100));
    // C uses A without a loop through C.
    let c = add_comp(&mut p, "C", 100, 100, FrameRate::new(25, 1), 1.0, None);
    add_layer(&mut p, c, "A", LayerSource::Comp { item: a }, (100, 100));
    let r = export_all(&p);
    assert_eq!(r.report.count(LossCategory::UnsupportedLayers), 4, "{:?}", r.report.losses);
    assert_eq!(r.report.exported.precomp_layers, 1);
    // What was written still parses and imports.
    let q = reimport(&r.bytes);
    assert!(q.items.values().any(|i| i.name == "A"));
}
