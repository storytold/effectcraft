//! Graph Editor ▸ Allow Keyframes Between Frames (After Effects' Graph Editor option): off (the
//! default), keys dragged in the Graph Editor move by whole frames and keys scaled with its
//! transform box or by an Alt-drag in the Timeline land on frames; the bar's Between Frames button
//! turns it on, and the same gestures keep the keys at the exact time they were moved or scaled to.

use effectcraft_engine::Session;
use effectcraft_engine::project::LayerId;
use effectcraft_ui_egui::EffectcraftApp;
use egui::{Event, Modifiers, Pos2, pos2, vec2};
use egui_kittest::Harness;
use serde_json::json;

/// One frame at 30 fps, in ticks.
const F: i64 = 8_467_200_000;

/// A 4 s, 30 fps comp with a Box layer whose Opacity has keys at `seconds`, revealed in a Timeline
/// wide enough for the whole Graph Editor button bar; returns the harness, the layer and the
/// property uid.
fn harness(seconds: &[f64]) -> (Harness<'static, EffectcraftApp>, LayerId, u64) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Keys", "width": 320, "height": 180, "frameRate": 30, "duration": 4})).unwrap();
    let id = LayerId(s.execute("layer.newSolid", json!({"name": "Box", "color": "#e04020", "width": 80, "height": 80})).unwrap()["layer"].as_u64().unwrap());
    for (i, t) in seconds.iter().enumerate() {
        s.execute("prop.addKey", json!({"layer": id.0, "path": "transform/opacity", "time": t, "value": i as f64 * 50.0})).unwrap();
    }
    let uid = s.active_comp().unwrap().layer(id).unwrap().props.prop("transform/opacity").unwrap().uid;
    s.execute("layer.select", json!({"layers": [id.0]})).unwrap();
    let mut h = Harness::builder().with_size(vec2(2800.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.run_steps(3);
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "timeline.reveal.opacity", json!({})).unwrap();
    h.run_steps(4);
    (h, id, uid)
}

/// The stored Opacity key times (layer ticks).
fn ticks(h: &Harness<'_, EffectcraftApp>, id: LayerId) -> Vec<i64> {
    h.state().session.active_comp().unwrap().layer(id).unwrap().props.prop("transform/opacity").unwrap().keys.iter().map(|k| k.time.0).collect()
}

/// The centre of the automation element `id`.
fn center(h: &Harness<'_, EffectcraftApp>, id: &str) -> Pos2 {
    let e = h.state().auto.find(id).unwrap_or_else(|| panic!("no {id}"));
    pos2(e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0)
}

/// Press at `from`, move to `to` in small steps with `modifiers` held (a real drag), release.
fn drag(h: &mut Harness<'_, EffectcraftApp>, from: Pos2, to: Pos2, modifiers: Modifiers) {
    h.input_mut().events.push(Event::PointerMoved(from));
    h.step();
    h.input_mut().events.push(Event::ModifiersChanged(modifiers));
    h.input_mut().events.push(Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers });
    h.step();
    for i in 1..=24 {
        h.input_mut().events.push(Event::PointerMoved(from + (to - from) * (i as f32 / 24.0)));
        h.step();
    }
    h.input_mut().events.push(Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers });
    h.step();
    h.input_mut().events.push(Event::ModifiersChanged(Modifiers::default()));
    h.run_steps(2);
}

/// Click the Graph Editor bar's Between Frames button (it must be on the bar, not in "»").
fn toggle_between_frames(h: &mut Harness<'_, EffectcraftApp>) {
    let p = center(h, "timeline.graph.betweenFrames");
    h.input_mut().events.push(Event::PointerMoved(p));
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::default() });
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::default() });
    h.run_steps(3);
}

#[test]
fn graph_editor_key_drags_land_between_frames_only_when_allowed() {
    let (mut h, id, uid) = harness(&[0.0, 1.0]);
    // Opened before selecting, so it fits all graphs (the whole comp) rather than the two keys.
    h.state_mut().ui.timeline.graph_editor = true;
    h.run_steps(2);
    assert!(!h.state().ui.timeline.graph_between_frames, "off by default");
    let s = &mut h.state_mut().session;
    s.execute("prop.select", json!({"layer": id.0, "prop": uid})).unwrap();
    s.execute("keys.select", json!({"keys": [{"layer": id.0, "prop": uid, "time": 1.0}]})).unwrap();
    h.run_steps(4);
    let key = |h: &Harness<'_, EffectcraftApp>, i: usize| center(h, &format!("timeline.graph.key.{uid}.0.{i}"));
    let (k0, k1) = (key(&h, 0), key(&h, 1));
    let px_per_frame = (k1.x - k0.x) / 30.0;
    assert!(px_per_frame > 5.0, "frames are wide enough to drag a fraction of one: {px_per_frame}");
    // 0.6 frame later, Shift keeping the time axis: rounded to frame 31.
    let shift = Modifiers { shift: true, ..Default::default() };
    drag(&mut h, k1, k1 + vec2(px_per_frame * 0.6, 0.0), shift);
    assert_eq!(ticks(&h, id), vec![0, 31 * F], "on a frame");
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    h.run_steps(2);
    assert_eq!(ticks(&h, id), vec![0, 30 * F]);

    toggle_between_frames(&mut h);
    assert!(h.state().ui.timeline.graph_between_frames, "the button turns it on");
    let k1 = key(&h, 1);
    drag(&mut h, k1, k1 + vec2(px_per_frame * 0.6, 0.0), shift);
    let t = ticks(&h, id);
    let frames = t[1] as f64 / F as f64;
    assert!((frames - 30.6).abs() < 0.05, "0.6 frame later, between frames: {frames}");
    assert_eq!(t[0], 0, "the unselected key stays");
    let undo = h.state().session.history.undo.iter().filter(|(l, _)| l == "Transform Keyframes").count();
    assert_eq!(undo, 1, "one undo step per drag");

    toggle_between_frames(&mut h);
    assert!(!h.state().ui.timeline.graph_between_frames, "and off again");
}

#[test]
fn timeline_alt_drag_scaling_puts_the_inner_keys_between_frames_only_when_allowed() {
    let one = 30 * F;
    let run = |between: bool| {
        let (mut h, id, uid) = harness(&[0.0, 1.0, 2.0]);
        h.state_mut().ui.timeline.graph_between_frames = between;
        let keys: Vec<_> = [0.0, 1.0, 2.0].iter().map(|t| json!({"layer": id.0, "prop": uid, "time": t})).collect();
        h.state_mut().session.execute("keys.select", json!({"keys": keys})).unwrap();
        h.run_steps(2);
        let k0 = center(&h, &format!("timeline.keyAt.{uid}.0"));
        let last = center(&h, &format!("timeline.keyAt.{uid}.{}", 2 * one));
        let px_per_frame = (last.x - k0.x) / 60.0;
        // The last key from frame 60 to frame 61, about the first key: the middle key goes to
        // frame 30.5.
        drag(&mut h, last, last + vec2(px_per_frame, 0.0), Modifiers { alt: true, ..Default::default() });
        ticks(&h, id)
    };
    assert_eq!(run(false), vec![0, 30 * F, 61 * F], "rounded to frames (30.5 goes down to 30)");
    assert_eq!(run(true), vec![0, 30 * F + F / 2, 61 * F], "the middle key at frame 30.5, the dragged key on frame 61");
}

/// Opacity keys at 0, 1 and 2 s, all selected, in the Graph Editor (fitted to the whole comp);
/// returns the harness, the layer, the property uid and the points per second.
fn graph_with_three_keys(between: bool) -> (Harness<'static, EffectcraftApp>, LayerId, u64, f32) {
    let (mut h, id, uid) = harness(&[0.0, 1.0, 2.0]);
    h.state_mut().ui.timeline.graph_editor = true;
    h.state_mut().ui.timeline.graph_between_frames = between;
    h.run_steps(2);
    let s = &mut h.state_mut().session;
    s.execute("prop.select", json!({"layer": id.0, "prop": uid})).unwrap();
    let keys: Vec<_> = [0.0, 1.0, 2.0].iter().map(|t| json!({"layer": id.0, "prop": uid, "time": t})).collect();
    s.execute("keys.select", json!({"keys": keys})).unwrap();
    h.run_steps(4);
    let key = |i: usize| center(&h, &format!("timeline.graph.key.{uid}.0.{i}"));
    let px_per_s = (key(2).x - key(0).x) / 2.0;
    (h, id, uid, px_per_s)
}

#[test]
fn transform_box_scaling_puts_keys_between_frames_only_when_allowed() {
    let run = |between: bool| {
        let (mut h, id, _, px_per_s) = graph_with_three_keys(between);
        // The right edge handle 0.41 s to the right: the keys scale by about 1.205 about the first.
        let r = center(&h, "timeline.graph.transformBox.5");
        drag(&mut h, r, r + vec2(px_per_s * 0.41, 0.0), Modifiers::default());
        let undo = h.state().session.history.undo.iter().filter(|(l, _)| l == "Transform Keyframes").count();
        assert_eq!(undo, 1, "one undo step per drag");
        (ticks(&h, id), px_per_s)
    };
    let (on_frames, px_per_s) = run(false);
    assert!(on_frames.iter().all(|t| t % F == 0), "on frames: {on_frames:?}");
    let (between, _) = run(true);
    assert_eq!(between[0], 0, "the first key is the anchor");
    assert!((2 * between[1] - between[2]).abs() <= 1, "the middle key stays halfway, to the tick: {between:?}");
    assert!(between[1] % F != 0 && between[2] % F != 0, "between frames: {between:?}");
    // The handle follows the pointer from where it was pressed (it sits a few points outside the
    // last key): the last key moved 0.41 s, give or take a point.
    let moved = (between[2] - 60 * F) as f64 / (30 * F) as f64;
    assert!((moved - 0.41).abs() < 1.0 / px_per_s as f64, "moved {moved} s");
    // With frames the same drag rounds the same keys to the nearest frames.
    assert_eq!(on_frames, between.iter().map(|t| (t + F / 2) / F * F).collect::<Vec<_>>());
}

#[test]
fn graph_editor_drags_keep_a_quick_offset_stagger() {
    // Opened before selecting, so it fits all graphs (the whole comp).
    let (mut h, a, uid) = harness(&[1.0, 2.0]);
    h.state_mut().ui.timeline.graph_editor = true;
    h.run_steps(2);
    // The keys at 1 s of three layers, staggered by a one-frame Quick Offset: frame 30, 30.5, 31.
    let s = &mut h.state_mut().session;
    let mut ids = vec![a];
    for name in ["B", "C"] {
        let id = LayerId(s.execute("layer.newSolid", json!({"name": name, "color": "#2060e0", "width": 80, "height": 80})).unwrap()["layer"].as_u64().unwrap());
        s.execute("prop.addKey", json!({"layer": id.0, "path": "transform/opacity", "time": 1.0, "value": 50})).unwrap();
        ids.push(id);
    }
    // The first layer's Opacity shows in the graph (selecting it clears the key selection).
    s.execute("prop.select", json!({"layer": a.0, "prop": uid})).unwrap();
    let keys: Vec<_> = ids.iter().map(|id| json!({"layer": id.0, "path": "transform/opacity", "time": 1.0})).collect();
    s.execute("keys.select", json!({"keys": keys.clone()})).unwrap();
    s.execute("keys.quickOffset", json!({"delta": 1.0 / 30.0})).unwrap();
    h.run_steps(4);
    let at = |h: &Harness<'_, EffectcraftApp>| ids.iter().map(|id| ticks(h, *id)[0]).collect::<Vec<_>>();
    assert_eq!(at(&h), vec![30 * F, 30 * F + F / 2, 31 * F]);
    // Its key at frame 30 drags all three, 2.3 frames later along the
    // time axis. Each moves two whole frames; the middle one stays half a frame from the others.
    let key = |h: &Harness<'_, EffectcraftApp>, i: usize| center(h, &format!("timeline.graph.key.{uid}.0.{i}"));
    let (k0, k1) = (key(&h, 0), key(&h, 1));
    let px_per_frame = (k1.x - k0.x) / 30.0;
    assert!(px_per_frame > 5.0, "{px_per_frame}");
    let shift = Modifiers { shift: true, ..Default::default() };
    drag(&mut h, k0, k0 + vec2(px_per_frame * 2.3, 0.0), shift);
    assert_eq!(at(&h), vec![32 * F, 32 * F + F / 2, 33 * F], "the stagger is kept");
    // Between frames: the exact move, still the same stagger.
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    h.state_mut().session.execute("keys.select", json!({"keys": keys})).unwrap();
    h.state_mut().ui.timeline.graph_between_frames = true;
    h.run_steps(2);
    let k0 = key(&h, 0);
    drag(&mut h, k0, k0 + vec2(px_per_frame * 2.3, 0.0), shift);
    let t = at(&h);
    assert!(((t[0] - 30 * F) as f64 / F as f64 - 2.3).abs() < 0.05, "{t:?}");
    assert_eq!(vec![t[1] - t[0], t[2] - t[0]], vec![F / 2, F], "the same stagger, to the tick");
}
