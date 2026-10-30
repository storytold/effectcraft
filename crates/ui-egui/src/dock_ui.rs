//! EffectCraft's panels in craft-ui's shared docking. craft-ui draws the frames, tab strips,
//! stacked panels, floating windows, dividers and drop previews and owns the gestures (drag a tab
//! onto a group to tab it there or split it off to a side; hold Cmd/Ctrl at the release, or drop
//! outside every group, to float it). EffectCraft draws the panel bodies, the ≡ panel menu, the
//! Timeline's comp tabs and the viewer locks in each group header, and runs every layout change
//! through its command path (`window.dockAction`), so pointer, menu and control-channel changes
//! share one route.

use crate::state::UiState;
use craft_ui::docking::{
    Action, Divider, DockArea, DockCommand, DockContent, DockError, DockEvent, DockStyle, FloatSpace, Floating, Hit, Node, ResizeEdge, UiDock,
};
use egui::{Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use serde_json::{Value, json};

use crate::dock::{self, DocAction, DocTab, PanelKind, Workspace, WorkspaceExt as _};
use crate::icons::{self, Icon};
use crate::theme::Tokens;
use crate::{EffectcraftApp, panels};

/// The dock area's egui id; `DockArea::hit_rects` and `float_space` read by it.
pub fn area_id() -> egui::Id {
    egui::Id::unique("effectcraft-docking")
}

/// Width of the ≡ panel menu button at the end of each tab strip.
const MENU_W: f32 = 26.0;
/// Width of the viewer lock before it.
const LOCK_W: f32 = 18.0;

/// The smallest and largest native panel window, in points.
pub const PANEL_WINDOW_MIN: [f32; 2] = [160.0, 100.0];
pub const PANEL_WINDOW_MAX: f32 = 16_384.0;
/// The farthest a native panel window may sit from the desktop origin, in points.
pub const PANEL_WINDOW_MAX_POS: f32 = 100_000.0;

/// A native panel window's desktop rect made safe to hand to the windowing system: finite, with
/// a size within [`PANEL_WINDOW_MIN`]..=[`PANEL_WINDOW_MAX`] and a position within
/// ±[`PANEL_WINDOW_MAX_POS`] (hostile workspace files and bad monitor reports included).
pub fn sanitized_window_rect(rect: [f32; 4]) -> [f32; 4] {
    let finite = |v: f32, fallback: f32| if v.is_finite() { v } else { fallback };
    let [x, y, w, h] = rect;
    [
        finite(x, 0.0).clamp(-PANEL_WINDOW_MAX_POS, PANEL_WINDOW_MAX_POS),
        finite(y, 0.0).clamp(-PANEL_WINDOW_MAX_POS, PANEL_WINDOW_MAX_POS),
        finite(w, 420.0).clamp(PANEL_WINDOW_MIN[0], PANEL_WINDOW_MAX),
        finite(h, 320.0).clamp(PANEL_WINDOW_MIN[1], PANEL_WINDOW_MAX),
    ]
}

/// How long to wait for the main window to report its frame before migrated native windows
/// keep their places as they are, in seconds.
const FRAME_REPORT_WAIT: f64 = 2.0;

/// Moves the native rects of [`UiState::native_outer`]'s windows from the OS window's outer
/// position to its content position, by the main window's frame offset (`inner` less `outer`,
/// the main window's desktop rects): the earlier version's windows had the same OS title bar.
/// Without a report by [`FRAME_REPORT_WAIT`] (a backend without window positions), the places
/// stay as they are.
pub(crate) fn native_outer_to_content(ui: &mut UiState, outer: Option<Rect>, inner: Option<Rect>, now: f64) {
    let offset = match (outer, inner) {
        (Some(outer), Some(inner)) if outer.min.is_finite() && inner.min.is_finite() => inner.min - outer.min,
        _ if now < FRAME_REPORT_WAIT => return,
        _ => egui::Vec2::ZERO,
    };
    let ids = std::mem::take(&mut ui.native_outer);
    for window in ui.dock.layout.floating.iter_mut().filter(|w| ids.contains(&w.id)) {
        if let Some([x, y, w, h]) = window.native {
            window.native = Some(sanitized_window_rect([x + offset.x, y + offset.y, w, h]));
        }
    }
}

/// A new floating group's default rect near the centre of `screen`.
pub fn default_float_rect(screen: Rect) -> [f32; 4] {
    let c = screen.center();
    [c.x - 210.0, c.y - 160.0, 420.0, 320.0]
}

/// A panel named by its title or id ("Effect Controls", "EffectControls") or its serialized
/// form (`{"Viewer": 2}`).
fn panel_param(value: &Value) -> Result<PanelKind, String> {
    match value.as_str() {
        Some(name) => PanelKind::from_name(name).ok_or(format!("unknown panel `{name}`")),
        None => serde_json::from_value::<PanelKind>(value.clone()).map_err(|_| "unknown `panel`".to_string()),
    }
}

fn check_raw_action(action: &Action<PanelKind>) -> Result<(), String> {
    if matches!(action, Action::Open { .. } | Action::OpenAt { .. }) {
        return Err("use the panel open commands to preserve viewer and ScriptUI lifecycle callbacks".into());
    }
    Ok(())
}

/// The docking commands: `window.dockAction {action}`, craft-ui's `window.panel.*` commands,
/// `ui.dock` and `ui.dock.reset`. `None` for any other id.
pub(crate) fn route(app: &mut EffectcraftApp, ctx: &egui::Context, id: &str, params: &Value) -> Option<Result<Value, String>> {
    match id {
        "window.dockAction" => Some((|| {
            let value = params.get("action").cloned().ok_or("missing docking action")?;
            let action: Action<PanelKind> = serde_json::from_value(value).map_err(|error| format!("invalid docking action: {error}"))?;
            check_raw_action(&action)?;
            app.apply_dock(action)?;
            Ok(Value::Null)
        })()),
        "ui.dock.reset" => {
            let name = app.ui.workspace.clone();
            app.set_workspace(&name);
            Some(Ok(json!({"docking": app.ui.dock})))
        }
        "ui.dock" => Some((|| {
            let mut tagged = if params.is_object() { params.clone() } else { json!({}) };
            if let Some(fields) = tagged.as_object_mut() {
                fields.insert("command".into(), json!("ui.dock"));
            }
            let command: DockCommand<PanelKind> = serde_json::from_value(tagged).map_err(|error| format!("invalid ui.dock params: {error}"))?;
            app.run_dock_command(ctx, command)?;
            Ok(json!({"docking": app.ui.dock}))
        })()),
        _ if id.starts_with("window.panel.") && DockCommand::<PanelKind>::IDS.contains(&id) => Some((|| {
            let mut tagged = if params.is_object() { params.clone() } else { json!({}) };
            if let Some(fields) = tagged.as_object_mut() {
                fields.insert("command".into(), json!(id));
                // EffectCraft's panel names ("Effect Controls") as well as craft-ui's ids.
                for key in ["panel", "anchor", "before"] {
                    if let Some(name) = fields.get(key).and_then(Value::as_str) {
                        let panel = PanelKind::from_name(name).ok_or(format!("unknown panel `{name}`"))?;
                        fields.insert(key.into(), json!(panel));
                    }
                }
            }
            let command: DockCommand<PanelKind> = serde_json::from_value(tagged).map_err(|error| format!("invalid docking command: {error}"))?;
            app.run_dock_command(ctx, command)?;
            Ok(Value::Null)
        })()),
        "window.panelWindow" => Some((|| {
            let panel = panel_param(params.get("panel").ok_or("panelWindow: need `panel`")?).map_err(|e| format!("panelWindow: {e}"))?;
            let window = match params.get("window") {
                None | Some(Value::Null) => None,
                Some(value) => Some(value.as_bool().ok_or("panelWindow: `window` must be true or false")?),
            };
            let window = app.set_panel_window(ctx, panel, window)?;
            Ok(json!({"window": window}))
        })()),
        _ => None,
    }
}

impl EffectcraftApp {
    /// A layout action from the dock area or a menu, through the command path the control
    /// channel uses.
    pub(crate) fn dock_command(&mut self, ctx: &egui::Context, action: Action<PanelKind>) -> Result<(), String> {
        let value = serde_json::to_value(action).map_err(|error| error.to_string())?;
        crate::menus::invoke(self, ctx, "window.dockAction", json!({"action": value})).map(|_| ())
    }

    /// Run a craft-ui docking command on the current workspace, all or nothing. Named
    /// workspaces and Reset are EffectCraft's own (`window.workspace`, `window.resetWorkspace`).
    pub(crate) fn run_dock_command(&mut self, ctx: &egui::Context, command: DockCommand<PanelKind>) -> Result<(), String> {
        if let DockCommand::Layout { action } | DockCommand::UiDock(UiDock::Action { action }) = &command {
            check_raw_action(action)?;
        }
        if command.is_app_owned() {
            if matches!(command, DockCommand::ResetWorkspace {}) {
                let name = self.ui.workspace.clone();
                self.set_workspace(&name);
                return Ok(());
            }
            return Err("use the window.workspace commands for named workspaces".into());
        }
        // A closed panel opens through EffectCraft's own path, which puts it in its usual place
        // and runs the viewer and ScriptUI lifecycles.
        let shown = match &command {
            DockCommand::Activate { panel } | DockCommand::UiDock(UiDock::Operation(craft_ui::docking::DockOperation::Open { panel })) => Some(*panel),
            DockCommand::UiDock(UiDock::Operation(craft_ui::docking::DockOperation::Activate { panel })) => Some(*panel),
            _ => None,
        };
        if let Some(panel) = shown.filter(|panel| !self.ui.dock.is_open(*panel)) {
            self.show_panel(panel);
            return Ok(());
        }
        let space = craft_ui::docking::float_space(ctx, area_id()).unwrap_or_else(|| FloatSpace::new(ctx.content_rect()));
        match command.to_actions(&self.ui.dock, false, &space) {
            Ok(actions) => self.apply_dock_all(actions),
            // Showing a hidden panel at an edge, and Lock, change more than the layout.
            Err(DockError::NotAllowed) => {
                let mut workspace = self.ui.dock.clone();
                let closed = command.apply(&mut workspace, false, &space, dock::permissions).map_err(|error| error.to_string())?;
                self.commit_workspace(workspace, &closed, true);
                Ok(())
            }
            Err(error) => Err(error.to_string()),
        }
    }

    pub fn apply_dock(&mut self, action: Action<PanelKind>) -> Result<(), String> {
        self.apply_dock_all(vec![action])
    }

    /// Apply `actions` all or none. No move or float may empty the docked tree, so a floating
    /// panel always has somewhere to dock back to; closing the last docked panel stays allowed.
    pub(crate) fn apply_dock_all(&mut self, actions: Vec<Action<PanelKind>>) -> Result<(), String> {
        let mut workspace = self.ui.dock.clone();
        let mut relocating = false;
        let mut closed = Vec::new();
        for action in actions {
            relocating |= matches!(
                &action,
                Action::Move { .. }
                    | Action::Float { .. }
                    | Action::MoveGroup { .. }
                    | Action::MoveWindow { .. }
                    | Action::MovePanel { .. }
                    | Action::MoveColumn { .. }
            );
            let closing = matches!(&action, Action::Close { .. } | Action::CloseGroup { .. } | Action::CloseWindow { .. } | Action::CloseOthers { .. });
            let docked = workspace.layout.root.is_some();
            closed.extend(workspace.apply(action, dock::permissions).map_err(|error| error.to_string())?);
            if !closing && docked && workspace.layout.root.is_none() {
                return Err("the last docked panel must remain in the workspace".into());
            }
        }
        self.commit_workspace(workspace, &closed, relocating);
        Ok(())
    }

    /// Take a changed workspace and release the state of the panels it closed: a ScriptUI
    /// panel's script window closes (its onClose runs), a viewer forgets its comp.
    fn commit_workspace(&mut self, workspace: Workspace, closed: &[PanelKind], relocating: bool) {
        self.ui.dock = workspace;
        for &p in closed {
            if let PanelKind::ScriptPanel(id) = p
                && self.session.script_ui.window(id).is_some()
                && let Err(e) = self.session.execute("scriptui.close", json!({"window": id}))
            {
                self.ui.status = e.to_string();
            }
            panels::viewers::on_close(self, p);
        }
        if relocating || self.ui.maximized.is_some_and(|m| !self.ui.dock.is_docked(m)) {
            self.ui.maximized = None;
        }
    }

    /// Close a panel wherever it is (docked or floating). A rejected close changes nothing.
    pub fn close_panel(&mut self, p: PanelKind) {
        if self.ui.dock.is_open(p) {
            let _ = self.apply_dock(Action::Close { panel: p });
        }
    }

    /// Toggle the maximized panel (`p`, or none). Only a docked frame maximizes.
    pub fn toggle_maximize(&mut self, p: PanelKind) {
        self.ui.maximized = if self.ui.maximized == Some(p) || !self.ui.dock.is_docked(p) { None } else { Some(p) };
    }

    /// The panel whose body is under `pos` (from the last frame), else the focused one.
    pub fn panel_at(&self, pos: Option<egui::Pos2>) -> PanelKind {
        pos.and_then(|p| self.dock_rects.iter().rev().find(|(_, r)| r.contains(p)).map(|(k, _)| *k)).unwrap_or(self.ui.focused)
    }

    /// Whether panel `p` is in a floating window with a desktop place: its own OS window on
    /// desktop (craft-ui `native-windows`); on the web and in embedded viewports the window
    /// stays in the page and the place is kept for a desktop session.
    pub fn in_panel_window(&self, p: PanelKind) -> bool {
        self.ui.dock.layout.floating.iter().any(|w| w.contains(&p) && w.native.is_some())
    }

    /// Move panel `p` into a floating window of its own, which on desktop is an OS window that
    /// can go on another monitor, or back into the main window (`window` false), where it joins
    /// the first docked group. `None` toggles. A docked panel is undocked first, and its window
    /// opens over the panel's place in the main window. Returns whether `p` is now in a window
    /// of its own.
    pub fn set_panel_window(&mut self, ctx: &egui::Context, p: PanelKind, window: Option<bool>) -> Result<bool, String> {
        let current = self.in_panel_window(p);
        let window = window.unwrap_or(!current);
        if window == current {
            return Ok(current);
        }
        if !window {
            // On desktop every floating window is an OS window, so the way back is docking.
            let anchor = self.ui.dock.layout.root.as_ref().and_then(|root| {
                let mut panels = Vec::new();
                dock::node_panels(root, &mut panels);
                panels.into_iter().find(|panel| *panel != p)
            });
            let actions = match anchor {
                Some(anchor) => vec![Action::Move { panel: p, anchor, placement: craft_ui::docking::Placement::Tab { before: None } }],
                // Nothing is docked: the panel becomes the docked layout.
                None => vec![Action::Close { panel: p }, Action::Open { panel: p, anchor: None }],
            };
            let mut workspace = self.ui.dock.clone();
            // One transaction, so the panel is never reported closed and keeps its state.
            let closed = workspace.apply_all(actions, dock::permissions).map_err(|_| format!("can't dock {}", p.title()))?;
            self.commit_workspace(workspace, &closed, true);
            return Ok(false);
        }
        if !self.ui.dock.is_floating(p) {
            if !self.ui.dock.is_docked(p) {
                return Err(format!("{} is not open", p.title()));
            }
            let rect = default_float_rect(ctx.input_for(egui::ViewportId::ROOT, |i| i.content_rect()));
            self.apply_dock(Action::Float { panel: p, rect }).map_err(|_| format!("can't undock {}", p.title()))?;
        }
        let Some([x, y, w, h]) = self.ui.dock.layout.floating.iter().find(|f| f.contains(&p)).map(|f| f.rect) else {
            return Err(format!("can't undock {}", p.title()));
        };
        let origin = ctx.input_for(egui::ViewportId::ROOT, |i| i.viewport().inner_rect.map(|r| r.min)).unwrap_or(pos2(0.0, 0.0));
        let rect = sanitized_window_rect([origin.x + x, origin.y + y, w, h]);
        self.apply_dock(Action::SetNative { panel: p, rect: Some(rect) })?;
        if self.ui.maximized == Some(p) {
            self.ui.maximized = None;
        }
        Ok(true)
    }

    /// Tab labels that name what the panel shows, as After Effects does: "Composition Intro",
    /// "Effect Controls Title", "Properties: Title" (the Timeline's tabs are its comps, see
    /// [`Self::tab_docs`]).
    fn tab_titles(&self) -> Vec<(PanelKind, String)> {
        let mut out = Vec::new();
        // ScriptUI panels are named after their script.
        for w in self.session.script_ui.windows.iter().filter(|w| w.kind == effectcraft_engine::scriptui::WindowKind::Panel) {
            if let Some(t) = panels::scriptui_view::panel_title(self, w.id) {
                out.push((PanelKind::ScriptPanel(w.id), t));
            }
        }
        // Each Composition viewer is named after the comp it shows.
        for id in std::iter::once(0).chain(self.ui.viewers.keys().copied().filter(|v| *v != 0)) {
            if let Some(t) = panels::viewers::title(self, id) {
                out.push((panels::viewers::panel(id), t));
            }
        }
        let Some(comp) = self.session.active_comp() else { return out };
        if let Some(l) = self.session.state.selected_layers.first().and_then(|id| comp.layer(*id)) {
            out.push((PanelKind::EffectControls, format!("{} {}", crate::i18n::command_label("window.panel", "Effect Controls"), l.name)));
            out.push((PanelKind::Properties, format!("{}: {}", crate::i18n::command_label("window.panel", "Properties"), l.name)));
        }
        out
    }

    /// A comp's label colour as a tab swatch (faint without a label).
    fn comp_swatch(&self, cid: effectcraft_engine::project::ItemId) -> egui::Color32 {
        match self.session.project.item(cid).map(|i| i.label) {
            Some(l) if l != effectcraft_engine::color::Label::None => self.tokens.label(l),
            _ => self.tokens.text_faint,
        }
    }

    /// The Timeline's comp tabs: one per open comp, as in After Effects (opening a precomp adds a
    /// tab next to its parent's instead of replacing it).
    fn tab_docs(&self) -> Vec<DocTab> {
        let st = &self.session.state;
        st.open_comps
            .iter()
            .filter_map(|id| {
                let it = self.session.project.item(*id).filter(|i| i.as_comp().is_some())?;
                Some(DocTab { id: id.0, title: it.name.clone(), swatch: self.comp_swatch(*id), active: st.active_comp == Some(*id) })
            })
            .collect()
    }

    /// The label-colour swatch before a tab's name: each Composition viewer's comp, and Effect
    /// Controls' (and with Use Label Color for Related Tabs, Properties') selected layer.
    fn tab_swatches(&self) -> Vec<(PanelKind, egui::Color32)> {
        let mut v = Vec::new();
        for id in std::iter::once(0).chain(self.ui.viewers.keys().copied().filter(|v| *v != 0)) {
            let swatch = panels::viewers::comp_of(self, id).map_or(self.tokens.text_faint, |c| self.comp_swatch(c));
            v.push((panels::viewers::panel(id), swatch));
        }
        if let Some(l) = self.session.active_comp().and_then(|c| self.session.state.selected_layers.first().and_then(|id| c.layer(*id)))
            && l.label != effectcraft_engine::color::Label::None
        {
            v.push((PanelKind::EffectControls, self.tokens.label(l.label)));
            if self.session.prefs.appearance.use_label_color_for_tabs {
                v.push((PanelKind::Properties, self.tokens.label(l.label)));
            }
        }
        v
    }

    /// Draw panel `p`'s body in `rect`: register its automation region, put the Composition Navigator above an active
    /// viewer when a nested-comp flow exists, then call the panel's `show` and (for a viewer) the sketch overlay.
    fn panel_body(&mut self, ui: &mut egui::Ui, p: PanelKind, rect: Rect) {
        self.auto.add(&format!("panel.{}", p.id()), rect, p.title());
        let mut content = rect.shrink(1.0);
        let viewer = p == panels::viewers::active_panel(self);
        if viewer && !self.ui.start_screen && panels::precomp::has_flow(self) {
            // Composition Navigator: the flow of nested comps above the viewer.
            let nav = Rect::from_min_size(content.min, vec2(content.width(), panels::precomp::NAV_H));
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(nav).id_salt("comp-navigator"));
            child.set_clip_rect(nav.intersect(ui.clip_rect()));
            panels::precomp::navigator(self, &mut child, nav);
            content.min.y = nav.max.y;
        }
        let mut child = ui.new_child(
            egui::UiBuilder::new().max_rect(content).id_salt(("panel", p.id())).ui_stack_info(egui::UiStackInfo::default().with_tag(crate::widgets::PANEL_TAG)),
        );
        child.set_clip_rect(content.intersect(ui.clip_rect()));
        panels::show(self, &mut child, p, content);
        if viewer && !self.ui.start_screen {
            panels::anim_tools::sketch_overlay(self, &mut child);
        }
    }

    /// A press in a panel (its body, tab or group header) focuses it; another Composition viewer
    /// also becomes the active one.
    fn focus_panel(&mut self, ctx: &egui::Context, p: PanelKind) {
        self.ui.focused = p;
        panels::viewer_text::panel_clicked(self, ctx, p);
        if let Some(id) = panels::viewers::id_of(p) {
            panels::viewers::activate(self, ctx, id);
        }
    }

    /// Whether floating panels' OS windows take no input: a dialog of the app's own or a
    /// command's OS file dialog is up. egui's `Modal` blocks only the window it is in.
    pub(crate) fn floats_blocked(&mut self, ctx: &egui::Context) -> bool {
        let (focused, now) = ctx.input(|i| (i.focused || i.raw.viewports.values().any(|v| v.focused == Some(true)), i.time));
        let file_dialog = self.file_dialogs.up(focused, now);
        self.dialog.is_some() || panels::dialogs::modal_open(ctx) || file_dialog
    }

    /// Draw the Home screen or workspace in `body`. Shared craft-ui docking owns the dock tree,
    /// group chrome, tab dragging and floating native windows; this adapter draws panel bodies,
    /// applies the returned events/actions and shows the panel menu.
    pub(crate) fn dock_area(&mut self, ui: &mut egui::Ui, body: Rect) {
        let t = self.tokens;
        let ctx = ui.ctx().clone();
        if !self.ui.native_outer.is_empty() {
            let (outer, inner, now) = ctx.input_for(egui::ViewportId::ROOT, |i| (i.viewport().outer_rect, i.viewport().inner_rect, i.time));
            native_outer_to_content(&mut self.ui, outer, inner, now);
        }
        if self.ui.start_screen {
            // Home covers the whole workspace, as in After Effects (the panels wait behind it).
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(body).id_salt("home"));
            child.set_clip_rect(body.intersect(ui.clip_rect()));
            panels::home::show(self, &mut child, body);
            return;
        }
        let style = dock_style(ui, &t);
        let maximized = self.ui.maximized.filter(|m| self.ui.dock.is_docked(*m));
        let mut area = DockArea::new(area_id()).float_bounds(body).maximized(maximized).locked(self.ui.dock.locked).input_blocked(self.floats_blocked(&ctx));
        if let Some(panel) = self.pending_panel_raise.take() {
            area = area.raise_panel(panel);
        }
        let layout = self.ui.dock.layout.clone();
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(body).id_salt("effectcraft-dock-area"));
        child.set_clip_rect(body.intersect(ui.clip_rect()));
        let docs = self.tab_docs();
        let ime = NativeIme::new(&layout.floating, &self.unpublished_viewports);
        let mut content = EffectContent {
            titles: self.tab_titles(),
            swatches: self.tab_swatches(),
            docs,
            ctx: ctx.clone(),
            style: style.clone(),
            app: self,
            tokens: t,
            doc_actions: Vec::new(),
            locks: Vec::new(),
            menu: None,
            bodies: Vec::new(),
            ime,
        };
        let mut output = area.show_content(&mut child, &layout, &style, &mut content);
        let EffectContent { doc_actions, locks, menu, bodies, ime, .. } = content;
        self.dock_rects = bodies;
        self.unpublished_viewports = ime.unpublished();
        // A window that closed or docked takes its composition with it.
        let previous = std::mem::replace(&mut self.native_viewports, ime.begun());
        let gone: Vec<_> = previous.into_iter().filter(|v| !self.native_viewports.contains(v)).collect();
        for viewport in gone {
            panels::viewer_text::end_viewport(self, &ctx, viewport);
        }
        self.forward_window_input(&ctx, std::mem::take(&mut output.forwarded_events), std::mem::take(&mut output.window_files));
        if let Some(panel) = output.focused {
            self.focus_panel(&ctx, panel);
        }
        if let Some(error) = output.error {
            self.ui.status = error.to_string();
        }
        for event in output.events {
            if let DockEvent::ToggleMaximize { panel } = event {
                self.toggle_maximize(panel);
            }
        }
        for action in output.actions {
            if let Err(error) = self.dock_command(&ctx, action) {
                self.ui.status = error;
            }
        }
        for p in locks {
            if !self.ui.locked_tabs.remove(&p.id()) {
                self.ui.locked_tabs.insert(p.id().to_string());
            }
        }
        for action in doc_actions {
            let result = match action {
                // The Timeline's comp tabs: show that comp / close its Timeline.
                DocAction::Activate(id) => {
                    let _ = self.apply_dock(Action::Activate { panel: PanelKind::Timeline });
                    self.focus_panel(&ctx, PanelKind::Timeline);
                    if self.session.state.active_comp.map(|c| c.0) == Some(id) {
                        Ok(Value::Null)
                    } else {
                        crate::menus::invoke(self, &ctx, "comp.open", json!({"comp": id}))
                    }
                }
                DocAction::Close(id) => crate::menus::invoke(self, &ctx, "comp.close", json!({"comp": id})),
            };
            if let Err(error) = result {
                self.ui.status = error;
            }
        }
        if let Some((panel, pos)) = menu {
            ctx.data_mut(|d| d.insert_temp(egui::Id::unique("panel-menu"), (panel, pos)));
        }
        let hits = DockArea::<PanelKind>::hit_rects(&ctx, area_id());
        self.register_dock_hits(&hits);
        panels::panel_menu_popup(self, ui);
    }

    /// Keys and dropped files that reach a floating panel's own OS window go to the main
    /// window's next frame ([`crate::EffectcraftApp::raw_input_hook`]), so its shortcuts and
    /// file import handle them. Text is not forwarded, and nothing is while a text field in
    /// the main window has the keyboard.
    fn forward_window_input(&mut self, ctx: &egui::Context, events: Vec<egui::Event>, files: Vec<craft_ui::docking::WindowFiles<PanelKind>>) {
        let typing = ctx.egui_wants_keyboard_input();
        let events = events.into_iter().filter(|event| !typing && !matches!(event, egui::Event::Text(_) | egui::Event::Ime(_)));
        self.forwarded.events.extend(events);
        self.forwarded.files.extend(files.into_iter().flat_map(|window| window.dropped));
        if !self.forwarded.events.is_empty() || !self.forwarded.files.is_empty() {
            ctx.request_repaint();
        }
    }

    /// Automation ids for craft-ui's gesture sources: `panel.tab.<Panel>`, `panel.drag.<Panel>`
    /// (the group's drag bar), `panel.border.<Panel>` (the top border above its tabs),
    /// `panel.tabs.more.<Panel>`, `panel.header.<Panel>` (a stacked panel's open/close button),
    /// `panel.float.<n>.move`, `.close`, `.resize` (bottom-right
    /// corner) and `.resize.<Edge>`, `dock.gutter.<path>` (`a`/`b` per split level; a stack's gap
    /// below entry `i` adds `s<i>`), `dock.corner.<outer>.<inner>` where two dividers meet, and
    /// the same inside floating window `n` as `panel.float.<n>.gutter.…` and `.corner.…`.
    fn register_dock_hits(&mut self, hits: &[(Hit<PanelKind>, Rect)]) {
        let window = |panel: &PanelKind| self.ui.dock.layout.floating.iter().position(|window| window.contains(panel));
        // Tabs are labelled as drawn: "Composition Intro", not "Composition".
        let titles = self.tab_titles();
        let title = |panel: &PanelKind| titles.iter().find(|(p, _)| p == panel).map_or_else(|| panel.title().to_string(), |(_, t)| t.clone());
        let mut found = Vec::new();
        for (hit, rect) in hits {
            let entry = match hit {
                Hit::Tab(panel) => (format!("panel.tab.{}", panel.id()), title(panel)),
                Hit::DragBar(panel) => (format!("panel.drag.{}", panel.id()), "Move panel group".into()),
                Hit::Border(panel) => (format!("panel.border.{}", panel.id()), "Move panel group".into()),
                Hit::Overflow(panel) => (format!("panel.tabs.more.{}", panel.id()), "More tabs".into()),
                Hit::StackToggle(panel) => (format!("panel.header.{}", panel.id()), title(panel)),
                Hit::Title(panel) => match window(panel) {
                    Some(index) => (format!("panel.float.{index}.move"), "Move floating panel".into()),
                    None => continue,
                },
                Hit::Close(panel) => match window(panel) {
                    Some(index) => (format!("panel.float.{index}.close"), "Close floating panel".into()),
                    None => continue,
                },
                Hit::Resize(panel, edge) => match window(panel) {
                    Some(index) if *edge == ResizeEdge::BottomRight => (format!("panel.float.{index}.resize"), "Resize floating panel".into()),
                    Some(index) => (format!("panel.float.{index}.resize.{edge:?}"), "Resize floating panel".into()),
                    None => continue,
                },
                Hit::Divider(Divider::Split(path)) => (format!("dock.gutter.{}", path_id(path)), "gutter".into()),
                Hit::Divider(Divider::Stack(panel)) => match dock::stack_gutter_id(&self.ui.dock.layout, *panel) {
                    Some(id) => (id, "gutter".into()),
                    None => continue,
                },
                Hit::Divider(Divider::Intersection(outer, inner)) => (format!("dock.corner.{}.{}", path_id(outer), path_id(inner)), "gutter corner".into()),
                Hit::Divider(Divider::WindowSplit(panel, path)) => match window(panel) {
                    Some(index) => (format!("panel.float.{index}.gutter.{}", path_id(path)), "gutter".into()),
                    None => continue,
                },
                Hit::Divider(Divider::WindowIntersection(panel, outer, inner)) => match window(panel) {
                    Some(index) => (format!("panel.float.{index}.corner.{}.{}", path_id(outer), path_id(inner)), "gutter corner".into()),
                    None => continue,
                },
                _ => continue,
            };
            found.push((entry, *rect, DockArea::hit_id(area_id(), hit)));
        }
        // The widget id lets keyboard focus on a divider or grip map to its element.
        for ((id, label), rect, widget) in found {
            self.auto.add(&id, rect, &label);
            if let Some(element) = self.auto.elements.last_mut() {
                element.response_id = Some(widget);
            }
        }
    }
}

/// A split path as `a`/`b` per level (first/second child).
fn path_id(path: &[bool]) -> String {
    path.iter().map(|second| if *second { 'b' } else { 'a' }).collect()
}

/// After Effects' window is all frames: drops on all four window edges, Ctrl/Cmd-drag undocks,
/// a release over no target floats, and a floating window's title bar only moves it.
fn dock_style(ui: &egui::Ui, t: &Tokens) -> DockStyle {
    let mut style = DockStyle::from_ui(ui);
    style.side_edge_zones = true;
    style.new_dock_zones = false;
    style.maximize_item = true;
    style.close_others_item = true;
    style.command_press_floats = true;
    style.float_on_empty_drop = true;
    style.title_bar_docks = false;
    style.float_hint = true;
    style.wheel_selects_tabs = true;
    style.middle_click_closes_tabs = true;
    style.float_label = "Undock Panel".into();
    style.float_group_label = "Undock Frame".into();
    style.close_group_label = "Close Frame".into();
    style.tab_height = t.tab_h;
    style.gap = t.gap;
    style.background = t.panel_bg;
    style.tab_background = t.panel_bg;
    style.active_background = t.panel_bg;
    style.floating_background = t.panel_bg;
    style.text = t.tab_text_active;
    style.inactive_text = t.tab_text;
    style.accent = t.focus;
    style.font = Tokens::ui(12.0);
    style
}

/// Panels with a viewer lock: the Composition viewers and the Timeline keep their comp while
/// locked.
fn has_lock(panel: PanelKind) -> bool {
    matches!(panel, PanelKind::Composition | PanelKind::Viewer(_) | PanelKind::Timeline)
}

/// EffectCraft's side of the dock area for one frame.
struct EffectContent<'a> {
    app: &'a mut EffectcraftApp,
    tokens: Tokens,
    titles: Vec<(PanelKind, String)>,
    swatches: Vec<(PanelKind, egui::Color32)>,
    docs: Vec<DocTab>,
    /// Measures the comp tabs for their width in the tab row.
    ctx: egui::Context,
    style: DockStyle,
    doc_actions: Vec<DocAction>,
    /// Viewer locks clicked.
    locks: Vec<PanelKind>,
    /// The ≡ button clicked: its panel and where its menu opens.
    menu: Option<(PanelKind, egui::Pos2)>,
    /// Where each panel body was drawn, in drawing order (floating windows last).
    bodies: Vec<(PanelKind, Rect)>,
    /// Where each native window's IME descriptor is set up and published.
    ime: NativeIme,
}

/// Which body of a native floating window publishes its IME descriptor.
///
/// Each OS window is an egui pass of its own. `viewer_text::finish_ime` publishes what the pass's
/// text editor recorded, and with nothing recorded it clears the composition and the preedit. A
/// window with another group before the Viewer that published after every body would clear the
/// preedit before the Viewer drew, and the Viewer's publication would then be a new context that
/// interrupts the composition, every pass. So a window publishes after the body in which its
/// editor has recorded its geometry and after its last body.
pub(crate) struct NativeIme {
    /// The bodies each native window draws this pass, as its layout says.
    expected: Vec<(egui::ViewportId, usize)>,
    /// The bodies each native window has drawn so far, in the order the windows began.
    drawn: Vec<(egui::ViewportId, usize)>,
    /// Windows that published this pass.
    published: Vec<egui::ViewportId>,
}

impl NativeIme {
    /// `unpublished` are the windows that drew last pass but never published: the layout's count
    /// was too high for them, so they publish after every body this pass.
    pub(crate) fn new(floating: &[Floating<PanelKind>], unpublished: &[egui::ViewportId]) -> Self {
        let expected = floating
            .iter()
            .filter_map(|window| {
                let viewport = window_viewport(window.id)?;
                let count = if unpublished.contains(&viewport) { 0 } else { window_bodies(window) };
                Some((viewport, count))
            })
            .collect();
        Self { expected, drawn: Vec::new(), published: Vec::new() }
    }

    /// Count a body of `viewport` that starts drawing. True for the window's first body: its
    /// pass sets up the IME before it.
    pub(crate) fn begin_body(&mut self, viewport: egui::ViewportId) -> bool {
        match self.drawn.iter_mut().find(|(v, _)| *v == viewport) {
            Some((_, n)) => {
                *n = n.saturating_add(1);
                false
            }
            None => {
                self.drawn.push((viewport, 1));
                true
            }
        }
    }

    /// Whether the body of `viewport` that just drew publishes the IME descriptor: once the
    /// window's editor has `recorded` its geometry this pass, and after the window's last body.
    /// A window the layout does not name publishes after every body.
    pub(crate) fn publishes(&mut self, viewport: egui::ViewportId, recorded: bool) -> bool {
        let drawn = self.drawn.iter().find(|(v, _)| *v == viewport).map_or(0, |(_, n)| *n);
        let expected = self.expected.iter().find(|(v, _)| *v == viewport).map_or(0, |(_, n)| *n);
        let publishes = recorded || drawn >= expected;
        if publishes && !self.published.contains(&viewport) {
            self.published.push(viewport);
        }
        publishes
    }

    /// The native windows whose pass began.
    pub(crate) fn begun(&self) -> Vec<egui::ViewportId> {
        self.drawn.iter().map(|(v, _)| *v).collect()
    }

    /// The native windows that drew bodies but did not publish.
    pub(crate) fn unpublished(&self) -> Vec<egui::ViewportId> {
        self.drawn.iter().map(|(v, _)| *v).filter(|v| !self.published.contains(v)).collect()
    }
}

/// The OS window craft-ui opens for the floating window `id`.
#[cfg(not(target_arch = "wasm32"))]
fn window_viewport(id: u64) -> Option<egui::ViewportId> {
    Some(DockArea::<PanelKind>::window_viewport(area_id(), id))
}

/// The web keeps floating windows in the page.
#[cfg(target_arch = "wasm32")]
fn window_viewport(_id: u64) -> Option<egui::ViewportId> {
    None
}

/// The panel bodies craft-ui draws in a floating window: the active panel of each group and each
/// open stack entry. A minimized window shows its bar only.
fn window_bodies(window: &Floating<PanelKind>) -> usize {
    if window.minimized {
        return 0;
    }
    let mut count = 0usize;
    let mut nodes = vec![&window.root];
    while let Some(node) = nodes.pop() {
        match node {
            Node::Split { first, second, .. } => nodes.extend([&**first, &**second]),
            Node::Tabs { panels, active } => count = count.saturating_add(usize::from(panels.get(*active).is_some())),
            Node::Stack { entries } => count = count.saturating_add(entries.iter().filter(|e| e.open).count()),
            _ => {}
        }
    }
    count
}

impl EffectContent<'_> {
    fn header_width(&self, active: PanelKind, strip_width: f32) -> f32 {
        let lock = if has_lock(active) { LOCK_W } else { 0.0 };
        (lock + MENU_W).min(strip_width * 2.0 / 3.0).max(MENU_W)
    }

    /// Whether the group of `panels` shows comp tabs: it holds the Timeline and comps are open.
    fn has_doc_tabs(&self, panels: &[PanelKind]) -> bool {
        panels.contains(&PanelKind::Timeline) && !self.docs.is_empty()
    }
}

impl DockContent<PanelKind> for EffectContent<'_> {
    fn body(&mut self, ui: &mut egui::Ui, panel: &PanelKind) {
        let t = self.tokens;
        let rect = ui.max_rect();
        self.bodies.push((*panel, rect));
        // A floating panel's OS window is a pass of its own: its text IME descriptor is set up
        // before its first body and published after its bodies (the main window does both in
        // `draw`).
        let viewport = ui.ctx().viewport_id();
        let native = viewport != egui::ViewportId::ROOT && !ui.ctx().embed_viewports();
        if native && self.ime.begin_body(viewport) {
            panels::viewer_text::begin_ui(ui.ctx());
        }
        self.app.panel_body(ui, *panel, rect);
        if native && self.ime.publishes(viewport, panels::viewer_text::ime_recorded(ui.ctx())) {
            panels::viewer_text::finish_ime(self.app, ui);
        }
        // The body fills its frame; allocating it lets a floated panel keep its full size.
        ui.allocate_rect(rect, Sense::hover());
        // Painted after the body so child backgrounds cannot hide the focus outline.
        if self.app.ui.focused == *panel {
            ui.painter().rect_stroke(rect, 0.0, Stroke::new(1.0, t.focus), StrokeKind::Inside);
        }
    }

    fn logical_label(&self, panel: &PanelKind) -> String {
        let title = panel.title();
        // The Window menu names most panels; `Markers` is only in the panel catalog, and the two
        // panels in neither (`History`, `ScriptUI Panel`) keep their English names.
        let listed = crate::i18n::command_label("window.panel", title);
        if listed == title { crate::i18n::tr(title).to_string() } else { listed.to_string() }
    }

    /// The tab names what the panel shows ("Composition Intro"), after the label swatch of its
    /// comp or layer. With comps open, the Timeline's own tab is just "Timeline"; each comp has a
    /// tab of its own after the group's panel tabs.
    fn label(&mut self, _ui: &egui::Ui, panel: &PanelKind, logical: &str, font: &egui::FontId, color: egui::Color32) -> egui::WidgetText {
        let title = self.titles.iter().find(|(p, _)| p == panel).map_or(logical, |(_, s)| s.as_str());
        let Some((_, swatch)) = self.swatches.iter().find(|(p, _)| p == panel) else { return title.into() };
        let mut job = egui::text::LayoutJob::default();
        job.append("■ ", 0.0, egui::TextFormat::simple(font.clone(), *swatch));
        job.append(title, 0.0, egui::TextFormat::simple(font.clone(), color));
        job.into()
    }

    fn group_header_width(&self, _panels: &[PanelKind], active: &PanelKind, _floating: bool, strip_width: f32) -> f32 {
        self.header_width(*active, strip_width)
    }

    /// The viewer lock and the ≡ panel menu of the shown panel.
    fn group_header(&mut self, ui: &mut egui::Ui, _panels: &[PanelKind], active: &PanelKind, _floating: bool) {
        let t = self.tokens;
        let header = ui.max_rect();
        let menu = Rect::from_min_max(pos2((header.max.x - MENU_W).max(header.min.x), header.min.y), header.max);
        if has_lock(*active) {
            let lock = Rect::from_center_size(pos2(menu.min.x - LOCK_W / 2.0, header.center().y), vec2(14.0, 14.0));
            let locked = self.app.ui.locked_tabs.contains(&active.id());
            if dock::lock_button(ui, lock, *active, locked, &t, &mut self.app.auto) {
                self.locks.push(*active);
            }
        }
        let rect = Rect::from_center_size(menu.center(), vec2(18.0, 18.0));
        let response = ui.interact(rect, egui::Id::unique(("panel-menu-button", active.id())), Sense::click());
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, response.enabled(), "panel menu"));
        self.app.auto.add_response(&format!("panel.menu.{}", active.id()), &response, "panel menu");
        let color = if response.hovered() { t.tab_text_active } else { t.tab_text };
        icons::paint(ui.painter(), Rect::from_center_size(rect.center(), vec2(16.0, 16.0)), Icon::Hamburger, color);
        crate::widgets::focus_outline(ui, &response, &t);
        if response.clicked() {
            self.app.ui.focused = *active;
            self.menu = Some((*active, rect.left_bottom()));
        }
    }

    fn group_extra_tabs_width(&self, panels: &[PanelKind], _active: &PanelKind, _floating: bool) -> f32 {
        if !self.has_doc_tabs(panels) {
            return 0.0;
        }
        dock::doc_widths(&self.ctx, &self.docs, &self.style.font).iter().sum()
    }

    /// The Timeline's comp tabs, after its group's panel tabs whichever panel is shown.
    fn group_extra_tabs(&mut self, ui: &mut egui::Ui, panels: &[PanelKind], active: &PanelKind, _floating: bool) {
        if self.has_doc_tabs(panels) {
            let selected = *active == PanelKind::Timeline;
            self.doc_actions.extend(dock::doc_tabs(ui, ui.max_rect(), &self.docs, selected, &self.style, &mut self.app.auto));
        }
    }
}
