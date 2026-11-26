//! EffectCraft's docking command vocabulary: `window.panel.*` and `ui.dock`, as the control
//! channel, the menus and the dock area's own changes write them. Each command becomes the
//! craft-behavior layout actions it stands for on the current workspace.

use craft_behavior::DockSide;
use craft_behavior::dock::{Action, DockEdge, DockError, DockPlacement, Layout, PanelPlace, SplitSize, Target, node_at};
use craft_ui::docking::FloatSpace;
use egui::{Rect, pos2, vec2};
use serde::{Deserialize, Deserializer};

use crate::dock::{Node, PanelKind, Workspace};

/// A drop zone by name. PascalCase names of the side zones also parse.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ZoneName {
    Tab,
    #[serde(alias = "Center")]
    Center,
    #[serde(alias = "Left")]
    Left,
    #[serde(alias = "Right")]
    Right,
    #[serde(alias = "Top")]
    Top,
    #[serde(alias = "Bottom")]
    Bottom,
    Above,
    Below,
}

/// A present field, `null` included: `Some(None)` for `null`, `None` when absent.
fn present<'de, D: Deserializer<'de>, T: Deserialize<'de>>(deserializer: D) -> Result<Option<T>, D::Error> {
    T::deserialize(deserializer).map(Some)
}

/// A required field that may be `null`.
fn required<'de, D: Deserializer<'de>, T: Deserialize<'de>>(deserializer: D) -> Result<T, D::Error> {
    T::deserialize(deserializer)
}

/// A `ui.dock` operation, tagged by `operation`. Rects are `[x, y, width, height]`.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "operation", rename_all = "camelCase")]
pub enum DockOperation {
    Open {
        panel: PanelKind,
    },
    Close {
        panel: PanelKind,
    },
    Activate {
        panel: PanelKind,
    },
    Float {
        panel: PanelKind,
        rect: [f32; 4],
    },
    MoveFloating {
        panel: PanelKind,
        rect: [f32; 4],
    },
    /// `zone` defaults to `center`; `before` only applies to a center or tab move.
    Move {
        panel: PanelKind,
        target: PanelKind,
        #[serde(default)]
        zone: Option<ZoneName>,
        #[serde(default, deserialize_with = "present")]
        before: Option<Option<PanelKind>>,
    },
    SetStackOpen {
        panel: PanelKind,
        open: bool,
    },
    /// `height` is required; `null` clears it.
    ResizeStack {
        panel: PanelKind,
        #[serde(deserialize_with = "required")]
        height: Option<f32>,
    },
    ResizeSplit {
        path: Vec<bool>,
        size: SplitSize,
    },
}

/// `ui.dock`: a raw layout action or an operation.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum UiDock {
    Action { action: Action<PanelKind> },
    Operation(DockOperation),
}

/// A docking command, tagged by `command` (its id).
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "command")]
pub enum DockCommand {
    /// A raw layout action.
    #[serde(rename = "window.panel.layout")]
    Layout { action: Action<PanelKind> },
    /// Move a panel (or its group, with `group`) next to `anchor`; `zone` defaults to `tab`.
    #[serde(rename = "window.panel.move")]
    Move {
        panel: PanelKind,
        anchor: PanelKind,
        #[serde(default)]
        zone: Option<ZoneName>,
        #[serde(default, deserialize_with = "present")]
        before: Option<Option<PanelKind>>,
        #[serde(default)]
        group: bool,
        #[serde(default)]
        placement: Option<DockPlacement<PanelKind>>,
    },
    /// Float a panel, its group or its column; given values replace the rect's.
    #[serde(rename = "window.panel.float")]
    Float {
        panel: PanelKind,
        #[serde(default)]
        x: Option<f32>,
        #[serde(default)]
        y: Option<f32>,
        #[serde(default)]
        width: Option<f32>,
        #[serde(default)]
        height: Option<f32>,
        #[serde(default)]
        group: bool,
        #[serde(default)]
        column: bool,
    },
    /// Dock a panel: beside `anchor` in `zone` (default `tab`), as the last column of the dock on
    /// `side`, along an outer edge (a side `zone` alone), or back into the docked layout.
    #[serde(rename = "window.panel.dock")]
    Dock {
        panel: PanelKind,
        #[serde(default)]
        anchor: Option<PanelKind>,
        #[serde(default)]
        zone: Option<ZoneName>,
        #[serde(default, deserialize_with = "present")]
        before: Option<Option<PanelKind>>,
        #[serde(default)]
        group: bool,
        #[serde(default)]
        column: bool,
        #[serde(default)]
        side: Option<DockEdge>,
    },
    #[serde(rename = "window.panel.activate")]
    Activate { panel: PanelKind },
    #[serde(rename = "window.panel.close")]
    Close { panel: PanelKind },
    #[serde(rename = "window.panel.closeGroup")]
    CloseGroup { panel: PanelKind },
    #[serde(rename = "window.panel.closeOthers")]
    CloseOthers { panel: PanelKind },
    /// Minimize or restore the panel's group; without `minimized` it toggles.
    #[serde(rename = "window.panel.minimize")]
    Minimize {
        panel: PanelKind,
        #[serde(default)]
        minimized: Option<bool>,
    },
    /// Collapse the panel's column to icons or expand it; without `iconic` it toggles.
    #[serde(rename = "window.panel.iconic")]
    Iconic {
        panel: PanelKind,
        #[serde(default)]
        iconic: Option<bool>,
    },
    /// The panel in a floating window of its own (`native` true, the default) or docked again.
    #[serde(rename = "window.panel.window")]
    Window {
        panel: PanelKind,
        #[serde(default)]
        native: Option<bool>,
    },
    #[serde(rename = "ui.dock")]
    UiDock(UiDock),
}

/// The size a command floats a panel at when the dock has drawn nothing yet.
const DEFAULT_FLOAT: egui::Vec2 = vec2(420.0, 320.0);

/// `[x, y, width, height]` as a rect.
pub fn rect_of([x, y, width, height]: [f32; 4]) -> Rect {
    Rect::from_min_size(pos2(x, y), vec2(width, height))
}

/// How much of the layout a command moves.
#[derive(Clone, Copy)]
enum Unit {
    Panel,
    Group,
    Column,
}

impl Unit {
    fn of(group: bool, column: bool) -> Self {
        if column {
            Self::Column
        } else if group {
            Self::Group
        } else {
            Self::Panel
        }
    }

    /// The action that moves this unit of `panel` to `to`. A floating panel's column is its whole window.
    fn to(self, layout: &Layout<PanelKind>, panel: PanelKind, to: Target<PanelKind>) -> Action<PanelKind> {
        let floating = matches!(layout.place(&panel), Some(PanelPlace::Floating { .. }));
        match (self, to) {
            (Self::Panel, Target::At { anchor, placement, .. }) => Action::move_beside(panel, anchor, placement),
            (Self::Panel, to) => Action::move_panel(panel, to),
            (Self::Group, to) => Action::move_group(panel, to),
            (Self::Column, to) if floating => Action::move_window(panel, to),
            (Self::Column, to) => Action::move_column(panel, to),
        }
    }

    /// The panels that move with `panel`.
    fn members(self, layout: &Layout<PanelKind>, panel: PanelKind) -> Vec<PanelKind> {
        let place = layout.place(&panel);
        match self {
            Self::Panel => vec![panel],
            Self::Group => match place.as_ref() {
                Some(PanelPlace::Node(_)) => match place.as_ref().and_then(|place| layout.node_of(place)) {
                    Some(Node::Tabs { panels, .. }) => panels.clone(),
                    _ => vec![panel],
                },
                Some(place) => layout.group(place).map(|group| group.panels.clone()).unwrap_or_else(|| vec![panel]),
                None => vec![panel],
            },
            Self::Column => match place.as_ref() {
                Some(place @ (PanelPlace::Dock { .. } | PanelPlace::Floating { .. })) => {
                    layout.column(place).map(|column| column.panels().copied().collect()).unwrap_or_else(|| vec![panel])
                }
                _ => vec![panel],
            },
        }
    }
}

/// Where a command puts what it moves relative to `anchor`: `zone` (default `default`) and
/// `before`, with side zones mapped onto the anchor's container by [`DockPlacement::for_zone`].
fn placement(
    layout: &Layout<PanelKind>,
    anchor: PanelKind,
    zone: Option<ZoneName>,
    before: Option<Option<PanelKind>>,
    default: ZoneName,
) -> Result<DockPlacement<PanelKind>, DockError> {
    let side = |side| DockPlacement::for_zone(layout, &anchor, side);
    Ok(match (zone.unwrap_or(default), before) {
        (ZoneName::Tab | ZoneName::Center, Some(before)) => DockPlacement::tab(before),
        (ZoneName::Tab, None) => DockPlacement::tab(None),
        (ZoneName::Center, None) => DockPlacement::Split(DockSide::Center),
        (_, Some(_)) => return Err(DockError::InvalidPlacement),
        (ZoneName::Left, None) => side(DockSide::Start)?,
        (ZoneName::Right, None) => side(DockSide::End)?,
        (ZoneName::Top, None) => side(DockSide::Top)?,
        (ZoneName::Bottom, None) => side(DockSide::Bottom)?,
        (ZoneName::Above, None) => DockPlacement::group(false, None),
        (ZoneName::Below, None) => DockPlacement::group(true, None),
    })
}

/// The panels of the last column of the dock on `side`, or `None` without such a dock.
fn last_column(layout: &Layout<PanelKind>, side: DockEdge) -> Option<Vec<PanelKind>> {
    let path = layout.dock_path(side)?;
    match node_at(layout.root.as_ref()?, &path)? {
        Node::Dock { columns, .. } => Some(columns.last()?.panels().copied().collect()),
        _ => None,
    }
}

impl DockCommand {
    /// Every `window.panel.*` command id.
    pub const IDS: &'static [&'static str] = &[
        "window.panel.layout",
        "window.panel.move",
        "window.panel.float",
        "window.panel.dock",
        "window.panel.activate",
        "window.panel.close",
        "window.panel.closeGroup",
        "window.panel.closeOthers",
        "window.panel.minimize",
        "window.panel.iconic",
        "window.panel.window",
    ];

    /// The command `id` with `params`. Its own `command` key is set from `id`.
    pub fn parse(id: &str, params: &serde_json::Value) -> Result<Self, String> {
        let mut tagged = if params.is_object() { params.clone() } else { serde_json::json!({}) };
        if let Some(fields) = tagged.as_object_mut() {
            fields.insert("command".into(), serde_json::json!(id));
        }
        serde_json::from_value(tagged).map_err(|error| error.to_string())
    }

    /// The raw layout action the command carries, if it is one.
    pub fn raw_action(&self) -> Option<&Action<PanelKind>> {
        match self {
            Self::Layout { action } | Self::UiDock(UiDock::Action { action }) => Some(action),
            _ => None,
        }
    }

    /// The panel the command shows: opening or activating it, or docking it back into the layout.
    pub fn shown(&self) -> Option<PanelKind> {
        match self {
            Self::Activate { panel }
            | Self::Dock { panel, anchor: None, side: None, .. }
            | Self::UiDock(UiDock::Operation(DockOperation::Open { panel } | DockOperation::Activate { panel })) => Some(*panel),
            _ => None,
        }
    }

    /// The outer edge `window.panel.dock` goes to: a side zone without an anchor or side.
    pub fn edge(&self) -> Option<DockSide> {
        match self {
            Self::Dock { anchor: None, side: None, zone: Some(zone), .. } => match zone {
                ZoneName::Left => Some(DockSide::Start),
                ZoneName::Right => Some(DockSide::End),
                ZoneName::Top => Some(DockSide::Top),
                ZoneName::Bottom => Some(DockSide::Bottom),
                _ => None,
            },
            _ => None,
        }
    }

    /// The layout actions this command makes on `workspace`, to apply all or none. A panel or
    /// anchor that is not shown is [`DockError::MissingPanel`] or [`DockError::MissingTarget`].
    /// `space` is where a float without a rect goes: the dock's last [`FloatSpace`].
    pub fn to_actions(&self, workspace: &Workspace, space: Option<&FloatSpace<PanelKind>>) -> Result<Vec<Action<PanelKind>>, DockError> {
        let layout = workspace.layout();
        let shown = |panel: &PanelKind| if layout.contains(panel) { Ok(()) } else { Err(DockError::MissingPanel) };
        let one = |action: Action<PanelKind>| Ok(vec![action]);
        match self {
            Self::Layout { action } | Self::UiDock(UiDock::Action { action }) => one(action.clone()),
            Self::Move { panel, anchor, zone, before, group, placement: given } => {
                shown(panel)?;
                let placement = match given {
                    Some(placement) => placement.clone(),
                    None => placement(layout, *anchor, *zone, *before, ZoneName::Tab)?,
                };
                one(Unit::of(*group, false).to(layout, *panel, Target::at(*anchor, placement)))
            }
            Self::Float { panel, x, y, width, height, group, column } => {
                shown(panel)?;
                let given = [*x, *y, *width, *height];
                let rect = |base: Rect| {
                    let mut values = [base.min.x, base.min.y, base.width(), base.height()];
                    for (value, given) in values.iter_mut().zip(given) {
                        *value = given.unwrap_or(*value);
                    }
                    rect_of(values)
                };
                let unit = Unit::of(*group, *column);
                let window = layout.floating.iter().find(|window| window.contains(panel));
                let lone = window.is_some_and(|window| window.panels().count() == 1);
                let whole = window.is_some_and(|window| window.groups().len() == 1);
                let placed = || {
                    let size = space.and_then(|space| space.panel(panel)).or(space.map(|space| space.default_size)).unwrap_or(DEFAULT_FLOAT);
                    rect(space.map_or_else(|| Rect::from_center_size(pos2(840.0, 510.0), size), |space| space.place(size)))
                };
                match (unit, window.map(|window| window.rect)) {
                    (Unit::Panel, Some(base)) if lone => one(Action::move_floating(*panel, rect(base))),
                    (Unit::Column, Some(base)) => one(Action::move_floating(*panel, rect(base))),
                    (Unit::Group, Some(base)) if whole => one(Action::move_floating(*panel, rect(base))),
                    (Unit::Panel, _) => one(Action::float(*panel, placed())),
                    (unit, _) => one(unit.to(layout, *panel, Target::float(placed()))),
                }
            }
            Self::Dock { panel, anchor, zone, before, group, column, side } => {
                shown(panel)?;
                let unit = Unit::of(*group, *column);
                if let Some(anchor) = anchor {
                    let placement = placement(layout, *anchor, *zone, *before, ZoneName::Tab)?;
                    return one(unit.to(layout, *panel, Target::at(*anchor, placement)));
                }
                let members = unit.members(layout, *panel);
                if let Some(side) = side {
                    let to = match last_column(layout, *side) {
                        Some(column) => match column.into_iter().find(|p| !members.contains(p)) {
                            Some(anchor) => Target::at(anchor, DockPlacement::column(true)),
                            // What moves already is that dock's last column.
                            None => return Ok(Vec::new()),
                        },
                        None => Target::new_dock(*side),
                    };
                    let unit = match (unit, &to) {
                        (Unit::Panel, Target::NewDock { .. }) if Unit::Group.members(layout, *panel).len() == 1 => Unit::Group,
                        (unit, _) => unit,
                    };
                    return one(unit.to(layout, *panel, to));
                }
                if let Some(zone) = self.edge() {
                    if before.is_some() {
                        return Err(DockError::InvalidPlacement);
                    }
                    // Every EffectCraft panel moves and closes: no edge narrows to a protected canvas.
                    let anchor = layout.edge_anchor(false, &|_| false, &|p| members.contains(p)).ok_or(DockError::MissingTarget)?;
                    return one(unit.to(layout, *panel, Target::at(anchor, DockPlacement::Edge(zone))));
                }
                if !matches!(layout.place(panel), Some(PanelPlace::Floating { .. })) {
                    return Ok(Vec::new());
                }
                let anchor = layout
                    .panels()
                    .into_iter()
                    .copied()
                    .filter(|p| !members.contains(p))
                    .find(|p| !matches!(layout.place(p), Some(PanelPlace::Floating { .. })))
                    .ok_or(DockError::MissingTarget)?;
                one(unit.to(layout, *panel, Target::at(anchor, DockPlacement::tab(None))))
            }
            Self::Activate { panel } => {
                shown(panel)?;
                one(Action::activate(*panel))
            }
            Self::Close { panel } => one(Action::close(*panel)),
            Self::CloseGroup { panel } => one(Action::close_group(*panel)),
            Self::CloseOthers { panel } => one(Action::close_others(*panel)),
            Self::Minimize { panel, minimized } => {
                let place = layout.place(panel).ok_or(DockError::MissingPanel)?;
                // An accordion entry is minimized while it is closed.
                let current = match &place {
                    PanelPlace::Node(path) => match layout.root.as_ref().and_then(|root| node_at(root, path)) {
                        Some(Node::Stack { entries, .. }) => entries.iter().any(|entry| entry.panel == *panel && !entry.open),
                        _ => false,
                    },
                    place => layout.group(place).is_some_and(|group| group.minimized),
                };
                one(Action::set_minimized(*panel, minimized.unwrap_or(!current)))
            }
            Self::Iconic { panel, iconic } => {
                let place = layout.place(panel).ok_or(DockError::MissingPanel)?;
                let current = layout.column(&place).is_some_and(|column| column.iconic);
                one(Action::set_iconic(*panel, iconic.unwrap_or(!current)))
            }
            // A floating window of its own is the app's (`EffectcraftApp::set_panel_window`).
            Self::Window { .. } => Err(DockError::NotAllowed),
            Self::UiDock(UiDock::Operation(operation)) => match operation {
                DockOperation::Open { panel } | DockOperation::Activate { panel } => {
                    shown(panel)?;
                    one(Action::activate(*panel))
                }
                DockOperation::Close { panel } => one(Action::close(*panel)),
                DockOperation::Float { panel, rect } => one(Action::float(*panel, rect_of(*rect))),
                DockOperation::MoveFloating { panel, rect } => one(Action::move_floating(*panel, rect_of(*rect))),
                DockOperation::Move { panel, target, zone, before } => {
                    one(Action::move_beside(*panel, *target, placement(layout, *target, *zone, *before, ZoneName::Center)?))
                }
                DockOperation::SetStackOpen { panel, open } => one(Action::set_stack_open(*panel, *open)),
                DockOperation::ResizeStack { panel, height } => one(Action::resize_stack(*panel, *height)),
                DockOperation::ResizeSplit { path, size } => one(Action::resize_split(path.clone(), *size)),
            },
        }
    }
}
