//! The linked-project importer lists saved comps by identity and commits selections together.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use effectcraft_project::{ItemId, ItemKind};
use serde_json::{Value, json};

use crate::{Services, Session};

fn source(name: &str) -> (Session, std::path::PathBuf) {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-out").join(name);
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("Source.ecproj");
    let mut source = Session::default();
    for name in ["Opening", "Credits"] {
        source.execute("comp.new", json!({"name":name,"width":16,"height":16,"duration":2})).unwrap();
    }
    source.execute("file.saveAs", json!({"path":path.to_string_lossy()})).unwrap();
    (source, path)
}

fn inspect(destination: &mut Session, path: &std::path::Path) -> Value {
    destination.execute("file.linkableCompositions", json!({"path":path.to_string_lossy()})).unwrap()
}

#[test]
fn linked_import_list_is_read_only_and_already_linked_follows_identity() {
    let (mut source, path) = source("linked-import-list");
    let mut destination = Session::default();
    let before = destination.project.clone();
    let history = destination.history.undo.len();
    let listing = inspect(&mut destination, &path);
    assert!(Arc::ptr_eq(&before, &destination.project));
    assert_eq!(destination.history.undo.len(), history);
    let comps = listing["compositions"].as_array().unwrap();
    assert_eq!(comps.len(), source.project.comps().count());
    assert!(comps.iter().all(|comp| comp["alreadyLinked"] == false));
    let first = comps[0]["id"].clone();
    let linked = destination
        .execute("file.linkComposition", json!({"path":path.to_string_lossy(),"comps":[first],"sourceProjectId":listing["sourceProjectId"]}))
        .unwrap()["items"][0]
        .clone();
    let comp_id = source.project.comps().next().unwrap().0.0;
    source.execute("project.rename", json!({"item":comp_id,"name":"Renamed Opening"})).unwrap();
    source.execute("file.save", json!({})).unwrap();
    let renamed = inspect(&mut destination, &path);
    let renamed_comp = renamed["compositions"].as_array().unwrap().iter().find(|comp| comp["id"] == first).unwrap();
    assert_eq!(renamed_comp["name"], "Renamed Opening");
    assert_eq!(renamed_comp["alreadyLinked"], true);
    assert_eq!(renamed_comp["linkedItems"][0], linked);
    let other = renamed["compositions"].as_array().unwrap().iter().find(|comp| comp["id"] != first).unwrap();
    assert_eq!(other["alreadyLinked"], false);

    // The same source identity is recognized at another path, without rewriting the link.
    let copied_path = path.with_file_name("Source Copy.ecproj");
    std::fs::copy(&path, &copied_path).unwrap();
    assert!(
        inspect(&mut destination, &copied_path)["compositions"].as_array().unwrap().iter().any(|comp| comp["id"] == first && comp["alreadyLinked"] == true)
    );
    let (_, unrelated) = self::source("linked-import-other-source");
    assert!(inspect(&mut destination, &unrelated)["compositions"].as_array().unwrap().iter().all(|comp| comp["alreadyLinked"] == false));
}

#[test]
fn linked_import_batch_is_atomic_reuses_existing_and_undoes_together() {
    let (_, path) = source("linked-import-batch");
    let mut destination = Session::default();
    let listing = inspect(&mut destination, &path);
    let ids: Vec<_> = listing["compositions"].as_array().unwrap().iter().map(|comp| comp["id"].clone()).collect();
    let first = destination.execute("file.linkComposition", json!({"path":path.to_string_lossy(),"comp":ids[0]})).unwrap()["item"].clone();
    let before = destination.project.clone();
    let undo = destination.history.undo.len();
    assert!(destination.execute("file.linkComposition", json!({"path":path.to_string_lossy(),"comps":[ids[1],"deleted-comp"]})).is_err());
    assert!(Arc::ptr_eq(&before, &destination.project), "a stale list cannot partly create a selection");
    assert_eq!(destination.history.undo.len(), undo);
    let result = destination
        .execute("file.linkComposition", json!({"path":path.to_string_lossy(),"comps":[ids[0],ids[1],ids[1]],"sourceProjectId":listing["sourceProjectId"]}))
        .unwrap();
    assert_eq!(result["existingItems"], json!([first]));
    assert_eq!(result["createdItems"].as_array().unwrap().len(), 1);
    assert_eq!(destination.project.items.len(), ids.len());
    assert_eq!(destination.history.undo.len(), undo + 1);
    let after_count = destination.project.items.len();
    let repeated = destination.execute("file.linkComposition", json!({"path":path.to_string_lossy(),"comps":ids})).unwrap();
    assert!(repeated["createdItems"].as_array().unwrap().is_empty());
    assert_eq!(destination.project.items.len(), after_count);
    assert_eq!(destination.history.undo.len(), undo + 1, "reusing links changes only selection");
    destination.execute("edit.undo", json!({})).unwrap();
    assert_eq!(destination.project.items.len(), before.items.len());
    assert!(destination.project.item(ItemId(first.as_u64().unwrap())).is_some());
    destination.execute("edit.redo", json!({})).unwrap();
    assert_eq!(destination.project.items.len(), after_count);

    let mut empty = Session::default();
    let undo = empty.history.undo.len();
    empty.execute("file.linkComposition", json!({"path":path.to_string_lossy(),"comps":ids})).unwrap();
    assert_eq!(empty.history.undo.len(), undo + 1, "multiple new links are one edit");
    empty.execute("edit.undo", json!({})).unwrap();
    assert!(empty.project.items.is_empty());
}

#[test]
fn linked_import_rechecks_project_identity_before_committing_the_picker() {
    let (mut source, path) = source("linked-import-changed-source");
    let mut destination = Session::default();
    let listing = inspect(&mut destination, &path);
    let selected = listing["compositions"][0]["id"].as_str().unwrap();
    let old = source.project.items.values().find(|item| item.stable_id == selected).unwrap().id;
    Arc::make_mut(&mut source.project).items.remove(&old);
    source.execute("comp.new", json!({"name":selected,"width":16,"height":16})).unwrap();
    source.execute("file.save", json!({})).unwrap();
    let before = destination.project.clone();
    assert!(
        destination
            .execute("file.linkComposition", json!({"path":path.to_string_lossy(),"comps":[selected],"sourceProjectId":listing["sourceProjectId"]}),)
            .is_err(),
        "a deleted stable ID cannot fall back to another composition's name"
    );
    assert!(Arc::ptr_eq(&before, &destination.project));
    let (_, other_path) = self::source("linked-import-replacement-source");
    std::fs::copy(other_path, &path).unwrap();
    let before = destination.project.clone();
    let error = destination
        .execute(
            "file.linkComposition",
            json!({"path":path.to_string_lossy(),"comps":[listing["compositions"][0]["id"]],"sourceProjectId":listing["sourceProjectId"]}),
        )
        .unwrap_err();
    assert!(error.to_string().contains("different source project"));
    assert!(Arc::ptr_eq(&before, &destination.project));
}

struct OneRead {
    reads: AtomicUsize,
}

impl Services for OneRead {
    fn read_file(&self, path: &str) -> std::io::Result<Vec<u8>> {
        if self.reads.fetch_add(1, Ordering::SeqCst) != 0 {
            return Err(std::io::Error::other("source became unreadable after validation"));
        }
        crate::FsServices.read_file(path)
    }

    fn write_file(&self, _: &str, _: &[u8]) -> std::io::Result<()> {
        Err(std::io::Error::other("read-only test host"))
    }
}

#[test]
fn linked_import_uses_one_validated_source_snapshot_for_every_selected_comp() {
    let (_, path) = source("linked-import-one-snapshot");
    let mut destination = Session::default();
    let listing = inspect(&mut destination, &path);
    let services = Arc::new(OneRead { reads: AtomicUsize::new(0) });
    destination.services = services.clone();
    let selected: Vec<_> = listing["compositions"].as_array().unwrap().iter().map(|comp| comp["id"].clone()).collect();
    let result = destination.execute("file.linkComposition", json!({"path":path.to_string_lossy(),"comps":selected})).unwrap();
    assert!(result["links"].as_array().unwrap().iter().all(|link| link["ready"] == true));
    assert_eq!(services.reads.load(Ordering::SeqCst), 1);
    let snapshots: Vec<_> = destination
        .project
        .items
        .values()
        .filter_map(|item| match &item.kind {
            ItemKind::Footage(f) => f.linked_comp.as_ref().and_then(|link| link.resolved.as_ref()),
            _ => None,
        })
        .collect();
    assert!(snapshots.windows(2).all(|pair| Arc::ptr_eq(pair[0], pair[1])));
}
