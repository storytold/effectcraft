//! The Tools bar: home, the tool slots, tool options, snapping, workspaces and the community
//! buttons (Discord is always one click away).

use crate::i18n::{tr, tr_args};
use egui::{Align2, Color32, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use serde_json::json;

use crate::icons::{self, Icon};
use crate::state::Tool;
use crate::theme::Tokens;
use crate::widgets;
use crate::{Dialog, EffectcraftApp};
use effectcraft_color::BlendMode;
use effectcraft_engine::commands::shape_tool::{PaintKind, ToolPaint};
use effectcraft_engine::geom::vec2 as gv2;
use effectcraft_engine::project::{LayerId, LayerSource, PropGroup};
use effectcraft_engine::viewer as vw;

/// Draws the Tools bar into `rect`: brand (About), Home, the tool slots with their fly-out
/// menus and press-and-hold picks, options for the active tool (shape paint, puppet, roto),
/// Snapping, then Discord, GitHub, website, appearance and workspace controls. Clicks apply
/// tool, dialog, theme and workspace changes to `app` or run their commands.
pub fn show(app: &mut EffectcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let p = ui.painter().clone();
    p.rect_filled(rect, 0.0, t.header_bg);
    p.line_segment([rect.left_bottom(), rect.right_bottom()], Stroke::new(1.0, t.app_bg));
    let cy = rect.center().y;
    let x = rect.min.x + if app.integrated_titlebar && !app.ui.show_menu_bar { 80.0 } else { 10.0 };
    let x = brand_and_home(app, ui, &p, x, cy);
    let x = tool_slots(app, ui, &p, x, cy);
    let x = tool_options_bar(app, ui, &p, x + 10.0, cy);
    let x = snapping_toggle(app, ui, &p, x, cy);
    right_side(app, ui, &p, rect, x, cy);
}

/// The brand mark (About) and the Home button; returns the x after them.
fn brand_and_home(app: &mut EffectcraftApp, ui: &mut egui::Ui, p: &egui::Painter, mut x: f32, cy: f32) -> f32 {
    let t = app.tokens;
    let brand = Rect::from_min_size(pos2(x, cy - 12.0), vec2(24.0, 24.0));
    paint_logo(p, brand);
    let bresp = ui.interact(brand, egui::Id::unique("brand"), Sense::click());
    app.auto.add_response("header.about", &bresp, "About EffectCraft");
    widgets::focus_outline(ui, &bresp, &t);
    if bresp.on_hover_text(tr("About EffectCraft")).clicked() {
        app.dialog = Some(Dialog::About);
    }
    x += 32.0;

    let home = Rect::from_min_size(pos2(x, cy - 13.0), vec2(26.0, 26.0));
    let home_response = widgets::icon_button(ui, home, Icon::Home, app.ui.start_screen, &t, egui::Id::unique("tool-home")).on_hover_text(tr("Home"));
    app.auto.add_response("header.home", &home_response, "Home");
    if home_response.clicked() {
        app.ui.start_screen = !app.ui.start_screen;
    }
    x += 30.0;
    p.line_segment([pos2(x, cy - 10.0), pos2(x, cy + 10.0)], Stroke::new(1.0, t.separator));
    x + 6.0
}

/// The tool slots (with group separators after camera tools and pan-behind); returns the x after them.
fn tool_slots(app: &mut EffectcraftApp, ui: &mut egui::Ui, p: &egui::Painter, mut x: f32, cy: f32) -> f32 {
    let t = app.tokens;
    let held_id = egui::Id::unique("tool-slot-held");
    if ui.input(|i| i.pointer.any_pressed()) {
        ui.data_mut(|d| d.remove::<egui::Id>(held_id));
    }
    for (si, slot) in Tool::SLOTS.iter().enumerate() {
        let r = Rect::from_min_size(pos2(x, cy - 13.0), vec2(26.0, 26.0));
        tool_slot(app, ui, p, si, slot, r, held_id);
        x += 28.0;
        if matches!(si, 2 | 5 | 7 | 10 | 13) {
            x += 4.0;
            p.line_segment([pos2(x, cy - 10.0), pos2(x, cy + 10.0)], Stroke::new(1.0, t.separator));
            x += 6.0;
        }
    }
    x
}

/// One tool slot button in `r`: select on click, Ctrl+double-click Pan Behind, and the fly-out.
fn tool_slot(app: &mut EffectcraftApp, ui: &mut egui::Ui, p: &egui::Painter, si: usize, slot: &[Tool], r: Rect, held_id: egui::Id) {
    let t = app.tokens;
    let cur = app.ui.slot_tools.get(si).copied().unwrap_or(slot[0]);
    let active = slot.contains(&app.ui.tool);
    let id = egui::Id::unique(("tool-slot", si));
    let resp = widgets::icon_button(ui, r, cur.icon(), active, &t, id);
    if slot.len() > 1 {
        // Flyout corner triangle.
        p.add(egui::Shape::convex_polygon(
            vec![pos2(r.max.x - 2.0, r.max.y - 6.0), pos2(r.max.x - 2.0, r.max.y - 2.0), pos2(r.max.x - 6.0, r.max.y - 2.0)],
            if active { Color32::WHITE } else { t.text_dim },
            Stroke::NONE,
        ));
    }
    app.auto.add_response(&format!("tools.{cur:?}"), &resp, cur.label());
    // The drawn tooltip asks the catalog; the automation label above keeps the English name.
    let label = crate::i18n::tr(cur.label());
    let tip = match cur.shortcut() {
        Some(s) => format!("{} ({})", label, crate::menus::shortcut_text(s)),
        None => label.to_string(),
    };
    let resp = resp.on_hover_text(tip);
    let held = ui.data(|d| d.get_temp::<egui::Id>(held_id)) == Some(id);
    if resp.clicked() && !held {
        app.ui.tool = cur;
    }
    // Ctrl+double-click Pan Behind: Center Anchor Point in Layer Content.
    if cur == Tool::PanBehind
        && resp.double_clicked()
        && ui.input(|i| i.modifiers.command)
        && let Err(e) = crate::menus::invoke(app, ui.ctx(), "layer.centerAnchor", json!({}))
    {
        app.ui.status = e;
    } else if resp.double_clicked()
        && let Some((id, params)) = create_on_double_click(app, cur)
        && let Err(e) = crate::menus::invoke(app, ui.ctx(), id, params)
    {
        app.ui.status = e;
    }
    if slot.len() > 1 {
        slot_flyout(app, ui, si, slot, &resp, held_id);
    }
}

/// The fly-out menu of a multi-tool slot: right-click or press-and-hold opens it.
fn slot_flyout(app: &mut EffectcraftApp, ui: &mut egui::Ui, si: usize, slot: &[Tool], resp: &egui::Response, held_id: egui::Id) {
    let id = resp.id;
    let held = ui.data(|d| d.get_temp::<egui::Id>(held_id)) == Some(id);
    let held_for = resp.is_pointer_button_down_on().then(|| ui.input(|i| i.pointer.press_start_time().map(|s| i.time - s))).flatten();
    if let Some(seconds) = held_for
        && seconds < 0.35
    {
        ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(0.35 - seconds));
    }
    let long_press = held_for.is_some_and(|seconds| seconds >= 0.35) && !held;
    let command = if resp.secondary_clicked() || long_press {
        if long_press {
            ui.data_mut(|d| d.insert_temp(held_id, id));
        }
        Some(egui::SetOpenCommand::Bool(true))
    } else if resp.clicked() && !held {
        Some(egui::SetOpenCommand::Bool(false))
    } else {
        None
    };
    let close_behavior = if held && resp.clicked() { egui::PopupCloseBehavior::IgnoreClicks } else { egui::PopupCloseBehavior::CloseOnClickOutside };
    egui::Popup::menu(resp).style(crate::widgets::menu_style).open_memory(command).close_behavior(close_behavior).show(|ui| {
        for tool in slot.iter() {
            let row = ui.selectable_label(app.ui.tool == *tool, crate::i18n::tr(tool.label()));
            app.auto.add(&format!("tools.select.{tool:?}"), row.rect, tool.label());
            if row.clicked() {
                app.ui.tool = *tool;
                if let Some(saved) = app.ui.slot_tools.get_mut(si) {
                    *saved = *tool;
                }
                ui.close();
            }
        }
    });
}

/// The ids of the selected shape layers: the options show their paint and edit it.
fn selected_shape_layers(app: &EffectcraftApp) -> Vec<u64> {
    app.session
        .active_comp()
        .map(|c| {
            app.session.state.selected_layers.iter().filter(|id| c.layer(**id).is_some_and(|l| matches!(l.source, LayerSource::Shape))).map(|id| id.0).collect()
        })
        .unwrap_or_default()
}

/// Mirrors the first selected shape layer's paint into the shape tool (as in After Effects).
fn sync_layer_paint(app: &mut EffectcraftApp, shape_layers: &[u64]) {
    let layer_paint = shape_layers.first().and_then(|id| app.session.active_comp()?.layer(LayerId(*id))).map(|l| {
        let lt = l.layer_time(app.session.time());
        (effectcraft_engine::commands::shape_stroke::first_paint(l, lt), effectcraft_engine::commands::shape_stroke::first_paint_kinds(l, lt))
    });
    if let Some(((fill, stroke, width), (fill_kind, stroke_kind))) = layer_paint {
        let rgb = |c: [f64; 4]| [c[0] as f32, c[1] as f32, c[2] as f32];
        let tool = &mut app.session.state.shape_tool;
        tool.fill.kind = fill_kind;
        tool.stroke.kind = stroke_kind;
        if let Some(c) = fill {
            tool.fill.color = rgb(c);
        }
        if let Some(c) = stroke {
            tool.stroke.color = rgb(c);
        }
        tool.stroke_width = width.unwrap_or(0.0);
    }
}

/// Options for the active tool (shape creation, fill and stroke, puppet, roto); returns the x after them.
fn tool_options_bar(app: &mut EffectcraftApp, ui: &mut egui::Ui, p: &egui::Painter, mut x: f32, cy: f32) -> f32 {
    // Selected shape layers: the options show their paint and edit it (as in After Effects).
    let shape_layers = selected_shape_layers(app);
    sync_layer_paint(app, &shape_layers);
    // One undo step per picker session or width drag: end the merge once both are done.
    let picking = ["fill", "stroke"].iter().any(|k| widgets::popup_is_open(ui, picker_id(k)));
    if !picking && !ui.input(|i| i.pointer.any_down()) && app.session.history.merge_key.as_deref().is_some_and(|k| k.starts_with("tool-")) {
        app.session.history.merge_key = None;
    }
    let drawing = app.ui.tool.is_shape() || app.ui.tool == Tool::Pen;
    // Tool Creates Shape / Tool Creates Mask, with a shape layer selected (as in After Effects).
    if drawing && !shape_layers.is_empty() {
        x = creates_buttons(app, ui, x, cy);
    }
    // Fill and Stroke, hidden while the tools draw masks.
    let masking = drawing && !shape_layers.is_empty() && app.session.state.shape_tool.creates_mask;
    if (drawing || !shape_layers.is_empty()) && !masking {
        x = fill_stroke_options(app, ui, p, &shape_layers, x, cy);
    }
    if app.ui.tool.puppet_kind().is_some() {
        x = puppet_options(app, ui, p, x, cy);
    }
    if app.ui.tool.roto_kind(false).is_some() {
        x = roto_options(app, ui, p, x, cy);
    }
    x
}

/// The Snapping checkbox, its label and options menu; returns the x where it started.
fn snapping_toggle(app: &mut EffectcraftApp, ui: &mut egui::Ui, p: &egui::Painter, x: f32, cy: f32) -> f32 {
    let t = app.tokens;
    let snap = Rect::from_min_size(pos2(x, cy - 10.0), vec2(20.0, 20.0));
    // The engine owns snapping (View ▸ Snapping); the checkbox mirrors it.
    app.ui.snapping = app.session.state.snapping;
    if widgets::checkbox(ui, snap, app.ui.snapping, &t, egui::Id::unique("snapping")).clicked() {
        let _ = app.session.execute("view.snapping", serde_json::json!({}));
        app.ui.snapping = app.session.state.snapping;
    }
    app.auto.add("header.snapping", snap, "Snapping");
    let label = p.text(pos2(snap.max.x + 4.0, cy), Align2::LEFT_CENTER, tr("Snapping"), Tokens::ui(12.0), t.text_dim);
    snapping_options(app, ui, Rect::from_min_size(pos2(label.max.x + 2.0, cy - 9.0), vec2(18.0, 18.0)));
    x
}

/// Right side: community buttons, appearance and workspaces; `left` is where the tools end.
fn right_side(app: &mut EffectcraftApp, ui: &mut egui::Ui, p: &egui::Painter, rect: Rect, left: f32, cy: f32) {
    let t = app.tokens;
    let rx = community_buttons(app, ui, p, rect.max.x - 10.0, cy);
    let mut rx = appearance_button(app, ui, rx, cy);
    rx -= 10.0;
    p.line_segment([pos2(rx, cy - 10.0), pos2(rx, cy + 10.0)], Stroke::new(1.0, t.separator));
    rx -= 10.0;
    let rx = workspace_menu(app, ui, p, rx, cy);
    workspace_tabs(app, ui, p, rx, left, cy);
}

/// The Discord pill and the GitHub and website buttons, right to left from `rx`; returns the new `rx`.
fn community_buttons(app: &mut EffectcraftApp, ui: &mut egui::Ui, p: &egui::Painter, mut rx: f32, cy: f32) -> f32 {
    let t = app.tokens;
    // The pill fits its label (icon, gap, text) with even padding: it was a fixed 104 px with the
    // text at its left, which left a wide gap on the right (#581).
    let label = p.layout_no_wrap(tr("Discord").to_string(), Tokens::semibold(12.0), Color32::WHITE);
    let discord = Rect::from_min_max(pos2(rx - (10.0 + 14.0 + 6.0 + label.size().x + 12.0), cy - 13.0), pos2(rx, cy + 13.0));
    let dresp = ui.interact(discord, egui::Id::unique("hdr-discord"), Sense::click());
    let dc = Color32::from_rgb(0x58, 0x65, 0xf2);
    p.rect_filled(discord, 13.0, if dresp.hovered() { dc.gamma_multiply(1.2) } else { dc });
    icons::paint(p, Rect::from_center_size(pos2(discord.min.x + 17.0, cy), vec2(14.0, 14.0)), Icon::Chat, Color32::WHITE);
    p.galley(pos2(discord.min.x + 30.0, cy - label.size().y / 2.0), label, Color32::WHITE);
    app.auto.add("header.discord", discord, "Join the ArtCraft Discord");
    if dresp.on_hover_text(tr("Join the ArtCraft community on Discord")).clicked() {
        let _ = app.session.execute("help.discord", json!({}));
    }
    rx = discord.min.x - 6.0;
    for (id, icon, tip, cmd) in
        [("hdr-github", Icon::Code, "EffectCraft on GitHub", "help.github"), ("hdr-web", Icon::Globe, "EffectCraft on getartcraft.com", "help.appPage")]
    {
        let r = Rect::from_min_max(pos2(rx - 26.0, cy - 13.0), pos2(rx, cy + 13.0));
        if widgets::icon_button(ui, r, icon, false, &t, egui::Id::unique(id)).on_hover_text(tip).clicked() {
            let _ = app.session.execute(cmd, json!({}));
        }
        app.auto.add(&format!("header.{}", &id[4..]), r, tip);
        rx = r.min.x - 4.0;
    }
    rx
}

/// Appearance: Sync with System, Light, Dark (a monitor, sun or moon), one click to the next;
/// returns the new `rx`.
fn appearance_button(app: &mut EffectcraftApp, ui: &mut egui::Ui, rx: f32, cy: f32) -> f32 {
    let t = app.tokens;
    let (icon, mode) = match app.session.prefs.appearance.appearance_mode.as_str() {
        "auto" => (Icon::Monitor, tr("Sync with System")),
        "light" => (Icon::Sun, tr("Light")),
        _ => (Icon::Moon, tr("Dark")),
    };
    let tip = tr_args("Appearance Mode: {}", &[&mode]);
    let r = Rect::from_min_max(pos2(rx - 26.0, cy - 13.0), pos2(rx, cy + 13.0));
    let response = widgets::icon_button(ui, r, icon, false, &t, egui::Id::unique("hdr-appearance")).on_hover_text(&tip);
    if response.clicked() {
        app.cycle_appearance(ui.ctx());
    }
    response.context_menu(|ui| appearance_menu(app, ui));
    app.auto.add("header.appearance", r, &tip);
    r.min.x - 4.0
}

/// The appearance button's context menu: settings and the favorite themes.
fn appearance_menu(app: &mut EffectcraftApp, ui: &mut egui::Ui) {
    let settings = ui.button(tr("Appearance Settings…"));
    app.auto.add("header.appearanceSettings", settings.rect, "Appearance Settings…");
    if settings.clicked() {
        crate::panels::settings::open(app, "appearance");
        ui.close();
    }
    let favorites = app.session.prefs.appearance.favorite_themes.clone();
    for options in [effectcraft_engine::prefs::LIGHT_THEMES, effectcraft_engine::prefs::DARK_THEMES] {
        let group: Vec<_> = options.iter().filter(|(_, id)| favorites.iter().any(|saved| saved == id)).collect();
        if !group.is_empty() {
            ui.separator();
        }
        for (label, id) in group {
            let r = ui.selectable_label(app.ui.theme.name() == *id, tr(label));
            app.auto.add(&format!("header.favoriteTheme.{id}"), r.rect, label);
            if r.clicked() {
                if let Some(kind) = crate::theme::ThemeKind::from_name(id) {
                    app.set_theme(ui.ctx(), kind);
                }
                ui.close();
            }
        }
    }
}

/// The » button with the menu of all workspaces; returns the new `rx`.
fn workspace_menu(app: &mut EffectcraftApp, ui: &mut egui::Ui, p: &egui::Painter, rx: f32, cy: f32) -> f32 {
    let t = app.tokens;
    let more = Rect::from_min_max(pos2(rx - 20.0, cy - 11.0), pos2(rx, cy + 11.0));
    let mresp = ui.interact(more, egui::Id::unique("ws-more"), Sense::click());
    p.text(more.center(), Align2::CENTER_CENTER, "»", Tokens::ui(15.0), if mresp.hovered() { t.text } else { t.text_dim });
    app.auto.add("header.workspaces", more, "Workspaces");
    crate::widgets::context_menu(&mresp, |ui| ws_menu(app, ui));
    if mresp.clicked() {
        widgets::open_popup(ui, egui::Id::unique("ws-pop"));
    }
    let names: Vec<String> = crate::dock::WORKSPACES.iter().map(|s| crate::i18n::command_label("window.workspace", s).to_string()).collect();
    if let Some(i) = widgets::popup_menu(
        ui,
        egui::Id::unique("ws-pop"),
        more.left_bottom() - vec2(140.0, 0.0),
        &names,
        crate::dock::WORKSPACES.iter().position(|w| *w == app.ui.workspace),
    ) {
        app.set_workspace(crate::dock::WORKSPACES[i]);
    }
    more.min.x - 6.0
}

/// Workspace tabs, right to left from `rx`, stopping 120 px right of `left`.
/// After Effects 2026's order (right to left here): Default, Review, Learn, Small Screen, Standard.
fn workspace_tabs(app: &mut EffectcraftApp, ui: &mut egui::Ui, p: &egui::Painter, mut rx: f32, left: f32, cy: f32) {
    let t = app.tokens;
    let shown = ["Standard", "Small Screen", "Learn", "Review", "Default"];
    for name in shown {
        let g = p.layout_no_wrap(crate::i18n::command_label("window.workspace", name).to_string(), Tokens::ui(12.0), t.text);
        let w = g.size().x + 16.0;
        let r = Rect::from_min_max(pos2(rx - w, cy - 13.0), pos2(rx, cy + 13.0));
        if r.min.x < left + 120.0 {
            break;
        }
        let active = app.ui.workspace == name;
        let resp = ui.interact(r, egui::Id::unique(("ws", name)), Sense::click());
        let col = if active {
            t.hot_text
        } else if resp.hovered() {
            t.text
        } else {
            t.text_dim
        };
        p.galley_with_override_text_color(pos2(r.min.x + 8.0, cy - g.size().y / 2.0), g, col);
        if active {
            p.line_segment([pos2(r.min.x + 8.0, r.max.y - 3.0), pos2(r.max.x - 8.0, r.max.y - 3.0)], Stroke::new(2.0, t.hot_text));
        }
        app.auto.add(&format!("header.workspace.{name}"), r, name);
        if resp.clicked() {
            app.set_workspace(name);
        }
        rx = r.min.x - 2.0;
    }
}

/// Double-clicking a creation tool in the Tools bar makes an instance of what it draws, as in
/// After Effects (#1072): a text layer from the Type tools; from the shape tools, a shape that
/// fills the comp — into the selected shape layer's Contents, as a mask on the selected layer
/// (any non-shape layer, or Tool Creates Mask), or as a new shape layer when neither applies.
fn create_on_double_click(app: &EffectcraftApp, tool: Tool) -> Option<(&'static str, serde_json::Value)> {
    match tool {
        // `invoke` turns empty params into an insertion point in the comp centre that starts
        // editing (#866), which is what double-clicking the Type tool should do.
        Tool::Type | Tool::TypeVertical => Some(("layer.newText", json!({}))),
        Tool::Rectangle | Tool::RoundedRect | Tool::Ellipse | Tool::Polygon | Tool::Star => {
            let kind = match tool {
                Tool::Rectangle => "rect",
                Tool::RoundedRect => "rounded",
                Tool::Ellipse => "ellipse",
                Tool::Polygon => "polygon",
                _ => "star",
            };
            // Without a comp the command still runs and reports its error on the status line.
            let Some(comp) = app.session.active_comp() else {
                return Some(("shape.newShape", json!({"kind": kind})));
            };
            let (cw, ch) = (comp.width as f64, comp.height as f64);
            // Rectangles and ellipses fill the comp; a Polystar's Outer Radius is half the comp
            // height (Inner Radius: a quarter of it), so it gets a comp-height square.
            let (w, h) = if matches!(tool, Tool::Polygon | Tool::Star) { (ch, ch) } else { (cw, ch) };
            // A mask on the selected layer: any non-shape layer (the tool switches to Mask by
            // itself), or a shape layer while Tool Creates Mask is on — as dragging does.
            let sel = app.session.state.selected_layers.first().copied().and_then(|id| comp.layer(id));
            if let Some(l) = sel.filter(|l| l.source.is_av() && (app.session.state.shape_tool.creates_mask || !matches!(l.source, LayerSource::Shape))) {
                // The box in layer space, so a moved, scaled or rotated layer still gets a
                // comp-filling mask: all four corners are mapped, and their bounds are the
                // smallest axis-aligned box around the comp box.
                let ctx = effectcraft_engine::render::EvalCtx::new(&app.session.project, app.session.active_comp_id()?, comp, app.session.time());
                let inv = ctx.layer_to_comp(l).0.inverse().unwrap_or(effectcraft_engine::geom::Mat3::IDENTITY);
                let (x0, y0, x1, y1) = (cw / 2.0 - w / 2.0, ch / 2.0 - h / 2.0, cw / 2.0 + w / 2.0, ch / 2.0 + h / 2.0);
                let pts = [gv2(x0, y0), gv2(x1, y0), gv2(x1, y1), gv2(x0, y1)].map(|p| inv.apply(p));
                let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
                for p in pts {
                    lo = [lo[0].min(p.x), lo[1].min(p.y)];
                    hi = [hi[0].max(p.x), hi[1].max(p.y)];
                }
                let rect = [lo[0], lo[1], hi[0] - lo[0], hi[1] - lo[1]];
                return Some(("layer.addMask", json!({"layer": l.id.0, "shape": kind, "rect": rect})));
            }
            // Into the selected shape layer's Contents, else a new shape layer (the engine's own
            // target), painted with the Tools bar's Fill and Stroke.
            Some(("shape.newShape", json!({"kind": kind, "size": [w, h]})))
        }
        _ => None,
    }
}

/// The Snapping options menu beside the checkbox: After Effects' Snap Edges Extended and Snap to
/// Features in Collapsed Compositions and Text Layers (`view.snappingOptions`).
fn snapping_options(app: &mut EffectcraftApp, ui: &mut egui::Ui, r: Rect) {
    let t = app.tokens;
    let pop = egui::Id::unique("snap-options");
    if widgets::icon_button(ui, r, Icon::ChevronDown, false, &t, egui::Id::unique("snap-options-button")).on_hover_text(tr("Snapping options")).clicked() {
        widgets::open_popup(ui, pop);
    }
    app.auto.add("header.snappingOptions", r, "Snapping options");
    if !widgets::popup_is_open(ui, pop) {
        return;
    }
    let f = app.session.state.snap_features;
    let items: Vec<String> = vw::SnapFeatures::OPTIONS.iter().map(|(k, label)| widgets::check_label(f.option(k), label)).collect();
    if let Some((k, _)) = widgets::popup_menu(ui, pop, r.left_bottom(), &items, None).and_then(|i| vw::SnapFeatures::OPTIONS.get(i))
        && let Err(e) = app.session.execute("view.snappingOptions", json!({"toggle": k}))
    {
        app.ui.status = e.to_string();
    }
}

/// Set the shape tools' options (`shape.toolOptions`).
fn tool_options(app: &mut EffectcraftApp, v: serde_json::Value) {
    if let Err(e) = app.session.execute("shape.toolOptions", v) {
        app.ui.status = e.to_string();
    }
}

/// The Colors property of the first Gradient Fill (`gfill`) or Gradient Stroke (`gstroke`) on the
/// shape `layers`, in contents order through nested groups: (layer, property uid). The toolbar's
/// gradient swatch edits it in the Gradient Editor.
fn gradient_colors(app: &EffectcraftApp, layers: &[u64], paint: &str) -> Option<(u64, u64)> {
    fn find(g: &PropGroup, paint: &str, depth: usize) -> Option<u64> {
        if depth >= 16 {
            return None;
        }
        g.groups().filter(|c| c.enabled).find_map(|c| match c.match_id.as_str() {
            m if m == paint => c.get("colors").map(|p| p.uid),
            "group" | "contents" => find(c, paint, depth + 1),
            _ => None,
        })
    }
    let comp = app.session.active_comp()?;
    layers.iter().find_map(|&l| comp.layer(LayerId(l)).and_then(|layer| layer.props.sub("contents")).and_then(|c| find(c, paint, 0)).map(|u| (l, u)))
}

/// The colour picker of the Fill or Stroke swatch.
fn picker_id(key: &str) -> egui::Id {
    egui::Id::unique(("tool-pick", key))
}

/// Tool Creates Shape / Tool Creates Mask: with a shape layer selected, whether the shape tools
/// and the Pen draw shapes into it or masks on it. Returns the next x.
fn creates_buttons(app: &mut EffectcraftApp, ui: &mut egui::Ui, mut x: f32, cy: f32) -> f32 {
    let t = app.tokens;
    for (mask, icon, tip, key) in [(false, Icon::ShapeLayer, "Tool Creates Shape", "createsShape"), (true, Icon::MaskVis, "Tool Creates Mask", "createsMask")] {
        let r = Rect::from_min_size(pos2(x, cy - 13.0), vec2(26.0, 26.0));
        let on = app.session.state.shape_tool.creates_mask == mask;
        if widgets::icon_button(ui, r, icon, on, &t, egui::Id::unique(("tool-creates", key))).on_hover_text(tip).clicked() {
            tool_options(app, json!({"createsMask": mask}));
        }
        app.auto.add(&format!("header.{key}"), r, tip);
        x += 28.0;
    }
    x + 10.0
}

/// Fill and Stroke (the words open Fill Options / Stroke Options), their swatches and the
/// Stroke Width: for new shapes, and for the selected shape layers. Returns the next x.
fn fill_stroke_options(app: &mut EffectcraftApp, ui: &mut egui::Ui, p: &egui::Painter, shape_layers: &[u64], mut x: f32, cy: f32) -> f32 {
    let t = app.tokens;
    // The selected shape layers take the colours and the width too, one undo step per picker
    // session or drag.
    let paint = |app: &mut EffectcraftApp, key: &str, v: serde_json::Value| {
        if shape_layers.is_empty() {
            return;
        }
        let merge = format!("tool-{key}");
        if let Err(e) = app.session.execute("shape.fillStroke", json!({"layers": shape_layers, key: v, "merge": merge})) {
            app.ui.status = e.to_string();
        }
    };
    for (key, title) in [("fill", "Fill"), ("stroke", "Stroke")] {
        let tp = if key == "fill" { &app.session.state.shape_tool.fill } else { &app.session.state.shape_tool.stroke }.clone();
        let options = format!("{title} Options");
        let g = p.layout_no_wrap(format!("{title}:"), Tokens::ui(12.0), t.text_dim);
        let wr = Rect::from_min_size(pos2(x, cy - g.size().y / 2.0), g.size());
        let wresp = ui.interact(wr, egui::Id::unique(("tool-paint-word", key)), Sense::click()).on_hover_text(&options);
        let col = if wresp.hovered() { t.text } else { t.text_dim };
        if wresp.hovered() {
            p.line_segment([pos2(wr.min.x, wr.max.y), pos2(wr.max.x - 3.0, wr.max.y)], Stroke::new(1.0, col));
        }
        p.galley_with_override_text_color(wr.min, g, col);
        app.auto.add(&format!("header.{key}Options"), wr, &options);
        let opts = egui::Id::unique(("tool-paint-options", key));
        if wresp.clicked() {
            widgets::open_popup(ui, opts);
        }
        // The swatch picks a Solid Color's colour, edits a Linear or Radial Gradient's colours in
        // the Gradient Editor (the selected shape layer's Gradient Fill / Gradient Stroke), and
        // opens the options otherwise; Alt-click steps through the kinds (as in After Effects).
        let sr = Rect::from_min_size(pos2(wr.max.x + 6.0, cy - 8.0), vec2(20.0, 16.0));
        let sresp = ui.interact(sr, egui::Id::unique(("tool-paint-swatch", key)), Sense::click()).on_hover_text(format!("{title}: {}", tp.kind.label()));
        paint_swatch(p, sr, &tp, sresp.hovered(), &t);
        app.auto.add(&format!("header.{key}"), sr, &format!("{title} Color"));
        if sresp.clicked() {
            if ui.input(|i| i.modifiers.alt) {
                let at = PaintKind::ALL.iter().position(|k| *k == tp.kind).unwrap_or(0);
                let next = PaintKind::ALL.get((at + 1) % PaintKind::ALL.len()).copied().unwrap_or_default();
                paint_kind(app, shape_layers, key, next);
            } else if tp.kind == PaintKind::Solid {
                widgets::open_popup(ui, picker_id(key));
            } else if let Some((layer, prop)) = matches!(tp.kind, PaintKind::Linear | PaintKind::Radial)
                .then(|| gradient_colors(app, shape_layers, if key == "fill" { "gfill" } else { "gstroke" }))
                .flatten()
            {
                let ctx = ui.ctx().clone();
                if let Err(e) = crate::menus::invoke(app, &ctx, "prop.editGradient", json!({"layer": layer, "prop": prop})) {
                    app.ui.status = e;
                }
            } else {
                // No selected shape layer with that gradient: its options (new shapes' kind).
                widgets::open_popup(ui, opts);
            }
        }
        let mut c = tp.color;
        if widgets::color_popup(ui, picker_id(key), sr.left_bottom(), &mut c) {
            tool_options(app, json!({key: c}));
            paint(app, key, json!(c));
        }
        paint_options_popup(app, ui, shape_layers, key, &options, wr.left_bottom());
        x = sr.max.x + 12.0;
    }
    let width = app.session.state.shape_tool.stroke_width;
    let (r, v, _) = widgets::hot_number_at(
        ui,
        pos2(x - 6.0, cy - 9.0),
        egui::Id::unique("tool-stroke-w"),
        width,
        &widgets::HotSpec { speed: 0.2, range: (0.0, 1000.0), decimals: 0, suffix: " px" },
        &t,
    );
    if let Some(v) = v {
        tool_options(app, json!({"strokeWidth": v}));
        paint(app, "strokeWidth", json!(v));
    }
    app.auto.add("header.strokeWidth", r, "Stroke Width");
    r.max.x + 14.0
}

/// A Fill or Stroke kind: for new shapes, and (as in After Effects) for the selected shape
/// layers' fills or strokes, which None switches off (#732).
fn paint_kind(app: &mut EffectcraftApp, shape_layers: &[u64], key: &str, kind: PaintKind) {
    let k_type = format!("{key}Type");
    tool_options(app, json!({&k_type: kind.id()}));
    if shape_layers.is_empty() {
        return;
    }
    let mut v = json!({"layers": shape_layers, k_type: kind.id()});
    // A Solid Color takes the swatch's colour.
    let color = if key == "fill" { app.session.state.shape_tool.fill.color } else { app.session.state.shape_tool.stroke.color };
    if let (PaintKind::Solid, Some(o)) = (kind, v.as_object_mut()) {
        o.insert(key.to_string(), json!(color));
    }
    if let Err(e) = app.session.execute("shape.fillStroke", v) {
        app.ui.status = e.to_string();
    }
}

/// Fill Options / Stroke Options: None, Solid Color, Linear Gradient or Radial Gradient, and the
/// blend mode and opacity new shapes get (After Effects' dialogs, applied as they change).
fn paint_options_popup(app: &mut EffectcraftApp, ui: &mut egui::Ui, shape_layers: &[u64], key: &str, title: &str, pos: egui::Pos2) {
    let id = egui::Id::unique(("tool-paint-options", key));
    if !widgets::popup_is_open(ui, id) {
        return;
    }
    let t = app.tokens;
    let tp = if key == "fill" { &app.session.state.shape_tool.fill } else { &app.session.state.shape_tool.stroke }.clone();
    let blend_pop = id.with("blend");
    let mut set = None;
    let mut kind = None;
    let mut blend_rect = Rect::NOTHING;
    let area = egui::Area::new(id.with("area")).order(egui::Order::Foreground).fixed_pos(pos + vec2(0.0, 4.0)).show(ui.ctx(), |ui| {
        egui::Frame::popup(ui.style()).show(ui, |ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(236.0, 112.0), Sense::hover());
            let p = ui.painter().clone();
            let row = |y: f32| rect.min + vec2(0.0, y);
            p.text(row(8.0), Align2::LEFT_CENTER, title, Tokens::semibold(12.0), t.text);
            for (i, k) in PaintKind::ALL.into_iter().enumerate() {
                let r = Rect::from_min_size(row(22.0) + vec2(i as f32 * 32.0, 0.0), vec2(28.0, 22.0));
                let resp = ui.interact(r, id.with(k.id()), Sense::click()).on_hover_text(k.label());
                if k == tp.kind {
                    p.rect_stroke(r, 3.0, Stroke::new(1.5, t.accent), StrokeKind::Outside);
                }
                paint_swatch(&p, r.shrink(3.0), &ToolPaint { kind: k, ..tp.clone() }, resp.hovered(), &t);
                app.auto.add(&format!("header.{key}Options.{}", k.id()), r, k.label());
                if resp.clicked() {
                    kind = Some(k);
                }
            }
            p.text(row(33.0) + vec2(134.0, 0.0), Align2::LEFT_CENTER, tp.kind.label(), Tokens::ui(11.5), t.text_dim);
            p.text(row(66.0), Align2::LEFT_CENTER, tr("Blend Mode:"), Tokens::ui(12.0), t.text_dim);
            blend_rect = Rect::from_min_size(row(56.0) + vec2(84.0, 0.0), vec2(150.0, 20.0));
            if widgets::dropdown(ui, blend_rect, tp.blend.label(), &t, id.with("blend-dd")).clicked() {
                widgets::open_popup(ui, blend_pop);
            }
            app.auto.add(&format!("header.{key}Options.blend"), blend_rect, "Blend Mode");
            p.text(row(98.0), Align2::LEFT_CENTER, tr("Opacity:"), Tokens::ui(12.0), t.text_dim);
            let (or, v, _) = widgets::hot_number_at(
                ui,
                row(89.0) + vec2(84.0, 0.0),
                id.with("opacity"),
                tp.opacity,
                &widgets::HotSpec { speed: 0.5, range: (0.0, 100.0), decimals: 0, suffix: "%" },
                &t,
            );
            app.auto.add(&format!("header.{key}Options.opacity"), or, "Opacity");
            if let Some(v) = v {
                let k_opacity = format!("{key}Opacity");
                set = Some(json!({k_opacity: v}));
            }
        });
    });
    // The blend modes in the Layer ▸ Blending Mode menu's groups, over the options.
    let modes: Vec<String> =
        BlendMode::ALL.iter().flat_map(|m| std::iter::once(m.label().to_string()).chain(m.ends_group().then(|| "-".to_string()))).collect();
    let current = modes.iter().position(|m| m == tp.blend.label());
    if let Some(m) = widgets::popup_menu(ui, blend_pop, blend_rect.left_bottom(), &modes, current).and_then(|i| modes.get(i)) {
        let k_blend = format!("{key}Blend");
        set = Some(json!({k_blend: m}));
    }
    // Closes on a press outside or Escape, but stays open while its blend mode menu is.
    if !widgets::popup_is_open(ui, blend_pop) && (widgets::pressed_outside(ui.ctx(), &area.response) || ui.input(|i| i.key_pressed(egui::Key::Escape))) {
        ui.data_mut(|d| d.insert_temp(id.with("open"), false));
    }
    if let Some(v) = set {
        tool_options(app, v);
    }
    if let Some(k) = kind {
        paint_kind(app, shape_layers, key, k);
    }
}

/// A Fill or Stroke swatch: its colour, a gradient (white to black), or None's red slash.
fn paint_swatch(p: &egui::Painter, r: Rect, paint: &ToolPaint, hovered: bool, t: &Tokens) {
    let clip = p.with_clip_rect(r);
    match paint.kind {
        PaintKind::None => {
            clip.rect_filled(r, 2.0, Color32::WHITE);
            clip.line_segment([r.left_bottom(), r.right_top()], Stroke::new(1.5, Color32::from_rgb(0xe0, 0x30, 0x30)));
        }
        PaintKind::Solid => {
            clip.rect_filled(r, 2.0, widgets::srgb32(paint.color));
        }
        PaintKind::Linear => {
            let mut m = egui::Mesh::default();
            for (q, c) in
                [(r.left_top(), Color32::WHITE), (r.right_top(), Color32::BLACK), (r.right_bottom(), Color32::BLACK), (r.left_bottom(), Color32::WHITE)]
            {
                m.colored_vertex(q, c);
            }
            m.add_triangle(0, 1, 2);
            m.add_triangle(0, 2, 3);
            clip.add(m);
        }
        PaintKind::Radial => {
            let rad = r.size().length() / 2.0;
            for i in 0..8 {
                let f = 1.0 - i as f32 / 8.0;
                clip.circle_filled(r.center(), rad * f, Color32::from_gray((255.0 * (1.0 - f)) as u8));
            }
        }
    }
    p.rect_stroke(r, 2.0, Stroke::new(1.0, if hovered { t.text } else { t.field_border }), StrokeKind::Inside);
}

/// Roto Brush / Refine Edge tool options: the brush Diameter (⌘/Ctrl-drag in the Layer panel
/// sets it too, as in After Effects). Returns the next x.
fn roto_options(app: &mut EffectcraftApp, ui: &mut egui::Ui, p: &egui::Painter, mut x: f32, cy: f32) -> f32 {
    let t = app.tokens;
    let refine = app.ui.tool.roto_kind(false).is_some_and(|k| k.starts_with("refine"));
    let o = &app.session.state.roto;
    let (key, v) = if refine { ("refineDiameter", o.refine_diameter) } else { ("diameter", o.diameter) };
    let label = tr("Diameter:");
    let lr = p.text(pos2(x, cy), Align2::LEFT_CENTER, label, Tokens::ui(12.0), t.text_dim);
    x = lr.max.x + 6.0;
    let (r, nv, _) = widgets::hot_number_at(
        ui,
        pos2(x, cy - 9.0),
        egui::Id::unique(("roto-opt", key)),
        v,
        &widgets::HotSpec { speed: 0.5, range: (1.0, 5000.0), decimals: 0, suffix: " px" },
        &t,
    );
    app.auto.add("header.roto.diameter", r, label);
    if let Some(nv) = nv
        && let Err(e) = app.session.execute("roto.options", json!({key: nv}))
    {
        app.ui.status = e.to_string();
    }
    r.max.x + 12.0
}

/// Puppet tool options: Mesh: Show, Expansion, Density (for new meshes and the selected
/// layer's meshes). Returns the next x.
fn puppet_options(app: &mut EffectcraftApp, ui: &mut egui::Ui, p: &egui::Painter, mut x: f32, cy: f32) -> f32 {
    let t = app.tokens;
    let o = app.session.state.puppet.clone();
    p.text(pos2(x, cy), Align2::LEFT_CENTER, tr("Mesh:"), Tokens::ui(12.0), t.text_dim);
    x += 40.0;
    let show = Rect::from_min_size(pos2(x, cy - 10.0), vec2(20.0, 20.0));
    if widgets::checkbox(ui, show, o.show_mesh, &t, egui::Id::unique("puppet-show-mesh")).clicked() {
        let _ = app.session.execute("puppet.mesh", json!({"showMesh": !o.show_mesh}));
    }
    app.auto.add("header.puppet.showMesh", show, "Show mesh");
    p.text(pos2(show.max.x + 2.0, cy), Align2::LEFT_CENTER, tr("Show"), Tokens::ui(12.0), t.text_dim);
    x = show.max.x + 44.0;
    for (label, key, v, range) in [("Expansion:", "expansion", o.expansion, (-100.0, 200.0)), ("Density:", "density", o.density, (0.0, 100.0))] {
        p.text(pos2(x, cy), Align2::LEFT_CENTER, label, Tokens::ui(12.0), t.text_dim);
        x += if key == "expansion" { 64.0 } else { 52.0 };
        let (r, nv, _) = widgets::hot_number_at(
            ui,
            pos2(x, cy - 9.0),
            egui::Id::unique(("puppet-opt", key)),
            v,
            &widgets::HotSpec { speed: 0.2, range, decimals: 0, suffix: "" },
            &t,
        );
        app.auto.add(&format!("header.puppet.{key}"), r, label);
        if let Some(nv) = nv {
            let _ = app.session.execute("puppet.mesh", json!({key: nv, "merge": format!("puppet-opt-{key}")}));
        }
        x = r.max.x + 12.0;
    }
    // Record Options… (⌘/Ctrl-drag a pin records its motion in real time).
    let label = "Record Options...";
    let w = 104.0;
    let r = Rect::from_min_size(pos2(x, cy - 10.0), vec2(w, 20.0));
    let resp = ui.interact(r, egui::Id::unique("puppet-record-options"), egui::Sense::click()).on_hover_text(tr("⌘/Ctrl-drag a pin to record its motion"));
    p.rect_stroke(r, 3.0, egui::Stroke::new(1.0, if resp.hovered() { t.accent } else { t.field_border }), egui::StrokeKind::Inside);
    p.text(r.center(), Align2::CENTER_CENTER, label, Tokens::ui(11.5), t.text);
    app.auto.add("header.puppet.recordOptions", r, label);
    if resp.clicked() {
        let ctx = ui.ctx().clone();
        if let Err(e) = crate::menus::invoke(app, &ctx, "puppet.recordOptions", json!({})) {
            app.ui.status = e;
        }
    }
    // Follow-Through… (select the leader pin, then Shift-click the pins that trail it).
    let label = "Follow-Through...";
    let r = Rect::from_min_size(pos2(r.max.x + 8.0, cy - 10.0), vec2(w, 20.0));
    let resp = ui
        .interact(r, egui::Id::unique("puppet-follow"), egui::Sense::click())
        .on_hover_text(tr("Select the leader pin, then Shift-click the pins that should trail it (hair, cloth, tails)"));
    p.rect_stroke(r, 3.0, egui::Stroke::new(1.0, if resp.hovered() { t.accent } else { t.field_border }), egui::StrokeKind::Inside);
    p.text(r.center(), Align2::CENTER_CENTER, label, Tokens::ui(11.5), t.text);
    app.auto.add("header.puppet.follow", r, label);
    if resp.clicked() {
        let ctx = ui.ctx().clone();
        if let Err(e) = crate::menus::invoke(app, &ctx, "puppet.follow", json!({})) {
            app.ui.status = e;
        }
    }
    r.max.x + 16.0
}

/// A workspace's drawn name: built-ins read the menu bar's Window ▸ Workspace row; names the user
/// saved stay verbatim, even when one matches a catalog source such as "Composition".
fn workspace_label(name: &str) -> &str {
    if crate::dock::WORKSPACES.contains(&name) { crate::i18n::command_label("window.workspace", name) } else { name }
}

fn ws_menu(app: &mut EffectcraftApp, ui: &mut egui::Ui) {
    for w in app.workspace_names() {
        if ui.selectable_label(app.ui.workspace == w, workspace_label(&w)).clicked() {
            app.set_workspace(&w);
            ui.close();
        }
    }
    ui.separator();
    if ui.button(tr("Reset to Saved Layout")).clicked() {
        let n = app.ui.workspace.clone();
        app.set_workspace(&n);
        ui.close();
    }
}

/// The ArtCraft mark as supplied in `docs/brand` (the README's and getartcraft.com's logo; used
/// unmodified, see `docs/brand/LICENSE-brand.txt`).
static ARTCRAFT_MARK: &[u8] = include_bytes!("../../../docs/brand/artcraft-mark.png");

/// The ArtCraft mark in `r` (decoded once into a mipmapped texture, so it stays crisp from the
/// 24 px Tools bar to the About dialog). Nothing is drawn if it can't be decoded.
pub fn paint_logo(p: &egui::Painter, r: Rect) {
    let ctx = p.ctx();
    let id = egui::Id::unique("artcraft-mark");
    let tex = ctx.data(|d| d.get_temp::<Option<egui::TextureHandle>>(id)).unwrap_or_else(|| {
        let tex = image::load_from_memory_with_format(ARTCRAFT_MARK, image::ImageFormat::Png).ok().map(|img| {
            let img = img.to_rgba8();
            let ci = egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw());
            ctx.load_texture("artcraft-mark", ci, egui::TextureOptions::LINEAR.with_mipmap_mode(Some(egui::TextureFilter::Linear)))
        });
        ctx.data_mut(|d| d.insert_temp(id, tex.clone()));
        tex
    });
    if let Some(t) = tex {
        p.image(t.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(app: &mut EffectcraftApp, ctx: &egui::Context, time: f64, events: Vec<egui::Event>) {
        app.auto.begin_frame();
        let mut out = ctx.run_ui(
            egui::RawInput { time: Some(time), events, screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(1400.0, 500.0))), ..Default::default() },
            |ui| {
                show(app, ui, Rect::from_min_size(egui::Pos2::ZERO, vec2(1400.0, 40.0)));
            },
        );
        out.textures_delta.clear();
    }

    #[test]
    fn saved_workspace_names_stay_verbatim() {
        crate::i18n::set_current("zh-hans");
        assert_ne!(workspace_label("Default"), "Default");
        assert_ne!(crate::i18n::tr("Composition"), "Composition");
        assert_eq!(workspace_label("Composition"), "Composition");
        crate::i18n::set_current("en");
    }

    #[test]
    fn long_press_type_keeps_the_tool_until_vertical_is_selected() {
        let mut app = EffectcraftApp::new(effectcraft_engine::Session::default());
        let ctx = egui::Context::default();
        crate::theme::install(&ctx, &app.tokens, crate::i18n::language(&app));
        frame(&mut app, &ctx, 0.0, vec![]);
        frame(&mut app, &ctx, 0.1, vec![]);
        let center = |element: &crate::automation::Element| {
            let [x, y, w, h] = element.rect;
            egui::pos2(x + w / 2.0, y + h / 2.0)
        };
        let at = center(app.auto.elements.iter().find(|w| w.id == "tools.Type").unwrap());
        let pointer = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        frame(&mut app, &ctx, 1.0, vec![egui::Event::PointerMoved(at), pointer(at, true)]);
        frame(&mut app, &ctx, 1.36, vec![]);
        frame(&mut app, &ctx, 1.4, vec![pointer(at, false)]);
        frame(&mut app, &ctx, 1.45, vec![]);
        assert_eq!(app.ui.tool, Tool::Selection);
        let row = center(app.auto.elements.iter().find(|w| w.id == "tools.select.TypeVertical").unwrap());
        frame(&mut app, &ctx, 2.0, vec![egui::Event::PointerMoved(row), pointer(row, true)]);
        frame(&mut app, &ctx, 2.05, vec![pointer(row, false)]);
        assert_eq!(app.ui.tool, Tool::TypeVertical);
        assert_eq!(app.ui.slot_tools[10], Tool::TypeVertical);
        frame(&mut app, &ctx, 2.1, vec![]);
        assert!(!app.auto.elements.iter().any(|w| w.id == "tools.select.TypeVertical"));
    }

    /// Double-clicking a creation tool in the Tools bar makes an instance of what it draws
    /// (#1072): with nothing selected a shape layer whose shape fills the comp, into the selected
    /// shape layer's Contents, a mask on a non-shape layer, a text layer from the Type tools.
    /// Tools that create nothing stay quiet.
    #[test]
    fn double_clicking_creation_tools_makes_instances() {
        let mut s = effectcraft_engine::Session::default();
        s.execute("comp.new", json!({"width": 640, "height": 360, "duration": 2})).unwrap();
        let mut app = EffectcraftApp::new(s);
        let ctx = egui::Context::default();
        crate::theme::install(&ctx, &app.tokens, crate::i18n::language(&app));
        frame(&mut app, &ctx, 0.0, vec![]);
        frame(&mut app, &ctx, 0.1, vec![]);
        let pointer = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        // Both click pairs in one frame, as egui counts a double-click.
        let double_click = |app: &mut EffectcraftApp, ctx: &egui::Context, t: f64, id: &str| {
            let e = app.auto.elements.iter().find(|w| w.id == id).unwrap_or_else(|| panic!("no {id}"));
            let at = egui::pos2(e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0);
            frame(app, ctx, t, vec![egui::Event::PointerMoved(at), pointer(at, true), pointer(at, false), pointer(at, true), pointer(at, false)]);
        };
        // Nothing selected: a shape layer with a Rectangle Path 1 the size of the comp.
        double_click(&mut app, &ctx, 1.0, "tools.Rectangle");
        {
            let comp = app.session.active_comp().unwrap();
            assert_eq!(comp.layers.len(), 1, "the double-click made a layer");
            let l = &comp.layers[0];
            assert!(matches!(l.source, LayerSource::Shape), "a shape layer");
            let size = l.props.prop("contents/Rectangle 1/contents/Rectangle Path 1/size").expect("Rectangle Path 1");
            assert_eq!(size.value.as_vec2(), [640.0, 360.0], "the rectangle fills the comp");
        }
        // The shape layer is selected: the Ellipse goes on top of its Contents, no new layer.
        app.ui.slot_tools[8] = Tool::Ellipse;
        frame(&mut app, &ctx, 1.05, vec![]);
        double_click(&mut app, &ctx, 2.0, "tools.Ellipse");
        {
            let comp = app.session.active_comp().unwrap();
            assert_eq!(comp.layers.len(), 1, "into the selected shape layer's Contents");
            let l = &comp.layers[0];
            assert!(l.props.prop("contents/Ellipse 1/contents/Ellipse Path 1/size").is_some(), "the new ellipse is on top");
            assert!(l.props.prop("contents/Rectangle 1/contents/Rectangle Path 1/size").is_some(), "the rectangle still stands");
        }
        // The Type tool: a text layer above the selection, editing started (an insertion point, #866).
        double_click(&mut app, &ctx, 3.0, "tools.Type");
        {
            let comp = app.session.active_comp().unwrap();
            assert_eq!(comp.layers.len(), 2);
            assert!(matches!(comp.layers[0].source, LayerSource::Text));
            assert!(app.session.state.text_edit.is_some(), "the text layer is being edited");
        }
        // A non-shape layer is selected, so the Rectangle becomes a comp-filling mask on it.
        app.ui.slot_tools[8] = Tool::Rectangle;
        frame(&mut app, &ctx, 3.05, vec![]);
        double_click(&mut app, &ctx, 4.0, "tools.Rectangle");
        {
            let comp = app.session.active_comp().unwrap();
            assert_eq!(comp.layers.len(), 2, "a mask, not a layer");
            let p = comp.layers[0].props.prop("masks/Mask 1/path").expect("Mask 1");
            let sh = p.value.as_path().expect("a mask path");
            let b = effectcraft_path::bounds(&[effectcraft_path::to_kurbo(sh)]).expect("bounds");
            // The text layer is anchored top-left and centred in the comp: layer = comp - centre.
            for (got, want) in [(b.x0, -320.0), (b.y0, -180.0), (b.x1, 320.0), (b.y1, 180.0)] {
                assert!((got - want).abs() < 1.0, "the mask fills the comp: {got} vs {want}");
            }
        }
        // A tool that creates nothing: no layer, no error.
        double_click(&mut app, &ctx, 5.0, "tools.Hand");
        assert_eq!(app.session.active_comp().unwrap().layers.len(), 2, "the Hand tool creates nothing");
        assert!(app.ui.status.is_empty(), "{}", app.ui.status);
        // Without a comp the command fails with the status line, never a panic.
        let mut bare = EffectcraftApp::new(effectcraft_engine::Session::default());
        let ctx = egui::Context::default();
        crate::theme::install(&ctx, &bare.tokens, crate::i18n::language(&bare));
        frame(&mut bare, &ctx, 0.0, vec![]);
        frame(&mut bare, &ctx, 0.1, vec![]);
        double_click(&mut bare, &ctx, 1.0, "tools.Rectangle");
        assert!(!bare.ui.status.is_empty(), "no comp: a status message, not a crash");
    }

    /// #1765: a double-click on the Ellipse tool with nothing selected makes its own shape layer
    /// the size of the comp, as the Rectangle does.
    #[test]
    fn double_click_ellipse_with_no_selection_makes_a_layer() {
        let mut s = effectcraft_engine::Session::default();
        s.execute("comp.new", json!({"width": 640, "height": 360, "duration": 2})).unwrap();
        let mut app = EffectcraftApp::new(s);
        let ctx = egui::Context::default();
        crate::theme::install(&ctx, &app.tokens, crate::i18n::language(&app));
        frame(&mut app, &ctx, 0.0, vec![]);
        frame(&mut app, &ctx, 0.1, vec![]);
        app.ui.slot_tools[8] = Tool::Ellipse;
        frame(&mut app, &ctx, 1.0, vec![]);
        let pointer = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        let e = app.auto.elements.iter().find(|w| w.id == "tools.Ellipse").unwrap_or_else(|| panic!("no tools.Ellipse"));
        let at = egui::pos2(e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0);
        frame(&mut app, &ctx, 1.1, vec![egui::Event::PointerMoved(at), pointer(at, true), pointer(at, false), pointer(at, true), pointer(at, false)]);
        let comp = app.session.active_comp().unwrap();
        assert_eq!(comp.layers.len(), 1, "the double-click made a layer");
        let l = &comp.layers[0];
        assert!(matches!(l.source, LayerSource::Shape), "a shape layer");
        let size = l.props.prop("contents/Ellipse 1/contents/Ellipse Path 1/size").expect("Ellipse Path 1");
        assert_eq!(size.value.as_vec2(), [640.0, 360.0], "the ellipse fills the comp");
        assert!(app.ui.status.is_empty(), "{}", app.ui.status);
    }

    /// Tool Creates Mask on (#1072): a double-click on a shape tool masks the selected shape
    /// layer instead of adding to its Contents.
    #[test]
    fn double_click_masks_a_shape_layer_while_tool_creates_mask() {
        let mut s = effectcraft_engine::Session::default();
        s.execute("comp.new", json!({"width": 640, "height": 360, "duration": 2})).unwrap();
        let mut app = EffectcraftApp::new(s);
        let ctx = egui::Context::default();
        crate::theme::install(&ctx, &app.tokens, crate::i18n::language(&app));
        frame(&mut app, &ctx, 0.0, vec![]);
        frame(&mut app, &ctx, 0.1, vec![]);
        let pointer = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        let double_click = |app: &mut EffectcraftApp, ctx: &egui::Context, t: f64, id: &str| {
            let e = app.auto.elements.iter().find(|w| w.id == id).unwrap_or_else(|| panic!("no {id}"));
            let at = egui::pos2(e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0);
            frame(app, ctx, t, vec![egui::Event::PointerMoved(at), pointer(at, true), pointer(at, false), pointer(at, true), pointer(at, false)]);
        };
        // A shape layer, selected by the double-click that made it.
        double_click(&mut app, &ctx, 1.0, "tools.Rectangle");
        assert_eq!(app.session.active_comp().unwrap().layers.len(), 1);
        app.session.state.shape_tool.creates_mask = true;
        app.ui.slot_tools[8] = Tool::Ellipse;
        frame(&mut app, &ctx, 1.05, vec![]);
        double_click(&mut app, &ctx, 2.0, "tools.Ellipse");
        {
            let comp = app.session.active_comp().unwrap();
            assert_eq!(comp.layers.len(), 1, "a mask, not a second shape layer");
            let l = &comp.layers[0];
            assert!(l.props.prop("masks/Mask 1/path").is_some(), "an ellipse mask");
            assert!(l.props.prop("contents/Ellipse 1").is_none(), "nothing added to the Contents");
            let p = l.props.prop("contents/Rectangle 1/contents/Rectangle Path 1/size").expect("the rectangle stands");
            assert_eq!(p.value.as_vec2(), [640.0, 360.0]);
        }
        assert!(app.ui.status.is_empty(), "{}", app.ui.status);
    }

    /// The shape tools' geometry defaults (#1072): the Rounded Rectangle tool's Roundness is a
    /// fixed 20, the Polygon tool a 5-point polygon, the Star tool a 5-point star whose Inner
    /// Radius is a quarter of its Outer Radius.
    #[test]
    fn double_click_shape_tool_geometry_defaults() {
        let mut s = effectcraft_engine::Session::default();
        s.execute("comp.new", json!({"width": 640, "height": 360, "duration": 2})).unwrap();
        let mut app = EffectcraftApp::new(s);
        let ctx = egui::Context::default();
        crate::theme::install(&ctx, &app.tokens, crate::i18n::language(&app));
        frame(&mut app, &ctx, 0.0, vec![]);
        frame(&mut app, &ctx, 0.1, vec![]);
        let pointer = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        let double_click = |app: &mut EffectcraftApp, ctx: &egui::Context, t: f64, id: &str| {
            let e = app.auto.elements.iter().find(|w| w.id == id).unwrap_or_else(|| panic!("no {id}"));
            let at = egui::pos2(e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0);
            frame(app, ctx, t, vec![egui::Event::PointerMoved(at), pointer(at, true), pointer(at, false), pointer(at, true), pointer(at, false)]);
        };
        // Each shape on its own layer: deselect so the next double-click makes a new one.
        let deselected = |app: &mut EffectcraftApp, ctx: &egui::Context, t: f64| {
            app.session.execute("edit.deselectAll", json!({})).unwrap();
            frame(app, ctx, t, vec![]);
        };
        // The Rounded Rectangle: Roundness 20, filling the comp.
        app.ui.slot_tools[8] = Tool::RoundedRect;
        frame(&mut app, &ctx, 1.0, vec![]);
        double_click(&mut app, &ctx, 1.1, "tools.RoundedRect");
        {
            let comp = app.session.active_comp().unwrap();
            let g = comp.layers[0].props.group("contents/Rectangle 1/contents/Rectangle Path 1").expect("Rectangle Path 1");
            assert_eq!(g.prop("roundness").expect("roundness").value.as_f64(), 20.0, "a fixed roundness");
            assert_eq!(g.prop("size").expect("size").value.as_vec2(), [640.0, 360.0]);
        }
        // The Star: 5 points, Outer Radius half the comp height, Inner Radius a quarter.
        deselected(&mut app, &ctx, 2.0);
        app.ui.slot_tools[8] = Tool::Star;
        frame(&mut app, &ctx, 2.05, vec![]);
        double_click(&mut app, &ctx, 2.1, "tools.Star");
        {
            let comp = app.session.active_comp().unwrap();
            let g = comp.layers[0].props.group("contents/Polystar 1/contents/Polystar Path 1").expect("Polystar Path 1");
            assert_eq!(g.prop("type").expect("type").value.as_enum(), 0, "Star");
            assert_eq!(g.prop("points").expect("points").value.as_f64(), 5.0, "5 points");
            assert_eq!(g.prop("outerRadius").expect("outer").value.as_f64(), 180.0, "half the comp height");
            assert_eq!(g.prop("innerRadius").expect("inner").value.as_f64(), 90.0, "a quarter of the comp height");
        }
        // The Polygon: 5 points, Outer Radius half the comp height, no inner radius.
        deselected(&mut app, &ctx, 3.0);
        app.ui.slot_tools[8] = Tool::Polygon;
        frame(&mut app, &ctx, 3.05, vec![]);
        double_click(&mut app, &ctx, 3.1, "tools.Polygon");
        {
            let comp = app.session.active_comp().unwrap();
            let g = comp.layers[0].props.group("contents/Polystar 1/contents/Polystar Path 1").expect("Polystar Path 1");
            assert_eq!(g.prop("type").expect("type").value.as_enum(), 1, "Polygon");
            assert_eq!(g.prop("points").expect("points").value.as_f64(), 5.0, "5 points");
            assert_eq!(g.prop("outerRadius").expect("outer").value.as_f64(), 180.0, "half the comp height");
            assert!(g.prop("innerRadius").is_none(), "a polygon has no inner radius");
        }
        assert!(app.ui.status.is_empty(), "{}", app.ui.status);
    }

    /// A double-clicked shape is painted with the Tools bar's Fill and Stroke (#1072): the
    /// colours and stroke width the shape tool is set to — and no outline at all when the tool
    /// has its Stroke off.
    #[test]
    fn double_click_paints_with_the_shape_tools_fill_and_stroke() {
        let mut s = effectcraft_engine::Session::default();
        s.execute("comp.new", json!({"width": 640, "height": 360, "duration": 2})).unwrap();
        let mut app = EffectcraftApp::new(s);
        let ctx = egui::Context::default();
        crate::theme::install(&ctx, &app.tokens, crate::i18n::language(&app));
        frame(&mut app, &ctx, 0.0, vec![]);
        frame(&mut app, &ctx, 0.1, vec![]);
        let pointer = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        let double_click = |app: &mut EffectcraftApp, ctx: &egui::Context, t: f64, id: &str| {
            let e = app.auto.elements.iter().find(|w| w.id == id).unwrap_or_else(|| panic!("no {id}"));
            let at = egui::pos2(e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0);
            frame(app, ctx, t, vec![egui::Event::PointerMoved(at), pointer(at, true), pointer(at, false), pointer(at, true), pointer(at, false)]);
        };
        // The tool's own Fill and Stroke go onto the created shape.
        app.session.state.shape_tool.fill.color = [1.0, 0.0, 0.0];
        app.session.state.shape_tool.stroke.color = [0.0, 1.0, 0.0];
        app.session.state.shape_tool.stroke_width = 5.0;
        double_click(&mut app, &ctx, 1.0, "tools.Rectangle");
        {
            let comp = app.session.active_comp().unwrap();
            let g = comp.layers[0].props.group("contents/Rectangle 1/contents").expect("the contents");
            let fill = g.sub("fill").expect("the fill");
            assert_eq!(fill.get("color").expect("color").value.as_color(), [1.0, 0.0, 0.0, 1.0], "the tool's Fill");
            let stroke = g.sub("stroke").expect("the stroke");
            assert_eq!(stroke.get("color").expect("color").value.as_color(), [0.0, 1.0, 0.0, 1.0], "the tool's Stroke");
            assert_eq!(stroke.get("width").expect("width").value.as_f64(), 5.0, "the tool's Stroke Width");
        }
        // Stroke off ("Keine"): the next shape gets no outline at all, but keeps the Fill.
        app.session.execute("shape.toolOptions", json!({"stroke": false})).unwrap();
        app.session.execute("edit.deselectAll", json!({})).unwrap();
        app.ui.slot_tools[8] = Tool::Ellipse;
        frame(&mut app, &ctx, 1.05, vec![]);
        double_click(&mut app, &ctx, 2.0, "tools.Ellipse");
        {
            let comp = app.session.active_comp().unwrap();
            assert_eq!(comp.layers.len(), 2, "a new shape layer");
            let g = comp.layers[0].props.group("contents/Ellipse 1/contents").expect("the ellipse");
            assert!(g.sub("stroke").is_none(), "no outline when the tool's Stroke is off");
            assert_eq!(g.sub("fill").expect("the fill").get("color").expect("color").value.as_color(), [1.0, 0.0, 0.0, 1.0]);
        }
        assert!(app.ui.status.is_empty(), "{}", app.ui.status);
    }
}
