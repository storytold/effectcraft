//! ScriptUI: windows built by scripts are published to the session, drawn by frontends and
//! driven through the `scriptui.*` commands; dialogs block `show()` until they close; dockable
//! panels; File ▸ Scripts (installed and sample scripts).

use std::sync::Arc;

use effectcraft_engine::Session;
use effectcraft_engine::scriptui::{WidgetKind, WindowKind};
use serde_json::{Value, json};

use crate::run_code;

fn session() -> Session {
    let mut s = Session { expr: Some(Arc::new(effectcraft_expr::Expressions)), ..Default::default() };
    crate::install(&mut s);
    s.config = Some(Arc::new(effectcraft_engine::config::MemoryConfig::default()));
    s
}

fn comp_names(s: &Session) -> Vec<String> {
    s.project.items.values().filter(|i| i.as_comp().is_some()).map(|i| i.name.clone()).collect()
}

fn layer_names(s: &Session) -> Vec<String> {
    s.active_comp().map(|c| c.layers.iter().map(|l| l.name.clone()).collect()).unwrap_or_default()
}

const PALETTE: &str = r#"
var made = 0;
var w = new Window("palette", "Maker");
w.orientation = "column";
w.alignChildren = ["fill", "top"];
var name = w.add("edittext", undefined, "Box");
name.characters = 12;
var big = w.add("checkbox", undefined, "Big");
var row = w.add("group");
var make = row.add("button", undefined, "Make", { name: "make" });
var label = row.add("statictext", undefined, "none yet");
var size = w.add("slider", undefined, 50, 10, 200);
var kind = w.add("dropdownlist", undefined, ["Solid", "Null"]);
kind.selection = 0;
var echo = "";
name.onChange = function () { echo = name.text; };
make.onClick = function () {
  var c = app.project.activeItem;
  var s = big.value ? 200 : Math.round(size.value);
  if (kind.selection.index === 0) c.layers.addSolid([1, 0, 0], name.text, s, s, 1);
  else c.layers.addNull().name = name.text;
  made++;
  label.text = "made " + made;
  writeLn("made " + name.text + " " + s);
};
w.show();
"palette up"
"#;

#[test]
fn palette_controls_run_their_handlers() {
    let mut s = session();
    s.execute("comp.new", json!({"name": "Main", "width": 320, "height": 180, "duration": 2})).unwrap();
    let o = run_code(&mut s, PALETTE, "maker.jsx");
    assert!(o.error.is_none(), "{:?}", o.error);
    assert_eq!(o.result, json!("palette up"));
    assert_eq!(s.script_ui.windows.len(), 1);
    let w = s.script_ui.windows[0].clone();
    assert_eq!((w.kind, w.title.as_str(), w.script.as_str()), (WindowKind::Palette, "Maker", "maker.jsx"));
    let kinds: Vec<WidgetKind> = w.root.children.iter().map(|c| c.kind).collect();
    assert_eq!(kinds, [WidgetKind::EditText, WidgetKind::Checkbox, WidgetKind::Group, WidgetKind::Slider, WidgetKind::DropDownList]);
    // Laid out: the window has a size, the fill-aligned controls span its width.
    assert!(w.root.bounds[2] > 100.0 && w.root.bounds[3] > 100.0, "{:?}", w.root.bounds);
    let edit = &w.root.children[0];
    assert_eq!(edit.bounds[2], w.root.bounds[2] - 30.0);
    assert_eq!(w.root.children[3].value, 50.0);
    assert_eq!((w.root.children[3].min, w.root.children[3].max), (10.0, 200.0));
    assert_eq!(w.root.children[4].items, ["Solid", "Null"]);
    assert_eq!(w.root.children[4].selection, [0]);
    assert!(w.root.children[2].children[0].handlers.contains(&"onClick".to_string()));
    // Agents: list, read, click.
    let list = s.execute("scriptui.list", json!({})).unwrap();
    assert_eq!(list[0]["title"], "Maker");
    let tree = s.execute("scriptui.get", json!({"window": w.id})).unwrap();
    assert_eq!(tree["root"]["children"][2]["children"][0]["name"], "make");
    let r = s.execute("scriptui.click", json!({"widget": "make"})).unwrap();
    assert_eq!(r["output"], "made Box 50", "{r}");
    assert_eq!(layer_names(&s), ["Box"]);
    // Handlers' edits are undoable engine commands.
    assert!(s.undo());
    assert!(layer_names(&s).is_empty());
    // Set values like a user: text (onChange), checkbox, slider, list by item text.
    s.execute("scriptui.set", json!({"widget": "#1", "value": "Crate"})).unwrap();
    s.execute("scriptui.click", json!({"widget": "Big"})).unwrap();
    s.execute("scriptui.set", json!({"window": "Maker", "widget": 7, "value": "Null"})).unwrap();
    let r = s.execute("scriptui.click", json!({"widget": "make"})).unwrap();
    assert_eq!(r["output"], "made Crate 200");
    assert_eq!(layer_names(&s), ["Crate"]);
    assert!(s.active_comp().unwrap().layers[0].source == effectcraft_engine::project::LayerSource::Null);
    // The handler updated a label: the published tree follows.
    let w = s.script_ui.windows[0].clone();
    assert_eq!(w.root.children[2].children[1].text, "made 2");
    assert!(w.root.children[1].checked);
    assert_eq!(w.root.children[4].selection, [1]);
    // Errors.
    assert!(s.execute("scriptui.click", json!({"widget": "nope"})).is_err());
    assert!(s.execute("scriptui.get", json!({"window": 999_999})).is_err());
    // Closing the window ends the script host.
    s.execute("scriptui.close", json!({})).unwrap();
    assert!(s.script_ui.windows.is_empty());
    assert!(s.execute("scriptui.list", json!({})).unwrap().as_array().unwrap().is_empty());
}

const DIALOG: &str = r#"
var d = new Window("dialog", "New Comp");
d.alignChildren = "left";
var e = d.add("edittext", undefined, "Untitled");
var fps = d.add("radiobutton", undefined, "24 fps");
var fps30 = d.add("radiobutton", undefined, "30 fps");
fps.value = true;
var g = d.add("group");
g.add("button", undefined, "Cancel", { name: "cancel" });
var ok = g.add("button", undefined, "OK", { name: "ok" });
writeLn("before");
var r = d.show();
writeLn("result " + r + " " + e.text + " " + (fps30.value ? 30 : 24));
if (r === 1) app.project.items.addComp(e.text, 100, 100, 1, 1, fps30.value ? 30 : 24);
r
"#;

#[test]
fn dialogs_block_show_until_they_close() {
    let mut s = session();
    let o = run_code(&mut s, DIALOG, "dialog.jsx");
    // The script waits in show(); the session is back here meanwhile.
    assert!(o.waiting, "{o:?}");
    assert_eq!(o.output, ["before"]);
    assert_eq!(s.script_ui.windows.len(), 1);
    let w = s.script_ui.windows[0].clone();
    assert!(w.modal && w.kind == WindowKind::Dialog);
    // The app keeps working while the dialog is up.
    s.execute("comp.new", json!({"name": "Other", "width": 64, "height": 64, "duration": 1})).unwrap();
    s.execute("scriptui.set", json!({"widget": "#1", "value": "Shot 010"})).unwrap();
    s.execute("scriptui.click", json!({"widget": "30 fps"})).unwrap();
    let w = s.script_ui.windows[0].clone();
    assert!(!w.root.children[1].checked && w.root.children[2].checked, "radio buttons are exclusive");
    // OK closes the dialog with 1: the script runs on.
    let r = s.execute("scriptui.click", json!({"widget": "ok"})).unwrap();
    assert_eq!(r["output"], "result 1 Shot 010 30", "{r}");
    assert_eq!(r["result"], 1);
    assert!(r.get("waiting").is_none());
    assert!(s.script_ui.windows.is_empty());
    let names = comp_names(&s);
    assert!(names.contains(&"Shot 010".to_string()) && names.contains(&"Other".to_string()), "{names:?}");
    // Cancel (or closing the window) returns 2.
    let o = run_code(&mut s, DIALOG, "dialog.jsx");
    assert!(o.waiting);
    let r = s.execute("scriptui.close", json!({})).unwrap();
    assert_eq!(r["output"], "result 2 Untitled 24");
    assert_eq!(comp_names(&s).len(), 2);
}

#[test]
fn layout_and_handler_errors() {
    let mut s = session();
    let code = r#"
    var w = new Window("palette", "L");
    var g = w.add("group");
    g.orientation = "row";
    var a = g.add("button", undefined, "A");
    var b = g.add("button", undefined, "B");
    b.preferredSize = [120, 30];
    var p = w.add("panel", undefined, "Options");
    p.add("checkbox", undefined, "One");
    var bad = w.add("button", undefined, "Bad");
    bad.onClick = function () { nope(); };
    w.layout.layout(true);
    w.show();
    [a.bounds.width, b.size.width, b.size.height, b.bounds[0] - a.bounds[0], w.bounds.width > 200, p.bounds.top > a.bounds.bottom]
    "#;
    let o = run_code(&mut s, code, "layout.jsx");
    assert!(o.error.is_none(), "{:?}", o.error);
    assert_eq!(o.result, json!([80, 120, 30, 90, true, true]));
    // An error in a handler is reported, the window stays.
    let r = s.execute("scriptui.click", json!({"widget": "Bad"})).unwrap();
    assert_eq!(r["ok"], false);
    assert!(r["error"]["message"].as_str().unwrap().contains("nope"), "{r}");
    assert_eq!(s.script_ui.windows.len(), 1);
    // Resource strings build windows too (see tests_ui_more).
    let o = run_code(&mut s, "new Window(\"dialog { text: 'x' }\").text", "res.jsx");
    assert_eq!(o.result, json!("x"));
    s.execute("scriptui.close", json!({})).unwrap();
}

#[test]
fn console_windows_run_inline() {
    let mut s = session();
    let req = effectcraft_engine::ScriptRequest {
        code: "var cw = new Window('palette', 'Console UI'); var cb = cw.add('button', undefined, 'Hi'); var hits = 0; cb.onClick = function () { hits++; writeLn('hit ' + hits); }; cw.show(); 1",
        name: "Script Console",
        console: true,
    };
    let o = crate::run(&mut s, &req);
    assert!(o.error.is_none(), "{:?}", o.error);
    assert_eq!(s.script_ui.windows.len(), 1);
    let r = s.execute("scriptui.click", json!({"widget": "Hi"})).unwrap();
    assert_eq!(r["output"], "hit 1");
    // The console context sees the handler's state.
    let o = crate::run(&mut s, &effectcraft_engine::ScriptRequest { code: "hits", name: "Script Console", console: true });
    assert_eq!(o.result, json!(1));
    // Dialogs don't block in the console.
    let o = crate::run(
        &mut s,
        &effectcraft_engine::ScriptRequest { code: "var dd = new Window('dialog', 'D'); dd.show(); 'went on'", name: "Script Console", console: true },
    );
    assert_eq!(o.result, json!("went on"));
    assert_eq!(s.script_ui.windows.len(), 2);
    s.execute("scriptui.close", json!({"window": "D"})).unwrap();
    s.execute("scriptui.close", json!({"window": "Console UI"})).unwrap();
    assert!(s.script_ui.windows.is_empty());
}

fn solid(s: &mut Session, name: &str, at: f64) -> u64 {
    let id = s.execute("layer.newSolid", json!({"name": name, "color": "#808080", "width": 20, "height": 20})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.timing", json!({"layers": [id], "in": at})).unwrap();
    id
}

#[test]
fn sample_scripts_and_installing() {
    let mut s = session();
    s.execute("comp.new", json!({"name": "Main", "width": 400, "height": 200, "duration": 10})).unwrap();
    for (n, at) in [("C", 2.0), ("A", 0.0), ("B", 1.0)] {
        solid(&mut s, n, at);
    }
    // File ▸ Scripts lists the samples.
    let list = s.execute("file.scripts.list", json!({})).unwrap();
    let names: Vec<&str> = list.as_array().unwrap().iter().map(|e| e["name"].as_str().unwrap()).collect();
    for want in ["Rename Layers.jsx", "Sort Layers by In Point.jsx", "Create Null at Selected Layers.jsx", "Render Queue Batch.jsx", "Layer Tools.jsx"] {
        assert!(names.contains(&want), "{names:?}");
    }
    // Sort Layers by In Point (all layers when none are selected).
    s.execute("edit.deselectAll", json!({})).unwrap();
    s.execute("file.runScript", json!({"name": "Sort Layers by In Point.jsx"})).unwrap();
    assert_eq!(layer_names(&s), ["A", "B", "C"]);
    assert!(s.undo());
    assert_eq!(layer_names(&s), ["B", "A", "C"]);
    // Create Null at Selected Layers.
    let ids: Vec<u64> = s.active_comp().unwrap().layers.iter().take(2).map(|l| l.id.0).collect();
    s.execute("layer.select", json!({"layers": ids})).unwrap();
    s.execute("file.runScript", json!({"name": "Create Null at Selected Layers.jsx"})).unwrap();
    let c = s.active_comp().unwrap();
    let null = c.layers.iter().find(|l| l.name == "Centroid").unwrap();
    assert!(c.layers.iter().filter(|l| ids.contains(&l.id.0)).all(|l| l.parent == Some(null.id)));
    // Rename Layers: a dialog; fill it in and press OK.
    s.execute("layer.select", json!({"layers": ids})).unwrap();
    let r = s.execute("file.runScript", json!({"name": "Rename Layers.jsx"})).unwrap();
    assert_eq!(r["waiting"], true);
    s.execute("scriptui.set", json!({"widget": "#11", "value": "Shot_"})).unwrap();
    s.execute("scriptui.click", json!({"widget": "Number the layers"})).unwrap();
    s.execute("scriptui.click", json!({"widget": "ok"})).unwrap();
    let names = layer_names(&s);
    assert!(names.contains(&"Shot_B 1".to_string()) && names.contains(&"Shot_A 2".to_string()), "{names:?}");
    // Install a script and a ScriptUI panel (they live in the settings store).
    let dir = std::env::temp_dir().join(format!("ec-scripts-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("Hello.jsx");
    std::fs::write(&file, "writeLn('hello from ' + $.fileName); 42").unwrap();
    s.execute("file.installScript", json!({"path": file.to_string_lossy()})).unwrap();
    let panel = dir.join("Mini Panel.jsx");
    std::fs::write(&panel, "var p = this; p.add('button', undefined, 'Go').onClick = function () { writeLn('go'); }; p.layout.layout(true);").unwrap();
    s.execute("file.installScriptUIPanel", json!({"path": panel.to_string_lossy()})).unwrap();
    let list = s.execute("file.scripts.list", json!({})).unwrap();
    assert!(list.as_array().unwrap().iter().any(|e| e["name"] == "Hello.jsx" && e["source"] == "installed" && e["panel"] == false));
    assert!(list.as_array().unwrap().iter().any(|e| e["name"] == "Mini Panel.jsx" && e["panel"] == true));
    let r = s.execute("file.runScript", json!({"name": "Hello.jsx"})).unwrap();
    assert_eq!(r["result"], 42);
    // Window ▸ Mini Panel.jsx: a dockable panel; `this` is the panel.
    let r = s.execute("window.scriptPanel", json!({"name": "Mini Panel.jsx"})).unwrap();
    let wid = r["window"].as_u64().unwrap();
    let w = s.script_ui.window(wid as u32).unwrap().clone();
    assert_eq!((w.kind, w.title.as_str()), (WindowKind::Panel, "Mini Panel"));
    let r = s.execute("scriptui.click", json!({"window": wid, "widget": "Go"})).unwrap();
    assert_eq!(r["output"], "go");
    // Opening it again brings the same panel forward.
    let r = s.execute("window.scriptPanel", json!({"name": "Mini Panel.jsx"})).unwrap();
    assert_eq!(r["window"], wid);
    // The sample panel docks too, and works.
    let r = s.execute("window.scriptPanel", json!({"name": "Layer Tools.jsx"})).unwrap();
    let tools = r["window"].as_u64().unwrap();
    let before = s.active_comp().unwrap().layers.len();
    s.execute("layer.select", json!({"layers": ids})).unwrap();
    s.execute("scriptui.click", json!({"window": tools, "widget": "Null at Centroid"})).unwrap();
    assert_eq!(s.active_comp().unwrap().layers.len(), before + 1);
    s.execute("scriptui.close", json!({"window": tools})).unwrap();
    s.execute("scriptui.close", json!({"window": wid})).unwrap();
    assert!(s.execute("file.installScript", json!({"path": "/nope/x.txt"})).is_err());
    s.execute("file.uninstallScript", json!({"name": "Hello.jsx"})).unwrap();
    assert!(s.execute("file.runScript", json!({"name": "Hello.jsx"})).is_err());
    let _ = std::fs::remove_dir_all(&dir);
    let _: Value = json!(null);
}
