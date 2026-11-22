use craft_ui::docking::{Column, Floating, Group, Location, Placement, Side, StackEntry};
use craft_ui::layout::SplitSize;
use effectcraft_engine::config::{ConfigStore, MemoryConfig};
use effectcraft_engine::scriptui::ScriptWindow;

use super::super::{FILE, WorkspaceStore};
use super::*;
use crate::dock::{Layout, Node, docked, hsplit, tabs, vsplit};

fn script(id: u32) -> PanelKind {
    PanelKind::ScriptPanel(id)
}

fn windows(list: &[(u32, WindowKind, &str)]) -> ScriptUi {
    let windows = list.iter().map(|(id, kind, script)| ScriptWindow { id: *id, kind: *kind, script: script.to_string(), ..Default::default() }).collect();
    ScriptUi { windows, ..Default::default() }
}

fn ui_with(dock: Workspace, names: &[(u32, &str)]) -> UiState {
    let mut ui = UiState { dock, ..Default::default() };
    ui.script_panels = names.iter().map(|(id, name)| (*id, name.to_string())).collect();
    ui
}

fn place(anchor: PanelKind, before: Option<PanelKind>, sibling: Vec<PanelKind>) -> Location<PanelKind> {
    Location { floating: None, anchor: Some(anchor), placement: Placement::Tab { before }, size: None, stack: None, sibling, group: None }
}

/// One reference in each place a workspace can hold a panel, numbered from `base`.
fn every_place(base: u32) -> Workspace {
    let side = Node::Dock { side: Side::Start, columns: vec![Column::new(vec![Group::new(vec![script(base + 1)], 0)], 240.0)] };
    let stack = Node::Stack { entries: vec![StackEntry { panel: script(base + 2), open: true, height: None }] };
    let shown = tabs(&[script(base + 3), PanelKind::Viewer(3)], 1);
    let floating = vec![Floating::single(vec![script(base + 4)], 0, [10.0, 20.0, 300.0, 200.0])];
    let mut workspace = Workspace::new(Layout { root: Some(hsplit(SplitSize::Ratio(0.2), side, vsplit(SplitSize::Ratio(0.5), stack, shown))), floating });
    workspace.hidden.push((script(base + 5), place(script(base + 6), Some(script(base + 7)), vec![script(base + 8)])));
    workspace.saved = Some(Layout { root: Some(tabs(&[script(base + 9)], 0)), floating: Vec::new() });
    workspace
}

#[test]
fn every_place_a_workspace_holds_a_panel_is_rewritten() {
    let mut workspace = every_place(0);
    workspace.validate().unwrap();
    remap(&mut workspace, &mut |id| id + 100).unwrap();
    assert_eq!(workspace, every_place(100));
}

#[test]
fn a_rewrite_that_fails_changes_nothing() {
    let mut workspace = every_place(0);
    let before = workspace.clone();
    assert!(remap(&mut workspace, &mut |_| 1).is_err(), "one window in two places");
    assert_eq!(workspace, before);
}

#[test]
fn the_deepest_valid_tree_is_rewritten_and_a_deeper_value_is_an_error() {
    let mut root = tabs(&[script(1)], 0);
    for i in 1..craft_ui::docking::MAX_DEPTH {
        root = hsplit(SplitSize::Ratio(0.5), tabs(&[PanelKind::Viewer(i as u32)], 0), root);
    }
    let mut workspace = Workspace::new(Layout { root: Some(root), floating: Vec::new() });
    workspace.validate().unwrap();
    remap(&mut workspace, &mut |id| id + 1).unwrap();
    assert!(workspace.is_docked(script(2)));

    let mut deep = Value::Null;
    for _ in 0..=MAX_JSON_DEPTH {
        deep = Value::Array(vec![deep]);
    }
    assert!(remap_value(&mut deep, &mut |id| id, 0).is_err());
}

#[test]
fn binding_twice_changes_nothing_and_keeps_one_id_per_script() {
    let dock = docked(tabs(&[PanelKind::Composition, script(DETACHED), script(DETACHED + 1)], 0));
    let mut ui = ui_with(dock, &[(DETACHED, "A.jsx"), (DETACHED + 1, "B.jsx")]);
    ui.focused = script(DETACHED);
    ui.maximized = Some(script(DETACHED));
    let open = windows(&[(7, WindowKind::Panel, "A.jsx")]);
    bind(&mut ui, &open);
    assert_eq!(ui.dock.panels(), [PanelKind::Composition, script(7), script(DETACHED + 1)]);
    assert_eq!((ui.focused, ui.maximized), (script(7), Some(script(7))));
    let once = (ui.dock.clone(), ui.script_panels.clone());
    bind(&mut ui, &open);
    assert_eq!((ui.dock.clone(), ui.script_panels.clone()), once);
    assert_eq!(ui.script_panels, BTreeMap::from([(7, "A.jsx".to_string()), (DETACHED + 1, "B.jsx".to_string())]));
}

#[test]
fn binding_reaches_the_stored_workspaces_and_only_panel_windows() {
    let mut ui = ui_with(docked(tabs(&[PanelKind::Composition], 0)), &[(DETACHED, "A.jsx")]);
    ui.saved_workspaces.insert("Mine".into(), docked(tabs(&[script(DETACHED)], 0)));
    bind(&mut ui, &windows(&[(7, WindowKind::Palette, "A.jsx")]));
    assert_eq!(ui.saved_workspaces["Mine"].panels(), [script(DETACHED)], "a palette of the same script is no panel");
    bind(&mut ui, &windows(&[(7, WindowKind::Panel, "A.jsx"), (8, WindowKind::Panel, "A.jsx")]));
    assert_eq!(ui.saved_workspaces["Mine"].panels(), [script(8)], "the last window, which window.scriptPanel docks");
}

#[test]
fn a_rewrite_onto_a_shown_window_leaves_that_workspace_valid() {
    let open = windows(&[(7, WindowKind::Panel, "A.jsx")]);
    let mut ui = ui_with(docked(tabs(&[script(DETACHED), script(7)], 0)), &[(DETACHED, "A.jsx")]);
    bind(&mut ui, &open);
    ui.dock.validate().unwrap();
    fit(&mut ui, &open);
    assert_eq!(ui.dock.panels(), [script(7)]);
    assert_eq!(ui.script_panels, BTreeMap::from([(7, "A.jsx".to_string())]));
}

#[test]
fn fitting_hides_a_closed_scripts_panel_and_keeps_its_stored_place() {
    let stored = docked(tabs(&[PanelKind::Project, script(DETACHED)], 1));
    let mut ui = ui_with(stored.clone(), &[(DETACHED, "A.jsx")]);
    ui.saved_workspaces.insert("Default".into(), stored.clone());
    ui.focused = script(DETACHED);
    ui.maximized = Some(script(DETACHED));
    fit(&mut ui, &windows(&[]));
    assert_eq!(ui.dock.panels(), [PanelKind::Project]);
    assert_eq!(ui.dock.hidden.iter().map(|(panel, _)| *panel).collect::<Vec<_>>(), [script(DETACHED)]);
    assert_eq!((ui.focused, ui.maximized), (UiState::default().focused, None));
    assert_eq!(ui.saved_workspaces["Default"], stored);
}

#[test]
fn the_file_names_what_the_workspaces_still_refer_to() {
    let mut dock = docked(tabs(&[PanelKind::Composition], 0));
    dock.hidden.push((script(1), place(script(2), None, Vec::new())));
    let ui = ui_with(dock, &[(1, "Hidden.jsx"), (2, "Anchor.jsx"), (3, "Gone.jsx")]);
    assert_eq!(captured(&ui), BTreeMap::from([(1, "Hidden.jsx".to_string()), (2, "Anchor.jsx".to_string())]));
}

fn snapshot_of(dock: Workspace, names: &[(u32, &str)]) -> Snapshot {
    let mut snapshot = Snapshot::capture(&UiState::default());
    snapshot.dock = dock;
    snapshot.script_panels = names.iter().map(|(id, name)| (*id, name.to_string())).collect();
    snapshot
}

#[test]
fn detaching_moves_every_id_even_one_already_in_the_detached_range() {
    let dock = docked(tabs(&[PanelKind::Composition, script(DETACHED), script(DETACHED + 1), script(5)], 0));
    let mut snapshot = snapshot_of(dock, &[(DETACHED + 1, "A.jsx"), (5, "B.jsx")]);
    detach(&mut snapshot).unwrap();
    snapshot.dock.validate().unwrap();
    assert_eq!(snapshot.script_panels, BTreeMap::from([(DETACHED, "B.jsx".to_string()), (DETACHED + 1, "A.jsx".to_string())]));
    assert_eq!(snapshot.dock.panels(), [PanelKind::Composition, script(DETACHED + 1), script(DETACHED)], "the file's unnamed id is dropped");
}

#[test]
fn a_dropped_id_left_in_a_place_never_names_a_window() {
    let mut dock = docked(tabs(&[PanelKind::Composition, script(5)], 0));
    dock.hidden.push((PanelKind::Align, place(script(5), Some(script(5)), Vec::new())));
    let mut snapshot = snapshot_of(dock, &[]);
    detach(&mut snapshot).unwrap();
    let (_, location) = &snapshot.dock.hidden[0];
    assert!(matches!(location.anchor, Some(PanelKind::ScriptPanel(id)) if id >= DETACHED), "{location:?}");
    assert_eq!(snapshot.dock.panels(), [PanelKind::Composition]);
}

#[test]
fn an_unnamed_focus_or_maximized_panel_goes_back_to_the_default() {
    let mut snapshot = snapshot_of(docked(tabs(&[PanelKind::Project, script(5)], 0)), &[]);
    snapshot.focused = script(5);
    snapshot.maximized = Some(script(5));
    detach(&mut snapshot).unwrap();
    assert_eq!((snapshot.focused, snapshot.maximized), (UiState::default().focused, None));
}

#[test]
fn a_version_one_file_loads_without_its_script_panels() {
    let text = r#"{"version":1,"workspace":"Default","dock":{"Tabs":{"panels":["Composition",{"ScriptPanel":3}],"active":1}},
        "floating":[{"panels":[{"ScriptPanel":4}],"active":0,"rect":[10.0,10.0,300.0,200.0]}],
        "focused":{"ScriptPanel":3},"maximized":null,"saved_workspaces":{},"saved_floating":{}}"#;
    let config = MemoryConfig::default();
    config.write(FILE, text).unwrap();
    let mut ui = UiState::default();
    let store = WorkspaceStore::load(Some(&config), &mut ui);
    assert_eq!(store.error, None);
    assert_eq!(ui.dock.panels(), [PanelKind::Composition]);
    assert!(ui.dock.layout.floating.is_empty());
    assert_eq!(ui.focused, UiState::default().focused);
}

#[test]
fn a_name_learned_for_a_shown_panel_reaches_the_file() {
    let config = MemoryConfig::default();
    let mut ui = UiState::default();
    let mut store = WorkspaceStore::load(Some(&config), &mut ui);
    ui.dock = docked(tabs(&[PanelKind::Composition, script(7)], 0));
    assert!(store.save(Some(&config), &ui).unwrap());
    assert!(!store.has_changes(&ui));
    bind(&mut ui, &windows(&[(7, WindowKind::Panel, "A.jsx")]));
    assert!(store.has_changes(&ui), "the dock is the same but the file doesn't name the panel");
    assert!(store.save(Some(&config), &ui).unwrap());
    assert!(!store.has_changes(&ui));
    let saved: Value = serde_json::from_str(&config.read(FILE).unwrap()).unwrap();
    assert_eq!(saved["script_panels"], serde_json::json!({"7": "A.jsx"}));
}

#[test]
fn a_file_without_script_panels_writes_no_table() {
    let text = serde_json::to_string(&Snapshot::capture(&UiState::default())).unwrap();
    assert!(!text.contains("script_panels"), "{text}");
}

#[test]
fn a_name_repeated_in_the_table_keeps_only_its_first_id() {
    let dock = docked(tabs(&[PanelKind::Composition, script(5), script(6)], 0));
    let mut snapshot = snapshot_of(dock, &[(5, "A.jsx"), (6, "A.jsx")]);
    detach(&mut snapshot).unwrap();
    assert_eq!(snapshot.script_panels, BTreeMap::from([(DETACHED, "A.jsx".to_string())]));
    assert_eq!(snapshot.dock.panels(), [PanelKind::Composition, script(DETACHED)]);
    assert!(snapshot.dock.hidden.is_empty(), "the second id leaves no reference to bind");
}

fn file_with_table(table: Value) -> String {
    let dock = docked(tabs(&[PanelKind::Composition, script(5), script(6)], 0));
    let mut doc = serde_json::to_value(snapshot_of(dock, &[])).unwrap();
    doc["script_panels"] = table;
    doc.to_string()
}

#[test]
fn a_table_entry_that_is_not_an_id_and_a_name_is_an_id_without_a_name() {
    let config = MemoryConfig::default();
    config.write(FILE, &file_with_table(serde_json::json!({"not a number": "B.jsx", "5": "A.jsx", "6": 7}))).unwrap();
    let mut ui = UiState::default();
    let store = WorkspaceStore::load(Some(&config), &mut ui);
    assert_eq!(store.error, None);
    assert_eq!(ui.script_panels, BTreeMap::from([(DETACHED, "A.jsx".to_string())]));
    assert_eq!(ui.dock.panels(), [PanelKind::Composition, script(DETACHED)]);
}

#[test]
fn a_table_of_the_wrong_shape_names_nothing() {
    let config = MemoryConfig::default();
    config.write(FILE, &file_with_table(serde_json::json!(["A.jsx"]))).unwrap();
    let mut ui = UiState::default();
    let store = WorkspaceStore::load(Some(&config), &mut ui);
    assert_eq!(store.error, None);
    assert_eq!(ui.dock.panels(), [PanelKind::Composition]);
}

#[test]
fn a_place_that_is_not_finite_is_reported_and_the_file_kept() {
    let mut dock = docked(tabs(&[PanelKind::Composition, script(5)], 0));
    dock.hidden.push((PanelKind::Align, place(script(5), None, Vec::new())));
    let mut doc = serde_json::to_value(snapshot_of(dock, &[(5, "A.jsx")])).unwrap();
    doc["dock"]["hidden"][0][1]["floating"] = serde_json::json!([0.0, 0.0, 10.0, 10.0]);
    let text = doc.to_string().replacen("[0.0,0.0,10.0,10.0]", "[1e300,0.0,10.0,10.0]", 1);
    assert!(text.contains("1e300"), "{text}");
    let config = MemoryConfig::default();
    config.write(FILE, &text).unwrap();
    let mut ui = UiState::default();
    let store = WorkspaceStore::load(Some(&config), &mut ui);
    assert!(store.error.is_some(), "{text}");
    assert_eq!(config.read(FILE).as_deref(), Some(text.as_str()));
}

/// Workspaces where a panel can't go back to its gone anchor or to a new frame at the edge.
fn no_room_for_an_edge_frame() -> Vec<Workspace> {
    let side = Node::Dock { side: Side::Start, columns: vec![Column::new(vec![Group::new(vec![PanelKind::Project], 0)], 240.0)] };
    let mut deep = tabs(&[PanelKind::Project], 0);
    for i in 1..craft_ui::docking::MAX_DEPTH {
        deep = hsplit(SplitSize::Ratio(0.5), tabs(&[PanelKind::Viewer(i as u32)], 0), deep);
    }
    let empty_dock = Node::Dock { side: Side::Start, columns: Vec::new() };
    let rooted = |root| Workspace::new(Layout { root: Some(root), floating: Vec::new() });
    vec![rooted(side), rooted(deep), rooted(empty_dock), Workspace::default()]
}

#[test]
fn an_open_panel_whose_anchor_is_gone_still_gets_a_tab() {
    for (case, mut dock) in no_room_for_an_edge_frame().into_iter().enumerate() {
        for locked in [false, true] {
            dock.locked = locked;
            dock.hidden = vec![(script(DETACHED), place(script(DETACHED + 1), None, Vec::new()))];
            let shown = dock.panels().contains(&PanelKind::Project).then_some(PanelKind::Project);
            let mut ui = ui_with(dock.clone(), &[(DETACHED, "A.jsx")]);
            let open = windows(&[(7, WindowKind::Panel, "A.jsx")]);
            bind(&mut ui, &open);
            fit(&mut ui, &open);
            assert!(ui.dock.is_open(script(7)), "case {case}, locked {locked}: {:?}", ui.dock);
            assert!(ui.dock.hidden.is_empty(), "case {case}, locked {locked}");
            ui.dock.validate().unwrap();
            if let Some(panel) = shown {
                assert!(ui.dock.is_visible(panel), "case {case}, locked {locked}: the selected tab stays selected");
            }
        }
    }
}

#[test]
fn a_panel_opened_past_a_gone_anchor_at_launch_leaves_no_queued_event() {
    for mut dock in no_room_for_an_edge_frame() {
        dock.hidden = vec![(script(DETACHED), place(script(DETACHED + 1), None, Vec::new()))];
        let mut ui = ui_with(dock, &[(DETACHED, "A.jsx")]);
        ui.focused = PanelKind::Project;
        let mut session = Session { script_ui: windows(&[(7, WindowKind::Panel, "A.jsx")]), ..Default::default() };
        session.events.push(effectcraft_engine::Event::Frontend { command: "window.scriptPanel".into(), params: serde_json::json!({"window": 7}) });
        restore(&mut ui, &mut session);
        assert!(session.events.is_empty(), "{:?}", ui.dock);
        assert_eq!(ui.focused, PanelKind::Project);
    }
}

#[test]
fn restoring_drops_only_the_open_events_of_panels_already_placed() {
    use effectcraft_engine::Event;
    let open = |window: Value| Event::Frontend { command: "window.scriptPanel".into(), params: serde_json::json!({"window": window}) };
    let mut ui = ui_with(docked(tabs(&[PanelKind::Composition, script(DETACHED)], 0)), &[(DETACHED, "A.jsx")]);
    let mut session = Session { script_ui: windows(&[(7, WindowKind::Panel, "A.jsx"), (8, WindowKind::Panel, "B.jsx")]), ..Default::default() };
    let zoom = Event::Frontend { command: "view.zoomIn".into(), params: serde_json::json!({"window": 7}) };
    session.events = vec![open(7.into()), open(8.into()), open(Value::Null), zoom.clone(), Event::PurgeCaches];
    restore(&mut ui, &mut session);
    assert!(ui.dock.is_open(script(7)), "{:?}", ui.dock);
    assert_eq!(session.events, [open(8.into()), open(Value::Null), zoom, Event::PurgeCaches]);
}
