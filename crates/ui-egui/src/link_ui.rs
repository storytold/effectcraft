//! Craft Link in the UI: the `Back to FilmCraft` header button and the `app.activate` control
//! method. Logic is in `effectcraft-link`. The web build has none of it: `craft-ipc` uses local
//! sockets.

#[cfg(not(target_arch = "wasm32"))]
pub use native::{activate, back_button, close_step};
#[cfg(target_arch = "wasm32")]
pub use web::{activate, back_button, close_step};

/// Where a window close stands. One value lives in the app.
#[derive(Clone, Copy, Debug, Default)]
pub struct CloseState {
    /// The window stays open while the result for FilmCraft renders.
    pub rendering: bool,
    /// The next close request goes through without a render: the render finished. Only the
    /// native build renders a result.
    #[cfg(not(target_arch = "wasm32"))]
    pub pass: bool,
    /// The user closed a second time to skip the render. The close requests that follow go
    /// through while the unsaved-changes prompt of that close decides, and once it is confirmed.
    /// A prompt that ends without closing ends the skip too, so the next close renders again.
    #[cfg(not(target_arch = "wasm32"))]
    pub skipped: bool,
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::sync::Arc;

    use serde_json::{Value, json};

    use crate::EffectcraftApp;
    use crate::i18n::tr;
    use crate::theme::Tokens;
    use crate::widgets;

    /// Register `link.back` once, then keep its enablement in step with the active comp.
    fn sync(app: &mut EffectcraftApp, ctx: &egui::Context) {
        if !effectcraft_link::back_installed(&app.session) {
            let backend = Arc::new(effectcraft_link::EguiActivation::new(ctx.clone()));
            if let Err(err) = effectcraft_link::install_back_command(&mut app.session, None, backend) {
                app.ui.status = crate::i18n::tr_args("Back to FilmCraft is not available: {}", &[&err]);
                return;
            }
        }
        effectcraft_link::sync_back_enabled(&app.session);
    }

    /// `Back to FilmCraft`, drawn like the other text header buttons, left of `rx`.
    /// Nothing is drawn unless the active comp is bound to a link. Returns the next `rx`.
    pub fn back_button(app: &mut EffectcraftApp, ui: &mut egui::Ui, p: &egui::Painter, rx: f32, cy: f32) -> f32 {
        sync(app, ui.ctx());
        if !effectcraft_link::back_enabled(&app.session) {
            return rx;
        }
        // The click drew "Rendering..." last frame. The render blocks this thread, so it runs now.
        let pending = egui::Id::unique("header.link.back.pending");
        if ui.data_mut(|d| d.remove_temp::<bool>(pending)).unwrap_or(false) {
            run_back(app);
        }
        let label = tr("Back to FilmCraft");
        let tokens = app.tokens;
        let galley = p.layout_no_wrap(label.to_string(), Tokens::medium(12.0), tokens.text);
        let rect = egui::Rect::from_min_max(egui::pos2(rx - galley.size().x - 16.0, cy - 13.0), egui::pos2(rx, cy + 13.0));
        let resp = widgets::text_button(ui, rect, label, false, &tokens, egui::Id::unique("header.link.back"));
        app.auto.add("header.link.back", rect, "Back to FilmCraft");
        if resp.clicked() {
            app.ui.status = tr("Rendering the result for FilmCraft...").to_string();
            ui.data_mut(|d| d.insert_temp(pending, true));
            ui.ctx().request_repaint();
        }
        rect.min.x - 6.0
    }

    /// Run the command and tell the user what FilmCraft answered. An error says why and keeps the user here.
    /// The text goes to the status line, which stays until it is clicked, so a refusal cannot pass unseen.
    fn run_back(app: &mut EffectcraftApp) {
        app.ui.status = match app.session.execute(effectcraft_link::BACK_COMMAND, json!({})) {
            Ok(done) => match done.get("activated") {
                Some(Value::Bool(true)) => tr("Result sent to FilmCraft.").to_string(),
                // The OS was asked and nobody confirmed it. Do not say the window came forward.
                Some(Value::String(text)) if text == "requested" => tr("Result sent to FilmCraft. Click its window if it stays behind.").to_string(),
                // `false`: no raise happened. FilmCraft asked for attention instead.
                _ => tr("Result sent to FilmCraft. It did not come to the front. Switch to its window.").to_string(),
            },
            Err(err) => err.to_string(),
        };
    }

    /// The window was asked to close. Returns true while this holds the window open.
    ///
    /// A comp bound to a link owes FilmCraft its result movie. The first request writes the link
    /// projects and, when a result is missing or older than the edits, renders it in the
    /// background with "Rendering result for FilmCraft..." on the status line, then closes. A
    /// second request while that runs cancels the render and closes. Either way the link project
    /// is on disk first, so the next `link.attach` shows the edits.
    pub fn close_step(app: &mut EffectcraftApp, ctx: &egui::Context) -> bool {
        use effectcraft_link::Closing;
        if ctx.input(|i| i.viewport().close_requested()) {
            if std::mem::take(&mut app.link_close.pass) {
                return false;
            }
            if app.link_close.skipped {
                let unsaved = &app.dialog_state.unsaved;
                if unsaved.quitting || app.dialog == Some(crate::Dialog::UnsavedChanges) {
                    return false;
                }
                app.link_close.skipped = false;
            }
            if app.link_close.rendering {
                app.link_close.rendering = false;
                app.link_close.skipped = true;
                effectcraft_link::live::abort_close(&mut app.session);
                app.ui.status = tr("Closed without the result for FilmCraft.").to_string();
                return false;
            }
            if effectcraft_link::live::begin_close(&mut app.session) == Closing::Ready {
                return false;
            }
            app.link_close.rendering = true;
            app.ui.status = tr("Rendering result for FilmCraft...").to_string();
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.request_repaint();
            return true;
        }
        if !app.link_close.rendering {
            return false;
        }
        if effectcraft_link::live::close_state(&mut app.session) == Closing::Rendering {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
            return true;
        }
        app.link_close.rendering = false;
        app.link_close.pass = true;
        let _ = effectcraft_link::live::flush_sidecars(&app.session);
        app.ui.status = String::new();
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        true
    }

    /// Control method `app.activate`: bring this window forward, as FilmCraft asked.
    pub fn activate(app: &mut EffectcraftApp, ctx: &egui::Context, params: &Value) -> Value {
        let backend = effectcraft_link::EguiActivation::new(ctx.clone());
        // A link the session binds opens its comp. That comp is the point of the hand-off, so the
        // Home screen must not stay over it (as for `link.open` and `link.attach`).
        let opens = effectcraft_link::ActivateRequest::parse(params)
            .ok()
            .and_then(|request| request.open_link)
            .is_some_and(|link_id| app.session.project.craft_links.contains_key(&link_id));
        let reply = effectcraft_link::handle_activate(&mut app.session, &backend, params);
        if opens && reply.get("ok") == Some(&Value::Bool(true)) {
            app.ui.start_screen = false;
        }
        reply
    }
}

#[cfg(target_arch = "wasm32")]
mod web {
    use serde_json::{Value, json};

    use crate::EffectcraftApp;

    pub fn back_button(_app: &mut EffectcraftApp, _ui: &mut egui::Ui, _p: &egui::Painter, rx: f32, _cy: f32) -> f32 {
        rx
    }

    pub fn close_step(_app: &mut EffectcraftApp, _ctx: &egui::Context) -> bool {
        false
    }

    pub fn activate(_app: &mut EffectcraftApp, _ctx: &egui::Context, _params: &Value) -> Value {
        json!({"ok": false, "error": {"code": "not_ready", "message": "app.activate is not available in the web build", "data": {}}})
    }
}
