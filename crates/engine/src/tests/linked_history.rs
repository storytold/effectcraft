//! Restored link items use the current source generation rather than their history's snapshot.

use std::sync::Arc;

use effectcraft_project::{Footage, ItemId, ItemKind};
use effectcraft_time::Tick;
use serde_json::json;

use crate::Session;

fn linked(s: &Session, id: ItemId) -> &Footage {
    match &s.project.item(id).unwrap().kind {
        ItemKind::Footage(f) => f,
        _ => panic!("linked footage"),
    }
}

#[test]
fn redoing_a_link_uses_other_compositions_current_source_generation() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-out/linked-history");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Titles.ecproj");
    let mut source = Session::default();
    source.execute("comp.new", json!({"name":"Opening", "width":16, "height":16, "duration":1})).unwrap();
    source.execute("layer.newSolid", json!({"color":"#ff0000"})).unwrap();
    source.execute("comp.new", json!({"name":"Credits", "width":16, "height":16, "duration":1})).unwrap();
    source.execute("layer.newSolid", json!({"color":"#00ff00"})).unwrap();
    let credits = source.active_comp_id().unwrap();
    source.execute("file.saveAs", json!({"path":path.to_string_lossy()})).unwrap();

    let mut destination = Session::default();
    destination.execute("comp.new", json!({"name":"Main", "width":16, "height":16, "duration":1})).unwrap();
    let a = destination.execute("file.linkComposition", json!({"path":path.to_string_lossy(), "comp":"Opening"})).unwrap();
    let a = ItemId(a["item"].as_u64().unwrap());
    let b = destination.execute("file.linkComposition", json!({"path":path.to_string_lossy(), "comp":"Credits"})).unwrap();
    let b = ItemId(b["item"].as_u64().unwrap());
    let previous = linked(&destination, b).linked_comp.as_ref().unwrap().resolved.clone().unwrap();
    destination.execute("edit.undo", json!({})).unwrap();
    assert!(destination.project.item(b).is_none());

    source.execute("layer.newSolid", json!({"color":"#0000ff"})).unwrap();
    source.execute("project.rename", json!({"item":credits.0, "name":"Revised Credits"})).unwrap();
    source.execute("file.saveAs", json!({"path":path.to_string_lossy()})).unwrap();
    destination.execute("project.refreshLinkedCompositions", json!({"item":a.0})).unwrap();
    destination.execute("edit.redo", json!({})).unwrap();
    let opening = linked(&destination, a).linked_comp.as_ref().unwrap();
    let restored = linked(&destination, b).linked_comp.as_ref().unwrap();
    assert!(Arc::ptr_eq(opening.resolved.as_ref().unwrap(), restored.resolved.as_ref().unwrap()));
    assert_eq!(opening.generation, restored.generation);
    assert_eq!(destination.project.item(b).unwrap().name, "Revised Credits");
    assert_ne!(
        effectcraft_render::render_frame(&previous, credits, Tick::ZERO, 1.0).data,
        effectcraft_render::render_frame(restored.resolved.as_ref().unwrap(), restored.resolved_comp.unwrap(), Tick::ZERO, 1.0).data,
        "redo must use the source's refreshed picture"
    );

    // A composition deleted while its destination link is undone must stay missing on redo.
    destination.execute("edit.undo", json!({})).unwrap();
    source.execute("project.delete", json!({"items":[credits.0]})).unwrap();
    source.execute("file.saveAs", json!({"path":path.to_string_lossy()})).unwrap();
    destination.execute("project.refreshLinkedCompositions", json!({"item":a.0})).unwrap();
    destination.execute("edit.redo", json!({})).unwrap();
    let restored = linked(&destination, b);
    let link = restored.linked_comp.as_ref().unwrap();
    assert!(restored.missing && link.error.is_some());
    assert!(link.resolved.is_none(), "redo cannot resurrect the deleted source's old picture");
}
