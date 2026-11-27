//! Application-wide color sampling for the eyedropper.
//!
//! Before this module, a color pick was handled entirely inside the Composition viewer: the
//! button armed [`FxPick`] (or a popup/text pick), the viewer changed the cursor and read a
//! pixel out of `app.viewer_image`. A press anywhere else in the window was an ordinary UI
//! click, and the cursor fell back to the widget's own (often an outlined hover rectangle), so
//! sampling seemed to work only in one panel.
//!
//! Here the armed state is observed once, in `raw_input_hook`, before egui routes input to any
//! widget. While a color pick is armed:
//!
//! - the primary press is taken out of the input, so it cannot also press a button or drag a
//!   widget under the crosshair;
//! - the pixel at the pointer is read from the **screen** through the host's
//!   [`Hooks::screen_pixel`] (a real screen-DC read on Windows), converted from egui points to
//!   physical screen pixels with the window's own origin and scale;
//! - the result is delivered to whichever control armed the picker, and the pick is cleared;
//! - Escape cancels without changing the value.
//!
//! The crosshair is the native cursor: the host sets it to `Crosshair` after the last widget
//! has had a chance to change it, for the whole frame. Because it is drawn by the desktop and
//! not painted into egui, it cannot contaminate the pixel being read.

use egui::{Context, Event, Key, PointerButton, Pos2, RawInput, Rect, ViewportId};
use serde_json::json;

use crate::EffectcraftApp;
use crate::state::FxPick;

const STATE_ID: &str = "effectcraft.screen-pick.state";

#[derive(Clone, Copy, Default)]
struct State {
    /// A color picker is armed and owns the pointer.
    armed: bool,
    /// The host can sample real screen pixels (the desktop app, not the web/tests).
    native: bool,
}

fn state(ctx: &Context) -> State {
    ctx.data(|d| d.get_temp::<State>(egui::Id::unique(STATE_ID))).unwrap_or_default()
}

fn set_state(ctx: &Context, s: State) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::unique(STATE_ID), s));
}

/// Whether the host can sample pixels outside the rendered composition frame. When it cannot,
/// the picker keeps its original, viewer-only behaviour.
pub fn native_available(ctx: &Context) -> bool {
    state(ctx).native
}

/// Whether a color picker currently owns the pointer. The host uses this to keep the native
/// crosshair cursor for the whole frame, whatever widget is under the mouse.
pub fn armed(ctx: &Context) -> bool {
    state(ctx).armed
}

/// Keyers sample their own input, in layer space (`effect.pickColor`), not a screen pixel: the
/// displayed frame is already keyed. Their eyedropper therefore keeps the viewer interaction.
fn is_keyer(app: &EffectcraftApp, pick: &FxPick) -> bool {
    let Some(comp) = app.session.active_comp() else { return false };
    let Some(layer) = comp.layer(effectcraft_engine::project::LayerId(pick.layer)) else { return false };
    layer
        .effects()
        .and_then(|fx| fx.groups().find(|g| g.find(pick.prop).is_some()))
        .and_then(|g| effectcraft_engine::effects::find(&g.match_id))
        .is_some_and(|info| info.category == "Keying")
}

/// Whether `pick` will be served by the screen sampler rather than the composition viewer.
pub fn uses_screen(app: &EffectcraftApp, pick: &FxPick) -> bool {
    app.hooks.screen_pixel.is_some() && pick.kind == "color" && !is_keyer(app, pick)
}

/// The same eligibility check for a caller that has a layer id and property uid rather than an
/// [`FxPick`] (the Effect Controls and Timeline tooltips).
pub fn uses_screen_pick(app: &EffectcraftApp, kind: &str, layer: u64, prop: u64) -> bool {
    uses_screen(app, &FxPick { kind: kind.into(), layer, prop, name: String::new() })
}

/// A color pick that the screen sampler should serve: an Effect Controls / Timeline color
/// property (but not a keyer's input sample), a Character panel color, or a color picker popup's
/// eyedropper. Point crosshairs and Cryptomatte picks are deliberately untouched.
fn picking(app: &EffectcraftApp, ctx: &Context) -> bool {
    if crate::color_picker::eyedropper_armed(ctx) {
        return true;
    }
    if let Some(pick) = &app.ui.fx_pick
        && pick.kind == "color"
        && !is_keyer(app, pick)
    {
        return true;
    }
    app.ui.text_color_pick.is_some()
}

/// Map an egui point inside the window to a physical desktop pixel.
///
/// `inner_rect.min` is the window client area's origin in monitor points (egui-winit derives it
/// from `Window::inner_position()`), `pos` is window-local, and `pixels_per_point` is the
/// effective scale (native DPI × egui zoom). Adding the origin before scaling and rounding keeps
/// the mapping correct when the window is moved, maximized, restored, or on a scaled monitor.
pub fn to_screen_pixel(pos: Pos2, inner_rect: Rect, pixels_per_point: f32) -> Option<(i32, i32)> {
    // egui's pointer position is **window-local**; `inner_rect.min` is the window's origin in
    // monitor points. Testing `inner_rect.contains(pos)` here would reject every click once
    // the window is moved away from (0, 0), which is exactly the offset bug this replaces.
    let size = inner_rect.size();
    if pos.x < 0.0 || pos.y < 0.0 || pos.x >= size.x || pos.y >= size.y || !pixels_per_point.is_finite() || pixels_per_point <= 0.0 {
        return None;
    }
    let screen = inner_rect.min + pos.to_vec2();
    let clamp = |v: f32| v.clamp(i32::MIN as f32, i32::MAX as f32).round() as i32;
    Some((clamp(screen.x * pixels_per_point), clamp(screen.y * pixels_per_point)))
}

/// Consume this frame's input while a color pick is armed. Must be called from
/// `App::raw_input_hook`, before egui sees the events.
pub fn capture_input(app: &mut EffectcraftApp, ctx: &Context, raw_input: &mut RawInput) {
    let native = app.hooks.screen_pixel.is_some();
    let active = native && picking(app, ctx);
    set_state(ctx, State { armed: active, native });
    if !active {
        return;
    }

    let mut press = None;
    let mut cancel = false;
    let mut kept = Vec::with_capacity(raw_input.events.len());
    for event in raw_input.events.drain(..) {
        match event {
            Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, .. } => press = Some(pos),
            // Swallow both halves of the primary click: leaving a release without its press
            // would confuse egui's pointer state (and drag gestures) for the next frame.
            Event::PointerButton { button: PointerButton::Primary, .. } => {}
            Event::Key { key: Key::Escape, pressed: true, .. } => cancel = true,
            other => kept.push(other),
        }
    }
    raw_input.events = kept;

    if cancel {
        finish(app, ctx, None);
        return;
    }
    let Some(pos) = press else { return };
    // `viewport()` would panic if a host sent no root viewport; fall back to no sample.
    let inner = raw_input.viewports.get(&ViewportId::ROOT).and_then(|v| v.inner_rect);
    let Some(inner) = inner else { return };
    let Some((x, y)) = to_screen_pixel(pos, inner, ctx.pixels_per_point()) else { return };
    let rgb = app.hooks.screen_pixel.as_ref().and_then(|sample| sample(x, y));
    finish(app, ctx, rgb);
}

/// Apply (or cancel) the sample and disarm every picker that could have claimed it.
fn finish(app: &mut EffectcraftApp, ctx: &Context, rgb: Option<[f32; 3]>) {
    // A popup picker takes priority: its button can be armed while a property's eyedropper is
    // also set, and the popup is the control that is currently in the user's hands.
    if crate::color_picker::eyedropper_armed(ctx) {
        match rgb {
            Some(rgb) => crate::color_picker::deliver_sample(ctx, rgb),
            None => crate::color_picker::disarm_any(ctx),
        }
        // One completed (or cancelled) sample ends the whole pick session. A popup can be
        // armed by keyboard while another owner is still set; leaving that owner armed would
        // make the next ordinary click write to its property.
        app.ui.fx_pick = None;
        app.ui.text_color_pick = None;
        app.ui.status.clear();
    } else if let Some(pick) = app.ui.fx_pick.clone()
        && pick.kind == "color"
        && !is_keyer(app, &pick)
    {
        if let Some(rgb) = rgb {
            let params = json!({"layer": pick.layer, "prop": pick.prop, "value": [rgb[0], rgb[1], rgb[2], 1.0]});
            match crate::menus::invoke(app, ctx, "prop.set", params) {
                Ok(_) => app.ui.status.clear(),
                Err(e) => app.ui.status = e,
            }
        }
        app.ui.fx_pick = None;
    } else if let Some(pick) = app.ui.text_color_pick.clone() {
        if let Some(rgb) = rgb {
            let mut params = json!({"layer": pick.layer});
            params[&pick.field] = json!([rgb[0], rgb[1], rgb[2]]);
            if let Some(range) = pick.range {
                params["range"] = json!(range);
            }
            match crate::menus::invoke(app, ctx, "layer.setText", params) {
                Ok(_) => app.ui.status.clear(),
                Err(e) => app.ui.status = e,
            }
        }
        app.ui.text_color_pick = None;
    }
    set_state(ctx, State { armed: false, native: state(ctx).native });
    ctx.request_repaint();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect() -> Rect {
        Rect::from_min_size(Pos2::new(0.0, 0.0), egui::vec2(800.0, 600.0))
    }

    #[test]
    fn maps_window_origin_and_dpi_scale() {
        let moved = Rect::from_min_size(Pos2::new(320.0, 180.0), egui::vec2(800.0, 600.0));
        assert_eq!(to_screen_pixel(Pos2::new(10.0, 20.0), rect(), 1.0), Some((10, 20)));
        assert_eq!(to_screen_pixel(Pos2::new(10.0, 20.0), moved, 1.0), Some((330, 200)));
        assert_eq!(to_screen_pixel(Pos2::new(10.5, 20.5), moved, 2.0), Some((661, 401)));
        assert_eq!(to_screen_pixel(Pos2::new(1.0, 1.0), moved, 1.5), Some((482, 272)));
    }

    #[test]
    fn rejects_points_outside_the_client_area() {
        assert_eq!(to_screen_pixel(Pos2::new(-1.0, 5.0), rect(), 1.0), None);
        assert_eq!(to_screen_pixel(Pos2::new(5.0, 601.0), rect(), 1.0), None);
        assert_eq!(to_screen_pixel(Pos2::new(800.0, 5.0), rect(), 1.0), None);
        assert_eq!(to_screen_pixel(Pos2::new(5.0, 5.0), rect(), 0.0), None);
    }

    #[test]
    fn state_defaults_to_viewer_only() {
        let ctx = egui::Context::default();
        assert!(!armed(&ctx));
        assert!(!native_available(&ctx));
    }

    fn app_with_pick(sampler: Option<Box<dyn Fn(i32, i32) -> Option<[f32; 3]>>>) -> EffectcraftApp {
        let mut app = EffectcraftApp::new(effectcraft_engine::Session::default());
        app.hooks.screen_pixel = sampler;
        app.ui.fx_pick = Some(FxPick { kind: "color".into(), layer: 1, prop: 2, name: "Color A".into() });
        app
    }

    fn input(events: Vec<Event>) -> RawInput {
        let mut raw = RawInput::default();
        raw.viewports.insert(
            ViewportId::ROOT,
            egui::ViewportInfo { inner_rect: Some(Rect::from_min_size(Pos2::new(320.0, 180.0), egui::vec2(800.0, 600.0))), ..Default::default() },
        );
        raw.events = events;
        raw
    }

    fn click() -> Event {
        Event::PointerButton { pos: Pos2::new(10.0, 20.0), button: PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE }
    }

    #[test]
    fn armed_click_is_consumed_and_samples_the_window_pixel() {
        let ctx = egui::Context::default();
        let calls = std::rc::Rc::new(std::cell::Cell::new(0));
        let counted = calls.clone();
        let mut app = app_with_pick(Some(Box::new(move |x, y| {
            assert_eq!((x, y), (330, 200));
            counted.set(counted.get() + 1);
            Some([0.1, 0.2, 0.3])
        })));
        let mut raw = input(vec![click()]);
        capture_input(&mut app, &ctx, &mut raw);
        assert!(raw.events.is_empty(), "the pick must own the click");
        assert_eq!(calls.get(), 1);
        assert!(app.ui.fx_pick.is_none());
        assert!(!armed(&ctx));
    }

    #[test]
    fn escape_cancels_without_sampling_or_changing_the_value() {
        let ctx = egui::Context::default();
        let calls = std::rc::Rc::new(std::cell::Cell::new(0));
        let counted = calls.clone();
        let mut app = app_with_pick(Some(Box::new(move |_, _| {
            counted.set(counted.get() + 1);
            Some([1.0, 0.0, 0.0])
        })));
        let mut raw = input(vec![Event::Key { key: Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE }]);
        capture_input(&mut app, &ctx, &mut raw);
        assert!(raw.events.is_empty());
        assert_eq!(calls.get(), 0);
        assert!(app.ui.fx_pick.is_none());
        assert!(!armed(&ctx));
    }

    #[test]
    fn without_a_host_sampler_the_viewer_keeps_the_click() {
        let ctx = egui::Context::default();
        let mut app = app_with_pick(None);
        let mut raw = input(vec![click()]);
        capture_input(&mut app, &ctx, &mut raw);
        assert_eq!(raw.events.len(), 1);
        assert!(app.ui.fx_pick.is_some());
        assert!(!armed(&ctx));
    }
}
