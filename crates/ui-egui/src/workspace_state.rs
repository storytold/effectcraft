//! Persistent workspace layouts, separate from document-bound and transient UI state.

use std::collections::BTreeMap;

use effectcraft_engine::config::ConfigStore;
use serde::{Deserialize, Serialize};

use crate::dock::{DockNode, Floating, PanelKind, PanelWindow, SplitSize};
use crate::state::UiState;

const FILE: &str = "workspaces.json";
const MAX_BYTES: usize = 1024 * 1024;
const MAX_WORKSPACES: usize = 64;
const MAX_NODES: usize = 4096;
const MAX_DEPTH: usize = 32;
const MAX_PANELS: usize = 128;
/// Leave room for new viewers without approaching the u32 allocator limit.
const MAX_VIEWER_ID: u32 = 65_535;
const MAX_COORD: f32 = 1_000_000.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Snapshot {
    version: u32,
    workspace: String,
    dock: DockNode,
    floating: Vec<Floating>,
    focused: PanelKind,
    maximized: Option<PanelKind>,
    saved_workspaces: BTreeMap<String, DockNode>,
    saved_floating: BTreeMap<String, Vec<Floating>>,
    /// Floating groups in their own native window (absent from files of earlier versions).
    #[serde(default)]
    panel_windows: Vec<PanelWindow>,
}

impl Snapshot {
    fn capture(ui: &UiState) -> Self {
        Self {
            version: 1,
            workspace: ui.workspace.clone(),
            dock: ui.dock.clone(),
            floating: ui.floating.clone(),
            focused: ui.focused,
            maximized: ui.maximized,
            saved_workspaces: ui.saved_workspaces.clone(),
            saved_floating: ui.saved_floating.clone(),
            panel_windows: ui.panel_windows.clone(),
        }
    }

    fn apply(self, ui: &mut UiState) {
        ui.workspace = self.workspace;
        ui.dock = self.dock;
        ui.floating = self.floating;
        ui.focused = self.focused;
        ui.maximized = self.maximized;
        ui.saved_workspaces = self.saved_workspaces;
        ui.saved_floating = self.saved_floating;
        // A window is only kept for a panel that still floats, and only where a window can be.
        let floating = |panel: PanelKind| ui.floating.iter().any(|group| group.panels.contains(&panel));
        ui.panel_windows = self
            .panel_windows
            .into_iter()
            .filter(|window| floating(window.panel))
            .map(|window| PanelWindow { panel: window.panel, rect: PanelWindow::sanitized(window.rect) })
            .collect();
        // Reserve restored viewer slots without restoring any project-specific composition
        // links. New Viewer allocates from this map, including after switching named layouts.
        let mut panels = Vec::new();
        for root in std::iter::once(&ui.dock).chain(ui.saved_workspaces.values()) {
            root.panels(&mut panels);
        }
        for groups in std::iter::once(&ui.floating).chain(ui.saved_floating.values()) {
            for group in groups {
                panels.extend(group.panels.iter().copied());
            }
        }
        for panel in panels {
            if let PanelKind::Viewer(id) = panel {
                ui.viewers.entry(id).or_insert(None);
            }
        }
    }

    fn validate(&self) -> Result<(), String> {
        if self.version != 1 {
            return Err(format!("unsupported workspace version {}", self.version));
        }
        let name_ok = |name: &str| !name.trim().is_empty() && name.len() <= 256;
        if !name_ok(&self.workspace)
            || self.saved_workspaces.len() > MAX_WORKSPACES
            || self.saved_floating.len() > MAX_WORKSPACES
            || self.saved_workspaces.keys().chain(self.saved_floating.keys()).any(|name| !name_ok(name))
        {
            return Err("workspace names or count exceed supported limits".into());
        }
        if !crate::dock::WORKSPACES.contains(&self.workspace.as_str()) && !self.saved_workspaces.contains_key(&self.workspace) {
            return Err("current workspace has no saved definition".into());
        }
        let panel_ok = |panel: PanelKind| match panel {
            PanelKind::Viewer(id) => (1..=MAX_VIEWER_ID).contains(&id),
            _ => true,
        };
        if !panel_ok(self.focused) || self.maximized.is_some_and(|panel| !panel_ok(panel)) || self.panel_windows.iter().any(|w| !panel_ok(w.panel)) {
            return Err("workspace viewer ID is outside the supported range 1–65535".into());
        }
        let mut nodes = 0usize;
        for root in std::iter::once(&self.dock).chain(self.saved_workspaces.values()) {
            let mut pending = vec![(root, 0usize)];
            while let Some((node, depth)) = pending.pop() {
                nodes = nodes.saturating_add(1);
                if depth > MAX_DEPTH || nodes > MAX_NODES {
                    return Err("workspace dock tree exceeds supported limits".into());
                }
                match node {
                    DockNode::Split { size, a, b, .. } => {
                        let valid = match *size {
                            SplitSize::Ratio(v) => v.is_finite() && (0.0..=1.0).contains(&v),
                            SplitSize::FixedA(v) | SplitSize::FixedB(v) => v.is_finite() && (0.0..=MAX_COORD).contains(&v),
                        };
                        if !valid {
                            return Err("invalid workspace split size".into());
                        }
                        pending.push((a, depth.saturating_add(1)));
                        pending.push((b, depth.saturating_add(1)));
                    }
                    DockNode::Tabs { panels, active } => {
                        // Closing the last docked panel leaves an empty group with active=0.
                        if panels.len() > MAX_PANELS || *active >= panels.len().max(1) || panels.iter().any(|panel| !panel_ok(*panel)) {
                            return Err("invalid workspace tab group".into());
                        }
                    }
                    DockNode::Stack { entries } => {
                        if entries.len() > MAX_PANELS
                            || entries.iter().any(|e| {
                                e.panels.is_empty()
                                    || e.panels.len() > MAX_PANELS
                                    || e.active >= e.panels.len()
                                    || e.panels.iter().any(|p| !panel_ok(*p))
                                    || e.height.is_some_and(|v| !v.is_finite() || !(0.0..=MAX_COORD).contains(&v))
                            })
                        {
                            return Err("invalid workspace panel stack".into());
                        }
                    }
                }
            }
        }
        if self.panel_windows.len() > MAX_PANELS || self.panel_windows.iter().any(|w| !w.rect.iter().all(|v| v.is_finite())) {
            return Err("invalid workspace panel windows".into());
        }
        for groups in std::iter::once(&self.floating).chain(self.saved_floating.values()) {
            if groups.len() > MAX_PANELS {
                return Err("too many floating workspace groups".into());
            }
            for group in groups {
                let [x, y, width, height] = group.rect;
                if group.panels.is_empty()
                    || group.panels.len() > MAX_PANELS
                    || group.panels.iter().any(|panel| !panel_ok(*panel))
                    || group.active >= group.panels.len()
                    || !group.rect.iter().all(|v| v.is_finite())
                    || x.abs() > MAX_COORD
                    || y.abs() > MAX_COORD
                    || !(1.0..=MAX_COORD).contains(&width)
                    || !(1.0..=MAX_COORD).contains(&height)
                {
                    return Err("invalid floating workspace group".into());
                }
            }
        }
        crate::dock_shared::encode(&self.dock, &self.floating)?.validate().map_err(|error| error.to_string())?;
        for (name, root) in &self.saved_workspaces {
            let floating = self.saved_floating.get(name).map(Vec::as_slice).unwrap_or(&[]);
            crate::dock_shared::encode(root, floating)?.validate().map_err(|error| error.to_string())?;
        }
        Ok(())
    }
}

pub(crate) struct WorkspaceStore {
    saved: Snapshot,
    last_attempt: f64,
    writable: bool,
    pub(crate) error: Option<String>,
}

impl WorkspaceStore {
    pub(crate) fn load(config: Option<&dyn ConfigStore>, ui: &mut UiState) -> Self {
        let loaded = config.map_or(Ok(None), |c| c.read_bounded(FILE, MAX_BYTES)).map_err(|e| e.to_string()).and_then(|text| {
            text.map(|text| {
                let value: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
                // Only an explicitly legacy-shaped record can migrate. A versioned future
                // snapshot must never deserialize as an empty defaulted legacy record.
                let snapshot = if value.get("version").is_none()
                    && value.as_object().is_some_and(|fields| fields.contains_key("workspaces") || fields.contains_key("floating"))
                {
                    let legacy: crate::SavedWorkspaces = serde_json::from_value(value).map_err(|e| e.to_string())?;
                    let mut snapshot = Snapshot::capture(ui);
                    snapshot.saved_workspaces = legacy.workspaces;
                    snapshot.saved_floating = legacy.floating;
                    snapshot
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
                None
            }
            Ok(None) => None,
            Err(e) => Some(format!("Could not load workspaces: {e}. The existing file will be preserved for this session.")),
        };
        if let Some(error) = &error {
            log::warn!("{error}");
            ui.status = error.clone();
        }
        // An unreadable or newer file is never replaced by this session, even after a later
        // layout edit. Ordinary failed writes remain dirty and retryable.
        Self { saved: Snapshot::capture(ui), last_attempt: f64::NEG_INFINITY, writable: error.is_none(), error }
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
        self.saved.workspace != ui.workspace
            || self.saved.dock != ui.dock
            || self.saved.floating != ui.floating
            || self.saved.focused != ui.focused
            || self.saved.maximized != ui.maximized
            || self.saved.saved_workspaces != ui.saved_workspaces
            || self.saved.saved_floating != ui.saved_floating
            || self.saved.panel_windows != ui.panel_windows
    }

    pub(crate) fn save(&mut self, config: Option<&dyn ConfigStore>, ui: &UiState) -> Result<bool, String> {
        let Some(config) = config else { return Ok(false) };
        if !self.has_changes(ui) {
            return Ok(false);
        }
        let snapshot = Snapshot::capture(ui);
        snapshot.validate()?;
        let text = serde_json::to_string_pretty(&snapshot).map_err(|e| format!("serialize workspaces: {e}"))?;
        if text.len() > MAX_BYTES {
            return Err("workspace file exceeds 1 MiB".into());
        }
        config.write(FILE, &text).map_err(|e| format!("save workspaces: {e}"))?;
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
    use effectcraft_engine::config::MemoryConfig;

    #[test]
    fn corrupt_or_future_workspace_keeps_defaults_and_original_file() {
        let valid = Snapshot::capture(&UiState::default());
        for text in ["{".into(), serde_json::to_string(&Snapshot { version: 2, ..valid }).unwrap(), " ".repeat(MAX_BYTES + 1)] {
            let config = MemoryConfig::default();
            config.write(FILE, &text).unwrap();
            let mut ui = UiState::default();
            let mut store = WorkspaceStore::load(Some(&config), &mut ui);
            assert!(store.error.is_some());
            assert_eq!(ui.workspace, "Default");
            ui.focused = PanelKind::Project;
            ui.dock = crate::dock::workspace("Minimal");
            assert!(!store.save(Some(&config), &ui).unwrap());
            assert!(store.save_now(Some(&config), &ui).unwrap_err().contains("couldn't be loaded"));
            assert_eq!(config.read(FILE).as_deref(), Some(text.as_str()));
        }
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
        snapshot.dock = DockNode::Tabs { panels: vec![], active: usize::MAX };
        assert!(snapshot.validate().is_err());
        snapshot.dock = DockNode::Split {
            vertical: false,
            size: SplitSize::FixedA(f32::NAN),
            a: Box::new(crate::dock::workspace("Minimal")),
            b: Box::new(crate::dock::workspace("Minimal")),
        };
        assert!(snapshot.validate().is_err());
        snapshot.dock = crate::dock::workspace("Minimal");
        for _ in 0..=MAX_DEPTH {
            snapshot.dock = DockNode::Split {
                vertical: false,
                size: SplitSize::Ratio(0.5),
                a: Box::new(snapshot.dock),
                b: Box::new(DockNode::Tabs { panels: vec![PanelKind::Project], active: 0 }),
            };
        }
        assert!(snapshot.validate().is_err());
    }

    #[test]
    fn closing_all_docked_panels_remains_a_persistable_layout() {
        let config = MemoryConfig::default();
        let mut ui = UiState::default();
        let mut store = WorkspaceStore::load(Some(&config), &mut ui);
        let mut panels = Vec::new();
        ui.dock.panels(&mut panels);
        for panel in panels {
            ui.dock.close(panel);
        }
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
        ui.dock.close(PanelKind::Preview);
        ui.floating = vec![Floating { panels: vec![PanelKind::Preview], active: 0, rect: [50.0, 80.0, 200.0, 160.0] }];
        ui.saved_workspaces.insert(ui.workspace.clone(), ui.dock.clone());
        ui.saved_floating.insert(ui.workspace.clone(), ui.floating.clone());
        ui.maximized = Some(PanelKind::Composition);
        ui.project_search = "do not persist".into();
        assert!(store.save(Some(&config), &ui).unwrap());
        let mut restored = UiState::default();
        WorkspaceStore::load(Some(&config), &mut restored);
        assert_eq!(Snapshot::capture(&restored), Snapshot::capture(&ui));
        assert!(restored.project_search.is_empty());
    }

    #[test]
    fn restored_viewer_slots_do_not_reuse_ids_or_restore_document_links() {
        let config = MemoryConfig::default();
        let mut ui = UiState::default();
        let mut store = WorkspaceStore::load(Some(&config), &mut ui);
        ui.dock = DockNode::Tabs { panels: vec![PanelKind::Composition, PanelKind::Viewer(2)], active: 0 };
        ui.saved_workspaces.insert("Two views".into(), DockNode::Tabs { panels: vec![PanelKind::Composition, PanelKind::Viewer(5)], active: 0 });
        ui.viewers.insert(2, Some(42));
        assert!(store.save(Some(&config), &ui).unwrap());
        let mut app = crate::EffectcraftApp::new(effectcraft_engine::Session::default());
        WorkspaceStore::load(Some(&config), &mut app.ui);
        assert_eq!(app.ui.viewers.get(&2), Some(&None));
        assert_eq!(app.ui.viewers.get(&5), Some(&None));
        assert_eq!(crate::panels::viewers::new_viewer(&mut app), 6);
        assert!(app.ui.dock.contains(PanelKind::Viewer(2)));
    }

    #[test]
    fn invalid_viewer_ids_are_rejected_in_every_workspace_reference() {
        for id in [0, MAX_VIEWER_ID + 1, u32::MAX] {
            for location in 0..8 {
                let mut snapshot = Snapshot::capture(&UiState::default());
                let panel = PanelKind::Viewer(id);
                let dock = DockNode::Tabs { panels: vec![panel], active: 0 };
                let stack = DockNode::Stack { entries: vec![crate::dock::StackEntry::new(panel, true, None)] };
                let floating = vec![Floating { panels: vec![panel], active: 0, rect: [0.0, 0.0, 200.0, 100.0] }];
                match location {
                    0 => snapshot.focused = panel,
                    1 => snapshot.maximized = Some(panel),
                    2 => snapshot.dock = dock,
                    3 => snapshot.dock = stack,
                    4 => snapshot.floating = floating,
                    5 => {
                        snapshot.saved_workspaces.insert("Saved".into(), dock);
                    }
                    6 => {
                        snapshot.saved_workspaces.insert("Saved".into(), stack);
                    }
                    _ => {
                        snapshot.saved_floating.insert("Saved".into(), floating);
                    }
                }
                assert!(snapshot.validate().is_err(), "viewer {id} at location {location} must be rejected");
            }
        }
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
