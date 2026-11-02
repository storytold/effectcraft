//! Startup and Shutdown scripts, as in After Effects: the scripts in the user's `Scripts/Startup`
//! and `Scripts/Shutdown` folders (in the settings folder) run in alphabetical order at launch and
//! as the app quits, after the project is closed.

use std::sync::Arc;

use effectcraft_engine::Session;
use effectcraft_engine::commands::scripts::{OPEN_PANELS_FILE, PANELS_DIR, SCRIPTS_DIR, SHUTDOWN_DIR, STARTUP_DIR};
use effectcraft_engine::config::{ConfigStore, MemoryConfig};
use effectcraft_engine::scriptui::WindowKind;
use serde_json::json;

fn session(store: &Arc<MemoryConfig>) -> Session {
    let mut s = Session::default();
    crate::install(&mut s);
    s.config = Some(store.clone());
    s
}

/// A script that appends `mark` to the `Startup Test` / `order` script setting.
fn mark(mark: &str) -> String {
    format!(
        r#"var k = app.settings.haveSetting("Startup Test", "order") ? app.settings.getSetting("Startup Test", "order") : "";
app.settings.saveSetting("Startup Test", "order", k + "{mark}");"#
    )
}

fn setting(s: &mut Session, key: &str) -> serde_json::Value {
    s.execute("script.settings.get", json!({"section": "Startup Test", "key": key})).unwrap()["value"].clone()
}

#[test]
fn startup_scripts_run_in_alphabetical_order_and_a_failing_one_does_not_stop_the_rest() {
    let store = Arc::new(MemoryConfig::default());
    store.write(&format!("{STARTUP_DIR}/b.jsx"), &mark("b")).unwrap();
    store.write(&format!("{STARTUP_DIR}/A.jsx"), &mark("A")).unwrap();
    store.write(&format!("{STARTUP_DIR}/B2.jsx"), &mark("B")).unwrap();
    store.write(&format!("{STARTUP_DIR}/C.js"), "throw new Error('no');").unwrap();
    store.write(&format!("{STARTUP_DIR}/d.jsx"), &mark("d")).unwrap();
    // Not scripts, or not in the Startup folder: not run.
    store.write(&format!("{STARTUP_DIR}/notes.txt"), &mark("x")).unwrap();
    store.write(&format!("{SCRIPTS_DIR}/Menu Script.jsx"), &mark("m")).unwrap();
    store.write(&format!("{SHUTDOWN_DIR}/Quit.jsx"), &mark("q")).unwrap();
    let mut s = session(&store);
    let r = s.execute("file.runStartupScripts", json!({})).unwrap();
    assert_eq!(r["ran"], json!(["A.jsx", "b.jsx", "B2.jsx", "d.jsx"]), "{r}");
    let failed: Vec<&str> = r["failed"].as_array().unwrap().iter().map(|f| f["name"].as_str().unwrap()).collect();
    assert_eq!(failed, ["C.js"]);
    assert_eq!(setting(&mut s, "order"), "AbBd");
    // The failing script is reported in a toast.
    let events = s.drain_events();
    assert!(events.iter().any(|e| matches!(e, effectcraft_engine::Event::Toast { message, error: true } if message.starts_with("C.js"))), "{events:?}");
    // The Startup folder's scripts are not offered in File ▸ Scripts.
    let list = s.execute("file.scripts.list", json!({})).unwrap();
    assert!(list.as_array().unwrap().iter().all(|e| e["name"] != "b.jsx"), "{list}");
}

#[test]
fn shutdown_scripts_run_after_the_project_is_closed() {
    let store = Arc::new(MemoryConfig::default());
    let probe = r#"app.settings.saveSetting("Startup Test", "items", String(app.project.numItems));"#;
    store.write(&format!("{SHUTDOWN_DIR}/Save State.jsx"), probe).unwrap();
    let mut s = session(&store);
    s.execute("script.run", json!({"code": r#"app.project.items.addComp("Main", 64, 64, 1, 1, 24);"#})).unwrap();
    assert!(!s.project.items.is_empty());
    let r = s.execute("file.runShutdownScripts", json!({})).unwrap();
    assert_eq!(r, json!({"ran": ["Save State.jsx"], "waiting": [], "failed": []}));
    assert_eq!(setting(&mut s, "items"), "0");
    assert!(s.project.items.is_empty());
}

#[test]
fn a_startup_dialog_stays_open_and_its_script_goes_on_when_it_closes() {
    let store = Arc::new(MemoryConfig::default());
    store.write(&format!("{STARTUP_DIR}/A Dialog.jsx"), &format!("new Window('dialog', 'Welcome').show();\n{}", mark("A"))).unwrap();
    store.write(&format!("{STARTUP_DIR}/B.jsx"), &mark("B")).unwrap();
    let mut s = session(&store);
    let r = s.execute("file.runStartupScripts", json!({})).unwrap();
    assert_eq!(r, json!({"ran": ["B.jsx"], "waiting": ["A Dialog.jsx"], "failed": []}));
    assert_eq!(setting(&mut s, "order"), "B");
    assert!(s.script_ui.windows.iter().any(|w| w.modal && w.title == "Welcome"));
    s.execute("scriptui.close", json!({})).unwrap();
    assert_eq!(setting(&mut s, "order"), "BA");
}

#[test]
fn startup_scripts_tasks_go_on_after_a_project_opens() {
    use web_time::{Duration, Instant};
    let store = Arc::new(MemoryConfig::default());
    store.write(&format!("{STARTUP_DIR}/A Poll.jsx"), "app.scheduleTask('writeLn(\"poll\")', 100, true);").unwrap();
    // A task scheduled after the script's dialog closes.
    store.write(&format!("{STARTUP_DIR}/B Ask.jsx"), "new Window('dialog', 'Ask').show();\napp.scheduleTask('writeLn(\"ask\")', 100, true);").unwrap();
    let mut s = session(&store);
    s.execute("file.runStartupScripts", json!({})).unwrap();
    s.execute("scriptui.close", json!({})).unwrap();
    // The launch opens a project after the Startup scripts ran.
    s.execute("file.closeProject", json!({})).unwrap();
    let r = crate::tasks::run_tasks_at(&mut s, Instant::now() + Duration::from_millis(500));
    assert_eq!(r["ran"], json!(2), "{r}");
    // A script run from File ▸ Scripts still stops when the project closes.
    s.execute("script.run", json!({"code": "app.scheduleTask('writeLn(\"menu\")', 100, true);", "name": "Menu.jsx"})).unwrap();
    s.execute("file.closeProject", json!({})).unwrap();
    let r = crate::tasks::run_tasks_at(&mut s, Instant::now() + Duration::from_millis(5000));
    assert_eq!(r["ran"], json!(2), "{r}");
}

#[test]
fn shutdown_dialogs_close_as_if_canceled() {
    let store = Arc::new(MemoryConfig::default());
    let code = r#"var a = new Window("dialog", "One").show();
var b = new Window("dialog", "Two").show();
app.settings.saveSetting("Startup Test", "shown", a + "," + b);"#;
    store.write(&format!("{SHUTDOWN_DIR}/Ask.jsx"), code).unwrap();
    // An error after the dialog closes is a failure.
    store.write(&format!("{SHUTDOWN_DIR}/Broken.jsx"), "new Window('dialog', 'Three').show();\nthrow new Error('late');").unwrap();
    let mut s = session(&store);
    let r = s.execute("file.runShutdownScripts", json!({})).unwrap();
    assert_eq!((&r["ran"], &r["waiting"]), (&json!(["Ask.jsx"]), &json!([])), "{r}");
    assert_eq!(r["failed"][0]["name"], "Broken.jsx", "{r}");
    assert!(r["failed"][0]["error"].as_str().is_some_and(|e| e.contains("late")), "{r}");
    assert_eq!(setting(&mut s, "shown"), "2,2");
    assert!(s.script_ui.windows.is_empty());
}

#[test]
fn shutdown_closes_only_its_own_scripts_dialogs() {
    let store = Arc::new(MemoryConfig::default());
    store.write(&format!("{STARTUP_DIR}/Same.jsx"), &format!("new Window('dialog', 'Startup').show();\n{}", mark("S"))).unwrap();
    store.write(&format!("{SHUTDOWN_DIR}/Same.jsx"), &format!("new Window('dialog', 'Shutdown').show();\n{}", mark("Q"))).unwrap();
    let mut s = session(&store);
    assert_eq!(s.execute("file.runStartupScripts", json!({})).unwrap()["waiting"], json!(["Same.jsx"]));
    let r = s.execute("file.runShutdownScripts", json!({})).unwrap();
    assert_eq!(r, json!({"ran": ["Same.jsx"], "waiting": [], "failed": []}));
    // The Startup script's dialog is still open, and its script has not gone on.
    assert_eq!(setting(&mut s, "order"), "Q");
    let titles: Vec<&str> = s.script_ui.windows.iter().map(|w| w.title.as_str()).collect();
    assert_eq!(titles, ["Startup"]);
}

#[test]
fn oversized_scripts_are_not_run() {
    let store = Arc::new(MemoryConfig::default());
    let big = format!("{}\n{}", "//".repeat(8 << 20), mark("x"));
    store.write(&format!("{STARTUP_DIR}/Big.jsx"), &big).unwrap();
    let mut s = session(&store);
    let r = s.execute("file.runStartupScripts", json!({})).unwrap();
    assert_eq!(r["ran"], json!([]));
    assert_eq!(r["failed"][0]["name"], "Big.jsx");
    let events = s.drain_events();
    assert!(events.iter().any(|e| matches!(e, effectcraft_engine::Event::Toast { message, error: true } if message.starts_with("Big.jsx"))), "{events:?}");
    assert_eq!(setting(&mut s, "order"), "");
}

#[test]
fn scripts_in_a_settings_folder_on_disk_include_files_beside_them() {
    use effectcraft_engine::config::DirConfig;
    let root = std::env::temp_dir().join(format!("ec-startup-include-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for (file, code) in [
        (format!("{STARTUP_DIR}/lib/helper.jsx"), mark("i")),
        (format!("{STARTUP_DIR}/Main.jsx"), format!("#include \"lib/helper.jsx\"\n{}", mark("m"))),
        (format!("{SHUTDOWN_DIR}/Ask.jsx"), format!("new Window('dialog', 'Bye').show();\n{}", mark("q"))),
    ] {
        let path = root.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, code).unwrap();
    }
    let mut s = Session::default();
    crate::install(&mut s);
    s.config = Some(Arc::new(DirConfig::new(&root)));
    let r = s.execute("file.runStartupScripts", json!({})).unwrap();
    assert_eq!(r, json!({"ran": ["Main.jsx"], "waiting": [], "failed": []}));
    assert_eq!(setting(&mut s, "order"), "im");
    // A Shutdown script run as its file still has its dialog closed.
    let r = s.execute("file.runShutdownScripts", json!({})).unwrap();
    assert_eq!(r, json!({"ran": ["Ask.jsx"], "waiting": [], "failed": []}));
    assert_eq!(setting(&mut s, "order"), "imq");
    let _ = std::fs::remove_dir_all(&root);
}

/// A session with one comp, for checking that a refused command leaves the project open.
fn session_with_comp(store: &Arc<MemoryConfig>) -> Session {
    let mut s = session(store);
    s.execute("script.run", json!({"code": r#"app.project.items.addComp("Main", 64, 64, 1, 1, 24);"#})).unwrap();
    assert!(!s.project.items.is_empty());
    s
}

const FOLDER_COMMANDS: [&str; 2] = ["file.runStartupScripts", "file.runShutdownScripts"];

#[test]
fn scripts_cannot_run_the_startup_or_shutdown_folders() {
    let store = Arc::new(MemoryConfig::default());
    for id in FOLDER_COMMANDS {
        let mut s = session_with_comp(&store);
        let r = s.execute("script.run", json!({"code": format!("app.run('{id}', {{}})")})).unwrap();
        assert!(r["error"]["message"].as_str().is_some_and(|m| m.contains("scripts can't run scripts")), "{id}: {r}");
        assert!(!s.project.items.is_empty(), "{id}");
    }
}

#[test]
fn command_scripts_cannot_run_the_startup_or_shutdown_folders() {
    let store = Arc::new(MemoryConfig::default());
    for id in FOLDER_COMMANDS {
        let mut s = session_with_comp(&store);
        let e = s.execute("file.runScript", json!({"steps": [{"command": id}]})).unwrap_err().to_string();
        assert!(e.contains("scripts can't run scripts"), "{id}: {e}");
        assert!(!s.project.items.is_empty(), "{id}");
    }
}

#[test]
fn batches_cannot_run_the_startup_or_shutdown_folders() {
    let store = Arc::new(MemoryConfig::default());
    for id in FOLDER_COMMANDS {
        let mut s = session_with_comp(&store);
        let e = s.execute("engine.batch", json!({"steps": [{"command": id}]})).unwrap_err().to_string();
        assert!(e.contains("can't run inside a batch"), "{id}: {e}");
        assert!(!s.project.items.is_empty(), "{id}");
    }
}

/// A ScriptUI panel whose script appends `p` to the `Startup Test` / `order` setting each time it
/// runs, remembered as open at the last quit.
fn session_with_panel(store: &Arc<MemoryConfig>) -> Session {
    store.write(&format!("{PANELS_DIR}/Mark Panel.jsx"), &format!("this.add('statictext', undefined, 'Mark');\n{}", mark("p"))).unwrap();
    store.write(OPEN_PANELS_FILE, &json!({"open": ["Mark Panel.jsx"]}).to_string()).unwrap();
    session(store)
}

fn panel_open(s: &Session) -> bool {
    s.script_ui.windows.iter().any(|w| w.kind == WindowKind::Panel && w.script == "Mark Panel.jsx")
}

/// The commands that open ScriptUI panels, which run the panel's script.
fn panel_commands() -> [(&'static str, serde_json::Value); 2] {
    [("window.scriptPanel", json!({"name": "Mark Panel.jsx"})), ("window.restoreScriptPanels", json!({}))]
}

#[test]
fn scripts_cannot_open_script_panels() {
    for (id, params) in panel_commands() {
        let store = Arc::new(MemoryConfig::default());
        let mut s = session_with_panel(&store);
        // Opened by the user: a script's call would only bring it forward.
        s.execute("window.scriptPanel", json!({"name": "Mark Panel.jsx"})).unwrap();
        let r = s.execute("script.run", json!({"code": format!("app.run('{id}', {params})")})).unwrap();
        assert!(r["error"]["message"].as_str().is_some_and(|m| m.contains("scripts can't run scripts")), "{id}: {r}");
        assert_eq!(setting(&mut s, "order"), "p", "{id}");
        assert_eq!(store.read(OPEN_PANELS_FILE).unwrap_or_default(), json!({"open": ["Mark Panel.jsx"]}).to_string(), "{id}");
    }
}

#[test]
fn command_scripts_and_batches_cannot_open_script_panels() {
    for (id, params) in panel_commands() {
        let store = Arc::new(MemoryConfig::default());
        let mut s = session_with_panel(&store);
        let e = s.execute("file.runScript", json!({"steps": [{"command": id, "params": params}]})).unwrap_err().to_string();
        assert!(e.contains("scripts can't run scripts"), "{id}: {e}");
        let e = s.execute("engine.batch", json!({"steps": [{"command": id, "params": params}]})).unwrap_err().to_string();
        assert!(e.contains("can't run inside a batch"), "{id}: {e}");
        assert!(!panel_open(&s), "{id}");
        assert_eq!(setting(&mut s, "order"), "", "{id}");
    }
}

#[test]
fn the_app_still_reopens_script_panels_at_launch() {
    let store = Arc::new(MemoryConfig::default());
    let mut s = session_with_panel(&store);
    // As the desktop and web apps call it at launch.
    let r = s.execute("window.restoreScriptPanels", json!({})).unwrap();
    assert_eq!(r, json!({"opened": ["Mark Panel.jsx"], "failed": []}));
    assert!(panel_open(&s));
    assert_eq!(setting(&mut s, "order"), "p");
}

#[test]
fn empty_or_missing_folders_run_nothing() {
    let none = json!({"ran": [], "waiting": [], "failed": []});
    let store = Arc::new(MemoryConfig::default());
    let mut s = session(&store);
    assert_eq!(s.execute("file.runStartupScripts", json!({})).unwrap(), none);
    // A session without a settings folder.
    let mut s = Session::default();
    crate::install(&mut s);
    assert_eq!(s.execute("file.runShutdownScripts", json!({})).unwrap(), none);
}
