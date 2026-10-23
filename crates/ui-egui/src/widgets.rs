//! Small custom widgets in the motion-graphics app visual language.

use egui::{Align2, Color32, Rect, Response, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

use crate::automation::Registry;
use crate::icons::{self, Icon};
use crate::theme::Tokens;

/// The [`egui::UiStackInfo`] tag of a dock panel's `Ui`: Tab moves between the hot-text fields
/// of one panel.
pub const PANEL_TAG: &str = "effectcraft.panel";

/// A panel's hot-text fields in drawing order (this pass's and the last one's), and a Tab
/// pressed while typing in one: (that field, Shift held, the pass).
#[derive(Clone, Default)]
struct TabOrder {
    pass: u64,
    current: Vec<egui::Id>,
    last: Vec<egui::Id>,
    pending: Option<(egui::Id, bool, u64)>,
}

fn tab_order_id(ui: &Ui) -> egui::Id {
    let panel = ui.stack().iter().find(|s| s.tags().contains(PANEL_TAG)).map_or(egui::Id::NULL, |s| s.id);
    egui::Id::new(("hot-tab-order", panel))
}

/// Puts hot-text field `id` in its panel's Tab order. True when a Tab in the field before it
/// (Shift+Tab: after it) moves the typing here.
fn tab_stop(ui: &Ui, id: egui::Id) -> bool {
    let key = tab_order_id(ui);
    let pass = ui.ctx().cumulative_pass_nr();
    ui.data_mut(|d| {
        let o = d.get_temp_mut_or_default::<TabOrder>(key);
        if o.pass != pass {
            o.last = std::mem::take(&mut o.current);
            o.pass = pass;
        }
        o.current.push(id);
        let Some((from, back, at)) = o.pending else { return false };
        let i = o.last.iter().position(|f| *f == from);
        let next = i.and_then(|i| if back { i.checked_sub(1) } else { i.checked_add(1) }).and_then(|i| o.last.get(i)).copied();
        if next == Some(id) || next.is_none() || pass > at.saturating_add(2) {
            o.pending = None;
        }
        next == Some(id)
    })
}

/// "Hot text": a blue number you drag horizontally to scrub, click to type. Tab / Shift+Tab
/// while typing commits and types in the panel's next / previous one, as in After Effects.
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
    let mut editing: Option<String> = ui.data(|d| d.get_temp(editing_id));
    if tab_stop(ui, id) && editing.is_none() {
        editing = Some(format!("{value:.decimals$}"));
    }
    let text = format!("{value:.decimals$}{suffix}");
    let galley = ui.painter().layout_no_wrap(text, Tokens::ui(12.0), t.hot_text);
    let rect = Rect::from_min_size(rect_min, galley.size() + vec2(4.0, 4.0));
    if let Some(mut buf) = editing {
        let er = Rect::from_min_size(rect_min - vec2(2.0, 1.0), vec2((galley.size().x + 24.0).max(54.0), 18.0));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(er));
        // (Tab stays with the field: it moves to the next hot-text field, not the next widget.)
        let r = child.add(
            egui::TextEdit::singleline(&mut buf)
                .id(id.with("text"))
                .desired_width(er.width())
                .font(Tokens::ui(12.0))
                .margin(egui::Margin::symmetric(2, 1))
                .lock_focus(true),
        );
        let tab = r.has_focus().then(|| ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Tab).then_some(i.modifiers.shift))).flatten();
        let mut out = None;
        if r.lost_focus() || tab.is_some() {
            let txt = buf.trim().trim_end_matches(suffix).trim().replace(',', ".");
            if !ui.input(|i| i.key_pressed(egui::Key::Escape))
                && let Ok(v) = txt.parse::<f64>()
                && v.is_finite()
            {
                out = Some(v.clamp(range.0, range.1));
            }
            ui.data_mut(|d| d.remove::<String>(editing_id));
            if let Some(back) = tab {
                r.surrender_focus();
                let (key, pass) = (tab_order_id(ui), ui.ctx().cumulative_pass_nr());
                ui.data_mut(|d| d.get_temp_mut_or_default::<TabOrder>(key).pending = Some((id, back, pass)));
            }
        } else {
            keep_focus(ui.ctx(), &r, &buf);
            ui.data_mut(|d| d.insert_temp(editing_id, buf));
        }
        return (er, out, out.is_some());
    }
    let resp = ui.interact(rect, id, Sense::click_and_drag());
    crate::pointer_wrap::track(&resp);
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
            } else if i.modifiers.command && !proportional_scrub(i.modifiers) {
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

/// Ctrl+Alt (Cmd+Opt on macOS) held while scrubbing hot text: After Effects' Proportional
/// Scrubbing, which spreads the change over the selected layers by selection order. Ctrl alone
/// still scrubs finely and Shift by tens.
pub fn proportional_scrub(m: egui::Modifiers) -> bool {
    m.command && m.alt
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

/// Standard numeric editors use the same continuous native scrubbing as hot text.
pub fn drag_value(value: egui::DragValue<'_>) -> impl egui::Widget {
    move |ui: &mut Ui| {
        let response = ui.add(value);
        crate::pointer_wrap::track(&response);
        response
    }
}

/// Keep the keyboard on an inline editor that is open until it loses the focus. The frame it
/// opens its whole `text` is selected, so typing replaces the value; after that the caret is the
/// user's (arrow keys move it), even if the field is without the focus for a frame (#488).
/// (The focus is only requested while the field doesn't have it: a request resets what keys the
/// field keeps for itself, such as Tab with [`egui::TextEdit::lock_focus`].)
pub fn keep_focus(ctx: &egui::Context, field: &Response, text: &str) {
    // The editor is open in consecutive passes; a gap means it was closed and opened again.
    let shown = field.id.with("keep-focus-pass");
    let pass = ctx.cumulative_pass_nr();
    let previous = ctx.data_mut(|d| {
        let previous = d.get_temp::<u64>(shown);
        d.insert_temp(shown, pass);
        previous
    });
    let opening = previous.is_none_or(|p| p.saturating_add(1) < pass);
    if !ctx.memory(|m| m.has_focus(field.id)) {
        if opening {
            select_all(ctx, field.id, text);
        }
        field.request_focus();
    }
}

/// Put the keyboard on a dialog's first text field the frame the dialog opens, with its whole
/// `text` selected, so typing replaces the value without a click. Unlike [`keep_focus`] the
/// focus is the user's afterwards.
pub fn focus_on_open(ctx: &egui::Context, field: &Response, text: &str) {
    // The dialog is open in consecutive passes; a gap means it was closed and opened again.
    let shown = field.id.with("focus-on-open-pass");
    let pass = ctx.cumulative_pass_nr();
    let previous = ctx.data_mut(|d| {
        let previous = d.get_temp::<u64>(shown);
        d.insert_temp(shown, pass);
        previous
    });
    if previous.is_none_or(|p| p.saturating_add(1) < pass) {
        select_all(ctx, field.id, text);
        field.request_focus();
    }
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
    ui.painter().rect_filled(rect, 2.0, col);
    ui.painter().rect_stroke(rect, 2.0, Stroke::new(1.0, if resp.hovered() { t.text } else { t.field_border }), StrokeKind::Inside);
    resp
}

// The colour picker and its helpers (every colour input uses them).
pub use crate::color_picker::{color_popup, hex_rgb, hsv_to_rgb, rgb_to_hsv, srgb_color_button, srgb_hex, srgb32};

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

/// A flat icon toggle inside a rect (`sense`: click, or click and drag for a switch dragged
/// over several rows).
pub fn icon_toggle(ui: &mut Ui, rect: Rect, icon: Icon, on: bool, t: &Tokens, id: egui::Id, sense: Sense) -> Response {
    let resp = ui.interact(rect, id, sense);
    if resp.hovered() {
        ui.painter().rect_filled(rect, 2.0, t.hover);
    }
    if on || resp.hovered() {
        icons::paint(ui.painter(), rect.shrink(rect.width() * 0.18), icon, if on { t.icon } else { t.text_faint });
    }
    resp
}

/// Icon button (toolbar style) with optional active state.
pub fn icon_button(ui: &mut Ui, rect: Rect, icon: Icon, active: bool, t: &Tokens, id: egui::Id) -> Response {
    let resp = ui.interact(rect, id, Sense::click());
    if active {
        ui.painter().rect_filled(rect, 4.0, t.accent);
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, 4.0, t.hover);
    }
    let col = if active {
        Color32::WHITE
    } else if resp.hovered() {
        t.icon_active
    } else {
        t.icon
    };
    icons::paint(ui.painter(), rect.shrink(rect.width() * 0.22), icon, col);
    focus_outline(ui, &resp, t);
    resp
}

/// Keep keyboard focus visible on custom-painted controls, including their active state.
pub fn focus_outline(ui: &Ui, response: &Response, t: &Tokens) {
    if response.has_focus() {
        ui.painter().rect_stroke(response.rect.shrink(1.0), 4.0, Stroke::new(2.0, t.text), StrokeKind::Inside);
    }
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

/// Rounded search field with a magnifier icon.
pub fn search_field(ui: &mut Ui, rect: Rect, text: &mut String, hint: &str, t: &Tokens) -> Response {
    field(ui, rect, text, hint, t, true)
}

/// Rounded text field.
pub fn text_field(ui: &mut Ui, rect: Rect, text: &mut String, hint: &str, t: &Tokens) -> Response {
    field(ui, rect, text, hint, t, false)
}

fn field(ui: &mut Ui, rect: Rect, text: &mut String, hint: &str, t: &Tokens, search: bool) -> Response {
    let h = rect.height();
    ui.painter().rect_filled(rect, h / 2.0, t.field_bg);
    ui.painter().rect_stroke(rect, h / 2.0, Stroke::new(1.0, t.field_border), StrokeKind::Inside);
    if search {
        icons::paint(ui.painter(), Rect::from_center_size(pos2(rect.min.x + 12.0, rect.center().y), vec2(12.0, 12.0)), Icon::Search, t.text_dim);
    }
    let inner = Rect::from_min_max(pos2(rect.min.x + if search { 22.0 } else { 9.0 }, rect.min.y + 1.0), pos2(rect.max.x - 8.0, rect.max.y - 1.0));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner));
    child.add(
        egui::TextEdit::singleline(text)
            .hint_text(hint)
            .frame(egui::Frame::NONE)
            .desired_width(inner.width())
            // As tall as the field, so the text and the hint sit on its middle (they sat high, #581).
            .min_size(inner.size())
            .font(Tokens::ui(12.0))
            .vertical_align(egui::Align::Center),
    )
}

/// Dropdown-looking control ("Full ▾").
pub fn dropdown(ui: &mut Ui, rect: Rect, text: &str, t: &Tokens, id: egui::Id) -> Response {
    dropdown_tip(ui, rect, text, None, t, id)
}

/// [`dropdown`] with a tooltip `tip` (shown always; it replaces the elided text's tooltip).
pub fn dropdown_tip(ui: &mut Ui, rect: Rect, text: &str, tip: Option<&str>, t: &Tokens, id: egui::Id) -> Response {
    let resp = ui.interact(rect, id, Sense::click());
    ui.painter().rect_filled(rect, 3.0, if resp.hovered() { t.hover } else { t.field_bg });
    ui.painter().rect_stroke(rect, 3.0, Stroke::new(1.0, t.field_border), StrokeKind::Inside);
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
    if let Some(tip) = tip {
        return resp.on_hover_text(tip.to_string());
    }
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
    ui.painter().rect_filled(rect, rect.height() / 2.0, bg);
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, text, Tokens::medium(12.0), if primary { Color32::WHITE } else { t.text });
    resp
}

/// Checkbox in the AE look (small square).
pub fn checkbox(ui: &mut Ui, rect: Rect, on: bool, t: &Tokens, id: egui::Id) -> Response {
    let resp = ui.interact(rect, id, Sense::click());
    let b = Rect::from_center_size(rect.center(), vec2(12.0, 12.0));
    ui.painter().rect_filled(b, 2.0, if on { t.accent } else { t.field_bg });
    ui.painter().rect_stroke(b, 2.0, Stroke::new(1.0, if on { t.accent } else { t.field_border }), StrokeKind::Inside);
    if on {
        let p = |x: f32, y: f32| pos2(b.min.x + x * b.width(), b.min.y + y * b.height());
        ui.painter().line(vec![p(0.2, 0.52), p(0.42, 0.74), p(0.8, 0.28)], Stroke::new(1.6, Color32::WHITE));
    }
    resp
}

/// A scroll bar along `track` (vertical when it is taller than wide) for content scrolled
/// `scroll` of `max`, registered for automation as `key` ("scroll/max"): drag the thumb, or
/// press the track beside it to centre the thumb there. The thumb's length is the share in view
/// (the view is about as long as its track), at least 16 points but never longer than the track
/// (a panel only a few points tall still draws, #231). Returns the new scroll.
pub fn scroll_bar(ui: &mut Ui, auto: &mut Registry, key: &str, track: Rect, scroll: f32, max: f32, t: &Tokens) -> f32 {
    if !track.is_positive() {
        // (A panel too short for its bar.)
        return scroll.clamp(0.0, max.max(0.0));
    }
    let vertical = track.height() >= track.width();
    let a = usize::from(vertical);
    let len = track.size()[a];
    let thumb_len = (len * len / (len + max.max(0.0)).max(1.0)).max(16.0).min(len).max(0.0);
    let travel = (len - thumb_len).max(1.0);
    let thumb_at = |s: f32| track.min[a] + (len - thumb_len) * if max > 0.0 { (s / max).clamp(0.0, 1.0) } else { 0.0 };
    let resp = ui.interact(track.expand2(if vertical { vec2(3.0, 0.0) } else { vec2(0.0, 3.0) }), egui::Id::new(key), Sense::drag());
    let pressed = resp.is_pointer_button_down_on() && ui.input(|i| i.pointer.any_pressed());
    let mut s = scroll;
    if let Some(p) = resp.interact_pointer_pos().filter(|p| pressed && !(thumb_at(scroll)..=thumb_at(scroll) + thumb_len).contains(&p[a])) {
        s = (p[a] - thumb_len / 2.0 - track.min[a]) * max / travel;
    } else if resp.dragged() {
        s += resp.drag_delta()[a] * max / travel;
    }
    let s = s.clamp(0.0, max.max(0.0));
    let thumb = if vertical {
        Rect::from_min_size(pos2(track.min.x, thumb_at(s)), vec2(track.width(), thumb_len))
    } else {
        Rect::from_min_size(pos2(thumb_at(s), track.min.y), vec2(thumb_len, track.height()))
    };
    ui.painter().rect_filled(track, 2.0, t.field_bg);
    ui.painter().rect_filled(thumb, 2.0, if resp.hovered() || resp.dragged() { t.text_dim } else { t.text_faint });
    auto.add(key, track, &format!("{s}/{max}"));
    if s != scroll {
        ui.ctx().request_repaint();
    }
    s
}

/// Vertical scrolling for a panel that lays out its own rows: [`PanelScroll::begin`] gives the
/// offset to draw them at (moved by the mouse wheel over `area`), [`PanelScroll::end`] takes the
/// rows' height and draws a [`scroll_bar`] on the right edge while they overflow, so every
/// panel can be scrolled without a wheel or touchpad (#271).
pub struct PanelScroll {
    id: egui::Id,
    area: Rect,
    /// How far the rows are scrolled up.
    pub offset: f32,
}

impl PanelScroll {
    pub fn begin(ui: &Ui, id: egui::Id, area: Rect) -> Self {
        // (Clamped to last frame's overflow: the rows' height is known once they are drawn.)
        let (mut offset, max): (f32, f32) = ui.data(|d| d.get_temp(id)).unwrap_or_default();
        if ui.rect_contains_pointer(area) {
            offset -= ui.input(|i| i.smooth_scroll_delta.y);
        }
        Self { id, area, offset: offset.clamp(0.0, max.max(0.0)) }
    }

    /// The rows were `content` points tall: keep the offset, with the scroll bar registered for
    /// automation as `key`.
    pub fn end(self, ui: &mut Ui, auto: &mut Registry, key: &str, content: f32, t: &Tokens) {
        let max = (content - self.area.height()).max(0.0);
        let mut offset = self.offset.min(max);
        if max > 0.0 {
            let track = Rect::from_min_max(pos2(self.area.max.x - 6.0, self.area.min.y + 1.0), pos2(self.area.max.x - 2.0, self.area.max.y - 1.0));
            offset = scroll_bar(ui, auto, key, track, offset, max, t);
        }
        ui.data_mut(|d| d.insert_temp(self.id, (offset, max)));
    }
}

/// A pointer press this frame landed outside a popup `Area` (its `show` response). egui
/// hit-tests a fixed-position area where it was asked to go, not where it was moved to fit the
/// window, so `contains_pointer` / `hovered` are false over a menu shifted up from the bottom of
/// the window and the menu would close on the press before its entry is clicked (#117). The
/// area's response rect is where it was drawn.
pub fn pressed_outside(ctx: &egui::Context, area: &Response) -> bool {
    ctx.input(|i| i.pointer.any_pressed() && !i.pointer.interact_pos().is_some_and(|p| area.rect.contains(p)))
}

/// A [`popup_menu`] option with a check mark when `on` (the labels line up either way).
pub fn check_label(on: bool, label: &str) -> String {
    if on { format!("✓ {label}") } else { format!("   {label}") }
}

fn label_swatch_id() -> egui::Id {
    egui::Id::new("label-swatch")
}

/// The colour swatch of a Label menu row, as an atom for its button: Label menus show each
/// label's colour before its name (After Effects), so a label is picked by its colour. Paint it
/// with [`paint_label_swatch`] once the button is laid out.
pub fn label_swatch() -> egui::Atom<'static> {
    egui::Atom::custom(label_swatch_id(), vec2(12.0, 12.0))
}

/// Paint the [`label_swatch`] of a laid-out button: label `l`'s colour, an empty box for None.
pub fn paint_label_swatch(ui: &Ui, out: &egui::AtomLayoutResponse, l: effectcraft_engine::color::Label, t: &Tokens) {
    let Some(r) = out.rect(label_swatch_id()) else { return };
    let r = Rect::from_center_size(r.center(), vec2(11.0, 11.0));
    if l == effectcraft_engine::color::Label::None {
        ui.painter().rect_stroke(r, 2.0, Stroke::new(1.0, t.text_dim), StrokeKind::Inside);
    } else {
        ui.painter().rect_filled(r, 2.0, t.label(l));
    }
}

/// A Label menu row: label `l`'s swatch then `name`, highlighted when it is the `current` label.
pub fn label_entry(ui: &mut Ui, l: effectcraft_engine::color::Label, name: &str, current: bool, t: &Tokens) -> Response {
    let out = egui::Button::selectable(current, (label_swatch(), name)).atom_ui(ui);
    paint_label_swatch(ui, &out, l, t);
    out.response
}

/// The open label menu `id` at `pos` (open it with [`open_popup`]): every label's swatch and its
/// name from Settings ▸ Labels (`name`). Returns the chosen label.
pub fn label_popup(
    ui: &mut Ui,
    id: egui::Id,
    pos: egui::Pos2,
    current: effectcraft_engine::color::Label,
    name: impl Fn(effectcraft_engine::color::Label) -> String,
    t: &Tokens,
) -> Option<effectcraft_engine::color::Label> {
    if !popup_is_open(ui, id) {
        return None;
    }
    let labels = effectcraft_engine::color::Label::ALL;
    popup_list(ui, id, pos, Rect::NOTHING, labels.len(), 0, |ui| {
        ui.set_min_width(160.0);
        let mut chosen = None;
        for l in labels {
            if label_entry(ui, l, &name(l), l == current, t).clicked() {
                chosen = Some(l);
            }
        }
        chosen
    })
}

/// Show a popup menu anchored at `pos` with string options; returns the chosen index.
pub fn popup_menu(ui: &mut Ui, id: egui::Id, pos: egui::Pos2, options: &[String], current: Option<usize>) -> Option<usize> {
    if !popup_is_open(ui, id) {
        return None;
    }
    let seps = options.iter().filter(|o| *o == "-").count();
    popup_list(ui, id, pos, Rect::NOTHING, options.len().saturating_sub(seps), seps, |ui| {
        ui.set_min_width(160.0);
        let mut chosen = None;
        for (i, o) in options.iter().enumerate() {
            if o == "-" {
                ui.separator();
            } else if ui.selectable_label(current == Some(i), o).clicked() {
                chosen = Some(i);
            }
        }
        chosen
    })
}

/// The open popup list `id` at `pos`, drawn by `body` (`rows` entries and `seps` separators,
/// for its height): as tall as the window allows, scrolling beyond with a scroll bar (a shape
/// layer's Add menu has 20 entries, the blend modes 40, the fonts hundreds), and moved up when
/// it would run past the bottom of the window (the Timeline's menus, #269). Closes when `body`
/// returns a choice, on Escape, or on a press outside it and outside `anchor` (a control that
/// toggles the list itself). Returns the choice.
pub fn popup_list<R>(ui: &mut Ui, id: egui::Id, pos: egui::Pos2, anchor: Rect, rows: usize, seps: usize, body: impl FnOnce(&mut Ui) -> Option<R>) -> Option<R> {
    let screen = ui.ctx().content_rect();
    let max_h = (screen.height() - 24.0).max(120.0);
    let sp = ui.spacing();
    let est = (rows as f32 * (sp.interact_size.y + sp.item_spacing.y + 4.0) + seps as f32 * (2.0 * sp.item_spacing.y + 2.0) + 16.0).min(max_h + 12.0);
    let pos = egui::pos2(pos.x, pos.y.min(screen.bottom() - est).max(screen.top()));
    let mut chosen = None;
    let area = egui::Area::new(id.with("area")).order(egui::Order::Foreground).fixed_pos(pos).show(ui.ctx(), |ui| {
        egui::Frame::popup(ui.style()).show(ui, |ui| {
            // (An area's content is laid out in last frame's size: ask for the estimate.)
            egui::ScrollArea::vertical().max_height(max_h).min_scrolled_height((est - 16.0).min(max_h)).show(ui, |ui| chosen = body(ui));
        });
    });
    note_open_popup_list(ui.ctx());
    let outside = pressed_outside(ui.ctx(), &area.response) && !ui.input(|i| i.pointer.interact_pos()).is_some_and(|p| anchor.contains(p));
    if chosen.is_some() || outside || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        ui.data_mut(|d| d.insert_temp(id.with("open"), false));
    }
    chosen
}

/// Custom popup lists also hold the app's keyboard shortcuts while open.
pub(crate) fn note_open_popup_list(ctx: &egui::Context) {
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|d| d.insert_temp(popup_list_pass_id(), pass));
}

/// The pass a [`popup_list`] was last drawn in.
fn popup_list_pass_id() -> egui::Id {
    egui::Id::new("popup-list-pass")
}

/// Whether a menu, context menu or popup list is open, or the menu bar has the keyboard (as of
/// the last frame): the app's keyboard shortcuts wait until it closes (#279).
pub fn any_menu_open(ctx: &egui::Context) -> bool {
    let list_pass: Option<u64> = ctx.data(|d| d.get_temp(popup_list_pass_id()));
    egui::Popup::is_any_open(ctx) || crate::menu_keys::active(ctx) || list_pass.is_some_and(|p| p + 1 >= ctx.cumulative_pass_nr())
}

/// A menu's (or submenu's) entries, scrolling with a scroll bar when they are taller than the
/// window instead of running off it (Edit on a short screen, #269; Blending Mode). Others show
/// whole: egui sizes a new menu from a default 400 pt area, so without a minimum a longer one
/// (Window ▸ Workspace) got stuck at that height with its last entries scrolled out of view
/// (#191).
pub fn menu_scroll<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let max_h = ui.ctx().content_rect().height() - 40.0;
    egui::ScrollArea::vertical().max_height(max_h).min_scrolled_height(max_h).show(ui, add).inner
}

/// egui's menu style with no gap between the rows: the gap the app's item spacing left between
/// two entries highlighted neither and swallowed a click there. The rows grow by that gap, so a
/// menu keeps its size.
pub fn menu_style(style: &mut egui::Style) {
    egui::containers::menu::menu_style(style);
    style.spacing.interact_size.y += style.spacing.item_spacing.y;
    style.spacing.item_spacing.y = 0.0;
}

/// `resp`'s context menu in the app's [`menu_style`] (its submenus inherit it).
pub fn context_menu(resp: &Response, add_contents: impl FnOnce(&mut Ui)) -> Option<egui::InnerResponse<()>> {
    egui::Popup::context_menu(resp).style(menu_style).show(add_contents)
}

/// A button opening a menu in the app's [`menu_style`], for one outside the menu bar.
pub fn menu_button<'a, R>(ui: &mut Ui, atoms: impl egui::IntoAtoms<'a>, add_contents: impl FnOnce(&mut Ui) -> R) -> egui::InnerResponse<Option<R>> {
    let config = egui::containers::menu::MenuConfig::new().style(menu_style);
    let (response, inner) = egui::containers::menu::MenuButton::new(atoms).config(config).ui(ui, add_contents);
    egui::InnerResponse::new(inner.map(|i| i.inner), response)
}

/// Whether a [`popup_menu`] is open (build its options only then: they can be long).
pub fn popup_is_open(ui: &Ui, id: egui::Id) -> bool {
    ui.data(|d| d.get_temp(id.with("open")).unwrap_or(false))
}

pub fn open_popup(ui: &Ui, id: egui::Id) {
    ui.data_mut(|d| d.insert_temp(id.with("open"), true));
}

/// Linked values after component `d` of `c` became `v`: the others scale by the same ratio
/// (Constrain Proportions); from 0 the others that are 0 too follow the value.
pub(crate) fn constrained(c: &[f64], d: usize, v: f64) -> Vec<f64> {
    let Some(&old) = c.get(d) else { return c.to_vec() };
    c.iter()
        .enumerate()
        .map(|(e, &x)| {
            if e == d {
                v
            } else if old.abs() > 1e-9 {
                x * v / old
            } else if x.abs() <= 1e-9 {
                v
            } else {
                x
            }
        })
        .collect()
}

/// Applies a click on a Constrain Proportions chain to the shared unlinked set and returns whether
/// the property is linked afterwards, so the icon can be painted in its new state in the same frame.
pub(crate) fn toggle_link(unlinked: &mut std::collections::BTreeSet<u64>, uid: u64, clicked: bool) -> bool {
    if clicked && !unlinked.remove(&uid) {
        unlinked.insert(uid);
    }
    !unlinked.contains(&uid)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #2243: a click on a Constrain Proportions chain flips its state and the returned state is
    /// the new one, so the icon is painted right away instead of after the next hover.
    #[test]
    fn toggle_link_returns_the_state_after_the_click() {
        let mut unlinked = std::collections::BTreeSet::new();
        assert!(toggle_link(&mut unlinked, 7, false), "linked by default");
        assert!(!toggle_link(&mut unlinked, 7, true), "a click unlinks, and the result already says so");
        assert!(unlinked.contains(&7));
        assert!(!toggle_link(&mut unlinked, 7, false), "stays unlinked without a click");
        assert!(toggle_link(&mut unlinked, 7, true), "a click links again");
        assert!(unlinked.is_empty());
    }

    /// #488: an inline editor selects its whole text only as it opens. Should it be without the
    /// keyboard for a frame later (an input method, another handler), it takes the keyboard back
    /// with the caret where the user put it instead of selecting everything again, which made
    /// the arrows seem dead and Backspace erase the whole name.
    #[test]
    fn inline_editors_select_all_only_as_they_open() {
        let ctx = egui::Context::default();
        let mut text = String::from("Comp 1");
        let mut id = egui::Id::NULL;
        let frame = |ctx: &egui::Context, text: &mut String, id: &mut egui::Id| {
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                let r = ui.add(egui::TextEdit::singleline(text));
                keep_focus(ui.ctx(), &r, text);
                *id = r.id;
            });
            out.textures_delta.clear();
        };
        let range = |ctx: &egui::Context, id: egui::Id| egui::TextEdit::load_state(ctx, id).and_then(|s| s.cursor.char_range());
        let ix = |n: usize| egui::text::CCursor::new(n).index;
        frame(&ctx, &mut text, &mut id);
        frame(&ctx, &mut text, &mut id);
        let all = range(&ctx, id).unwrap();
        assert_eq!((all.primary.index.min(all.secondary.index), all.primary.index.max(all.secondary.index)), (ix(0), ix(6)), "opened with all selected");
        // The caret after "Co", then a frame without the keyboard.
        let mut state = egui::TextEdit::load_state(&ctx, id).unwrap();
        state.cursor.set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(2))));
        state.store(&ctx, id);
        ctx.memory_mut(|m| m.surrender_focus(id));
        frame(&ctx, &mut text, &mut id);
        frame(&ctx, &mut text, &mut id);
        assert!(ctx.memory(|m| m.has_focus(id)), "the editor keeps the keyboard");
        let r = range(&ctx, id).unwrap();
        assert_eq!((r.primary.index, r.secondary.index), (ix(2), ix(2)), "the caret stays where it was");
        // Closed for a frame and opened again: all selected again.
        ctx.run_ui(egui::RawInput::default(), |_| {}).textures_delta.clear();
        frame(&ctx, &mut text, &mut id);
        frame(&ctx, &mut text, &mut id);
        let r = range(&ctx, id).unwrap();
        assert_eq!((r.primary.index.min(r.secondary.index), r.primary.index.max(r.secondary.index)), (ix(0), ix(6)));
    }

    /// #581: a field's text edit fills it, so its text and hint sit on the field's middle line
    /// (it was a row at the top of the field, its text high).
    #[test]
    fn field_text_is_vertically_centred() {
        let ctx = egui::Context::default();
        let t = Tokens::for_kind(Default::default());
        for h in [22.0, 26.0, 30.0] {
            let field = Rect::from_min_size(pos2(20.0, 40.0), vec2(220.0, h));
            let mut text = String::new();
            let mut got = None;
            let mut out = ctx.run_ui(Default::default(), |ui| {
                got = Some(search_field(ui, field, &mut text, "Filter recent files", &t).rect);
            });
            out.textures_delta.clear();
            let r = got.expect("the field was drawn");
            assert!((r.center().y - field.center().y).abs() < 0.5 && r.height() >= h - 2.5, "{h}: {r:?} in {field:?}");
        }
    }
}
