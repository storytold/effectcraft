//! The host's hooks and when they run. A hidden window gets logic ticks and no paint.

use std::cell::Cell;
use std::rc::Rc;

use effectcraft_engine::Session;
use effectcraft_ui_egui::EffectcraftApp;
use egui_kittest::Harness;

/// The per-tick hook runs on a logic tick that never paints, the per-frame hook does not.
/// The plug-in host serves the calls other threads queued from the per-tick hook, so a hidden
/// window must reach it.
#[test]
fn the_per_tick_hook_runs_on_a_logic_tick_and_the_per_frame_hook_does_not() {
    let ticks = Rc::new(Cell::new(0u32));
    let frames = Rc::new(Cell::new(0u32));
    let mut h = Harness::builder().with_size(egui::vec2(800.0, 600.0)).build_eframe({
        let (ticks, frames) = (Rc::clone(&ticks), Rc::clone(&frames));
        move |_| {
            let mut app = EffectcraftApp::new(Session::default());
            app.hooks.per_tick = Some(Box::new(move |_, _| ticks.set(ticks.get() + 1)));
            app.hooks.per_frame = Some(Box::new(move |_, _| frames.set(frames.get() + 1)));
            app
        }
    });
    h.run_steps(2);
    let (ticks_painted, frames_painted) = (ticks.get(), frames.get());
    assert!(ticks_painted > 0 && frames_painted > 0, "a painted step runs both hooks");

    let ctx = h.ctx.clone();
    let frame = eframe::Frame::_new_kittest();
    let raw = egui::RawInput::default();
    for _ in 0..3 {
        let _ = ctx.run_logic(&raw, |ctx| h.state_mut().tick(ctx, &frame));
    }
    assert_eq!(ticks.get(), ticks_painted + 3, "every logic tick runs the per-tick hook");
    assert_eq!(frames.get(), frames_painted, "a logic tick never draws, so it never runs the per-frame hook");
}
