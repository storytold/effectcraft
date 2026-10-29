use super::clipboard::copy_image;
use super::window::fit_window;
use super::*;

impl EffectcraftApp {
    /// Completed, non-discarded UI passes since launch.
    pub fn ui_passes(&self) -> u64 {
        self.ui_pass_seq
    }

    // ---------------------------------------------------------------- control channel

    pub(crate) fn raise_for_control(&mut self, ctx: &egui::Context) {
        ctx.request_repaint();
    }

    /// Handle the control channel for this tick: drain queued requests plus previously deferred ones, run each
    /// through `control::handle`, answer `Done` immediately, retry `Retry` until its three-second deadline, hold
    /// input/job/screenshot replies until their event happens and finish job waiters. Retry timeouts and refused
    /// screenshots are answered as `{"ok": false, "error": ...}`; the receiver is kept for the next tick.
    fn drain_control(&mut self, ctx: &egui::Context, now: f64) {
        let Some(rx) = self.control_rx.take() else { return };
        let mut reqs: Vec<(ControlRequest, f64)> = std::mem::take(&mut self.deferred);
        while let Ok(req) = rx.try_recv() {
            reqs.push((req, now + 3.0));
        }
        for (req, deadline) in reqs {
            let reply = req.reply.clone();
            match control::handle(self, ctx, &req, now) {
                control::Outcome::Done(v) => {
                    let _ = reply.send(v);
                }
                control::Outcome::Retry(msg) => {
                    if now < deadline {
                        self.deferred.push((req, deadline));
                        ctx.request_repaint();
                    } else {
                        let _ = reply.send(json!({"ok": false, "error": msg}));
                    }
                }
                control::Outcome::AfterInput => {
                    self.input_waiters.push(reply);
                    // The reply waits for a real UI pass: a hidden window must run one.
                    self.want_paint = true;
                }
                control::Outcome::AwaitJobs(w) => {
                    self.job_waiters.push((w, reply));
                    ctx.request_repaint();
                }
                control::Outcome::Screenshot { path, target } => {
                    // A covered or minimized window presents no frames (macOS skips its redraws), so
                    // a screenshot would never arrive and nothing would tick its timeout. The
                    // hidden-window capture path (a pending screenshot forces a pass in 0.37) needs
                    // a native platform run before this gate is removed; the web build never paints
                    // a hidden tab.
                    if ctx.input(|i| i.viewport().occluded == Some(true) || i.viewport().minimized == Some(true)) {
                        let _ = reply.send(json!({"ok": false, "error": "window is covered or minimized: call ui.focus first (render.frame renders comp pixels without the window)"}));
                        continue;
                    }
                    self.admit_screenshot(ctx, path, target, reply);
                }
            }
        }
        self.control_rx = Some(rx);
        self.finish_job_waiters(ctx);
    }

    /// Reply to the control requests whose offloaded jobs ended.
    fn finish_job_waiters(&mut self, ctx: &egui::Context) {
        if self.job_waiters.is_empty() {
            return;
        }
        self.session.poll_render();
        self.session.poll_offload();
        let waiters = std::mem::take(&mut self.job_waiters);
        for (w, reply) in waiters {
            if w.pending(self) {
                self.job_waiters.push((w, reply));
            } else {
                let v = w.finish(self);
                let _ = reply.send(v);
            }
        }
        if !self.job_waiters.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    // ---------------------------------------------------------------- frame

    /// Apply the engine events drained this tick: open a comp, show a toast, open a URL, run a frontend command,
    /// purge frame caches or copy an image; command and clipboard failures go to `app.ui.status`.
    pub(super) fn handle_events(&mut self, ctx: &egui::Context) {
        for ev in self.session.drain_events() {
            match ev {
                effectcraft_engine::Event::OpenComp(c) => {
                    panels::viewers::on_open_comp(self, c);
                    self.ui.timeline.pps = None;
                }
                effectcraft_engine::Event::Toast { message, .. } => self.toast = Some((message, self.now)),
                effectcraft_engine::Event::OpenUrl(url) => ctx.open_url(egui::OpenUrl::new_tab(url)),
                // Frames are keyed by content: an edit keeps those of comps it doesn't touch.
                effectcraft_engine::Event::ProjectChanged { .. } => {}
                effectcraft_engine::Event::Frontend { command, params } => {
                    if let Err(e) = crate::menus::frontend(self, ctx, &command, params) {
                        self.ui.status = e;
                    }
                }
                effectcraft_engine::Event::PurgeCaches => self.frames.clear(),
                effectcraft_engine::Event::CopyImage { width, height, rgba } => {
                    if let Err(e) = copy_image(ctx, width, height, &rgba) {
                        self.ui.status = e;
                    }
                }
            }
        }
    }

    /// The app's only non-paint entry: one tick of background work.
    ///
    /// This runs for every eframe logic callback, including the logic-only ticks of a hidden
    /// window (which never reach [`Self::draw`]). It must therefore do no input handling, no
    /// layout and no presentation: engine/job queues, autosave, playback, audio, the control
    /// channel, screenshots and the close guard. Everything here is idempotent for a pass that
    /// `Context::request_discard` replays.
    pub fn tick(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        self.now = self.clock.now();
        // The compositor exists before the first paint and while hidden (device-loss handling
        // must run there too).
        self.init_gpu(frame);
        self.apply_gpu_failure(ctx);
        if self.gpu_failure_applied == Some(true) {
            self.close_after_device_loss(ctx);
            return;
        }
        // Focus comes from the fresh viewport copy; a full pass's `InputState::focused` is only a
        // fallback (a logic-only tick leaves it at the previous pass's value).
        let focused = ctx.input(|i| i.viewport().focused.or(Some(i.focused)));
        self.focused = focused.unwrap_or(self.focused);
        // The interface language's fallback fonts are installed in `draw` (fonts need a pass), but
        // commands handled here use the right catalog already.
        let language = crate::i18n::language(self);
        crate::i18n::set_current(language);
        if self.styled_language != language {
            self.want_paint = true;
        }
        // Frames and worker completions wake the app even while it is hidden.
        self.frames.set_context(ctx);
        updates_ui::tick(self, ctx);
        if self.session.render_job.is_some() {
            self.session.poll_render();
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        if self.session.track_job.is_some() {
            self.session.poll_track();
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
        if self.session.mask_job.is_some() {
            self.session.poll_mask_track();
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
        // Background tasks (Content-Aware Fill, Scene Edit Detection, imports).
        if !self.session.tasks.is_empty() {
            self.session.poll_jobs();
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        // Analyses running in a worker (the web app).
        if !self.session.offloaded.is_empty() {
            self.session.poll_offload();
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        // Warp Stabilizer: finish analyses and start queued (re-)analyses in the background.
        if self.session.warp_job.is_some() || !self.session.warp_pending.is_empty() {
            self.session.poll_warp(true);
            self.session.poll_roto(true);
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        // Roto Brush propagation and Object Matte tracking / Freeze: finish jobs (a Freeze writes
        // its mattes) and start queued ones in the background.
        if self.session.roto_job.is_some() || !self.session.roto_pending.is_empty() {
            self.session.poll_roto(true);
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        // 3D Camera Tracker: likewise (tracking, then solving).
        if self.session.camera_job.is_some() || !self.session.camera_pending.is_empty() {
            self.session.poll_camera(true);
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        self.run_script_tasks(ctx);
        self.run_dialog_answers();
        self.handle_events(ctx);
        if let Some(rx) = self.command_inbox.take() {
            while let Ok(id) = rx.try_recv() {
                if let Err(e) = menus::invoke(self, ctx, &id, json!({})) {
                    self.ui.status = e;
                }
            }
            self.command_inbox = Some(rx);
        }
        // A close request (the window's button, Quit, `app.quit`) must be answered while hidden.
        // An answered file dialog can change the project, so the answers above run first. The host
        // calls `panels::unsaved::quit_on_close` last in its `logic`.
        panels::unsaved::on_close_requested(self, ctx);
        let title = panels::unsaved::window_title(self);
        if title != self.window_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.window_title = title;
        }
        self.tick_autosave(ctx);
        self.tick_workspace_autosave(ctx);
        let now = self.now;
        prefs_live::tick(self, ctx, now);
        // Playback advances without a paint (its frames and the audio clock are background work).
        // Idle prefetch keeps its old condition: it only runs once a viewer has drawn.
        if (self.playback.playing || self.last_viewer_scale.is_some())
            && let Some(cid) = self.session.active_comp_id()
        {
            let (scale, active) = (self.last_viewer_scale.unwrap_or(1.0), self.input_active);
            self.advance_playback(ctx, scale, now, active, cid);
        }
        self.drain_control(ctx, now);
        // Screenshot completion, deadlines and issuance are app-side work: the callback only
        // stores pixels and wakes this viewport.
        self.tick_screenshots(ctx);
        // Time-based FPS readout over UI passes (a hidden logic-only tick is not a frame).
        let dt = (now - self.last_time) as f32;
        if dt > 0.0 && dt < 2.0 {
            self.fps = self.fps * 0.9 + (1.0 / dt).min(480.0) * 0.1;
        }
        self.last_time = now;
        // A hidden window paints when work needs a UI pass: queued input and waits, a settled
        // screenshot, or playback into a child viewport.
        let focused = self.focused;
        let child_viewports = !self.ui.dock.layout.floating.is_empty() || prefs_live::video_preview_active(self, focused);
        let paint = self.want_paint
            || !self.synthetic.is_empty()
            || !self.input_waiters.is_empty()
            || self.pending_screenshots.iter().any(|s| s.slot.is_none())
            || (self.playback.playing && child_viewports);
        if paint {
            self.request_hidden_paint(ctx);
        }
    }

    /// Ask the host for a UI pass even when the window is minimized or occluded.
    ///
    /// Native eframe honours `RequestPaintWhileHidden`; the shared web backend does not carry it
    /// yet (a browser does not paint a hidden tab), so the web build only requests a normal
    /// repaint and hidden web screenshots/input remain unqualified.
    fn request_hidden_paint(&mut self, ctx: &egui::Context) {
        self.want_paint = false;
        #[cfg(not(target_arch = "wasm32"))]
        ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaintWhileHidden);
        ctx.request_repaint();
    }

    /// Install the theme and fonts for the current language. Fonts need a real pass: the pass
    /// that installs them draws nothing, and the next pass draws with them (the two-pass warm-up
    /// the app has always had). Never call this from a logic-only tick.
    fn ui_setup(&mut self, ctx: &egui::Context) {
        panels::viewer_text::install_hooks(ctx);
        let language = crate::i18n::language(self);
        crate::i18n::set_current(language);
        if (!self.styled || self.styled_language != language) && self.synthetic_in_flight {
            // The install needs a pass that draws nothing, but this pass has already applied a
            // synthetic batch: defer the theme/font install by one pass instead of dropping the
            // input (feeds are only put into a pass that can draw).
            ctx.request_repaint();
            return;
        }
        if !self.styled || self.styled_language != language {
            theme::install(ctx, &self.tokens, language);
            self.styled_language = language;
            if !self.styled {
                fit_window(ctx);
                self.styled = true;
            }
            self.fonts_ready = false;
            ctx.request_repaint();
        } else if !self.fonts_ready {
            // The pass after the install draws: egui has the new font definitions by now.
            self.fonts_ready = true;
            ctx.request_repaint();
        }
    }

    /// One UI pass: setup, input-driven shortcuts, layout and painting. Called from
    /// `eframe::App::ui` and by tests with the root `Ui`.
    pub fn draw(&mut self, ui: &mut egui::Ui, frame: &eframe::Frame) {
        panels::viewer_text::begin_ui(ui.ctx());
        self.draw_inner(ui, frame);
        // All exits, including fonts/device loss, observe this pass's real custom availability.
        panels::viewer_text::finish_ime(self, ui);
    }

    /// Context creators install these hooks before the first full pass.
    pub fn install_ime_reset_hooks(ctx: &egui::Context) {
        panels::viewer_text::install_hooks(ctx);
    }

    /// Host wrappers call this after their last logic mutation, without replaying UI events.
    pub fn finish_ime_logic(&self, ctx: &egui::Context) {
        panels::viewer_text::finish_logic(self, ctx);
    }

    /// One UI pass after the IME begin hook: close on an applied GPU failure, run `ui_setup`, skip drawing until
    /// fonts are ready, draw the frame, finish pointer wrapping, pump queued wasm frames, record input activity,
    /// publish the completed pass's geometry and pass sequence, acknowledge applied synthetic input and answer
    /// `AfterInput` replies only after a non-discarded pass. Requests repaints while frames are in flight.
    fn draw_inner(&mut self, ui: &mut egui::Ui, frame: &eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.gpu_failure_applied == Some(true) {
            self.close_after_device_loss(&ctx);
            return;
        }
        self.ui_setup(&ctx);
        if !self.fonts_ready {
            // A pass that installs fonts draws nothing. No synthetic batch is in flight here (the
            // hook feeds input only to a ready app), and events egui already applied are never
            // fed again: that would double the pointer/key state of one click.
            ctx.request_repaint();
            return;
        }
        self.frame(ui);
        #[cfg(not(target_arch = "wasm32"))]
        pointer_wrap::finish_native(&ctx, frame);
        // wasm32: no frame threads; render queued frames now, between UI frames.
        if cfg!(target_arch = "wasm32") && self.frames.pump(std::time::Duration::from_millis(if self.playback.playing { 24 } else { 40 })) {
            ctx.request_repaint();
        }
        // Input belongs to this known UI phase: record it for the next tick.
        let active = ctx.input(|i| !i.events.is_empty() || i.pointer.any_down() || i.pointer.is_moving());
        self.input_active = active;
        self.last_pointer_down = ctx.input(|i| i.pointer.any_down());
        // Geometry of a completed pass (a discarded pass is laid out again; only the final one
        // describes the surface the next capture will see). Window position is irrelevant.
        if !ctx.will_discard() {
            let rect = ctx.content_rect();
            let ppp = ctx.pixels_per_point();
            let changed = self.draw_geom.is_none_or(|g| g.rect.size() != rect.size() || g.ppp != ppp);
            if changed {
                self.geometry_epoch = self.geometry_epoch.wrapping_add(1);
            }
            self.draw_geom = Some(DrawGeometry { rect, ppp, epoch: self.geometry_epoch });
            self.ui_pass_seq = self.ui_pass_seq.saturating_add(1);
        }
        // A batch is acknowledged once a UI pass applied it, even if layout was later discarded.
        self.mark_synthetic_applied();
        // `AfterInput` replies wait for the final (non-discarded) pass that applied the last
        // synthetic event: a hidden request is answered only after its forced pass, and a pass
        // egui throws away cannot produce an early success reply.
        if !ctx.will_discard() && self.synthetic.is_empty() {
            for w in self.input_waiters.drain(..) {
                let _ = w.send(json!({"ok": true, "result": null}));
            }
        } else if !self.input_waiters.is_empty() {
            self.want_paint = true;
            ctx.request_repaint();
        }
        if self.frames.inflight() > 0 {
            ctx.request_repaint_after(std::time::Duration::from_millis(30));
        }
    }

    /// The panels, menus, dialogs and overlays of one pass.
    fn frame(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        // Only a viewer that draws in *this* pass records a scale: a closed viewer, the Home
        // screen or another comp must stop idle prefetch at a stale scale.
        self.last_viewer_scale = None;
        self.apply_prefs(&ctx);
        // Video Preview is a second viewport: only a real pass can update it.
        prefs_live::frame(self, &ctx);
        self.auto.begin_frame();
        let tab_had_widget_focus = ctx.egui_wants_keyboard_input();
        menus::handle_shortcuts(self, &ctx);
        if !ctx.input(|i| i.pointer.primary_down()) {
            self.ui.viewer.property_interacting = false;
        }
        let t = self.tokens;
        let full = ui.max_rect();
        if self.dialog.is_some() {
            ui.disable();
        }
        ui.painter().rect_filled(full, 0.0, t.app_bg);
        let mut top = full.min.y;
        if self.ui.show_menu_bar {
            let mb = egui::Rect::from_min_size(full.min, egui::vec2(full.width(), 24.0));
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(mb));
            child.painter().rect_filled(mb, 0.0, t.header_bg);
            menus::menu_bar(self, &mut child);
            top = mb.max.y;
        }
        let header_h = 40.0;
        let header = egui::Rect::from_min_size(egui::pos2(full.min.x, top), egui::vec2(full.width(), header_h));
        header::show(self, ui, header);
        let body = egui::Rect::from_min_max(egui::pos2(full.min.x + 4.0, header.max.y + 2.0), egui::pos2(full.max.x - 4.0, full.max.y - 4.0));
        self.dock_area(ui, body);
        #[cfg(not(target_arch = "wasm32"))]
        prefs_live::dropped_files(self, ui, full);
        if !tab_had_widget_focus {
            menus::handle_tab_shortcut(self, &ctx);
        }
        panels::precomp::mini_flowchart(self, &ctx);
        panels::home::capture_thumbnail(self);
        panels::dialogs::show(self, &ctx);
        updates_ui::show(self, &ctx);
        panels::scriptui_view::sync_panels(self);
        panels::scriptui_view::show_windows(self, &ctx);
        panels::learn::coach(self, &ctx);
        let lift = panels::media_panels::import_card(self, ui, full);
        self.draw_toast(ui, full, lift);
    }

    /// Workspace-layout auto-save: a background write that also runs while hidden, once the
    /// pointer has lifted.
    fn tick_workspace_autosave(&mut self, ctx: &egui::Context) {
        if self.session.config.is_none() || !self.workspace_store.has_changes(&self.ui) || self.last_pointer_down {
            return;
        }
        if self.workspace_store.due(self.now) {
            self.save_workspace_state();
        }
        if self.workspace_store.has_changes(&self.ui) {
            ctx.request_repaint_after(std::time::Duration::from_secs(1));
        }
    }

    pub(super) fn save_workspace_state(&mut self) {
        if let Err(error) = self.workspace_store.save(self.session.config.as_deref(), &self.ui) {
            if self.workspace_store.error.as_ref() != Some(&error) {
                log::warn!("{error}");
                self.ui.status = error.clone();
            }
            self.workspace_store.error = Some(error);
        }
    }

    /// The status message or the latest toast, bottom left (`lift` points higher while the
    /// Importing card is there).
    fn draw_toast(&mut self, ui: &mut egui::Ui, full: egui::Rect, lift: f32) {
        let now = self.now;
        let msg = if !self.ui.status.is_empty() {
            Some(self.ui.status.clone())
        } else {
            self.toast.as_ref().filter(|(_, at)| now - at < 4.0).map(|(m, _)| m.clone())
        };
        if let Some(m) = msg {
            let t = &self.tokens;
            let galley = ui.painter().layout_no_wrap(m.clone(), Tokens::ui(12.0), t.text);
            let r = egui::Rect::from_min_size(egui::pos2(full.min.x + 16.0, full.max.y - 44.0 - lift), galley.size() + egui::vec2(24.0, 14.0));
            ui.painter().rect_filled(r, 6.0, t.panel_bg);
            ui.painter().rect_stroke(r, 6.0, egui::Stroke::new(1.0, t.field_border), egui::StrokeKind::Inside);
            ui.painter().galley(r.min + egui::vec2(12.0, 7.0), galley, t.text);
            self.auto.add("toast", r, &m);
            let resp = ui.interact(r, egui::Id::unique("toast"), egui::Sense::click());
            if resp.clicked() {
                self.ui.status.clear();
                self.toast = None;
            }
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
            if !self.ui.status.is_empty() {
                let since: f64 = ui.data(|d| d.get_temp(egui::Id::unique("status-since")).unwrap_or(now));
                ui.data_mut(|d| d.insert_temp(egui::Id::unique("status-since"), since));
                if now - since > 5.0 {
                    self.ui.status.clear();
                    ui.data_mut(|d| d.remove::<f64>(egui::Id::unique("status-since")));
                }
            }
        }
    }
}

impl EffectcraftApp {
    /// Scripts' `app.scheduleTask` tasks that are due (repeating ones keep a ScriptUI panel
    /// fresh): errors as a toast, the last line printed in the status bar, and a repaint when
    /// the next one is due.
    fn run_script_tasks(&mut self, ctx: &egui::Context) {
        if self.session.script_ui.task_hosts.is_empty() {
            return;
        }
        let previous_comp = self.session.active_comp_id();
        let Some(r) = effectcraft_engine::scriptui::ScriptUi::run_tasks(&mut self.session) else { return };
        if self.session.active_comp_id().is_some() && self.session.active_comp_id() != previous_comp {
            self.ui.start_screen = false;
        }
        if let Some(msg) = r["errors"].as_array().and_then(|e| e.first()).and_then(|e| e["message"].as_str()) {
            self.toast = Some((format!("Scheduled script task: {msg}"), self.now));
        } else if let Some(line) = r["output"].as_array().and_then(|o| o.last()).and_then(|l| l.as_str()) {
            self.ui.status = line.to_string();
        }
        if let Some(next) = r["next"].as_u64() {
            ctx.request_repaint_after(std::time::Duration::from_millis(next));
        }
    }

    /// Auto-save a dirty project when the interval has passed (Settings ▸ Project ▸ Auto-Save).
    fn tick_autosave(&mut self, ctx: &egui::Context) {
        // Monotonic app time: a wall-clock jump must not skip or repeat an auto-save.
        let now = self.now;
        if let Some(Err(e)) = self.session.autosave_tick(now) {
            self.ui.status = e.to_string();
        }
        if self.session.prefs.auto_save.enabled {
            ctx.request_repaint_after(std::time::Duration::from_secs(30));
        }
        // File ▸ Watch Folder: look for new projects every 10 seconds.
        if self.session.state.watch_folder.is_some() {
            let key = egui::Id::unique("watchFolder.lastPoll");
            let last: f64 = ctx.data(|d| d.get_temp(key)).unwrap_or(0.0);
            if now - last >= 10.0 {
                ctx.data_mut(|d| d.insert_temp(key, now));
                match self.session.execute("file.watchFolder.poll", serde_json::json!({})) {
                    Ok(r) => {
                        if let Some(n) = r["rendered"].as_array().map(Vec::len).filter(|n| *n > 0) {
                            self.ui.status = format!("Watch Folder: rendered {n} project(s)");
                        }
                    }
                    Err(e) => self.ui.status = e.to_string(),
                }
            }
            ctx.request_repaint_after(std::time::Duration::from_secs(10));
        }
        self.tick_footage_reload(ctx, now);
    }

    /// Settings ▸ Import ▸ Reload Footage Changed on Disk: every two seconds, and as soon as the
    /// window comes back to the front (after saving in another app), stamp the footage files on a
    /// worker thread and reload the ones that changed.
    fn tick_footage_reload(&mut self, ctx: &egui::Context, now: f64) {
        use effectcraft_engine::footage_reload;
        if let Some(Err(e)) = footage_reload::poll_scan(&mut self.session) {
            self.ui.status = e.to_string();
        }
        let (last_key, focus_key) = (egui::Id::unique("footageReload.lastScan"), egui::Id::unique("footageReload.focused"));
        let focused = self.focused;
        let was_focused: bool = ctx.data(|d| d.get_temp(focus_key)).unwrap_or(focused);
        ctx.data_mut(|d| d.insert_temp(focus_key, focused));
        if self.session.prefs.import.auto_reload_footage == "off" || self.session.project.items.is_empty() {
            return;
        }
        let last: f64 = ctx.data(|d| d.get_temp(last_key)).unwrap_or(0.0);
        if now - last >= 2.0 || (focused && !was_focused) {
            ctx.data_mut(|d| d.insert_temp(last_key, now));
            footage_reload::start_scan(&mut self.session);
        }
        if self.session.footage_scan.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        } else {
            ctx.request_repaint_after(std::time::Duration::from_secs(2));
        }
    }
}
