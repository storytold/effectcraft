//! Precomposition workflow: the Pre-compose dialog (Leave / Move all attributes, Adjust
//! composition duration, Open New Composition), the Composition Navigator bar above the viewer
//! (the flow of nested comps, click to open) and the Composition Mini-Flowchart popup (Tab).

use crate::i18n::{tr, tr_args};
use effectcraft_engine::project::{ItemId, LayerSource, Project};
use egui::{Align2, Color32, Rect, Sense, Stroke, pos2, vec2};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;

use crate::theme::Tokens;
use crate::{Dialog, EffectcraftApp};

// ---------------------------------------------------------------- flow graph

/// Comps that contain a layer whose source is `cid` (upstream), in project order.
pub fn upstream(project: &Project, cid: ItemId) -> Vec<ItemId> {
    project.comps().filter(|(_, c)| c.layers.iter().any(|l| l.source == LayerSource::Comp { item: cid })).map(|(id, _)| *id).collect()
}

/// Comps used as layer sources in `cid` (downstream), first use first, without repeats.
pub fn downstream(project: &Project, cid: ItemId) -> Vec<ItemId> {
    let mut out = vec![];
    if let Some(c) = project.comp(cid) {
        for l in &c.layers {
            if let LayerSource::Comp { item } = l.source
                && !out.contains(&item)
                && project.comp(item).is_some()
            {
                out.push(item);
            }
        }
    }
    out
}

/// The navigator chain ending at `cid`: walk upstream, preferring comps opened most recently
/// (`open` is the open-comps order), stopping at a root or a cycle.
pub fn chain(project: &Project, cid: ItemId, open: &[ItemId]) -> Vec<ItemId> {
    let mut v = vec![cid];
    let mut cur = cid;
    while v.len() < 32 {
        let ups = upstream(project, cur);
        let Some(next) = open.iter().rev().find(|o| ups.contains(o)).or(ups.first()).copied() else { break };
        if v.contains(&next) {
            break;
        }
        v.push(next);
        cur = next;
    }
    v.reverse();
    v
}

fn name(project: &Project, id: ItemId) -> String {
    project.item(id).map(|i| i.name.clone()).unwrap_or_default()
}

// ---------------------------------------------------------------- Pre-compose dialog

#[derive(Clone, Debug, Default)]
pub struct PrecomposeDraft {
    pub layers: Vec<u64>,
    pub name: String,
    /// "Leave all attributes in" (only one footage/solid/precomp layer).
    pub leave: bool,
    pub can_leave: bool,
    pub layer_name: String,
    pub adjust: bool,
    pub open: bool,
}

/// Open the Pre-compose dialog for the selected (or given) layers.
pub fn open(app: &mut EffectcraftApp, p: &serde_json::Value) -> Result<(), String> {
    let s = &app.session;
    let c = s.active_comp().ok_or("no composition is open")?;
    let layers: Vec<u64> = match p.get("layers").and_then(|v| v.as_array()) {
        Some(a) => a.iter().filter_map(|v| v.as_u64()).collect(),
        None => s.state.selected_layers.iter().map(|l| l.0).collect(),
    };
    if layers.is_empty() {
        return Err("select the layers to pre-compose".into());
    }
    let first = c.layer(effectcraft_engine::project::LayerId(layers[0]));
    let can_leave = layers.len() == 1 && first.is_some_and(|l| l.source.item().is_some());
    let n = s.project.comps().count() + 1;
    let base = if layers.len() == 1 { first.map(|l| format!("{} Comp 1", l.name)) } else { None };
    app.dialog_state.precompose = PrecomposeDraft {
        layers,
        name: base.unwrap_or_else(|| format!("Pre-comp {n}")),
        leave: false,
        can_leave,
        layer_name: first.map(|l| l.name.clone()).unwrap_or_default(),
        adjust: false,
        open: true,
    };
    app.dialog = Some(Dialog::Precompose);
    Ok(())
}

/// The `layer.precompose` parameters of the dialog.
pub fn draft_params(d: &PrecomposeDraft) -> serde_json::Value {
    json!({
        "layers": d.layers,
        "name": d.name,
        "mode": if d.leave && d.can_leave { "leave" } else { "move" },
        "adjustDuration": d.adjust && !(d.leave && d.can_leave),
        "open": d.open,
    })
}

pub fn dialog(app: &mut EffectcraftApp, ctx: &egui::Context, t: &Tokens) {
    let mut d = app.dialog_state.precompose.clone();
    let (mut ok, mut cancel) = (false, false);
    super::dialogs::modal(ctx, "Pre-compose", vec2(520.0, 360.0), t, |ui| {
        ui.horizontal(|ui| {
            ui.label(tr("New composition name:"));
            let r = ui.add(egui::TextEdit::singleline(&mut d.name).desired_width(260.0));
            crate::widgets::focus_on_open(ui.ctx(), &r, &d.name);
            app.auto.add("dialog.precompose.name", r.rect, "New composition name");
        });
        ui.add_space(10.0);
        ui.add_enabled_ui(d.can_leave, |ui| {
            let r = ui.radio_value(&mut d.leave, true, tr_args("Leave all attributes in \"{}\"", &[&d.layer_name]));
            app.auto.add("dialog.precompose.leave", r.rect, "Leave all attributes");
            ui.label(
                egui::RichText::new(format!(
                    "Use this option to create a new intermediate composition with only \"{}\" in it. The new composition will become the source to the current layer.",
                    d.layer_name
                ))
                .color(t.text_dim)
                .size(11.0),
            );
        });
        ui.add_space(6.0);
        let r = ui.radio_value(&mut d.leave, false, "Move all attributes into the new composition");
        app.auto.add("dialog.precompose.move", r.rect, "Move all attributes");
        ui.label(
            egui::RichText::new(tr("Use this option to place the currently selected layers together into a new intermediate composition."))
                .color(t.text_dim)
                .size(11.0),
        );
        ui.add_space(4.0);
        ui.add_enabled_ui(!d.leave, |ui| {
            let r = ui.checkbox(&mut d.adjust, tr("Adjust composition duration to the time span of the selected layers"));
            app.auto.add("dialog.precompose.adjust", r.rect, "Adjust composition duration");
        });
        ui.add_space(8.0);
        let r = ui.checkbox(&mut d.open, tr("Open New Composition"));
        app.auto.add("dialog.precompose.open", r.rect, "Open New Composition");
        ui.add_space(14.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let r = ui.add(egui::Button::new(egui::RichText::new(tr("   OK   ")).color(Color32::WHITE)).fill(t.accent));
            app.auto.add("dialog.precompose.ok", r.rect, "OK");
            ok = r.clicked();
            let r = ui.button(tr("Cancel"));
            app.auto.add("dialog.precompose.cancel", r.rect, "Cancel");
            cancel = r.clicked();
        });
    });
    app.dialog_state.precompose = d.clone();
    if ok || super::dialogs::ui_enter(ctx) {
        if let Err(e) = app.session.execute("layer.precompose", draft_params(&d)) {
            app.ui.status = e.to_string();
        }
        cancel = true;
    }
    if cancel {
        app.dialog = None;
    }
}

// ---------------------------------------------------------------- navigator + mini-flowchart

/// Height of the Composition Navigator bar.
pub const NAV_H: f32 = 22.0;

/// Whether the active comp has a flow to show (it is nested somewhere or nests others).
pub fn has_flow(app: &EffectcraftApp) -> bool {
    app.session.active_comp_id().is_some_and(|c| !upstream(&app.session.project, c).is_empty() || !downstream(&app.session.project, c).is_empty())
}

/// The Composition Navigator bar: the upstream chain › active comp › (dim) its precomps.
pub fn navigator(app: &mut EffectcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let Some(cid) = app.session.active_comp_id() else { return };
    let p = ui.painter().with_clip_rect(rect);
    p.rect_filled(rect, 0.0, t.panel_bg);
    p.line_segment([rect.left_bottom(), rect.right_bottom()], Stroke::new(1.0, t.separator));
    let proj = app.session.project.clone();
    let ch = chain(&proj, cid, &app.session.state.open_comps);
    let down = downstream(&proj, cid);
    let mut x = rect.min.x + 10.0;
    let cy = rect.center().y;
    let mut open: Option<ItemId> = None;
    let items: Vec<(ItemId, bool)> = ch.iter().map(|c| (*c, false)).chain(down.iter().take(1).map(|c| (*c, true))).collect();
    for (i, (id, downstream_item)) in items.iter().enumerate() {
        if i > 0 {
            crate::icons::paint(&p, Rect::from_center_size(pos2(x + 6.0, cy), vec2(8.0, 8.0)), crate::icons::Icon::ChevronRight, t.text_faint);
            x += 16.0;
        }
        let active = *id == cid;
        let label = name(&proj, *id);
        let col = if active {
            t.tab_text_active
        } else if *downstream_item {
            t.text_faint
        } else {
            t.text_dim
        };
        let g = p.layout_no_wrap(label.clone(), Tokens::ui(11.5), col);
        let r = Rect::from_min_size(pos2(x, rect.min.y + 2.0), vec2(g.size().x + 12.0, rect.height() - 4.0));
        let resp = ui.interact(r, egui::Id::new(("comp-nav", id.0)), Sense::click());
        if active {
            p.rect_filled(r, 3.0, t.row_selected);
        } else if resp.hovered() {
            p.rect_filled(r, 3.0, t.hover);
        }
        p.galley(pos2(r.min.x + 6.0, cy - g.size().y / 2.0), g, col);
        app.auto.add(&format!("viewer.navigator.{}", id.0), r, &label);
        if resp.clicked() && !active {
            open = Some(*id);
        }
        x = r.max.x + 2.0;
    }
    let fr = Rect::from_min_size(pos2(rect.max.x - 26.0, rect.min.y + 2.0), vec2(22.0, rect.height() - 4.0));
    if crate::widgets::icon_button(ui, fr, crate::icons::Icon::Flowchart, false, &t, egui::Id::new("comp-nav-flow"))
        .on_hover_text(tr("Composition Mini-Flowchart"))
        .clicked()
    {
        open_mini(app, [fr.max.x, fr.max.y + 2.0], true);
    }
    app.auto.add("viewer.navigator.flowchart", fr, "Composition Mini-Flowchart");
    if let Some(id) = open {
        app.session.open_comp(id);
    }
}

// ---------------------------------------------------------------- Composition Mini-Flowchart

/// Columns the Mini-Flowchart expands to on each side of the active comp: a deeper (or cyclic)
/// nesting stops here.
pub const MINI_MAX_DEPTH: usize = 12;
/// Nodes per Mini-Flowchart column.
pub const MINI_MAX_COLUMN: usize = 64;

/// A Mini-Flowchart node: the side of the active comp (`up`: the comps that nest it, drawn on the
/// left; else the comps it nests, on the right) and the comps from the active one outward. The
/// empty path is the active comp itself.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MiniNode {
    #[serde(default)]
    pub up: bool,
    #[serde(default)]
    pub path: Vec<u64>,
}

impl MiniNode {
    /// The active comp (the centre node).
    pub fn is_centre(&self) -> bool {
        self.path.is_empty()
    }

    /// The node one level closer to the active comp.
    pub fn parent(&self) -> MiniNode {
        let mut path = self.path.clone();
        path.pop();
        if path.is_empty() { MiniNode::default() } else { MiniNode { up: self.up, path } }
    }

    /// Whether `self` lies inside the branch of `other` (or is it).
    fn within(&self, other: &MiniNode) -> bool {
        self.up == other.up && self.path.starts_with(&other.path)
    }

    /// The node's automation id (`flowchart.mini.up.<comp>.<comp>…`, `flowchart.mini.down.…`).
    pub fn automation_id(&self) -> String {
        let path: Vec<String> = self.path.iter().map(u64::to_string).collect();
        format!("flowchart.mini.{}.{}", if self.up { "up" } else { "down" }, path.join("."))
    }
}

/// The open Mini-Flowchart's focus (the comp the camera is on) and expanded branches.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MiniFlowState {
    /// The comp it was opened for (another active comp starts over).
    #[serde(default)]
    pub comp: Option<u64>,
    /// Hang below its anchor, right-aligned (the navigator button), instead of centring on it.
    #[serde(default)]
    pub below: bool,
    /// The selected comp, drawn in the centre with its own branches on both sides (`None`: the
    /// active comp). The arrow keys move it.
    #[serde(default)]
    pub focus: Option<u64>,
    /// The comp the focus came from and whether the focus nests it (`true`: it moved to a comp
    /// that uses the previous one). The previous comp's other comps on that side are the focus's
    /// siblings, in its column (Up / Down move between them).
    #[serde(default)]
    pub from: Option<(u64, bool)>,
    /// Nodes whose next level is shown (paths from the focus).
    #[serde(default)]
    pub expanded: Vec<MiniNode>,
    /// The camera has been placed since it opened (until then it jumps instead of easing).
    #[serde(skip)]
    pub placed: bool,
    /// Where the new focus was, relative to the old one, when it moved: the flowchart slides from
    /// there to its new place.
    #[serde(skip)]
    pub slide: Option<[f32; 2]>,
}

/// A node of a Mini-Flowchart column.
#[derive(Clone, Debug, PartialEq)]
pub struct MiniEntry {
    pub key: MiniNode,
    pub id: ItemId,
    /// It has a further level on its side (it shows an expand arrow).
    pub more: bool,
    /// That level is shown.
    pub open: bool,
}

/// Which comps nest which, from one pass over the layers.
struct NestGraph {
    up: HashMap<u64, Vec<ItemId>>,
    down: HashMap<u64, Vec<ItemId>>,
}

impl NestGraph {
    fn new(project: &Project) -> Self {
        let mut g = NestGraph { up: HashMap::new(), down: HashMap::new() };
        for (id, c) in project.comps() {
            for l in &c.layers {
                if let LayerSource::Comp { item } = l.source
                    && project.comp(item).is_some()
                {
                    let down = g.down.entry(id.0).or_default();
                    if !down.contains(&item) {
                        down.push(item);
                    }
                    let up = g.up.entry(item.0).or_default();
                    if !up.contains(id) {
                        up.push(*id);
                    }
                }
            }
        }
        g
    }

    /// The next level from `id` on one side, without the active comp and the comps already on the
    /// branch (a cyclic nesting ends there).
    fn next(&self, cid: ItemId, path: &[u64], id: ItemId, up: bool) -> Vec<ItemId> {
        let map = if up { &self.up } else { &self.down };
        map.get(&id.0).map(|v| v.iter().filter(|n| **n != cid && !path.contains(&n.0)).copied().collect()).unwrap_or_default()
    }

    /// The columns on one side of `cid`, outward: the first level, and the next level of every
    /// expanded node, at most [`MINI_MAX_DEPTH`] columns of [`MINI_MAX_COLUMN`] nodes.
    fn columns(&self, cid: ItemId, up: bool, expanded: &[MiniNode]) -> Vec<Vec<MiniEntry>> {
        let is_open = |path: &[u64]| expanded.iter().any(|e| e.up == up && e.path == path);
        let mut cols: Vec<Vec<MiniEntry>> = Vec::new();
        let mut frontier: Vec<(Vec<u64>, ItemId)> = vec![(Vec::new(), cid)];
        for depth in 0..MINI_MAX_DEPTH {
            let mut col = Vec::new();
            'parents: for (path, id) in &frontier {
                if depth > 0 && !is_open(path) {
                    continue;
                }
                for n in self.next(cid, path, *id, up) {
                    if col.len() >= MINI_MAX_COLUMN {
                        break 'parents;
                    }
                    let mut p = path.clone();
                    p.push(n.0);
                    let more = depth + 1 < MINI_MAX_DEPTH && !self.next(cid, &p, n, up).is_empty();
                    let open = more && is_open(&p);
                    col.push(MiniEntry { key: MiniNode { up, path: p }, id: n, more, open });
                }
            }
            if col.is_empty() {
                break;
            }
            frontier = col.iter().map(|e| (e.key.path.clone(), e.id)).collect();
            cols.push(col);
        }
        cols
    }
}

/// The Mini-Flowchart's columns on one side of `cid` (`up`: the comps that nest it), outward from
/// it, for the given expanded nodes.
pub fn mini_columns(project: &Project, cid: ItemId, up: bool, expanded: &[MiniNode]) -> Vec<Vec<MiniEntry>> {
    NestGraph::new(project).columns(cid, up, expanded)
}

/// Layout constants of the Mini-Flowchart.
const MINI_COL_W: f32 = 150.0;
const MINI_GAP: f32 = 34.0;
const MINI_ROW: f32 = 26.0;

/// What the Mini-Flowchart shows around its focus comp.
pub struct MiniLayout {
    /// The focus comp (the centre, where the camera is).
    pub focus: ItemId,
    /// The comps that nest the focus (left), outward.
    pub ups: Vec<Vec<MiniEntry>>,
    /// The comps the focus nests (right), outward.
    pub downs: Vec<Vec<MiniEntry>>,
    /// The centre column: the focus and its siblings (the other comps the previous focus is
    /// linked to on that side), the focus at `centre`.
    pub column: Vec<ItemId>,
    pub centre: usize,
}

/// The Mini-Flowchart around the state's focus (the active comp `cid` until the arrows move it).
pub fn mini_layout(project: &Project, cid: ItemId, state: &MiniFlowState) -> MiniLayout {
    mini_layout_in(&NestGraph::new(project), project, cid, state)
}

fn mini_layout_in(g: &NestGraph, project: &Project, cid: ItemId, state: &MiniFlowState) -> MiniLayout {
    let focus = state.focus.map(ItemId).filter(|f| project.comp(*f).is_some()).unwrap_or(cid);
    let ups = g.columns(focus, true, &state.expanded);
    let downs = g.columns(focus, false, &state.expanded);
    let mut column: Vec<ItemId> = match state.from {
        Some((prev, up)) if focus != cid || state.focus.is_some() => {
            let map = if up { &g.up } else { &g.down };
            map.get(&prev).cloned().unwrap_or_default()
        }
        _ => Vec::new(),
    };
    column.truncate(MINI_MAX_COLUMN);
    if !column.contains(&focus) {
        column = vec![focus];
    }
    let centre = column.iter().position(|c| *c == focus).unwrap_or(0);
    MiniLayout { focus, ups, downs, column, centre }
}

impl MiniLayout {
    /// A node's centre relative to the flowchart's centre, from its column (0 = the left one),
    /// row and the column's length.
    fn offset(&self, col: usize, row: usize, len: usize) -> egui::Vec2 {
        let cols = (self.ups.len() + 1 + self.downs.len()) as f32;
        let x = (col as f32 - (cols - 1.0) / 2.0) * (MINI_COL_W + MINI_GAP);
        let y = (row as f32 - (len.max(1) as f32 - 1.0) / 2.0) * MINI_ROW;
        vec2(x, y)
    }

    /// The focus's centre relative to the flowchart's centre.
    fn focus_offset(&self) -> egui::Vec2 {
        self.offset(self.ups.len(), self.centre, self.column.len())
    }
}

/// Move the Mini-Flowchart's focus with an arrow key, as in After Effects: the comp it arrives on
/// becomes the centre, showing all its branches at once (the comps that use it on the left, the
/// comps it uses on the right). Left / Right step to a comp that uses / is used by the focus (back
/// to the previous one first), Up / Down to its siblings in the centre column.
pub fn mini_nav(state: &mut MiniFlowState, key: egui::Key, project: &Project, cid: ItemId) {
    use egui::Key::{ArrowDown, ArrowLeft, ArrowRight, ArrowUp};
    let g = NestGraph::new(project);
    let l = mini_layout_in(&g, project, cid, state);
    let here = l.focus_offset();
    let first = |cols: &[Vec<MiniEntry>], back: Option<u64>| -> Option<(usize, usize, ItemId)> {
        let col = cols.first()?;
        let i = back.and_then(|b| col.iter().position(|e| e.id.0 == b)).unwrap_or(0);
        Some((i, col.len(), col.get(i)?.id))
    };
    let (target, from, there) = match key {
        ArrowLeft => {
            let back = state.from.filter(|(_, up)| !*up).map(|(p, _)| p);
            let Some((i, n, id)) = first(&l.ups, back) else { return };
            (id, Some((l.focus.0, true)), l.offset(l.ups.len() - 1, i, n))
        }
        ArrowRight => {
            let back = state.from.filter(|(_, up)| *up).map(|(p, _)| p);
            let Some((i, n, id)) = first(&l.downs, back) else { return };
            (id, Some((l.focus.0, false)), l.offset(l.ups.len() + 1, i, n))
        }
        ArrowUp | ArrowDown => {
            let i = if key == ArrowUp { l.centre.checked_sub(1) } else { Some(l.centre + 1) };
            let Some((i, id)) = i.and_then(|i| l.column.get(i).map(|id| (i, *id))) else { return };
            (id, state.from, l.offset(l.ups.len(), i, l.column.len()))
        }
        _ => return,
    };
    state.focus = Some(target.0);
    state.from = from;
    state.expanded.clear();
    // The flowchart around the new focus starts where the new focus was and slides to the camera.
    let d = there - here;
    state.slide = Some([d.x, d.y]);
}

/// Expand or collapse a node's next level (collapsing forgets the expanded nodes inside it).
pub fn mini_toggle(state: &mut MiniFlowState, key: &MiniNode) {
    if key.is_centre() {
        return;
    }
    if state.expanded.contains(key) {
        state.expanded.retain(|e| !e.within(key));
    } else {
        state.expanded.push(key.clone());
    }
}

/// Open the Mini-Flowchart for the active comp, centred on `at` (`below`: hanging below it).
pub fn open_mini(app: &mut EffectcraftApp, at: [f32; 2], below: bool) {
    app.ui.mini_flowchart = Some(at);
    app.ui.mini_flow = MiniFlowState { comp: app.session.active_comp_id().map(|c| c.0), below, ..Default::default() };
}

/// Close the Mini-Flowchart.
pub fn close_mini(app: &mut EffectcraftApp) {
    app.ui.mini_flowchart = None;
    app.ui.mini_flow = MiniFlowState::default();
}

/// The open Mini-Flowchart's comp; closes it when no comp is active.
fn mini_comp(app: &mut EffectcraftApp) -> Option<ItemId> {
    app.ui.mini_flowchart?;
    let Some(cid) = app.session.active_comp_id() else {
        close_mini(app);
        return None;
    };
    if app.ui.mini_flow.comp != Some(cid.0) {
        app.ui.mini_flow = MiniFlowState { comp: Some(cid.0), below: app.ui.mini_flow.below, ..Default::default() };
    }
    Some(cid)
}

/// The open Mini-Flowchart's keys, before the panels and the shortcuts see them: the arrows move
/// the focus, Enter opens the focused comp, Escape or Tab closes it.
pub fn mini_flowchart_keys(app: &mut EffectcraftApp, ctx: &egui::Context) {
    use egui::Key::{ArrowDown, ArrowLeft, ArrowRight, ArrowUp, Enter, Escape, Tab};
    let Some(cid) = mini_comp(app) else { return };
    if app.dialog.is_some() {
        return;
    }
    let keys = [Escape, Tab, Enter, ArrowLeft, ArrowRight, ArrowUp, ArrowDown];
    if !ctx.input(|i| keys.iter().any(|k| i.key_pressed(*k))) {
        return;
    }
    // These keys are the popup's: no keyboard-focus move behind it.
    ctx.memory_mut(|m| m.move_focus(egui::FocusDirection::None));
    let take = |k: egui::Key| ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, k));
    let (escape, tab) = (take(Escape), take(Tab));
    if escape || tab {
        close_mini(app);
        return;
    }
    for key in [ArrowLeft, ArrowRight, ArrowUp, ArrowDown] {
        if take(key) {
            let project = app.session.project.clone();
            mini_nav(&mut app.ui.mini_flow, key, &project, cid);
        }
    }
    if take(Enter) {
        let target = app.ui.mini_flow.focus.map(ItemId).filter(|f| *f != cid);
        close_mini(app);
        if let Some(id) = target {
            app.session.open_comp(id);
        }
    }
}

/// Composition Mini-Flowchart (Tab): the focused comp (the active one at first) in the centre, the
/// comps that use it on the left and the comps it uses on the right, each expandable one more
/// level; the arrows move the focus (the camera stays on it), a click opens a comp.
pub fn mini_flowchart(app: &mut EffectcraftApp, ctx: &egui::Context) {
    const COL_W: f32 = MINI_COL_W;
    const GAP: f32 = MINI_GAP;
    const ROW: f32 = MINI_ROW;
    const NODE_H: f32 = 22.0;
    const ARROW_W: f32 = 16.0;
    const MARGIN: f32 = 10.0;
    let Some(at) = app.ui.mini_flowchart else { return };
    let Some(cid) = mini_comp(app) else { return };
    let t = app.tokens;
    let proj = app.session.project.clone();
    let l = mini_layout(&proj, cid, &app.ui.mini_flow);
    let (ups, downs, focus) = (&l.ups, &l.downs, l.focus);
    let below = app.ui.mini_flow.below;
    let mut open: Option<ItemId> = None;
    let mut toggle: Option<MiniNode> = None;
    let mut close = false;
    let mut autos: Vec<(String, Rect, String)> = Vec::new();
    let screen = ctx.content_rect();
    let n_cols = ups.len() + 1 + downs.len();
    let tallest = ups.iter().chain(downs.iter()).map(Vec::len).chain([l.column.len()]).max().unwrap_or(1).max(1);
    let empty = ups.is_empty() && downs.is_empty();
    let size = vec2(n_cols as f32 * COL_W + (n_cols - 1) as f32 * GAP, tallest as f32 * ROW + if empty { 24.0 } else { 4.0 });
    // The "camera" is locked on the focused comp, as in After Effects: it stays at `at` and the
    // flowchart slides around it (eased) as the arrows move the focus.
    let (pivot, pos) = if below {
        (Align2::RIGHT_TOP, pos2(at[0], at[1]))
    } else {
        let f = l.focus_offset();
        let (x, y) = (MARGIN + size.x / 2.0 + f.x, MARGIN + size.y / 2.0 + f.y);
        let id = |k: &str| egui::Id::new(("mini-flowchart-camera", k));
        let st = &mut app.ui.mini_flow;
        if !st.placed {
            st.placed = true;
            st.slide = Some([0.0, 0.0]);
        }
        if let Some([dx, dy]) = st.slide.take() {
            ctx.animate_value_with_time(id("dx"), dx, 0.0);
            ctx.animate_value_with_time(id("dy"), dy, 0.0);
        }
        let (dx, dy) = (ctx.animate_value_with_time(id("dx"), 0.0, 0.12), ctx.animate_value_with_time(id("dy"), 0.0, 0.12));
        (Align2::LEFT_TOP, pos2(at[0] - x + dx, at[1] - y + dy))
    };
    let area = egui::Area::new(egui::Id::new("mini-flowchart")).order(egui::Order::Foreground).constrain(true).pivot(pivot).fixed_pos(pos).show(ctx, |ui| {
        egui::Frame::popup(ui.style()).fill(t.panel_bg).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
            // The popup grows with every expanded level (up to the screen): the scroll area
            // asks for the whole flowchart rather than the size it had when it opened.
            let (max_w, max_h) = ((screen.width() - 60.0).max(200.0), (screen.height() - 60.0).max(100.0));
            egui::ScrollArea::both().max_width(max_w).max_height(max_h).min_scrolled_width(size.x.min(max_w)).min_scrolled_height(size.y.min(max_h)).show(
                ui,
                |ui| {
                    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
                    let p = ui.painter().clone();
                    let place = |c: usize, i: usize, n: usize| Rect::from_center_size(rect.center() + l.offset(c, i, n), vec2(COL_W, NODE_H));
                    let mid = place(ups.len(), l.centre, l.column.len());
                    let arrow = |a: egui::Pos2, b: egui::Pos2| {
                        p.line_segment([a, b], Stroke::new(1.2, t.text_dim));
                        let d = (b - a).normalized();
                        let n = vec2(-d.y, d.x);
                        p.add(egui::Shape::convex_polygon(vec![b, b - d * 7.0 + n * 3.5, b - d * 7.0 - n * 3.5], t.text_dim, Stroke::NONE));
                    };
                    // Place every column, then draw the links under the nodes.
                    let mut placed: Vec<(MiniEntry, Rect)> = Vec::new();
                    for (k, col) in ups.iter().enumerate() {
                        for (i, e) in col.iter().enumerate() {
                            placed.push((e.clone(), place(ups.len() - 1 - k, i, col.len())));
                        }
                    }
                    for (k, col) in downs.iter().enumerate() {
                        for (i, e) in col.iter().enumerate() {
                            placed.push((e.clone(), place(ups.len() + 1 + k, i, col.len())));
                        }
                    }
                    for (e, r) in &placed {
                        let parent = e.key.parent();
                        let pr = if parent.is_centre() { Some(mid) } else { placed.iter().find(|(o, _)| o.key == parent).map(|(_, r)| *r) };
                        if let Some(pr) = pr {
                            // Arrows point the way a nested comp flows into the comp that uses it.
                            if e.key.up {
                                arrow(pr.left_center(), r.right_center());
                            } else {
                                arrow(r.left_center(), pr.right_center());
                            }
                        }
                    }
                    // The focus's siblings link to the previous focus like the focus does.
                    let siblings: Vec<(usize, ItemId)> = l.column.iter().copied().enumerate().filter(|(i, _)| *i != l.centre).collect();
                    if let Some((prev, up)) = app.ui.mini_flow.from {
                        let side = if up { downs } else { ups };
                        if let Some(pr) = side
                            .first()
                            .and_then(|c| c.iter().find(|e| e.id.0 == prev))
                            .and_then(|e| placed.iter().find(|(o, _)| o.key == e.key))
                            .map(|(_, r)| *r)
                        {
                            for (i, _) in &siblings {
                                let r = place(ups.len(), *i, l.column.len());
                                if up {
                                    arrow(pr.left_center(), r.right_center());
                                } else {
                                    arrow(r.left_center(), pr.right_center());
                                }
                            }
                        }
                    }
                    let mut named: std::collections::HashSet<u64> = std::collections::HashSet::new();
                    // `role`: 0 the focus, 1 a sibling of it, 2 another node.
                    let mut node = |ui: &mut egui::Ui, r: Rect, key: &MiniNode, id: ItemId, role: u8, more: bool, opened: bool| {
                        let is_focus = role == 0;
                        let active = id == cid;
                        let resp = ui.interact(r, egui::Id::new(("mfc", key.up, key.path.clone(), role, id.0)), Sense::click());
                        // The active comp keeps the accent wherever it is; the focus is outlined.
                        let fill = if active {
                            t.accent
                        } else if resp.hovered() {
                            t.hover
                        } else {
                            t.field_bg
                        };
                        p.rect_filled(r, 4.0, fill);
                        if is_focus {
                            p.rect_stroke(r, 4.0, Stroke::new(1.5, if active { Color32::WHITE } else { t.text }), egui::StrokeKind::Outside);
                        }
                        let label = name(&proj, id);
                        let mut text = r.shrink2(vec2(6.0, 0.0));
                        let expand = more.then(|| {
                            let er = if key.up {
                                Rect::from_min_size(r.min, vec2(ARROW_W, NODE_H))
                            } else {
                                Rect::from_min_size(pos2(r.max.x - ARROW_W, r.min.y), vec2(ARROW_W, NODE_H))
                            };
                            if key.up {
                                text.min.x = er.max.x;
                            } else {
                                text.max.x = er.min.x;
                            }
                            er
                        });
                        let col = if active { Color32::WHITE } else { t.text };
                        let galley = p.layout_no_wrap(label.clone(), Tokens::ui(11.5), col);
                        let x = if galley.size().x <= text.width() { text.center().x - galley.size().x / 2.0 } else { text.min.x };
                        p.with_clip_rect(text.intersect(p.clip_rect())).galley(pos2(x, r.center().y - galley.size().y / 2.0), galley, col);
                        if named.insert(id.0) {
                            autos.push((format!("flowchart.mini.{}", id.0), r, label.clone()));
                        }
                        match role {
                            0 => autos.push(("flowchart.mini.focus".into(), r, label.clone())),
                            1 => autos.push((format!("flowchart.mini.sibling.{}", id.0), r, label.clone())),
                            _ => autos.push((key.automation_id(), r, label.clone())),
                        }
                        let mut toggled = false;
                        if let Some(er) = expand {
                            let eresp = ui.interact(er, egui::Id::new(("mfc-expand", key.up, key.path.clone())), Sense::click());
                            if eresp.hovered() {
                                p.rect_filled(er.shrink(2.0), 3.0, t.hover);
                            }
                            let icon = if opened {
                                crate::icons::Icon::ChevronDown
                            } else if key.up {
                                crate::icons::Icon::ChevronLeft
                            } else {
                                crate::icons::Icon::ChevronRight
                            };
                            crate::icons::paint(&p, Rect::from_center_size(er.center(), vec2(10.0, 10.0)), icon, t.text_dim);
                            autos.push((format!("{}.expand", key.automation_id()), er, if opened { "Collapse".into() } else { "Expand".into() }));
                            toggled = eresp.clicked();
                        }
                        (resp.clicked() && !toggled, toggled)
                    };
                    let (clicked, _) = node(ui, mid, &MiniNode::default(), focus, 0, false, false);
                    if clicked {
                        if focus == cid {
                            close = true;
                        } else {
                            open = Some(focus);
                        }
                    }
                    for (i, id) in &siblings {
                        let (clicked, _) = node(ui, place(ups.len(), *i, l.column.len()), &MiniNode::default(), *id, 1, false, false);
                        if clicked {
                            open = Some(*id);
                        }
                    }
                    for (e, r) in &placed {
                        let (clicked, toggled) = node(ui, *r, &e.key, e.id, 2, e.more, e.open);
                        if toggled {
                            toggle = Some(e.key.clone());
                        } else if clicked {
                            open = Some(e.id);
                        }
                    }
                    if empty {
                        p.text(pos2(rect.center().x, rect.max.y - 2.0), Align2::CENTER_BOTTOM, tr("No nested compositions"), Tokens::ui(10.5), t.text_faint);
                    }
                },
            );
        });
    });
    for (id, r, label) in &autos {
        app.auto.add(id, *r, label);
    }
    let outside = crate::widgets::pressed_outside(ctx, &area.response);
    if let Some(key) = toggle {
        mini_toggle(&mut app.ui.mini_flow, &key);
    }
    if let Some(id) = open {
        close_mini(app);
        app.session.open_comp(id);
    } else if close || outside {
        close_mini(app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use effectcraft_engine::Session;

    #[test]
    fn flow_chain_and_precompose_dialog_params() {
        let mut s = Session::default();
        s.execute("comp.new", json!({"name": "Main", "width": 200, "height": 100, "frameRate": 25, "duration": 2})).unwrap();
        let main = s.active_comp_id().unwrap();
        let l = s.execute("layer.newSolid", json!({"color": "#ff0000"})).unwrap()["layer"].as_u64().unwrap();
        let r = s.execute("layer.precompose", json!({"layers": [l], "name": "Mid", "open": true})).unwrap();
        let mid = ItemId(r["comp"].as_u64().unwrap());
        let l2 = s.project.comp(mid).unwrap().layers[0].id.0;
        let r = s.execute("layer.precompose", json!({"layers": [l2], "name": "Inner", "open": true})).unwrap();
        let inner = ItemId(r["comp"].as_u64().unwrap());
        assert_eq!(upstream(&s.project, inner), vec![mid]);
        assert_eq!(downstream(&s.project, main), vec![mid]);
        assert_eq!(chain(&s.project, inner, &s.state.open_comps), vec![main, mid, inner]);
        assert_eq!(chain(&s.project, main, &[]), vec![main]);

        let mut app = EffectcraftApp::new(s);
        app.session.open_comp(main);
        let top = app.session.active_comp().unwrap().layers[0].id.0;
        app.session.execute("layer.select", json!({"layers": [top]})).unwrap();
        open(&mut app, &json!({})).unwrap();
        assert_eq!(app.dialog, Some(Dialog::Precompose));
        let d = &mut app.dialog_state.precompose;
        assert!(d.can_leave);
        d.leave = true;
        d.adjust = true;
        let p = draft_params(d);
        assert_eq!(p["mode"], "leave");
        assert_eq!(p["adjustDuration"], false);
        d.leave = false;
        assert_eq!(draft_params(d)["mode"], "move");
        assert_eq!(draft_params(d)["adjustDuration"], true);
    }
}
