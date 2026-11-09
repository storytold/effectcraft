//! Software updates, background check at startup, and startup prompt modal.

use crate::EffectcraftApp;

#[cfg(not(target_arch = "wasm32"))]
use craft_update::dialog::{CheckResultDialog, CheckResultResponse};
#[cfg(not(target_arch = "wasm32"))]
use craft_update::updater::{Action, Status, Updater};

pub(crate) struct Runtime {
    #[cfg(not(target_arch = "wasm32"))]
    updater: Updater,
    /// The desktop app turns the check at start on; tests and the web build leave it off.
    #[cfg(not(target_arch = "wasm32"))]
    startup_check: bool,
    #[cfg(not(target_arch = "wasm32"))]
    check_requested: bool,
    pub(crate) manual_window_open: bool,
    #[cfg(target_arch = "wasm32")]
    pub(crate) status_message: Option<String>,
    /// How many times the app called [`Runtime::exiting`].
    #[cfg(test)]
    exits: u32,
    /// How many compositions the project held when the app last called [`Runtime::exiting`].
    #[cfg(test)]
    comps_at_exit: Option<usize>,
}

impl Default for Runtime {
    fn default() -> Self {
        Self {
            #[cfg(not(target_arch = "wasm32"))]
            updater: Updater::new(&crate::UPDATE_APP, env!("CARGO_PKG_VERSION")),
            #[cfg(not(target_arch = "wasm32"))]
            startup_check: false,
            #[cfg(not(target_arch = "wasm32"))]
            check_requested: false,
            manual_window_open: false,
            #[cfg(target_arch = "wasm32")]
            status_message: None,
            #[cfg(test)]
            exits: 0,
            #[cfg(test)]
            comps_at_exit: None,
        }
    }
}

impl Runtime {
    /// Starts the helper of an update that waits for the app to exit. Later calls do nothing.
    pub(crate) fn exiting(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        self.updater.start_pending_update();
        #[cfg(test)]
        {
            self.exits += 1;
        }
    }
}

impl EffectcraftApp {
    /// Whether the app asks for a newer release when it starts (off until the desktop app turns it on).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn set_startup_update_check(&mut self, on: bool) {
        self.updates_rt.startup_check = on;
    }

    /// Runs the scripts in `Scripts/Shutdown` with the project closed, then starts the helper of
    /// an update that waits for the app to exit. The desktop app calls this at the end of its
    /// `on_exit`: on macOS the process ends before the app is dropped.
    pub fn run_shutdown_scripts_and_start_update(&mut self) {
        let _ = self.session.execute("file.runShutdownScripts", serde_json::json!({}));
        #[cfg(test)]
        {
            self.updates_rt.comps_at_exit = Some(self.session.project.comps().count());
        }
        self.updates_rt.exiting();
    }
}

pub(crate) fn check(app: &mut EffectcraftApp) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        app.updates_rt.check_requested = true;
        app.updates_rt.manual_window_open = true;
    }
    #[cfg(target_arch = "wasm32")]
    {
        app.updates_rt.manual_window_open = true;
        app.updates_rt.status_message = Some(crate::i18n::tr("Update check is not supported in the web browser.").to_string());
    }
}

pub(crate) fn tick(app: &mut EffectcraftApp, ctx: &egui::Context) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let rt = &mut app.updates_rt;
        if std::mem::take(&mut rt.check_requested) {
            rt.updater.check_now(ctx);
        } else if rt.startup_check {
            rt.updater.check_at_start(ctx);
        }
        rt.updater.tick(ctx);
    }
    #[cfg(target_arch = "wasm32")]
    let _ = (app, ctx);
}

pub(crate) fn show(app: &mut EffectcraftApp, ctx: &egui::Context) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        // The restart helper is scheduled before the app closes, so no update starts over unsaved work.
        let clean = !app.session.is_dirty();
        match app.updates_rt.updater.show_if(ctx, clean) {
            Action::OpenUrl(url) => ctx.open_url(egui::OpenUrl::new_tab(url)),
            Action::Close => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Action::None => {}
        }
        let rt = &mut app.updates_rt;
        if rt.manual_window_open {
            match CheckResultDialog::new(&crate::UPDATE_APP, env!("CARGO_PKG_VERSION")).show(ctx, rt.updater.status()) {
                CheckResultResponse::None => {}
                CheckResultResponse::Close => {
                    rt.manual_window_open = false;
                    rt.updater.dismiss_status();
                }
                CheckResultResponse::CheckAgain => rt.updater.check_now(ctx),
                CheckResultResponse::RestartToUpdate => rt.updater.restart_to_update(),
            }
            // A check that found a release leaves the status idle and the startup prompt takes over.
            if matches!(rt.updater.status(), Status::Idle) && !rt.check_requested {
                rt.manual_window_open = false;
            }
        }
    }

    #[cfg(target_arch = "wasm32")]
    if app.updates_rt.manual_window_open {
        let mut open = true;
        let mut close = false;
        egui::Window::new(crate::i18n::tr("Check for Updates"))
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                if let Some(msg) = &app.updates_rt.status_message {
                    ui.label(msg);
                }
                ui.add_space(8.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(crate::i18n::tr("Close")).clicked() {
                        close = true;
                    }
                });
            });
        if !open || close {
            app.updates_rt.manual_window_open = false;
            app.updates_rt.status_message = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::App as _;

    #[test]
    fn the_update_exit_hook_runs_once_after_the_shutdown_scripts() {
        let mut app = EffectcraftApp::new(effectcraft_engine::Session::default());
        app.session.execute("comp.new", serde_json::json!({"name": "Main", "width": 64, "height": 64})).unwrap();
        app.on_exit();
        assert_eq!(app.updates_rt.exits, 0);
        app.run_shutdown_scripts_and_start_update();
        assert_eq!(app.updates_rt.exits, 1);
        assert_eq!(app.updates_rt.comps_at_exit, Some(0));
    }
}
