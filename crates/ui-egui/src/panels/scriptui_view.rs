//! ScriptUI windows: the dialogs, palettes and dockable panels scripts build
//! (`Session::script_ui`). Each control is drawn at the bounds the engine's ScriptUI layout
//! computed; what the user does goes back through the `scriptui.click` / `scriptui.set` /
//! `scriptui.close` commands, which run the script's handlers. Every control registers an
//! automation id `scriptui.<window>.<control>` (and `scriptui.<window>.<name>` when the script
//! named it). Controls with an `onDraw` handler are painted from their draw list; edit texts and
//! sliders send live `changing` updates (onChanging) while typing / dragging. Mouse events
//! (`mousedown`, `mouseup`, `mousemove`, `mouseover`, `mouseout`, `click`) on a control go to the
//! script through `scriptui.mouse` when a listener wants them (`Widget::mouse`).

use crate::i18n::tr;
use effectcraft_engine::Services;
use effectcraft_engine::scriptui::{DrawOp, ImageRef, PathSeg, ScriptWindow, Widget, WidgetKind, WindowKind, layout};
use egui::{Align2, Color32, Rect, Sense, Stroke, StrokeKind, UiBuilder, pos2, vec2};
use serde_json::{Value, json};

use crate::EffectcraftApp;
use crate::dock::{PanelKind, WorkspaceExt as _};
use crate::theme::Tokens;

enum Act {
    Click(u32, u32),
    Set(u32, u32, Value),
    /// A live update (each keystroke / slider step): fires onChanging.
    Changing(u32, u32, Value),
    Close(u32),
    /// A mouse event (`scriptui.mouse` parameters without window and widget).
    Mouse(u32, u32, Value),
    /// A docked panel's new size (`scriptui.resize`).
    Resize(u32, [f32; 2]),
}

fn rect_of(origin: egui::Pos2, b: [f64; 4]) -> Rect {
    Rect::from_min_size(origin + vec2(b[0] as f32, b[1] as f32), vec2(b[2] as f32, b[3] as f32))
}

fn register(app: &mut EffectcraftApp, win: u32, w: &Widget, rect: Rect) {
    let label = if w.text.is_empty() { format!("{:?}", w.kind) } else { w.text.clone() };
    app.auto.add(&format!("scriptui.{win}.{}", w.id), rect, &label);
    if !w.name.is_empty() {
        app.auto.add(&format!("scriptui.{win}.{}", w.name), rect, &label);
    }
}

/// Draw `w`'s children (and, for containers, their children) at `origin` + bounds.
fn draw_children(app: &mut EffectcraftApp, ui: &mut egui::Ui, origin: egui::Pos2, win: u32, w: &Widget, acts: &mut Vec<Act>) {
    let t = app.tokens;
    if w.kind == WidgetKind::TabbedPanel {
        // Tab headers, then the active tab only.
        let r = rect_of(origin, w.bounds);
        let mut x = r.min.x;
        for (i, tab) in w.children.iter().enumerate() {
            let tw = (tab.text.chars().count() as f32 * 7.0 + 20.0).max(48.0);
            let hr = Rect::from_min_size(pos2(x, r.min.y), vec2(tw, 22.0));
            let active = i == w.active_tab;
            let resp = ui.interact(hr, egui::Id::unique(("sui-tab", win, tab.id)), Sense::click());
            ui.painter().rect_filled(
                hr,
                3.0,
                if active {
                    t.row_selected
                } else if resp.hovered() {
                    t.hover
                } else {
                    t.field_bg
                },
            );
            ui.painter().text(hr.center(), Align2::CENTER_CENTER, &tab.text, Tokens::ui(12.0), t.text);
            register(app, win, tab, hr);
            if resp.clicked() && !active && tab.enabled {
                acts.push(Act::Click(win, tab.id));
            }
            x += tw + 2.0;
        }
        ui.painter().rect_stroke(Rect::from_min_max(pos2(r.min.x, r.min.y + 22.0), r.max), 3.0, Stroke::new(1.0, t.field_border), StrokeKind::Inside);
        if let Some(tab) = w.children.get(w.active_tab) {
            draw_children(app, ui, origin, win, tab, acts);
        }
        return;
    }
    for c in w.children.iter().filter(|c| c.visible) {
        draw_widget(app, ui, origin, win, c, acts);
    }
}

/// Draws ScriptUI widget `w` of window `win` at its bounds offset by `origin`, registering it for
/// automation, and recurses into containers. Widgets with an `onDraw` handler are painted by the
/// script (replacing the default look, or on top of it when it called `drawOSControl()`) while
/// buttons, checkboxes and radios still take clicks. Edits and clicks are appended to `acts` for
/// [`perform`]; while a field or slider is in use its live value lives in egui temp data.
fn draw_widget(app: &mut EffectcraftApp, ui: &mut egui::Ui, origin: egui::Pos2, win: u32, w: &Widget, acts: &mut Vec<Act>) {
    let t = app.tokens;
    let services = app.session.services.clone();
    let rect = rect_of(origin, w.bounds);
    register(app, win, w, rect);
    let id = egui::Id::unique(("sui", win, w.id));
    let tip = |r: egui::Response| if w.help_tip.is_empty() { r } else { r.on_hover_text(&w.help_tip) };
    // onDraw: the script paints the control. Without drawOSControl() its paint replaces the
    // default look (buttons and toggles still respond to clicks); with it, it goes on top.
    let custom = w.handlers.iter().any(|h| h == "onDraw");
    let os = w.draw.iter().any(|d| matches!(d, DrawOp::Os));
    if custom && !os {
        if w.kind.is_container() {
            paint_draw(ui, rect, &w.draw, &t, &*services);
            draw_children(app, ui, origin, win, w, acts);
            return;
        }
        let clickable = matches!(w.kind, WidgetKind::Button | WidgetKind::IconButton | WidgetKind::Checkbox | WidgetKind::RadioButton);
        let r = ui.interact(rect, id, if clickable && w.enabled { Sense::click() } else { Sense::hover() });
        paint_draw(ui, rect, &w.draw, &t, &*services);
        if tip(r).clicked() && clickable {
            acts.push(Act::Click(win, w.id));
        }
        return;
    }
    if custom && w.kind.is_container() {
        paint_draw(ui, rect, &w.draw, &t, &*services);
    }
    match w.kind {
        WidgetKind::Group | WidgetKind::Tab | WidgetKind::Window | WidgetKind::TabbedPanel => draw_children(app, ui, origin, win, w, acts),
        WidgetKind::Panel => {
            let p = ui.painter();
            p.rect_stroke(rect.shrink(1.0), 4.0, Stroke::new(1.0, t.field_border), StrokeKind::Inside);
            if !w.text.is_empty() {
                let g = p.layout_no_wrap(w.text.clone(), Tokens::ui(11.5), t.text_dim);
                let at = pos2(rect.min.x + 10.0, rect.min.y - 1.0);
                p.rect_filled(Rect::from_min_size(at - vec2(3.0, 0.0), g.size() + vec2(6.0, 0.0)), 0.0, t.panel_bg);
                p.galley(at, g, t.text_dim);
            }
            draw_children(app, ui, origin, win, w, acts);
        }
        WidgetKind::Button | WidgetKind::IconButton => {
            let r = ui.add_enabled_ui(w.enabled, |ui| ui.put(rect, egui::Button::new(&w.text))).inner;
            if let Some(img) = &w.image {
                paint_image(ui, rect.shrink(3.0), img, &*services);
            }
            if tip(r).clicked() {
                acts.push(Act::Click(win, w.id));
            }
        }
        WidgetKind::StaticText => {
            let col = if w.enabled { t.text } else { t.text_faint };
            ui.painter().with_clip_rect(rect.expand(2.0)).text(pos2(rect.min.x, rect.min.y + 1.0), Align2::LEFT_TOP, &w.text, Tokens::ui(12.0), col);
        }
        WidgetKind::EditText => {
            // The text being typed lives here until the field loses focus (then onChange).
            let mut buf: String = ui.data(|d| d.get_temp::<String>(id)).unwrap_or_else(|| w.text.clone());
            let focused = ui.memory(|m| m.has_focus(id));
            if !focused {
                buf = w.text.clone();
            }
            let te = if w.multiline { egui::TextEdit::multiline(&mut buf) } else { egui::TextEdit::singleline(&mut buf) };
            let te = te.id(id).interactive(!w.read_only).font(Tokens::ui(12.0));
            let r = ui.add_enabled_ui(w.enabled, |ui| ui.put(rect, te)).inner;
            let r = tip(r);
            // Live updates already sent the text: the commit still runs onChange.
            let dirty_id = id.with("dirty");
            let dirty = ui.data(|d| d.get_temp::<bool>(dirty_id)).unwrap_or(false);
            if r.lost_focus() && (buf != w.text || dirty) {
                acts.push(Act::Set(win, w.id, json!(buf)));
                ui.data_mut(|d| d.remove::<bool>(dirty_id));
            } else if r.changed() && w.handlers.iter().any(|h| h == "onChanging") {
                // Each keystroke reaches the script's onChanging (onChange runs on commit).
                acts.push(Act::Changing(win, w.id, json!(buf)));
                ui.data_mut(|d| d.insert_temp(dirty_id, true));
            }
            ui.data_mut(|d| d.insert_temp(id, buf));
        }
        WidgetKind::Checkbox => {
            let mut v = w.checked;
            let r = ui.add_enabled_ui(w.enabled, |ui| ui.put(snug(rect, &w.text), egui::Checkbox::new(&mut v, &w.text))).inner;
            if tip(r).clicked() {
                acts.push(Act::Click(win, w.id));
            }
        }
        WidgetKind::RadioButton => {
            let r = ui.add_enabled_ui(w.enabled, |ui| ui.put(snug(rect, &w.text), egui::RadioButton::new(w.checked, &w.text))).inner;
            if tip(r).clicked() && !w.checked {
                acts.push(Act::Click(win, w.id));
            }
        }
        WidgetKind::Slider | WidgetKind::Scrollbar => {
            // While dragging, the value lives here; the release sends it (onChanging / onChange).
            let mut v: f64 = ui.data(|d| d.get_temp::<f64>(id)).unwrap_or(w.value);
            let (lo, hi) = if w.max > w.min { (w.min, w.max) } else { (0.0, 100.0) };
            let r = ui
                .add_enabled_ui(w.enabled, |ui| {
                    ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
                        ui.spacing_mut().slider_width = rect.width() - 4.0;
                        ui.add(egui::Slider::new(&mut v, lo..=hi).show_value(false))
                    })
                    .inner
                })
                .inner;
            let r = tip(r);
            if r.dragged() {
                // Live: each step of the drag reaches the script (onChanging).
                if r.changed() {
                    acts.push(Act::Changing(win, w.id, json!(v)));
                }
                ui.data_mut(|d| d.insert_temp(id, v));
            } else {
                ui.data_mut(|d| d.remove::<f64>(id));
                if r.drag_stopped() || r.changed() {
                    acts.push(Act::Set(win, w.id, json!(v)));
                }
            }
        }
        WidgetKind::Progressbar => {
            let span = if w.max > w.min { w.max - w.min } else { 100.0 };
            let f = ((w.value - w.min) / span).clamp(0.0, 1.0) as f32;
            ui.put(rect, egui::ProgressBar::new(f).desired_height(rect.height()));
        }
        WidgetKind::DropDownList => {
            let cur = w.selection.first().and_then(|i| w.items.get(*i)).cloned().unwrap_or_default();
            let mut pick: Option<usize> = None;
            ui.add_enabled_ui(w.enabled, |ui| {
                ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
                    craft_ui::ComboBox::from_id_salt(id).selected_text(cur).width(rect.width() - 8.0).show_ui(ui, |ui| {
                        for (i, it) in w.items.iter().enumerate() {
                            if it == "-" {
                                ui.separator();
                            } else if ui.selectable_label(w.selection.contains(&i), it).clicked() {
                                pick = Some(i);
                            }
                        }
                    });
                });
            });
            if let Some(i) = pick
                && !w.selection.contains(&i)
            {
                acts.push(Act::Set(win, w.id, json!(i)));
            }
        }
        WidgetKind::ListBox => {
            let mut pick: Option<usize> = None;
            ui.painter().rect_filled(rect, 2.0, t.field_bg);
            ui.add_enabled_ui(w.enabled, |ui| {
                ui.scope_builder(UiBuilder::new().max_rect(rect.shrink(2.0)), |ui| {
                    egui::ScrollArea::vertical().id_salt(id).max_height(rect.height() - 4.0).show(ui, |ui| {
                        ui.set_width(rect.width() - 8.0);
                        for (i, it) in w.items.iter().enumerate() {
                            if ui.selectable_label(w.selection.contains(&i), it).clicked() {
                                pick = Some(i);
                            }
                        }
                    });
                });
            });
            if let Some(i) = pick {
                acts.push(Act::Set(win, w.id, json!(i)));
            }
        }
        WidgetKind::Image | WidgetKind::Unknown => {
            if !w.image.as_ref().is_some_and(|img| paint_image(ui, rect, img, &*services)) {
                ui.painter().rect_stroke(rect, 2.0, Stroke::new(1.0, t.field_border), StrokeKind::Inside);
            }
        }
    }
    if custom && !w.kind.is_container() {
        paint_draw(ui, rect, &w.draw, &t, &*services);
    }
}

/// A draw-list colour (`None` is the theme colour). Script colours are display (sRGB) values
/// in 0..1, as in After Effects, not linear light: `[43/255, 43/255, 43/255, 1]` is RGB 43.
fn draw_color(c: &Option<[f32; 4]>, theme: Color32) -> Color32 {
    c.map_or(theme, super::gradient_editor::to_color32)
}

/// Paint an onDraw draw list in `rect` (control coordinates → screen).
fn paint_draw(ui: &egui::Ui, rect: Rect, ops: &[DrawOp], t: &Tokens, services: &dyn Services) {
    let p = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    let at = |x: f64, y: f64| rect.min + vec2(x as f32, y as f32);
    for op in ops {
        match op {
            DrawOp::Fill { color, path } => {
                // Any path shape (concave, self-intersecting, with holes): non-zero winding
                // triangles, not egui's convex-only polygon fill.
                let c = draw_color(color, t.field_bg);
                let mut mesh = egui::Mesh::default();
                for tri in PathSeg::fill_triangles(path, 48, false) {
                    let base = mesh.vertices.len() as u32;
                    for q in tri {
                        mesh.colored_vertex(at(q[0], q[1]), c);
                    }
                    mesh.add_triangle(base, base + 1, base + 2);
                }
                if !mesh.is_empty() {
                    p.add(egui::Shape::mesh(mesh));
                }
            }
            DrawOp::Stroke { color, width, path } => {
                let s = Stroke::new(*width as f32, draw_color(color, t.text));
                for (pts, closed) in PathSeg::polylines(path, 48) {
                    let pts: Vec<egui::Pos2> = pts.iter().map(|q| at(q[0], q[1])).collect();
                    p.add(if closed { egui::Shape::closed_line(pts, s) } else { egui::Shape::line(pts, s) });
                }
            }
            DrawOp::Text { text, color, x, y, size, .. } => {
                p.text(at(*x, *y), Align2::LEFT_TOP, text, Tokens::ui(*size as f32), draw_color(color, t.text));
            }
            DrawOp::Image { image, x, y, w, h } => {
                let tex = image.as_ref().and_then(|i| image_texture(ui.ctx(), i, services));
                let size = match &tex {
                    Some(tx) if *w <= 0.0 || *h <= 0.0 => tx.size_vec2(),
                    _ => vec2(*w as f32, *h as f32),
                };
                let r = Rect::from_min_size(at(*x, *y), size);
                match tex {
                    Some(tx) => {
                        p.image(tx.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                    }
                    None => {
                        p.rect_stroke(r, 0.0, Stroke::new(1.0, t.field_border), StrokeKind::Inside);
                    }
                }
            }
            DrawOp::Os => {}
        }
    }
}

/// A ScriptUIImage as a texture (decoded once and kept; `None` when it can't be read or
/// decoded).
fn image_texture(ctx: &egui::Context, img: &ImageRef, services: &dyn Services) -> Option<egui::TextureHandle> {
    let id = egui::Id::unique(("scriptui-image", img));
    if let Some(cached) = ctx.data(|d| d.get_temp::<Option<egui::TextureHandle>>(id)) {
        return cached;
    }
    let tex = img.bytes(services).and_then(|b| decode_ui_image(&b)).map(|d| {
        let rgba = d.to_rgba8();
        let ci = egui::ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw());
        ctx.load_texture(format!("scriptui-image-{}", img.name), ci, egui::TextureOptions::LINEAR)
    });
    ctx.data_mut(|d| d.insert_temp(id, tex.clone()));
    tex
}

/// The largest side of a ScriptUI image (icons and banners are far smaller).
const MAX_UI_IMAGE_SIDE: u32 = 4096;

/// A ScriptUI image decoded within [`MAX_UI_IMAGE_SIDE`] per side and a matching allocation
/// limit, checked from the header before the pixels (and their RGBA copy) are allocated.
fn decode_ui_image(bytes: &[u8]) -> Option<image::DynamicImage> {
    let mut r = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_UI_IMAGE_SIDE);
    limits.max_image_height = Some(MAX_UI_IMAGE_SIDE);
    limits.max_alloc = Some(u64::from(MAX_UI_IMAGE_SIDE) * u64::from(MAX_UI_IMAGE_SIDE) * 16);
    r.limits(limits);
    r.decode().ok()
}

/// An image control's or icon button's image, fitted (aspect kept) and centred in `r`.
fn paint_image(ui: &egui::Ui, r: Rect, img: &ImageRef, services: &dyn Services) -> bool {
    let Some(tex) = image_texture(ui.ctx(), img, services) else { return false };
    let s = tex.size_vec2();
    let k = (r.width() / s.x.max(1.0)).min(r.height() / s.y.max(1.0)).min(1.0);
    let dst = Rect::from_center_size(r.center(), s * k);
    ui.painter().with_clip_rect(r.intersect(ui.clip_rect())).image(tex.id(), dst, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    true
}

/// Where the pointer was and what it did over one script window (for mouseover / mouseout and
/// click counts).
#[derive(Clone, Default)]
struct MouseTrack {
    hover: Option<u32>,
    at: Option<egui::Pos2>,
    /// The last press: control, time, position (a quick second press there counts as a double
    /// click).
    last_down: Option<(u32, f64, egui::Pos2)>,
    clicks: u32,
    /// The control and button pressed (a release on the same control is a click).
    down: Option<(u32, u8)>,
}

/// The deepest visible control of `w` at `p` (`w` itself when no child is there): later
/// siblings are on top; a tabbed panel shows its active tab only.
fn hit_test(w: &Widget, origin: egui::Pos2, p: egui::Pos2, depth: usize) -> Option<&Widget> {
    if !w.visible || !rect_of(origin, w.bounds).contains(p) {
        return None;
    }
    // Widget trees come from scripts: the depth is bounded.
    if depth < 64 {
        if w.kind == WidgetKind::TabbedPanel {
            if let Some(hit) = w.children.get(w.active_tab).and_then(|t| hit_test(t, origin, p, depth + 1)) {
                return Some(hit);
            }
        } else if let Some(hit) = w.children.iter().rev().find_map(|c| hit_test(c, origin, p, depth + 1)) {
            return Some(hit);
        }
    }
    Some(w)
}

fn wants_mouse(w: &Widget, depth: usize) -> bool {
    !w.mouse.is_empty() || (depth < 64 && w.children.iter().any(|c| wants_mouse(c, depth + 1)))
}

/// Mouse events over a script window's controls (drawn at `origin`, visible in `area`) for the
/// script's listeners: mouseover / mouseout when the control under the pointer changes,
/// mousemove, mousedown / mouseup, and click (a press and release of the left button on the
/// same control; it replaces the plain click the control reported, so a checkbox toggles once).
/// Only events a listener wants are sent; none while `active` is false (a modal dialog is up).
fn mouse_events(ui: &egui::Ui, origin: egui::Pos2, area: Rect, win: u32, root: &Widget, active: bool, acts: &mut Vec<Act>) {
    let id = egui::Id::unique(("sui-mouse", win));
    if !wants_mouse(root, 0) {
        ui.data_mut(|d| d.remove::<MouseTrack>(id));
        return;
    }
    let mut st: MouseTrack = ui.data(|d| d.get_temp(id)).unwrap_or_default();
    let (latest, events, mods, time, held) = ui.input(|i| {
        let held = |b: egui::PointerButton, bit: u8| if i.pointer.button_down(b) { bit } else { 0 };
        let held = held(egui::PointerButton::Primary, 1) | held(egui::PointerButton::Secondary, 2) | held(egui::PointerButton::Middle, 4);
        (i.pointer.latest_pos(), i.events.clone(), i.modifiers, i.time, held)
    });
    let ctx = ui.ctx();
    let layer = ui.layer_id();
    let target = |p: egui::Pos2| -> Option<&Widget> {
        if !active || !area.contains(p) || ctx.layer_id_at(p).is_some_and(|l| l != layer) {
            return None;
        }
        hit_test(root, origin, p, 0)
    };
    let send = |w: &Widget, ty: &str, p: egui::Pos2, button: u8, detail: u32, acts: &mut Vec<Act>| {
        if !w.enabled || !w.mouse.iter().any(|m| m == ty) {
            return;
        }
        let at = rect_of(origin, w.bounds).min;
        acts.push(Act::Mouse(
            win,
            w.id,
            json!({
                "type": ty, "x": p.x - at.x, "y": p.y - at.y, "screenX": p.x, "screenY": p.y,
                "button": button, "buttons": held, "detail": detail,
                "altKey": mods.alt, "ctrlKey": mods.ctrl, "shiftKey": mods.shift, "metaKey": mods.mac_cmd,
            }),
        ));
    };
    // The pointer moved: out of the old control, over the new one, along it.
    let now = latest.and_then(|p| target(p).map(|w| (w, p)));
    let now_id = now.map(|(w, _)| w.id);
    if now_id != st.hover {
        if let Some(old) = st.hover.and_then(|h| root.find(h)) {
            send(old, "mouseout", latest.unwrap_or(origin), 0, 0, acts);
        }
        if let Some((w, p)) = now {
            send(w, "mouseover", p, 0, 0, acts);
        }
        st.hover = now_id;
    }
    // While a button is held, the control it was pressed on keeps getting the moves, inside or
    // outside the window, as in After Effects (#2584).
    let captured = st.down.and_then(|(c, _)| root.find(c));
    if let Some(p) = latest.filter(|p| st.at != Some(*p)) {
        match (captured, now) {
            (Some(c), _) => send(c, "mousemove", p, 0, 0, acts),
            (None, Some((w, _))) => send(w, "mousemove", p, 0, 0, acts),
            (None, None) => {}
        }
    }
    st.at = latest;
    // Presses and releases.
    for e in &events {
        let egui::Event::PointerButton { pos, button, pressed, .. } = e else { continue };
        let b = match button {
            egui::PointerButton::Primary => 0,
            egui::PointerButton::Middle => 1,
            egui::PointerButton::Secondary => 2,
            _ => continue,
        };
        // A release goes to the control the button was pressed on, wherever the pointer is.
        let pressed_on = st.down.filter(|(_, db)| !*pressed && *db == b).and_then(|(c, _)| root.find(c));
        let Some(w) = pressed_on.or_else(|| target(*pos)) else {
            if !pressed {
                st.down = None;
            }
            continue;
        };
        if *pressed {
            let again = st.last_down.is_some_and(|(c, t, at)| c == w.id && time - t < 0.5 && at.distance(*pos) < 6.0);
            st.clicks = if again { st.clicks.saturating_add(1) } else { 1 };
            st.last_down = Some((w.id, time, *pos));
            st.down = Some((w.id, b));
            send(w, "mousedown", *pos, b, st.clicks, acts);
        } else {
            send(w, "mouseup", *pos, b, st.clicks, acts);
            let over = target(*pos).is_some_and(|t| t.id == w.id);
            if over && st.down == Some((w.id, b)) && b == 0 && w.enabled && w.mouse.iter().any(|m| m == "click") {
                // The click with the pointer's details, after mousedown / mouseup, in place of
                // the plain click the control reported.
                acts.retain(|a| !matches!(a, Act::Click(aw, ac) if *aw == win && *ac == w.id));
                send(w, "click", *pos, b, st.clicks, acts);
            }
            st.down = None;
        }
    }
    ui.data_mut(|d| d.insert_temp(id, st));
}

/// Checkboxes and radio buttons sit at the left of their (possibly filled) bounds.
fn snug(r: Rect, text: &str) -> Rect {
    let w = (text.chars().count() as f32 * 7.0 + 26.0).min(r.width());
    Rect::from_min_size(r.min, vec2(w, r.height()))
}

/// Runs the actions collected while drawing a ScriptUI window as `scriptui.*` commands, in order.
/// A reply carrying an `error` becomes a toast with the script's message; otherwise a non-empty
/// `output` becomes the status line (its last line). Command failures also go to `app.ui.status`.
fn perform(app: &mut EffectcraftApp, ctx: &egui::Context, acts: Vec<Act>) {
    for a in acts {
        let (cmd, p) = match a {
            Act::Click(w, id) => ("scriptui.click", json!({"window": w, "widget": id})),
            Act::Set(w, id, v) => ("scriptui.set", json!({"window": w, "widget": id, "value": v})),
            Act::Changing(w, id, v) => ("scriptui.set", json!({"window": w, "widget": id, "value": v, "changing": true})),
            Act::Close(w) => ("scriptui.close", json!({"window": w})),
            Act::Resize(w, [x, y]) => ("scriptui.resize", json!({"window": w, "size": [x, y]})),
            Act::Mouse(w, id, mut v) => {
                if let Some(o) = v.as_object_mut() {
                    o.insert("window".into(), json!(w));
                    o.insert("widget".into(), json!(id));
                }
                ("scriptui.mouse", v)
            }
        };
        match crate::menus::invoke(app, ctx, cmd, p) {
            Ok(r) => {
                if let Some(e) = r.get("error").filter(|e| !e.is_null()) {
                    let msg = e["message"].as_str().unwrap_or("script error").to_string();
                    app.toast = Some((msg, app.now()));
                } else if let Some(o) = r["output"].as_str().filter(|o| !o.is_empty()) {
                    app.ui.status = o.lines().last().unwrap_or_default().to_string();
                }
            }
            Err(e) => app.ui.status = e,
        }
    }
}

/// The floating script windows (dialogs, palettes, windows), drawn over the app.
pub fn show_windows(app: &mut EffectcraftApp, ctx: &egui::Context) {
    let windows: Vec<ScriptWindow> = app.session.script_ui.windows.iter().filter(|w| w.kind != WindowKind::Panel).cloned().collect();
    if windows.is_empty() {
        return;
    }
    let t = app.tokens;
    let mut acts = vec![];
    let screen = ctx.content_rect();
    let blocked = windows.iter().any(|w| w.modal);
    if blocked {
        // A modal dialog blocks the app behind it, as in After Effects.
        let r = egui::Area::new(egui::Id::unique("scriptui-modal-block")).order(egui::Order::Middle).fixed_pos(screen.min).show(ctx, |ui| {
            ui.allocate_exact_size(screen.size(), Sense::click_and_drag());
            ui.painter().rect_filled(screen, 0.0, Color32::from_black_alpha(90));
        });
        let _ = r;
    }
    for w in &windows {
        let size = vec2(w.root.bounds[2] as f32, w.root.bounds[3] as f32);
        let mut open = true;
        let order = if w.modal { egui::Order::Foreground } else { egui::Order::Middle };
        let resp = egui::Window::new(if w.title.is_empty() { &w.script } else { &w.title })
            .id(egui::Id::unique(("scriptui-window", w.id)))
            .order(order)
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .default_pos(screen.center() - size / 2.0)
            .frame(egui::Frame::window(&ctx.global_style()).fill(t.panel_bg))
            .show(ctx, |ui| {
                let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
                draw_children(app, ui, rect.min, w.id, &w.root, &mut acts);
                mouse_events(ui, rect.min, rect, w.id, &w.root, w.modal || !blocked, &mut acts);
                rect
            });
        if let Some(r) = resp {
            app.auto.add(&format!("scriptui.{}", w.id), r.response.rect, &w.title);
        }
        if !open {
            acts.push(Act::Close(w.id));
        }
    }
    perform(app, ctx, acts);
}

/// A dockable ScriptUI panel's content (Window ▸ `<panel>.jsx`).
pub fn panel(app: &mut EffectcraftApp, ui: &mut egui::Ui, rect: Rect, id: u32) {
    let t = app.tokens;
    let Some(mut w) = app.session.script_ui.window(id).cloned() else {
        ui.painter().text(rect.center(), Align2::CENTER_CENTER, tr("This ScriptUI panel has closed"), Tokens::ui(12.0), t.text_faint);
        return;
    };
    // Docked panels fill their frame (ScriptUI lays them out at the panel's size).
    let (pw, ph) = (rect.width() as f64, rect.height() as f64);
    w.root.fixed_bounds = Some([0.0, 0.0, pw.max(w.root.bounds[2]), ph.max(w.root.bounds[3])]);
    layout(&mut w);
    // A modal script dialog blocks the panels too.
    let modal = app.session.script_ui.windows.iter().any(|w| w.modal);
    let mut acts = vec![];
    // The script learns its dock's size when it changes (#2585), as After Effects tells a
    // docked panel: its window takes the size and onResize runs.
    let dock = [rect.width().round(), rect.height().round()];
    let told = egui::Id::unique(("scriptui-dock-size", id));
    if ui.data(|d| d.get_temp::<[f32; 2]>(told)) != Some(dock) && dock[0] >= 1.0 && dock[1] >= 1.0 {
        ui.data_mut(|d| d.insert_temp(told, dock));
        acts.push(Act::Resize(id, dock));
    }
    let window = vec2(w.root.bounds[2] as f32, w.root.bounds[3] as f32);
    panel_scroll(ui, ("scriptui-panel", id), rect.size(), window, |ui, r| {
        draw_children(app, ui, r.min, id, &w.root, &mut acts);
        mouse_events(ui, r.min, r.intersect(ui.clip_rect()), id, &w.root, !modal, &mut acts);
    });
    app.auto.add(&format!("scriptui.{id}"), rect, &w.title);
    let ctx = ui.ctx().clone();
    perform(app, &ctx, acts);
}

/// The scroll area a docked ScriptUI panel shows its `window`-sized window in, at most `dock`
/// big, with the app's scroll style (solid bars, #269). `body` draws the window in the rect it is
/// given.
fn panel_scroll<R>(
    ui: &mut egui::Ui,
    salt: impl std::hash::Hash + std::fmt::Debug,
    dock: egui::Vec2,
    window: egui::Vec2,
    body: impl FnOnce(&mut egui::Ui, Rect) -> R,
) -> egui::scroll_area::ScrollAreaOutput<R> {
    egui::ScrollArea::both().id_salt(salt).max_width(dock.x).max_height(dock.y).scroll_bar_visibility(panel_bars(dock, window)).show(ui, |ui| {
        let (r, _) = ui.allocate_exact_size(window, Sense::hover());
        body(ui, r)
    })
}

/// Whether a docked panel's scroll bars may show: never while its window fits the dock.
///
/// The fit is decided from the window and the dock, not from what the scroll area has left. A
/// solid bar, once shown, takes its width from the viewport, so a window that exactly fills its
/// dock was "too large" for the narrower viewport and its bars never went away (#2796). The fit
/// rounds the dock up to the whole point, as egui's scroll area does, because the script is told
/// its dock's size rounded (#2585) and its window can be a fraction of a point bigger. A window
/// that overflows only one way still gets both bars: the first one's width hides the edge of a
/// window laid out at the dock's full size, and the second one scrolls to it.
fn panel_bars(dock: egui::Vec2, window: egui::Vec2) -> egui::scroll_area::ScrollBarVisibility {
    use egui::scroll_area::ScrollBarVisibility;
    if window.x <= dock.x.ceil() && window.y <= dock.y.ceil() { ScrollBarVisibility::AlwaysHidden } else { ScrollBarVisibility::VisibleWhenNeeded }
}

/// Tab title of a script panel.
pub fn panel_title(app: &EffectcraftApp, id: u32) -> Option<String> {
    app.session.script_ui.window(id).map(|w| if w.title.is_empty() { w.script.clone() } else { w.title.clone() })
}

/// Close the dock panels whose script window has gone (the script closed it, or a saved
/// workspace from an earlier run).
pub fn sync_panels(app: &mut EffectcraftApp) {
    for p in app.ui.dock.panels() {
        if let PanelKind::ScriptPanel(id) = p
            && app.session.script_ui.window(id).is_none()
        {
            app.close_panel(p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOCK: egui::Vec2 = egui::vec2(300.0, 200.0);

    /// Twelve frames of a ScriptUI panel docked in `dock`, with the app's solid scroll bars
    /// (#269): its window is `first` big on the first frame and `then` big after that. The last
    /// frame's scroll output (viewport and content size).
    fn panel_frames(dock: egui::Vec2, first: egui::Vec2, then: egui::Vec2) -> (Rect, egui::Vec2) {
        let ctx = egui::Context::default();
        ctx.all_styles_mut(|s| s.spacing.scroll = egui::style::ScrollStyle::solid());
        let screen = Rect::from_min_size(egui::Pos2::ZERO, vec2(800.0, 600.0));
        let mut last = (Rect::NOTHING, egui::Vec2::ZERO);
        for frame in 0..12 {
            let window = if frame == 0 { first } else { then };
            let input = egui::RawInput { screen_rect: Some(screen), time: Some(f64::from(frame) * 0.1), predicted_dt: 0.1, ..Default::default() };
            let mut out = ctx.run_ui(input, |ui| {
                let mut panel = ui.new_child(UiBuilder::new().max_rect(Rect::from_min_size(egui::Pos2::ZERO, dock)));
                let o = panel_scroll(&mut panel, "panel-test", dock, window, |_, _| ());
                last = (o.inner_rect, o.content_size);
            });
            // Nothing paints these frames, and egui expects their texture uploads to be consumed.
            out.textures_delta.clear();
        }
        last
    }

    /// #2796: a solid bar, once shown, took its width from the viewport, so a window that exactly
    /// fills its dock stayed "too large" for what was left and kept its bars for good. A window
    /// that overflowed for one frame (a dock dragged smaller before the script learns its size)
    /// and then fits gets the whole dock back.
    #[test]
    fn a_panel_whose_window_fits_hides_its_bars() {
        let (viewport, _) = panel_frames(DOCK, DOCK + vec2(40.0, 40.0), DOCK);
        assert_eq!(viewport.size(), DOCK, "the bars go away once the window fits");
    }

    /// The script is told its dock's size rounded (#2585), so in a dock a fraction of a pixel
    /// smaller its window is that fraction bigger. That still fits.
    #[test]
    fn a_panel_in_a_fractional_dock_hides_its_bars() {
        let dock = vec2(299.6, 199.6);
        let (viewport, _) = panel_frames(dock, dock + vec2(40.0, 40.0), DOCK);
        // egui snaps rects to 1/32 of a point; a bar would take 10 points.
        assert!((viewport.size() - dock).length() < 0.05, "a window within the whole pixel fits ({viewport:?})");
    }

    /// #269 holds: a window bigger than its dock keeps solid bars that take their room from the
    /// panel, and scrolls.
    #[test]
    fn a_panel_whose_window_is_too_big_keeps_its_solid_bars() {
        let big = DOCK + vec2(120.0, 80.0);
        let (viewport, content) = panel_frames(DOCK, big, big);
        let bar = egui::style::ScrollStyle::solid().allocated_width();
        assert_eq!(viewport.size(), DOCK - vec2(bar, bar), "solid bars on both axes");
        assert_eq!(content, big, "the whole window is scrollable");
    }

    #[test]
    fn panel_bars_only_show_for_a_window_bigger_than_its_dock() {
        use egui::scroll_area::ScrollBarVisibility::{AlwaysHidden, VisibleWhenNeeded};
        assert_eq!(panel_bars(DOCK, DOCK), AlwaysHidden);
        assert_eq!(panel_bars(vec2(299.4, 199.4), DOCK), AlwaysHidden);
        assert_eq!(panel_bars(DOCK, DOCK + vec2(1.0, 0.0)), VisibleWhenNeeded);
        assert_eq!(panel_bars(DOCK, DOCK + vec2(0.0, 1.0)), VisibleWhenNeeded);
    }

    /// Script colours are sRGB, not linear: `#2B2B2B` drew as RGB 114 when read as linear light.
    #[test]
    fn draw_colours_are_srgb() {
        let v = 43.0 / 255.0;
        assert_eq!(draw_color(&Some([v, v, v, 1.0]), Color32::RED), Color32::from_rgb(43, 43, 43));
        let orange = [242.0 / 255.0, 140.0 / 255.0, 40.0 / 255.0, 1.0];
        assert_eq!(draw_color(&Some(orange), Color32::RED), Color32::from_rgb(242, 140, 40));
        assert_eq!(draw_color(&None, Color32::RED), Color32::RED, "no colour is the theme colour");
    }

    /// fillPath, strokePath and drawString all paint the script's colour as sRGB.
    #[test]
    fn paint_draw_paints_srgb_colours() {
        let v = 43.0 / 255.0;
        let color = Some([v, v, v, 1.0]);
        let path = vec![PathSeg::R { x: 0.0, y: 0.0, w: 10.0, h: 10.0 }];
        let ops = [
            DrawOp::Fill { color, path: path.clone() },
            DrawOp::Stroke { color, width: 1.0, path },
            DrawOp::Text { text: "Hi".into(), color, x: 0.0, y: 0.0, size: 12.0, style: String::new() },
        ];
        let t = Tokens::for_kind(crate::theme::ThemeKind::Dark);
        let mut out = egui::Context::default().run_ui(egui::RawInput::default(), |ui| {
            paint_draw(ui, Rect::from_min_size(pos2(0.0, 0.0), vec2(50.0, 50.0)), &ops, &t, &effectcraft_engine::FsServices);
        });
        out.textures_delta.clear();
        let want = Color32::from_rgb(43, 43, 43);
        let (mut fill, mut stroke, mut text) = (false, false, false);
        for s in out.shapes {
            match s.shape {
                egui::Shape::Mesh(m) => fill |= m.vertices.iter().all(|v| v.color == want),
                egui::Shape::Path(p) => stroke |= matches!(p.stroke.color, egui::epaint::ColorMode::Solid(c) if c == want),
                egui::Shape::Text(t) => text |= t.fallback_color == want,
                _ => {}
            }
        }
        assert!(fill && stroke && text, "fill {fill}, stroke {stroke}, text {text}");
    }
}
