//! Two built-in panels After Effects ships as scripts, written from scratch as panels:
//!
//! - **Create Nulls From Paths** (Window ▸ Create Nulls From Paths): with a mask or shape path (or
//!   a layer that has one) selected, Points Follow Nulls / Nulls Follow Points / Trace Path
//!   (`paths.*` commands; Trace Path can loop). Puppet pins (selected, or a layer that has pins)
//!   rig the same way with the first two.
//! - **VR Comp Editor** (Window ▸ VR Comp Editor): the project's VR environments (Composition ▸ VR
//!   ▸ Create VR Environment); pick one and edit its 360 view's camera orientation (pan, tilt,
//!   roll) with `comp.vr.setView`; open its output.
//!
//! Automation ids: `createNulls.pointsFollowNulls`, `createNulls.nullsFollowPoints`,
//! `createNulls.tracePath`, `createNulls.loop`, `vrEditor.env.<output id>`, `vrEditor.pan`,
//! `vrEditor.tilt`, `vrEditor.roll`, `vrEditor.reset`, `vrEditor.open`.

use crate::i18n::tr;
use egui::{Rect, RichText};
use serde_json::{Value, json};

use crate::EffectcraftApp;

fn run(app: &mut EffectcraftApp, ctx: &egui::Context, actions: Vec<(&str, Value)>) {
    for (id, p) in actions {
        match crate::menus::invoke(app, ctx, id, p) {
            Ok(_) => {}
            Err(e) => app.ui.status = e,
        }
    }
}

fn area(ui: &mut egui::Ui, rect: Rect) -> egui::Ui {
    ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink(10.0)).layout(egui::Layout::top_down(egui::Align::Min)))
}

/// Window ▸ Create Nulls From Paths.
pub fn create_nulls(app: &mut EffectcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let ctx = ui.ctx().clone();
    let mut ui = area(ui, rect);
    let loop_id = egui::Id::unique("createNulls.loop");
    let mut looping: bool = ui.data(|d| d.get_temp(loop_id)).unwrap_or(false);
    let ready = app.session.is_enabled("paths.nullsFollowPoints") && !app.session.state.selected_layers.is_empty();
    ui.label(RichText::new(tr("Select a layer with a mask or shape path (or the path property), or Puppet pins.")).color(t.text_dim).small());
    ui.add_space(6.0);
    let mut acts = vec![];
    for (id, label, tip) in [
        ("paths.pointsFollowNulls", "Points Follow Nulls", "A null on each vertex; move the nulls to reshape the path"),
        ("paths.nullsFollowPoints", "Nulls Follow Points", "A null on each vertex that follows the (animated) path"),
        ("paths.tracePath", "Trace Path", "A null moving along the path, driven by its Progress slider"),
    ] {
        let r = ui.add_enabled(ready, egui::Button::new(label).min_size(egui::vec2(170.0, 24.0))).on_hover_text(tip);
        let auto = format!("createNulls.{}", id.trim_start_matches("paths."));
        app.auto.add(&auto, r.rect, label);
        if r.clicked() {
            acts.push((id, if id == "paths.tracePath" { json!({"loop": looping}) } else { json!({}) }));
        }
    }
    let r = crate::widgets::labelled_checkbox(&mut ui, &mut looping, tr("Loop (Trace Path)"));
    app.auto.add("createNulls.loop", r.rect, "Loop");
    ui.data_mut(|d| d.insert_temp(loop_id, looping));
    run(app, &ctx, acts);
}

/// Window ▸ VR Comp Editor.
pub fn vr_editor(app: &mut EffectcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let ctx = ui.ctx().clone();
    let mut ui = area(ui, rect);
    let envs = app.session.execute("comp.vr.environments", json!({})).ok().and_then(|v| v.as_array().cloned()).unwrap_or_default();
    if envs.is_empty() {
        ui.label(RichText::new(tr("No VR environments yet. Create one with Composition ▸ VR ▸ Create VR Environment…")).color(t.text_faint));
        return;
    }
    let sel_id = egui::Id::unique("vrEditor.selected");
    let active = app.session.state.active_comp.map(|c| c.0);
    let mut sel: u64 = ui.data(|d| d.get_temp(sel_id)).unwrap_or(0);
    // Default: the active comp's environment, else the first.
    let owns = |e: &Value, c: u64| e["output"] == c || e["cubeMap"] == c || e["faces"].as_array().is_some_and(|f| f.iter().any(|x| x["comp"] == c));
    if !envs.iter().any(|e| e["output"] == sel) {
        sel = active.and_then(|c| envs.iter().find(|e| owns(e, c))).or(envs.first()).and_then(|e| e["output"].as_u64()).unwrap_or(0);
    }
    let mut acts = vec![];
    ui.label(RichText::new(tr("VR Environments")).strong());
    for e in &envs {
        let out = e["output"].as_u64().unwrap_or(0);
        let r = ui.selectable_label(out == sel, e["name"].as_str().unwrap_or(""));
        app.auto.add(&format!("vrEditor.env.{out}"), r.rect, e["name"].as_str().unwrap_or(""));
        if r.clicked() {
            sel = out;
        }
    }
    ui.data_mut(|d| d.insert_temp(sel_id, sel));
    let Some(env) = envs.iter().find(|e| e["output"] == sel) else { return };
    ui.separator();
    ui.label(RichText::new(tr("360 View Orientation")).strong());
    let view: Vec<f64> = env["view"].as_array().map(|a| a.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
    let signed = |a: f64| if a > 180.0 { a - 360.0 } else { a };
    let (mut tilt, mut pan, mut roll) =
        (signed(view.first().copied().unwrap_or(0.0)), signed(view.get(1).copied().unwrap_or(0.0)), signed(view.get(2).copied().unwrap_or(0.0)));
    let mut changed = false;
    for (label, auto, v) in [("Pan", "vrEditor.pan", &mut pan), ("Tilt", "vrEditor.tilt", &mut tilt), ("Roll", "vrEditor.roll", &mut roll)] {
        ui.horizontal(|ui| {
            ui.label(label);
            let r = ui.add(egui::Slider::new(v, -180.0..=180.0).suffix("°").max_decimals(1));
            app.auto.add(auto, r.rect, label);
            changed |= r.changed();
        });
    }
    if changed {
        acts.push(("comp.vr.setView", json!({"comp": sel, "pan": pan, "tilt": tilt, "roll": roll})));
    }
    ui.horizontal(|ui| {
        let r = ui.button(tr("Reset"));
        app.auto.add("vrEditor.reset", r.rect, "Reset");
        if r.clicked() {
            acts.push(("comp.vr.setView", json!({"comp": sel, "orientation": [0.0, 0.0, 0.0]})));
        }
        let r = ui.button(tr("Open Output"));
        app.auto.add("vrEditor.open", r.rect, "Open Output");
        if r.clicked() {
            acts.push(("comp.open", json!({"comp": sel})));
        }
    });
    ui.label(RichText::new(tr("All six face cameras turn together; the scene is not changed.")).color(t.text_faint).small());
    run(app, &ctx, acts);
}
