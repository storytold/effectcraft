//! EffectCraft's panels and built-in workspaces in craft-ui's shared docking model
//! ([`craft_ui::docking`]): splits, tab groups, After Effects' stacked panels and floating
//! windows. craft-ui owns the layout model, its validation and every transaction; EffectCraft
//! owns the panel ids, the built-in workspaces and the viewer locks in its group headers.

use egui::{Align2, Rect, Sense, pos2, vec2};
use serde::{Deserialize, Serialize};

pub use craft_behavior::dock::SplitSize;
pub use craft_behavior::dock::{Floating, Node, PanelState, Permissions, StackEntry};
pub use craft_behavior::flow::Axis;

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
            PanelKind::LumetriScopes => "Video Scopes",
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
    /// The shortcut scope of the panel: an additional viewer is a Composition panel.
    pub fn shortcut_scope(self) -> String {
        match self {
            PanelKind::Viewer(_) => PanelKind::Composition.id(),
            p => p.id(),
        }
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

/// The docked tree plus floating windows.
pub type Layout = craft_behavior::dock::Layout<PanelKind>;
/// What a workspace saves: the layout, closed panels with where each goes back, and the lock.
pub type Workspace = craft_behavior::dock::Workspace<PanelKind>;

/// What may happen to a panel. Every panel moves, floats and closes, as in After Effects
/// (the Composition panel too: Window ▸ Composition reopens it); the last docked group staying
/// docked is checked per transaction ([`crate::dock_ui`]).
pub fn permissions(_panel: &PanelKind) -> Permissions {
    Permissions::default()
}

/// A tab group.
pub fn tabs(p: &[PanelKind], active: usize) -> Node<PanelKind> {
    Node::tabs(p.to_vec(), active)
}
/// Stacked panels: (panel, open, height). Open entries share the stack's height in proportion to
/// their heights; an entry without one shares the stack's height with the others.
pub fn stack(e: &[(PanelKind, bool, Option<f32>)]) -> Node<PanelKind> {
    Node::stack(
        e.iter()
            .map(|&(panel, open, height)| {
                let mut entry = StackEntry::new(panel);
                entry.open = open;
                entry.height = height;
                entry
            })
            .collect(),
    )
}
/// Two frames side by side.
pub fn hsplit(size: SplitSize, a: Node<PanelKind>, b: Node<PanelKind>) -> Node<PanelKind> {
    Node::split(Axis::X, size, a, b)
}
/// Two frames one above the other.
pub fn vsplit(size: SplitSize, a: Node<PanelKind>, b: Node<PanelKind>) -> Node<PanelKind> {
    Node::split(Axis::Y, size, a, b)
}

/// A workspace of just `root`, with no floating windows.
pub fn docked(root: Node<PanelKind>) -> Workspace {
    Workspace::new(Layout::new(Some(root), Vec::new()))
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

/// The floating windows a workspace starts with (Undocked Panels), in points for a window of
/// about 1680×1020 (craft-ui keeps them inside the window when drawn). Each window has a fixed
/// id, so a built-in layout built again is the same set of windows.
fn workspace_floating(name: &str) -> Vec<Floating<PanelKind>> {
    use PanelKind::*;
    let f = |id: u64, panels: &[PanelKind], [x, y, w, h]: [f32; 4]| Floating::single(id, panels.to_vec(), 0, Rect::from_min_size(pos2(x, y), vec2(w, h)));
    match name {
        "Undocked Panels" => vec![
            f(1, &[Project, EffectControls], [24.0, 110.0, 320.0, 420.0]),
            f(2, &[Preview, Info, Audio], [1330.0, 110.0, 320.0, 300.0]),
            f(3, &[EffectsPresets, Properties, Character, Paragraph], [1330.0, 430.0, 320.0, 360.0]),
            f(4, &[Timeline, RenderQueue], [24.0, 640.0, 1280.0, 340.0]),
        ],
        _ => vec![],
    }
}

/// The height of a frame's tab strip ([`Tokens::tab_h`]).
pub const TAB_STRIP: f32 = 30.0;
/// The height of Default's Preview frame: its tab strip over a 46-point body that fits the
/// playback controls.
pub const PREVIEW_FRAME: f32 = TAB_STRIP + 46.0;

/// The built-in layout of a named workspace (an unknown name gets Default's).
pub fn workspace(name: &str) -> Layout {
    Layout::new(Some(workspace_root(name)), workspace_floating(name))
}

/// The docked tree of a named workspace.
fn workspace_root(name: &str) -> Node<PanelKind> {
    use PanelKind::*;
    use SplitSize::*;
    let right = |p: &[PanelKind], q: &[PanelKind]| vsplit(Ratio(0.45), tabs(p, 0), tabs(q, 0));
    match name {
        "Animation" => vsplit(
            Ratio(0.55),
            hsplit(
                FixedFirst(300.0),
                tabs(&[Project, EffectControls], 1),
                hsplit(FixedSecond(300.0), tabs(&[Composition, Layer], 0), right(&[Info, Preview, Audio], &[EffectsPresets, Smoother, Wiggler, MotionSketch])),
            ),
            tabs(&[Timeline, RenderQueue], 0),
        ),
        "Effects" => vsplit(
            Ratio(0.58),
            hsplit(
                FixedFirst(340.0),
                tabs(&[EffectControls, Project], 0),
                hsplit(FixedSecond(300.0), tabs(&[Composition, Layer], 0), right(&[Info, Preview], &[EffectsPresets])),
            ),
            tabs(&[Timeline, RenderQueue], 0),
        ),
        "Motion Tracking" => vsplit(
            Ratio(0.6),
            hsplit(
                FixedFirst(300.0),
                tabs(&[Project, EffectControls], 0),
                hsplit(FixedSecond(300.0), tabs(&[Layer, Composition], 0), right(&[Info, Preview], &[Tracker])),
            ),
            tabs(&[Timeline], 0),
        ),
        "Paint" => vsplit(
            Ratio(0.6),
            hsplit(
                FixedFirst(300.0),
                tabs(&[Project, EffectControls], 0),
                hsplit(FixedSecond(300.0), tabs(&[Layer, Composition], 0), right(&[Paint, Info], &[Brushes, Preview])),
            ),
            tabs(&[Timeline], 0),
        ),
        "Text" => vsplit(
            Ratio(0.58),
            hsplit(
                FixedFirst(300.0),
                tabs(&[Project, EffectControls], 0),
                hsplit(FixedSecond(300.0), tabs(&[Composition], 0), right(&[Character, Paragraph], &[Align, Preview, EffectsPresets])),
            ),
            tabs(&[Timeline], 0),
        ),
        "Minimal" => vsplit(Ratio(0.6), tabs(&[Composition], 0), tabs(&[Timeline], 0)),
        // Review: a large viewer with the markers and the project beside it.
        "Review" => vsplit(
            Ratio(0.66),
            hsplit(
                FixedFirst(280.0),
                tabs(&[Project, Markers], 0),
                hsplit(FixedSecond(260.0), tabs(&[Composition, Layer], 0), right(&[Preview, Info], &[Audio])),
            ),
            tabs(&[Timeline, RenderQueue], 0),
        ),
        // Essential Graphics: the Properties panel (After Effects' successor to the Essential
        // Graphics panel) full height on the right, type panels beside the viewer.
        "Essential Graphics" => hsplit(
            FixedSecond(320.0),
            vsplit(
                Ratio(0.58),
                hsplit(
                    FixedFirst(280.0),
                    tabs(&[Project, EffectControls], 0),
                    hsplit(FixedSecond(260.0), tabs(&[Composition, Layer], 0), right(&[Character, Paragraph], &[Align, Preview])),
                ),
                tabs(&[Timeline], 0),
            ),
            tabs(&[Properties, EffectsPresets], 0),
        ),
        // Color: wide Effect Controls for grading effects, Info for colour readouts.
        "Color" => vsplit(
            Ratio(0.6),
            hsplit(
                FixedFirst(360.0),
                tabs(&[EffectControls, Project], 0),
                hsplit(FixedSecond(340.0), tabs(&[Composition, Layer], 0), right(&[LumetriScopes, Info, Preview], &[EffectsPresets])),
            ),
            tabs(&[Timeline], 0),
        ),
        // Undocked Panels: the Composition panel docked, the others floating
        // ([`workspace_floating`]).
        "Undocked Panels" => tabs(&[Composition, Layer, Flowchart], 0),
        "Small Screen" => vsplit(
            Ratio(0.55),
            hsplit(
                FixedFirst(240.0),
                tabs(&[Project, EffectControls], 0),
                hsplit(FixedSecond(240.0), tabs(&[Composition], 0), tabs(&[Info, Preview, EffectsPresets], 1)),
            ),
            tabs(&[Timeline], 0),
        ),
        "All Panels" => vsplit(
            Ratio(0.55),
            hsplit(
                FixedFirst(300.0),
                tabs(&[Project, EffectControls, Flowchart, History, MediaBrowser, Metadata], 0),
                hsplit(
                    FixedSecond(300.0),
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
                FixedFirst(300.0),
                tabs(&[Project, EffectControls], 0),
                hsplit(FixedSecond(290.0), tabs(&[Composition, Layer], 0), right(&[Info, Audio], &[Preview, EffectsPresets])),
            ),
            tabs(&[Timeline, RenderQueue], 0),
        ),
        // Default (as in After Effects): Project/Effect Controls | Composition over the Timeline,
        // and a full-height right column: a short Preview frame above the Properties frame.
        _ => hsplit(
            FixedSecond(300.0),
            vsplit(
                Ratio(0.56),
                hsplit(FixedFirst(300.0), tabs(&[Project, EffectControls], 0), tabs(&[Composition, Layer], 0)),
                tabs(&[Timeline, RenderQueue], 0),
            ),
            vsplit(FixedFirst(PREVIEW_FRAME), tabs(&[Preview], 0), tabs(&[Properties, Align, Audio, EffectsPresets], 0)),
        ),
    }
}

/// Queries on a workspace that EffectCraft's menus, commands and panels ask.
pub trait WorkspaceExt {
    /// `p` is in the docked tree.
    fn is_docked(&self, p: PanelKind) -> bool;
    /// `p` is in a floating window.
    fn is_floating(&self, p: PanelKind) -> bool;
    /// `p` is open, docked or floating.
    fn is_open(&self, p: PanelKind) -> bool;
    /// `p` shows: the front tab of its group, an open stacked panel, not in a minimized window.
    fn is_visible(&self, p: PanelKind) -> bool;
    /// The panels of the tab group holding `p` (a stacked panel is a group of its own).
    fn group_of(&self, p: PanelKind) -> Vec<PanelKind>;
    /// Every open panel, docked first.
    fn panels(&self) -> Vec<PanelKind>;
}

pub(crate) fn node_panels(node: &Node<PanelKind>, out: &mut Vec<PanelKind>) {
    match node {
        Node::Split { first, second, .. } => {
            node_panels(first, out);
            node_panels(second, out);
        }
        Node::Tabs { panels, .. } => out.extend(panels.iter().copied()),
        Node::Stack { entries, .. } => out.extend(entries.iter().map(|e| e.panel)),
        Node::Dock { columns, .. } => {
            for group in columns.iter().flat_map(|c| c.groups.iter()) {
                out.extend(group.panels.iter().copied());
            }
        }
        _ => {}
    }
}

fn node_group(node: &Node<PanelKind>, p: PanelKind) -> Option<Vec<PanelKind>> {
    match node {
        Node::Split { first, second, .. } => node_group(first, p).or_else(|| node_group(second, p)),
        Node::Tabs { panels, .. } => panels.contains(&p).then(|| panels.clone()),
        Node::Stack { entries, .. } => entries.iter().any(|e| e.panel == p).then(|| vec![p]),
        Node::Dock { columns, .. } => columns.iter().flat_map(|c| c.groups.iter()).find(|g| g.panels.contains(&p)).map(|g| g.panels.clone()),
        _ => None,
    }
}

impl WorkspaceExt for Workspace {
    fn is_docked(&self, p: PanelKind) -> bool {
        self.layout().root.as_ref().is_some_and(|root| node_group(root, p).is_some())
    }
    fn is_floating(&self, p: PanelKind) -> bool {
        self.layout().floating.iter().any(|w| w.contains(&p))
    }
    fn is_open(&self, p: PanelKind) -> bool {
        self.layout().contains(&p)
    }
    fn is_visible(&self, p: PanelKind) -> bool {
        self.layout().panel_state(&p) == PanelState::Front
    }
    fn group_of(&self, p: PanelKind) -> Vec<PanelKind> {
        let layout = self.layout();
        layout
            .root
            .as_ref()
            .and_then(|root| node_group(root, p))
            .or_else(|| layout.floating.iter().flat_map(|w| w.groups()).find(|g| g.panels.contains(&p)).map(|g| g.panels.clone()))
            .unwrap_or_default()
    }
    fn panels(&self) -> Vec<PanelKind> {
        let layout = self.layout();
        let mut out = Vec::new();
        if let Some(root) = &layout.root {
            node_panels(root, &mut out);
        }
        out.extend(layout.floating.iter().flat_map(|w| w.panels().copied()));
        out
    }
}

/// Every panel a workspace names: open, or closed with a recorded place.
pub fn referenced_panels(workspace: &Workspace) -> Vec<PanelKind> {
    let mut out = workspace.panels();
    out.extend(workspace.hidden().iter().map(|(p, _)| *p));
    out
}

/// The automation id of the gap below stacked panel `p`: `dock.gutter.<path>s<i>`, with the
/// stack's split path (`a`/`b` per level) and the entry's index.
pub fn stack_gutter_id(layout: &Layout, p: PanelKind) -> Option<String> {
    fn rec(node: &Node<PanelKind>, p: PanelKind, path: &mut String, depth: usize) -> Option<String> {
        if depth > craft_behavior::dock::MAX_DEPTH {
            return None;
        }
        match node {
            Node::Split { first, second, .. } => {
                path.push('a');
                if let Some(id) = rec(first, p, path, depth + 1) {
                    return Some(id);
                }
                path.pop();
                path.push('b');
                let found = rec(second, p, path, depth + 1);
                path.pop();
                found
            }
            Node::Stack { entries, .. } => entries.iter().position(|e| e.panel == p).map(|i| format!("dock.gutter.{path}s{i}")),
            _ => None,
        }
    }
    rec(layout.root.as_ref()?, p, &mut String::new(), 0)
}

/// A panel lock button (Composition viewers and the Timeline keep their comp while locked,
/// Effect Controls its layer): a closed padlock while locked, an open one while unlocked.
/// Returns true when clicked. Automation: `panel.tab.<panel>.lock`.
pub fn lock_button(ui: &mut egui::Ui, rect: Rect, panel: PanelKind, locked: bool, t: &Tokens, reg: &mut crate::automation::Registry) -> bool {
    let resp = ui.interact(rect, egui::Id::unique(("tab-lock", panel.id())), Sense::click());
    let color = if locked {
        t.tab_text_active
    } else if resp.hovered() {
        t.tab_text
    } else {
        t.text_faint
    };
    icons::paint(ui.painter(), rect.shrink(2.0), if locked { Icon::Lock } else { Icon::LockOpen }, color);
    reg.add_response(&format!("panel.tab.{}.lock", panel.id()), &resp, if locked { "Unlock" } else { "Lock" });
    crate::widgets::focus_outline(ui, &resp, t);
    resp.clicked()
}

/// Placeholder body for panels that are not implemented yet.
pub fn placeholder(ui: &mut egui::Ui, rect: Rect, t: &Tokens, text: &str) {
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, text, Tokens::ui(12.0), t.text_faint);
}

#[cfg(test)]
mod tests {
    use super::*;
    use PanelKind::*;
    use craft_behavior::DockSide;
    use craft_behavior::dock::{Action, DockPlacement};

    fn apply(w: &mut Workspace, action: Action<PanelKind>) {
        w.apply(action, false, permissions).unwrap();
    }

    /// Only the title changes: existing saved workspace layouts still deserialize.
    #[test]
    fn scopes_label_is_descriptive_and_its_saved_identity_stays_compatible() {
        let panel: PanelKind = serde_json::from_str("\"LumetriScopes\"").unwrap();
        assert_eq!(panel, LumetriScopes);
        assert_eq!(panel.title(), "Video Scopes");
        assert_eq!(panel.id(), "LumetriScopes");
        assert_eq!(serde_json::to_string(&panel).unwrap(), "\"LumetriScopes\"");
    }

    #[test]
    fn after_effects_workspaces_exist_with_their_core_panels() {
        for name in WORKSPACES {
            let w = Workspace::new(workspace(name));
            w.validate().unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(w.is_docked(Composition), "{name}");
            if *name != "Undocked Panels" {
                assert!(w.is_docked(Timeline), "{name}");
            }
        }
        let all = Workspace::new(workspace("All Panels"));
        for p in PanelKind::ALL.iter().filter(|p| !matches!(p, ScriptConsole | EssentialGraphics | MaskInterpolation)) {
            assert!(all.is_open(*p), "All Panels lacks {p:?}");
        }
        // No built-in workspace uses accordions: right columns are frames split vertically.
        fn no_stack(node: &Node<PanelKind>) -> bool {
            match node {
                Node::Split { first, second, .. } => no_stack(first) && no_stack(second),
                Node::Stack { .. } => false,
                _ => true,
            }
        }
        for name in WORKSPACES {
            let layout = workspace(name);
            assert!(layout.root.iter().all(no_stack), "{name}");
        }
    }

    #[test]
    fn default_right_column_is_a_short_preview_frame_over_the_properties_frame() {
        let default = workspace("Default");
        let Some(Node::Split { second, size: SplitSize::FixedSecond(300.0), .. }) = &default.root else { panic!("{default:?}") };
        let Node::Split { axis: Axis::Y, size: SplitSize::FixedFirst(PREVIEW_FRAME), first, second, .. } = second.as_ref() else { panic!("{second:?}") };
        assert_eq!(**first, tabs(&[Preview], 0));
        assert_eq!(**second, tabs(&[Properties, Align, Audio, EffectsPresets], 0));
        let mut w = Workspace::new(default);
        assert!(w.is_visible(Preview) && w.is_visible(Properties) && !w.is_visible(Align));
        // The Properties frame takes tabs; the Preview frame stays a frame of its own.
        apply(&mut w, Action::move_beside(Project, Properties, DockPlacement::tab(None)));
        assert_eq!(w.group_of(Project), vec![Properties, Align, Audio, EffectsPresets, Project]);
        assert_eq!(w.group_of(Preview), vec![Preview]);
    }

    #[test]
    fn panels_split_off_and_join_tab_groups() {
        let mut w = docked(hsplit(SplitSize::FixedFirst(300.0), tabs(&[Project, EffectControls], 0), tabs(&[Composition, Layer], 0)));
        apply(&mut w, Action::move_beside(EffectControls, Composition, DockPlacement::Split(DockSide::End)));
        assert_eq!(w.group_of(EffectControls), vec![EffectControls]);
        assert_eq!(w.group_of(Project), vec![Project]);
        apply(&mut w, Action::move_beside(EffectControls, Composition, DockPlacement::tab(Some(Layer))));
        assert_eq!(w.group_of(Composition), vec![Composition, EffectControls, Layer]);
        assert!(w.is_visible(EffectControls));
        // A rejected move (onto itself, onto a closed panel) changes nothing.
        let before = w.clone();
        assert!(w.apply(Action::move_beside(Project, Info, DockPlacement::Split(DockSide::Start)), false, permissions).is_err());
        assert_eq!(w, before);
    }

    #[test]
    fn closed_panels_reopen_where_they_were_docked_or_floating() {
        let mut w = Workspace::new(workspace("Standard"));
        apply(&mut w, Action::close(Info));
        assert!(!w.is_open(Info));
        w.set_visible(Info, true, None, false, permissions).unwrap();
        assert_eq!(w.group_of(Info), vec![Info, Audio]);
        apply(&mut w, Action::float(Audio, Rect::from_min_size(pos2(40.0, 50.0), vec2(300.0, 200.0))));
        apply(&mut w, Action::close(Audio));
        w.set_visible(Audio, true, None, false, permissions).unwrap();
        assert!(w.is_floating(Audio));
        // Closing an earlier, inactive tab keeps the shown panel.
        let mut w = docked(tabs(&[Properties, Align, EffectsPresets], 2));
        apply(&mut w, Action::close(Properties));
        assert!(w.is_visible(EffectsPresets));
        // A panel never laid out opens beside its anchor.
        apply(&mut w, Action::open(History, Some(Align)));
        assert_eq!(w.group_of(History), vec![Align, EffectsPresets, History]);
        assert_eq!(referenced_panels(&w).iter().filter(|p| **p == Properties).count(), 1, "the closed panel is still known");
    }
}
