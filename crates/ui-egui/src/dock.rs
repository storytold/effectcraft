//! Docking: a tree of splits and tab groups. Panels are rounded frames separated by
//! thin gutters; each group has a tab strip (active tab bright, with a panel menu "≡"); the focused
//! panel gets a blue outline. Gutters drag to resize; workspaces are serialized trees.

use egui::{Align2, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use serde::{Deserialize, Serialize};

use crate::icons::{self, Icon};
use crate::theme::Tokens;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PanelKind {
    Project,
    EffectControls,
    Composition,
    Layer,
    Timeline,
    Info,
    Audio,
    Preview,
    EffectsPresets,
    Properties,
    Character,
    Paragraph,
    Align,
    Tracker,
    Wiggler,
    Smoother,
    MotionSketch,
    Paint,
    Brushes,
    RenderQueue,
    Flowchart,
    History,
    Markers,
    MaskInterpolation,
    ScriptConsole,
    EssentialGraphics,
    /// A dockable ScriptUI panel (a script from the ScriptUI Panels folder), by script window id.
    ScriptPanel(u32),
    /// Another Composition viewer (View ▸ New Viewer), by viewer id (`panels::viewers`).
    Viewer(u32),
    LumetriScopes,
    Footage,
    MediaBrowser,
    Metadata,
    Progress,
    ContentAwareFill,
    /// Window ▸ Create Nulls From Paths (our own panel).
    CreateNullsFromPaths,
    /// Window ▸ VR Comp Editor (our own panel).
    VrCompEditor,
}

impl PanelKind {
    pub const ALL: [PanelKind; 34] = [
        PanelKind::Project,
        PanelKind::EffectControls,
        PanelKind::Composition,
        PanelKind::Layer,
        PanelKind::Timeline,
        PanelKind::Info,
        PanelKind::Audio,
        PanelKind::Preview,
        PanelKind::EffectsPresets,
        PanelKind::Properties,
        PanelKind::Character,
        PanelKind::Paragraph,
        PanelKind::Align,
        PanelKind::Tracker,
        PanelKind::Wiggler,
        PanelKind::Smoother,
        PanelKind::MotionSketch,
        PanelKind::Paint,
        PanelKind::Brushes,
        PanelKind::RenderQueue,
        PanelKind::Flowchart,
        PanelKind::History,
        PanelKind::Markers,
        PanelKind::MaskInterpolation,
        PanelKind::ScriptConsole,
        PanelKind::EssentialGraphics,
        PanelKind::LumetriScopes,
        PanelKind::Footage,
        PanelKind::MediaBrowser,
        PanelKind::Metadata,
        PanelKind::Progress,
        PanelKind::ContentAwareFill,
        PanelKind::CreateNullsFromPaths,
        PanelKind::VrCompEditor,
    ];
    pub fn title(self) -> &'static str {
        match self {
            PanelKind::Project => "Project",
            PanelKind::EffectControls => "Effect Controls",
            PanelKind::Composition => "Composition",
            PanelKind::Layer => "Layer",
            PanelKind::Timeline => "Timeline",
            PanelKind::Info => "Info",
            PanelKind::Audio => "Audio",
            PanelKind::Preview => "Preview",
            PanelKind::EffectsPresets => "Effects & Presets",
            PanelKind::Properties => "Properties",
            PanelKind::Character => "Character",
            PanelKind::Paragraph => "Paragraph",
            PanelKind::Align => "Align",
            PanelKind::Tracker => "Tracker",
            PanelKind::Wiggler => "Wiggler",
            PanelKind::Smoother => "Smoother",
            PanelKind::MotionSketch => "Motion Sketch",
            PanelKind::Paint => "Paint",
            PanelKind::Brushes => "Brushes",
            PanelKind::RenderQueue => "Render Queue",
            PanelKind::Flowchart => "Flowchart",
            PanelKind::History => "History",
            PanelKind::Markers => "Markers",
            PanelKind::MaskInterpolation => "Mask Interpolation",
            PanelKind::ScriptConsole => "Script Console",
            PanelKind::EssentialGraphics => "Essential Graphics",
            PanelKind::ScriptPanel(_) => "ScriptUI Panel",
            PanelKind::Viewer(_) => "Composition",
            PanelKind::LumetriScopes => "Lumetri Scopes",
            PanelKind::Footage => "Footage",
            PanelKind::MediaBrowser => "Media Browser",
            PanelKind::Metadata => "Metadata",
            PanelKind::Progress => "Progress",
            PanelKind::ContentAwareFill => "Content-Aware Fill",
            PanelKind::CreateNullsFromPaths => "Create Nulls From Paths",
            PanelKind::VrCompEditor => "VR Comp Editor",
        }
    }
    pub fn id(self) -> String {
        format!("{self:?}")
    }
    pub fn from_name(s: &str) -> Option<PanelKind> {
        let n = s.to_ascii_lowercase().replace([' ', '_', '-', '&'], "");
        Self::ALL.iter().copied().find(|p| format!("{p:?}").to_ascii_lowercase() == n || p.title().to_ascii_lowercase().replace([' ', '&', '-'], "") == n)
    }
    /// Window-menu shortcut.
    pub fn window_shortcut(self) -> Option<&'static str> {
        match self {
            PanelKind::Project => Some("Cmd+0"),
            PanelKind::Info => Some("Cmd+2"),
            PanelKind::Preview => Some("Cmd+3"),
            PanelKind::Audio => Some("Cmd+4"),
            PanelKind::EffectsPresets => Some("Cmd+5"),
            PanelKind::Character => Some("Cmd+6"),
            PanelKind::Paragraph => Some("Cmd+7"),
            PanelKind::Paint => Some("Cmd+8"),
            PanelKind::Brushes => Some("Cmd+9"),
            PanelKind::RenderQueue => Some("Cmd+Alt+0"),
            PanelKind::EffectControls => Some("F3"),
            _ => None,
        }
    }
    /// Chrome-less panels (none in the AE layout; kept for parity with the docking code).
    pub fn compact(self) -> bool {
        false
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum SplitSize {
    Ratio(f32),
    /// First child has a fixed size in points.
    FixedA(f32),
    /// Second child has a fixed size in points.
    FixedB(f32),
}

/// One panel in a [`DockNode::Stack`].
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct StackEntry {
    pub panel: PanelKind,
    pub open: bool,
    /// Content height when open (set by dragging the gaps); `None` = share the remaining height.
    pub height: Option<f32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum DockNode {
    /// `vertical`: children stacked top/bottom; else side by side.
    Split {
        vertical: bool,
        size: SplitSize,
        a: Box<DockNode>,
        b: Box<DockNode>,
    },
    Tabs {
        panels: Vec<PanelKind>,
        active: usize,
    },
    /// After Effects' stacked panels (e.g. the Default workspace's right column): each panel has
    /// a header row; clicking it expands or collapses the panel in place, and dragging the gap
    /// between two open panels resizes them.
    Stack {
        entries: Vec<StackEntry>,
    },
}

fn tabs(p: &[PanelKind], active: usize) -> DockNode {
    DockNode::Tabs { panels: p.to_vec(), active }
}
fn stack(e: &[(PanelKind, bool, Option<f32>)]) -> DockNode {
    DockNode::Stack { entries: e.iter().map(|&(panel, open, height)| StackEntry { panel, open, height }).collect() }
}
fn hsplit(size: SplitSize, a: DockNode, b: DockNode) -> DockNode {
    DockNode::Split { vertical: false, size, a: Box::new(a), b: Box::new(b) }
}
fn vsplit(size: SplitSize, a: DockNode, b: DockNode) -> DockNode {
    DockNode::Split { vertical: true, size, a: Box::new(a), b: Box::new(b) }
}

/// The built-in workspaces: After Effects 2026's (Window ▸ Workspace), less Libraries (an Adobe
/// service). Learn shows the Home screen (community links instead of Adobe's tutorials);
/// Essential Graphics uses the Properties panel; Undocked Panels floats the panels over the
/// Composition panel.
pub const WORKSPACES: &[&str] = &[
    "Default",
    "Standard",
    "Small Screen",
    "Animation",
    "Effects",
    "Motion Tracking",
    "Paint",
    "Text",
    "Minimal",
    "All Panels",
    "Review",
    "Learn",
    "Essential Graphics",
    "Color",
    "Undocked Panels",
];

/// The floating panels a workspace starts with (Undocked Panels), in points for a window of
/// about 1680×1020 (they are kept inside the window when drawn).
pub fn workspace_floating(name: &str) -> Vec<Floating> {
    use PanelKind::*;
    let f = |panels: &[PanelKind], rect: [f32; 4]| Floating { panels: panels.to_vec(), active: 0, rect };
    match name {
        "Undocked Panels" => vec![
            f(&[Project, EffectControls], [24.0, 110.0, 320.0, 420.0]),
            f(&[Preview, Info, Audio], [1330.0, 110.0, 320.0, 300.0]),
            f(&[EffectsPresets, Properties, Character, Paragraph], [1330.0, 430.0, 320.0, 360.0]),
            f(&[Timeline, RenderQueue], [24.0, 640.0, 1280.0, 340.0]),
        ],
        _ => vec![],
    }
}

/// The default layout of a named workspace.
pub fn workspace(name: &str) -> DockNode {
    use PanelKind::*;
    use SplitSize::*;
    let right = |p: &[PanelKind], q: &[PanelKind]| vsplit(Ratio(0.45), tabs(p, 0), tabs(q, 0));
    match name {
        "Animation" => vsplit(
            Ratio(0.55),
            hsplit(
                FixedA(300.0),
                tabs(&[Project, EffectControls], 1),
                hsplit(FixedB(300.0), tabs(&[Composition, Layer], 0), right(&[Info, Preview, Audio], &[EffectsPresets, Smoother, Wiggler, MotionSketch])),
            ),
            tabs(&[Timeline, RenderQueue], 0),
        ),
        "Effects" => vsplit(
            Ratio(0.58),
            hsplit(
                FixedA(340.0),
                tabs(&[EffectControls, Project], 0),
                hsplit(FixedB(300.0), tabs(&[Composition, Layer], 0), right(&[Info, Preview], &[EffectsPresets])),
            ),
            tabs(&[Timeline, RenderQueue], 0),
        ),
        "Motion Tracking" => vsplit(
            Ratio(0.6),
            hsplit(
                FixedA(300.0),
                tabs(&[Project, EffectControls], 0),
                hsplit(FixedB(300.0), tabs(&[Layer, Composition], 0), right(&[Info, Preview], &[Tracker])),
            ),
            tabs(&[Timeline], 0),
        ),
        "Paint" => vsplit(
            Ratio(0.6),
            hsplit(
                FixedA(300.0),
                tabs(&[Project, EffectControls], 0),
                hsplit(FixedB(300.0), tabs(&[Layer, Composition], 0), right(&[Paint, Info], &[Brushes, Preview])),
            ),
            tabs(&[Timeline], 0),
        ),
        "Text" => vsplit(
            Ratio(0.58),
            hsplit(
                FixedA(300.0),
                tabs(&[Project, EffectControls], 0),
                hsplit(FixedB(300.0), tabs(&[Composition], 0), right(&[Character, Paragraph], &[Align, Preview, EffectsPresets])),
            ),
            tabs(&[Timeline], 0),
        ),
        "Minimal" => vsplit(Ratio(0.6), tabs(&[Composition], 0), tabs(&[Timeline], 0)),
        // Review: a large viewer with the markers and the project beside it.
        "Review" => vsplit(
            Ratio(0.66),
            hsplit(FixedA(280.0), tabs(&[Project, Markers], 0), hsplit(FixedB(260.0), tabs(&[Composition, Layer], 0), right(&[Preview, Info], &[Audio]))),
            tabs(&[Timeline, RenderQueue], 0),
        ),
        // Essential Graphics: the Properties panel (After Effects' successor to the Essential
        // Graphics panel) full height on the right, type panels beside the viewer.
        "Essential Graphics" => hsplit(
            FixedB(320.0),
            vsplit(
                Ratio(0.58),
                hsplit(
                    FixedA(280.0),
                    tabs(&[Project, EffectControls], 0),
                    hsplit(FixedB(260.0), tabs(&[Composition, Layer], 0), right(&[Character, Paragraph], &[Align, Preview])),
                ),
                tabs(&[Timeline], 0),
            ),
            stack(&[(Properties, true, None), (EffectsPresets, false, None)]),
        ),
        // Color: wide Effect Controls for grading effects, Info for colour readouts.
        "Color" => vsplit(
            Ratio(0.6),
            hsplit(
                FixedA(360.0),
                tabs(&[EffectControls, Project], 0),
                hsplit(FixedB(340.0), tabs(&[Composition, Layer], 0), right(&[LumetriScopes, Info, Preview], &[EffectsPresets])),
            ),
            tabs(&[Timeline], 0),
        ),
        // Undocked Panels: the Composition panel docked, the others floating
        // ([`workspace_floating`]).
        "Undocked Panels" => tabs(&[Composition, Layer, Flowchart], 0),
        "Small Screen" => vsplit(
            Ratio(0.55),
            hsplit(
                FixedA(240.0),
                tabs(&[Project, EffectControls], 0),
                hsplit(FixedB(240.0), tabs(&[Composition], 0), tabs(&[Info, Preview, EffectsPresets], 1)),
            ),
            tabs(&[Timeline], 0),
        ),
        "All Panels" => vsplit(
            Ratio(0.55),
            hsplit(
                FixedA(300.0),
                tabs(&[Project, EffectControls, Flowchart, History, MediaBrowser, Metadata], 0),
                hsplit(
                    FixedB(300.0),
                    tabs(&[Composition, Layer, Footage], 0),
                    right(
                        &[Info, Preview, Audio, Align, Character, Paragraph, LumetriScopes],
                        &[
                            EffectsPresets,
                            Properties,
                            Tracker,
                            Wiggler,
                            Smoother,
                            MotionSketch,
                            Paint,
                            Brushes,
                            Markers,
                            ContentAwareFill,
                            CreateNullsFromPaths,
                            VrCompEditor,
                        ],
                    ),
                ),
            ),
            tabs(&[Timeline, RenderQueue, Progress], 0),
        ),
        "Standard" => vsplit(
            Ratio(0.56),
            hsplit(
                FixedA(300.0),
                tabs(&[Project, EffectControls], 0),
                hsplit(FixedB(290.0), tabs(&[Composition, Layer], 0), right(&[Info, Audio], &[Preview, EffectsPresets])),
            ),
            tabs(&[Timeline, RenderQueue], 0),
        ),
        // Default (as in After Effects): Project/Effect Controls | Composition over the Timeline,
        // and a full-height right column with Preview above the Properties panel.
        _ => hsplit(
            FixedB(300.0),
            vsplit(Ratio(0.56), hsplit(FixedA(300.0), tabs(&[Project, EffectControls], 0), tabs(&[Composition, Layer], 0)), tabs(&[Timeline, RenderQueue], 0)),
            stack(&[(Preview, true, Some(46.0)), (Properties, true, None), (Align, false, None), (Audio, false, None), (EffectsPresets, false, None)]),
        ),
    }
}

impl DockNode {
    /// The complete tabbed or stacked panel group containing `panel`. Splits are workspace
    /// structure, not a group; keep the walk iterative for deeply nested saved layouts.
    pub fn panel_group(&self, panel: PanelKind) -> Option<&DockNode> {
        let mut pending = vec![self];
        while let Some(node) = pending.pop() {
            match node {
                DockNode::Split { a, b, .. } => {
                    pending.push(b);
                    pending.push(a);
                }
                DockNode::Tabs { panels, .. } if panels.contains(&panel) => return Some(node),
                DockNode::Stack { entries } if entries.iter().any(|e| e.panel == panel) => return Some(node),
                _ => {}
            }
        }
        None
    }

    /// Every panel in the tree.
    pub fn panels(&self, out: &mut Vec<PanelKind>) {
        match self {
            DockNode::Split { a, b, .. } => {
                a.panels(out);
                b.panels(out);
            }
            DockNode::Tabs { panels, .. } => out.extend(panels.iter().copied()),
            DockNode::Stack { entries } => out.extend(entries.iter().map(|e| e.panel)),
        }
    }
    pub fn contains(&self, p: PanelKind) -> bool {
        let mut v = Vec::new();
        self.panels(&mut v);
        v.contains(&p)
    }
    /// Make `p` the active tab of its group. Returns false if not present.
    pub fn activate(&mut self, p: PanelKind) -> bool {
        match self {
            DockNode::Split { a, b, .. } => a.activate(p) || b.activate(p),
            DockNode::Tabs { panels, active } => {
                if let Some(i) = panels.iter().position(|x| *x == p) {
                    *active = i;
                    true
                } else {
                    false
                }
            }
            DockNode::Stack { entries } => match entries.iter_mut().find(|e| e.panel == p) {
                Some(e) => {
                    e.open = true;
                    true
                }
                None => false,
            },
        }
    }
    /// Expand or collapse a stacked panel. Returns false if `p` is not in a stack.
    pub fn toggle_stacked(&mut self, p: PanelKind) -> bool {
        match self {
            DockNode::Split { a, b, .. } => a.toggle_stacked(p) || b.toggle_stacked(p),
            DockNode::Tabs { .. } => false,
            DockNode::Stack { entries } => match entries.iter_mut().find(|e| e.panel == p) {
                Some(e) => {
                    e.open = !e.open;
                    true
                }
                None => false,
            },
        }
    }
    pub fn is_visible(&self, p: PanelKind) -> bool {
        match self {
            DockNode::Split { a, b, .. } => a.is_visible(p) || b.is_visible(p),
            DockNode::Tabs { panels, active } => panels.get(*active) == Some(&p),
            DockNode::Stack { entries } => entries.iter().any(|e| e.panel == p && e.open),
        }
    }
    /// Close a panel (remove its tab; empty groups collapse their split).
    pub fn close(&mut self, p: PanelKind) {
        if let DockNode::Split { a, b, .. } = self {
            a.close(p);
            b.close(p);
            let empty = |n: &DockNode| match n {
                DockNode::Tabs { panels, .. } => panels.is_empty(),
                DockNode::Stack { entries } => entries.is_empty(),
                DockNode::Split { .. } => false,
            };
            if empty(a) {
                *self = (**b).clone();
            } else if empty(b) {
                *self = (**a).clone();
            }
        } else if let DockNode::Tabs { panels, active } = self {
            let shown = panels.get(*active).copied();
            panels.retain(|x| *x != p);
            // Keep a surviving active panel; otherwise prefer its successor, then predecessor.
            *active = shown.and_then(|p| panels.iter().position(|x| *x == p)).unwrap_or((*active).min(panels.len().saturating_sub(1)));
        } else if let DockNode::Stack { entries } = self {
            entries.retain(|e| e.panel != p);
        }
    }
    /// Add a panel as a tab next to `near` (or into the first group).
    pub fn open_near(&mut self, p: PanelKind, near: PanelKind) {
        if self.contains(p) {
            self.activate(p);
            return;
        }
        fn add(n: &mut DockNode, p: PanelKind, near: PanelKind) -> bool {
            match n {
                DockNode::Split { a, b, .. } => add(a, p, near) || add(b, p, near),
                DockNode::Tabs { panels, active } => {
                    if panels.contains(&near) {
                        panels.push(p);
                        *active = panels.len() - 1;
                        true
                    } else {
                        false
                    }
                }
                DockNode::Stack { entries } => match entries.iter().position(|e| e.panel == near) {
                    Some(i) => {
                        entries.insert(i + 1, StackEntry { panel: p, open: true, height: None });
                        true
                    }
                    None => false,
                },
            }
        }
        if !add(self, p, near) && !add(self, p, PanelKind::EffectsPresets) {
            add(self, p, PanelKind::Project);
        }
    }
}

/// Where a dragged panel tab lands on a group: as a tab (center) or split off to one side.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Zone {
    Center,
    Left,
    Right,
    Top,
    Bottom,
}

impl Zone {
    pub fn from_name(s: &str) -> Option<Zone> {
        Some(match s.to_ascii_lowercase().as_str() {
            "center" | "tab" => Zone::Center,
            "left" => Zone::Left,
            "right" => Zone::Right,
            "top" => Zone::Top,
            "bottom" => Zone::Bottom,
            _ => return None,
        })
    }
}

/// The drop zone for a pointer at `pos` over a group `rect` (with a `tab_h` tab strip): the tab
/// strip and the middle of the group are Center; within a quarter of the size from an edge,
/// the nearest edge.
pub fn drop_zone(rect: Rect, tab_h: f32, pos: egui::Pos2) -> Zone {
    if pos.y < rect.min.y + tab_h {
        return Zone::Center;
    }
    let fx = ((pos.x - rect.min.x) / rect.width().max(1.0)).clamp(0.0, 1.0);
    let fy = ((pos.y - rect.min.y) / rect.height().max(1.0)).clamp(0.0, 1.0);
    let d = [(fx, Zone::Left), (1.0 - fx, Zone::Right), (fy, Zone::Top), (1.0 - fy, Zone::Bottom)];
    let (m, z) = d.into_iter().min_by(|a, b| a.0.total_cmp(&b.0)).unwrap_or((1.0, Zone::Center));
    if m > 0.25 { Zone::Center } else { z }
}

/// The part of a group `rect` a drop into `zone` would occupy (the AE-style highlight).
pub fn zone_rect(rect: Rect, zone: Zone) -> Rect {
    let c = rect.center();
    match zone {
        Zone::Center => rect,
        Zone::Left => Rect::from_min_max(rect.min, pos2(c.x, rect.max.y)),
        Zone::Right => Rect::from_min_max(pos2(c.x, rect.min.y), rect.max),
        Zone::Top => Rect::from_min_max(rect.min, pos2(rect.max.x, c.y)),
        Zone::Bottom => Rect::from_min_max(pos2(rect.min.x, c.y), rect.max),
    }
}

/// A panel group floating over the dock (undocked).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Floating {
    pub panels: Vec<PanelKind>,
    pub active: usize,
    /// [x, y, w, h] in points.
    pub rect: [f32; 4],
}

impl DockNode {
    /// Insert panel `p` relative to the group that holds `anchor`: as a tab next to it
    /// (Center) or in a new group split off to one side. `p` is removed from its old place
    /// first. Returns false (tree unchanged) if `anchor` isn't docked or is `p` itself alone.
    pub fn dock_panel(&mut self, p: PanelKind, anchor: PanelKind, zone: Zone) -> bool {
        if p == anchor || !self.contains(anchor) {
            return false;
        }
        let before = self.clone();
        self.close(p);
        fn rec(n: &mut DockNode, p: PanelKind, anchor: PanelKind, zone: Zone) -> bool {
            let here = match n {
                DockNode::Split { a, b, .. } => return rec(a, p, anchor, zone) || rec(b, p, anchor, zone),
                DockNode::Tabs { panels, .. } => panels.contains(&anchor),
                DockNode::Stack { entries } => entries.iter().any(|e| e.panel == anchor),
            };
            if !here {
                return false;
            }
            match (zone, &mut *n) {
                (Zone::Center, DockNode::Tabs { panels, active }) => {
                    let i = panels.iter().position(|x| *x == anchor).map_or(panels.len(), |i| i + 1);
                    panels.insert(i, p);
                    *active = i;
                }
                (Zone::Center, DockNode::Stack { entries }) => {
                    let i = entries.iter().position(|e| e.panel == anchor).map_or(entries.len(), |i| i + 1);
                    entries.insert(i, StackEntry { panel: p, open: true, height: None });
                }
                (z, node) => {
                    let new = tabs(&[p], 0);
                    let old = std::mem::replace(node, tabs(&[], 0));
                    let vertical = matches!(z, Zone::Top | Zone::Bottom);
                    let (a, b) = if matches!(z, Zone::Left | Zone::Top) { (new, old) } else { (old, new) };
                    *node = DockNode::Split { vertical, size: SplitSize::Ratio(0.5), a: Box::new(a), b: Box::new(b) };
                }
            }
            true
        }
        if rec(self, p, anchor, zone) {
            true
        } else {
            *self = before;
            false
        }
    }

    /// Move panel `p` into the group that holds `anchor` as a tab before `before` (one of that
    /// group's panels) or last, and show it. False (tree unchanged) if `anchor` isn't docked.
    pub fn insert_tab(&mut self, p: PanelKind, anchor: PanelKind, before: Option<PanelKind>) -> bool {
        if p == anchor || !self.contains(anchor) {
            return false;
        }
        let saved = self.clone();
        self.close(p);
        fn rec(n: &mut DockNode, p: PanelKind, anchor: PanelKind, before: Option<PanelKind>) -> bool {
            match n {
                DockNode::Split { a, b, .. } => rec(a, p, anchor, before) || rec(b, p, anchor, before),
                DockNode::Tabs { panels, active } if panels.contains(&anchor) => {
                    let i = before.and_then(|b| panels.iter().position(|x| *x == b)).unwrap_or(panels.len());
                    panels.insert(i, p);
                    *active = i;
                    true
                }
                DockNode::Stack { entries } if entries.iter().any(|e| e.panel == anchor) => {
                    let i = before.and_then(|b| entries.iter().position(|e| e.panel == b)).unwrap_or(entries.len());
                    entries.insert(i, StackEntry { panel: p, open: true, height: None });
                    true
                }
                _ => false,
            }
        }
        if rec(self, p, anchor, before) {
            true
        } else {
            *self = saved;
            false
        }
    }

    /// Path of the group containing `p` (as in [`Group::path`]; stacks add `sN`).
    pub fn path_of(&self, p: PanelKind) -> Option<String> {
        fn rec(n: &DockNode, p: PanelKind, path: &str) -> Option<String> {
            match n {
                DockNode::Split { a, b, .. } => rec(a, p, &format!("{path}a")).or_else(|| rec(b, p, &format!("{path}b"))),
                DockNode::Tabs { panels, .. } => panels.contains(&p).then(|| path.to_string()),
                DockNode::Stack { entries } => entries.iter().position(|e| e.panel == p).map(|i| format!("{path}s{i}")),
            }
        }
        rec(self, p, "")
    }

    /// Number of panels in the tree.
    pub fn panel_count(&self) -> usize {
        let mut v = vec![];
        self.panels(&mut v);
        v.len()
    }
}

/// A full layout: the docked tree plus floating panels (what a workspace saves).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    pub root: DockNode,
    #[serde(default)]
    pub floating: Vec<Floating>,
}

impl Layout {
    /// Undock `p` into a floating group at `rect` (no-op if it is the last docked panel).
    pub fn float(&mut self, p: PanelKind, rect: [f32; 4]) -> bool {
        if self.root.contains(p) {
            if self.root.panel_count() <= 1 {
                return false;
            }
            self.root.close(p);
        } else if !self.unfloat(p) {
            return false;
        }
        self.floating.push(Floating { panels: vec![p], active: 0, rect });
        true
    }

    /// Remove `p` from the floating groups (dropping emptied groups). True if it was floating.
    pub fn unfloat(&mut self, p: PanelKind) -> bool {
        let mut found = false;
        for f in &mut self.floating {
            if let Some(i) = f.panels.iter().position(|x| *x == p) {
                let shown = f.panels.get(f.active).copied();
                f.panels.remove(i);
                f.active = shown.and_then(|p| f.panels.iter().position(|x| *x == p)).unwrap_or(f.active.min(f.panels.len().saturating_sub(1)));
                found = true;
            }
        }
        self.floating.retain(|f| !f.panels.is_empty());
        found
    }

    /// Move `p` (docked or floating) into the group of `anchor` (docked or floating) as a tab
    /// before `before` or last: where a tab dragged onto a tab strip lands.
    pub fn insert_tab(&mut self, p: PanelKind, anchor: PanelKind, before: Option<PanelKind>) -> bool {
        if p == anchor || before == Some(p) {
            return false;
        }
        if self.floating.iter().any(|f| f.panels.contains(&anchor)) {
            if self.root.contains(p) {
                if self.root.panel_count() <= 1 {
                    return false;
                }
                self.root.close(p);
            } else {
                self.unfloat(p);
            }
            // (Removing `p` may have dropped an emptied group: look the anchor up again.)
            let Some(f) = self.floating.iter_mut().find(|f| f.panels.contains(&anchor)) else { return false };
            let i = before.and_then(|b| f.panels.iter().position(|x| *x == b)).unwrap_or(f.panels.len());
            f.panels.insert(i, p);
            f.active = i;
            return true;
        }
        if !self.root.contains(anchor) {
            return false;
        }
        // A floating panel isn't in the tree, so insert_tab's removal is a no-op for it.
        self.unfloat(p);
        self.root.insert_tab(p, anchor, before)
    }

    /// Dock `p` (docked or floating) next to `anchor`; `anchor` may be floating too (then Center
    /// adds a tab to that floating group).
    pub fn dock(&mut self, p: PanelKind, anchor: PanelKind, zone: Zone) -> bool {
        if p == anchor {
            return false;
        }
        if self.floating.iter().any(|f| f.panels.contains(&anchor)) {
            if zone != Zone::Center {
                return false;
            }
            if self.root.contains(p) {
                if self.root.panel_count() <= 1 {
                    return false;
                }
                self.root.close(p);
            } else {
                self.unfloat(p);
            }
            let Some(f) = self.floating.iter_mut().find(|f| f.panels.contains(&anchor)) else { return false };
            f.panels.push(p);
            f.active = f.panels.len() - 1;
            return true;
        }
        if !self.root.contains(anchor) {
            return false;
        }
        // A floating panel isn't in the tree, so dock_panel's removal is a no-op for it.
        self.unfloat(p);
        self.root.dock_panel(p, anchor, zone)
    }

    pub fn contains(&self, p: PanelKind) -> bool {
        self.root.contains(p) || self.floating.iter().any(|f| f.panels.contains(&p))
    }
}

/// One laid-out tab group.
pub struct Group {
    pub path: String,
    pub rect: Rect,
    pub content: Rect,
    pub panels: Vec<PanelKind>,
    pub active: usize,
    /// In a [`DockNode::Stack`]: whether the panel is expanded.
    pub stacked: Option<bool>,
}

/// Actions produced by interacting with the dock chrome.
#[derive(Debug, Clone, PartialEq)]
pub enum DockAction {
    Activate(PanelKind),
    /// Maximize or restore the complete group containing this panel.
    ToggleMaximize(PanelKind),
    /// Expand/collapse a stacked panel.
    ToggleStacked(PanelKind),
    Focus(PanelKind),
    Close(PanelKind),
    PanelMenu(PanelKind, egui::Pos2),
    /// A tab started being dragged (re-dock or undock).
    BeginDrag(PanelKind),
    /// The tab's lock was toggled (Composition / Timeline viewer lock).
    ToggleLock(PanelKind),
    /// A document tab of a panel was clicked (the Timeline's tab of comp `id`).
    ActivateDoc(PanelKind, u64),
    /// A document tab's close button (or middle click): close that document (comp `id`).
    CloseDoc(PanelKind, u64),
}

/// One document a panel shows as a tab of its own (the Timeline: one tab per open comp, as in
/// After Effects).
#[derive(Debug, Clone, PartialEq)]
pub struct DocTab {
    pub id: u64,
    pub title: String,
    pub deco: Option<TabDeco>,
    /// The document the panel shows now.
    pub active: bool,
}

/// What a group's tabs show: labels, the comp swatch / close / lock decorations and the panels
/// that show one tab per document.
pub struct TabInfo<'a> {
    pub title: &'a dyn Fn(PanelKind) -> String,
    pub decos: &'a [(PanelKind, TabDeco)],
    pub docs: &'a [(PanelKind, Vec<DocTab>)],
}

/// What drawing a group's chrome produced: actions and where each panel's tabs are (a panel with
/// document tabs spans all of them; tabs hidden behind the overflow chevron are left out).
#[derive(Default)]
pub struct Chrome {
    pub actions: Vec<DockAction>,
    pub tabs: Vec<(PanelKind, Rect)>,
}

/// After Effects-style extras on a tab that shows a composition (Composition, Timeline): a close
/// button (×), the item's label colour as a small square and the viewer lock.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TabDeco {
    pub swatch: egui::Color32,
    pub locked: bool,
    /// Close button and lock (Composition, Timeline); Effect Controls shows just the swatch.
    pub viewer: bool,
}

/// Lay out the tree into group rects (with `gap` gutters) and handle gutter dragging.
pub fn layout(ui: &mut egui::Ui, node: &mut DockNode, rect: Rect, t: &Tokens, path: &str, out: &mut Vec<Group>, reg: &mut crate::automation::Registry) {
    layout_at_depth(ui, node, rect, t, path, out, reg, 0);
}

// Bound this display traversal only. An overdeep branch is omitted for this paint;
// the saved workspace is not rewritten. Other DockNode traversals have their own policy.
const MAX_LAYOUT_DEPTH: usize = 128;
#[allow(clippy::too_many_arguments)]
fn layout_at_depth(
    ui: &mut egui::Ui,
    node: &mut DockNode,
    rect: Rect,
    t: &Tokens,
    path: &str,
    out: &mut Vec<Group>,
    reg: &mut crate::automation::Registry,
    depth: usize,
) {
    if depth >= MAX_LAYOUT_DEPTH
        || ![rect.min.x, rect.min.y, rect.max.x, rect.max.y, rect.width(), rect.height()].into_iter().all(f32::is_finite)
        || rect.width() < 0.0
        || rect.height() < 0.0
    {
        return;
    }
    match node {
        DockNode::Tabs { panels, active } => {
            let tab_h = if panels.len() == 1 && panels[0].compact() { 8.0 } else { t.tab_h };
            let head = tab_h.max(0.0).clamp(0.0, rect.height());
            let content = Rect::from_min_max(pos2(rect.min.x, (rect.min.y + head).min(rect.max.y)), rect.max);
            out.push(Group { path: path.to_string(), rect, content, panels: panels.clone(), active: *active, stacked: None });
        }
        DockNode::Stack { entries } => {
            // A header for every entry; the open panels' bodies share the rest.
            let head = t.tab_h.max(0.0).clamp(0.0, rect.height());
            let g = t.gap.max(0.0).clamp(0.0, rect.height());
            let n = entries.len() as f32;
            let avail = rect.height() - n * head - (n - 1.0).max(0.0) * g;
            let bodies = stack_bodies(entries, avail);
            let mut y = rect.min.y;
            let mut drag = None;
            for (i, e) in entries.iter().enumerate() {
                if y >= rect.max.y {
                    break;
                }
                let body = bodies.get(i).copied().unwrap_or(0.0);
                let r = Rect::from_min_max(pos2(rect.min.x, y), pos2(rect.max.x, (y + head + body).min(rect.max.y)));
                let content = Rect::from_min_max(pos2(r.min.x, (r.min.y + head).min(r.max.y)), r.max);
                let group = format!("{path}s{i}");
                // The gap below a panel resizes the open panels on either side of it.
                let above = entries.get(..=i).and_then(|s| s.iter().rposition(|x| x.open));
                let below = entries.get(i + 1..).and_then(|s| s.iter().position(|x| x.open)).map(|k| i + 1 + k);
                if let (Some(above), Some(below)) = (above, below) {
                    let gap = Rect::from_min_max(pos2(rect.min.x, r.max.y), pos2(rect.max.x, (r.max.y + g).min(rect.max.y)));
                    if let Some(d) = gutter(ui, gap, rect, true, &group, t, reg) {
                        drag = Some((above, below, d));
                    }
                }
                out.push(Group { path: group, rect: r, content, panels: vec![e.panel], active: 0, stacked: Some(e.open) });
                y = (r.max.y + g).min(rect.max.y);
            }
            if let Some((above, below, d)) = drag {
                resize_stacked(entries, &bodies, above, below, d);
            }
        }
        DockNode::Split { vertical, size, a, b } => {
            let total = if *vertical { rect.height() } else { rect.width() };
            let g = t.gap.max(0.0).clamp(0.0, total);
            let avail = (total - g).max(0.0);
            // Preserve 20-point children when they fit; otherwise share the available
            // space equally. Corrupt saved sizes get a balanced display fallback.
            let minimum = 20.0_f32.min(avail * 0.5);
            let first = match *size {
                SplitSize::Ratio(r) if r.is_finite() => avail * r.clamp(0.0, 1.0),
                SplitSize::FixedA(px) if px.is_finite() => px.clamp(0.0, avail),
                SplitSize::FixedB(px) if px.is_finite() => avail - px.clamp(0.0, avail),
                _ => avail * 0.5,
            }
            .clamp(minimum, avail - minimum);
            let (ra, gutter_rect, rb) = if *vertical {
                let split = (rect.min.y + first).min(rect.max.y);
                let end = (split + g).min(rect.max.y);
                (
                    Rect::from_min_max(rect.min, pos2(rect.max.x, split)),
                    Rect::from_min_max(pos2(rect.min.x, split), pos2(rect.max.x, end)),
                    Rect::from_min_max(pos2(rect.min.x, end), rect.max),
                )
            } else {
                let split = (rect.min.x + first).min(rect.max.x);
                let end = (split + g).min(rect.max.x);
                (
                    Rect::from_min_max(rect.min, pos2(split, rect.max.y)),
                    Rect::from_min_max(pos2(split, rect.min.y), pos2(end, rect.max.y)),
                    Rect::from_min_max(pos2(end, rect.min.y), rect.max),
                )
            };
            if let Some(d) = gutter(ui, gutter_rect, rect, *vertical, path, t, reg)
                && avail > 0.0
            {
                // A zero-size parent cannot produce a ratio. Use f64 for the
                // addition so a finite pointer delta cannot overflow first + d.
                let minimum = 40.0_f32.min(avail * 0.5);
                let nf = (f64::from(first) + f64::from(d)).clamp(f64::from(minimum), f64::from(avail - minimum)) as f32;
                *size = match *size {
                    SplitSize::Ratio(_) => SplitSize::Ratio(nf / avail),
                    SplitSize::FixedA(_) => SplitSize::FixedA(nf),
                    SplitSize::FixedB(_) => SplitSize::FixedB(avail - nf),
                };
            }
            layout_at_depth(ui, a, ra, t, &format!("{path}a"), out, reg, depth + 1);
            layout_at_depth(ui, b, rb, t, &format!("{path}b"), out, reg, depth + 1);
        }
    }
}

/// A gap between dock areas that drags to resize them (`vertical`: up and down), with a slightly
/// larger hit area than the gap. Returns the drag this frame.
fn gutter(ui: &mut egui::Ui, gap: Rect, parent: Rect, vertical: bool, path: &str, t: &Tokens, reg: &mut crate::automation::Registry) -> Option<f32> {
    let hit = gap.expand2(if vertical { vec2(0.0, 3.0) } else { vec2(3.0, 0.0) }).intersect(parent);
    let resp = ui.interact(hit, egui::Id::new(("dock-gutter", path.to_string())), Sense::drag());
    reg.add(&format!("dock.gutter.{path}"), hit, "gutter");
    if resp.hovered() || resp.dragged() {
        ui.ctx().set_cursor_icon(if vertical { egui::CursorIcon::ResizeVertical } else { egui::CursorIcon::ResizeHorizontal });
        ui.painter().rect_filled(gap, 0.0, t.focus.gamma_multiply(if resp.dragged() { 0.9 } else { 0.4 }));
    }
    resp.dragged().then(|| if vertical { resp.drag_delta().y } else { resp.drag_delta().x }).filter(|d| d.is_finite())
}

/// The least body height of an open stacked panel (when the stack has room for it).
const STACK_MIN: f32 = 40.0;

/// An open stacked panel without a height of its own: it shares the room the others leave.
fn flexible(e: &StackEntry) -> bool {
    e.open && !e.height.is_some_and(f32::is_finite)
}

/// The body heights of a stack's panels in `avail` points (headers and gaps taken out). Open
/// panels with a height keep it and the others share the rest; when no panel takes the rest, or
/// there is too little room, the heights grow or shrink in proportion (down to [`STACK_MIN`]) so
/// the open panels fill the stack.
fn stack_bodies(entries: &[StackEntry], avail: f32) -> Vec<f32> {
    let avail = if avail.is_finite() { avail.max(0.0) } else { 0.0 };
    let open = entries.iter().filter(|e| e.open).count();
    let min = STACK_MIN.min(avail / open.max(1) as f32);
    let fixed: Vec<f32> = entries.iter().filter(|e| e.open && !flexible(e)).filter_map(|e| e.height).map(|h| h.max(min)).collect();
    let flex = open.saturating_sub(fixed.len());
    let k = fixed.len() as f32;
    // What the panels with a height may take, and how much of it lies above their minimum.
    let room = avail - flex as f32 * min;
    let sum: f32 = fixed.iter().sum();
    let (excess, target) = (sum - k * min, (room - k * min).max(0.0));
    let fit = flex == 0 || sum > room;
    let size = |h: f32| match (fit, excess > 0.0) {
        (false, _) => h,
        (true, true) => min + (h - min) / excess * target,
        (true, false) => min + target / k.max(1.0),
    };
    let used: f32 = fixed.iter().map(|&h| size(h)).sum();
    let share = (avail - used).max(0.0) / flex.max(1) as f32;
    entries
        .iter()
        .map(|e| match (e.open, flexible(e)) {
            (false, _) => 0.0,
            (true, true) => share,
            (true, false) => size(e.height.unwrap_or(min).max(min)),
        })
        .collect()
}

/// Drag the gap between the open stacked panels `above` and `below` (laid out at `bodies`) by
/// `d` points: one grows as much as the other shrinks, neither below [`STACK_MIN`]. The new sizes
/// become the panels' heights, except that the last panel without one keeps sharing the rest
/// (so the stack still fills its column when the window is resized).
fn resize_stacked(entries: &mut [StackEntry], bodies: &[f32], above: usize, below: usize, d: f32) {
    let (Some(&a), Some(&b)) = (bodies.get(above), bodies.get(below)) else { return };
    let pair = a + b;
    let lo = STACK_MIN.min(pair / 2.0);
    let na = (a + d).max(lo).min(pair - lo);
    for (i, h) in [(above, na), (below, pair - na)] {
        let last_flexible = entries.iter().filter(|e| flexible(e)).count() == 1;
        if let Some(e) = entries.get_mut(i)
            && !(last_flexible && flexible(e))
        {
            e.height = Some(h);
        }
    }
}

/// Draw a group's frame + tab strip: one tab per panel, or per document for panels that show
/// several (the Timeline's comps). Tab labels may name the comp or layer they show (After
/// Effects style).
pub fn draw_group_chrome(ui: &mut egui::Ui, g: &Group, focused: PanelKind, t: &Tokens, reg: &mut crate::automation::Registry, info: &TabInfo) -> Chrome {
    struct Entry {
        panel: PanelKind,
        doc: Option<u64>,
        label: String,
        deco: Option<TabDeco>,
        /// The group's shown tab.
        active: bool,
        /// The document its panel shows (even behind another tab).
        shown_doc: bool,
    }
    let mut out = Chrome::default();
    let painter = ui.painter().clone();
    painter.rect_filled(g.rect, 0.0, t.panel_bg);
    let active_panel = g.panels.get(g.active).copied();
    let compact = g.panels.len() == 1 && g.panels[0].compact();
    if compact {
        // grip dots
        let c = pos2(g.rect.center().x, g.rect.min.y + 4.0);
        for dx in [-4.0, 0.0, 4.0] {
            painter.circle_filled(c + vec2(dx, 0.0), 1.0, t.text_faint);
        }
    } else {
        let mut entries: Vec<Entry> = vec![];
        for (pi, p) in g.panels.iter().enumerate() {
            let shown = pi == g.active;
            match info.docs.iter().find(|(k, d)| k == p && !d.is_empty()) {
                Some((_, docs)) => entries.extend(docs.iter().map(|d| Entry {
                    panel: *p,
                    doc: Some(d.id),
                    label: d.title.clone(),
                    deco: d.deco,
                    active: shown && d.active,
                    shown_doc: d.active,
                })),
                None => entries.push(Entry {
                    panel: *p,
                    doc: None,
                    label: (info.title)(*p),
                    deco: info.decos.iter().find(|(k, _)| k == p).map(|(_, d)| *d),
                    active: shown,
                    shown_doc: false,
                }),
            }
        }
        let ai = entries.iter().position(|e| e.active).unwrap_or(0);
        let strip = Rect::from_min_size(g.rect.min, vec2(g.rect.width().max(0.0), t.tab_h.max(0.0))).intersect(g.rect);
        if t.gradients {
            // Settings ▸ Appearance ▸ Use Gradients.
            crate::theme::gradient_rect(&painter, strip.shrink2(vec2(t.radius, 0.0)), t.grad_top(t.panel_bg), t.panel_bg);
        }
        let activate = |e: &Entry| match e.doc {
            Some(id) => DockAction::ActivateDoc(e.panel, id),
            None => DockAction::Activate(e.panel),
        };
        let close = |e: &Entry| match e.doc {
            Some(id) => DockAction::CloseDoc(e.panel, id),
            None => DockAction::Close(e.panel),
        };
        let natural_width = |k: usize, e: &Entry| {
            let active = k == ai && g.stacked != Some(false);
            let lock = e.deco.is_some_and(|d| d.viewer) && (e.doc.is_none() || active);
            let decorations = match e.deco {
                Some(d) if d.viewer => (if active { 16.0 } else { 0.0 }) + 14.0 + if lock { 16.0 } else { 0.0 },
                Some(_) => 14.0,
                None => 0.0,
            };
            painter.layout_no_wrap(e.label.clone(), Tokens::ui(12.0), t.tab_text).size().x + 16.0 + if active { 20.0 } else { 0.0 } + decorations
        };
        let widths: Vec<f32> = entries.iter().enumerate().map(|(k, e)| natural_width(k, e)).collect();
        let left_padding = 8.0_f32.min(strip.width().max(0.0));
        let overflow = entries.len() > 1 && widths.iter().sum::<f32>() + 4.0 * entries.len().saturating_sub(1) as f32 > strip.width() - left_padding;
        let overflow_width = if overflow { 20.0_f32.min((strip.width() - left_padding).max(0.0)) } else { 0.0 };
        let tab_right = strip.max.x - overflow_width;
        let available = (tab_right - strip.min.x - left_padding).max(0.0);
        // Slide the visible window so an earlier long caption cannot push the active tab away.
        let mut first = 0;
        let mut through_active = widths.iter().take(ai.saturating_add(1)).sum::<f32>() + 4.0 * ai as f32;
        while first < ai && through_active > available {
            through_active -= widths.get(first).copied().unwrap_or(0.0) + 4.0;
            first += 1;
        }
        let mut x = strip.min.x + left_padding;
        let text_y = strip.center().y;
        for (k, e) in entries.iter().enumerate().skip(first) {
            let p = &e.panel;
            // A collapsed stacked panel's header is plain text (no underline or panel menu).
            let is_active = k == ai && g.stacked != Some(false);
            let menu_w = if is_active { 20.0 } else { 0.0 };
            // The lock belongs to the panel: on its shown document's tab only.
            let lock = e.deco.is_some_and(|d| d.viewer) && (e.doc.is_none() || is_active);
            let desired = widths.get(k).copied().unwrap_or(0.0);
            if x >= tab_right || (k > ai && x + desired > tab_right) {
                break;
            }
            let w = desired.min((tab_right - x).max(0.0));
            let tab = Rect::from_min_size(pos2(x, strip.min.y), vec2(w, strip.height()));
            let painter = painter.with_clip_rect(tab.intersect(ui.clip_rect()));
            match out.tabs.iter_mut().find(|(q, _)| q == p) {
                Some((_, r)) => *r = r.union(tab),
                None => out.tabs.push((*p, tab)),
            }
            let resp = ui.interact(tab, egui::Id::new(("tab", g.path.clone(), k)), Sense::click_and_drag());
            if resp.drag_started() {
                out.actions.push(DockAction::BeginDrag(*p));
            }
            // A document tab also answers to the panel's id while it is the shown one.
            let auto_id = match e.doc {
                Some(id) => format!("panel.tab.{}.{id}", p.id()),
                None => format!("panel.tab.{}", p.id()),
            };
            reg.add(&auto_id, tab, &e.label);
            if e.shown_doc {
                reg.add(&format!("panel.tab.{}", p.id()), tab, &e.label);
            }
            let mut label_x = tab.min.x + 8.0;
            let mut control_under_pointer = false;
            if let Some(d) = e.deco {
                let mut dx = tab.min.x + 6.0;
                if is_active && d.viewer && tab.height() >= 14.0 && dx + 12.0 <= tab.max.x - menu_w {
                    let cr = Rect::from_center_size(pos2(dx + 5.0, text_y), vec2(10.0, 10.0));
                    control_under_pointer |= ui.ctx().pointer_interact_pos().is_some_and(|pos| cr.expand(2.0).intersect(tab).contains(pos));
                    let cresp = ui.interact(cr.expand(2.0).intersect(tab), egui::Id::new(("tab-close", g.path.clone(), k)), Sense::click());
                    let cc = if cresp.hovered() { t.tab_text_active } else { t.tab_text };
                    let kk = 3.5;
                    painter.line_segment([cr.center() + vec2(-kk, -kk), cr.center() + vec2(kk, kk)], Stroke::new(1.2, cc));
                    painter.line_segment([cr.center() + vec2(-kk, kk), cr.center() + vec2(kk, -kk)], Stroke::new(1.2, cc));
                    reg.add(&format!("panel.tab.{}.close", p.id()), cr, "Close");
                    if cresp.clicked() {
                        out.actions.push(close(e));
                    }
                    dx += 16.0;
                }
                let sw = Rect::from_center_size(pos2(dx + 5.0, text_y), vec2(9.0, 9.0));
                painter.rect_filled(sw, 1.0, d.swatch);
                dx += 14.0;
                label_x = dx;
                if lock && tab.height() >= 16.0 && dx + 14.0 <= tab.max.x - menu_w {
                    let lr = Rect::from_center_size(pos2(dx + 6.0, text_y), vec2(12.0, 12.0));
                    control_under_pointer |= ui.ctx().pointer_interact_pos().is_some_and(|pos| lr.expand(2.0).intersect(tab).contains(pos));
                    let lresp = ui.interact(lr.expand(2.0).intersect(tab), egui::Id::new(("tab-lock", g.path.clone(), k)), Sense::click());
                    let lc = if d.locked {
                        t.tab_text_active
                    } else if lresp.hovered() {
                        t.tab_text
                    } else {
                        t.text_faint
                    };
                    icons::paint(&painter, lr, Icon::Lock, lc);
                    reg.add(&format!("panel.tab.{}.lock", p.id()), lr, if d.locked { "Unlock" } else { "Lock" });
                    if lresp.clicked() {
                        out.actions.push(DockAction::ToggleLock(*p));
                    }
                    label_x = dx + 16.0;
                }
            }
            let caption_right = (tab.max.x - menu_w).max(tab.min.x);
            let col = if is_active || resp.hovered() { t.tab_text_active } else { t.tab_text };
            if label_x < caption_right {
                crate::widgets::text_fit(&painter, pos2(label_x, text_y), Align2::LEFT_CENTER, &e.label, Tokens::ui(12.0), caption_right - label_x, col);
            }
            if is_active && tab.width() >= 20.0 && tab.height() >= 10.0 {
                let mr = Rect::from_center_size(pos2(tab.max.x - 10.0, text_y), vec2(12.0, 10.0));
                control_under_pointer |= ui.ctx().pointer_interact_pos().is_some_and(|pos| mr.expand(3.0).intersect(tab).contains(pos));
                let mresp = ui.interact(mr.expand(3.0).intersect(tab), egui::Id::new(("tab-menu", g.path.clone())), Sense::click());
                reg.add(&format!("panel.menu.{}", p.id()), mr, "panel menu");
                let mc = if mresp.hovered() { t.tab_text_active } else { t.tab_text };
                for dy in [-3.5, 0.0, 3.5] {
                    painter.line_segment([pos2(mr.min.x, mr.center().y + dy), pos2(mr.max.x, mr.center().y + dy)], Stroke::new(1.5, mc));
                }
                let uy = strip.max.y - 3.0;
                painter.line_segment([pos2(label_x.min(mr.max.x), uy), pos2(mr.max.x, uy)], Stroke::new(1.0, t.tab_text_active));
                if mresp.clicked() {
                    out.actions.push(DockAction::PanelMenu(*p, mr.left_bottom()));
                }
            }
            if resp.double_clicked() && is_active && g.stacked.is_none() && !control_under_pointer {
                out.actions.push(DockAction::ToggleMaximize(*p));
                out.actions.push(DockAction::Focus(*p));
            } else if resp.clicked() {
                if g.stacked.is_some() {
                    out.actions.push(DockAction::ToggleStacked(*p));
                } else {
                    out.actions.push(activate(e));
                }
                out.actions.push(DockAction::Focus(*p));
            }
            if resp.middle_clicked() {
                out.actions.push(close(e));
            }
            x += w + 4.0;
        }
        if overflow && overflow_width > 0.0 && strip.height() > 0.0 {
            let r = Rect::from_min_max(pos2(tab_right, strip.min.y), strip.max);
            let resp = ui.interact(r, egui::Id::new(("tab-overflow", g.path.clone())), Sense::click());
            let p = painter.with_clip_rect(r.intersect(ui.clip_rect()));
            icons::paint(&p, r.shrink(4.0), Icon::ChevronRight, t.tab_text);
            if let Some(panel) = active_panel {
                reg.add(&format!("panel.tabOverflow.{}", panel.id()), r, "Next hidden tab");
            }
            if resp.clicked()
                && let Some(next) = entries.get((ai + 1) % entries.len())
            {
                out.actions.push(activate(next));
            }
            x = strip.max.x;
        }
        // The unused tab well has the same maximize/restore gesture as the active caption.
        let well = Rect::from_min_max(pos2(x.min(strip.max.x), strip.min.y), strip.max);
        if well.width() > 0.0
            && let Some(panel) = active_panel
        {
            let response = ui.interact(well, egui::Id::new(("tab-well", g.path.clone())), Sense::click());
            reg.add(&format!("panel.tabWell.{}", panel.id()), well, "Empty panel tab area");
            if response.double_clicked() {
                out.actions.push(DockAction::ToggleMaximize(panel));
                out.actions.push(DockAction::Focus(panel));
            }
        }
    }
    // focus outline
    if active_panel == Some(focused) {
        painter.rect_stroke(g.rect, 0.0, Stroke::new(1.0, t.focus), StrokeKind::Inside);
    }
    // clicking anywhere in the panel focuses it
    if let Some(p) = active_panel
        && ui.rect_contains_pointer(g.rect)
        && ui.input(|i| i.pointer.any_pressed())
    {
        out.actions.push(DockAction::Focus(p));
    }
    out
}

/// Placeholder body for panels that are not implemented yet.
pub fn placeholder(ui: &mut egui::Ui, rect: Rect, t: &Tokens, text: &str) {
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, text, Tokens::ui(12.0), t.text_faint);
}

#[cfg(test)]
#[path = "dock_split_safety_tests.rs"]
mod split_safety_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Color32;

    struct LongTabFixture {
        ctx: egui::Context,
        tokens: Tokens,
        group: Group,
        registry: crate::automation::Registry,
        time: f64,
    }
    impl LongTabFixture {
        fn new(width: f32, active: usize) -> Self {
            let ctx = egui::Context::default();
            let tokens = Tokens::for_kind(crate::theme::ThemeKind::Dark);
            crate::theme::install(&ctx, &tokens);
            let rect = Rect::from_min_size(pos2(20.0, 20.0), vec2(width, 200.0));
            Self {
                ctx,
                tokens,
                group: Group {
                    path: "long-tab-bounds".into(),
                    rect,
                    content: rect,
                    panels: vec![PanelKind::Composition, PanelKind::Layer],
                    active,
                    stacked: None,
                },
                registry: Default::default(),
                time: 0.0,
            }
        }
        fn frame(&mut self, events: Vec<egui::Event>) -> (Chrome, bool) {
            self.time += 0.02;
            self.registry.begin_frame();
            let title = |_: PanelKind| "Original synthetic very long composition caption ".repeat(20);
            let decos = [PanelKind::Composition, PanelKind::Layer].map(|panel| (panel, TabDeco { swatch: Color32::GRAY, locked: false, viewer: true }));
            let info = TabInfo { title: &title, decos: &decos, docs: &[] };
            let mut chrome = Chrome::default();
            let mut adjacent_clicked = false;
            let output = self.ctx.run_ui(egui::RawInput { time: Some(self.time), events, ..Default::default() }, |ui| {
                let adjacent = Rect::from_min_size(pos2(self.group.rect.max.x + 2.0, self.group.rect.min.y), vec2(100.0, self.tokens.tab_h));
                adjacent_clicked = ui.interact(adjacent, egui::Id::new("adjacent-group-hit"), Sense::click()).clicked();
                chrome = draw_group_chrome(ui, &self.group, PanelKind::Composition, &self.tokens, &mut self.registry, &info);
            });
            let strip = Rect::from_min_size(self.group.rect.min, vec2(self.group.rect.width(), self.tokens.tab_h));
            for shape in &output.shapes {
                if matches!(shape.shape, egui::epaint::Shape::Text(_)) {
                    assert!(strip.contains_rect(shape.clip_rect), "caption paint escapes the header clip");
                }
            }
            output.drop_without_applying_deltas();
            for element in &self.registry.elements {
                let rect = Rect::from_min_size(pos2(element.rect[0], element.rect[1]), vec2(element.rect[2], element.rect[3]));
                assert!(strip.contains_rect(rect), "{} escapes header: {rect:?}", element.id);
            }
            for (_, rect) in &chrome.tabs {
                assert!(strip.contains_rect(*rect), "tab hit rectangle escapes header");
            }
            (chrome, adjacent_clicked)
        }
        fn click(&mut self, pos: egui::Pos2) -> (Chrome, bool) {
            self.frame(vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE },
            ]);
            self.frame(vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE },
            ])
        }
        fn center(&self, id: &str) -> egui::Pos2 {
            let e = self.registry.elements.iter().find(|e| e.id == id).unwrap();
            pos2(e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0)
        }
    }

    #[test]
    fn narrow_long_active_and_earlier_tabs_keep_controls_and_overflow_reachable() {
        for active in [0, 1] {
            for width in [100.0, 180.0] {
                let mut fixture = LongTabFixture::new(width, active);
                fixture.frame(vec![]);
                fixture.frame(vec![]);
                let panel = fixture.group.panels[active];
                for suffix in ["close", "lock", "menu"] {
                    let id = if suffix == "menu" { format!("panel.menu.{}", panel.id()) } else { format!("panel.tab.{}.{suffix}", panel.id()) };
                    let pos = fixture.center(&id);
                    let (result, _) = fixture.click(pos);
                    assert!(
                        result.actions.iter().any(|action| match suffix {
                            "close" => *action == DockAction::Close(panel),
                            "lock" => *action == DockAction::ToggleLock(panel),
                            _ => matches!(action, DockAction::PanelMenu(p, _) if *p == panel),
                        }),
                        "{suffix} lost its clickable target"
                    );
                    assert!(!result.actions.iter().any(|a| matches!(a, DockAction::ToggleMaximize(_))));
                }
                let pos = fixture.center(&format!("panel.tabOverflow.{}", panel.id()));
                let (result, _) = fixture.click(pos);
                let next = fixture.group.panels[(active + 1) % 2];
                assert!(result.actions.contains(&DockAction::Activate(next)), "hidden tabs must remain reachable");
            }
        }
    }

    #[test]
    fn long_tab_does_not_steal_clicks_from_an_adjacent_group() {
        for active in [0, 1] {
            let mut fixture = LongTabFixture::new(100.0, active);
            fixture.frame(vec![]);
            fixture.frame(vec![]);
            let pos = pos2(fixture.group.rect.max.x + 30.0, fixture.group.rect.min.y + fixture.tokens.tab_h / 2.0);
            let (result, adjacent) = fixture.click(pos);
            assert!(adjacent, "long caption stole neighboring group's pointer target");
            assert!(result.actions.is_empty(), "outside click activated a tab/control: {:?}", result.actions);
        }
    }

    #[test]
    fn very_small_long_tab_headers_never_register_escaped_targets() {
        for width in [0.0, 8.0, 24.0, 60.0] {
            let mut fixture = LongTabFixture::new(width, 0);
            fixture.frame(vec![]);
            fixture.frame(vec![]);
        }
    }
    #[test]
    fn compact_tab_controls_stay_inside_the_header_and_remain_clickable() {
        let ctx = egui::Context::default();
        let mut tokens = Tokens::for_kind(crate::theme::ThemeKind::Dark);
        tokens.tab_h = 26.0;
        let rect = Rect::from_min_size(pos2(20.0, 20.0), vec2(480.0, 200.0));
        let group = Group {
            path: "fixture".to_string(),
            rect,
            content: Rect::from_min_max(pos2(rect.min.x, rect.min.y + tokens.tab_h), rect.max),
            panels: vec![PanelKind::Composition],
            active: 0,
            stacked: None,
        };
        let title = |_: PanelKind| "Composition Original Fixture".to_string();
        let decos = [(PanelKind::Composition, TabDeco { swatch: Color32::GRAY, locked: false, viewer: true })];
        let info = TabInfo { title: &title, decos: &decos, docs: &[] };
        let mut reg = crate::automation::Registry::default();
        let mut actions = Vec::new();
        let mut draw = |events| {
            reg.begin_frame();
            actions.clear();
            ctx.run_ui(egui::RawInput { events, ..Default::default() }, |ui| {
                actions = draw_group_chrome(ui, &group, PanelKind::Composition, &tokens, &mut reg, &info).actions;
            })
            .drop_without_applying_deltas();
            (reg.elements.clone(), actions.clone())
        };
        let (controls, _) = draw(vec![]);
        let header = Rect::from_min_size(rect.min, vec2(rect.width(), tokens.tab_h));
        for id in ["panel.tab.Composition.close", "panel.tab.Composition.lock", "panel.menu.Composition"] {
            let control = controls.iter().find(|e| e.id == id).unwrap();
            let r = Rect::from_min_size(pos2(control.rect[0], control.rect[1]), vec2(control.rect[2], control.rect[3]));
            assert!(header.contains_rect(r), "{id} escapes compact header: {r:?}");
            assert!((r.center().y - header.center().y).abs() < 0.01, "{id} is off-center");
        }
        let menu = controls.iter().find(|e| e.id == "panel.menu.Composition").unwrap();
        let pos = pos2(menu.rect[0] + menu.rect[2] / 2.0, menu.rect[1] + menu.rect[3] / 2.0);
        draw(vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE },
        ]);
        let (_, actions) =
            draw(vec![egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE }]);
        assert!(actions.iter().any(|a| matches!(a, DockAction::PanelMenu(PanelKind::Composition, _))));
    }

    #[test]
    fn viewer_tab_close_lock_and_menu_double_click_do_not_maximize() {
        use egui::{Event, Modifiers, PointerButton};
        for control in ["close", "lock", "menu"] {
            let ctx = egui::Context::default();
            let tokens = Tokens::for_kind(crate::theme::ThemeKind::Dark);
            crate::theme::install(&ctx, &tokens);
            let mut registry = crate::automation::Registry::default();
            let group = Group {
                path: "viewer-control-test".into(),
                rect: Rect::from_min_size(pos2(20.0, 20.0), vec2(700.0, 300.0)),
                content: Rect::from_min_size(pos2(20.0, 50.0), vec2(700.0, 270.0)),
                panels: vec![PanelKind::Composition, PanelKind::Layer],
                active: 0,
                stacked: None,
            };
            let title = |panel: PanelKind| panel.title().to_string();
            let decos = [(PanelKind::Composition, TabDeco { swatch: Color32::RED, locked: false, viewer: true })];
            let info = TabInfo { title: &title, decos: &decos, docs: &[] };
            let mut actions = Vec::new();
            registry.begin_frame();
            ctx.run_ui(egui::RawInput { time: Some(0.0), ..Default::default() }, |ui| {
                actions.extend(draw_group_chrome(ui, &group, PanelKind::Composition, &tokens, &mut registry, &info).actions);
            })
            .drop_without_applying_deltas();
            // Use the actual decorated-control geometry, not the caption's overlapping rect.
            let id = if control == "menu" { "panel.menu.Composition".to_string() } else { format!("panel.tab.Composition.{control}") };
            let element = registry.elements.iter().find(|e| e.id == id).unwrap();
            let pos = pos2(element.rect[0] + element.rect[2] / 2.0, element.rect[1] + element.rect[3] / 2.0);
            for (index, pressed) in [true, false, true, false].into_iter().enumerate() {
                registry.begin_frame();
                ctx.run_ui(
                    egui::RawInput {
                        time: Some(1.0 + index as f64 * 0.02),
                        events: vec![
                            Event::PointerMoved(pos),
                            Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE },
                        ],
                        ..Default::default()
                    },
                    |ui| {
                        actions.extend(draw_group_chrome(ui, &group, PanelKind::Composition, &tokens, &mut registry, &info).actions);
                    },
                )
                .drop_without_applying_deltas();
            }
            assert!(!actions.iter().any(|a| matches!(a, DockAction::ToggleMaximize(_))), "{control}");
            assert!(
                actions.iter().any(|a| match control {
                    "close" => matches!(a, DockAction::Close(PanelKind::Composition)),
                    "lock" => matches!(a, DockAction::ToggleLock(PanelKind::Composition)),
                    _ => matches!(a, DockAction::PanelMenu(PanelKind::Composition, _)),
                }),
                "{control} must retain its original action"
            );
        }
    }

    #[test]
    fn stacked_panels_toggle_open_close() {
        use PanelKind::*;
        let mut d = stack(&[(Preview, true, Some(46.0)), (Properties, true, None), (Align, false, None)]);
        assert!(d.is_visible(Properties) && !d.is_visible(Align));
        assert!(d.toggle_stacked(Align));
        assert!(d.is_visible(Align));
        assert!(d.toggle_stacked(Align) && !d.is_visible(Align));
        // Showing a collapsed panel expands it; opening a new panel near one inserts after it.
        assert!(d.activate(Align) && d.is_visible(Align));
        d.open_near(Info, Properties);
        let mut v = vec![];
        d.panels(&mut v);
        assert_eq!(v, [Preview, Properties, Info, Align]);
        d.close(Preview);
        assert!(!d.contains(Preview));
        assert!(!d.toggle_stacked(Timeline));
    }

    fn entry(panel: PanelKind, open: bool, height: Option<f32>) -> StackEntry {
        StackEntry { panel, open, height }
    }

    fn close_to(a: &[f32], b: &[f32]) -> bool {
        a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-3)
    }

    #[test]
    fn stacked_panels_fill_their_column() {
        use PanelKind::*;
        // A panel with a height keeps it; the open ones without share the rest.
        let s = [entry(Preview, true, Some(100.0)), entry(Properties, true, None), entry(Align, false, None), entry(Audio, true, None)];
        assert!(close_to(&stack_bodies(&s, 500.0), &[100.0, 200.0, 0.0, 200.0]));
        // Too little room: the heights shrink so the others keep their minimum.
        let s = [entry(Preview, true, Some(500.0)), entry(Properties, true, None)];
        assert!(close_to(&stack_bodies(&s, 300.0), &[260.0, STACK_MIN]));
        // Nothing to take the rest: the heights grow in proportion (above the minimum) to fill.
        let s = [entry(Preview, true, Some(100.0)), entry(Properties, true, Some(300.0)), entry(Align, false, Some(80.0))];
        assert!(close_to(&stack_bodies(&s, 600.0), &[137.5, 462.5, 0.0]));
        let s = [entry(Preview, true, Some(46.0)), entry(Properties, false, None)];
        assert!(close_to(&stack_bodies(&s, 600.0), &[600.0, 0.0]));
        // Hostile sizes never make a body negative or non-finite.
        for avail in [-50.0, 0.0, 10.0, f32::NAN, f32::INFINITY] {
            for h in [None, Some(-5.0), Some(f32::NAN), Some(f32::INFINITY), Some(1e30), Some(f32::MAX)] {
                let s = [entry(Preview, true, h), entry(Properties, true, Some(60.0)), entry(Align, false, h)];
                let b = stack_bodies(&s, avail);
                assert!(b.iter().all(|x| x.is_finite() && *x >= 0.0), "{avail} {h:?}: {b:?}");
            }
        }
        assert!(stack_bodies(&[], 100.0).is_empty());
    }

    #[test]
    fn dragging_a_stack_gap_trades_height_between_its_neighbours() {
        use PanelKind::*;
        let mut s = vec![entry(Preview, true, Some(46.0)), entry(Properties, true, None), entry(Align, false, None)];
        let bodies = stack_bodies(&s, 800.0);
        resize_stacked(&mut s, &bodies, 0, 1, 100.0);
        // Preview keeps its new height; Properties, the only panel without one, takes the rest.
        assert_eq!(s[0].height, Some(146.0));
        assert_eq!(s[1].height, None);
        assert!(close_to(&stack_bodies(&s, 800.0), &[146.0, 654.0, 0.0]));
        // Neither side goes below the minimum.
        for (d, want) in [(-1000.0, STACK_MIN), (1000.0, 800.0 - STACK_MIN)] {
            let bodies = stack_bodies(&s, 800.0);
            resize_stacked(&mut s, &bodies, 0, 1, d);
            assert!(close_to(&stack_bodies(&s, 800.0)[..1], &[want]), "{d}");
        }
        // Two sharing panels: the upper one gets a height, the lower one keeps sharing.
        let mut s = vec![entry(Preview, true, None), entry(Align, false, None), entry(Properties, true, None)];
        let bodies = stack_bodies(&s, 400.0);
        resize_stacked(&mut s, &bodies, 0, 2, 50.0);
        assert_eq!((s[0].height, s[2].height), (Some(250.0), None));
        assert!(close_to(&stack_bodies(&s, 400.0), &[250.0, 0.0, 150.0]));
        // Out-of-range indices change nothing.
        let before = s.clone();
        resize_stacked(&mut s, &bodies, 0, 9, 50.0);
        assert_eq!(s, before);
    }

    #[test]
    fn default_workspace_has_ae_right_column_stack() {
        let d = workspace("Default");
        assert!(d.is_visible(PanelKind::Properties) && d.is_visible(PanelKind::Preview));
        assert!(d.contains(PanelKind::EffectsPresets) && !d.is_visible(PanelKind::EffectsPresets));
    }

    #[test]
    fn workspaces_contain_core_panels() {
        for w in WORKSPACES {
            let d = workspace(w);
            let floating: Vec<PanelKind> = workspace_floating(w).into_iter().flat_map(|f| f.panels).collect();
            assert!(d.contains(PanelKind::Timeline) || floating.contains(&PanelKind::Timeline), "{w}");
            assert!(d.contains(PanelKind::Composition) || d.contains(PanelKind::Layer), "{w}");
            // A panel is in one place only.
            let mut all = vec![];
            d.panels(&mut all);
            all.extend(floating);
            let n = all.len();
            all.sort_by_key(|p| p.id());
            all.dedup();
            assert_eq!(all.len(), n, "{w} has a panel twice");
        }
    }

    #[test]
    fn after_effects_workspaces_exist() {
        // Window ▸ Workspace in After Effects 2026, less Libraries (an Adobe service).
        for w in [
            "Default",
            "Review",
            "Learn",
            "Small Screen",
            "Standard",
            "All Panels",
            "Animation",
            "Essential Graphics",
            "Color",
            "Effects",
            "Minimal",
            "Motion Tracking",
            "Paint",
            "Text",
            "Undocked Panels",
        ] {
            assert!(WORKSPACES.contains(&w), "{w}");
        }
        assert!(!WORKSPACES.contains(&"Libraries"));
        // Layouts differ from Default (except Learn, which is Default plus the Home screen).
        let def = workspace("Default");
        for w in WORKSPACES.iter().filter(|w| !matches!(**w, "Default" | "Learn")) {
            assert_ne!(workspace(w), def, "{w} falls back to Default");
        }
        assert!(workspace("Essential Graphics").contains(PanelKind::Properties));
        assert!(workspace("Color").contains(PanelKind::EffectControls));
        assert!(!workspace_floating("Undocked Panels").is_empty());
        // Every built-in workspace is in Window ▸ Workspace.
        let menu: Vec<String> = effectcraft_engine::menus::entries()
            .into_iter()
            .filter(|(_, e)| e.command == "window.workspace")
            .filter_map(|(_, e)| e.params.get("name").and_then(|v| v.as_str()).map(str::to_string))
            .collect();
        for w in WORKSPACES {
            assert!(menu.iter().any(|m| m == w), "{w} missing from Window > Workspace");
        }
    }

    #[test]
    fn drop_zone_math() {
        let r = Rect::from_min_size(pos2(100.0, 100.0), vec2(400.0, 300.0));
        let tab = 28.0;
        assert_eq!(drop_zone(r, tab, pos2(300.0, 110.0)), Zone::Center, "tab strip");
        assert_eq!(drop_zone(r, tab, r.center()), Zone::Center);
        assert_eq!(drop_zone(r, tab, pos2(120.0, 250.0)), Zone::Left);
        assert_eq!(drop_zone(r, tab, pos2(490.0, 250.0)), Zone::Right);
        assert_eq!(drop_zone(r, tab, pos2(300.0, 140.0)), Zone::Top);
        assert_eq!(drop_zone(r, tab, pos2(300.0, 390.0)), Zone::Bottom);
        // Corners go to the nearer edge.
        assert_eq!(drop_zone(r, tab, pos2(105.0, 395.0)), Zone::Left);
        assert_eq!(zone_rect(r, Zone::Right), Rect::from_min_max(pos2(300.0, 100.0), r.max));
        assert_eq!(zone_rect(r, Zone::Top).height(), 150.0);
        assert_eq!(zone_rect(r, Zone::Center), r);
    }

    #[test]
    fn dock_panel_splits_and_inserts_tabs() {
        use PanelKind::*;
        let mut d = workspace("Standard");
        let n = d.panel_count();
        // Tab next to an anchor.
        assert!(d.dock_panel(Info, Timeline, Zone::Center));
        assert_eq!(d.panel_count(), n);
        let p = d.path_of(Timeline).unwrap();
        assert_eq!(d.path_of(Info).unwrap(), p);
        assert!(d.is_visible(Info));
        // Split to the right of the Composition group: a new single-tab group beside it.
        assert!(d.dock_panel(Info, Composition, Zone::Right));
        let comp_path = d.path_of(Composition).unwrap();
        let info_path = d.path_of(Info).unwrap();
        assert_eq!(comp_path.len(), info_path.len());
        assert!(comp_path.ends_with('a') && info_path.ends_with('b'));
        assert!(d.is_visible(Info));
        // Top: the new group comes first in a vertical split.
        assert!(d.dock_panel(Audio, Timeline, Zone::Top));
        assert!(d.path_of(Audio).unwrap().ends_with('a'));
        // Dropping a panel on itself or an undocked anchor changes nothing.
        let before = d.clone();
        assert!(!d.dock_panel(Audio, Audio, Zone::Left));
        assert!(!d.dock_panel(Audio, Tracker, Zone::Left));
        assert_eq!(d, before);
        // Into a stack.
        let mut s = workspace("Default");
        assert!(s.dock_panel(Tracker, Properties, Zone::Center));
        assert!(s.path_of(Tracker).unwrap().contains('s'));
        // Moving the only panel of a group collapses its split.
        let mut m = hsplit(SplitSize::Ratio(0.5), tabs(&[Project], 0), tabs(&[Composition], 0));
        assert!(m.dock_panel(Project, Composition, Zone::Center));
        assert_eq!(m, tabs(&[Composition, Project], 1));
    }

    #[test]
    fn floating_layout_round_trips() {
        use PanelKind::*;
        let mut l = Layout { root: workspace("Standard"), floating: vec![] };
        assert!(l.float(Info, [200.0, 150.0, 320.0, 240.0]));
        assert!(!l.root.contains(Info) && l.contains(Info));
        // Tab another panel into the floating group, then dock it back.
        assert!(l.dock(Audio, Info, Zone::Center));
        assert_eq!(l.floating[0].panels, [Info, Audio]);
        assert!(!l.dock(Audio, Info, Zone::Left), "floating groups only take tabs");
        assert!(l.dock(Info, Timeline, Zone::Bottom));
        assert_eq!(l.floating[0].panels, [Audio]);
        assert!(l.root.contains(Info));
        let s = serde_json::to_string(&l).unwrap();
        let back: Layout = serde_json::from_str(&s).unwrap();
        assert_eq!(back, l);
        // A workspace saved before floating panels existed still loads.
        let old: Layout = serde_json::from_str(&format!("{{\"root\": {}}}", serde_json::to_string(&l.root).unwrap())).unwrap();
        assert!(old.floating.is_empty());
        // The last docked panel can't float.
        let mut one = Layout { root: tabs(&[Timeline], 0), floating: vec![] };
        assert!(!one.float(Timeline, [0.0; 4]));
        assert!(l.unfloat(Audio) && l.floating.is_empty());
    }

    #[test]
    fn close_and_open() {
        let mut d = workspace("Default");
        d.close(PanelKind::Paragraph);
        assert!(!d.contains(PanelKind::Paragraph));
        d.open_near(PanelKind::Tracker, PanelKind::EffectsPresets);
        assert!(d.is_visible(PanelKind::Tracker));
        let s = serde_json::to_string(&d).unwrap();
        let back: DockNode = serde_json::from_str(&s).unwrap();
        assert_eq!(back, d);
        assert_eq!(PanelKind::from_name("effects & presets"), Some(PanelKind::EffectsPresets));
    }

    #[test]
    fn closing_docked_tabs_keeps_the_active_panel_and_selects_a_neighbor_when_removed() {
        use PanelKind::*;
        for (active, closed, visible) in [
            (1, Project, EffectControls),
            (1, History, EffectControls),
            (1, EffectControls, History),
            (2, History, EffectControls),
            (usize::MAX, Project, History),
        ] {
            let mut d = hsplit(SplitSize::FixedA(300.0), tabs(&[Project, EffectControls, History], active), tabs(&[Composition], 0));
            d.close(closed);
            assert!(d.is_visible(visible), "closing {closed:?} with index {active} must show {visible:?}");
            assert!(!d.contains(closed));
            assert!(d.is_visible(Composition));
            let encoded = serde_json::to_string(&d).unwrap();
            assert_eq!(serde_json::from_str::<DockNode>(&encoded).unwrap(), d);
        }
        let mut only = tabs(&[Project], usize::MAX);
        only.close(Project);
        assert_eq!(only, tabs(&[], 0));
    }

    #[test]
    fn removing_floating_tabs_keeps_the_active_panel_and_preserves_other_groups() {
        use PanelKind::*;
        let untouched = Floating { panels: vec![Info, Audio], active: 1, rect: [500.0, 80.0, 300.0, 200.0] };
        for (active, removed, visible) in [
            (1, Project, EffectControls),
            (1, History, EffectControls),
            (1, EffectControls, History),
            (2, History, EffectControls),
            (usize::MAX, Project, History),
        ] {
            let root = tabs(&[Composition], 0);
            let rect = [40.0, 60.0, 320.0, 240.0];
            let mut layout =
                Layout { root: root.clone(), floating: vec![Floating { panels: vec![Project, EffectControls, History], active, rect }, untouched.clone()] };
            assert!(layout.unfloat(removed));
            let group = &layout.floating[0];
            assert_eq!(group.panels.get(group.active), Some(&visible), "removing {removed:?} with index {active} must show {visible:?}");
            assert!(!group.panels.contains(&removed));
            assert_eq!(group.rect, rect);
            assert_eq!(layout.floating[1], untouched);
            assert_eq!(layout.root, root);
            let before = layout.clone();
            assert!(!layout.unfloat(Tracker));
            assert_eq!(layout, before);
        }
        let mut only = Layout {
            root: tabs(&[Composition], 0),
            floating: vec![Floating { panels: vec![Project], active: usize::MAX, rect: [0.0, 0.0, 100.0, 100.0] }, untouched.clone()],
        };
        assert!(only.unfloat(Project));
        assert_eq!(only.floating, vec![untouched]);
    }
}
