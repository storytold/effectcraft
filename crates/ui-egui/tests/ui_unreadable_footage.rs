//! Footage whose codec the decoders lack says so: a warning marker in the Project panel and a
//! notice on the layer in the Composition viewer, instead of a black picture with no reason.

use std::sync::Arc;

use effectcraft_engine::Session;
use effectcraft_engine::project::{Footage, FootageKind, ItemId, ItemKind};
use effectcraft_engine::render::FootageSource;
use effectcraft_render::Image;
use effectcraft_time::Tick;
use effectcraft_ui_egui::EffectcraftApp;
use egui_kittest::Harness;
use serde_json::json;

const WHY: &str = "H.264 (High 4:2:2) isn't supported by the built-in decoder: chroma_format_idc 2 (only 4:2:0 is implemented)";

/// A source that decodes nothing and, for `broken` movies, says why.
struct Stub(bool);

impl FootageSource for Stub {
    fn frame(&self, _: ItemId, _: &Footage, _: Tick) -> Option<Arc<Image>> {
        None
    }
    fn problem(&self, _: &Footage) -> Option<String> {
        self.0.then(|| WHY.to_string())
    }
}

fn harness(broken: bool) -> (Harness<'static, EffectcraftApp>, ItemId) {
    let mut s = Session { footage: Arc::new(Stub(broken)), ..Default::default() };
    s.execute("comp.new", json!({"name": "Main", "width": 320, "height": 180, "duration": 4})).unwrap();
    let footage = Footage {
        kind: FootageKind::Video,
        path: "clip.mp4".into(),
        width: 64,
        height: 36,
        has_video: true,
        duration: Tick::from_seconds_f64(2.0),
        ..Default::default()
    };
    let clip = std::sync::Arc::make_mut(&mut s.project).add_item("Clip", Default::default(), None, ItemKind::Footage(footage));
    s.execute("layer.addItem", json!({"item": clip.0})).unwrap();
    s.execute("project.select", json!({"items": []})).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.run_steps(4);
    (h, clip)
}

#[test]
fn undecodable_footage_is_marked_in_the_project_panel_and_the_viewer() {
    let (h, clip) = harness(true);
    let marker = h.state().auto.find(&format!("project.item.{}.problem", clip.0)).expect("a warning on the footage item").clone();
    assert!(marker.label.contains("4:2:2"), "{marker:?}");
    let notice = h.state().auto.query("viewer.footageProblem.").first().copied().expect("a notice in the viewer").clone();
    assert!(notice.label.contains(WHY), "{notice:?}");
}

#[test]
fn readable_footage_has_no_notice() {
    let (h, clip) = harness(false);
    assert!(h.state().auto.find(&format!("project.item.{}.problem", clip.0)).is_none());
    assert!(h.state().auto.query("viewer.footageProblem.").is_empty());
}
