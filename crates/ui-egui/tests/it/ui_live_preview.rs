//! Completed edits must reach the viewer even when a newer edit is still rendering.

use std::sync::{Arc, Mutex};

use effectcraft_engine::{
    Session,
    project::LayerId,
    render::{NoFootage, Renderer},
};
use effectcraft_ui_egui::{
    EffectcraftApp,
    frames::{RemoteDone, RemoteFrame, RemoteFrames, RemoteJob, to_color_image},
};
use egui_kittest::Harness;
use serde_json::json;

/// Hold completions explicitly: input/render ordering is deterministic, without timing sleeps.
#[derive(Default)]
struct HeldRenderer(Mutex<Vec<(RemoteJob, RemoteDone)>>);

impl RemoteFrames for HeldRenderer {
    fn slots(&self) -> usize {
        1
    }
    fn start(&self, job: RemoteJob, done: RemoteDone) {
        self.0.lock().unwrap().push((job, done));
    }
}

impl HeldRenderer {
    fn finish(&self) -> egui::ColorImage {
        let (job, done) = self.0.lock().unwrap().remove(0);
        let image = Renderer::new(&job.project, &NoFootage, job.opts).comp_frame(job.comp, job.t);
        let pixels = to_color_image(&image);
        done(Ok(RemoteFrame { width: image.width, height: image.height, rgba: pixels.pixels.iter().flat_map(|c| c.to_array()).collect(), ms: 20.0 }));
        pixels
    }
}

#[test]
fn opacity_edits_show_completed_intermediate_frames_before_the_drag_stops() {
    let mut session = Session::default();
    session.execute("comp.new", json!({"name":"Live opacity", "width":64, "height":36, "duration":1})).unwrap();
    let layer = session.execute("layer.newSolid", json!({"color":"#ffffff"})).unwrap()["layer"].as_u64().unwrap();
    let renderer = Arc::new(HeldRenderer::default());
    let mut app = EffectcraftApp::new(session);
    app.frames.set_remote(Some(renderer.clone()));
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(0.016).build_eframe(|_| app);
    h.run_steps(3);
    assert_eq!(h.state().frames.dispatch_remote(), 1);
    renderer.finish();
    h.step();
    let alpha = |h: &mut Harness<'_, EffectcraftApp>| h.state_mut().viewer_pixels().unwrap().pixels[32 + 18 * 64].a();
    assert_eq!(alpha(&mut h), 255);
    for opacity in [80, 60, 40, 20] {
        h.state_mut().session.execute("prop.set", json!({"layer":layer, "path":"transform/opacity", "value":opacity, "merge":"scrub"})).unwrap();
        h.step();
        assert_eq!(h.state().frames.dispatch_remote(), 1);
        // Another input arrives before that frame completes, as in a fast slider drag.
        h.state_mut().session.execute("prop.set", json!({"layer":layer, "path":"transform/opacity", "value":opacity - 1, "merge":"scrub"})).unwrap();
        renderer.finish();
        h.step();
        assert_eq!(alpha(&mut h), (opacity as f32 * 2.55).round() as u8, "completed {opacity}% opacity must display while the next edit renders");
    }
    // Releasing the slider still converges to the exact final state.
    assert_eq!(h.state().frames.dispatch_remote(), 1);
    renderer.finish();
    h.step();
    assert_eq!(alpha(&mut h), (19.0_f32 * 2.55).round() as u8);
    let series = h.state().shown_series(h.state().session.active_comp_id().unwrap());
    assert!(h.state().frames.is_cached(&series));
}

#[test]
fn position_anchor_and_effect_edits_present_intermediate_pixels() {
    for (effect, path, first, second) in [
        (None, "transform/position", json!([22, 18, 0]), json!([18, 18, 0])),
        (None, "transform/anchor", json!([9, 8, 0]), json!([6, 8, 0])),
        (Some("Gaussian Blur"), "blurriness", json!(3), json!(6)),
        (Some("CC Radial Fast Blur"), "center", json!([4, 4]), json!([18, 12])),
    ] {
        let mut session = Session::default();
        session.execute("comp.new", json!({"width":64,"height":36,"duration":1})).unwrap();
        let layer = session.execute("layer.newSolid", json!({"color":"#ffffff","width":24,"height":16})).unwrap()["layer"].as_u64().unwrap();
        if effect == Some("CC Radial Fast Blur") {
            // A uniform solid is invariant under a radial blur; give the point edit visible detail.
            session.execute("effect.apply", json!({"layers":[layer],"effect":"Gradient Ramp"})).unwrap();
        }
        let property = if let Some(effect) = effect {
            session.execute("effect.apply", json!({"layers":[layer],"effect":effect})).unwrap();
            session.active_comp().unwrap().layer(LayerId(layer)).unwrap().effects().unwrap().groups().last().unwrap().get(path).unwrap().uid
        } else {
            session.active_comp().unwrap().layer(LayerId(layer)).unwrap().props.prop(path).unwrap().uid
        };
        let renderer = Arc::new(HeldRenderer::default());
        let mut app = EffectcraftApp::new(session);
        app.frames.set_remote(Some(renderer.clone()));
        let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(0.016).build_eframe(|_| app);
        h.run_steps(3);
        h.state().frames.dispatch_remote();
        let initial = renderer.finish();
        h.step();
        h.state_mut().session.execute("prop.set", json!({"layer":layer,"prop":property,"value":first,"merge":"drag"})).unwrap();
        h.step();
        assert_eq!(h.state().frames.dispatch_remote(), 1);
        h.state_mut().session.execute("prop.set", json!({"layer":layer,"prop":property,"value":second,"merge":"drag"})).unwrap();
        let intermediate = renderer.finish();
        assert!(initial.pixels != intermediate.pixels, "{effect:?} {path}: fixture must change rendered pixels");
        h.step();
        assert_eq!(h.state_mut().viewer_pixels().unwrap().pixels, intermediate.pixels, "{effect:?} {path}: display finished edit while next edit renders");
        assert_eq!(h.state().frames.dispatch_remote(), 1);
        let final_image = renderer.finish();
        h.step();
        assert_eq!(h.state_mut().viewer_pixels().unwrap().pixels, final_image.pixels, "{effect:?} {path}: converge to exact final edit");
    }
}

/// Manual throughput probe: no timing threshold in CI. Includes egui layout and native workers,
/// but no window compositor or GPU upload; it measures presentation progress during edits.
#[test]
#[ignore = "manual native-worker preview throughput probe"]
fn continuous_opacity_preview_probe() {
    use std::time::{Duration, Instant};
    let mut session = Session::default();
    session.execute("comp.new", json!({"width":1920,"height":1080,"duration":1})).unwrap();
    let layer = session.execute("layer.newSolid", json!({"color":"#ffffff"})).unwrap()["layer"].as_u64().unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(1.0 / 60.0).build_eframe(|_| EffectcraftApp::new(session));
    let ready = Instant::now();
    while h.state_mut().viewer_pixels().is_none() && ready.elapsed() < Duration::from_secs(5) {
        h.step();
        std::thread::sleep(Duration::from_millis(5));
    }
    let mut last = h.state_mut().viewer_pixels().expect("initial viewer frame");
    let start = Instant::now();
    let mut updates = 0;
    let mut longest_hold = Duration::ZERO;
    let mut last_update = start;
    for i in 0..120 {
        h.state_mut().session.execute("prop.set", json!({"layer":layer,"path":"transform/opacity","value":99-i%90,"merge":"scrub"})).unwrap();
        h.step();
        let now = h.state_mut().viewer_pixels().unwrap();
        if !Arc::ptr_eq(&now, &last) {
            updates += 1;
            longest_hold = longest_hold.max(last_update.elapsed());
            last_update = Instant::now();
            last = now;
        }
        let deadline = start + Duration::from_secs_f64((i + 1) as f64 / 60.0);
        std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
    }
    longest_hold = longest_hold.max(last_update.elapsed());
    println!(
        "LIVE_PREVIEW_PROBE updates={updates}/120 elapsed_ms={:.1} longest_hold_ms={:.1} last_render_ms={:.3}",
        start.elapsed().as_secs_f64() * 1000.0,
        longest_hold.as_secs_f64() * 1000.0,
        *h.state().frames.last_ms.lock().unwrap()
    );
}
