//! Animation dialogs: Keyframe Velocity, Keyframe Interpolation and Time Stretch. Each one is
//! pre-filled from the engine (`keys.info`, the layer) and commits through one engine command.

use crate::i18n::tr;
use egui::{Color32, vec2};
use serde_json::{Value, json};

use crate::theme::Tokens;
use crate::{Dialog, EffectcraftApp};

#[derive(Clone, Debug, Default)]
pub struct VelocityDraft {
    pub dims: usize,
    pub units: String,
    pub name: String,
    pub in_speed: Vec<f64>,
    pub in_inf: Vec<f64>,
    pub out_speed: Vec<f64>,
    pub out_inf: Vec<f64>,
    /// Which sides exist (first key has no incoming side, last none outgoing).
    pub has_in: bool,
    pub has_out: bool,
    pub continuous: bool,
}

#[derive(Clone, Debug, Default)]
pub struct InterpDraft {
    /// Index into TEMPORAL / SPATIAL / ROVING (0 = Current Settings).
    pub temporal: usize,
    pub spatial: usize,
    pub roving: usize,
    pub has_spatial: bool,
    pub count: usize,
}

#[derive(Clone, Debug, Default)]
pub struct StretchDraft {
    pub percent: f64,
    pub duration: f64,
    /// 0 = Layer In-point, 1 = Current Frame, 2 = Layer Out-point.
    pub hold: usize,
    pub orig_duration: f64,
    pub orig_percent: f64,
}

pub const TEMPORAL: [(&str, &str); 6] = [
    ("Current Settings", ""),
    ("Linear", "linear"),
    ("Bezier", "bezier"),
    ("Continuous Bezier", "continuousBezier"),
    ("Auto Bezier", "autoBezier"),
    ("Hold", "hold"),
];
pub const SPATIAL: [(&str, &str); 5] =
    [("Current Settings", ""), ("Linear", "linear"), ("Bezier", "bezier"), ("Continuous Bezier", "continuousBezier"), ("Auto Bezier", "autoBezier")];
pub const ROVING: [&str; 3] = ["Current Settings", "Lock To Time", "Rove Across Time"];

fn num(v: &Value) -> f64 {
    v.as_f64().unwrap_or(0.0)
}

/// Animation ▸ Keyframe Velocity… (⇧⌘K) with keys selected.
pub fn open_velocity(app: &mut EffectcraftApp) -> Result<(), String> {
    let info = app.session.execute("keys.info", json!({})).map_err(|e| e.to_string())?;
    let first = info.as_array().and_then(|a| a.first()).ok_or("select keyframes first")?;
    let dims = first["dims"].as_u64().unwrap_or(1).max(1) as usize;
    let side = |k: &str| -> (Vec<f64>, Vec<f64>, bool) {
        let a = first[k].as_array().cloned().unwrap_or_default();
        let has = a.iter().any(|e| !e.is_null());
        let sp = (0..dims).map(|d| a.get(d).map(|e| num(&e["speed"])).unwrap_or(0.0)).collect();
        let inf = (0..dims).map(|d| a.get(d).map(|e| num(&e["influence"])).filter(|v| *v > 0.0).unwrap_or(33.33)).collect();
        (sp, inf, has)
    };
    let (in_speed, in_inf, has_in) = side("inEase");
    let (out_speed, out_inf, has_out) = side("outEase");
    app.dialog_state.velocity = VelocityDraft {
        dims,
        units: first["units"].as_str().unwrap_or("units/sec").into(),
        name: first["name"].as_str().unwrap_or("").into(),
        in_speed,
        in_inf,
        out_speed,
        out_inf,
        has_in,
        has_out,
        continuous: first["continuous"].as_bool().unwrap_or(false),
    };
    app.dialog = Some(Dialog::KeyVelocity);
    Ok(())
}

/// Animation ▸ Keyframe Interpolation… (⌥⌘K).
pub fn open_interpolation(app: &mut EffectcraftApp) -> Result<(), String> {
    let info = app.session.execute("keys.info", json!({})).map_err(|e| e.to_string())?;
    let a = info.as_array().cloned().unwrap_or_default();
    if a.is_empty() {
        return Err("select keyframes first".into());
    }
    app.dialog_state.interp = InterpDraft { has_spatial: a.iter().any(|k| !k["spatial"].is_null()), count: a.len(), ..Default::default() };
    app.dialog = Some(Dialog::KeyInterpolation);
    Ok(())
}

/// Layer ▸ Time ▸ Time Stretch…
pub fn open_time_stretch(app: &mut EffectcraftApp) -> Result<(), String> {
    let lid = *app.session.state.selected_layers.first().ok_or("select a layer first")?;
    let l = app.session.active_comp().and_then(|c| c.layer(lid)).ok_or("no layer")?;
    let dur = (l.out_point - l.in_point).seconds();
    app.dialog_state.stretch = StretchDraft { percent: l.stretch, duration: dur, hold: 0, orig_duration: dur, orig_percent: l.stretch };
    app.dialog = Some(Dialog::TimeStretch);
    Ok(())
}

fn buttons(ui: &mut egui::Ui, app: &mut EffectcraftApp, t: &Tokens, prefix: &str) -> (bool, bool) {
    let (mut ok, mut cancel) = (false, false);
    ui.add_space(14.0);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let r = ui.add(egui::Button::new(egui::RichText::new(tr("   OK   ")).color(Color32::WHITE)).fill(t.accent));
        app.auto.add(&format!("{prefix}.ok"), r.rect, "OK");
        ok = r.clicked();
        let r = ui.button(tr("Cancel"));
        app.auto.add(&format!("{prefix}.cancel"), r.rect, "Cancel");
        cancel = r.clicked();
    });
    (ok, cancel)
}

fn drag(ui: &mut egui::Ui, app: &mut EffectcraftApp, id: &str, v: &mut f64, speed: f64, range: std::ops::RangeInclusive<f64>, suffix: &str, enabled: bool) {
    let r = ui.add_enabled(enabled, crate::widgets::drag_value(egui::DragValue::new(v).speed(speed).range(range).max_decimals(2).suffix(suffix)));
    app.auto.add(id, r.rect, id);
}

pub fn velocity(app: &mut EffectcraftApp, ctx: &egui::Context, t: &Tokens) {
    let mut d = app.dialog_state.velocity.clone();
    let (mut ok, mut cancel) = (false, false);
    let title = if d.name.is_empty() { "Keyframe Velocity".to_string() } else { format!("Keyframe Velocity: {}", d.name) };
    let h = 250.0 + 2.0 * 26.0 * d.dims as f32;
    super::dialogs::modal(ctx, &title, vec2(440.0, h), t, |ui| {
        for (label, out) in [("Incoming Velocity", false), ("Outgoing Velocity", true)] {
            ui.label(egui::RichText::new(label).font(Tokens::medium(12.5)));
            let enabled = if out { d.has_out } else { d.has_in };
            egui::Grid::new(("vel", out)).num_columns(3).spacing([12.0, 6.0]).show(ui, |ui| {
                for dim in 0..d.dims {
                    let tag = if d.dims > 1 { format!("Dimension {}", ["X", "Y", "Z", "W"][dim.min(3)]) } else { String::new() };
                    ui.label(egui::RichText::new(tag).color(t.text_dim));
                    let side = if out { "out" } else { "in" };
                    ui.horizontal(|ui| {
                        ui.label(tr("Speed:"));
                        let v = if out { &mut d.out_speed[dim] } else { &mut d.in_speed[dim] };
                        drag(ui, app, &format!("dialog.velocity.{side}Speed.{dim}"), v, 1.0, -1e7..=1e7, "", enabled);
                        ui.label(egui::RichText::new(&d.units).color(t.text_dim));
                    });
                    ui.horizontal(|ui| {
                        ui.label(tr("Influence:"));
                        let v = if out { &mut d.out_inf[dim] } else { &mut d.in_inf[dim] };
                        drag(ui, app, &format!("dialog.velocity.{side}Influence.{dim}"), v, 0.2, 0.1..=100.0, " %", enabled);
                    });
                    ui.end_row();
                }
            });
            ui.add_space(8.0);
        }
        let r = ui.checkbox(&mut d.continuous, tr("Continuous (Lock Outgoing to Incoming)"));
        app.auto.add("dialog.velocity.continuous", r.rect, "Continuous");
        if d.continuous {
            d.out_speed = d.in_speed.clone();
        }
        (ok, cancel) = buttons(ui, app, t, "dialog.velocity");
    });
    app.dialog_state.velocity = d.clone();
    if ok || super::dialogs::ui_enter(ctx) {
        let mut params = json!({"continuous": d.continuous});
        if d.has_in {
            params["inSpeed"] = json!(d.in_speed);
            params["inInfluence"] = json!(d.in_inf);
        }
        if d.has_out {
            params["outSpeed"] = json!(d.out_speed);
            params["outInfluence"] = json!(d.out_inf);
        }
        if let Err(e) = app.session.execute("keys.velocity", params) {
            app.ui.status = e.to_string();
        }
        cancel = true;
    }
    if cancel {
        app.dialog = None;
    }
}

fn combo(ui: &mut egui::Ui, app: &mut EffectcraftApp, id: &str, sel: &mut usize, opts: &[&str], enabled: bool) {
    ui.add_enabled_ui(enabled, |ui| {
        // The options are the English source strings; the automation id and label stay English.
        let r = craft_ui::ComboBox::from_id_salt(id).width(200.0).selected_text(tr(opts[*sel])).show_ui(ui, |ui| {
            for (i, o) in opts.iter().enumerate() {
                ui.selectable_value(sel, i, tr(o));
            }
        });
        app.auto.add(id, r.response.rect, id);
    });
}

pub fn interpolation(app: &mut EffectcraftApp, ctx: &egui::Context, t: &Tokens) {
    let mut d = app.dialog_state.interp.clone();
    let (mut ok, mut cancel) = (false, false);
    super::dialogs::modal(ctx, "Keyframe Interpolation", vec2(440.0, 250.0), t, |ui| {
        egui::Grid::new("interp-grid").num_columns(2).spacing([14.0, 10.0]).show(ui, |ui| {
            ui.label(tr("Temporal Interpolation:"));
            combo(ui, app, "dialog.interpolation.temporal", &mut d.temporal, &TEMPORAL.map(|x| x.0), true);
            ui.end_row();
            ui.label(tr("Spatial Interpolation:"));
            combo(ui, app, "dialog.interpolation.spatial", &mut d.spatial, &SPATIAL.map(|x| x.0), d.has_spatial);
            ui.end_row();
            ui.label(tr("Roving:"));
            combo(ui, app, "dialog.interpolation.roving", &mut d.roving, &ROVING, d.has_spatial);
            ui.end_row();
        });
        ui.add_space(6.0);
        ui.label(egui::RichText::new(format!("{} keyframe{} selected", d.count, if d.count == 1 { "" } else { "s" })).color(t.text_dim));
        (ok, cancel) = buttons(ui, app, t, "dialog.interpolation");
    });
    app.dialog_state.interp = d.clone();
    if ok || super::dialogs::ui_enter(ctx) {
        let mut params = json!({});
        if d.temporal > 0 {
            params["interpolation"] = json!(TEMPORAL[d.temporal].1);
        }
        if d.spatial > 0 && d.has_spatial {
            params["spatial"] = json!(SPATIAL[d.spatial].1);
        }
        if d.roving > 0 && d.has_spatial {
            params["roving"] = json!(d.roving == 2);
        }
        if params.as_object().is_some_and(|m| !m.is_empty())
            && let Err(e) = app.session.execute("keys.interpolation", params)
        {
            app.ui.status = e.to_string();
        }
        cancel = true;
    }
    if cancel {
        app.dialog = None;
    }
}

pub fn time_stretch(app: &mut EffectcraftApp, ctx: &egui::Context, t: &Tokens) {
    let mut d = app.dialog_state.stretch.clone();
    let (mut ok, mut cancel) = (false, false);
    super::dialogs::modal(ctx, "Time Stretch", vec2(420.0, 300.0), t, |ui| {
        ui.label(egui::RichText::new(tr("Stretch")).font(Tokens::medium(12.5)));
        egui::Grid::new("stretch-grid").num_columns(2).spacing([14.0, 8.0]).show(ui, |ui| {
            ui.label(tr("Original Duration:"));
            ui.label(format!("{:.2} s", d.orig_duration));
            ui.end_row();
            ui.label(tr("Stretch Factor:"));
            let before = d.percent;
            drag(ui, app, "dialog.timeStretch.percent", &mut d.percent, 0.5, -10000.0..=10000.0, " %", true);
            if (before - d.percent).abs() > 1e-9 && d.orig_percent.abs() > 1e-9 {
                d.duration = d.orig_duration * d.percent.abs() / d.orig_percent.abs();
            }
            ui.end_row();
            ui.label(tr("New Duration:"));
            let before = d.duration;
            drag(ui, app, "dialog.timeStretch.duration", &mut d.duration, 0.05, 0.01..=100000.0, " s", true);
            if (before - d.duration).abs() > 1e-9 && d.orig_duration > 1e-9 {
                d.percent = d.orig_percent.signum() * d.orig_percent.abs() * d.duration / d.orig_duration;
            }
            ui.end_row();
        });
        ui.add_space(10.0);
        ui.label(egui::RichText::new(tr("Hold In Place")).font(Tokens::medium(12.5)));
        for (i, l) in ["Layer In-point", "Current Frame", "Layer Out-point"].into_iter().enumerate() {
            let r = ui.radio_value(&mut d.hold, i, l);
            app.auto.add(&format!("dialog.timeStretch.hold.{i}"), r.rect, l);
        }
        (ok, cancel) = buttons(ui, app, t, "dialog.timeStretch");
    });
    app.dialog_state.stretch = d.clone();
    if ok || super::dialogs::ui_enter(ctx) {
        let hold = ["in", "current", "out"][d.hold.min(2)];
        if let Err(e) = app.session.execute("layer.timeStretch", json!({"percent": d.percent, "hold": hold})) {
            app.ui.status = e.to_string();
        }
        cancel = true;
    }
    if cancel {
        app.dialog = None;
    }
}
