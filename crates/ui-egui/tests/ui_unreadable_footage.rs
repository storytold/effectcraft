//! Footage (or the proxy in use for it) whose codec the decoders lack says so: a warning marker in
//! the Project panel and a notice on the layer in the Composition viewer, instead of a black
//! picture with no reason.

use std::sync::Arc;

use effectcraft_engine::Session;
use effectcraft_engine::project::{Footage, FootageKind, ItemId, ItemKind, Proxy};
use effectcraft_engine::render::FootageSource;
use effectcraft_render::Image;
use effectcraft_time::Tick;
use effectcraft_ui_egui::EffectcraftApp;
use egui_kittest::Harness;
use serde_json::json;

const WHY: &str = "H.264 (High 4:2:2) isn't supported by the built-in decoder: chroma_format_idc 2 (only 4:2:0 is implemented)";

/// A source that decodes nothing and says why for the files `broken` picks.
struct Stub(fn(&Footage) -> bool);

impl FootageSource for Stub {
    fn frame(&self, _: ItemId, _: &Footage, _: Tick) -> Option<Arc<Image>> {
        None
    }
    fn problem(&self, f: &Footage) -> Option<String> {
        (self.0)(f).then(|| WHY.to_string())
    }
}

fn harness(broken: fn(&Footage) -> bool, proxy: Option<Footage>) -> (Harness<'static, EffectcraftApp>, ItemId) {
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
    let p = std::sync::Arc::make_mut(&mut s.project);
    let clip = p.add_item("Clip", Default::default(), None, ItemKind::Footage(footage));
    if let (Some(item), Some(footage)) = (p.items.get_mut(&clip), proxy) {
        item.proxy = Some(Box::new(Proxy { footage, enabled: true }));
    }
    s.execute("layer.addItem", json!({"item": clip.0})).unwrap();
    (open(s), clip)
}

fn open(mut s: Session) -> Harness<'static, EffectcraftApp> {
    s.execute("project.select", json!({"items": []})).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.run_steps(4);
    h
}

#[test]
fn undecodable_footage_is_marked_in_the_project_panel_and_the_viewer() {
    let (h, clip) = harness(|_| true, None);
    let marker = h.state().auto.find(&format!("project.item.{}.problem", clip.0)).expect("a warning on the footage item").clone();
    assert!(marker.label.contains("4:2:2"), "{marker:?}");
    let notice = h.state().auto.query("viewer.footageProblem.").first().copied().expect("a notice in the viewer").clone();
    assert!(notice.label.contains(WHY), "{notice:?}");
}

#[test]
fn readable_footage_has_no_notice() {
    let (h, clip) = harness(|_| false, None);
    assert!(h.state().auto.find(&format!("project.item.{}.problem", clip.0)).is_none());
    assert!(h.state().auto.query("viewer.footageProblem.").is_empty());
}

/// The clip's own file is fine, but the proxy in use for it can't be decoded: the item and the
/// layer say why, and that it is the proxy.
#[test]
fn an_undecodable_proxy_in_use_is_marked_as_the_proxy() {
    let proxy = Footage { kind: FootageKind::Video, path: "clip_proxy.avi".into(), width: 32, height: 18, has_video: true, ..Default::default() };
    let (h, clip) = harness(|f| f.path.ends_with(".avi"), Some(proxy));
    let marker = h.state().auto.find(&format!("project.item.{}.problem", clip.0)).expect("a warning on the footage item").clone();
    assert!(marker.label.starts_with("Proxy: ") && marker.label.contains("4:2:2"), "{marker:?}");
    let notice = h.state().auto.query("viewer.footageProblem.").first().copied().expect("a notice in the viewer").clone();
    assert!(notice.label.contains(&format!("Proxy: {WHY}")), "{notice:?}");
}

/// A precomp whose proxy can't be decoded: the layer says so while it renders through the
/// proxy, but not with Collapse Transformations on, which draws the precomp's own layers.
#[test]
fn a_collapsed_precomp_has_no_notice_for_its_proxy() {
    for collapse in [false, true] {
        let mut s = Session { footage: Arc::new(Stub(|f| f.path.ends_with(".avi"))), ..Default::default() };
        s.execute("comp.new", json!({"name": "Inner", "width": 64, "height": 36, "duration": 4})).unwrap();
        s.execute("comp.new", json!({"name": "Main", "width": 320, "height": 180, "duration": 4})).unwrap();
        let p = Arc::make_mut(&mut s.project);
        let inner = p.items.values_mut().find(|i| i.name == "Inner").unwrap();
        let proxy = Footage { kind: FootageKind::Video, path: "inner_proxy.avi".into(), width: 64, height: 36, has_video: true, ..Default::default() };
        inner.proxy = Some(Box::new(Proxy { footage: proxy, enabled: true }));
        let inner = inner.id;
        s.execute("layer.addItem", json!({"item": inner.0})).unwrap();
        s.execute("layer.setSwitch", json!({"layers": ["#1"], "switch": "collapse", "value": collapse})).unwrap();
        let h = open(s);
        assert_eq!(h.state().auto.query("viewer.footageProblem.").is_empty(), collapse, "collapse {collapse}");
    }
}
