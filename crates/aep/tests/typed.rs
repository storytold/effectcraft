//! The typed view, checked against the values the generated corpus was made with (the synthetic
//! MANIFEST: composition `C` 1920x1080 at 25 fps with one solid layer `L`, then one change per
//! file).

mod common;

use common::{FIXTURES, fixture};
use effectcraft_aep::{AepFile, Body, ItemKind, KeyList, Layer, Project, PropGroup, PropNode, Property, parse};

fn first_layer<'f, 'a>(project: &Project<'f, 'a>) -> Layer<'f, 'a> {
    let comp = project.all_items().into_iter().find_map(|i| i.comp()).expect("a composition");
    comp.layers().next().expect("a layer")
}

fn transform<'f, 'a>(layer: &Layer<'f, 'a>) -> PropGroup<'f, 'a> {
    match layer.properties().expect("layer properties").get("ADBE Transform Group") {
        Some(PropNode::Group(g)) => g,
        other => panic!("transform group: {other:?}"),
    }
}

fn property<'f, 'a>(group: &PropGroup<'f, 'a>, name: &str) -> Property<'f, 'a> {
    match group.get(name) {
        Some(PropNode::Property(p)) => p,
        other => panic!("{name}: {other:?}"),
    }
}

#[test]
fn base_project_items() {
    let bytes = fixture("000-base.aep");
    let file = parse(&bytes).unwrap();
    let project = Project::new(&file).unwrap();
    // Comp C and the folder Solids at the top; the solid's footage item lives inside the folder.
    let top: Vec<_> = project.items().map(|i| (i.kind(), i.name())).collect();
    assert_eq!(top, [(Some(ItemKind::Composition), Some("C".to_string())), (Some(ItemKind::Folder), Some("Solids".to_string()))]);
    let all: Vec<_> = project.all_items().iter().map(|i| i.kind()).collect();
    assert_eq!(all, [Some(ItemKind::Composition), Some(ItemKind::Folder), Some(ItemKind::Footage)]);
    // numItems = 3 in the manifest.
    assert_eq!(all.len(), 3);
    let folder = project.items().nth(1).unwrap();
    assert_eq!(folder.children().count(), 1);
}

#[test]
fn empty_project_has_no_items_of_a_composition() {
    let bytes = fixture("001-empty.aep");
    let file = parse(&bytes).unwrap();
    let project = Project::new(&file).unwrap();
    assert!(project.all_items().iter().all(|i| i.comp().is_none()));
}

#[test]
fn composition_time_base_gives_the_frame_rate() {
    let bytes = fixture("000-base.aep");
    let file = parse(&bytes).unwrap();
    let comp = Project::new(&file).unwrap().items().find_map(|i| i.comp()).unwrap();
    let tb = comp.time_base().unwrap();
    assert_eq!((tb.frame_duration, tb.timescale), (1024, 25600));
    assert_eq!(tb.frame_rate(), Some(25.0));
    assert_eq!(tb.seconds(25600), Some(1.0));
    // 031: frameRate 25 -> 30 changes the time base.
    let bytes = fixture("031-comp-fps30.aep");
    let file = parse(&bytes).unwrap();
    let comp = Project::new(&file).unwrap().items().find_map(|i| i.comp()).unwrap();
    assert_eq!(comp.time_base().unwrap().frame_rate(), Some(30.0));
}

#[test]
fn layers_and_names() {
    // Base: one layer L whose name is its source's (no explicit name stored).
    let bytes = fixture("000-base.aep");
    let file = parse(&bytes).unwrap();
    let comp = Project::new(&file).unwrap().items().find_map(|i| i.comp()).unwrap();
    let layers: Vec<_> = comp.layers().collect();
    assert_eq!(layers.len(), 1);
    assert_eq!(layers[0].name(), None);
    assert_eq!(layers[0].is_three_d(), Some(false));
    // 110: a text layer T on top of L.
    let bytes = fixture("110-text-A.aep");
    let file = parse(&bytes).unwrap();
    let comp = Project::new(&file).unwrap().items().find_map(|i| i.comp()).unwrap();
    let names: Vec<_> = comp.layers().map(|l| l.name()).collect();
    assert_eq!(names, [Some("T".to_string()), None]);
}

#[test]
fn three_d_switch() {
    let bytes = fixture("015-layer-3d.aep");
    let file = parse(&bytes).unwrap();
    let project = Project::new(&file).unwrap();
    let layer = first_layer(&project);
    assert_eq!(layer.is_three_d(), Some(true));
    // The 3D switch adds the Z rotation to the transform group.
    assert!(transform(&layer).names().iter().any(|n| n == "ADBE Rotate Z"));
    let bytes = fixture("000-base.aep");
    let file = parse(&bytes).unwrap();
    let project = Project::new(&file).unwrap();
    assert!(!transform(&first_layer(&project)).names().iter().any(|n| n == "ADBE Rotate Z"));
}

#[test]
fn match_names_come_from_the_tdmn_chunks() {
    let bytes = fixture("000-base.aep");
    let file = parse(&bytes).unwrap();
    let layer = first_layer(&Project::new(&file).unwrap());
    let root = layer.properties().unwrap();
    let names = root.names();
    assert_eq!(&names[..2], ["ADBE Time Remapping", "ADBE Transform Group"]);
    assert!(names.iter().any(|n| n == "ADBE Material Options Group"));
    // The path lookup follows nested groups; "ADBE Group End" names nothing.
    assert!(matches!(root.find(&["ADBE Transform Group"]), Some(PropNode::Group(_))));
    assert!(!names.iter().any(|n| n == "ADBE Group End"));
    assert!(root.find(&["ADBE Transform Group", "ADBE Nonexistent"]).is_none());
    // The raw tdmn body is 40 bytes, NUL padded.
    let raw = root.chunk().children().iter().find(|c| c.is(b"tdmn")).unwrap();
    assert_eq!(raw.bytes().unwrap().len(), 40);
}

#[test]
fn static_value_is_the_first_dims_doubles_of_cdat() {
    // 040: Position [960,540,0] -> [100,200,0]; AE stores only non-default transform values.
    let bytes = fixture("040-static-position-100-200.aep");
    let file = parse(&bytes).unwrap();
    let layer = first_layer(&Project::new(&file).unwrap());
    let position = property(&transform(&layer), "ADBE Position");
    assert_eq!(position.dims(), Some(3));
    assert!(!position.is_animated());
    assert_eq!(position.header().map(|h| h.len()), Some(124));
    assert_eq!(position.header().and_then(|h| h.get(68).copied()), Some(0));
    assert_eq!(position.static_value(), Some(vec![100.0, 200.0, 0.0]));
    assert!(position.keys().is_none());
    // The base file stores no Position at all.
    let bytes = fixture("000-base.aep");
    let file = parse(&bytes).unwrap();
    let layer = first_layer(&Project::new(&file).unwrap());
    assert!(transform(&layer).get("ADBE Position").is_none());
}

#[test]
fn effect_parameter_value() {
    // 071: Slider Control effect with Slider = 77.
    let bytes = fixture("071-fx-slider-value77.aep");
    let file = parse(&bytes).unwrap();
    let layer = first_layer(&Project::new(&file).unwrap());
    let parade = match layer.properties().unwrap().get("ADBE Effect Parade") {
        Some(PropNode::Group(g)) => g,
        other => panic!("{other:?}"),
    };
    assert_eq!(parade.names(), ["ADBE Slider Control"]);
    let Some(PropNode::Other(instance)) = parade.get("ADBE Slider Control") else { panic!("effect instance") };
    assert!(instance.is_list(b"sspc"));
    // The effect's name is in a nested fnam chunk.
    let fnam = instance.child(b"fnam").unwrap();
    assert!(matches!(fnam.body, Body::Chunks(_)));
    assert_eq!(fnam.children()[0].bytes(), Some(&b"Slider Control"[..]));
    // The parameters are a property group inside the instance.
    let params = PropGroup::from_chunk(instance.child_list(b"tdgp").unwrap()).unwrap();
    assert_eq!(params.names(), ["ADBE Slider Control-0000", "ADBE Slider Control-0001", "ADBE Effect Built In Params"]);
    let slider = property(&params, "ADBE Slider Control-0001");
    assert_eq!(slider.dims(), Some(1));
    assert_eq!(slider.static_value(), Some(vec![77.0]));
}

#[test]
fn position_keyframes() {
    // 055: Position keys at 0 s [0,0,0], 1 s [100,0,0] and 2.5 s [200,100,0], all linear.
    let bytes = fixture("055-key-position-3key-inserted-t2.5.aep");
    let file = parse(&bytes).unwrap();
    let project = Project::new(&file).unwrap();
    let comp = project.items().find_map(|i| i.comp()).unwrap();
    let tb = comp.time_base().unwrap();
    let layer = first_layer(&project);
    let position = property(&transform(&layer), "ADBE Position");
    assert!(position.is_animated());
    assert_eq!(position.header().and_then(|h| h.get(68).copied()), Some(1));
    assert_eq!(position.dims(), Some(3));
    assert_eq!(position.static_value(), None);
    let keys = position.keys().unwrap();
    // lhd3: record count (word 2), record size (word 4), capacity rounded up to 4 (word 7).
    assert_eq!(keys.header.magic, 0x00d0_0bee);
    assert_eq!(keys.header.count, 3);
    assert_eq!(keys.header.record_size, 128);
    assert_eq!(keys.header.words[3], 1);
    assert_eq!(keys.header.words[7], 4);
    assert_eq!(keys.len(), 3);
    let times: Vec<i32> = keys.keyframes().map(|k| k.time_units().unwrap()).collect();
    assert_eq!(times, [0, 25_600, 64_000]);
    let seconds: Vec<f64> = times.iter().map(|t| tb.seconds(*t).unwrap()).collect();
    assert_eq!(seconds, [0.0, 1.0, 2.5]);
    let values: Vec<[f64; 3]> = keys.keyframes().map(|k| k.spatial_value().unwrap()).collect();
    assert_eq!(values, [[0.0, 0.0, 0.0], [100.0, 0.0, 0.0], [200.0, 100.0, 0.0]]);
}

#[test]
fn list_header_facts_hold_in_every_list_of_every_fixture() {
    let mut lists = 0;
    let mut with_records = 0;
    for name in FIXTURES {
        let bytes = fixture(name);
        let file = parse(&bytes).unwrap();
        for (_, c) in file.walk() {
            if !c.is_list(b"list") || c.child(b"lhd3").is_none() {
                continue;
            }
            lists += 1;
            let keys = KeyList::from_chunk(c).unwrap_or_else(|| panic!("{name}: len(ldat) != count * record size"));
            assert_eq!(keys.header.magic, 0x00d0_0bee, "{name}");
            assert!(keys.header.words[8..].iter().all(|w| *w == 0), "{name}");
            if !keys.is_empty() {
                with_records += 1;
            }
            assert_eq!(keys.records().count(), keys.len());
        }
    }
    assert!(lists >= 9 * 10, "lists seen: {lists}");
    assert!(with_records >= 1);
}

/// Visits every property under a group, depth-limited.
fn visit(group: &PropGroup<'_, '_>, depth: usize, f: &mut dyn FnMut(&Property<'_, '_>)) {
    if depth > 24 {
        return;
    }
    for e in group.entries() {
        match e.node {
            PropNode::Group(g) => visit(&g, depth + 1, f),
            PropNode::Property(p) => f(&p),
            PropNode::Other(c) => {
                if let Some(g) = c.child_list(b"tdgp").and_then(PropGroup::from_chunk) {
                    visit(&g, depth + 1, f);
                }
            }
        }
    }
}

#[test]
fn property_header_facts_hold_in_every_property_of_every_fixture() {
    let (mut props, mut animated) = (0, 0);
    for name in FIXTURES {
        let bytes = fixture(name);
        let file: AepFile<'_> = parse(&bytes).unwrap();
        let Some(project) = Project::new(&file) else { continue };
        for item in project.all_items() {
            let Some(comp) = item.comp() else { continue };
            for layer in comp.layers() {
                let Some(root) = layer.properties() else { continue };
                visit(&root, 0, &mut |p| {
                    props += 1;
                    let header = p.header().unwrap_or_else(|| panic!("{name}: property without tdb4"));
                    assert_eq!(header.len(), 124, "{name}");
                    // Constant first three bytes, and byte 68 = has keyframes.
                    assert_eq!(&header[..3], &[0xdb, 0x99, 0x00], "{name}");
                    assert_eq!(header[68] == 1, p.is_animated(), "{name}");
                    if p.is_animated() {
                        animated += 1;
                        let keys = p.keys().unwrap_or_else(|| panic!("{name}: no key list"));
                        let (count, w) = (keys.header.count, keys.header.words);
                        assert_eq!(w[3], count.div_ceil(4), "{name}: word 3");
                        assert_eq!(w[7], count.div_ceil(4) * 4, "{name}: word 7");
                        assert!(p.static_value().is_none(), "{name}");
                    }
                });
            }
        }
    }
    assert!(props > 100, "properties seen: {props}");
    assert!(animated >= 1);
}
