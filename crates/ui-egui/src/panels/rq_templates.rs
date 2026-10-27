//! Edit ▸ Templates ▸ Render Settings… / Output Module…: the Defaults (which template new
//! Render Queue items, frames, pre-renders and proxies start from) and the template list
//! (settings of the selected template; Duplicate, Delete, New from a Render Queue item).
//!
//! Every action runs a `renderQueue.*Template*` engine command. Automation ids:
//! `templates.default.<Slot>`, `templates.select`, `templates.duplicate`, `templates.delete`,
//! `templates.newName`, `templates.saveFromItem`, `templates.ok`.

use crate::i18n::{tr, tr_args};
use effectcraft_engine::project::render_templates::{RenderTemplates, TemplateKind, TemplateSlot};
use egui::{Color32, vec2};
use serde_json::{Value, json};

use crate::theme::Tokens;
use crate::{Dialog, EffectcraftApp};

#[derive(Clone, Default)]
struct State {
    output: bool,
    selected: String,
    new_name: String,
}

/// The `egui::Id` of the render-settings / output-module template dialog's state ([`State`]):
/// which kind it edits, the selected template and the name being typed.
fn state_id() -> egui::Id {
    egui::Id::unique("rq-templates-dialog")
}

/// Open the dialog for `kind` (`renderSettings` | `outputModule`).
pub fn open(app: &mut EffectcraftApp, ctx: &egui::Context, kind: &str) {
    let output = TemplateKind::parse(kind) == Some(TemplateKind::OutputModule);
    let t = &app.session.project.render_templates;
    let k = if output { TemplateKind::OutputModule } else { TemplateKind::RenderSettings };
    let selected = t.default_name(k, TemplateSlot::Movie);
    ctx.data_mut(|d| d.insert_temp(state_id(), State { output, selected, new_name: String::new() }));
    app.dialog = Some(Dialog::RenderTemplates);
}

fn names(t: &RenderTemplates, output: bool) -> Vec<String> {
    if output { t.output_module_list().into_iter().map(|m| m.name).collect() } else { t.render_settings_list().into_iter().map(|r| r.name).collect() }
}

pub fn show(app: &mut EffectcraftApp, ctx: &egui::Context, t: &Tokens) {
    let mut st: State = ctx.data(|d| d.get_temp(state_id())).unwrap_or_default();
    let kind = if st.output { "outputModule" } else { "renderSettings" };
    let k = if st.output { TemplateKind::OutputModule } else { TemplateKind::RenderSettings };
    let templates = app.session.project.render_templates.clone();
    let list = names(&templates, st.output);
    if !list.contains(&st.selected) {
        st.selected = list.first().cloned().unwrap_or_default();
    }
    let mut actions: Vec<(&'static str, Value)> = vec![];
    let mut close = false;
    let title = if st.output { "Output Module Templates" } else { "Render Settings Templates" };
    super::dialogs::modal(ctx, title, vec2(600.0, 560.0), t, |ui| {
        ui.label(egui::RichText::new(tr("Defaults")).font(Tokens::semibold(12.5)).color(t.text));
        egui::Grid::new("tpl-defaults").num_columns(2).spacing(vec2(14.0, 6.0)).show(ui, |ui| {
            for slot in TemplateSlot::ALL {
                ui.label(slot.label());
                let cur = templates.default_name(k, slot);
                let mut pick = cur.clone();
                let r = craft_ui::ComboBox::from_id_salt(("tpl-def", slot.label())).selected_text(&cur).width(300.0).show_ui(ui, |ui| {
                    for n in &list {
                        ui.selectable_value(&mut pick, n.clone(), n);
                    }
                });
                app.auto.add(&format!("templates.default.{slot:?}"), r.response.rect, &cur);
                if pick != cur {
                    actions.push(("renderQueue.setTemplateDefault", json!({"kind": kind, "slot": format!("{slot:?}"), "name": pick})));
                }
                ui.end_row();
            }
        });
        ui.add_space(12.0);
        ui.separator();
        ui.label(egui::RichText::new(tr("Settings")).font(Tokens::semibold(12.5)).color(t.text));
        ui.horizontal(|ui| {
            ui.label(tr("Settings Name:"));
            let r = craft_ui::ComboBox::from_id_salt("tpl-select").selected_text(&st.selected).width(300.0).show_ui(ui, |ui| {
                for n in &list {
                    ui.selectable_value(&mut st.selected, n.clone(), n);
                }
            });
            app.auto.add("templates.select", r.response.rect, &st.selected);
        });
        let builtin = RenderTemplates::is_builtin(k, &st.selected)
            && !match k {
                TemplateKind::RenderSettings => templates.render_settings.iter().any(|r| r.name.eq_ignore_ascii_case(&st.selected)),
                TemplateKind::OutputModule => templates.output_modules.iter().any(|m| m.name.eq_ignore_ascii_case(&st.selected)),
            };
        // The selected template's settings.
        let lines: Vec<(&'static str, String)> = if st.output {
            templates.output_module(&st.selected).map(|m| m.describe()).unwrap_or_default()
        } else {
            templates.render_settings(&st.selected).map(|r| r.describe(None)).unwrap_or_default()
        };
        egui::Frame::new().fill(t.field_bg).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
            ui.set_min_height(180.0);
            ui.set_width(ui.available_width());
            egui::ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
                egui::Grid::new("tpl-lines").num_columns(2).spacing(vec2(12.0, 2.0)).show(ui, |ui| {
                    for (k, v) in &lines {
                        ui.label(egui::RichText::new(*k).color(t.text_dim).size(11.5));
                        ui.label(egui::RichText::new(v).size(11.5));
                        ui.end_row();
                    }
                });
            });
        });
        ui.horizontal(|ui| {
            let r = ui.button(tr("Duplicate"));
            app.auto.add("templates.duplicate", r.rect, "Duplicate");
            if r.clicked() {
                let mut n = format!("{} copy", st.selected);
                let mut i = 2;
                while list.iter().any(|x| x.eq_ignore_ascii_case(&n)) {
                    n = format!("{} copy {i}", st.selected);
                    i += 1;
                }
                actions.push(("renderQueue.saveTemplate", json!({"kind": kind, "name": n, "from": st.selected})));
                st.selected = n;
            }
            let r = ui.add_enabled(!builtin, egui::Button::new(tr("Delete")));
            app.auto.add("templates.delete", r.rect, "Delete");
            if r.clicked() {
                actions.push(("renderQueue.deleteTemplate", json!({"kind": kind, "name": st.selected})));
            }
            if builtin {
                ui.label(egui::RichText::new(tr("built-in")).color(t.text_faint).size(11.0));
            }
        });
        ui.add_space(8.0);
        ui.separator();
        // New template from the selected (or only) Render Queue item.
        let queue = &app.session.project.render_queue;
        ui.horizontal(|ui| {
            ui.label(tr("New from Render Queue item:"));
            let r = ui.add(egui::TextEdit::singleline(&mut st.new_name).hint_text(tr("template name")).desired_width(180.0));
            app.auto.add("templates.newName", r.rect, &st.new_name);
            let item = queue.last().map(|i| i.id);
            let ok = item.is_some() && !st.new_name.trim().is_empty();
            let r = ui.add_enabled(ok, egui::Button::new(tr_args("Save from #{}", &[&(queue.len().max(1))])));
            app.auto.add("templates.saveFromItem", r.rect, "Save from item");
            if r.clicked()
                && let Some(id) = item
            {
                actions.push(("renderQueue.saveTemplate", json!({"kind": kind, "name": st.new_name.trim(), "item": id})));
                st.selected = st.new_name.trim().to_string();
                st.new_name.clear();
            }
        });
        ui.add_space(12.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let r = ui.add(egui::Button::new(egui::RichText::new(tr("   OK   ")).color(Color32::WHITE)).fill(t.accent));
            app.auto.add("templates.ok", r.rect, "OK");
            close = r.clicked();
        });
    });
    for (id, p) in actions {
        if let Err(e) = app.session.execute(id, p) {
            app.ui.status = e.to_string();
        }
    }
    ctx.data_mut(|d| d.insert_temp(state_id(), st));
    if close || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.dialog = None;
    }
}
