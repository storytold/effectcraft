//! Essential Graphics panel (Window ▸ Essential Graphics).
//!
//! - **Primary** composition, **Solo Supported Properties** (the timeline lists only what can
//!   be exposed) and the template name.
//! - The controls: drag a property name from the timeline onto the list (or Animation ▸ Add
//!   Property to Essential Graphics); each shows as a text / colour / slider / checkbox / point /
//!   angle / dropdown control editing the source property (Add Selected As: Source Text as a font
//!   control, Scale as one uniform slider); rename by double-clicking the name;
//!   reorder with ▲▼, remove with ✕; groups and comments from Add Formatting; Media
//!   Replacement for the selected footage layer.
//! - **Mirrors and links**: adding a property that is already in the panel adds a *mirror*
//!   (same value, its own name and place); a control's ⋯ menu adds a mirror, links the selected
//!   timeline property (the control then drives both) or unlinks one. Mirrors and linked
//!   controls are badged (`essential.control.<id>.badge`, `.menu`).
//! - **Export Template…** writes an open `.ectemplate`.
//! - With a precomp layer that has Essential Properties selected, its instance values: edit them
//!   (overrides), Revert or Push to Comp.
//!
//! Every control runs an engine `essential.*` / `prop.set` command and registers an automation
//! id (`essential.primary`, `essential.solo`, `essential.name`, `essential.control.<id>.value`,
//! `essential.instance.<id>.value`, `essential.export`…).

use crate::i18n::{tr, tr_args};
use effectcraft_engine::keyframe::Value as KV;
use effectcraft_engine::project::essential::{self, ControlType, EgControl, EgKind};
use effectcraft_engine::project::{ItemId, ItemKind, LayerSource, ParamUi, Property};
use egui::{Rect, RichText};
use serde_json::{Value, json};

use crate::panels::DragPayload;
use crate::{EffectcraftApp, widgets};

type Actions = Vec<(String, Value)>;

/// The comp the panel edits: the Primary comp, else the active one.
fn primary(app: &EffectcraftApp) -> Option<ItemId> {
    let s = &app.session;
    s.state.essential_primary.filter(|c| s.project.comp(*c).is_some()).or(s.active_comp_id())
}

pub fn show(app: &mut EffectcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let ctx = ui.ctx().clone();
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink(8.0)).layout(egui::Layout::top_down(egui::Align::Min)));
    let mut out = None;
    egui::ScrollArea::vertical().id_salt("eg-panel").auto_shrink([false, false]).show(&mut child, |ui| out = Some(body(app, ui)));
    let Some((actions, invoke)) = out else { return };
    for (id, p) in actions {
        if let Err(e) = app.session.execute(&id, p) {
            app.ui.status = e.to_string();
        }
    }
    for (id, p) in invoke {
        if let Err(e) = crate::menus::invoke(app, &ctx, &id, p) {
            app.ui.status = e;
        }
    }
}

fn body(app: &mut EffectcraftApp, ui: &mut egui::Ui) -> (Actions, Actions) {
    let t = app.tokens;
    let ctx = ui.ctx().clone();
    let mut actions: Actions = vec![];
    let mut invoke: Vec<(String, Value)> = vec![];
    let Some(cid) = primary(app) else {
        ui.label(RichText::new(tr("Open a composition to build Essential Graphics")).color(t.text_faint));
        return (actions, invoke);
    };
    let project = app.session.project.clone();
    let comp_name = project.item(cid).map(|i| i.name.clone()).unwrap_or_default();

    // ---- Primary + Solo.
    ui.horizontal(|ui| {
        ui.label(RichText::new(tr("Primary:")).color(t.text_dim));
        let r = egui::ComboBox::from_id_salt("eg-primary").selected_text(&comp_name).width(150.0).show_ui(ui, |ui| {
            for (id, _) in project.comps() {
                let name = project.item(*id).map(|i| i.name.clone()).unwrap_or_default();
                if ui.selectable_label(*id == cid, name).clicked() {
                    actions.push(("essential.setPrimary".into(), json!({"comp": id.0})));
                }
            }
        });
        // The label beside the combo names it for a screen reader; the selected comp alone
        // doesn't say what the dropdown chooses. Keep the visible label and value together.
        let primary_name = format!("{} {}", tr("Primary:"), comp_name);
        r.response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, ui.is_enabled(), &primary_name));
        app.auto.add("essential.primary", r.response.rect, &comp_name);
    });
    let solo = app.session.state.essential_solo;
    let r = ui.add(egui::Button::new(tr("Solo Supported Properties")).selected(solo));
    app.auto.add("essential.solo", r.rect, "Solo Supported Properties");
    if r.clicked() {
        actions.push(("essential.soloSupported".into(), json!({"on": !solo})));
    }
    // ---- Name.
    let eg = project.comp(cid).and_then(|c| c.essential.clone()).unwrap_or_default();
    let shown_name = if eg.name.is_empty() { comp_name.clone() } else { eg.name.clone() };
    let name_id = egui::Id::unique("eg-name-buf");
    let mut name: String = ctx.data(|d| d.get_temp(name_id)).unwrap_or_else(|| shown_name.clone());
    let r = ui.add(egui::TextEdit::singleline(&mut name).hint_text(tr("Template name")).desired_width(f32::INFINITY));
    app.auto.add("essential.name", r.rect, &shown_name);
    if r.has_focus() {
        ctx.data_mut(|d| d.insert_temp(name_id, name.clone()));
    } else {
        if r.lost_focus() && name.trim() != shown_name {
            actions.push(("essential.setName".into(), json!({"comp": cid.0, "name": name.trim()})));
        }
        ctx.data_mut(|d| d.remove::<String>(name_id));
    }
    ui.separator();

    // ---- Controls (drop target for timeline properties).
    let list = ui.scope(|ui| {
        ui.set_min_height(60.0);
        if eg.controls.is_empty() {
            ui.label(RichText::new(tr("Drag properties here from the timeline,\nor use Animation > Add Property to Essential Graphics.")).color(t.text_faint));
        }
        controls(app, ui, cid, &eg.controls, None, &mut actions);
    });
    let drop_rect = list.response.rect.expand2(egui::vec2(0.0, 4.0));
    app.auto.add("essential.dropZone", drop_rect, "Drag properties here");
    if let Some(payload) = egui::DragAndDrop::payload::<DragPayload>(&ctx)
        && let DragPayload::Property { layer, prop } = *payload
        && ui.rect_contains_pointer(drop_rect)
    {
        ui.painter().rect_stroke(drop_rect, 2.0, egui::Stroke::new(2.0, t.accent), egui::StrokeKind::Inside);
        if ctx.input(|i| i.pointer.any_released()) {
            let comp = app.session.active_comp_id().map(|c| c.0);
            actions.push(("essential.addProperty".into(), json!({"comp": comp, "layer": layer, "prop": prop})));
            egui::DragAndDrop::clear_payload(&ctx);
        }
    }
    ui.separator();

    // ---- Bottom buttons.
    ui.horizontal_wrapped(|ui| {
        let r = crate::widgets::menu_button(ui, tr("Add Formatting"), |ui| {
            if ui.button(tr("Add Group")).clicked() {
                actions.push(("essential.addGroup".into(), json!({"comp": cid.0})));
                ui.close();
            }
            if ui.button(tr("Add Comment")).clicked() {
                actions.push(("essential.addComment".into(), json!({"comp": cid.0})));
                ui.close();
            }
        });
        app.auto.add("essential.addFormatting", r.response.rect, "Add Formatting");
        // The selected property as a Font control (Source Text) or a uniform Scale slider.
        let has_sel = !app.session.state.selected_props.is_empty();
        let r = ui.add_enabled_ui(has_sel, |ui| {
            crate::widgets::menu_button(ui, tr("Add Selected As"), |ui| {
                for (label, as_type) in [("Font Control", "font"), ("Uniform Scale", "scale")] {
                    if ui.button(label).clicked() {
                        actions.push(("essential.addProperty".into(), json!({"as": as_type})));
                        ui.close();
                    }
                }
            })
        });
        app.auto.add("essential.addAs", r.response.rect, "Add Selected As");
        let footage_layer = app.session.active_comp().and_then(|c| {
            app.session.state.selected_layers.iter().filter_map(|l| c.layer(*l)).find(|l| matches!(l.source, LayerSource::Footage { .. })).map(|l| l.id.0)
        });
        let r = ui.add_enabled(footage_layer.is_some(), egui::Button::new(tr("Media Replacement")));
        app.auto.add("essential.addMedia", r.rect, "Add Media Replacement");
        if r.clicked()
            && let Some(l) = footage_layer
        {
            actions.push(("essential.addMedia".into(), json!({"layer": l})));
        }
        let r = ui.add_enabled(!eg.controls.is_empty(), egui::Button::new(tr("Export Template…")));
        app.auto.add("essential.export", r.rect, "Export Template");
        if r.clicked() {
            invoke.push(("essential.exportTemplate".into(), json!({"comp": cid.0})));
        }
    });

    // ---- The selected instance's Essential Properties.
    instance(app, ui, &mut actions);
    (actions, invoke)
}

/// One list of controls (top level or a group's children).
fn controls(app: &mut EffectcraftApp, ui: &mut egui::Ui, cid: ItemId, list: &[EgControl], group: Option<u64>, actions: &mut Actions) {
    let t = app.tokens;
    let n = list.len();
    for (i, c) in list.iter().enumerate() {
        let row = ui.horizontal(|ui| {
            // Reorder / remove.
            for (label, auto, enabled, to) in [("▲", "up", i > 0, i.saturating_sub(1)), ("▼", "down", i + 1 < n, i + 1)] {
                let r = ui.add_enabled(enabled, egui::Button::new(RichText::new(label).size(9.0)).small().frame(false));
                app.auto.add(&format!("essential.control.{}.{auto}", c.id), r.rect, label);
                if r.clicked() {
                    actions.push(("essential.move".into(), json!({"comp": cid.0, "control": c.id, "group": group, "index": to})));
                }
            }
            match &c.kind {
                EgKind::Group { .. } => {
                    name_label(app, ui, cid, c, RichText::new(&c.name).strong(), actions);
                }
                EgKind::Comment { text } => {
                    let shown = EgControl { name: text.clone(), ..c.clone() };
                    name_label(app, ui, cid, &shown, RichText::new(text).italics().color(t.text_dim), actions);
                }
                EgKind::Property { .. } | EgKind::Mirror { .. } => {
                    name_label(app, ui, cid, c, RichText::new(&c.name), actions);
                    // Mirrors and linked properties are marked (hover: what they share).
                    let eg = app.session.project.comp(cid).and_then(|x| x.essential.clone()).unwrap_or_default();
                    let badge = match &c.kind {
                        EgKind::Mirror { of } => Some(("mirror".to_string(), format!("Mirror of {}", eg.find(*of).map(|m| m.name.as_str()).unwrap_or("?")))),
                        EgKind::Property { links, .. } if !links.is_empty() => {
                            let names: Vec<String> = essential::linked_props(&app.session.project, cid, c)
                                .into_iter()
                                .map(|(l, p)| format!("{} › {}", l.name, l.props.name_path_of(p.uid).unwrap_or_else(|| p.name.clone())))
                                .collect();
                            Some((format!("+{} linked", links.len()), format!("Also drives:\n{}", names.join("\n"))))
                        }
                        _ => None,
                    };
                    if let Some((text, tip)) = badge {
                        let r = ui.label(RichText::new(text).color(t.accent).size(10.0)).on_hover_text(&tip);
                        app.auto.add(&format!("essential.control.{}.badge", c.id), r.rect, &tip);
                    }
                    let src = essential::source_prop(&app.session.project, cid, c).map(|(l, p)| (l.id.0, l.layer_time(app.session.time()), p.clone()));
                    match src {
                        Some((layer, lt, p)) => {
                            let v = p.value_at(lt);
                            if let Some(nv) = value_widget(
                                app,
                                ui,
                                &format!("essential.control.{}.value", c.id),
                                essential::effective_type(eg.resolve(c.id).unwrap_or(c), &p),
                                &p,
                                &v,
                            ) {
                                actions.push((
                                    "prop.set".into(),
                                    json!({"comp": cid.0, "layer": layer, "prop": p.uid, "value": nv, "merge": format!("eg-{}", c.id)}),
                                ));
                            }
                        }
                        None => {
                            ui.label(RichText::new(tr("(missing)")).color(t.danger));
                        }
                    }
                    link_menu(app, ui, cid, c, group, actions);
                }
                EgKind::Media { .. } => {
                    name_label(app, ui, cid, c, RichText::new(&c.name), actions);
                    let item = essential::source_media(&app.session.project, cid, c);
                    let name = item.and_then(|i| app.session.project.item(i)).map(|i| i.name.clone()).unwrap_or_default();
                    ui.label(RichText::new(tr_args("▣ {}", &[&name])).color(t.text_dim));
                }
            }
            let r = ui.add(egui::Button::new(RichText::new("×").size(10.0)).small().frame(false)).on_hover_text(tr("Remove"));
            app.auto.add(&format!("essential.control.{}.remove", c.id), r.rect, "Remove");
            if r.clicked() {
                actions.push(("essential.remove".into(), json!({"comp": cid.0, "control": c.id})));
            }
        });
        app.auto.add(&format!("essential.control.{}.row", c.id), row.response.rect, &c.name);
        if let EgKind::Group { children } = &c.kind {
            ui.indent(("eg-group", c.id), |ui| controls(app, ui, cid, children, Some(c.id), actions));
        }
    }
}

/// A property control's "⋯" menu: Add Mirror, Link Selected Property, Unlink.
fn link_menu(app: &mut EffectcraftApp, ui: &mut egui::Ui, cid: ItemId, c: &EgControl, group: Option<u64>, actions: &mut Actions) {
    let links: Vec<(u64, u64, String)> = match &c.kind {
        EgKind::Property { .. } => essential::linked_props(&app.session.project, cid, c)
            .into_iter()
            .map(|(l, p)| (l.id.0, p.uid, format!("{} › {}", l.name, l.props.name_path_of(p.uid).unwrap_or_else(|| p.name.clone()))))
            .collect(),
        _ => vec![],
    };
    let has_sel = !app.session.state.selected_props.is_empty();
    let r = crate::widgets::menu_button(ui, RichText::new("⋯").size(10.0), |ui| {
        if ui.button(tr("Add Mirror")).clicked() {
            actions.push(("essential.addMirror".into(), json!({"comp": cid.0, "control": c.id, "group": group})));
            ui.close();
        }
        if ui.add_enabled(has_sel, egui::Button::new(tr("Link Selected Property"))).clicked() {
            actions.push(("essential.linkProperty".into(), json!({"comp": cid.0, "control": c.id})));
            ui.close();
        }
        for (layer, prop, name) in &links {
            if ui.button(tr_args("Unlink {}", &[&name])).clicked() {
                actions.push(("essential.unlinkProperty".into(), json!({"comp": cid.0, "control": c.id, "layer": layer, "prop": prop})));
                ui.close();
            }
        }
    });
    app.auto.add(&format!("essential.control.{}.menu", c.id), r.response.rect, "Mirror and link");
}

/// A control's name; double-click to rename (comments: edit the text).
fn name_label(app: &mut EffectcraftApp, ui: &mut egui::Ui, cid: ItemId, c: &EgControl, text: RichText, actions: &mut Actions) {
    let edit_id = egui::Id::unique(("eg-rename", c.id));
    let editing: Option<String> = ui.ctx().data(|d| d.get_temp(edit_id));
    match editing {
        Some(mut buf) => {
            let r = ui.add(egui::TextEdit::singleline(&mut buf).desired_width(120.0));
            app.auto.add(&format!("essential.control.{}.rename", c.id), r.rect, &c.name);
            if r.lost_focus() {
                if !ui.input(|i| i.key_pressed(egui::Key::Escape)) && !buf.trim().is_empty() && buf.trim() != c.name {
                    actions.push(("essential.rename".into(), json!({"comp": cid.0, "control": c.id, "name": buf.trim()})));
                }
                ui.ctx().data_mut(|d| d.remove::<String>(edit_id));
            } else {
                widgets::keep_focus(ui.ctx(), &r, &buf);
                ui.ctx().data_mut(|d| d.insert_temp(edit_id, buf));
            }
        }
        None => {
            let r = ui.add(egui::Label::new(text).sense(egui::Sense::click()).truncate());
            app.auto.add(&format!("essential.control.{}.name", c.id), r.rect, &c.name);
            if r.double_clicked() {
                ui.ctx().data_mut(|d| d.insert_temp(edit_id, c.name.clone()));
            }
        }
    }
}

/// The editor for a control's value; returns the new value (JSON for `prop.set`).
fn value_widget(app: &mut EffectcraftApp, ui: &mut egui::Ui, auto: &str, ty: Option<ControlType>, p: &Property, v: &KV) -> Option<Value> {
    let ty = ty?;
    let mut out = None;

    let rect = match ty {
        ControlType::Text => {
            let cur = match v {
                KV::Text(d) => d.text.clone(),
                KV::Str(s) => s.clone(),
                _ => String::new(),
            };
            let id = egui::Id::unique(("eg-text", auto));
            let mut buf: String = ui.ctx().data(|d| d.get_temp(id)).unwrap_or_else(|| cur.clone());
            let r = ui.add(egui::TextEdit::singleline(&mut buf).desired_width(140.0));
            if r.has_focus() {
                ui.ctx().data_mut(|d| d.insert_temp(id, buf));
            } else {
                if r.lost_focus() && buf != cur {
                    out = Some(json!(buf));
                }
                ui.ctx().data_mut(|d| d.remove::<String>(id));
            }
            r.rect
        }
        ControlType::Color => {
            let c = v.as_color();
            let mut rgb = [c[0], c[1], c[2]];
            let r = crate::widgets::srgb_color_button(ui, &mut rgb);
            if r.changed() {
                out = Some(json!([rgb[0], rgb[1], rgb[2], c[3]]));
            }
            r.rect
        }
        ControlType::Checkbox => {
            let mut on = v.as_bool();
            let r = ui.checkbox(&mut on, "");
            if r.changed() {
                out = Some(json!(on));
            }
            r.rect
        }
        ControlType::Dropdown => {
            let opts = match &p.ui {
                ParamUi::Popup { options } => options.clone(),
                _ => vec![],
            };
            let cur = v.as_enum() as usize;
            let r = egui::ComboBox::from_id_salt(("eg-dd", auto)).selected_text(opts.get(cur).cloned().unwrap_or_default()).show_ui(ui, |ui| {
                for (i, o) in opts.iter().enumerate() {
                    if ui.selectable_label(i == cur, o).clicked() {
                        out = Some(json!(i));
                    }
                }
            });
            r.response.rect
        }
        // Scale keeps its Constrain Proportions link, on by default, as in After Effects (#338).
        // The link is the property's own, shared with the Timeline's chain link.
        ControlType::Point if p.match_id == "scale" && matches!(p.ui, ParamUi::Percent) => {
            let comps = v.components();
            let linked = !app.ui.timeline.unlinked.contains(&p.uid);
            let (lr, link) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::click());
            let t = app.tokens;
            crate::icons::paint(ui.painter(), lr, crate::icons::Icon::Link, if linked { t.accent } else { t.text_faint });
            let link = link.on_hover_text(tr("Constrain Proportions"));
            app.auto.add(&format!("{auto}.link"), lr, "Constrain Proportions");
            if link.clicked() && !app.ui.timeline.unlinked.remove(&p.uid) {
                app.ui.timeline.unlinked.insert(p.uid);
            }
            let mut rect = lr;
            for (d, &old) in comps.iter().enumerate() {
                let mut x = old;
                let r = ui.add(egui::DragValue::new(&mut x).speed(0.5).suffix(if d + 1 == comps.len() { "%" } else { "" }).max_decimals(1));
                app.auto.add(&format!("{auto}.{d}"), r.rect, &p.name);
                rect = rect.union(r.rect);
                if r.changed() {
                    // Alt edits one value of a linked pair, as in the Timeline.
                    let next = if linked && !ui.input(|i| i.modifiers.alt) {
                        widgets::constrained(&comps, d, x)
                    } else {
                        comps.iter().enumerate().map(|(e, &c)| if e == d { x } else { c }).collect()
                    };
                    out = Some(json!(next));
                }
            }
            rect
        }
        ControlType::Point => {
            let mut a = v.as_vec2();
            let r1 = ui.add(crate::widgets::drag_value(egui::DragValue::new(&mut a[0]).speed(1.0).max_decimals(1)));
            let r2 = ui.add(crate::widgets::drag_value(egui::DragValue::new(&mut a[1]).speed(1.0).max_decimals(1)));
            if r1.changed() || r2.changed() {
                out = Some(json!(a));
            }
            r1.rect.union(r2.rect)
        }
        ControlType::Angle => {
            let mut a = v.as_f64();
            let r = ui.add(crate::widgets::drag_value(egui::DragValue::new(&mut a).speed(1.0).suffix("°").max_decimals(1)));
            if r.changed() {
                out = Some(json!(a));
            }
            r.rect
        }
        ControlType::Font => {
            // Family (from the installed fonts), style and size; the text stays as it is.
            let KV::Text(doc) = v else { return None };
            let fams = effectcraft_engine::text::fonts::families();
            let mut d = (**doc).clone();
            let mut changed = false;
            let r1 = egui::ComboBox::from_id_salt(("eg-font", auto)).selected_text(&d.font).width(110.0).show_ui(ui, |ui| {
                for (f, _) in &fams {
                    if ui.selectable_label(*f == d.font, f).clicked() {
                        d.font = f.clone();
                        changed = true;
                    }
                }
            });
            let styles: Vec<String> = fams.iter().find(|(f, _)| *f == d.font).map(|(_, st)| st.clone()).unwrap_or_default();
            let r2 = egui::ComboBox::from_id_salt(("eg-style", auto)).selected_text(&d.style).width(70.0).show_ui(ui, |ui| {
                for st in &styles {
                    if ui.selectable_label(*st == d.style, st).clicked() {
                        d.style = st.clone();
                        changed = true;
                    }
                }
            });
            let r3 = ui.add(crate::widgets::drag_value(egui::DragValue::new(&mut d.size).speed(0.5).range(1.0..=2000.0).suffix(" px").max_decimals(1)));
            app.auto.add(&format!("{auto}.font"), r1.response.rect, "Font");
            app.auto.add(&format!("{auto}.style"), r2.response.rect, "Style");
            app.auto.add(&format!("{auto}.size"), r3.rect, "Size");
            if changed || r3.changed() {
                out = serde_json::to_value(&d).ok();
            }
            r1.response.rect.union(r3.rect)
        }
        ControlType::Scale => {
            // One percentage for every axis.
            let comps = v.components();
            let mut x = comps.first().copied().unwrap_or(100.0);
            let r = ui.add(crate::widgets::drag_value(egui::DragValue::new(&mut x).speed(0.5).suffix("%").max_decimals(1)));
            if r.changed() {
                out = Some(json!(vec![x; comps.len().max(1)]));
            }
            r.rect
        }
        ControlType::Slider | ControlType::Media => {
            let mut x = v.as_f64();
            let r = match p.ui {
                ParamUi::Slider { slider_min, slider_max, .. } => ui.add(egui::Slider::new(&mut x, slider_min..=slider_max).max_decimals(2)),
                _ => ui.add(crate::widgets::drag_value(egui::DragValue::new(&mut x).speed(0.5).max_decimals(2))),
            };
            if r.changed() {
                out = Some(json!(x));
            }
            r.rect
        }
    };
    app.auto.add(auto, rect, &p.name);
    out
}

/// Essential Properties of the selected precomp layer: per-instance values, overrides marked ●,
/// Revert and Push to Comp.
fn instance(app: &mut EffectcraftApp, ui: &mut egui::Ui, actions: &mut Actions) {
    let t = app.tokens;
    let Some(comp) = app.session.active_comp_arc() else { return };
    let Some(layer) = app.session.state.selected_layers.iter().filter_map(|l| comp.layer(*l)).find(|l| essential::group(l).is_some()).cloned() else { return };
    let LayerSource::Comp { item: src } = layer.source else { return };
    let Some(eg) = app.session.project.comp(src).and_then(|c| c.essential.clone()) else { return };
    let over = essential::overridden(&layer);
    let lt = layer.layer_time(app.session.time());
    ui.separator();
    ui.label(RichText::new(tr_args("Essential Properties — {}", &[&layer.name])).strong());
    let group = essential::group(&layer).cloned().unwrap_or_else(|| effectcraft_engine::project::PropGroup::new(0, "", ""));
    for c in eg.flat() {
        let m = essential::match_id(c.id);
        let mut pr = None;
        group.walk("", &mut |_, x| {
            if x.match_id == m {
                pr = Some(x.clone());
            }
        });
        let Some(p) = pr else { continue };
        let is_over = over.contains(&p.uid);
        ui.horizontal(|ui| {
            ui.label(RichText::new(if is_over { "●" } else { "○" }).color(if is_over { t.accent } else { t.text_faint })).on_hover_text(if is_over {
                "Overridden in this instance"
            } else {
                "Follows the source composition"
            });
            ui.label(&c.name);
            let auto = format!("essential.instance.{}.value", c.id);
            match c.kind {
                EgKind::Media { .. } => {
                    let cur = ItemId(p.value.as_f64() as u64);
                    let cur_name = app.session.project.item(cur).map(|i| i.name.clone()).unwrap_or_default();
                    let items: Vec<(ItemId, String)> = app
                        .session
                        .project
                        .items
                        .values()
                        .filter(|i| matches!(&i.kind, ItemKind::Footage(f) if f.has_video))
                        .map(|i| (i.id, i.name.clone()))
                        .collect();
                    let r = egui::ComboBox::from_id_salt(("eg-media", layer.id.0, c.id)).selected_text(cur_name).show_ui(ui, |ui| {
                        for (id, name) in &items {
                            if ui.selectable_label(*id == cur, name).clicked() {
                                actions.push(("essential.set".into(), json!({"layer": layer.id.0, "control": c.id, "item": id.0})));
                            }
                        }
                    });
                    app.auto.add(&auto, r.response.rect, &c.name);
                }
                _ => {
                    let v = p.value_at(lt);
                    if let Some(nv) = value_widget(app, ui, &auto, essential::effective_type(eg.resolve(c.id).unwrap_or(c), &p), &p, &v) {
                        actions.push((
                            "essential.set".into(),
                            json!({"layer": layer.id.0, "control": c.id, "value": nv, "merge": format!("egi-{}-{}", layer.id.0, c.id)}),
                        ));
                    }
                }
            }
            if is_over {
                let r = ui.small_button("Revert");
                app.auto.add(&format!("essential.instance.{}.revert", c.id), r.rect, "Revert");
                if r.clicked() {
                    actions.push(("essential.revert".into(), json!({"layer": layer.id.0, "control": c.id})));
                }
                let r = ui.small_button("Push");
                app.auto.add(&format!("essential.instance.{}.push", c.id), r.rect, "Push to Comp");
                if r.clicked() {
                    actions.push(("essential.pushToComp".into(), json!({"layer": layer.id.0, "control": c.id})));
                }
            }
        });
    }
    ui.horizontal(|ui| {
        let any = !over.is_empty();
        let r = ui.add_enabled(any, egui::Button::new(tr("Push All to Comp")));
        app.auto.add("essential.instance.pushAll", r.rect, "Push Override Values to Source");
        if r.clicked() {
            actions.push(("essential.pushToComp".into(), json!({"layer": layer.id.0})));
        }
        let r = ui.add_enabled(any, egui::Button::new(tr("Revert All")));
        app.auto.add("essential.instance.revertAll", r.rect, "Revert All");
        if r.clicked() {
            actions.push(("essential.revert".into(), json!({"layer": layer.id.0})));
        }
    });
}
