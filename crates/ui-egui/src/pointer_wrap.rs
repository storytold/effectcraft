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

/// The temp-data id of the [`State`] that tracks a numeric scrub across monitor edges.
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
mod tests;
