//! Shared gradient stop editor for Timeline, Effect Controls and Layer Styles.
//! Each edit returns an ordinary property value, preserving undo, keyframes and
//! serialization through the existing `prop.set` command.

use effectcraft_engine::keyframe::Gradient;
use egui::{Id, Rect, Ui};

use crate::widgets;

const MAX_VISIBLE_STOPS: usize = 64;

/// Show an open gradient editor next to its clicked swatch/button.
pub(crate) fn popup(ui: &mut Ui, id: Id, anchor: Rect, original: &Gradient) -> Option<Gradient> {
    if !widgets::popup_is_open(ui, id) {
        return None;
    }
    let mut next = original.clone();
    let mut changed = false;
    let rows = (next.colors.len().min(MAX_VISIBLE_STOPS) + next.opacities.len().min(MAX_VISIBLE_STOPS) + 5).min(32);
    let _ = widgets::popup_list(ui, id, anchor.left_bottom(), anchor, rows, 0, |ui| {
        changed = controls(ui, &mut next);
        None::<()>
    });
    changed.then_some(next)
}

/// Stop colours and opacity are separate channels; editing one must not
/// overwrite the other. Keep stops sorted for gradient interpolation.
fn controls(ui: &mut Ui, gradient: &mut Gradient) -> bool {
    let mut changed = false;
    ui.set_min_width(310.0);
    ui.label("Color stops");
    let mut remove_color = None;
    let color_count = gradient.colors.len();
    for (i, (position, color)) in gradient.colors.iter_mut().take(MAX_VISIBLE_STOPS).enumerate() {
        ui.horizontal(|ui| {
            ui.label(format!("{}", i + 1));
            changed |= ui
                .add(egui::DragValue::new(position).speed(0.01).range(0.0..=1.0))
                .changed();
            let mut rgb = [color[0], color[1], color[2]];
            if widgets::srgb_color_button(ui, &mut rgb).changed() {
                color[..3].copy_from_slice(&rgb);
                changed = true;
            }
            ui.label("Alpha");
            changed |= ui
                .add(egui::DragValue::new(&mut color[3]).speed(0.01).range(0.0..=1.0))
                .changed();
            if color_count > 2 && ui.small_button("Remove").clicked() {
                remove_color = Some(i);
            }
        });
    }
    if let Some(i) = remove_color {
        gradient.colors.remove(i);
        changed = true;
    }
    if gradient.colors.len() < MAX_VISIBLE_STOPS && ui.button("+ Color stop").clicked() {
        let sample = gradient.sample(0.5);
        gradient.colors.push((0.5, [sample[0], sample[1], sample[2], 1.0]));
        changed = true;
    }
    if gradient.colors.len() > MAX_VISIBLE_STOPS {
        ui.label("Only the first 64 color stops are shown.");
    }

    ui.separator();
    ui.label("Opacity stops");
    let mut remove_opacity = None;
    let opacity_count = gradient.opacities.len();
    for (i, (position, alpha)) in gradient.opacities.iter_mut().take(MAX_VISIBLE_STOPS).enumerate() {
        ui.horizontal(|ui| {
            ui.label(format!("{}", i + 1));
            changed |= ui
                .add(egui::DragValue::new(position).speed(0.01).range(0.0..=1.0))
                .changed();
            changed |= ui
                .add(egui::Slider::new(alpha, 0.0..=1.0).show_value(true))
                .changed();
            if opacity_count > 2 && ui.small_button("Remove").clicked() {
                remove_opacity = Some(i);
            }
        });
    }
    if let Some(i) = remove_opacity {
        gradient.opacities.remove(i);
        changed = true;
    }
    if gradient.opacities.len() < MAX_VISIBLE_STOPS && ui.button("+ Opacity stop").clicked() {
        gradient.opacities.push((0.5, 1.0));
        changed = true;
    }
    if gradient.opacities.len() > MAX_VISIBLE_STOPS {
        ui.label("Only the first 64 opacity stops are shown.");
    }
    if changed {
        sort_stops(gradient);
    }
    changed
}

fn sort_stops(gradient: &mut Gradient) {
    gradient.colors.sort_by(|a, b| a.0.total_cmp(&b.0));
    gradient.opacities.sort_by(|a, b| a.0.total_cmp(&b.0));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reordering_a_stop_keeps_its_color_and_opacity_independent() {
        let mut g = Gradient::default();
        let original_white = g.colors[0].1;
        let original_black = g.colors[1].1;
        g.colors[0].0 = 0.9;
        g.colors[1].0 = 0.1;
        g.opacities[0].1 = 0.25;
        sort_stops(&mut g);
        assert_eq!(g.colors[0], (0.1, original_black));
        assert_eq!(g.colors[1], (0.9, original_white));
        assert_eq!(g.opacities, vec![(0.0, 0.25), (1.0, 1.0)]);
    }
}
