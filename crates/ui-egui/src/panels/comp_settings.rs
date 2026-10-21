//! Composition Settings dialog (also New Composition), laid out like After Effects': name, then
//! Basic / Advanced / 3D Renderer tabs, Cancel / OK. Every control registers an automation id
//! (`dialog.comp.<field>`).

use crate::i18n::{tr, tr_args};
use crate::icons::Icon;
use effectcraft_engine::prefs::CompPreset;
use effectcraft_engine::time::{FrameRate, format_timecode_frames, parse_timecode};
use egui::{Color32, vec2};
use serde_json::json;

use crate::automation::Registry;
use crate::theme::Tokens;

/// AE's composition presets: (name, width, height, pixel aspect, fps). "Custom" is implicit.
pub const PRESETS: &[(&str, u32, u32, f64, f64)] = &[
    ("Social Media Portrait HD · 1080x1920 · 30 fps", 1080, 1920, 1.0, 30.0),
    ("Social Media Landscape HD · 1920x1080 · 30 fps", 1920, 1080, 1.0, 30.0),
    ("Social Media Square · 1080x1080 · 30 fps", 1080, 1080, 1.0, 30.0),
    ("Social Media Portrait · 1080x1350 · 30 fps", 1080, 1350, 1.0, 30.0),
    ("NTSC DV", 720, 480, 0.9091, 29.97),
    ("NTSC DV Widescreen", 720, 480, 1.2121, 29.97),
    ("NTSC D1", 720, 486, 0.9091, 29.97),
    ("NTSC D1 Widescreen", 720, 486, 1.2121, 29.97),
    ("NTSC D1 Square Pix", 720, 540, 1.0, 29.97),
    ("NTSC D1 Widescreen Square Pix", 872, 486, 1.0, 29.97),
    ("PAL D1/DV", 720, 576, 1.0940, 25.0),
    ("PAL D1/DV Widescreen", 720, 576, 1.4587, 25.0),
    ("PAL D1/DV Square Pix", 788, 576, 1.0, 25.0),
    ("PAL D1/DV Widescreen Square Pix", 1050, 576, 1.0, 25.0),
    ("HDV/HDTV 720 29.97", 1280, 720, 1.0, 29.97),
    ("HDV/HDTV 720 25", 1280, 720, 1.0, 25.0),
    ("DVCPRO HD 720 23.976", 960, 720, 1.3333, 23.976),
    ("DVCPRO HD 720 25", 960, 720, 1.3333, 25.0),
    ("DVCPRO HD 720 29.97", 960, 720, 1.3333, 29.97),
    ("HDTV 1080 24", 1920, 1080, 1.0, 24.0),
    ("HDTV 1080 25", 1920, 1080, 1.0, 25.0),
    ("HDTV 1080 29.97", 1920, 1080, 1.0, 29.97),
    ("HDV 1080 25", 1440, 1080, 1.3333, 25.0),
    ("HDV 1080 29.97", 1440, 1080, 1.3333, 29.97),
    ("DVCPRO HD 1080 25", 1440, 1080, 1.3333, 25.0),
    ("DVCPRO HD 1080 29.97", 1280, 1080, 1.5, 29.97),
    ("Cineon Half", 1828, 1332, 1.0, 24.0),
    ("Cineon Full", 3656, 2664, 1.0, 24.0),
    ("Film (2K)", 2048, 1556, 1.0, 24.0),
    ("Film (4K)", 4096, 3112, 1.0, 24.0),
    ("UHD 4K 23.976", 3840, 2160, 1.0, 23.976),
    ("UHD 4K 25", 3840, 2160, 1.0, 25.0),
    ("UHD 4K 29.97", 3840, 2160, 1.0, 29.97),
    ("UHD 8K 23.976", 7680, 4320, 1.0, 23.976),
    ("UHD 8K 25", 7680, 4320, 1.0, 25.0),
    ("UHD 8K 29.97", 7680, 4320, 1.0, 29.97),
];

/// AE's Pixel Aspect Ratio choices.
pub const PIXEL_ASPECTS: &[(&str, f64)] = &[
    ("Square Pixels", 1.0),
    ("D1/DV NTSC (0.91)", 0.9091),
    ("D1/DV NTSC Widescreen (1.21)", 1.2121),
    ("D1/DV PAL (1.09)", 1.0940),
    ("D1/DV PAL Widescreen (1.46)", 1.4587),
    ("Anamorphic 2:1 (2)", 2.0),
    ("HDV 1080/DVCPRO HD 720 (1.33)", 1.3333),
    ("DVCPRO HD 1080 (1.5)", 1.5),
];

const FRAME_RATES: &[f64] = &[8.0, 10.0, 12.0, 12.5, 15.0, 23.976, 24.0, 25.0, 29.97, 30.0, 48.0, 50.0, 59.94, 60.0, 120.0];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Basic,
    Advanced,
    Renderer,
}

/// The dialog's working copy.
#[derive(Clone, Debug)]
pub struct CompDraft {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub pixel_aspect: f64,
    pub fps: f64,
    pub duration: f64,
    pub start: f64,
    pub bg: [f32; 3],
    pub lock_aspect: bool,
    pub anchor: u8,
    pub shutter_angle: f64,
    pub shutter_phase: f64,
    pub samples: u32,
    pub adaptive_limit: u32,
    pub preserve_frame_rate: bool,
    pub preserve_resolution: bool,
    pub advanced_3d: bool,
    pub tab: Tab,
    /// Timecode text being edited (start, duration).
    pub start_tc: Option<String>,
    pub dur_tc: Option<String>,
    /// The name being typed for Save Preset (the name row shows while `Some`).
    pub preset_name: Option<String>,
}

impl Default for CompDraft {
    fn default() -> Self {
        CompDraft {
            name: "Comp 1".into(),
            width: 1920,
            height: 1080,
            pixel_aspect: 1.0,
            fps: 29.97,
            duration: 10.0,
            start: 0.0,
            bg: [0.0, 0.0, 0.0],
            lock_aspect: true,
            anchor: 4,
            shutter_angle: 180.0,
            shutter_phase: -90.0,
            samples: 16,
            adaptive_limit: 128,
            preserve_frame_rate: false,
            preserve_resolution: false,
            advanced_3d: false,
            tab: Tab::Basic,
            start_tc: None,
            dur_tc: None,
            preset_name: None,
        }
    }
}

/// `16:9 (1.78)`: the nearest small whole-number ratio and its decimal.
pub fn aspect_label(w: f64, h: f64) -> String {
    let r = w / h.max(1.0);
    let best = (1..=32u32).map(|b| ((r * b as f64).round() as u32, b)).filter(|(a, _)| *a > 0).find(|(a, b)| (*a as f64 / *b as f64 - r).abs() < 0.006);
    match best {
        Some((a, b)) => format!("{a}:{b} ({r:.2})"),
        None => format!("{r:.2}"),
    }
}

/// AE's name for a background color when it is one of the obvious ones.
fn color_name(c: [f32; 3]) -> &'static str {
    match c.map(|v| (v * 255.0).round() as u8) {
        [0, 0, 0] => "Black",
        [255, 255, 255] => "White",
        _ => "",
    }
}

/// AE prints the hour without a leading zero: `0;00;10;00`.
fn ae_timecode(smpte: &str) -> String {
    let (neg, rest) = smpte.strip_prefix('-').map_or((false, smpte), |r| (true, r));
    let rest = rest.strip_prefix('0').filter(|r| r.as_bytes().first().is_some_and(u8::is_ascii_digit)).unwrap_or(rest);
    format!("{}{rest}", if neg { "-" } else { "" })
}

fn timecode_field(ui: &mut egui::Ui, buf: &mut Option<String>, secs: &mut f64, rate: FrameRate, min_frames: i64) -> egui::Response {
    let df = rate.supports_drop_frame();
    // Durations are whole frames in AE: 10 s at 29.97 fps is 0;00;10;00, not 9;29.
    let shown = ae_timecode(&format_timecode_frames((*secs * rate.as_f64()).round() as i64, rate, df));
    let mut text = buf.clone().unwrap_or_else(|| shown.clone());
    let r = ui.add(egui::TextEdit::singleline(&mut text).desired_width(96.0).font(Tokens::mono(12.0)));
    if r.changed() {
        *buf = Some(text.clone());
    }
    if r.lost_focus() {
        if let Ok(f) = parse_timecode(&text, rate, df, 0) {
            *secs = rate.tick_of(f.max(min_frames)).seconds();
        }
        *buf = None;
    }
    let base = format!("Base {}{}", rate.timecode_base(), if df { "drop" } else { "" });
    ui.label(egui::RichText::new(format!("is {shown}  {base}")).color(Color32::GRAY));
    r
}

fn name_focus_id() -> egui::Id {
    egui::Id::new("comp-preset-name-focus")
}

/// Save Preset's Choose Name prompt (drawn in its own modal over the dialog while
/// `d.preset_name` is `Some`): OK or Enter saves the draft's frame size, pixel aspect and frame
/// rate under the name, replacing a saved preset with that name; Cancel or Escape closes it.
pub fn choose_name(
    ui: &mut egui::Ui,
    d: &mut CompDraft,
    t: &Tokens,
    auto: &mut Registry,
    custom: &[CompPreset],
    cmds: &mut Vec<(&'static str, serde_json::Value)>,
) {
    let Some(mut name) = d.preset_name.take() else { return };
    let mut keep = true;
    let r = ui.add(egui::TextEdit::singleline(&mut name).desired_width(ui.available_width()));
    auto.add("dialog.comp.presetName", r.rect, "Preset Name");
    if ui.data_mut(|m| m.remove_temp::<bool>(name_focus_id())).unwrap_or(false) {
        r.request_focus();
    }
    let trimmed = name.trim();
    let builtin = PRESETS.iter().any(|p| p.0 == trimmed);
    let exists = custom.iter().any(|q| q.name == trimmed);
    let ok = !trimmed.is_empty() && !builtin && trimmed.chars().count() <= CompPreset::MAX_NAME;
    let note = if builtin {
        tr("A built-in preset has this name")
    } else if exists {
        tr("Replaces your preset with this name")
    } else {
        ""
    };
    ui.add_space(4.0);
    ui.label(egui::RichText::new(note).color(Color32::GRAY));
    ui.add_space(8.0);
    let mut save = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    let mut cancel = ui.input(|i| i.key_pressed(egui::Key::Escape));
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let b = ui.add_enabled(ok, egui::Button::new(egui::RichText::new(tr("   OK   ")).color(Color32::WHITE)).fill(t.accent));
        auto.add("dialog.comp.presetNameSave", b.rect, "OK");
        save |= b.clicked();
        let c = ui.button(tr("Cancel"));
        auto.add("dialog.comp.presetNameCancel", c.rect, "Cancel");
        cancel |= c.clicked();
    });
    if ok && save {
        cmds.push(("comp.savePreset", json!({"name": trimmed, "width": d.width, "height": d.height, "pixelAspect": d.pixel_aspect, "frameRate": d.fps})));
        keep = false;
    }
    if cancel {
        keep = false;
    }
    if keep {
        d.preset_name = Some(name);
    }
}

/// Whether the draft's frame size, pixel aspect and frame rate are those of a preset.
fn matches(d: &CompDraft, w: u32, h: u32, par: f64, fps: f64) -> bool {
    w == d.width && h == d.height && (par - d.pixel_aspect).abs() < 1e-3 && (fps - d.fps).abs() < 1e-3
}

/// Draw the dialog. Returns `Some(params)` when OK is pressed, `Some(Null)` on Cancel. `custom`
/// are the presets the user saved; saving or deleting one pushes its engine command to `cmds`.
pub fn show(
    ui: &mut egui::Ui,
    d: &mut CompDraft,
    t: &Tokens,
    auto: &mut Registry,
    custom: &[CompPreset],
    cmds: &mut Vec<(&'static str, serde_json::Value)>,
) -> Option<serde_json::Value> {
    let mut out = None;
    ui.horizontal(|ui| {
        ui.label(tr("Composition Name:"));
        let r = ui.add(egui::TextEdit::singleline(&mut d.name).desired_width(300.0));
        crate::widgets::focus_on_open(ui.ctx(), &r, &d.name);
        auto.add("dialog.comp.name", r.rect, "Composition Name");
    });
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        for (tab, label, key) in [(Tab::Basic, "Basic", "basic"), (Tab::Advanced, "Advanced", "advanced"), (Tab::Renderer, "3D Renderer", "renderer")] {
            let r = ui.selectable_label(d.tab == tab, label);
            auto.add(&format!("dialog.comp.tab.{key}"), r.rect, label);
            if r.clicked() {
                d.tab = tab;
            }
        }
    });
    ui.separator();
    let rate = FrameRate::from_f64(d.fps);
    match d.tab {
        Tab::Basic => {
            egui::Grid::new("comp-basic").num_columns(2).spacing([12.0, 9.0]).show(ui, |ui| {
                ui.label(tr("Preset:"));
                // The user's own presets win over a built-in one with the same values.
                let own = custom.iter().find(|q| matches(d, q.width, q.height, q.pixel_aspect, q.frame_rate));
                let current = own
                    .map(|q| q.name.as_str())
                    .or_else(|| PRESETS.iter().find(|(_, w, h, par, f)| matches(d, *w, *h, *par, *f)).map(|p| p.0))
                    .unwrap_or("Custom");
                ui.horizontal(|ui| {
                    let r = egui::ComboBox::from_id_salt("comp-preset").width(300.0).selected_text(current).show_ui(ui, |ui| {
                        // Your own presets first, then the built-in ones.
                        for q in custom {
                            if ui.selectable_label(q.name == current, &q.name).clicked() {
                                (d.width, d.height, d.pixel_aspect, d.fps) = (q.width, q.height, q.pixel_aspect, q.frame_rate);
                            }
                        }
                        if !custom.is_empty() {
                            ui.separator();
                        }
                        for (name, w, h, par, f) in PRESETS {
                            if ui.selectable_label(*name == current, *name).clicked() {
                                (d.width, d.height, d.pixel_aspect, d.fps) = (*w, *h, *par, *f);
                            }
                        }
                    });
                    auto.add("dialog.comp.preset", r.response.rect, "Preset");
                    let (save_r, _) = ui.allocate_exact_size(vec2(20.0, 20.0), egui::Sense::hover());
                    let save = crate::widgets::icon_button(ui, save_r, Icon::Plus, d.preset_name.is_some(), t, egui::Id::new("comp-preset-save"))
                        .on_hover_text(tr("Save Preset…"));
                    auto.add("dialog.comp.presetSave", save_r, "Save Preset");
                    if save.clicked() {
                        d.preset_name = Some(own.map(|q| q.name.clone()).unwrap_or_default());
                        ui.data_mut(|m| m.insert_temp(name_focus_id(), true));
                    }
                    if let Some(q) = own {
                        let (del_r, _) = ui.allocate_exact_size(vec2(20.0, 20.0), egui::Sense::hover());
                        let del = crate::widgets::icon_button(ui, del_r, Icon::Trash, false, t, egui::Id::new("comp-preset-delete"))
                            .on_hover_text(tr("Delete Preset"));
                        auto.add("dialog.comp.presetDelete", del_r, "Delete Preset");
                        if del.clicked() {
                            cmds.push(("comp.deletePreset", json!({"name": q.name})));
                        }
                    }
                });
                ui.end_row();
                ui.label(tr("Width:"));
                ui.horizontal(|ui| {
                    let (ow, oh) = (d.width, d.height);
                    let r = ui.add(crate::widgets::drag_value(egui::DragValue::new(&mut d.width).range(4..=30000).suffix(" px")));
                    auto.add("dialog.comp.width", r.rect, "Width");
                    let lbl = format!("Lock Aspect Ratio to {}", aspect_label(ow as f64, oh as f64));
                    let r = ui.checkbox(&mut d.lock_aspect, lbl);
                    auto.add("dialog.comp.lockAspect", r.rect, "Lock Aspect Ratio");
                    if d.lock_aspect && ow != d.width && ow > 0 {
                        d.height = ((d.width as f64) * oh as f64 / ow as f64).round().max(4.0) as u32;
                    }
                });
                ui.end_row();
                ui.label(tr("Height:"));
                let (ow, oh) = (d.width, d.height);
                let r = ui.add(crate::widgets::drag_value(egui::DragValue::new(&mut d.height).range(4..=30000).suffix(" px")));
                auto.add("dialog.comp.height", r.rect, "Height");
                if d.lock_aspect && oh != d.height && oh > 0 {
                    d.width = ((d.height as f64) * ow as f64 / oh as f64).round().max(4.0) as u32;
                }
                ui.end_row();
                ui.label(tr("Pixel Aspect Ratio:"));
                ui.horizontal(|ui| {
                    let cur = PIXEL_ASPECTS
                        .iter()
                        .find(|(_, v)| (v - d.pixel_aspect).abs() < 1e-3)
                        .map(|p| p.0.to_string())
                        .unwrap_or_else(|| format!("{:.2}", d.pixel_aspect));
                    let r = egui::ComboBox::from_id_salt("comp-par").width(220.0).selected_text(cur).show_ui(ui, |ui| {
                        for (name, v) in PIXEL_ASPECTS {
                            if ui.selectable_label((v - d.pixel_aspect).abs() < 1e-3, *name).clicked() {
                                d.pixel_aspect = *v;
                            }
                        }
                    });
                    auto.add("dialog.comp.pixelAspect", r.response.rect, "Pixel Aspect Ratio");
                    ui.label(
                        egui::RichText::new(tr_args("Frame Aspect Ratio: {}", &[&(aspect_label(d.width as f64 * d.pixel_aspect, d.height as f64))]))
                            .color(Color32::GRAY),
                    );
                });
                ui.end_row();
                ui.label(tr("Frame Rate:"));
                ui.horizontal(|ui| {
                    // Any rate can be typed; the list holds the common ones.
                    let r = ui.add(crate::widgets::drag_value(egui::DragValue::new(&mut d.fps).range(1.0..=999.0).speed(0.01).max_decimals(3)));
                    auto.add("dialog.comp.frameRate", r.rect, "Frame Rate");
                    let r = egui::ComboBox::from_id_salt("comp-fps").width(18.0).selected_text("").show_ui(ui, |ui| {
                        for f in FRAME_RATES {
                            if ui.selectable_label((d.fps - f).abs() < 1e-3, format!("{f}")).clicked() {
                                d.fps = *f;
                            }
                        }
                    });
                    auto.add("dialog.comp.frameRates", r.response.rect, "Frame Rate presets");
                    ui.label(tr("frames per second"));
                    if rate.supports_drop_frame() {
                        ui.label(egui::RichText::new(tr("Drop Frame")).color(Color32::GRAY));
                    }
                });
                ui.end_row();
                ui.label("");
                let mb = d.width as f64 * d.height as f64 * 4.0 / (1024.0 * 1024.0);
                ui.label(egui::RichText::new(tr_args("{} x {}, {}MB per 8bpc frame", &[&d.width, &d.height, &format!("{mb:.1}")])).color(Color32::GRAY));
                ui.end_row();
                ui.label(tr("Start Timecode:"));
                let r = ui.horizontal(|ui| timecode_field(ui, &mut d.start_tc, &mut d.start, rate, i64::MIN / 4)).inner;
                auto.add("dialog.comp.startTimecode", r.rect, "Start Timecode");
                ui.end_row();
                ui.label(tr("Duration:"));
                let r = ui.horizontal(|ui| timecode_field(ui, &mut d.dur_tc, &mut d.duration, rate, 1)).inner;
                auto.add("dialog.comp.duration", r.rect, "Duration");
                ui.end_row();
                ui.label(tr("Background Color:"));
                ui.horizontal(|ui| {
                    let r = crate::widgets::srgb_color_button(ui, &mut d.bg);
                    auto.add("dialog.comp.background", r.rect, "Background Color");
                    ui.label(color_name(d.bg));
                });
                ui.end_row();
            });
        }
        Tab::Advanced => {
            egui::Grid::new("comp-adv").num_columns(2).spacing([12.0, 9.0]).show(ui, |ui| {
                ui.label(tr("Anchor:"));
                egui::Grid::new("comp-anchor").spacing([2.0, 2.0]).show(ui, |ui| {
                    for i in 0..9u8 {
                        let on = d.anchor == i;
                        let b = egui::Button::new(if on { "●" } else { "" }).min_size(vec2(22.0, 22.0)).fill(if on { t.accent } else { t.field_bg });
                        let r = ui.add(b);
                        auto.add(&format!("dialog.comp.anchor.{i}"), r.rect, "Anchor");
                        if r.clicked() {
                            d.anchor = i;
                        }
                        if i % 3 == 2 {
                            ui.end_row();
                        }
                    }
                });
                ui.end_row();
                ui.label("");
                let r = ui.checkbox(&mut d.preserve_frame_rate, tr("Preserve frame rate when nested or in render queue"));
                auto.add("dialog.comp.preserveFrameRate", r.rect, "Preserve frame rate when nested or in render queue");
                ui.end_row();
                ui.label("");
                let r = ui.checkbox(&mut d.preserve_resolution, tr("Preserve resolution when nested"));
                auto.add("dialog.comp.preserveResolution", r.rect, "Preserve resolution when nested");
                ui.end_row();
                ui.label(egui::RichText::new(tr("Motion Blur")).strong());
                ui.end_row();
                ui.label(tr("Shutter Angle:"));
                let r = ui.add(crate::widgets::drag_value(egui::DragValue::new(&mut d.shutter_angle).range(0.0..=720.0).suffix("°")));
                auto.add("dialog.comp.shutterAngle", r.rect, "Shutter Angle");
                ui.end_row();
                ui.label(tr("Shutter Phase:"));
                let r = ui.add(crate::widgets::drag_value(egui::DragValue::new(&mut d.shutter_phase).range(-360.0..=360.0).suffix("°")));
                auto.add("dialog.comp.shutterPhase", r.rect, "Shutter Phase");
                ui.end_row();
                ui.label(tr("Samples Per Frame:"));
                let r = ui.add(crate::widgets::drag_value(egui::DragValue::new(&mut d.samples).range(2..=64)));
                auto.add("dialog.comp.samples", r.rect, "Samples Per Frame");
                ui.end_row();
                ui.label(tr("Adaptive Sample Limit:"));
                let r = ui.add(crate::widgets::drag_value(egui::DragValue::new(&mut d.adaptive_limit).range(16..=256)));
                auto.add("dialog.comp.adaptiveLimit", r.rect, "Adaptive Sample Limit");
                ui.end_row();
            });
        }
        Tab::Renderer => {
            ui.horizontal(|ui| {
                ui.label(tr("Renderer:"));
                let r = egui::ComboBox::from_id_salt("comp-renderer").selected_text(tr(if d.advanced_3d { "Advanced 3D" } else { "Classic 3D" })).show_ui(
                    ui,
                    |ui| {
                        ui.selectable_value(&mut d.advanced_3d, false, tr("Classic 3D"));
                        ui.selectable_value(&mut d.advanced_3d, true, tr("Advanced 3D"));
                    },
                );
                auto.add("dialog.comp.renderer", r.response.rect, "Renderer");
            });
        }
    }
    ui.add_space(14.0);
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let r = ui.add(egui::Button::new(egui::RichText::new(tr("   OK   ")).color(Color32::WHITE)).fill(t.accent));
            auto.add("dialog.comp.ok", r.rect, "OK");
            if r.clicked() {
                out = Some(params(d));
            }
            let r = ui.button(tr("Cancel"));
            auto.add("dialog.comp.cancel", r.rect, "Cancel");
            if r.clicked() {
                out = Some(serde_json::Value::Null);
            }
        });
    });
    out
}

/// `comp.new` / `comp.settings` parameters for the draft.
pub fn params(d: &CompDraft) -> serde_json::Value {
    json!({
        "name": d.name, "width": d.width, "height": d.height, "pixelAspect": d.pixel_aspect, "frameRate": d.fps,
        "duration": d.duration, "startTime": d.start, "background": [d.bg[0], d.bg[1], d.bg[2]], "anchor": d.anchor,
        "shutterAngle": d.shutter_angle, "shutterPhase": d.shutter_phase, "motionBlurSamples": d.samples, "adaptiveSampleLimit": d.adaptive_limit,
        "preserveFrameRate": d.preserve_frame_rate, "preserveResolution": d.preserve_resolution,
        "renderer": if d.advanced_3d { "advanced3D" } else { "classic3D" },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aspect_labels_match_ae() {
        assert_eq!(aspect_label(1920.0, 1080.0), "16:9 (1.78)");
        assert_eq!(aspect_label(1080.0, 1080.0), "1:1 (1.00)");
        assert_eq!(aspect_label(1080.0, 1350.0), "4:5 (0.80)");
        assert_eq!(aspect_label(720.0 * 0.9091, 480.0), "15:11 (1.36)");
    }

    #[test]
    fn ae_timecode_drops_hour_padding() {
        assert_eq!(ae_timecode("00;00;10;00"), "0;00;10;00");
        assert_eq!(ae_timecode("12:00:00:00"), "12:00:00:00");
        assert_eq!(ae_timecode("-00:00:01:00"), "-0:00:01:00");
    }

    #[test]
    fn presets_resolve_by_value() {
        let p = PRESETS.iter().find(|p| p.0 == "HDTV 1080 29.97").unwrap();
        assert_eq!((p.1, p.2), (1920, 1080));
    }
}
