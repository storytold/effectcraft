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
pub mod gradient_editor;
pub mod gradient_handles;
pub mod graph;
pub mod graph_tools;
pub mod home;
pub mod info;
pub mod key_dialogs;
pub mod layer_panel;
pub mod layer_styles_dialog;
pub mod learn;
pub mod live_preview;
pub mod markers_ui;
pub mod media_panels;
pub mod misc;
pub mod object_matte_tool;
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
pub mod text_range_handles;
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

use crate::i18n::tr;
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
    /// An animation preset from Effects & Presets, with its display name for the drag ghost.
    Preset { preset: PresetRef, name: String },
    /// A property dragged from the timeline (to the Essential Graphics panel).
    Property { layer: u64, prop: u64 },
    /// Files from the Media Browser (import; into the timeline: import and add).
    Files(Vec<String>),
}

/// An animation preset in Effects & Presets.
#[derive(Clone, Debug, PartialEq)]
pub enum PresetRef {
    /// A text animation preset id (`layer.applyTextPreset`).
    Text(String),
    /// A built-in animation preset id, e.g. a transition (`anim.applyBuiltinPreset`).
    Builtin(String),
    /// A user `.ecpreset` file path (`anim.applyPreset`).
    File(String),
}

impl PresetRef {
    /// The command and params that apply the preset to the selected layers.
    pub fn command(&self) -> (&'static str, serde_json::Value) {
        match self {
            PresetRef::Text(id) => ("layer.applyTextPreset", serde_json::json!({"preset": id})),
            PresetRef::Builtin(id) => ("anim.applyBuiltinPreset", serde_json::json!({"preset": id})),
            PresetRef::File(path) => ("anim.applyPreset", serde_json::json!({"path": path})),
        }
    }

    /// The command and params that apply the preset to `layer` (a drop target).
    pub fn apply_to(&self, layer: u64) -> (&'static str, serde_json::Value) {
        let (id, mut params) = self.command();
        if let Some(obj) = params.as_object_mut() {
            // `layer.applyTextPreset` takes one `layer`; the animation preset commands take `layers`.
            match self {
                PresetRef::Text(_) => obj.insert("layer".into(), serde_json::json!(layer)),
                PresetRef::Builtin(_) | PresetRef::File(_) => obj.insert("layers".into(), serde_json::json!([layer])),
            };
        }
        (id, params)
    }
}

/// Layers carried by a Project item drag. Dragging a selected item uses the
/// complete selection, while dragging an unselected item uses only that item.
/// Folders and data files are not composition layers.
pub(crate) fn project_drop_items(app: &EffectcraftApp, dragged: u64) -> Vec<u64> {
    use effectcraft_engine::project::{FootageKind, ItemId, ItemKind};

    let selected = &app.session.state.project_selection;
    let candidates: Vec<u64> = if selected.iter().any(|id| id.0 == dragged) {
        selected.iter().map(|id| id.0).collect()
    } else {
        // One unselected item goes through as is, so a folder or data file still gets the
        // engine's error instead of a silent no-op.
        return vec![dragged];
    };
    candidates
        .into_iter()
        .filter(|id| {
            app.session.project.item(ItemId(*id)).is_some_and(|item| match &item.kind {
                ItemKind::Comp(_) | ItemKind::Solid(_) => true,
                ItemKind::Footage(f) => f.kind != FootageKind::Data,
                ItemKind::Folder => false,
            })
        })
        .collect()
}

/// The current time formatted per project settings (timecode with `;` for drop-frame, frames
/// or Feet + Frames).
pub fn timecode(session: &Session, comp: &Comp, t: Tick) -> String {
    effectcraft_engine::commands::time::display_time(session, comp, t)
}

/// Ctrl/Cmd-click on a current-time display toggles the project's Time Display Style between
/// Timecode and Frames, as in After Effects (through Project Settings: one undo step).
pub(crate) fn toggle_time_display(app: &mut EffectcraftApp, resp: &egui::Response) {
    if !(resp.clicked() && resp.ctx.input(|i| i.modifiers.command)) {
        return;
    }
    let timecode = app.session.project.settings.time_display == effectcraft_engine::project::TimeDisplayStyle::Timecode;
    if let Err(e) = app.session.execute("file.projectSettings", serde_json::json!({"timeDisplay": if timecode { "frames" } else { "timecode" }})) {
        app.ui.status = e.to_string();
    }
}

/// Spacebar held outside a text field: the Hand tool (drags pan the viewer and scroll the
/// Timeline).
pub(crate) fn space_hand(ctx: &egui::Context) -> bool {
    ctx.input(|i| i.key_down(egui::Key::Space)) && !ctx.egui_wants_keyboard_input()
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
    // The menu opens in the window that shows the panel (a panel can have its own window).
    if app.panel_viewport(ui.ctx(), panel) != ui.ctx().viewport_id() {
        return;
    }
    let mut close = false;
    let area = egui::Area::new(id.with("area")).order(egui::Order::Foreground).fixed_pos(pos).show(ui.ctx(), |ui| {
        egui::Frame::popup(ui.style()).show(ui, |ui| {
            ui.set_min_width(200.0);
            if ui.button(tr("Close Panel")).clicked() {
                let _ = app.dock_command(ui.ctx(), craft_ui::docking::Action::Close { panel });
                close = true;
            }
            let floating = app.ui.floating.iter().any(|f| f.panels.contains(&panel));
            if !floating && ui.button(tr("Undock Panel")).clicked() {
                let r = crate::dock_ui::default_float_rect(ui.ctx().content_rect());
                let _ = app.dock_command(ui.ctx(), craft_ui::docking::Action::Float { panel, rect: r });
                close = true;
            }
            // Multiple monitors: a native window of its own, which can go on another screen.
            if !ui.ctx().embed_viewports() {
                let windowed = app.in_panel_window(panel);
                if ui.button(if windowed { tr("Move Panel to Main Window") } else { tr("Move Panel to New Window") }).clicked() {
                    let _ = crate::menus::invoke(app, &ui.ctx().clone(), "window.panelWindow", serde_json::json!({"panel": panel, "window": !windowed}));
                    close = true;
                }
            }
            if !floating && ui.button(if app.ui.maximized == Some(panel) { tr("Restore Panel Size") } else { tr("Maximize Panel") }).clicked() {
                app.toggle_maximize(panel);
                close = true;
            }
            ui.separator();
            match panel {
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
                    if ui.button(tr("Composition Settings…")).clicked() {
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

#[cfg(test)]
mod preset_drop_tests {
    use super::PresetRef;
    use serde_json::json;

    #[test]
    fn preset_drops_target_the_layer_with_each_commands_param() {
        assert_eq!(PresetRef::Text("typewriter".into()).apply_to(7), ("layer.applyTextPreset", json!({"preset": "typewriter", "layer": 7})));
        assert_eq!(PresetRef::Builtin("fadeIn".into()).apply_to(7), ("anim.applyBuiltinPreset", json!({"preset": "fadeIn", "layers": [7]})));
        assert_eq!(PresetRef::File("/p/a.ecpreset".into()).apply_to(7), ("anim.applyPreset", json!({"path": "/p/a.ecpreset", "layers": [7]})));
        // Double-click applies to the selection: no layer given.
        assert_eq!(PresetRef::Text("typewriter".into()).command(), ("layer.applyTextPreset", json!({"preset": "typewriter"})));
    }
}
