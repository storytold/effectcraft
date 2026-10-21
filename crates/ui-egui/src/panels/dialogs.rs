//! Modal dialogs: About (community links, Contributors and Models credits), New Composition /
//! Composition Settings, Solid Settings and the command palette (Camera/Light Settings live in `dialogs_3d`).

use crate::i18n::{tr, tr_args};
use effectcraft_engine::project::{ItemId, ItemKind, LayerId, LayerSource};
use egui::{Align2, Color32, Rect, Sense, pos2, vec2};
use serde_json::{Value, json};

pub use super::forms::{Field, form, info, open_form};
use crate::icons::{self, Icon};
use crate::theme::Tokens;
use crate::{Dialog, EffectcraftApp};

pub use super::comp_settings::CompDraft;

#[derive(Clone, Debug, Default)]
pub struct DialogState {
    pub comp: CompDraft,
    /// The settings of the last composition made with New Composition (the next one starts
    /// from them, as in After Effects).
    pub last_new_comp: Option<CompDraft>,
    pub editing_existing: bool,
    /// The composition Composition Settings edits (the Project panel's row menu can pick one
    /// the viewer doesn't show).
    pub editing_comp: Option<ItemId>,
    pub solid_name: String,
    pub solid_color: [f32; 3],
    pub solid_size: [u32; 2],
    pub solid_pixel_aspect: f64,
    /// Solid Settings edits this layer's solid (Layer ▸ Layer Settings) instead of making one.
    pub solid_layer: Option<u64>,
    /// Other layers use the edited solid too.
    pub solid_shared: bool,
    /// "Affect all layers that use this solid" (off: the layer gets its own copy).
    pub solid_affect_all: bool,
    pub palette_query: String,
    pub palette_sel: usize,
    /// Settings dialog page id (`general`, `appearance`…).
    pub settings_page: String,
    /// Settings when the dialog opened (Cancel restores them).
    pub prefs_snapshot: Option<effectcraft_engine::prefs::Prefs>,
    /// Keyboard Shortcuts editor.
    pub shortcuts: super::shortcut_editor::EditorState,
    /// The open parameter form.
    pub form: super::forms::Form,
    /// Message box (title, body).
    pub info: (String, String),
    pub camera: super::dialogs_3d::CameraDraft,
    pub light: super::dialogs_3d::LightDraft,
    pub velocity: super::key_dialogs::VelocityDraft,
    pub interp: super::key_dialogs::InterpDraft,
    pub stretch: super::key_dialogs::StretchDraft,
    pub track_options: super::tracker::OptionsDraft,
    pub track_target: super::tracker::TargetDraft,
    pub track_apply: super::tracker::ApplyDraft,
    /// Composition/Layer Marker dialog.
    pub marker: super::markers_ui::MarkerDraft,
    /// Pre-compose dialog.
    pub precompose: super::precomp::PrecomposeDraft,
    /// Layer Style dialog.
    pub layer_style: super::layer_styles_dialog::LayerStyleState,
    /// The unsaved-changes prompt and the command it holds.
    pub unsaved: super::unsaved::Pending,
    /// Deleting Project items in use: the deletion waiting on the prompt.
    pub delete_items: super::delete_items::Pending,
    /// The Gradient Editor's draft and the property it edits.
    pub gradient: super::gradient_editor::Draft,
    /// The user template the Delete Template prompt asks about: its id and name.
    pub delete_template: Option<(String, String)>,
}

pub fn open_new_comp(app: &mut EffectcraftApp) {
    let n = app.session.project.comps().count() + 1;
    let last = app.dialog_state.last_new_comp.clone().unwrap_or_default();
    app.dialog_state.comp =
        CompDraft { name: format!("Comp {n}"), tab: super::comp_settings::Tab::Basic, start_tc: None, dur_tc: None, preset_name: None, ..last };
    app.dialog_state.editing_existing = false;
    app.dialog = Some(Dialog::NewComp);
}

pub fn open_comp_settings(app: &mut EffectcraftApp) -> Result<(), String> {
    let cid = app.session.active_comp_id().ok_or("no composition is open")?;
    open_comp_settings_for(app, cid)
}

/// Composition Settings on `cid`, open in the viewer or not.
pub fn open_comp_settings_for(app: &mut EffectcraftApp, cid: ItemId) -> Result<(), String> {
    let c = app.session.project.comp(cid).ok_or("no composition")?;
    app.dialog_state.comp = CompDraft {
        name: app.session.project.item(cid).map(|i| i.name.clone()).unwrap_or_default(),
        width: c.width,
        height: c.height,
        pixel_aspect: c.pixel_aspect,
        fps: c.frame_rate.as_f64(),
        duration: c.duration.seconds(),
        start: c.display_start.seconds(),
        bg: c.background,
        shutter_angle: c.shutter_angle,
        shutter_phase: c.shutter_phase,
        samples: c.motion_blur_samples,
        adaptive_limit: c.motion_blur_adaptive_limit,
        preserve_frame_rate: c.preserve_frame_rate,
        preserve_resolution: c.preserve_resolution,
        advanced_3d: c.renderer == effectcraft_engine::project::Renderer::Advanced3D,
        ..Default::default()
    };
    app.dialog_state.editing_existing = true;
    app.dialog_state.editing_comp = Some(cid);
    app.dialog = Some(Dialog::CompSettings);
    Ok(())
}

pub fn open_new_solid(app: &mut EffectcraftApp) -> Result<(), String> {
    let c = app.session.active_comp().ok_or("no composition is open")?;
    let n = app.session.project.items.values().filter(|i| matches!(i.kind, effectcraft_engine::project::ItemKind::Solid(_))).count() + 1;
    let d = &mut app.dialog_state;
    d.solid_name = format!("Solid {n}");
    d.solid_size = [c.width, c.height];
    d.solid_pixel_aspect = c.pixel_aspect;
    d.solid_layer = None;
    if d.solid_color == [0.0; 3] {
        d.solid_color = [0.2, 0.45, 0.85];
    }
    app.dialog = Some(Dialog::SolidSettings);
    Ok(())
}

/// Layer ▸ Layer Settings on a solid or adjustment layer: Solid Settings on its solid.
fn open_solid_settings(app: &mut EffectcraftApp, layer: u64, item: ItemId) -> Result<(), String> {
    let s = &app.session;
    let it = s.project.item(item).ok_or("the layer's solid is missing")?;
    let ItemKind::Solid(so) = &it.kind else { return Err("not a solid".into()) };
    let users = s.project.comps().flat_map(|(_, c)| &c.layers).filter(|l| l.source == LayerSource::Solid { item }).count();
    let name = s.active_comp().and_then(|c| c.layer(LayerId(layer))).map(|l| l.name.clone()).unwrap_or_else(|| it.name.clone());
    let d = &mut app.dialog_state;
    (d.solid_name, d.solid_color, d.solid_size, d.solid_pixel_aspect) = (name, so.color, [so.width, so.height], so.pixel_aspect);
    (d.solid_layer, d.solid_shared, d.solid_affect_all) = (Some(layer), users > 1, false);
    app.dialog = Some(Dialog::SolidSettings);
    Ok(())
}

/// Layer ▸ Layer Settings without parameters: Solid Settings for solids and adjustment layers,
/// the layer's name for nulls (cameras and lights: `dialogs_3d::route`). Returns true when a
/// dialog opened.
pub fn route_layer_settings(app: &mut EffectcraftApp, id: &str, params: &Value) -> Result<bool, String> {
    if id != "layer.settings" || !params.as_object().is_none_or(|m| m.keys().all(|k| k == "layer")) {
        return Ok(false);
    }
    let s = &app.session;
    let Some(l) =
        params.get("layer").and_then(Value::as_u64).or_else(|| s.state.selected_layers.first().map(|l| l.0)).and_then(|l| s.active_comp()?.layer(LayerId(l)))
    else {
        return Ok(false);
    };
    let (lid, name) = (l.id.0, l.name.clone());
    match l.source {
        LayerSource::Solid { item } => open_solid_settings(app, lid, item).map(|_| true),
        LayerSource::Null => {
            form(app, "Null Settings", "layer.settings", json!({"layer": lid}), vec![Field::text("name", "Name", &name)]);
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// The room a modal dialog may take: the window less a margin.
pub(crate) fn modal_space(ctx: &egui::Context) -> egui::Vec2 {
    (ctx.content_rect().size() - vec2(48.0, 48.0)).max(vec2(120.0, 80.0))
}

pub(crate) fn modal(ctx: &egui::Context, title: &str, size: egui::Vec2, t: &Tokens, body: impl FnOnce(&mut egui::Ui)) {
    modal_with_scroll(ctx, title, size, t, false, body);
}

pub(crate) fn modal_vertical(ctx: &egui::Context, title: &str, size: egui::Vec2, t: &Tokens, body: impl FnOnce(&mut egui::Ui)) {
    modal_with_scroll(ctx, title, size, t, true, body);
}

fn modal_with_scroll(ctx: &egui::Context, title: &str, size: egui::Vec2, t: &Tokens, vertical_only: bool, body: impl FnOnce(&mut egui::Ui)) {
    let max_height = (modal_space(ctx).y - 84.0).max(40.0);
    modal_fit(ctx, title, size, t, |ui, _| {
        let scroll = if vertical_only { egui::ScrollArea::vertical() } else { egui::ScrollArea::both() };
        scroll.max_height(max_height).auto_shrink([false, true]).show(ui, body);
    });
}

/// [`modal`] without its scroll area, for a dialog that keeps its buttons in view and scrolls
/// only its content: `body` gets the height left under the title, with `size` kept within the
/// window.
pub(crate) fn modal_fit(ctx: &egui::Context, title: &str, size: egui::Vec2, t: &Tokens, body: impl FnOnce(&mut egui::Ui, f32)) {
    let size = size.min(modal_space(ctx));
    let id = egui::Id::new(("modal", title));
    // Dragging the title row moves the dialog off what it edits. The offset is kept within the
    // window, so dragging back never has to undo a part the dialog could not follow.
    let offset_id = id.with("offset");
    let room = ((ctx.content_rect().size() - size) * 0.5).max(vec2(0.0, 0.0));
    let offset = ctx.data(|d| d.get_temp::<egui::Vec2>(offset_id)).unwrap_or_default().clamp(-room, room);
    egui::Modal::new(id)
        .area(egui::Modal::default_area(id).anchor(Align2::CENTER_CENTER, offset))
        .backdrop_color(Color32::from_black_alpha(120))
        .frame(egui::Frame::window(&ctx.style_of(ctx.theme())).fill(t.panel_bg).inner_margin(egui::Margin::same(18)))
        .show(ctx, |ui| {
            ui.set_width(size.x - 36.0);
            ui.set_min_height(size.y - 36.0);
            // The id stays keyed on the untranslated source, so switching language cannot move it.
            let head = ui.label(egui::RichText::new(tr(title)).font(Tokens::semibold(15.0)).color(t.tab_text_active));
            let row = Rect::from_x_y_ranges(ui.max_rect().x_range(), head.rect.y_range());
            let grip = ui.interact(row, id.with("grip"), Sense::drag()).on_hover_cursor(egui::CursorIcon::Grab);
            if grip.dragged() {
                let moved = (offset + grip.drag_delta()).clamp(-room, room);
                ui.ctx().data_mut(|d| d.insert_temp(offset_id, moved));
            }
            ui.add_space(10.0);
            let title_h = ui.cursor().top() - ui.min_rect().top();
            body(ui, (size.y - 36.0 - title_h).max(40.0));
        });
}

/// The buttons of a [`modal_fit`] body of `height`, kept in view: `footer` lays them out first,
/// at the bottom (bottom up: its first row is the lowest), and the rest of the body gets the room
/// above them. Returns that room (a left-to-right row, clipped to it, so a long or wide body
/// scrolls or clips under the buttons instead of pushing them out of the dialog) and its height.
pub(crate) fn above_footer(ui: &mut egui::Ui, height: f32, footer: impl FnOnce(&mut egui::Ui)) -> (egui::Ui, f32) {
    let area = ui.allocate_space(vec2(ui.available_width(), height)).1;
    let mut foot = ui.new_child(egui::UiBuilder::new().max_rect(area).layout(egui::Layout::bottom_up(egui::Align::Min)));
    footer(&mut foot);
    foot.add_space(8.0);
    let body_h = (foot.min_rect().top() - area.top()).max(0.0);
    let room = Rect::from_min_size(area.min, vec2(area.width(), body_h));
    let mut body = ui.new_child(egui::UiBuilder::new().max_rect(room).layout(egui::Layout::left_to_right(egui::Align::Min)));
    body.set_clip_rect(room.intersect(body.clip_rect()));
    (body, body_h)
}

pub fn show(app: &mut EffectcraftApp, ctx: &egui::Context) {
    let Some(d) = app.dialog else { return };
    let t = app.tokens;
    match d {
        Dialog::About => about(app, ctx, &t),
        Dialog::NewComp | Dialog::CompSettings => comp_settings(app, ctx, &t, d == Dialog::CompSettings),
        Dialog::SolidSettings => solid(app, ctx, &t),
        Dialog::CommandPalette => palette(app, ctx, &t),
        Dialog::Settings => super::settings::show(app, ctx, &t),
        Dialog::Shortcuts => super::shortcut_editor::show(app, ctx, &t),
        Dialog::Recovery => super::settings::recovery(app, ctx, &t),
        Dialog::Form => super::forms::show_form(app, ctx, &t),
        Dialog::Info => super::forms::show_info(app, ctx, &t),
        Dialog::ViewOptions => super::forms::show_view_options(app, ctx, &t),
        Dialog::CameraSettings => super::dialogs_3d::camera(app, ctx, &t),
        Dialog::LightSettings => super::dialogs_3d::light(app, ctx, &t),
        Dialog::KeyVelocity => super::key_dialogs::velocity(app, ctx, &t),
        Dialog::KeyInterpolation => super::key_dialogs::interpolation(app, ctx, &t),
        Dialog::TimeStretch => super::key_dialogs::time_stretch(app, ctx, &t),
        Dialog::TrackOptions => super::tracker::options_dialog(app, ctx, &t),
        Dialog::Marker => super::markers_ui::dialog(app, ctx, &t),
        Dialog::Precompose => super::precomp::dialog(app, ctx, &t),
        Dialog::LayerStyles => super::layer_styles_dialog::show(app, ctx, &t),
        Dialog::TrackTarget => super::tracker::target_dialog(app, ctx, &t),
        Dialog::TrackApply => super::tracker::apply_dialog(app, ctx, &t),
        Dialog::RenderTemplates => super::rq_templates::show(app, ctx, &t),
        Dialog::UnsavedChanges => super::unsaved::show(app, ctx, &t),
        Dialog::DeleteItems => super::delete_items::show(app, ctx, &t),
        Dialog::GradientEditor => super::gradient_editor::show(app, ctx, &t),
        Dialog::DeleteTemplate => super::templates::show_delete(app, ctx, &t),
    }
}

/// About tabs: About · Contributors · Models.
const ABOUT_TABS: [(&str, &str); 3] = [("About", "about"), ("Contributors", "contributors"), ("Models", "models")];
/// Height of the Contributors / Models tab bodies (they scroll inside it).
const CREDITS_HEIGHT: f32 = 380.0;

fn about(app: &mut EffectcraftApp, ctx: &egui::Context, t: &Tokens) {
    let mut close = false;
    let mut cmd: Option<&str> = None;
    let tab_id = egui::Id::new("about_tab");
    modal(ctx, "About EffectCraft", vec2(680.0, 520.0), t, |ui| {
        let mut tab = ui.data_mut(|d| d.get_temp::<usize>(tab_id)).unwrap_or(0);
        ui.horizontal(|ui| {
            for (i, (label, id)) in ABOUT_TABS.iter().enumerate() {
                let r = ui.selectable_label(tab == i, *label);
                app.auto.add(&format!("about.tab.{id}"), r.rect, label);
                if r.clicked() {
                    tab = i;
                }
            }
        });
        ui.data_mut(|d| d.insert_temp(tab_id, tab));
        ui.separator();
        match tab {
            1 | 2 => {
                let h = CREDITS_HEIGHT.min(ui.available_height().max(120.0));
                ui.allocate_ui(vec2(ui.available_width(), h), |ui| {
                    ui.set_max_height(h);
                    if tab == 1 {
                        crate::credits::contributors_ui(ui, &mut app.auto);
                    } else {
                        crate::credits::models_ui(ui);
                    }
                });
            }
            _ => about_main(app, ui, t, &mut cmd),
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(tr("MIT OR Apache-2.0 • Fonts: Inter, JetBrains Mono (OFL)")).small().color(t.text_faint));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let r = ui.button(tr("Close"));
                app.auto.add("about.close", r.rect, "Close");
                if r.clicked() {
                    close = true;
                }
            });
        });
    });
    if let Some(c) = cmd {
        let _ = app.session.execute(c, json!({}));
    }
    if close {
        app.dialog = None;
    }
}

/// The About tab: logo, version, blurb and community links.
fn about_main(app: &mut EffectcraftApp, ui: &mut egui::Ui, t: &Tokens, cmd: &mut Option<&'static str>) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 96.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(r, 10.0, Color32::from_rgb(0x1b, 0x22, 0x3c));
    crate::header::paint_logo(p, Rect::from_min_size(r.min + vec2(18.0, 20.0), vec2(56.0, 56.0)));
    p.text(r.min + vec2(90.0, 34.0), Align2::LEFT_CENTER, "EffectCraft", Tokens::semibold(24.0), Color32::WHITE);
    p.text(
        r.min + vec2(90.0, 62.0),
        Align2::LEFT_CENTER,
        format!("Version {}  •  Motion graphics & VFX in pure Rust", env!("CARGO_PKG_VERSION")),
        Tokens::ui(12.0),
        t.text_dim,
    );
    ui.add_space(12.0);
    ui.label(tr("A clean-room, open-source compositor for motion graphics and visual effects: native on macOS, Windows and Linux, and in the browser. Part of the ArtCraft family of creative apps."));
    ui.add_space(14.0);
    let links: [(Icon, &str, &str, &'static str); 4] = [
        (Icon::Chat, "Join the ArtCraft Discord", effectcraft_engine::links::DISCORD, "help.discord"),
        (Icon::Globe, "ArtCraft website", effectcraft_engine::links::WEBSITE, "help.website"),
        (Icon::Sparkle, "EffectCraft home page", effectcraft_engine::links::APP_PAGE, "help.appPage"),
        (Icon::Code, "Source code on GitHub", effectcraft_engine::links::GITHUB, "help.github"),
    ];
    for (icon, label, url, c) in links {
        let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
        let p = ui.painter();
        let discord = c == "help.discord";
        p.rect_filled(
            r,
            6.0,
            if discord {
                Color32::from_rgb(0x58, 0x65, 0xf2)
            } else if resp.hovered() {
                t.hover
            } else {
                t.field_bg
            },
        );
        let foreground = if discord { Color32::WHITE } else { t.text };
        icons::paint(p, Rect::from_center_size(pos2(r.min.x + 18.0, r.center().y), vec2(15.0, 15.0)), icon, foreground);
        p.text(pos2(r.min.x + 36.0, r.center().y), Align2::LEFT_CENTER, tr(label), Tokens::medium(12.5), foreground);
        p.text(
            pos2(r.max.x - 12.0, r.center().y),
            Align2::RIGHT_CENTER,
            url,
            Tokens::ui(11.0),
            if discord { Color32::from_white_alpha(200) } else { t.text_dim },
        );
        app.auto.add(&format!("about.{c}"), r, label);
        if resp.clicked() {
            *cmd = Some(c);
        }
        ui.add_space(4.0);
    }
}

fn comp_settings(app: &mut EffectcraftApp, ctx: &egui::Context, t: &Tokens, existing: bool) {
    let mut d = app.dialog_state.comp.clone();
    let mut result = None;
    let custom = app.session.prefs.comp_presets.clone();
    let mut cmds = Vec::new();
    modal(ctx, "Composition Settings", vec2(600.0, 470.0), t, |ui| {
        result = super::comp_settings::show(ui, &mut d, t, &mut app.auto, &custom, &mut cmds);
    });
    if d.preset_name.is_some() {
        // Save Preset asks for the name in its own window over the dialog, as After Effects does.
        result = None;
        modal(ctx, "Choose Name", vec2(360.0, 150.0), t, |ui| super::comp_settings::choose_name(ui, &mut d, t, &mut app.auto, &custom, &mut cmds));
    }
    // Enter that saved a preset name (or while one is typed) doesn't also press OK.
    let preset_busy = !cmds.is_empty() || d.preset_name.is_some();
    for (id, params) in cmds {
        if let Err(e) = app.session.execute(id, params) {
            app.ui.status = e.to_string();
        }
    }
    if result.is_none() && !preset_busy && ui_enter(ctx) {
        result = Some(super::comp_settings::params(&d));
    }
    app.dialog_state.comp = d;
    match result {
        Some(serde_json::Value::Null) => app.dialog = None,
        Some(mut params) => {
            if existing && let (Some(cid), Some(m)) = (app.dialog_state.editing_comp, params.as_object_mut()) {
                m.insert("comp".into(), json!(cid.0));
            }
            let r = if existing { app.session.execute("comp.settings", params) } else { app.session.execute("comp.new", params) };
            if !existing && r.is_ok() {
                app.dialog_state.last_new_comp = Some(app.dialog_state.comp.clone());
            }
            if let Err(e) = r {
                app.ui.status = e.to_string();
            }
            app.dialog = None;
        }
        None => {}
    }
}

/// One frame of the open dialog with `key` pressed.
#[cfg(test)]
pub(crate) fn press_key(app: &mut EffectcraftApp, ctx: &egui::Context, key: egui::Key, modifiers: egui::Modifiers) {
    let mut raw = egui::RawInput::default();
    raw.events.push(egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers });
    let mut out = ctx.run_ui(raw, |ui| {
        app.auto.begin_frame();
        show(app, ui.ctx());
    });
    out.textures_delta.clear();
}

/// The letter that answers the open confirmation with Cmd (Ctrl elsewhere): Cmd+D is Delete in
/// Delete Items, Delete Template and Delete Preset, Cmd+R is Revert in Revert Project. The native menu bar hands these keys back
/// while the prompt is open ([`crate::native_menu::activate`]).
pub(crate) fn command_key(app: &EffectcraftApp) -> Option<egui::Key> {
    match app.dialog? {
        Dialog::DeleteItems | Dialog::DeleteTemplate => Some(egui::Key::D),
        Dialog::Shortcuts if super::shortcut_editor::confirming(app) => Some(egui::Key::D),
        Dialog::UnsavedChanges if super::unsaved::is_revert(app) => Some(egui::Key::R),
        _ => None,
    }
}

/// Cmd+`key` (Ctrl elsewhere) pressed, also while a text field has focus: ask before the dialog
/// is drawn, so the field does not edit with the key first.
pub(crate) fn ui_command_key(ctx: &egui::Context, key: egui::Key) -> bool {
    ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, key))
}

pub(crate) fn ui_enter(ctx: &egui::Context) -> bool {
    !ctx.egui_wants_keyboard_input() && ctx.input(|i| i.key_pressed(egui::Key::Enter))
}

/// Solid Settings: a new solid (Layer ▸ New ▸ Solid…) or the layer's solid (Layer Settings).
fn solid(app: &mut EffectcraftApp, ctx: &egui::Context, t: &Tokens) {
    let (mut close, mut ok) = (false, false);
    let ds = &app.dialog_state;
    let (mut name, mut color, mut size, mut par, mut affect_all) =
        (ds.solid_name.clone(), ds.solid_color, ds.solid_size, ds.solid_pixel_aspect, ds.solid_affect_all);
    let (editing, shared) = (ds.solid_layer, ds.solid_shared);
    let comp_size = app.session.active_comp().map(|c| (c.width, c.height)).unwrap_or((1920, 1080));
    let lock_id = egui::Id::new("solid-lock-aspect");
    let mut lock = ctx.data(|m| m.get_temp::<bool>(lock_id)).unwrap_or(true);
    let auto = &mut app.auto;
    modal(ctx, "Solid Settings", vec2(480.0, if editing.is_some() { 430.0 } else { 390.0 }), t, |ui| {
        ui.horizontal(|ui| {
            ui.label(tr("Name:"));
            let r = ui.add(egui::TextEdit::singleline(&mut name).desired_width(320.0));
            crate::widgets::focus_on_open(ui.ctx(), &r, &name);
            auto.add("dialog.solid.name", r.rect, "Name");
        });
        ui.add_space(8.0);
        ui.label(egui::RichText::new(tr("Size")).strong());
        egui::Grid::new("solid-grid").num_columns(2).spacing([14.0, 8.0]).show(ui, |ui| {
            let (ow, oh) = (size[0], size[1]);
            ui.label(tr("Width:"));
            ui.horizontal(|ui| {
                let r = ui.add(crate::widgets::drag_value(egui::DragValue::new(&mut size[0]).range(1..=30000).suffix(" px")));
                auto.add("dialog.solid.width", r.rect, "Width");
                let r = ui.checkbox(&mut lock, tr_args("Lock Aspect Ratio to {}", &[&(super::comp_settings::aspect_label(ow as f64, oh as f64))]));
                auto.add("dialog.solid.lockAspect", r.rect, "Lock Aspect Ratio");
            });
            ui.end_row();
            ui.label(tr("Height:"));
            let r = ui.add(crate::widgets::drag_value(egui::DragValue::new(&mut size[1]).range(1..=30000).suffix(" px")));
            auto.add("dialog.solid.height", r.rect, "Height");
            ui.end_row();
            if lock && ow > 0 && oh > 0 {
                if size[0] != ow {
                    size[1] = ((size[0] as f64) * oh as f64 / ow as f64).round().max(1.0) as u32;
                } else if size[1] != oh {
                    size[0] = ((size[1] as f64) * ow as f64 / oh as f64).round().max(1.0) as u32;
                }
            }
            ui.label(tr("Pixel Aspect Ratio:"));
            let cur = super::comp_settings::PIXEL_ASPECTS
                .iter()
                .find(|(_, v)| (v - par).abs() < 1e-3)
                .map(|p| p.0.to_string())
                .unwrap_or_else(|| format!("{par:.2}"));
            let r = egui::ComboBox::from_id_salt("solid-par").width(240.0).selected_text(cur).show_ui(ui, |ui| {
                for (label, v) in super::comp_settings::PIXEL_ASPECTS {
                    if ui.selectable_label((v - par).abs() < 1e-3, *label).clicked() {
                        par = *v;
                    }
                }
            });
            auto.add("dialog.solid.pixelAspect", r.response.rect, "Pixel Aspect Ratio");
            ui.end_row();
        });
        let pct = |a: u32, b: u32| 100.0 * a as f64 / b.max(1) as f64;
        ui.label(
            egui::RichText::new(format!(
                "Width: {:.1}% of comp\nHeight: {:.1}% of comp\nFrame Aspect Ratio: {}",
                pct(size[0], comp_size.0),
                pct(size[1], comp_size.1),
                super::comp_settings::aspect_label(size[0] as f64 * par, size[1] as f64)
            ))
            .color(Color32::GRAY),
        );
        ui.add_space(4.0);
        let r = ui.button(tr("Make Comp Size"));
        auto.add("dialog.solid.makeCompSize", r.rect, "Make Comp Size");
        if r.clicked() {
            size = [comp_size.0, comp_size.1];
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(tr("Color:"));
            let r = crate::widgets::srgb_color_button(ui, &mut color);
            auto.add("dialog.solid.color", r.rect, "Color");
        });
        if editing.is_some() {
            ui.add_space(8.0);
            let r = ui
                .add_enabled(shared, egui::Checkbox::new(&mut affect_all, tr("Affect all layers that use this solid")))
                .on_disabled_hover_text(tr("Only this layer uses this solid."));
            auto.add("dialog.solid.affectAll", r.rect, "Affect all layers that use this solid");
            if shared && !affect_all {
                ui.label(egui::RichText::new(tr("This change will create a new solid for this layer.")).color(t.text_dim).size(11.0));
            }
        }
        ui.add_space(14.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let r = ui.add(egui::Button::new(egui::RichText::new(tr("   OK   ")).color(Color32::WHITE)).fill(t.accent));
            auto.add("dialog.solid.ok", r.rect, "OK");
            ok = r.clicked();
            let r = ui.button(tr("Cancel"));
            auto.add("dialog.solid.cancel", r.rect, "Cancel");
            close = r.clicked();
        });
    });
    ctx.data_mut(|m| m.insert_temp(lock_id, lock));
    let ds = &mut app.dialog_state;
    (ds.solid_name, ds.solid_color, ds.solid_size, ds.solid_pixel_aspect, ds.solid_affect_all) = (name, color, size, par, affect_all);
    if ok || ui_enter(ctx) {
        if let Err(e) = app.session.execute(solid_command(ds), solid_params(ds)) {
            app.ui.status = e.to_string();
        }
        close = true;
    }
    if close {
        app.dialog = None;
    }
}

/// The command Solid Settings' OK runs.
fn solid_command(d: &DialogState) -> &'static str {
    if d.solid_layer.is_some() { "layer.settings" } else { "layer.newSolid" }
}

/// Its parameters.
fn solid_params(d: &DialogState) -> Value {
    let mut p = json!({
        "name": d.solid_name, "color": d.solid_color, "width": d.solid_size[0], "height": d.solid_size[1], "pixelAspect": d.solid_pixel_aspect,
    });
    if let Some(l) = d.solid_layer {
        p["layer"] = json!(l);
        p["affectAll"] = json!(d.solid_affect_all || !d.solid_shared);
    }
    p
}

/// ⌘⇧P: fuzzy search over every command (engine + UI) and effect.
fn palette(app: &mut EffectcraftApp, ctx: &egui::Context, t: &Tokens) {
    let mut q = app.dialog_state.palette_query.clone();
    let mut run: Option<(String, serde_json::Value)> = None;
    let mut close = false;
    let items: Vec<(String, String, serde_json::Value, String)> = {
        let mut v: Vec<(String, String, serde_json::Value, String)> = crate::menus::menu_items(app)
            .into_iter()
            .filter(|m| m.enabled)
            .map(|m| (m.path.join(" ▸ ") + " ▸ " + &m.label, m.id, if m.params.is_null() { json!({}) } else { m.params }, m.shortcut.unwrap_or_default()))
            .collect();
        for c in effectcraft_engine::command_specs().iter().filter(|c| c.menu.is_empty() && c.journal && c.params == "{}") {
            if app.session.is_enabled(c.id) {
                v.push((c.label.to_string(), c.id.to_string(), json!({}), c.shortcut.unwrap_or("").to_string()));
            }
        }
        v
    };
    let ql = q.to_lowercase();
    let matches: Vec<&(String, String, serde_json::Value, String)> = items
        .iter()
        .filter(|(label, id, ..)| {
            let hay = format!("{} {}", label.to_lowercase(), id.to_lowercase());
            ql.split_whitespace().all(|w| hay.contains(w))
        })
        .take(14)
        .collect();
    modal(ctx, "Command Palette", vec2(560.0, 470.0), t, |ui| {
        let r = ui.add(egui::TextEdit::singleline(&mut q).hint_text(tr("Type a command or effect…")).desired_width(f32::INFINITY).font(Tokens::ui(14.0)));
        // Take the keyboard when the palette opens (and if anything else takes it away), but not
        // while the field already has it: egui's `request_focus` interrupts the IME composition,
        // and asking on every frame cancelled the pinyin being typed, so no Chinese could be
        // entered.
        if !ctx.memory(|m| m.has_focus(r.id)) {
            r.request_focus();
        }
        ui.add_space(8.0);
        let sel = app.dialog_state.palette_sel.min(matches.len().saturating_sub(1));
        for (i, (label, id, params, sc)) in matches.iter().enumerate() {
            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::click());
            let p = ui.painter();
            if i == sel || resp.hovered() {
                p.rect_filled(r, 4.0, if i == sel { t.row_selected } else { t.hover });
            }
            p.text(pos2(r.min.x + 8.0, r.center().y), Align2::LEFT_CENTER, label, Tokens::ui(12.5), t.text);
            if !sc.is_empty() {
                p.text(pos2(r.max.x - 8.0, r.center().y), Align2::RIGHT_CENTER, crate::menus::shortcut_text(sc), Tokens::ui(11.0), t.text_dim);
            }
            if resp.clicked() {
                run = Some((id.clone(), (*params).clone()));
            }
        }
    });
    let (down, up, enter) = ctx.input(|i| (i.key_pressed(egui::Key::ArrowDown), i.key_pressed(egui::Key::ArrowUp), i.key_pressed(egui::Key::Enter)));
    if down {
        app.dialog_state.palette_sel = (app.dialog_state.palette_sel + 1).min(matches.len().saturating_sub(1));
    }
    if up {
        app.dialog_state.palette_sel = app.dialog_state.palette_sel.saturating_sub(1);
    }
    if enter && let Some((_, id, params, _)) = matches.get(app.dialog_state.palette_sel) {
        run = Some((id.clone(), (*params).clone()));
    }
    if q != app.dialog_state.palette_query {
        app.dialog_state.palette_sel = 0;
    }
    app.dialog_state.palette_query = q;
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        close = true;
    }
    if let Some((id, params)) = run {
        app.dialog = None;
        if let Err(e) = crate::menus::invoke(app, ctx, &id, params) {
            app.ui.status = e;
        }
        return;
    }
    if close {
        app.dialog = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> (EffectcraftApp, egui::Context) {
        let ctx = egui::Context::default();
        let mut app = EffectcraftApp::new(effectcraft_engine::Session::default());
        crate::theme::install(&ctx, &app.tokens, crate::i18n::language(&app));
        app.session.execute("comp.new", json!({"name": "Main", "width": 640, "height": 360})).unwrap();
        (app, ctx)
    }

    fn frame(app: &mut EffectcraftApp, ctx: &egui::Context) {
        let mut out = ctx.run_ui(Default::default(), |ui| {
            app.auto.begin_frame();
            show(app, ui.ctx());
        });
        out.textures_delta.clear();
    }

    /// One frame with `events` in its input; returns what it asked the platform to do.
    fn frame_with(app: &mut EffectcraftApp, ctx: &egui::Context, events: Vec<egui::Event>) -> egui::FullOutput {
        let input = egui::RawInput { events, ..Default::default() };
        let mut out = ctx.run_ui(input, |ui| {
            app.auto.begin_frame();
            show(app, ui.ctx());
        });
        out.textures_delta.clear();
        out
    }

    /// The IME composing `text` so far (what the platform sends as the user types pinyin).
    fn composing(text: &str) -> egui::Event {
        egui::Event::Ime(egui::ImeEvent::Preedit { text: text.to_string(), active_range_chars: None })
    }

    /// Whether the frame asked the platform to cancel the composition in progress. winit does
    /// that whenever egui's focus is requested (`Memory::request_focus` interrupts the IME), and
    /// on Windows the IME then drops what is being typed.
    fn cancels_composition(out: &egui::FullOutput) -> bool {
        out.platform_output.ime.as_ref().is_some_and(|i| i.should_interrupt_composition)
    }

    /// The palette's search field used to take the keyboard on every frame it was drawn, and
    /// asking egui for the focus interrupts the IME composition: the pinyin being typed was
    /// cancelled as it was typed, so no Chinese could be entered.
    #[test]
    fn the_palette_leaves_a_pinyin_composition_alone() {
        let (mut app, ctx) = app();
        crate::menus::invoke(&mut app, &ctx, "app.commandPalette", json!({})).unwrap();
        assert_eq!(app.dialog, Some(Dialog::CommandPalette));
        // The field takes the keyboard when the palette opens.
        frame(&mut app, &ctx);
        // Typing pinyin: the composition goes on and nothing cancels it.
        let mut typed = String::new();
        for k in "nihao".chars() {
            typed.push(k);
            let out = frame_with(&mut app, &ctx, vec![composing(&typed)]);
            assert!(out.platform_output.ime.is_some(), "the field owns the IME");
            assert!(!cancels_composition(&out), "typing `{typed}` cancelled the composition");
        }
        // The whole composition is in the field (and in the query the next frame filters with).
        assert_eq!(app.dialog_state.palette_query, "nihao");
        // Something else taking the keyboard takes it away for a frame; the field claims it back
        // (a request is what gets the keys back, and there is no composition to interrupt then).
        ctx.memory_mut(|m| m.request_focus(egui::Id::new("elsewhere")));
        assert!(frame_with(&mut app, &ctx, vec![]).platform_output.ime.is_none(), "the field lost the keyboard");
        assert!(frame_with(&mut app, &ctx, vec![]).platform_output.ime.is_some(), "and takes it back");
    }

    fn solid_item(app: &EffectcraftApp, l: u64) -> ItemId {
        match app.session.active_comp().unwrap().layer(LayerId(l)).unwrap().source {
            LayerSource::Solid { item } => item,
            _ => panic!("not a solid"),
        }
    }

    #[test]
    fn layer_settings_opens_solid_settings_on_the_layers_solid() {
        let (mut app, ctx) = app();
        let a =
            app.session.execute("layer.newSolid", json!({"name": "Red", "color": "#ff0000", "width": 200, "height": 100})).unwrap()["layer"].as_u64().unwrap();
        crate::menus::invoke(&mut app, &ctx, "layer.settings", json!({})).unwrap();
        assert_eq!(app.dialog, Some(Dialog::SolidSettings));
        let d = &app.dialog_state;
        assert_eq!((d.solid_layer, d.solid_shared, d.solid_size, d.solid_name.as_str()), (Some(a), false, [200, 100], "Red"));
        frame(&mut app, &ctx);
        for id in ["name", "width", "lockAspect", "height", "pixelAspect", "makeCompSize", "color", "affectAll", "ok", "cancel"] {
            assert!(app.auto.find(&format!("dialog.solid.{id}")).is_some(), "dialog.solid.{id}");
        }
        // Only this layer uses the solid: OK changes it in place.
        app.dialog_state.solid_color = [0.0, 1.0, 0.0];
        assert_eq!(solid_command(&app.dialog_state), "layer.settings");
        app.session.execute(solid_command(&app.dialog_state), solid_params(&app.dialog_state)).unwrap();
        let item = solid_item(&app, a);
        assert!(matches!(&app.session.project.item(item).unwrap().kind, ItemKind::Solid(so) if so.color == [0.0, 1.0, 0.0]));
        // Shared with a duplicate: "Affect all" starts off, and OK gives the layer its own solid.
        let b = app.session.execute("edit.duplicate", json!({})).unwrap()[0].as_u64().unwrap();
        crate::menus::invoke(&mut app, &ctx, "layer.settings", json!({"layer": b})).unwrap();
        assert!(app.dialog_state.solid_shared && !app.dialog_state.solid_affect_all);
        app.dialog_state.solid_size = [64, 64];
        app.session.execute(solid_command(&app.dialog_state), solid_params(&app.dialog_state)).unwrap();
        assert_ne!(solid_item(&app, a), solid_item(&app, b));
        assert!(matches!(&app.session.project.item(solid_item(&app, a)).unwrap().kind, ItemKind::Solid(so) if so.width == 200));
        // A null asks for its name; New Solid makes a solid with the comp's pixel aspect.
        app.session.execute("layer.newNull", json!({})).unwrap();
        crate::menus::invoke(&mut app, &ctx, "layer.settings", json!({})).unwrap();
        assert_eq!((app.dialog, app.dialog_state.form.command.as_str()), (Some(Dialog::Form), "layer.settings"));
        open_new_solid(&mut app).unwrap();
        assert_eq!((solid_command(&app.dialog_state), app.dialog_state.solid_pixel_aspect), ("layer.newSolid", 1.0));
    }

    #[test]
    fn new_comp_from_selection_asks_when_several_items_are_selected() {
        let (mut app, ctx) = app();
        let a = app.session.execute("file.importSolid", json!({"name": "A", "width": 100, "height": 50})).unwrap();
        let b = app.session.execute("file.importSolid", json!({"name": "B", "width": 300, "height": 50})).unwrap();
        let ids: Vec<ItemId> = [a, b].iter().map(|r| ItemId(r["item"].as_u64().unwrap())).collect();
        app.session.state.project_selection = vec![ids[0]];
        // One item: made at once.
        crate::menus::invoke(&mut app, &ctx, "file.newCompFromSelection", json!({})).unwrap();
        assert!(app.dialog.is_none());
        app.session.state.project_selection = ids.clone();
        crate::menus::invoke(&mut app, &ctx, "file.newCompFromSelection", json!({})).unwrap();
        assert_eq!((app.dialog, app.dialog_state.form.command.as_str()), (Some(Dialog::Form), "file.newCompFromSelection"));
        frame(&mut app, &ctx);
        for k in ["single", "dimensionsFrom", "duration", "addToRenderQueue", "sequence", "overlap", "overlapDuration", "transition"] {
            assert!(app.auto.find(&format!("form.field.{k}")).is_some(), "{k}");
        }
        // Its parameters are ones the command accepts (checked as agents' calls are).
        let comps = app.session.project.comps().count();
        app.session.execute_checked("file.newCompFromSelection", app.dialog_state.form.params()).unwrap();
        assert_eq!(app.session.project.comps().count(), comps + 1, "Single Composition is the default");
    }

    #[test]
    fn a_dialog_opens_with_its_name_field_focused_and_selected() {
        let (mut app, ctx) = app();
        let selected = |ctx: &egui::Context| {
            let id = ctx.memory(|m| m.focused())?;
            egui::TextEdit::load_state(ctx, id)?.cursor.char_range().map(|r| r.as_sorted_char_range()).map(|r| r.start.0..r.end.0)
        };
        open_new_comp(&mut app);
        let name = app.dialog_state.comp.name.clone();
        frame(&mut app, &ctx);
        frame(&mut app, &ctx);
        assert_eq!(selected(&ctx), Some(0..name.chars().count()), "typing replaces “{name}”");
        // Afterwards the focus is the user's: it is not taken back.
        ctx.memory_mut(|m| m.stop_text_input());
        frame(&mut app, &ctx);
        assert!(ctx.memory(|m| m.focused()).is_none());
        // Closed and opened again: focused again.
        app.dialog = None;
        frame(&mut app, &ctx);
        frame(&mut app, &ctx);
        open_new_solid(&mut app).unwrap();
        frame(&mut app, &ctx);
        frame(&mut app, &ctx);
        assert_eq!(selected(&ctx), Some(0..app.dialog_state.solid_name.chars().count()));
    }

    #[test]
    fn composition_settings_register_every_control_and_new_comp_remembers() {
        let (mut app, ctx) = app();
        open_new_comp(&mut app);
        for tab in [super::super::comp_settings::Tab::Basic, super::super::comp_settings::Tab::Advanced, super::super::comp_settings::Tab::Renderer] {
            app.dialog_state.comp.tab = tab;
            frame(&mut app, &ctx);
        }
        let ids = [
            "name",
            "tab.basic",
            "tab.advanced",
            "tab.renderer",
            "ok",
            "cancel",
            "renderer", // 3D Renderer tab (last shown)
        ];
        for id in ids {
            assert!(app.auto.find(&format!("dialog.comp.{id}")).is_some(), "dialog.comp.{id}");
        }
        app.dialog_state.comp.tab = super::super::comp_settings::Tab::Advanced;
        frame(&mut app, &ctx);
        for id in ["anchor.4", "preserveFrameRate", "preserveResolution", "shutterAngle", "shutterPhase", "samples", "adaptiveLimit"] {
            assert!(app.auto.find(&format!("dialog.comp.{id}")).is_some(), "dialog.comp.{id}");
        }
        app.dialog_state.comp.tab = super::super::comp_settings::Tab::Basic;
        frame(&mut app, &ctx);
        for id in ["preset", "width", "lockAspect", "height", "pixelAspect", "frameRate", "frameRates", "startTimecode", "duration", "background"] {
            assert!(app.auto.find(&format!("dialog.comp.{id}")).is_some(), "dialog.comp.{id}");
        }
        // A typed rate that isn't in the list, then OK (Enter): the next New Composition starts
        // from these settings with a new name.
        (app.dialog_state.comp.width, app.dialog_state.comp.height, app.dialog_state.comp.fps) = (1000, 500, 18.0);
        let p = super::super::comp_settings::params(&app.dialog_state.comp);
        assert_eq!(p["frameRate"], json!(18.0));
        let t = app.tokens;
        let mut out = ctx.run_ui(Default::default(), |ui| {
            app.auto.begin_frame();
            ui.input_mut(|i| {
                i.events.push(egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() })
            });
            comp_settings(&mut app, ui.ctx(), &t, false);
        });
        out.textures_delta.clear();
        assert!(app.dialog.is_none());
        let c = app.session.active_comp().unwrap();
        assert_eq!((c.width, c.height, c.frame_rate.as_f64()), (1000, 500, 18.0));
        open_new_comp(&mut app);
        let n = &app.dialog_state.comp;
        assert_eq!((n.width, n.height, n.fps, n.name.as_str()), (1000, 500, 18.0, "Comp 3"));
    }
}
