use super::*;

/// A keyframe reference: layer, property uid, key time (layer time).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct KeyRef {
    pub layer: LayerId,
    pub prop: Uid,
    pub time: Tick,
}

/// A keyframe drag in progress (`keys.move` steps sharing a merge key): the selection it
/// started from and the total offset so far, so every step is applied to the keys as they were
/// before the drag. A key passed over on the way is never overwritten; only the drop position
/// counts, as in After Effects.
#[derive(Clone, Debug)]
pub struct KeyMove {
    pub merge: String,
    pub from: Vec<KeyRef>,
    pub total: Tick,
}

/// Keyframes of one property on the keyframe clipboard.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KeyClip {
    pub layer: LayerId,
    /// Match-id path relative to the layer (`transform/position`), portable between layers.
    pub path: String,
    /// Keys with times relative to the earliest copied key (layer time).
    pub keys: Vec<effectcraft_keyframe::Keyframe>,
}

/// A selected mask vertex: layer, mask group uid, vertex index.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct VertexRef {
    pub layer: LayerId,
    pub mask: Uid,
    pub index: usize,
}

/// One composition's selection. The active comp's copy lives in [`EditorState::selected_layers`]
/// (and the property, key, and vertex lists); every other comp keeps its own here (#1001).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CompSelection {
    pub layers: Vec<LayerId>,
    pub props: Vec<(LayerId, Uid)>,
    pub keys: Vec<KeyRef>,
    pub vertices: Vec<VertexRef>,
}

impl CompSelection {
    pub(crate) fn is_empty(&self) -> bool {
        self.layers.is_empty() && self.props.is_empty() && self.keys.is_empty() && self.vertices.is_empty()
    }
}

/// Editing state that commands depend on (headless-relevant, serde for agents).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct EditorState {
    pub active_comp: Option<ItemId>,
    /// Open compositions (viewer/timeline tabs), in tab order.
    pub open_comps: Vec<ItemId>,
    /// Compositions that were active before the current one, most recent first (Shift+Esc).
    #[serde(default)]
    pub recent_comps: Vec<ItemId>,
    /// Current time per comp (the CTI).
    pub times: std::collections::BTreeMap<ItemId, Tick>,
    /// Layer selection of the active comp. Other comps' selections are in [`Self::comp_selections`].
    pub selected_layers: Vec<LayerId>,
    /// Selections of comps that are not active, keyed by comp id (#1001).
    #[serde(default)]
    pub comp_selections: std::collections::BTreeMap<ItemId, CompSelection>,
    /// Selected properties / groups (by uid) of selected layers.
    pub selected_props: Vec<(LayerId, Uid)>,
    pub selected_keys: Vec<KeyRef>,
    pub project_selection: Vec<ItemId>,
    /// Layer clipboard (serialized layers) and keyframe clipboard.
    #[serde(skip)]
    pub clipboard: Vec<Layer>,
    /// Copied keyframes (times relative to the earliest copied key).
    #[serde(skip)]
    pub key_clipboard: Vec<KeyClip>,
    /// The last copy was keyframes (Paste pastes keys at the CTI).
    #[serde(skip)]
    pub clip_is_keys: bool,
    /// Values of selected unanimated properties (match path, value), copied with Edit ▸ Copy;
    /// Paste sets the same properties of the selected layers.
    #[serde(skip)]
    pub value_clipboard: Vec<(String, effectcraft_keyframe::Value)>,
    /// Copied effect instances, with comp-time key offsets from the earliest copied key.
    /// Paste adds them to the selected layers at the destination comp's CTI.
    #[serde(skip)]
    pub effect_clipboard: Vec<effectcraft_project::PropGroup>,
    /// Copied shape items (Edit ▸ Copy with groups, paths, paints or path operations selected in
    /// a shape layer's Contents); Paste adds them to the selected shape layers.
    #[serde(skip)]
    pub contents_clipboard: Vec<effectcraft_project::PropGroup>,
    /// Selected mask vertices (viewer Selection tool / pen).
    #[serde(default)]
    pub selected_vertices: Vec<VertexRef>,
    pub snapping: bool,
    /// Tools bar ▸ Snapping options (which features snap, Snap Edges Extended).
    #[serde(default)]
    pub snap_features: viewer::SnapFeatures,
    /// Last applied effect id (Effect ▸ last effect).
    pub last_effect: Option<String>,
    /// Viewer region of interest `[x, y, w, h]` in comp pixels (Composition ▸ Crop Comp to Region
    /// of Interest).
    #[serde(default)]
    pub region_of_interest: Option<[f64; 4]>,
    /// Property-link clipboard (Edit ▸ Copy with Property Links / Copy Expression Only).
    #[serde(skip)]
    pub link_clipboard: Option<LinkClip>,
    /// File ▸ Interpret Footage ▸ Remember Interpretation.
    #[serde(skip)]
    pub interpretation: Option<effectcraft_project::Footage>,
    /// Viewer 3D view per comp (Active Camera / Front / … / Custom View 3 and edited view cameras).
    #[serde(default)]
    pub views3d: std::collections::BTreeMap<ItemId, effectcraft_render::three_d::Views3D>,
    /// Tracker panel ▸ Current Track: (tracked layer, tracker group uid) in the active comp.
    #[serde(default)]
    pub current_track: Option<(LayerId, Uid)>,
    /// Tracker panel ▸ Method in mask mode (Track Mask).
    #[serde(default)]
    pub mask_track_method: mask_track::MaskMethod,
    /// 3D Camera Tracker: the selected track points (ids) in the viewer.
    #[serde(default)]
    pub camera_points: Vec<u32>,
    /// Mask Interpolation panel options.
    #[serde(default)]
    pub mask_interp: commands::mask_interp::MaskInterpOptions,
    /// Paint and Brushes panel options (Brush, Clone Stamp and Eraser tools).
    #[serde(default)]
    pub paint: commands::paint::PaintOptions,
    /// Roto Brush and Refine Edge tool options.
    #[serde(default)]
    pub roto: roto::RotoOptions,
    /// Puppet tool options for new meshes.
    #[serde(default)]
    pub puppet: commands::puppet::PuppetOptions,
    /// The shape tools' and the Pen's options (Tool Creates Shape / Mask, Fill and Stroke).
    #[serde(default)]
    pub shape_tool: commands::shape_tool::ShapeTool,
    /// View ▸ Switch View Layout: 1, 2 or 4 views side by side in the Composition viewer.
    #[serde(default = "one_view")]
    pub view_layout: u8,
    /// View ▸ Switch View Layout ▸ Share View Options (grid, guides… in every view).
    #[serde(default)]
    pub share_view_options: bool,
    /// Layer ▸ Mask ▸ Hide Locked Masks (viewer outlines).
    #[serde(default)]
    pub hide_locked_masks: bool,
    /// On-canvas text editing (Type tool): the edited layer, selection and insertion style.
    #[serde(default)]
    pub text_edit: Option<commands::text_edit::TextEdit>,
    /// Text copied while editing (with its formatting), for Paste / Paste Text Formatting Only.
    #[serde(skip)]
    pub text_clipboard: Option<effectcraft_keyframe::TextDoc>,
    /// Composition viewer display options (Show Channel, exposure, snapshot, Fast Previews).
    #[serde(default)]
    pub viewer: commands::viewer_cmds::ViewOptions,
    /// Essential Graphics panel ▸ Primary composition (`None` = the active comp).
    #[serde(default)]
    pub essential_primary: Option<ItemId>,
    /// Essential Graphics panel ▸ Solo Supported Properties (the timeline shows only properties
    /// Essential Graphics can expose).
    #[serde(default)]
    pub essential_solo: bool,
    /// File ▸ Watch Folder: the folder being watched for projects to render (frontends call
    /// `file.watchFolder.poll` periodically while it is set).
    #[serde(default)]
    pub watch_folder: Option<String>,
    /// The Footage panel: the footage shown, its time and In/Out marks (source time).
    #[serde(default)]
    pub footage_panel: Option<commands::footage_panel::FootageView>,
    /// Media Browser: the folder shown and the favourites.
    #[serde(default)]
    pub media_browser: media_browser::BrowserState,
    /// Content-Aware Fill panel settings.
    #[serde(default)]
    pub content_fill: commands::content_fill::FillSettings,
    /// View ▸ Split with New Locked Viewer: the second viewer's comp and 3D view.
    #[serde(default)]
    pub locked_viewer: Option<commands::viewer_cmds::LockedViewer>,
}

fn one_view() -> u8 {
    1
}

/// What Copy with Property Links / Copy Expression Only put on the clipboard.
#[derive(Clone, Debug, PartialEq)]
pub enum LinkClip {
    /// Expressions that link back to the source properties: (match path, expression text).
    Links { relative: bool, links: Vec<(String, String)> },
    /// Expressions only: (match path, expression).
    Expressions(Vec<(String, effectcraft_project::Expression)>),
}
