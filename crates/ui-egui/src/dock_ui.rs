//! EffectCraft's panels in craft-ui's shared docking. craft-ui's `DockArea` draws the frames,
//! tab strips (the Timeline's comp tabs included, as its panel's documents), stacked panels,
//! floating windows, dividers and drop previews and owns the gestures (drag a tab onto a group to
//! tab it there or split it off to a side; hold Cmd/Ctrl at the release, or drop outside every
//! group, to float it). EffectCraft draws the panel bodies and the viewer lock and ≡ panel menu in
//! each group header, and runs every layout change through its command path
//! (`window.dockAction`), so pointer, menu and control-channel changes share one route.

use std::sync::Arc;

use craft_behavior::dock::{Action, Floating};
use craft_ui::contract::{Bind, BodyCx, PlainData, Tracked};
use craft_ui::docking::{DividerKind, DockConfig, DockEvent, DockPanels, DockSource, FloatSpace, PanelDocument, PanelSpec, PanelSpecs, SourceKind};
use craft_ui::patterns::dock::{DockArea, DockAreaOutput, WindowFiles};
use craft_ui::runtime::windows::ResizeEdge;
use egui::{Rect, Sense, Stroke, StrokeKind, Ui, vec2};
use serde_json::{Value, json};

use crate::dock::{self, PanelKind, Permissions, Workspace, WorkspaceExt as _};
use crate::dock_command::DockCommand;
use crate::icons::{self, Icon};
use crate::theme::Tokens;
use crate::{EffectcraftApp, panels};

/// Width of the ≡ panel menu button in each group header.
const MENU_W: f32 = 26.0;
/// Width of the viewer lock before it.
const LOCK_W: f32 = 18.0;

/// A new floating group's default rect near the centre of `screen`.
pub fn default_float_rect(screen: Rect) -> Rect {
    Rect::from_center_size(screen.center(), vec2(420.0, 320.0))
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

/// The docking commands: `window.dockAction {action}`, the `window.panel.*` commands, `ui.dock`
/// and `ui.dock.reset`. `None` for any other id.
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
            let command = DockCommand::parse("ui.dock", params).map_err(|error| format!("invalid ui.dock params: {error}"))?;
            app.run_dock_command(ctx, command)?;
            Ok(json!({"docking": app.ui.dock}))
        })()),
        _ if DockCommand::IDS.contains(&id) => Some((|| {
            let mut params = if params.is_object() { params.clone() } else { json!({}) };
            if let Some(fields) = params.as_object_mut() {
                // EffectCraft's panel names ("Effect Controls") as well as the serialized ids.
                for key in ["panel", "anchor", "before"] {
                    if let Some(name) = fields.get(key).and_then(Value::as_str) {
                        let panel = PanelKind::from_name(name).ok_or(format!("unknown panel `{name}`"))?;
                        fields.insert(key.into(), json!(panel));
                    }
                }
            }
            let command = DockCommand::parse(id, &params).map_err(|error| format!("invalid docking command: {error}"))?;
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

    /// Run a docking command on the current workspace, all or nothing. Named workspaces and
    /// Reset are EffectCraft's own (`window.workspace`, `window.resetWorkspace`).
    pub(crate) fn run_dock_command(&mut self, ctx: &egui::Context, command: DockCommand) -> Result<(), String> {
        if let Some(action) = command.raw_action() {
            check_raw_action(action)?;
        }
        if let DockCommand::Window { panel, native } = command {
            return self.set_panel_window(ctx, panel, native.or(Some(true))).map(drop);
        }
        // A closed panel opens through EffectCraft's own path, which puts it in its usual place
        // and runs the viewer and ScriptUI lifecycles.
        if let Some(panel) = command.shown().filter(|panel| !self.ui.dock.is_open(*panel)) {
            self.show_panel(panel);
            if command.edge().is_none() {
                return Ok(());
            }
        }
        let actions = command.to_actions(&self.ui.dock, self.live_dock.float_space.as_ref()).map_err(|error| error.to_string())?;
        self.apply_dock_all(actions)
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
            let docked = workspace.layout().root.is_some();
            closed.extend(workspace.apply(action, self.ui.dock_locked, dock::permissions).map_err(|error| error.to_string())?);
            if !closing && docked && workspace.layout().root.is_none() {
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
            let _ = self.apply_dock(Action::close(p));
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

    /// Whether panel `p` is in a floating window: on desktop each is an OS window of its own
    /// (`DockConfig::native_windows`), which can go on another monitor; on the web and in
    /// embedded viewports it stays in the page.
    pub fn in_panel_window(&self, p: PanelKind) -> bool {
        self.ui.dock.is_floating(p)
    }

    /// Move panel `p` into a floating window of its own, or back into the main window
    /// (`window` false), where it joins the first docked group. `None` toggles. A docked panel
    /// floats over its place in the main window. Returns whether `p` is now in a window.
    pub fn set_panel_window(&mut self, ctx: &egui::Context, p: PanelKind, window: Option<bool>) -> Result<bool, String> {
        let current = self.in_panel_window(p);
        let window = window.unwrap_or(!current);
        if window == current {
            return Ok(current);
        }
        if !window {
            let anchor = self.ui.dock.layout().root.as_ref().and_then(|root| {
                let mut panels = Vec::new();
                dock::node_panels(root, &mut panels);
                panels.into_iter().find(|panel| *panel != p)
            });
            let actions = match anchor {
                Some(anchor) => vec![Action::move_beside(p, anchor, craft_behavior::dock::DockPlacement::tab(None))],
                // Nothing is docked: the panel becomes the docked layout.
                None => vec![Action::close(p), Action::open(p, None)],
            };
            let mut workspace = self.ui.dock.clone();
            // One transaction, so the panel is never reported closed and keeps its state.
            let closed = workspace.apply_all(actions, self.ui.dock_locked, dock::permissions).map_err(|_| format!("can't dock {}", p.title()))?;
            self.commit_workspace(workspace, &closed, true);
            return Ok(false);
        }
        if !self.ui.dock.is_docked(p) {
            return Err(format!("{} is not open", p.title()));
        }
        let place = self.dock_rects.iter().find(|(panel, _)| *panel == p).map(|(_, rect)| *rect);
        let rect = place.unwrap_or_else(|| default_float_rect(ctx.input_for(egui::ViewportId::ROOT, |i| i.content_rect())));
        self.apply_dock(Action::float(p, rect)).map_err(|_| format!("can't undock {}", p.title()))?;
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
        // Effect Controls names the layer it shows (the one it is locked to, if any).
        if let Some(l) = panels::effect_controls::shown_layer_id(self).and_then(|id| comp.layer(id)) {
            out.push((PanelKind::EffectControls, format!("{} {}", crate::i18n::command_label("window.panel", "Effect Controls"), l.name)));
        }
        if let Some(l) = self.session.state.selected_layers.first().and_then(|id| comp.layer(*id)) {
            out.push((PanelKind::Properties, format!("{}: {}", crate::i18n::command_label("window.panel", "Properties"), l.name)));
        }
        out
    }

    /// A comp's label colour as a tab swatch; `None` without a label (the theme's empty swatch).
    fn comp_swatch(&self, cid: effectcraft_engine::project::ItemId) -> Option<egui::Color32> {
        match self.session.project.item(cid).map(|i| i.label) {
            Some(l) if l != effectcraft_engine::color::Label::None => Some(self.tokens.label(l)),
            _ => None,
        }
    }

    /// The Timeline's comp tabs: one per open comp, as in After Effects (opening a precomp adds a
    /// tab next to its parent's instead of replacing it).
    fn tab_docs(&self) -> Vec<DocRow> {
        let st = &self.session.state;
        st.open_comps
            .iter()
            .filter_map(|id| {
                let it = self.session.project.item(*id).filter(|i| i.as_comp().is_some())?;
                Some(DocRow { id: id.0, label: it.name.clone(), swatch: self.comp_swatch(*id).map(rgba), shown: st.active_comp == Some(*id) })
            })
            .collect()
    }

    /// The label-colour swatch before a tab's name: each Composition viewer's comp, and Effect
    /// Controls' (and with Use Label Color for Related Tabs, Properties') selected layer.
    fn tab_swatches(&self) -> Vec<(PanelKind, Option<egui::Color32>)> {
        let mut v = Vec::new();
        for id in std::iter::once(0).chain(self.ui.viewers.keys().copied().filter(|v| *v != 0)) {
            let swatch = panels::viewers::comp_of(self, id).and_then(|c| self.comp_swatch(c));
            v.push((panels::viewers::panel(id), swatch));
        }
        if let Some(l) = self.session.active_comp().and_then(|c| self.session.state.selected_layers.first().and_then(|id| c.layer(*id)))
            && l.label != effectcraft_engine::color::Label::None
        {
            v.push((PanelKind::EffectControls, Some(self.tokens.label(l.label))));
            if self.session.prefs.appearance.use_label_color_for_tabs {
                v.push((PanelKind::Properties, Some(self.tokens.label(l.label))));
            }
        }
        v
    }

    /// The panel name the Window menu uses; `Markers` is only in the panel catalog, and the two
    /// panels in neither (`History`, `ScriptUI Panel`) keep their English names.
    fn logical_label(panel: PanelKind) -> String {
        let title = panel.title();
        let listed = crate::i18n::command_label("window.panel", title);
        if listed == title { crate::i18n::tr(title).to_string() } else { listed.to_string() }
    }

    /// Read the panels' tab names, swatches and the Timeline's comps into the dock's spec model,
    /// moving its revision only when one changed.
    fn refresh_dock_specs(&mut self) {
        let titles = self.tab_titles();
        let swatches = self.tab_swatches();
        let docs = self.tab_docs();
        let rows: Vec<SpecRow> = self
            .ui
            .dock
            .panels()
            .into_iter()
            .map(|panel| SpecRow {
                panel: panel.id(),
                label: titles.iter().find(|(p, _)| *p == panel).map_or_else(|| Self::logical_label(panel), |(_, t)| t.clone()),
                swatch: swatches.iter().find(|(p, _)| *p == panel).map(|(_, swatch)| swatch.map(rgba)),
                documents: if panel == PanelKind::Timeline { docs.clone() } else { Vec::new() },
            })
            .collect();
        if self.live_dock.specs.rows != rows {
            self.live_dock.specs.rows = rows;
        }
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
    pub(crate) fn focus_panel(&mut self, ctx: &egui::Context, p: PanelKind) {
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

    /// Draw the Home screen or workspace in `body`. craft-ui's `DockArea` owns the dock tree,
    /// group chrome, tab dragging and floating native windows; this adapter keeps the panels'
    /// specs, draws their bodies and headers, and applies the returned actions and events.
    pub(crate) fn dock_area(&mut self, ui: &mut egui::Ui, body: Rect) {
        let t = self.tokens;
        let ctx = ui.ctx().clone();
        if self.ui.start_screen {
            // Home covers the whole workspace, as in After Effects (the panels wait behind it).
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(body).id_salt("home"));
            child.set_clip_rect(body.intersect(ui.clip_rect()));
            panels::home::show(self, &mut child, body);
            return;
        }
        self.refresh_dock_specs();
        let shown = self.live_dock.shown(&self.ui.dock);
        // The specs leave the app while its panels borrow it, and come back with their revision.
        let specs = std::mem::take(&mut self.live_dock.specs);
        let maximized = self.ui.maximized.filter(|m| self.ui.dock.is_docked(*m));
        let locked = self.ui.dock_locked;
        let raise = self.pending_panel_raise.take();
        let ime = NativeIme::new(&shown.layout().floating, &self.live_dock.windows, &self.unpublished_viewports);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(body).id_salt("effectcraft-dock-area"));
        child.set_clip_rect(body.intersect(ui.clip_rect()));
        let mut panels = EffectPanels { app: &mut *self, tokens: t, locks: Vec::new(), menu: None, bodies: Vec::new(), ime };
        let output = {
            let mut area = DockArea::new("Panels", Bind::given(Arc::clone(&shown)), &specs, &mut panels, dock_config());
            if let Some(panel) = maximized {
                area = area.maximized(panel);
            }
            if locked {
                area = area.locked();
            }
            if let Some(panel) = raise {
                area = area.raise(panel);
            }
            area.show(&mut child)
        };
        let EffectPanels { locks, menu, bodies, ime, .. } = panels;
        self.live_dock.specs = specs;
        let DockAreaOutput { actions, requested, events, focused, pressed, picked, error, float_space, windows, window_files, sources, .. } = output;
        self.dock_rects = bodies;
        self.unpublished_viewports = ime.unpublished();
        // A window that closed or docked takes its composition with it.
        let previous = std::mem::replace(&mut self.native_viewports, ime.begun());
        let gone: Vec<_> = previous.into_iter().filter(|v| !self.native_viewports.contains(v)).collect();
        for viewport in gone {
            panels::viewer_text::end_viewport(self, &ctx, viewport);
        }
        if float_space.is_some() {
            self.live_dock.float_space = float_space;
        }
        self.forward_window_input(&ctx, &windows, window_files);
        self.live_dock.windows = windows;
        // A press in a panel's body, a tab picked, or key focus moving to a group focuses that panel.
        let moved = focused != self.live_dock.focused;
        self.live_dock.focused = focused;
        if let Some(panel) = pressed.or(picked).or(focused.filter(|_| moved)) {
            self.focus_panel(&ctx, panel);
        }
        if let Some(error) = error {
            self.ui.status = error.error.to_string();
        }
        for event in events {
            match event {
                DockEvent::ToggleMaximize { panel, .. } => self.toggle_maximize(panel),
                // The Timeline's comp tabs: show that comp / close its Timeline.
                DockEvent::DocumentPicked { panel: PanelKind::Timeline, document, .. } => {
                    self.focus_panel(&ctx, PanelKind::Timeline);
                    if self.session.state.active_comp.map(|c| c.0) != Some(document)
                        && let Err(error) = crate::menus::invoke(self, &ctx, "comp.open", json!({"comp": document}))
                    {
                        self.ui.status = error;
                    }
                }
                DockEvent::DocumentClosed { panel: PanelKind::Timeline, document, .. } => {
                    if let Err(error) = crate::menus::invoke(self, &ctx, "comp.close", json!({"comp": document})) {
                        self.ui.status = error;
                    }
                }
                _ => {}
            }
        }
        // A layout a panel changed during the pass takes priority over the gestures of the one shown.
        if self.ui.dock == *shown {
            for action in actions.into_iter().chain(requested) {
                if let Err(error) = self.dock_command(&ctx, action) {
                    self.ui.status = error;
                }
            }
        } else {
            craft_ui::runtime::drag::cancel(&ctx, craft_behavior::CancelReason::HostChanged);
        }
        for p in locks {
            if p == PanelKind::EffectControls {
                // Locks on the layer the panel shows (nothing to lock without one).
                let on = self.ui.effect_controls_lock.is_none();
                if panels::effect_controls::set_lock(self, on) != on {
                    self.ui.status = crate::i18n::tr("Select a layer to lock Effect Controls").to_string();
                }
            } else if !self.ui.locked_tabs.remove(&p.id()) {
                self.ui.locked_tabs.insert(p.id().to_string());
            }
        }
        if let Some(open) = menu {
            ctx.data_mut(|d| d.insert_temp::<panels::OpenPanelMenu>(egui::Id::unique("panel-menu"), open));
            // the owner window draws it on its next pass
            ctx.request_repaint();
        }
        drop_menu_of_closed_window(&ctx, &self.native_viewports);
        self.register_dock_hits(&sources);
        panels::panel_menu_popup(self, ui);
    }

    /// Keys and dropped files that reach a floating panel's own OS window go to the main
    /// window's next frame ([`crate::EffectcraftApp::raw_input_hook`]), so its shortcuts and
    /// file import handle them. Text is not forwarded, and no key is while a text field has the
    /// keyboard.
    fn forward_window_input(&mut self, ctx: &egui::Context, windows: &[(PanelKind, egui::ViewportId)], files: Vec<WindowFiles<PanelKind>>) {
        let typing = crate::native_menu::text_field_window(ctx).is_some();
        for (_, viewport) in windows {
            let events = ctx.input_for(*viewport, |i| i.events.clone());
            self.forwarded.events.extend(events.into_iter().filter(|event| !typing && !matches!(event, egui::Event::Text(_) | egui::Event::Ime(_))));
        }
        if handed_over_to_the_timeline(files.iter().map(|drop| (drop.panel, !drop.files.is_empty()))) {
            self.forwarded.dropped_on_timeline = true;
        }
        #[cfg(not(target_arch = "wasm32"))]
        self.forwarded.files.extend(files.into_iter().flat_map(|drop| drop.files).map(|file| Arc::new(WindowFile(file)) as egui::DroppedFileHandle));
        #[cfg(target_arch = "wasm32")]
        let _ = files;
        if !self.forwarded.events.is_empty() || !self.forwarded.files.is_empty() {
            ctx.request_repaint();
        }
    }

    /// Automation ids for the dock's gesture sources: `panel.tab.<Panel>`, `panel.tab.<Panel>.<document>`
    /// (the Timeline's comp tabs), `panel.drag.<Panel>` (the group's drag bar), `panel.float.<n>.move`
    /// and `.resize` (bottom-right corner) and `.resize.<Edge>` for floating window `n`,
    /// `dock.gutter.<path>` (`a`/`b` per split level), `dock.gutter.group.<Panel>` under a dock
    /// column's group and `dock.column.<Panel>` at a dock column's inner edge.
    fn register_dock_hits(&mut self, sources: &[DockSource<PanelKind>]) {
        let window = |panel: &PanelKind| self.ui.dock.layout().floating.iter().position(|window| window.contains(panel));
        // Tabs are labelled as drawn: "Composition Intro", not "Composition".
        let titles = self.tab_titles();
        let title = |panel: &PanelKind| titles.iter().find(|(p, _)| p == panel).map_or_else(|| panel.title().to_string(), |(_, t)| t.clone());
        let mut found = Vec::new();
        for source in sources {
            let entry = match &source.kind {
                SourceKind::Tab(panel) => (format!("panel.tab.{}", panel.id()), title(panel)),
                SourceKind::Document(panel, document) => {
                    let name = self.session.project.item(effectcraft_engine::project::ItemId(*document)).map_or_else(String::new, |item| item.name.clone());
                    (format!("panel.tab.{}.{document}", panel.id()), name)
                }
                SourceKind::GroupBar(panel) => (format!("panel.drag.{}", panel.id()), "Move panel group".into()),
                SourceKind::Overflow(panel) => (format!("panel.overflow.{}", panel.id()), "More panels".into()),
                SourceKind::WindowGrip(panel) => match window(panel) {
                    Some(index) => (format!("panel.float.{index}.move"), "Move floating panel".into()),
                    None => continue,
                },
                SourceKind::WindowResize(panel, edge) => match window(panel) {
                    Some(index) if *edge == ResizeEdge::BottomRight => (format!("panel.float.{index}.resize"), "Resize floating panel".into()),
                    Some(index) => (format!("panel.float.{index}.resize.{edge:?}"), "Resize floating panel".into()),
                    None => continue,
                },
                SourceKind::Divider(DividerKind::Split(path)) => (format!("dock.gutter.{}", path_id(path)), "gutter".into()),
                SourceKind::Divider(DividerKind::Groups(panel)) => (format!("dock.gutter.group.{}", panel.id()), "gutter".into()),
                SourceKind::Divider(DividerKind::ColumnEdge(panel)) => (format!("dock.column.{}", panel.id()), "gutter".into()),
                _ => continue,
            };
            found.push((entry, source.rect, source.id));
        }
        // The node id lets keyboard focus on a divider or grip map to its element.
        for ((id, label), rect, node) in found {
            self.auto.add(&id, rect, &label);
            if let Some(element) = self.auto.elements.last_mut() {
                element.response_id = Some(node);
            }
        }
    }
}

/// A split path as `a`/`b` per level (first/second child).
fn path_id(path: &[bool]) -> String {
    path.iter().map(|second| if *second { 'b' } else { 'a' }).collect()
}

/// After Effects' window is all frames: drops on all four window edges, Ctrl/Cmd-drag undocks,
/// a release over no target floats, a floating window's title bar only moves it, and the tabs
/// close with their ×, a middle click, or the group menu's Close Other Panels in Group.
fn dock_config() -> DockConfig {
    DockConfig::default()
        .side_edge_zones(true)
        .new_dock_zones(false)
        .maximize_item(true)
        .close_others_item(true)
        .float_on_empty_drop(true)
        .title_bar_docks(false)
        .wheel_selects_tabs(true)
        .tab_close_buttons(true)
        .native_windows(true)
}

/// A colour as the spec model keeps it.
fn rgba(color: egui::Color32) -> [u8; 4] {
    color.to_srgba_unmultiplied()
}

/// A kept colour as a tab swatch.
fn srgba([r, g, b, a]: [u8; 4]) -> craft_theme::Srgba {
    craft_theme::Srgba::from(egui::Color32::from_rgba_unmultiplied(r, g, b, a))
}

/// What the dock area keeps between frames: the workspace it shows, the panels' specs, and what
/// its last pass reported that the next one or a command needs.
#[derive(Default)]
pub(crate) struct LiveDock {
    shown: Option<Arc<Workspace>>,
    specs: Tracked<DockSpecs>,
    /// Where a command floats a panel without a rect.
    float_space: Option<FloatSpace<PanelKind>>,
    /// The OS windows the floating windows showed in, named by their first panel.
    windows: Vec<(PanelKind, egui::ViewportId)>,
    /// The panel whose group held key focus.
    focused: Option<PanelKind>,
}

impl LiveDock {
    /// The `Arc` the dock shows for `workspace`: the last one while the arrangement is the same,
    /// so an idle pass keeps the workspace's revision and the dock reads no spec.
    fn shown(&mut self, workspace: &Workspace) -> Arc<Workspace> {
        if let Some(shown) = self.shown.as_ref().filter(|shown| ***shown == *workspace) {
            return Arc::clone(shown);
        }
        let shown = Arc::new(workspace.clone());
        self.shown = Some(Arc::clone(&shown));
        shown
    }
}

/// What the dock reads of the open panels: tab names, label swatches and the Timeline's comps.
#[derive(Clone, Debug, Default, PartialEq, PlainData)]
struct DockSpecs {
    rows: Vec<SpecRow>,
}

/// One open panel's spec, by panel id.
#[derive(Clone, Debug, PartialEq, PlainData)]
struct SpecRow {
    panel: String,
    label: String,
    /// The label swatch before the name; `Some(None)` is the theme's empty swatch.
    swatch: Option<Option<[u8; 4]>>,
    documents: Vec<DocRow>,
}

/// One of the Timeline's comp tabs.
#[derive(Clone, Debug, PartialEq, PlainData)]
struct DocRow {
    id: u64,
    label: String,
    swatch: Option<[u8; 4]>,
    /// The comp the Timeline shows.
    shown: bool,
}

impl PanelSpecs<PanelKind> for DockSpecs {
    fn spec(&self, panel: &PanelKind) -> PanelSpec {
        let id = panel.id();
        let Some(row) = self.rows.iter().find(|row| row.panel == id) else { return PanelSpec::new(panel.title()).host_body() };
        // Every panel body is EffectCraft's own egui code: the dock runs it with the window's style.
        let mut spec = PanelSpec::new(row.label.clone()).host_body();
        if let Some(swatch) = row.swatch {
            spec = spec.swatch(swatch.map(srgba));
        }
        if !row.documents.is_empty() {
            let documents = row
                .documents
                .iter()
                .map(|doc| PanelDocument::new(doc.id, doc.label.clone()).swatch(doc.swatch.map(srgba)).closable(true).shown(doc.shown))
                .collect();
            spec = spec.documents(documents);
        }
        spec
    }

    fn permissions(&self, panel: &PanelKind) -> Permissions {
        dock::permissions(panel)
    }
}

/// Panels with a lock: the Composition viewers and the Timeline keep their comp while locked,
/// Effect Controls its layer.
fn has_lock(panel: PanelKind) -> bool {
    matches!(panel, PanelKind::Composition | PanelKind::Viewer(_) | PanelKind::Timeline | PanelKind::EffectControls)
}

/// Whether `panel`'s tab lock is on. Effect Controls keeps the layer it is locked to
/// (`UiState::effect_controls_lock`), the others are in `UiState::locked_tabs`.
fn is_locked(app: &EffectcraftApp, panel: PanelKind) -> bool {
    match panel {
        PanelKind::EffectControls => app.ui.effect_controls_lock.is_some(),
        _ => app.ui.locked_tabs.contains(&panel.id()),
    }
}

/// EffectCraft's side of the dock area for one frame.
struct EffectPanels<'a> {
    app: &'a mut EffectcraftApp,
    tokens: Tokens,
    /// Viewer locks clicked.
    locks: Vec<PanelKind>,
    /// The ≡ button clicked: its panel, where its menu opens and the viewport it was clicked in.
    menu: Option<panels::OpenPanelMenu>,
    /// Where each panel body was drawn, in drawing order (floating windows last).
    bodies: Vec<(PanelKind, Rect)>,
    /// Where each native window's IME descriptor is set up and published.
    ime: NativeIme,
}

impl DockPanels<PanelKind> for EffectPanels<'_> {
    fn body(&mut self, panel: &PanelKind, ui: &mut Ui, _: &mut BodyCx<'_>) {
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
        // The ≡ menu of a panel in its own OS window is drawn by that window's pass, after its
        // last body, at that window's coordinates.
        if native && self.ime.menu_turn(viewport) {
            panels::panel_menu_area(self.app, ui);
        }
        // The body fills its frame; allocating it lets a floated panel keep its full size.
        ui.allocate_rect(rect, Sense::hover());
        // Painted after the body so child backgrounds cannot hide the focus outline.
        if self.app.ui.focused == *panel {
            ui.painter().rect_stroke(rect, 0.0, Stroke::new(1.0, t.focus), StrokeKind::Inside);
        }
    }

    /// The viewer lock and the ≡ panel menu of the shown panel, before the tabs.
    fn group_header(&mut self, panels: &[PanelKind], active: usize, ui: &mut Ui, _: &mut BodyCx<'_>) {
        let Some(active) = panels.get(active).copied() else { return };
        let t = self.tokens;
        let height = ui.available_height().max(18.0);
        if has_lock(active) {
            let (slot, _) = ui.allocate_exact_size(vec2(LOCK_W, height), Sense::hover());
            let lock = Rect::from_center_size(slot.center(), vec2(14.0, 14.0));
            let locked = is_locked(self.app, active);
            if dock::lock_button(ui, lock, active, locked, &t, &mut self.app.auto) {
                self.locks.push(active);
            }
        }
        let (slot, _) = ui.allocate_exact_size(vec2(MENU_W, height), Sense::hover());
        let rect = Rect::from_center_size(slot.center(), vec2(18.0, 18.0));
        let response = ui.interact(rect, egui::Id::unique(("panel-menu-button", active.id())), Sense::click());
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, response.enabled(), "panel menu"));
        self.app.auto.add_response(&format!("panel.menu.{}", active.id()), &response, "panel menu");
        let color = if response.hovered() { t.tab_text_active } else { t.tab_text };
        icons::paint(ui.painter(), Rect::from_center_size(rect.center(), vec2(16.0, 16.0)), Icon::Hamburger, color);
        crate::widgets::focus_outline(ui, &response, &t);
        if response.clicked() {
            self.app.ui.focused = active;
            self.menu = Some((active, rect.left_bottom(), ui.ctx().viewport_id()));
        }
    }
}

/// A file dropped on a floating panel's OS window, handed to the main window's file import.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug)]
struct WindowFile(craft_ui::runtime::drag::OsFile);

#[cfg(not(target_arch = "wasm32"))]
impl egui::DroppedFile for WindowFile {
    fn path(&self) -> &std::path::Path {
        self.0.path.as_deref().unwrap_or_else(|| std::path::Path::new(&self.0.name))
    }

    fn bytes(&self) -> Result<Vec<u8>, String> {
        match (&self.0.bytes, &self.0.path) {
            (Some(bytes), _) => Ok(bytes.to_vec()),
            (None, Some(path)) => std::fs::read(path).map_err(|error| error.to_string()),
            (None, None) => Err(format!("{}: no path or contents", self.0.name)),
        }
    }
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
    /// Windows that drew their panel menu this pass.
    menu_drawn: Vec<egui::ViewportId>,
    /// Windows that published this pass.
    published: Vec<egui::ViewportId>,
}

impl NativeIme {
    /// `viewports` are the OS windows the dock reported last pass, each named by a panel of its
    /// window. A window not among them yet (it opens this pass) publishes after every body.
    /// `unpublished` are the windows that drew last pass but never published: the layout's count
    /// was too high for them, so they publish after every body this pass.
    pub(crate) fn new(floating: &[Floating<PanelKind>], viewports: &[(PanelKind, egui::ViewportId)], unpublished: &[egui::ViewportId]) -> Self {
        let expected = floating
            .iter()
            .filter_map(|window| {
                let viewport = viewports.iter().find(|(panel, _)| window.contains(panel)).map(|(_, viewport)| *viewport)?;
                let count = if unpublished.contains(&viewport) { 0 } else { window_bodies(window) };
                Some((viewport, count))
            })
            .collect();
        Self { expected, drawn: Vec::new(), menu_drawn: Vec::new(), published: Vec::new() }
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

    /// Whether the body of `viewport` that just drew is the one that draws the window's panel
    /// menu: after the window's last body, once per pass.
    pub(crate) fn menu_turn(&mut self, viewport: egui::ViewportId) -> bool {
        let drawn = self.drawn.iter().find(|(v, _)| *v == viewport).map_or(0, |(_, n)| *n);
        let expected = self.expected.iter().find(|(v, _)| *v == viewport).map_or(0, |(_, n)| *n);
        let turn = drawn >= expected && !self.menu_drawn.contains(&viewport);
        if turn {
            self.menu_drawn.push(viewport);
        }
        turn
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

/// A panel menu left open by a panel window that has since closed has nowhere to draw: drop it.
/// `windows` are the panel windows that drew this pass.
pub(crate) fn drop_menu_of_closed_window(ctx: &egui::Context, windows: &[egui::ViewportId]) {
    let id = egui::Id::unique("panel-menu");
    ctx.data_mut(|d| {
        if d.get_temp::<panels::OpenPanelMenu>(id).is_some_and(|(_, _, owner)| owner != egui::ViewportId::ROOT && !windows.contains(&owner)) {
            d.remove::<panels::OpenPanelMenu>(id);
        }
    });
}

/// The panel bodies the dock draws in a floating window: the shown panel of each group that is
/// not minimized. A window collapsed to icons shows none.
fn window_bodies(window: &Floating<PanelKind>) -> usize {
    if window.column.iconic {
        return 0;
    }
    window.groups().iter().filter(|group| !group.minimized && group.active_panel().is_some()).count()
}

/// Whether a handed-over *drop* named the Timeline. Only a drop with files counts, so a window
/// that only saw a file pass over it never sends the next drop somewhere else. The panel comes
/// from the window, which the main window's pointer can never reach (#2661).
fn handed_over_to_the_timeline(drops: impl Iterator<Item = (PanelKind, bool)>) -> bool {
    drops.into_iter().any(|(panel, has_files)| has_files && matches!(panel, PanelKind::Timeline))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A drop handed over by a floating window says which panel it landed on; only the Timeline
    /// turns it into a layer instead of a project item. A drop without files must not count, or
    /// its flag would outlive the drag (#2661).
    #[test]
    fn only_a_timeline_drop_is_handed_over_to_the_timeline() {
        let cases = [
            (vec![(PanelKind::Timeline, true)], true),
            (vec![(PanelKind::Project, true)], false),
            (vec![(PanelKind::Timeline, false)], false),
            (vec![(PanelKind::Project, true), (PanelKind::Timeline, true)], true),
            (vec![], false),
        ];
        for (windows, want) in cases {
            assert_eq!(handed_over_to_the_timeline(windows.into_iter()), want, "{want}");
        }
    }
}
