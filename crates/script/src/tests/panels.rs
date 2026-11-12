//! ScriptUI Panels, as in After Effects: scripts in the user's `Scripts/ScriptUI Panels` folder
//! (in the settings folder) are listed at the bottom of the Window menu, open as dockable panels
//! with `this` = the panel, and the panels open when the app quits open again at the next launch.

use std::sync::Arc;

use effectcraft_engine::Session;
use effectcraft_engine::commands::scripts::{OPEN_PANELS_FILE, PANELS_DIR};
use effectcraft_engine::config::{ConfigStore, MemoryConfig};
use effectcraft_engine::scriptui::WindowKind;
use serde_json::json;

const PANEL: &str = "var p = this;\np.add('statictext', undefined, 'Hello from a panel');\np.layout.layout(true);\n";

fn session(store: &Arc<MemoryConfig>) -> Session {
    let mut s = Session::default();
    crate::install(&mut s);
    s.config = Some(store.clone());
    s
}

fn panel_window(s: &Session, name: &str) -> Option<u32> {
    s.script_ui.windows.iter().find(|w| w.kind == WindowKind::Panel && w.script == name).map(|w| w.id)
}

fn remembered(store: &MemoryConfig) -> serde_json::Value {
    serde_json::from_str(&store.read(OPEN_PANELS_FILE).unwrap_or_default()).unwrap_or_default()
}

#[test]
fn a_panel_dropped_in_the_folder_is_listed_and_opens_as_a_panel() {
    let store = Arc::new(MemoryConfig::default());
    store.write(&format!("{PANELS_DIR}/Hello Panel.jsx"), PANEL).unwrap();
    // Not a script: not listed.
    store.write(&format!("{PANELS_DIR}/notes.txt"), "x").unwrap();
    let mut s = session(&store);
    let cx = effectcraft_engine::menus::DynCtx::default();
    let (entries, _) = effectcraft_engine::menus::dynamic(&s, "scriptPanels", &cx);
    let names: Vec<&str> = entries.iter().map(|e| e.label.as_str()).collect();
    assert!(names.contains(&"Hello Panel.jsx") && !names.contains(&"notes.txt"), "{names:?}");
    let e = entries.iter().find(|e| e.label == "Hello Panel.jsx").unwrap();
    assert_eq!((e.command.as_str(), &e.params), ("window.scriptPanel", &json!({"name": "Hello Panel.jsx"})));
    let r = s.execute("window.scriptPanel", json!({"name": "Hello Panel.jsx"})).unwrap();
    let id = r["window"].as_u64().unwrap() as u32;
    let w = s.script_ui.window(id).unwrap();
    assert_eq!((w.kind, w.title.as_str()), (WindowKind::Panel, "Hello Panel"));
    assert_eq!(w.root.children[0].text, "Hello from a panel");
    // Frontends dock it (`window.scriptPanel {window}` for the UI).
    let events = s.drain_events();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, effectcraft_engine::Event::Frontend { command, params } if command == "window.scriptPanel" && params["window"] == id))
    );
}

#[test]
fn open_panels_open_again_at_the_next_launch() {
    let store = Arc::new(MemoryConfig::default());
    store.write(&format!("{PANELS_DIR}/Hello Panel.jsx"), PANEL).unwrap();
    let mut s = session(&store);
    s.execute("window.scriptPanel", json!({"name": "Hello Panel.jsx"})).unwrap();
    s.execute("window.scriptPanel", json!({"name": "Layer Tools.jsx"})).unwrap();
    assert_eq!(remembered(&store), json!({"open": ["Hello Panel.jsx", "Layer Tools.jsx"]}));
    // Quit with both open; the next launch opens them again.
    drop(s);
    let mut s = session(&store);
    let r = s.execute("window.restoreScriptPanels", json!({})).unwrap();
    assert_eq!(r, json!({"opened": ["Hello Panel.jsx", "Layer Tools.jsx"], "failed": []}));
    assert!(panel_window(&s, "Hello Panel.jsx").is_some() && panel_window(&s, "Layer Tools.jsx").is_some());
    // Closing a panel (its tab) forgets it.
    let id = panel_window(&s, "Layer Tools.jsx").unwrap();
    s.execute("scriptui.close", json!({"window": id})).unwrap();
    assert_eq!(remembered(&store), json!({"open": ["Hello Panel.jsx"]}));
    // A panel whose script was removed, or that fails, is reported and forgotten; the others open.
    store.write(OPEN_PANELS_FILE, &json!({"open": ["Gone.jsx", "Broken.jsx", "Hello Panel.jsx"]}).to_string()).unwrap();
    store.write(&format!("{PANELS_DIR}/Broken.jsx"), "throw new Error('nope');").unwrap();
    let mut s = session(&store);
    let r = s.execute("window.restoreScriptPanels", json!({})).unwrap();
    assert_eq!(r["opened"], json!(["Hello Panel.jsx"]));
    let failed: Vec<&str> = r["failed"].as_array().unwrap().iter().map(|f| f["name"].as_str().unwrap()).collect();
    assert_eq!(failed, ["Gone.jsx", "Broken.jsx"]);
    assert_eq!(remembered(&store), json!({"open": ["Hello Panel.jsx"]}));
    // A corrupt list opens nothing (and is no error).
    store.write(OPEN_PANELS_FILE, "{not json").unwrap();
    let mut s = session(&store);
    assert_eq!(s.execute("window.restoreScriptPanels", json!({})).unwrap(), json!({"opened": [], "failed": []}));
}

/// A panel in a subfolder (Window ▸ Motion Kit ▸ Animate.jsx) is `Motion Kit/Animate.jsx`: it opens
/// titled after its file and opens again at the next launch.
#[test]
fn a_panel_in_a_subfolder_opens_and_opens_again_at_the_next_launch() {
    let store = Arc::new(MemoryConfig::default());
    store.write(&format!("{PANELS_DIR}/Motion Kit/Animate.jsx"), PANEL).unwrap();
    let mut s = session(&store);
    let list = s.execute("file.scripts.list", json!({})).unwrap();
    assert!(list.as_array().unwrap().iter().any(|e| e["name"] == "Motion Kit/Animate.jsx" && e["panel"] == true), "{list}");
    let r = s.execute("window.scriptPanel", json!({"name": "Motion Kit/Animate.jsx"})).unwrap();
    let w = s.script_ui.window(r["window"].as_u64().unwrap() as u32).unwrap();
    assert_eq!((w.kind, w.title.as_str()), (WindowKind::Panel, "Animate"));
    assert_eq!(remembered(&store), json!({"open": ["Motion Kit/Animate.jsx"]}));
    // A saved name that leaves the folder, or goes two folders deep, fails and is forgotten.
    store.write(OPEN_PANELS_FILE, &json!({"open": ["../Top.jsx", "Motion Kit/Animate.jsx", "a/b/c.jsx"]}).to_string()).unwrap();
    drop(s);
    let mut s = session(&store);
    let r = s.execute("window.restoreScriptPanels", json!({})).unwrap();
    assert_eq!(r["opened"], json!(["Motion Kit/Animate.jsx"]));
    assert_eq!(r["failed"].as_array().unwrap().iter().map(|f| f["name"].as_str().unwrap()).collect::<Vec<_>>(), ["../Top.jsx", "a/b/c.jsx"]);
    assert!(panel_window(&s, "Motion Kit/Animate.jsx").is_some());
    assert_eq!(remembered(&store), json!({"open": ["Motion Kit/Animate.jsx"]}));
}
