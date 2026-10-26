//! Persistent workspace layouts, separate from document-bound and transient UI state.

use std::collections::BTreeMap;

use effectcraft_engine::config::ConfigStore;
use serde::{Deserialize, Serialize};

use crate::dock::{PanelKind, Workspace};
use crate::state::UiState;

mod legacy;

const FILE: &str = "workspaces.json";
/// An earlier version's file, kept when it is first rewritten in the current format.
const BACKUP: &str = "workspaces.old.json";
const MAX_BYTES: usize = 1024 * 1024;
const MAX_WORKSPACES: usize = 64;
/// Leave room for new viewers without approaching the u32 allocator limit.
const MAX_VIEWER_ID: u32 = 65_535;
/// The snapshot version: 2 holds craft-ui docking workspaces.
const VERSION: u32 = 2;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Snapshot {
    version: u32,
    workspace: String,
    dock: Workspace,
    focused: PanelKind,
    maximized: Option<PanelKind>,
    saved_workspaces: BTreeMap<String, Workspace>,
}

impl Snapshot {
    fn capture(ui: &UiState) -> Self {
        Self {
            version: VERSION,
            workspace: ui.workspace.clone(),
            dock: ui.dock.clone(),
            focused: ui.focused,
            maximized: ui.maximized,
            saved_workspaces: ui.saved_workspaces.clone(),
        }
    }

    fn apply(self, ui: &mut UiState) {
        ui.workspace = self.workspace;
        ui.dock = self.dock;
        ui.focused = self.focused;
        ui.maximized = self.maximized;
        ui.saved_workspaces = self.saved_workspaces;
        // A native window's place is made safe to hand to the windowing system.
        for workspace in std::iter::once(&mut ui.dock).chain(ui.saved_workspaces.values_mut()) {
            for window in &mut workspace.layout.floating {
                window.native = window.native.map(crate::dock_ui::sanitized_window_rect);
            }
        }
        // Reserve restored viewer slots without restoring any project-specific composition
        // links. New Viewer allocates from this map, including after switching named layouts.
        for workspace in std::iter::once(&ui.dock).chain(ui.saved_workspaces.values()) {
            for panel in crate::dock::referenced_panels(workspace) {
                if let PanelKind::Viewer(id) = panel {
                    ui.viewers.entry(id).or_insert(None);
                }
            }
        }
    }

    fn validate(&self) -> Result<(), String> {
        if self.version != VERSION {
            return Err(format!("unsupported workspace version {}", self.version));
        }
        let name_ok = |name: &str| !name.trim().is_empty() && name.len() <= 256;
        if !name_ok(&self.workspace) || self.saved_workspaces.len() > MAX_WORKSPACES || self.saved_workspaces.keys().any(|name| !name_ok(name)) {
            return Err("workspace names or count exceed supported limits".into());
        }
        if !crate::dock::WORKSPACES.contains(&self.workspace.as_str()) && !self.saved_workspaces.contains_key(&self.workspace) {
            return Err("current workspace has no saved definition".into());
        }
        let panel_ok = |panel: PanelKind| match panel {
            PanelKind::Viewer(id) => (1..=MAX_VIEWER_ID).contains(&id),
            _ => true,
        };
        if !panel_ok(self.focused) || self.maximized.is_some_and(|panel| !panel_ok(panel)) {
            return Err("workspace viewer ID is outside the supported range 1–65535".into());
        }
        for workspace in std::iter::once(&self.dock).chain(self.saved_workspaces.values()) {
            // craft-ui checks the trees, selections, sizes, rects and limits.
            workspace.validate().map_err(|error| error.to_string())?;
            if crate::dock::referenced_panels(workspace).into_iter().any(|panel| !panel_ok(panel)) {
                return Err("workspace viewer ID is outside the supported range 1–65535".into());
            }
        }
        Ok(())
    }
}

pub(crate) struct WorkspaceStore {
    saved: Snapshot,
    last_attempt: f64,
    writable: bool,
    /// The text of an earlier version's file that loaded: the first save copies it to
    /// [`BACKUP`] and writes the file in the current format.
    migrated_from: Option<String>,
    pub(crate) error: Option<String>,
}

impl WorkspaceStore {
    pub(crate) fn load(config: Option<&dyn ConfigStore>, ui: &mut UiState) -> Self {
        let mut original = None;
        let mut outer = Vec::new();
        let loaded = config.map_or(Ok(None), |c| c.read_bounded(FILE, MAX_BYTES)).map_err(|e| e.to_string()).and_then(|text| {
            text.map(|text| {
                let value: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
                let snapshot = if legacy::is_legacy(&value) {
                    let (snapshot, windows) = legacy::migrate(value, ui)?;
                    original = Some(text);
                    outer = windows;
                    snapshot
                } else if value.get("version").is_none() {
                    return Err("not a workspace file".to_string());
                } else {
                    serde_json::from_value::<Snapshot>(value).map_err(|e| e.to_string())?
                };
                snapshot.validate()?;
                Ok(snapshot)
            })
            .transpose()
        });
        let error = match loaded {
            Ok(Some(snapshot)) => {
                snapshot.apply(ui);
                ui.native_outer = outer;
                None
            }
            Ok(None) => None,
            Err(e) => Some(format!("Could not load workspaces: {e}. The existing file will be preserved for this session.")),
        };
        if let Some(error) = &error {
            log::warn!("{error}");
            ui.status = error.clone();
            original = None;
        }
        // An unreadable or newer file is never replaced by this session, even after a later
        // layout edit. Ordinary failed writes remain dirty and retryable.
        Self { saved: Snapshot::capture(ui), last_attempt: f64::NEG_INFINITY, writable: error.is_none(), migrated_from: original, error }
    }

    pub(crate) fn due(&mut self, now: f64) -> bool {
        if now - self.last_attempt < 1.0 {
            return false;
        }
        self.last_attempt = now;
        true
    }

    pub(crate) fn has_changes(&self, ui: &UiState) -> bool {
        if !self.writable {
            return false;
        }
        self.migrated_from.is_some()
            || self.saved.workspace != ui.workspace
            || self.saved.dock != ui.dock
            || self.saved.focused != ui.focused
            || self.saved.maximized != ui.maximized
            || self.saved.saved_workspaces != ui.saved_workspaces
    }

    pub(crate) fn save(&mut self, config: Option<&dyn ConfigStore>, ui: &UiState) -> Result<bool, String> {
        let Some(config) = config else { return Ok(false) };
        // A window still at its outer position would be saved as if that were its content's.
        if !self.has_changes(ui) || !ui.native_outer.is_empty() {
            return Ok(false);
        }
        let snapshot = Snapshot::capture(ui);
        snapshot.validate()?;
        let text = serde_json::to_string_pretty(&snapshot).map_err(|e| format!("serialize workspaces: {e}"))?;
        if text.len() > MAX_BYTES {
            return Err("workspace file exceeds 1 MiB".into());
        }
        if let Some(original) = &self.migrated_from {
            config.write(BACKUP, original).map_err(|e| format!("keep the earlier workspace file: {e}"))?;
        }
        config.write(FILE, &text).map_err(|e| format!("save workspaces: {e}"))?;
        self.migrated_from = None;
        self.saved = snapshot;
        self.error = None;
        Ok(true)
    }

    /// An explicit save (Save Workspace, Save as New Workspace, Edit Workspaces): unlike
    /// [`Self::save`], no settings folder or a file blocked at load is an error, not "nothing to do".
    pub(crate) fn save_now(&mut self, config: Option<&dyn ConfigStore>, ui: &UiState) -> Result<(), String> {
        let Some(config) = config else {
            return Err("there is no settings folder to save them in".into());
        };
        if !self.writable {
            let path = config.dir().map_or_else(|| FILE.to_string(), |dir| dir.join(FILE).display().to_string());
            return Err(format!("{path} couldn't be loaded at startup, so it is left untouched; fix or move it and restart"));
        }
        self.save(Some(config), ui).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dock::{Floating, Layout, Node, StackEntry, WorkspaceExt as _};
    use craft_ui::docking::Action;
    use craft_ui::layout::{SplitAxis, SplitSize};
    use effectcraft_engine::config::MemoryConfig;

    fn minimal() -> Workspace {
        Workspace::new(crate::dock::workspace("Minimal"))
    }

    fn tabs(panels: &[PanelKind]) -> Node<PanelKind> {
        Node::Tabs { panels: panels.to_vec(), active: 0 }
    }

    #[test]
    fn corrupt_future_or_unknown_workspace_keeps_defaults_and_original_file() {
        let valid = Snapshot::capture(&UiState::default());
        // Damaged files of the earlier version: a panel in two places, a bad split, an unknown
        // panel, a tree deeper than craft-ui allows, a missing field.
        let earlier = |dock: &str| {
            format!(
                r#"{{"version":1,"workspace":"Default","dock":{dock},"floating":[],"focused":"Composition","maximized":null,"saved_workspaces":{{}},"saved_floating":{{}}}}"#
            )
        };
        let mut deep = r#"{"Tabs":{"panels":["Composition"],"active":0}}"#.to_string();
        for _ in 0..=craft_ui::docking::MAX_DEPTH {
            deep = format!(r#"{{"Split":{{"vertical":true,"size":{{"Ratio":0.5}},"a":{deep},"b":{{"Tabs":{{"panels":[],"active":0}}}}}}}}"#);
        }
        let twice =
            r#"{"Split":{"vertical":true,"size":{"Ratio":0.5},"a":{"Tabs":{"panels":["Project"],"active":0}},"b":{"Tabs":{"panels":["Project"],"active":0}}}}"#;
        let bad_split = r#"{"Split":{"vertical":true,"size":{"Ratio":7.0},"a":{"Tabs":{"panels":["Project"],"active":0}},"b":{"Tabs":{"panels":["Timeline"],"active":0}}}}"#;
        for text in [
            "{".into(),
            "{}".into(),
            "[1, 2]".into(),
            serde_json::to_string(&Snapshot { version: VERSION + 1, ..valid }).unwrap(),
            " ".repeat(MAX_BYTES + 1),
            earlier(twice),
            earlier(bad_split),
            earlier(r#"{"Tabs":{"panels":["NoSuchPanel"],"active":0}}"#),
            earlier(&deep),
            r#"{"version":1,"workspace":"Default"}"#.into(),
            r#"{"workspaces":{"Mine":{"Tabs":{"panels":["Composition"],"active":3}}}}"#.into(),
        ] {
            let config = MemoryConfig::default();
            config.write(FILE, &text).unwrap();
            let mut ui = UiState::default();
            let mut store = WorkspaceStore::load(Some(&config), &mut ui);
            assert!(store.error.is_some());
            assert_eq!(ui.workspace, "Default");
            assert_eq!(ui.dock, UiState::default().dock);
            ui.focused = PanelKind::Project;
            ui.dock = minimal();
            assert!(!store.save(Some(&config), &ui).unwrap());
            assert!(store.save_now(Some(&config), &ui).unwrap_err().contains("couldn't be loaded"));
            assert_eq!(config.read(FILE).as_deref(), Some(text.as_str()));
            assert_eq!(config.read(BACKUP), None);
        }
    }

    /// A file written by EffectCraft 0.10 (craft-libs 5c874d51): the Default workspace with
    /// Preview floating, Render Queue floating in its own native window, Align closed,
    /// Effects & Presets moved into the Project group and Audio split beside the Timeline,
    /// saved as "My Layout"; then Layer closed.
    const V1: &str = include_str!("../tests/fixtures/workspaces-v1.json");

    #[test]
    fn version_one_file_loads_with_every_panel_in_place() {
        let config = MemoryConfig::default();
        config.write(FILE, V1).unwrap();
        let mut ui = UiState::default();
        let store = WorkspaceStore::load(Some(&config), &mut ui);
        assert_eq!(store.error, None);
        assert_eq!(ui.workspace, "My Layout");
        assert_eq!(ui.focused, PanelKind::Composition);
        use PanelKind::*;
        let root = crate::dock::hsplit(
            SplitSize::FixedSecond(300.0),
            crate::dock::vsplit(
                SplitSize::Ratio(0.56),
                crate::dock::hsplit(SplitSize::FixedFirst(300.0), crate::dock::tabs(&[Project, EffectControls, EffectsPresets], 2), tabs(&[Composition])),
                crate::dock::hsplit(SplitSize::Ratio(0.5), tabs(&[Timeline]), tabs(&[Audio])),
            ),
            tabs(&[Properties]),
        );
        assert_eq!(ui.dock.layout.root, Some(root));
        let floating: Vec<_> = ui.dock.layout.floating.iter().map(|w| (w.panels().copied().collect::<Vec<_>>(), w.rect, w.native)).collect();
        assert_eq!(
            floating,
            [
                (vec![Preview], [900.0, 120.0, 300.0, 260.0], None),
                (vec![RenderQueue], [200.0, 500.0, 520.0, 240.0], Some(crate::dock_ui::sanitized_window_rect([200.0, 500.0, 520.0, 240.0]))),
            ]
        );
        // Closed panels stay closed.
        for panel in [Align, Layer] {
            assert!(!ui.dock.is_open(panel), "{panel:?}");
        }
        // The native window's place is its outer position until the main window reports its
        // frame (here a 28-point title bar); the file is not rewritten before.
        let id = ui.dock.layout.floating[1].id;
        assert_eq!(ui.native_outer, [id]);
        let mut store = store;
        assert!(!store.save(Some(&config), &ui).unwrap());
        let outer = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(1600.0, 1000.0));
        crate::dock_ui::native_outer_to_content(&mut ui, Some(outer), Some(outer.translate(egui::vec2(0.0, 28.0))), 0.1);
        assert_eq!(ui.dock.layout.floating[1].native, Some([200.0, 528.0, 520.0, 240.0]));
        assert!(ui.native_outer.is_empty());
        assert!(store.save(Some(&config), &ui).unwrap());
        // The named workspace still has Layer, closed only after it was saved.
        let saved = ui.saved_workspaces.get("My Layout").unwrap();
        assert!(saved.is_docked(Layer) && saved.is_floating(Preview) && !saved.is_open(Align));
        assert_eq!(saved.layout.floating.iter().filter_map(|w| w.native).count(), 0);
    }

    #[test]
    fn version_one_file_is_rewritten_once_in_the_current_format_and_kept_as_a_backup() {
        let config = MemoryConfig::default();
        config.write(FILE, V1).unwrap();
        let mut ui = UiState::default();
        let mut store = WorkspaceStore::load(Some(&config), &mut ui);
        // A backend that reports no window positions: the native place stays as it was.
        let native = ui.dock.layout.floating[1].native;
        crate::dock_ui::native_outer_to_content(&mut ui, None, None, 1.0);
        assert!(!ui.native_outer.is_empty(), "waits for a report");
        crate::dock_ui::native_outer_to_content(&mut ui, None, None, 5.0);
        assert!(ui.native_outer.is_empty());
        assert_eq!(ui.dock.layout.floating[1].native, native);
        let migrated = Snapshot::capture(&ui);
        assert!(store.has_changes(&ui), "the migrated layout is written without waiting for an edit");
        assert!(store.save(Some(&config), &ui).unwrap());
        assert_eq!(config.read(BACKUP).as_deref(), Some(V1));
        let text = config.read(FILE).unwrap();
        assert_eq!(serde_json::from_str::<serde_json::Value>(&text).unwrap()["version"], VERSION);
        assert!(!store.has_changes(&ui));
        let mut restored = UiState::default();
        let reloaded = WorkspaceStore::load(Some(&config), &mut restored);
        assert_eq!(reloaded.error, None);
        assert!(!reloaded.has_changes(&restored));
        assert_eq!(Snapshot::capture(&restored), migrated);
    }

    #[test]
    fn stacked_frames_with_tabs_and_an_empty_root_migrate() {
        let text = r#"{"version":1,"workspace":"Mine","dock":{"Tabs":{"panels":[],"active":0}},
            "floating":[{"panels":["Project"],"active":0,"rect":[10.0,10.0,300.0,200.0]}],
            "focused":"Project","maximized":null,
            "saved_workspaces":{"Mine":{"Stack":{"entries":[{"panel":"Info","open":true},{"panels":["Audio","Preview"],"active":1,"open":true,"height":80.0}]}},
                "Column":{"Stack":{"entries":[{"panel":"Align","open":false},{"panel":"Preview","open":true,"height":46.0},{"panels":["Properties","Paint"],"active":1,"open":false,"height":300.0},{"panel":"Info","open":true}]}}},
            "saved_floating":{"Gone":[{"panels":["Align"],"active":0,"rect":[0.0,0.0,100.0,100.0]}]}}"#;
        let config = MemoryConfig::default();
        config.write(FILE, text).unwrap();
        let mut ui = UiState::default();
        let store = WorkspaceStore::load(Some(&config), &mut ui);
        assert_eq!(store.error, None);
        assert_eq!(ui.dock.layout.root, None);
        assert!(ui.dock.is_floating(PanelKind::Project));
        use PanelKind::*;
        // Each open frame is a tab group with all its tabs; a frame's height is a fixed size
        // (with its tab strip), and the frame without one takes the rest.
        let column = crate::dock::vsplit(SplitSize::FixedSecond(80.0 + crate::dock::TAB_STRIP), tabs(&[Info]), crate::dock::tabs(&[Audio, Preview], 1));
        assert_eq!(ui.saved_workspaces.get("Mine").and_then(|w| w.layout.root.clone()), Some(column));
        // Closed frames' tabs join the open frame above them, or below at the top.
        let column = crate::dock::vsplit(SplitSize::FixedFirst(46.0 + crate::dock::TAB_STRIP), tabs(&[Preview, Properties, Paint, Align]), tabs(&[Info]));
        assert_eq!(ui.saved_workspaces.get("Column").and_then(|w| w.layout.root.clone()), Some(column));
        assert_eq!(ui.saved_workspaces.len(), 2, "floating groups of no named workspace are dropped");
    }

    #[test]
    fn explicit_save_without_a_settings_folder_is_an_error() {
        let mut ui = UiState::default();
        let mut store = WorkspaceStore::load(None, &mut ui);
        ui.focused = PanelKind::Project;
        assert!(!store.save(None, &ui).unwrap());
        assert!(store.save_now(None, &ui).is_err());
    }

    #[test]
    fn explicit_save_with_nothing_new_succeeds() {
        let config = MemoryConfig::default();
        let mut ui = UiState::default();
        let mut store = WorkspaceStore::load(Some(&config), &mut ui);
        ui.focused = PanelKind::Project;
        store.save_now(Some(&config), &ui).unwrap();
        store.save_now(Some(&config), &ui).unwrap();
        assert!(config.read(FILE).is_some());
    }

    #[test]
    fn invalid_tabs_split_and_deep_tree_are_rejected() {
        let mut snapshot = Snapshot::capture(&UiState::default());
        snapshot.dock = Workspace::new(Layout { root: Some(Node::Tabs { panels: vec![PanelKind::Project], active: usize::MAX }), floating: vec![] });
        assert!(snapshot.validate().is_err());
        let split = |size, first, second| Node::Split { axis: SplitAxis::Horizontal, size, first: Box::new(first), second: Box::new(second) };
        snapshot.dock = Workspace::new(Layout {
            root: Some(split(SplitSize::FixedFirst(f32::NAN), tabs(&[PanelKind::Project]), tabs(&[PanelKind::Timeline]))),
            floating: vec![],
        });
        assert!(snapshot.validate().is_err());
        let mut root = tabs(&[PanelKind::Composition]);
        for _ in 0..=craft_ui::docking::MAX_DEPTH {
            root = split(SplitSize::Ratio(0.5), root, tabs(&[]));
        }
        snapshot.dock = Workspace::new(Layout { root: Some(root), floating: vec![] });
        assert!(snapshot.validate().is_err());
        // A panel in two places.
        snapshot.dock = Workspace::new(Layout {
            root: Some(tabs(&[PanelKind::Project])),
            floating: vec![Floating::single(vec![PanelKind::Project], 0, [0.0, 0.0, 200.0, 100.0])],
        });
        assert!(snapshot.validate().is_err());
    }

    #[test]
    fn closing_all_docked_panels_remains_a_persistable_layout() {
        let config = MemoryConfig::default();
        let mut ui = UiState::default();
        let mut store = WorkspaceStore::load(Some(&config), &mut ui);
        for panel in ui.dock.panels() {
            ui.dock.apply(Action::Close { panel }, crate::dock::permissions).unwrap();
        }
        assert!(ui.dock.layout.root.is_none());
        assert!(store.save(Some(&config), &ui).unwrap());
        let mut restored = UiState::default();
        let loaded = WorkspaceStore::load(Some(&config), &mut restored);
        assert!(loaded.error.is_none());
        assert_eq!(restored.dock, ui.dock);
    }

    #[test]
    fn floating_groups_and_named_snapshots_roundtrip_without_document_state() {
        let config = MemoryConfig::default();
        let mut ui = UiState::default();
        let mut store = WorkspaceStore::load(Some(&config), &mut ui);
        ui.workspace = "Floating".into();
        ui.dock.apply(Action::Float { panel: PanelKind::Preview, rect: [50.0, 80.0, 200.0, 160.0] }, crate::dock::permissions).unwrap();
        ui.dock.apply(Action::Close { panel: PanelKind::Align }, crate::dock::permissions).unwrap();
        ui.saved_workspaces.insert(ui.workspace.clone(), ui.dock.clone());
        ui.maximized = Some(PanelKind::Composition);
        ui.project_search = "do not persist".into();
        assert!(store.save(Some(&config), &ui).unwrap());
        let mut restored = UiState::default();
        WorkspaceStore::load(Some(&config), &mut restored);
        assert_eq!(Snapshot::capture(&restored), Snapshot::capture(&ui));
        // The closed panel's place comes back with it.
        assert!(restored.dock.hidden.iter().any(|(p, _)| *p == PanelKind::Align));
        assert!(restored.project_search.is_empty());
    }

    #[test]
    fn restored_viewer_slots_do_not_reuse_ids_or_restore_document_links() {
        let config = MemoryConfig::default();
        let mut ui = UiState::default();
        let mut store = WorkspaceStore::load(Some(&config), &mut ui);
        let two = |viewer| Workspace::new(Layout { root: Some(tabs(&[PanelKind::Composition, PanelKind::Viewer(viewer)])), floating: vec![] });
        ui.dock = two(2);
        ui.saved_workspaces.insert("Two views".into(), two(5));
        ui.viewers.insert(2, Some(42));
        assert!(store.save(Some(&config), &ui).unwrap());
        let mut app = crate::EffectcraftApp::new(effectcraft_engine::Session::default());
        WorkspaceStore::load(Some(&config), &mut app.ui);
        assert_eq!(app.ui.viewers.get(&2), Some(&None));
        assert_eq!(app.ui.viewers.get(&5), Some(&None));
        assert_eq!(crate::panels::viewers::new_viewer(&mut app), 6);
        assert!(app.ui.dock.is_docked(PanelKind::Viewer(2)));
    }

    #[test]
    fn invalid_viewer_ids_are_rejected_in_every_workspace_reference() {
        for id in [0, MAX_VIEWER_ID + 1, u32::MAX] {
            for location in 0..8 {
                let mut snapshot = Snapshot::capture(&UiState::default());
                let panel = PanelKind::Viewer(id);
                let docked = Workspace::new(Layout { root: Some(tabs(&[panel])), floating: vec![] });
                let stacked =
                    Workspace::new(Layout { root: Some(Node::Stack { entries: vec![StackEntry { panel, open: true, height: None }] }), floating: vec![] });
                let floating = Workspace::new(Layout {
                    root: Some(tabs(&[PanelKind::Composition])),
                    floating: vec![Floating::single(vec![panel], 0, [0.0, 0.0, 200.0, 100.0])],
                });
                let mut hidden = Workspace::new(Layout { root: Some(tabs(&[PanelKind::Composition, panel])), floating: vec![] });
                hidden.apply(Action::Close { panel }, crate::dock::permissions).unwrap();
                match location {
                    0 => snapshot.focused = panel,
                    1 => snapshot.maximized = Some(panel),
                    2 => snapshot.dock = docked,
                    3 => snapshot.dock = stacked,
                    4 => snapshot.dock = floating,
                    5 => snapshot.dock = hidden,
                    6 => {
                        snapshot.saved_workspaces.insert("Saved".into(), stacked);
                    }
                    _ => {
                        snapshot.saved_workspaces.insert("Saved".into(), floating);
                    }
                }
                assert!(snapshot.validate().is_err(), "viewer {id} at location {location} must be rejected");
            }
        }
    }

    #[test]
    fn hostile_native_window_places_are_made_safe_on_load() {
        let config = MemoryConfig::default();
        let mut ui = UiState::default();
        let mut store = WorkspaceStore::load(Some(&config), &mut ui);
        ui.dock.apply(Action::Float { panel: PanelKind::Project, rect: [10.0, 10.0, 300.0, 200.0] }, crate::dock::permissions).unwrap();
        ui.dock.apply(Action::SetNative { panel: PanelKind::Project, rect: Some([90_000.0, -90_000.0, 1.0e5, 0.5]) }, crate::dock::permissions).unwrap();
        assert!(store.save(Some(&config), &ui).unwrap());
        let mut restored = UiState::default();
        WorkspaceStore::load(Some(&config), &mut restored);
        let native = restored.dock.layout.floating.iter().find(|w| w.contains(&PanelKind::Project)).and_then(|w| w.native).unwrap();
        assert_eq!(native, crate::dock_ui::sanitized_window_rect(native));
        assert!(native[2] <= crate::dock_ui::PANEL_WINDOW_MAX && native[3] >= crate::dock_ui::PANEL_WINDOW_MIN[1]);
    }

    #[test]
    fn failed_write_keeps_previous_snapshot_and_retries() {
        struct Fails<'a>(&'a MemoryConfig);
        impl ConfigStore for Fails<'_> {
            fn read(&self, name: &str) -> Option<String> {
                self.0.read(name)
            }
            fn write(&self, _name: &str, _data: &str) -> std::io::Result<()> {
                Err(std::io::Error::other("synthetic full disk"))
            }
            fn remove(&self, _name: &str) -> std::io::Result<()> {
                Err(std::io::Error::other("unused"))
            }
        }
        let config = MemoryConfig::default();
        let mut ui = UiState::default();
        let mut store = WorkspaceStore::load(Some(&config), &mut ui);
        ui.focused = PanelKind::Project;
        assert!(store.save(Some(&config), &ui).unwrap());
        let previous = config.read(FILE).unwrap();
        ui.focused = PanelKind::Timeline;
        assert!(store.save(Some(&Fails(&config)), &ui).unwrap_err().contains("synthetic full disk"));
        assert_eq!(config.read(FILE).as_deref(), Some(previous.as_str()));
        assert!(store.has_changes(&ui));
        assert!(store.save(Some(&config), &ui).unwrap());
        let mut restored = UiState::default();
        WorkspaceStore::load(Some(&config), &mut restored);
        assert_eq!(restored.focused, PanelKind::Timeline);
    }
}
