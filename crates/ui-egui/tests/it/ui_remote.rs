//! Remote storage in the UI: the File menu, the open-project hook and the dialog's save path.

use std::cell::RefCell;
use std::rc::Rc;

use effectcraft_engine::Session;
use effectcraft_ui_egui::panels::remote_dialog::save_target;
use effectcraft_ui_egui::{EffectcraftApp, RemoteInfo};
use serde_json::json;

/// A host that offers one mount.
fn remote() -> RemoteInfo {
    RemoteInfo { mounts: vec![("Server".into(), "/data".into())] }
}

/// The File menu's entries as (id, label, path).
fn items(app: &EffectcraftApp) -> Vec<(String, String, Vec<String>)> {
    effectcraft_ui_egui::menus::menu_items(app).into_iter().map(|i| (i.id, i.label, i.path)).collect()
}

/// Without remote the menus are the engines.
#[test]
fn without_remote_the_menus_are_the_engines() {
    let app = EffectcraftApp::new(Session::default());
    let want: Vec<(String, String)> = effectcraft_engine::menus::entries().into_iter().map(|(_, e)| (e.command.clone(), e.label.clone())).collect();
    let got: Vec<(String, String)> = items(&app).into_iter().map(|(id, label, _)| (id, label)).collect();
    assert_eq!(got.len(), want.len());
    assert!(got.iter().all(|(id, _)| !id.ends_with("Remote")));
    assert!(got.iter().any(|(id, l)| id == "file.open" && l == "Open Project..."));
}

/// With remote the file menu has local and remote entries.
#[test]
fn with_remote_the_file_menu_has_local_and_remote_entries() {
    let mut app = EffectcraftApp::new(Session::default());
    app.hooks.remote = Some(remote());
    let v = items(&app);
    let label = |id: &str| v.iter().find(|(i, _, _)| i == id).map(|(_, l, _)| l.clone());
    assert_eq!(label("file.open").as_deref(), Some("Open Local Project..."));
    assert_eq!(label("file.openRemote").as_deref(), Some("Open Remote Project..."));
    assert_eq!(label("file.import").as_deref(), Some("Local File..."));
    assert_eq!(label("file.importMultiple").as_deref(), Some("Multiple Local Files..."));
    assert_eq!(label("file.importRemote").as_deref(), Some("Import from Remote..."));
    assert_eq!(label("file.saveAsRemote").as_deref(), Some("Save As Remote..."));
    let path = |id: &str| v.iter().find(|(i, _, _)| i == id).map(|(_, _, p)| p.clone()).unwrap();
    assert_eq!(path("file.importRemote"), ["File", "Import"]);
    assert_eq!(path("file.saveAsRemote"), ["File", "Save As"]);
    let enabled = |id: &str| effectcraft_ui_egui::menus::menu_items(&app).into_iter().find(|i| i.id == id).unwrap().enabled;
    assert!(enabled("file.openRemote") && enabled("file.importRemote"));
}

/// Open by path goes through the host hook.
#[test]
fn open_by_path_goes_through_the_host_hook() {
    let mut app = EffectcraftApp::new(Session::default());
    let seen = Rc::new(RefCell::new(Vec::<String>::new()));
    let s = seen.clone();
    app.hooks.open_project = Some(Box::new(move |p| s.borrow_mut().push(p.to_string())));
    app.session.prefs.recent_projects = vec!["/data/a.ecproj".into(), "/data/b.ecproj".into()];
    let ctx = egui::Context::default();
    effectcraft_ui_egui::menus::invoke(&mut app, &ctx, "file.open", json!({"path": "/data/x.ecproj"})).unwrap();
    effectcraft_ui_egui::menus::invoke(&mut app, &ctx, "file.openRecent", json!({"index": 1})).unwrap();
    effectcraft_ui_egui::menus::invoke(&mut app, &ctx, "file.openRecent", json!({})).unwrap();
    assert_eq!(*seen.borrow(), ["/data/x.ecproj", "/data/b.ecproj", "/data/a.ecproj"]);
}

/// The remote dialog opens only with remote storage.
#[test]
fn the_remote_dialog_opens_only_with_remote_storage() {
    let mut app = EffectcraftApp::new(Session::default());
    let ctx = egui::Context::default();
    assert!(effectcraft_ui_egui::menus::invoke(&mut app, &ctx, "file.openRemote", json!({})).is_err());
    app.hooks.remote = Some(remote());
    assert!(effectcraft_ui_egui::menus::invoke(&mut app, &ctx, "file.openRemote", json!({})).is_ok());
    assert_eq!(app.dialog, Some(effectcraft_ui_egui::Dialog::Remote));
}

/// Save names stay in the folder.
#[test]
fn save_names_stay_in_the_folder() {
    assert_eq!(save_target("/data/p", "Film").unwrap(), "/data/p/Film.ecproj");
    assert_eq!(save_target("/data/p/", "a.ECPROJ").unwrap(), "/data/p/a.ECPROJ");
    for bad in ["", " ", ".", "..", "a/b", "a\\b", "a\nb"] {
        assert!(save_target("/data", bad).is_err(), "{bad:?}");
    }
    assert!(save_target("", "x").is_err());
}
