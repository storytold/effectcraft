//! Layer ▸ Layer Styles ▸ Layer Style Options… (and choosing a style from that menu): a Layer
//! Style dialog with the style list on the left (a check box per style, Blending Options first)
//! and the selected style's controls on the right. Every control edits the ordinary
//! `layerStyles/<style>/<property>` property, so the viewer previews live; Cancel rolls the
//! edits made in the dialog back, OK keeps them (one undo step per control drag).

use crate::i18n::tr;
use effectcraft_engine::commands::text_edit::active_editor_snapshot;
use effectcraft_engine::keyframe::Value as KV;
use effectcraft_engine::project::styles::{BLENDING, STYLES};
use effectcraft_engine::project::{Layer, LayerId, ParamUi, Property};
use egui::vec2;
use serde_json::{Value, json};

use crate::theme::Tokens;
use crate::{Dialog, EffectcraftApp};

/// Dialog state: layer, selected list entry and the undo depth when it opened.
#[derive(Clone, Debug, Default)]
pub struct LayerStyleState {
    pub layer: u64,
    pub selected: String,
    pub undo_mark: usize,
    checkpoint: Option<Checkpoint>,
}

#[derive(Clone)]
struct Checkpoint {
    project: std::sync::Arc<effectcraft_engine::project::Project>,
    history: effectcraft_engine::history::History,
}

impl std::fmt::Debug for Checkpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Checkpoint").field("undo_steps", &self.history.undo.len()).finish_non_exhaustive()
    }
}

fn checkpoint(app: &mut EffectcraftApp) -> Checkpoint {
    app.session.history.merge_key = None;
    Checkpoint { project: app.session.project.clone(), history: app.session.history.clone() }
}

/// Open the dialog on the first selected (or given) layer, showing `style` (a style id;
/// Blending Options when none).
pub fn open(app: &mut EffectcraftApp, params: &Value) -> Result<(), String> {
    let comp = app.session.active_comp().ok_or("no composition is open")?;
    let layer = params
        .get("layer")
        .and_then(Value::as_u64)
        .or_else(|| app.session.state.selected_layers.first().map(|l| l.0))
        .filter(|l| comp.layer(LayerId(*l)).is_some())
        .ok_or("select a layer first")?;
    let style = params.get("style").and_then(Value::as_str).unwrap_or(BLENDING).to_string();
    if style != BLENDING && !STYLES.iter().any(|s| s.0 == style) {
        return Err(format!("unknown layer style `{style}`"));
    }
    let checkpoint = checkpoint(app);
    app.dialog_state.layer_style = LayerStyleState { layer, selected: style, undo_mark: app.session.history.undo.len(), checkpoint: Some(checkpoint) };
    app.dialog = Some(Dialog::LayerStyles);
    Ok(())
}

/// Menu routing: choosing one of the Layer ▸ Layer Styles ▸ `<style>` items (no parameters) adds
/// the style as usual and opens the dialog on it.
pub fn route(app: &mut EffectcraftApp, id: &str, params: &Value) -> Result<bool, String> {
    let Some(style) = id.strip_prefix("layer.style.").filter(|s| STYLES.iter().any(|x| x.0 == *s)) else { return Ok(false) };
    if !params.as_object().is_none_or(|m| m.is_empty()) {
        return Ok(false);
    }
    let mark = app.session.history.undo.len();
    let checkpoint = checkpoint(app);
    app.session.execute(id, json!({})).map_err(|e| e.to_string())?;
    open(app, &json!({"style": style}))?;
    // Cancel also removes the style just added.
    app.dialog_state.layer_style.undo_mark = mark;
    app.dialog_state.layer_style.checkpoint = Some(checkpoint);
    Ok(true)
}

fn layer(app: &EffectcraftApp) -> Option<Layer> {
    app.session.active_comp()?.layer(LayerId(app.dialog_state.layer_style.layer)).cloned()
}

/// One control row for a property. Returns the new value when it changed.
fn control(ui: &mut egui::Ui, p: &Property, v: &KV) -> Option<Value> {
    match (v, &p.ui) {
        (KV::Bool(b), _) => {
            let mut on = *b;
            ui.checkbox(&mut on, "").changed().then(|| json!(on))
        }
        (KV::Enum(i), ParamUi::Popup { options }) => {
            let mut sel = *i as usize;
            craft_ui::ComboBox::from_id_salt(("ls-popup", p.uid)).selected_text(options.get(sel).cloned().unwrap_or_default()).show_ui(ui, |ui| {
                for (k, o) in options.iter().enumerate() {
                    ui.selectable_value(&mut sel, k, o);
                }
            });
            (sel != *i as usize).then(|| json!(sel))
        }
        (KV::Scalar(x), ui_kind) => {
            let mut y = *x;
            let r = match ui_kind {
                ParamUi::Slider { slider_min, slider_max, decimals, .. } => {
                    ui.add(egui::Slider::new(&mut y, *slider_min..=*slider_max).clamping(egui::SliderClamping::Never).max_decimals(*decimals as usize))
                }
                ParamUi::Percent => ui.add(crate::widgets::drag_value(egui::DragValue::new(&mut y).speed(0.5).suffix("%"))),
                ParamUi::Angle => ui.add(crate::widgets::drag_value(egui::DragValue::new(&mut y).speed(0.5).suffix("°"))),
                ParamUi::Pixels => ui.add(crate::widgets::drag_value(egui::DragValue::new(&mut y).speed(0.25).suffix(" px"))),
                _ => ui.add(crate::widgets::drag_value(egui::DragValue::new(&mut y).speed(0.1))),
            };
            (r.changed() && y != *x).then(|| json!(y))
        }
        (KV::Color(c), _) => {
            let mut rgb = [c[0] as f32, c[1] as f32, c[2] as f32];
            let changed = crate::widgets::srgb_color_button(ui, &mut rgb).changed();
            changed.then(|| json!(rgb))
        }
        (KV::Gradient(_), _) => {
            ui.label(egui::RichText::new(tr("Edit in the Timeline")).weak());
            None
        }
        _ => {
            ui.label(egui::RichText::new(format!("{v:?}").chars().take(24).collect::<String>()).weak());
            None
        }
    }
}

pub fn show(app: &mut EffectcraftApp, ctx: &egui::Context, t: &Tokens) {
    let Some(l) = layer(app) else {
        app.dialog = None;
        return;
    };
    let lid = l.id.0;
    let styles = l.layer_styles().cloned();
    let sel = app.dialog_state.layer_style.selected.clone();
    let time = app.session.time();
    let lt = l.layer_time(time);
    let mut actions: Vec<(String, Value)> = Vec::new();
    let mut close: Option<bool> = None;
    super::dialogs::modal_fit(ctx, &format!("Layer Style — {}", l.name), vec2(720.0, 520.0), t, |ui, height| {
        // OK and Cancel stay at the bottom, in view; the list and the style's controls scroll
        // above them (#1062).
        let (mut body, body_h) = super::dialogs::above_footer(ui, height, |foot| {
            foot.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let ok = ui.button(tr("OK"));
                    app.auto.add("dialog.layerStyle.ok", ok.rect, "OK");
                    if ok.clicked() {
                        close = Some(true);
                    }
                    let cancel = ui.button(tr("Cancel"));
                    app.auto.add("dialog.layerStyle.cancel", cancel.rect, "Cancel");
                    if cancel.clicked() {
                        close = Some(false);
                    }
                });
            });
        });
        {
            let ui = &mut body;
            // Style list.
            ui.vertical(|ui| {
                ui.set_width(200.0);
                egui::ScrollArea::vertical().id_salt("ls-list").max_height(body_h).min_scrolled_height(0.0).show(ui, |ui| {
                    let mut entries = vec![(BLENDING, "Blending Options")];
                    entries.extend(STYLES.iter().copied());
                    for (id, name) in entries {
                        ui.horizontal(|ui| {
                            if id != BLENDING {
                                let g = styles.as_ref().and_then(|s| s.sub(id));
                                let mut on = g.is_some_and(|g| g.enabled);
                                if ui.checkbox(&mut on, "").changed() {
                                    if g.is_none() {
                                        actions.push(("layer.style.add".into(), json!({"layers": [lid], "style": id})));
                                    } else {
                                        actions.push(("layer.style.toggle".into(), json!({"layer": lid, "style": id, "value": on})));
                                    }
                                }
                            } else {
                                ui.add_space(24.0);
                            }
                            let r = ui.selectable_label(sel == id, name);
                            if r.clicked() {
                                app.dialog_state.layer_style.selected = id.to_string();
                            }
                            let rect = r.rect;
                            app.auto.add(&format!("dialog.layerStyle.list.{id}"), rect, name);
                        });
                    }
                });
            });
            ui.separator();
            // Controls of the selected style.
            ui.vertical(|ui| {
                let name = if sel == BLENDING { "Blending Options" } else { STYLES.iter().find(|s| s.0 == sel).map(|s| s.1).unwrap_or("") };
                let head = ui.label(egui::RichText::new(name).font(Tokens::semibold(13.0)));
                app.auto.add("dialog.layerStyle.page", head.rect, &sel);
                ui.add_space(6.0);
                let Some(g) = styles.as_ref().and_then(|s| s.sub(&sel)) else {
                    ui.label(egui::RichText::new(tr("Not applied: tick it in the list to add it.")).weak());
                    return;
                };
                // Both ways, in the room left under the heading: in a narrow window (or at a high UI
                // Scale) the controls scroll sideways rather than being cut off.
                egui::ScrollArea::both()
                    .id_salt("ls-controls")
                    .max_width(ui.available_width().max(0.0))
                    .max_height(ui.available_height().max(0.0))
                    .min_scrolled_width(0.0)
                    .min_scrolled_height(0.0)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        egui::Grid::new("ls-grid").num_columns(2).spacing(vec2(12.0, 6.0)).show(ui, |ui| {
                            g.walk("", &mut |_, p| {
                                if matches!(p.ui, ParamUi::Hidden) {
                                    return;
                                }
                                let lbl = ui.label(&p.name);
                                let v = p.value_at(lt);
                                let edit = ui.scope(|ui| control(ui, p, &v));
                                if let Some(nv) = edit.inner {
                                    actions.push((
                                        "prop.set".into(),
                                        json!({"layer": lid, "prop": p.uid, "value": nv, "merge": format!("layer-style-dialog-{}", p.uid)}),
                                    ));
                                }
                                let path = g.match_path_of(p.uid).unwrap_or_default();
                                app.auto.add(&format!("dialog.layerStyle.prop.{sel}/{path}"), lbl.rect, &p.name);
                                app.auto.add(&format!("dialog.layerStyle.value.{sel}/{path}"), edit.response.rect, &p.name);
                                ui.end_row();
                            });
                        });
                    });
            });
        }
    });
    for (id, p) in actions {
        if let Err(e) = app.session.execute(&id, p) {
            app.ui.status = e.to_string();
        }
    }
    match close {
        Some(true) => finish(app, true),
        Some(false) => finish(app, false),
        None => {}
    }
}

/// Show another entry of the list (`blendingOptions` or a style id).
pub fn select(app: &mut EffectcraftApp, style: &str) {
    app.dialog_state.layer_style.selected = style.to_string();
}

/// Close the dialog: OK keeps the edits, Cancel undoes everything done since it opened.
pub fn finish(app: &mut EffectcraftApp, ok: bool) {
    if let Some(checkpoint) = app.dialog_state.layer_style.checkpoint.take()
        && !ok
    {
        let changed = !std::sync::Arc::ptr_eq(&app.session.project, &checkpoint.project);
        if changed {
            // Snapshot only when Cancel has something to restore: opening and cancelling without
            // edits must not copy the whole TextDoc.
            let edited = active_editor_snapshot(&app.session);
            app.session.project = checkpoint.project;
            app.session.history = checkpoint.history;
            app.session.sanitize_after_replacement(edited.as_ref());
            app.session.bump();
        } else {
            // No edit was committed, so nothing can have become invalid. In particular, do not
            // call sanitize_state here: its text-edit cleanup clones the whole TextDoc.
            app.session.history = checkpoint.history;
        }
    }
    app.session.history.merge_key = None;
    app.dialog = None;
}

#[cfg(test)]
mod tests {
    use super::*;
    use effectcraft_engine::Session;
    use effectcraft_engine::commands::text_edit::layer_doc;
    use effectcraft_engine::project::LayerId;
    use effectcraft_engine::project::styles::BLENDING;
    use serde_json::json;

    fn editing_app() -> (EffectcraftApp, u64) {
        let mut s = Session::default();
        s.execute("comp.new", json!({"name": "Styles", "width": 320, "height": 180, "duration": 1})).unwrap();
        let lid = s.execute("layer.newText", json!({"text": "abcdef"})).unwrap()["layer"].as_u64().unwrap();
        s.execute("text.edit", json!({"layer": lid, "caret": 3})).unwrap();
        (EffectcraftApp::new(s), lid)
    }

    fn editor_state(app: &EffectcraftApp) -> Option<(LayerId, u64, usize, usize)> {
        app.session.state.text_edit.as_ref().map(|e| (e.layer, e.session, e.anchor, e.caret))
    }

    #[test]
    fn cancel_keeps_an_open_text_editor_when_the_dialog_did_not_change_its_text() {
        let (mut app, lid) = editing_app();
        open(&mut app, &json!({"layer": lid, "style": BLENDING})).unwrap();
        // A live preview edit changes the project but not the edited Source Text.
        app.session.execute("prop.set", json!({"layer": lid, "path": "transform/opacity", "value": 40})).unwrap();
        let before = editor_state(&app);
        assert!(before.is_some());

        finish(&mut app, false);
        assert_eq!(editor_state(&app), before, "Cancel keeps an editor whose document it did not replace");
    }

    #[test]
    fn cancel_without_a_dialog_edit_keeps_the_open_editor_and_its_document_untouched() {
        let (mut app, lid) = editing_app();
        open(&mut app, &json!({"layer": lid, "style": BLENDING})).unwrap();
        let before = editor_state(&app);
        let project = app.session.project.clone();
        let undo = app.session.history.undo.len();
        assert!(before.is_some());

        finish(&mut app, false);
        assert!(std::sync::Arc::ptr_eq(&app.session.project, &project), "a no-op Cancel restores the same project");
        assert_eq!(app.session.history.undo.len(), undo, "and the same undo history");
        assert_eq!(editor_state(&app), before, "and leaves the open editor untouched");
    }

    #[test]
    fn cancel_replaces_an_open_text_editor_when_the_preview_changed_its_text() {
        let (mut app, lid) = editing_app();
        open(&mut app, &json!({"layer": lid, "style": BLENDING})).unwrap();
        app.session.execute("layer.setText", json!({"layer": lid, "text": "ab"})).unwrap();
        let preview = editor_state(&app).expect("still editing");
        assert_eq!(layer_doc(&app.session, LayerId(lid)).unwrap().text, "ab");

        finish(&mut app, false);
        let restored = editor_state(&app).expect("the editor survives the rollback");
        assert_eq!(layer_doc(&app.session, LayerId(lid)).unwrap().text, "abcdef", "Cancel restores the pre-dialog text");
        assert_ne!(restored.1, preview.1, "the restored editor starts a new epoch");
        assert_eq!((restored.2, restored.3), (2, 2), "the caret stays in the restored document");
    }
}
