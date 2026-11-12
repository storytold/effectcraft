//! #1118: a preview with Video off in the Preview panel says so while it plays.

use effectcraft_engine::Session;
use effectcraft_ui_egui::EffectcraftApp;
use egui_kittest::Harness;
use serde_json::json;

fn frame(h: &Harness<'_, EffectcraftApp>) -> i64 {
    let app = h.state();
    let c = app.session.active_comp().unwrap();
    c.frame_rate.frame_at(app.session.time())
}

/// #1118: with Video off in the Preview panel the preview plays audio only and the current time
/// stays put; the viewer says so while it plays, and stops saying so once it stops.
#[test]
fn an_audio_only_preview_says_video_preview_is_off() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Muted", "width": 320, "height": 180, "duration": 10})).unwrap();
    let sc = s.prefs.preview.current;
    s.prefs.preview.get_mut(sc).include_video = false;
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(1.0 / 30.0).build_eframe(|_| EffectcraftApp::new(s));
    h.run_steps(4);
    let chip = |h: &Harness<'_, EffectcraftApp>| h.ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::unique("viewer-video-off-chip")));
    assert!(chip(&h).is_none());
    let now = h.ctx.input(|i| i.time);
    h.state_mut().play(now);
    h.run_steps(10);
    assert!(h.state().video_preview_off().is_some());
    assert!(chip(&h).is_some(), "the viewer shows the Video-off notice");
    assert_eq!(frame(&h), 0, "the current time stays put with Video off");
    h.state_mut().stop();
    h.ctx.data_mut(|d| d.remove::<egui::Rect>(egui::Id::unique("viewer-video-off-chip")));
    h.run_steps(3);
    assert!(h.state().video_preview_off().is_none());
    assert!(chip(&h).is_none(), "no notice once stopped");
}
