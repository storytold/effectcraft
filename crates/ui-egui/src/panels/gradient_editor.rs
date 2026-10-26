//! The Gradient Editor (#612), opened by a gradient property's "Edit Gradient…" in the Timeline
//! (Gradient Fill / Gradient Stroke ▸ Colors, Gradient Overlay ▸ Gradient): `prop.editGradient`.
//! Opacity stops sit above the preview bar and colour stops below it. Click above or below the
//! bar to add a stop (it takes that track's value there), drag a stop to move it, edit the
//! selected stop's location, opacity or colour, and Delete removes it (never the last stop of a
//! track). The editor works on a draft: OK sets the property once with `prop.set` at the time it
//! opened (one undo step, nothing at all when nothing changed); Cancel and Escape leave it as it
//! was.
//!
//! Automation ids: `dialog.gradient.bar`, `dialog.gradient.addOpacity`, `dialog.gradient.addColor`,
//! `dialog.gradient.opacityStop.<i>`, `dialog.gradient.colorStop.<i>` (in order along the bar),
//! `dialog.gradient.location`, `dialog.gradient.opacity`, `dialog.gradient.color`,
//! `dialog.gradient.delete`, `dialog.gradient.ok`, `dialog.gradient.cancel`.

use effectcraft_engine::keyframe::{Gradient, Value};
use effectcraft_engine::project::{ItemId, LayerId};
use egui::{Color32, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use serde_json::{Value as Json, json};

use crate::i18n::tr;
use crate::theme::Tokens;
use crate::{Dialog, EffectcraftApp};

/// Which row of stops: opacity (above the bar) or colour (below it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Track {
    Opacity,
    Color,
}

/// A stop in the draft. `id` stays with the stop when it moves past another one (the tracks are
/// kept in order along the bar, so its index changes).
#[derive(Clone, Debug, PartialEq)]
pub struct Stop<T> {
    pub id: u64,
    pub pos: f64,
    pub value: T,
}

/// The property being edited, as it was when the editor opened.
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    pub comp: u64,
    pub layer: u64,
    pub prop: u64,
    /// Composition seconds, snapped to the nearest frame: the value is read and set there.
    pub time: f64,
}

/// The editor's draft gradient and selection.
#[derive(Clone, Debug, Default)]
pub struct Draft {
    pub target: Option<Target>,
    pub colors: Vec<Stop<[f32; 4]>>,
    pub opacities: Vec<Stop<f32>>,
    pub selected: Option<(Track, u64)>,
    /// The gradient as it opened, stops in order: OK with an equal draft changes nothing.
    pub baseline: Gradient,
    /// Stop counts (colour, opacity) of the keyframes just before and after the edit time.
    pub neighbors: Vec<(usize, usize)>,
    next_id: u64,
}

fn by_pos<T>(v: &mut [Stop<T>]) {
    // Stable: stops at the same location keep their order.
    v.sort_by(|a, b| a.pos.total_cmp(&b.pos));
}

fn clamp_pos(p: f64) -> f64 {
    if p.is_finite() { p.clamp(0.0, 1.0) } else { 0.0 }
}

impl Draft {
    /// A draft of `g` (stops put in order, locations clamped to 0..1).
    pub fn new(g: &Gradient, target: Option<Target>, neighbors: Vec<(usize, usize)>) -> Draft {
        let mut d = Draft { target, neighbors, ..Default::default() };
        for &(pos, value) in &g.colors {
            let id = d.fresh();
            d.colors.push(Stop { id, pos: clamp_pos(pos), value });
        }
        for &(pos, value) in &g.opacities {
            let id = d.fresh();
            d.opacities.push(Stop { id, pos: clamp_pos(pos), value });
        }
        by_pos(&mut d.colors);
        by_pos(&mut d.opacities);
        d.baseline = d.gradient();
        d
    }

    fn fresh(&mut self) -> u64 {
        self.next_id = self.next_id.saturating_add(1);
        self.next_id
    }

    /// The draft as a gradient (the tracks are always in order).
    pub fn gradient(&self) -> Gradient {
        Gradient { colors: self.colors.iter().map(|s| (s.pos, s.value)).collect(), opacities: self.opacities.iter().map(|s| (s.pos, s.value)).collect() }
    }

    pub fn len(&self, track: Track) -> usize {
        match track {
            Track::Color => self.colors.len(),
            Track::Opacity => self.opacities.len(),
        }
    }

    /// The id of the stop `i`-th along the bar.
    pub fn id_at(&self, track: Track, i: usize) -> Option<u64> {
        match track {
            Track::Color => self.colors.get(i).map(|s| s.id),
            Track::Opacity => self.opacities.get(i).map(|s| s.id),
        }
    }

    /// Where stop `id` is along the bar (its index), if it exists.
    pub fn index_of(&self, track: Track, id: u64) -> Option<usize> {
        match track {
            Track::Color => self.colors.iter().position(|s| s.id == id),
            Track::Opacity => self.opacities.iter().position(|s| s.id == id),
        }
    }

    /// Add a stop at `pos` with that track's current value there, select it, return its id.
    pub fn add(&mut self, track: Track, pos: f64) -> u64 {
        let pos = clamp_pos(pos);
        let g = self.gradient();
        let id = self.fresh();
        match track {
            Track::Color => self.colors.push(Stop { id, pos, value: g.color_at(pos) }),
            Track::Opacity => self.opacities.push(Stop { id, pos, value: g.opacity_at(pos) }),
        }
        self.sort();
        self.selected = Some((track, id));
        id
    }

    /// Move stop `id` to `pos` (clamped) and keep the track in order.
    pub fn move_to(&mut self, track: Track, id: u64, pos: f64) {
        let pos = clamp_pos(pos);
        match track {
            Track::Color => {
                if let Some(s) = self.colors.iter_mut().find(|s| s.id == id) {
                    s.pos = pos;
                }
            }
            Track::Opacity => {
                if let Some(s) = self.opacities.iter_mut().find(|s| s.id == id) {
                    s.pos = pos;
                }
            }
        }
        self.sort();
    }

    /// Remove stop `id` unless it is the last of its track. Returns whether it went.
    pub fn remove(&mut self, track: Track, id: u64) -> bool {
        if self.len(track) <= 1 {
            return false;
        }
        let before = self.len(track);
        match track {
            Track::Color => self.colors.retain(|s| s.id != id),
            Track::Opacity => self.opacities.retain(|s| s.id != id),
        }
        if self.selected == Some((track, id)) {
            self.selected = None;
        }
        self.len(track) < before
    }

    /// Set colour stop `id`'s RGB, keeping its alpha.
    pub fn set_rgb(&mut self, id: u64, rgb: [f32; 3]) {
        if let Some(s) = self.colors.iter_mut().find(|s| s.id == id) {
            s.value = [rgb[0], rgb[1], rgb[2], s.value[3]];
        }
    }

    /// Set opacity stop `id` (0..1).
    pub fn set_opacity(&mut self, id: u64, a: f32) {
        if let Some(s) = self.opacities.iter_mut().find(|s| s.id == id) {
            s.value = if a.is_finite() { a.clamp(0.0, 1.0) } else { s.value };
        }
    }

    fn sort(&mut self) {
        by_pos(&mut self.colors);
        by_pos(&mut self.opacities);
    }

    /// The draft's stop counts differ from a neighbouring keyframe's, so the two can't
    /// interpolate (gradient keyframes blend only with the same number of stops).
    pub fn mismatched(&self) -> bool {
        self.neighbors.iter().any(|&(c, o)| c != self.colors.len() || o != self.opacities.len())
    }
}

/// `prop.editGradient {layer, prop, comp?}`: open the editor on that gradient property at the
/// current time (snapped to the nearest frame, as `prop.set` snaps it).
pub fn open(app: &mut EffectcraftApp, p: &Json) -> Result<Json, String> {
    let cid = match p.get("comp").and_then(Json::as_u64) {
        Some(c) => ItemId(c),
        None => app.session.active_comp_id().ok_or("no composition is open")?,
    };
    let comp = app.session.project.comp(cid).ok_or_else(|| format!("no composition {}", cid.0))?;
    let lid = p.get("layer").and_then(Json::as_u64).ok_or("missing `layer`")?;
    let layer = comp.layer(LayerId(lid)).ok_or_else(|| format!("no layer {lid}"))?;
    let uid = p.get("prop").and_then(Json::as_u64).ok_or("missing `prop`")?;
    let prop = layer.props.find(uid).ok_or_else(|| format!("no property @{uid}"))?;
    let ct = comp.frame_rate.snap_nearest(app.session.time_of(cid));
    let lt = layer.layer_time(ct);
    let Value::Gradient(g) = prop.value_at(lt) else {
        return Err(format!("{} is not a gradient", prop.name));
    };
    // The keyframes either side of the edit time (one exactly there is the one being edited).
    let counts = |v: &Value| match v {
        Value::Gradient(g) => Some((g.colors.len(), g.opacities.len())),
        _ => None,
    };
    let before = prop.keys.iter().rev().find(|k| k.time < lt).and_then(|k| counts(&k.value));
    let after = prop.keys.iter().find(|k| k.time > lt).and_then(|k| counts(&k.value));
    let target = Target { comp: cid.0, layer: lid, prop: uid, time: ct.seconds() };
    app.dialog_state.gradient = Draft::new(&g, Some(target), before.into_iter().chain(after).collect());
    app.dialog = Some(Dialog::GradientEditor);
    Ok(json!({"dialog": "gradientEditor"}))
}

/// Set the property to the draft: one `prop.set` at the time the editor opened, in composition
/// time, or nothing when the draft is unchanged (so OK adds no keyframe).
fn commit(app: &mut EffectcraftApp, ctx: &egui::Context) {
    app.dialog = None;
    let d = std::mem::take(&mut app.dialog_state.gradient);
    let g = d.gradient();
    let Some(tg) = d.target else { return };
    if g == d.baseline {
        return;
    }
    let params = json!({"comp": tg.comp, "layer": tg.layer, "prop": tg.prop, "time": tg.time, "timeBase": "comp", "value": Value::Gradient(g).to_json()});
    if let Err(e) = crate::menus::invoke(app, ctx, "prop.set", params) {
        app.ui.status = e;
    }
}

fn to_color32(c: [f32; 4]) -> Color32 {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color32::from_rgba_unmultiplied(b(c[0]), b(c[1]), b(c[2]), b(c[3]))
}

pub fn show(app: &mut EffectcraftApp, ctx: &egui::Context, t: &Tokens) {
    if app.dialog_state.gradient.target.is_none() {
        app.dialog = None;
        return;
    }
    let mut answer: Option<bool> = None;
    super::dialogs::modal(ctx, tr("Gradient Editor"), vec2(520.0, 270.0), t, |ui| {
        let w = ui.available_width().max(200.0);
        let (area, _) = ui.allocate_exact_size(vec2(w, 72.0), Sense::hover());
        let bar = Rect::from_min_max(pos2(area.min.x + 8.0, area.min.y + 22.0), pos2(area.max.x - 8.0, area.min.y + 48.0));
        let opacity_row = Rect::from_min_max(pos2(bar.min.x, area.min.y), pos2(bar.max.x, bar.min.y - 2.0));
        let color_row = Rect::from_min_max(pos2(bar.min.x, bar.max.y + 2.0), pos2(bar.max.x, area.max.y));
        let pos_of = |x: f32| ((x - bar.min.x) / bar.width().max(1.0)) as f64;
        let x_of = |p: f64| bar.min.x + bar.width() * p as f32;
        let painter = ui.painter().clone();

        // The bar: a checkerboard under the gradient, so opacity shows.
        let cell = 6.0;
        let mut y = bar.min.y;
        let mut row = 0;
        while y < bar.max.y {
            let mut x = bar.min.x;
            let mut col = row;
            while x < bar.max.x {
                let c = if col % 2 == 0 { Color32::from_gray(200) } else { Color32::from_gray(150) };
                painter.rect_filled(Rect::from_min_max(pos2(x, y), pos2((x + cell).min(bar.max.x), (y + cell).min(bar.max.y))), 0.0, c);
                x += cell;
                col += 1;
            }
            y += cell;
            row += 1;
        }
        let g = app.dialog_state.gradient.gradient();
        let n = 96;
        for k in 0..n {
            let x0 = bar.min.x + bar.width() * k as f32 / n as f32;
            let x1 = bar.min.x + bar.width() * (k + 1) as f32 / n as f32;
            let c = g.sample((k as f64 + 0.5) / n as f64);
            painter.rect_filled(Rect::from_min_max(pos2(x0, bar.min.y), pos2(x1, bar.max.y)), 0.0, to_color32(c));
        }
        painter.rect_stroke(bar, 0.0, Stroke::new(1.0, t.field_border), StrokeKind::Outside);
        app.auto.add("dialog.gradient.bar", bar, "Gradient");

        // Clicking a row's empty space adds a stop there.
        for (track, rowr, id) in [(Track::Opacity, opacity_row, "dialog.gradient.addOpacity"), (Track::Color, color_row, "dialog.gradient.addColor")] {
            let r = ui.interact(rowr, egui::Id::unique(("gradient-add", id)), Sense::click());
            app.auto.add(id, rowr, id);
            if r.clicked()
                && let Some(p) = r.interact_pointer_pos()
            {
                app.dialog_state.gradient.add(track, pos_of(p.x));
            }
        }

        // The stops (drawn and hit-tested over the rows, so they take the clicks).
        for track in [Track::Opacity, Track::Color] {
            for i in 0..app.dialog_state.gradient.len(track) {
                let d = &app.dialog_state.gradient;
                let Some(id) = d.id_at(track, i) else { continue };
                let (pos, fill) = match track {
                    Track::Color => d.colors.get(i).map(|s| (s.pos, to_color32([s.value[0], s.value[1], s.value[2], 1.0]))),
                    Track::Opacity => d.opacities.get(i).map(|s| (s.pos, Color32::from_gray((s.value.clamp(0.0, 1.0) * 255.0) as u8))),
                }
                .unwrap_or((0.0, Color32::WHITE));
                let selected = d.selected == Some((track, id));
                let x = x_of(pos);
                let r = match track {
                    Track::Opacity => Rect::from_min_max(pos2(x - 5.0, opacity_row.max.y - 15.0), pos2(x + 5.0, opacity_row.max.y)),
                    Track::Color => Rect::from_min_max(pos2(x - 5.0, color_row.min.y), pos2(x + 5.0, color_row.min.y + 15.0)),
                };
                painter.rect_filled(r, 1.0, fill);
                painter.rect_stroke(
                    r,
                    1.0,
                    Stroke::new(if selected { 2.0 } else { 1.0 }, if selected { t.accent } else { t.field_border }),
                    StrokeKind::Outside,
                );
                let resp = ui.interact(r.expand(2.0), egui::Id::unique(("gradient-stop", track == Track::Color, id)), Sense::click_and_drag());
                let name = match track {
                    Track::Opacity => format!("dialog.gradient.opacityStop.{i}"),
                    Track::Color => format!("dialog.gradient.colorStop.{i}"),
                };
                app.auto.add(&name, r, &name);
                if resp.clicked() || resp.drag_started() {
                    app.dialog_state.gradient.selected = Some((track, id));
                }
                if resp.dragged()
                    && let Some(p) = resp.interact_pointer_pos()
                {
                    app.dialog_state.gradient.move_to(track, id, pos_of(p.x));
                }
            }
        }
        ui.add_space(8.0);

        // The selected stop's fields.
        let selected = app.dialog_state.gradient.selected;
        let selected = selected.and_then(|(track, id)| app.dialog_state.gradient.index_of(track, id).map(|i| (track, id, i)));
        ui.horizontal(|ui| {
            let Some((track, id, i)) = selected else {
                ui.label(egui::RichText::new(tr("Click above the bar to add an opacity stop, below it to add a color stop.")).color(t.text_dim));
                return;
            };
            ui.label(tr("Location:"));
            let d = &app.dialog_state.gradient;
            let pos = match track {
                Track::Color => d.colors.get(i).map(|s| s.pos),
                Track::Opacity => d.opacities.get(i).map(|s| s.pos),
            }
            .unwrap_or(0.0);
            let mut pct = pos * 100.0;
            let r = ui.add(egui::DragValue::new(&mut pct).range(0.0..=100.0).speed(0.5).max_decimals(1).suffix("%"));
            app.auto.add("dialog.gradient.location", r.rect, "Location");
            if r.changed() {
                app.dialog_state.gradient.move_to(track, id, pct / 100.0);
            }
            // Moving can reorder the track: find the stop again by its id, so the fields below
            // show this stop and not the one now at its old index.
            let Some(i) = app.dialog_state.gradient.index_of(track, id) else { return };
            match track {
                Track::Opacity => {
                    ui.label(tr("Opacity:"));
                    let mut a = app.dialog_state.gradient.opacities.get(i).map_or(1.0, |s| s.value) as f64 * 100.0;
                    let r = ui.add(egui::DragValue::new(&mut a).range(0.0..=100.0).speed(0.5).max_decimals(1).suffix("%"));
                    app.auto.add("dialog.gradient.opacity", r.rect, "Opacity");
                    if r.changed() {
                        app.dialog_state.gradient.set_opacity(id, (a / 100.0) as f32);
                    }
                }
                Track::Color => {
                    ui.label(tr("Color:"));
                    let c = app.dialog_state.gradient.colors.get(i).map_or([1.0; 4], |s| s.value);
                    // The shared colour picker (sRGB, hex field, OK / Cancel); the stop keeps its alpha.
                    let mut rgb = [c[0], c[1], c[2]];
                    let swatch = crate::widgets::srgb_color_button(ui, &mut rgb);
                    app.auto.add("dialog.gradient.color", swatch.rect, "Color");
                    if swatch.changed() {
                        app.dialog_state.gradient.set_rgb(id, rgb);
                    }
                }
            }
            let can_delete = app.dialog_state.gradient.len(track) > 1;
            let r = ui.add_enabled(can_delete, egui::Button::new(tr("Delete")));
            app.auto.add("dialog.gradient.delete", r.rect, "Delete");
            if r.clicked() {
                app.dialog_state.gradient.remove(track, id);
            }
        });
        if app.dialog_state.gradient.mismatched() {
            ui.add_space(4.0);
            ui.label(egui::RichText::new(tr("These adjacent keyframes cannot interpolate because their stop counts differ.")).color(t.danger).size(11.0));
        }
        ui.add_space(14.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let r = ui.button(tr("Cancel"));
            app.auto.add("dialog.gradient.cancel", r.rect, "Cancel");
            if r.clicked() {
                answer = Some(false);
            }
            let r = ui.button(tr("OK"));
            app.auto.add("dialog.gradient.ok", r.rect, "OK");
            if r.clicked() {
                answer = Some(true);
            }
        });
    });
    // Delete / Backspace remove the selected stop, but not while typing in a field.
    if answer.is_none()
        && !ctx.egui_wants_keyboard_input()
        && ctx.input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace))
        && let Some((track, id)) = app.dialog_state.gradient.selected
    {
        app.dialog_state.gradient.remove(track, id);
    }
    match answer {
        Some(true) => commit(app, ctx),
        Some(false) => {
            app.dialog = None;
            app.dialog_state.gradient = Draft::default();
        }
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft() -> Draft {
        Draft::new(&Gradient { colors: vec![(0.0, [1.0, 0.0, 0.0, 1.0]), (1.0, [0.0, 0.0, 1.0, 1.0])], opacities: vec![(0.0, 1.0), (1.0, 0.5)] }, None, vec![])
    }

    /// Dragging the first stop past the second keeps the track in order and the same stop
    /// selected; a stop added afterwards samples the reordered track.
    #[test]
    fn stops_stay_in_order_and_selected_while_moving() {
        for track in [Track::Color, Track::Opacity] {
            let g = Gradient { colors: vec![(0.0, [1.0, 0.0, 0.0, 1.0]), (0.6, [0.0, 0.0, 1.0, 1.0])], opacities: vec![(0.0, 1.0), (0.6, 0.5)] };
            let mut d = Draft::new(&g, None, vec![]);
            let first = d.id_at(track, 0).unwrap();
            d.selected = Some((track, first));
            d.move_to(track, first, 0.3);
            d.move_to(track, first, 0.8);
            assert_eq!(d.index_of(track, first), Some(1), "{track:?}: dragged past the stop at 0.6, it is now second");
            assert_eq!(d.selected, Some((track, first)), "{track:?}: the selection followed it");
            let g = d.gradient();
            let positions: Vec<f64> = match track {
                Track::Color => g.colors.iter().map(|c| c.0).collect(),
                Track::Opacity => g.opacities.iter().map(|c| c.0).collect(),
            };
            assert!(positions.windows(2).all(|w| w[0] <= w[1]), "{track:?}: {positions:?}");
            let added = d.add(track, 0.5);
            assert_eq!(d.index_of(track, added), Some(0), "{track:?}: the new stop sorts before the stops at 0.6 and 0.8");
            assert_eq!(d.selected, Some((track, added)));
        }
        // After the swap the colour track runs blue (at 0) to red (at 1): a stop added at 0.5 is
        // their mix in the swapped order, not the original red-to-blue.
        let mut d = draft();
        let red = d.id_at(Track::Color, 0).unwrap();
        let blue = d.id_at(Track::Color, 1).unwrap();
        d.move_to(Track::Color, red, 1.0);
        d.move_to(Track::Color, blue, 0.0);
        let id = d.add(Track::Color, 0.25);
        let i = d.index_of(Track::Color, id).unwrap();
        let c = d.colors[i].value;
        assert!(c[2] > c[0], "blue dominates near 0 after the swap: {c:?}");
    }

    #[test]
    fn last_stop_stays_and_new_stops_keep_their_tracks_values() {
        let mut d = draft();
        let a = d.id_at(Track::Opacity, 0).unwrap();
        let b = d.id_at(Track::Opacity, 1).unwrap();
        assert!(d.remove(Track::Opacity, a));
        assert!(!d.remove(Track::Opacity, b), "the last opacity stop stays");
        let mut d = draft();
        let id = d.add(Track::Opacity, 0.5);
        let i = d.index_of(Track::Opacity, id).unwrap();
        assert!((d.opacities[i].value - 0.75).abs() < 1e-6, "the opacity track's own value");
        let id = d.add(Track::Color, 0.5);
        let i = d.index_of(Track::Color, id).unwrap();
        assert!((d.colors[i].value[3] - 1.0).abs() < 1e-6, "colour stops keep their own alpha, not times opacity");
        d.set_rgb(id, [0.0, 1.0, 0.0]);
        assert_eq!(d.colors[i].value, [0.0, 1.0, 0.0, 1.0]);
        d.set_opacity(d.id_at(Track::Opacity, 0).unwrap(), f32::NAN);
        assert!(d.opacities.iter().all(|s| s.value.is_finite()));
        d.move_to(Track::Color, id, f64::INFINITY);
        assert!(d.colors.iter().all(|s| (0.0..=1.0).contains(&s.pos)));
    }

    #[test]
    fn unchanged_drafts_equal_their_baseline() {
        // Out-of-order and out-of-range stops come in sorted and clamped: still "unchanged".
        let g = Gradient { colors: vec![(1.2, [0.0, 0.0, 1.0, 1.0]), (0.0, [1.0, 0.0, 0.0, 1.0])], opacities: vec![(0.5, 1.0)] };
        let d = Draft::new(&g, None, vec![]);
        assert_eq!(d.gradient(), d.baseline);
        assert_eq!(d.gradient().colors.iter().map(|c| c.0).collect::<Vec<_>>(), vec![0.0, 1.0]);
        let mut e = d.clone();
        e.add(Track::Opacity, 0.2);
        assert_ne!(e.gradient(), e.baseline);
    }

    #[test]
    fn the_warning_follows_the_neighbours_stop_counts() {
        let mut d = Draft::new(&Gradient::default(), None, vec![(2, 2), (2, 2)]);
        assert!(!d.mismatched(), "same counts as both neighbours");
        d.add(Track::Color, 0.5);
        assert!(d.mismatched(), "three colour stops next to keyframes with two");
        assert!(!Draft::new(&Gradient::default(), None, vec![]).mismatched(), "not animated: no neighbours");
    }
}
