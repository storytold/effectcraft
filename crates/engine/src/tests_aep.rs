//! The AEP import command must leave the current project intact on bad input.

use serde_json::json;

use crate::Session;

#[test]
fn relink_cropped_vector_preserves_binding_and_undo() {
    use effectcraft_project::{Footage, FootageKind, ItemKind, SourceLayer};
    struct VectorImporter;
    impl crate::Importer for VectorImporter {
        fn probe(&self, path: &str) -> Result<Footage, String> {
            Ok(Footage { path: path.into(), kind: FootageKind::Still, codec: "AI".into(), width: 1000, height: 800, has_video: true, ..Default::default() })
        }
    }
    let mut session = Session { importer: Some(std::sync::Arc::new(VectorImporter)), ..Default::default() };
    let binding = SourceLayer { index: 2, name: "Hand".into(), layer_size: true, crop_bounds: Some([20.25, 20.5, 40.5, 60.75]), ..Default::default() };
    let item = std::sync::Arc::make_mut(&mut session.project).add_item(
        "Hand",
        Default::default(),
        None,
        ItemKind::Footage(Footage {
            path: "/offline/art.ai".into(),
            kind: FootageKind::Still,
            width: 21,
            height: 41,
            missing: true,
            layer: Some(binding.clone()),
            ..Default::default()
        }),
    );
    session.state.project_selection = vec![item];
    let before = session.project.to_file_json().unwrap();
    session.execute("file.replaceFootage", json!({"item":item.0,"path":"/relinked/art.ai"})).unwrap();
    let reopened = effectcraft_project::Project::from_json(&session.project.to_file_json().unwrap()).unwrap();
    let ItemKind::Footage(footage) = &reopened.item(item).unwrap().kind else { panic!("not footage") };
    assert_eq!(footage.layer.as_ref(), Some(&binding));
    assert_eq!((footage.width, footage.height), (21, 41));
    assert_eq!(footage.path, "/relinked/art.ai");
    assert!(!footage.missing);
    session.execute("edit.undo", json!({})).unwrap();
    assert_eq!(session.project.to_file_json().unwrap(), before);
}

#[test]
fn malformed_aep_ir_does_not_change_project() {
    let mut session = Session::default();
    session.execute("comp.new", json!({"name": "Keep Me"})).unwrap();
    let before = session.project.to_file_json().unwrap();
    let path = std::env::temp_dir().join(format!("effectcraft-bad-aep-ir-{}.json", std::process::id()));
    std::fs::write(&path, b"{broken").unwrap();

    let result = session.execute("file.importAeIr", json!({"path": path.to_string_lossy()}));
    assert!(result.is_err());
    assert_eq!(session.project.to_file_json().unwrap(), before);
}

#[test]
fn strict_import_rejects_declared_loss_without_mutating_project() {
    let mut session = Session::default();
    session.execute("comp.new", json!({"name": "Keep Me"})).unwrap();
    let before = session.project.to_file_json().unwrap();
    let path = std::env::temp_dir().join(format!("effectcraft-lossy-aep-ir-{}.json", std::process::id()));
    std::fs::write(
        &path,
        json!({"schema_version":1,"items":[
        {"id":1,"kind":"comp","name":"Rejected","width":320,"height":180,"duration":1,"frame_rate":24,"layers":[]}
    ],"losses":[{"reason":"opaque_value"}]})
        .to_string(),
    )
    .unwrap();

    assert!(session.execute("file.importAeIr", json!({"path":path.to_string_lossy()})).is_err());
    assert_eq!(session.project.to_file_json().unwrap(), before);
}

#[test]
fn imports_editable_comp_as_one_undo_step() {
    let mut session = Session::default();
    let path = std::env::temp_dir().join(format!("effectcraft-good-aep-ir-{}.json", std::process::id()));
    let ir = json!({"schema_version":1,"items":[
        {"id":10,"kind":"comp","name":"Imported","width":320,"height":180,"duration":2,"frame_rate":24,
         "layers":[{"id":11,"kind":"null","name":"Controller","timing":{"out_point":2}}]}
    ]});
    std::fs::write(&path, ir.to_string()).unwrap();

    let reply = session.execute("file.importAeIr", json!({"path":path.to_string_lossy()})).unwrap();
    assert_eq!(reply["comps"].as_array().unwrap().len(), 1);
    let imported = effectcraft_project::ItemId(reply["comps"][0].as_u64().unwrap());
    assert_eq!(session.project.comp(imported).unwrap().layers.len(), 1);
    assert!(reply["losses"].as_array().unwrap().is_empty());
    let original = session.project.comp(imported).unwrap().layers[0].transform().unwrap().get("position").unwrap().value.clone();
    session.execute("prop.set", json!({"comp":imported.0,"layer":"#1","path":"transform/position","value":[30,40,0]})).unwrap();
    assert_ne!(session.project.comp(imported).unwrap().layers[0].transform().unwrap().get("position").unwrap().value, original);
    session.execute("edit.undo", json!({})).unwrap();
    assert_eq!(session.project.comp(imported).unwrap().layers[0].transform().unwrap().get("position").unwrap().value, original);
    session.execute("edit.undo", json!({})).unwrap();
    assert!(session.project.comp(imported).is_none());
}
