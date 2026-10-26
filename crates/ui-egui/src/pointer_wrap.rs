//! Continuous numeric scrubbing across native monitor edges.

use egui::{Context, Event, Id, PointerButton, Pos2, RawInput, Response, Vec2};
#[cfg(any(not(target_arch = "wasm32"), test))]
use egui::{Rect, pos2, vec2};

#[derive(Clone, Default)]
struct State {
    numeric: Option<(Id, u64)>,
    offset: Vec2,
    pending: Option<(Pos2, Pos2)>,
    disabled: bool,
}

fn state_id() -> Id {
    Id::unique("numeric-pointer-wrap")
}

/// Opt in only number scrubs, never object, keyframe, panel or scrollbar drags.
pub(crate) fn track(response: &Response) {
    if response.dragged_by(PointerButton::Primary) {
        let pass = response.ctx.cumulative_pass_nr();
        response.ctx.data_mut(|d| {
            let state = d.get_temp_mut_or_default::<State>(state_id());
            state.numeric = Some((response.id, pass));
        });
    }
}

/// Keep egui's drag coordinates continuous while the visible native pointer wraps.
/// Rebase only when the OS acknowledges a successful warp: queued events from before
/// it must not be interpreted as another monitor-width movement. A coalesced event
/// may include a little real movement beyond the requested destination.
/// Returns whether a numeric gesture ended, so the app can close its Undo group. `injected`:
/// this frame carries pointer events from the control channel (an agent's `ui.drag`), whose
/// gesture must never move the user's real cursor.
pub(crate) fn prepare_input(ctx: &Context, raw: &mut RawInput, injected: bool) -> bool {
    let lost_focus = !raw.focused || raw.events.iter().any(|e| matches!(e, Event::WindowFocused(false)));
    let (down, last_pos, modifiers) = ctx.input(|i| (i.pointer.primary_down(), i.pointer.latest_pos(), i.modifiers));
    ctx.data_mut(|d| {
        let state = d.get_temp_mut_or_default::<State>(state_id());
        if lost_focus
            && down
            && state.numeric.is_some()
            && let Some(pos) = last_pos
        {
            // The OS may deliver the eventual button-up to another app. Explicitly
            // finish this scrub so focus returning cannot restart it or leave Undo merged.
            raw.events.push(Event::PointerButton { pos: pos - state.offset, button: PointerButton::Primary, pressed: false, modifiers });
        }
        let touch = injected || raw.events.iter().any(|e| matches!(e, Event::Touch { .. }));
        if lost_focus
            || touch
            || raw.events.iter().any(|e| matches!(e, Event::WindowFocused(false) | Event::PointerButton { button: PointerButton::Primary, .. }))
        {
            let ended = state.numeric.is_some();
            *state = State { disabled: touch, ..Default::default() };
            return ended;
        }
        if state.numeric.is_none() {
            return false;
        }
        // Mouse capture can report leaving the client area mid-scrub. Keep the
        // drag's previous coordinate until its next captured movement arrives.
        raw.events.retain(|e| !matches!(e, Event::PointerGone));
        for event in &mut raw.events {
            let p = match event {
                Event::PointerMoved(p) | Event::PointerButton { pos: p, .. } => p,
                _ => continue,
            };
            if let Some((from, to)) = state.pending
                && p.distance_sq(to) < p.distance_sq(from)
            {
                state.offset += from - to;
                state.pending = None;
            }
            *p += state.offset;
        }
        false
    })
}

#[cfg(any(not(target_arch = "wasm32"), test))]
fn wrap_target(p: Pos2, bounds: Rect) -> Option<Pos2> {
    if !p.is_finite() || !bounds.is_finite() || bounds.width() < 32.0 || bounds.height() < 32.0 {
        return None;
    }
    let axis = |v: f32, lo: f32, hi: f32| {
        if v <= lo + 2.0 {
            hi - 8.0
        } else if v >= hi - 2.0 {
            lo + 8.0
        } else {
            v
        }
    };
    let to = pos2(axis(p.x, bounds.min.x, bounds.max.x), axis(p.y, bounds.min.y, bounds.max.y));
    (to != p).then_some(to)
}

#[cfg(any(not(target_arch = "wasm32"), test))]
fn finish_frame(ctx: &Context, bounds: Rect, warp: impl FnOnce(Pos2) -> Option<Pos2>) {
    let state = ctx.data(|d| d.get_temp::<State>(state_id())).unwrap_or_default();
    let active = state.numeric.is_some_and(|(id, pass)| pass == ctx.cumulative_pass_nr() && ctx.is_being_dragged(id));
    let (down, focused, virtual_pos) = ctx.input(|i| (i.pointer.primary_down(), i.focused, i.pointer.latest_pos()));
    if !active || !down || !focused {
        ctx.data_mut(|d| d.remove::<State>(state_id()));
        return;
    }
    if state.disabled || state.pending.is_some() {
        return;
    }
    let Some(from) = virtual_pos.map(|p| p - state.offset) else { return };
    let Some(to) = wrap_target(from, bounds) else { return };
    let destination = warp(to);
    ctx.data_mut(|d| {
        let state = d.get_temp_mut_or_default::<State>(state_id());
        if let Some(to) = destination {
            state.pending = Some((from, to));
        } else {
            // Unsupported backends keep ordinary dragging; do not retry/log every frame.
            state.disabled = true;
        }
    });
    if destination.is_some() {
        ctx.request_repaint();
    }
}

/// Native monitor coordinates are physical pixels; egui input uses UI points.
/// Use the monitor containing the original press, including negative desktop origins.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn finish_native(ctx: &Context, frame: &eframe::Frame) {
    let pass = ctx.cumulative_pass_nr();
    if !ctx.data(|d| d.get_temp::<State>(state_id()).is_some_and(|s| s.numeric.is_some_and(|(_, p)| p == pass))) {
        ctx.data_mut(|d| d.remove::<State>(state_id()));
        return;
    }
    let Some(window) = frame.winit_window() else { return };
    let Ok(origin) = window.inner_position() else { return };
    let ppp = ctx.pixels_per_point();
    if !ppp.is_finite() || ppp <= 0.0 {
        return;
    }
    let Some(press) = ctx.input(|i| i.pointer.press_origin()) else { return };
    let screen_press = press * ppp + vec2(origin.x as f32, origin.y as f32);
    let monitor = window
        .available_monitors()
        .find(|m| {
            let p = m.position();
            let s = m.size();
            Rect::from_min_size(pos2(p.x as f32, p.y as f32), vec2(s.width as f32, s.height as f32)).contains(screen_press)
        })
        .or_else(|| window.current_monitor());
    let Some(monitor) = monitor else { return };
    let p = monitor.position();
    let s = monitor.size();
    let bounds = Rect::from_min_size(
        pos2((p.x as f64 - origin.x as f64) as f32, (p.y as f64 - origin.y as f64) as f32) / ppp,
        vec2(s.width as f32, s.height as f32) / ppp,
    );
    finish_frame(ctx, bounds, |to| {
        // Reuse winit's physical position type obtained above, now client-relative.
        let mut target = origin;
        target.x = (to.x * ppp).round() as i32;
        target.y = (to.y * ppp).round() as i32;
        match window.set_cursor_position(target) {
            // Remember the actual device pixel to avoid fractional-DPI drift on every wrap.
            Ok(()) => Some(pos2(target.x as f32 / ppp, target.y as f32 / ppp)),
            Err(e) => {
                log::warn!("numeric cursor wrap is unavailable: {e}");
                None
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, Id, Modifiers, PointerButton, pos2, vec2};

    struct Scrub {
        ctx: Context,
        value: f64,
        bounds: Rect,
        time: f64,
        accept_warp: bool,
        warps: Vec<Pos2>,
        standard: bool,
        ordinary: bool,
        angle: bool,
        angle_rects: Option<(Rect, Rect)>,
        modifiers: Modifiers,
        injected: bool,
    }

    impl Scrub {
        fn new() -> Self {
            Self {
                ctx: Context::default(),
                value: 0.0,
                bounds: Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0)),
                time: 0.0,
                accept_warp: true,
                warps: Vec::new(),
                standard: false,
                ordinary: false,
                angle: false,
                angle_rects: None,
                modifiers: Modifiers::NONE,
                injected: false,
            }
        }

        fn frame(&mut self, events: Vec<Event>, focused: bool) {
            self.time += 0.02;
            let mut raw = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0))),
                time: Some(self.time),
                events,
                focused,
                ..Default::default()
            };
            raw.events.push(Event::ModifiersChanged(self.modifiers));
            prepare_input(&self.ctx, &mut raw, self.injected);
            let mut out = self.ctx.run_ui(raw, |ui| {
                if self.standard {
                    ui.put(
                        Rect::from_min_size(pos2(100.0, 100.0), vec2(60.0, 20.0)),
                        crate::widgets::drag_value(egui::DragValue::new(&mut self.value).speed(1.0).max_decimals(3)),
                    );
                } else if self.ordinary {
                    ui.interact(Rect::from_min_size(pos2(100.0, 100.0), vec2(60.0, 20.0)), Id::unique("ordinary"), egui::Sense::drag());
                } else if self.angle {
                    let (rr, dr, value) = crate::panels::fx_widgets::angle_field(
                        ui,
                        pos2(100.0, 100.0),
                        Id::unique("scrub"),
                        self.value,
                        1,
                        &crate::theme::Tokens::for_kind(crate::theme::ThemeKind::Dark),
                    );
                    self.angle_rects = Some((rr, dr));
                    if let Some(v) = value {
                        self.value = v;
                    }
                } else {
                    let (_, value, _) = crate::widgets::hot_number_at(
                        ui,
                        pos2(100.0, 100.0),
                        Id::unique("scrub"),
                        self.value,
                        1.0,
                        (-1e9, 1e9),
                        1,
                        "",
                        &crate::theme::Tokens::for_kind(crate::theme::ThemeKind::Dark),
                    );
                    if let Some(v) = value {
                        self.value = v;
                    }
                }
                finish_frame(ui.ctx(), self.bounds, |p| {
                    self.warps.push(p);
                    self.accept_warp.then_some(p)
                });
            });
            out.textures_delta.clear();
        }

        fn move_to(&mut self, p: Pos2) {
            self.frame(vec![Event::PointerMoved(p)], true);
        }

        fn begin(&mut self) {
            self.frame(vec![], true);
            let p = pos2(105.0, 108.0);
            self.frame(
                vec![Event::PointerMoved(p), Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }],
                true,
            );
            self.move_to(p + vec2(10.0, 0.0));
        }
    }

    #[test]
    fn angle_degrees_continue_across_zero_and_repeated_monitor_wraps() {
        for initial in [0.0, 359.5, -359.5, 720.0, -720.0] {
            for direction in [-1.0, 1.0] {
                let mut s = Scrub::new();
                s.angle = true;
                s.value = initial;
                s.frame(vec![], true);
                let p = s.angle_rects.unwrap().1.center();
                s.frame(
                    vec![Event::PointerMoved(p), Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }],
                    true,
                );
                let mut last = p;
                let mut expected = initial;
                // Feed small real movements, including zero/whole-turn crossings, then
                // the synthetic OS warp. Every real pixel contributes exactly once.
                for step in 0..800 {
                    let direction = if step < 400 { direction } else { -direction };
                    let next = pos2((last.x + direction * 13.0).clamp(1.0, 799.0), last.y);
                    expected += f64::from(next.x - last.x) * 0.5;
                    let count = s.warps.len();
                    s.move_to(next);
                    assert!((s.value - expected).abs() < 1e-6, "initial={initial}, direction={direction}, actual={}, expected={expected}", s.value);
                    last = next;
                    if s.warps.len() > count {
                        last = *s.warps.last().unwrap();
                        s.move_to(last);
                        assert!((s.value - expected).abs() < 1e-6, "the teleport must not reset the angle");
                    }
                }
                assert!(s.warps.len() >= 5);
            }
        }
    }

    #[test]
    fn numeric_drag_wraps_repeatedly_without_counting_the_teleport() {
        let mut s = Scrub::new();
        s.begin();
        for _ in 0..5 {
            s.move_to(pos2(799.0, 108.0));
            let target = *s.warps.last().expect("numeric drag must wrap at the monitor edge");
            assert!(target.x < 16.0);
            let before = s.value;
            s.move_to(target);
            assert_eq!(s.value, before, "the warp itself must not change the value");
            s.move_to(target + vec2(20.0, 0.0));
            assert_eq!(s.value, before + 20.0);
        }
        assert_eq!(s.warps.len(), 5);
        s.move_to(pos2(0.0, 108.0));
        let target = *s.warps.last().unwrap();
        assert!(target.x > 780.0);
        let before = s.value;
        s.move_to(target - vec2(3.0, 0.0));
        assert_eq!(s.value, before - 3.0, "coalesced movement after a left-edge warp is preserved");
    }

    /// An agent's `ui.drag` over the control channel scrubs the value but never moves the user's
    /// real cursor, even at the window's (monitor's) edge.
    #[test]
    fn injected_drags_never_warp_the_real_cursor() {
        let mut s = Scrub::new();
        s.injected = true;
        s.begin();
        s.move_to(pos2(799.0, 108.0));
        assert!(s.warps.is_empty());
    }

    #[test]
    fn normal_pointer_movement_does_not_warp() {
        let mut s = Scrub::new();
        s.move_to(pos2(799.0, 108.0));
        assert!(s.warps.is_empty());
    }

    #[test]
    fn standard_editor_wraps_at_top_bottom_and_corner_without_a_value_jump() {
        let mut s = Scrub::new();
        s.standard = true;
        s.begin();
        for edge in [pos2(200.0, 0.0), pos2(200.0, 599.0), pos2(799.0, 599.0)] {
            s.move_to(edge);
            let target = *s.warps.last().unwrap();
            assert_ne!(target, edge);
            let before = s.value;
            s.move_to(target);
            assert_eq!(s.value, before);
            s.move_to(target + vec2(5.0, -5.0));
            assert_eq!(s.value, before + 10.0);
        }
    }

    #[test]
    fn release_and_focus_loss_end_wrapping_and_restore_normal_hit_testing() {
        for lose_focus in [false, true] {
            let mut s = Scrub::new();
            s.begin();
            s.move_to(pos2(799.0, 108.0));
            let target = *s.warps.last().unwrap();
            s.move_to(target);
            let value = s.value;
            let events = if lose_focus {
                vec![Event::WindowFocused(false)]
            } else {
                vec![Event::PointerButton { pos: target, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }]
            };
            s.frame(events, !lose_focus);
            s.move_to(pos2(799.0, 108.0));
            assert_eq!(s.warps.len(), 1);
            assert_eq!(s.value, value);
            assert_eq!(s.ctx.input(|i| i.pointer.latest_pos()), Some(pos2(799.0, 108.0)));
            s.begin();
            s.move_to(pos2(799.0, 108.0));
            assert_eq!(s.warps.len(), 2, "the next independent drag still wraps");
        }
    }

    #[test]
    fn object_drag_does_not_wrap_and_failed_warp_is_not_retried() {
        let mut s = Scrub::new();
        s.ordinary = true;
        s.begin();
        s.move_to(pos2(799.0, 108.0));
        assert!(s.warps.is_empty());

        let mut s = Scrub::new();
        s.accept_warp = false;
        s.begin();
        s.move_to(pos2(799.0, 108.0));
        for _ in 0..3 {
            s.move_to(pos2(799.0, 108.0));
        }
        assert_eq!(s.warps.len(), 1);
        let value = s.value;
        s.move_to(pos2(779.0, 108.0));
        assert_eq!(s.value, value - 20.0);
    }

    #[test]
    fn modifiers_keep_their_sensitivity_after_wrapping() {
        for (modifiers, multiplier) in [(Modifiers::SHIFT, 10.0), (Modifiers::COMMAND, 0.1)] {
            let mut s = Scrub::new();
            s.modifiers = modifiers;
            s.begin();
            s.move_to(pos2(799.0, 108.0));
            let target = *s.warps.last().unwrap();
            let value = s.value;
            s.move_to(target + vec2(10.0, 0.0));
            assert!((s.value - value - 10.0 * multiplier).abs() < 1e-8);
        }
    }

    #[test]
    fn monitor_rect_can_extend_beyond_the_app_and_have_a_negative_origin() {
        let mut s = Scrub::new();
        s.bounds = Rect::from_min_size(pos2(-200.0, -100.0), vec2(1200.0, 800.0));
        s.begin();
        s.move_to(pos2(999.0, 108.0));
        let target = *s.warps.last().unwrap();
        assert_eq!(target, pos2(-192.0, 108.0));
        let value = s.value;
        s.frame(vec![Event::PointerGone, Event::PointerMoved(target)], true);
        assert_eq!(s.value, value);
        s.move_to(target + vec2(10.0, 0.0));
        assert_eq!(s.value, value + 10.0);
    }

    #[test]
    fn real_properties_and_angle_drags_keep_each_gesture_in_one_undo_step() {
        use crate::{EffectcraftApp, dock::PanelKind};
        use effectcraft_engine::project::LayerId;
        use eframe::App;
        use serde_json::json;

        for angle in [false, true] {
            let mut session = effectcraft_host::session();
            session.execute("comp.new", json!({"width": 32, "height": 16})).unwrap();
            let lid = session.execute("layer.newSolid", json!({"width": 8, "height": 8})).unwrap()["layer"].as_u64().unwrap();
            let path = if angle { "transform/rotation" } else { "transform/position" };
            let prop = session.active_comp().unwrap().layer(LayerId(lid)).unwrap().props.prop(path).unwrap();
            let uid = prop.uid;
            let original = prop.value.clone();
            let undo = session.history.undo.len();
            let panel = PanelKind::Properties;
            let id = if angle { format!("properties.prop.{uid}.value") } else { format!("properties.prop.{uid}.value.0") };
            let mut app = EffectcraftApp::new(session);
            app.show_panel(panel);
            app.toggle_maximize(panel);
            let ctx = Context::default();
            let bounds = Rect::from_min_size(Pos2::ZERO, vec2(1600.0, 1000.0));
            let mut frame = eframe::Frame::_new_kittest();
            let mut time = 0.0;
            let mut step = |app: &mut EffectcraftApp, events| {
                time += 1.0 / 60.0;
                let mut raw = RawInput { screen_rect: Some(bounds), time: Some(time), events, ..Default::default() };
                app.raw_input_hook(&ctx, &mut raw);
                let mut warp = None;
                let mut out = ctx.run_ui(raw, |ui| {
                    app.logic(ui.ctx(), &mut frame);
                    app.ui(ui, &mut frame);
                    finish_frame(ui.ctx(), bounds, |p| {
                        warp = Some(p);
                        Some(p)
                    });
                });
                out.textures_delta.clear();
                warp
            };
            for _ in 0..4 {
                step(&mut app, vec![]);
            }
            for gesture in 1..=2 {
                let direction = if gesture == 1 { -1.0 } else { 1.0 };
                let r = app.auto.find(&id).unwrap().rect;
                let p = pos2(r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
                step(
                    &mut app,
                    vec![Event::PointerMoved(p), Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }],
                );
                step(&mut app, vec![Event::PointerMoved(p + vec2(direction * 10.0, 0.0))]);
                let mut last = p;
                for _ in 0..3 {
                    let edge = if direction < 0.0 { 1.0 } else { 1599.0 };
                    let target = step(&mut app, vec![Event::PointerMoved(pos2(edge, p.y))]).expect("real property scrubs wrap");
                    let before = app.session.active_comp().unwrap().layer(LayerId(lid)).unwrap().props.find(uid).unwrap().value.clone();
                    step(&mut app, vec![Event::PointerMoved(target)]);
                    assert_eq!(app.session.active_comp().unwrap().layer(LayerId(lid)).unwrap().props.find(uid).unwrap().value, before);
                    last = target + vec2(direction * 10.0, 0.0);
                    step(&mut app, vec![Event::PointerMoved(last)]);
                    if angle {
                        let value = app.session.active_comp().unwrap().layer(LayerId(lid)).unwrap().props.find(uid).unwrap().value.as_f64();
                        assert!((value - before.as_f64() - f64::from(direction) * 5.0).abs() < 1e-6);
                    }
                }
                step(&mut app, vec![Event::PointerButton { pos: last, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }]);
                assert_eq!(app.session.history.undo.len(), undo + gesture, "each drag is one undo step: {panel:?}");
            }
            app.session.execute("edit.undo", json!({})).unwrap();
            app.session.execute("edit.undo", json!({})).unwrap();
            assert_eq!(app.session.active_comp().unwrap().layer(LayerId(lid)).unwrap().props.find(uid).unwrap().value, original);
        }
    }
}
