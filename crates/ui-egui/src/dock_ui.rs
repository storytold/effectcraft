//! The dock area: lays out the docked tree (or the maximized panel), draws each group's chrome
//! and body, floating (undocked) panel groups over it, and tab drag-and-drop: drag a tab onto a
//! group to tab it there (center) or split it off to a side (After Effects' drop zones, with a
//! highlight of where it will land); hold Cmd/Ctrl when releasing, or drop outside every group,
//! to float it. `` ` `` maximizes the panel under the pointer. A floating group can also move
//! out of the main window into its own native window ([`EffectcraftApp::set_panel_window`]),
//! which can sit on another monitor.

use egui::{Color32, Rect, Sense, Stroke, StrokeKind, pos2, vec2};

use crate::dock::{self, DockAction, DockNode, Group, Layout, PanelKind, PanelWindow};
use crate::{EffectcraftApp, panels};
use craft_ui::docking::{Action, DropTarget, resolve_drop};

fn drag_id() -> egui::Id {
    egui::Id::unique("dock-tab-drag")
}

impl EffectcraftApp {
    pub(crate) fn dock_command(&mut self, ctx: &egui::Context, action: Action<PanelKind>) -> Result<(), String> {
        let value = serde_json::to_value(action).map_err(|error| error.to_string())?;
        crate::menus::invoke(self, ctx, "window.dockAction", serde_json::json!({"action": value})).map(|_| ())
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
    fn comp_swatch(&self, cid: effectcraft_engine::project::ItemId) -> Color32 {
        match self.session.project.item(cid).map(|i| i.label) {
            Some(l) if l != effectcraft_engine::color::Label::None => self.tokens.label(l),
            _ => self.tokens.text_faint,
        }
    }

    /// The Timeline's tabs: one per open comp, as in After Effects (opening a precomp adds a tab
    /// next to its parent's instead of replacing it).
    fn tab_docs(&self) -> Vec<(PanelKind, Vec<dock::DocTab>)> {
        let st = &self.session.state;
        let locked = self.ui.locked_tabs.contains(&PanelKind::Timeline.id());
        let docs = st
            .open_comps
            .iter()
            .filter_map(|id| {
                let it = self.session.project.item(*id).filter(|i| i.as_comp().is_some())?;
                let deco = dock::TabDeco { swatch: self.comp_swatch(*id), locked, viewer: true };
                Some(dock::DocTab { id: id.0, title: it.name.clone(), deco: Some(deco), active: st.active_comp == Some(*id) })
            })
            .collect();
        vec![(PanelKind::Timeline, docs)]
    }

    /// The Composition and Timeline tabs' close button, label-colour swatch and viewer lock.
    fn tab_decos(&self) -> Vec<(PanelKind, dock::TabDeco)> {
        let Some(cid) = self.session.active_comp_id() else { return vec![] };
        let swatch = self.comp_swatch(cid);
        let mut v: Vec<(PanelKind, dock::TabDeco)> =
            vec![(PanelKind::Timeline, dock::TabDeco { swatch, locked: self.ui.locked_tabs.contains(&PanelKind::Timeline.id()), viewer: true })];
        // Every Composition viewer: its comp's swatch and its own lock.
        for id in std::iter::once(0).chain(self.ui.viewers.keys().copied().filter(|v| *v != 0)) {
            let p = panels::viewers::panel(id);
            let swatch = panels::viewers::comp_of(self, id).map_or(self.tokens.text_faint, |c| self.comp_swatch(c));
            v.push((p, dock::TabDeco { swatch, locked: self.ui.locked_tabs.contains(&p.id()), viewer: true }));
        }
        // Effect Controls carries the selected layer's label colour.
        if let Some(l) = self.session.active_comp().and_then(|c| self.session.state.selected_layers.first().and_then(|id| c.layer(*id)))
            && l.label != effectcraft_engine::color::Label::None
        {
            v.push((PanelKind::EffectControls, dock::TabDeco { swatch: self.tokens.label(l.label), locked: false, viewer: false }));
        }
        v
    }

    /// Settings ▸ Appearance ▸ Use Label Color for Related Tabs: the Composition and Timeline
    /// tabs carry their comp's label colour, Effect Controls and Properties the layer's.
    fn label_tab_marks(&self, ui: &egui::Ui) {
        if !self.session.prefs.appearance.use_label_color_for_tabs {
            return;
        }
        let Some(cid) = self.session.active_comp_id() else { return };
        let comp_label = self.session.project.item(cid).map(|i| i.label);
        let layer_label = self.session.active_comp().and_then(|c| self.session.state.selected_layers.first().and_then(|l| c.layer(*l))).map(|l| l.label);
        // (the Composition, Timeline and Effect Controls tabs carry their swatch already)
        let _ = comp_label;
        for (p, label) in [(PanelKind::Properties, layer_label)] {
            let (Some(label), Some(e)) = (label, self.auto.find(&format!("panel.tab.{}", p.id()))) else { continue };
            if label == effectcraft_engine::color::Label::None {
                continue;
            }
            let r = Rect::from_min_size(pos2(e.rect[0] + 2.0, e.rect[1] + 9.0), vec2(4.0, (e.rect[3] - 16.0).max(6.0)));
            ui.painter().rect_filled(r, 1.0, self.tokens.label(label));
        }
    }

    /// Edit the full layout (docked tree + floating groups) with a [`Layout`] operation.
    pub fn edit_layout(&mut self, f: impl FnOnce(&mut Layout) -> bool) -> bool {
        let Ok(shared) = crate::dock_shared::encode(&self.ui.dock, &self.ui.floating) else { return false };
        if shared.validate().is_err() {
            return false;
        }
        let mut l = Layout { root: self.ui.dock.clone(), floating: self.ui.floating.clone() };
        if !f(&mut l) {
            return false;
        }
        self.ui.dock = l.root;
        self.ui.floating = l.floating;
        if self.ui.maximized.is_some_and(|m| !self.ui.dock.contains(m)) {
            self.ui.maximized = None;
        }
        true
    }

    /// Close a panel wherever it is (docked or floating).
    pub fn close_panel(&mut self, p: PanelKind) {
        // Commit the layout first: rejected closes must not release viewer or script state.
        if !self.edit_layout(|layout| layout.apply(Action::Close { panel: p }).is_ok()) {
            return;
        }
        // Closing a ScriptUI panel closes its script window (its script's onClose runs).
        if let PanelKind::ScriptPanel(id) = p
            && self.session.script_ui.window(id).is_some()
            && let Err(e) = self.session.execute("scriptui.close", serde_json::json!({"window": id}))
        {
            self.ui.status = e.to_string();
        }
        panels::viewers::on_close(self, p);
        if self.ui.maximized == Some(p) {
            self.ui.maximized = None;
        }
    }

    /// Toggle the maximized panel (`p`, or none).
    pub fn toggle_maximize(&mut self, p: PanelKind) {
        self.ui.maximized = if self.ui.maximized == Some(p) || !self.ui.dock.contains(p) { None } else { Some(p) };
    }

    /// The docked panel whose group is under `pos` (from the last frame), else the focused one.
    pub fn panel_at(&self, pos: Option<egui::Pos2>) -> PanelKind {
        pos.and_then(|p| self.dock_rects.iter().rev().find(|(_, r)| r.contains(p)).map(|(k, _)| *k)).unwrap_or(self.ui.focused)
    }

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

    /// Apply one action after the panel bodies have drawn. A panel-window pass runs this at
    /// its own pass end so the final IME descriptor sees the post-action state.
    fn handle_dock_action(&mut self, ctx: &egui::Context, action: DockAction) {
        match action {
            DockAction::Activate(p) => {
                let _ = self.dock_command(ctx, Action::Activate { panel: p });
            }
            DockAction::ToggleStacked(p) => {
                let _ = self.dock_command(ctx, Action::SetStackOpen { panel: p, open: !self.ui.dock.is_visible(p) });
            }
            DockAction::Focus(p) => {
                self.ui.focused = p;
                panels::viewer_text::panel_clicked(self, ctx, p);
                // A click in (or on the tab of) another Composition viewer makes it active.
                if let Some(id) = panels::viewers::id_of(p) {
                    panels::viewers::activate(self, ctx, id);
                }
            }
            DockAction::Close(p) => {
                let _ = self.dock_command(ctx, Action::Close { panel: p });
            }
            DockAction::PanelMenu(p, pos) => {
                ctx.data_mut(|d| d.insert_temp(egui::Id::unique("panel-menu"), (p, pos)));
            }
            DockAction::BeginDrag(p) => ctx.data_mut(|d| {
                d.insert_temp(drag_id(), p);
            }),
            DockAction::ToggleLock(p) => {
                if !self.ui.locked_tabs.remove(&p.id()) {
                    self.ui.locked_tabs.insert(p.id().to_string());
                }
            }
            // The Timeline's comp tabs: show that comp / close its Timeline.
            DockAction::ActivateDoc(p, id) => {
                let _ = self.dock_command(ctx, Action::Activate { panel: p });
                if self.session.state.active_comp.map(|c| c.0) != Some(id) {
                    let _ = crate::menus::invoke(self, ctx, "comp.open", serde_json::json!({"comp": id}));
                }
            }
            DockAction::CloseDoc(_, id) => {
                let _ = crate::menus::invoke(self, ctx, "comp.close", serde_json::json!({"comp": id}));
            }
        }
    }

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
        // The maximized panel fills the area; the tree is kept as it is.
        let maximized = self.ui.maximized.filter(|m| self.ui.dock.contains(*m));
        let mut dock = match maximized {
            Some(m) => DockNode::Tabs { panels: vec![m], active: 0 },
            None => std::mem::replace(&mut self.ui.dock, DockNode::Tabs { panels: vec![], active: 0 }),
        };
        let mut groups = Vec::new();
        let mut layout_actions = Vec::new();
        dock::layout_with_actions(ui, &mut dock, body, &t, "", &mut groups, &mut self.auto, &mut layout_actions);
        let mut actions = Vec::new();
        let titles = self.tab_titles();
        let decos = self.tab_decos();
        let docs = self.tab_docs();
        let title = |p: PanelKind| {
            titles.iter().find(|(k, _)| *k == p).map(|(_, s)| s.clone()).unwrap_or_else(|| crate::i18n::command_label("window.panel", p.title()).to_string())
        };
        let info = dock::TabInfo { title: &title, decos: &decos, docs: &docs };
        // Where each group's tabs are (tab drags show where a tab will land between them).
        let mut tab_rects: Vec<(String, Vec<(PanelKind, Rect)>)> = vec![];
        for g in &groups {
            let c = dock::draw_group_chrome(ui, g, &t, &mut self.auto, &info);
            actions.extend(c.actions);
            tab_rects.push((g.path.clone(), c.tabs));
        }
        self.label_tab_marks(ui);
        if maximized.is_none() {
            self.ui.dock = dock;
        }
        self.dock_rects = groups.iter().filter_map(|g| Some((*g.panels.get(g.active)?, g.rect))).collect();
        for g in &groups {
            let Some(p) = g.panels.get(g.active).copied() else { continue };
            if g.content.height() < 2.0 {
                continue; // a collapsed stacked panel: header only
            }
            self.panel_body(ui, p, g.content);
        }
        for g in &groups {
            if g.panels.get(g.active) == Some(&self.ui.focused) {
                ui.painter().rect_stroke(g.rect, 0.0, Stroke::new(1.0, t.focus), StrokeKind::Inside);
            }
        }
        // Floating groups over the dock.
        let mut float_groups: Vec<Group> = vec![];
        let layers = ctx.memory(|memory| memory.layer_ids().collect::<Vec<_>>());
        let mut floating_order: Vec<_> = (0..self.ui.floating.len()).collect();
        floating_order.sort_by_key(|index| {
            self.ui
                .floating
                .get(*index)
                .and_then(|group| group.panels.first())
                .and_then(|panel| {
                    let id = egui::Id::unique(("floating-panel", panel.id()));
                    layers.iter().position(|layer| layer.id == id)
                })
                .unwrap_or(usize::MAX)
        });
        // A native window is kept only for a panel that still floats. A native viewport that
        // goes away releases its IME context: its composition must not reappear elsewhere.
        let ui_state = &mut self.ui;
        let gone: Vec<egui::ViewportId> = ui_state
            .panel_windows
            .iter()
            .filter(|w| !ui_state.floating.iter().any(|group| group.panels.contains(&w.panel)))
            .map(|w| panel_window_id(w.panel))
            .collect();
        ui_state.panel_windows.retain(|w| ui_state.floating.iter().any(|group| group.panels.contains(&w.panel)));
        for viewport in gone {
            panels::viewer_text::end_viewport(self, &ctx, viewport);
        }
        for i in floating_order {
            // Groups in their own native window take no tab drops from the main window.
            if let Some(window) = self.ui.floating.get(i).and_then(|group| self.panel_window_of(&group.panels)) {
                self.panel_window(&ctx, i, window, &info);
                continue;
            }
            if let Some((g, tabs)) = self.floating_window(&ctx, i, &info, &mut actions) {
                tab_rects.push((g.path.clone(), tabs));
                float_groups.push(g);
            }
        }
        for action in layout_actions {
            let _ = self.dock_command(&ctx, action);
        }
        for a in actions {
            self.handle_dock_action(&ctx, a);
        }
        self.tab_drag(&ctx, &groups, &float_groups, &tab_rects);
        panels::panel_menu_popup(self, ui);
    }

    /// One floating group: a window with a tab strip (drag the empty strip to move it, the
    /// corner to resize it) and the active panel's body.
    fn floating_window(
        &mut self,
        ctx: &egui::Context,
        i: usize,
        info: &dock::TabInfo,
        actions: &mut Vec<DockAction>,
    ) -> Option<(Group, Vec<(PanelKind, Rect)>)> {
        let t = self.tokens;
        let f = self.ui.floating.get(i)?.clone();
        let screen = ctx.content_rect();
        let mut rect = Rect::from_min_size(pos2(f.rect[0], f.rect[1]), vec2(f.rect[2].max(160.0), f.rect[3].max(100.0)));
        // Keep the strip reachable.
        rect = rect.translate(vec2(
            (screen.min.x - rect.min.x).max(0.0) + (screen.max.x - 40.0 - rect.min.x).min(0.0),
            (screen.min.y - rect.min.y).max(0.0) + (screen.max.y - 30.0 - rect.min.y).min(0.0),
        ));
        let id = egui::Id::unique(("floating-panel", f.panels.first().map(|p| p.id()).unwrap_or_default()));
        let mut group = None;
        // Not constrained to the window: the group is drawn where its rect says (the clamp above
        // keeps its strip reachable), so it follows the pointer past the window's edges.
        egui::Area::new(id).order(egui::Order::Middle).fixed_pos(rect.min).constrain(false).interactable(true).show(ctx, |ui| {
            let (r, _) = ui.allocate_exact_size(rect.size(), Sense::hover());
            ui.painter().rect_filled(r.expand(1.0), t.radius, Color32::from_black_alpha(90));
            let strip = Rect::from_min_size(r.min, vec2(r.width(), t.tab_h));
            // Move by the strip (tabs, registered later, take precedence over it).
            // (`id.with("move")` is the id of the `Area`'s own background widget.)
            let mv = ui.interact(strip, id.with("strip"), Sense::drag());
            self.auto.add(&format!("panel.float.{i}.move"), strip, "Move floating panel");
            let mut nr = r;
            if mv.dragged() {
                nr = nr.translate(mv.drag_delta());
            }
            let g = Group {
                path: format!("f{i}"),
                rect: r,
                content: Rect::from_min_max(pos2(r.min.x, r.min.y + t.tab_h), r.max),
                panels: f.panels.clone(),
                active: f.active.min(f.panels.len().saturating_sub(1)),
                stacked: None,
            };
            let c = dock::draw_group_chrome(ui, &g, &t, &mut self.auto, info);
            actions.extend(c.actions);
            if let Some(p) = g.panels.get(g.active).copied() {
                self.panel_body(ui, p, g.content);
            }
            ui.painter().rect_stroke(r, t.radius, Stroke::new(1.0, t.field_border), StrokeKind::Outside);
            if g.panels.get(g.active) == Some(&self.ui.focused) {
                ui.painter().rect_stroke(r, t.radius, Stroke::new(1.0, t.focus), StrokeKind::Inside);
            }
            // Resize from the corner.
            let grip = Rect::from_min_max(r.max - vec2(14.0, 14.0), r.max);
            let rs = ui.interact(grip, id.with("resize"), Sense::click_and_drag());
            rs.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Splitter, rs.enabled(), crate::i18n::tr("Resize floating panel")));
            self.auto.add_response(&format!("panel.float.{i}.resize"), &rs, "Resize floating panel");
            for k in 0..3 {
                let o = 4.0 + k as f32 * 4.0;
                ui.painter().line_segment([pos2(r.max.x - o, r.max.y - 2.0), pos2(r.max.x - 2.0, r.max.y - o)], Stroke::new(1.0, t.text_faint));
            }
            if rs.hovered() || rs.dragged() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeNwSe);
            }
            let delta = floating_resize_delta(ui, &rs);
            if rs.has_focus() {
                ui.painter().rect_stroke(grip, 2.0, Stroke::new(1.5, t.focus), StrokeKind::Inside);
            }
            if delta != egui::Vec2::ZERO {
                nr = Rect::from_min_size(nr.min, (nr.size() + delta).max(vec2(160.0, 100.0)));
            }
            if nr != r
                && let Some(panel) = f.panels.first().copied()
            {
                let _ = self.dock_command(ctx, Action::MoveFloating { panel, rect: [nr.min.x, nr.min.y, nr.width(), nr.height()] });
            }
            group = Some((g, c.tabs));
        });
        group
    }

    /// The native window a floating group (its `panels`) shows in, if any.
    pub(crate) fn panel_window_of(&self, panels: &[PanelKind]) -> Option<PanelWindow> {
        self.ui.panel_windows.iter().find(|w| panels.contains(&w.panel)).copied()
    }

    /// Whether panel `p` shows in its own native window.
    pub fn in_panel_window(&self, p: PanelKind) -> bool {
        self.ui.floating.iter().any(|group| group.panels.contains(&p) && self.panel_window_of(&group.panels).is_some())
    }

    /// The viewport panel `p` is drawn in: its native window's, or the main window's (also when
    /// the platform embeds viewports, as the browser build does).
    pub(crate) fn panel_viewport(&self, ctx: &egui::Context, p: PanelKind) -> egui::ViewportId {
        if ctx.embed_viewports() {
            return egui::ViewportId::ROOT;
        }
        self.ui
            .floating
            .iter()
            .find(|group| group.panels.contains(&p))
            .and_then(|group| self.panel_window_of(&group.panels))
            .map_or(egui::ViewportId::ROOT, |w| panel_window_id(w.panel))
    }

    /// Move panel `p` (and the floating group it is in) into its own native window, which can be
    /// placed on another monitor, or back into the main window (`window` false; it floats there
    /// where it was before). `None` toggles. A docked panel is undocked first. The window opens
    /// over the place the panel had in the main window. Returns whether `p` is now in a window.
    pub fn set_panel_window(&mut self, ctx: &egui::Context, p: PanelKind, window: Option<bool>) -> Result<bool, String> {
        let group_of = |app: &Self| app.ui.floating.iter().find(|group| group.panels.contains(&p)).map(|group| (group.panels.clone(), group.rect));
        let current = self.in_panel_window(p);
        let window = window.unwrap_or(!current);
        if window == current {
            return Ok(current);
        }
        if !window {
            if let Some((panels, _)) = group_of(self) {
                self.ui.panel_windows.retain(|w| !panels.contains(&w.panel));
            }
            return Ok(false);
        }
        if group_of(self).is_none() {
            if !self.ui.dock.contains(p) {
                return Err(format!("{} is not open", p.title()));
            }
            let rect = default_float_rect(ctx.input_for(egui::ViewportId::ROOT, |i| i.content_rect()));
            if !self.edit_layout(|layout| layout.float(p, rect)) {
                return Err(format!("can't undock {}", p.title()));
            }
        }
        let Some((_, [x, y, w, h])) = group_of(self) else { return Err(format!("can't undock {}", p.title())) };
        // Over the panel's place in the main window (its content's position on the desktop).
        let origin = ctx.input_for(egui::ViewportId::ROOT, |i| i.viewport().inner_rect.map(|r| r.min)).unwrap_or(pos2(0.0, 0.0));
        let rect = PanelWindow::sanitized([origin.x + x, origin.y + y, w, h]);
        ctx.data_mut(|d| d.remove::<[f32; 4]>(panel_window_spawn_id(p)));
        self.ui.panel_windows.push(PanelWindow { panel: p, rect });
        if self.ui.maximized == Some(p) {
            self.ui.maximized = None;
        }
        Ok(true)
    }

    /// One floating group in its own native window: the group's tab strip and active panel fill
    /// the window. Closing the window brings the group back into the main window.
    fn panel_window(&mut self, ctx: &egui::Context, i: usize, window: PanelWindow, info: &dock::TabInfo) {
        let t = self.tokens;
        let Some(f) = self.ui.floating.get(i).cloned() else { return };
        let active = f.active.min(f.panels.len().saturating_sub(1));
        let Some(shown) = f.panels.get(active).copied() else { return };
        // The window opens at its saved place; after that the window system owns its position and
        // size (handing them back every frame would fight the user dragging it).
        let spawn_id = panel_window_spawn_id(window.panel);
        let spawn = ctx.data_mut(|d| *d.get_temp_mut_or_insert_with(spawn_id, || PanelWindow::sanitized(window.rect)));
        let builder = egui::ViewportBuilder::default()
            .with_title((info.title)(shown))
            .with_position([spawn[0], spawn[1]])
            .with_inner_size([spawn[2], spawn[3]])
            .with_min_inner_size(dock::PANEL_WINDOW_MIN);
        let mut closed = false;
        let mut placed = None;
        let mut pass_viewport = None;
        let mut actions = Vec::new();
        ctx.show_viewport_immediate(panel_window_id(window.panel), builder, |ui, class| {
            pass_viewport = Some(ui.ctx().viewport_id());
            let native = matches!(class, egui::ViewportClass::Immediate | egui::ViewportClass::Deferred);
            if native {
                panels::viewer_text::begin_ui(ui.ctx());
            }
            egui::CentralPanel::default().frame(egui::Frame::NONE.fill(t.panel_bg)).show(ui, |ui| {
                let r = ui.max_rect();
                let g = Group {
                    path: format!("w{i}"),
                    rect: r,
                    content: Rect::from_min_max(pos2(r.min.x, (r.min.y + t.tab_h).min(r.max.y)), r.max),
                    panels: f.panels.clone(),
                    active,
                    stacked: None,
                };
                let c = dock::draw_group_chrome(ui, &g, &t, &mut self.auto, info);
                // Tabs can't be dragged between native windows yet.
                actions.extend(c.actions.into_iter().filter(|a| !matches!(a, DockAction::BeginDrag(_))));
                self.panel_body(ui, shown, g.content);
                if shown == self.ui.focused {
                    ui.painter().rect_stroke(r, 0.0, Stroke::new(1.0, t.focus), StrokeKind::Inside);
                }
                if native {
                    panels::panel_menu_popup(self, ui);
                }
            });
            // Apply this window's panel/tab actions before publishing its IME descriptor: a
            // click-away or close in the floating panel must own the pass's final output.
            for action in actions.drain(..) {
                self.handle_dock_action(ui.ctx(), action);
            }
            panels::viewer_text::finish_ime(self, ui);
            // An embedded window (no native windows on this platform) reads the main window's
            // input: its close request and place aren't this window's.
            if native {
                closed = ui.input(|i| i.viewport().close_requested());
                placed = ui.input(|i| {
                    let v = i.viewport();
                    let inner = v.inner_rect?;
                    let min = v.outer_rect.map_or(pos2(window.rect[0], window.rect[1]), |outer| outer.min);
                    Some([min.x, min.y, inner.width(), inner.height()])
                });
            }
        });
        if closed {
            self.ui.panel_windows.retain(|w| !f.panels.contains(&w.panel));
            if let Some(viewport) = pass_viewport {
                panels::viewer_text::end_viewport(self, ctx, viewport);
            }
            panels::viewer_text::end_viewport(self, ctx, panel_window_id(window.panel));
            ctx.data_mut(|d| d.remove::<[f32; 4]>(spawn_id));
            return;
        }
        // Remember where the window is, so the workspace reopens it there.
        if let Some(rect) = placed.map(PanelWindow::sanitized)
            && rect != window.rect
            && let Some(w) = self.ui.panel_windows.iter_mut().find(|w| w.panel == window.panel)
        {
            w.rect = rect;
        }
    }

    /// A tab being dragged. Over a tab strip (or the middle of a group) it shows where the tab
    /// will land: an insertion mark between two tabs (a drop there reorders a group's own tabs
    /// too); near a group's edge, the half it will split off. On release it lands there (or
    /// floats with Cmd/Ctrl or outside every group).
    fn tab_drag(&mut self, ctx: &egui::Context, groups: &[Group], floats: &[Group], tab_rects: &[(String, Vec<(PanelKind, Rect)>)]) {
        let Some(p) = ctx.data(|d| d.get_temp::<PanelKind>(drag_id())) else { return };
        let t = self.tokens;
        let canceled =
            ctx.input(|input| input.key_pressed(egui::Key::Escape) || !input.focused || (!input.pointer.primary_down() && !input.pointer.primary_released()));
        if canceled || (!self.ui.dock.contains(p) && !self.ui.floating.iter().any(|group| group.panels.contains(&p))) {
            ctx.data_mut(|data| data.remove::<PanelKind>(drag_id()));
            return;
        }
        let Some(pos) = ctx.pointer_latest_pos() else { return };
        // The shared resolver receives groups in paint order; a floating group occludes the dock.
        let targets: Vec<_> = groups
            .iter()
            .map(|g| (g, false))
            .chain(floats.iter().map(|g| (g, true)))
            .filter_map(|(g, floating)| {
                Some(DropTarget {
                    anchor: *g.panels.get(g.active)?,
                    rect: g.rect,
                    tab_strip: Some(Rect::from_min_max(g.rect.min, pos2(g.rect.max.x, g.content.min.y))),
                    tabs: {
                        let mut tabs = tab_rects.iter().find(|(path, _)| *path == g.path).map(|(_, tabs)| tabs.clone()).unwrap_or_default();
                        for panel in &g.panels {
                            if !tabs.iter().any(|(candidate, _)| candidate == panel) {
                                tabs.push((*panel, Rect::NOTHING));
                            }
                        }
                        tabs
                    },
                    allow_splits: !floating,
                    accepts_tabs: true,
                })
            })
            .collect();
        let over_group = targets.iter().any(|target| target.rect.contains(pos));
        let float_drop = ctx.input(|i| i.modifiers.command);
        let plan = (!float_drop).then(|| resolve_drop(&p, pos, &targets)).flatten();
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("dock-drop")));
        if let Some(proposal) = &plan {
            let preview = proposal.preview.shrink(2.0);
            painter.rect_filled(preview, t.radius, t.focus.gamma_multiply(0.18));
            painter.rect_stroke(preview, t.radius, Stroke::new(2.0, t.focus), StrokeKind::Inside);
            if let Some(mark) = proposal.insertion {
                insertion_mark(&painter, mark.center().x, mark, t.focus);
            }
        }
        // Ghost of the dragged tab.
        let g = painter.layout_no_wrap(crate::i18n::command_label("window.panel", p.title()).to_string(), crate::theme::Tokens::ui(12.0), Color32::WHITE);
        let gr = Rect::from_min_size(pos + vec2(12.0, 10.0), g.size() + vec2(16.0, 8.0));
        painter.rect_filled(gr, 4.0, t.focus.gamma_multiply(0.85));
        painter.galley(gr.min + vec2(8.0, 4.0), g, Color32::WHITE);
        ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
        ctx.request_repaint();
        if ctx.input(|i| i.pointer.primary_released()) {
            ctx.data_mut(|d| d.remove::<PanelKind>(drag_id()));
            let ok = match plan {
                Some(proposal) => self.dock_command(ctx, Action::Move { panel: p, anchor: proposal.anchor, placement: proposal.placement }).is_ok(),
                None if float_drop || !over_group => {
                    let r = [pos.x - 40.0, pos.y - 12.0, 420.0, 320.0];
                    self.dock_command(ctx, Action::Float { panel: p, rect: r }).is_ok()
                }
                None => false,
            };
            if ok {
                self.ui.focused = p;
                self.show_panel(p);
            }
        }
    }
}

/// The insertion mark of a tab drag: a bar between two tabs at `x`, with caps.
fn insertion_mark(painter: &egui::Painter, x: f32, strip: Rect, col: Color32) {
    let (y0, y1) = (strip.min.y + 3.0, strip.max.y - 2.0);
    painter.line_segment([pos2(x, y0), pos2(x, y1)], Stroke::new(2.5, col));
    for (y, d) in [(y0, 1.0), (y1, -1.0)] {
        painter.add(egui::Shape::convex_polygon(vec![pos2(x - 4.5, y), pos2(x + 4.5, y), pos2(x, y + 5.0 * d)], col, Stroke::NONE));
    }
}

/// The native window of the floating group whose window entry names `panel`.
pub fn panel_window_id(panel: PanelKind) -> egui::ViewportId {
    egui::ViewportId::from_hash_of(("effectcraft-panel-window", panel.id()))
}

/// Where a native panel window was told to open (kept for as long as it stays open).
fn panel_window_spawn_id(panel: PanelKind) -> egui::Id {
    egui::Id::unique(("panel-window-spawn", panel.id()))
}

/// A new floating group's default rect near the centre of `screen`.
pub fn default_float_rect(screen: Rect) -> [f32; 4] {
    let c = screen.center();
    [c.x - 210.0, c.y - 160.0, 420.0, 320.0]
}

/// A corner is a two-axis resize control; focused arrows belong to it and Tab moves on.
fn floating_resize_delta(ui: &mut egui::Ui, response: &egui::Response) -> egui::Vec2 {
    if response.drag_started() {
        response.request_focus();
    }
    let mut delta = if response.dragged() { response.drag_delta() } else { egui::Vec2::ZERO };
    if response.has_focus() {
        ui.memory_mut(|memory| {
            memory.set_focus_lock_filter(response.id, egui::EventFilter { horizontal_arrows: true, vertical_arrows: true, ..Default::default() })
        });
        ui.input_mut(|input| {
            for (key, amount) in [
                (egui::Key::ArrowLeft, vec2(-8.0, 0.0)),
                (egui::Key::ArrowRight, vec2(8.0, 0.0)),
                (egui::Key::ArrowUp, vec2(0.0, -8.0)),
                (egui::Key::ArrowDown, vec2(0.0, 8.0)),
            ] {
                if input.consume_key(egui::Modifiers::NONE, key) {
                    delta += amount;
                }
            }
        });
    }
    if response.enabled() { delta } else { egui::Vec2::ZERO }
}
