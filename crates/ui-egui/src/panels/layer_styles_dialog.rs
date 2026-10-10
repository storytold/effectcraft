//! Layer ▸ Layer Styles ▸ Layer Style Options… (and choosing a style from that menu): a Layer
//! Style dialog with the style list on the left (a check box per style, Blending Options first)
//! and the selected style's controls on the right. Every control edits the ordinary
//! `layerStyles/<style>/<property>` property, so the viewer previews live; Cancel rolls the
//! edits made in the dialog back, OK keeps them (one undo step per control drag).

use crate::i18n::tr;
use effectcraft_engine::keyframe::Value as KV;
use effectcraft_engine::project::styles::{BLENDING, STYLES};
use effectcraft_engine::project::{Layer, LayerId, ParamUi, Property};
use egui::{Align2, vec2};
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

/// Menu routing: choosing one of the Layer ▸ Layer Styles ▸ <style> items (no parameters) adds
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
            egui::ComboBox::from_id_salt(("ls-popup", p.uid)).selected_text(options.get(sel).cloned().unwrap_or_default()).show_ui(ui, |ui| {
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
                ParamUi::Percent => ui.add(egui::DragValue::new(&mut y).speed(0.5).suffix("%")),
                ParamUi::Angle => ui.add(egui::DragValue::new(&mut y).speed(0.5).suffix("°")),
                ParamUi::Pixels => ui.add(egui::DragValue::new(&mut y).speed(0.25).suffix(" px")),
                _ => ui.add(egui::DragValue::new(&mut y).speed(0.1)),
            };
            (r.changed() && y != *x).then(|| json!(y))
        }
        (KV::Color(c), _) => {
            let mut rgba = egui::Rgba::from_rgba_unmultiplied(c[0] as f32, c[1] as f32, c[2] as f32, c[3] as f32);
            let changed = egui::color_picker::color_edit_button_rgba(ui, &mut rgba, egui::color_picker::Alpha::Opaque).changed();
            changed.then(|| json!([rgba.r(), rgba.g(), rgba.b()]))
        }
        (KV::Gradient(g), _) => {
            let resp = ui.button(tr("Edit Gradient…"));
            let popup = egui::Id::new(("ls-gradient-popup", p.uid));
            if resp.clicked() {
                crate::widgets::open_popup(ui, popup);
            }
            super::gradient_editor::popup(ui, popup, resp.rect, g).map(|edited| json!(edited))
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
    super::dialogs::modal(ctx, &format!("Layer Style — {}", l.name), vec2(720.0, 520.0), t, |ui| {
        ui.horizontal_top(|ui| {
            // Style list.
            ui.vertical(|ui| {
                ui.set_width(200.0);
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
                egui::ScrollArea::vertical().max_height(400.0).show(ui, |ui| {
                    egui::Grid::new("ls-grid").num_columns(2).spacing(vec2(12.0, 6.0)).show(ui, |ui| {
                        g.walk("", &mut |_, p| {
                            if matches!(p.ui, ParamUi::Hidden) {
                                return;
                            }
                            let lbl = ui.label(&p.name);
                            let v = p.value_at(lt);
                            if let Some(nv) = control(ui, p, &v) {
                                actions.push((
                                    "prop.set".into(),
                                    json!({"layer": lid, "prop": p.uid, "value": nv, "merge": format!("layer-style-dialog-{}", p.uid)}),
                                ));
                            }
                            let path = g.match_path_of(p.uid).unwrap_or_default();
                            app.auto.add(&format!("dialog.layerStyle.prop.{sel}/{path}"), lbl.rect, &p.name);
                            ui.end_row();
                        });
                    });
                });
            });
        });
        ui.add_space(8.0);
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
            let _ = Align2::LEFT_CENTER;
        });
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
        app.session.project = checkpoint.project;
        app.session.history = checkpoint.history;
        app.session.sanitize_state();
        if changed {
            app.session.bump();
        }
    }
    app.session.history.merge_key = None;
    app.dialog = None;
}
