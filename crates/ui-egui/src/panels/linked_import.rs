//! The linked-composition importer lists saved source comps without importing their contents.

use crate::i18n::{tr, tr_args};
use crate::theme::Tokens;
use crate::{Dialog, EffectcraftApp};
use egui::{Id, vec2};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Composition {
    id: String,
    name: String,
    width: u32,
    height: u32,
    frame_rate: f64,
    duration: f64,
    already_linked: bool,
    #[serde(skip)]
    selected: bool,
}

impl Composition {
    fn stats(&self) -> String {
        tr_args(
            "{} × {} · {} fps · {} s",
            &[&self.width.to_string(), &self.height.to_string(), &format!("{:.2}", self.frame_rate), &format!("{:.2}", self.duration)],
        )
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Source {
    source_project_id: String,
    compositions: Vec<Composition>,
}

#[derive(Clone, Debug, Default)]
pub struct Draft {
    path: String,
    source_project_id: String,
    compositions: Vec<Composition>,
    error: Option<String>,
    focus_first: bool,
    captured_focus: bool,
    return_focus: Option<Id>,
    restore_pending: Option<Id>,
}

/// Capture focus before opening the file picker or disabling the document behind the modal.
pub fn prepare(app: &mut EffectcraftApp, ctx: &egui::Context) {
    if app.dialog != Some(Dialog::LinkedImport) {
        app.dialog_state.linked_import = Draft { captured_focus: true, return_focus: ctx.memory(|memory| memory.focused()), ..Default::default() };
    }
}

/// Inspect the source through the registered engine command; only Link Selected edits it.
pub fn open(app: &mut EffectcraftApp, path: &str) -> Result<Value, String> {
    let previous = app.dialog_state.linked_import.captured_focus.then(|| app.dialog_state.linked_import.clone());
    let result = app
        .session
        .execute("file.linkableCompositions", json!({"path":path}))
        .map_err(|error| error.to_string())
        .and_then(|result| serde_json::from_value::<Source>(result).map_err(|error| tr_args("Cannot read composition list: {}", &[&error.to_string()])));
    let mut draft = Draft { path: path.into(), focus_first: true, ..Default::default() };
    if let Some(previous) = previous {
        draft.captured_focus = previous.captured_focus;
        draft.return_focus = previous.return_focus;
    }
    match result {
        Ok(source) => {
            draft.source_project_id = source.source_project_id;
            draft.compositions = source.compositions;
        }
        Err(error) => {
            app.ui.status = error.clone();
            draft.error = Some(error);
        }
    }
    let error = draft.error.clone();
    app.dialog_state.linked_import = draft;
    app.dialog = Some(Dialog::LinkedImport);
    match error {
        Some(error) => Err(error),
        None => Ok(json!({"dialog":"file.linkComposition"})),
    }
}

pub fn close(app: &mut EffectcraftApp, ctx: &egui::Context) {
    let focus = app.dialog_state.linked_import.return_focus;
    app.dialog_state.linked_import.captured_focus = false;
    app.dialog_state.linked_import.restore_pending = focus;
    app.dialog = None;
    ctx.request_repaint();
}

/// egui keeps the previous modal layer through its closing pass. Restore after it has gone,
/// so the background widget can receive focus and Escape cannot clear the restored focus.
pub fn restore_focus(app: &mut EffectcraftApp, ctx: &egui::Context) {
    if app.dialog.is_none()
        && !super::dialogs::modal_open(ctx)
        && ctx.memory(|memory| memory.top_modal_layer()).is_none()
        && let Some(focus) = app.dialog_state.linked_import.restore_pending.take()
    {
        ctx.memory_mut(|memory| memory.request_focus(focus));
        ctx.request_repaint();
    }
}

pub fn show(app: &mut EffectcraftApp, ctx: &egui::Context, t: &Tokens) {
    let mut draft = app.dialog_state.linked_import.clone();
    if !draft.captured_focus {
        draft.return_focus = ctx.memory(|memory| memory.focused());
        draft.captured_focus = true;
    }
    let (mut link, mut cancel, mut choose, mut reload) = (false, false, false, false);
    let mut first = draft.focus_first;
    super::dialogs::modal_fit(ctx, "Linked EffectCraft Composition", vec2(640.0, 520.0), t, |ui, height| {
        let (mut body, body_height) = super::dialogs::above_footer(ui, height, |ui| {
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let ready = draft.compositions.iter().any(|composition| composition.selected && !composition.already_linked);
                    let response = ui.add_enabled(ready, egui::Button::new(tr("Link Selected")));
                    app.auto.add("dialog.linkedImport.link", response.rect, "Link Selected");
                    link = response.clicked();
                    let response = ui.button(tr("Cancel"));
                    app.auto.add("dialog.linkedImport.cancel", response.rect, "Cancel");
                    cancel = response.clicked();
                });
            });
            if let Some(error) = &draft.error {
                ui.add_space(8.0);
                let response = ui.add(egui::Label::new(egui::RichText::new(tr_args("Could not link compositions: {}", &[error])).color(t.danger)).wrap());
                ctx.accesskit_node_builder(response.id, |node| {
                    node.set_role(egui::accesskit::Role::Alert);
                    node.set_live(egui::accesskit::Live::Assertive);
                });
            }
        });
        body.vertical(|ui| {
            ui.horizontal(|ui| {
                let response = ui.button(tr("Choose Project…"));
                if first && !draft.compositions.iter().any(|composition| !composition.already_linked) {
                    response.request_focus();
                }
                app.auto.add("dialog.linkedImport.chooseProject", response.rect, "Choose Project…");
                choose = response.clicked();
                let project_name = std::path::Path::new(&draft.path).file_stem().and_then(std::ffi::OsStr::to_str).unwrap_or(&draft.path);
                ui.add(egui::Label::new(egui::RichText::new(project_name).font(Tokens::semibold(12.0))).truncate()).on_hover_text(&draft.path);
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let response = ui.button(tr("Reload List"));
                app.auto.add("dialog.linkedImport.reload", response.rect, "Reload List");
                reload = response.clicked();
                ui.label(tr("Choose compositions to link."));
            });
            ui.label(egui::RichText::new(tr("Their contents remain in the source project.")).small().color(t.text_dim));
            ui.add_space(8.0);
            let remaining = ui.available_height().min(body_height).max(0.0);
            egui::ScrollArea::vertical().id_salt("linked-import-comps").max_height(remaining).min_scrolled_height(0.0).auto_shrink([false, true]).show(
                ui,
                |ui| {
                    if draft.compositions.is_empty() && draft.error.is_none() {
                        ui.label(tr("This project contains no compositions."));
                    }
                    let stats: Vec<_> = draft.compositions.iter().map(Composition::stats).collect();
                    let small_font = egui::TextStyle::Small.resolve(ui.style());
                    let stats_width = stats
                        .iter()
                        .map(|text| ui.painter().layout_no_wrap(text.clone(), small_font.clone(), t.text_dim).size().x)
                        .fold(0.0_f32, f32::max)
                        .ceil()
                        .min(ui.available_width() * 0.5);
                    let linked_width = ui.painter().layout_no_wrap(tr("Already linked").to_owned(), small_font, t.text_dim).size().x.ceil();
                    for (composition, stats) in draft.compositions.iter_mut().zip(stats) {
                        ui.push_id(&composition.id, |ui| {
                            let label =
                                if composition.already_linked { tr_args("{} — Already linked", &[&composition.name]) } else { composition.name.clone() };
                            let mut selected = composition.selected || composition.already_linked;
                            let response = ui
                                .horizontal(|ui| {
                                    let height = ui.spacing().interact_size.y;
                                    let gap = ui.spacing().item_spacing.x;
                                    let tag_width = if composition.already_linked { linked_width } else { 0.0 };
                                    let gaps = if composition.already_linked { 2.0 } else { 1.0 };
                                    let name_width = (ui.available_width() - stats_width - tag_width - gaps * gap).max(24.0);
                                    let response = ui
                                        .allocate_ui_with_layout(vec2(name_width, height), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                                            ui.set_min_width(name_width);
                                            ui.set_max_width(name_width);
                                            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                                            ui.add_enabled_ui(!composition.already_linked, |ui| {
                                                crate::widgets::labelled_checkbox(ui, &mut selected, &composition.name)
                                            })
                                            .inner
                                        })
                                        .inner;
                                    crate::a11y::name(&response, &label);
                                    if composition.already_linked {
                                        ui.add_sized(
                                            vec2(linked_width, height),
                                            egui::Label::new(egui::RichText::new(tr("Already linked")).small().color(t.text_dim)).truncate(),
                                        );
                                    }
                                    ui.add_sized(
                                        vec2(stats_width, height),
                                        egui::Label::new(egui::RichText::new(stats).small().color(t.text_dim)).halign(egui::Align::RIGHT).truncate(),
                                    );
                                    response
                                })
                                .inner;
                            if !composition.already_linked {
                                composition.selected = selected;
                                if first {
                                    response.request_focus();
                                    response.scroll_to_me(Some(egui::Align::Center));
                                    first = false;
                                }
                                if response.gained_focus() {
                                    response.scroll_to_me(Some(egui::Align::Center));
                                }
                            }
                            app.auto.add(&format!("dialog.linkedImport.comp.{}", composition.id), response.rect, &label);
                        });
                    }
                },
            );
        });
    });
    draft.focus_first = false;
    app.dialog_state.linked_import = draft.clone();
    if cancel {
        close(app, ctx);
    } else if choose {
        if let Err(error) = crate::menus::invoke(app, ctx, "file.linkComposition", json!({})) {
            app.dialog_state.linked_import.error = Some(error);
        }
    } else if reload {
        let _ = open(app, &draft.path);
    } else if link {
        let compositions: Vec<_> =
            draft.compositions.iter().filter(|composition| composition.selected && !composition.already_linked).map(|composition| &composition.id).collect();
        match app.session.execute("file.linkComposition", json!({"path":draft.path,"sourceProjectId":draft.source_project_id,"comps":compositions})) {
            Ok(_) => {
                close(app, ctx);
                app.ui.start_screen = false;
                app.show_panel(crate::dock::PanelKind::Project);
            }
            Err(error) => {
                let error = error.to_string();
                app.ui.status = error.clone();
                app.dialog_state.linked_import.error = Some(error);
            }
        }
    }
}
