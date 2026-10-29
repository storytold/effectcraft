//! Workspace files of earlier versions: EffectCraft's own dock tree (craft-libs 5c874d51 and
//! before), read into private copies of its types and turned into craft-ui workspaces.
//!
//! - Version 1 (`{version: 1, workspace, dock, floating, …, panel_windows}`) keeps the current
//!   arrangement, the named workspaces, focus and the maximized panel.
//! - The unversioned record (`{workspaces, floating}`) holds only named workspaces; the
//!   current arrangement stays the default.
//! - A column of stacked frames becomes tab groups split top to bottom. Each open frame is a
//!   group with all its tabs; a frame with a height keeps it as a fixed size (its tab strip
//!   added), and the others share the rest. A closed frame's tabs join the open frame above it
//!   (below it, at the top of the column), as a split has no collapsed frames.
//! - A floating group in its own native window (`panel_windows`) keeps that place as the
//!   window's native rect. The earlier version stored the OS window's outer position, so the
//!   window's id is returned for [`crate::dock_ui::native_outer_to_content`] to move it to its
//!   content position once the main window reports its frame.
//! - Closed panels had no recorded place: they stay closed and reopen at their default place.

use std::collections::BTreeMap;

use craft_ui::docking::{Floating, Node};
use craft_ui::layout::{SplitAxis, SplitSize};
use serde::Deserialize;

use super::{Snapshot, VERSION};
use crate::dock::{Layout, PanelKind, Workspace};
use crate::state::UiState;

#[derive(Deserialize)]
enum OldSplitSize {
    Ratio(f32),
    FixedA(f32),
    FixedB(f32),
}

/// A stacked frame; files from before stacked frames held tabs name one `panel`.
#[derive(Deserialize)]
struct OldStackEntry {
    #[serde(default)]
    panel: Option<PanelKind>,
    #[serde(default)]
    panels: Vec<PanelKind>,
    #[serde(default)]
    active: usize,
    open: bool,
    #[serde(default)]
    height: Option<f32>,
}

#[derive(Deserialize)]
enum OldNode {
    Split { vertical: bool, size: OldSplitSize, a: Box<OldNode>, b: Box<OldNode> },
    Tabs { panels: Vec<PanelKind>, active: usize },
    Stack { entries: Vec<OldStackEntry> },
}

#[derive(Deserialize)]
struct OldFloating {
    panels: Vec<PanelKind>,
    active: usize,
    rect: [f32; 4],
}

/// The floating group holding `panel` shows in its own native window at `rect`.
#[derive(Deserialize)]
struct OldPanelWindow {
    panel: PanelKind,
    rect: [f32; 4],
}

#[derive(Deserialize)]
struct OldSnapshot {
    workspace: String,
    dock: OldNode,
    floating: Vec<OldFloating>,
    focused: PanelKind,
    maximized: Option<PanelKind>,
    saved_workspaces: BTreeMap<String, OldNode>,
    saved_floating: BTreeMap<String, Vec<OldFloating>>,
    #[serde(default)]
    panel_windows: Vec<OldPanelWindow>,
}

#[derive(Deserialize)]
struct OldRecord {
    #[serde(default)]
    workspaces: BTreeMap<String, OldNode>,
    #[serde(default)]
    floating: BTreeMap<String, Vec<OldFloating>>,
}

/// Whether `value` is a file of an earlier version that [`migrate`] reads.
pub(super) fn is_legacy(value: &serde_json::Value) -> bool {
    match value.get("version") {
        Some(version) => version.as_u64() == Some(1),
        None => value.as_object().is_some_and(|fields| fields.contains_key("workspaces") || fields.contains_key("floating")),
    }
}

/// The snapshot an earlier version's file describes, and the ids of its floating windows whose
/// native rect holds the OS window's outer position. `ui` supplies what an unversioned record
/// leaves out (the current arrangement). The caller validates the result.
pub(super) fn migrate(value: serde_json::Value, ui: &UiState) -> Result<(Snapshot, Vec<u64>), String> {
    if value.get("version").is_none() {
        let record: OldRecord = serde_json::from_value(value).map_err(|e| e.to_string())?;
        let mut snapshot = Snapshot::capture(ui);
        snapshot.saved_workspaces = named(record.workspaces, record.floating)?;
        return Ok((snapshot, Vec::new()));
    }
    let old: OldSnapshot = serde_json::from_value(value).map_err(|e| e.to_string())?;
    let dock = workspace(old.dock, old.floating, &old.panel_windows)?;
    let outer = dock.layout.floating.iter().filter(|window| window.native.is_some()).map(|window| window.id).collect();
    let snapshot = Snapshot {
        version: VERSION,
        workspace: old.workspace,
        dock,
        focused: old.focused,
        maximized: old.maximized,
        saved_workspaces: named(old.saved_workspaces, old.saved_floating)?,
    };
    Ok((snapshot, outer))
}

/// Named workspaces with their floating groups; floating groups of no named workspace are
/// dropped, as the earlier version never showed them.
fn named(roots: BTreeMap<String, OldNode>, mut floating: BTreeMap<String, Vec<OldFloating>>) -> Result<BTreeMap<String, Workspace>, String> {
    roots
        .into_iter()
        .map(|(name, root)| {
            let groups = floating.remove(&name).unwrap_or_default();
            workspace(root, groups, &[]).map(|workspace| (name, workspace))
        })
        .collect()
}

fn workspace(root: OldNode, floating: Vec<OldFloating>, windows: &[OldPanelWindow]) -> Result<Workspace, String> {
    // Closing the last docked panel left an empty tab group as the root.
    let root = match root {
        OldNode::Tabs { panels, .. } if panels.is_empty() => None,
        root => Some(node(root, 0)?),
    };
    let floating = floating
        .into_iter()
        .map(|group| {
            let native = windows.iter().find(|window| group.panels.contains(&window.panel)).map(|window| window.rect);
            let mut window = Floating::single(group.panels, group.active, group.rect);
            window.native = native.map(crate::dock_ui::sanitized_window_rect);
            window
        })
        .collect();
    Ok(Workspace::new(Layout { root, floating }))
}

fn node(old: OldNode, depth: usize) -> Result<Node<PanelKind>, String> {
    if depth > craft_ui::docking::MAX_DEPTH {
        return Err("workspace dock tree exceeds supported limits".into());
    }
    Ok(match old {
        OldNode::Tabs { panels, active } => Node::Tabs { panels, active },
        OldNode::Split { vertical, size, a, b } => Node::Split {
            axis: if vertical { SplitAxis::Vertical } else { SplitAxis::Horizontal },
            size: match size {
                OldSplitSize::Ratio(value) => SplitSize::Ratio(value),
                OldSplitSize::FixedA(value) => SplitSize::FixedFirst(value),
                OldSplitSize::FixedB(value) => SplitSize::FixedSecond(value),
            },
            first: Box::new(node(*a, depth.saturating_add(1))?),
            second: Box::new(node(*b, depth.saturating_add(1))?),
        },
        OldNode::Stack { entries } => column(entries)?,
    })
}

/// The space between two frames of a column, in points ([`crate::theme::Tokens::gap`]).
const FRAME_GAP: f32 = 4.0;
/// The largest frame height a migrated column keeps, in points.
const MAX_FRAME: f32 = 10_000.0;

/// A tab group of a column: its panels, shown tab, and body height when it has one.
struct Frame {
    panels: Vec<PanelKind>,
    active: usize,
    height: Option<f32>,
}

/// A column of stacked frames as tab groups split top to bottom (see the module docs).
fn column(entries: Vec<OldStackEntry>) -> Result<Node<PanelKind>, String> {
    let mut frames: Vec<Frame> = Vec::new();
    // Tabs of closed frames above the first open one.
    let mut waiting: Vec<PanelKind> = Vec::new();
    for entry in entries {
        let mut panels = entry.panels;
        if let Some(panel) = entry.panel
            && !panels.contains(&panel)
        {
            panels.insert(0, panel);
        }
        if panels.is_empty() {
            continue;
        }
        let active = entry.active.min(panels.len().saturating_sub(1));
        if entry.open {
            let height = entry.height.filter(|h| h.is_finite() && *h > 0.0).map(|h| h.min(MAX_FRAME));
            frames.push(Frame { panels, active, height });
        } else if let Some(above) = frames.last_mut() {
            above.panels.extend(panels);
        } else {
            waiting.extend(panels);
        }
    }
    match frames.first_mut() {
        Some(top) => top.panels.extend(waiting),
        None if !waiting.is_empty() => frames.push(Frame { panels: waiting, active: 0, height: None }),
        None => return Err("a stacked frame holds no panels".into()),
    }
    let frame_size = |frame: &Frame| frame.height.map(|h| h + crate::dock::TAB_STRIP);
    let mut node: Option<Node<PanelKind>> = None;
    // Below the frame being added: the frames' fixed sizes and gaps, or None when one shares.
    let mut below_fixed = Some(0.0_f32);
    let mut below_flexible = 0_usize;
    for frame in frames.into_iter().rev() {
        let own = frame_size(&frame);
        let gap = if node.is_some() { FRAME_GAP } else { 0.0 };
        let tabs = Node::Tabs { panels: frame.panels, active: frame.active };
        node = Some(match node {
            None => tabs,
            Some(rest) => {
                let size = match (own, below_fixed) {
                    (Some(own), _) => SplitSize::FixedFirst(own),
                    (None, Some(below)) => SplitSize::FixedSecond(below),
                    // Shares the rest with the flexible frames below.
                    (None, None) => SplitSize::Ratio(1.0 / (below_flexible as f32 + 1.0)),
                };
                Node::Split { axis: SplitAxis::Vertical, size, first: Box::new(tabs), second: Box::new(rest) }
            }
        });
        below_fixed = match (own, below_fixed) {
            (Some(own), Some(below)) => Some(below + gap + own),
            _ => None,
        };
        if own.is_none() {
            below_flexible = below_flexible.saturating_add(1);
        }
    }
    node.ok_or_else(|| "a stacked frame holds no panels".into())
}
