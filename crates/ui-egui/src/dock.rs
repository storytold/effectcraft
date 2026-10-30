//! EffectCraft's panels and built-in workspaces in craft-ui's shared docking model
//! ([`craft_ui::docking`]): splits, tab groups, After Effects' stacked panels and floating
//! windows. craft-ui owns the layout model, its validation and every transaction; EffectCraft
//! owns the panel ids, the built-in workspaces and the decorations of its tabs (the comp tabs of
//! the Timeline, label swatches and viewer locks).

use egui::{Align2, Rect, Sense, Stroke, pos2, vec2};
use serde::{Deserialize, Serialize};

pub use craft_ui::docking::{Floating, Node, PanelState, Permissions, StackEntry};
pub use craft_ui::layout::{SplitAxis, SplitSize};

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

/// The docked tree plus floating windows.
pub type Layout = craft_ui::docking::Layout<PanelKind>;
/// What a workspace saves: the layout, closed panels with where each goes back, and the lock.
pub type Workspace = craft_ui::docking::Workspace<PanelKind>;

/// What may happen to a panel. Every panel moves, floats and closes, as in After Effects
/// (the Composition panel too: Window ▸ Composition reopens it); the last docked group staying
/// docked is checked per transaction ([`crate::dock_ui`]).
pub fn permissions(_panel: &PanelKind) -> Permissions {
    Permissions::default()
}

/// A tab group.
pub fn tabs(p: &[PanelKind], active: usize) -> Node<PanelKind> {
    Node::Tabs { panels: p.to_vec(), active }
}
/// Stacked panels: (panel, open, height). Open entries share the stack's height in proportion to
/// their heights; an entry without one counts as craft-ui's `DockStyle::min_pane` (40).
pub fn stack(e: &[(PanelKind, bool, Option<f32>)]) -> Node<PanelKind> {
    Node::Stack { entries: e.iter().map(|&(panel, open, height)| StackEntry { panel, open, height }).collect() }
}
/// Two frames side by side.
pub fn hsplit(size: SplitSize, a: Node<PanelKind>, b: Node<PanelKind>) -> Node<PanelKind> {
    Node::Split { axis: SplitAxis::Horizontal, size, first: Box::new(a), second: Box::new(b) }
}
/// Two frames one above the other.
pub fn vsplit(size: SplitSize, a: Node<PanelKind>, b: Node<PanelKind>) -> Node<PanelKind> {
    Node::Split { axis: SplitAxis::Vertical, size, first: Box::new(a), second: Box::new(b) }
}

/// A workspace of just `root`, with no floating windows.
pub fn docked(root: Node<PanelKind>) -> Workspace {
    Workspace::new(Layout { root: Some(root), floating: Vec::new() })
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
    let f = |id: u64, panels: &[PanelKind], rect: [f32; 4]| Floating::single(panels.to_vec(), 0, rect).with_id(id);
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
    Layout { root: Some(workspace_root(name)), floating: workspace_floating(name) }
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
        Node::Stack { entries } => out.extend(entries.iter().map(|e| e.panel)),
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
        Node::Stack { entries } => entries.iter().any(|e| e.panel == p).then(|| vec![p]),
        Node::Dock { columns, .. } => columns.iter().flat_map(|c| c.groups.iter()).find(|g| g.panels.contains(&p)).map(|g| g.panels.clone()),
        _ => None,
    }
}

impl WorkspaceExt for Workspace {
    fn is_docked(&self, p: PanelKind) -> bool {
        self.layout.root.as_ref().is_some_and(|root| node_group(root, p).is_some())
    }
    fn is_floating(&self, p: PanelKind) -> bool {
        self.layout.floating.iter().any(|w| w.contains(&p))
    }
    fn is_open(&self, p: PanelKind) -> bool {
        self.layout.contains(&p)
    }
    fn is_visible(&self, p: PanelKind) -> bool {
        self.layout.panel_state(&p) == PanelState::Front
    }
    fn group_of(&self, p: PanelKind) -> Vec<PanelKind> {
        self.layout.root.iter().chain(self.layout.floating.iter().map(|w| &w.root)).find_map(|node| node_group(node, p)).unwrap_or_default()
    }
    fn panels(&self) -> Vec<PanelKind> {
        let mut out = Vec::new();
        for node in self.layout.root.iter().chain(self.layout.floating.iter().map(|w| &w.root)) {
            node_panels(node, &mut out);
        }
        out
    }
}

/// Every panel a workspace names: open, or closed with a recorded place.
pub fn referenced_panels(workspace: &Workspace) -> Vec<PanelKind> {
    let mut out = workspace.panels();
    out.extend(workspace.hidden.iter().map(|(p, _)| *p));
    if let Some(saved) = &workspace.saved {
        for node in saved.root.iter().chain(saved.floating.iter().map(|w| &w.root)) {
            node_panels(node, &mut out);
        }
    }
    out
}

/// The automation id of the gap below stacked panel `p`: `dock.gutter.<path>s<i>`, with the
/// stack's split path (`a`/`b` per level) and the entry's index.
pub fn stack_gutter_id(layout: &Layout, p: PanelKind) -> Option<String> {
    fn rec(node: &Node<PanelKind>, p: PanelKind, path: &mut String, depth: usize) -> Option<String> {
        if depth > craft_ui::docking::MAX_DEPTH {
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
            Node::Stack { entries } => entries.iter().position(|e| e.panel == p).map(|i| format!("dock.gutter.{path}s{i}")),
            _ => None,
        }
    }
    rec(layout.root.as_ref()?, p, &mut String::new(), 0)
}

/// One document a panel shows as a tab of its own (the Timeline: one tab per open comp, as in
/// After Effects).
#[derive(Debug, Clone, PartialEq)]
pub struct DocTab {
    pub id: u64,
    pub title: String,
    /// The comp's label colour.
    pub swatch: egui::Color32,
    /// The document the panel shows now.
    pub active: bool,
}

/// What the Timeline's comp tabs did this frame.
#[derive(Debug, Clone, PartialEq)]
pub enum DocAction {
    /// Show comp `id`.
    Activate(u64),
    /// Close comp `id`'s Timeline (its ×, or a middle click).
    Close(u64),
}

/// Room around a tab's label, as craft-ui gives its dock tabs.
const TAB_PADDING: f32 = 24.0;
/// Width of the » button that lists the comp tabs with no room in the strip.
const DOC_OVERFLOW: f32 = 22.0;
/// Room for the × after a comp's name. Every comp tab keeps it, so a tab keeps its width when it
/// is shown or hovered.
const DOC_CLOSE: f32 = 16.0;

/// A comp tab's text: the label swatch, then the name, as the panel tabs with a swatch show it.
fn doc_job(doc: &DocTab, font: &egui::FontId, color: egui::Color32) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    job.append("\u{25a0} ", 0.0, egui::TextFormat::simple(font.clone(), doc.swatch));
    job.append(&doc.title, 0.0, egui::TextFormat::simple(font.clone(), color));
    job
}

/// The width of one line of `job` at the fonts' current size.
fn text_width(ctx: &egui::Context, job: egui::text::LayoutJob) -> f32 {
    ctx.fonts_mut(|f| f.layout_job(job)).size().x
}

/// The width each comp tab asks for in the Timeline group's tab row.
pub fn doc_widths(ctx: &egui::Context, docs: &[DocTab], font: &egui::FontId) -> Vec<f32> {
    docs.iter().map(|d| text_width(ctx, doc_job(d, font, egui::Color32::WHITE)) + TAB_PADDING + DOC_CLOSE).collect()
}

/// The × of comp tab `doc` at `rect`, clipped to `clip`; true when clicked. `shown` is the comp
/// the Timeline shows while it is the group's shown panel. Automation:
/// `panel.tab.Timeline.<comp>.close`, and `panel.tab.Timeline.close` for the shown comp's.
fn doc_close(
    ui: &mut egui::Ui,
    rect: Rect,
    clip: Rect,
    doc: &DocTab,
    shown: bool,
    style: &craft_ui::docking::DockStyle,
    reg: &mut crate::automation::Registry,
) -> bool {
    let timeline = PanelKind::Timeline.id();
    let hit = ui.interact(rect.expand(2.0), egui::Id::unique(("comp-tab-close", doc.id)), Sense::click());
    hit.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, hit.enabled(), "Close"));
    let cc = if hit.hovered() { style.text } else { style.inactive_text };
    let k = 3.5;
    let painter = ui.painter().with_clip_rect(clip);
    painter.line_segment([rect.center() + vec2(-k, -k), rect.center() + vec2(k, k)], Stroke::new(1.2, cc));
    painter.line_segment([rect.center() + vec2(-k, k), rect.center() + vec2(k, -k)], Stroke::new(1.2, cc));
    reg.add_response(&format!("panel.tab.{timeline}.{}.close", doc.id), &hit, "Close");
    if shown {
        reg.add_response(&format!("panel.tab.{timeline}.close"), &hit, "Close");
    }
    hit.clicked()
}

/// Draw the Timeline's comp tabs into `strip`, the part of the Timeline group's tab row after its
/// panel tabs. Each is a tab like the panel tabs: the comp's label swatch and name, then a ×
/// that closes the comp's Timeline: always on the shown comp while the Timeline is the group's
/// shown panel (`selected`), on the other comp tabs while the pointer is over them. Tabs that do
/// not fit go to a » menu at the end; the shown comp's tab always stays in the row. Shift+, and
/// Shift+. step through every open comp. Automation: `panel.tab.Timeline.<comp>`, the ×s (see
/// [`doc_close`]), `panel.tab.Timeline.more` for », and `panel.tab.Timeline.more.<comp>` for its
/// items.
pub fn doc_tabs(
    ui: &mut egui::Ui,
    strip: Rect,
    docs: &[DocTab],
    selected: bool,
    style: &craft_ui::docking::DockStyle,
    reg: &mut crate::automation::Registry,
) -> Vec<DocAction> {
    let mut out = Vec::new();
    let timeline = PanelKind::Timeline.id();
    let widths = doc_widths(ui.ctx(), docs, &style.font);
    let active = docs.iter().position(|d| d.active).unwrap_or(0);
    let fit = craft_ui::tabs::fit_whole(&widths, active, strip.width(), DOC_OVERFLOW);
    let clip = strip.intersect(ui.clip_rect());
    let mid = strip.center().y;
    let mut x = strip.min.x;
    for (index, width) in &fit.shown {
        let Some(doc) = docs.get(*index) else { continue };
        let tab = Rect::from_min_size(pos2(x, strip.min.y), vec2(*width, strip.height()));
        x += width;
        let on = selected && doc.active;
        let color = if on { style.text } else { style.inactive_text };
        let mut job = doc_job(doc, &style.font, color);
        job.wrap = egui::text::TextWrapping {
            max_width: (tab.width() - TAB_PADDING - DOC_CLOSE).max(0.0),
            max_rows: 1,
            break_anywhere: true,
            overflow_character: Some('\u{2026}'),
        };
        let galley = ui.fonts_mut(|f| f.layout_job(job));
        let label_end = tab.min.x + TAB_PADDING * 0.5 + galley.size().x;
        let resp = craft_ui::tabs::Tab::new(egui::Id::unique(("comp-tab", doc.id)), &doc.title, on).focus_stroke(Stroke::new(1.0, style.accent)).show_at(
            ui,
            tab,
            |ui, response| {
                let painter = ui.painter().with_clip_rect(tab.intersect(clip));
                painter.rect_filled(tab, 0.0, if on || response.hovered() { style.active_background } else { style.tab_background });
                painter.galley(pos2(tab.min.x + TAB_PADDING * 0.5, mid - galley.size().y * 0.5), galley.clone(), color);
            },
        );
        let resp = if galley.elided { resp.on_hover_text(&doc.title) } else { resp };
        reg.add_response(&format!("panel.tab.{timeline}.{}", doc.id), &resp, &doc.title);
        // After the tab, so the × takes the clicks over it.
        if on || ui.rect_contains_pointer(tab.intersect(clip)) {
            let cr = Rect::from_center_size(pos2(label_end + DOC_CLOSE / 2.0, mid), vec2(10.0, 10.0));
            if doc_close(ui, cr, tab.intersect(clip), doc, on, style, reg) {
                out.push(DocAction::Close(doc.id));
            }
        }
        if resp.clicked() {
            out.push(DocAction::Activate(doc.id));
        }
        if resp.middle_clicked() {
            out.push(DocAction::Close(doc.id));
        }
    }
    if !fit.overflow.is_empty() && fit.overflow_width > 0.0 {
        let more = Rect::from_min_size(pos2(x, strip.min.y), vec2(fit.overflow_width, strip.height()));
        let resp = ui.interact(more, egui::Id::unique("comp-tabs-more"), Sense::click());
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, resp.enabled(), "More compositions"));
        reg.add_response(&format!("panel.tab.{timeline}.more"), &resp, "More compositions");
        let color = if resp.hovered() { style.text } else { style.inactive_text };
        ui.painter().with_clip_rect(more.intersect(clip)).text(more.center(), Align2::CENTER_CENTER, "\u{bb}", style.font.clone(), color);
        egui::Popup::menu(&resp).show(|ui| {
            egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                for index in &fit.overflow {
                    let Some(doc) = docs.get(*index) else { continue };
                    let item = ui.button(doc_job(doc, &style.font, style.text));
                    item.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, item.enabled(), &doc.title));
                    reg.add_response(&format!("panel.tab.{timeline}.more.{}", doc.id), &item, &doc.title);
                    if item.clicked() {
                        out.push(DocAction::Activate(doc.id));
                        ui.close();
                    }
                }
            });
        });
    }
    out
}

/// A viewer lock button (Composition viewers and the Timeline keep their comp while locked): a
/// closed padlock while locked, an open one while unlocked.
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
    use craft_ui::docking::{Action, Placement, Zone};

    fn apply(w: &mut Workspace, action: Action<PanelKind>) {
        w.apply(action, permissions).unwrap();
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
            assert!(layout.root.iter().chain(layout.floating.iter().map(|f| &f.root)).all(no_stack), "{name}");
        }
    }

    #[test]
    fn default_right_column_is_a_short_preview_frame_over_the_properties_frame() {
        let default = workspace("Default");
        let Some(Node::Split { second, size: SplitSize::FixedSecond(300.0), .. }) = &default.root else { panic!("{default:?}") };
        let Node::Split { axis: SplitAxis::Vertical, size: SplitSize::FixedFirst(PREVIEW_FRAME), first, second } = second.as_ref() else {
            panic!("{second:?}")
        };
        assert_eq!(**first, tabs(&[Preview], 0));
        assert_eq!(**second, tabs(&[Properties, Align, Audio, EffectsPresets], 0));
        let mut w = Workspace::new(default);
        assert!(w.is_visible(Preview) && w.is_visible(Properties) && !w.is_visible(Align));
        // The Properties frame takes tabs; the Preview frame stays a frame of its own.
        apply(&mut w, Action::Move { panel: Project, anchor: Properties, placement: Placement::Tab { before: None } });
        assert_eq!(w.group_of(Project), vec![Properties, Align, Audio, EffectsPresets, Project]);
        assert_eq!(w.group_of(Preview), vec![Preview]);
    }

    #[test]
    fn panels_split_off_and_join_tab_groups() {
        let mut w = docked(hsplit(SplitSize::FixedFirst(300.0), tabs(&[Project, EffectControls], 0), tabs(&[Composition, Layer], 0)));
        apply(&mut w, Action::Move { panel: EffectControls, anchor: Composition, placement: Placement::Split(Zone::Right) });
        assert_eq!(w.group_of(EffectControls), vec![EffectControls]);
        assert_eq!(w.group_of(Project), vec![Project]);
        apply(&mut w, Action::Move { panel: EffectControls, anchor: Composition, placement: Placement::Tab { before: Some(Layer) } });
        assert_eq!(w.group_of(Composition), vec![Composition, EffectControls, Layer]);
        assert!(w.is_visible(EffectControls));
        // A rejected move (onto itself, onto a closed panel) changes nothing.
        let before = w.clone();
        assert!(w.apply(Action::Move { panel: Project, anchor: Info, placement: Placement::Split(Zone::Left) }, permissions).is_err());
        assert_eq!(w, before);
    }

    #[test]
    fn closed_panels_reopen_where_they_were_docked_or_floating() {
        let mut w = Workspace::new(workspace("Standard"));
        apply(&mut w, Action::Close { panel: Info });
        assert!(!w.is_open(Info));
        w.set_visible(Info, true, None, permissions).unwrap();
        assert_eq!(w.group_of(Info), vec![Info, Audio]);
        apply(&mut w, Action::Float { panel: Audio, rect: [40.0, 50.0, 300.0, 200.0] });
        apply(&mut w, Action::Close { panel: Audio });
        w.set_visible(Audio, true, None, permissions).unwrap();
        assert!(w.is_floating(Audio));
        // Closing an earlier, inactive tab keeps the shown panel.
        let mut w = docked(tabs(&[Properties, Align, EffectsPresets], 2));
        apply(&mut w, Action::Close { panel: Properties });
        assert!(w.is_visible(EffectsPresets));
        // A panel never laid out opens beside its anchor.
        apply(&mut w, Action::Open { panel: History, anchor: Some(Align) });
        assert_eq!(w.group_of(History), vec![Align, EffectsPresets, History]);
        assert_eq!(referenced_panels(&w).iter().filter(|p| **p == Properties).count(), 1, "the closed panel is still known");
    }
}
