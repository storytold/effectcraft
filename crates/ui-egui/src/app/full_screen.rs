use crate::{EffectcraftApp, FullScreen, FullScreenView, FullScreenWindow, panels};
use serde_json::json;

/// Windows: whether the OS plays its minimize and maximize animations on the window. Failing leaves
/// them on, which only looks worse.
#[cfg(windows)]
fn set_window_transitions(frame: &eframe::Frame, on: bool) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let Ok(handle) = frame.window_handle() else { return };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else { return };
    let target = handle.hwnd.get();
    // Stopping at the window makes the enumeration report a failure.
    let _ = winsafe::EnumThreadWindows(winsafe::GetCurrentThreadId(), |hwnd| {
        if hwnd.ptr() as isize != target {
            return true;
        }
        if let Err(e) = hwnd.DwmSetWindowAttribute(winsafe::DwmAttr::TransitionsForceDisabled(!on)) {
            log::warn!("could not set the window animations: {e}");
        }
        false
    });
}

#[cfg(not(windows))]
fn set_window_transitions(_frame: &eframe::Frame, _on: bool) {}

/// The toast View ▸ Enter Full Screen shows: the menus are hidden, so this is the way out.
const EXIT_HINT: &str = "Press Esc to exit Full Screen";

impl FullScreen {
    /// A viewer view `s` times bigger, so the same part of the comp shows in the same place. The
    /// pan follows the zoom actually set, or a clamped zoom would show another part of the comp.
    fn scale_view(zoom: Option<f32>, pan: [f32; 2], s: f32) -> (Option<f32>, [f32; 2]) {
        let (ratio, zoom) = zoom.map_or((s, None), |z| {
            let scaled = (z * s).clamp(0.01, 32.0);
            (scaled / z, Some(scaled))
        });
        let ratio = if ratio.is_finite() { ratio } else { s };
        let moved = pan.map(|p| p * ratio);
        (zoom, if moved.iter().all(|p| p.is_finite()) { moved } else { pan })
    }

    /// The panel's view for the view showing now: the one Full Screen blew up, or a view changed in
    /// Full Screen scaled back down to the panel.
    pub(crate) fn panel_view(&self, zoom: Option<f32>, pan: [f32; 2]) -> (Option<f32>, [f32; 2]) {
        match self.shown {
            // Not blown up yet: the view is the panel's.
            None => (zoom, pan),
            Some(v) if v.zoom == zoom && v.pan == pan => (self.saved_zoom, self.saved_pan),
            Some(v) if v.scale.is_finite() && v.scale > 0.0 => Self::scale_view(zoom, pan, 1.0 / v.scale),
            Some(_) => (self.saved_zoom, self.saved_pan),
        }
    }

    /// The view for a Full Screen area `s` times the panel's, from the panel's view each time, so
    /// a window that gets there in steps ends the same. A view changed in Full Screen is the
    /// panel's view now, as it would be on leaving.
    pub(crate) fn blow_up(&mut self, zoom: Option<f32>, pan: [f32; 2], s: f32) -> (Option<f32>, [f32; 2]) {
        (self.saved_zoom, self.saved_pan) = self.panel_view(zoom, pan);
        let (zoom, pan) = Self::scale_view(self.saved_zoom, self.saved_pan, s);
        self.shown = Some(FullScreenView { zoom, pan, scale: s });
        (zoom, pan)
    }
}

impl EffectcraftApp {
    /// Full Screen needs a video to show: a comp, and no Home screen over the workspace.
    pub(crate) fn full_screen_possible(&self) -> bool {
        !self.ui.start_screen && self.session.active_comp_id().is_some()
    }

    /// Show only the active viewer's video (`from_preview`: until that preview ends).
    pub(crate) fn enter_full_screen(&mut self, from_preview: bool) {
        // The viewer's text editor is part of what Full Screen doesn't run.
        if self.session.state.text_edit.is_some()
            && let Err(e) = self.session.execute("text.endEdit", json!({}))
        {
            self.ui.status = e.to_string();
        }
        self.full_screen = Some(FullScreen {
            saved_zoom: self.ui.viewer.zoom,
            saved_pan: self.ui.viewer.pan,
            panel: None,
            shown: None,
            saved_focus: self.ui.focused,
            from_preview,
        });
        self.ui.focused = panels::viewers::active_panel(self);
        // Nothing else on screen says how to leave. A preview ends it the way it ends itself, so
        // only View ▸ Enter Full Screen gets the hint, which fades like any other toast.
        if !from_preview {
            self.toast = Some((crate::i18n::tr(EXIT_HINT).to_string(), self.now));
        }
    }

    /// Back to the workspace; the window follows in [`Self::sync_full_screen`]. A zoom or pan
    /// changed in Full Screen stays, scaled back down to the panel.
    pub(crate) fn exit_full_screen(&mut self) {
        if let Some(f) = self.full_screen.take() {
            (self.ui.viewer.zoom, self.ui.viewer.pan) = f.panel_view(self.ui.viewer.zoom, self.ui.viewer.pan);
            self.ui.focused = f.saved_focus;
        }
    }

    /// Escape in Full Screen. A preview that started it ends with it.
    pub(crate) fn escape_full_screen(&mut self) {
        if self.full_screen.is_some_and(|f| f.from_preview) {
            self.stop();
        }
        self.exit_full_screen();
    }

    /// The pass draws Full Screen. The mode starts wherever a command runs, also during a pass,
    /// and the window follows after the pass is painted: until then the workspace stays as it is.
    pub(crate) fn full_screen_shown(&self) -> bool {
        self.full_screen.is_some() && self.full_screen_window.is_some_and(|w| !w.waiting)
    }

    /// The window's OS animations stay off around Full Screen: Windows otherwise animates leaving
    /// it, from the bottom of the screen up to maximized. On again two passes after the window
    /// changed, so one was composited without them.
    pub(super) fn full_screen_transitions(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        let pass = ctx.cumulative_pass_nr();
        match self.transitions_off_until {
            None if self.full_screen_window.is_some() => {
                set_window_transitions(frame, false);
                self.transitions_off_until = Some(u64::MAX);
            }
            None => {}
            Some(_) if self.full_screen_window.is_some() => self.transitions_off_until = Some(u64::MAX),
            Some(u64::MAX) => {
                self.transitions_off_until = Some(pass + 2);
                ctx.request_repaint();
            }
            Some(until) if pass >= until => {
                set_window_transitions(frame, true);
                self.transitions_off_until = None;
            }
            Some(_) => ctx.request_repaint(),
        }
    }

    /// Bring the OS window in line with [`Self::full_screen`], and the mode in line with a window
    /// the OS took out of full screen.
    ///
    /// A maximized window on Windows goes full screen first and is un-maximized there, one pass
    /// later, by putting it back on the monitor it already covers. winit keeps the maximized style
    /// on a window that goes full screen from maximized, so the shell draws the taskbar black
    /// instead of hiding it and the window comes back over the taskbar (winit#4756, PhotoCraft's
    /// `screen_mode_commands`). Un-maximizing first, while the window has its title bar, plays the
    /// restore animation over the full screen window. Leaving needs no maximize: winit puts back
    /// the placement it saved on the way in, maximized or not, and the normal size with it.
    pub(crate) fn sync_full_screen(&mut self, ctx: &egui::Context) {
        if self.full_screen.is_some() && !self.full_screen_possible() {
            self.exit_full_screen();
        }
        let (full, maximized, outer, monitor) = ctx.input(|i| {
            let v = i.viewport();
            (v.fullscreen, v.maximized.unwrap_or(false), v.outer_rect, v.monitor_size)
        });
        let is_full = full.unwrap_or(false);
        match (self.full_screen.is_some(), self.full_screen_window) {
            (true, None) => {
                if !is_full {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
                }
                let still_maximized = cfg!(windows) && !is_full && maximized;
                if cfg!(windows) && !is_full {
                    ctx.request_repaint();
                }
                let waiting = cfg!(windows) && full == Some(false);
                self.full_screen_window = Some(FullScreenWindow { was_full_screen: is_full, still_maximized, seen_full_screen: is_full, waiting });
            }
            (true, Some(mut window)) => {
                if window.seen_full_screen && full == Some(false) {
                    self.exit_full_screen();
                    self.full_screen_window = None;
                } else {
                    window.seen_full_screen |= is_full;
                    window.waiting &= !is_full;
                    let covered = outer.filter(|o| is_full && monitor.is_some_and(|m| (o.size() - m).length() < 1.0));
                    if let Some(screen) = covered.filter(|_| window.still_maximized) {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(false));
                        ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(screen.min));
                        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(screen.size()));
                        window.still_maximized = false;
                    }
                    self.full_screen_window = Some(window);
                }
            }
            (false, Some(window)) => {
                self.full_screen_window = None;
                // Left on the pass before the un-maximize: a window still maximized wouldn't take
                // the saved placement.
                if window.still_maximized {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(false));
                }
                if !window.was_full_screen {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
                }
            }
            (false, None) => {}
        }
    }

    /// The active viewer over the whole window, in place of the workspace.
    pub(super) fn full_screen_area(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        self.hide_dock_area(ui, rect);
        let viewer = panels::viewers::active_panel(self);
        self.dock_rects = vec![(viewer, rect)];
        let builder = egui::UiBuilder::new()
            .max_rect(rect)
            .id_salt(("panel", viewer.id()))
            .ui_stack_info(egui::UiStackInfo::default().with_tag(crate::widgets::PANEL_TAG));
        let mut child = ui.new_child(builder);
        child.set_clip_rect(rect.intersect(ui.clip_rect()));
        panels::show(self, &mut child, viewer, rect);
    }
}
