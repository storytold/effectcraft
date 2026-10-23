//! Rendering over files that already exist asks first: the Render button, and anything else that
//! starts a render through the UI (`ui.menu.invoke`, the command palette), lists what exists and
//! offers Replace or Cancel. Agents running `renderQueue.render` with `engine.execute` are never
//! asked: the engine refuses until they pass `overwrite: true`. Footage is never replaced, and the
//! engine says so itself (the item fails with the reason), so there is nothing to ask about it.
//!
//! Automation ids: `dialog.overwrite.replace`, `dialog.overwrite.cancel`. Escape cancels.

use crate::i18n::{tr, tr_args};
use egui::{Color32, vec2};
use serde_json::{Value, json};

use crate::theme::Tokens;
use crate::{Dialog, EffectcraftApp};

/// How many of the outputs the prompt lists before "and N more".
const LISTED: usize = 6;

/// An output that would replace files (`renderQueue.checkOutputs`' `existing`).
#[derive(Clone, Debug, PartialEq)]
pub struct Output {
    pub path: String,
    /// How many files it replaces (a sequence's frames that exist).
    pub files: u64,
    pub sequence: bool,
}

/// The render waiting on the prompt: its params, and the outputs that exist.
#[derive(Clone, Debug, Default)]
pub struct Pending {
    pub command: Option<(Value, Vec<Output>)>,
}

/// The outputs the queue would replace right now (`renderQueue.checkOutputs`' `existing`); `None` when the check fails.
fn current_outputs(app: &mut EffectcraftApp) -> Option<Vec<Output>> {
    let found = app.session.execute("renderQueue.checkOutputs", json!({})).ok()?;
    let existing = found["existing"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| {
            Some(Output { path: e["path"].as_str()?.to_string(), files: e["files"].as_u64().unwrap_or(1), sequence: e["sequence"].as_bool().unwrap_or(false) })
        })
        .collect();
    Some(existing)
}

/// Whether `id` needs the prompt first. When it does, the prompt opens holding the command and
/// the caller must not run it (it runs once the user answers Replace).
pub fn guard(app: &mut EffectcraftApp, id: &str, params: &Value) -> bool {
    if id != "renderQueue.render" || params.get("overwrite").and_then(Value::as_bool) == Some(true) || !app.session.is_enabled(id) {
        return false;
    }
    let Some(existing) = current_outputs(app).filter(|existing| !existing.is_empty()) else { return false };
    app.dialog_state.overwrite.command = Some((params.clone(), existing));
    app.dialog = Some(Dialog::Overwrite);
    true
}

/// The sentence that asks, and whether it speaks of one file.
fn question(outputs: &[Output]) -> (String, bool) {
    match outputs {
        [only] if !only.sequence => {
            let path = std::path::Path::new(&only.path);
            let name = path.file_name().map_or_else(|| only.path.clone(), |n| n.to_string_lossy().into_owned());
            let folder = path.parent().map(|p| p.to_string_lossy().into_owned()).filter(|p| !p.is_empty()).unwrap_or_else(|| ".".into());
            (tr_args("“{}” already exists in “{}”. Do you want to replace it?", &[&name, &folder]), true)
        }
        _ => {
            let files: u64 = outputs.iter().map(|o| o.files).sum();
            (tr_args("{} files that this render writes already exist. Do you want to replace them?", &[&files]), false)
        }
    }
}

pub fn show(app: &mut EffectcraftApp, ctx: &egui::Context, t: &Tokens) {
    let Some((params, outputs)) = app.dialog_state.overwrite.command.clone() else {
        app.dialog = None;
        return;
    };
    let (text, one) = question(&outputs);
    let mut answer: Option<bool> = None;
    let title = if one { "Replace Existing File?" } else { "Replace Existing Files?" };
    super::dialogs::modal_vertical(ctx, title, vec2(520.0, if one { 170.0 } else { 230.0 }), t, |ui| {
        ui.label(egui::RichText::new(text).color(t.tab_text_active));
        if !one {
            ui.add_space(6.0);
            for o in outputs.iter().take(LISTED) {
                let line = if o.sequence { tr_args("{} ({} frames)", &[&o.path, &o.files]) } else { o.path.clone() };
                ui.label(egui::RichText::new(line).font(Tokens::ui(11.5)).color(t.text_dim));
            }
            if let Some(more) = outputs.len().checked_sub(LISTED).filter(|m| *m > 0) {
                ui.label(egui::RichText::new(tr_args("and {} more", &[&more])).font(Tokens::ui(11.5)).color(t.text_dim));
            }
        }
        ui.add_space(4.0);
        ui.label(egui::RichText::new(tr(if one { "Replacing it can't be undone." } else { "Replacing them can't be undone." })).color(t.text_dim).size(11.0));
        ui.add_space(18.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // Replacing loses the earlier files: red, like Delete.
            let r = ui.add(egui::Button::new(egui::RichText::new(tr("  Replace  ")).color(Color32::WHITE)).fill(t.danger));
            app.auto.add("dialog.overwrite.replace", r.rect, "Replace");
            if r.clicked() {
                answer = Some(true);
            }
            let r = ui.button(tr("Cancel"));
            app.auto.add("dialog.overwrite.cancel", r.rect, "Cancel");
            if r.clicked() {
                answer = Some(false);
            }
        });
    });
    if answer.is_none() && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        answer = Some(false);
    }
    let Some(replace) = answer else { return };
    app.dialog = None;
    app.dialog_state.overwrite.command = None;
    if replace {
        // The answer goes with this request, so it can't carry over to a later render.
        let mut params = if params.is_object() { params } else { json!({}) };
        // Replace answers for the files this prompt listed. If the queue's outputs changed while it was open (an agent
        // can run `renderQueue.setOutput` over the control channel), the answer does not reach the new ones: the prompt
        // asks again about what is there now.
        match current_outputs(app) {
            Some(now) if now == outputs => params["overwrite"] = json!(true),
            Some(now) if !now.is_empty() => {
                app.dialog_state.overwrite.command = Some((params, now));
                app.dialog = Some(Dialog::Overwrite);
                return;
            }
            // Nothing left to replace, or the check failed: no permission to replace is given, and the engine decides.
            _ => {}
        }
        if let Err(e) = crate::menus::invoke(app, ctx, "renderQueue.render", params) {
            app.ui.status = e;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn out(path: &str, files: u64, sequence: bool) -> Output {
        Output { path: path.into(), files, sequence }
    }

    #[test]
    fn the_question_names_the_file_and_its_folder() {
        let (text, one) = question(&[out("/renders/Main.mp4", 1, false)]);
        assert!(one);
        assert!(text.contains("“Main.mp4”") && text.contains("“/renders”") && text.contains("replace it"), "{text}");
        let (text, one) = question(&[out("Main.mp4", 1, false)]);
        assert!(one && text.contains("“.”"), "a bare name is in the working folder: {text}");
        let (text, one) = question(&[out("/a/x.mp4", 1, false), out("/a/frames_[#####].png", 3, true)]);
        assert!(!one && text.contains("4 files"), "{text}");
        let (text, one) = question(&[out("/a/frames_[#####].png", 2, true)]);
        assert!(!one && text.contains("2 files"), "a sequence is listed even when it is the only output: {text}");
    }
}
