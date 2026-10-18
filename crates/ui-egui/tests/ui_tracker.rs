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

fn click(h: &mut Harness<'_, EffectcraftApp>, at: egui::Pos2) {
    h.input_mut().events.push(egui::Event::PointerMoved(at));
    h.step();
    h.input_mut().events.push(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
    h.step();
    h.input_mut().events.push(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
    h.run_steps(2);
}

/// Track point 1 of the current track: its centre and its attach point (layer pixels).
fn point(h: &mut Harness<'_, EffectcraftApp>) -> ([f64; 2], [f64; 2]) {
    let st = h.state_mut().session.execute("track.status", json!({})).unwrap();
    let p = &st["tracker"]["points"][0];
    let v = |k: &str| [p[k][0].as_f64().unwrap(), p[k][1].as_f64().unwrap()];
    let (c, off) = (v("center"), v("attachOffset"));
    (c, [c[0] + off[0], c[1] + off[1]])
}

fn plate_position(h: &Harness<'_, EffectcraftApp>) -> Vec<f64> {
    let c = h.state().session.active_comp().unwrap();
    c.layers.iter().find(|l| l.name == "Plate").unwrap().props.prop("transform/position").unwrap().value.components()
}

/// A press on a track point in the viewer gives it the arrow keys: they move its feature region,
/// search region and attach point by 1 screen pixel (Shift: 10), and with Alt the two regions
/// while the attach point stays. A click away gives the arrows back to the layers.
#[test]
fn arrow_keys_move_the_track_point_pressed_in_the_viewer() {
    use egui::{Key, Modifiers};
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| app());
    let s = &mut h.state_mut().session;
    s.execute("track.motion", json!({})).unwrap();
    s.execute("track.setPoint", json!({"point": 1, "featureSize": [60, 60], "searchSize": [120, 120]})).unwrap();
    h.run_steps(3);
    let px = 640.0 / rect(&h, "viewer.comp").width() as f64;
    let near = |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).abs() < 1e-6 && (a[1] - b[1]).abs() < 1e-6;

    // No point pressed yet: the arrows nudge the selected layer.
    let (home, p0) = (plate_position(&h), point(&mut h));
    assert!(h.state().auto.find("viewer.track.1.keyboard").is_none());
    h.key_press(Key::ArrowRight);
    h.run_steps(2);
    assert!((plate_position(&h)[0] - home[0] - px).abs() < 1e-6, "{:?}", plate_position(&h));
    assert_eq!(point(&mut h), p0);
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    h.run_steps(2);

    // Press inside the feature region, off the attach point's crosshair.
    let at = rect(&h, "viewer.track.1.feature").center() + egui::vec2(15.0, 0.0);
    click(&mut h, at);
    assert!(h.state().auto.find("viewer.track.1.keyboard").is_some(), "the point has the keyboard");
    let (c0, a0) = point(&mut h);
    h.key_press(Key::ArrowRight);
    h.run_steps(2);
    let (c, a) = point(&mut h);
    assert!(near(c, [c0[0] + px, c0[1]]) && near(a, [a0[0] + px, a0[1]]), "{c:?} {a:?} from {c0:?} {a0:?} ({px} px)");
    h.key_press_modifiers(Modifiers::SHIFT, Key::ArrowUp);
    h.run_steps(2);
    let (c, a) = point(&mut h);
    assert!(near(c, [c0[0] + px, c0[1] - 10.0 * px]) && near(a, [a0[0] + px, a0[1] - 10.0 * px]), "{c:?} {a:?}");
    // Alt: the regions move, the attach point stays.
    let (c1, a1) = (c, a);
    h.key_press_modifiers(Modifiers::ALT, Key::ArrowLeft);
    h.run_steps(2);
    h.key_press_modifiers(Modifiers::ALT | Modifiers::SHIFT, Key::ArrowDown);
    h.run_steps(2);
    let (c, a) = point(&mut h);
    assert!(near(c, [c1[0] - px, c1[1] + 10.0 * px]) && near(a, a1), "{c:?} {a:?} from {c1:?} {a1:?}");
    assert_eq!(plate_position(&h), home, "the layer stayed");

    // A click away from the point: the arrows are the layers' again.
    let away = rect(&h, "viewer.track.1.search").right_bottom() + egui::vec2(40.0, 40.0);
    click(&mut h, away);
    // (One more frame: the widgets are listed before the press is read.)
    h.step();
    assert!(h.state().auto.find("viewer.track.1.keyboard").is_none());
    let before = point(&mut h);
    h.key_press(Key::ArrowRight);
    h.run_steps(2);
    assert_eq!(point(&mut h), before);
    assert!((plate_position(&h)[0] - home[0] - px).abs() < 1e-6);

    // Agents: `track.selectPoint`.
    let ctx = h.ctx.clone();
    let r = effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "track.selectPoint", json!({"point": 1})).unwrap();
    assert_eq!(r["point"], 1);
    assert!(effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "track.selectPoint", json!({"point": 9})).is_err());
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "track.nudgeRegions.up10", json!({})).unwrap();
    assert!(near(point(&mut h).0, [before.0[0], before.0[1] - 10.0 * px]));
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "track.selectPoint", json!({})).unwrap();
    assert_eq!(h.state().ui.viewer.track_point, None);
}
