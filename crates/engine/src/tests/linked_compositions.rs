//! Real saved-project workflow for the manual sandbox prototype.
use std::sync::Arc;

use effectcraft_project::{Footage, FootageKind, ItemId, ItemKind};
use effectcraft_render::{FootageSource, RenderOpts};
use effectcraft_time::Tick;
use serde_json::json;

use crate::Session;

fn directory(name: &str) -> std::path::PathBuf {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-out").join(name);
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn comp(s: &mut Session, name: &str, size: u32) -> ItemId {
    s.execute("comp.new", json!({"name":name,"width":size,"height":size,"duration":2,"frameRate":30})).unwrap();
    s.active_comp_id().unwrap()
}

fn save(s: &mut Session, path: &std::path::Path) {
    s.execute("file.saveAs", json!({"path":path.to_string_lossy()})).unwrap();
}

fn link(s: &mut Session, path: &std::path::Path, name: &str) -> ItemId {
    let r = s.execute("file.linkComposition", json!({"path":path.to_string_lossy(),"comp":name})).unwrap();
    ItemId(r["item"].as_u64().unwrap())
}

fn write_png(path: &std::path::Path, image: &crate::Image) {
    image::save_buffer(path, &image.to_rgba8(), image.width, image.height, image::ColorType::Rgba8).unwrap();
}

struct RejectWrites;
impl crate::Services for RejectWrites {
    fn read_file(&self, path: &str) -> std::io::Result<Vec<u8>> {
        crate::Services::read_file(&crate::FsServices, path)
    }
    fn write_file(&self, _: &str, _: &[u8]) -> std::io::Result<()> {
        Err(std::io::Error::other("simulated save failure"))
    }
}

#[test]
fn linked_compositions_two_saved_sources_refresh_without_importing_their_layers() {
    let dir = directory("linked-compositions-demo");
    let source_path = dir.join("Titles.ecproj");
    let destination_path = dir.join("Main_Show.ecproj");
    let mut source = Session::default();
    let opening = comp(&mut source, "Opening Title", 24);
    source.execute("layer.newSolid", json!({"color":"#ff0000"})).unwrap();
    comp(&mut source, "End Credits", 24);
    source.execute("layer.newSolid", json!({"color":"#00ff00"})).unwrap();
    save(&mut source, &source_path);

    let mut destination = Session::default();
    let main = comp(&mut destination, "Main Show", 64);
    let initial_items = destination.project.items.len();
    let a = link(&mut destination, &source_path, "Opening Title");
    let b = link(&mut destination, &source_path, "End Credits");
    assert_eq!(destination.project.items.len(), initial_items + 2, "source dependencies stay hidden");
    assert!(destination.execute("comp.open", json!({"comp":a.0})).is_err());
    let left = destination.execute("layer.addItem", json!({"item":a.0,"comp":main.0})).unwrap()["layer"].as_u64().unwrap();
    let right = destination.execute("layer.addItem", json!({"item":b.0,"comp":main.0})).unwrap()["layer"].as_u64().unwrap();
    destination.execute("prop.set", json!({"layer":left,"path":"transform/position","value":[18,32]})).unwrap();
    destination.execute("prop.set", json!({"layer":right,"path":"transform/position","value":[46,32]})).unwrap();
    destination.execute("prop.set", json!({"layer":left,"path":"transform/opacity","value":75})).unwrap();
    let local = destination.project.comp(main).unwrap().layers.clone();
    save(&mut destination, &destination_path);
    let stored = std::fs::read_to_string(&destination_path).unwrap();
    assert!(!stored.contains("resolved_comp"), "project files save a reference");
    assert!(!stored.contains("__linkedSnapshots"));
    let disk = effectcraft_project::Project::from_json(&stored).unwrap();
    let ItemKind::Footage(f) = &disk.item(a).unwrap().kind else { panic!("linked footage") };
    assert!(std::path::Path::new(&f.path).is_relative());

    let before = destination.render(main, Tick::ZERO, RenderOpts::default());
    write_png(&dir.join("before.png"), &before);
    source.execute("comp.open", json!({"comp":opening.0})).unwrap();
    source.execute("layer.newSolid", json!({"color":"#0000ff"})).unwrap();
    // Unsaved source edits are invisible.
    destination.execute("project.refreshLinkedCompositions", json!({"item":a.0})).unwrap();
    assert_eq!(destination.render(main, Tick::ZERO, Default::default()).data, before.data);
    source.services = Arc::new(RejectWrites);
    assert!(source.execute("file.save", json!({})).is_err());
    destination.execute("project.refreshLinkedCompositions", json!({"item":a.0})).unwrap();
    assert_eq!(destination.render(main, Tick::ZERO, Default::default()).data, before.data, "failed saves do not publish source edits");
    source.services = Arc::new(crate::FsServices);
    save(&mut source, &source_path);
    let result = destination.execute("project.refreshLinkedCompositions", json!({"item":a.0})).unwrap();
    assert!(result["errors"].as_array().unwrap().is_empty());
    assert_eq!(destination.project.comp(main).unwrap().layers, local);
    assert!(!destination.is_dirty(), "external refresh does not edit the destination");
    let after = destination.render(main, Tick::ZERO, Default::default());
    assert_ne!(before.data, after.data);
    assert_eq!(before.data[(32 * 64 + 46) as usize], after.data[(32 * 64 + 46) as usize], "other linked comp stays green");
    write_png(&dir.join("after.png"), &after);
    let get = |id| match &destination.project.item(id).unwrap().kind {
        ItemKind::Footage(f) => f.linked_comp.as_ref().unwrap(),
        _ => panic!("footage"),
    };
    assert!(Arc::ptr_eq(get(a).resolved.as_ref().unwrap(), get(b).resolved.as_ref().unwrap()), "one shared source snapshot");
    assert_eq!(get(a).generation, get(b).generation);

    let mut reopened = Session::default();
    reopened.execute("file.open", json!({"path":destination_path.to_string_lossy()})).unwrap();
    assert_eq!(reopened.render(main, Tick::ZERO, Default::default()).data, after.data);
    assert_eq!(reopened.project.comp(main).unwrap().layers, local);
    assert_eq!(reopened.project.items.len(), initial_items + 2);
    let worker = effectcraft_project::Project::from_render_json(&destination.project.to_render_json().unwrap()).unwrap();
    assert_eq!(effectcraft_render::render_frame(&worker, main, Tick::ZERO, 1.0).data, after.data);
}

#[test]
fn linked_compositions_follow_rename_but_never_reused_item_ids() {
    let dir = directory("linked-compositions-identity");
    let path = dir.join("source.ecproj");
    let mut source = Session::default();
    let original = comp(&mut source, "Original", 16);
    save(&mut source, &path);
    let mut destination = Session::default();
    let main = comp(&mut destination, "Main", 32);
    let id = link(&mut destination, &path, "Original");
    destination.execute("layer.addItem", json!({"item":id.0})).unwrap();
    source.execute("project.rename", json!({"item":original.0,"name":"Renamed"})).unwrap();
    save(&mut source, &path);
    destination.execute("project.refreshLinkedCompositions", json!({})).unwrap();
    assert_eq!(destination.project.item(id).unwrap().name, "Renamed");
    source.execute("edit.undo", json!({})).unwrap(); // rename
    source.execute("edit.undo", json!({})).unwrap(); // creation, including allocator
    let replacement = comp(&mut source, "Renamed", 16);
    assert_eq!(replacement, original, "exercise numeric ID reuse");
    save(&mut source, &path);
    let r = destination.execute("project.refreshLinkedCompositions", json!({})).unwrap();
    assert!(!r["errors"].as_array().unwrap().is_empty());
    assert!(destination.render_rgba8(main, Tick::ZERO, 0).is_err());
    assert!(destination.project.comp(main).is_some(), "offline destination stays editable");
}

#[test]
fn linked_compositions_keep_identity_and_refreshed_pixels_across_unrelated_undo() {
    let dir = directory("linked-compositions-undo");
    let path = dir.join("source.ecproj");
    let mut source = Session::default();
    let original = comp(&mut source, "Source", 16);
    source.execute("layer.newSolid", json!({"color":"#ff0000"})).unwrap();
    save(&mut source, &path);
    let mut destination = Session::default();
    let main = comp(&mut destination, "Main", 32);
    let linked = link(&mut destination, &path, "Source");
    let layer = destination.execute("layer.addItem", json!({"item":linked.0})).unwrap()["layer"].as_u64().unwrap();
    let local = destination.project.comp(main).unwrap().layers.clone();
    destination.execute("prop.set", json!({"layer":layer,"path":"transform/opacity","value":50})).unwrap();
    let identity = source.project.item(original).unwrap().stable_id.clone();
    source.execute("edit.undo", json!({})).unwrap(); // remove red layer, keep source comp
    assert_eq!(source.project.item(original).unwrap().stable_id, identity);
    source.execute("layer.newSolid", json!({"color":"#0000ff"})).unwrap();
    save(&mut source, &path);
    let refreshed = destination.execute("project.refreshLinkedCompositions", json!({})).unwrap();
    assert!(refreshed["errors"].as_array().unwrap().is_empty());
    destination.execute("edit.undo", json!({})).unwrap(); // opacity only
    assert_eq!(destination.project.comp(main).unwrap().layers, local);
    let frame = destination.render(main, Tick::ZERO, Default::default());
    assert!(frame.data.iter().any(|p| p[2] > p[0]), "undo keeps the refreshed blue source");
    assert!(!frame.data.iter().any(|p| p[0] > p[2]), "old red source never reappears");
}

struct PngFiles;
impl FootageSource for PngFiles {
    fn frame(&self, _: ItemId, f: &Footage, _: Tick) -> Option<Arc<crate::Image>> {
        let image = image::open(&f.path).ok()?.to_rgba8();
        Some(Arc::new(crate::Image::from_rgba8(image.width(), image.height(), image.as_raw())))
    }
}

#[test]
fn linked_compositions_resolve_hidden_relative_footage_and_refresh_its_pixels() {
    let dir = directory("linked-compositions-footage");
    let path = dir.join("source.ecproj");
    let media = dir.join("texture.png");
    image::save_buffer(&media, &[255, 0, 0, 255].repeat(16 * 16), 16, 16, image::ColorType::Rgba8).unwrap();
    let mut source = Session::default();
    comp(&mut source, "Plate", 16);
    let id = Arc::make_mut(&mut source.project).add_item(
        "Texture",
        Default::default(),
        None,
        ItemKind::Footage(Footage { path: "texture.png".into(), kind: FootageKind::Still, width: 16, height: 16, has_video: true, ..Default::default() }),
    );
    source.execute("layer.addItem", json!({"item":id.0})).unwrap();
    save(&mut source, &path);
    let mut destination = Session { footage: Arc::new(PngFiles), ..Default::default() };
    let main = comp(&mut destination, "Main", 16);
    let linked = link(&mut destination, &path, "Plate");
    destination.execute("layer.addItem", json!({"item":linked.0})).unwrap();
    let before = destination.render(main, Tick::ZERO, Default::default());
    assert!(before.data.iter().any(|p| p[0] > p[2]));
    image::save_buffer(&media, &[0, 0, 255, 255].repeat(16 * 16), 16, 16, image::ColorType::Rgba8).unwrap();
    destination.execute("project.refreshLinkedCompositions", json!({})).unwrap();
    let after = destination.render(main, Tick::ZERO, Default::default());
    assert_ne!(before.data, after.data);
    assert_eq!(destination.project.items.len(), 2, "media stays in the hidden source snapshot");
    // Missing source footage is an explicit link error, never successful blank export.
    std::fs::remove_file(&media).unwrap();
    destination.execute("project.refreshLinkedCompositions", json!({})).unwrap();
    assert!(destination.render_rgba8(main, Tick::ZERO, 0).unwrap_err().to_string().contains("Missing source footage"));
}

#[test]
fn linked_compositions_reject_nested_projects_and_bridge_resolution_without_changing_destination() {
    let dir = directory("linked-compositions-bounds");
    let source_path = dir.join("source.ecproj");
    let nested_path = dir.join("nested.ecproj");
    let mut source = Session::default();
    comp(&mut source, "Source", 16);
    save(&mut source, &source_path);
    let mut nested = Session::default();
    comp(&mut nested, "Nested", 16);
    link(&mut nested, &source_path, "Source");
    save(&mut nested, &nested_path);
    let mut destination = Session::default();
    comp(&mut destination, "Main", 16);
    let before = destination.project.clone();
    assert!(destination.execute("file.linkComposition", json!({"path":nested_path.to_string_lossy(),"comp":"Nested"})).is_err());
    assert!(destination.execute("file.open", json!({"path":nested_path.to_string_lossy(),"resolveLinks":false})).is_err());
    assert!(Arc::ptr_eq(&before, &destination.project));
}
