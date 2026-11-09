//! Headless checks for the Tracker panel and the viewer's track point widgets (egui_kittest, UI
//! logic only: no GPU needed).

use effectcraft_engine::Session;
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::dock::PanelKind;
use egui_kittest::Harness;
use serde_json::json;

fn app() -> EffectcraftApp {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Track", "width": 640, "height": 360, "duration": 2})).unwrap();
    s.execute("layer.newSolid", json!({"name": "Plate", "color": "#406080"})).unwrap();
    s.execute("layer.newSolid", json!({"name": "Target", "width": 80, "height": 80})).unwrap();
    s.execute("layer.select", json!({"layers": ["Plate"]})).unwrap();
    EffectcraftApp::new(s)
}

fn ids(h: &Harness<'_, EffectcraftApp>) -> Vec<String> {
    h.state().auto.previous.iter().chain(h.state().auto.elements.iter()).map(|e| e.id.clone()).collect()
}

fn rect(h: &Harness<'_, EffectcraftApp>, id: &str) -> egui::Rect {
    let e = h.state().auto.find(id).unwrap_or_else(|| panic!("no {id}"));
    egui::Rect::from_min_size(egui::pos2(e.rect[0], e.rect[1]), egui::vec2(e.rect[2], e.rect[3]))
}

#[test]
fn tracker_panel_and_track_point_widgets() {
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| app());
    h.state_mut().show_panel(PanelKind::Tracker);
    h.run_steps(3);
    let all = ids(&h);
    for want in [
        "tracker.trackCamera",
        "tracker.warpStabilizer",
        "tracker.trackMotion",
        "tracker.stabilizeMotion",
        "tracker.motionSource",
        "tracker.currentTrack",
        "tracker.trackType",
        "tracker.position",
        "tracker.rotation",
        "tracker.scale",
        "tracker.motionTarget",
        "tracker.editTarget",
        "tracker.options",
        "tracker.analyze.frameBackward",
        "tracker.analyze.backward",
        "tracker.analyze.forward",
        "tracker.analyze.frameForward",
        "tracker.reset",
        "tracker.apply",
    ] {
        assert!(all.iter().any(|i| i == want), "missing {want}");
    }
    // Track Motion from the panel.
    let r = rect(&h, "tracker.trackMotion");
    h.input_mut().events.push(egui::Event::PointerMoved(r.center()));
    h.input_mut().events.push(egui::Event::PointerButton {
        pos: r.center(),
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: Default::default(),
    });
    h.input_mut().events.push(egui::Event::PointerButton {
        pos: r.center(),
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: Default::default(),
    });
    h.run_steps(3);
    let st = h.state_mut().session.execute("track.status", json!({})).unwrap();
    assert_eq!(st["tracker"]["name"], "Tracker 1", "{st}");
    h.state_mut().session.execute("track.setPoint", json!({"point": 1, "featureSize": [60, 60], "searchSize": [120, 120]})).unwrap();
    h.state_mut().show_panel(PanelKind::Composition);
    h.run_steps(3);
    let all = ids(&h);
    for want in ["viewer.track.1.feature", "viewer.track.1.search", "viewer.track.1.attach", "viewer.track.1.feature.0", "viewer.track.1.search.2"] {
        assert!(all.iter().any(|i| i == want), "missing {want}");
    }
    // Drag the feature region: the point moves by the drag in layer pixels (one undo step).
    let c0 = st["tracker"]["points"][0]["center"].clone();
    let fr = rect(&h, "viewer.track.1.feature");
    let zoom = {
        let a = rect(&h, "viewer.comp");
        a.width() / 640.0
    };
    // Inside the feature region, away from the attach point crosshair (which sits on the centre).
    let from = fr.center() + egui::vec2(15.0, 0.0);
    let to = from + egui::vec2(40.0, 20.0);
    h.input_mut().events.push(egui::Event::PointerMoved(from));
    h.input_mut().events.push(egui::Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
    h.step();
    for i in 1..=8 {
        h.input_mut().events.push(egui::Event::PointerMoved(from + (to - from) * (i as f32 / 8.0)));
        h.step();
    }
    h.input_mut().events.push(egui::Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
    h.run_steps(2);
    let st = h.state_mut().session.execute("track.status", json!({})).unwrap();
    let c1 = &st["tracker"]["points"][0]["center"];
    let dx = c1[0].as_f64().unwrap() - c0[0].as_f64().unwrap();
    let dy = c1[1].as_f64().unwrap() - c0[1].as_f64().unwrap();
    assert!((dx - 40.0 / zoom as f64).abs() < 1.0 && (dy - 20.0 / zoom as f64).abs() < 1.0, "moved by ({dx}, {dy}) at zoom {zoom}");
    let undo = h.state().session.history.undo.iter().filter(|(l, _)| l == "Move Track Point").count();
    assert_eq!(undo, 2, "one undo step per drag");
    // Options and Edit Target dialogs.
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "track.optionsDialog", json!({})).unwrap();
    assert_eq!(h.state().dialog, Some(effectcraft_ui_egui::Dialog::TrackOptions));
    h.step();
    assert!(ids(&h).iter().any(|i| i == "dialog.trackOptions.ok"));
    h.state_mut().dialog = None;
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "track.editTargetDialog", json!({})).unwrap();
    assert_eq!(h.state().dialog, Some(effectcraft_ui_egui::Dialog::TrackTarget));
}

fn drag(h: &mut Harness<'_, EffectcraftApp>, from: egui::Pos2, to: egui::Pos2) {
    h.input_mut().events.push(egui::Event::PointerMoved(from));
    h.input_mut().events.push(egui::Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
    h.step();
    for i in 1..=8 {
        h.input_mut().events.push(egui::Event::PointerMoved(from + (to - from) * (i as f32 / 8.0)));
        h.step();
    }
    h.input_mut().events.push(egui::Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
    h.run_steps(2);
}

/// #1067: zoomed out, a default track point's feature region is smaller on screen than the attach
/// point's grab area, which used to swallow every press inside it: the point itself could not be
/// grabbed. Inside the region off its centre moves the point at any zoom; the centre still grabs the
/// attach point.
#[test]
fn zoomed_out_track_point_can_be_grabbed() {
    for mag in [0.25f32, 0.5, 1.0] {
        let mut s = Session::default();
        s.execute("comp.new", json!({"name": "Track", "width": 1920, "height": 1080, "duration": 2})).unwrap();
        s.execute("layer.newSolid", json!({"name": "Plate", "color": "#406080"})).unwrap();
        s.execute("layer.select", json!({"layers": ["Plate"]})).unwrap();
        s.execute("track.motion", json!({})).unwrap();
        let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(move |_| EffectcraftApp::new(s));
        h.state_mut().show_panel(PanelKind::Composition);
        h.run_steps(3);
        h.state_mut().ui.viewer.zoom = Some(mag);
        h.run_steps(3);
        let zoom = rect(&h, "viewer.comp").width() / 1920.0;
        let point = |h: &mut Harness<'_, EffectcraftApp>| h.state_mut().session.execute("track.status", json!({})).unwrap()["tracker"]["points"][0].clone();
        let p0 = point(&mut h);
        let fr = rect(&h, "viewer.track.1.feature");
        let from = fr.center() + egui::vec2(fr.width() * 0.35, 0.0);
        drag(&mut h, from, from + egui::vec2(40.0, 20.0));
        let p1 = point(&mut h);
        let dx = p1["center"][0].as_f64().unwrap() - p0["center"][0].as_f64().unwrap();
        let dy = p1["center"][1].as_f64().unwrap() - p0["center"][1].as_f64().unwrap();
        assert!((dx - 40.0 / zoom as f64).abs() < 1.0 && (dy - 20.0 / zoom as f64).abs() < 1.0, "{mag}×: the point moved by ({dx}, {dy}), {p1}");
        assert_eq!(p1["attachOffset"], p0["attachOffset"], "{mag}×: the attach point moves with the point");
        let fr = rect(&h, "viewer.track.1.feature");
        drag(&mut h, fr.center(), fr.center() + egui::vec2(40.0, 20.0));
        let p2 = point(&mut h);
        assert_eq!(p2["center"], p1["center"], "{mag}×: the centre grabs the attach point, not the point");
        assert_ne!(p2["attachOffset"], p1["attachOffset"], "{mag}×: the centre grabs the attach point");
        // Dragged out of the region, the attach point keeps its full grab area.
        assert_eq!(rect(&h, "viewer.track.1.attach").width(), 12.0, "{mag}×");
    }
}
