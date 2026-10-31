//! A Roto Brush instance with two objects: the Layer panel's boundary view outlines each in its own colour, and
//! Show Object limits the matte to one (egui_kittest, a real render). A stand-in tracker follows two regions, so no
//! weights are needed. `ROTO_OBJECTS_SNAPSHOT=/abs/out.png` also saves the rendered panel.

use std::sync::Arc;

use effectcraft_engine::Session;
use effectcraft_segment::{ClipFrame, MaskModel, ModelInfo, ObjectTracker, Prompt, TrackRequest, TrackedFrame};
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::dock::PanelKind;
use egui_kittest::Harness;
use serde_json::json;

/// Follows two fixed regions of the layer: object 0 on the left, object 1 on the right.
struct TwoRegions;

impl MaskModel for TwoRegions {
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

impl ObjectTracker for TwoRegions {
    fn track(
        &self,
        req: &TrackRequest,
        frame: &mut dyn FnMut(i64) -> Option<Arc<ClipFrame>>,
        _: &dyn Fn() -> bool,
        out: &mut dyn FnMut(usize, i64, TrackedFrame),
    ) -> effectcraft_segment::Result<()> {
        for g in req.first..req.first + req.count as i64 {
            let img = frame(g).ok_or("no frame")?;
            for o in 0..req.objects.len() {
                let (x0, x1) = if o == 0 { (img.w / 8, img.w * 3 / 8) } else { (img.w * 5 / 8, img.w * 7 / 8) };
                let prob = (0..img.w * img.h)
                    .map(|i| {
                        let (x, y) = (i % img.w, i / img.w);
                        f32::from(x >= x0 && x < x1 && y > img.h / 4 && y < img.h * 3 / 4)
                    })
                    .collect();
                out(o, g, TrackedFrame { prob, w: img.w, h: img.h, confidence: 0.99, occluded: false });
            }
        }
        Ok(())
    }
}

/// Pixels of the viewer (above the panel's bar, whose object swatch has the same colour) within a few levels of `want`.
fn count(image: &image::RgbaImage, want: [u8; 3]) -> usize {
    let limit = image.height() * 54 / 100;
    image.enumerate_pixels().filter(|(_, y, p)| *y < limit && p.0[..3].iter().zip(want).all(|(a, b)| a.abs_diff(b) <= 12)).count()
}

#[test]
fn the_boundary_view_outlines_each_object_and_show_object_picks_one() {
    let _gpu = crate::gpu_test::lock();
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "View", "width": 640, "height": 360, "duration": 4})).unwrap();
    s.execute("layer.newSolid", json!({"name": "Box", "color": "#e04020", "width": 80, "height": 80})).unwrap();
    let id = s.active_comp().unwrap().layers.iter().find(|l| l.name == "Box").unwrap().id.0;
    for (object, x) in [(0, 22.0), (1, 58.0)] {
        s.execute("roto.stroke", json!({"layer": id, "object": object, "points": [[x, 38.0], [x, 42.0]], "radius": 4})).unwrap();
    }
    let _model = crate::roto_model_guard::install(Arc::new(TwoRegions));
    s.execute("roto.propagate", json!({"layer": id, "direction": "both", "wait": true})).unwrap();
    let st = s.execute("roto.status", json!({"layer": id})).unwrap();
    assert_eq!(st["objects"], json!([{"id": 0, "strokes": 1}, {"id": 1, "strokes": 1}]), "{st}");

    // object 0 keeps the boundary colour (pink by default), object 1 is the first palette colour (blue-green)
    let (pink, cyan) = ([255, 64, 191], [60, 200, 255]);
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(move |_| EffectcraftApp::new(s));
    h.run_steps(3);
    h.state_mut().session.execute("layer.select", json!({"layers": [id]})).unwrap();
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "layer.openLayer", json!({"layer": id})).unwrap();
    h.run_steps(4);
    assert_eq!(h.state().ui.focused, PanelKind::Layer);
    let both = h.render().expect("the panel renders");
    if let Some(path) = std::env::var_os("ROTO_OBJECTS_SNAPSHOT") {
        both.save(path).unwrap();
    }
    // the bar offers to remove the current object's strokes, and says how many it has
    assert!(h.state().auto.find("layerPanel.roto.clearObject").is_some(), "the remove button is on screen");
    assert!(h.state().auto.find("layerPanel.roto.object").is_some_and(|e| e.label.contains("(1)")), "the object's stroke count is on the button");
    let (a, b) = (count(&both, pink), count(&both, cyan));
    assert!(a > 50 && b > 50, "both objects are outlined: {a} pink, {b} cyan pixels");

    // Show Object 2 (object id 1): only its outline remains
    h.state_mut().session.execute("prop.set", json!({"layer": id, "path": "effects/#1/rotoBrushMatte/object", "value": 2})).unwrap();
    h.run_steps(4);
    let one = h.render().expect("the panel renders");
    let (a, b) = (count(&one, pink), count(&one, cyan));
    assert!(a < 10 && b > 50, "only object 1 is shown: {a} pink, {b} cyan pixels");
}
