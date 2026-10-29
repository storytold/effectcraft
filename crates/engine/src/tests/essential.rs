//! Essential Graphics: authoring controls, master properties (per-instance overrides, Push to
//! Comp, Revert), `.ectemplate` export/import, and Responsive Design — Time.

use effectcraft_project::essential;
use effectcraft_project::{ItemId, LayerId};
use effectcraft_time::Tick;
use serde_json::{Value, json};

use crate::Session;
use crate::render::RenderOpts;

fn out_dir(name: &str) -> std::path::PathBuf {
    let d = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-out").join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A "Card" comp (64×64): a white solid with a Fill effect (red) and a text layer; its Fill
/// colour, opacity and text are exposed. Returns (session, card comp, solid layer).
fn card() -> (Session, ItemId, u64, Value) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Card", "width": 64, "height": 64, "frameRate": 30, "duration": 2})).unwrap();
    let card = s.active_comp_id().unwrap();
    let solid = s.execute("layer.newSolid", json!({"name": "BG", "color": "#ffffff"})).unwrap()["layer"].as_u64().unwrap();
    s.execute("effect.apply", json!({"layer": solid, "effect": "Fill"})).unwrap();
    let text = s.execute("layer.newText", json!({"text": "Hello"})).unwrap()["layer"].as_u64().unwrap();
    let color = s.execute("essential.addProperty", json!({"layer": solid, "path": "effects/#1/color", "name": "Card Color"})).unwrap();
    let opacity = s.execute("essential.addProperty", json!({"layer": solid, "path": "transform/opacity"})).unwrap();
    let g = s.execute("essential.addGroup", json!({"name": "Words"})).unwrap()["control"].as_u64().unwrap();
    let title = s.execute("essential.addProperty", json!({"layer": text, "path": "text/sourceText", "group": g})).unwrap();
    s.execute("essential.addComment", json!({"text": "Pick a colour"})).unwrap();
    s.execute("essential.setName", json!({"name": "Lower Card"})).unwrap();
    let ids = json!({
        "color": color["controls"][0], "opacity": opacity["controls"][0], "group": g, "title": title["controls"][0],
    });
    (s, card, solid, ids)
}

fn center(s: &Session, comp: ItemId) -> [f32; 4] {
    let img = s.render(comp, Tick::ZERO, RenderOpts::default());
    img.get(img.width as i64 / 2, img.height as i64 / 2)
}

#[test]
fn controls_are_listed_typed_and_editable() {
    let (mut s, card, solid, ids) = card();
    let l = s.execute("essential.list", json!({"supported": true})).unwrap();
    assert_eq!(l["name"], "Lower Card");
    let top: Vec<&str> = l["controls"].as_array().unwrap().iter().map(|c| c["kind"].as_str().unwrap()).collect();
    assert_eq!(top, ["property", "property", "group", "comment"]);
    assert_eq!(l["controls"][0]["name"], "Card Color");
    assert_eq!(l["controls"][0]["type"], "color");
    assert_eq!(l["controls"][1]["type"], "slider");
    assert_eq!(l["controls"][2]["children"][0]["type"], "text");
    assert!(l["supported"].as_array().unwrap().iter().any(|p| p["path"] == "transform/position" && p["type"] == "point"));
    assert!(l["supported"].as_array().unwrap().iter().any(|p| p["path"] == "transform/rotation" && p["type"] == "angle"));
    // Rename, move into the group, remove; all undoable.
    let opacity = ids["opacity"].as_u64().unwrap();
    s.execute("essential.rename", json!({"control": opacity, "name": "Fade"})).unwrap();
    s.execute("essential.move", json!({"control": opacity, "group": ids["group"], "index": 0})).unwrap();
    let l = s.execute("essential.list", json!({})).unwrap();
    assert_eq!(l["controls"][1]["children"][0]["name"], "Fade");
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    let l = s.execute("essential.list", json!({})).unwrap();
    assert_eq!(l["controls"][1]["name"], "Opacity");
    s.execute("essential.remove", json!({"control": opacity})).unwrap();
    assert_eq!(s.execute("essential.list", json!({})).unwrap()["controls"].as_array().unwrap().len(), 3);
    // Unsupported properties are refused; adding the same property twice adds a mirror (or
    // nothing with `mirror: false`).
    s.execute("layer.addMask", json!({"layer": solid})).unwrap();
    assert!(s.execute("essential.addProperty", json!({"layer": solid, "path": "masks/#1/path"})).is_err());
    let again = s.execute("essential.addProperty", json!({"layer": solid, "path": "effects/#1/color", "mirror": false})).unwrap();
    assert_eq!(again["controls"], json!([]));
    let again = s.execute("essential.addProperty", json!({"layer": solid, "path": "effects/#1/color"})).unwrap();
    assert_eq!(again["mirrors"], again["controls"]);
    s.execute("edit.undo", json!({})).unwrap();
    // Dropdown Menu Control: its items are editable and it becomes a dropdown control.
    s.execute("effect.apply", json!({"layer": solid, "effect": "Dropdown Menu Control"})).unwrap();
    s.execute("effect.editDropdown", json!({"layer": solid, "items": ["Small", "Medium", "Large", "Huge"]})).unwrap();
    let d = s.execute("essential.addProperty", json!({"layer": solid, "path": "effects/#2/menu"})).unwrap()["controls"][0].as_u64().unwrap();
    let l = s.execute("essential.list", json!({})).unwrap();
    let c = l["controls"].as_array().unwrap().iter().find(|c| c["id"] == d).unwrap().clone();
    assert_eq!(c["type"], "dropdown");
    let comp = s.project.comp(card).unwrap();
    let menu = comp.layer(LayerId(solid)).unwrap().props.prop("effects/#2/menu").unwrap();
    assert!(matches!(&menu.ui, effectcraft_project::ParamUi::Popup { options } if options.len() == 4 && options[3] == "Huge"));
}

#[test]
fn master_properties_override_per_instance_push_and_revert() {
    let (mut s, card, solid, ids) = card();
    s.execute("comp.new", json!({"name": "Main", "width": 64, "height": 64, "frameRate": 30, "duration": 2})).unwrap();
    let main = s.active_comp_id().unwrap();
    let a = s.execute("layer.addItem", json!({"item": card.0})).unwrap()["layer"].as_u64().unwrap();
    let b = s.execute("layer.addItem", json!({"item": card.0})).unwrap()["layer"].as_u64().unwrap();
    // Both instances show the controls under Essential Properties.
    let inst = s.execute("essential.instance", json!({"layer": a})).unwrap();
    let names: Vec<&str> = inst["controls"].as_array().unwrap().iter().map(|c| c["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["Card Color", "Opacity", "Words", "Source Text", "Comment"]);
    assert_eq!(inst["controls"][0]["value"], json!([1.0, 0.0, 0.0, 1.0]));
    // Override the colour of the top instance (a = layer #2, b on top is #1).
    s.execute("layer.select", json!({"layers": [b]})).unwrap();
    s.execute("essential.set", json!({"layer": b, "control": ids["color"], "value": [0.0, 0.0, 1.0, 1.0]})).unwrap();
    let px = center(&s, main);
    assert!(px[2] > 0.99 && px[0] < 0.01, "instance b is blue: {px:?}");
    let inst_b = s.execute("essential.instance", json!({"layer": b})).unwrap();
    assert_eq!(inst_b["controls"][0]["overridden"], true);
    assert_eq!(s.execute("essential.instance", json!({"layer": a})).unwrap()["controls"][0]["overridden"], false);
    // The source comp is untouched; hiding b shows a still red.
    assert!(center(&s, card)[0] > 0.99);
    s.execute("layer.setSwitch", json!({"layer": b, "switch": "video", "value": false})).unwrap();
    let px = center(&s, main);
    assert!(px[0] > 0.99 && px[2] < 0.01, "instance a stays red: {px:?}");
    s.execute("edit.undo", json!({})).unwrap();
    // Editing the master property in the timeline (prop.set) also overrides.
    let path = inst_b["controls"][1]["path"].as_str().unwrap().to_string();
    assert!(path.starts_with("essential/"), "{path}");
    s.execute("prop.set", json!({"layer": a, "path": path, "value": 50})).unwrap();
    assert_eq!(s.execute("essential.instance", json!({"layer": a})).unwrap()["controls"][1]["overridden"], true);
    // A change to the source shows in instances that don't override it.
    s.execute("prop.set", json!({"comp": card.0, "layer": solid, "path": "transform/opacity", "value": 80})).unwrap();
    assert_eq!(s.execute("essential.instance", json!({"layer": b})).unwrap()["controls"][1]["value"], json!(80.0));
    assert_eq!(s.execute("essential.instance", json!({"layer": a})).unwrap()["controls"][1]["value"], json!(50.0));
    // Revert a's opacity.
    s.execute("essential.revert", json!({"layer": a, "control": ids["opacity"]})).unwrap();
    let inst = s.execute("essential.instance", json!({"layer": a})).unwrap();
    assert_eq!(inst["controls"][1]["overridden"], false);
    assert_eq!(inst["controls"][1]["value"], json!(80.0));
    // Push b's colour to the source comp: the override goes away and every instance is blue.
    s.execute("essential.pushToComp", json!({"layer": b})).unwrap();
    let src = s.project.comp(card).unwrap().layer(LayerId(solid)).unwrap().props.prop("effects/#1/color").unwrap().value.clone();
    assert_eq!(src.to_json(), json!([0.0, 0.0, 1.0, 1.0]));
    assert_eq!(s.execute("essential.instance", json!({"layer": b})).unwrap()["controls"][0]["overridden"], false);
    assert_eq!(s.execute("essential.instance", json!({"layer": a})).unwrap()["controls"][0]["value"], json!([0.0, 0.0, 1.0, 1.0]));
    // Undo restores the override and the red source.
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.execute("essential.instance", json!({"layer": b})).unwrap()["controls"][0]["overridden"], true);
    let src = s.project.comp(card).unwrap().layer(LayerId(solid)).unwrap().props.prop("effects/#1/color").unwrap().value.clone();
    assert_eq!(src.to_json(), json!([1.0, 0.0, 0.0, 1.0]));
    // Removing a control removes it from instances.
    s.execute("essential.remove", json!({"comp": card.0, "control": ids["color"]})).unwrap();
    let l = s.project.comp(main).unwrap().layer(LayerId(b)).unwrap().clone();
    assert!(essential::group(&l).unwrap().children.iter().all(|n| n.name() != "Card Color"));
}

#[test]
fn mirrored_and_linked_properties() {
    let (mut s, card, solid, ids) = card();
    let opacity = ids["opacity"].as_u64().unwrap();
    // A second solid whose opacity the Opacity control drives too.
    let bg2 = s.execute("layer.newSolid", json!({"name": "BG2", "color": "#00ff00"})).unwrap()["layer"].as_u64().unwrap();
    s.execute("essential.linkProperty", json!({"control": opacity, "layer": bg2, "path": "transform/opacity"})).unwrap();
    // Not the same kind of property; already driven.
    assert!(s.execute("essential.linkProperty", json!({"control": opacity, "layer": bg2, "path": "transform/position"})).is_err());
    assert!(s.execute("essential.linkProperty", json!({"control": ids["color"], "layer": bg2, "path": "transform/opacity"})).is_err());
    let opa = |s: &Session, l: u64| s.project.comp(card).unwrap().layer(LayerId(l)).unwrap().props.prop("transform/opacity").unwrap().value.as_f64();
    // The link follows the main property, and the main property follows the link.
    s.execute("prop.set", json!({"comp": card.0, "layer": solid, "path": "transform/opacity", "value": 40})).unwrap();
    assert_eq!(opa(&s, bg2), 40.0);
    s.execute("prop.set", json!({"comp": card.0, "layer": bg2, "path": "transform/opacity", "value": 70})).unwrap();
    assert_eq!(opa(&s, solid), 70.0);
    let l = s.execute("essential.list", json!({})).unwrap();
    let c = l["controls"].as_array().unwrap().iter().find(|c| c["id"] == opacity).unwrap().clone();
    assert_eq!(c["links"][0]["layer"], bg2);
    assert_eq!(c["links"][0]["path"], "transform/opacity");
    // Adding the Opacity again (into the Words group) mirrors it.
    let m = s.execute("essential.addProperty", json!({"layer": solid, "path": "transform/opacity", "group": ids["group"], "name": "Fade"})).unwrap();
    let mirror = m["mirrors"][0].as_u64().unwrap();
    let l = s.execute("essential.list", json!({})).unwrap();
    let mc = &l["controls"][2]["children"][1];
    assert_eq!((mc["id"].as_u64(), mc["kind"].as_str(), mc["of"].as_u64(), mc["name"].as_str()), (Some(mirror), Some("mirror"), Some(opacity), Some("Fade")));
    assert_eq!(mc["value"], json!(70.0));
    // A mirror of the mirror shows the same master; the linked solid also counts as present.
    let m2 = s.execute("essential.addMirror", json!({"control": mirror})).unwrap();
    assert_eq!(m2["of"], opacity);
    let again = s.execute("essential.addProperty", json!({"layer": bg2, "path": "transform/opacity"})).unwrap();
    assert_eq!(again["mirrors"].as_array().unwrap().len(), 1);
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    // Instances: setting the mirror sets the master (and both links) for that instance only.
    s.execute("comp.new", json!({"name": "Main", "width": 64, "height": 64, "frameRate": 30, "duration": 2})).unwrap();
    let main = s.active_comp_id().unwrap();
    let a = s.execute("layer.addItem", json!({"item": card.0})).unwrap()["layer"].as_u64().unwrap();
    let inst = s.execute("essential.instance", json!({"layer": a})).unwrap();
    let find = |inst: &Value, id: u64| inst["controls"].as_array().unwrap().iter().find(|c| c["control"] == id).unwrap().clone();
    assert_eq!(find(&inst, mirror)["kind"], "mirror");
    s.execute("essential.set", json!({"layer": a, "control": mirror, "value": 20})).unwrap();
    let inst = s.execute("essential.instance", json!({"layer": a})).unwrap();
    assert_eq!((find(&inst, opacity)["value"].clone(), find(&inst, opacity)["overridden"].clone()), (json!(20.0), json!(true)));
    assert_eq!((find(&inst, mirror)["value"].clone(), find(&inst, mirror)["overridden"].clone()), (json!(20.0), json!(true)));
    // Rendered: both solids at 20 % (green BG2 on top of the red-filled solid).
    let px = center(&s, main);
    assert!((px[3] - (1.0 - 0.8 * 0.8)).abs() < 0.02, "{px:?}");
    assert_eq!(opa(&s, solid), 70.0, "the source comp keeps its value");
    // Setting the master updates the mirror.
    s.execute("essential.set", json!({"layer": a, "control": opacity, "value": 55})).unwrap();
    assert_eq!(find(&s.execute("essential.instance", json!({"layer": a})).unwrap(), mirror)["value"], json!(55.0));
    // Reverting the mirror reverts the master too.
    s.execute("essential.revert", json!({"layer": a, "control": mirror})).unwrap();
    let inst = s.execute("essential.instance", json!({"layer": a})).unwrap();
    assert_eq!((find(&inst, opacity)["overridden"].clone(), find(&inst, mirror)["value"].clone()), (json!(false), json!(70.0)));
    // Push to Comp from a mirror writes the master and its links.
    s.execute("essential.set", json!({"layer": a, "control": mirror, "value": 35})).unwrap();
    s.execute("essential.pushToComp", json!({"layer": a, "control": mirror})).unwrap();
    assert_eq!((opa(&s, solid), opa(&s, bg2)), (35.0, 35.0));
    // Saved and loaded with the project.
    let back = effectcraft_project::Project::from_json(&s.project.to_json()).unwrap();
    assert_eq!(back.comp(card).unwrap().essential, s.project.comp(card).unwrap().essential);
    // Unlink; removing the master removes its mirrors.
    s.execute("essential.unlinkProperty", json!({"comp": card.0, "control": opacity, "layer": bg2, "path": "transform/opacity"})).unwrap();
    s.execute("prop.set", json!({"comp": card.0, "layer": solid, "path": "transform/opacity", "value": 10})).unwrap();
    assert_eq!(opa(&s, bg2), 35.0);
    let r = s.execute("essential.remove", json!({"comp": card.0, "control": opacity})).unwrap();
    assert_eq!(r["mirrors"], json!([mirror]));
    let eg = s.project.comp(card).unwrap().essential.clone().unwrap();
    assert!(eg.find(mirror).is_none() && essential::group(s.project.comp(main).unwrap().layer(LayerId(a)).unwrap()).is_some());
}

#[test]
fn template_export_import_round_trip_with_overrides() {
    let dir = out_dir("ectemplate");
    let (mut s, card, _solid, ids) = card();
    let path = dir.join("Lower Card.ectemplate").to_string_lossy().to_string();
    let r = s.execute("essential.exportTemplate", json!({"comp": card.0, "path": path})).unwrap();
    assert_eq!(r["name"], "Lower Card");
    let info = s.execute("essential.templateInfo", json!({"path": path})).unwrap();
    assert_eq!(info["format"], "effectcraft-template");
    assert_eq!(info["width"], 64);
    let kinds: Vec<&str> = info["controls"].as_array().unwrap().iter().map(|c| c["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["property", "property", "group", "property", "comment"]);
    assert_eq!(info["controls"][3]["group"], ids["group"]);
    let entries = effectcraft_lottie::zip::read_stored(&std::fs::read(&path).unwrap());
    let names: Vec<&str> = entries.iter().map(|(n, _)| n.as_str()).collect();
    assert!(names.contains(&"manifest.json") && names.contains(&"project.ecproj") && names.contains(&"poster.png"), "{names:?}");

    // A fresh project imports it and gets an instance in its active comp.
    let mut t = Session::default();
    t.execute("comp.new", json!({"name": "Show", "width": 64, "height": 64, "frameRate": 30, "duration": 2})).unwrap();
    let show = t.active_comp_id().unwrap();
    t.execute("layer.newSolid", json!({"name": "Back", "color": "#00ff00"})).unwrap();
    let r = t.execute("essential.importTemplate", json!({"path": path})).unwrap();
    let tcomp = ItemId(r["comp"].as_u64().unwrap());
    let inst = r["layer"].as_u64().unwrap();
    assert_eq!(t.project.item(tcomp).unwrap().name, "Card");
    assert!(t.project.items.values().any(|i| i.is_folder() && i.name == "Lower Card"));
    // Ids were remapped consistently: the template comp's controls point at its own layers.
    let eg = t.project.comp(tcomp).unwrap().essential.clone().unwrap();
    for c in eg.flat() {
        if let essential::EgKind::Property { .. } = c.kind {
            assert!(essential::source_prop(&t.project, tcomp, c).is_some(), "{c:?}");
        }
    }
    let px = center(&t, show);
    assert!(px[0] > 0.99 && px[1] < 0.01, "the card renders red over green: {px:?}");
    // Override on the imported instance, then save/load keeps it.
    let color_ctl = eg.flat().iter().find(|c| c.name == "Card Color").unwrap().id;
    t.execute("essential.set", json!({"layer": inst, "control": color_ctl, "value": "#ffff00"})).unwrap();
    let px = center(&t, show);
    assert!(px[0] > 0.99 && px[1] > 0.99 && px[2] < 0.01, "yellow: {px:?}");
    let saved = effectcraft_project::Project::from_json(&t.project.to_json()).unwrap();
    let mut u = Session::default();
    u.replace_project(saved, None);
    u.open_comp(show);
    let px = center(&u, show);
    assert!(px[0] > 0.99 && px[1] > 0.99 && px[2] < 0.01, "override survives save/load: {px:?}");
    // Importing again keeps both copies apart (fresh ids).
    let r2 = t.execute("essential.importTemplate", json!({"path": path, "addToComp": false})).unwrap();
    assert_ne!(r2["comp"], r["comp"]);
    assert!(r2["layer"].is_null());
    // Not a template.
    std::fs::write(dir.join("bad.ectemplate"), b"nope").unwrap();
    assert!(t.execute("essential.importTemplate", json!({"path": dir.join("bad.ectemplate").to_string_lossy()})).is_err());
}

#[test]
fn template_import_writes_media_only_into_its_media_folder() {
    let root = out_dir("ectemplate-names");
    let dir = root.join("a").join("b");
    std::fs::create_dir_all(&dir).unwrap();
    let (mut s, card, _solid, _ids) = card();
    let src = dir.join("Src.ectemplate");
    s.execute("essential.exportTemplate", json!({"comp": card.0, "path": src.to_string_lossy()})).unwrap();
    let mut entries = effectcraft_lottie::zip::read_stored(&std::fs::read(&src).unwrap());
    std::fs::remove_file(&src).unwrap();
    let abs = root.join("abs").join("x").to_string_lossy().replace('\\', "/");
    for name in [
        "../x",
        "media/../../x",
        "media/1/../../../x",
        &format!("media/{abs}"),
        &format!("media/6{abs}"),
        "media/C:\\x",
        "media/7/C:\\x",
        "media/x/clip.png",
        "media/8/sub/clip.png",
        "media/2/..",
        "media/3/",
        "media/4/clip.png",
        "media/5/clip.png",
    ] {
        entries.push((name.to_string(), b"data".to_vec()));
    }
    let path = dir.join("T.ectemplate");
    std::fs::write(&path, effectcraft_lottie::zip::store(&entries)).unwrap();
    s.execute("essential.importTemplate", json!({"path": path.to_string_lossy()})).unwrap();

    let media = dir.join("T Media");
    fn files(d: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for e in std::fs::read_dir(d).unwrap().map(|e| e.unwrap().path()) {
            if e.is_dir() { files(&e, out) } else { out.push(e) }
        }
    }
    let mut found = vec![];
    files(&root, &mut found);
    let in_item = |f: &std::path::Path| f.parent().and_then(std::path::Path::parent) == Some(media.as_path());
    for f in &found {
        assert!(f == &path || in_item(f), "written outside an item folder of the media folder: {f:?}");
    }
    let mut names: Vec<String> = found.iter().filter(|f| in_item(f)).map(|f| f.strip_prefix(&media).unwrap().to_string_lossy().replace('\\', "/")).collect();
    names.sort();
    assert_eq!(names, ["4/clip.png", "5/clip.png"]);
}

#[test]
fn media_replacement_swaps_footage_per_instance() {
    use std::sync::Arc;
    // Two "footage" items rendered by a fake source: item → solid colour.
    struct Colors;
    impl crate::render::FootageSource for Colors {
        fn frame(&self, _: ItemId, f: &effectcraft_project::Footage, _: Tick) -> Option<Arc<effectcraft_raster::Image>> {
            let c = if f.path.contains("red") { [1.0, 0.0, 0.0, 1.0] } else { [0.0, 0.0, 1.0, 1.0] };
            Some(Arc::new(effectcraft_raster::Image::filled(16, 16, c)))
        }
    }
    let mut s = Session { footage: Arc::new(Colors), ..Default::default() };
    let foot = |p: &str| effectcraft_project::Footage {
        path: p.into(),
        kind: effectcraft_project::FootageKind::Still,
        width: 16,
        height: 16,
        has_video: true,
        ..Default::default()
    };
    let (red, blue) = s
        .edit("x", None, |proj, _| {
            let r = proj.add_item("red.png", Default::default(), None, effectcraft_project::ItemKind::Footage(foot("red.png")));
            let b = proj.add_item("blue.png", Default::default(), None, effectcraft_project::ItemKind::Footage(foot("blue.png")));
            Ok((r, b))
        })
        .unwrap();
    s.execute("comp.new", json!({"name": "Slot", "width": 16, "height": 16, "frameRate": 30, "duration": 1})).unwrap();
    let slot = s.active_comp_id().unwrap();
    let fl = s.execute("layer.addItem", json!({"item": red.0})).unwrap()["layer"].as_u64().unwrap();
    let m = s.execute("essential.addMedia", json!({"layer": fl})).unwrap()["control"].as_u64().unwrap();
    s.execute("comp.new", json!({"name": "Main", "width": 16, "height": 16, "frameRate": 30, "duration": 1})).unwrap();
    let main = s.active_comp_id().unwrap();
    let inst = s.execute("layer.addItem", json!({"item": slot.0})).unwrap()["layer"].as_u64().unwrap();
    assert!(center(&s, main)[0] > 0.99);
    s.execute("essential.set", json!({"layer": inst, "control": m, "item": blue.0})).unwrap();
    let px = center(&s, main);
    assert!(px[2] > 0.99 && px[0] < 0.01, "{px:?}");
    assert!(center(&s, slot)[0] > 0.99, "the source keeps its media");
    assert_eq!(s.execute("essential.instance", json!({"layer": inst})).unwrap()["controls"][0]["type"], "media");
    s.execute("essential.revert", json!({"layer": inst})).unwrap();
    assert!(center(&s, main)[0] > 0.99);
}

/// Responsive Design — Time: stretching a precomp keeps its protected regions at their
/// original speed; the unprotected parts absorb the stretch.
#[test]
fn protected_regions_keep_speed_under_time_stretch() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Inner", "width": 32, "height": 32, "frameRate": 30, "duration": 4})).unwrap();
    let inner = s.active_comp_id().unwrap();
    // Intro 0–1 s and outro 3–4 s.
    s.execute("time.set", json!({"time": 1.0})).unwrap();
    s.execute("comp.responsiveTime", json!({"op": "intro"})).unwrap();
    s.execute("time.set", json!({"time": 3.0})).unwrap();
    s.execute("comp.responsiveTime", json!({"op": "outro"})).unwrap();
    let comp = s.project.comp(inner).unwrap().clone();
    assert_eq!(crate::render::eval::protected_regions(&comp), vec![(0.0, 1.0), (3.0, 4.0)]);
    s.execute("comp.new", json!({"name": "Outer", "width": 32, "height": 32, "frameRate": 30, "duration": 10})).unwrap();
    let outer = s.active_comp_id().unwrap();
    let l = s.execute("layer.addItem", json!({"item": inner.0})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.select", json!({"layers": [l]})).unwrap();
    s.execute("layer.timeStretch", json!({"percent": 200})).unwrap();
    let st = |s: &Session, t: f64| {
        let c = s.project.comp(outer).unwrap();
        let layer = c.layer(LayerId(l)).unwrap();
        crate::render::EvalCtx::new(&s.project, outer, c, Tick::from_seconds_f64(t)).source_time(layer).seconds()
    };
    // 8 s total: intro 1 s at speed 1, middle 2 s stretched to 6 s, outro 1 s at speed 1.
    let near = |a: f64, b: f64| (a - b).abs() < 1e-6;
    assert!(near(st(&s, 0.5), 0.5));
    assert!(near(st(&s, 1.0), 1.0));
    assert!(near(st(&s, 4.0), 2.0));
    assert!(near(st(&s, 7.0), 3.0));
    assert!(near(st(&s, 7.5), 3.5), "outro plays at full speed: {}", st(&s, 7.5));
    // Protected regions play at the original speed: one comp second = one source second.
    assert!(near(st(&s, 0.8) - st(&s, 0.3), 0.5));
    assert!(near(st(&s, 7.9) - st(&s, 7.4), 0.5));
    assert!(near(st(&s, 5.0) - st(&s, 4.0), 1.0 / 3.0), "the middle is slowed down 3×");
    // Compressing to 75 %: the middle plays faster.
    s.execute("layer.timeStretch", json!({"percent": 75})).unwrap();
    assert!(near(st(&s, 0.5), 0.5));
    assert!(near(st(&s, 1.0 + 0.5), 1.0 + 0.5 / 0.5), "{}", st(&s, 1.5));
    // Without protected regions the plain stretch applies.
    s.execute("comp.open", json!({"comp": inner.0})).ok();
    let mut p = (*s.project).clone();
    p.comp_mut(inner).unwrap().markers.clear();
    s.project = std::sync::Arc::new(p);
    assert!(near(st(&s, 1.5), 2.0));
}
