//! The shape a shape tool is dragging out, and the keys that change it before the mouse button
//! is released: Up / Down add or remove a point of a polygon or star, or make a rounded
//! rectangle's corners rounder or sharper; Left / Right lower or raise the Outer Roundness of a
//! polygon or star, or make a rounded rectangle's corners sharp or fully round; Page Up / Page
//! Down raise or lower a star's Inner Roundness. Holding Spacebar moves the shape with the
//! pointer instead of resizing it, and holding Cmd (Ctrl) changes a star's outer radius alone.

use effectcraft_engine::commands::shape_tool::ShapeOptions;
use egui::Key;

/// One press of Up / Down on a rounded rectangle's corners, and of the roundness keys, %.
const STEP: f64 = 10.0;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Draft {
    /// The `shape.newShape` kind being dragged out.
    kind: &'static str,
    pub options: ShapeOptions,
    /// The pointer at the last frame (comp px): Spacebar moves the shape by its travel.
    last: Option<[f64; 2]>,
    roundness_base: Option<f64>,
    /// The star's inner radius (comp px) at the last frame without Cmd: what Cmd holds still.
    inner: Option<f64>,
}

fn id() -> egui::Id {
    egui::Id::unique("viewer-shape-draft")
}

/// A shape tool starts dragging a `kind` out.
pub(crate) fn begin(ctx: &egui::Context, kind: &'static str) {
    let options = ctx.data(|d| d.get_temp::<ShapeOptions>(egui::Id::unique(("shape-tool-settings", kind)))).unwrap_or_default();
    ctx.data_mut(|d| d.insert_temp(id(), Draft { kind, options, ..Default::default() }));
}

/// The drag ended; returns what the keys made of the shape.
pub(crate) fn end(ctx: &egui::Context) -> ShapeOptions {
    ctx.data_mut(|d| {
        let Some(draft) = d.remove_temp::<Draft>(id()) else { return ShapeOptions::default() };
        d.insert_temp(egui::Id::unique(("shape-tool-settings", draft.kind)), draft.options);
        draft.options
    })
}

fn get(ctx: &egui::Context) -> Option<Draft> {
    ctx.data(|d| d.get_temp(id()))
}

pub(crate) fn active(ctx: &egui::Context) -> bool {
    get(ctx).is_some()
}

pub(crate) fn reset(ctx: &egui::Context, kind: &'static str) {
    ctx.data_mut(|d| d.remove::<ShapeOptions>(egui::Id::unique(("shape-tool-settings", kind))));
}

/// A key event while a shape is being dragged out: true when the draft takes it (the arrows and
/// Page Up / Page Down change the shape; Spacebar is read as held, so it must not preview).
pub(crate) fn key(ctx: &egui::Context, key: Key, pressed: bool) -> bool {
    let Some(mut d) = get(ctx) else { return false };
    if !matches!(key, Key::ArrowUp | Key::ArrowDown | Key::ArrowLeft | Key::ArrowRight | Key::PageUp | Key::PageDown | Key::Space) {
        return false;
    }
    if pressed {
        d.press(key);
        ctx.data_mut(|m| m.insert_temp(id(), d));
        ctx.request_repaint();
    }
    true
}

impl Draft {
    fn press(&mut self, key: Key) {
        let o = &mut self.options;
        let round = |v: f64, by: f64| (v + by).clamp(ShapeOptions::ROUND.0, ShapeOptions::ROUND.1);
        match (self.kind, key) {
            ("rounded", Key::ArrowUp | Key::ArrowDown) => {
                let by = if key == Key::ArrowUp { STEP } else { -STEP };
                o.roundness = Some((o.roundness.unwrap_or(self.roundness_base.unwrap_or(30.0)) + by).clamp(0.0, 100.0));
            }
            ("rounded", Key::ArrowLeft) => o.roundness = Some(0.0),
            ("rounded", Key::ArrowRight) => o.roundness = Some(100.0),
            ("polygon" | "star", Key::ArrowUp | Key::ArrowDown) => {
                let by = if key == Key::ArrowUp { 1.0 } else { -1.0 };
                let tool = 5.0;
                o.points = Some((o.points.unwrap_or(tool) + by).clamp(ShapeOptions::POINTS.0, ShapeOptions::POINTS.1));
            }
            ("polygon" | "star", Key::ArrowLeft) => o.outer_roundness = round(o.outer_roundness, -STEP),
            ("polygon" | "star", Key::ArrowRight) => o.outer_roundness = round(o.outer_roundness, STEP),
            ("star", Key::PageUp) => o.inner_roundness = round(o.inner_roundness, STEP),
            ("star", Key::PageDown) => o.inner_roundness = round(o.inner_roundness, -STEP),
            _ => {}
        }
    }
}

/// Advance the draft to the pointer at `cpt` (comp px) for a drag that started at `start`:
/// returns where the drag starts now (Spacebar held moves it along with the pointer) and the
/// shape's options (Cmd held on a star keeps its inner radius where it was).
pub(crate) fn frame(ctx: &egui::Context, start: [f64; 2], cpt: [f64; 2], mods: egui::Modifiers) -> ([f64; 2], ShapeOptions) {
    let Some(mut d) = get(ctx) else { return (start, ShapeOptions::default()) };
    let mut start = start;
    if let Some(last) = d.last
        && ctx.input(|i| i.key_down(Key::Space))
    {
        start = [start[0] + cpt[0] - last[0], start[1] + cpt[1] - last[1]];
    }
    if d.kind == "rounded" && d.options.roundness.is_none() {
        let mut size = [(cpt[0] - start[0]).abs(), (cpt[1] - start[1]).abs()];
        if mods.command {
            size = size.map(|v| v * 2.0);
        }
        let shorter = if mods.shift { size[0].max(size[1]) } else { size[0].min(size[1]) };
        if shorter > 1e-6 {
            d.roundness_base = Some((40.0 / shorter * 100.0).clamp(0.0, 100.0));
        }
    }
    d.last = Some(cpt);
    if d.kind == "star" {
        let outer = (cpt[0] - start[0]).hypot(cpt[1] - start[1]);
        match d.inner {
            Some(inner) if mods.command && outer > 1e-6 => d.options.inner_radius = Some((inner / outer * 100.0).clamp(0.0, 100.0)),
            _ => d.inner = Some(outer * d.options.inner_radius.unwrap_or(50.0) / 100.0),
        }
    }
    ctx.data_mut(|m| m.insert_temp(id(), d));
    (start, d.options)
}

/// Drafting consumes wheel input and its smoothed tail before viewer zoom/pan/scrubbing.
pub(crate) fn capture_wheel(ctx: &egui::Context, scroll: &mut super::ViewerScroll) {
    let raw = ctx.input(|i| {
        i.events
            .iter()
            .filter_map(|e| match e {
                egui::Event::MouseWheel { delta, source, .. } if *source != egui::MouseWheelSource::Momentum => Some(delta.y),
                _ => None,
            })
            .sum::<f32>()
    });
    let id = egui::Id::unique("viewer-shape-wheel-capture");
    let was = ctx.data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
    let drafting = active(ctx);
    let moving = scroll.pan != egui::Vec2::ZERO || (scroll.zoom - 1.0).abs() > 3e-4;
    let capture = drafting || (was && raw == 0.0 && moving);
    ctx.data_mut(|d| d.insert_temp(id, capture));
    if drafting && raw != 0.0 {
        key(ctx, if raw > 0.0 { Key::ArrowUp } else { Key::ArrowDown }, true);
    }
    if capture {
        scroll.pan = egui::Vec2::ZERO;
        scroll.zoom = 1.0;
        scroll.wheel_lines = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft(kind: &'static str, keys: &[Key]) -> ShapeOptions {
        let mut d = Draft { kind, ..Default::default() };
        for k in keys {
            d.press(*k);
        }
        d.options
    }

    #[test]
    fn keys_change_the_shape_of_their_tool_only() {
        assert_eq!(draft("star", &[Key::ArrowUp, Key::ArrowUp]).points, Some(7.0));
        assert_eq!(draft("polygon", &[Key::ArrowDown; 9]).points, Some(3.0), "never fewer than three");
        assert_eq!(draft("polygon", &[Key::ArrowRight, Key::ArrowRight, Key::ArrowLeft]).outer_roundness, 10.0);
        assert_eq!(draft("star", &[Key::PageUp, Key::PageUp, Key::PageDown]).inner_roundness, 10.0);
        assert_eq!(draft("polygon", &[Key::PageUp]), ShapeOptions::default(), "a polygon has no inner roundness");
        assert_eq!(draft("rounded", &[Key::ArrowUp]).roundness, Some(40.0));
        assert_eq!(draft("rounded", &[Key::ArrowLeft]).roundness, Some(0.0));
        assert_eq!(draft("rounded", &[Key::ArrowRight, Key::ArrowDown]).roundness, Some(90.0));
        for kind in ["rect", "ellipse"] {
            assert_eq!(draft(kind, &[Key::ArrowUp, Key::ArrowLeft, Key::PageUp]), ShapeOptions::default());
        }
    }
}
