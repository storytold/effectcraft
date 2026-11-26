//! Software updates, background check at startup, and startup prompt modal, as craft-update's
//! dialogs on the app's [`DialogHost`](craft_ui::dialog::DialogHost).

use crate::EffectcraftApp;

#[cfg(not(target_arch = "wasm32"))]
use craft_ui::dialog::DialogResult;
#[cfg(not(target_arch = "wasm32"))]
use craft_update::stack_dialogs::{CheckChoice, UpdateCheckDialog};
#[cfg(not(target_arch = "wasm32"))]
use craft_update::updater::{Action, Status, Updater};

pub(crate) struct Runtime {
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) updater: Updater,
    /// The desktop app turns the check at start on; tests and the web build leave it off.
    #[cfg(not(target_arch = "wasm32"))]
    startup_check: bool,
    #[cfg(not(target_arch = "wasm32"))]
    check_requested: bool,
    pub(crate) manual_window_open: bool,
    #[cfg(target_arch = "wasm32")]
    pub(crate) status_message: Option<String>,
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
        }
    }
}

impl EffectcraftApp {
    /// Whether the app asks for a newer release when it starts (off until the desktop app turns it on).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn set_startup_update_check(&mut self, on: bool) {
        self.updates_rt.startup_check = on;
    }
}

/// Restart to Update. With unsaved changes the result dialog closes and the window is asked to
/// close, so the unsaved-changes prompt opens. Cancel there leaves the update ready. An open
/// startup prompt keeps its own Restart to Update and save-first note, so the window stays open.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn restart_to_update(rt: &mut Runtime, ctx: &egui::Context, clean: bool) {
    if clean {
        rt.updater.restart_to_update();
        return;
    }
    rt.manual_window_open = false;
    rt.updater.dismiss_status();
    if rt.updater.prompt_open() {
        return;
    }
    // Without a prompt this only takes the close a finished installation queued, so a save after
    // Cancel does not close the app.
    let _ = rt.updater.take_close(true);
    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
}

pub(crate) fn check(app: &mut EffectcraftApp) {
    // A Nightly session is replaced by ArtCraft Launcher, never by the stable release feed: a
    // "restart to update" there would install stable over the nightly build.
    if !crate::channel::stable_services_allowed() {
        app.ui.status = "Updates come with the ArtCraft Launcher's Nightly channel.".into();
        return;
    }
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
            rt.updater.check_now();
        } else if rt.startup_check {
            rt.updater.check_at_start();
        }
        rt.updater.tick(ctx);
    }
    #[cfg(target_arch = "wasm32")]
    let _ = (app, ctx);
}

/// Whether the check result dialog shows this frame. It answers Help ▸ Check for Updates until the
/// requested check ends, and it shows whenever the status is not idle: at the first start after an
/// update, the check at start leaves `Updated` or `InstallFailed`. A check that finds a release
/// leaves the status idle and the startup prompt takes over, so the dialog closes then.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn result_window_open(open: bool, status: &Status, check_requested: bool) -> bool {
    !matches!(status, Status::Idle) || (open && check_requested)
}

/// One pass of the update flow, before the dialog host runs: answers the result dialog closed in
/// the last pass, drives the update prompt, and keeps the result dialog open and current while it
/// answers Help ▸ Check for Updates or the updater has something to report.
pub(crate) fn show(app: &mut EffectcraftApp, ctx: &egui::Context) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        // The restart helper is scheduled before the app closes, so no update starts over unsaved work.
        let clean = !app.session.is_dirty();
        // Answered before the dialog is kept open below, so a dismissed outcome does not open again at once.
        match app.framework_dialogs.take::<UpdateCheckDialog>() {
            Some(DialogResult::Ok(CheckChoice::CheckNow | CheckChoice::CheckAgain)) => app.updates_rt.updater.check_now(),
            Some(DialogResult::Ok(CheckChoice::RestartToUpdate)) => restart_to_update(&mut app.updates_rt, ctx, clean),
            Some(DialogResult::Ok(CheckChoice::Update)) => {
                app.updates_rt.updater.request_install(clean);
            }
            Some(DialogResult::Ok(CheckChoice::Download)) => ctx.open_url(egui::OpenUrl::new_tab(crate::UPDATE_APP.releases_url())),
            // Closing the dialog dismisses the outcome it showed.
            Some(_) => {
                app.updates_rt.manual_window_open = false;
                app.updates_rt.updater.dismiss_status();
            }
            None => {}
        }
        match app.updates_rt.updater.run(&mut app.framework_dialogs, clean) {
            Action::OpenUrl(url) => ctx.open_url(egui::OpenUrl::new_tab(url)),
            Action::Close => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            _ => {}
        }
        let rt = &mut app.updates_rt;
        rt.manual_window_open = result_window_open(rt.manual_window_open, rt.updater.status(), rt.check_requested);
        let shows = rt.manual_window_open;
        let (status, installing, message) = (rt.updater.status().clone(), rt.updater.is_installing(), rt.updater.status_message());
        let dialogs = &mut app.framework_dialogs;
        if shows && !dialogs.is_open::<UpdateCheckDialog>() {
            dialogs.open(UpdateCheckDialog::new(env!("CARGO_PKG_VERSION")));
        } else if !shows && dialogs.is_open::<UpdateCheckDialog>() {
            dialogs.cancel::<UpdateCheckDialog>(ctx);
        }
        if let Some(dialog) = dialogs.get_mut::<UpdateCheckDialog>() {
            dialog.set_status(status);
            dialog.set_installing(installing);
            dialog.set_message(message);
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
