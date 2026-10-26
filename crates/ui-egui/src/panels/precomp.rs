//! Precomposition workflow: the Pre-compose dialog (Leave / Move all attributes, Adjust
//! composition duration, Open New Composition), the Composition Navigator bar above the viewer
//! (the flow of nested comps, click to open) and the Composition Mini-Flowchart popup (Tab).

use crate::i18n::{tr, tr_args};
use effectcraft_engine::project::{ItemId, LayerSource, Project};
use egui::{Align2, Color32, Rect, Sense, Stroke, pos2, vec2};
use serde_json::json;

use crate::theme::Tokens;
use crate::{Dialog, EffectcraftApp};

// ---------------------------------------------------------------- flow graph

/// Comps that contain a layer whose source is `cid` (upstream), in project order.
pub fn upstream(project: &Project, cid: ItemId) -> Vec<ItemId> {
    project.comps().filter(|(_, c)| c.layers.iter().any(|l| l.source == LayerSource::Comp { item: cid })).map(|(id, _)| *id).collect()
}

/// Comps used as layer sources in `cid` (downstream), first use first, without repeats.
pub fn downstream(project: &Project, cid: ItemId) -> Vec<ItemId> {
    let mut out = vec![];
    if let Some(c) = project.comp(cid) {
        for l in &c.layers {
            if let LayerSource::Comp { item } = l.source
                && !out.contains(&item)
                && project.comp(item).is_some()
            {
                out.push(item);
            }
        }
    }
    out
}

/// The navigator chain ending at `cid`: walk upstream, preferring comps opened most recently
/// (`open` is the open-comps order), stopping at a root or a cycle.
pub fn chain(project: &Project, cid: ItemId, open: &[ItemId]) -> Vec<ItemId> {
    let mut v = vec![cid];
    let mut cur = cid;
    while v.len() < 32 {
        let ups = upstream(project, cur);
        let Some(next) = open.iter().rev().find(|o| ups.contains(o)).or(ups.first()).copied() else { break };
        if v.contains(&next) {
            break;
        }
        v.push(next);
        cur = next;
    }
    v.reverse();
    v
}

fn name(project: &Project, id: ItemId) -> String {
    project.item(id).map(|i| i.name.clone()).unwrap_or_default()
}

// ---------------------------------------------------------------- Pre-compose dialog

#[derive(Clone, Debug, Default)]
pub struct PrecomposeDraft {
    pub layers: Vec<u64>,
    pub name: String,
    /// "Leave all attributes in" (only one footage/solid/precomp layer).
    pub leave: bool,
    pub can_leave: bool,
    pub layer_name: String,
    pub adjust: bool,
    pub open: bool,
}

/// Open the Pre-compose dialog for the selected (or given) layers.
pub fn open(app: &mut EffectcraftApp, p: &serde_json::Value) -> Result<(), String> {
    let s = &app.session;
    let c = s.active_comp().ok_or("no composition is open")?;
    let layers: Vec<u64> = match p.get("layers").and_then(|v| v.as_array()) {
        Some(a) => a.iter().filter_map(|v| v.as_u64()).collect(),
        None => s.state.selected_layers.iter().map(|l| l.0).collect(),
    };
    if layers.is_empty() {
        return Err("select the layers to pre-compose".into());
    }
    let first = c.layer(effectcraft_engine::project::LayerId(layers[0]));
    let can_leave = layers.len() == 1 && first.is_some_and(|l| l.source.item().is_some());
    let n = s.project.comps().count() + 1;
    let base = if layers.len() == 1 { first.map(|l| format!("{} Comp 1", l.name)) } else { None };
    app.dialog_state.precompose = PrecomposeDraft {
        layers,
        name: base.unwrap_or_else(|| format!("Pre-comp {n}")),
        leave: false,
        can_leave,
        layer_name: first.map(|l| l.name.clone()).unwrap_or_default(),
        adjust: false,
        open: true,
    };
    app.dialog = Some(Dialog::Precompose);
    Ok(())
}

/// The `layer.precompose` parameters of the dialog.
pub fn draft_params(d: &PrecomposeDraft) -> serde_json::Value {
    json!({
        "layers": d.layers,
        "name": d.name,
        "mode": if d.leave && d.can_leave { "leave" } else { "move" },
        "adjustDuration": d.adjust && !(d.leave && d.can_leave),
        "open": d.open,
    })
}

pub fn dialog(app: &mut EffectcraftApp, ctx: &egui::Context, t: &Tokens) {
    let mut d = app.dialog_state.precompose.clone();
    let (mut ok, mut cancel) = (false, false);
    super::dialogs::modal(ctx, "Pre-compose", vec2(520.0, 360.0), t, |ui| {
        ui.horizontal(|ui| {
            ui.label(tr("New composition name:"));
            let r = ui.add(egui::TextEdit::singleline(&mut d.name).desired_width(260.0));
            crate::widgets::focus_on_open(ui.ctx(), &r, &d.name);
            app.auto.add("dialog.precompose.name", r.rect, "New composition name");
        });
        ui.add_space(10.0);
        ui.add_enabled_ui(d.can_leave, |ui| {
            let r = ui.radio_value(&mut d.leave, true, tr_args("Leave all attributes in \"{}\"", &[&d.layer_name]));
            app.auto.add("dialog.precompose.leave", r.rect, "Leave all attributes");
            ui.label(
                egui::RichText::new(format!(
                    "Use this option to create a new intermediate composition with only \"{}\" in it. The new composition will become the source to the current layer.",
                    d.layer_name
                ))
                .color(t.text_dim)
                .size(11.0),
            );
        });
        ui.add_space(6.0);
        let r = ui.radio_value(&mut d.leave, false, "Move all attributes into the new composition");
        app.auto.add("dialog.precompose.move", r.rect, "Move all attributes");
        ui.label(
            egui::RichText::new(tr("Use this option to place the currently selected layers together into a new intermediate composition."))
                .color(t.text_dim)
                .size(11.0),
        );
        ui.add_space(4.0);
        ui.add_enabled_ui(!d.leave, |ui| {
            let r = ui.checkbox(&mut d.adjust, tr("Adjust composition duration to the time span of the selected layers"));
            app.auto.add("dialog.precompose.adjust", r.rect, "Adjust composition duration");
        });
        ui.add_space(8.0);
        let r = ui.checkbox(&mut d.open, tr("Open New Composition"));
        app.auto.add("dialog.precompose.open", r.rect, "Open New Composition");
        ui.add_space(14.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let r = ui.add(egui::Button::new(egui::RichText::new(tr("   OK   ")).color(Color32::WHITE)).fill(t.accent));
            app.auto.add("dialog.precompose.ok", r.rect, "OK");
            ok = r.clicked();
            let r = ui.button(tr("Cancel"));
            app.auto.add("dialog.precompose.cancel", r.rect, "Cancel");
            cancel = r.clicked();
        });
    });
    app.dialog_state.precompose = d.clone();
    if ok || super::dialogs::ui_enter(ctx) {
        if let Err(e) = app.session.execute("layer.precompose", draft_params(&d)) {
            app.ui.status = e.to_string();
        }
        cancel = true;
    }
    if cancel {
        app.dialog = None;
    }
}

// ---------------------------------------------------------------- navigator + mini-flowchart

/// Height of the Composition Navigator bar.
pub const NAV_H: f32 = 22.0;

/// Whether the active comp has a flow to show (it is nested somewhere or nests others).
pub fn has_flow(app: &EffectcraftApp) -> bool {
    app.session.active_comp_id().is_some_and(|c| !upstream(&app.session.project, c).is_empty() || !downstream(&app.session.project, c).is_empty())
}

/// The Composition Navigator bar: the upstream chain › active comp › (dim) its precomps.
pub fn navigator(app: &mut EffectcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let Some(cid) = app.session.active_comp_id() else { return };
    let p = ui.painter().with_clip_rect(rect);
    p.rect_filled(rect, 0.0, t.panel_bg);
    p.line_segment([rect.left_bottom(), rect.right_bottom()], Stroke::new(1.0, t.separator));
    let proj = app.session.project.clone();
    let ch = chain(&proj, cid, &app.session.state.open_comps);
    let down = downstream(&proj, cid);
    let mut x = rect.min.x + 10.0;
    let cy = rect.center().y;
    let mut open: Option<ItemId> = None;
    let items: Vec<(ItemId, bool)> = ch.iter().map(|c| (*c, false)).chain(down.iter().take(1).map(|c| (*c, true))).collect();
    for (i, (id, downstream_item)) in items.iter().enumerate() {
        if i > 0 {
            crate::icons::paint(&p, Rect::from_center_size(pos2(x + 6.0, cy), vec2(8.0, 8.0)), crate::icons::Icon::ChevronRight, t.text_faint);
            x += 16.0;
        }
        let active = *id == cid;
        let label = name(&proj, *id);
        let col = if active {
            t.tab_text_active
        } else if *downstream_item {
            t.text_faint
        } else {
            t.text_dim
        };
        let g = p.layout_no_wrap(label.clone(), Tokens::ui(11.5), col);
        let r = Rect::from_min_size(pos2(x, rect.min.y + 2.0), vec2(g.size().x + 12.0, rect.height() - 4.0));
        let resp = ui.interact(r, egui::Id::unique(("comp-nav", id.0)), Sense::click());
        if active {
            p.rect_filled(r, 3.0, t.row_selected);
        } else if resp.hovered() {
            p.rect_filled(r, 3.0, t.hover);
        }
        p.galley(pos2(r.min.x + 6.0, cy - g.size().y / 2.0), g, col);
        app.auto.add(&format!("viewer.navigator.{}", id.0), r, &label);
        if resp.clicked() && !active {
            open = Some(*id);
        }
        x = r.max.x + 2.0;
    }
    let fr = Rect::from_min_size(pos2(rect.max.x - 26.0, rect.min.y + 2.0), vec2(22.0, rect.height() - 4.0));
    if crate::widgets::icon_button(ui, fr, crate::icons::Icon::Flowchart, false, &t, egui::Id::unique("comp-nav-flow"))
        .on_hover_text(tr("Composition Mini-Flowchart"))
        .clicked()
    {
        app.ui.mini_flowchart = Some([fr.min.x, fr.max.y]);
    }
    app.auto.add("viewer.navigator.flowchart", fr, "Composition Mini-Flowchart");
    if let Some(id) = open {
        app.session.open_comp(id);
    }
}

/// Composition Mini-Flowchart: upstream comps → active → downstream comps; click to open.
pub fn mini_flowchart(app: &mut EffectcraftApp, ctx: &egui::Context) {
    let Some(at) = app.ui.mini_flowchart else { return };
    let Some(cid) = app.session.active_comp_id() else {
        app.ui.mini_flowchart = None;
        return;
    };
    let t = app.tokens;
    let proj = app.session.project.clone();
    let ups = upstream(&proj, cid);
    let downs = downstream(&proj, cid);
    let mut open = None;
    let mut close = false;
    let area =
        egui::Area::new(egui::Id::unique("mini-flowchart")).order(egui::Order::Foreground).constrain(true).fixed_pos(pos2(at[0], at[1])).show(ctx, |ui| {
            egui::Frame::popup(ui.style()).fill(t.panel_bg).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
                let col_w = 160.0;
                let rows = ups.len().max(downs.len()).max(1) as f32;
                let h = rows * 26.0 + 8.0;
                let (rect, _) = ui.allocate_exact_size(vec2(col_w * 3.0 + 60.0, h), Sense::hover());
                let p = ui.painter().clone();
                let node = |ui: &mut egui::Ui, app: &mut EffectcraftApp, r: Rect, id: ItemId, active: bool| {
                    let resp = ui.interact(r, egui::Id::unique(("mfc", id.0)), Sense::click());
                    p.rect_filled(
                        r,
                        4.0,
                        if active {
                            t.accent
                        } else if resp.hovered() {
                            t.hover
                        } else {
                            t.field_bg
                        },
                    );
                    p.text(r.center(), Align2::CENTER_CENTER, name(&proj, id), Tokens::ui(11.5), if active { Color32::WHITE } else { t.text });
                    app.auto.add(&format!("flowchart.mini.{}", id.0), r, &name(&proj, id));
                    resp.clicked()
                };
                let mid = Rect::from_center_size(rect.center(), vec2(col_w, 22.0));
                if node(ui, app, mid, cid, true) {
                    close = true;
                }
                let col = |i: usize, n: usize, x: f32| {
                    let y0 = rect.center().y - (n as f32 * 26.0) / 2.0 + 13.0;
                    Rect::from_center_size(pos2(x, y0 + i as f32 * 26.0), vec2(col_w, 22.0))
                };
                let arrow = |a: egui::Pos2, b: egui::Pos2| {
                    p.line_segment([a, b], Stroke::new(1.2, t.text_dim));
                    let d = (b - a).normalized();
                    let n = vec2(-d.y, d.x);
                    p.add(egui::Shape::convex_polygon(vec![b, b - d * 7.0 + n * 3.5, b - d * 7.0 - n * 3.5], t.text_dim, Stroke::NONE));
                };
                for (i, u) in ups.iter().enumerate() {
                    let r = col(i, ups.len(), rect.min.x + col_w / 2.0);
                    // Upstream comps contain the active one: arrow from the nested comp up into them.
                    arrow(mid.left_center(), r.right_center());
                    if node(ui, app, r, *u, false) {
                        open = Some(*u);
                    }
                }
                for (i, d) in downs.iter().enumerate() {
                    let r = col(i, downs.len(), rect.max.x - col_w / 2.0);
                    arrow(r.left_center(), mid.right_center());
                    if node(ui, app, r, *d, false) {
                        open = Some(*d);
                    }
                }
                if ups.is_empty() && downs.is_empty() {
                    p.text(pos2(rect.center().x, rect.max.y - 4.0), Align2::CENTER_BOTTOM, tr("No nested compositions"), Tokens::ui(10.5), t.text_faint);
                }
            });
        });
    let outside = crate::widgets::pressed_outside(ctx, &area.response);
    if let Some(id) = open {
        app.session.open_comp(id);
        close = true;
    }
    if close || outside || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.ui.mini_flowchart = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use effectcraft_engine::Session;

    #[test]
    fn flow_chain_and_precompose_dialog_params() {
        let mut s = Session::default();
        s.execute("comp.new", json!({"name": "Main", "width": 200, "height": 100, "frameRate": 25, "duration": 2})).unwrap();
        let main = s.active_comp_id().unwrap();
        let l = s.execute("layer.newSolid", json!({"color": "#ff0000"})).unwrap()["layer"].as_u64().unwrap();
        let r = s.execute("layer.precompose", json!({"layers": [l], "name": "Mid", "open": true})).unwrap();
        let mid = ItemId(r["comp"].as_u64().unwrap());
        let l2 = s.project.comp(mid).unwrap().layers[0].id.0;
        let r = s.execute("layer.precompose", json!({"layers": [l2], "name": "Inner", "open": true})).unwrap();
        let inner = ItemId(r["comp"].as_u64().unwrap());
        assert_eq!(upstream(&s.project, inner), vec![mid]);
        assert_eq!(downstream(&s.project, main), vec![mid]);
        assert_eq!(chain(&s.project, inner, &s.state.open_comps), vec![main, mid, inner]);
        assert_eq!(chain(&s.project, main, &[]), vec![main]);

        let mut app = EffectcraftApp::new(s);
        app.session.open_comp(main);
        let top = app.session.active_comp().unwrap().layers[0].id.0;
        app.session.execute("layer.select", json!({"layers": [top]})).unwrap();
        open(&mut app, &json!({})).unwrap();
        assert_eq!(app.dialog, Some(Dialog::Precompose));
        let d = &mut app.dialog_state.precompose;
        assert!(d.can_leave);
        d.leave = true;
        d.adjust = true;
        let p = draft_params(d);
        assert_eq!(p["mode"], "leave");
        assert_eq!(p["adjustDuration"], false);
        d.leave = false;
        assert_eq!(draft_params(d)["mode"], "move");
        assert_eq!(draft_params(d)["adjustDuration"], true);
    }
}
