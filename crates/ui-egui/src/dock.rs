//! Docking: a tree of splits and tab groups. Panels are rounded frames separated by
//! thin gutters; each group has a tab strip (active tab bright, with a panel menu "≡"); the focused
//! panel gets a blue outline. Gutters drag to resize; workspaces are serialized trees.

use egui::{Align2, Rect, Sense, Stroke, pos2, vec2};
use serde::{Deserialize, Serialize};

use craft_ui::layout::{ResizeHandle, ResizeHandleStyle, SplitAxis, SplitSize as SharedSplitSize, split_rect};

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

impl SplitSize {
    fn shared(self) -> SharedSplitSize {
        match self {
            Self::Ratio(value) => SharedSplitSize::Ratio(value),
            Self::FixedA(value) => SharedSplitSize::FixedFirst(value),
            Self::FixedB(value) => SharedSplitSize::FixedSecond(value),
        }
    }
}

/// One frame in a [`DockNode::Stack`]: a header with one tab per panel (a panel dropped onto a
/// stacked panel joins its frame as a tab, as in After Effects).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(from = "StackEntryRepr")]
pub struct StackEntry {
    pub panels: Vec<PanelKind>,
    /// The shown tab.
    pub active: usize,
    pub open: bool,
    /// Content height when open (set by dragging the gaps); `None` = share the remaining height.
    pub height: Option<f32>,
}

/// A saved [`StackEntry`]: workspaces saved before stacked frames held tabs name one `panel`.
#[derive(Deserialize)]
struct StackEntryRepr {
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

impl From<StackEntryRepr> for StackEntry {
    fn from(r: StackEntryRepr) -> StackEntry {
        let mut panels = r.panels;
        if let Some(p) = r.panel
            && !panels.contains(&p)
        {
            panels.insert(0, p);
        }
        let active = r.active.min(panels.len().saturating_sub(1));
        StackEntry { panels, active, open: r.open, height: r.height }
    }
}

impl StackEntry {
    /// A frame showing just `panel`.
    pub fn new(panel: PanelKind, open: bool, height: Option<f32>) -> StackEntry {
        StackEntry { panels: vec![panel], active: 0, open, height }
    }
    pub fn contains(&self, p: PanelKind) -> bool {
        self.panels.contains(&p)
    }
    /// The frame's shown panel.
    pub fn shown(&self) -> Option<PanelKind> {
        self.panels.get(self.active).or(self.panels.first()).copied()
    }
    /// Show `p` (one of its tabs).
    fn show(&mut self, p: PanelKind) {
        if let Some(i) = self.panels.iter().position(|x| *x == p) {
            self.active = i;
        }
    }
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
    DockNode::Stack { entries: e.iter().map(|&(panel, open, height)| StackEntry::new(panel, open, height)).collect() }
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
    /// Every panel in the tree.
    pub fn panels(&self, out: &mut Vec<PanelKind>) {
        match self {
            DockNode::Split { a, b, .. } => {
                a.panels(out);
                b.panels(out);
            }
            DockNode::Tabs { panels, .. } => out.extend(panels.iter().copied()),
            DockNode::Stack { entries } => out.extend(entries.iter().flat_map(|e| e.panels.iter().copied())),
        }
    }
    pub fn contains(&self, p: PanelKind) -> bool {
        let mut v = Vec::new();
        self.panels(&mut v);
        v.contains(&p)
    }
    /// The tabs of the group `p` is in, in order.
    pub fn group_of(&self, p: PanelKind) -> Option<&[PanelKind]> {
        match self {
            DockNode::Split { a, b, .. } => a.group_of(p).or_else(|| b.group_of(p)),
            DockNode::Tabs { panels, .. } => panels.contains(&p).then_some(panels.as_slice()),
            DockNode::Stack { entries } => entries.iter().find(|e| e.contains(p)).map(|e| e.panels.as_slice()),
        }
    }
    /// Make `p` the active tab of its group. Returns false if not present.
    pub fn activate(&mut self, p: PanelKind) -> bool {
        if !crate::dock_shared::valid_tree(self) {
            return false;
        }
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
            DockNode::Stack { entries } => match entries.iter_mut().find(|e| e.contains(p)) {
                Some(e) => {
                    e.show(p);
                    e.open = true;
                    true
                }
                None => false,
            },
        }
    }
    /// Expand or collapse a stacked panel's frame (showing `p`). Returns false if `p` is not in
    /// a stack.
    pub fn toggle_stacked(&mut self, p: PanelKind) -> bool {
        if !crate::dock_shared::valid_tree(self) {
            return false;
        }
        match self {
            DockNode::Split { a, b, .. } => a.toggle_stacked(p) || b.toggle_stacked(p),
            DockNode::Tabs { .. } => false,
            DockNode::Stack { entries } => match entries.iter_mut().find(|e| e.contains(p)) {
                Some(e) => {
                    e.show(p);
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
            DockNode::Stack { entries } => entries.iter().any(|e| e.open && e.shown() == Some(p)),
        }
    }
    /// Close a panel (remove its tab; empty groups collapse their split).
    pub fn close(&mut self, p: PanelKind) {
        if !crate::dock_shared::valid_for_close(self) {
            return;
        }
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
            *active = shown.and_then(|shown| panels.iter().position(|x| *x == shown)).unwrap_or((*active).min(panels.len().saturating_sub(1)));
        } else if let DockNode::Stack { entries } = self {
            for e in entries.iter_mut() {
                let shown = e.shown();
                e.panels.retain(|x| *x != p);
                e.active = shown.and_then(|shown| e.panels.iter().position(|x| *x == shown)).unwrap_or(e.active.min(e.panels.len().saturating_sub(1)));
            }
            entries.retain(|e| !e.panels.is_empty());
        }
    }
    /// Add a panel as a tab next to `near` (or into the first group).
    pub fn open_near(&mut self, p: PanelKind, near: PanelKind) {
        if !crate::dock_shared::valid_tree(self) {
            return;
        }
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
                DockNode::Stack { entries } => match entries.iter().position(|e| e.contains(near)) {
                    Some(i) => {
                        entries.insert(i + 1, StackEntry::new(p, true, None));
                        true
                    }
                    None => false,
                },
            }
        }
        fn add_first(n: &mut DockNode, p: PanelKind) -> bool {
            match n {
                DockNode::Split { a, b, .. } => add_first(a, p) || add_first(b, p),
                DockNode::Tabs { panels, active } if !panels.is_empty() => {
                    panels.push(p);
                    *active = panels.len() - 1;
                    true
                }
                DockNode::Stack { entries } if !entries.is_empty() => {
                    entries.push(StackEntry::new(p, true, None));
                    true
                }
                _ => false,
            }
        }
        if self.panel_count() == 0 {
            // Every panel was closed: there is no anchor, so the panel becomes the whole dock.
            *self = tabs(&[p], 0);
        } else if !add(self, p, near) && !add(self, p, PanelKind::EffectsPresets) && !add(self, p, PanelKind::Project) {
            // None of the anchors is docked (e.g. Minimal with Timeline closed): use the first group.
            add_first(self, p);
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
    pub(crate) fn shared(self) -> craft_ui::docking::Zone {
        match self {
            Self::Center => craft_ui::docking::Zone::Center,
            Self::Left => craft_ui::docking::Zone::Left,
            Self::Right => craft_ui::docking::Zone::Right,
            Self::Top => craft_ui::docking::Zone::Top,
            Self::Bottom => craft_ui::docking::Zone::Bottom,
        }
    }

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
    match craft_ui::docking::drop_zone(rect, tab_h, pos) {
        Some(craft_ui::docking::Zone::Left) => Zone::Left,
        Some(craft_ui::docking::Zone::Right) => Zone::Right,
        Some(craft_ui::docking::Zone::Top) => Zone::Top,
        Some(craft_ui::docking::Zone::Bottom) => Zone::Bottom,
        _ => Zone::Center,
    }
}

/// The shared preview region, or an empty rectangle for invalid geometry.
pub fn zone_rect(rect: Rect, zone: Zone) -> Rect {
    craft_ui::docking::zone_rect(rect, zone.shared()).unwrap_or(Rect::NOTHING)
}

/// A panel group floating over the dock (undocked).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Floating {
    pub panels: Vec<PanelKind>,
    pub active: usize,
    /// [x, y, w, h] in points.
    pub rect: [f32; 4],
}

/// A floating group moved out of the main window into its own native window (which can sit on
/// another monitor): the group holding `panel` shows there.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PanelWindow {
    pub panel: PanelKind,
    /// The window's [x, y] outer position and [w, h] inner size, in desktop points.
    pub rect: [f32; 4],
}

/// The smallest and largest native panel window, in points.
pub const PANEL_WINDOW_MIN: [f32; 2] = [160.0, 100.0];
pub const PANEL_WINDOW_MAX: f32 = 16_384.0;
/// The farthest a native panel window may sit from the desktop origin, in points.
pub const PANEL_WINDOW_MAX_POS: f32 = 100_000.0;

impl PanelWindow {
    /// The rect made safe to hand to the windowing system: finite, with a size within
    /// [`PANEL_WINDOW_MIN`]..=[`PANEL_WINDOW_MAX`] and a position within
    /// ±[`PANEL_WINDOW_MAX_POS`] (hostile workspace files and bad monitor reports included).
    pub fn sanitized(rect: [f32; 4]) -> [f32; 4] {
        let finite = |v: f32, fallback: f32| if v.is_finite() { v } else { fallback };
        let [x, y, w, h] = rect;
        [
            finite(x, 0.0).clamp(-PANEL_WINDOW_MAX_POS, PANEL_WINDOW_MAX_POS),
            finite(y, 0.0).clamp(-PANEL_WINDOW_MAX_POS, PANEL_WINDOW_MAX_POS),
            finite(w, 420.0).clamp(PANEL_WINDOW_MIN[0], PANEL_WINDOW_MAX),
            finite(h, 320.0).clamp(PANEL_WINDOW_MIN[1], PANEL_WINDOW_MAX),
        ]
    }
}

impl DockNode {
    /// Insert panel `p` relative to the group that holds `anchor`: as a tab next to it
    /// (Center) or in a new group split off to one side. In a stack, the group is `anchor`'s
    /// frame: Center adds a tab to that frame and Top / Bottom add a frame above or below it
    /// (#302); Left / Right split the column. `p` is removed from its old place first. Returns
    /// false (tree unchanged) if `anchor` isn't docked or is `p` itself alone.
    pub fn dock_panel(&mut self, p: PanelKind, anchor: PanelKind, zone: Zone) -> bool {
        if !crate::dock_shared::valid_tree(self) || p == anchor || !self.contains(anchor) {
            return false;
        }
        let before = self.clone();
        self.close(p);
        fn rec(n: &mut DockNode, p: PanelKind, anchor: PanelKind, zone: Zone) -> bool {
            let here = match n {
                DockNode::Split { a, b, .. } => return rec(a, p, anchor, zone) || rec(b, p, anchor, zone),
                DockNode::Tabs { panels, .. } => panels.contains(&anchor),
                DockNode::Stack { entries } => entries.iter().any(|e| e.contains(anchor)),
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
                    let Some(e) = entries.iter_mut().find(|e| e.contains(anchor)) else { return false };
                    let i = e.panels.iter().position(|x| *x == anchor).map_or(e.panels.len(), |i| i + 1);
                    e.panels.insert(i, p);
                    e.active = i;
                    e.open = true;
                }
                (Zone::Top | Zone::Bottom, DockNode::Stack { entries }) => {
                    let Some(i) = entries.iter().position(|e| e.contains(anchor)) else { return false };
                    let at = if zone == Zone::Top { i } else { i + 1 };
                    entries.insert(at, StackEntry::new(p, true, None));
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
        if !crate::dock_shared::valid_tree(self) {
            return false;
        }
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
                DockNode::Stack { entries } => {
                    // A tab of the anchor's frame.
                    let Some(e) = entries.iter_mut().find(|e| e.contains(anchor)) else { return false };
                    let i = before.and_then(|b| e.panels.iter().position(|x| *x == b)).unwrap_or(e.panels.len());
                    e.panels.insert(i, p);
                    e.active = i;
                    e.open = true;
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
                DockNode::Stack { entries } => entries.iter().position(|e| e.contains(p)).map(|i| format!("{path}s{i}")),
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
    /// Apply a shared transaction without replacing the persisted app schema.
    pub fn apply(&mut self, action: craft_ui::docking::Action<PanelKind>) -> Result<(), String> {
        crate::dock_shared::encode(&self.root, &self.floating)?.validate().map_err(|error| error.to_string())?;
        let moving_to_float = match &action {
            craft_ui::docking::Action::Float { panel, .. } => Some(*panel),
            craft_ui::docking::Action::Move { panel, anchor, .. } if self.floating.iter().any(|group| group.panels.contains(anchor)) => Some(*panel),
            _ => None,
        };
        if moving_to_float.is_some_and(|panel| self.root.contains(panel) && self.root.panel_count() <= 1) {
            return Err("the last docked panel must remain in the workspace".into());
        }
        crate::dock_shared::apply(self, action)
    }

    /// Undock an existing panel; the final docked panel stays in the workspace.
    pub fn float(&mut self, p: PanelKind, rect: [f32; 4]) -> bool {
        self.apply(craft_ui::docking::Action::Float { panel: p, rect }).is_ok()
    }

    /// Close an existing floating panel and collapse its empty group.
    pub fn unfloat(&mut self, p: PanelKind) -> bool {
        crate::dock_shared::unfloat(self, p)
    }

    /// Move or reorder a docked/floating tab through the shared transaction.
    pub fn insert_tab(&mut self, p: PanelKind, anchor: PanelKind, before: Option<PanelKind>) -> bool {
        if !crate::dock_shared::valid_tree(&self.root) {
            return false;
        }
        self.apply(craft_ui::docking::Action::Move { panel: p, anchor, placement: craft_ui::docking::Placement::Tab { before } }).is_ok()
    }

    /// Dock a panel beside an anchor; floating destinations accept tabs only.
    pub fn dock(&mut self, p: PanelKind, anchor: PanelKind, zone: Zone) -> bool {
        if !crate::dock_shared::valid_tree(&self.root) {
            return false;
        }
        self.apply(craft_ui::docking::Action::Move { panel: p, anchor, placement: craft_ui::docking::Placement::Split(zone.shared()) }).is_ok()
    }

    pub fn contains(&self, p: PanelKind) -> bool {
        self.root.contains(p) || self.floating.iter().any(|group| group.panels.contains(&p))
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
    let mut actions = Vec::new();
    layout_with_actions(ui, node, rect, t, path, out, reg, &mut actions);
    for action in actions {
        crate::dock_shared::apply_tree(node, action);
    }
}

/// Lay out chrome while returning mutations for the application's UI command route.
pub fn layout_with_actions(
    ui: &mut egui::Ui,
    node: &mut DockNode,
    rect: Rect,
    t: &Tokens,
    path: &str,
    out: &mut Vec<Group>,
    reg: &mut crate::automation::Registry,
    actions: &mut Vec<craft_ui::docking::Action<PanelKind>>,
) {
    layout_inner(ui, node, rect, t, path, &[], out, reg, actions);
}

fn layout_inner(
    ui: &mut egui::Ui,
    node: &mut DockNode,
    rect: Rect,
    t: &Tokens,
    path: &str,
    tree_path: &[bool],
    out: &mut Vec<Group>,
    reg: &mut crate::automation::Registry,
    actions: &mut Vec<craft_ui::docking::Action<PanelKind>>,
) {
    match node {
        DockNode::Tabs { panels, active } => {
            let tab_h = if panels.len() == 1 && panels[0].compact() { 8.0 } else { t.tab_h };
            let content = Rect::from_min_max(pos2(rect.min.x, rect.min.y + tab_h), rect.max);
            out.push(Group { path: path.to_string(), rect, content, panels: panels.clone(), active: *active, stacked: None });
        }
        DockNode::Stack { entries } => {
            // A header for every entry; the open panels' bodies share the rest.
            let head = t.tab_h;
            let g = t.gap;
            let n = entries.len() as f32;
            let avail = rect.height() - n * head - (n - 1.0).max(0.0) * g;
            let bodies = stack_bodies(entries, avail);
            let mut y = rect.min.y;
            let mut drag = None;
            for (i, e) in entries.iter().enumerate() {
                let body = bodies.get(i).copied().unwrap_or(0.0);
                let r = Rect::from_min_max(pos2(rect.min.x, y), pos2(rect.max.x, (y + head + body).min(rect.max.y)));
                let content = Rect::from_min_max(pos2(r.min.x, r.min.y + head), r.max);
                let group = format!("{path}s{i}");
                // The gap below a panel resizes the open panels on either side of it.
                let above = entries.get(..=i).and_then(|s| s.iter().rposition(|x| x.open));
                let below = entries.get(i + 1..).and_then(|s| s.iter().position(|x| x.open)).map(|k| i + 1 + k);
                if let (Some(above), Some(below)) = (above, below) {
                    let gap = Rect::from_min_max(pos2(rect.min.x, r.max.y), pos2(rect.max.x, r.max.y + g));
                    if let Some(d) = gutter(ui, gap, true, &group, t, reg) {
                        drag = Some((above, below, d));
                    }
                }
                out.push(Group { path: group, rect: r, content, panels: e.panels.clone(), active: e.active, stacked: Some(e.open) });
                y = r.max.y + g;
            }
            if let Some((above, below, d)) = drag {
                let mut proposed = entries.clone();
                resize_stacked(&mut proposed, &bodies, above, below, d);
                for (before, after) in entries.iter().zip(proposed) {
                    if before.height != after.height
                        && let Some(panel) = after.shown()
                    {
                        actions.push(craft_ui::docking::Action::ResizeStack { panel, height: after.height });
                    }
                }
            }
        }
        DockNode::Split { vertical, size, a, b } => {
            let axis = if *vertical { SplitAxis::Vertical } else { SplitAxis::Horizontal };
            let Some(split) = split_rect(rect, axis, size.shared(), t.gap, 20.0) else { return };
            let response = resize_handle(split.divider, *vertical, path, t).value(split.first_size(), 0.0..=split.available()).show(ui);
            reg.add(&format!("dock.gutter.{path}"), response.response.rect, "gutter");
            if response.delta != 0.0
                && let Some(resized) = split.resized(size.shared(), response.delta, 40.0)
            {
                actions.push(craft_ui::docking::Action::ResizeSplit { path: tree_path.to_vec(), size: resized });
            }
            let mut first_path = tree_path.to_vec();
            first_path.push(false);
            let mut second_path = tree_path.to_vec();
            second_path.push(true);
            layout_inner(ui, a, split.first, t, &format!("{path}a"), &first_path, out, reg, actions);
            layout_inner(ui, b, split.second, t, &format!("{path}b"), &second_path, out, reg, actions);
        }
    }
}

/// A gap between dock areas that drags to resize them (`vertical`: up and down), with a slightly
/// larger hit area than the gap. Returns the drag this frame.
fn gutter(ui: &mut egui::Ui, gap: Rect, vertical: bool, path: &str, t: &Tokens, reg: &mut crate::automation::Registry) -> Option<f32> {
    let response = resize_handle(gap, vertical, path, t).show(ui);
    reg.add(&format!("dock.gutter.{path}"), response.response.rect, "gutter");
    (response.delta != 0.0).then_some(response.delta)
}

fn resize_handle(gap: Rect, vertical: bool, path: &str, t: &Tokens) -> ResizeHandle<'static> {
    let axis = if vertical { SplitAxis::Vertical } else { SplitAxis::Horizontal };
    let style = ResizeHandleStyle { hovered: t.focus.gamma_multiply(0.4), dragged: t.focus.gamma_multiply(0.9), focused: t.focus };
    let label = if vertical { crate::i18n::tr("Height:") } else { crate::i18n::tr("Width:") };
    ResizeHandle::new(egui::Id::new(("dock-gutter", path.to_string())), axis, gap, label, style)
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

/// `v` held to `lo..=hi`, never panicking: when the room is too small for both minimums
/// (`hi < lo`), `lo` wins; a non-finite `v` gives `lo`.
#[cfg(test)]
fn clamp_ordered(v: f32, lo: f32, hi: f32) -> f32 {
    let lo = if lo.is_finite() { lo } else { 0.0 };
    let hi = if hi.is_finite() { hi.max(lo) } else { lo };
    if v.is_finite() { v.max(lo).min(hi) } else { lo }
}

/// The size of a split's first side in `avail` points (both sides keep 20 pt when there is room).
/// `f32::clamp` panics when its bounds cross, which a split shorter than 40 pt did (#306).
#[cfg(test)]
fn split_first(size: SplitSize, avail: f32) -> f32 {
    let avail = if avail.is_finite() { avail.max(0.0) } else { 0.0 };
    let first = match size {
        SplitSize::Ratio(r) => avail * r,
        SplitSize::FixedA(px) => px.min(avail - 20.0),
        SplitSize::FixedB(px) => avail - px.min(avail - 20.0),
    };
    let lo = 20.0_f32.min(avail);
    clamp_ordered(first, lo, avail - 20.0).min(avail)
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
pub fn draw_group_chrome(ui: &mut egui::Ui, g: &Group, t: &Tokens, reg: &mut crate::automation::Registry, info: &TabInfo) -> Chrome {
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
    painter.rect_filled(g.rect, t.radius, t.panel_bg);
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
        let strip = Rect::from_min_size(g.rect.min, vec2(g.rect.width(), t.tab_h));
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
        let mut x = strip.min.x + 12.0;
        let text_y = strip.min.y + 16.0;
        for (k, e) in entries.iter().enumerate() {
            let p = &e.panel;
            // A collapsed stacked panel's header is plain text (no underline or panel menu).
            let is_active = k == ai && g.stacked != Some(false);
            let galley = painter.layout_no_wrap(e.label.clone(), Tokens::ui(12.0), if is_active { t.tab_text_active } else { t.tab_text });
            let menu_w = if is_active { 20.0 } else { 0.0 };
            // The lock belongs to the panel: on its shown document's tab only.
            let lock = e.deco.is_some_and(|d| d.viewer) && (e.doc.is_none() || is_active);
            // × (active tab only), swatch and lock before the label.
            let deco_w = match e.deco {
                Some(d) if d.viewer => (if is_active { 16.0 } else { 0.0 }) + 14.0 + if lock { 16.0 } else { 0.0 },
                Some(_) => 14.0,
                None => 0.0,
            };
            let w = galley.size().x + 16.0 + menu_w + deco_w;
            if x + w > strip.max.x - 20.0 && k > ai {
                let r = Rect::from_min_size(pos2(strip.max.x - 22.0, strip.min.y + 6.0), vec2(18.0, 20.0));
                let resp = ui.interact(r, egui::Id::new(("tab-overflow", g.path.clone())), Sense::click());
                icons::paint(&painter, r.shrink(4.0).translate(vec2(-2.0, 0.0)), Icon::ChevronRight, t.tab_text);
                icons::paint(&painter, r.shrink(4.0).translate(vec2(2.0, 0.0)), Icon::ChevronRight, t.tab_text);
                crate::widgets::focus_outline(ui, &resp, t);
                reg.add_response(&format!("panel.overflow.{}", g.path), &resp, "More tabs");
                if resp.clicked()
                    && let Some(next) = entries.get((ai + 1) % entries.len())
                {
                    out.actions.push(activate(next));
                }
                break;
            }
            let tab = Rect::from_min_size(pos2(x, strip.min.y), vec2(w, t.tab_h));
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
            reg.add_response(&auto_id, &resp, &e.label);
            if e.shown_doc {
                reg.add_response(&format!("panel.tab.{}", p.id()), &resp, &e.label);
            }
            let mut label_x = tab.min.x + 8.0;
            if let Some(d) = e.deco {
                let mut dx = tab.min.x + 6.0;
                if is_active && d.viewer {
                    let cr = Rect::from_center_size(pos2(dx + 5.0, text_y), vec2(10.0, 10.0));
                    let cresp = ui.interact(cr.expand(2.0), egui::Id::new(("tab-close", g.path.clone(), k)), Sense::click());
                    let cc = if cresp.hovered() { t.tab_text_active } else { t.tab_text };
                    let kk = 3.5;
                    painter.line_segment([cr.center() + vec2(-kk, -kk), cr.center() + vec2(kk, kk)], Stroke::new(1.2, cc));
                    painter.line_segment([cr.center() + vec2(-kk, kk), cr.center() + vec2(kk, -kk)], Stroke::new(1.2, cc));
                    reg.add_response(&format!("panel.tab.{}.close", p.id()), &cresp, "Close");
                    crate::widgets::focus_outline(ui, &cresp, t);
                    if cresp.clicked() {
                        out.actions.push(close(e));
                    }
                    dx += 16.0;
                }
                let sw = Rect::from_center_size(pos2(dx + 5.0, text_y), vec2(9.0, 9.0));
                painter.rect_filled(sw, 1.0, d.swatch);
                dx += 14.0;
                label_x = dx;
                if lock {
                    let lr = Rect::from_center_size(pos2(dx + 6.0, text_y), vec2(12.0, 12.0));
                    let lresp = ui.interact(lr.expand(2.0), egui::Id::new(("tab-lock", g.path.clone(), k)), Sense::click());
                    let lc = if d.locked {
                        t.tab_text_active
                    } else if lresp.hovered() {
                        t.tab_text
                    } else {
                        t.text_faint
                    };
                    icons::paint(&painter, lr, Icon::Lock, lc);
                    reg.add_response(&format!("panel.tab.{}.lock", p.id()), &lresp, if d.locked { "Unlock" } else { "Lock" });
                    crate::widgets::focus_outline(ui, &lresp, t);
                    if lresp.clicked() {
                        out.actions.push(DockAction::ToggleLock(*p));
                    }
                    label_x = dx + 16.0;
                }
            }
            let label_w = galley.size().x;
            let col = if is_active || resp.hovered() { t.tab_text_active } else { t.tab_text };
            painter.galley_with_override_text_color(pos2(label_x, text_y - galley.size().y / 2.0), galley, col);
            if is_active {
                let mr = Rect::from_center_size(pos2(label_x + label_w + 12.0, text_y), vec2(12.0, 10.0));
                let mresp = ui.interact(mr.expand(3.0), egui::Id::new(("tab-menu", g.path.clone())), Sense::click());
                reg.add_response(&format!("panel.menu.{}", p.id()), &mresp, "panel menu");
                let mc = if mresp.hovered() { t.tab_text_active } else { t.tab_text };
                for dy in [-3.5, 0.0, 3.5] {
                    painter.line_segment([pos2(mr.min.x, mr.center().y + dy), pos2(mr.max.x, mr.center().y + dy)], Stroke::new(1.5, mc));
                }
                let uy = strip.min.y + 23.0;
                painter.line_segment([pos2(label_x, uy), pos2(mr.max.x, uy)], Stroke::new(1.0, t.tab_text_active));
                crate::widgets::focus_outline(ui, &mresp, t);
                if mresp.clicked() {
                    out.actions.push(DockAction::PanelMenu(*p, mr.left_bottom()));
                }
            }
            crate::widgets::focus_outline(ui, &resp, t);
            if resp.clicked() {
                // A stacked frame's shown tab expands / collapses it; another tab shows that
                // panel (expanding the frame).
                if g.stacked.is_some() && (k == ai || entries.len() == 1) {
                    out.actions.push(DockAction::ToggleStacked(*p));
                } else {
                    out.actions.push(activate(e));
                }
                out.actions.push(DockAction::Focus(*p));
            }
            if resp.middle_clicked() {
                out.actions.push(close(e));
            }
            x += w + 8.0;
        }
        // A stacked panel's whole header bar expands/collapses it, not just its name (as in After
        // Effects): the frame with its shown tab.
        if g.stacked.is_some()
            && let Some(e) = entries.get(ai).or(entries.first())
        {
            let rest = Rect::from_min_max(pos2((x - 8.0).max(strip.min.x), strip.min.y), strip.max);
            if rest.width() > 0.0 {
                let resp = ui.interact(rest, egui::Id::new(("tab-rest", g.path.clone())), Sense::click());
                reg.add(&format!("panel.header.{}", e.panel.id()), rest, &e.label);
                if resp.clicked() {
                    out.actions.push(DockAction::ToggleStacked(e.panel));
                    out.actions.push(DockAction::Focus(e.panel));
                }
            }
        }
    }
    // clicking anywhere in the panel focuses it; a press on another of its tabs waits for that
    // tab's click, which focuses the panel actually chosen (a text edit stays open in Character)
    let at = ui.input(|i| i.pointer.interact_pos());
    if let Some(p) = active_panel
        && ui.rect_contains_pointer(g.rect)
        && ui.input(|i| i.pointer.any_pressed())
        && !out.tabs.iter().any(|(q, r)| *q != p && at.is_some_and(|a| r.contains(a)))
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
mod tests {
    use super::*;

    /// The original split clamp panicked whenever 0 < available < 40. Run the actual dock
    /// layout without the app's last-resort panic guard so that regression cannot be hidden.
    #[test]
    fn tiny_split_layouts_stay_inside_their_parent() {
        let ctx = egui::Context::default();
        let tokens = Tokens::for_kind(crate::theme::ThemeKind::Dark);
        for extent in [20.0, 0.0, 1.0, 5.0, 15.0, 35.0, 39.9, 40.0, 80.0] {
            for vertical in [false, true] {
                for size in [SplitSize::Ratio(0.5), SplitSize::FixedA(300.0), SplitSize::FixedB(300.0)] {
                    let rect = Rect::from_min_size(egui::Pos2::ZERO, egui::Vec2::splat(extent));
                    let mut node =
                        DockNode::Split { vertical, size, a: Box::new(tabs(&[PanelKind::Project], 0)), b: Box::new(tabs(&[PanelKind::Composition], 0)) };
                    let mut groups = vec![];
                    let mut registry = crate::automation::Registry::default();
                    let mut output = ctx.run_ui(egui::RawInput::default(), |ui| layout(ui, &mut node, rect, &tokens, "", &mut groups, &mut registry));
                    output.textures_delta.clear();
                    assert_eq!(groups.len(), 2);
                    for group in groups {
                        assert!(rect.contains_rect(group.rect), "{extent}, {vertical}: {:?}", group.rect);
                        assert!(group.rect.width() >= 0.0 && group.rect.height() >= 0.0);
                    }
                }
            }
        }
    }

    #[test]
    fn open_near_in_an_empty_dock_makes_the_panel_the_dock() {
        for p in [PanelKind::Composition, PanelKind::Timeline, PanelKind::Project] {
            let mut d = tabs(&[], 0);
            d.open_near(p, PanelKind::EffectsPresets);
            assert!(d.is_visible(p));
            assert_eq!(d.panel_count(), 1);
        }
    }

    #[test]
    fn open_near_without_any_anchor_uses_the_first_group() {
        let mut d = tabs(&[PanelKind::Composition], 0);
        d.open_near(PanelKind::Timeline, PanelKind::EffectsPresets);
        assert!(d.is_visible(PanelKind::Timeline));
        assert_eq!(d.panel_count(), 2);
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
        StackEntry::new(panel, open, height)
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
    fn short_splits_lay_out_without_panicking() {
        // #306: a split 0 < avail < 40 pt tall made `f32::clamp` panic (its bounds crossed).
        for avail in [0.0, 5.0, 19.0, 20.0, 30.0, 39.0, 40.0, 41.0, -10.0, f32::NAN, f32::INFINITY] {
            for size in [SplitSize::Ratio(0.5), SplitSize::Ratio(f32::NAN), SplitSize::FixedA(300.0), SplitSize::FixedB(300.0), SplitSize::FixedA(-5.0)] {
                let f = split_first(size, avail);
                assert!(f.is_finite() && f >= 0.0, "{size:?} {avail}: {f}");
                if avail.is_finite() && avail > 0.0 {
                    assert!(f <= avail, "{size:?} {avail}: {f}");
                }
            }
        }
        assert_eq!(split_first(SplitSize::FixedA(300.0), 30.0), 20.0);
        assert_eq!(split_first(SplitSize::Ratio(0.5), 200.0), 100.0);
        assert_eq!(clamp_ordered(5.0, 40.0, -4.0), 40.0);
        // Every workspace, laid out in rects too short and too narrow for its minimums.
        let ctx = egui::Context::default();
        let t = Tokens::for_kind(crate::theme::ThemeKind::Dark);
        for w in WORKSPACES {
            for (wd, ht) in [(30.0, 30.0), (800.0, 36.0), (800.0, 45.0), (36.0, 600.0), (1.0, 1.0), (0.0, 0.0)] {
                let mut node = workspace(w);
                let mut frame = ctx.run_ui(egui::RawInput::default(), |ui| {
                    let mut out = vec![];
                    let mut reg = crate::automation::Registry::default();
                    layout(ui, &mut node, Rect::from_min_size(pos2(0.0, 0.0), vec2(wd, ht)), &t, "", &mut out, &mut reg);
                    assert!(!out.is_empty(), "{w}");
                });
                frame.textures_delta.clear();
            }
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
        // Into a stack: a tab of the anchor's frame (#302).
        let mut s = workspace("Default");
        let props = s.path_of(Properties).unwrap();
        assert!(s.dock_panel(Tracker, Properties, Zone::Center));
        assert_eq!(s.path_of(Tracker), Some(props.clone()));
        assert!(s.is_visible(Tracker) && !s.is_visible(Properties));
        assert!(s.activate(Properties) && s.is_visible(Properties));
        // Top / Bottom: a frame of its own above / below the anchor's, not a split column.
        assert!(s.dock_panel(Info, Properties, Zone::Bottom));
        let below = s.path_of(Info).unwrap();
        assert_ne!(below, props);
        assert_eq!(s.path_of(Properties), Some(props.clone()));
        assert!(s.dock_panel(Info, Properties, Zone::Top));
        assert_eq!(s.path_of(Info), Some(props.clone()), "above Properties: its frame's index");
        // Closing a tab keeps the frame; closing the frame's last tab removes it.
        s.close(Tracker);
        assert!(s.contains(Properties) && !s.contains(Tracker));
        // A tab dragged onto a stacked frame's tab strip joins that frame.
        let mut t = workspace("Default");
        assert!(t.insert_tab(Info, Preview, None));
        assert_eq!(t.path_of(Info), t.path_of(Preview));
        // Moving the only panel of a group collapses its split.
        let mut m = hsplit(SplitSize::Ratio(0.5), tabs(&[Project], 0), tabs(&[Composition], 0));
        assert!(m.dock_panel(Project, Composition, Zone::Center));
        assert_eq!(m, tabs(&[Composition, Project], 1));
    }

    /// #302: workspaces saved when each stacked frame held one `panel` still load.
    #[test]
    fn old_stack_entries_load() {
        let e: StackEntry = serde_json::from_str(r#"{"panel":"Preview","open":true,"height":46.0}"#).unwrap();
        assert_eq!(e, StackEntry::new(PanelKind::Preview, true, Some(46.0)));
        let s = serde_json::to_string(&workspace("Default")).unwrap();
        assert_eq!(serde_json::from_str::<DockNode>(&s).unwrap(), workspace("Default"));
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

    /// Ported from #630 (Ni-zav): the reporter's reproduction on the Minimal workspace.
    #[test]
    fn window_panels_reopen_after_last_panel_is_closed() {
        use PanelKind::*;
        for reopen in [Composition, Timeline, Project] {
            let mut d = workspace("Minimal");
            d.close(Composition);
            d.close(Timeline);
            assert_eq!(d.panel_count(), 0);
            d.open_near(reopen, EffectsPresets);
            assert_eq!(d.panel_count(), 1);
            assert!(d.is_visible(reopen), "{reopen:?} must be visible");
        }
        let mut d = tabs(&[Audio], 0);
        d.open_near(History, Project);
        assert_eq!(d, tabs(&[Audio, History], 1));
        let mut d = workspace("Minimal");
        d.open_near(EffectsPresets, Timeline);
        assert_eq!(d.path_of(EffectsPresets), d.path_of(Timeline));
    }

    #[test]
    fn closing_earlier_inactive_tab_keeps_the_shown_panel() {
        use PanelKind::*;
        let mut d = tabs(&[Properties, Align, EffectsPresets], 1);
        d.close(Properties);
        assert!(d.is_visible(Align), "closing an inactive tab keeps the shown panel");
        assert!(!d.is_visible(EffectsPresets));
        assert_eq!(d.panel_count(), 2);
    }

    #[test]
    fn closing_active_tab_keeps_existing_neighbor_choice() {
        use PanelKind::*;
        for (active, closed, shown) in [(0, Properties, Align), (1, Align, EffectsPresets), (2, EffectsPresets, Align)] {
            let mut d = tabs(&[Properties, Align, EffectsPresets], active);
            d.close(closed);
            assert!(d.is_visible(shown), "active {active}: {closed:?} -> {shown:?}");
        }
    }

    #[test]
    fn close_normalizes_invalid_active_and_collapses_empty_splits() {
        use PanelKind::*;
        let mut d = tabs(&[Properties, Align, EffectsPresets], usize::MAX);
        d.close(History);
        assert!(d.is_visible(EffectsPresets));
        d.close(Properties);
        assert!(d.is_visible(EffectsPresets));
        d.close(EffectsPresets);
        assert!(d.is_visible(Align));
        d.close(Align);
        assert_eq!(d, tabs(&[], 0));
        let mut d = DockNode::Split { vertical: true, size: SplitSize::Ratio(0.5), a: Box::new(tabs(&[Properties], 0)), b: Box::new(tabs(&[Align], 0)) };
        d.close(Properties);
        assert_eq!(d, tabs(&[Align], 0));
        d.close(Align);
        assert_eq!(d, tabs(&[], 0));
    }

    #[test]
    fn removing_earlier_inactive_floating_tab_keeps_the_shown_panel() {
        use PanelKind::*;
        let mut layout = Layout {
            root: tabs(&[Composition], 0),
            floating: vec![Floating { panels: vec![Properties, Align, EffectsPresets], active: 1, rect: [0.0, 0.0, 300.0, 200.0] }],
        };
        assert!(layout.unfloat(Properties));
        let f = &layout.floating[0];
        assert_eq!(f.panels.get(f.active), Some(&Align));
        assert!(layout.root.is_visible(Composition));
    }

    #[test]
    fn floating_removal_keeps_active_neighbor_and_handles_invalid_index() {
        use PanelKind::*;
        for (active, closed, shown) in
            [(0, Properties, Align), (1, Align, EffectsPresets), (2, EffectsPresets, Align), (usize::MAX, Properties, EffectsPresets)]
        {
            let mut layout = Layout {
                root: tabs(&[Composition], 0),
                floating: vec![Floating { panels: vec![Properties, Align, EffectsPresets], active, rect: [0.0, 0.0, 300.0, 200.0] }],
            };
            assert!(layout.unfloat(closed));
            let f = &layout.floating[0];
            assert_eq!(f.panels.get(f.active), Some(&shown));
            for p in [Properties, Align, EffectsPresets] {
                layout.unfloat(p);
            }
            assert!(layout.floating.is_empty());
            assert!(!layout.unfloat(History));
        }
    }
}
