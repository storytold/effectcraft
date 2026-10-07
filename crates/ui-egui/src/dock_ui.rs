//! The dock area: lays out the docked tree (or the maximized panel), draws each group's chrome
//! and body, floating (undocked) panel groups over it, and tab drag-and-drop: drag a tab onto a
//! group to tab it there (center) or split it off to a side (After Effects' drop zones, with a
//! highlight of where it will land); hold Cmd/Ctrl when releasing, or drop outside every group,
//! to float it. `` ` `` maximizes the panel under the pointer.

use egui::{Color32, Rect, Sense, Stroke, StrokeKind, pos2, vec2};

use crate::dock::{self, DockAction, DockNode, Group, Layout, PanelKind, Zone};
use crate::{EffectcraftApp, panels};

fn drag_id() -> egui::Id {
    egui::Id::new("dock-tab-drag")
}

fn floating_id(floating: &dock::Floating) -> egui::Id {
    egui::Id::new(("floating-panel", floating.panels.first().map(|p| p.id()).unwrap_or_default()))
}

impl EffectcraftApp {
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
            out.push((PanelKind::EffectControls, format!("Effect Controls {}", l.name)));
            out.push((PanelKind::Properties, format!("Properties: {}", l.name)));
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
        let mut l = Layout { root: self.ui.dock.clone(), floating: self.ui.floating.clone() };
        if !f(&mut l) {
            return false;
        }
        self.ui.dock = l.root;
        self.ui.floating = l.floating;
        if self.ui.maximized.is_some_and(|m| self.maximized_group(m).is_none()) {
            self.ui.maximized = None;
        }
        true
    }

    /// Close a panel wherever it is (docked or floating).
    pub fn close_panel(&mut self, p: PanelKind) {
        // Closing a ScriptUI panel closes its script window (its script's onClose runs).
        if let PanelKind::ScriptPanel(id) = p
            && self.session.script_ui.window(id).is_some()
            && let Err(e) = self.session.execute("scriptui.close", serde_json::json!({"window": id}))
        {
            self.ui.status = e.to_string();
        }
        if self.ui.maximized == Some(p) {
            let survivor = |panels: &[PanelKind], active: usize| {
                panels
                    .get(active)
                    .copied()
                    .filter(|panel| *panel != p)
                    .or_else(|| panels.iter().skip(active).copied().find(|panel| *panel != p))
                    .or_else(|| panels.iter().rev().copied().find(|panel| *panel != p))
            };
            // The anchor identifies the maximized group, not an obligation to keep its
            // original tab open. Retarget before removal invalidates the old lookup.
            // Stacked panels retain their separate close/restore policy.
            let next = match self.ui.dock.panel_group(p) {
                Some(DockNode::Tabs { panels, active }) => survivor(panels, *active),
                Some(_) => None,
                None => self.ui.floating.iter().find(|group| group.panels.contains(&p)).and_then(|group| survivor(&group.panels, group.active)),
            };
            if let Some(next) = next {
                self.ui.maximized = Some(next);
            }
        }
        self.ui.dock.close(p);
        self.edit_layout(|l| l.unfloat(p));
        panels::viewers::on_close(self, p);
        if self.ui.maximized == Some(p) {
            self.ui.maximized = None;
        }
    }

    /// The complete group of the maximized anchor, without altering its saved layout.
    fn maximized_group(&self, anchor: PanelKind) -> Option<DockNode> {
        let mut group = match self.ui.dock.panel_group(anchor) {
            Some(group) => group.clone(),
            None => {
                let floating = self.ui.floating.iter().find(|f| f.panels.contains(&anchor))?;
                DockNode::Tabs { panels: floating.panels.clone(), active: floating.active }
            }
        };
        // A fixed-height stack entry must become usable when maximized. Solo only this
        // display copy: keep sibling headers and the saved expansion/height preferences.
        if let DockNode::Stack { entries } = &mut group {
            for entry in entries {
                entry.open = entry.panel == anchor;
                if entry.open {
                    entry.height = None;
                }
            }
        }
        // Full-screen preview temporarily shows its viewer even if a sibling tab was active.
        // Activate only this display copy; stop restores the original workspace untouched.
        if self.playback.playing && self.playback.restore_max.is_some() && anchor == panels::viewers::active_panel(self) {
            group.activate(anchor);
        }
        Some(group)
    }

    pub(crate) fn maximized_contains(&self, panel: PanelKind) -> bool {
        self.ui.maximized.is_some_and(|anchor| {
            self.ui.dock.panel_group(anchor).is_some_and(|group| group.contains(panel))
                || self.ui.floating.iter().any(|group| group.panels.contains(&anchor) && group.panels.contains(&panel))
        })
    }

    /// Toggle the complete panel group. An anchor remains compatible with saved UI state;
    /// selecting a sibling while maximized must not lose its tabs or start a second maximize.
    pub fn toggle_maximize(&mut self, panel: PanelKind) {
        self.ui.maximized = if self.maximized_contains(panel) || self.maximized_group(panel).is_none() { None } else { Some(panel) };
    }

    /// The visible panel whose group is under `pos` (from the last frame), else the focused one.
    pub fn panel_at(&self, pos: Option<egui::Pos2>) -> PanelKind {
        pos.and_then(|p| self.dock_rects.iter().rev().find(|(_, r)| r.contains(p)).map(|(k, _)| *k)).unwrap_or(self.ui.focused)
    }

    /// Resolve overlapping floating groups through egui's actual raised-area order.
    pub(crate) fn panel_at_in(&self, ctx: &egui::Context, pos: Option<egui::Pos2>) -> PanelKind {
        if self.ui.maximized.is_none()
            && let Some(layer) = pos.and_then(|pos| ctx.layer_id_at(pos))
            && let Some(floating) = self.ui.floating.iter().find(|f| layer == egui::LayerId::new(egui::Order::Middle, floating_id(f)))
            && let Some(panel) = floating.panels.get(floating.active)
        {
            return *panel;
        }
        self.panel_at(pos)
    }

    fn panel_body(&mut self, ui: &mut egui::Ui, p: PanelKind, rect: Rect) {
        self.auto.add(&format!("panel.{}", p.id()), rect, p.title());
        let mut content = rect;
        let viewer = p == panels::viewers::active_panel(self);
        if viewer && !self.ui.start_screen && panels::precomp::has_flow(self) {
            // Composition Navigator: the flow of nested comps above the viewer.
            let nav = Rect::from_min_size(content.min, vec2(content.width(), panels::precomp::NAV_H));
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(nav).id_salt("comp-navigator"));
            child.set_clip_rect(nav.intersect(ui.clip_rect()));
            panels::precomp::navigator(self, &mut child, nav);
            content.min.y = nav.max.y;
        }
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(content).id_salt(("panel", p.id())));
        child.set_clip_rect(content.intersect(ui.clip_rect()));
        panels::show(self, &mut child, p, content);
        if viewer && !self.ui.start_screen {
            panels::anim_tools::sketch_overlay(self, &mut child);
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
        // Maximize the entire tabbed/stacked group; its original docking and floating
        // geometry stay untouched so restoring recovers the user's workspace exactly.
        let maximized = self.ui.maximized.and_then(|anchor| self.maximized_group(anchor));
        let is_maximized = maximized.is_some();
        let mut dock = match maximized {
            Some(group) => group,
            None => std::mem::replace(&mut self.ui.dock, DockNode::Tabs { panels: vec![], active: 0 }),
        };
        let mut groups = Vec::new();
        dock::layout(ui, &mut dock, body, &t, "", &mut groups, &mut self.auto);
        let mut actions = Vec::new();
        let titles = self.tab_titles();
        let decos = self.tab_decos();
        let docs = self.tab_docs();
        let title = |p: PanelKind| titles.iter().find(|(k, _)| *k == p).map(|(_, s)| s.clone()).unwrap_or_else(|| p.title().to_string());
        let info = dock::TabInfo { title: &title, decos: &decos, docs: &docs };
        // Where each group's tabs are (tab drags show where a tab will land between them).
        let mut tab_rects: Vec<(String, Vec<(PanelKind, Rect)>)> = vec![];
        for g in &groups {
            let c = dock::draw_group_chrome(ui, g, self.ui.focused, &t, &mut self.auto, &info);
            actions.extend(c.actions);
            tab_rects.push((g.path.clone(), c.tabs));
        }
        self.label_tab_marks(ui);
        if !is_maximized {
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
        // Floating groups over the dock.
        let mut float_groups: Vec<Group> = vec![];
        if !is_maximized {
            for i in 0..self.ui.floating.len() {
                if let Some((g, tabs)) = self.floating_window(&ctx, i, &info, &mut actions) {
                    if let Some(panel) = g.panels.get(g.active) {
                        self.dock_rects.push((*panel, g.rect));
                    }
                    tab_rects.push((g.path.clone(), tabs));
                    float_groups.push(g);
                }
            }
        }
        for a in actions {
            match a {
                DockAction::Activate(p) => {
                    if !self.ui.dock.activate(p)
                        && let Some(f) = self.ui.floating.iter_mut().find(|f| f.panels.contains(&p))
                    {
                        f.active = f.panels.iter().position(|x| *x == p).unwrap_or(0);
                    }
                }
                DockAction::ToggleMaximize(p) => self.toggle_maximize(p),
                DockAction::ToggleStacked(p) => {
                    if self.maximized_contains(p) {
                        // Selecting another header switches the temporary solo panel.
                        self.ui.maximized = Some(p);
                    } else {
                        self.ui.dock.toggle_stacked(p);
                    }
                }
                DockAction::Focus(p) => {
                    self.ui.focused = p;
                    // A click in (or on the tab of) another Composition viewer makes it active.
                    if let Some(id) = panels::viewers::id_of(p) {
                        panels::viewers::activate(self, &ctx, id);
                    }
                }
                DockAction::Close(p) => self.close_panel(p),
                DockAction::PanelMenu(p, pos) => {
                    ctx.data_mut(|d| d.insert_temp(egui::Id::new("panel-menu"), (p, pos)));
                }
                DockAction::BeginDrag(p) => {
                    // Return to the actual drop-zone geometry before docking a maximized tab.
                    if self.ui.maximized.take().is_some() {
                        ctx.request_repaint();
                    }
                    ctx.data_mut(|d| d.insert_temp(drag_id(), p));
                }
                DockAction::ToggleLock(p) => {
                    if !self.ui.locked_tabs.remove(&p.id()) {
                        self.ui.locked_tabs.insert(p.id().to_string());
                    }
                }
                // The Timeline's comp tabs: show that comp / close its Timeline.
                DockAction::ActivateDoc(p, id) => {
                    if !self.ui.dock.activate(p)
                        && let Some(f) = self.ui.floating.iter_mut().find(|f| f.panels.contains(&p))
                    {
                        f.active = f.panels.iter().position(|x| *x == p).unwrap_or(0);
                    }
                    if self.session.state.active_comp.map(|c| c.0) != Some(id) {
                        let _ = crate::menus::invoke(self, &ctx, "comp.open", serde_json::json!({"comp": id}));
                    }
                }
                DockAction::CloseDoc(_, id) => {
                    let _ = crate::menus::invoke(self, &ctx, "comp.close", serde_json::json!({"comp": id}));
                }
            }
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
        let id = floating_id(&f);
        let mut group = None;
        egui::Area::new(id).order(egui::Order::Middle).fixed_pos(rect.min).interactable(true).show(ctx, |ui| {
            let (r, _) = ui.allocate_exact_size(rect.size(), Sense::hover());
            ui.painter().rect_filled(r.expand(1.0), t.radius, Color32::from_black_alpha(90));
            let strip = Rect::from_min_size(r.min, vec2(r.width(), t.tab_h));
            // Move by the strip (tabs, registered later, take precedence over it).
            let mv = ui.interact(strip, id.with("move"), Sense::drag());
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
            let c = dock::draw_group_chrome(ui, &g, self.ui.focused, &t, &mut self.auto, info);
            actions.extend(c.actions);
            ui.painter().rect_stroke(r, t.radius, Stroke::new(1.0, t.field_border), StrokeKind::Outside);
            if let Some(p) = g.panels.get(g.active).copied() {
                self.panel_body(ui, p, g.content);
            }
            // Resize from the corner.
            let grip = Rect::from_min_max(r.max - vec2(14.0, 14.0), r.max);
            let rs = ui.interact(grip, id.with("resize"), Sense::drag());
            self.auto.add(&format!("panel.float.{i}.resize"), grip, "Resize floating panel");
            for k in 0..3 {
                let o = 4.0 + k as f32 * 4.0;
                ui.painter().line_segment([pos2(r.max.x - o, r.max.y - 2.0), pos2(r.max.x - 2.0, r.max.y - o)], Stroke::new(1.0, t.text_faint));
            }
            if rs.hovered() || rs.dragged() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeNwSe);
            }
            if rs.dragged() {
                nr = Rect::from_min_size(nr.min, (nr.size() + rs.drag_delta()).max(vec2(160.0, 100.0)));
            }
            if nr != r
                && let Some(fl) = self.ui.floating.get_mut(i)
            {
                fl.rect = [nr.min.x, nr.min.y, nr.width(), nr.height()];
            }
            group = Some((g, c.tabs));
        });
        group
    }

    /// A tab being dragged. Over a tab strip (or the middle of a group) it shows where the tab
    /// will land: an insertion mark between two tabs (a drop there reorders a group's own tabs
    /// too); near a group's edge, the half it will split off. On release it lands there (or
    /// floats with Cmd/Ctrl or outside every group).
    fn tab_drag(&mut self, ctx: &egui::Context, groups: &[Group], floats: &[Group], tab_rects: &[(String, Vec<(PanelKind, Rect)>)]) {
        let Some(p) = ctx.data(|d| d.get_temp::<PanelKind>(drag_id())) else { return };
        let t = self.tokens;
        let Some(pos) = ctx.pointer_latest_pos() else { return };
        // Area layers are ordered back-to-front. Search only floating windows, not
        // the foreground drop painter/ghost, and never target a covered dock group.
        let floating_target = ctx.memory(|memory| {
            memory
                .layer_ids()
                .filter_map(|layer| {
                    floats.iter().find(|g| {
                        g.rect.contains(pos)
                            && layer
                                == egui::LayerId::new(
                                    egui::Order::Middle,
                                    egui::Id::new(("floating-panel", g.panels.first().map(|p| p.id()).unwrap_or_default())),
                                )
                    })
                })
                .last()
        });
        let target = floating_target
            .map(|g| (g, true))
            .or_else(|| if floats.iter().any(|g| g.rect.contains(pos)) { None } else { groups.iter().find(|g| g.rect.contains(pos)).map(|g| (g, false)) });
        let float_drop = ctx.input(|i| i.modifiers.command);
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("dock-drop")));
        let plan: Option<DropPlan> = match target {
            Some((g, is_float)) if !float_drop => {
                let others: Vec<PanelKind> = g.panels.iter().copied().filter(|x| *x != p).collect();
                let zone = if is_float { Zone::Center } else { dock::drop_zone(g.rect, t.tab_h, pos) };
                match (others.first().copied(), zone) {
                    // The dragged tab alone in its group: nowhere to go here.
                    (None, _) => None,
                    (Some(first), Zone::Center) => {
                        let rects: Vec<(PanelKind, Rect)> = tab_rects
                            .iter()
                            .find(|(path, _)| *path == g.path)
                            .map(|(_, r)| r.iter().copied().filter(|(k, _)| *k != p).collect())
                            .unwrap_or_default();
                        let strip = Rect::from_min_size(g.rect.min, vec2(g.rect.width(), t.tab_h));
                        let before = if strip.contains(pos) {
                            // Before the first tab whose middle is right of the pointer.
                            rects.iter().find(|(_, r)| r.center().x > pos.x).map(|(k, _)| *k)
                        } else if g.panels.contains(&p) {
                            // The middle of its own group: it stays where it is.
                            Some(p)
                        } else {
                            // The middle of another group: right after its shown tab.
                            let shown = g.panels.get(g.active).and_then(|a| others.iter().position(|x| x == a)).unwrap_or(others.len());
                            others.get(shown + 1).copied()
                        };
                        if before == Some(p) {
                            None
                        } else {
                            let x = match before.and_then(|b| rects.iter().find(|(k, _)| *k == b)) {
                                Some((_, r)) => r.min.x - 4.0,
                                None => rects.last().map_or(strip.min.x + 8.0, |(_, r)| r.max.x + 4.0),
                            };
                            painter.rect_filled(strip.shrink2(vec2(2.0, 1.0)), t.radius, t.focus.gamma_multiply(0.12));
                            if !strip.contains(pos) {
                                painter.rect_stroke(g.rect, t.radius, Stroke::new(1.0, t.focus.gamma_multiply(0.6)), StrokeKind::Inside);
                            }
                            insertion_mark(&painter, x, strip, t.focus);
                            Some(DropPlan::Tab { anchor: first, before })
                        }
                    }
                    (Some(first), zone) => {
                        let shown = g.panels.get(g.active).copied().filter(|a| *a != p).unwrap_or(first);
                        let hr = dock::zone_rect(g.rect, zone).shrink(2.0);
                        painter.rect_filled(hr, t.radius, t.focus.gamma_multiply(0.25));
                        painter.rect_stroke(hr, t.radius, Stroke::new(2.0, t.focus), StrokeKind::Inside);
                        // Outline the whole group for the side zones, like AE's drop-zone frame.
                        painter.rect_stroke(g.rect, t.radius, Stroke::new(1.0, t.focus.gamma_multiply(0.6)), StrokeKind::Inside);
                        Some(DropPlan::Side { anchor: shown, zone })
                    }
                }
            }
            _ => None,
        };
        // Ghost of the dragged tab.
        let g = painter.layout_no_wrap(p.title().to_string(), crate::theme::Tokens::ui(12.0), Color32::WHITE);
        let gr = Rect::from_min_size(pos + vec2(12.0, 10.0), g.size() + vec2(16.0, 8.0));
        painter.rect_filled(gr, 4.0, t.focus.gamma_multiply(0.85));
        painter.galley(gr.min + vec2(8.0, 4.0), g, Color32::WHITE);
        ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
        ctx.request_repaint();
        if ctx.input(|i| i.pointer.any_released()) {
            ctx.data_mut(|d| d.remove::<PanelKind>(drag_id()));
            let ok = match plan {
                Some(DropPlan::Tab { anchor, before }) => self.edit_layout(|l| l.insert_tab(p, anchor, before)),
                Some(DropPlan::Side { anchor, zone }) => self.edit_layout(|l| l.dock(p, anchor, zone)),
                None if float_drop || target.is_none() => {
                    let r = [pos.x - 40.0, pos.y - 12.0, 420.0, 320.0];
                    self.edit_layout(|l| l.float(p, r))
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

/// Where a dragged tab lands.
enum DropPlan {
    /// A tab in the group of `anchor`, before `before` (last when `None`).
    Tab { anchor: PanelKind, before: Option<PanelKind> },
    /// A new group split off one side of `anchor`'s group.
    Side { anchor: PanelKind, zone: Zone },
}

/// The insertion mark of a tab drag: a bar between two tabs at `x`, with caps.
fn insertion_mark(painter: &egui::Painter, x: f32, strip: Rect, col: Color32) {
    let (y0, y1) = (strip.min.y + 3.0, strip.max.y - 2.0);
    painter.line_segment([pos2(x, y0), pos2(x, y1)], Stroke::new(2.5, col));
    for (y, d) in [(y0, 1.0), (y1, -1.0)] {
        painter.add(egui::Shape::convex_polygon(vec![pos2(x - 4.5, y), pos2(x + 4.5, y), pos2(x, y + 5.0 * d)], col, Stroke::NONE));
    }
}

/// A new floating group's default rect near the centre of `screen`.
pub fn default_float_rect(screen: Rect) -> [f32; 4] {
    let c = screen.center();
    [c.x - 210.0, c.y - 160.0, 420.0, 320.0]
}

#[cfg(test)]
mod maximize_tests {
    use super::*;
    use crate::dock::SplitSize;
    use egui::{Event, Modifiers, PointerButton};

    struct Fixture {
        app: EffectcraftApp,
        ctx: egui::Context,
        time: f64,
        body: Rect,
    }

    impl Fixture {
        fn new() -> Self {
            let mut app = EffectcraftApp::new(effectcraft_engine::Session::default());
            app.ui.start_screen = false;
            app.ui.dock = DockNode::Split {
                vertical: false,
                size: SplitSize::FixedA(340.0),
                a: Box::new(DockNode::Tabs { panels: vec![PanelKind::Project, PanelKind::EffectControls], active: 0 }),
                b: Box::new(DockNode::Tabs { panels: vec![PanelKind::Composition, PanelKind::Layer], active: 0 }),
            };
            app.ui.saved_workspaces.insert("Original".into(), app.ui.dock.clone());
            let ctx = egui::Context::default();
            crate::theme::install(&ctx, &app.tokens);
            let mut fixture = Self { app, ctx, time: 0.0, body: Rect::from_min_size(pos2(20.0, 20.0), vec2(1100.0, 700.0)) };
            fixture.frame(vec![]);
            fixture.frame(vec![]);
            fixture
        }

        fn frame(&mut self, events: Vec<Event>) {
            self.time += 0.02;
            self.app.auto.begin_frame();
            self.ctx
                .run_ui(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(1200.0, 800.0))),
                        time: Some(self.time),
                        events,
                        ..Default::default()
                    },
                    |ui| {
                        crate::menus::handle_shortcuts(&mut self.app, ui.ctx());
                        self.app.dock_area(ui, self.body);
                    },
                )
                .drop_without_applying_deltas();
            assert!(self.app.gpu.is_none());
            assert_eq!(self.app.frames.inflight(), 0, "empty-comp fixture must not start a render worker");
        }

        fn rect(&self, id: &str) -> Rect {
            let element = self.app.auto.elements.iter().find(|e| e.id == id).unwrap_or_else(|| panic!("missing {id}"));
            Rect::from_min_size(pos2(element.rect[0], element.rect[1]), vec2(element.rect[2], element.rect[3]))
        }

        fn click(&mut self, pos: egui::Pos2) {
            for pressed in [true, false] {
                self.frame(vec![Event::PointerMoved(pos), Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE }]);
            }
            self.frame(vec![]);
        }

        fn double_click(&mut self, pos: egui::Pos2) {
            self.time += 1.0;
            self.click(pos);
            self.click(pos);
            self.frame(vec![]);
        }

        fn grave(&mut self, pos: egui::Pos2) {
            self.frame(vec![
                Event::PointerMoved(pos),
                Event::Key { key: egui::Key::Backtick, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE },
            ]);
            self.frame(vec![Event::Key { key: egui::Key::Backtick, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::NONE }]);
        }
    }

    #[test]
    fn active_tab_double_click_keeps_sibling_tabs_and_restores_workspace_geometry() {
        let mut fixture = Fixture::new();
        let original = fixture.app.ui.dock.clone();
        let saved = fixture.app.ui.saved_workspaces.clone();
        let original_rect = fixture.rect("panel.Project");
        fixture.double_click(fixture.rect("panel.tab.Project").center());
        assert!(fixture.app.maximized_contains(PanelKind::Project));
        assert!(fixture.app.maximized_contains(PanelKind::EffectControls));
        assert_eq!(fixture.rect("panel.Project").width(), fixture.body.width());
        assert!(fixture.app.auto.elements.iter().any(|e| e.id == "panel.tab.EffectControls"));
        assert_eq!(fixture.app.ui.dock, original);
        fixture.click(fixture.rect("panel.tab.EffectControls").center());
        assert!(fixture.app.maximized_contains(PanelKind::EffectControls));
        assert_eq!(fixture.rect("panel.EffectControls").width(), fixture.body.width());
        // Restoring through a sibling caption must restore this same group, not maximize twice.
        fixture.double_click(fixture.rect("panel.tab.EffectControls").center());
        assert!(fixture.app.ui.maximized.is_none());
        let mut expected = original;
        expected.activate(PanelKind::EffectControls);
        assert_eq!(fixture.app.ui.dock, expected);
        assert_eq!(fixture.rect("panel.EffectControls"), original_rect);
        assert_eq!(fixture.app.ui.saved_workspaces, saved);
    }

    #[test]
    fn empty_tab_well_double_click_and_grave_restore_the_same_group() {
        let mut fixture = Fixture::new();
        let original = fixture.app.ui.dock.clone();
        // The right group remains deliberately unfocused: pointer location chooses it.
        fixture.app.ui.focused = PanelKind::Project;
        fixture.double_click(fixture.rect("panel.tabWell.Composition").center());
        assert!(fixture.app.maximized_contains(PanelKind::Composition));
        assert!(fixture.app.maximized_contains(PanelKind::Layer));
        fixture.grave(fixture.rect("panel.Composition").center());
        assert!(fixture.app.ui.maximized.is_none());
        assert_eq!(fixture.app.ui.dock, original);
        fixture.app.ui.focused = PanelKind::Project;
        fixture.grave(fixture.rect("panel.Composition").center());
        assert!(fixture.app.maximized_contains(PanelKind::Layer));
        fixture.double_click(fixture.rect("panel.tabWell.Composition").center());
        assert!(fixture.app.ui.maximized.is_none());
        assert_eq!(fixture.app.ui.dock, original);
    }

    #[test]
    fn floating_group_grave_keeps_tabs_and_preserves_original_floating_geometry() {
        let mut fixture = Fixture::new();
        fixture.app.ui.floating.push(dock::Floating { panels: vec![PanelKind::Info, PanelKind::Audio], active: 0, rect: [500.0, 180.0, 360.0, 260.0] });
        fixture.frame(vec![]);
        fixture.frame(vec![]);
        let original = fixture.app.ui.dock.clone();
        let floating = fixture.app.ui.floating.clone();
        let original_rect = fixture.rect("panel.Info");
        assert_eq!(fixture.app.panel_at(Some(original_rect.center())), PanelKind::Info);
        fixture.grave(original_rect.center());
        assert!(fixture.app.maximized_contains(PanelKind::Info));
        assert!(fixture.app.maximized_contains(PanelKind::Audio));
        assert_eq!(fixture.rect("panel.Info").width(), fixture.body.width());
        assert!(fixture.app.auto.elements.iter().any(|e| e.id == "panel.tab.Audio"));
        fixture.grave(fixture.rect("panel.Info").center());
        assert!(fixture.app.ui.maximized.is_none());
        assert_eq!(fixture.app.ui.dock, original);
        assert_eq!(fixture.app.ui.floating, floating);
        assert_eq!(fixture.rect("panel.Info"), original_rect);
    }

    #[test]
    fn full_screen_preview_shows_hidden_viewer_without_changing_saved_active_sibling() {
        let mut fixture = Fixture::new();
        fixture.app.session.execute("comp.new", serde_json::json!({"name": "Preview", "width": 4, "height": 4, "duration": 1})).unwrap();
        fixture.app.ui.dock.activate(PanelKind::Layer);
        fixture.app.ui.maximized = Some(PanelKind::Project);
        let original = fixture.app.ui.dock.clone();
        let saved = fixture.app.ui.saved_workspaces.clone();
        let preset = fixture.app.session.prefs.preview.get_mut(effectcraft_engine::preview::PreviewShortcut::Spacebar);
        preset.full_screen = true;
        preset.include_audio = false;
        preset.cache_before_playback = true;
        fixture.app.play_with(1.0, effectcraft_engine::preview::PreviewShortcut::Spacebar);
        assert!(fixture.app.playback.playing);
        assert_eq!(fixture.app.ui.maximized, Some(PanelKind::Composition));
        let displayed = fixture.app.maximized_group(PanelKind::Composition).unwrap();
        assert!(
            matches!(displayed, DockNode::Tabs { panels, active } if panels.get(active) == Some(&PanelKind::Composition) && panels.contains(&PanelKind::Layer))
        );
        assert_eq!(fixture.app.ui.dock, original);
        fixture.app.stop();
        assert_eq!(fixture.app.ui.maximized, Some(PanelKind::Project));
        assert_eq!(fixture.app.ui.dock, original);
        assert_eq!(fixture.app.ui.saved_workspaces, saved);
        let restored = fixture.app.maximized_group(PanelKind::Composition).unwrap();
        assert!(matches!(restored, DockNode::Tabs { panels, active } if panels.get(active) == Some(&PanelKind::Layer)));
        assert!(fixture.app.gpu.is_none());
        assert_eq!(fixture.app.frames.inflight(), 0);
    }

    #[test]
    fn grave_targets_raised_floating_group_instead_of_later_created_covered_group() {
        let mut fixture = Fixture::new();
        fixture.app.ui.floating = vec![
            dock::Floating { panels: vec![PanelKind::Info, PanelKind::Audio], active: 0, rect: [500.0, 180.0, 360.0, 260.0] },
            dock::Floating { panels: vec![PanelKind::Preview, PanelKind::Align], active: 0, rect: [600.0, 200.0, 360.0, 260.0] },
        ];
        for _ in 0..3 {
            fixture.frame(vec![]);
        }
        let overlap = pos2(650.0, 300.0);
        assert_eq!(fixture.app.panel_at_in(&fixture.ctx, Some(overlap)), PanelKind::Preview);
        // Click an exposed part of the earlier group: egui raises it above the later group.
        fixture.click(pos2(520.0, 300.0));
        let raised = egui::LayerId::new(egui::Order::Middle, floating_id(&fixture.app.ui.floating[0]));
        assert_eq!(fixture.ctx.layer_id_at(overlap), Some(raised));
        assert_eq!(fixture.app.panel_at_in(&fixture.ctx, Some(overlap)), PanelKind::Info);
        let floating = fixture.app.ui.floating.clone();
        fixture.grave(overlap);
        assert!(fixture.app.maximized_contains(PanelKind::Info));
        assert!(fixture.app.maximized_contains(PanelKind::Audio));
        assert!(!fixture.app.maximized_contains(PanelKind::Preview));
        fixture.grave(fixture.rect("panel.Info").center());
        assert_eq!(fixture.app.ui.floating, floating);
    }

    #[test]
    fn showing_a_sibling_keeps_the_group_maximized_and_other_groups_restore_the_workspace() {
        let mut fixture = Fixture::new();
        let original = fixture.app.ui.dock.clone();
        fixture.app.toggle_maximize(PanelKind::Project);
        fixture.app.show_panel(PanelKind::EffectControls);
        fixture.frame(vec![]);
        assert!(fixture.app.maximized_contains(PanelKind::Project));
        assert_eq!(fixture.rect("panel.EffectControls").width(), fixture.body.width());
        fixture.app.show_panel(PanelKind::Composition);
        fixture.frame(vec![]);
        assert!(fixture.app.ui.maximized.is_none());
        let mut expected = original;
        expected.activate(PanelKind::EffectControls);
        assert_eq!(fixture.app.ui.dock, expected);
    }

    #[test]
    fn showing_a_closed_stacked_sibling_switches_solo_without_changing_the_saved_layout() {
        let mut fixture = Fixture::new();
        let stack = DockNode::Stack {
            entries: vec![
                dock::StackEntry { panel: PanelKind::Preview, open: true, height: Some(46.0) },
                dock::StackEntry { panel: PanelKind::Properties, open: false, height: None },
                dock::StackEntry { panel: PanelKind::Align, open: true, height: Some(90.0) },
            ],
        };
        fixture.app.ui.dock = stack.clone();
        fixture.frame(vec![]);
        let original_preview = fixture.rect("panel.Preview");
        let original_align = fixture.rect("panel.Align");
        fixture.app.toggle_maximize(PanelKind::Preview);
        fixture.frame(vec![]);
        fixture.app.show_panel(PanelKind::Properties);
        fixture.frame(vec![]);
        assert_eq!(fixture.app.ui.maximized, Some(PanelKind::Properties));
        assert_eq!(fixture.app.ui.focused, PanelKind::Properties);
        assert!(fixture.rect("panel.Properties").height() > 500.0, "Window/show_panel must expose the requested stacked panel");
        assert!(!fixture.app.auto.elements.iter().any(|e| e.id == "panel.Preview"));
        assert_eq!(fixture.app.ui.dock, stack, "showing a previously closed sibling must not open its saved entry");
        fixture.app.raise_panel(PanelKind::Preview);
        fixture.frame(vec![]);
        assert_eq!(fixture.app.ui.maximized, Some(PanelKind::Preview));
        assert_eq!(fixture.app.ui.focused, PanelKind::Properties, "raising a panel must not move keyboard focus");
        assert!(fixture.rect("panel.Preview").height() > 500.0);
        assert_eq!(fixture.app.ui.dock, stack);
        fixture.app.toggle_maximize(PanelKind::Preview);
        fixture.frame(vec![]);
        assert_eq!(fixture.app.ui.dock, stack);
        assert_eq!(fixture.rect("panel.Preview"), original_preview);
        assert_eq!(fixture.rect("panel.Align"), original_align);
        assert!(!fixture.app.auto.elements.iter().any(|e| e.id == "panel.Properties"), "restoring must leave the sibling closed");
    }

    #[test]
    fn stacked_group_maximization_keeps_expansion_state_and_saved_state_compatibility() {
        let mut fixture = Fixture::new();
        let mut stack = dock::workspace("Default");
        stack.toggle_stacked(PanelKind::Align);
        fixture.app.ui.dock = stack.clone();
        fixture.frame(vec![]);
        let original_preview = fixture.rect("panel.Preview");
        let original_properties = fixture.rect("panel.Properties");
        let original_align = fixture.rect("panel.Align");
        fixture.app.toggle_maximize(PanelKind::Preview);
        fixture.frame(vec![]);
        assert!(fixture.app.maximized_contains(PanelKind::Preview));
        assert!(fixture.app.maximized_contains(PanelKind::Align));
        let expanded = fixture.rect("panel.Preview");
        assert!(expanded.height() > 400.0, "maximized Preview must have usable controls, not its fixed transport-row height");
        for id in ["preview.shortcut", "preview.includeVideo", "preview.loop", "preview.resolution", "preview.fullScreen", "preview.moveTimeToPreviewTime"] {
            assert!(expanded.contains_rect(fixture.rect(id)), "{id} must be inside the clickable body");
        }
        let before_loop = fixture.app.session.prefs.preview.spacebar.loop_;
        fixture.click(fixture.rect("preview.loop").center());
        assert_eq!(fixture.app.session.prefs.preview.spacebar.loop_, !before_loop);
        assert_eq!(fixture.app.ui.dock, stack);
        fixture.click(fixture.rect("panel.tab.Properties").center());
        assert_eq!(fixture.app.ui.maximized, Some(PanelKind::Properties));
        assert!(fixture.rect("panel.Properties").height() > 400.0);
        assert!(fixture.app.auto.elements.iter().any(|e| e.id == "panel.tab.Preview"));
        assert_eq!(fixture.app.ui.dock, stack, "switching solo headers must not change saved expansion or heights");
        let encoded = serde_json::to_string(&fixture.app.ui).unwrap();
        let loaded: crate::state::UiState = serde_json::from_str(&encoded).unwrap();
        assert_eq!(loaded.dock, stack);
        assert_eq!(loaded.maximized, Some(PanelKind::Properties));
        fixture.app.toggle_maximize(PanelKind::Align);
        assert!(fixture.app.ui.maximized.is_none());
        fixture.frame(vec![]);
        assert_eq!(fixture.app.ui.dock, stack);
        assert_eq!(fixture.rect("panel.Preview"), original_preview);
        assert_eq!(fixture.rect("panel.Properties"), original_properties);
        assert_eq!(fixture.rect("panel.Align"), original_align);
    }

    #[test]
    fn tab_drop_targets_the_visible_floating_group_in_both_creation_and_raised_order() {
        for raise_earlier in [false, true] {
            let mut fixture = Fixture::new();
            fixture.app.ui.floating = vec![
                dock::Floating { panels: vec![PanelKind::Info, PanelKind::Audio], active: 0, rect: [500.0, 180.0, 360.0, 260.0] },
                dock::Floating { panels: vec![PanelKind::Preview, PanelKind::Align], active: 0, rect: [600.0, 200.0, 360.0, 260.0] },
            ];
            for _ in 0..3 {
                fixture.frame(vec![]);
            }
            if raise_earlier {
                fixture.click(pos2(520.0, 300.0));
            }
            let target_index = if raise_earlier { 0 } else { 1 };
            let target_layer = egui::LayerId::new(egui::Order::Middle, floating_id(&fixture.app.ui.floating[target_index]));
            let drop = pos2(650.0, 300.0);
            assert_eq!(fixture.ctx.layer_id_at(drop), Some(target_layer));
            let before = fixture.app.ui.floating.clone();
            let start = fixture.rect("panel.tab.Project").min + vec2(10.0, 12.0);
            fixture.frame(vec![
                Event::PointerMoved(start),
                Event::PointerButton { pos: start, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE },
            ]);
            fixture.frame(vec![Event::PointerMoved(drop)]);
            fixture.frame(vec![Event::PointerMoved(drop)]);
            assert_eq!(fixture.ctx.data(|d| d.get_temp::<PanelKind>(drag_id())), Some(PanelKind::Project), "real pointer drag must start");
            assert_eq!(fixture.ctx.layer_id_at(drop), Some(target_layer), "the foreground drop ghost must not become a target");
            fixture.frame(vec![Event::PointerButton { pos: drop, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }]);
            fixture.frame(vec![]);
            assert!(fixture.app.ui.floating[target_index].panels.contains(&PanelKind::Project));
            assert_eq!(fixture.app.ui.floating[1 - target_index], before[1 - target_index], "covered floating group must stay unchanged");
            assert_eq!(fixture.app.ui.floating[target_index].rect, before[target_index].rect);
            assert!(!fixture.app.ui.dock.contains(PanelKind::Project), "covered dock group must not receive the drop");
            assert!(fixture.app.ui.dock.contains(PanelKind::EffectControls));
            assert_eq!(fixture.app.ui.floating.len(), 2);
        }
    }

    fn check_maximized_tab_close(floating: bool) {
        for (anchor, shown, remaining, visible) in [
            (PanelKind::Project, PanelKind::EffectControls, vec![PanelKind::EffectControls, PanelKind::History], PanelKind::EffectControls),
            (PanelKind::Project, PanelKind::Project, vec![PanelKind::EffectControls, PanelKind::History], PanelKind::EffectControls),
            (PanelKind::History, PanelKind::History, vec![PanelKind::Project, PanelKind::EffectControls], PanelKind::EffectControls),
        ] {
            let mut fixture = Fixture::new();
            let panels = vec![PanelKind::Project, PanelKind::EffectControls, PanelKind::History];
            let float_rect = [300.0, 140.0, 600.0, 450.0];
            if floating {
                fixture.app.ui.dock = DockNode::Tabs { panels: vec![PanelKind::Composition, PanelKind::Layer], active: 0 };
                fixture.app.ui.floating = vec![dock::Floating { panels, active: 0, rect: float_rect }];
            } else {
                fixture.app.ui.dock = DockNode::Split {
                    vertical: false,
                    size: SplitSize::FixedA(340.0),
                    a: Box::new(DockNode::Tabs { panels, active: 0 }),
                    b: Box::new(DockNode::Tabs { panels: vec![PanelKind::Composition, PanelKind::Layer], active: 0 }),
                };
            }
            fixture.app.raise_panel(anchor);
            fixture.app.toggle_maximize(anchor);
            fixture.app.show_panel(shown);
            fixture.frame(vec![]);
            assert_eq!(fixture.app.ui.maximized, Some(anchor));
            let close = fixture.rect(&format!("panel.tab.{}", anchor.id())).min + vec2(10.0, 12.0);
            for pressed in [true, false] {
                fixture.frame(vec![
                    Event::PointerMoved(close),
                    Event::PointerButton { pos: close, button: PointerButton::Middle, pressed, modifiers: Modifiers::NONE },
                ]);
            }
            fixture.frame(vec![]);
            assert_eq!(fixture.app.ui.maximized, Some(visible), "closing {anchor:?} must preserve the surviving maximized group");
            assert_eq!(fixture.rect(&format!("panel.{}", visible.id())).width(), fixture.body.width());
            if floating {
                let group = &fixture.app.ui.floating[0];
                assert_eq!(group.panels, remaining);
                assert_eq!(group.panels.get(group.active), Some(&visible));
                assert_eq!(group.rect, float_rect);
            } else {
                assert!(
                    matches!(fixture.app.ui.dock.panel_group(visible), Some(DockNode::Tabs { panels, active }) if *panels == remaining && panels.get(*active) == Some(&visible))
                );
            }
            let dock = fixture.app.ui.dock.clone();
            let floats = fixture.app.ui.floating.clone();
            fixture.app.toggle_maximize(visible);
            fixture.frame(vec![]);
            assert!(fixture.app.ui.maximized.is_none());
            assert_eq!(fixture.app.ui.dock, dock);
            assert_eq!(fixture.app.ui.floating, floats);
            assert!(fixture.rect(&format!("panel.{}", visible.id())).width() < fixture.body.width());
        }
    }

    #[test]
    fn closing_inactive_or_active_anchor_keeps_docked_tab_group_maximized() {
        check_maximized_tab_close(false);
    }

    #[test]
    fn closing_inactive_or_active_anchor_keeps_floating_tab_group_maximized() {
        check_maximized_tab_close(true);
    }
}
