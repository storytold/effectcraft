//! Camera Settings and Light Settings dialogs (Layer ▸ New ▸ Camera/Light, Layer ▸ Camera/Light
//! Settings, Layer Settings on a camera or light). OK runs `layer.newCamera` / `layer.newLight`
//! or `layer.cameraSettings` / `layer.lightSettings` with the dialog's values.

use crate::i18n::tr;
use effectcraft_engine::project::{AutoOrient, LayerSource, LightKind};
use effectcraft_engine::render::three_d::camera::{PRESETS, angle_of_view, default_aperture, focal_for_zoom, zoom_for_focal};
use egui::{Color32, vec2};
use serde_json::{Value, json};

use crate::theme::Tokens;
use crate::{Dialog, EffectcraftApp};

#[derive(Clone, Debug, Default)]
pub struct CameraDraft {
    /// Existing camera layer (Camera Settings) or None (New Camera).
    pub layer: Option<u64>,
    pub name: String,
    pub two_node: bool,
    pub zoom: f64,
    pub dof: bool,
    pub focus: f64,
    pub lock_to_zoom: bool,
    pub aperture: f64,
    pub blur: f64,
}

#[derive(Clone, Debug, Default)]
pub struct LightDraft {
    pub layer: Option<u64>,
    pub name: String,
    pub kind: usize,
    pub color: [f32; 3],
    pub intensity: f64,
    pub cone_angle: f64,
    pub cone_feather: f64,
    pub falloff: usize,
    pub radius: f64,
    pub falloff_distance: f64,
    pub casts_shadows: bool,
    pub shadow_darkness: f64,
    pub shadow_diffusion: f64,
}

const KINDS: [LightKind; 4] = [LightKind::Parallel, LightKind::Spot, LightKind::Point, LightKind::Ambient];
const FALLOFFS: [&str; 3] = ["None", "Smooth", "Inverse Square Clamped"];

fn comp_w(app: &EffectcraftApp) -> f64 {
    app.session.active_comp().map(|c| c.width as f64).unwrap_or(1920.0)
}

/// Open Camera Settings: for `layer` (existing) or a new camera.
pub fn open_camera(app: &mut EffectcraftApp, layer: Option<u64>) -> Result<(), String> {
    let comp = app.session.active_comp().ok_or("no composition is open")?.clone();
    let w = comp.width as f64;
    let t = app.session.time();
    let d = match layer.and_then(|id| comp.layer(effectcraft_engine::project::LayerId(id))) {
        Some(l) => {
            let f = |p: &str, d: f64| l.props.prop(p).map(|pr| pr.value_at(l.layer_time(t)).as_f64()).unwrap_or(d);
            let zoom = f("cameraOptions/zoom", 1000.0);
            let focus = f("cameraOptions/focusDistance", zoom);
            CameraDraft {
                layer: Some(l.id.0),
                name: l.name.clone(),
                two_node: l.auto_orient == AutoOrient::TowardsPointOfInterest,
                zoom,
                dof: l.props.prop("cameraOptions/dof").map(|p| p.value_at(l.layer_time(t)).as_bool()).unwrap_or(false),
                focus,
                lock_to_zoom: (focus - zoom).abs() < 1e-6,
                aperture: f("cameraOptions/aperture", default_aperture(w)),
                blur: f("cameraOptions/blurLevel", 100.0),
            }
        }
        None => {
            let n = comp.layers.iter().filter(|l| l.is_camera()).count() + 1;
            let zoom = zoom_for_focal(w, 50.0);
            CameraDraft {
                layer: None,
                name: format!("Camera {n}"),
                two_node: true,
                zoom,
                dof: false,
                focus: zoom,
                lock_to_zoom: true,
                aperture: default_aperture(w),
                blur: 100.0,
            }
        }
    };
    let editing = d.layer.is_some();
    app.dialog_state.camera = d;
    app.dialog = Some(Dialog::CameraSettings);
    if editing {
        super::live_preview::begin(app, Dialog::CameraSettings);
    } else {
        app.dialog_state.live = Default::default();
    }
    Ok(())
}

/// Open Light Settings: for `layer` (existing) or a new light.
pub fn open_light(app: &mut EffectcraftApp, layer: Option<u64>) -> Result<(), String> {
    let comp = app.session.active_comp().ok_or("no composition is open")?.clone();
    let t = app.session.time();
    let d = match layer.and_then(|id| comp.layer(effectcraft_engine::project::LayerId(id))) {
        Some(l) => {
            let LayerSource::Light { kind } = l.source else { return Err("not a light".into()) };
            let v = |p: &str| l.props.prop(&format!("lightOptions/{p}")).map(|pr| pr.value_at(l.layer_time(t)));
            let f = |p: &str, d: f64| v(p).map(|x| x.as_f64()).unwrap_or(d);
            let c = v("color").map(|x| x.as_color()).unwrap_or([1.0; 4]);
            LightDraft {
                layer: Some(l.id.0),
                name: l.name.clone(),
                kind: KINDS.iter().position(|k| *k == kind).unwrap_or(1),
                color: [c[0], c[1], c[2]],
                intensity: f("intensity", 100.0),
                cone_angle: f("coneAngle", 90.0),
                cone_feather: f("coneFeather", 50.0),
                falloff: v("falloff").map(|x| x.as_enum() as usize).unwrap_or(0).min(2),
                radius: f("radius", 500.0),
                falloff_distance: f("falloffDistance", 500.0),
                casts_shadows: v("castsShadows").map(|x| x.as_bool()).unwrap_or(false),
                shadow_darkness: f("shadowDarkness", 100.0),
                shadow_diffusion: f("shadowDiffusion", 0.0),
            }
        }
        None => {
            let n = comp.layers.iter().filter(|l| l.is_light()).count() + 1;
            LightDraft {
                layer: None,
                name: format!("Light {n}"),
                kind: 1,
                color: [1.0, 1.0, 1.0],
                intensity: 100.0,
                cone_angle: 90.0,
                cone_feather: 50.0,
                falloff: 0,
                radius: 500.0,
                falloff_distance: 500.0,
                casts_shadows: false,
                shadow_darkness: 100.0,
                shadow_diffusion: 0.0,
            }
        }
    };
    let editing = d.layer.is_some();
    app.dialog_state.light = d;
    app.dialog = Some(Dialog::LightSettings);
    if editing {
        super::live_preview::begin(app, Dialog::LightSettings);
    } else {
        app.dialog_state.live = Default::default();
    }
    Ok(())
}

fn buttons(ui: &mut egui::Ui, t: &Tokens, ok: &mut bool, close: &mut bool, preview: Option<&mut bool>) -> (egui::Rect, egui::Rect, egui::Rect) {
    let mut r = (egui::Rect::NOTHING, egui::Rect::NOTHING, egui::Rect::NOTHING);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let b = ui.add(egui::Button::new(egui::RichText::new(tr("   OK   ")).color(Color32::WHITE)).fill(t.accent));
        r.0 = b.rect;
        if b.clicked() {
            *ok = true;
        }
        let c = ui.button(tr("Cancel"));
        r.1 = c.rect;
        if c.clicked() {
            *close = true;
        }
        if let Some(on) = preview {
            r.2 = ui.checkbox(on, tr("Preview")).rect;
        }
    });
    r
}

pub fn camera(app: &mut EffectcraftApp, ctx: &egui::Context, t: &Tokens) {
    let w = comp_w(app);
    let mut d = app.dialog_state.camera.clone();
    let (mut ok, mut close) = (false, false);
    let mut rects = (egui::Rect::NOTHING, egui::Rect::NOTHING, egui::Rect::NOTHING);
    let live = super::live_preview::active(app);
    let place = super::live_preview::place(app);
    let mut preview = app.dialog_state.live.on;
    super::dialogs::modal_placed(ctx, "Camera Settings", vec2(520.0, 420.0), t, place, |ui| {
        egui::Grid::new("cam-grid").num_columns(2).spacing([14.0, 9.0]).show(ui, |ui| {
            ui.label(tr("Type"));
            egui::ComboBox::from_id_salt("cam-type").selected_text(if d.two_node { "Two-Node Camera" } else { "One-Node Camera" }).show_ui(ui, |ui| {
                ui.selectable_value(&mut d.two_node, true, tr("Two-Node Camera"));
                ui.selectable_value(&mut d.two_node, false, tr("One-Node Camera"));
            });
            ui.end_row();
            ui.label(tr("Name"));
            let r = ui.add(egui::TextEdit::singleline(&mut d.name).desired_width(240.0));
            crate::widgets::focus_on_open(ui.ctx(), &r, &d.name);
            ui.end_row();
            ui.label(tr("Preset"));
            let focal = focal_for_zoom(w, d.zoom);
            let cur = PRESETS.iter().find(|(_, f)| (f - focal).abs() < 1e-6).map(|(n, _)| *n).unwrap_or("Custom");
            egui::ComboBox::from_id_salt("cam-preset").selected_text(cur).show_ui(ui, |ui| {
                for (n, f) in PRESETS {
                    if ui.selectable_label(cur == n, n).clicked() {
                        d.zoom = zoom_for_focal(w, f);
                    }
                }
            });
            ui.end_row();
            ui.label(tr("Zoom"));
            ui.horizontal(|ui| {
                ui.add(crate::widgets::drag_value(egui::DragValue::new(&mut d.zoom).range(1.0..=100000.0).speed(1.0).suffix(" px")));
                let mut aov = angle_of_view(w, d.zoom);
                if ui.add(crate::widgets::drag_value(egui::DragValue::new(&mut aov).range(0.1..=179.0).speed(0.1).suffix("°"))).changed() {
                    d.zoom = w * 0.5 / (aov.to_radians() * 0.5).tan();
                }
                let mut fl = focal_for_zoom(w, d.zoom);
                if ui.add(crate::widgets::drag_value(egui::DragValue::new(&mut fl).range(0.1..=10000.0).speed(0.1).suffix(" mm"))).changed() {
                    d.zoom = zoom_for_focal(w, fl);
                }
            });
            ui.end_row();
            ui.label(tr("Depth of Field"));
            ui.checkbox(&mut d.dof, tr("Enable Depth of Field"));
            ui.end_row();
            ui.label(tr("Focus Distance"));
            ui.horizontal(|ui| {
                ui.add_enabled(!d.lock_to_zoom, crate::widgets::drag_value(egui::DragValue::new(&mut d.focus).range(0.0..=100000.0).suffix(" px")));
                ui.checkbox(&mut d.lock_to_zoom, tr("Lock to Zoom"));
            });
            ui.end_row();
            ui.label(tr("Aperture"));
            ui.add(crate::widgets::drag_value(egui::DragValue::new(&mut d.aperture).range(0.0..=10000.0).speed(0.1).suffix(" px")));
            ui.end_row();
            ui.label(tr("Blur Level"));
            ui.add(crate::widgets::drag_value(egui::DragValue::new(&mut d.blur).range(0.0..=10000.0).suffix(" %")));
            ui.end_row();
        });
        ui.add_space(14.0);
        rects = buttons(ui, t, &mut ok, &mut close, live.then_some(&mut preview));
    });
    app.dialog_state.live.on = preview;
    if d.lock_to_zoom {
        d.focus = d.zoom;
    }
    app.auto.add("dialog.camera.ok", rects.0, "OK");
    app.auto.add("dialog.camera.cancel", rects.1, "Cancel");
    if live {
        app.auto.add("dialog.camera.preview", rects.2, "Preview");
    }
    app.dialog_state.camera = d.clone();
    let (id, p) = camera_command(&d);
    if ok || super::dialogs::ui_enter(ctx) {
        // A preview already made the edit (one undo step): OK keeps it.
        if !super::live_preview::keep(app)
            && let Err(e) = app.session.execute(id, p)
        {
            app.ui.status = e.to_string();
        }
        close = true;
    } else if close {
        super::live_preview::cancel(app);
    } else {
        super::live_preview::sync(app, id, p);
    }
    if close {
        app.dialog = None;
    }
}

/// The command Camera Settings' OK runs, and its parameters.
fn camera_command(d: &CameraDraft) -> (&'static str, Value) {
    let mut p = json!({
        "name": d.name,
        "type": if d.two_node { "twoNode" } else { "oneNode" },
        "zoom": d.zoom,
        "dof": d.dof,
        "focusDistance": d.focus,
        "aperture": d.aperture,
        "blurLevel": d.blur,
    });
    let id = match d.layer {
        Some(l) => {
            p["layer"] = json!(l);
            "layer.cameraSettings"
        }
        None => "layer.newCamera",
    };
    (id, p)
}

pub fn light(app: &mut EffectcraftApp, ctx: &egui::Context, t: &Tokens) {
    let mut d = app.dialog_state.light.clone();
    let (mut ok, mut close) = (false, false);
    let mut rects = (egui::Rect::NOTHING, egui::Rect::NOTHING, egui::Rect::NOTHING);
    let live = super::live_preview::active(app);
    let place = super::live_preview::place(app);
    let mut preview = app.dialog_state.live.on;
    super::dialogs::modal_placed(ctx, "Light Settings", vec2(480.0, 470.0), t, place, |ui| {
        egui::Grid::new("light-grid").num_columns(2).spacing([14.0, 9.0]).show(ui, |ui| {
            ui.label(tr("Name"));
            let r = ui.add(egui::TextEdit::singleline(&mut d.name).desired_width(220.0));
            crate::widgets::focus_on_open(ui.ctx(), &r, &d.name);
            ui.end_row();
            ui.label(tr("Light Type"));
            egui::ComboBox::from_id_salt("light-type").selected_text(KINDS[d.kind.min(3)].label()).show_ui(ui, |ui| {
                for (i, k) in KINDS.iter().enumerate() {
                    ui.selectable_value(&mut d.kind, i, k.label());
                }
            });
            ui.end_row();
            ui.label(tr("Color"));
            crate::widgets::srgb_color_button(ui, &mut d.color);
            ui.end_row();
            ui.label(tr("Intensity"));
            ui.add(crate::widgets::drag_value(egui::DragValue::new(&mut d.intensity).range(-10000.0..=10000.0).suffix(" %")));
            ui.end_row();
            let kind = KINDS[d.kind.min(3)];
            if kind == LightKind::Spot {
                ui.label(tr("Cone Angle"));
                ui.add(crate::widgets::drag_value(egui::DragValue::new(&mut d.cone_angle).range(0.0..=180.0).suffix("°")));
                ui.end_row();
                ui.label(tr("Cone Feather"));
                ui.add(crate::widgets::drag_value(egui::DragValue::new(&mut d.cone_feather).range(0.0..=100.0).suffix(" %")));
                ui.end_row();
            }
            if kind != LightKind::Ambient {
                ui.label(tr("Falloff"));
                egui::ComboBox::from_id_salt("light-falloff").selected_text(FALLOFFS[d.falloff.min(2)]).show_ui(ui, |ui| {
                    for (i, f) in FALLOFFS.iter().enumerate() {
                        ui.selectable_value(&mut d.falloff, i, *f);
                    }
                });
                ui.end_row();
                if d.falloff > 0 {
                    ui.label(tr("Radius"));
                    ui.add(crate::widgets::drag_value(egui::DragValue::new(&mut d.radius).range(0.0..=100000.0).suffix(" px")));
                    ui.end_row();
                    ui.label(tr("Falloff Distance"));
                    ui.add(crate::widgets::drag_value(egui::DragValue::new(&mut d.falloff_distance).range(0.0..=100000.0).suffix(" px")));
                    ui.end_row();
                }
                ui.label(tr("Shadows"));
                ui.checkbox(&mut d.casts_shadows, tr("Casts Shadows"));
                ui.end_row();
                ui.label(tr("Shadow Darkness"));
                ui.add_enabled(d.casts_shadows, crate::widgets::drag_value(egui::DragValue::new(&mut d.shadow_darkness).range(0.0..=100.0).suffix(" %")));
                ui.end_row();
                ui.label(tr("Shadow Diffusion"));
                ui.add_enabled(d.casts_shadows, crate::widgets::drag_value(egui::DragValue::new(&mut d.shadow_diffusion).range(0.0..=1000.0).suffix(" px")));
                ui.end_row();
            }
        });
        ui.add_space(14.0);
        ui.label(egui::RichText::new(tr("Note: Shadows are only cast from layers with Cast Shadows enabled.")).font(Tokens::ui(11.0)).color(t.text_dim));
        ui.add_space(8.0);
        rects = buttons(ui, t, &mut ok, &mut close, live.then_some(&mut preview));
    });
    app.dialog_state.live.on = preview;
    app.auto.add("dialog.light.ok", rects.0, "OK");
    app.auto.add("dialog.light.cancel", rects.1, "Cancel");
    if live {
        app.auto.add("dialog.light.preview", rects.2, "Preview");
    }
    app.dialog_state.light = d.clone();
    let (id, p) = light_command(&d);
    if ok || super::dialogs::ui_enter(ctx) {
        // A preview already made the edit (one undo step): OK keeps it.
        if !super::live_preview::keep(app)
            && let Err(e) = app.session.execute(id, p)
        {
            app.ui.status = e.to_string();
        }
        close = true;
    } else if close {
        super::live_preview::cancel(app);
    } else {
        super::live_preview::sync(app, id, p);
    }
    if close {
        app.dialog = None;
    }
}

/// The command Light Settings' OK runs, and its parameters.
fn light_command(d: &LightDraft) -> (&'static str, Value) {
    let kind = KINDS[d.kind.min(3)];
    let mut p = json!({
        "name": d.name,
        "kind": kind.label(),
        "color": [d.color[0], d.color[1], d.color[2]],
        "intensity": d.intensity,
    });
    if kind == LightKind::Spot {
        p["coneAngle"] = json!(d.cone_angle);
        p["coneFeather"] = json!(d.cone_feather);
    }
    if kind != LightKind::Ambient {
        p["falloff"] = json!(d.falloff);
        p["radius"] = json!(d.radius);
        p["falloffDistance"] = json!(d.falloff_distance);
        p["castsShadows"] = json!(d.casts_shadows);
        p["shadowDarkness"] = json!(d.shadow_darkness);
        p["shadowDiffusion"] = json!(d.shadow_diffusion);
    }
    let id = match d.layer {
        Some(l) => {
            p["layer"] = json!(l);
            "layer.lightSettings"
        }
        None => "layer.newLight",
    };
    (id, p)
}

/// Menu/shortcut routing: commands that open a 3D settings dialog when invoked without params.
/// Returns true when a dialog was opened.
pub fn route(app: &mut EffectcraftApp, id: &str, params: &Value) -> Result<bool, String> {
    if !params.as_object().is_none_or(|m| m.is_empty() || m.keys().all(|k| k == "layer")) {
        return Ok(false);
    }
    let selected = || -> Option<(u64, bool, bool)> {
        let c = app.session.active_comp()?;
        let id = params.get("layer").and_then(Value::as_u64).or_else(|| app.session.state.selected_layers.first().map(|l| l.0))?;
        let l = c.layer(effectcraft_engine::project::LayerId(id))?;
        Some((id, l.is_camera(), l.is_light()))
    };
    match id {
        "layer.newCamera" if params.get("layer").is_none() => open_camera(app, None).map(|_| true),
        "layer.newLight" if params.get("layer").is_none() => open_light(app, None).map(|_| true),
        "layer.cameraSettings" | "layer.lightSettings" | "layer.settings" => match selected() {
            Some((l, true, _)) => open_camera(app, Some(l)).map(|_| true),
            Some((l, _, true)) => open_light(app, Some(l)).map(|_| true),
            _ => Ok(false),
        },
        _ => Ok(false),
    }
}
