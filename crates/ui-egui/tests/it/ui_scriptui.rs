//! M13.1 UI: ScriptUI dialogs, palettes and dockable panels drawn from the session's script
//! windows (clicks reach the script's handlers), the branching History panel, and the Window /
//! File ▸ Scripts menus listing scripts (egui_kittest, UI logic only).

use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::dock::PanelKind;
use effectcraft_ui_egui::dock::WorkspaceExt as _;
use egui::{Event, Rect, pos2, vec2};
use egui_kittest::Harness;
use serde_json::json;

fn harness() -> Harness<'static, EffectcraftApp> {
    let mut s = effectcraft_host::session();
    s.execute("comp.new", json!({"name": "Main", "width": 320, "height": 180, "duration": 4})).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.run_steps(3);
    h
}

fn rect(h: &Harness<'_, EffectcraftApp>, id: &str) -> Rect {
    let e = h.state().auto.find(id).unwrap_or_else(|| panic!("no {id}"));
    Rect::from_min_size(pos2(e.rect[0], e.rect[1]), vec2(e.rect[2], e.rect[3]))
}

fn click(h: &mut Harness<'_, EffectcraftApp>, id: &str) {
    let c = rect(h, id).center();
    h.input_mut().events.push(Event::PointerMoved(c));
    for pressed in [true, false] {
        h.input_mut().events.push(Event::PointerButton { pos: c, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() });
    }
    h.run_steps(3);
}

fn layer_names(h: &Harness<'_, EffectcraftApp>) -> Vec<String> {
    h.state().session.active_comp().map(|c| c.layers.iter().map(|l| l.name.clone()).collect()).unwrap_or_default()
}

#[test]
fn script_windows_are_drawn_and_clickable() {
    let mut h = harness();
    let code = r#"
      var w = new Window("palette", "Quick Solid");
      var n = w.add("edittext", undefined, "Card");
      n.characters = 10;
      var go = w.add("button", undefined, "Add", { name: "add" });
      go.onClick = function () { app.project.activeItem.layers.addSolid([0, 1, 0], n.text, 50, 50, 1); };
      w.show();
    "#;
    let r = h.state_mut().session.execute("script.run", json!({"code": code, "name": "quick.jsx"})).unwrap();
    assert_eq!(r["ok"], true, "{r}");
    h.run_steps(3);
    let win = h.state().session.script_ui.windows[0].id;
    // Every control has an automation id (by id and by name).
    for id in [format!("scriptui.{win}"), format!("scriptui.{win}.1"), format!("scriptui.{win}.2"), format!("scriptui.{win}.add")] {
        assert!(h.state().auto.find(&id).is_some(), "missing {id}");
    }
    click(&mut h, &format!("scriptui.{win}.add"));
    assert_eq!(layer_names(&h), ["Card"]);
    // A modal dialog: the script waits in show() until OK is clicked.
    let code = r#"
      var d = new Window("dialog", "Confirm");
      d.add("statictext", undefined, "Make a null?");
      var g = d.add("group");
      g.add("button", undefined, "Cancel", { name: "cancel" });
      g.add("button", undefined, "OK", { name: "ok" });
      if (d.show() === 1) app.project.activeItem.layers.addNull().name = "Yes";
    "#;
    let r = h.state_mut().session.execute("script.run", json!({"code": code, "name": "confirm.jsx"})).unwrap();
    assert_eq!(r["waiting"], true, "{r}");
    h.run_steps(3);
    let d = h.state().session.script_ui.windows.iter().find(|w| w.modal).unwrap().id;
    click(&mut h, &format!("scriptui.{d}.ok"));
    assert_eq!(layer_names(&h), ["Yes", "Card"]);
    assert!(h.state().session.script_ui.windows.iter().all(|w| !w.modal));
    // Window ▸ <panel>: the sample ScriptUI panel docks as its own tab.
    h.state_mut().session.execute("window.scriptPanel", json!({"name": "Layer Tools.jsx"})).unwrap();
    h.run_steps(3);
    let panel = h.state().session.script_ui.windows.iter().find(|w| w.script == "Layer Tools.jsx").unwrap().id;
    assert!(h.state().ui.dock.is_docked(PanelKind::ScriptPanel(panel)), "the panel docks");
    assert!(h.state().auto.find(&format!("scriptui.{panel}")).is_some());
    // The menus list scripts and panels.
    let cx = effectcraft_engine::menus::DynCtx::default();
    let (panels, _) = effectcraft_engine::menus::dynamic(&h.state().session, "scriptPanels", &cx);
    assert!(panels.iter().any(|e| e.label == "Layer Tools.jsx" && e.command == "window.scriptPanel"));
    let (scripts, _) = effectcraft_engine::menus::dynamic(&h.state().session, "scripts", &cx);
    assert!(scripts.iter().any(|e| e.label == "Rename Layers.jsx" && e.command == "file.runScript"));
    // Closing the panel's tab closes its script window.
    h.state_mut().close_panel(PanelKind::ScriptPanel(panel));
    h.run_steps(2);
    assert!(h.state().session.script_ui.window(panel).is_none());
    h.state_mut().session.execute("scriptui.close", json!({})).unwrap();
}

/// A ScriptUI panel open when the app quit opens again, docked, at the next launch (the
/// frontends call `window.restoreScriptPanels` before building the app).
#[test]
fn panels_open_at_quit_dock_again_at_launch() {
    use effectcraft_engine::config::{ConfigStore, MemoryConfig};
    let store = std::sync::Arc::new(MemoryConfig::default());
    store.write("Scripts/ScriptUI Panels/Hello Panel.jsx", "this.add('button', undefined, 'Hi', { name: 'hi' });").unwrap();
    store.write(effectcraft_engine::commands::scripts::OPEN_PANELS_FILE, r#"{"open": ["Hello Panel.jsx"]}"#).unwrap();
    let mut s = effectcraft_host::session();
    s.config = Some(store.clone());
    s.execute("window.restoreScriptPanels", json!({})).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.run_steps(3);
    let panel = h.state().session.script_ui.windows.iter().find(|w| w.script == "Hello Panel.jsx").expect("the panel's window").id;
    assert!(h.state().ui.dock.is_docked(PanelKind::ScriptPanel(panel)), "the panel docks");
    assert!(h.state().auto.find(&format!("scriptui.{panel}.hi")).is_some(), "its controls are drawn");
    // Closing its tab forgets it for the next launch.
    h.state_mut().close_panel(PanelKind::ScriptPanel(panel));
    h.run_steps(2);
    assert_eq!(store.read(effectcraft_engine::commands::scripts::OPEN_PANELS_FILE).as_deref(), Some(r#"{"open":[]}"#));
}

/// The bundled Ease Presets ScriptUI panel (Window ▸ Ease Presets.jsx) docks, and clicking a
/// preset in its list eases the selected keyframes in one undo step.
#[test]
fn ease_presets_panel_eases_the_selected_keys_from_its_list() {
    use effectcraft_engine::keyframe::{Ease, Interp};
    use egui_kittest::kittest::Queryable;
    let mut h = harness();
    let s = &mut h.state_mut().session;
    let l = s.execute("layer.newSolid", json!({"name": "Box", "color": "#e04020", "width": 80, "height": 80})).unwrap()["layer"].as_u64().unwrap();
    for (t, v) in [(0.0, 0.0), (1.0, 100.0)] {
        s.execute("prop.addKey", json!({"layer": l, "path": "transform/opacity", "time": t, "value": v})).unwrap();
    }
    s.execute("prop.select", json!({"layer": l, "path": "transform/opacity"})).unwrap();
    let cx = effectcraft_engine::menus::DynCtx::default();
    let (panels, _) = effectcraft_engine::menus::dynamic(s, "scriptPanels", &cx);
    assert!(panels.iter().any(|e| e.label == "Ease Presets.jsx"), "listed in the Window menu");
    s.execute("window.scriptPanel", json!({"name": "Ease Presets.jsx"})).unwrap();
    h.run_steps(3);
    let panel = h.state().session.script_ui.windows.iter().find(|w| w.script == "Ease Presets.jsx").unwrap().id;
    assert!(h.state().ui.dock.is_docked(PanelKind::ScriptPanel(panel)), "the panel docks");
    h.state_mut().ui.maximized = Some(PanelKind::ScriptPanel(panel));
    h.run_steps(3);
    let steps = h.state().session.history.undo.len();
    let item = h.query_by_label("Ease In-Out").expect("the Ease In-Out preset").rect().center();
    h.input_mut().events.push(Event::PointerMoved(item));
    for pressed in [true, false] {
        h.input_mut().events.push(Event::PointerButton { pos: item, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() });
    }
    h.run_steps(3);
    let op = h.state().session.active_comp().unwrap().layer(effectcraft_engine::project::LayerId(l)).unwrap().props.prop("transform/opacity").unwrap().clone();
    let eased = Ease { speed: 0.0, influence: 0.5 };
    assert_eq!((op.keys[0].out_interp, op.keys[0].out_ease[0]), (Interp::Bezier, eased));
    assert_eq!((op.keys[1].in_interp, op.keys[1].in_ease[0]), (Interp::Bezier, eased));
    assert_eq!(h.state().session.history.undo.len(), steps + 1);
}

#[test]
fn history_panel_jumps_between_branches() {
    let mut h = harness();
    for n in ["A", "B"] {
        h.state_mut().session.execute("layer.newSolid", json!({"name": n, "color": "#ffffff", "width": 10, "height": 10})).unwrap();
    }
    h.state_mut().session.undo();
    h.state_mut().session.execute("layer.newSolid", json!({"name": "C", "color": "#ffffff", "width": 10, "height": 10})).unwrap();
    assert_eq!(layer_names(&h), ["C", "A"]);
    h.state_mut().show_panel(PanelKind::History);
    h.run_steps(3);
    // Original, New Composition, A, C, and B on its branch.
    let states = h.state().session.history_tree();
    assert_eq!(states.len(), 5);
    let b = states.iter().find(|n| n.depth == 1).unwrap().index;
    click(&mut h, &format!("history.state.{b}"));
    assert_eq!(layer_names(&h), ["B", "A"]);
    let c = h.state().session.history_tree().iter().find(|n| n.depth == 1).unwrap().index;
    click(&mut h, &format!("history.state.{c}"));
    assert_eq!(layer_names(&h), ["C", "A"]);
}

/// onDraw fills of concave paths and real images (drawImage, image controls), rendered.
#[test]
fn on_draw_concave_fills_and_images_render() {
    let dir = std::env::temp_dir().join(format!("effectcraft-scriptui-img-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let png = dir.join("green.png");
    image::RgbaImage::from_pixel(8, 8, image::Rgba([0, 255, 0, 255])).save(&png).unwrap();
    let png = png.to_string_lossy().replace('\\', "/");
    let mut s = effectcraft_host::session();
    s.execute("comp.new", json!({"name": "Main", "width": 320, "height": 180, "duration": 4})).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_pixels_per_point(1.0).wgpu().build_eframe(|_| EffectcraftApp::new(s));
    h.run_steps(3);
    let code = format!(
        r#"
      var w = new Window("palette", "Paint");
      var c = w.add("group");
      c.preferredSize = [120, 80];
      c.onDraw = function () {{
        var g = this.graphics;
        g.newPath();
        // An L: the top-right quarter is a notch (a convex-only fill would cover it).
        g.moveTo(0, 0); g.lineTo(60, 0); g.lineTo(60, 40); g.lineTo(120, 40); g.lineTo(120, 80); g.lineTo(0, 80); g.closePath();
        g.fillPath(g.newBrush(g.BrushType.SOLID_COLOR, [1, 0, 0, 1]));
        g.drawImage(ScriptUI.newImage(File("{png}")), 90, 5, 20, 20);
      }};
      var im = w.add("image", undefined, File("{png}"));
      im.preferredSize = [40, 40];
      w.show();
    "#
    );
    let r = h.state_mut().session.execute("script.run", json!({"code": code, "name": "paint.jsx"})).unwrap();
    assert_eq!(r["ok"], true, "{r}");
    h.run_steps(4);
    let win = h.state().session.script_ui.windows[0].clone();
    let g = &win.root.children[0];
    assert!(matches!(&g.draw[1], effectcraft_engine::scriptui::DrawOp::Image { image: Some(i), .. } if i.src.as_deref() == Some(png.as_str())));
    assert_eq!(win.root.children[1].image.as_ref().and_then(|i| i.src.clone()).as_deref(), Some(png.as_str()));
    let img = h.render().expect("render");
    if let Ok(d) = std::env::var("EC_SNAPSHOT_DIR") {
        img.save(format!("{d}/scriptui_paint.png")).unwrap();
    }
    let px = |p: egui::Pos2| img.get_pixel(p.x as u32, p.y as u32).0;
    let gr = rect(&h, &format!("scriptui.{}.{}", win.id, g.id));
    let red = |c: [u8; 4]| c[0] > 200 && c[1] < 60 && c[2] < 60;
    let green = |c: [u8; 4]| c[1] > 200 && c[0] < 60 && c[2] < 60;
    assert!(red(px(gr.min + vec2(20.0, 20.0))), "inside the L: {:?}", px(gr.min + vec2(20.0, 20.0)));
    assert!(red(px(gr.min + vec2(100.0, 60.0))), "the L's foot");
    assert!(!red(px(gr.min + vec2(75.0, 30.0))), "the notch stays empty: {:?}", px(gr.min + vec2(75.0, 30.0)));
    assert!(green(px(gr.min + vec2(100.0, 15.0))), "drawImage: {:?}", px(gr.min + vec2(100.0, 15.0)));
    let ir = rect(&h, &format!("scriptui.{}.{}", win.id, win.root.children[1].id));
    assert!(green(px(ir.center())), "image control: {:?}", px(ir.center()));
    h.state_mut().session.execute("scriptui.close", json!({})).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

/// ScriptUI mouse events (#766): the pointer over a script window reaches the script's
/// `addEventListener` listeners (element-relative position, button, click count), and a
/// pointer click on a checkbox with a click listener toggles it once.
#[test]
fn script_windows_send_mouse_events_to_listeners() {
    let mut h = harness();
    let code = r#"
      var w = new Window("palette", "Pad");
      var pad = w.add("group", undefined, undefined, { name: "pad" });
      pad.preferredSize = [120, 80];
      var log = w.add("statictext", undefined, "", { name: "log" });
      log.preferredSize = [400, 20];
      var snap = w.add("checkbox", undefined, "Snap", { name: "snap" });
      var seen = [];
      var rec = function (e) { seen.push(e.type + " " + Math.round(e.clientX) + "," + Math.round(e.clientY) + " b" + e.button + " d" + e.detail); log.text = seen.join("|"); };
      pad.addEventListener("mouseover", rec);
      pad.addEventListener("mousedown", rec);
      pad.addEventListener("mouseup", rec);
      pad.addEventListener("click", rec);
      snap.addEventListener("click", function (e) { log.text = "snap " + snap.value + " d" + e.detail; });
      w.show();
    "#;
    let r = h.state_mut().session.execute("script.run", json!({"code": code, "name": "pad.jsx"})).unwrap();
    assert_eq!(r["ok"], true, "{r}");
    h.run_steps(3);
    let win = h.state().session.script_ui.windows[0].id;
    let text = |h: &Harness<'_, EffectcraftApp>, name: &str| h.state().session.script_ui.windows[0].root.lookup(&json!(name)).cloned().unwrap();
    let pad = rect(&h, &format!("scriptui.{win}.pad"));
    let at = pad.min + vec2(10.0, 20.0);
    click_at(&mut h, at);
    assert_eq!(text(&h, "log").text, "mouseover 10,20 b0 d0|mousedown 10,20 b0 d1|mouseup 10,20 b0 d1|click 10,20 b0 d1");
    click(&mut h, &format!("scriptui.{win}.snap"));
    let (snap, log) = (text(&h, "snap"), text(&h, "log"));
    assert!(snap.checked, "toggled once");
    assert_eq!(log.text, "snap true d1");
    h.state_mut().session.execute("scriptui.close", json!({})).unwrap();
}

fn click_at(h: &mut Harness<'_, EffectcraftApp>, c: egui::Pos2) {
    h.input_mut().events.push(Event::PointerMoved(c));
    h.run_steps(1);
    for pressed in [true, false] {
        h.input_mut().events.push(Event::PointerButton { pos: c, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() });
    }
    h.run_steps(3);
}
#[test]
fn scheduled_import_reveals_its_composition_without_manual_clicks() {
    let mut s = effectcraft_host::session();
    let result = s.execute("script.run", json!({"code": "var w=new Window('palette','Automatic import');var b=w.add('button',undefined,'Open');b.onClick=function(){app.scheduleTask(\"var c=app.project.items.addComp('Imported',64,64,1,1,25);c.openInViewer();\",0,false);};w.show();"})).unwrap();
    assert_eq!(result["ok"], true, "{result}");
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.state_mut().ui.start_screen = true;
    h.run_steps(2);
    h.state_mut().session.execute("scriptui.click", json!({"window":"Automatic import","widget":"Open"})).unwrap();
    h.run_steps(4);
    let active = h.state().session.active_comp_id().unwrap();
    assert_eq!(h.state().session.project.items.get(&active).unwrap().name, "Imported");
    assert!(!h.state().ui.start_screen, "Home must not hide a composition opened by the scheduled import");
    h.state_mut().session.execute("scriptui.close", json!({"window":"Automatic import"})).unwrap();
}
