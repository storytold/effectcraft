//! The Tools bar: home, the tool slots, tool options, snapping, workspaces and the community
//! buttons (Discord is always one click away).

use egui::{Align2, Color32, Rect, Sense, Stroke, pos2, vec2};
use serde_json::json;

use crate::icons::Icon;
use crate::state::Tool;
use crate::theme::Tokens;
use crate::widgets;
use crate::{Dialog, EffectcraftApp};

pub fn show(app: &mut EffectcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let p = ui.painter().clone();
    p.rect_filled(rect, 0.0, t.header_bg);
    p.line_segment([rect.left_bottom(), rect.right_bottom()], Stroke::new(1.0, t.app_bg));
    let cy = rect.center().y;
    let mut x = rect.min.x + if app.integrated_titlebar && !app.ui.show_menu_bar { 80.0 } else { 6.0 };

    // Brand mark.
    let brand = Rect::from_min_size(pos2(x, cy - 9.0), vec2(18.0, 18.0));
    paint_logo(&p, brand);
    let bresp = ui.interact(brand, egui::Id::new("brand"), Sense::click());
    app.auto.add("header.about", brand, "About EffectCraft");
    if bresp.on_hover_text("About EffectCraft").clicked() {
        app.dialog = Some(Dialog::About);
    }
    x += 24.0;

    // Home.
    let home = Rect::from_min_size(pos2(x, cy - 12.0), vec2(24.0, 24.0));
    if widgets::icon_button(ui, home, Icon::Home, app.ui.start_screen, &t, egui::Id::new("tool-home")).on_hover_text("Home").clicked() {
        app.ui.start_screen = !app.ui.start_screen;
    }
    app.auto.add("header.home", home, "Home");
    x += 28.0;
    p.line_segment([pos2(x, cy - 10.0), pos2(x, cy + 10.0)], Stroke::new(1.0, t.separator));
    x += 6.0;

    // Tool slots (with group separators after camera tools and pan-behind).
    for (si, slot) in Tool::SLOTS.iter().enumerate() {
        let cur = app.ui.slot_tools.get(si).copied().unwrap_or(slot[0]);
        let r = Rect::from_min_size(pos2(x, cy - 12.0), vec2(24.0, 24.0));
        let active = slot.contains(&app.ui.tool);
        let id = egui::Id::new(("tool-slot", si));
        let resp = widgets::icon_button(ui, r, cur.icon(), active, &t, id);
        if slot.len() > 1 {
            // Flyout corner triangle.
            p.add(egui::Shape::convex_polygon(
                vec![pos2(r.max.x - 2.0, r.max.y - 6.0), pos2(r.max.x - 2.0, r.max.y - 2.0), pos2(r.max.x - 6.0, r.max.y - 2.0)],
                if active { Color32::WHITE } else { t.text_dim },
                Stroke::NONE,
            ));
        }
        app.auto.add(&format!("tools.{cur:?}"), r, cur.label());
        let tip = match cur.shortcut() {
            Some(s) => format!("{} ({})", cur.label(), crate::menus::shortcut_text(s)),
            None => cur.label().to_string(),
        };
        let resp = resp.on_hover_text(tip);
        let (choice, suppress_click) = if slot.len() > 1 { tool_flyout(&resp, slot, app.ui.tool, &mut app.auto) } else { (None, false) };
        if resp.clicked() && !suppress_click {
            app.ui.tool = cur;
        }
        // Ctrl+double-click Pan Behind: Center Anchor Point in Layer Content.
        if cur == Tool::PanBehind
            && resp.double_clicked()
            && ui.input(|i| i.modifiers.command)
            && let Err(e) = crate::menus::invoke(app, ui.ctx(), "layer.centerAnchor", json!({}))
        {
            app.ui.status = e;
        }
        if let Some(tool) = choice {
            app.ui.tool = tool;
            if let Some(slot_tool) = app.ui.slot_tools.get_mut(si) {
                *slot_tool = tool;
            }
        }
        x += 26.0;
        if matches!(si, 2 | 5 | 7 | 10 | 13) {
            x += 4.0;
            p.line_segment([pos2(x, cy - 10.0), pos2(x, cy + 10.0)], Stroke::new(1.0, t.separator));
            x += 6.0;
        }
    }

    // Tool options.
    x += 10.0;
    if app.ui.tool.is_shape() || app.ui.tool == Tool::Pen {
        p.text(pos2(x, cy), Align2::LEFT_CENTER, "Fill:", Tokens::ui(12.0), t.text_dim);
        x += 28.0;
        let fr = Rect::from_center_size(pos2(x + 10.0, cy), vec2(20.0, 16.0));
        let c = app.ui.fill_color;
        if widgets::swatch(ui, fr, [c[0], c[1], c[2], 1.0], egui::Id::new("tool-fill"), &t).clicked() {
            widgets::open_popup(ui, egui::Id::new("tool-fill-pop"));
        }
        color_popup(ui, egui::Id::new("tool-fill-pop"), fr.left_bottom(), &mut app.ui.fill_color);
        x += 32.0;
        p.text(pos2(x, cy), Align2::LEFT_CENTER, "Stroke:", Tokens::ui(12.0), t.text_dim);
        x += 44.0;
        let sr = Rect::from_center_size(pos2(x + 10.0, cy), vec2(20.0, 16.0));
        let c = app.ui.stroke_color;
        if widgets::swatch(ui, sr, [c[0], c[1], c[2], 1.0], egui::Id::new("tool-stroke"), &t).clicked() {
            widgets::open_popup(ui, egui::Id::new("tool-stroke-pop"));
        }
        color_popup(ui, egui::Id::new("tool-stroke-pop"), sr.left_bottom(), &mut app.ui.stroke_color);
        x += 26.0;
        let (r, v, _) =
            widgets::hot_number_at(ui, pos2(x, cy - 9.0), egui::Id::new("tool-stroke-w"), app.ui.stroke_width as f64, 0.2, (0.0, 1000.0), 0, " px", &t);
        if let Some(v) = v {
            app.ui.stroke_width = v as f32;
        }
        x = r.max.x + 14.0;
    }
    if app.ui.tool.puppet_kind().is_some() {
        x = puppet_options(app, ui, &p, x, cy);
    }
    let snap = Rect::from_min_size(pos2(x, cy - 10.0), vec2(20.0, 20.0));
    // The engine owns snapping (View ▸ Snapping); the checkbox mirrors it.
    app.ui.snapping = app.session.state.snapping;
    if widgets::checkbox(ui, snap, app.ui.snapping, &t, egui::Id::new("snapping")).clicked() {
        let _ = app.session.execute("view.snapping", serde_json::json!({}));
        app.ui.snapping = app.session.state.snapping;
    }
    app.auto.add("header.snapping", snap, "Snapping");
    p.text(pos2(snap.max.x + 4.0, cy), Align2::LEFT_CENTER, "Snapping", Tokens::ui(12.0), t.text_dim);

    // Right side: community buttons, workspaces.
    let mut rx = rect.max.x - 6.0;
    let discord = Rect::from_min_max(pos2(rx - 24.0, cy - 12.0), pos2(rx, cy + 12.0));
    let dresp = widgets::icon_button(ui, discord, Icon::Chat, false, &t, egui::Id::new("hdr-discord"));
    app.auto.add("header.discord", discord, "Join the ArtCraft Discord");
    if dresp.on_hover_text("Join the ArtCraft community on Discord").clicked() {
        let _ = app.session.execute("help.discord", json!({}));
    }
    rx = discord.min.x - 2.0;
    for (id, icon, tip, cmd) in
        [("hdr-github", Icon::Code, "EffectCraft on GitHub", "help.github"), ("hdr-web", Icon::Globe, "EffectCraft on getartcraft.com", "help.appPage")]
    {
        let r = Rect::from_min_max(pos2(rx - 24.0, cy - 12.0), pos2(rx, cy + 12.0));
        if widgets::icon_button(ui, r, icon, false, &t, egui::Id::new(id)).on_hover_text(tip).clicked() {
            let _ = app.session.execute(cmd, json!({}));
        }
        app.auto.add(&format!("header.{}", &id[4..]), r, tip);
        rx = r.min.x - 2.0;
    }
    rx -= 10.0;
    p.line_segment([pos2(rx, cy - 10.0), pos2(rx, cy + 10.0)], Stroke::new(1.0, t.separator));
    rx -= 10.0;
    // Workspace tabs (right to left), with a » menu of all.
    let more = Rect::from_min_max(pos2(rx - 20.0, cy - 11.0), pos2(rx, cy + 11.0));
    let mresp = ui.interact(more, egui::Id::new("ws-more"), Sense::click());
    p.text(more.center(), Align2::CENTER_CENTER, "»", Tokens::ui(15.0), if mresp.hovered() { t.text } else { t.text_dim });
    app.auto.add("header.workspaces", more, "Workspaces");
    mresp.context_menu(|ui| ws_menu(app, ui));
    if mresp.clicked() {
        widgets::open_popup(ui, egui::Id::new("ws-pop"));
    }
    let names: Vec<String> = crate::dock::WORKSPACES.iter().map(|s| s.to_string()).collect();
    if let Some(i) = widgets::popup_menu(
        ui,
        egui::Id::new("ws-pop"),
        more.left_bottom() - vec2(140.0, 0.0),
        &names,
        crate::dock::WORKSPACES.iter().position(|w| *w == app.ui.workspace),
    ) {
        app.set_workspace(crate::dock::WORKSPACES[i]);
    }
    rx = more.min.x - 6.0;
    // After Effects 2026's workspace bar order (right to left here): Default, Review, Learn,
    // Small Screen, Standard.
    let shown = ["Standard", "Small Screen", "Learn", "Review", "Default"];
    for name in shown {
        let g = p.layout_no_wrap(name.to_string(), Tokens::ui(12.0), t.text);
        let w = g.size().x + 16.0;
        let r = Rect::from_min_max(pos2(rx - w, cy - 12.0), pos2(rx, cy + 12.0));
        if r.min.x < x + 120.0 {
            break;
        }
        let active = app.ui.workspace == name;
        let resp = ui.interact(r, egui::Id::new(("ws", name)), Sense::click());
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

/// A primary-button hold reveals grouped tools; secondary click remains an immediate shortcut.
/// Remember one opening per press, and consume that press's release so it cannot activate the
/// original tool or close the flyout immediately. No application/document state lives in this timer.
fn tool_hold(response: &egui::Response) -> (bool, bool) {
    const HOLD_SECONDS: f64 = 0.4;
    let ctx = &response.ctx;
    let fired_id = response.id.with("tool-hold-fired");
    let cancelled_id = response.id.with("tool-hold-cancelled");
    let owned_id = response.id.with("tool-hold-owned");
    let max_distance = ctx.options(|o| o.input_options.max_click_dist);
    let (down, pressed, released, elapsed, moved) = ctx.input(|i| {
        (
            i.pointer.primary_down(),
            i.pointer.primary_pressed(),
            i.pointer.primary_released(),
            i.pointer.press_start_time().map(|start| i.time - start),
            i.pointer.press_origin().zip(i.pointer.latest_pos()).is_some_and(|(start, now)| start.distance(now) > max_distance),
        )
    });
    if pressed {
        // egui's interaction radius may claim nearby presses outside the painted button.
        // A hold must begin inside this tool as well as belonging to its response.
        let owns_press = response.is_pointer_button_down_on() && ctx.input(|i| i.pointer.press_origin().is_some_and(|pos| response.rect.contains(pos)));
        ctx.data_mut(|d| {
            d.remove::<bool>(fired_id);
            d.remove::<bool>(cancelled_id);
            d.insert_temp(owned_id, owns_press);
        });
    }
    let fired = ctx.data(|d| d.get_temp::<bool>(fired_id)).unwrap_or(false);
    if !down {
        // Keep the latch for all passes of the release frame (egui may perform a sizing pass).
        if !released {
            ctx.data_mut(|d| {
                d.remove::<bool>(fired_id);
                d.remove::<bool>(cancelled_id);
                d.remove::<bool>(owned_id);
            });
        }
        return (false, fired && released);
    }
    if moved {
        ctx.data_mut(|d| d.insert_temp(cancelled_id, true));
    }
    let cancelled = ctx.data(|d| d.get_temp::<bool>(cancelled_id)).unwrap_or(false);
    if fired {
        return (false, true);
    }
    // egui retires a click-only widget's press after its maximum click duration,
    // even without pointer movement. Keep the original owner for a delayed frame.
    let owned = ctx.data(|d| d.get_temp::<bool>(owned_id)).unwrap_or(false);
    if owned
        && !cancelled
        && let Some(elapsed) = elapsed.filter(|v| v.is_finite() && *v >= 0.0)
    {
        if elapsed >= HOLD_SECONDS {
            ctx.data_mut(|d| d.insert_temp(fired_id, true));
            return (true, true);
        }
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(HOLD_SECONDS - elapsed));
    }
    (false, false)
}

fn tool_flyout(response: &egui::Response, tools: &[Tool], current: Tool, reg: &mut crate::automation::Registry) -> (Option<Tool>, bool) {
    let (held_open, suppress_click) = tool_hold(response);
    let command = if held_open || response.secondary_clicked() {
        Some(egui::SetOpenCommand::Bool(true))
    } else if response.clicked() && !suppress_click {
        Some(egui::SetOpenCommand::Bool(false))
    } else {
        None
    };
    let mut choice = None;
    egui::Popup::menu(response)
        .open_memory(command)
        .close_behavior(if suppress_click { egui::PopupCloseBehavior::IgnoreClicks } else { egui::PopupCloseBehavior::CloseOnClick })
        .show(|ui| {
            for tool in tools {
                let response = ui.selectable_label(current == *tool, tool.label());
                reg.add(&format!("tools.flyout.{tool:?}"), response.rect, tool.label());
                if response.clicked() {
                    choice = Some(*tool);
                    ui.close();
                }
            }
        });
    (choice, suppress_click)
}

/// Puppet tool options: Mesh: Show, Expansion, Density (for new meshes and the selected
/// layer's meshes). Returns the next x.
fn puppet_options(app: &mut EffectcraftApp, ui: &mut egui::Ui, p: &egui::Painter, mut x: f32, cy: f32) -> f32 {
    let t = app.tokens;
    let o = app.session.state.puppet.clone();
    p.text(pos2(x, cy), Align2::LEFT_CENTER, "Mesh:", Tokens::ui(12.0), t.text_dim);
    x += 40.0;
    let show = Rect::from_min_size(pos2(x, cy - 10.0), vec2(20.0, 20.0));
    if widgets::checkbox(ui, show, o.show_mesh, &t, egui::Id::new("puppet-show-mesh")).clicked() {
        let _ = app.session.execute("puppet.mesh", json!({"showMesh": !o.show_mesh}));
    }
    app.auto.add("header.puppet.showMesh", show, "Show mesh");
    p.text(pos2(show.max.x + 2.0, cy), Align2::LEFT_CENTER, "Show", Tokens::ui(12.0), t.text_dim);
    x = show.max.x + 44.0;
    for (label, key, v, range) in [("Expansion:", "expansion", o.expansion, (-100.0, 200.0)), ("Density:", "density", o.density, (0.0, 100.0))] {
        p.text(pos2(x, cy), Align2::LEFT_CENTER, label, Tokens::ui(12.0), t.text_dim);
        x += if key == "expansion" { 64.0 } else { 52.0 };
        let (r, nv, _) = widgets::hot_number_at(ui, pos2(x, cy - 9.0), egui::Id::new(("puppet-opt", key)), v, 0.2, range, 0, "", &t);
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
    let resp = ui.interact(r, egui::Id::new("puppet-record-options"), egui::Sense::click()).on_hover_text("⌘/Ctrl-drag a pin to record its motion");
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
        .interact(r, egui::Id::new("puppet-follow"), egui::Sense::click())
        .on_hover_text("Select the leader pin, then Shift-click the pins that should trail it (hair, cloth, tails)");
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

fn ws_menu(app: &mut EffectcraftApp, ui: &mut egui::Ui) {
    for w in app.workspace_names() {
        if ui.selectable_label(app.ui.workspace == w, &w).clicked() {
            app.set_workspace(&w);
            ui.close();
        }
    }
    ui.separator();
    if ui.button("Reset to Saved Layout").clicked() {
        let n = app.ui.workspace.clone();
        app.set_workspace(&n);
        ui.close();
    }
}

/// A small colour picker popup bound to an RGB value.
pub fn color_popup(ui: &mut egui::Ui, id: egui::Id, pos: egui::Pos2, c: &mut [f32; 3]) -> bool {
    let open_id = id.with("open");
    if !ui.data(|d| d.get_temp::<bool>(open_id).unwrap_or(false)) {
        return false;
    }
    let mut changed = false;
    let area = egui::Area::new(id.with("area")).order(egui::Order::Foreground).fixed_pos(pos + vec2(0.0, 4.0)).show(ui.ctx(), |ui| {
        egui::Frame::popup(ui.style()).show(ui, |ui| {
            let mut col =
                egui::Color32::from_rgb((c[0].clamp(0.0, 1.0) * 255.0) as u8, (c[1].clamp(0.0, 1.0) * 255.0) as u8, (c[2].clamp(0.0, 1.0) * 255.0) as u8);
            if egui::color_picker::color_picker_color32(ui, &mut col, egui::color_picker::Alpha::Opaque) {
                *c = [col.r() as f32 / 255.0, col.g() as f32 / 255.0, col.b() as f32 / 255.0];
                changed = true;
            }
        });
    });
    if widgets::pressed_outside(ui.ctx(), &area.response) || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        ui.data_mut(|d| d.insert_temp(open_id, false));
    }
    changed
}

/// The ArtCraft mark as supplied in `docs/brand` (the README's and getartcraft.com's logo; used
/// unmodified, see `docs/brand/LICENSE-brand.txt`).
static ARTCRAFT_MARK: &[u8] = include_bytes!("../../../docs/brand/artcraft-mark.png");

/// The ArtCraft mark in `r` (decoded once into a mipmapped texture, so it stays crisp from the
/// 24 px Tools bar to the About dialog). Nothing is drawn if it can't be decoded.
pub fn paint_logo(p: &egui::Painter, r: Rect) {
    let ctx = p.ctx();
    let id = egui::Id::new("artcraft-mark");
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
    use egui::{Event, Modifiers, PointerButton};

    struct Fixture {
        ctx: egui::Context,
        tool: Tool,
        reg: crate::automation::Registry,
    }

    impl Fixture {
        fn new() -> Self {
            Self { ctx: egui::Context::default(), tool: Tool::Hand, reg: crate::automation::Registry::default() }
        }

        fn frame(&mut self, time: f64, events: Vec<Event>) {
            self.reg.begin_frame();
            self.ctx
                .run_ui(egui::RawInput { time: Some(time), events, ..Default::default() }, |ui| {
                    let r = Rect::from_min_size(pos2(20.0, 20.0), vec2(24.0, 24.0));
                    let response = ui.interact(r, egui::Id::new("fixture-tool"), Sense::click());
                    let (choice, suppress_click) = tool_flyout(&response, &[Tool::Rectangle, Tool::Ellipse], self.tool, &mut self.reg);
                    if response.clicked() && !suppress_click {
                        self.tool = Tool::Rectangle;
                    }
                    if let Some(tool) = choice {
                        self.tool = tool;
                    }
                })
                .drop_without_applying_deltas();
        }

        fn popup_open(&self) -> bool {
            egui::Popup::is_id_open(&self.ctx, egui::Id::new("fixture-tool").with("popup"))
        }
    }

    fn button(pos: egui::Pos2, button: PointerButton, pressed: bool) -> Vec<Event> {
        vec![Event::PointerMoved(pos), Event::PointerButton { pos, button, pressed, modifiers: Modifiers::NONE }]
    }

    #[test]
    fn tool_flyout_short_click_selects_without_opening() {
        let mut fixture = Fixture::new();
        let pos = pos2(32.0, 32.0);
        fixture.frame(0.0, vec![]);
        fixture.frame(0.1, button(pos, PointerButton::Primary, true));
        fixture.frame(0.2, button(pos, PointerButton::Primary, false));
        assert_eq!(fixture.tool, Tool::Rectangle);
        assert!(!fixture.popup_open());
    }

    #[test]
    fn tool_flyout_hold_survives_release_and_allows_a_new_tool_choice() {
        let mut fixture = Fixture::new();
        let pos = pos2(32.0, 32.0);
        fixture.frame(0.0, vec![]);
        fixture.frame(0.1, button(pos, PointerButton::Primary, true));
        fixture.frame(0.3, vec![]);
        assert!(!fixture.popup_open());
        fixture.frame(0.6, vec![]);
        assert!(fixture.popup_open());
        fixture.frame(0.65, button(pos, PointerButton::Primary, false));
        fixture.frame(0.7, vec![]);
        assert!(fixture.popup_open(), "the initiating release must not close the menu");
        assert_eq!(fixture.tool, Tool::Hand, "the initiating release must not select the original tool");
        let choice = fixture.reg.elements.iter().find(|e| e.id == "tools.flyout.Ellipse").unwrap();
        let pos = pos2(choice.rect[0] + choice.rect[2] / 2.0, choice.rect[1] + choice.rect[3] / 2.0);
        fixture.frame(0.8, button(pos, PointerButton::Primary, true));
        fixture.frame(0.9, button(pos, PointerButton::Primary, false));
        assert_eq!(fixture.tool, Tool::Ellipse);
        assert!(!fixture.popup_open());
    }

    #[test]
    fn tool_flyout_hold_opens_even_when_the_next_frame_arrives_late() {
        let mut fixture = Fixture::new();
        let pos = pos2(32.0, 32.0);
        fixture.frame(0.0, vec![]);
        fixture.frame(0.1, button(pos, PointerButton::Primary, true));
        fixture.frame(1.2, vec![]);
        assert!(fixture.popup_open(), "a slow frame is not a pointer drag");
        fixture.frame(1.3, button(pos, PointerButton::Primary, false));
        assert!(fixture.popup_open());
        assert_eq!(fixture.tool, Tool::Hand);
    }

    #[test]
    fn tool_flyout_hold_does_not_claim_a_press_from_outside_the_button() {
        let mut fixture = Fixture::new();
        let outside = pos2(19.0, 32.0);
        let inside = pos2(21.0, 32.0);
        fixture.frame(0.0, vec![]);
        fixture.frame(0.1, button(outside, PointerButton::Primary, true));
        // Cross the edge without exceeding egui's click-distance tolerance.
        fixture.frame(0.2, vec![Event::PointerMoved(inside)]);
        fixture.frame(1.2, vec![]);
        fixture.frame(1.3, button(inside, PointerButton::Primary, false));
        assert!(!fixture.popup_open());
        assert_eq!(fixture.tool, Tool::Hand);
    }

    #[test]
    fn tool_flyout_secondary_click_and_escape_preserve_the_tool() {
        let mut fixture = Fixture::new();
        let pos = pos2(32.0, 32.0);
        fixture.frame(0.0, vec![]);
        fixture.frame(0.1, button(pos, PointerButton::Secondary, true));
        fixture.frame(0.2, button(pos, PointerButton::Secondary, false));
        assert!(fixture.popup_open());
        fixture.frame(0.3, vec![Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }]);
        assert!(!fixture.popup_open());
        assert_eq!(fixture.tool, Tool::Hand);
    }

    #[test]
    fn tool_flyout_drag_does_not_open_a_hold_menu() {
        let mut fixture = Fixture::new();
        let pos = pos2(32.0, 32.0);
        fixture.frame(0.0, vec![]);
        fixture.frame(0.1, button(pos, PointerButton::Primary, true));
        fixture.frame(0.3, vec![Event::PointerMoved(pos2(90.0, 80.0))]);
        fixture.frame(0.7, vec![]);
        fixture.frame(0.8, button(pos2(90.0, 80.0), PointerButton::Primary, false));
        assert!(!fixture.popup_open());
        assert_eq!(fixture.tool, Tool::Hand);
    }
}
