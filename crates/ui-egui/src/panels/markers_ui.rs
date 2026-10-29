//! Markers in the Timeline (composition markers in the ruler, layer markers on layer bars) and
//! the Composition/Layer Marker dialog.
//!
//! Gestures, as in After Effects: drag moves a marker (snapping to the current time, the work
//! area and other markers), Alt-drag sets its duration, Cmd-click (Ctrl-click elsewhere)
//! deletes it, double-click opens the Marker dialog, right-click offers Settings / Convert /
//! Delete. Protected regions (Responsive Design) shade the timeline. A marker dragged out of the
//! composition marker bin, at the right end of the time ruler, adds a composition marker where it
//! is dropped. Every change is an engine command, so it is undoable and agent-drivable.

use crate::i18n::tr;
use effectcraft_engine::project::{Comp, Layer, Marker};
use effectcraft_engine::time::Tick;
use egui::{Align2, Color32, Pos2, Rect, Sense, Stroke, pos2, vec2};
use serde_json::{Value, json};

use super::timeline::TMap;
use crate::icons::{self, Icon};
use crate::theme::Tokens;
use crate::{Dialog, EffectcraftApp};

/// Which marker a gesture or the dialog refers to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MarkerRef {
    /// `None` = composition marker.
    pub layer: Option<u64>,
    pub index: usize,
}

impl MarkerRef {
    fn params(&self) -> Value {
        match self.layer {
            Some(l) => json!({"layer": l, "index": self.index}),
            None => json!({"index": self.index}),
        }
    }
    fn auto_id(&self) -> String {
        match self.layer {
            Some(l) => format!("timeline.layer.{l}.marker.{}", self.index),
            None => format!("timeline.marker.{}", self.index),
        }
    }
}

/// Marker dialog fields.
#[derive(Clone, Debug, Default)]
pub struct MarkerDraft {
    pub target: Option<MarkerRef>,
    pub title: String,
    pub time: f64,
    pub duration: f64,
    pub comment: String,
    pub chapter: String,
    pub url: String,
    pub frame_target: String,
    pub cue: bool,
    pub cue_navigation: bool,
    pub cue_name: String,
    pub cue_params: Vec<(String, String)>,
    pub protected: bool,
    pub label: usize,
}

/// Open the Composition/Layer Marker dialog for `m`.
pub fn open_dialog(app: &mut EffectcraftApp, m: MarkerRef) -> Result<(), String> {
    let mut p = json!({});
    if let Some(l) = m.layer {
        p["layer"] = json!(l);
    }
    let list = app.session.execute("markers.list", p).map_err(|e| e.to_string())?;
    let v = list.get(m.index).ok_or("no such marker")?;
    let s = |k: &str| v[k].as_str().unwrap_or_default().to_string();
    let cue = &v["cuePoint"];
    app.dialog_state.marker = MarkerDraft {
        target: Some(m),
        title: if m.layer.is_some() { "Layer Marker".into() } else { "Composition Marker".into() },
        time: v["time"].as_f64().unwrap_or(0.0),
        duration: v["duration"].as_f64().unwrap_or(0.0),
        comment: s("comment"),
        chapter: s("chapter"),
        url: s("url"),
        frame_target: s("frameTarget"),
        cue: !cue.is_null(),
        cue_navigation: cue["navigation"].as_bool().unwrap_or(false),
        cue_name: cue["name"].as_str().unwrap_or_default().to_string(),
        cue_params: cue["params"]
            .as_array()
            .map(|a| a.iter().map(|kv| (kv[0].as_str().unwrap_or_default().to_string(), kv[1].as_str().unwrap_or_default().to_string())).collect())
            .unwrap_or_default(),
        protected: v["protected"].as_bool().unwrap_or(false),
        label: effectcraft_engine::color::Label::from_name(&s("label"))
            .and_then(|l| effectcraft_engine::color::Label::ALL.iter().position(|x| *x == l))
            .unwrap_or(0),
    };
    app.dialog = Some(Dialog::Marker);
    Ok(())
}

/// The command parameters of the dialog's current fields.
pub fn draft_params(d: &MarkerDraft) -> Value {
    let mut p = d.target.map(|t| t.params()).unwrap_or_else(|| json!({}));
    p["time"] = json!(d.time);
    p["duration"] = json!(d.duration);
    p["comment"] = json!(d.comment);
    p["chapter"] = json!(d.chapter);
    p["url"] = json!(d.url);
    p["frameTarget"] = json!(d.frame_target);
    p["protected"] = json!(d.protected);
    p["label"] = json!(d.label);
    p["cuePoint"] = if d.cue {
        json!({"name": d.cue_name, "navigation": d.cue_navigation, "params": d.cue_params.iter().filter(|(k, _)| !k.is_empty()).map(|(k, v)| json!([k, v])).collect::<Vec<_>>()})
    } else {
        Value::Null
    };
    p
}

fn text_field(ui: &mut egui::Ui, app: &mut EffectcraftApp, id: &str, v: &mut String, w: f32) {
    let r = ui.add(egui::TextEdit::singleline(v).desired_width(w));
    app.auto.add(id, r.rect, id);
}

/// The Composition/Layer Marker dialog.
pub fn dialog(app: &mut EffectcraftApp, ctx: &egui::Context, t: &Tokens) {
    let mut d = app.dialog_state.marker.clone();
    let (mut ok, mut cancel, mut delete) = (false, false, false);
    // Cmd+Backspace (Ctrl elsewhere) is Delete, in a text field too.
    let delete_key = super::dialogs::ui_command_key(ctx, egui::Key::Backspace);
    // Cmd+Enter (Ctrl elsewhere) is OK, also in Comment, where Enter types a new line.
    let ok_key = super::dialogs::ui_command_key(ctx, egui::Key::Enter);
    let title = d.title.clone();
    super::dialogs::modal(ctx, &title, vec2(520.0, 560.0), t, |ui| {
        egui::Grid::new("marker-grid").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            ui.label(tr("Time:"));
            let r = ui.add(crate::widgets::drag_value(egui::DragValue::new(&mut d.time).speed(0.01).range(0.0..=1e6).suffix(" s").max_decimals(3)));
            app.auto.add("dialog.marker.time", r.rect, "Time");
            ui.end_row();
            ui.label(tr("Duration:"));
            let r = ui.add(crate::widgets::drag_value(egui::DragValue::new(&mut d.duration).speed(0.01).range(0.0..=1e6).suffix(" s").max_decimals(3)));
            app.auto.add("dialog.marker.duration", r.rect, "Duration");
            ui.end_row();
            ui.label(tr("Comment:"));
            let r = ui.add(egui::TextEdit::multiline(&mut d.comment).desired_rows(3).desired_width(320.0));
            crate::widgets::focus_on_open(ui.ctx(), &r, &d.comment);
            app.auto.add("dialog.marker.comment", r.rect, "Comment");
            ui.end_row();
            ui.label(tr("Chapter:"));
            text_field(ui, app, "dialog.marker.chapter", &mut d.chapter, 320.0);
            ui.end_row();
            ui.label(tr("URL:"));
            text_field(ui, app, "dialog.marker.url", &mut d.url, 320.0);
            ui.end_row();
            ui.label(tr("Frame Target:"));
            text_field(ui, app, "dialog.marker.frameTarget", &mut d.frame_target, 320.0);
            ui.end_row();
        });
        ui.add_space(8.0);
        let r = ui.checkbox(&mut d.cue, tr("Cue Point"));
        app.auto.add("dialog.marker.cue", r.rect, "Cue Point");
        ui.add_enabled_ui(d.cue, |ui| {
            ui.horizontal(|ui| {
                let r = ui.radio_value(&mut d.cue_navigation, false, tr("Event"));
                app.auto.add("dialog.marker.cue.event", r.rect, "Event");
                let r = ui.radio_value(&mut d.cue_navigation, true, tr("Navigation"));
                app.auto.add("dialog.marker.cue.navigation", r.rect, "Navigation");
                ui.label(tr("Name:"));
                text_field(ui, app, "dialog.marker.cue.name", &mut d.cue_name, 160.0);
            });
            let mut remove = None;
            for (i, (k, v)) in d.cue_params.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(tr("Parameter Name:"));
                    text_field(ui, app, &format!("dialog.marker.cue.param.{i}.name"), k, 110.0);
                    ui.label(tr("Value:"));
                    text_field(ui, app, &format!("dialog.marker.cue.param.{i}.value"), v, 110.0);
                    let r = ui.small_button("−");
                    app.auto.add(&format!("dialog.marker.cue.param.{i}.remove"), r.rect, "Remove");
                    if r.clicked() {
                        remove = Some(i);
                    }
                });
            }
            if let Some(i) = remove {
                d.cue_params.remove(i);
            }
            let r = ui.small_button("+ Parameter");
            app.auto.add("dialog.marker.cue.addParam", r.rect, "Add Parameter");
            if r.clicked() {
                d.cue_params.push((String::new(), String::new()));
            }
        });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let r = ui.checkbox(&mut d.protected, tr("Protected Region"));
            app.auto.add("dialog.marker.protected", r.rect, "Protected Region");
            ui.add_space(20.0);
            ui.label(tr("Label:"));
            let labels = effectcraft_engine::color::Label::ALL;
            let cur = labels.get(d.label).copied().unwrap_or_default();
            let r = craft_ui::ComboBox::from_id_salt("dialog.marker.label").selected_text(app.session.prefs.label_name(cur)).show_ui(ui, |ui| {
                for (i, l) in labels.into_iter().enumerate() {
                    if crate::widgets::label_entry(ui, l, &app.session.prefs.label_name(l), d.label == i, &app.tokens).clicked() {
                        d.label = i;
                    }
                }
            });
            app.auto.add("dialog.marker.label", r.response.rect, "Label");
        });
        ui.add_space(14.0);
        ui.horizontal(|ui| {
            let r = ui.button(tr("Delete"));
            app.auto.add("dialog.marker.delete", r.rect, "Delete");
            delete = r.clicked();
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let r = ui.add(egui::Button::new(egui::RichText::new(tr("   OK   ")).color(Color32::WHITE)).fill(app.tokens.accent));
                app.auto.add("dialog.marker.ok", r.rect, "OK");
                ok = r.clicked();
                let r = ui.button(tr("Cancel"));
                app.auto.add("dialog.marker.cancel", r.rect, "Cancel");
                cancel = r.clicked();
            });
        });
    });
    app.dialog_state.marker = d.clone();
    delete |= delete_key && !ok && !cancel;
    // Enter is OK, unless a text field has it.
    if ok || (!cancel && !delete && (ok_key || super::dialogs::ui_enter(ctx))) {
        if let Err(e) = app.session.execute("markers.set", draft_params(&d)) {
            app.ui.status = e.to_string();
        }
        cancel = true;
    }
    if delete && let Some(m) = d.target {
        if let Err(e) = app.session.execute("markers.delete", m.params()) {
            app.ui.status = e.to_string();
        }
        cancel = true;
    }
    if cancel {
        app.dialog = None;
    }
}

// ---------------------------------------------------------------- timeline drawing + gestures

/// The `egui::Id` of a marker drag in progress: the dragged marker's layer and index (updated as
/// the marker moves) and whether Alt was held when the drag started.
fn drag_state_id() -> egui::Id {
    egui::Id::unique("marker-drag")
}

/// Snap `t` (comp seconds) to the CTI, the work area and other marker times within 8 px.
fn snap(app: &EffectcraftApp, comp: &Comp, tm: TMap, t: f64, skip: MarkerRef) -> f64 {
    let fr = comp.frame_rate;
    let mut t = fr.snap_nearest(Tick::from_seconds_f64(t.max(0.0))).seconds();
    if !app.session.state.snapping {
        return t;
    }
    let mut cands = vec![app.session.time().seconds(), comp.work_area.0.seconds(), comp.work_area.1.seconds()];
    cands.extend(comp.markers.iter().enumerate().filter(|(i, _)| skip != MarkerRef { layer: None, index: *i }).map(|(_, m)| m.time.seconds()));
    let px = |a: f64| (a - t).abs() * tm.pps;
    if let Some(best) = cands.into_iter().filter(|c| px(*c) < 8.0).min_by(|a, b| px(*a).total_cmp(&px(*b))) {
        t = best;
    }
    t
}

/// A marker's colour: its label's, pale yellow without one.
fn marker_color(t: &Tokens, label: effectcraft_engine::color::Label) -> Color32 {
    if label == effectcraft_engine::color::Label::None { Color32::from_rgb(0xd8, 0xd8, 0x60) } else { t.label(label) }
}

/// The marker glyph at `x` from `y0`, `h` points tall: a tab pointing down at the time.
fn marker_shape(x: f32, y0: f32, h: f32) -> Vec<Pos2> {
    let ym = y0 + h * 0.6;
    vec![pos2(x - 4.0, y0), pos2(x + 4.0, y0), pos2(x + 4.0, ym), pos2(x, y0 + h), pos2(x - 4.0, ym)]
}

/// Draw one marker (triangle + duration bar + comment) at `x` in a strip `[y0, y1]` and handle
/// its gestures. `comp_t` is the marker's comp time.
#[allow(clippy::too_many_arguments)]
fn marker(app: &mut EffectcraftApp, ui: &mut egui::Ui, p: &egui::Painter, comp: &Comp, tm: TMap, m: &Marker, comp_t: f64, r: MarkerRef, y0: f32, y1: f32) {
    let t = app.tokens;
    let ctx = ui.ctx().clone();
    let x = tm.x(comp_t);
    let col = marker_color(&t, m.label);
    let h = (y1 - y0).min(10.0);
    let end = (m.duration > Tick(0)).then(|| tm.x(comp_t + m.duration.seconds()));
    if let Some(xe) = end {
        let bar = Rect::from_min_max(pos2(x, y0 + 1.0), pos2(xe, y0 + h - 1.0));
        p.rect_filled(bar, 1.0, col.gamma_multiply(if m.protected { 0.35 } else { 0.5 }));
        if m.protected {
            let mut hx = bar.min.x;
            while hx < bar.max.x {
                p.line_segment([pos2(hx, bar.max.y), pos2((hx + bar.height()).min(bar.max.x), bar.min.y)], Stroke::new(1.0, col));
                hx += 5.0;
            }
        }
        p.line_segment([pos2(xe, y0), pos2(xe, y0 + h)], Stroke::new(1.0, col));
    }
    p.add(egui::Shape::convex_polygon(marker_shape(x, y0, h), col, Stroke::NONE));
    if !m.comment.is_empty() {
        // The name sits in a small outlined box beside the flag, as in After Effects. It is drawn in the theme's field
        // colours, so it follows Dark, Darker and Light.
        let galley = p.layout_no_wrap(m.comment.lines().next().unwrap_or_default().to_string(), Tokens::ui(10.0), t.text);
        let (box_w, mut box_x) = (galley.size().x + 8.0, x + 6.0);
        // A duration that ends under the box would be hidden by its fill: the box steps past the end cap instead.
        if let Some(xe) = end
            && xe > box_x
            && xe < box_x + box_w
        {
            box_x = xe + 3.0;
        }
        let box_rect = Rect::from_min_size(pos2(box_x, y0), vec2(box_w, h.max(galley.size().y + 2.0)));
        p.rect(box_rect, t.radius_sm, t.field_bg, Stroke::new(1.0, t.field_border), egui::StrokeKind::Inside);
        p.galley(pos2(box_rect.min.x + 4.0, box_rect.center().y - galley.size().y / 2.0), galley, t.text);
    }
    let hit = Rect::from_min_max(pos2(x - 5.0, y0 - 1.0), pos2(x + 5.0, y1));
    let id = egui::Id::unique(("marker", r.layer, r.index));
    let resp = ui.interact(hit, id, Sense::click_and_drag());
    app.auto.add(&r.auto_id(), hit, if m.comment.is_empty() { "marker" } else { &m.comment });
    let tip = if m.comment.is_empty() { "Marker".to_string() } else { m.comment.clone() };
    let resp = resp.on_hover_text(tip);
    let mods = ui.input(|i| i.modifiers);
    if resp.double_clicked() {
        if let Err(e) = open_dialog(app, r) {
            app.ui.status = e;
        }
        return;
    }
    if resp.clicked() && (if cfg!(target_os = "macos") { mods.mac_cmd || mods.command } else { mods.ctrl }) {
        let _ = app.session.execute("markers.delete", r.params());
        return;
    }
    if resp.drag_started() {
        ctx.data_mut(|d| d.insert_temp(drag_state_id(), (r.layer, r.index, mods.alt)));
    }
    if resp.dragged()
        && let Some(pt) = resp.interact_pointer_pos()
        && let Some((layer, index, alt)) = ctx.data(|d| d.get_temp::<(Option<u64>, usize, bool)>(drag_state_id()))
    {
        let cur = MarkerRef { layer, index };
        let merge = format!("marker-drag-{layer:?}");
        let mut params = cur.params();
        params["merge"] = json!(merge);
        let tt = snap(app, comp, tm, tm.t(pt.x), cur);
        if alt {
            params["duration"] = json!((tt - comp_t).max(0.0));
        } else {
            params["time"] = json!(tt);
        }
        if let Ok(v) = app.session.execute("markers.set", params)
            && let Some(ni) = v["index"].as_u64()
        {
            ctx.data_mut(|d| d.insert_temp(drag_state_id(), (layer, ni as usize, alt)));
        }
    }
    if resp.drag_stopped() {
        ctx.data_mut(|d| d.remove::<(Option<u64>, usize, bool)>(drag_state_id()));
        app.session.history.merge_key = None;
    }
    crate::widgets::context_menu(&resp, |ui| {
        let run = |ui: &mut egui::Ui, label: &str, cmd: &str, params: Value, app: &mut EffectcraftApp| {
            if ui.button(label).clicked() {
                if cmd == "dialog" {
                    let _ = open_dialog(app, r);
                } else if let Err(e) = app.session.execute(cmd, params) {
                    app.ui.status = e.to_string();
                }
                ui.close();
            }
        };
        run(ui, "Settings…", "dialog", Value::Null, app);
        if r.layer.is_some() {
            run(ui, "Convert to Composition Marker", "markers.convert", r.params(), app);
            run(ui, "Lock Markers", "layer.markersLock", json!({"layers": [r.layer]}), app);
        } else {
            run(ui, "Convert to Layer Marker (selected layers)", "markers.convert", r.params(), app);
        }
        ui.separator();
        run(ui, "Delete This Marker", "markers.delete", r.params(), app);
        if let Some(l) = r.layer {
            run(ui, "Delete All Markers", "layer.deleteAllMarkers", json!({"layers": [l]}), app);
        }
    });
}

/// Composition markers in the ruler strip `[y0, y1]`, protected regions over `rows`.
pub(crate) fn comp_markers(app: &mut EffectcraftApp, ui: &mut egui::Ui, comp: &Comp, tm: TMap, strip: Rect, rows: Rect) {
    let p = ui.painter().with_clip_rect(strip);
    let rp = ui.painter().with_clip_rect(rows);
    for m in comp.markers.iter().filter(|m| m.protected && m.duration > Tick(0)) {
        let (x0, x1) = (tm.x(m.time.seconds()), tm.x((m.time + m.duration).seconds()));
        rp.rect_filled(Rect::from_min_max(pos2(x0, rows.min.y), pos2(x1, rows.max.y)), 0.0, Color32::from_rgba_unmultiplied(0x60, 0x90, 0xd0, 22));
        rp.line_segment([pos2(x0, rows.min.y), pos2(x0, rows.max.y)], Stroke::new(1.0, Color32::from_rgba_unmultiplied(0x60, 0x90, 0xd0, 90)));
        rp.line_segment([pos2(x1, rows.min.y), pos2(x1, rows.max.y)], Stroke::new(1.0, Color32::from_rgba_unmultiplied(0x60, 0x90, 0xd0, 90)));
    }
    for (i, m) in comp.markers.iter().enumerate() {
        marker(app, ui, &p, comp, tm, m, m.time.seconds(), MarkerRef { layer: None, index: i }, strip.min.y, strip.max.y);
    }
}

/// The composition marker bin at the right end of the time ruler (`bin`), as in After Effects
/// (#275): drag a marker out of it and it follows the pointer along the marker strip `strip`
/// (snapping like a dragged marker); dropping it there adds a composition marker at that time,
/// one undo step. Dropped back on the bin or outside the time graph's width, nothing is added.
pub(crate) fn marker_bin(app: &mut EffectcraftApp, ui: &mut egui::Ui, comp: &Comp, tm: TMap, bin: Rect, strip: Rect) {
    let t = app.tokens;
    let resp = ui.interact(bin, egui::Id::unique("marker-bin"), Sense::drag()).on_hover_text(tr("Composition marker bin: drag a marker to the time ruler"));
    app.auto.add("timeline.markerBin", bin, "Composition marker bin");
    let hot = resp.hovered() || resp.dragged();
    if hot {
        ui.ctx().set_cursor_icon(if resp.dragged() { egui::CursorIcon::Grabbing } else { egui::CursorIcon::Grab });
    }
    icons::paint(ui.painter(), bin, Icon::MarkerBin, if hot { t.icon_active } else { t.icon });
    let at = resp.interact_pointer_pos().filter(|p| p.x >= strip.min.x && p.x < strip.max.x).map(|p| {
        let snapped = snap(app, comp, tm, tm.t(p.x), MarkerRef { layer: None, index: usize::MAX });
        snapped.min(comp.duration.seconds())
    });
    let Some(at) = at else { return };
    if resp.dragged() {
        let h = strip.height().min(10.0);
        let col = marker_color(&t, effectcraft_engine::color::Label::None);
        let p = ui.painter().with_clip_rect(strip);
        p.add(egui::Shape::convex_polygon(marker_shape(tm.x(at), strip.min.y, h), col, Stroke::NONE));
    }
    if resp.drag_stopped()
        && let Err(e) = app.session.execute("comp.addMarker", json!({"time": at}))
    {
        app.ui.status = e.to_string();
    }
}

/// Layer markers on a layer's row `r`; on a precomp layer, its comp's markers too (read-only,
/// outlined: hover names them, a double-click opens the nested comp at the marker).
pub(crate) fn layer_markers(app: &mut EffectcraftApp, ui: &mut egui::Ui, clip: Rect, comp: &Comp, layer: &Layer, tm: TMap, r: Rect) {
    let p = ui.painter().with_clip_rect(clip);
    nested_markers(app, ui, &p, comp, layer, tm, r);
    for (i, m) in layer.markers.iter().enumerate() {
        marker(app, ui, &p, comp, tm, m, layer.comp_time(m.time).seconds(), MarkerRef { layer: Some(layer.id.0), index: i }, r.min.y + 3.0, r.max.y - 3.0);
    }
}

/// The nested comp's markers on a precomp layer's bar (`markers.nested`).
fn nested_markers(app: &mut EffectcraftApp, ui: &mut egui::Ui, p: &egui::Painter, comp: &Comp, layer: &Layer, tm: TMap, r: Rect) {
    let effectcraft_engine::project::LayerSource::Comp { item } = layer.source else { return };
    let Some(cid) = app.session.active_comp_id() else { return };
    let project = app.session.project.clone();
    let Some(nc) = project.comp(item) else { return };
    if nc.markers.is_empty() {
        return;
    }
    // Where they land, once per revision (with time remapping that samples the layer's frames).
    let (key, rev) = (egui::Id::unique(("nested-markers", cid.0, layer.id.0)), app.session.revision);
    let cached = ui.ctx().data(|d| d.get_temp::<(u64, Vec<(Tick, usize)>)>(key)).filter(|(r, _)| *r == rev);
    let at = cached.map(|(_, v)| v).unwrap_or_else(|| {
        let v: Vec<(Tick, usize)> =
            effectcraft_engine::commands::markers::nested_markers(&project, cid, comp, layer).into_iter().map(|(t, i, _)| (t, i)).collect();
        ui.ctx().data_mut(|d| d.insert_temp(key, (rev, v.clone())));
        v
    });
    let list: Vec<(Tick, usize, &Marker)> = at.into_iter().filter_map(|(t, i)| Some((t, i, nc.markers.get(i)?))).collect();
    let t = app.tokens;
    let name = project.item(item).map(|i| i.name.clone()).unwrap_or_default();
    let (y0, h) = (r.min.y + 3.0, (r.height() - 6.0).min(10.0));
    for (ct, i, m) in list {
        let x = tm.x(ct.seconds());
        let col = marker_color(&t, m.label).gamma_multiply(0.75);
        p.add(egui::Shape::closed_line(marker_shape(x, y0, h), Stroke::new(1.2, col)));
        if !m.comment.is_empty() {
            p.text(pos2(x + 7.0, y0 + h / 2.0), Align2::LEFT_CENTER, m.comment.lines().next().unwrap_or_default(), Tokens::ui(10.0), t.text_dim);
        }
        let hit = Rect::from_min_max(pos2(x - 5.0, y0 - 1.0), pos2(x + 5.0, r.max.y - 3.0));
        let tip = if m.comment.is_empty() { format!("Marker in {name}") } else { format!("{} (marker in {name})", m.comment) };
        let resp = ui.interact(hit, egui::Id::unique(("nested-marker", layer.id.0, i)), Sense::click()).on_hover_text(&tip);
        app.auto.add(&format!("timeline.layer.{}.nestedMarker.{i}", layer.id.0), hit, &tip);
        if resp.double_clicked() {
            let opened =
                app.session.execute("comp.open", json!({"comp": item.0})).and_then(|_| app.session.execute("time.set", json!({"time": m.time.seconds()})));
            if let Err(e) = opened {
                app.ui.status = e.to_string();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_dialog_reads_and_writes_every_field() {
        let mut s = effectcraft_engine::Session::default();
        s.execute("comp.new", json!({"name": "M", "width": 100, "height": 100, "frameRate": 30, "duration": 4})).unwrap();
        s.execute("markers.set", json!({"new": true, "time": 1.0, "comment": "hello", "cuePoint": {"name": "c", "params": [["a", "1"]]}, "label": "Blue"}))
            .unwrap();
        let mut app = EffectcraftApp::new(s);
        open_dialog(&mut app, MarkerRef { layer: None, index: 0 }).unwrap();
        assert_eq!(app.dialog, Some(Dialog::Marker));
        let d = &mut app.dialog_state.marker;
        assert_eq!((d.time, d.comment.as_str(), d.cue, d.cue_name.as_str()), (1.0, "hello", true, "c"));
        assert_eq!(d.cue_params, vec![("a".to_string(), "1".to_string())]);
        assert_eq!(effectcraft_engine::color::Label::ALL[d.label].name(), "Blue");
        d.duration = 0.5;
        d.chapter = "Ch".into();
        d.url = "https://getartcraft.com".into();
        d.frame_target = "_self".into();
        d.protected = true;
        d.cue = false;
        let p = draft_params(d);
        app.session.execute("markers.set", p).unwrap();
        let m = &app.session.execute("markers.list", json!({})).unwrap()[0];
        assert_eq!(m["duration"], 0.5);
        assert_eq!(m["chapter"], "Ch");
        assert_eq!(m["frameTarget"], "_self");
        assert_eq!(m["protected"], true);
        assert!(m["cuePoint"].is_null());
        assert_eq!(m["label"], "Blue");
        app.session.undo();
        assert_eq!(app.session.execute("markers.list", json!({})).unwrap()[0]["chapter"], "");
        // Enter is OK and Cmd+Backspace is Delete.
        let ctx = egui::Context::default();
        crate::theme::install(&ctx, &app.tokens, crate::i18n::language(&app));
        // The dialog opens with Comment focused (#922), a multi-line field where Enter types a
        // new line; with the focus elsewhere, Enter is OK.
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            app.auto.begin_frame();
            crate::panels::dialogs::show(&mut app, ui.ctx());
        });
        out.textures_delta.clear();
        ctx.memory_mut(|m| m.stop_text_input());
        app.dialog_state.marker.chapter = "Entered".into();
        crate::panels::dialogs::press_key(&mut app, &ctx, egui::Key::Enter, egui::Modifiers::NONE);
        assert!(app.dialog.is_none());
        assert_eq!(app.session.execute("markers.list", json!({})).unwrap()[0]["chapter"], "Entered");
        open_dialog(&mut app, MarkerRef { layer: None, index: 0 }).unwrap();
        // Also from a focused text field: Tab to one, then press the key.
        for _ in 0..40 {
            if ctx.egui_wants_keyboard_input() {
                break;
            }
            crate::panels::dialogs::press_key(&mut app, &ctx, egui::Key::Tab, egui::Modifiers::NONE);
        }
        assert!(ctx.egui_wants_keyboard_input(), "a text field has focus");
        // Cmd+Enter is OK from a focused text field, without typing a new line into it.
        app.dialog_state.marker.chapter = "Cmd".into();
        crate::panels::dialogs::press_key(&mut app, &ctx, egui::Key::Enter, egui::Modifiers::COMMAND);
        assert!(app.dialog.is_none());
        let m = &app.session.execute("markers.list", json!({})).unwrap()[0];
        assert_eq!((m["chapter"].as_str(), m["comment"].as_str()), (Some("Cmd"), Some("hello")));
        open_dialog(&mut app, MarkerRef { layer: None, index: 0 }).unwrap();
        for _ in 0..40 {
            if ctx.egui_wants_keyboard_input() {
                break;
            }
            crate::panels::dialogs::press_key(&mut app, &ctx, egui::Key::Tab, egui::Modifiers::NONE);
        }
        assert!(ctx.egui_wants_keyboard_input(), "a text field has focus");
        crate::panels::dialogs::press_key(&mut app, &ctx, egui::Key::Backspace, egui::Modifiers::COMMAND);
        assert!(app.dialog.is_none());
        assert_eq!(app.session.execute("markers.list", json!({})).unwrap().as_array().map(Vec::len), Some(0), "deleted");
    }
}
