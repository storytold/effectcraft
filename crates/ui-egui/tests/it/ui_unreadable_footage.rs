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

/// A precomp whose proxy can't be decoded: the layer says so with Collapse Transformations off
/// or on, since a proxy in use keeps the precomp on the flattened path that draws the proxy.
#[test]
fn a_precomp_with_an_undecodable_proxy_has_a_notice_collapsed_or_not() {
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
        assert!(!h.state().auto.query("viewer.footageProblem.").is_empty(), "collapse {collapse}");
    }
}

#[test]
fn missing_footage_marks_project_timeline_and_viewer_and_clears_after_recovery() {
    let (mut h, clip) = harness(|_| false, None);
    let set_missing = |h: &mut Harness<'static, EffectcraftApp>, missing| {
        let item = Arc::make_mut(&mut h.state_mut().session.project).item_mut(clip).unwrap();
        let ItemKind::Footage(f) = &mut item.kind else { panic!("footage fixture") };
        f.missing = missing;
    };
    set_missing(&mut h, true);
    h.run_steps(2);
    assert!(h.state().auto.find(&format!("project.item.{}.problem", clip.0)).unwrap().label.contains("Missing footage"));
    assert_eq!(h.state().auto.query("timeline.layer.").iter().filter(|e| e.id.ends_with(".problem")).count(), 1);
    assert!(!h.state().auto.query("viewer.footageProblem.").is_empty());
    set_missing(&mut h, false);
    h.run_steps(2);
    assert!(h.state().auto.find(&format!("project.item.{}.problem", clip.0)).is_none());
    assert!(h.state().auto.query("timeline.layer.").iter().all(|e| !e.id.ends_with(".problem")));
    assert!(h.state().auto.query("viewer.footageProblem.").is_empty());
}

#[test]
fn missing_original_with_working_proxy_warns_inventory_without_a_viewer_failure() {
    let proxy = Footage { path: "proxy.png".into(), has_video: true, width: 32, height: 18, ..Default::default() };
    let (mut h, clip) = harness(|_| false, Some(proxy));
    if let ItemKind::Footage(f) = &mut Arc::make_mut(&mut h.state_mut().session.project).item_mut(clip).unwrap().kind {
        f.missing = true;
    }
    h.run_steps(2);
    assert!(h.state().auto.find(&format!("project.item.{}.problem", clip.0)).unwrap().label.contains("clip.mp4"));
    assert!(h.state().auto.query("viewer.footageProblem.").is_empty(), "the active proxy still supplies the preview");
}

#[test]
fn missing_active_proxy_explains_the_proxy_repair_action() {
    let proxy = Footage { path: "lost_proxy.png".into(), missing: true, has_video: true, width: 32, height: 18, ..Default::default() };
    let (h, clip) = harness(|_| false, Some(proxy));
    let marker = h.state().auto.find(&format!("project.item.{}.problem", clip.0)).unwrap();
    assert!(marker.label.starts_with("Proxy:") && marker.label.contains("Set Proxy"));
    assert!(h.state().auto.query("viewer.footageProblem.").iter().any(|e| e.label.contains("lost_proxy.png")));
}

#[test]
fn restored_original_and_active_proxy_render_new_pixels_through_media_and_disk_caches() {
    use effectcraft_engine::render::RenderOpts;
    for proxy in [false, true] {
        let dir = std::env::temp_dir().join(format!("ec-offline-pixels-{}-{proxy}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let original = dir.join("original.png");
        image::RgbaImage::from_pixel(16, 16, image::Rgba([255, 0, 0, 255])).save(&original).unwrap();
        let mut s = effectcraft_host::session();
        s.prefs.import.auto_reload_footage = "off".into();
        let item = s.execute("file.import", json!({"paths": [original], "sequence": false})).unwrap()["items"][0].as_u64().unwrap();
        let active = if proxy {
            let path = dir.join("proxy.png");
            image::RgbaImage::from_pixel(16, 16, image::Rgba([0, 255, 0, 255])).save(&path).unwrap();
            s.execute("file.setProxy", json!({"item": item, "path": path})).unwrap();
            path
        } else {
            original
        };
        let inner = s.execute("comp.new", json!({"name": "Media", "width": 16, "height": 16, "duration": 1})).unwrap()["comp"].as_u64().unwrap();
        s.execute("layer.addItem", json!({"item": item})).unwrap();
        // A zero-echo identity effect reads the real layer-input cache, including for proxies.
        s.execute("effect.apply", json!({"layer": "#1", "effect": "Echo"})).unwrap();
        s.execute("prop.set", json!({"layer": "#1", "path": "effects/#1/numberOfEchoes", "value": 0})).unwrap();
        s.execute("prop.set", json!({"layer": "#1", "path": "effects/#1/echoOperator", "value": 4})).unwrap();
        s.execute("comp.new", json!({"name": "Output", "width": 16, "height": 16, "duration": 1})).unwrap();
        s.execute("layer.addItem", json!({"item": inner})).unwrap();
        let comp = s.active_comp_id().unwrap();
        let cache = effectcraft_render::disk_cache::DiskCache::open(dir.join("cache"), 1 << 20).unwrap();
        cache.set_min_layer_ms(0);
        s.disk_cache = Some(cache.clone());
        s.layer_cache.set_disk(Some(cache.clone()));
        let before = s.render(comp, Tick::ZERO, RenderOpts::default());
        assert_eq!(before.data[8 * 16 + 8], if proxy { [0.0, 1.0, 0.0, 1.0] } else { [1.0, 0.0, 0.0, 1.0] });
        cache.flush();
        assert!(cache.stats().entries > 0, "the first render populated the disk cache");
        s.layer_cache.clear();
        let cached = s.render(comp, Tick::ZERO, RenderOpts::default());
        assert_eq!(cached.data, before.data);
        assert!(cache.stats().hits > 0, "the same media was read back through the disk cache");
        std::fs::remove_file(&active).unwrap();
        assert_eq!(s.execute("footage.reloadChanged", json!({})).unwrap()["reloaded"], 0, "availability alone drives recovery in this fixture");
        let offline = s.render(comp, Tick::ZERO, RenderOpts::default());
        assert_eq!(offline.data[8 * 16 + 8], [0.0, 0.0, 0.0, 0.0]);
        image::RgbaImage::from_pixel(16, 16, image::Rgba([0, 0, 255, 255])).save(&active).unwrap();
        assert_eq!(s.execute("footage.reloadChanged", json!({})).unwrap()["reloaded"], 0, "availability alone drives recovery in this fixture");
        let restored = s.render(comp, Tick::ZERO, RenderOpts::default());
        assert_eq!(restored.data[8 * 16 + 8], [0.0, 0.0, 1.0, 1.0], "restored pixels replace decoded and composited cached frames");
        cache.flush();
        drop(cache);
        drop(s);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
