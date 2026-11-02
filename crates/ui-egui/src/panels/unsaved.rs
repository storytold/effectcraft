//! The unsaved-changes prompt: commands that close the open project (Quit and closing the
//! window, File ▸ New Project / Open / Open Recent / Close Project / Revert, the demo project, a
//! Home template) first ask to save a modified project. Save saves (asking for a path when the
//! project is untitled) and continues; Don't Save continues; Cancel keeps the project open.
//! Revert only asks to discard. Agents running commands with `engine.execute` are never asked.
//!
//! Automation ids: `dialog.unsaved.save`, `dialog.unsaved.dontSave` (Revert: `dialog.unsaved.revert`),
//! `dialog.unsaved.cancel`. Enter answers Save, Cmd+Backspace (Ctrl elsewhere) Don't Save and, in
//! the Revert prompt, Cmd+R Revert.

use crate::i18n::tr;
use egui::{Color32, vec2};
use serde_json::{Value, json};

use crate::theme::Tokens;
use crate::{Dialog, EffectcraftApp};

/// Commands that replace or close the open project (Quit is asked when the window closes).
const CLOSING: &[&str] = &[
    "file.newProject",
    "file.open",
    "file.openRecent",
    "file.closeProject",
    "file.revert",
    "file.openDemoProject",
    "templates.create",
    "file.runShutdownScripts",
];

/// The command waiting on the prompt, and whether the user already answered it.
#[derive(Clone, Debug, Default)]
pub struct Pending {
    pub command: Option<(String, Value)>,
    /// The next guarded command runs without asking (the user answered Save or Don't Save).
    pub answered: bool,
    /// The window closes without asking (the user answered the Quit prompt).
    pub quitting: bool,
}

/// Whether `id` needs the prompt first. When it does, the prompt opens holding the command and
/// the caller must not run it (it runs once the user answers).
pub fn guard(app: &mut EffectcraftApp, id: &str, params: &Value) -> bool {
    if !CLOSING.contains(&id) || std::mem::take(&mut app.dialog_state.unsaved.answered) {
        return false;
    }
    ask(app, id, params)
}

fn ask(app: &mut EffectcraftApp, id: &str, params: &Value) -> bool {
    if !app.session.is_dirty() {
        return false;
    }
    app.dialog_state.unsaved.command = Some((id.to_string(), params.clone()));
    app.dialog = Some(Dialog::UnsavedChanges);
    true
}

/// The window's close button (and Quit, which closes it): keep the window open and ask.
pub fn on_close_requested(app: &mut EffectcraftApp, ctx: &egui::Context) {
    if !ctx.input(|i| i.viewport().close_requested()) || app.dialog_state.unsaved.quitting {
        return;
    }
    // Logged, so a window that closes on its own shows it (#234).
    log::info!("the window was asked to close");
    if ask(app, "app.quit", &json!({})) {
        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
    }
}

/// On macOS a close that nothing held open quits through `-[NSApplication terminate:]` instead:
/// closing the window while AppKit's run loop is running crashes on a Mac with a Touch Bar
/// (#341). `on_exit` still runs. A desktop host calls it last in `logic`, after
/// `EffectcraftApp::tick` (which runs [`on_close_requested`]) and its own work. Returns whether
/// that quit was scheduled; other platforms close the window as before.
pub fn quit_on_close(ctx: &egui::Context) -> bool {
    craft_quit::on_close_request(ctx)
}

/// The project's display name: its file name, or `Untitled Project.ecproj`.
pub fn project_name(app: &EffectcraftApp) -> String {
    app.session
        .path
        .as_deref()
        .and_then(|p| std::path::Path::new(p).file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Untitled Project.ecproj".into())
}

/// The window title: the project's path (or name) and `*` while it has unsaved changes.
pub fn window_title(app: &EffectcraftApp) -> String {
    let doc = app.session.path.clone().unwrap_or_else(|| project_name(app));
    format!("EffectCraft - {doc}{}", if app.session.is_dirty() { " *" } else { "" })
}

/// Whether the prompt is Revert Project (Revert / Cancel) instead of Unsaved Changes.
pub(crate) fn is_revert(app: &EffectcraftApp) -> bool {
    app.dialog_state.unsaved.command.as_ref().is_some_and(|(id, _)| id == "file.revert")
}

enum Answer {
    Save,
    Discard,
    Cancel,
}

pub fn show(app: &mut EffectcraftApp, ctx: &egui::Context, t: &Tokens) {
    if app.dialog_state.unsaved.command.is_none() {
        app.dialog = None;
        return;
    }
    let revert = is_revert(app);
    let name = project_name(app);
    let mut answer = None;
    let title = if revert { "Revert Project" } else { "Unsaved Changes" };
    super::dialogs::modal(ctx, title, vec2(460.0, 170.0), t, |ui| {
        let text = if revert {
            format!("Revert “{name}” to the last saved version? Changes made since then will be lost.")
        } else {
            format!("Save changes to “{name}” before closing?")
        };
        ui.label(egui::RichText::new(text).color(t.tab_text_active));
        if !revert {
            ui.add_space(4.0);
            ui.label(egui::RichText::new(tr("Your changes will be lost if you don't save them.")).color(t.text_dim).size(11.0));
        }
        ui.add_space(18.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let (label, key) = if revert { ("  Revert  ", "dialog.unsaved.revert") } else { ("   Save   ", "dialog.unsaved.save") };
            // Revert discards work: red, like Delete.
            let r = ui.add(egui::Button::new(egui::RichText::new(label).color(Color32::WHITE)).fill(if revert { t.danger } else { t.accent }));
            app.auto.add(key, r.rect, label.trim());
            if r.clicked() {
                answer = Some(if revert { Answer::Discard } else { Answer::Save });
            }
            let r = ui.button(tr("Cancel"));
            app.auto.add("dialog.unsaved.cancel", r.rect, "Cancel");
            if r.clicked() {
                answer = Some(Answer::Cancel);
            }
            if !revert {
                // Set apart from Save, as is customary.
                ui.add_space(ui.available_width() - 90.0);
                let r = ui.button(tr("Don't Save"));
                app.auto.add("dialog.unsaved.dontSave", r.rect, "Don't Save");
                if r.clicked() {
                    answer = Some(Answer::Discard);
                }
            }
        });
    });
    if answer.is_none() && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        answer = Some(Answer::Cancel);
    }
    // Enter is Save, the default button; Cmd+Backspace (Ctrl elsewhere) is Don't Save, as in
    // macOS save prompts. Revert discards work, so it takes neither: Cmd+R.
    if answer.is_none() && !revert && super::dialogs::ui_enter(ctx) {
        answer = Some(Answer::Save);
    }
    let discard = if revert { egui::Key::R } else { egui::Key::Backspace };
    if answer.is_none() && super::dialogs::ui_command_key(ctx, discard) {
        answer = Some(Answer::Discard);
    }
    if let Some(a) = answer {
        answer_with(app, ctx, a);
    }
}

fn answer_with(app: &mut EffectcraftApp, ctx: &egui::Context, a: Answer) {
    app.dialog = None;
    let Some((id, params)) = app.dialog_state.unsaved.command.take() else { return };
    match a {
        Answer::Cancel => return,
        Answer::Save => {
            // An untitled project asks for a path; cancelling that dialog cancels the command.
            if let Err(e) = crate::menus::invoke(app, ctx, "file.save", json!({})) {
                app.ui.status = e;
                return;
            }
            if app.session.is_dirty() {
                return;
            }
        }
        Answer::Discard => {}
    }
    if id == "app.quit" {
        app.dialog_state.unsaved.quitting = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        return;
    }
    app.dialog_state.unsaved.answered = true;
    let before = (app.session.path.clone(), app.session.revision);
    if let Err(e) = crate::menus::invoke(app, ctx, &id, params) {
        app.ui.status = e;
    }
    // Leave Home once something opened from it (not when its file dialog was cancelled).
    if (app.session.path.clone(), app.session.revision) != before {
        app.ui.start_screen = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> (EffectcraftApp, egui::Context) {
        let ctx = egui::Context::default();
        let app = EffectcraftApp::new(effectcraft_engine::Session::default());
        crate::theme::install(&ctx, &app.tokens, crate::i18n::language(&app));
        (app, ctx)
    }

    /// One frame of the dialogs, with `close` = the window's close button pressed; returns
    /// whether the frame asked the window to stay open.
    fn frame(app: &mut EffectcraftApp, ctx: &egui::Context, close: bool) -> bool {
        let mut raw = egui::RawInput::default();
        if close {
            raw.viewports.entry(egui::ViewportId::ROOT).or_default().events.push(egui::ViewportEvent::Close);
        }
        let mut out = ctx.run_ui(raw, |ui| {
            app.auto.begin_frame();
            on_close_requested(app, ui.ctx());
            crate::panels::dialogs::show(app, ui.ctx());
        });
        out.textures_delta.clear();
        out.viewport_output.get(&egui::ViewportId::ROOT).is_some_and(|v| v.commands.contains(&egui::ViewportCommand::CancelClose))
    }

    /// One frame of the dialogs with the window's close button pressed, through
    /// [`on_close_requested`] and then [`quit_on_close`] as the app runs them; returns whether the
    /// close was canceled and whether the platform quit was scheduled.
    fn close_frame(app: &mut EffectcraftApp, ctx: &egui::Context) -> (bool, bool) {
        let mut raw = egui::RawInput::default();
        raw.viewports.entry(egui::ViewportId::ROOT).or_default().events.push(egui::ViewportEvent::Close);
        let mut quits = false;
        let mut out = ctx.run_ui(raw, |ui| {
            app.auto.begin_frame();
            on_close_requested(app, ui.ctx());
            quits = quit_on_close(ui.ctx());
            crate::panels::dialogs::show(app, ui.ctx());
        });
        out.textures_delta.clear();
        let canceled = out.viewport_output.get(&egui::ViewportId::ROOT).is_some_and(|v| v.commands.contains(&egui::ViewportCommand::CancelClose));
        (canceled, quits)
    }

    fn modified(app: &mut EffectcraftApp) {
        app.session.execute("comp.new", json!({"name": "Main", "width": 64, "height": 64})).unwrap();
        assert!(app.session.is_dirty());
    }

    #[test]
    fn shutdown_scripts_prompt_before_discarding_unsaved_work() {
        let (mut app, ctx) = app();
        modified(&mut app);
        let result = crate::menus::invoke(&mut app, &ctx, "file.runShutdownScripts", json!({})).unwrap();
        assert_eq!(result, json!({"dialog": "unsavedChanges"}));
        assert_eq!(app.session.project.comps().count(), 1);
        assert!(app.session.is_dirty());
        answer_with(&mut app, &ctx, Answer::Cancel);
        assert_eq!(app.session.project.comps().count(), 1);
        assert!(app.session.is_dirty());
        crate::menus::invoke(&mut app, &ctx, "file.runShutdownScripts", json!({})).unwrap();
        answer_with(&mut app, &ctx, Answer::Discard);
        assert_eq!(app.session.project.comps().count(), 0);
        assert!(!app.session.is_dirty() && app.dialog.is_none());
    }

    #[test]
    fn closing_a_modified_project_asks_and_cancel_keeps_it() {
        let (mut app, ctx) = app();
        // An unmodified project closes without asking.
        crate::menus::invoke(&mut app, &ctx, "file.newProject", json!({})).unwrap();
        assert!(app.dialog.is_none());
        modified(&mut app);
        assert!(window_title(&app).ends_with("Untitled Project.ecproj *"), "{}", window_title(&app));
        let r = crate::menus::invoke(&mut app, &ctx, "file.newProject", json!({})).unwrap();
        assert_eq!(r, json!({"dialog": "unsavedChanges"}));
        assert_eq!(app.dialog, Some(Dialog::UnsavedChanges));
        assert_eq!(app.session.project.comps().count(), 1, "nothing closed yet");
        frame(&mut app, &ctx, false);
        for id in ["dialog.unsaved.save", "dialog.unsaved.dontSave", "dialog.unsaved.cancel"] {
            assert!(app.auto.find(id).is_some(), "{id}");
        }
        answer_with(&mut app, &ctx, Answer::Cancel);
        assert!(app.dialog.is_none() && app.session.is_dirty());
        assert_eq!(app.session.project.comps().count(), 1);
        // Don't Save runs the command.
        crate::menus::invoke(&mut app, &ctx, "file.newProject", json!({})).unwrap();
        answer_with(&mut app, &ctx, Answer::Discard);
        assert_eq!(app.session.project.comps().count(), 0);
        assert!(!app.session.is_dirty() && app.dialog.is_none());
        assert!(!app.dialog_state.unsaved.answered, "the answer is used once");
        // Agents (`engine.execute`) are never asked.
        modified(&mut app);
        app.session.execute("file.closeProject", json!({})).unwrap();
        assert!(app.dialog.is_none() && app.session.project.comps().count() == 0);
    }

    #[test]
    fn cmd_backspace_is_dont_save_but_not_revert() {
        let press = |app: &mut EffectcraftApp, ctx: &egui::Context| crate::panels::dialogs::press_key(app, ctx, egui::Key::Backspace, egui::Modifiers::COMMAND);
        let (mut app, ctx) = app();
        modified(&mut app);
        crate::menus::invoke(&mut app, &ctx, "file.newProject", json!({})).unwrap();
        assert_eq!(app.dialog, Some(Dialog::UnsavedChanges));
        press(&mut app, &ctx);
        assert!(app.dialog.is_none() && !app.session.is_dirty());
        assert_eq!(app.session.project.comps().count(), 0, "closed without saving");
        // The Revert prompt has no Don't Save.
        let dir = std::env::temp_dir().join(format!("ec-unsaved-key-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        modified(&mut app);
        app.session.execute("file.saveAs", json!({"path": dir.join("Kept.ecproj").to_string_lossy()})).unwrap();
        app.session.execute("layer.newNull", json!({})).unwrap();
        crate::menus::invoke(&mut app, &ctx, "file.revert", json!({})).unwrap();
        press(&mut app, &ctx);
        assert_eq!(app.dialog, Some(Dialog::UnsavedChanges));
        assert_eq!(app.session.active_comp().unwrap().layers.len(), 1, "not reverted");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn enter_is_save_and_cmd_r_is_revert() {
        let press = |app: &mut EffectcraftApp, ctx: &egui::Context| crate::panels::dialogs::press_key(app, ctx, egui::Key::Enter, egui::Modifiers::NONE);
        let (mut app, ctx) = app();
        let dir = std::env::temp_dir().join(format!("ec-unsaved-enter-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("Kept.ecproj");
        modified(&mut app);
        app.session.execute("file.saveAs", json!({"path": path.to_string_lossy()})).unwrap();
        // The Revert prompt discards work: Enter leaves it open.
        app.session.execute("layer.newNull", json!({})).unwrap();
        crate::menus::invoke(&mut app, &ctx, "file.revert", json!({})).unwrap();
        press(&mut app, &ctx);
        assert_eq!(app.dialog, Some(Dialog::UnsavedChanges));
        assert_eq!(app.session.active_comp().unwrap().layers.len(), 1, "not reverted");
        // Cmd+R is Revert.
        crate::panels::dialogs::press_key(&mut app, &ctx, egui::Key::R, egui::Modifiers::COMMAND);
        assert!(app.dialog.is_none());
        assert_eq!(app.session.active_comp().unwrap().layers.len(), 0, "reverted");
        app.session.execute("layer.newNull", json!({})).unwrap();
        // Save: the change is written, then the project closes.
        crate::menus::invoke(&mut app, &ctx, "file.closeProject", json!({})).unwrap();
        press(&mut app, &ctx);
        assert!(app.dialog.is_none());
        assert_eq!(app.session.project.comps().count(), 0, "closed");
        app.session.execute("file.open", json!({"path": path.to_string_lossy()})).unwrap();
        assert_eq!(app.session.active_comp().unwrap().layers.len(), 1, "the null was saved");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn save_saves_then_continues_and_revert_only_asks_to_discard() {
        let (mut app, ctx) = app();
        let dir = std::env::temp_dir().join(format!("ec-unsaved-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("Kept.ecproj");
        modified(&mut app);
        app.session.execute("file.saveAs", json!({"path": path.to_string_lossy()})).unwrap();
        assert!(!app.session.is_dirty());
        assert!(window_title(&app).ends_with("Kept.ecproj"), "{}", window_title(&app));
        // Revert: Revert / Cancel.
        app.session.execute("layer.newNull", json!({})).unwrap();
        crate::menus::invoke(&mut app, &ctx, "file.revert", json!({})).unwrap();
        frame(&mut app, &ctx, false);
        assert!(app.auto.find("dialog.unsaved.revert").is_some() && app.auto.find("dialog.unsaved.save").is_none());
        answer_with(&mut app, &ctx, Answer::Discard);
        assert_eq!(app.session.active_comp().unwrap().layers.len(), 0, "reverted");
        // Save: the change is written, then the project closes.
        app.session.execute("layer.newNull", json!({})).unwrap();
        crate::menus::invoke(&mut app, &ctx, "file.closeProject", json!({})).unwrap();
        answer_with(&mut app, &ctx, Answer::Save);
        assert_eq!(app.session.project.comps().count(), 0, "closed");
        app.session.execute("file.open", json!({"path": path.to_string_lossy()})).unwrap();
        assert_eq!(app.session.active_comp().unwrap().layers.len(), 1, "the null was saved");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_window_stays_open_until_the_quit_prompt_is_answered() {
        let (mut app, ctx) = app();
        assert!(!frame(&mut app, &ctx, true), "an unmodified project quits at once");
        modified(&mut app);
        assert!(frame(&mut app, &ctx, true), "the close is cancelled");
        assert_eq!(app.dialog, Some(Dialog::UnsavedChanges));
        answer_with(&mut app, &ctx, Answer::Discard);
        assert!(app.dialog_state.unsaved.quitting);
        assert!(!frame(&mut app, &ctx, true), "Don't Save lets the window close");
        // Undoing back to the saved state needs no prompt.
        let (mut app, ctx) = self::app();
        modified(&mut app);
        app.session.execute("edit.undo", json!({})).unwrap();
        assert!(!app.session.is_dirty());
        assert!(!frame(&mut app, &ctx, true));
    }

    /// Closing the window while AppKit's run loop is running crashes on a Mac with a Touch Bar
    /// (#341). On macOS the close is canceled and the app quits through
    /// `-[NSApplication terminate:]`. Other platforms close the window.
    #[test]
    fn closing_a_saved_project_quits_through_the_platform_on_macos() {
        let (mut app, ctx) = app();
        let macos = cfg!(target_os = "macos");
        assert_eq!(close_frame(&mut app, &ctx), (macos, macos));
        assert!(app.dialog.is_none(), "nothing to ask");
    }

    #[test]
    fn closing_a_modified_project_asks_before_the_platform_quit() {
        let (mut app, ctx) = app();
        modified(&mut app);
        assert_eq!(close_frame(&mut app, &ctx), (true, false), "the prompt holds the window and nothing quits");
        assert_eq!(app.dialog, Some(Dialog::UnsavedChanges));
        answer_with(&mut app, &ctx, Answer::Cancel);
        assert!(app.session.is_dirty() && !app.dialog_state.unsaved.quitting);
        assert_eq!(close_frame(&mut app, &ctx), (true, false), "asked again after Cancel");
        answer_with(&mut app, &ctx, Answer::Discard);
        let macos = cfg!(target_os = "macos");
        assert_eq!(close_frame(&mut app, &ctx), (macos, macos), "Don't Save quits");
    }

    /// While the window is minimized or hidden, eframe runs `App::logic` without `App::ui` and
    /// still handles a close: `logic` asks about a modified project.
    #[test]
    fn a_close_while_only_logic_runs_asks_about_a_modified_project() {
        let (mut app, ctx) = app();
        modified(&mut app);
        let mut raw = egui::RawInput::default();
        raw.viewports.entry(egui::ViewportId::ROOT).or_default().events.push(egui::ViewportEvent::Close);
        let mut frame = eframe::Frame::_new_kittest();
        let out = ctx.run_logic(&raw, |ctx| eframe::App::logic(&mut app, ctx, &mut frame));
        let canceled = out.viewport_commands.get(&egui::ViewportId::ROOT).is_some_and(|c| c.contains(&egui::ViewportCommand::CancelClose));
        assert!(canceled, "the close is canceled");
        assert_eq!(app.dialog, Some(Dialog::UnsavedChanges));
    }

    /// A file dialog answered in the frame that closes the window runs its command before the
    /// close decision, so a change it makes is asked about.
    #[test]
    fn a_dialog_answer_runs_before_the_close_decision() {
        let (mut app, ctx) = app();
        assert!(!app.session.is_dirty());
        let answer = crate::DialogAnswer { command: "comp.new".into(), key: "name", params: json!({"width": 64, "height": 64}), paths: vec!["Main".into()] };
        app.dialog_answers.0.send(answer).unwrap();
        let mut raw = egui::RawInput::default();
        raw.viewports.entry(egui::ViewportId::ROOT).or_default().events.push(egui::ViewportEvent::Close);
        let mut frame = eframe::Frame::_new_kittest();
        let out = ctx.run_logic(&raw, |ctx| eframe::App::logic(&mut app, ctx, &mut frame));
        let canceled = out.viewport_commands.get(&egui::ViewportId::ROOT).is_some_and(|c| c.contains(&egui::ViewportCommand::CancelClose));
        assert!(app.session.is_dirty(), "the answer ran");
        assert!(canceled, "the close is canceled");
        assert_eq!(app.dialog, Some(Dialog::UnsavedChanges));
    }

    /// The tick makes the close decision and leaves the platform quit to the host, which runs it
    /// last in `logic`, after its own work (the desktop host shows the file dialogs asked for).
    /// On macOS `logic` cancels the close and quits; other platforms close the window.
    #[test]
    fn the_platform_quit_runs_after_the_tick() {
        let (mut app, ctx) = app();
        let mut raw = egui::RawInput::default();
        raw.viewports.entry(egui::ViewportId::ROOT).or_default().events.push(egui::ViewportEvent::Close);
        let frame = eframe::Frame::_new_kittest();
        let canceled =
            |out: &egui::LogicOutput| out.viewport_commands.get(&egui::ViewportId::ROOT).is_some_and(|c| c.contains(&egui::ViewportCommand::CancelClose));
        let out = ctx.run_logic(&raw, |ctx| app.tick(ctx, &frame));
        assert!(!canceled(&out), "the tick leaves a saved project's close to the host");
        let mut frame = eframe::Frame::_new_kittest();
        let out = ctx.run_logic(&raw, |ctx| eframe::App::logic(&mut app, ctx, &mut frame));
        assert_eq!(canceled(&out), cfg!(target_os = "macos"), "logic quits through the platform on macOS");
        assert!(app.dialog.is_none(), "nothing to ask");
    }
}
