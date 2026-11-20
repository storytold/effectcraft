//! Reload Footage reaches saved linked sources through the same user command as normal media.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use effectcraft_project::{Footage, FootageKind, ItemId, ItemKind};
use effectcraft_time::Tick;
use serde_json::json;

use crate::Session;

struct Fixture {
    source: Session,
    destination: Session,
    path: PathBuf,
    main: ItemId,
    opening: ItemId,
    credits: ItemId,
    a: ItemId,
    b: ItemId,
}

fn comp(s: &mut Session, name: &str, size: u32) -> ItemId {
    let result = s.execute("comp.new", json!({"name":name,"width":size,"height":size,"duration":2,"frameRate":30})).unwrap();
    ItemId(result["comp"].as_u64().unwrap())
}

fn fixture(name: &str) -> Fixture {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-out").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Titles.ecproj");
    let mut source = Session::default();
    let opening = comp(&mut source, "Opening", 24);
    source.execute("layer.newSolid", json!({"color":"#ff0000"})).unwrap();
    let credits = comp(&mut source, "Credits", 24);
    source.execute("layer.newSolid", json!({"color":"#00ff00"})).unwrap();
    source.execute("file.saveAs", json!({"path":path.to_string_lossy()})).unwrap();

    let mut destination = Session::default();
    let main = comp(&mut destination, "Show", 96);
    let a = destination.execute("file.linkComposition", json!({"path":path.to_string_lossy(),"comp":"Opening"})).unwrap();
    let a = ItemId(a["item"].as_u64().unwrap());
    let b = destination.execute("file.linkComposition", json!({"path":path.to_string_lossy(),"comp":"Credits"})).unwrap();
    let b = ItemId(b["item"].as_u64().unwrap());
    let left = destination.execute("layer.addItem", json!({"item":a.0,"comp":main.0})).unwrap()["layer"].as_u64().unwrap();
    let right = destination.execute("layer.addItem", json!({"item":b.0,"comp":main.0})).unwrap()["layer"].as_u64().unwrap();
    destination.execute("prop.set", json!({"layer":left,"path":"transform/position","value":[24,48]})).unwrap();
    destination.execute("prop.set", json!({"layer":right,"path":"transform/position","value":[72,48]})).unwrap();
    destination.execute("prop.set", json!({"layer":left,"path":"transform/opacity","value":75})).unwrap();
    destination.execute("file.saveAs", json!({"path":dir.join("Show.ecproj").to_string_lossy()})).unwrap();
    Fixture { source, destination, path, main, opening, credits, a, b }
}

fn edit_and_save_source(f: &mut Fixture) {
    f.source.execute("comp.open", json!({"comp":f.opening.0})).unwrap();
    f.source.execute("layer.newSolid", json!({"color":"#0000ff"})).unwrap();
    f.source.execute("comp.open", json!({"comp":f.credits.0})).unwrap();
    f.source.execute("layer.newSolid", json!({"color":"#ffff00"})).unwrap();
    f.source.execute("file.save", json!({})).unwrap();
}

fn linked_footage(s: &Session, id: ItemId) -> &Footage {
    let ItemKind::Footage(f) = &s.project.item(id).unwrap().kind else { panic!("linked footage") };
    f
}

#[test]
fn reload_footage_refreshes_the_selected_links_shared_source_without_a_media_importer() {
    let mut f = fixture("linked-reload-selected");
    let before = f.destination.render(f.main, Tick::ZERO, Default::default());
    let layers = f.destination.project.comp(f.main).unwrap().layers.clone();
    let item_count = f.destination.project.items.len();
    let undo = f.destination.history.undo.len();
    let generation = linked_footage(&f.destination, f.a).linked_comp.as_ref().unwrap().generation;
    edit_and_save_source(&mut f);
    f.destination.execute("project.select", json!({"items":[f.a.0]})).unwrap();
    assert!(f.destination.importer.is_none(), "linked rendering does not require a movie decoder");
    assert!(f.destination.is_enabled("file.reloadFootage"));
    let result = f.destination.execute("file.reloadFootage", json!({})).unwrap();
    assert!(result["reloaded"].as_u64().unwrap() > 0);
    let after = f.destination.render(f.main, Tick::ZERO, Default::default());
    assert_ne!(before.data[48 * 96 + 24], after.data[48 * 96 + 24]);
    assert_ne!(before.data[48 * 96 + 72], after.data[48 * 96 + 72], "unselected comp from the same source refreshes too");
    assert_eq!(f.destination.project.comp(f.main).unwrap().layers, layers);
    assert_eq!(f.destination.project.items.len(), item_count, "source dependencies remain hidden");
    assert_eq!(f.destination.history.undo.len(), undo, "external refresh is not a destination edit");
    assert!(!f.destination.is_dirty());
    let a = linked_footage(&f.destination, f.a).linked_comp.as_ref().unwrap();
    let b = linked_footage(&f.destination, f.b).linked_comp.as_ref().unwrap();
    assert!(a.generation > generation);
    assert!(Arc::ptr_eq(a.resolved.as_ref().unwrap(), b.resolved.as_ref().unwrap()));
}

struct PlainImporter {
    path: String,
    probed: Mutex<Vec<String>>,
}

impl crate::Importer for PlainImporter {
    fn probe(&self, path: &str) -> std::result::Result<Footage, String> {
        self.probed.lock().unwrap().push(path.to_string());
        if path != self.path {
            return Err("linked projects must not be probed as movies".into());
        }
        Ok(Footage { path: path.into(), kind: FootageKind::Still, width: 16, height: 16, has_video: true, ..Default::default() })
    }
}

#[test]
fn reload_footage_keeps_ordinary_media_reload_working_in_a_mixed_selection() {
    let mut f = fixture("linked-reload-mixed");
    let path = f.path.with_file_name("ordinary.png").to_string_lossy().into_owned();
    let ordinary = f
        .destination
        .edit("Add ordinary footage", None, |project, _| {
            Ok(project.add_item(
                "Ordinary",
                Default::default(),
                None,
                ItemKind::Footage(Footage { path: path.clone(), kind: FootageKind::Still, width: 8, height: 8, has_video: true, ..Default::default() }),
            ))
        })
        .unwrap();
    let importer = Arc::new(PlainImporter { path: path.clone(), probed: Mutex::new(vec![]) });
    f.destination.importer = Some(importer.clone());
    let old_width = f.destination.project.item(ordinary).unwrap().dimensions().unwrap().0;
    let before = f.destination.render(f.main, Tick::ZERO, Default::default());
    edit_and_save_source(&mut f);
    f.destination.execute("project.select", json!({"items":[ordinary.0,f.a.0,f.b.0]})).unwrap();
    f.destination.execute("file.reloadFootage", json!({})).unwrap();
    assert!(f.destination.project.item(ordinary).unwrap().dimensions().unwrap().0 > old_width);
    assert_eq!(*importer.probed.lock().unwrap(), [path]);
    assert_ne!(f.destination.render(f.main, Tick::ZERO, Default::default()).data, before.data);
    let a = linked_footage(&f.destination, f.a).linked_comp.as_ref().unwrap();
    let b = linked_footage(&f.destination, f.b).linked_comp.as_ref().unwrap();
    assert!(Arc::ptr_eq(a.resolved.as_ref().unwrap(), b.resolved.as_ref().unwrap()));
}

#[test]
fn reload_footage_reports_a_missing_linked_source_and_keeps_the_destination_editable() {
    let mut f = fixture("linked-reload-missing");
    let layers = f.destination.project.comp(f.main).unwrap().layers.clone();
    let before = f.destination.project.clone();
    assert!(f.destination.execute("file.reloadFootage", json!({"item":f.a.0,"resolveLinks":false})).is_err());
    assert!(Arc::ptr_eq(&before, &f.destination.project), "refused bridge resolution leaves the project untouched");
    std::fs::remove_file(&f.path).unwrap();
    let error = f.destination.execute("file.reloadFootage", json!({"item":f.a.0})).unwrap_err();
    assert!(error.to_string().to_lowercase().contains("linked"), "{error}");
    assert!(linked_footage(&f.destination, f.a).missing);
    assert!(linked_footage(&f.destination, f.b).missing);
    assert!(f.destination.render_rgba8(f.main, Tick::ZERO, 0).is_err());
    assert_eq!(f.destination.project.comp(f.main).unwrap().layers, layers);
    f.destination.execute("layer.newNull", json!({})).unwrap();
    assert!(f.destination.project.comp(f.main).unwrap().layers.len() > layers.len());
}
