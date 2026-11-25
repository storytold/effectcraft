//! File ▸ Import ▸ EffectCraft Project: another project merges into the open one with fresh ids,
//! its references intact, in one undo step.

use std::collections::HashSet;

use effectcraft_project::{Footage, FootageKind, ItemId, ItemKind, LayerSource, Project};
use serde_json::json;

use crate::Session;

fn out_dir(name: &str) -> std::path::PathBuf {
    let d = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-out").join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Every item, layer and property uid of `p` (they share one counter).
fn all_ids(p: &Project) -> Vec<u64> {
    let mut v = vec![];
    for (id, it) in &p.items {
        v.push(id.0);
        if let ItemKind::Comp(c) = &it.kind {
            for l in &c.layers {
                v.push(l.id.0);
                fn uids(g: &effectcraft_project::PropGroup, v: &mut Vec<u64>) {
                    v.push(g.uid);
                    for c in &g.children {
                        match c {
                            effectcraft_project::Node::Prop(p) => v.push(p.uid),
                            effectcraft_project::Node::Group(g) => uids(g, v),
                        }
                    }
                }
                uids(&l.props, &mut v);
            }
        }
    }
    v
}

/// A project with "Main" nesting "Inner" under a parented null, a data file next to it by a
/// relative path and a footage file that's gone. Saved as `<dir>/Library Card.ecproj`.
fn source_project(dir: &std::path::Path) -> String {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Inner", "width": 64, "height": 64, "frameRate": 30, "duration": 2})).unwrap();
    let inner = s.active_comp_id().unwrap();
    s.execute("layer.newSolid", json!({"name": "Fill", "color": "#ff0000"})).unwrap();
    s.execute("comp.new", json!({"name": "Main", "width": 64, "height": 64, "frameRate": 30, "duration": 2})).unwrap();
    let main = s.active_comp_id().unwrap();
    let nest = s.execute("layer.addItem", json!({"comp": main.0, "item": inner.0, "time": 0.0})).unwrap()["layer"].as_u64().unwrap();
    let rig = s.execute("layer.newNull", json!({"name": "Rig"})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.setParent", json!({"layers": [nest], "parent": rig})).unwrap();
    std::fs::write(dir.join("data.json"), b"{\"v\": 1}").unwrap();
    let p = std::sync::Arc::make_mut(&mut s.project);
    p.add_item("data.json", Default::default(), None, ItemKind::Footage(Footage { path: "data.json".into(), kind: FootageKind::Data, ..Default::default() }));
    p.add_item(
        "gone.mov",
        Default::default(),
        None,
        ItemKind::Footage(Footage { path: "/offline/gone.mov".into(), kind: FootageKind::Video, ..Default::default() }),
    );
    let path = dir.join("Library Card.ecproj").to_string_lossy().to_string();
    s.execute("file.saveAs", json!({"path": path})).unwrap();
    path
}

#[test]
fn import_project_preserves_a_frozen_object_matte_across_new_ids_and_reopen() {
    check_imported_object_matte(false);
}

#[test]
fn import_project_clears_the_object_matte_when_the_input_roto_freeze_is_discarded() {
    check_imported_object_matte(true);
}

fn check_imported_object_matte(with_roto_brush: bool) {
    use effectcraft_effects::{
        object_matte as om,
        roto::{self, FROZEN, FrozenCache, INPUT_KEY},
    };
    use effectcraft_keyframe::Value;
    use effectcraft_project::LayerId;

    let dir = out_dir(if with_roto_brush { "import-project-roto-stack" } else { "import-project-frozen-object-matte" });
    let width = 18;
    let height = 12;
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Matte template", "width": width, "height": height, "frameRate": 25, "duration": 1})).unwrap();
    let comp = s.active_comp_id().unwrap();
    std::fs::write(dir.join("clip.mov"), b"placeholder footage").unwrap();
    let item = std::sync::Arc::make_mut(&mut s.project).add_item(
        "Source",
        Default::default(),
        None,
        ItemKind::Footage(Footage { path: "clip.mov".into(), kind: FootageKind::Video, width: width as u32, height: height as u32, ..Default::default() }),
    );
    let layer = LayerId(s.execute("layer.addItem", json!({"item": item.0, "time": 0.0})).unwrap()["layer"].as_u64().unwrap());
    if with_roto_brush {
        s.execute("roto.stroke", json!({"layer": layer.0, "points": [[7, 6]], "radius": 3})).unwrap();
        let c = s.project.comp(comp).unwrap();
        let l = c.layer(layer).unwrap();
        let brush = l.effects().unwrap().groups().find(|g| crate::roto::is_roto(g)).unwrap();
        let uid = brush.uid;
        let params = crate::roto::params_static(brush);
        let key = roto::frozen_key(&params, crate::roto::layer_size(&s.project, c, l), c.frame_rate.as_f64());
        let mut cache = FrozenCache { key, size: [width, height], ..Default::default() };
        cache.frames.insert(0, roto::freeze_frame(&vec![1.0; width * height], &[]));
        let brush = std::sync::Arc::make_mut(&mut s.project).comp_mut(comp).unwrap().layer_mut(layer).unwrap().props.find_group_mut(uid).unwrap();
        brush.get_mut(FROZEN).unwrap().value = Value::Str(cache.to_json());
    }
    let effect = s.execute("objectMatte.apply", json!({"layer": layer.0, "point": [7, 6]})).unwrap()["effect"].as_u64().unwrap();
    let c = s.project.comp(comp).unwrap();
    let l = c.layer(layer).unwrap();
    let g = l.props.find_group(effect).unwrap();
    let params = crate::roto::params_static(g);
    let old_input = params.get(INPUT_KEY).unwrap().clone();
    let old_source = l.source.item().unwrap();
    let layer_size = crate::roto::layer_size(&s.project, c, l);
    let fps = c.frame_rate.as_f64();
    let alpha: Vec<_> = (0..width * height).map(|i| if i % width < width / 2 { 1.0 } else { 0.0 }).collect();
    let mut frozen = FrozenCache { key: om::frozen_key(&params, layer_size, fps, None), size: [width, height], ..Default::default() };
    frozen.frames.insert(0, roto::freeze_frame(&alpha, &[]));
    let group = std::sync::Arc::make_mut(&mut s.project).comp_mut(comp).unwrap().layer_mut(layer).unwrap().props.find_group_mut(effect).unwrap();
    group.get_mut(FROZEN).unwrap().value = Value::Str(frozen.to_json());
    let source_params = crate::roto::params_static(group);
    assert_eq!(om::frozen_alpha(&source_params, layer_size, fps, 0, width, height, None), Some(alpha.clone()));
    let source = dir.join("Source.ecproj").to_string_lossy().into_owned();
    s.execute("file.saveAs", json!({"path": source})).unwrap();

    let mut destination = Session::default();
    destination.execute("comp.new", json!({"name": "Destination"})).unwrap();
    let imported = destination.execute("file.importProject", json!({"path": source})).unwrap();
    let comp = ItemId(imported["primaryComp"].as_u64().unwrap());
    assert_eq!(imported["missingMedia"], json!([]));
    if with_roto_brush {
        let l = &destination.project.comp(comp).unwrap().layers[0];
        let matte = l.effects().unwrap().groups().find(|g| g.match_id == om::ID).unwrap();
        assert_eq!(matte.get(FROZEN).unwrap().value, Value::Str(String::new()));
        assert_eq!(matte.get(INPUT_KEY).unwrap().value, Value::Str(crate::roto::input_key(&destination.project, comp, l, matte.uid)));
        assert!(destination.roto_pending.contains(&(comp, l.id, matte.uid)));
        return;
    }
    assert!(destination.roto_pending.is_empty(), "the imported freeze needs no propagation");
    let path = dir.join("Destination.ecproj").to_string_lossy().into_owned();
    destination.execute("file.saveAs", json!({"path": path})).unwrap();

    let mut reopened = Session::default();
    reopened.execute("file.open", json!({"path": path})).unwrap();
    let c = reopened.project.comp(comp).unwrap();
    let l = &c.layers[0];
    assert_ne!(l.id, layer);
    assert_ne!(l.source.item(), Some(old_source));
    let effect = l.effects().unwrap().groups().find(|g| g.match_id == om::ID).unwrap();
    let params = crate::roto::params_static(effect);
    assert_ne!(params.get(INPUT_KEY), Some(&old_input));
    assert_eq!(om::frozen_alpha(&params, layer_size, fps, 0, width, height, None), Some(alpha));

    let (layer, effect) = (l.id, effect.uid);
    assert!(crate::roto::imported_freeze_matches(&reopened.project, comp, layer, effect));
    // A payload can still be present when an earlier sync changes its input. It must stay queued.
    let mut changed = (*reopened.project).clone();
    changed.comp_mut(comp).unwrap().layer_mut(layer).unwrap().stretch = 200.0;
    assert!(!crate::roto::imported_freeze_matches(&changed, comp, layer, effect));
}

#[test]
fn import_discards_a_frozen_object_matte_if_its_nested_input_changes() {
    use effectcraft_effects::{
        object_matte as om,
        roto::{self, FROZEN, FrozenCache},
    };
    use effectcraft_keyframe::Value;
    use effectcraft_project::LayerId;

    let (width, height) = (18, 12);
    let alpha: Vec<_> = (0..width * height).map(|i| if i % width < width / 2 { 1.0 } else { 0.0 }).collect();
    for (case, switches, changes_nested_input) in [
        ("unchanged", None, false),
        ("changed", Some((true, true)), true),
        ("effects-off", Some((false, true)), false),
        ("effect-disabled", Some((true, false)), false),
    ] {
        let dir = out_dir(&format!("import-frozen-{case}-precomp"));
        let mut s = Session::default();
        let mut make_comp = |name: &str| {
            s.execute("comp.new", json!({"name": name, "width": width, "height": height, "frameRate": 25, "duration": 1})).unwrap();
            s.active_comp_id().unwrap()
        };
        // The frozen outer layer's comp comes before its sources in ID order.
        let outer = make_comp("Outer");
        let middle = make_comp("Middle");
        let inner = make_comp("Inner");
        let solid = LayerId(s.execute("layer.newSolid", json!({"name": "Solid", "color": "#ff0000"})).unwrap()["layer"].as_u64().unwrap());
        if let Some((effects_on, brush_enabled)) = switches {
            s.execute("roto.stroke", json!({"layer": solid.0, "points": [[7, 6]], "radius": 3})).unwrap();
            let comp = s.project.comp(inner).unwrap();
            let layer = comp.layer(solid).unwrap();
            let brush = layer.effects().unwrap().groups().find(|g| crate::roto::is_roto(g)).unwrap();
            let uid = brush.uid;
            let key = roto::frozen_key(&crate::roto::params_static(brush), [width as f64, height as f64], 25.0);
            let mut cache = FrozenCache { key, size: [width, height], ..Default::default() };
            cache.frames.insert(0, roto::freeze_frame(&alpha, &[]));
            let layer = std::sync::Arc::make_mut(&mut s.project).comp_mut(inner).unwrap().layer_mut(solid).unwrap();
            layer.switches.effects = effects_on;
            let brush = layer.props.find_group_mut(uid).unwrap();
            brush.enabled = brush_enabled;
            brush.get_mut(FROZEN).unwrap().value = Value::Str(cache.to_json());
        }
        s.open_comp(middle);
        s.execute("layer.addItem", json!({"item": inner.0, "time": 0.0})).unwrap();
        s.open_comp(outer);
        let layer = LayerId(s.execute("layer.addItem", json!({"item": middle.0, "time": 0.0})).unwrap()["layer"].as_u64().unwrap());
        let effect = s.execute("objectMatte.apply", json!({"layer": layer.0, "point": [7, 6]})).unwrap()["effect"].as_u64().unwrap();
        let g = s.project.comp(outer).unwrap().layer(layer).unwrap().props.find_group(effect).unwrap();
        let key = om::frozen_key(&crate::roto::params_static(g), [width as f64, height as f64], 25.0, None);
        let mut cache = FrozenCache { key, size: [width, height], ..Default::default() };
        cache.frames.insert(0, roto::freeze_frame(&alpha, &[]));
        let g = std::sync::Arc::make_mut(&mut s.project).comp_mut(outer).unwrap().layer_mut(layer).unwrap().props.find_group_mut(effect).unwrap();
        g.get_mut(FROZEN).unwrap().value = Value::Str(cache.to_json());
        let path = dir.join("Source.ecproj");
        s.execute("file.saveAs", json!({"path": path})).unwrap();

        let mut destination = Session::default();
        destination.execute("comp.new", json!({"name": "Destination"})).unwrap();
        let imported = destination.execute("file.importProject", json!({"path": path})).unwrap();
        let comp = ItemId(imported["primaryComp"].as_u64().unwrap());
        let layer = &destination.project.comp(comp).unwrap().layers[0];
        let matte = layer.effects().unwrap().groups().find(|g| g.match_id == om::ID).unwrap();
        if switches.is_some() {
            let middle = destination.project.comp(layer.source.item().unwrap()).unwrap();
            let inner = destination.project.comp(middle.layers[0].source.item().unwrap()).unwrap();
            let brush = inner.layers[0].effects().unwrap().groups().find(|g| crate::roto::is_roto(g)).unwrap();
            assert_eq!(brush.get(FROZEN).unwrap().value, Value::Str(String::new()), "{case}: the inner freeze is cleared on import");
        }
        let params = crate::roto::params_static(matte);
        let actual = om::frozen_alpha(&params, [width as f64, height as f64], 25.0, 0, width, height, None);
        assert_eq!(destination.roto_pending.contains(&(comp, layer.id, matte.uid)), changes_nested_input);
        assert_eq!(actual, (!changes_nested_input).then(|| alpha.clone()), "only a freeze whose precomp changed is discarded");
    }
}

#[test]
fn import_project_merges_with_fresh_ids_and_one_undo() {
    let dir = out_dir("import-project");
    let path = source_project(&dir);

    let mut t = Session::default();
    t.execute("comp.new", json!({"name": "Show", "width": 64, "height": 64, "frameRate": 30, "duration": 2})).unwrap();
    t.execute("layer.newSolid", json!({"name": "Back", "color": "#00ff00"})).unwrap();
    let before = t.project.items.len();
    let before_ids: HashSet<u64> = all_ids(&t.project).into_iter().collect();

    let r = t.execute("file.importProject", json!({"path": path})).unwrap();
    let folder = ItemId(r["folder"].as_u64().unwrap());
    assert_eq!(t.project.item(folder).unwrap().name, "Library Card.ecproj");
    // Inner, Main, the Solids folder and its solid, two footage items, and the new folder.
    assert_eq!(t.project.items.len(), before + 7);
    let imported: Vec<ItemId> = r["items"].as_array().unwrap().iter().map(|v| ItemId(v.as_u64().unwrap())).collect();
    // Everything lands in the folder (the Solids folder keeps its solid).
    let top = |mut id: ItemId| {
        while let Some(up) = t.project.item(id).and_then(|i| i.parent).filter(|p| *p != folder) {
            id = up;
        }
        t.project.item(id).and_then(|i| i.parent)
    };
    assert!(imported.iter().all(|id| top(*id) == Some(folder)), "everything lands in the folder");

    // No id is taken twice, and none of the open project's ids moved.
    let ids = all_ids(&t.project);
    let unique: HashSet<u64> = ids.iter().copied().collect();
    assert_eq!(ids.len(), unique.len(), "ids collide");
    assert!(before_ids.is_subset(&unique));
    assert!(t.project.next_id > *ids.iter().max().unwrap());

    // References follow the new ids: Main nests the imported Inner, under the imported null.
    let main = ItemId(r["primaryComp"].as_u64().unwrap());
    assert_eq!(t.project.item(main).unwrap().name, "Main");
    assert_eq!(r["rootComps"], json!([main.0]));
    let comp = t.project.comp(main).unwrap();
    let nest = comp.layers.iter().find(|l| matches!(l.source, LayerSource::Comp { .. })).unwrap();
    let LayerSource::Comp { item } = nest.source else { panic!("a precomp layer") };
    assert_eq!(t.project.item(item).unwrap().name, "Inner");
    assert!(imported.contains(&item));
    let parent = nest.parent.unwrap();
    assert_eq!(comp.layer(parent).unwrap().name, "Rig");

    // A relative footage path resolves next to the imported project; a missing one is reported.
    let data = t.project.items.values().find(|i| i.name == "data.json").unwrap();
    let ItemKind::Footage(f) = &data.kind else { panic!("footage") };
    assert_eq!(std::fs::canonicalize(&f.path).unwrap(), std::fs::canonicalize(dir.join("data.json")).unwrap());
    assert_eq!(r["missingMedia"], json!(["/offline/gone.mov"]));

    // One undo step removes all of it; redo brings it back.
    t.execute("edit.undo", json!({})).unwrap();
    assert_eq!(t.project.items.len(), before);
    t.execute("edit.redo", json!({})).unwrap();
    assert_eq!(t.project.items.len(), before + 7);

    // Importing again keeps the copies apart, and File ▸ Import routes .ecproj here.
    let r2 = t.execute("file.import", json!({"paths": [path]})).unwrap();
    let again = &r2["projects"][0];
    assert_ne!(again["primaryComp"], r["primaryComp"]);
    assert!(r2["items"].as_array().unwrap().is_empty(), "a project is not footage");
    let ids = all_ids(&t.project);
    assert_eq!(ids.len(), ids.iter().copied().collect::<HashSet<_>>().len());
}

#[test]
fn import_project_rejects_bad_files_and_leaves_the_project() {
    let dir = out_dir("import-project-bad");
    let mut t = Session::default();
    t.execute("comp.new", json!({"name": "Show", "width": 64, "height": 64, "frameRate": 30, "duration": 2})).unwrap();
    let before = t.project.items.len();

    std::fs::write(dir.join("junk.ecproj"), b"not a project").unwrap();
    assert!(t.execute("file.importProject", json!({"path": dir.join("junk.ecproj").to_string_lossy()})).is_err());
    assert!(t.execute("file.importProject", json!({"path": dir.join("absent.ecproj").to_string_lossy()})).is_err());

    // Regression: an id at the top of the range saturated instead of overflowing (opening and
    // importing must not panic), and the import refuses it.
    let mut p = Project::default();
    let id = p.add_item("Huge", Default::default(), None, ItemKind::Folder);
    let mut it = p.items.remove(&id).unwrap();
    it.id = ItemId(u64::MAX);
    p.items.insert(it.id, it);
    let huge = dir.join("huge.ecproj");
    std::fs::write(&huge, p.to_json()).unwrap();
    assert_eq!(Project::from_json(&p.to_json()).unwrap().next_id, u64::MAX);
    let e = t.execute("file.importProject", json!({"path": huge.to_string_lossy()})).unwrap_err();
    assert!(e.to_string().contains("out of range"), "{e}");
    assert_eq!(t.project.items.len(), before);
}
