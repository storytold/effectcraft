//! The Roto Brush span bar colours the frames a tracker followed by its confidence: green when it is sure,
//! amber when it is doubtful, red when it is not, slate where the object was absent (egui_kittest, UI logic
//! and a real render). A stand-in tracker supplies the confidences, so no weights are needed.
//! `ROTO_SPAN_SNAPSHOT=/abs/out.png` also saves the rendered panel.

use std::sync::Arc;

use effectcraft_engine::Session;
use effectcraft_segment::{ClipFrame, MaskModel, ModelInfo, ObjectTracker, Prompt, TrackRequest, TrackedFrame};
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::dock::PanelKind;
use egui_kittest::Harness;
use serde_json::json;

/// Follows a fixed disc; its confidence per frame is scripted.
struct Scripted;

impl MaskModel for Scripted {
    fn info(&self) -> &'static ModelInfo {
        &effectcraft_segment::SAM2_TINY
    }
    fn segment(&self, _: &[[f32; 3]], _: usize, _: usize, _: &Prompt) -> effectcraft_segment::Result<Vec<f32>> {
        Err("no weights".into())
    }
    fn tracker(&self) -> Option<&dyn ObjectTracker> {
        Some(self)
    }
}

/// (confidence, occluded) of layer frame `f`: sure, doubtful, unsure, absent, sure again.
fn script(f: i64) -> (f32, bool) {
    match f {
        0..=5 => (0.99, false),
        6..=9 => (0.6, false),
        10..=12 => (0.2, false),
        13..=15 => (0.0, true),
        _ => (0.99, false),
    }
}

impl ObjectTracker for Scripted {
    fn track(
        &self,
        req: &TrackRequest,
        frame: &mut dyn FnMut(i64) -> Option<Arc<ClipFrame>>,
        _: &dyn Fn() -> bool,
        out: &mut dyn FnMut(usize, i64, TrackedFrame),
    ) -> effectcraft_segment::Result<()> {
        for g in req.first..req.first + req.count as i64 {
            let img = frame(g).ok_or("no frame")?;
            let (confidence, occluded) = script(g);
            let prob = (0..img.w * img.h)
                .map(|i| if !occluded && (i % img.w).abs_diff(img.w / 2) < img.w / 4 && (i / img.w).abs_diff(img.h / 2) < img.h / 4 { 1.0 } else { 0.0 })
                .collect();
            out(0, g, TrackedFrame { prob, w: img.w, h: img.h, confidence, occluded });
        }
        Ok(())
    }
}

fn close(got: [u8; 4], want: [u8; 3]) -> bool {
    got.iter().zip(want).all(|(a, b)| a.abs_diff(b) <= 24)
}

#[test]
fn the_span_bar_shows_the_trackers_confidence() {
    let _gpu = crate::gpu_test::lock();
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "View", "width": 640, "height": 360, "duration": 4})).unwrap();
    s.execute("layer.newSolid", json!({"name": "Box", "color": "#e04020", "width": 80, "height": 80})).unwrap();
    let id = s.active_comp().unwrap().layers.iter().find(|l| l.name == "Box").unwrap().id.0;
    s.execute("roto.stroke", json!({"layer": id, "points": [[20.0, 40.0], [60.0, 40.0]], "radius": 6})).unwrap();
    s.execute("roto.stroke", json!({"layer": id, "kind": "bg", "points": [[5.0, 5.0], [75.0, 5.0]], "radius": 4})).unwrap();
    let _model = crate::roto_model_guard::install(Arc::new(Scripted));
    // the model stays loaded while the panel draws (its id is part of the cache keys)
    s.execute("roto.propagate", json!({"layer": id, "direction": "both", "wait": true})).unwrap();

    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(move |_| EffectcraftApp::new(s));
    h.run_steps(3);
    h.state_mut().session.execute("layer.select", json!({"layers": [id]})).unwrap();
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "layer.openLayer", json!({"layer": id})).unwrap();
    h.run_steps(4);
    assert_eq!(h.state().ui.focused, PanelKind::Layer);

    // the object picker is in the bar; "+" starts a new object, which the next stroke would belong to
    assert!(h.state().auto.find("layerPanel.roto.object").is_some(), "the object picker is on screen");
    assert!(h.state().auto.find("layerPanel.roto.newObject").is_some());
    assert_eq!(h.state().session.state.roto.object, 0);
    h.state_mut().session.execute("roto.options", json!({"object": 1})).unwrap();
    h.run_steps(2);
    assert_eq!(h.state().session.state.roto.object, 1);
    h.state_mut().session.execute("roto.options", json!({"object": 0})).unwrap();
    h.run_steps(2);
    let track = h.state().auto.find("layerPanel.rotoSpan").expect("the span bar is on screen").rect;
    let (comp, layer) = {
        let comp = h.state().session.active_comp().unwrap().clone();
        let layer = comp.layer(effectcraft_project::LayerId(id)).unwrap().clone();
        (comp, layer)
    };
    let lim = effectcraft_engine::roto::frame_limits(&comp, &layer);
    let n = (lim[1] - lim[0] + 1) as f32;
    let image = h.render().expect("the panel renders");
    if let Some(path) = std::env::var_os("ROTO_SPAN_SNAPSHOT") {
        image.save(path).unwrap();
    }
    let scale = image.width() as f32 / 1600.0;
    let sample = |f: i64| -> [u8; 4] {
        let x = (track[0] + ((f - lim[0]) as f32 + 0.5) / n * track[2]) * scale;
        let y = (track[1] + track[3] - 2.0) * scale;
        image.get_pixel(x as u32, y as u32).0
    };
    // the colours the bar uses (see `trust_color`)
    let (green, amber, red, slate) = ([70, 200, 90], [230, 180, 50], [220, 70, 60], [110, 118, 150]);
    for (f, want, what) in [(2, green, "sure"), (7, amber, "doubtful"), (11, red, "unsure"), (14, slate, "absent"), (18, green, "sure again")] {
        let got = sample(f);
        assert!(close(got, want), "frame {f} ({what}): drew {got:?}, wanted about {want:?}");
    }
}
