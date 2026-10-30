//! The colour picker every colour input opens, laid out like After Effects' Color Picker: a
//! square and a slider for the channel chosen with the H / S / B / R / G / B buttons, the new
//! colour over the original, the HSB and RGB values, the hex code with an eyedropper that samples
//! the Composition viewer, and OK / Cancel.
//!
//! Colours are sRGB components (0–1), as the project stores them. Changes apply as they are made
//! (After Effects' Preview); Cancel or Escape goes back to the colour the picker opened with.

use egui::{Color32, Rect, Response, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

use crate::i18n::tr;
use crate::widgets::{open_popup, popup_is_open, pressed_outside, select_all};

/// A colour button for egui-laid-out rows and dialogs: a swatch that opens [`color_popup`]. Don't
/// use egui's `color_edit_button_rgb(a)` for project colours: they read linear values, so the
/// swatch shows lighter and a picked colour is stored darker. The value is only rewritten when the
/// user picks a colour.
pub fn srgb_color_button(ui: &mut Ui, c: &mut [f32; 3]) -> Response {
    let (rect, mut resp) = ui.allocate_exact_size(ui.spacing().interact_size, Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ColorWell, ui.is_enabled(), srgb_hex(*c)));
    let stroke = if resp.hovered() { ui.visuals().widgets.hovered.fg_stroke } else { ui.visuals().widgets.inactive.bg_stroke };
    ui.painter().rect_filled(rect, 2.0, srgb32(*c));
    ui.painter().rect_stroke(rect, 2.0, stroke, StrokeKind::Inside);
    if resp.clicked() {
        open_popup(ui, resp.id);
    }
    if color_popup(ui, resp.id, rect.left_bottom(), c) {
        resp.mark_changed();
    }
    resp
}

/// The colour picker as a popup at `pos`, bound to `c`, opened with [`open_popup`]. Returns true
/// when the colour changed (Cancel, Escape and the original swatch change it back; the eyedropper
/// changes it to the colour it samples).
pub fn color_popup(ui: &mut Ui, id: egui::Id, pos: egui::Pos2, c: &mut [f32; 3]) -> bool {
    if !popup_is_open(ui, id) {
        return false;
    }
    let orig_id = id.with("original");
    let original = ui.data_mut(|d| *d.get_temp_mut_or_insert_with(orig_id, || *c));
    // Read before a sample is taken: the press that samples the viewer lands outside the picker, and
    // must not close it.
    let armed = armed_for(ui.ctx(), id);
    let mut sampled = false;
    if let Some(rgb) = take_sample(ui.ctx(), id)
        && rgb != *c
    {
        *c = rgb;
        sampled = true;
    }
    let mut out = Outcome::default();
    let area = egui::Area::new(id.with("area")).order(egui::Order::Foreground).fixed_pos(pos + vec2(0.0, 4.0)).show(ui.ctx(), |ui| {
        egui::Frame::popup(ui.style()).show(ui, |ui| out = color_picker(ui, id, c, original));
    });
    // Escape is the picker's: consume it, so nothing later this pass takes it too. While the
    // eyedropper is armed, Escape cancels it and the picker stays open.
    if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
        if armed {
            disarm(ui.ctx(), id);
        } else {
            out.close = Some(false);
        }
    }
    // A press outside closes the picker, unless it is the armed eyedropper's press on the viewer.
    let on_viewer = armed && eyedropper_point(ui.ctx()).is_some();
    // A press in the pass that first draws the picker must not close it: an Area's rect is only
    // at its requested position from its second pass on (the first pass reports it at the
    // origin), so the click that just opened the picker would be read as a press outside it and
    // the picker would flash and close in that one frame (#1476).
    let pass_id = id.with("open-pass");
    let drawn_before = ui.data(|d| d.get_temp::<u64>(pass_id)).is_some_and(|p| p < ui.ctx().cumulative_pass_nr());
    if drawn_before && pressed_outside(ui.ctx(), &area.response) && !on_viewer {
        out.close.get_or_insert(true);
    }
    note_overlay(ui.ctx(), area.response.rect);
    if let Some(keep) = out.close {
        if !keep && *c != original {
            *c = original;
            out.changed = true;
        }
        disarm(ui.ctx(), id);
        ui.data_mut(|d| {
            d.insert_temp(id.with("open"), false);
            d.remove::<[f32; 3]>(orig_id);
            d.remove::<u64>(pass_id);
            // Closed: the next Escape is the dialog's again.
            d.remove::<u64>(open_pass_id());
        });
    } else {
        refresh_armed(ui.ctx(), id);
        // Still open after this pass: dialogs leave Escape to the picker (see `open_recently`).
        let pass = ui.ctx().cumulative_pass_nr();
        ui.data_mut(|d| {
            d.insert_temp(pass_id, pass);
            d.insert_temp(open_pass_id(), pass);
        });
    }
    out.changed || sampled
}

/// The `egui::Id` of the pass number in which a colour picker last stayed open, which
/// [`open_recently`] uses to keep Escape for the picker rather than the dialog under it.
fn open_pass_id() -> egui::Id {
    egui::Id::unique("color-picker-open-pass")
}

/// A colour picker is open: it was drawn, and stayed open, in this pass or the last one (so one
/// that stops being drawn without closing, its dialog gone, doesn't count for long). Escape then
/// belongs to the picker (it goes back to the original colour and closes), not to the dialog it
/// was opened from. Closing clears this at once, so the next Escape reaches the dialog.
pub fn open_recently(ctx: &egui::Context) -> bool {
    let pass: Option<u64> = ctx.data(|d| d.get_temp(open_pass_id()));
    pass.is_some_and(|p| p.saturating_add(1) >= ctx.cumulative_pass_nr())
}

/// The armed eyedropper of a picker: the picker's popup id, the pass it was last drawn in, and the
/// colour the viewer sampled for it, if one is waiting for the picker's next draw. It lives in the
/// egui context, since the viewer that samples has no picker in hand.
#[derive(Clone, Copy)]
struct Armed {
    popup: egui::Id,
    pass: u64,
    sample: Option<[f32; 3]>,
    dialog: Option<crate::Dialog>,
}

fn armed_key() -> egui::Id {
    egui::Id::unique("color-picker-eyedropper")
}

fn dialog_key() -> egui::Id {
    egui::Id::unique("color-picker-dialog")
}

/// Record the modal before drawing the viewer. A picker may sample behind its own dialog, but
/// a new, unrelated modal must block it from its very first pass, before its rect is known.
pub(crate) fn note_dialog(ctx: &egui::Context, dialog: Option<crate::Dialog>) {
    ctx.data_mut(|data| data.insert_temp(dialog_key(), dialog));
}

fn current_dialog(ctx: &egui::Context) -> Option<crate::Dialog> {
    ctx.data(|data| data.get_temp::<Option<crate::Dialog>>(dialog_key())).flatten()
}

/// The armed eyedropper, if its picker was drawn in this pass or the last: one whose picker stops
/// being drawn (its dialog gone) lapses.
fn armed_record(ctx: &egui::Context) -> Option<Armed> {
    let armed: Option<Armed> = ctx.data(|d| d.get_temp(armed_key()));
    let pass = ctx.cumulative_pass_nr();
    armed.filter(|a| a.pass.saturating_add(1) >= pass)
}

fn armed_for(ctx: &egui::Context, popup: egui::Id) -> bool {
    armed_record(ctx).is_some_and(|a| a.popup == popup)
}

/// Some picker's eyedropper is armed: a press on the Composition viewer samples the frame for it.
pub fn eyedropper_armed(ctx: &egui::Context) -> bool {
    armed_record(ctx).is_some()
}

fn arm(ctx: &egui::Context, popup: egui::Id) {
    let pass = ctx.cumulative_pass_nr();
    let dialog = current_dialog(ctx);
    ctx.data_mut(|d| d.insert_temp(armed_key(), Armed { popup, pass, sample: None, dialog }));
}

fn disarm(ctx: &egui::Context, popup: egui::Id) {
    let armed: Option<Armed> = ctx.data(|d| d.get_temp(armed_key()));
    if armed.is_some_and(|a| a.popup == popup) {
        ctx.data_mut(|d| d.remove::<Armed>(armed_key()));
    }
}

/// Keeps `popup`'s eyedropper armed while its picker is drawn.
fn refresh_armed(ctx: &egui::Context, popup: egui::Id) {
    if let Some(armed) = armed_record(ctx).filter(|a| a.popup == popup) {
        let pass = ctx.cumulative_pass_nr();
        ctx.data_mut(|d| d.insert_temp(armed_key(), Armed { pass, ..armed }));
    }
}

/// The colour the viewer sampled for `popup`, once: taking it disarms the eyedropper.
fn take_sample(ctx: &egui::Context, popup: egui::Id) -> Option<[f32; 3]> {
    let armed = armed_record(ctx).filter(|a| a.popup == popup)?;
    let sample = armed.sample?;
    disarm(ctx, popup);
    Some(sample)
}

/// The viewer sampled `rgb` for the armed picker, which takes it on its next draw.
pub fn deliver_sample(ctx: &egui::Context, rgb: [f32; 3]) {
    if let Some(armed) = armed_record(ctx) {
        ctx.data_mut(|d| d.insert_temp(armed_key(), Armed { sample: Some(rgb), ..armed }));
        ctx.request_repaint();
    }
}

fn viewer_key() -> egui::Id {
    egui::Id::unique("color-picker-viewer")
}

/// The Composition viewer's rect as it was drawn this pass: where an armed eyedropper samples.
pub fn note_viewer(ctx: &egui::Context, rect: Rect) {
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|d| d.insert_temp(viewer_key(), (pass, rect)));
}

fn overlay_key() -> egui::Id {
    egui::Id::unique("color-picker-overlays")
}

/// A popup or dialog drawn over the viewer: a press inside `rect` is not a sample.
pub fn note_overlay(ctx: &egui::Context, rect: Rect) {
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|d| {
        // This pass's and the last's: the viewer may be drawn before an overlay in a pass.
        let mut rects = d.get_temp::<Vec<(u64, Rect)>>(overlay_key()).unwrap_or_default();
        rects.retain(|(p, _)| p.saturating_add(1) >= pass);
        rects.push((pass, rect));
        d.insert_temp(overlay_key(), rects);
    });
}

/// Whether `pos` is over a popup or dialog drawn this pass or the last.
fn overlaid(ctx: &egui::Context, pos: egui::Pos2) -> bool {
    let pass = ctx.cumulative_pass_nr();
    let rects: Option<Vec<(u64, Rect)>> = ctx.data(|d| d.get_temp(overlay_key()));
    rects.unwrap_or_default().iter().any(|(p, r)| p.saturating_add(1) >= pass && r.contains(pos))
}

/// Where an armed eyedropper samples: the pointer, if it is over the viewer and not over a popup or
/// dialog. The pointer is read rather than the viewer's responses, since a modal dialog over the
/// viewer takes its clicks.
pub fn eyedropper_point(ctx: &egui::Context) -> Option<egui::Pos2> {
    if armed_record(ctx).is_some_and(|armed| armed.dialog != current_dialog(ctx)) {
        return None;
    }
    let viewer: Option<(u64, Rect)> = ctx.data(|d| d.get_temp(viewer_key()));
    let viewer = viewer.filter(|(p, _)| p.saturating_add(1) >= ctx.cumulative_pass_nr())?.1;
    let pos = ctx.input(|i| i.pointer.interact_pos())?;
    (viewer.contains(pos) && !overlaid(ctx, pos)).then_some(pos)
}

/// The press that samples for an armed eyedropper this frame, if there is one.
pub fn sample_press(ctx: &egui::Context) -> Option<egui::Pos2> {
    if ctx.input(|i| i.pointer.primary_pressed()) { eyedropper_point(ctx) } else { None }
}

fn automation_key() -> egui::Id {
    egui::Id::unique("color-picker-eyedropper-rect")
}

/// The eyedropper's rect, for the automation registry; [`take_automation`] hands it over.
fn note_automation(ctx: &egui::Context, rect: Rect) {
    ctx.data_mut(|d| d.insert_temp(automation_key(), rect));
}

/// The eyedropper rect the last picker drawing noted, once. Pickers have no registry in hand, so the
/// app registers it each frame.
pub fn take_automation(ctx: &egui::Context) -> Option<Rect> {
    ctx.data_mut(|d| {
        let rect = d.get_temp::<Rect>(automation_key());
        d.remove::<Rect>(automation_key());
        rect
    })
}

/// The eyedropper of the hex row. A press arms it, or disarms it while armed; the next press on the
/// Composition viewer then samples the colour under the pointer into this picker.
fn eyedropper(ui: &mut Ui, popup: egui::Id) {
    let armed = armed_for(ui.ctx(), popup);
    let (rect, resp) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::click());
    let resp = resp.on_hover_text(tr("Eyedropper: click, then click in the Composition panel"));
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, resp.enabled(), "Eyedropper"));
    let fill = if armed {
        ui.visuals().selection.bg_fill
    } else if resp.hovered() {
        ui.visuals().widgets.hovered.bg_fill
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, 2.0, fill);
    let col = if armed { ui.visuals().selection.stroke.color } else { ui.visuals().strong_text_color() };
    draw_pipette(ui.painter(), rect, col);
    if resp.clicked() {
        if armed {
            disarm(ui.ctx(), popup);
        } else {
            arm(ui.ctx(), popup);
        }
    }
    note_automation(ui.ctx(), rect);
}

/// A pipette: a diagonal glass tube with a bulb at the top right and a tip at the bottom left, drawn
/// within `rect`.
pub fn draw_pipette(p: &egui::Painter, rect: Rect, col: Color32) {
    let r = rect.shrink(rect.width() * 0.2);
    let at = |x: f32, y: f32| pos2(r.min.x + x * r.width(), r.min.y + y * r.height());
    p.line_segment([at(0.08, 0.92), at(0.62, 0.38)], Stroke::new(2.2, col));
    p.circle_filled(at(0.76, 0.24), r.width() * 0.2, col);
    p.line_segment([at(0.5, 0.3), at(0.7, 0.5)], Stroke::new(1.4, col));
}

/// What the picker did this frame.
#[derive(Default)]
struct Outcome {
    changed: bool,
    /// Close the picker, keeping the colour (OK) or not (Cancel).
    close: Option<bool>,
}

/// The channel the slider sets; the square sets the other two of its model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Chan {
    H,
    S,
    V,
    R,
    G,
    B,
}

impl Chan {
    /// The square's (across, up) channels while `self` is on the slider (After Effects' layout).
    fn square(self) -> (Chan, Chan) {
        match self {
            Chan::H => (Chan::S, Chan::V),
            Chan::S => (Chan::H, Chan::V),
            Chan::V => (Chan::H, Chan::S),
            Chan::R => (Chan::B, Chan::G),
            Chan::G => (Chan::B, Chan::R),
            Chan::B => (Chan::R, Chan::G),
        }
    }

    fn index(self) -> usize {
        match self {
            Chan::H | Chan::R => 0,
            Chan::S | Chan::G => 1,
            Chan::V | Chan::B => 2,
        }
    }

    fn is_hsv(self) -> bool {
        matches!(self, Chan::H | Chan::S | Chan::V)
    }
}

/// The colour as both HSB and RGB: HSB keeps the hue (and saturation) of greys and black, where
/// RGB has none, so moving off black returns to the hue that was chosen.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Pick {
    hsv: [f32; 3],
    rgb: [f32; 3],
}

impl Pick {
    fn get(&self, ch: Chan) -> f32 {
        let v = if ch.is_hsv() { self.hsv } else { self.rgb };
        v.get(ch.index()).copied().unwrap_or(0.0)
    }

    fn set(mut self, ch: Chan, v: f32) -> Pick {
        let v = v.clamp(0.0, 1.0);
        if ch.is_hsv() {
            if let Some(x) = self.hsv.get_mut(ch.index()) {
                *x = v;
            }
            self.rgb = hsv_to_rgb(self.hsv);
        } else {
            if let Some(x) = self.rgb.get_mut(ch.index()) {
                *x = v;
            }
            self.hsv = keep_hue(rgb_to_hsv(self.rgb), self.hsv);
        }
        self
    }

    /// The colour at (`u` across, `v` up) on the square, or `t` up the slider.
    fn at(&self, ch: Chan, u: f32, v: f32) -> Pick {
        let (x, y) = ch.square();
        self.set(x, u).set(y, v)
    }
}

const SQUARE: f32 = 200.0;
const SLIDER_W: f32 = 16.0;

/// The colour picker popup's contents: saturation/value square, channel slider, number fields for
/// H/S/B and R/G/B, a hex field and an original-colour swatch, plus OK and Cancel. `*c` is updated
/// as the UI changes and `original` is what Cancel (or the swatch) restores. The chosen channel
/// and the HSV/RGB pair are remembered in egui temp data so reopening keeps the hue when
/// saturation or value reaches zero. Returns what the picker did this frame; it never runs a
/// command itself.
fn color_picker(ui: &mut Ui, id: egui::Id, c: &mut [f32; 3], original: [f32; 3]) -> Outcome {
    let state_id = id.with("pick");
    let mut pick = match ui.data(|d| d.get_temp::<([f32; 3], [f32; 3])>(state_id)) {
        Some((hsv, rgb)) if rgb == *c => Pick { hsv, rgb },
        Some((hsv, _)) => Pick { hsv: keep_hue(rgb_to_hsv(*c), hsv), rgb: *c },
        None => Pick { hsv: rgb_to_hsv(*c), rgb: *c },
    };
    // The slider's channel is remembered between pickers, as After Effects does.
    let chan_id = egui::Id::unique("color-picker-channel");
    let mut chan = ui.data(|d| d.get_temp::<usize>(chan_id)).and_then(|i| CHANS.get(i).map(|c| c.0)).unwrap_or(Chan::H);
    let before = pick;
    let mut out = Outcome::default();

    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        // The square and slider, with OK and Cancel under the square's left edge.
        ui.vertical(|ui| {
            ui.horizontal_top(|ui| {
                square(ui, chan, &mut pick);
                slider(ui, chan, &mut pick);
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                if ui.add_sized(vec2(72.0, 20.0), egui::Button::new(tr("OK"))).clicked() {
                    out.close = Some(true);
                }
                if ui.add_sized(vec2(72.0, 20.0), egui::Button::new(tr("Cancel"))).clicked() {
                    out.close = Some(false);
                }
            });
        });
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            if swatches(ui, pick.rgb, original) {
                pick = Pick { hsv: keep_hue(rgb_to_hsv(original), pick.hsv), rgb: original };
            }
            ui.add_space(4.0);
            let mut field = |ui: &mut Ui, ch: Chan, name: &str, scale: f32, suffix: &str| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    let radio = ui.radio(chan == ch, format!("{name}:"));
                    // Line the fields up whatever each label's width.
                    ui.add_space((40.0 - radio.rect.width()).max(0.0));
                    if radio.clicked() {
                        chan = ch;
                    }
                    // Rounded as the hex code rounds, and 360° shows as 0°.
                    let mut v = (pick.get(ch) * scale).round();
                    if ch == Chan::H && v >= 360.0 {
                        v = 0.0;
                    }
                    let drag = egui::DragValue::new(&mut v).range(0.0..=scale).fixed_decimals(0).suffix(suffix).speed(scale / 255.0);
                    if ui.add_sized(vec2(56.0, 18.0), drag).labelled_by(radio.id).changed() {
                        pick = pick.set(ch, v.round() / scale);
                    }
                });
            };
            field(ui, Chan::H, "H", 360.0, "°");
            field(ui, Chan::S, "S", 100.0, "%");
            field(ui, Chan::V, "B", 100.0, "%");
            ui.add_space(4.0);
            field(ui, Chan::R, "R", 255.0, "");
            field(ui, Chan::G, "G", 255.0, "");
            field(ui, Chan::B, "B", 255.0, "");
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let mut rgb = pick.rgb;
                if hex_field(ui, id.with("hex"), &mut rgb) {
                    pick = Pick { hsv: keep_hue(rgb_to_hsv(rgb), pick.hsv), rgb };
                }
                eyedropper(ui, id);
            });
        });
    });

    if let Some(i) = CHANS.iter().position(|(c, _)| *c == chan) {
        ui.data_mut(|d| d.insert_temp(chan_id, i));
    }
    if pick != before && pick.rgb != *c {
        *c = pick.rgb;
        out.changed = true;
    }
    ui.data_mut(|d| d.insert_temp(state_id, (pick.hsv, pick.rgb)));
    out
}

/// The channels in the order of their buttons.
const CHANS: [(Chan, &str); 6] = [(Chan::H, "H"), (Chan::S, "S"), (Chan::V, "B"), (Chan::R, "R"), (Chan::G, "G"), (Chan::B, "B")];

/// Drag or click on the square to set its two channels.
fn square(ui: &mut Ui, chan: Chan, pick: &mut Pick) {
    let (rect, resp) = ui.allocate_exact_size(vec2(SQUARE, SQUARE), Sense::click_and_drag());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Slider, true, "Color field"));
    let (x, y) = chan.square();
    if let Some(p) = resp.interact_pointer_pos().filter(|_| resp.is_pointer_button_down_on()) {
        *pick = pick.at(chan, (p.x - rect.min.x) / rect.width(), 1.0 - (p.y - rect.min.y) / rect.height());
    }
    // Hue changes unevenly across a row, so the HSB squares get a finer grid.
    let n: u32 = if chan.is_hsv() { 24 } else { 8 };
    let row = n + 1;
    let mut mesh = egui::Mesh::default();
    for yi in 0..=n {
        for xi in 0..=n {
            let (u, v) = (xi as f32 / n as f32, yi as f32 / n as f32);
            mesh.colored_vertex(rect.lerp_inside(vec2(u, v)), srgb32(pick.at(chan, u, 1.0 - v).rgb));
            if xi < n && yi < n {
                let k = yi * row + xi;
                mesh.add_triangle(k, k + 1, k + row);
                mesh.add_triangle(k + 1, k + row + 1, k + row);
            }
        }
    }
    ui.painter().add(mesh);
    let at = rect.lerp_inside(vec2(pick.get(x), 1.0 - pick.get(y)));
    let light = pick.rgb[0] * 0.3 + pick.rgb[1] * 0.59 + pick.rgb[2] * 0.11 > 0.6;
    ui.painter().circle_stroke(at, 5.0, Stroke::new(1.5, if light { Color32::BLACK } else { Color32::WHITE }));
}

/// Drag or click on the slider to set `chan`; arrows on both sides mark its value.
fn slider(ui: &mut Ui, chan: Chan, pick: &mut Pick) {
    let (outer, resp) = ui.allocate_exact_size(vec2(SLIDER_W + 12.0, SQUARE), Sense::click_and_drag());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Slider, true, "Color slider"));
    let bar = Rect::from_center_size(outer.center(), vec2(SLIDER_W, SQUARE));
    if let Some(p) = resp.interact_pointer_pos().filter(|_| resp.is_pointer_button_down_on()) {
        *pick = pick.set(chan, 1.0 - (p.y - bar.min.y) / bar.height());
    }
    let n: u32 = if chan == Chan::H { 36 } else { 16 };
    let mut mesh = egui::Mesh::default();
    for i in 0..=n {
        let t = i as f32 / n as f32;
        // The hue slider shows pure hues; the others vary their channel in the current colour.
        let col = if chan == Chan::H { hsv_to_rgb([1.0 - t, 1.0, 1.0]) } else { pick.set(chan, 1.0 - t).rgb };
        let y = bar.min.y + t * bar.height();
        mesh.colored_vertex(pos2(bar.min.x, y), srgb32(col));
        mesh.colored_vertex(pos2(bar.max.x, y), srgb32(col));
        if i < n {
            let k = 2 * i;
            mesh.add_triangle(k, k + 1, k + 2);
            mesh.add_triangle(k + 1, k + 3, k + 2);
        }
    }
    ui.painter().add(mesh);
    let y = bar.min.y + (1.0 - pick.get(chan)) * bar.height();
    let fg = ui.visuals().strong_text_color();
    let stroke = Stroke::new(1.2, fg);
    for (edge, dir) in [(outer.min.x + 1.0, 1.0), (outer.max.x - 1.0, -1.0)] {
        let tip = pos2(edge + dir * 4.0, y);
        ui.painter().line_segment([pos2(edge, y - 4.0), tip], stroke);
        ui.painter().line_segment([pos2(edge, y + 4.0), tip], stroke);
    }
}

/// The new colour over the original; returns true when the original is clicked (to go back to it).
fn swatches(ui: &mut Ui, new: [f32; 3], original: [f32; 3]) -> bool {
    let (rect, _) = ui.allocate_exact_size(vec2(64.0, 64.0), Sense::hover());
    let (top, bottom) = rect.split_top_bottom_at_fraction(0.5);
    let new_resp = ui.interact(top, ui.scope_id().with("new-swatch"), Sense::hover());
    new_resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Unknown, true, "New color"));
    let old = ui.interact(bottom, ui.scope_id().with("original-swatch"), Sense::click()).on_hover_text(tr("Original color: click to go back to it"));
    old.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Original color"));
    ui.painter().rect_filled(top, 0.0, srgb32(new));
    ui.painter().rect_filled(bottom, 0.0, srgb32(original));
    ui.painter().rect_stroke(rect, 0.0, ui.visuals().widgets.inactive.bg_stroke, StrokeKind::Outside);
    old.clicked()
}

/// `hsv` with `old`'s hue where `hsv` has none (a grey or black), and `old`'s saturation where it
/// has neither (black).
fn keep_hue(mut hsv: [f32; 3], old: [f32; 3]) -> [f32; 3] {
    if hsv[1] == 0.0 || hsv[2] == 0.0 {
        hsv[0] = old[0];
    }
    if hsv[2] == 0.0 {
        hsv[1] = old[1];
    }
    hsv
}

/// The hex field. While it has focus it keeps what was typed (`id` holds the text); otherwise it
/// shows the colour. Focusing it selects the code, so typing replaces it; a valid code applies as
/// it is typed.
fn hex_field(ui: &mut Ui, id: egui::Id, c: &mut [f32; 3]) -> bool {
    let mut text = ui.data(|d| d.get_temp::<String>(id)).unwrap_or_else(|| srgb_hex(*c).trim_start_matches('#').to_string());
    let mut changed = false;
    let label = ui.label("#");
    let resp = ui
        .add(egui::TextEdit::singleline(&mut text).id(id.with("edit")).desired_width(62.0).char_limit(7).font(egui::TextStyle::Monospace))
        .labelled_by(label.id);
    if resp.gained_focus() {
        select_all(ui.ctx(), resp.id, &text);
    }
    if resp.changed()
        && let Some(rgb) = hex_rgb(&text)
    {
        let v = rgb.map(|b| b as f32 / 255.0);
        if v != *c {
            *c = v;
            changed = true;
        }
    }
    if resp.has_focus() {
        ui.data_mut(|d| d.insert_temp(id, text.clone()));
    } else {
        ui.data_mut(|d| d.remove::<String>(id));
    }
    changed
}

/// An sRGB colour (components 0–1) as an egui colour.
pub fn srgb32(c: [f32; 3]) -> Color32 {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color32::from_rgb(b(c[0]), b(c[1]), b(c[2]))
}

/// An sRGB colour (components 0–1) as `#RRGGBB`.
pub fn srgb_hex(c: [f32; 3]) -> String {
    let col = srgb32(c);
    format!("#{:02X}{:02X}{:02X}", col.r(), col.g(), col.b())
}

/// Parse a hex colour: `#RRGGBB`, `RRGGBB`, `#RGB` or `RGB` (any case, surrounding space ignored).
pub fn hex_rgb(s: &str) -> Option<[u8; 3]> {
    let h = s.trim().trim_start_matches('#');
    if !h.is_ascii() {
        return None;
    }
    let digit = |i: usize| h.get(i..i + 1).and_then(|d| u8::from_str_radix(d, 16).ok());
    let pair = |i: usize| h.get(i..i + 2).and_then(|d| u8::from_str_radix(d, 16).ok());
    match h.len() {
        3 => Some([digit(0)? * 17, digit(1)? * 17, digit(2)? * 17]),
        6 => Some([pair(0)?, pair(2)?, pair(4)?]),
        _ => None,
    }
}

/// sRGB (components 0–1) to hue, saturation and value (0–1).
pub fn rgb_to_hsv([r, g, b]: [f32; 3]) -> [f32; 3] {
    let (r, g, b) = (r.clamp(0.0, 1.0), g.clamp(0.0, 1.0), b.clamp(0.0, 1.0));
    let max = r.max(g).max(b);
    let d = max - r.min(g).min(b);
    if d <= 0.0 {
        return [0.0, 0.0, max];
    }
    let h = if max == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    [h / 6.0, d / max, max]
}

/// Hue, saturation and value (0–1) to sRGB (components 0–1).
pub fn hsv_to_rgb([h, s, v]: [f32; 3]) -> [f32; 3] {
    let h = h.clamp(0.0, 1.0) * 6.0;
    let (s, v) = (s.clamp(0.0, 1.0), v.clamp(0.0, 1.0));
    let f = |n: f32| {
        let k = (n + h) % 6.0;
        v - v * s * k.min(4.0 - k).clamp(0.0, 1.0)
    };
    [f(5.0), f(3.0), f(1.0)]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sampling_viewer() -> egui_kittest::Harness<'static, crate::EffectcraftApp> {
        let mut session = effectcraft_engine::Session::default();
        session.execute("comp.new", serde_json::json!({"width": 320, "height": 180, "duration": 2})).unwrap();
        session.execute("layer.newSolid", serde_json::json!({"color": "#204060"})).unwrap();
        let mut harness = egui_kittest::Harness::builder().with_size(vec2(1200.0, 800.0)).build_eframe(|_| crate::EffectcraftApp::new(session));
        settle_sampling_viewer(&mut harness);
        harness
    }

    fn settle_sampling_viewer(harness: &mut egui_kittest::Harness<'_, crate::EffectcraftApp>) {
        for _ in 0..200 {
            harness.step();
            if harness.state().frames.inflight() == 0 && harness.state().viewer_image.is_some() {
                harness.run_steps(3);
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("the synthetic viewer frame did not arrive");
    }

    fn press_sampling_viewer(harness: &mut egui_kittest::Harness<'_, crate::EffectcraftApp>) -> Option<[f32; 3]> {
        let ctx = harness.ctx.clone();
        arm(&ctx, egui::Id::unique("test-popup"));
        let point = crate::panels::viewer::comp_to_screen(&ctx, [20.0, 20.0]).unwrap();
        harness.event(egui::Event::PointerMoved(point));
        harness.event(egui::Event::PointerButton { pos: point, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
        harness.step();
        let sample = ctx.data(|data| data.get_temp::<Armed>(armed_key())).and_then(|armed| armed.sample);
        harness.event(egui::Event::PointerButton { pos: point, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
        harness.step();
        sample
    }

    #[test]
    fn popup_eyedropper_samples_the_displayed_channel_and_exposure() {
        let mut harness = sampling_viewer();
        harness.state_mut().session.state.viewer.channel = effectcraft_engine::viewer::Channel::Green;
        let green = press_sampling_viewer(&mut harness).unwrap();
        assert!(green.iter().all(|value| (*value - 64.0 / 255.0).abs() < 0.01), "{green:?}");
        let options = &mut harness.state_mut().session.state.viewer;
        options.channel = effectcraft_engine::viewer::Channel::Rgb;
        options.exposure = 1.0;
        let exposed = press_sampling_viewer(&mut harness).unwrap();
        let mut expected = [[32, 64, 96, 255]];
        effectcraft_engine::viewer::display_transform(&mut expected, effectcraft_engine::viewer::Channel::Rgb, false, 1.0);
        for (actual, expected) in exposed.iter().zip(expected[0]) {
            assert!((*actual - expected as f32 / 255.0).abs() < 0.01, "{exposed:?}");
        }
    }

    #[test]
    fn popup_eyedropper_samples_snapshot_and_refuses_wireframe() {
        let mut harness = sampling_viewer();
        harness.state_mut().session.execute("view.takeSnapshot", serde_json::json!({})).unwrap();
        harness.state_mut().session.execute("layer.newSolid", serde_json::json!({"color": "#a0b0c0"})).unwrap();
        settle_sampling_viewer(&mut harness);
        harness.event(egui::Event::Key { key: egui::Key::F5, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE });
        // Let the snapshot texture's discard passes finish before arming this synthetic picker.
        harness.step();
        let snapshot = press_sampling_viewer(&mut harness).unwrap();
        for (actual, expected) in snapshot.iter().zip([32, 64, 96]) {
            assert!((*actual - expected as f32 / 255.0).abs() < 0.01, "{snapshot:?}");
        }
        let options = &mut harness.state_mut().session.state.viewer;
        options.show_snapshot = false;
        options.fast_previews = effectcraft_engine::commands::viewer_cmds::FastPreviews::Wireframe;
        harness.event(egui::Event::Key { key: egui::Key::F5, physical_key: None, pressed: false, repeat: false, modifiers: egui::Modifiers::NONE });
        assert_eq!(press_sampling_viewer(&mut harness), None, "wireframe must not sample an old pixel frame");
    }

    #[test]
    fn a_new_modal_backdrop_cannot_deliver_a_pending_sample() {
        let mut harness = sampling_viewer();
        harness.state_mut().dialog = Some(crate::Dialog::About);
        assert_eq!(press_sampling_viewer(&mut harness), None, "the first modal pass must already exclude its backdrop");
    }

    #[test]
    fn a_picker_can_sample_behind_its_own_dialog_but_not_a_new_modal() {
        let ctx = egui::Context::default();
        let point = pos2(20.0, 20.0);
        let mut output = ctx.run_ui(egui::RawInput { events: vec![egui::Event::PointerMoved(point)], ..Default::default() }, |ui| {
            let ctx = ui.ctx();
            note_dialog(ctx, Some(crate::Dialog::SolidSettings));
            note_viewer(ctx, Rect::from_min_size(pos2(0.0, 0.0), vec2(320.0, 180.0)));
            note_overlay(ctx, Rect::from_min_size(pos2(100.0, 100.0), vec2(120.0, 80.0)));
            arm(ctx, egui::Id::unique("test-picker"));
            assert_eq!(eyedropper_point(ctx), Some(point));
            note_dialog(ctx, Some(crate::Dialog::About));
            assert_eq!(eyedropper_point(ctx), None);
        });
        output.textures_delta.clear();
    }

    #[test]
    fn armed_eyedropper_takes_one_sample_then_disarms() {
        let ctx = egui::Context::default();
        let popup = egui::Id::unique("test-picker");
        arm(&ctx, popup);
        assert!(eyedropper_armed(&ctx));
        assert_eq!(take_sample(&ctx, popup), None);
        deliver_sample(&ctx, [0.1, 0.2, 0.3]);
        assert_eq!(take_sample(&ctx, popup), Some([0.1, 0.2, 0.3]));
        assert!(!eyedropper_armed(&ctx));
        assert_eq!(take_sample(&ctx, popup), None);
    }

    #[test]
    fn points_over_a_dialog_are_not_samples() {
        let ctx = egui::Context::default();
        let dialog = Rect::from_min_size(pos2(100.0, 100.0), vec2(120.0, 80.0));
        note_overlay(&ctx, dialog);
        assert!(overlaid(&ctx, pos2(150.0, 150.0)));
        assert!(!overlaid(&ctx, pos2(20.0, 20.0)));
    }
}
