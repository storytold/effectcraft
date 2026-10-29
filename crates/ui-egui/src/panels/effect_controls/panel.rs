use super::common::{Actions, PARAM_NAME_X, ROW, effect_active, selected_layer};
use super::editors::{curves_editor, editor_height, histogram_only, levels_editor, object_matte_editor, roto_editor, warp_editor};
use super::rows::{FxScope, group_rows, toggle_closed};
use super::{Align2, Color32, EffectcraftApp, EvalCtx, GroupKind, Icon, Layer, PropGroup, Rect, Sense, Stroke, Tokens, json, pos2, vec2, widgets};

// ---------------------------------------------------------------------------------------------

/// Open the rename field on effect `uid` (Enter with the effect selected), name selected.
pub fn start_rename(ctx: &egui::Context, uid: u64, name: &str) {
    let id = egui::Id::unique(("ec-rename", uid));
    ctx.data_mut(|d| d.insert_temp(id, name.to_string()));
    ctx.data_mut(|d| d.insert_temp(id.with("select"), true));
    ctx.memory_mut(|m| m.request_focus(id.with("field")));
}

/// Where an effect header's parts sit, for its rename field.
struct EffectHdr<'a> {
    g: &'a PropGroup,
    shown: &'a str,
    r: Rect,
    tw: Rect,
    reset: Rect,
    rename_id: egui::Id,
    layer_id: u64,
}

/// The effect header's name field while renaming: Enter or blur commits, Escape cancels.
fn rename_field(app: &mut EffectcraftApp, ui: &mut egui::Ui, h: &EffectHdr, mut value: String, actions: &mut Actions) {
    let EffectHdr { g, shown, r, tw, reset, rename_id, layer_id } = *h;
    let ctx = ui.ctx().clone();
    let name_rect = Rect::from_min_max(pos2(tw.max.x + 6.0, r.min.y + 2.0), pos2(reset.min.x - 6.0, r.max.y - 2.0));
    let response = ui.put(name_rect, egui::TextEdit::singleline(&mut value).id(rename_id.with("field")).font(Tokens::semibold(12.0)));
    // Focus the field the frame it first exists: focusing an id not yet on screen leaves
    // the accessibility tree with a focus it doesn't contain.
    if ctx.data_mut(|d| d.remove_temp::<bool>(rename_id.with("start"))).is_some() {
        crate::widgets::select_all(&ctx, response.id, &value);
        response.request_focus();
    }
    app.auto.add(&format!("effectControls.effect.{}.nameEdit", g.uid), name_rect, "Effect name");
    if ctx.data_mut(|d| d.remove_temp::<bool>(rename_id.with("select"))).unwrap_or(false) {
        widgets::select_all(&ctx, response.id, &value);
    }
    // Escape cancels. egui drops the field's focus on Escape in the same frame, so the
    // key is checked on the focus being lost as well as held.
    let escape = (response.has_focus() || response.lost_focus()) && ui.input(|i| i.key_pressed(egui::Key::Escape));
    let commit = response.lost_focus() || (response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
    if escape || commit {
        ctx.data_mut(|d| d.remove::<String>(rename_id));
        // The field opens with the drawn name, so committing it unchanged is not a rename.
        if !escape && !value.trim().is_empty() && value != shown {
            actions.push(("prop.renameGroup".into(), json!({"layer": layer_id, "prop": g.uid, "name": value})));
        }
    } else {
        ctx.data_mut(|d| d.insert_temp(rename_id, value));
    }
}

struct EditorCx<'a> {
    bp: &'a egui::Painter,
    layer: &'a Layer,
    ectx: &'a EvalCtx<'a>,
}

/// An effect's custom editor (curves, levels, roto...), or the histogram-only fallback.
fn effect_editor(app: &mut EffectcraftApp, ui: &mut egui::Ui, cx: &EditorCx, g: &PropGroup, er: Rect, actions: &mut Actions) {
    let GroupKind::Effect { effect } = &g.kind else { return };
    match effect.as_str() {
        "ec.color.curves" => curves_editor(app, ui, cx.bp, cx.layer, g, cx.ectx, er, actions),
        effectcraft_engine::effects::warp_stab::ID => warp_editor(app, ui, cx.bp, cx.layer, g, er, actions),
        effectcraft_engine::effects::camera_tracker::ID => crate::panels::camera_tracker_ui::editor(app, ui, cx.bp, cx.layer, g, er, actions),
        effectcraft_engine::effects::roto::ID => roto_editor(app, ui, cx.bp, cx.layer, g, er, actions),
        effectcraft_engine::effects::object_matte::ID => object_matte_editor(app, ui, cx.bp, cx.layer, g, er, actions),
        "ec.color.levels" | "ec.color.levelsic" => levels_editor(app, ui, cx.bp, cx.layer, g, effect, cx.ectx, er, actions),
        crate::panels::fx_editors::GLOW | crate::panels::fx_editors::RESHAPE | crate::panels::fx_editors::EXTRACTOR => {
            crate::panels::fx_editors::header_editor(app, ui, cx.bp, cx.layer, g, effect, cx.ectx, er, actions)
        }
        _ => histogram_only(app, ui, cx.bp, g, er),
    }
}

struct ReorderDrag<'a> {
    tops: &'a [(u64, f32)],
    y: f32,
    du: u64,
    drag_id: egui::Id,
    layer_id: u64,
}

/// Drag an effect header to reorder: the insertion line between headers, and the drop.
fn reorder_drag(ctx: &egui::Context, bp: &egui::Painter, body: Rect, t: &Tokens, st: &ReorderDrag, actions: &mut Actions) {
    let ReorderDrag { tops, y, du, drag_id, layer_id } = *st;
    if let Some(pos) = ctx.pointer_interact_pos() {
        let slot = tops.iter().position(|(_, ty)| pos.y < *ty + (ROW + 2.0) / 2.0).unwrap_or(tops.len());
        let ly = tops.get(slot).map(|(_, ty)| *ty).unwrap_or(y - 4.0);
        bp.line_segment([pos2(body.min.x + 4.0, ly), pos2(body.max.x - 4.0, ly)], Stroke::new(2.0, t.accent));
        if ctx.input(|i| i.pointer.any_released()) {
            if let Some(from) = tops.iter().position(|(u, _)| *u == du) {
                let to = if slot > from { slot - 1 } else { slot };
                if to != from {
                    actions.push(("effect.reorder".into(), json!({"layer": layer_id, "effect": du, "index": to + 1})));
                }
            }
            ctx.data_mut(|d| d.remove::<u64>(drag_id));
        }
    }
    if !ctx.input(|i| i.pointer.any_down()) {
        ctx.data_mut(|d| d.remove::<u64>(drag_id));
    }
}

/// Drop effects and animation presets here.
fn drop_payload(ui: &egui::Ui, rect: Rect, p: &egui::Painter, t: &Tokens, layer_id: u64, actions: &mut Actions) {
    let ctx = ui.ctx().clone();
    if let Some(payload) = egui::DragAndDrop::payload::<crate::panels::DragPayload>(&ctx)
        && ui.rect_contains_pointer(rect)
        && let Some((id, params)) = match payload.as_ref() {
            crate::panels::DragPayload::Effect(e) => Some(("effect.apply", json!({"effect": e, "layers": [layer_id]}))),
            crate::panels::DragPayload::Preset { preset, .. } => Some(preset.apply_to(layer_id)),
            _ => None,
        }
    {
        p.rect_stroke(rect, 0.0, Stroke::new(2.0, t.accent), egui::StrokeKind::Inside);
        if ctx.input(|i| i.pointer.any_released()) {
            actions.push((id.into(), params));
            egui::DragAndDrop::clear_payload(&ctx);
        }
    }
}

fn about_button(app: &mut EffectcraftApp, ui: &mut egui::Ui, bp: &egui::Painter, t: &Tokens, effect: &str, g: &PropGroup, r: Rect) {
    // About…: the effect's name and category (AE's About dialog).
    let about = Rect::from_min_size(pos2(r.max.x - 50.0, r.center().y - 9.0), vec2(44.0, 18.0));
    let aresp = ui.interact(about, egui::Id::unique(("ec-about", g.uid)), Sense::click());
    bp.text(about.center(), Align2::CENTER_CENTER, crate::i18n::tr("About..."), Tokens::ui(11.5), if aresp.hovered() { t.hot_text } else { t.text_dim });
    app.auto.add(&format!("effectControls.effect.{}.about", g.uid), about, "About");
    let apop = egui::Id::unique(("ec-about-pop", g.uid));
    if aresp.clicked() {
        widgets::open_popup(ui, apop);
    }
    if let Some(spec) = effectcraft_engine::effects::find(effect) {
        let lines = vec![
            crate::i18n::effects::name(spec.id, spec.name),
            format!("Category: {}", crate::i18n::effects::category(spec.category)),
            "EffectCraft built-in effect (MIT OR Apache-2.0)".to_string(),
        ];
        let _ = widgets::popup_menu(ui, apop, about.left_bottom(), &lines, None);
    }
}

/// The Effect Controls panel for the selected layer: a header with the comp and layer names, then
/// one header per effect (Fx toggle, twirl, rename, Reset, About, label bar, reorder drag and its
/// right-click menu) followed by that effect's custom editor and its property rows. Dragging
/// effects or presets onto the panel applies them; a right-click on empty space opens the Effect
/// menu. The rows read from a snapshot of the project, and every edit is queued and run through
/// [`crate::menus::invoke`], with failures reported in `app.ui.status`.
pub fn show(app: &mut EffectcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let p = ui.painter().with_clip_rect(rect);
    let ctx = ui.ctx().clone();
    let Some(layer) = selected_layer(app) else {
        p.text(rect.center(), Align2::CENTER_CENTER, crate::i18n::tr("Select a layer to see its effects"), Tokens::ui(12.0), t.text_faint);
        return;
    };
    let Some(cid) = app.session.active_comp_id() else { return };
    let comp = app.session.project.comp_arc(cid).unwrap_or_else(|| effectcraft_engine::project::Comp::new(1, 1, Default::default(), Default::default()).into());
    let comp_name = app.session.project.item(cid).map(|i| i.name.clone()).unwrap_or_default();
    let snap_project = app.session.project.clone();
    let snap_expr = app.session.expr.clone();
    let ectx = EvalCtx { project: &snap_project, comp_id: cid, comp: &comp, time: app.session.time(), expr: snap_expr.as_deref(), footage: None };
    let hdr = Rect::from_min_size(rect.min, vec2(rect.width(), 24.0));
    p.text(pos2(hdr.min.x + 10.0, hdr.center().y), Align2::LEFT_CENTER, format!("{} • {}", comp_name, layer.name), Tokens::ui(11.5), t.text_dim);
    p.line_segment([hdr.left_bottom(), hdr.right_bottom()], Stroke::new(1.0, t.separator));
    let body = Rect::from_min_max(pos2(rect.min.x, hdr.max.y), rect.max);
    let scroll = widgets::PanelScroll::begin(ui, egui::Id::unique("ec-scroll"), body);
    let mut y = body.min.y + 4.0 - scroll.offset;
    let mut actions: Actions = vec![];
    let bp = p.with_clip_rect(body);
    // Rows scroll under the header: clip their widgets (and hit tests) to the body.
    let panel_clip = ui.clip_rect();
    ui.set_clip_rect(body.intersect(panel_clip));
    let fx: Vec<PropGroup> = layer.effects().map(|f| f.groups().cloned().collect()).unwrap_or_default();
    if fx.is_empty() {
        bp.text(
            pos2(body.center().x, body.min.y + 40.0),
            Align2::CENTER_CENTER,
            "No effects. Drag one from Effects & Presets.",
            Tokens::ui(12.0),
            t.text_faint,
        );
    }
    // Clicking empty space deselects the effects.
    let bg = ui.interact(body, egui::Id::unique("ec-bg"), Sense::click());
    if bg.clicked() {
        app.session.state.selected_props.retain(|(l, u)| !(*l == layer.id && fx.iter().any(|g| effect_active(g, layer.id, &[(*l, *u)]))));
    }
    let has_clip = !app.session.state.effect_clipboard.is_empty();
    let drag_id = egui::Id::unique("ec-drag-effect");
    let dragging: Option<u64> = ctx.data(|d| d.get_temp(drag_id));
    // Header tops, for drag-to-reorder.
    let mut tops: Vec<(u64, f32)> = vec![];
    for (fi, g) in fx.iter().enumerate() {
        let GroupKind::Effect { effect } = &g.kind else { continue };
        // The header draws the effect's name in the interface language; the automation labels
        // below keep the property tree's name, which is the registry's English name (#821).
        let shown = crate::i18n::effects::group(effect, &g.name);
        let r = Rect::from_min_size(pos2(body.min.x, y), vec2(body.width(), ROW + 2.0));
        tops.push((g.uid, r.min.y));
        y += ROW + 2.0;
        let open = !app.ui.fx_closed.contains(&g.uid);
        let selected = app.session.state.selected_props.iter().any(|(l, u)| *l == layer.id && *u == g.uid);
        let active = effect_active(g, layer.id, &app.session.state.selected_props);
        bp.rect_filled(r, 0.0, if active { t.fx_header_active } else { t.fx_header });
        // The effect's colour label (26.5): a bar on the header's left edge.
        if let Some(lab) = g.label.filter(|l| *l != effectcraft_engine::color::Label::None) {
            let [lr, lg, lb] = app.session.prefs.label_rgb(lab);
            bp.rect_filled(Rect::from_min_size(r.min, vec2(4.0, r.height())), 0.0, Color32::from_rgb(lr, lg, lb));
            app.auto.add(
                &format!("effectControls.effect.{}.label", g.uid),
                Rect::from_min_size(r.min, vec2(4.0, r.height())),
                &app.session.prefs.label_name(lab),
            );
        }
        let fxr = Rect::from_center_size(pos2(r.min.x + 14.0, r.center().y), vec2(16.0, 16.0));
        if widgets::icon_toggle(ui, fxr, Icon::Fx, g.enabled, &t, egui::Id::unique(("ec-fx", g.uid)), Sense::click()).clicked() {
            actions.push(("effect.toggle".into(), json!({"layer": layer.id.0, "effect": g.uid})));
        }
        app.auto.add(&format!("effectControls.effect.{}.fx", g.uid), fxr, &g.name);
        let tw = Rect::from_center_size(pos2(r.min.x + 32.0, r.center().y), vec2(12.0, 12.0));
        if widgets::twirl(ui, tw, open, egui::Id::unique(("ec-tw", g.uid)), &t).clicked() {
            toggle_closed(app, g, open, ui.input(|i| i.modifiers.alt || i.modifiers.command));
        }
        app.auto.add(&format!("effectControls.effect.{}.twirl", g.uid), tw, &g.name);
        let rename_id = egui::Id::unique(("ec-rename", g.uid));
        let editing: Option<String> = ctx.data(|d| d.get_temp(rename_id));
        let reset = Rect::from_min_size(pos2(r.max.x - 96.0, r.center().y - 9.0), vec2(40.0, 18.0));
        let hdr_cx = EffectHdr { g, shown: &shown, r, tw, reset, rename_id, layer_id: layer.id.0 };
        if let Some(value) = editing.as_ref().cloned() {
            rename_field(app, ui, &hdr_cx, value, &mut actions);
        } else {
            let name = bp.text(pos2(tw.max.x + 6.0, r.center().y), Align2::LEFT_CENTER, &shown, Tokens::semibold(12.0), t.text);
            app.auto.add(&format!("effectControls.effect.{}.name", g.uid), name, &g.name);
        }
        let rresp = ui.interact(reset, egui::Id::unique(("ec-reset", g.uid)), Sense::click());
        bp.text(reset.center(), Align2::CENTER_CENTER, crate::i18n::tr("Reset"), Tokens::ui(11.5), if rresp.hovered() { t.hot_text } else { t.text_dim });
        app.auto.add(&format!("effectControls.effect.{}.reset", g.uid), reset, "Reset");
        if rresp.clicked() {
            actions.push(("effect.reset".into(), json!({"layer": layer.id.0, "effect": g.uid})));
        }
        about_button(app, ui, &bp, &t, effect, g, r);
        let hresp = ui.interact(
            Rect::from_min_max(pos2(tw.max.x, r.min.y), pos2(reset.min.x - 4.0, r.max.y)),
            egui::Id::unique(("ec-hdr", g.uid)),
            if editing.is_some() { Sense::hover() } else { Sense::click_and_drag() },
        );
        if hresp.double_clicked() {
            ctx.data_mut(|d| d.insert_temp(rename_id, shown.clone()));
            ctx.data_mut(|d| d.insert_temp(rename_id.with("start"), true));
        }
        hresp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, active, &shown));
        app.auto.add(&format!("effectControls.effect.{}", g.uid), r, &g.name);
        if hresp.clicked() || hresp.drag_started() || hresp.secondary_clicked() {
            let add = ui.input(|i| i.modifiers.shift || i.modifiers.command);
            if add && hresp.clicked() {
                if selected {
                    app.session.state.selected_props.retain(|(l, u)| !(*l == layer.id && *u == g.uid));
                } else {
                    app.session.state.selected_props.push((layer.id, g.uid));
                }
            } else if !selected || hresp.clicked() {
                app.session.state.selected_props = vec![(layer.id, g.uid)];
            }
        }
        if hresp.drag_started() {
            ctx.data_mut(|d| d.insert_temp(drag_id, g.uid));
        }
        crate::widgets::context_menu(&hresp, |ui| {
            let lid = layer.id.0;
            let mut picked_label: Option<usize> = None;
            let mut item = |ui: &mut egui::Ui, label: &str, enabled: bool, cmd: &str, params: serde_json::Value| {
                if ui.add_enabled(enabled, egui::Button::new(crate::i18n::context_item(cmd, &params, label))).clicked() {
                    actions.push((cmd.into(), params));
                    ui.close();
                }
            };
            item(ui, "Copy", true, "effect.copy", json!({"layer": lid, "effect": g.uid}));
            item(ui, "Paste", has_clip, "effect.paste", json!({"layers": [lid]}));
            item(ui, "Duplicate", true, "effect.duplicate", json!({"layer": lid, "effect": g.uid}));
            if ui.button(crate::i18n::tr("Rename")).clicked() {
                ctx.data_mut(|d| d.insert_temp(rename_id, shown.clone()));
                ctx.data_mut(|d| d.insert_temp(rename_id.with("start"), true));
                ui.close();
            }
            ui.separator();
            ui.menu_button(crate::i18n::tr("Label"), |ui| {
                let cur = g.label.unwrap_or(effectcraft_engine::color::Label::None);
                for (i, l) in effectcraft_engine::color::Label::ALL.into_iter().enumerate() {
                    if widgets::label_entry(ui, l, &app.session.prefs.label_name(l), l == cur, &t).clicked() {
                        picked_label = Some(i);
                        ui.close();
                    }
                }
            });
            item(ui, "Reset", true, "effect.reset", json!({"layer": lid, "effect": g.uid}));
            item(ui, "Remove", true, "effect.remove", json!({"layer": lid, "effect": g.uid}));
            ui.separator();
            item(ui, "Move Up", fi > 0, "effect.reorder", json!({"layer": lid, "effect": g.uid, "index": fi.max(1)}));
            item(ui, "Move Down", fi + 1 < fx.len(), "effect.reorder", json!({"layer": lid, "effect": g.uid, "index": fi + 2}));
            if let Some(i) = picked_label {
                actions.push(("effect.setLabel".into(), json!({"layer": lid, "effect": g.uid, "label": i})));
            }
        });
        // What the effect can't render as asked (an OCIO config it can't read, transforms it
        // passes through), under its name.
        if let Some(w) = effectcraft_engine::effects::warning(g, &mut |pr| ectx.value(&layer, pr)) {
            let galley = bp.layout(format!("⚠ {w}"), Tokens::ui(11.0), t.warning, (body.width() - 20.0).max(60.0));
            let wr = Rect::from_min_size(pos2(body.min.x + 10.0, y + 3.0), galley.size());
            bp.galley(wr.min, galley, t.warning);
            app.auto.add(&format!("effectControls.effect.{}.warning", g.uid), wr, &w);
            y += wr.height() + 6.0;
        }
        if open {
            let eh = editor_height(effect, g, body.width());
            if eh > 0.0 {
                let er = Rect::from_min_size(pos2(body.min.x, y), vec2(body.width(), eh));
                y += eh;
                if er.max.y >= body.min.y && er.min.y <= body.max.y {
                    // Curves, Levels and other custom editors do not use prop_row.
                    // Clicking their controls must activate the owner as well.
                    if ui.input(|i| i.pointer.primary_pressed()) && ui.rect_contains_pointer(er) {
                        actions.push(("prop.select".into(), json!({"layer":layer.id.0,"prop":g.uid})));
                    }
                    effect_editor(app, ui, &EditorCx { bp: &bp, layer: &layer, ectx: &ectx }, g, er, &mut actions);
                }
            }
            let scope = FxScope { effect: effect.as_str(), root: g, path: String::new() };
            group_rows(app, ui, &bp, &layer, g, &ectx, body, &mut y, 0, &scope, &mut actions);
            if effect == "ec.color.tint" {
                let r = Rect::from_min_size(pos2(body.min.x + PARAM_NAME_X, y + 2.0), vec2(120.0, ROW));
                let label = crate::i18n::tr("Swap Colors");
                let resp = widgets::text_button(ui, r, label, false, &t, egui::Id::unique(("ec-tint-swap", g.uid)));
                app.auto.add_response(&format!("effectControls.effect.{}.swapColors", g.uid), &resp, label);
                if resp.clicked() {
                    actions.push(("prop.select".into(), json!({"layer": layer.id.0, "prop": g.uid})));
                    actions.push(("effect.swapTintColors".into(), json!({"layer": layer.id.0, "effect": g.uid})));
                }
                y += ROW + 4.0;
            }
        }
        y += 4.0;
    }
    if let Some(du) = dragging {
        reorder_drag(&ctx, &bp, body, &t, &ReorderDrag { tops: &tops, y, du, drag_id, layer_id: layer.id.0 }, &mut actions);
    }
    // Right-click anywhere a control has no menu of its own: the Effect menu, which applies to
    // the selected layers (After Effects). Drawn after the rows, whose menus open first.
    let menu_id = bg.id.with("effect-menu");
    let open = ui.input(|i| i.pointer.secondary_clicked())
        && ui.rect_contains_pointer(body)
        && (!egui::Popup::is_any_open(&ctx) || egui::Popup::is_id_open(&ctx, menu_id));
    egui::Popup::context_menu(&bg).style(crate::widgets::menu_style).id(menu_id).open_memory(open.then_some(egui::SetOpenCommand::Bool(true))).show(|ui| {
        ui.set_min_width(200.0);
        actions.extend(crate::menus::menu_contents(app, ui, "Effect"));
    });
    ui.set_clip_rect(panel_clip);
    let content_h = y + scroll.offset - body.min.y;
    scroll.end(ui, &mut app.auto, "effectControls.scroll", content_h, &t);
    drop_payload(ui, rect, &p, &t, layer.id.0, &mut actions);
    if ui.input(|i| i.pointer.primary_down()) && actions.iter().any(|(id, _)| id == "prop.set") {
        app.ui.viewer.property_interacting = true;
        ctx.request_repaint();
    }
    for (id, params) in actions {
        if let Err(e) = crate::menus::invoke(app, &ctx, &id, params) {
            app.ui.status = e;
        }
    }
}
