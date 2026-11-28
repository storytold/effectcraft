//! File ▸ Import ▸ Apple Motion… through the command registry: the new composition, undo and
//! redo, menu placement and errors that leave the project untouched.

use effectcraft_project::{ItemId, ItemKind, LayerSource};
use serde_json::json;

use crate::Session;

/// A fresh folder holding `name` with `text`; returns the file's path.
fn write(name: &str, text: &str) -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let d = std::env::temp_dir().join(format!("ec-motion-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir_all(&d).unwrap();
    let p = d.join(name);
    std::fs::write(&p, text).unwrap();
    p.to_string_lossy().to_string()
}

/// A 640×360 25 fps scene: one group with an orange Color Solid and an image whose media is
/// missing, so the footage imports as missing.
const SCENE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE ozxmlscene>
<ozml version="5.2">
<factory id="3" uuid="615c4bf6406511d88802000a95af90f2"><description>Generator</description><manufacturer>Apple</manufacturer><version>1</version></factory>
<factory id="5" uuid="66fc0d6af6a911d6a7a7000393670732"><description>Image</description><manufacturer>Apple</manufacturer><version>1</version></factory>
<scene>
<sceneSettings><width>640</width><height>360</height><duration>50</duration><frameRate>25</frameRate></sceneSettings>
<layer name="Group" id="2">
  <scenenode name="Orange" id="11" factoryID="3" pluginName="Color Solid">
    <parameter name="Object" id="2"><parameter name="Color Solid" id="1"><parameter name="Color" id="1">
      <parameter name="Red" id="1" value="1"/><parameter name="Green" id="2" value="0.5"/><parameter name="Blue" id="3" value="0"/>
    </parameter></parameter></parameter>
  </scenenode>
  <scenenode name="Photo" id="12" factoryID="5">
    <parameter name="Properties" id="1"><parameter name="Media" id="324"><parameter name="Source Media" id="300" value="901"/></parameter></parameter>
  </scenenode>
</layer>
<footage name="Media Layer" id="900">
  <clip name="Missing" id="901"><relativeURL>no-such-file.png</relativeURL><missingWidth>320</missingWidth><missingHeight>180</missingHeight><creationDuration>1</creationDuration></clip>
</footage>
</scene>
</ozml>"#;

#[test]
fn import_motion_makes_an_open_comp_and_undoes_in_one_step() {
    let path = write("Scene.motn", SCENE);
    let mut s = Session::default();
    let before = s.project.items.len();
    let r = s.execute_checked("file.importMotion", json!({"path": path})).unwrap();
    let cid = ItemId(r["comp"].as_u64().unwrap());
    // The new comp is named after the file, opened and selected in the Project panel.
    assert_eq!(s.project.item(cid).unwrap().name, "Scene");
    assert_eq!(s.state.active_comp, Some(cid));
    assert_eq!(s.state.project_selection, [cid]);
    let c = s.project.comp(cid).unwrap();
    assert_eq!((c.width, c.height), (640, 360));
    let names: Vec<&str> = c.layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["Orange", "Photo"]);
    let LayerSource::Footage { item } = c.layers[1].source else { panic!("footage expected") };
    let Some(ItemKind::Footage(f)) = s.project.item(item).map(|i| &i.kind) else { panic!("footage item") };
    assert!(f.missing);
    assert!(r["warnings"].as_array().unwrap().iter().any(|w| w.as_str().unwrap().contains("Missing")), "{r}");
    // Every created item is reported; one undo removes all of them, redo brings them back.
    let made = r["items"].as_array().unwrap().len();
    assert_eq!(s.project.items.len(), before + made);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.project.items.len(), before);
    assert!(s.project.comp(cid).is_none());
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(s.project.items.len(), before + made);
    assert_eq!(s.project.comp(cid).unwrap().layers.len(), 2);
}

#[test]
fn bad_motion_imports_are_errors_that_change_nothing() {
    let mut s = Session::default();
    let before = s.project.items.len();
    let missing = std::env::temp_dir().join("ec-motion-no-such-dir").join("x.motn");
    assert!(s.execute("file.importMotion", json!({"path": missing.to_string_lossy()})).is_err());
    assert!(s.execute("file.importMotion", json!({"path": write("Bad.motn", "<svg/>")})).is_err());
    assert!(s.execute("file.importMotion", json!({})).is_err());
    // Agents get unknown parameters rejected instead of ignored.
    assert!(s.execute_checked("file.importMotion", json!({"path": write("Ok.motn", SCENE), "bogus": 1})).is_err());
    assert_eq!(s.project.items.len(), before);
}

#[test]
fn motion_import_is_in_the_file_menu_and_export_is_not_offered() {
    let entries = crate::menus::entries();
    let find = |cmd: &str| entries.iter().find(|(_, e)| e.command == cmd).map(|(p, e)| (p.join(" > "), e.label.clone()));
    assert_eq!(find("file.importMotion"), Some(("File > Import".into(), "Apple Motion...".into())));
    assert_eq!(find("file.exportMotion"), None);
    assert!(crate::commands::find("file.exportMotion").is_none());
}
