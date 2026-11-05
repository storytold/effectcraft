//! #3128: a saved workspace names a ScriptUI panel by its script, not by a window id that only
//! means something in the process that made it. Restarts, Reset to Saved Layout and later opens
//! find each script's own place; a script that isn't open keeps its place without a dead tab.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use craft_ui::docking::{Action, Placement};
use effectcraft_engine::Session;
use effectcraft_engine::commands::scripts::OPEN_PANELS_FILE;
use effectcraft_engine::config::{ConfigStore, MemoryConfig};
use effectcraft_engine::scriptui::{ScriptWindow, WindowKind};
use effectcraft_ui_egui::dock::{self, PanelKind, SplitSize, Workspace, WorkspaceExt as _};
use effectcraft_ui_egui::{EffectcraftApp, WORKSPACES_FILE};
use egui_kittest::Harness;
use serde_json::{Value, json};

const BUTTON: &str = "this.add('button', undefined, 'Hi', { name: 'hi' });";

fn script_file(name: &str) -> String {
    format!("Scripts/ScriptUI Panels/{name}")
}

/// A settings store with `scripts` installed and remembered as open at the last quit.
fn store_with(scripts: &[&str]) -> Arc<MemoryConfig> {
    let store = Arc::new(MemoryConfig::default());
    for name in scripts {
        store.write(&script_file(name), BUTTON).unwrap();
    }
    store.write(OPEN_PANELS_FILE, &json!({ "open": scripts }).to_string()).unwrap();
    store
}

/// What a frontend has before it builds the app: the session, panels reopened.
fn restored(store: &Arc<MemoryConfig>) -> Session {
    let mut s = effectcraft_host::session();
    s.config = Some(store.clone());
    s.execute("window.restoreScriptPanels", json!({})).unwrap();
    s
}

fn harness(app: EffectcraftApp) -> Harness<'static, EffectcraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_| app);
    h.run_steps(3);
    h
}

fn panel_of(session: &Session, script: &str) -> PanelKind {
    let window = session.script_ui.windows.iter().find(|w| w.script == script).unwrap_or_else(|| panic!("no window of {script}"));
    PanelKind::ScriptPanel(window.id)
}

fn dead_tabs(app: &EffectcraftApp) -> Vec<PanelKind> {
    app.ui.dock.panels().into_iter().filter(|p| matches!(p, PanelKind::ScriptPanel(id) if app.session.script_ui.window(*id).is_none())).collect()
}

fn invoke(app: &mut EffectcraftApp, id: &str) {
    effectcraft_ui_egui::menus::invoke(app, &egui::Context::default(), id, json!({})).unwrap();
}

fn tab_beside(h: &mut Harness<'static, EffectcraftApp>, panel: PanelKind, anchor: PanelKind) {
    h.state_mut().apply_dock(Action::Move { panel, anchor, placement: Placement::Tab { before: None } }).unwrap();
}

/// Alpha.jsx goes with the Project panel and Beta.jsx with the Timeline, saved as the layout.
fn saved_layout_store() -> (Arc<MemoryConfig>, PanelKind, PanelKind) {
    let store = store_with(&["Alpha.jsx", "Beta.jsx"]);
    let mut h = harness(EffectcraftApp::new(restored(&store)));
    let (alpha, beta) = (panel_of(&h.state().session, "Alpha.jsx"), panel_of(&h.state().session, "Beta.jsx"));
    tab_beside(&mut h, alpha, PanelKind::Project);
    tab_beside(&mut h, beta, PanelKind::Timeline);
    invoke(h.state_mut(), "window.saveWorkspace");
    eframe::App::on_exit(h.state_mut());
    (store, alpha, beta)
}

fn id(panel: PanelKind) -> u64 {
    let PanelKind::ScriptPanel(id) = panel else { panic!("{panel:?} is not a script panel") };
    u64::from(id)
}

fn rename_ids(value: &mut Value, to: &BTreeMap<u64, u64>) {
    match value {
        Value::Object(fields) => {
            if let Some(id) = fields.get_mut("ScriptPanel") {
                let old = id.as_u64().unwrap();
                *id = json!(to.get(&old).copied().unwrap_or(old));
            }
            fields.values_mut().for_each(|v| rename_ids(v, to));
        }
        Value::Array(items) => items.iter_mut().for_each(|v| rename_ids(v, to)),
        _ => {}
    }
}

/// The ids in the saved file become `to`'s, as if another process had made its windows.
fn rewrite_saved_ids(store: &MemoryConfig, to: &[(PanelKind, PanelKind)]) {
    let to: BTreeMap<u64, u64> = to.iter().map(|(from, to)| (id(*from), id(*to))).collect();
    let mut doc: Value = serde_json::from_str(&store.read(WORKSPACES_FILE).unwrap()).unwrap();
    rename_ids(&mut doc, &to);
    if let Some(table) = doc.get_mut("script_panels").and_then(Value::as_object_mut) {
        for (old, name) in std::mem::take(table) {
            let old: u64 = old.parse().unwrap();
            table.insert(to.get(&old).copied().unwrap_or(old).to_string(), name);
        }
    }
    store.write(WORKSPACES_FILE, &doc.to_string()).unwrap();
}

fn saved_ids(value: &Value, out: &mut BTreeSet<u64>) {
    match value {
        Value::Object(fields) => {
            out.extend(fields.get("ScriptPanel").and_then(Value::as_u64));
            fields.values().for_each(|v| saved_ids(v, out));
        }
        Value::Array(items) => items.iter().for_each(|v| saved_ids(v, out)),
        _ => {}
    }
}

/// A saved file holding `dock` as the current layout and as the Default workspace's saved one.
fn file(dock: &Workspace, names: &[(u32, &str)]) -> String {
    let dock = serde_json::to_value(dock).unwrap();
    let mut doc = json!({
        "version": 2, "workspace": "Default", "dock": dock, "focused": "Composition", "maximized": null,
        "saved_workspaces": {"Default": dock},
    });
    if !names.is_empty() {
        let table: serde_json::Map<String, Value> = names.iter().map(|(id, name)| (id.to_string(), json!(name))).collect();
        doc["script_panels"] = table.into();
    }
    doc.to_string()
}

fn session_with_windows(store: &Arc<MemoryConfig>, windows: &[(u32, &str)]) -> Session {
    let mut s = Session { config: Some(store.clone()), ..Default::default() };
    for (id, script) in windows {
        s.script_ui.windows.push(ScriptWindow { id: *id, kind: WindowKind::Panel, script: script.to_string(), visible: true, ..Default::default() });
    }
    s
}

fn assert_loaded(app: &EffectcraftApp) {
    assert!(!app.ui.status.contains("Could not load"), "{}", app.ui.status);
}

#[test]
fn reset_puts_each_script_back_when_a_restart_swaps_their_window_ids() {
    let (store, alpha, beta) = saved_layout_store();
    let session = restored(&store);
    let (alpha_now, beta_now) = (panel_of(&session, "Alpha.jsx"), panel_of(&session, "Beta.jsx"));
    // The file's id for Alpha is Beta's window now, and the other way round.
    rewrite_saved_ids(&store, &[(alpha, beta_now), (beta, alpha_now)]);
    let mut h = harness(EffectcraftApp::new(session));
    invoke(h.state_mut(), "window.resetWorkspace");
    h.run_steps(3);
    let dock = &h.state().ui.dock;
    assert!(dock.group_of(alpha_now).contains(&PanelKind::Project), "Alpha is with {:?}", dock.group_of(alpha_now));
    assert!(dock.group_of(beta_now).contains(&PanelKind::Timeline), "Beta is with {:?}", dock.group_of(beta_now));
}

#[test]
fn a_missing_script_leaves_no_dead_tab_and_returns_to_its_saved_place() {
    let (store, _, _) = saved_layout_store();
    store.remove(&script_file("Beta.jsx")).unwrap();

    let mut app = EffectcraftApp::new(restored(&store));
    assert_eq!(dead_tabs(&app), [], "no dead tab at load");
    invoke(&mut app, "window.resetWorkspace");
    assert_eq!(dead_tabs(&app), [], "no dead tab on the first frame after Reset");
    let mut h = harness(app);
    assert_eq!(dead_tabs(h.state()), []);
    eframe::App::on_exit(h.state_mut());
    drop(h);

    // Still unknown for a whole session: the saved reference survives it.
    let mut h = harness(EffectcraftApp::new(restored(&store)));
    assert_eq!(dead_tabs(h.state()), []);
    eframe::App::on_exit(h.state_mut());
    drop(h);

    store.write(&script_file("Beta.jsx"), BUTTON).unwrap();
    let mut h = harness(EffectcraftApp::new(restored(&store)));
    h.state_mut().session.execute("window.scriptPanel", json!({"name": "Beta.jsx"})).unwrap();
    h.run_steps(3);
    let beta = panel_of(&h.state().session, "Beta.jsx");
    assert!(h.state().ui.dock.group_of(beta).contains(&PanelKind::Timeline), "Beta is with {:?}", h.state().ui.dock.group_of(beta));
}

#[test]
fn a_locked_workspace_shows_no_dead_tab_either() {
    let store = store_with(&[]);
    let mut dock = dock::docked(dock::tabs(&[PanelKind::Composition, PanelKind::ScriptPanel(900_001)], 0));
    dock.locked = true;
    store.write(WORKSPACES_FILE, &file(&dock, &[(900_001, "Gone.jsx")])).unwrap();
    let mut app = EffectcraftApp::new(session_with_windows(&store, &[]));
    assert_eq!(dead_tabs(&app), []);
    invoke(&mut app, "window.resetWorkspace");
    assert_eq!(dead_tabs(&app), []);
    let h = harness(app);
    assert_eq!(dead_tabs(h.state()), []);
}

#[test]
fn a_later_window_does_not_inherit_the_place_of_a_missing_script() {
    let store = store_with(&[]);
    let number = 900_001;
    let root = dock::hsplit(
        SplitSize::Ratio(0.5),
        dock::tabs(&[PanelKind::Project, PanelKind::ScriptPanel(number)], 0),
        dock::tabs(&[PanelKind::Composition, PanelKind::EffectsPresets], 0),
    );
    store.write(WORKSPACES_FILE, &file(&dock::docked(root), &[(number, "Gone.jsx")])).unwrap();
    let mut h = harness(EffectcraftApp::new(session_with_windows(&store, &[])));
    h.state_mut().session.script_ui.windows.push(ScriptWindow {
        id: number,
        kind: WindowKind::Panel,
        script: "Other.jsx".into(),
        visible: true,
        ..Default::default()
    });
    h.state_mut().session.events.push(effectcraft_engine::Event::Frontend { command: "window.scriptPanel".into(), params: json!({"window": number}) });
    h.run_steps(3);
    let group = h.state().ui.dock.group_of(PanelKind::ScriptPanel(number));
    assert!(!group.contains(&PanelKind::Project), "the new panel took the missing script's place: {group:?}");
}

#[test]
fn an_old_snapshot_drops_script_panel_ids_without_guessing() {
    let store = store_with(&[]);
    let (live, stale) = (PanelKind::ScriptPanel(900_001), PanelKind::ScriptPanel(900_002));
    let mut dock = dock::docked(dock::tabs(&[PanelKind::Composition, live, stale], 0));
    dock.apply(Action::Close { panel: stale }, dock::permissions).unwrap();
    store.write(WORKSPACES_FILE, &file(&dock, &[])).unwrap();
    let mut h = harness(EffectcraftApp::new(session_with_windows(&store, &[(900_001, "Other.jsx")])));
    assert_loaded(h.state());
    let referenced = |h: &Harness<'static, EffectcraftApp>| dock::referenced_panels(&h.state().ui.dock);
    assert!(!referenced(&h).contains(&live), "a number equal to a live window is not that window");
    assert!(!referenced(&h).contains(&stale));
    assert_eq!(h.state().ui.dock.panels(), [PanelKind::Composition]);
    invoke(h.state_mut(), "window.resetWorkspace");
    assert!(!referenced(&h).contains(&live) && !referenced(&h).contains(&stale), "nor in the saved layout");
}

#[test]
fn repeated_restarts_keep_places_and_one_reference_per_script() {
    let (store, _, _) = saved_layout_store();
    for restart in 0..4 {
        let mut h = harness(EffectcraftApp::new(restored(&store)));
        let (alpha, beta) = (panel_of(&h.state().session, "Alpha.jsx"), panel_of(&h.state().session, "Beta.jsx"));
        for _ in 0..2 {
            let dock = &h.state().ui.dock;
            assert!(dock.group_of(alpha).contains(&PanelKind::Project), "restart {restart}: Alpha is with {:?}", dock.group_of(alpha));
            assert!(dock.group_of(beta).contains(&PanelKind::Timeline), "restart {restart}: Beta is with {:?}", dock.group_of(beta));
            invoke(h.state_mut(), "window.resetWorkspace");
            h.run_steps(2);
        }
        invoke(h.state_mut(), "window.saveWorkspace");
        eframe::App::on_exit(h.state_mut());
        let doc: Value = serde_json::from_str(&store.read(WORKSPACES_FILE).unwrap()).unwrap();
        let mut ids = BTreeSet::new();
        saved_ids(&doc, &mut ids);
        assert_eq!(ids, BTreeSet::from([id(alpha), id(beta)]), "restart {restart}: {doc}");
        assert_eq!(doc["script_panels"].as_object().map_or(0, |table| table.len()), 2, "restart {restart}: {doc}");
    }
}

#[test]
fn focus_and_maximized_survive_a_restart_with_swapped_ids() {
    let (store, _, _) = saved_layout_store();
    let mut h = harness(EffectcraftApp::new(restored(&store)));
    let (alpha, beta) = (panel_of(&h.state().session, "Alpha.jsx"), panel_of(&h.state().session, "Beta.jsx"));
    h.state_mut().apply_dock(Action::Activate { panel: alpha }).unwrap();
    h.state_mut().apply_dock(Action::Activate { panel: PanelKind::Timeline }).unwrap();
    h.state_mut().ui.focused = alpha;
    h.state_mut().toggle_maximize(alpha);
    eframe::App::on_exit(h.state_mut());
    drop(h);

    let session = restored(&store);
    let (alpha_now, beta_now) = (panel_of(&session, "Alpha.jsx"), panel_of(&session, "Beta.jsx"));
    rewrite_saved_ids(&store, &[(alpha, beta_now), (beta, alpha_now)]);
    let app = EffectcraftApp::new(session);
    assert_eq!((app.ui.focused, app.ui.maximized), (alpha_now, Some(alpha_now)));
    let h = harness(app);
    let ui = &h.state().ui;
    assert_eq!((ui.focused, ui.maximized), (alpha_now, Some(alpha_now)), "the first frames keep them");
    assert!(ui.dock.is_visible(alpha_now) && ui.dock.is_visible(PanelKind::Timeline) && !ui.dock.is_visible(beta_now), "the front tabs stay");
}

#[test]
fn a_panel_without_the_focus_at_quit_does_not_take_it_at_launch() {
    let (store, _, _) = saved_layout_store();
    let mut h = harness(EffectcraftApp::new(restored(&store)));
    h.state_mut().ui.focused = PanelKind::Project;
    eframe::App::on_exit(h.state_mut());
    drop(h);
    let h = harness(EffectcraftApp::new(restored(&store)));
    assert_eq!(h.state().ui.focused, PanelKind::Project);
}

#[test]
fn hidden_and_floating_panels_are_remapped() {
    let store = store_with(&[]);
    let [alpha, beta, gamma] = [900_001, 900_002, 900_003].map(PanelKind::ScriptPanel);
    let mut dock = dock::docked(dock::hsplit(SplitSize::Ratio(0.5), dock::tabs(&[beta, gamma, alpha], 0), dock::tabs(&[PanelKind::Composition], 0)));
    dock.apply(Action::Float { panel: alpha, rect: [20.0, 30.0, 400.0, 300.0] }, dock::permissions).unwrap();
    dock.apply(Action::Close { panel: beta }, dock::permissions).unwrap();
    store.write(WORKSPACES_FILE, &file(&dock, &[(900_001, "Alpha.jsx"), (900_002, "Beta.jsx"), (900_003, "Gamma.jsx")])).unwrap();
    // Each script's window has the number of the next script in the file.
    let app = EffectcraftApp::new(session_with_windows(&store, &[(900_002, "Alpha.jsx"), (900_003, "Beta.jsx"), (900_001, "Gamma.jsx")]));
    let [alpha, beta, gamma] = [panel_of(&app.session, "Alpha.jsx"), panel_of(&app.session, "Beta.jsx"), panel_of(&app.session, "Gamma.jsx")];
    assert!(app.ui.dock.is_floating(alpha), "{:?}", app.ui.dock);
    assert!(app.ui.dock.is_docked(gamma), "{:?}", app.ui.dock);
    let (_, location) = app.ui.dock.hidden.iter().find(|(p, _)| *p == beta).unwrap_or_else(|| panic!("{:?}", app.ui.dock));
    assert_eq!(location.anchor, Some(gamma));
    assert_eq!(location.placement, Placement::Tab { before: Some(gamma) });
}

#[test]
fn a_table_naming_one_script_twice_does_not_reject_the_file() {
    let store = store_with(&[]);
    let dock = dock::docked(dock::tabs(&[PanelKind::Composition, PanelKind::ScriptPanel(900_001), PanelKind::ScriptPanel(900_002)], 0));
    store.write(WORKSPACES_FILE, &file(&dock, &[(900_001, "Same.jsx"), (900_002, "Same.jsx")])).unwrap();
    let app = EffectcraftApp::new(session_with_windows(&store, &[(900_002, "Same.jsx")]));
    assert_loaded(&app);
    assert_eq!(app.ui.dock.panels(), [PanelKind::Composition, PanelKind::ScriptPanel(900_002)]);
    assert_eq!(dead_tabs(&app), []);
}

#[test]
fn loading_or_resetting_a_workspace_runs_no_script() {
    let store = store_with(&[]);
    store.write(&script_file("Stay Shut.jsx"), "app.project.items.addComp('Ran', 64, 64, 1, 1, 25); this.add('button', undefined, 'Hi');").unwrap();
    let dock = dock::docked(dock::tabs(&[PanelKind::Composition, PanelKind::ScriptPanel(900_001)], 0));
    store.write(WORKSPACES_FILE, &file(&dock, &[(900_001, "Stay Shut.jsx")])).unwrap();
    let mut s = effectcraft_host::session();
    s.config = Some(store.clone());
    let mut app = EffectcraftApp::new(s);
    invoke(&mut app, "window.resetWorkspace");
    let h = harness(app);
    assert!(h.state().session.script_ui.windows.is_empty(), "the panel stays closed");
    assert!(h.state().session.project.items.is_empty(), "the script did not run");
}

#[test]
fn an_open_script_whose_panel_was_hidden_returns_to_its_place_at_launch() {
    let (store, _, _) = saved_layout_store();
    store.remove(&script_file("Beta.jsx")).unwrap();
    let mut h = harness(EffectcraftApp::new(restored(&store)));
    eframe::App::on_exit(h.state_mut());
    drop(h);
    store.write(&script_file("Beta.jsx"), BUTTON).unwrap();
    store.write(OPEN_PANELS_FILE, &json!({"open": ["Alpha.jsx", "Beta.jsx"]}).to_string()).unwrap();
    let h = harness(EffectcraftApp::new(restored(&store)));
    let beta = panel_of(&h.state().session, "Beta.jsx");
    assert!(h.state().ui.dock.group_of(beta).contains(&PanelKind::Timeline), "Beta is with {:?}", h.state().ui.dock.group_of(beta));
}

#[test]
fn a_script_missing_across_restarts_adds_nothing_to_the_file() {
    let (store, _, _) = saved_layout_store();
    store.remove(&script_file("Beta.jsx")).unwrap();
    let mut sizes = BTreeSet::new();
    for _ in 0..4 {
        let mut h = harness(EffectcraftApp::new(restored(&store)));
        eframe::App::on_exit(h.state_mut());
        let doc: Value = serde_json::from_str(&store.read(WORKSPACES_FILE).unwrap()).unwrap();
        let mut ids = BTreeSet::new();
        saved_ids(&doc, &mut ids);
        assert_eq!(ids.len(), 2, "{doc}");
        assert_eq!(doc["script_panels"].as_object().map(|table| table.len()), Some(2), "{doc}");
        let hidden = doc["dock"]["hidden"].as_array().unwrap();
        assert_eq!(hidden.iter().filter(|entry| entry[0].get("ScriptPanel").is_some()).count(), 1, "{doc}");
        sizes.insert(hidden.len());
    }
    assert_eq!(sizes.len(), 1, "hidden stays the same size: {sizes:?}");
}

#[test]
fn a_panel_closed_and_opened_again_in_a_session_returns_to_its_place() {
    let store = store_with(&["Alpha.jsx"]);
    let mut h = harness(EffectcraftApp::new(restored(&store)));
    let first = panel_of(&h.state().session, "Alpha.jsx");
    tab_beside(&mut h, first, PanelKind::Project);
    h.state_mut().close_panel(first);
    h.run_steps(2);
    assert!(h.state().session.script_ui.windows.is_empty());
    h.state_mut().session.execute("window.scriptPanel", json!({"name": "Alpha.jsx"})).unwrap();
    h.run_steps(3);
    let second = panel_of(&h.state().session, "Alpha.jsx");
    assert_ne!(first, second, "a new window");
    assert!(h.state().ui.dock.group_of(second).contains(&PanelKind::Project), "Alpha is with {:?}", h.state().ui.dock.group_of(second));
    assert!(!dock::referenced_panels(&h.state().ui.dock).contains(&first), "the closed window's id is gone");
}

#[test]
fn a_workspace_not_showing_now_keeps_each_script_at_its_place() {
    let (store, alpha, beta) = saved_layout_store();
    let mut doc: Value = serde_json::from_str(&store.read(WORKSPACES_FILE).unwrap()).unwrap();
    doc["saved_workspaces"]["Mine"] = doc["saved_workspaces"]["Default"].clone();
    doc["saved_workspaces"].as_object_mut().unwrap().remove("Default");
    store.write(WORKSPACES_FILE, &doc.to_string()).unwrap();
    let session = restored(&store);
    let (alpha_now, beta_now) = (panel_of(&session, "Alpha.jsx"), panel_of(&session, "Beta.jsx"));
    rewrite_saved_ids(&store, &[(alpha, beta_now), (beta, alpha_now)]);
    let mut h = harness(EffectcraftApp::new(session));
    h.state_mut().set_workspace("Mine");
    h.run_steps(2);
    let dock = &h.state().ui.dock;
    assert!(dock.group_of(alpha_now).contains(&PanelKind::Project), "Alpha is with {:?}", dock.group_of(alpha_now));
    assert!(dock.group_of(beta_now).contains(&PanelKind::Timeline), "Beta is with {:?}", dock.group_of(beta_now));
}

#[test]
fn reset_finds_a_panel_whose_window_has_not_docked_yet() {
    let store = store_with(&[]);
    let dock = dock::docked(dock::tabs(&[PanelKind::Project, PanelKind::ScriptPanel(900_001)], 0));
    store.write(WORKSPACES_FILE, &file(&dock, &[(900_001, "Alpha.jsx")])).unwrap();
    let mut app = EffectcraftApp::new(session_with_windows(&store, &[]));
    app.session.script_ui.windows.push(ScriptWindow { id: 7, kind: WindowKind::Panel, script: "Alpha.jsx".into(), visible: true, ..Default::default() });
    invoke(&mut app, "window.resetWorkspace");
    assert_eq!(app.ui.dock.group_of(PanelKind::ScriptPanel(7)), [PanelKind::Project, PanelKind::ScriptPanel(7)]);
}
