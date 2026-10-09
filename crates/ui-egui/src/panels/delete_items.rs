//! Deleting Project items that compositions use asks first, as in After Effects: the Delete key
//! and the Project panel's trash button (`project.delete` through the UI) show how many layers in
//! how many compositions go with the items (`project.usage`). Items nothing uses are deleted at
//! once. Agents running `project.delete` with `engine.execute` are never asked, and neither is
//! Delete Project Items Without Confirmation (Shift+Delete in the Project panel).
//!
//! Automation ids: `dialog.deleteItems.delete`, `dialog.deleteItems.cancel`.

use crate::i18n::tr;
use egui::{Color32, vec2};
use serde_json::Value;

use crate::theme::Tokens;
use crate::{Dialog, EffectcraftApp};

/// The deletion waiting on the prompt: its params and what it takes with it.
#[derive(Clone, Debug, Default)]
pub struct Pending {
    pub command: Option<(Value, Value)>,
}

fn plural(n: u64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Whether `id` needs the prompt first. When it does, the prompt opens holding the command and
/// the caller must not run it (it runs once the user answers Delete).
pub fn guard(app: &mut EffectcraftApp, id: &str, params: &Value) -> bool {
    if id != "project.delete" {
        return false;
    }
    let Ok(usage) = app.session.execute("project.usage", params.clone()) else { return false };
    if usage["layers"].as_u64().unwrap_or(0) == 0 {
        return false;
    }
    app.dialog_state.delete_items.command = Some((params.clone(), usage));
    app.dialog = Some(Dialog::DeleteItems);
    true
}

pub fn show(app: &mut EffectcraftApp, ctx: &egui::Context, t: &Tokens) {
    let Some((params, usage)) = app.dialog_state.delete_items.command.clone() else {
        app.dialog = None;
        return;
    };
    let n = |k: &str| usage[k].as_u64().unwrap_or(0);
    let mut answer: Option<bool> = None;
    super::dialogs::modal(ctx, "Delete Items", vec2(460.0, 160.0), t, |ui| {
        let text = format!(
            "Delete {}? {} in {} {} {} and will be deleted too.",
            plural(n("items"), "item", "items"),
            plural(n("layers"), "layer", "layers"),
            plural(n("comps"), "composition", "compositions"),
            if n("layers") == 1 { "uses" } else { "use" },
            if n("items") == 1 { "it" } else { "them" },
        );
        ui.label(egui::RichText::new(text).color(t.tab_text_active));
        ui.add_space(4.0);
        ui.label(egui::RichText::new(tr("You can undo this.")).color(t.text_dim).size(11.0));
        ui.add_space(18.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let r = ui.add(egui::Button::new(egui::RichText::new(tr("  Delete  ")).color(Color32::WHITE)).fill(t.danger));
            app.auto.add("dialog.deleteItems.delete", r.rect, "Delete");
            if r.clicked() {
                answer = Some(true);
            }
            let r = ui.button(tr("Cancel"));
            app.auto.add("dialog.deleteItems.cancel", r.rect, "Cancel");
            if r.clicked() {
                answer = Some(false);
            }
        });
    });
    if answer.is_none() && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        answer = Some(false);
    }
    let Some(delete) = answer else { return };
    app.dialog = None;
    app.dialog_state.delete_items.command = None;
    if delete {
        // An error is already in the status bar.
        let _ = delete_confirmed(app, ctx, params);
    }
}

/// Run `project.delete` with `params` without the prompt: this request is already confirmed (the
/// prompt's Delete, or Delete Project Items Without Confirmation). The confirmation goes with the
/// request, so it can't carry over to a later deletion.
pub fn delete_confirmed(app: &mut EffectcraftApp, ctx: &egui::Context, params: Value) -> Result<Value, String> {
    crate::menus::run_engine(app, ctx, "project.delete", params)
}
