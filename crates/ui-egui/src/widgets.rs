//! Small custom widgets in the motion-graphics app visual language.

use egui::{Align2, Color32, Rect, Response, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

use crate::icons::{self, Icon};
use crate::theme::Tokens;

/// "Hot text": a blue number you drag horizontally to scrub, click to type.
/// Returns (response, Some(new value) when changed, drag finished).
pub fn hot_number_at(
    ui: &mut Ui,
    rect_min: egui::Pos2,
    id: egui::Id,
    value: f64,
    speed: f64,
    range: (f64, f64),
    decimals: usize,
    suffix: &str,
    t: &Tokens,
) -> (Rect, Option<f64>, bool) {
    let editing_id = id.with("editing");
    let editing: Option<String> = ui.data(|d| d.get_temp(editing_id));
    let text = format!("{value:.decimals$}{suffix}");
    let galley = ui.painter().layout_no_wrap(text, Tokens::ui(12.0), t.hot_text);
    let rect = Rect::from_min_size(rect_min, galley.size() + vec2(4.0, 4.0));
    if let Some(mut buf) = editing {
        let er = Rect::from_min_size(rect_min - vec2(2.0, 1.0), vec2((galley.size().x + 24.0).max(54.0), 18.0));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(er));
        let r = child.add(egui::TextEdit::singleline(&mut buf).desired_width(er.width()).font(Tokens::ui(12.0)).margin(egui::Margin::symmetric(2, 1)));
        let mut out = None;
        if r.lost_focus() {
            let txt = buf.trim().trim_end_matches(suffix).trim().replace(',', ".");
            if !ui.input(|i| i.key_pressed(egui::Key::Escape))
                && let Ok(v) = txt.parse::<f64>()
                && v.is_finite()
            {
                out = Some(v.clamp(range.0, range.1));
            }
            ui.data_mut(|d| d.remove::<String>(editing_id));
        } else {
            keep_focus(ui.ctx(), &r, &buf);
            ui.data_mut(|d| d.insert_temp(editing_id, buf));
        }
        return (er, out, out.is_some());
    }
    let resp = ui.interact(rect, id, Sense::click_and_drag());
    let col = if resp.hovered() || resp.dragged() { t.accent_hover } else { t.hot_text };
    ui.painter().galley_with_override_text_color(rect.min + vec2(2.0, 2.0), galley, col);
    if resp.hovered() || resp.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }
    let y = rect.max.y - 2.0;
    ui.painter().line_segment(
        [pos2(rect.min.x + 2.0, y), pos2(rect.max.x - 2.0, y)],
        egui::Stroke::new(1.0, col.gamma_multiply(if resp.hovered() { 0.8 } else { 0.35 })),
    );
    let mut out = None;
    if resp.dragged() {
        let dx = resp.drag_delta().x as f64;
        let mult = ui.input(|i| {
            if i.modifiers.shift {
                10.0
            } else if i.modifiers.command {
                0.1
            } else {
                1.0
            }
        });
        let nv = (value + dx * speed * mult).clamp(range.0, range.1);
        if (nv - value).abs() > f64::EPSILON {
            out = Some(nv);
        }
    }
    if resp.clicked() {
        ui.data_mut(|d| d.insert_temp(editing_id, format!("{value:.decimals$}")));
    }
    (rect, out, resp.drag_stopped())
}

/// [`hot_number_at`] for a whole number (an angle's revolutions): a drag steps it once every
/// `1 / speed` pixels however slowly the pointer moves (the fractions add up between frames),
/// and a typed value is truncated. Returns (rect, Some(new value) when changed, drag finished).
pub fn hot_int_at(ui: &mut Ui, rect_min: egui::Pos2, id: egui::Id, value: i64, speed: f64, suffix: &str, t: &Tokens) -> (Rect, Option<i64>, bool) {
    let (rect, nv, done) = hot_number_at(ui, rect_min, id, value as f64, speed, (-1e6, 1e6), 0, suffix, t);
    let acc_id = id.with("acc");
    let acc = ui.data(|d| d.get_temp::<f64>(acc_id)).unwrap_or(0.0) + nv.map_or(0.0, |n| n - value as f64);
    // (Ten 0.1 steps add up to just under 1.)
    let step = (acc + acc.signum() * 1e-9).trunc();
    if ui.ctx().is_being_dragged(id) && !done {
        ui.data_mut(|d| d.insert_temp(acc_id, acc - step));
    } else {
        ui.data_mut(|d| d.remove::<f64>(acc_id));
    }
    (rect, (step != 0.0).then(|| value.saturating_add(step as i64)), done)
}

/// Keep the keyboard on an inline editor that is open until it loses the focus. The frame it
/// opens (it has no focus yet) its whole `text` is selected, so typing replaces the value.
pub fn keep_focus(ctx: &egui::Context, field: &Response, text: &str) {
    if !ctx.memory(|m| m.has_focus(field.id)) {
        select_all(ctx, field.id, text);
    }
    field.request_focus();
}

/// Select all of the text field `id`'s `text`, so typing replaces it.
pub fn select_all(ctx: &egui::Context, id: egui::Id, text: &str) {
    let mut state = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
    let all = egui::text::CCursorRange::two(egui::text::CCursor::default(), egui::text::CCursor::new(text.chars().count()));
    state.cursor.set_char_range(Some(all));
    state.store(ctx, id);
}

/// A colour swatch; returns the response (click opens the picker in the caller).
pub fn swatch(ui: &mut Ui, rect: Rect, c: [f32; 4], id: egui::Id, t: &Tokens) -> Response {
    let resp = ui.interact(rect, id, Sense::click());
    let col = Color32::from_rgb((c[0].clamp(0.0, 1.0) * 255.0) as u8, (c[1].clamp(0.0, 1.0) * 255.0) as u8, (c[2].clamp(0.0, 1.0) * 255.0) as u8);
    ui.painter().rect_filled(rect, t.radius_sm, col);
    ui.painter().rect_stroke(rect, t.radius_sm, Stroke::new(1.0, if resp.hovered() { t.text } else { t.field_border }), StrokeKind::Inside);
    resp
}

/// A colour button with egui's picker for a colour stored as sRGB components (0–1), as the
/// project stores colours (`color_edit_button_rgb` reads linear values and shows them lighter).
/// The value is only rewritten when the user picks a colour.
pub fn srgb_color_button(ui: &mut Ui, c: &mut [f32; 3]) -> Response {
    let mut u = c.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
    let r = ui.color_edit_button_srgb(&mut u);
    if r.changed() {
        *c = u.map(|v| v as f32 / 255.0);
    }
    r
}

/// A twirl-down triangle; returns clicked.
pub fn twirl(ui: &mut Ui, rect: Rect, open: bool, id: egui::Id, t: &Tokens) -> Response {
    let resp = ui.interact(rect, id, Sense::click());
    let col = if resp.hovered() { t.tab_text_active } else { t.text_dim };
    // An open chevron (> / v), as After Effects draws its twirl-downs.
    let c = rect.center();
    let k = rect.width().min(rect.height()) * 0.2;
    let pts = if open {
        vec![c + vec2(-1.6 * k, -0.8 * k), c + vec2(0.0, 0.8 * k), c + vec2(1.6 * k, -0.8 * k)]
    } else {
        vec![c + vec2(-0.8 * k, -1.6 * k), c + vec2(0.8 * k, 0.0), c + vec2(-0.8 * k, 1.6 * k)]
    };
    ui.painter().add(egui::Shape::line(pts, Stroke::new(1.4, col)));
    resp
}

/// A flat icon toggle inside a rect.
pub fn icon_toggle(ui: &mut Ui, rect: Rect, icon: Icon, on: bool, t: &Tokens, id: egui::Id, on_color: Option<Color32>) -> Response {
    let resp = ui.interact(rect, id, Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, t.radius_sm, t.hover);
    }
    let col = if on { on_color.unwrap_or(t.icon) } else { t.text_faint.gamma_multiply(0.7) };
    if on || resp.hovered() {
        icons::paint(ui.painter(), rect.shrink(rect.width() * 0.18), icon, if !on { t.text_faint } else { col });
    }
    resp
}

/// Icon button (toolbar style) with optional active state.
pub fn icon_button(ui: &mut Ui, rect: Rect, icon: Icon, active: bool, t: &Tokens, id: egui::Id) -> Response {
    let resp = ui.interact(rect, id, Sense::click());
    if active {
        ui.painter().rect_filled(rect, t.radius_sm, t.accent);
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, t.radius_sm, t.hover);
    }
    let col = if active || resp.hovered() { t.icon_active } else { t.icon };
    icons::paint(ui.painter(), rect.shrink(rect.width() * 0.22), icon, col);
    resp
}

/// One line of `text` at `pos` (placed by `align`), cut short with "…" past `max_w` points so it
/// never runs out of its panel. Returns where it was painted.
pub fn text_fit(p: &egui::Painter, pos: egui::Pos2, align: Align2, text: &str, font: egui::FontId, max_w: f32, color: Color32) -> Rect {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = egui::text::TextWrapping { max_width: max_w.max(0.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    let galley = p.layout_job(job);
    let rect = align.anchor_size(pos, galley.size());
    p.galley(rect.min, galley, color);
    rect
}

/// Compact search field with a magnifier icon and visible keyboard focus.
pub fn search_field(ui: &mut Ui, rect: Rect, text: &mut String, hint: &str, t: &Tokens) -> Response {
    ui.painter().rect_filled(rect, t.radius_sm, t.field_bg);
    ui.painter().rect_stroke(rect, t.radius_sm, Stroke::new(1.0, t.field_border), StrokeKind::Inside);
    icons::paint(ui.painter(), Rect::from_center_size(pos2(rect.min.x + 12.0, rect.center().y), vec2(12.0, 12.0)), Icon::Search, t.text_dim);
    let inner = Rect::from_min_max(pos2(rect.min.x + 22.0, rect.min.y + 1.0), pos2(rect.max.x - 8.0, rect.max.y - 1.0));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner));
    let resp = child.add(
        egui::TextEdit::singleline(text)
            .hint_text(hint)
            .frame(egui::Frame::NONE)
            .desired_width(inner.width())
            .font(Tokens::ui(12.0))
            .vertical_align(egui::Align::Center),
    );
    if resp.has_focus() {
        ui.painter().rect_stroke(rect, t.radius_sm, Stroke::new(1.0, t.focus), StrokeKind::Inside);
    }
    resp
}

/// Dropdown-looking control ("Full ▾").
pub fn dropdown(ui: &mut Ui, rect: Rect, text: &str, t: &Tokens, id: egui::Id) -> Response {
    let resp = ui.interact(rect, id, Sense::click());
    ui.painter().rect_filled(rect, t.radius_sm, if resp.hovered() { t.hover } else { t.field_bg });
    ui.painter().rect_stroke(rect, t.radius_sm, Stroke::new(1.0, t.field_border), StrokeKind::Inside);
    // Text that doesn't fit before the arrow is elided (narrow panels).
    let room = rect.width() - 7.0 - 18.0;
    let fits = |s: &str| ui.painter().layout_no_wrap(s.to_string(), Tokens::ui(11.5), t.text).size().x <= room;
    let shown = if fits(text) {
        text.to_string()
    } else {
        let chars: Vec<char> = text.chars().collect();
        let mut n = chars.len();
        loop {
            n = n.saturating_sub(1);
            let s: String = chars[..n].iter().collect::<String>().trim_end().to_string() + "…";
            if n == 0 || fits(&s) {
                break s;
            }
        }
    };
    ui.painter().text(pos2(rect.min.x + 7.0, rect.center().y), Align2::LEFT_CENTER, shown, Tokens::ui(11.5), t.text);
    icons::paint(ui.painter(), Rect::from_center_size(pos2(rect.max.x - 9.0, rect.center().y), vec2(8.0, 8.0)), Icon::ChevronDown, t.text_dim);
    if text.chars().count() > 0 && !fits(text) {
        return resp.on_hover_text(text.to_string());
    }
    resp
}

/// Text button with a subtle fill.
pub fn text_button(ui: &mut Ui, rect: Rect, text: &str, primary: bool, t: &Tokens, id: egui::Id) -> Response {
    let resp = ui.interact(rect, id, Sense::click());
    let bg = if primary {
        if resp.hovered() { t.accent_hover } else { t.accent }
    } else if resp.hovered() {
        t.pressed
    } else {
        t.hover
    };
    ui.painter().rect_filled(rect, t.radius_sm, bg);
    if !primary {
        ui.painter().rect_stroke(rect, t.radius_sm, Stroke::new(1.0, t.field_border), StrokeKind::Inside);
    }
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, text, Tokens::medium(12.0), if primary { Color32::WHITE } else { t.text });
    resp
}

/// Checkbox in the AE look (small square).
pub fn checkbox(ui: &mut Ui, rect: Rect, on: bool, t: &Tokens, id: egui::Id) -> Response {
    let resp = ui.interact(rect, id, Sense::click());
    let b = Rect::from_center_size(rect.center(), vec2(12.0, 12.0));
    ui.painter().rect_filled(b, t.radius_sm, if on { t.accent } else { t.field_bg });
    ui.painter().rect_stroke(b, t.radius_sm, Stroke::new(1.0, if on { t.accent } else { t.field_border }), StrokeKind::Inside);
    if on {
        let p = |x: f32, y: f32| pos2(b.min.x + x * b.width(), b.min.y + y * b.height());
        ui.painter().line(vec![p(0.2, 0.52), p(0.42, 0.74), p(0.8, 0.28)], Stroke::new(1.6, Color32::WHITE));
    }
    resp
}

/// A pointer press this frame landed outside a popup `Area` (its `show` response). egui
/// hit-tests a fixed-position area where it was asked to go, not where it was moved to fit the
/// window, so `contains_pointer` / `hovered` are false over a menu shifted up from the bottom of
/// the window and the menu would close on the press before its entry is clicked (#117). The
/// area's response rect is where it was drawn.
pub fn pressed_outside(ctx: &egui::Context, area: &Response) -> bool {
    ctx.input(|i| i.pointer.any_pressed() && !i.pointer.interact_pos().is_some_and(|p| area.rect.contains(p)))
}

/// Show a popup menu anchored at `pos` with string options; returns the chosen index.
pub fn popup_menu(ui: &mut Ui, id: egui::Id, pos: egui::Pos2, options: &[String], current: Option<usize>) -> Option<usize> {
    let mut chosen = None;
    let open_id = id.with("open");
    let open: bool = ui.data(|d| d.get_temp(open_id).unwrap_or(false));
    if !open {
        return None;
    }
    let area = egui::Area::new(id.with("area")).order(egui::Order::Foreground).fixed_pos(pos).show(ui.ctx(), |ui| {
        egui::Frame::popup(ui.style()).show(ui, |ui| {
            ui.set_min_width(160.0);
            egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                for (i, o) in options.iter().enumerate() {
                    if o == "-" {
                        ui.separator();
                        continue;
                    }
                    let sel = current == Some(i);
                    if ui.selectable_label(sel, o).clicked() {
                        chosen = Some(i);
                    }
                }
            });
        });
    });
    let clicked_outside = pressed_outside(ui.ctx(), &area.response);
    if chosen.is_some() || clicked_outside || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        ui.data_mut(|d| d.insert_temp(open_id, false));
    }
    chosen
}

/// Whether a [`popup_menu`] is open (build its options only then: they can be long).
pub fn popup_is_open(ui: &Ui, id: egui::Id) -> bool {
    ui.data(|d| d.get_temp(id.with("open")).unwrap_or(false))
}

pub fn open_popup(ui: &Ui, id: egui::Id) {
    ui.data_mut(|d| d.insert_temp(id.with("open"), true));
}
