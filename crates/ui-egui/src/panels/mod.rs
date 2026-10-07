//! Panel bodies.

pub mod anim_tools;
pub mod camera_tracker_ui;
pub mod comp_settings;
pub mod content_fill_panel;
pub mod delete_items;
pub mod dialogs;
pub mod dialogs_3d;
pub mod effect_controls;
pub mod effects_presets;
pub mod essential;
pub mod expr_bar;
pub mod expr_editor;
pub mod flowchart;
pub mod footage_panel;
pub mod forms;
pub mod fx_editors;
pub mod fx_widgets;
pub mod graph;
pub mod graph_tools;
pub mod home;
pub mod info;
pub mod key_dialogs;
pub mod layer_panel;
pub mod layer_styles_dialog;
pub mod learn;
pub mod markers_ui;
pub mod media_panels;
pub mod misc;
pub mod paint_panels;
pub mod panel_kit;
pub mod path_vr_panels;
pub mod precomp;
pub mod project;
pub mod properties;
pub mod puppet_tool;
pub mod render_queue;
pub mod roto_tool;
pub mod rq_templates;
pub mod scopes_panel;
pub mod script_console;
pub mod scriptui_view;
pub mod settings;
pub mod shortcut_editor;
pub mod templates;
pub mod text_panels;
pub mod timeline;
pub mod tracker;
pub mod unsaved;
pub mod viewer;
pub mod viewer_drop;
pub mod viewer_overlays;
pub mod viewer_text;
pub mod viewer_tools;
pub mod viewers;
pub mod waveform;

use effectcraft_engine::Session;
use effectcraft_engine::project::Comp;
use effectcraft_engine::time::Tick;
use egui::Rect;

use crate::EffectcraftApp;
use crate::dock::PanelKind;

/// Drag-and-drop payloads between panels.
#[derive(Clone, Debug)]
pub enum DragPayload {
    /// A project item (footage, comp, solid).
    Item(u64),
    /// An effect id from Effects & Presets.
    Effect(String),
    /// A property dragged from the timeline (to the Essential Graphics panel).
    Property { layer: u64, prop: u64 },
    /// Files from the Media Browser (import; into the timeline: import and add).
    Files(Vec<String>),
}

/// The current time formatted per project settings (timecode with `;` for drop-frame, frames
/// or Feet + Frames).
pub fn timecode(session: &Session, comp: &Comp, t: Tick) -> String {
    effectcraft_engine::commands::time::display_time(session, comp, t)
}

pub fn show(app: &mut EffectcraftApp, ui: &mut egui::Ui, p: PanelKind, rect: Rect) {
    match p {
        PanelKind::Composition => viewers::show(app, ui, 0, rect),
        PanelKind::Viewer(n) => viewers::show(app, ui, n, rect),
        PanelKind::Timeline => timeline::show(app, ui, rect),
        PanelKind::Project => project::show(app, ui, rect),
        PanelKind::EffectControls => effect_controls::show(app, ui, rect),
        PanelKind::EffectsPresets => effects_presets::show(app, ui, rect),
        PanelKind::Info => info::show(app, ui, rect),
        PanelKind::Preview => misc::preview(app, ui, rect),
        PanelKind::Character => text_panels::character(app, ui, rect),
        PanelKind::Paragraph => text_panels::paragraph(app, ui, rect),
        PanelKind::Align => text_panels::align(app, ui, rect),
        PanelKind::Properties => properties::show(app, ui, rect),
        PanelKind::Audio => misc::audio(app, ui, rect),
        PanelKind::History => misc::history(app, ui, rect),
        PanelKind::Markers => misc::markers(app, ui, rect),
        PanelKind::MaskInterpolation => anim_tools::mask_interpolation(app, ui, rect),
        PanelKind::ScriptConsole => script_console::show(app, ui, rect),
        PanelKind::Wiggler => anim_tools::wiggler(app, ui, rect),
        PanelKind::Smoother => anim_tools::smoother(app, ui, rect),
        PanelKind::MotionSketch => anim_tools::motion_sketch(app, ui, rect),
        PanelKind::RenderQueue => render_queue::show(app, ui, rect),
        PanelKind::Tracker => tracker::show(app, ui, rect),
        PanelKind::Layer => layer_panel::show(app, ui, rect),
        PanelKind::Paint => paint_panels::paint(app, ui, rect),
        PanelKind::Brushes => paint_panels::brushes(app, ui, rect),
        PanelKind::EssentialGraphics => essential::show(app, ui, rect),
        PanelKind::ScriptPanel(id) => scriptui_view::panel(app, ui, rect, id),
        PanelKind::Flowchart => flowchart::show(app, ui, rect),
        PanelKind::LumetriScopes => scopes_panel::show(app, ui, rect),
        PanelKind::Footage => footage_panel::show(app, ui, rect),
        PanelKind::MediaBrowser => media_panels::media_browser(app, ui, rect),
        PanelKind::Metadata => media_panels::metadata(app, ui, rect),
        PanelKind::Progress => media_panels::progress(app, ui, rect),
        PanelKind::ContentAwareFill => content_fill_panel::show(app, ui, rect),
        PanelKind::CreateNullsFromPaths => path_vr_panels::create_nulls(app, ui, rect),
        PanelKind::VrCompEditor => path_vr_panels::vr_editor(app, ui, rect),
    }
}

/// The ≡ panel menu (opened from a tab).
pub fn panel_menu_popup(app: &mut EffectcraftApp, ui: &mut egui::Ui) {
    let id = egui::Id::new("panel-menu");
    let Some((panel, pos)) = ui.ctx().data(|d| d.get_temp::<(PanelKind, egui::Pos2)>(id)) else { return };
    let mut close = false;
    let area = egui::Area::new(id.with("area")).order(egui::Order::Foreground).fixed_pos(pos).show(ui.ctx(), |ui| {
        egui::Frame::popup(ui.style()).show(ui, |ui| {
            ui.set_min_width(200.0);
            if ui.button("Close Panel").clicked() {
                app.close_panel(panel);
                close = true;
            }
            let floating = app.ui.floating.iter().any(|f| f.panels.contains(&panel));
            if !floating && ui.button("Undock Panel").clicked() {
                let r = crate::dock_ui::default_float_rect(ui.ctx().content_rect());
                app.edit_layout(|l| l.float(panel, r));
                close = true;
            }
            if ui.button(if app.maximized_contains(panel) { "Restore Panel Size" } else { "Maximize Panel" }).clicked() {
                app.toggle_maximize(panel);
                close = true;
            }
            ui.separator();
            match panel {
                PanelKind::EffectControls => {
                    for (name, key, value) in [
                        ("Show Effect Label Swatch", "appearance.showEffectLabelSwatch", app.session.prefs.appearance.show_effect_label_swatch),
                        (
                            "Use Label Color for Effect Background",
                            "appearance.useLabelColorForEffectBackground",
                            app.session.prefs.appearance.use_label_color_for_effect_background,
                        ),
                    ] {
                        let response = ui.selectable_label(value, name);
                        app.auto.add(&format!("effectControls.panelMenu.{key}"), response.rect, name);
                        if response.clicked() {
                            if let Err(error) = app.session.execute("prefs.set", serde_json::json!({"key": key, "value": !value})) {
                                app.ui.status = error.to_string();
                            }
                            close = true;
                        }
                    }
                }
                PanelKind::Timeline => {
                    for (label, id) in [
                        ("Hide Shy Layers", "hideShy"),
                        ("Enable Frame Blending", "frameBlending"),
                        ("Enable Motion Blur", "motionBlur"),
                        ("Draft 3D", "draft3d"),
                    ] {
                        if ui.button(label).clicked() {
                            let _ = app.session.execute("comp.setSwitch", serde_json::json!({"switch": id}));
                            close = true;
                        }
                    }
                    if ui.button("Composition Settings…").clicked() {
                        let _ = dialogs::open_comp_settings(app);
                        close = true;
                    }
                }
                PanelKind::Composition | PanelKind::Viewer(_) => {
                    for (label, cmd) in [
                        ("New Viewer", "view.newViewer"),
                        ("Composition Settings…", "app.compSettings"),
                        ("View Options…", "view.layerControls"),
                        ("Show Grid", "view.grid"),
                        ("Title/Action Safe", "view.safeMargins"),
                    ] {
                        if ui.button(label).clicked() {
                            let _ = crate::menus::invoke(app, &ui.ctx().clone(), cmd, serde_json::json!({}));
                            close = true;
                        }
                    }
                }
                _ => {
                    ui.label(egui::RichText::new(panel.title()).weak());
                }
            }
        });
    });
    if close || crate::widgets::pressed_outside(ui.ctx(), &area.response) {
        ui.ctx().data_mut(|d| d.remove::<(PanelKind, egui::Pos2)>(id));
    }
}
