//! Frontend view state (not project data). Serde so the control channel can read and set it.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::dock::{DockNode, PanelKind};
use crate::icons::Icon;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Tool {
    #[default]
    Selection,
    Hand,
    Zoom,
    Orbit,
    PanCamera,
    Dolly,
    Rotate,
    PanBehind,
    Rectangle,
    RoundedRect,
    Ellipse,
    Polygon,
    Star,
    Pen,
    PenAdd,
    PenDelete,
    PenConvert,
    MaskFeather,
    Type,
    TypeVertical,
    Brush,
    Clone,
    Eraser,
    RotoBrush,
    RefineEdge,
    Puppet,
    PuppetStarch,
    PuppetBend,
    PuppetAdvanced,
    PuppetOverlap,
}

impl Tool {
    /// Toolbar slots: each slot shows its current tool; the slot cycles with its shortcut.
    pub const SLOTS: &'static [&'static [Tool]] = &[
        &[Tool::Selection],
        &[Tool::Hand],
        &[Tool::Zoom],
        &[Tool::Orbit],
        &[Tool::PanCamera],
        &[Tool::Dolly],
        &[Tool::Rotate],
        &[Tool::PanBehind],
        &[Tool::Rectangle, Tool::RoundedRect, Tool::Ellipse, Tool::Polygon, Tool::Star],
        &[Tool::Pen, Tool::PenAdd, Tool::PenDelete, Tool::PenConvert, Tool::MaskFeather],
        &[Tool::Type, Tool::TypeVertical],
        &[Tool::Brush],
        &[Tool::Clone],
        &[Tool::Eraser],
        &[Tool::RotoBrush, Tool::RefineEdge],
        &[Tool::Puppet, Tool::PuppetStarch, Tool::PuppetBend, Tool::PuppetAdvanced, Tool::PuppetOverlap],
    ];
    pub const ALL: [Tool; 30] = [
        Tool::Selection,
        Tool::Hand,
        Tool::Zoom,
        Tool::Orbit,
        Tool::PanCamera,
        Tool::Dolly,
        Tool::Rotate,
        Tool::PanBehind,
        Tool::Rectangle,
        Tool::RoundedRect,
        Tool::Ellipse,
        Tool::Polygon,
        Tool::Star,
        Tool::Pen,
        Tool::PenAdd,
        Tool::PenDelete,
        Tool::PenConvert,
        Tool::MaskFeather,
        Tool::Type,
        Tool::TypeVertical,
        Tool::Brush,
        Tool::Clone,
        Tool::Eraser,
        Tool::RotoBrush,
        Tool::RefineEdge,
        Tool::Puppet,
        Tool::PuppetStarch,
        Tool::PuppetBend,
        Tool::PuppetAdvanced,
        Tool::PuppetOverlap,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Tool::Selection => "Selection Tool",
            Tool::Hand => "Hand Tool",
            Tool::Zoom => "Zoom Tool",
            Tool::Orbit => "Orbit Around Cursor Tool",
            Tool::PanCamera => "Pan Under Cursor Tool",
            Tool::Dolly => "Dolly Towards Cursor Tool",
            Tool::Rotate => "Rotation Tool",
            Tool::PanBehind => "Pan Behind (Anchor Point) Tool",
            Tool::Rectangle => "Rectangle Tool",
            Tool::RoundedRect => "Rounded Rectangle Tool",
            Tool::Ellipse => "Ellipse Tool",
            Tool::Polygon => "Polygon Tool",
            Tool::Star => "Star Tool",
            Tool::Pen => "Pen Tool",
            Tool::PenAdd => "Add Vertex Tool",
            Tool::PenDelete => "Delete Vertex Tool",
            Tool::PenConvert => "Convert Vertex Tool",
            Tool::MaskFeather => "Mask Feather Tool",
            Tool::Type => "Horizontal Type Tool",
            Tool::TypeVertical => "Vertical Type Tool",
            Tool::Brush => "Brush Tool",
            Tool::Clone => "Clone Stamp Tool",
            Tool::Eraser => "Eraser Tool",
            Tool::RotoBrush => "Roto Brush Tool",
            Tool::RefineEdge => "Refine Edge Tool",
            Tool::Puppet => "Puppet Position Pin Tool",
            Tool::PuppetStarch => "Puppet Starch Pin Tool",
            Tool::PuppetBend => "Puppet Bend Pin Tool",
            Tool::PuppetAdvanced => "Puppet Advanced Pin Tool",
            Tool::PuppetOverlap => "Puppet Overlap Pin Tool",
        }
    }
    pub fn shortcut(self) -> Option<&'static str> {
        match self {
            Tool::Selection => Some("V"),
            Tool::Hand => Some("H"),
            Tool::Zoom => Some("Z"),
            Tool::Orbit => Some("1"),
            Tool::PanCamera => Some("2"),
            Tool::Dolly => Some("3"),
            Tool::Rotate => Some("W"),
            Tool::PanBehind => Some("Y"),
            Tool::Rectangle | Tool::RoundedRect | Tool::Ellipse | Tool::Polygon | Tool::Star => Some("Q"),
            Tool::Pen | Tool::PenAdd | Tool::PenDelete | Tool::PenConvert | Tool::MaskFeather => Some("G"),
            Tool::Type | Tool::TypeVertical => Some("Cmd+T"),
            Tool::Brush | Tool::Clone | Tool::Eraser => Some("Cmd+B"),
            Tool::RotoBrush | Tool::RefineEdge => Some("Alt+W"),
            Tool::Puppet | Tool::PuppetStarch | Tool::PuppetBend | Tool::PuppetAdvanced | Tool::PuppetOverlap => Some("Cmd+P"),
        }
    }
    pub fn icon(self) -> Icon {
        match self {
            Tool::Selection => Icon::Selection,
            Tool::Hand => Icon::Hand,
            Tool::Zoom => Icon::Zoom,
            Tool::Orbit => Icon::Orbit,
            Tool::PanCamera => Icon::PanCamera,
            Tool::Dolly => Icon::Dolly,
            Tool::Rotate => Icon::Rotate,
            Tool::PanBehind => Icon::PanBehind,
            Tool::Rectangle => Icon::Rectangle,
            Tool::RoundedRect => Icon::RoundedRect,
            Tool::Ellipse => Icon::Ellipse,
            Tool::Polygon => Icon::Polygon,
            Tool::Star => Icon::Star,
            Tool::Pen => Icon::Pen,
            Tool::PenAdd => Icon::PenAdd,
            Tool::PenDelete => Icon::PenDelete,
            Tool::PenConvert => Icon::PenConvert,
            Tool::MaskFeather => Icon::MaskFeather,
            Tool::Type => Icon::Type,
            Tool::TypeVertical => Icon::TypeVertical,
            Tool::Brush => Icon::Brush,
            Tool::Clone => Icon::Clone,
            Tool::Eraser => Icon::Eraser,
            Tool::RotoBrush => Icon::RotoBrush,
            Tool::RefineEdge => Icon::RefineEdge,
            Tool::Puppet => Icon::Puppet,
            Tool::PuppetStarch => Icon::PuppetStarch,
            Tool::PuppetBend => Icon::PuppetBend,
            Tool::PuppetAdvanced => Icon::PuppetAdvanced,
            Tool::PuppetOverlap => Icon::PuppetOverlap,
        }
    }
    pub fn from_name(s: &str) -> Option<Tool> {
        let n = s.to_ascii_lowercase().replace([' ', '_', '-'], "");
        Tool::ALL.into_iter().find(|t| format!("{t:?}").to_ascii_lowercase() == n || t.label().to_ascii_lowercase().replace(' ', "").starts_with(&n))
    }
    /// Puppet pin tools, with the pin kind they place (`puppet.addPin` kind).
    pub fn puppet_kind(self) -> Option<&'static str> {
        Some(match self {
            Tool::Puppet => "position",
            Tool::PuppetStarch => "starch",
            Tool::PuppetBend => "bend",
            Tool::PuppetAdvanced => "advanced",
            Tool::PuppetOverlap => "overlap",
            _ => return None,
        })
    }
    /// Roto Brush / Refine Edge: the stroke kind they paint (`roto.stroke` kind), without and
    /// with Alt/Option.
    pub fn roto_kind(self, alt: bool) -> Option<&'static str> {
        Some(match (self, alt) {
            (Tool::RotoBrush, false) => "fg",
            (Tool::RotoBrush, true) => "bg",
            (Tool::RefineEdge, false) => "refine",
            (Tool::RefineEdge, true) => "refineErase",
            _ => return None,
        })
    }
    /// Paint tools, with their stroke kind (`paint.stroke` kind).
    pub fn paint_kind(self) -> Option<&'static str> {
        Some(match self {
            Tool::Brush => "brush",
            Tool::Clone => "clone",
            Tool::Eraser => "eraser",
            _ => return None,
        })
    }
    /// The Pen slot's tools (Pen, Add / Delete / Convert Vertex, Mask Feather).
    pub fn is_pen(self) -> bool {
        matches!(self, Tool::Pen | Tool::PenAdd | Tool::PenDelete | Tool::PenConvert | Tool::MaskFeather)
    }
    pub fn is_shape(self) -> bool {
        matches!(self, Tool::Rectangle | Tool::RoundedRect | Tool::Ellipse | Tool::Polygon | Tool::Star)
    }
}

/// Viewer resolution (Auto is sharp at rest and zoom-based during playback/dragging).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Resolution {
    #[default]
    Auto,
    Full,
    Half,
    Third,
    Quarter,
    /// View ▸ Resolution ▸ Custom: render every n-th pixel (2..=40).
    Custom(u8),
}

impl Resolution {
    pub const ALL: [Resolution; 5] = [Resolution::Auto, Resolution::Full, Resolution::Half, Resolution::Third, Resolution::Quarter];
    pub fn label(self) -> &'static str {
        match self {
            Resolution::Auto => "Auto",
            Resolution::Full => "Full",
            Resolution::Half => "Half",
            Resolution::Third => "Third",
            Resolution::Quarter => "Quarter",
            Resolution::Custom(_) => "Custom",
        }
    }
    /// Render scale for a viewer magnification (and display pixel density).
    pub fn scale(self, zoom: f32, ppp: f32) -> f64 {
        match self {
            // As in After Effects, Auto renders the pixels the magnification needs: the coarsest
            // whole factor that still gives every screen pixel a rendered one (Full above 50 %,
            // Half down to 33.3 %, Third down to 25 %, Quarter below), never fewer (#417).
            Resolution::Auto => {
                let n = (1.0 / (zoom * ppp) as f64 + 1e-3).floor();
                if n >= 1.0 { 1.0 / n.min(4.0) } else { 1.0 }
            }
            Resolution::Full => 1.0,
            Resolution::Half => 0.5,
            Resolution::Third => 1.0 / 3.0,
            Resolution::Quarter => 0.25,
            Resolution::Custom(n) => 1.0 / n.max(1) as f64,
        }
    }

    /// Full resolution for a paused frame, limited to roughly 4K worth of pixels.
    /// Large comps use whole-pixel downsample factors to bound memory use.
    pub fn paused_auto_scale(width: u32, height: u32) -> f64 {
        const MAX_PIXELS: f64 = 4096.0 * 2160.0;
        let pixels = u64::from(width) * u64::from(height);
        let factor = ((pixels as f64 / MAX_PIXELS).sqrt().ceil() as u32).clamp(1, 40);
        1.0 / f64::from(factor)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ViewerState {
    /// Magnification (1 = 100%); None = fit.
    pub zoom: Option<f32>,
    /// Pan offset in points from the centred position.
    pub pan: [f32; 2],
    pub res: Resolution,
    pub transparency_grid: bool,
    pub show_masks: bool,
    pub safe_margins: bool,
    pub grid: bool,
    pub rulers: bool,
    pub channel: String,
    pub show_layer_controls: bool,
    pub fast_preview: bool,
    /// View ▸ Show Guides / Snap to Guides / Lock Guides / Snap to Grid.
    pub guides: bool,
    pub snap_guides: bool,
    pub lock_guides: bool,
    pub snap_grid: bool,
    /// Grid and guide options ▸ Proportional Grid.
    pub proportional_grid: bool,
    /// Rulers zero point (comp pixels), moved by dragging the ruler corner.
    pub ruler_origin: [f64; 2],
    /// Region of Interest button armed: the next drag in the viewer draws the region.
    pub roi_draw: bool,
    /// The pointer is dragging in the viewer this frame (Adaptive Resolution).
    #[serde(skip)]
    pub interacting: bool,
    #[serde(skip)]
    pub property_interacting: bool,
    /// Extended Viewer: the comp-space region the viewer renders this frame (comp frame plus
    /// the visible pasteboard), set while a 3D view shows past the frame.
    #[serde(skip)]
    pub extended: Option<[f64; 4]>,
    /// View ▸ Panel Background Color (`None` = theme default).
    pub pasteboard: Option<[u8; 3]>,
    pub custom_pasteboard: [u8; 3],
}

impl Default for ViewerState {
    fn default() -> Self {
        ViewerState {
            zoom: None,
            pan: [0.0, 0.0],
            res: Resolution::Auto,
            transparency_grid: false,
            show_masks: true,
            safe_margins: false,
            grid: false,
            rulers: false,
            channel: "RGB".into(),
            show_layer_controls: true,
            fast_preview: false,
            guides: true,
            snap_guides: false,
            lock_guides: false,
            snap_grid: false,
            proportional_grid: false,
            ruler_origin: [0.0, 0.0],
            roi_draw: false,
            interacting: false,
            property_interacting: false,
            extended: None,
            pasteboard: None,
            custom_pasteboard: [0x80, 0x80, 0x80],
        }
    }
}

impl TimelineState {
    /// Show `kinds` on `layers` (twirled open; empty: twirled closed) and remember it as the last
    /// reveal. Other layers keep theirs.
    pub fn apply_reveal(&mut self, layers: &[u64], kinds: Vec<String>) {
        for id in layers {
            if kinds.is_empty() {
                self.open_layers.remove(id);
                self.layer_reveal.remove(id);
            } else {
                self.open_layers.insert(*id);
                self.layer_reveal.insert(*id, kinds.clone());
            }
        }
        self.reveal = kinds;
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TimelineState {
    /// Visible span start (seconds) and pixels per second; None = fit comp.
    pub start: f64,
    pub pps: Option<f64>,
    pub scroll_y: f32,
    /// Left column area width.
    pub columns_w: f32,
    /// Horizontal scroll of the outline columns (pixels) when they are wider than the pane.
    #[serde(default)]
    pub outline_scroll: f32,
    pub show_modes: bool,
    pub graph_editor: bool,
    pub search: String,
    /// Twirled-open layers and groups (by layer id / group uid).
    pub open_layers: BTreeSet<u64>,
    pub open_groups: BTreeSet<u64>,
    /// The last reveal shortcut's filter (P/S/R/T/A…): what pressing it again toggles or adds to.
    pub reveal: Vec<String>,
    /// Each revealed layer's filter: an open layer without one shows its whole property tree. A
    /// reveal shortcut sets it on the selected layers (all layers with none selected) and leaves
    /// the others as they are.
    #[serde(default)]
    pub layer_reveal: BTreeMap<u64, Vec<String>>,
    /// Properties / groups (uids) shown by the `props` reveal (Animation ▸ Reveal Properties…).
    #[serde(default)]
    pub reveal_props: BTreeSet<u64>,
    /// Graph Editor: `auto` (Auto-Select Graph Type, the default: the speed graph when every
    /// property shown is spatial, else the value graph), `value` or `speed`.
    #[serde(default = "auto_graph")]
    pub graph_mode: String,
    /// Show only the selected properties (else every animated property of the selected layers).
    #[serde(default = "yes")]
    pub graph_show_selected: bool,
    /// Auto-zoom the graph height to the visible curves.
    #[serde(default = "yes")]
    pub graph_auto_zoom: bool,
    /// Manual graph value range (when auto-zoom is off).
    #[serde(default)]
    pub graph_range: Option<(f64, f64)>,
    /// Graph Editor ▸ Snap: key drags snap to the current time, other keys and frames.
    #[serde(default = "yes")]
    pub graph_snap: bool,
    /// Graph Editor ▸ Show Reference Graph (the other graph type, faint, behind).
    #[serde(default)]
    pub graph_reference: bool,
    /// Graph Editor ▸ Show Transform Box When Multiple Keys Are Selected.
    #[serde(default = "yes")]
    pub graph_transform_box: bool,
    /// Properties whose inline expression editor is collapsed.
    #[serde(default)]
    pub expr_closed: BTreeSet<u64>,
    /// Inline expression editors given a height (in lines) by dragging their bottom edge;
    /// the others grow with their text, up to 8 lines.
    #[serde(default)]
    pub expr_lines: BTreeMap<u64, usize>,
    /// Scale and Mask Feather properties whose chain link (Constrain Proportions, on by default)
    /// was turned off.
    #[serde(default)]
    pub unlinked: BTreeSet<u64>,
    /// Visible optional columns (column header right-click ▸ Columns): `av`, `keys`, `label`,
    /// `num`, `comment`, `switches`, `parent`, `in`, `out`, `duration`, `stretch`. The name
    /// column is always shown; Modes follows `show_modes` (F4).
    #[serde(default = "default_tl_columns")]
    pub columns: BTreeSet<String>,
    /// The name column shows Source Name instead of Layer Name (click its header).
    #[serde(default)]
    pub source_name: bool,
}

pub fn default_tl_columns() -> BTreeSet<String> {
    ["av", "label", "num", "switches", "parent"].map(String::from).into_iter().collect()
}

fn auto_graph() -> String {
    "auto".into()
}
fn yes() -> bool {
    true
}

impl Default for TimelineState {
    fn default() -> Self {
        TimelineState {
            start: 0.0,
            pps: None,
            scroll_y: 0.0,
            columns_w: 560.0,
            outline_scroll: 0.0,
            show_modes: false,
            graph_editor: false,
            search: String::new(),
            open_layers: BTreeSet::new(),
            open_groups: BTreeSet::new(),
            reveal: vec![],
            layer_reveal: BTreeMap::new(),
            reveal_props: BTreeSet::new(),
            graph_mode: auto_graph(),
            graph_show_selected: true,
            graph_auto_zoom: true,
            graph_range: None,
            graph_snap: true,
            graph_reference: false,
            graph_transform_box: true,
            expr_closed: BTreeSet::new(),
            expr_lines: BTreeMap::new(),
            unlinked: BTreeSet::new(),
            columns: default_tl_columns(),
            source_name: false,
        }
    }
}

fn default_project_sort() -> String {
    "name".into()
}

pub fn default_project_columns() -> Vec<String> {
    ["type", "size", "duration", "fps"].map(String::from).to_vec()
}

/// What an Effect Controls crosshair / eyedropper click in the viewer sets.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FxPick {
    /// `point` (point parameter, layer space) or `color` (colour sampled from the frame).
    pub kind: String,
    pub layer: u64,
    pub prop: u64,
    /// Parameter name (for the viewer hint).
    pub name: String,
}

/// Effects & Presets contents-menu options.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EffectsView {
    /// `all`, `32` (32 bpc effects only) or `gpu` (GPU-accelerated only).
    pub depth: String,
    /// Show the "* Animation Presets" folder.
    pub presets: bool,
    /// Flat alphabetical list instead of categories.
    pub alphabetical: bool,
}

impl Default for EffectsView {
    fn default() -> Self {
        EffectsView { depth: "all".into(), presets: true, alphabetical: false }
    }
}

/// Wiggler, Smoother and Motion Sketch panel settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AnimToolsState {
    /// Wiggler Apply To: Spatial Path (true) or Temporal Path.
    pub wiggle_spatial: bool,
    /// Noise Type: Smooth (true) or Jagged.
    pub wiggle_smooth: bool,
    /// `one` (dimension `wiggle_dim`), `same` or `independent`.
    pub wiggle_dims: String,
    pub wiggle_dim: usize,
    pub wiggle_frequency: f64,
    pub wiggle_magnitude: f64,
    pub smooth_tolerance: f64,
    /// Motion Sketch: capture speed (%), smoothing, show wireframe / background.
    pub sketch_speed: f64,
    pub sketch_smoothing: f64,
    pub sketch_wireframe: bool,
    pub sketch_background: bool,
    /// Start Capture pressed: the next drag in the viewer records.
    pub sketch_armed: bool,
}

impl Default for AnimToolsState {
    fn default() -> Self {
        AnimToolsState {
            wiggle_spatial: true,
            wiggle_smooth: true,
            wiggle_dims: "independent".into(),
            wiggle_dim: 0,
            wiggle_frequency: 5.0,
            wiggle_magnitude: 1.0,
            smooth_tolerance: 1.0,
            sketch_speed: 100.0,
            sketch_smoothing: 1.0,
            sketch_wireframe: false,
            sketch_background: true,
            sketch_armed: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UiState {
    pub tool: Tool,
    /// Window ▸ Script Console input, output and history.
    #[serde(default)]
    pub script_console: crate::panels::script_console::ScriptConsole,
    /// Tool shown in each toolbar slot.
    pub slot_tools: Vec<Tool>,
    pub workspace: String,
    pub dock: DockNode,
    /// Saved workspace layouts (Save Changes to this Workspace / Save as New Workspace).
    #[serde(default)]
    pub saved_workspaces: std::collections::BTreeMap<String, DockNode>,
    /// Undocked (floating) panel groups.
    #[serde(default)]
    pub floating: Vec<crate::dock::Floating>,
    /// Floating panels of the saved workspaces.
    #[serde(default)]
    pub saved_floating: std::collections::BTreeMap<String, Vec<crate::dock::Floating>>,
    /// The panel maximized to fill the dock area (`~`), if any.
    #[serde(default)]
    pub maximized: Option<PanelKind>,
    /// Locked Composition / Timeline tabs (panel ids): opening another composition does not
    /// bring a locked panel forward (the tab's lock icon, as After Effects' viewer lock).
    #[serde(default)]
    pub locked_tabs: BTreeSet<String>,
    /// The comp each Composition viewer shows, by viewer id (0 = the Composition panel; see
    /// `panels::viewers`).
    #[serde(default)]
    pub viewers: std::collections::BTreeMap<u32, Option<u64>>,
    /// The active viewer (the interactive one, showing the active comp).
    #[serde(default)]
    pub active_viewer: u32,
    pub focused: PanelKind,
    pub viewer: ViewerState,
    pub timeline: TimelineState,
    pub theme: crate::theme::ThemeKind,
    pub show_menu_bar: bool,
    pub status: String,
    /// Effects & Presets search text.
    pub effects_search: String,
    pub effects_open: BTreeSet<String>,
    pub project_search: String,
    pub project_open_folders: BTreeSet<u64>,
    /// Project panel sort column (`name`, `type`, `size`, `fps`) and direction, as in AE's
    /// clickable column headers. Folders sort with everything else.
    #[serde(default = "default_project_sort")]
    pub project_sort: String,
    #[serde(default)]
    pub project_sort_desc: bool,
    /// Visible Project panel columns after Name and Label, in order (`type`, `size`, `fps`,
    /// `duration`, `path`, `comment`); toggled from the column header's context menu.
    #[serde(default = "default_project_columns")]
    pub project_columns: Vec<String>,
    /// Horizontal scroll of the Project panel's optional columns (Name and Label stay put).
    #[serde(default)]
    pub project_hscroll: f32,
    /// Vertical scroll of the Project panel's item list (points); only visible rows are drawn.
    #[serde(default)]
    pub project_scroll: f32,
    /// Effect Controls twirl state (group uids that are collapsed).
    pub fx_closed: BTreeSet<u64>,
    /// Slider params whose slider row is twirled open (AE hides sliders by default).
    #[serde(default)]
    pub fx_slider_open: BTreeSet<u64>,
    /// Effect Controls: a point crosshair or colour eyedropper waiting for a viewer click.
    #[serde(default)]
    pub fx_pick: Option<FxPick>,
    /// Curves editor: shown channel (index into RGB/Red/Green/Blue/Alpha) per effect uid.
    #[serde(default)]
    pub fx_curve_channel: BTreeMap<u64, usize>,
    /// Curves editors in pencil (freehand) mode, by effect uid.
    #[serde(default)]
    pub fx_curve_pencil: BTreeSet<u64>,
    /// Levels (Individual Controls) editor: shown channel per effect uid.
    #[serde(default)]
    pub fx_levels_channel: BTreeMap<u64, usize>,
    /// Effects & Presets: favourite effect ids (starred).
    #[serde(default)]
    pub effects_favorites: BTreeSet<String>,
    /// Effects & Presets: recently applied effect ids, most recent first.
    #[serde(default)]
    pub effects_recent: Vec<String>,
    /// Effects & Presets contents-menu view options.
    #[serde(default)]
    pub effects_view: EffectsView,
    pub snapping: bool,
    pub start_screen: bool,
    /// The Home screen shows its Learn tab (tutorials) instead of the recent projects.
    #[serde(default)]
    pub home_learn: bool,
    /// The Home screen shows its Templates tab (New from Template gallery).
    #[serde(default)]
    pub home_templates: bool,
    /// Align panel: Align Layers to Selection (off: to the composition).
    #[serde(default)]
    pub align_to_selection: bool,
    /// Composition ▸ Preview ▸ Cache Frames When Idle.
    pub cache_when_idle: bool,
    /// Tracker panel ▸ Motion Source chosen without a tracker yet (layer id).
    #[serde(default)]
    pub tracker_source: Option<u64>,
    /// Layer shown in the Layer panel (None = the first selected layer).
    #[serde(default)]
    pub layer_panel: Option<u64>,
    /// Layer panel View: number of effects rendered (None = through the last Paint effect).
    #[serde(default)]
    pub layer_view: Option<usize>,
    /// Wiggler / Smoother / Motion Sketch settings.
    #[serde(default)]
    pub anim_tools: AnimToolsState,
    /// Composition Mini-Flowchart popup position (open when set).
    #[serde(default)]
    pub mini_flowchart: Option<[f32; 2]>,
    /// Flowchart panel options (layers, effects, solids, direction, root comp).
    #[serde(default)]
    pub flowchart: crate::panels::flowchart::FlowOptions,
    /// Lumetri Scopes panel options.
    #[serde(default)]
    pub scopes: crate::panels::scopes_panel::ScopesState,
}

impl Default for UiState {
    fn default() -> Self {
        UiState {
            tool: Tool::Selection,
            script_console: Default::default(),
            slot_tools: Tool::SLOTS.iter().map(|s| s[0]).collect(),
            workspace: "Default".into(),
            dock: crate::dock::workspace("Default"),
            saved_workspaces: Default::default(),
            floating: vec![],
            saved_floating: Default::default(),
            maximized: None,
            locked_tabs: BTreeSet::new(),
            viewers: Default::default(),
            active_viewer: 0,
            focused: PanelKind::Composition,
            viewer: ViewerState::default(),
            timeline: TimelineState::default(),
            theme: Default::default(),
            show_menu_bar: true,
            status: String::new(),
            effects_search: String::new(),
            effects_open: BTreeSet::new(),
            project_search: String::new(),
            project_open_folders: BTreeSet::new(),
            project_sort: default_project_sort(),
            project_sort_desc: false,
            project_hscroll: 0.0,
            project_scroll: 0.0,
            project_columns: default_project_columns(),
            fx_closed: BTreeSet::new(),
            fx_slider_open: BTreeSet::new(),
            fx_pick: None,
            fx_curve_channel: BTreeMap::new(),
            fx_curve_pencil: BTreeSet::new(),
            fx_levels_channel: BTreeMap::new(),
            effects_favorites: BTreeSet::new(),
            effects_recent: Vec::new(),
            effects_view: EffectsView::default(),
            snapping: true,
            start_screen: false,
            home_learn: false,
            home_templates: false,
            align_to_selection: false,
            cache_when_idle: false,
            tracker_source: None,
            layer_panel: None,
            layer_view: None,
            anim_tools: AnimToolsState::default(),
            mini_flowchart: None,
            flowchart: Default::default(),
            scopes: Default::default(),
        }
    }
}
