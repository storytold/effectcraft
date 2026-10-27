use super::*;

impl EffectcraftApp {
    /// Keep the saved workspaces for the next session (see [`EffectcraftApp::new`]): After
    /// Effects' saved workspaces last until they are deleted (#320). `Err` when they weren't
    /// written (#696): the change applies for this session and a later save retries it, but until
    /// then a restart brings back what is on disk.
    pub(crate) fn store_saved_workspaces(&mut self) -> Result<(), String> {
        let Err(error) = self.workspace_store.save_now(self.session.config.as_deref(), &self.ui) else { return Ok(()) };
        let message = format!("Workspace changes couldn't be saved and may be lost after a restart: {error}");
        log::warn!("{message}");
        self.ui.status = message.clone();
        // The raw error, so the background retry doesn't report the same failure again.
        self.workspace_store.error = Some(error);
        Err(message)
    }

    pub fn set_workspace(&mut self, name: &str) {
        // Learn: the Home screen's Learn tab (tutorials) in the Composition panel. Leaving it for
        // another workspace closes the Home screen again: it stayed over every workspace until a
        // project was created (#272).
        if name == "Learn" {
            self.ui.start_screen = true;
            self.ui.home_learn = true;
            self.ui.home_templates = false;
        } else if self.ui.workspace == "Learn" {
            self.ui.start_screen = false;
            self.ui.home_learn = false;
        }
        self.ui.workspace = name.to_string();
        self.ui.dock = self.ui.saved_workspaces.get(name).cloned().unwrap_or_else(|| dock::Workspace::new(dock::workspace(name)));
        self.ui.maximized = None;
    }

    /// Built-in workspaces followed by the saved ones (Window ▸ Workspace ▸ Save as New Workspace).
    pub fn workspace_names(&self) -> Vec<String> {
        let mut v: Vec<String> = dock::WORKSPACES.iter().map(|s| s.to_string()).collect();
        v.extend(self.saved_workspace_names());
        v
    }

    /// The workspaces the user saved under a new name (built-ins whose changes were saved are
    /// not repeated), in name order.
    pub fn saved_workspace_names(&self) -> Vec<String> {
        self.ui.saved_workspaces.keys().filter(|k| !dock::WORKSPACES.contains(&k.as_str())).cloned().collect()
    }

    /// The open Gradient Editor's draft (`panels::gradient_editor`): what OK would set.
    pub fn gradient_draft(&self) -> &panels::gradient_editor::Draft {
        &self.dialog_state.gradient
    }

    /// The open Gradient Editor's draft, to edit (tests and automation; the editor's own fields
    /// do the same).
    pub fn gradient_draft_mut(&mut self) -> &mut panels::gradient_editor::Draft {
        &mut self.dialog_state.gradient
    }

    /// Show panel `p` (opening it in its usual place if it is closed), bring it to the front and
    /// give it the focus: Window ▸ `<panel>`.
    pub fn show_panel(&mut self, p: PanelKind) {
        if !self.ui.dock.is_floating(p) && self.ui.maximized.is_some_and(|m| m != p) {
            self.ui.maximized = None;
        }
        self.raise_panel(p);
        self.ui.focused = p;
    }

    /// Bring panel `p` to the front of its group, opening it if it is closed, without moving the
    /// focus (a comp opening brings up its viewer, an effect applied brings up Effect Controls).
    /// A closed panel goes back where it was; one never laid out goes to its usual place: the
    /// Composition panel in the centre with the Layer panel and other viewers (with none of
    /// those left, above the Timeline), Layer and the viewers beside the Composition panel, and
    /// so on.
    pub fn raise_panel(&mut self, p: PanelKind) {
        if !self.ui.dock.is_open(p) {
            let docked = |q: &PanelKind| self.ui.dock.is_docked(*q);
            let near = match p {
                PanelKind::Composition => self
                    .ui
                    .dock
                    .panels()
                    .into_iter()
                    .find(|q| matches!(q, PanelKind::Layer | PanelKind::Viewer(_) | PanelKind::Flowchart | PanelKind::Footage) && docked(q)),
                PanelKind::Layer | PanelKind::Flowchart | PanelKind::Viewer(_) => Some(PanelKind::Composition),
                PanelKind::RenderQueue => Some(PanelKind::Timeline),
                PanelKind::EffectControls | PanelKind::History => Some(PanelKind::Project),
                _ => Some(PanelKind::EffectsPresets),
            }
            .filter(|q| self.ui.dock.is_open(*q));
            let mut workspace = self.ui.dock.clone();
            let known = workspace.hidden.iter().any(|(q, _)| *q == p);
            let opened = if p == PanelKind::Composition && near.is_none() && !known && self.ui.dock.is_docked(PanelKind::Timeline) {
                workspace
                    .apply(
                        craft_ui::docking::Action::OpenAt {
                            panel: p,
                            anchor: PanelKind::Timeline,
                            placement: craft_ui::docking::Placement::Split(craft_ui::docking::Zone::Top),
                        },
                        dock::permissions,
                    )
                    .map(|_| ())
            } else if known {
                workspace.set_visible(p, true, near, dock::permissions)
            } else {
                workspace.apply(craft_ui::docking::Action::Open { panel: p, anchor: near }, dock::permissions).map(|_| ())
            };
            match opened {
                Ok(()) => self.ui.dock = workspace,
                Err(e) => {
                    self.ui.status = format!("can't open {}: {e}", p.title());
                    return;
                }
            }
        }
        let _ = self.ui.dock.apply(craft_ui::docking::Action::Activate { panel: p }, dock::permissions);
        if self.ui.dock.is_floating(p) {
            self.pending_panel_raise = Some(p);
        }
    }
}

/// Maximize a first window that doesn't fit the screen, which fits it to the space beside the
/// taskbar or dock. The 1680 × 1020 window on a smaller screen is only shrunk to the monitor's
/// size, so with its title bar it ran under the taskbar and cut menus off at the bottom of the
/// screen (#269).
pub(super) fn fit_window(ctx: &egui::Context) {
    let (monitor, window) = ctx.input(|i| (i.viewport().monitor_size, i.viewport().outer_rect.or(i.viewport().inner_rect)));
    if let (Some(monitor), Some(window)) = (monitor, window)
        && overflows_screen(window.size(), monitor)
    {
        ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(true));
    }
}

/// Whether a window `size` leaves no room on a `monitor` for a taskbar or dock (points).
fn overflows_screen(size: egui::Vec2, monitor: egui::Vec2) -> bool {
    const TASKBAR: f32 = 64.0;
    size.x > monitor.x || size.y > monitor.y - TASKBAR
}

#[cfg(test)]
mod fit_window_tests {
    use super::overflows_screen;
    use egui::vec2;

    #[test]
    fn a_window_taller_than_the_screen_beside_its_taskbar_overflows() {
        // The default window on a 1366 × 768 laptop (shrunk to the monitor) and on 1080p.
        assert!(overflows_screen(vec2(1366.0, 799.0), vec2(1366.0, 768.0)));
        assert!(overflows_screen(vec2(1680.0, 1051.0), vec2(1920.0, 1080.0)));
        assert!(!overflows_screen(vec2(1680.0, 1051.0), vec2(2560.0, 1440.0)));
    }
}
